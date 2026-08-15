//! Linux VA-API hardware H.264 encoder (PDF Task 2.2 hardware path).
//!
//! Implements the [`VideoEncoder`] boundary on top of libva (MIT-style,
//! https://01.org/linuxmedia/vaapi) with a dlopen-based client — the same
//! pattern as the PipeWire backend (`platform/linux.rs`): the build requires
//! NO VA development headers, only the runtime `libva.so.2` /
//! `libva-drm.so.2` libraries that any GPU-accelerated desktop already has.
//!
//! # Fallback contract
//!
//! [`VaapiEncoder::new`] fails with a typed
//! [`ScreenShareErrorKind::HardwareAccelerationUnavailable`] error whenever
//! the path cannot be used (no render node, no permission, no H.264 encode
//! entrypoint, driver init failure). The encoder factory in `codec.rs`
//! catches that error, logs it clearly, and falls back to the OpenH264
//! software encoder — the viewer never notices which backend produced the
//! baseline H.264 stream.
//!
//! # Encode flow
//!
//! One frame: RGBA8 → NV12 conversion in CPU, upload into a surface via
//! `vaCreateImage`/`vaPutImage`, then a `vaBeginPicture` →
//! `vaRenderPicture` (sequence + picture + slice + rate-control buffers) →
//! `vaEndPicture` → `vaSyncSurface` cycle; the coded bitstream is read back
//! from the `VAEncCodedBufferType` buffer. Keyframes (IDR) carry the
//! sequence/picture parameter sets; delta frames reference the previous
//! surface. Bitrate-only changes send a fresh rate-control misc buffer
//! without rebuilding the context (no config-generation bump), so
//! `AdaptiveQuality` keeps working on the hardware path.
//!
//! # Struct layouts
//!
//! The `#[repr(C)]` structs mirror `va.h` / `va_enc_h264.h` exactly; the
//! offsets were validated against the system headers with a C size probe
//! (see `vaapi_layouts_match_system_headers` test).
#![allow(missing_docs)]

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::fs::OpenOptions;
use std::os::fd::IntoRawFd;
use std::sync::Arc;

use libloading::Library;

use super::capture::{CapturedFrame, PixelFormat};
use super::codec::{
    now_micros, CodecConfig, CodecKind, CodecMetadata, EncodedPacket, QualityProfile, VideoEncoder,
};
use super::ScreenShareError;

// ─── VA-API constants (va.h / va_enc_h264.h) ───────────────────────────────

/// VAProfileH264ConstrainedBaseline (13) — the modern H.264 baseline profile
/// every Boru viewer decoder handles; i965 exposes `VAEntrypointEncSlice`
/// for it (verified on the build host).
const VA_PROFILE_H264_CONSTRAINED_BASELINE: c_int = 13;
/// Fallback: legacy VAProfileH264Baseline (5, deprecated alias).
const VA_PROFILE_H264_BASELINE: c_int = 5;
/// VAEntrypointEncSlice (6) — slice-level encode.
const VA_ENTRYPOINT_ENC_SLICE: c_int = 6;
/// VAEntrypointEncSliceLP (8) — low-power slice encode (some drivers only
/// expose this; accepted as an encode-capable entrypoint).
const VA_ENTRYPOINT_ENC_SLICE_LP: c_int = 8;

const VA_STATUS_SUCCESS: c_int = 0;
/// VA_RT_FORMAT_YUV420 (1) — 8-bit 4:2:0 surface format.
const VA_RT_FORMAT_YUV420: c_uint = 1;
/// VA_FOURCC_NV12 ('NV12' little-endian = 0x3231564e).
const VA_FOURCC_NV12: c_uint = 0x3231564e;
/// VA_INVALID_ID (0xffffffff) — "no id" sentinel.
const VA_INVALID_ID: u32 = 0xffff_ffff;
/// VA_PROGRESSIVE (1) — progressive (non-interlaced) context flag.
const VA_PROGRESSIVE: c_int = 0x1;

// VABufferType values.
const VA_ENC_CODED_BUFFER_TYPE: c_int = 21;
const VA_ENC_SEQUENCE_PARAMETER_BUFFER_TYPE: c_int = 22;
const VA_ENC_PICTURE_PARAMETER_BUFFER_TYPE: c_int = 23;
const VA_ENC_SLICE_PARAMETER_BUFFER_TYPE: c_int = 24;
const VA_ENC_MISC_PARAMETER_BUFFER_TYPE: c_int = 27;

// VAEncMiscParameterType values.
const VA_ENC_MISC_PARAMETER_TYPE_RATE_CONTROL: c_int = 1;
const VA_ENC_MISC_PARAMETER_TYPE_FRAME_RATE: c_int = 0;

// VAConfigAttribType values.
const VA_CONFIG_ATTRIB_RT_FORMAT: c_int = 0;
const VA_CONFIG_ATTRIB_RATE_CONTROL: c_int = 5;
// VA_RC_CBR (2) — constant bitrate, like the OpenH264 baseline.
const VA_RC_CBR: u32 = 0x0000_0002;

// VASurfaceAttribType values.
const VA_SURFACE_ATTRIB_PIXEL_FORMAT: c_int = 1;
// VASurfaceAttrib flags: settable.
const VA_SURFACE_ATTRIB_SETTABLE: u32 = 2;
// VAGenericValueTypeInteger (1).
const VA_GENERIC_VALUE_TYPE_INTEGER: c_int = 1;

/// Slice types (H.264 slice_type): 0 = P, 2 = I (no switching slices).
const SLICE_TYPE_P: u8 = 0;
const SLICE_TYPE_I: u8 = 2;

/// VAPictureH264 flags: short-term reference picture.
const VA_PICTURE_H264_SHORT_TERM_REFERENCE: u32 = 0x0000_0008;

/// Number of surfaces in the ping-pong reference pool. Baseline IPPP uses
/// one reference, so two surfaces (current + previous) are the minimum; the
/// third is a spare the driver may use for reconstruction.
const SURFACE_COUNT: usize = 3;

/// Coded buffer size: worst case one 4:2:0 frame × 3 (covers high-bitrate
/// keyframes at HD comfortably).
fn coded_buffer_size(config: &CodecConfig) -> usize {
    (config.width as usize * config.height as usize * 3 / 2).max(1 << 20)
}

// ─── FFI structs (mirror va.h / va_enc_h264.h; validated by the size test) ─

#[repr(C)]
#[derive(Clone, Copy)]
struct VAConfigAttrib {
    type_: c_int,
    value: u32,
}

#[repr(C)]
union GenericValueData {
    i: c_int,
    f: f32,
    p: *mut c_void,
}

#[repr(C)]
struct VAGenericValue {
    type_: c_int,
    value: GenericValueData,
}

#[repr(C)]
struct VASurfaceAttrib {
    type_: c_int,
    flags: u32,
    value: VAGenericValue,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VAPictureH264 {
    picture_id: u32,
    frame_idx: u32,
    flags: u32,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAEncSequenceParameterBufferH264 {
    seq_parameter_set_id: u8,
    level_idc: u8,
    intra_period: u32,
    intra_idr_period: u32,
    ip_period: u32,
    bits_per_second: u32,
    max_num_ref_frames: u32,
    picture_width_in_mbs: u16,
    picture_height_in_mbs: u16,
    seq_fields: u32,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    num_ref_frames_in_pic_order_cnt_cycle: u8,
    offset_for_non_ref_pic: i32,
    offset_for_top_to_bottom_field: i32,
    offset_for_ref_frame: [i32; 256],
    frame_cropping_flag: u8,
    frame_crop_left_offset: u32,
    frame_crop_right_offset: u32,
    frame_crop_top_offset: u32,
    frame_crop_bottom_offset: u32,
    vui_parameters_present_flag: u8,
    vui_fields: u32,
    aspect_ratio_idc: u8,
    sar_width: u32,
    sar_height: u32,
    num_units_in_tick: u32,
    time_scale: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAEncPictureParameterBufferH264 {
    curr_pic: VAPictureH264,
    reference_frames: [VAPictureH264; 16],
    coded_buf: u32,
    pic_parameter_set_id: u8,
    seq_parameter_set_id: u8,
    last_picture: u8,
    frame_num: u16,
    pic_init_qp: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    chroma_qp_index_offset: i8,
    second_chroma_qp_index_offset: i8,
    pic_fields: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAEncSliceParameterBufferH264 {
    macroblock_address: u32,
    num_macroblocks: u32,
    macroblock_info: u32,
    slice_type: u8,
    pic_parameter_set_id: u8,
    idr_pic_id: u16,
    pic_order_cnt_lsb: u16,
    delta_pic_order_cnt_bottom: i32,
    delta_pic_order_cnt: [i32; 2],
    direct_spatial_mv_pred_flag: u8,
    num_ref_idx_active_override_flag: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    ref_pic_list0: [VAPictureH264; 32],
    ref_pic_list1: [VAPictureH264; 32],
    luma_log2_weight_denom: u8,
    chroma_log2_weight_denom: u8,
    luma_weight_l0_flag: u8,
    luma_weight_l0: [i16; 32],
    luma_offset_l0: [i16; 32],
    chroma_weight_l0_flag: u8,
    chroma_weight_l0: [[i16; 2]; 32],
    chroma_offset_l0: [[i16; 2]; 32],
    luma_weight_l1_flag: u8,
    luma_weight_l1: [i16; 32],
    luma_offset_l1: [i16; 32],
    chroma_weight_l1_flag: u8,
    chroma_weight_l1: [[i16; 2]; 32],
    chroma_offset_l1: [[i16; 2]; 32],
    cabac_init_idc: u8,
    slice_qp_delta: i8,
    disable_deblocking_filter_idc: u8,
    slice_alpha_c0_offset_div2: i8,
    slice_beta_offset_div2: i8,
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAEncMiscParameterBuffer {
    type_: c_int,
    // Flexible array member follows in the same allocation.
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAEncMiscParameterRateControl {
    bits_per_second: u32,
    target_percentage: u32,
    window_size: u32,
    initial_qp: u32,
    min_qp: u32,
    basic_unit_size: u32,
    rc_flags: u32,
    icq_quality_factor: u32,
    max_qp: u32,
    quality_factor: u32,
    target_frame_size: u32,
    va_reserved: [u32; 4],
}

/// Layout of a rate-control misc buffer: header + rate control payload.
#[repr(C)]
#[derive(Clone, Copy)]
struct RateControlMisc {
    header: VAEncMiscParameterBuffer,
    data: VAEncMiscParameterRateControl,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAEncMiscParameterFrameRate {
    framerate: u32,
    va_reserved: [u32; 4],
}

/// Layout of a frame-rate misc buffer: header + frame-rate payload.
#[repr(C)]
struct FrameRateMisc {
    header: VAEncMiscParameterBuffer,
    data: VAEncMiscParameterFrameRate,
}

#[repr(C)]
struct VACodedBufferSegment {
    size: u32,
    bit_offset: u32,
    status: u32,
    reserved: u32,
    buf: *mut c_void,
    next: *mut c_void,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAImageFormat {
    fourcc: u32,
    byte_order: u32,
    bits_per_pixel: u32,
    depth: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    alpha_mask: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAImage {
    image_id: u32,
    format: VAImageFormat,
    buf: u32,
    width: u16,
    height: u16,
    data_size: u32,
    num_planes: u32,
    pitches: [u32; 3],
    offsets: [u32; 3],
    num_palette_entries: i32,
    entry_bytes: i32,
    component_order: [i8; 4],
    va_reserved: [u32; 4],
}

type VADisplay = *mut c_void;
type VAConfigID = u32;
type VAContextID = u32;
type VASurfaceID = u32;
type VABufferID = u32;
type VAImageID = u32;
type VAStatus = c_int;

// ─── dlopen function table (libva.so.2 + libva-drm.so.2) ───────────────────

/// dlopen function table (libva.so.2 + libva-drm.so.2).
///
/// Holds the loaded [`Library`] handles so the resolved function pointers
/// stay valid for the encoder's lifetime (dropping the library would unload
/// the code the pointers target).
struct VaFunctions {
    _library: Arc<Library>,
    _drm_library: Arc<Library>,
    get_display_drm: unsafe extern "C" fn(fd: c_int) -> VADisplay,
    initialize: unsafe extern "C" fn(dpy: VADisplay, major: *mut c_int, minor: *mut c_int) -> VAStatus,
    error_str: unsafe extern "C" fn(status: VAStatus) -> *const c_char,
    max_num_entrypoints: unsafe extern "C" fn(dpy: VADisplay) -> c_int,
    query_config_entrypoints: unsafe extern "C" fn(
        dpy: VADisplay,
        profile: c_int,
        entrypoints: *mut c_int,
        num_entrypoints: *mut c_int,
    ) -> VAStatus,
    create_config: unsafe extern "C" fn(
        dpy: VADisplay,
        profile: c_int,
        entrypoint: c_int,
        attrib_list: *mut VAConfigAttrib,
        num_attribs: c_int,
        config_id: *mut VAConfigID,
    ) -> VAStatus,
    create_context: unsafe extern "C" fn(
        dpy: VADisplay,
        config_id: VAConfigID,
        picture_width: c_int,
        picture_height: c_int,
        flag: c_int,
        render_targets: *mut VASurfaceID,
        num_render_targets: c_int,
        context_id: *mut VAContextID,
    ) -> VAStatus,
    create_surfaces: unsafe extern "C" fn(
        dpy: VADisplay,
        format: c_uint,
        width: c_uint,
        height: c_uint,
        surfaces: *mut VASurfaceID,
        num_surfaces: c_uint,
        attrib_list: *mut VASurfaceAttrib,
        num_attribs: c_uint,
    ) -> VAStatus,
    create_buffer: unsafe extern "C" fn(
        dpy: VADisplay,
        context_id: VAContextID,
        type_: c_int,
        size: c_uint,
        num_elements: c_uint,
        data: *mut c_void,
        buf_id: *mut VABufferID,
    ) -> VAStatus,
    map_buffer: unsafe extern "C" fn(dpy: VADisplay, buf_id: VABufferID, pbuf: *mut *mut c_void) -> VAStatus,
    unmap_buffer: unsafe extern "C" fn(dpy: VADisplay, buf_id: VABufferID) -> VAStatus,
    begin_picture: unsafe extern "C" fn(dpy: VADisplay, context_id: VAContextID, render_target: VASurfaceID) -> VAStatus,
    render_picture: unsafe extern "C" fn(
        dpy: VADisplay,
        context_id: VAContextID,
        buffers: *mut VABufferID,
        num_buffers: c_int,
    ) -> VAStatus,
    end_picture: unsafe extern "C" fn(dpy: VADisplay, context_id: VAContextID) -> VAStatus,
    sync_surface: unsafe extern "C" fn(dpy: VADisplay, render_target: VASurfaceID) -> VAStatus,
    destroy_buffer: unsafe extern "C" fn(dpy: VADisplay, buffer_id: VABufferID) -> VAStatus,
    destroy_surfaces: unsafe extern "C" fn(dpy: VADisplay, surfaces: *mut VASurfaceID, num_surfaces: c_int) -> VAStatus,
    destroy_context: unsafe extern "C" fn(dpy: VADisplay, context_id: VAContextID) -> VAStatus,
    destroy_config: unsafe extern "C" fn(dpy: VADisplay, config_id: VAConfigID) -> VAStatus,
    terminate: unsafe extern "C" fn(dpy: VADisplay) -> VAStatus,
    create_image: unsafe extern "C" fn(
        dpy: VADisplay,
        format: *const VAImageFormat,
        width: c_int,
        height: c_int,
        image: *mut VAImage,
    ) -> VAStatus,
    destroy_image: unsafe extern "C" fn(dpy: VADisplay, image_id: VAImageID) -> VAStatus,
    put_image: unsafe extern "C" fn(
        dpy: VADisplay,
        surface: VASurfaceID,
        image: VAImageID,
        src_x: c_int,
        src_y: c_int,
        src_width: c_uint,
        src_height: c_uint,
        dest_x: c_int,
        dest_y: c_int,
        dest_width: c_uint,
        dest_height: c_uint,
    ) -> VAStatus,
}

impl VaFunctions {
    /// dlopen libva.so.2 + libva-drm.so.2 and resolve every symbol we need.
    /// Returns a typed unavailable error when the runtime library is absent
    /// or a symbol is missing (an old libva without the encode surface).
    unsafe fn load() -> Result<Self, ScreenShareError> {
        let unavailable = |symbol: &str, error: libloading::Error| {
            ScreenShareError::hardware_acceleration_unavailable(format!(
                "VA-API symbol {symbol} unavailable: {error}"
            ))
        };
        // `libva` (safe) and `libva_drm` (kept alive for vaGetDisplayDRM).
        let library = Library::new("libva.so.2")
            .map_err(|e| ScreenShareError::hardware_acceleration_unavailable(format!("cannot load libva.so.2: {e}")))?;
        let drm_library = Library::new("libva-drm.so.2")
            .map_err(|e| ScreenShareError::hardware_acceleration_unavailable(format!("cannot load libva-drm.so.2: {e}")))?;
        // Keep both libraries alive for the lifetime of the function table.
        let library = Arc::new(library);
        let drm_library = Arc::new(drm_library);
        // Resolve each symbol individually.
        macro_rules! sym {
            ($lib:expr, $name:literal, $t:ty) => {
                *unsafe { $lib.get::<$t>($name) }
                    .map_err(|e| unavailable(std::str::from_utf8($name).unwrap_or("?"), e))?
            };
        }
        Ok(Self {
            _library: Arc::clone(&library),
            _drm_library: Arc::clone(&drm_library),
            get_display_drm: sym!(drm_library, b"vaGetDisplayDRM", unsafe extern "C" fn(c_int) -> VADisplay),
            initialize: sym!(library, b"vaInitialize", unsafe extern "C" fn(VADisplay, *mut c_int, *mut c_int) -> VAStatus),
            error_str: sym!(library, b"vaErrorStr", unsafe extern "C" fn(VAStatus) -> *const c_char),
            max_num_entrypoints: sym!(library, b"vaMaxNumEntrypoints", unsafe extern "C" fn(VADisplay) -> c_int),
            query_config_entrypoints: sym!(library, b"vaQueryConfigEntrypoints", unsafe extern "C" fn(VADisplay, c_int, *mut c_int, *mut c_int) -> VAStatus),
            create_config: sym!(library, b"vaCreateConfig", unsafe extern "C" fn(VADisplay, c_int, c_int, *mut VAConfigAttrib, c_int, *mut VAConfigID) -> VAStatus),
            create_context: sym!(library, b"vaCreateContext", unsafe extern "C" fn(VADisplay, VAConfigID, c_int, c_int, c_int, *mut VASurfaceID, c_int, *mut VAContextID) -> VAStatus),
            create_surfaces: sym!(library, b"vaCreateSurfaces", unsafe extern "C" fn(VADisplay, c_uint, c_uint, c_uint, *mut VASurfaceID, c_uint, *mut VASurfaceAttrib, c_uint) -> VAStatus),
            create_buffer: sym!(library, b"vaCreateBuffer", unsafe extern "C" fn(VADisplay, VAContextID, c_int, c_uint, c_uint, *mut c_void, *mut VABufferID) -> VAStatus),
            map_buffer: sym!(library, b"vaMapBuffer", unsafe extern "C" fn(VADisplay, VABufferID, *mut *mut c_void) -> VAStatus),
            unmap_buffer: sym!(library, b"vaUnmapBuffer", unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus),
            begin_picture: sym!(library, b"vaBeginPicture", unsafe extern "C" fn(VADisplay, VAContextID, VASurfaceID) -> VAStatus),
            render_picture: sym!(library, b"vaRenderPicture", unsafe extern "C" fn(VADisplay, VAContextID, *mut VABufferID, c_int) -> VAStatus),
            end_picture: sym!(library, b"vaEndPicture", unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus),
            sync_surface: sym!(library, b"vaSyncSurface", unsafe extern "C" fn(VADisplay, VASurfaceID) -> VAStatus),
            destroy_buffer: sym!(library, b"vaDestroyBuffer", unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus),
            destroy_surfaces: sym!(library, b"vaDestroySurfaces", unsafe extern "C" fn(VADisplay, *mut VASurfaceID, c_int) -> VAStatus),
            destroy_context: sym!(library, b"vaDestroyContext", unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus),
            destroy_config: sym!(library, b"vaDestroyConfig", unsafe extern "C" fn(VADisplay, VAConfigID) -> VAStatus),
            terminate: sym!(library, b"vaTerminate", unsafe extern "C" fn(VADisplay) -> VAStatus),
            create_image: sym!(library, b"vaCreateImage", unsafe extern "C" fn(VADisplay, *const VAImageFormat, c_int, c_int, *mut VAImage) -> VAStatus),
            destroy_image: sym!(library, b"vaDestroyImage", unsafe extern "C" fn(VADisplay, VAImageID) -> VAStatus),
            put_image: sym!(library, b"vaPutImage", unsafe extern "C" fn(VADisplay, VASurfaceID, VAImageID, c_int, c_int, c_uint, c_uint, c_int, c_int, c_uint, c_uint) -> VAStatus),
        })
    }
}

/// Render-node candidates, in preference order (the modern render node first,
/// then the legacy card node).
const RENDER_NODES: &[&str] = &["/dev/dri/renderD128", "/dev/dri/card0"];

fn open_render_node() -> Result<c_int, ScreenShareError> {
    for node in RENDER_NODES {
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(node) {
            return Ok(file.into_raw_fd());
        }
    }
    Err(ScreenShareError::hardware_acceleration_unavailable(
        "no usable DRI render node (/dev/dri/renderD128, /dev/dri/card0) — is the GPU driver loaded?",
    ))
}

/// H.264 level_idc for a resolution: 3.1 (31) up to 720p, 4.0 (40) up to
/// 1080p — matches what the OpenH264 baseline emits for the same sizes.
fn level_idc_for(width: u32, height: u32) -> u8 {
    if width <= 1280 && height <= 720 { 31 } else { 40 }
}

/// seq_fields bitfield (VAEncSequenceParameterBufferH264.seq_fields):
/// chroma_format_idc=1 (4:2:0), frame_mbs_only_flag=1,
/// direct_8x8_inference_flag=1, log2_max_frame_num_minus4=3 (frame_num range
/// 128 — covers the default 60-frame keyframe interval),
/// log2_max_pic_order_cnt_lsb_minus4=3 (POC range 128).
const SEQ_FIELDS: u32 = (1 << 0) | (1 << 2) | (1 << 5) | (3 << 6) | (3 << 12);

/// Mask applied to frame_num / POC after each frame (matches the
/// log2_max_frame_num_minus4=3 → 2^7 = 128 frame range above).
const FRAME_NUM_MASK: u16 = 0x7f;

/// pic_fields bitfield for a normal reference P frame:
/// reference_pic_flag=1 (short-term), deblocking_filter_control_present=1.
const PIC_FIELDS_P_REFERENCE: u32 = (1 << 1) | (1 << 9);

/// pic_fields bitfield for an IDR keyframe: idr_pic_flag=1 plus the
/// reference + deblocking bits.
const PIC_FIELDS_IDR: u32 = (1 << 0) | (1 << 1) | (1 << 9);

/// Hardware encoder state. Construct via [`Self::new`]; every `VideoEncoder`
/// operation maps onto the VA-API lifecycle.
pub struct VaapiEncoder {
    fns: VaFunctions,
    display: VADisplay,
    config_id: VAConfigID,
    context_id: VAContextID,
    surfaces: [VASurfaceID; SURFACE_COUNT],
    coded_buf: VABufferID,
    image: VAImage,
    /// NV12 upload staging buffer (w*h*3/2 bytes).
    nv12: Vec<u8>,
    config: CodecConfig,
    generation: u64,
    sequence: u64,
    frame_num: u16,
    /// Next frame is an IDR (first frame, force_keyframe, or bitrate change
    /// needing a re-sync).
    keyframe_requested: bool,
    /// Bitrate reconfigure pending: send a fresh rate-control misc buffer on
    /// the next frame without rebuilding the context (no generation bump).
    pending_bitrate: Option<u32>,
    /// Which surface index was last used (reference for the next P frame).
    last_surface: Option<usize>,
    shutdown: bool,
}

// The `VideoEncoder` trait requires `Send` (the host loop moves the encoder
// into the streaming task). The VA-API display/surfaces are NOT thread-safe
// and must never be shared across threads — the encoder is confined to the
// single host streaming task, exactly like the dlopen PipeWire backend
// (which holds a raw `libpipewire` handle in a `Send` struct). This is the
// same confinement contract: one encoder, one thread, no aliasing.
unsafe impl Send for VaapiEncoder {}

impl VaapiEncoder {
    /// Open the render node, initialize VA, and configure the encoder for
    /// `config`. Any failure is a typed [`ScreenShareErrorKind`] error the
    /// caller (factory) uses to fall back to OpenH264.
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        let fns = unsafe { VaFunctions::load()? };
        let fd = open_render_node()?;
        let display = unsafe { (fns.get_display_drm)(fd) };
        if display.is_null() {
            return Err(ScreenShareError::hardware_acceleration_unavailable(
                "vaGetDisplayDRM failed for the render node",
            ));
        }
        let mut major = 0;
        let mut minor = 0;
        let status = unsafe { (fns.initialize)(display, &mut major, &mut minor) };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::hardware_acceleration_unavailable(format!(
                "vaInitialize failed: {}",
                va_error(&fns, status)
            )));
        }
        // Find a usable H.264 encode entrypoint (constrained baseline first,
        // then legacy baseline).
        let entrypoint = find_encode_entrypoint(&fns, display)?;
        // Create the encode config with CBR rate control.
        let mut attribs = [
            VAConfigAttrib { type_: VA_CONFIG_ATTRIB_RT_FORMAT, value: VA_RT_FORMAT_YUV420 },
            VAConfigAttrib { type_: VA_CONFIG_ATTRIB_RATE_CONTROL, value: VA_RC_CBR },
        ];
        let mut config_id = VA_INVALID_ID;
        let status = unsafe {
            (fns.create_config)(
                display,
                entrypoint.profile,
                entrypoint.entrypoint,
                attribs.as_mut_ptr(),
                attribs.len() as c_int,
                &mut config_id,
            )
        };
        if status != VA_STATUS_SUCCESS || config_id == VA_INVALID_ID {
            return Err(ScreenShareError::hardware_acceleration_unavailable(format!(
                "vaCreateConfig failed: {}",
                va_error(&fns, status)
            )));
        }
        // Context + surfaces (NV12, ping-pong pool).
        let width = config.width;
        let height = config.height;
        let mut surfaces = [VA_INVALID_ID; SURFACE_COUNT];
        let status = unsafe {
            (fns.create_surfaces)(
                display,
                VA_RT_FORMAT_YUV420,
                width,
                height,
                surfaces.as_mut_ptr(),
                SURFACE_COUNT as c_uint,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::hardware_acceleration_unavailable(format!(
                "vaCreateSurfaces failed: {}",
                va_error(&fns, status)
            )));
        }
        let mut context_id = VA_INVALID_ID;
        let status = unsafe {
            (fns.create_context)(
                display,
                config_id,
                width as c_int,
                height as c_int,
                VA_PROGRESSIVE,
                surfaces.as_mut_ptr(),
                SURFACE_COUNT as c_int,
                &mut context_id,
            )
        };
        if status != VA_STATUS_SUCCESS || context_id == VA_INVALID_ID {
            return Err(ScreenShareError::hardware_acceleration_unavailable(format!(
                "vaCreateContext failed: {}",
                va_error(&fns, status)
            )));
        }
        // Coded output buffer (reused for every frame).
        let mut coded_buf = VA_INVALID_ID;
        let status = unsafe {
            (fns.create_buffer)(
                display,
                context_id,
                VA_ENC_CODED_BUFFER_TYPE,
                coded_buffer_size(&config) as c_uint,
                1,
                std::ptr::null_mut(),
                &mut coded_buf,
            )
        };
        if status != VA_STATUS_SUCCESS || coded_buf == VA_INVALID_ID {
            return Err(ScreenShareError::hardware_acceleration_unavailable(format!(
                "vaCreateBuffer(coded) failed: {}",
                va_error(&fns, status)
            )));
        }
        // NV12 upload image (reused every frame).
        let image_format = VAImageFormat {
            fourcc: VA_FOURCC_NV12,
            byte_order: 1, // VA_LSB_FIRST
            bits_per_pixel: 12,
            depth: 8,
            red_mask: 0,
            green_mask: 0,
            blue_mask: 0,
            alpha_mask: 0,
            va_reserved: [0; 4],
        };
        let mut image: VAImage = unsafe { std::mem::zeroed() };
        let status = unsafe {
            (fns.create_image)(
                display,
                &image_format,
                width as c_int,
                height as c_int,
                &mut image,
            )
        };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::hardware_acceleration_unavailable(format!(
                "vaCreateImage failed: {}",
                va_error(&fns, status)
            )));
        }
        let nv12 = vec![0u8; (width as usize * height as usize * 3 / 2).max(1)];
        Ok(Self {
            fns,
            display,
            config_id,
            context_id,
            surfaces,
            coded_buf,
            image,
            nv12,
            config,
            generation: 0,
            sequence: 0,
            frame_num: 0,
            keyframe_requested: true,
            pending_bitrate: None,
            last_surface: None,
            shutdown: false,
        })
    }

    fn ensure_running(&self) -> Result<(), ScreenShareError> {
        if self.shutdown {
            return Err(ScreenShareError::new("encoder is shut down"));
        }
        Ok(())
    }

    /// Build the sequence-parameter (SPS) buffer for an IDR frame.
    fn make_sequence_buffer(&self, config: &CodecConfig) -> VAEncSequenceParameterBufferH264 {
        let width_mbs = (config.width / 16) as u16;
        let height_mbs = (config.height / 16) as u16;
        VAEncSequenceParameterBufferH264 {
            seq_parameter_set_id: 0,
            level_idc: level_idc_for(config.width, config.height),
            intra_period: config.keyframe_interval as u32,
            intra_idr_period: config.keyframe_interval as u32,
            ip_period: 1,
            bits_per_second: config.target_bitrate_bps,
            max_num_ref_frames: 1,
            picture_width_in_mbs: width_mbs,
            picture_height_in_mbs: height_mbs,
            seq_fields: SEQ_FIELDS,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            offset_for_ref_frame: [0; 256],
            frame_cropping_flag: 0,
            frame_crop_left_offset: 0,
            frame_crop_right_offset: 0,
            frame_crop_top_offset: 0,
            frame_crop_bottom_offset: 0,
            vui_parameters_present_flag: 0,
            vui_fields: 0,
            aspect_ratio_idc: 0,
            sar_width: 0,
            sar_height: 0,
            num_units_in_tick: 0,
            time_scale: 0,
            va_reserved: [0; 4],
        }
    }

    /// Build the per-frame picture parameter buffer.
    fn make_picture_buffer(
        &self,
        config: &CodecConfig,
        current_surface: VASurfaceID,
        reference: Option<(VASurfaceID, u16)>,
        keyframe: bool,
        coded_buf: VABufferID,
        frame_num: u16,
    ) -> VAEncPictureParameterBufferH264 {
        let mut reference_frames = [VAPictureH264::default(); 16];
        if let Some((surface, frame_idx)) = reference {
            reference_frames[0] = VAPictureH264 {
                picture_id: surface,
                frame_idx: frame_idx as u32,
                flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                top_field_order_cnt: 0,
                bottom_field_order_cnt: 0,
                va_reserved: [0; 4],
            };
        }
        VAEncPictureParameterBufferH264 {
            curr_pic: VAPictureH264 {
                picture_id: current_surface,
                frame_idx: frame_num as u32,
                flags: 0,
                top_field_order_cnt: 0,
                bottom_field_order_cnt: 0,
                va_reserved: [0; 4],
            },
            reference_frames,
            coded_buf,
            pic_parameter_set_id: 0,
            seq_parameter_set_id: 0,
            last_picture: 0,
            frame_num,
            pic_init_qp: quality_profile_to_initial_qp(config.quality_profile),
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            chroma_qp_index_offset: 0,
            second_chroma_qp_index_offset: 0,
            pic_fields: if keyframe { PIC_FIELDS_IDR } else { PIC_FIELDS_P_REFERENCE },
            va_reserved: [0; 4],
        }
    }

    /// Build the per-frame slice parameter buffer (one full-frame slice).
    fn make_slice_buffer(
        &self,
        config: &CodecConfig,
        keyframe: bool,
        pic_order_cnt_lsb: u16,
        idr_pic_id: u16,
    ) -> VAEncSliceParameterBufferH264 {
        let total_mbs = (config.width / 16) * (config.height / 16);
        VAEncSliceParameterBufferH264 {
            macroblock_address: 0,
            num_macroblocks: total_mbs,
            macroblock_info: VA_INVALID_ID,
            slice_type: if keyframe { SLICE_TYPE_I } else { SLICE_TYPE_P },
            pic_parameter_set_id: 0,
            idr_pic_id,
            pic_order_cnt_lsb,
            delta_pic_order_cnt_bottom: 0,
            delta_pic_order_cnt: [0; 2],
            direct_spatial_mv_pred_flag: 0,
            num_ref_idx_active_override_flag: 0,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            ref_pic_list0: [VAPictureH264::default(); 32],
            ref_pic_list1: [VAPictureH264::default(); 32],
            luma_log2_weight_denom: 0,
            chroma_log2_weight_denom: 0,
            luma_weight_l0_flag: 0,
            luma_weight_l0: [0; 32],
            luma_offset_l0: [0; 32],
            chroma_weight_l0_flag: 0,
            chroma_weight_l0: [[0; 2]; 32],
            chroma_offset_l0: [[0; 2]; 32],
            luma_weight_l1_flag: 0,
            luma_weight_l1: [0; 32],
            luma_offset_l1: [0; 32],
            chroma_weight_l1_flag: 0,
            chroma_weight_l1: [[0; 2]; 32],
            chroma_offset_l1: [[0; 2]; 32],
            cabac_init_idc: 0,
            slice_qp_delta: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset_div2: 0,
            slice_beta_offset_div2: 0,
            va_reserved: [0; 4],
        }
    }

    /// Convert a CPU RGBA8/BGRA8 frame into NV12 at the encoder resolution.
    fn rgba_to_nv12(&mut self, frame: &CapturedFrame) -> Result<(), ScreenShareError> {
        if !matches!(frame.pixel_format, PixelFormat::Bgra8 | PixelFormat::Rgba8) {
            return Err(ScreenShareError::new(
                "VA-API H.264 requires a CPU BGRA8 or RGBA8 frame",
            ));
        }
        let src_w = frame.width as usize;
        let src_h = frame.height as usize;
        let dst_w = self.config.width as usize;
        let dst_h = self.config.height as usize;
        let expected = src_w.checked_mul(src_h).and_then(|n| n.checked_mul(4));
        let Some(expected) = expected else {
            return Err(ScreenShareError::new("frame dimensions overflow"));
        };
        if frame.pixels.len() != expected {
            return Err(ScreenShareError::new(
                "frame payload does not match dimensions",
            ));
        }
        if self.nv12.len() < dst_w * dst_h * 3 / 2 {
            return Err(ScreenShareError::new("NV12 staging buffer too small"));
        }
        let (y_plane, uv_plane) = self.nv12.split_at_mut(dst_w * dst_h);
        for y in 0..dst_h {
            for x in 0..dst_w {
                let sx = x * src_w / dst_w;
                let sy = y * src_h / dst_h;
                let from = (sy * src_w + sx) * 4;
                let (r, g, b) = if frame.pixel_format == PixelFormat::Bgra8 {
                    (frame.pixels[from + 2], frame.pixels[from + 1], frame.pixels[from])
                } else {
                    (frame.pixels[from], frame.pixels[from + 1], frame.pixels[from + 2])
                };
                // BT.601 studio-range YCbCr conversion (same coefficients the
                // OpenH264 path's RGB→YUV converter uses).
                let luma = ((66 * r as u32 + 129 * g as u32 + 25 * b as u32 + 128) >> 8) + 16;
                y_plane[y * dst_w + x] = luma.clamp(16, 235) as u8;
            }
        }
        for y in 0..dst_h / 2 {
            for x in 0..dst_w / 2 {
                let sx = (x * 2) * src_w / dst_w;
                let sy = (y * 2) * src_h / dst_h;
                let from = (sy * src_w + sx) * 4;
                let (r, g, b) = if frame.pixel_format == PixelFormat::Bgra8 {
                    (frame.pixels[from + 2], frame.pixels[from + 1], frame.pixels[from])
                } else {
                    (frame.pixels[from], frame.pixels[from + 1], frame.pixels[from + 2])
                };
                let u = (((-38 * r as i32 - 74 * g as i32 + 112 * b as i32 + 128) >> 8) + 128).clamp(16, 240) as u8;
                let v = (((112 * r as i32 - 94 * g as i32 - 18 * b as i32 + 128) >> 8) + 128).clamp(16, 240) as u8;
                let to = (y * dst_w / 2 + x) * 2;
                uv_plane[to] = u;
                uv_plane[to + 1] = v;
            }
        }
        Ok(())
    }

    /// Upload the NV12 staging buffer into `surface` via the VAImage path.
    fn upload_frame(&mut self, surface: VASurfaceID) -> Result<(), ScreenShareError> {
        let width = self.config.width as c_int;
        let height = self.config.height as c_int;
        // Map the image's data buffer and memcpy the NV12 planes.
        let mut mapped: *mut c_void = std::ptr::null_mut();
        let status = unsafe { (self.fns.map_buffer)(self.display, self.image.buf, &mut mapped) };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::new(format!(
                "vaMapBuffer(image) failed: {}",
                va_error(&self.fns, status)
            )));
        }
        // Copy plane-by-plane using the image's pitch/offset layout (drivers
        // may pad scanlines).
        let dst_w = self.config.width as usize;
        let dst_h = self.config.height as usize;
        unsafe {
            let base = mapped as *mut u8;
            let y_pitch = self.image.pitches[0] as usize;
            let y_offset = self.image.offsets[0] as usize;
            for row in 0..dst_h {
                let src = &self.nv12[row * dst_w..(row + 1) * dst_w];
                let dst = base.add(y_offset + row * y_pitch);
                std::ptr::copy_nonoverlapping(src.as_ptr(), dst, dst_w);
            }
            let uv_pitch = self.image.pitches[1] as usize;
            let uv_offset = self.image.offsets[1] as usize;
            let uv_len = dst_w * dst_h / 4;
            let src = &self.nv12[dst_w * dst_h..dst_w * dst_h + uv_len];
            for row in 0..dst_h / 2 {
                let row_src = &src[row * dst_w..(row + 1) * dst_w];
                let dst = base.add(uv_offset + row * uv_pitch);
                std::ptr::copy_nonoverlapping(row_src.as_ptr(), dst, dst_w);
            }
        }
        let status = unsafe { (self.fns.unmap_buffer)(self.display, self.image.buf) };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::new(format!(
                "vaUnmapBuffer(image) failed: {}",
                va_error(&self.fns, status)
            )));
        }
        let status = unsafe {
            (self.fns.put_image)(
                self.display,
                surface,
                self.image.image_id,
                0,
                0,
                width as c_uint,
                height as c_uint,
                0,
                0,
                width as c_uint,
                height as c_uint,
            )
        };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::new(format!(
                "vaPutImage failed: {}",
                va_error(&self.fns, status)
            )));
        }
        Ok(())
    }

    /// Read the coded bitstream out of the coded buffer into a Vec.
    fn read_coded_buffer(&mut self) -> Result<Vec<u8>, ScreenShareError> {
        let mut mapped: *mut c_void = std::ptr::null_mut();
        let status = unsafe { (self.fns.map_buffer)(self.display, self.coded_buf, &mut mapped) };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::new(format!(
                "vaMapBuffer(coded) failed: {}",
                va_error(&self.fns, status)
            )));
        }
        let mut out = Vec::new();
        unsafe {
            let mut segment = mapped as *const VACodedBufferSegment;
            while !segment.is_null() {
                let seg = &*segment;
                if !seg.buf.is_null() && seg.size > 0 {
                    let bytes = std::slice::from_raw_parts(seg.buf as *const u8, seg.size as usize);
                    out.extend_from_slice(bytes);
                }
                segment = seg.next as *const VACodedBufferSegment;
            }
        }
        let status = unsafe { (self.fns.unmap_buffer)(self.display, self.coded_buf) };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::new(format!(
                "vaUnmapBuffer(coded) failed: {}",
                va_error(&self.fns, status)
            )));
        }
        Ok(out)
    }

    /// Run one begin → render → end → sync encode cycle.
    fn encode_frame(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
        self.ensure_running()?;
        if frame.width == 0 || frame.height == 0 {
            return Err(ScreenShareError::new("frame dimensions must be non-zero"));
        }
        self.rgba_to_nv12(frame)?;

        // Ping-pong surface selection. The last encoded surface becomes the
        // reference for this P frame (or none for an IDR).
        let keyframe = self.keyframe_requested || self.frame_num == 0
            || self.sequence == 0;
        let current_idx = match self.last_surface {
            Some(prev) => (prev + 1) % SURFACE_COUNT,
            None => 0,
        };
        let current_surface = self.surfaces[current_idx];
        let reference = if keyframe {
            None
        } else {
            self.last_surface
                .map(|prev| (self.surfaces[prev], self.frame_num.wrapping_sub(1)))
        };

        self.upload_frame(current_surface)?;

        let mut buffers: Vec<VABufferID> = Vec::with_capacity(6);
        let mut owned_buffers: Vec<VABufferID> = Vec::new();

        // Rate control (fresh on bitrate change; also sent on keyframe so a
        // viewer re-sync never inherits a stale RC state).
        let rc_bitrate = self.pending_bitrate.unwrap_or(self.config.target_bitrate_bps);
        if self.pending_bitrate.is_some() || keyframe {
            let rate_control = RateControlMisc {
                header: VAEncMiscParameterBuffer { type_: VA_ENC_MISC_PARAMETER_TYPE_RATE_CONTROL },
                data: VAEncMiscParameterRateControl {
                    bits_per_second: rc_bitrate,
                    target_percentage: 100,
                    window_size: 1000,
                    initial_qp: 0,
                    min_qp: 0,
                    basic_unit_size: 0,
                    // disable_frame_skip=1 (bit 1): every captured frame must
                    // yield a decodable unit — mirrors the OpenH264
                    // skip_frames(false) regression (a static screen must not
                    // freeze the viewer).
                    rc_flags: 1 << 1,
                    icq_quality_factor: 0,
                    max_qp: 51,
                    quality_factor: 0,
                    target_frame_size: 0,
                    va_reserved: [0; 4],
                },
            };
            let buf = self.create_misc_buffer(&rate_control)?;
            buffers.push(buf);
            owned_buffers.push(buf);
            self.pending_bitrate = None;
        }

        // Sequence parameter (SPS) only on IDR.
        if keyframe {
            let seq = self.make_sequence_buffer(&self.config);
            let buf = self.create_param_buffer(VA_ENC_SEQUENCE_PARAMETER_BUFFER_TYPE, &seq)?;
            buffers.push(buf);
            owned_buffers.push(buf);
        }

        // Picture parameter (every frame).
        let picture = self.make_picture_buffer(
            &self.config,
            current_surface,
            reference,
            keyframe,
            self.coded_buf,
            self.frame_num,
        );
        let pic_buf = self.create_param_buffer(VA_ENC_PICTURE_PARAMETER_BUFFER_TYPE, &picture)?;
        buffers.push(pic_buf);
        owned_buffers.push(pic_buf);

        // Slice parameter (every frame).
        let slice = self.make_slice_buffer(
            &self.config,
            keyframe,
            self.frame_num, // pic_order_cnt_lsb = frame_num for type-0 POC
            self.frame_num, // idr_pic_id cycles with IDR frames
        );
        let slice_buf = self.create_param_buffer(VA_ENC_SLICE_PARAMETER_BUFFER_TYPE, &slice)?;
        buffers.push(slice_buf);
        owned_buffers.push(slice_buf);

        // Render.
        let status = unsafe { (self.fns.begin_picture)(self.display, self.context_id, current_surface) };
        if status != VA_STATUS_SUCCESS {
            self.destroy_owned_buffers(&owned_buffers);
            return Err(ScreenShareError::new(format!(
                "vaBeginPicture failed: {}",
                va_error(&self.fns, status)
            )));
        }
        let status = unsafe {
            (self.fns.render_picture)(
                self.display,
                self.context_id,
                buffers.as_mut_ptr(),
                buffers.len() as c_int,
            )
        };
        if status != VA_STATUS_SUCCESS {
            self.destroy_owned_buffers(&owned_buffers);
            return Err(ScreenShareError::new(format!(
                "vaRenderPicture failed: {}",
                va_error(&self.fns, status)
            )));
        }
        let status = unsafe { (self.fns.end_picture)(self.display, self.context_id) };
        if status != VA_STATUS_SUCCESS {
            self.destroy_owned_buffers(&owned_buffers);
            return Err(ScreenShareError::new(format!(
                "vaEndPicture failed: {}",
                va_error(&self.fns, status)
            )));
        }
        let status = unsafe { (self.fns.sync_surface)(self.display, current_surface) };
        if status != VA_STATUS_SUCCESS {
            self.destroy_owned_buffers(&owned_buffers);
            return Err(ScreenShareError::new(format!(
                "vaSyncSurface failed: {}",
                va_error(&self.fns, status)
            )));
        }
        // Owned per-frame buffers are destroyed after the render cycle; the
        // coded buffer is reused.
        self.destroy_owned_buffers(&owned_buffers);

        let bytes = self.read_coded_buffer()?;
        if bytes.is_empty() {
            return Err(ScreenShareError::new("VA-API encode produced no bitstream"));
        }

        let encoded = EncodedPacket {
            timestamp_us: frame.timestamp_us,
            encode_timestamp_us: now_micros(),
            sequence: self.sequence,
            keyframe,
            config_generation: self.generation,
            width: self.config.width,
            height: self.config.height,
            bytes,
        };
        self.sequence += 1;
        self.last_surface = Some(current_idx);
        self.frame_num = self.frame_num.wrapping_add(1) & FRAME_NUM_MASK;
        if keyframe {
            self.keyframe_requested = false;
        }
        Ok(encoded)
    }

    fn create_param_buffer<T: Copy>(&mut self, type_: c_int, data: &T) -> Result<VABufferID, ScreenShareError> {
        let size = std::mem::size_of::<T>();
        let mut buf = VA_INVALID_ID;
        let status = unsafe {
            (self.fns.create_buffer)(
                self.display,
                self.context_id,
                type_,
                size as c_uint,
                1,
                data as *const T as *mut c_void,
                &mut buf,
            )
        };
        if status != VA_STATUS_SUCCESS || buf == VA_INVALID_ID {
            return Err(ScreenShareError::new(format!(
                "vaCreateBuffer(type {type_}) failed: {}",
                va_error(&self.fns, status)
            )));
        }
        Ok(buf)
    }

    fn create_misc_buffer<T: Copy>(&mut self, data: &T) -> Result<VABufferID, ScreenShareError> {
        self.create_param_buffer(VA_ENC_MISC_PARAMETER_BUFFER_TYPE, data)
    }

    fn destroy_owned_buffers(&mut self, buffers: &[VABufferID]) {
        for &buf in buffers {
            unsafe {
                let _ = (self.fns.destroy_buffer)(self.display, buf);
            }
        }
    }

    /// Rebuild the encoder for a new resolution/fps (config generation bump).
    fn rebuild(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.teardown_hardware();
        let mut fresh = VaapiEncoder::new(config)?;
        fresh.generation = self.generation + 1;
        fresh.sequence = self.sequence;
        *self = fresh;
        Ok(())
    }

    fn teardown_hardware(&mut self) {
        unsafe {
            let _ = (self.fns.destroy_buffer)(self.display, self.coded_buf);
            let _ = (self.fns.destroy_image)(self.display, self.image.image_id);
            let _ = (self.fns.destroy_context)(self.display, self.context_id);
            let _ = (self.fns.destroy_surfaces)(self.display, self.surfaces.as_mut_ptr(), SURFACE_COUNT as c_int);
            let _ = (self.fns.destroy_config)(self.display, self.config_id);
            let _ = (self.fns.terminate)(self.display);
        }
        self.display = std::ptr::null_mut();
        self.config_id = VA_INVALID_ID;
        self.context_id = VA_INVALID_ID;
        self.coded_buf = VA_INVALID_ID;
    }
}

impl Drop for VaapiEncoder {
    fn drop(&mut self) {
        if !self.shutdown && !self.display.is_null() {
            self.teardown_hardware();
        }
    }
}

impl VideoEncoder for VaapiEncoder {
    fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        let config = config.validate()?;
        self.rebuild(config)
    }

    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
        self.encode_frame(frame)
    }

    fn force_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn is_keyframe_pending(&self) -> bool {
        self.keyframe_requested
    }

    fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        if bitrate_bps == 0 {
            return Err(ScreenShareError::new("bitrate must be non-zero"));
        }
        if bitrate_bps == self.config.target_bitrate_bps {
            return Ok(());
        }
        // VA-API supports dynamic rate control: a fresh rate-control misc
        // buffer takes effect on the next frame. No context rebuild, so no
        // config-generation bump — the decoder keeps its instance and
        // re-syncs on the forced keyframe.
        self.pending_bitrate = Some(bitrate_bps);
        self.config.target_bitrate_bps = bitrate_bps;
        self.keyframe_requested = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), ScreenShareError> {
        if self.shutdown {
            return Ok(());
        }
        self.teardown_hardware();
        self.shutdown = true;
        Ok(())
    }

    fn metadata(&self) -> CodecMetadata {
        CodecMetadata {
            codec: CodecKind::H264Vaapi,
            config: self.config,
            generation: self.generation,
        }
    }
}

/// Find an H.264 encode entrypoint on `display`: returns the profile and
/// entrypoint to use, or a typed unavailable error.
fn find_encode_entrypoint(fns: &VaFunctions, display: VADisplay) -> Result<EncodeEntrypoint, ScreenShareError> {
    for profile in [VA_PROFILE_H264_CONSTRAINED_BASELINE, VA_PROFILE_H264_BASELINE] {
        let max = unsafe { (fns.max_num_entrypoints)(display) };
        if max <= 0 {
            continue;
        }
        let mut entrypoints = vec![0 as c_int; max as usize];
        let mut num = max;
        let status = unsafe {
            (fns.query_config_entrypoints)(display, profile, entrypoints.as_mut_ptr(), &mut num)
        };
        if status != VA_STATUS_SUCCESS {
            continue;
        }
        for &entrypoint in entrypoints.iter().take(num as usize) {
            if entrypoint == VA_ENTRYPOINT_ENC_SLICE || entrypoint == VA_ENTRYPOINT_ENC_SLICE_LP {
                return Ok(EncodeEntrypoint { profile, entrypoint });
            }
        }
    }
    Err(ScreenShareError::hardware_acceleration_unavailable(
        "no VA-API H.264 encode entrypoint (VAEntrypointEncSlice) on this GPU/driver",
    ))
}

struct EncodeEntrypoint {
    profile: c_int,
    entrypoint: c_int,
}

fn va_error(fns: &VaFunctions, status: VAStatus) -> String {
    if status == VA_STATUS_SUCCESS {
        return "ok".into();
    }
    let ptr = unsafe { (fns.error_str)(status) };
    if ptr.is_null() {
        format!("VAStatus {status}")
    } else {
        let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
        cstr.to_string_lossy().into_owned()
    }
}

/// Probe whether VA-API H.264 encode is usable right now (dlopen + init +
/// entrypoint query). Cheap enough to call at Hello/negotiation time.
pub fn vaapi_encode_available() -> bool {
    (|| -> Result<(), ScreenShareError> {
        let fns = unsafe { VaFunctions::load()? };
        let fd = open_render_node()?;
        let display = unsafe { (fns.get_display_drm)(fd) };
        if display.is_null() {
            return Err(ScreenShareError::hardware_acceleration_unavailable("no display"));
        }
        let mut major = 0;
        let mut minor = 0;
        let status = unsafe { (fns.initialize)(display, &mut major, &mut minor) };
        if status != VA_STATUS_SUCCESS {
            return Err(ScreenShareError::hardware_acceleration_unavailable("init failed"));
        }
        let _ = find_encode_entrypoint(&fns, display)?;
        unsafe { let _ = (fns.terminate)(display); }
        Ok(())
    })()
    .is_ok()
}

/// Convert a `QualityProfile` to a VA-API QP hint (unused today — the
/// hardware path uses the driver's rate control; kept for parity with the
/// software path's documented knob surface).
fn quality_profile_to_initial_qp(profile: QualityProfile) -> u8 {
    match profile {
        QualityProfile::LowLatency => 32,
        QualityProfile::Balanced => 26,
        QualityProfile::HighQuality => 22,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn vaapi_layouts_match_system_headers() {
        // Offsets measured from libva-dev 2.20 headers on the build host
        // (gcc sizeof/offsetof probe). These must stay in sync with the
        // #[repr(C)] structs or the driver will misread the buffers.
        assert_eq!(std::mem::size_of::<VAConfigAttrib>(), 8);
        assert_eq!(std::mem::size_of::<VAGenericValue>(), 16);
        assert_eq!(std::mem::size_of::<VASurfaceAttrib>(), 24);
        assert_eq!(std::mem::size_of::<VAPictureH264>(), 36);
        assert_eq!(std::mem::size_of::<VAEncSequenceParameterBufferH264>(), 1132);
        assert_eq!(std::mem::size_of::<VAEncPictureParameterBufferH264>(), 648);
        assert_eq!(std::mem::size_of::<VAEncSliceParameterBufferH264>(), 3140);
        assert_eq!(std::mem::size_of::<VAEncMiscParameterBuffer>(), 4);
        assert_eq!(std::mem::size_of::<VAEncMiscParameterRateControl>(), 60);
        assert_eq!(std::mem::size_of::<VACodedBufferSegment>(), 48);
        assert_eq!(std::mem::size_of::<VAImageFormat>(), 48);
        assert_eq!(std::mem::size_of::<VAImage>(), 120);
        // Spot-check key offsets.
        assert_eq!(offset_of!(VAEncPictureParameterBufferH264, coded_buf), 612);
        assert_eq!(offset_of!(VAEncPictureParameterBufferH264, pic_fields), 628);
        assert_eq!(offset_of!(VAEncSliceParameterBufferH264, slice_type), 12);
        assert_eq!(offset_of!(VAEncSequenceParameterBufferH264, seq_fields), 28);
        assert_eq!(offset_of!(VAImage, buf), 52);
    }

    #[test]
    fn codec_kind_wire_names_round_trip() {
        assert_eq!(CodecKind::H264.wire_name(), "h264");
        assert_eq!(CodecKind::H264Vaapi.wire_name(), "h264_vaapi");
        assert_eq!(CodecKind::H264Mf.wire_name(), "h264_mf");
        assert_eq!(CodecKind::from_wire_name("h264_vaapi"), Some(CodecKind::H264Vaapi));
        assert_eq!(CodecKind::from_wire_name("H264"), Some(CodecKind::H264));
        assert_eq!(CodecKind::from_wire_name("vp8"), None);
        assert!(CodecKind::H264Vaapi.is_hardware());
        assert!(!CodecKind::H264.is_hardware());
    }

    #[test]
    fn nv12_conversion_matches_known_values() {
        // A pure-red pixel → known BT.601 Y/Cr/Cb.
        let config = CodecConfig { width: 2, height: 2, ..CodecConfig::profile_720p30() };
        let mut encoder = EncoderFixture::new(config);
        let frame = CapturedFrame::cpu(0, 2, 2, PixelFormat::Rgba8, vec![
            255, 0, 0, 255, 255, 0, 0, 255,
            255, 0, 0, 255, 255, 0, 0, 255,
        ])
        .unwrap();
        encoder.rgba_to_nv12(&frame).unwrap();
        // Pure red → BT.601 studio-range: Y=(66*255+128)>>8+16 = 82,
        // U = 90, V = 240 (chroma plane starts at w*h).
        let y = encoder.nv12[0] as u32;
        let u = encoder.nv12[2 * 2] as u32;
        let v = encoder.nv12[2 * 2 + 1] as u32;
        assert_eq!(y, 82, "pure red luma must be 82");
        assert_eq!(u, 90, "pure red U must be 90");
        assert_eq!(v, 240, "pure red V must be 240");
    }

    /// Minimal harness exposing the private conversion for unit testing
    /// without a GPU.
    struct EncoderFixture {
        config: CodecConfig,
        nv12: Vec<u8>,
    }
    impl EncoderFixture {
        fn new(config: CodecConfig) -> Self {
            let nv12 = vec![0u8; (config.width as usize * config.height as usize * 3 / 2).max(1)];
            Self { config, nv12 }
        }
        fn rgba_to_nv12(&mut self, frame: &CapturedFrame) -> Result<(), ScreenShareError> {
            let src_w = frame.width as usize;
            let src_h = frame.height as usize;
            let dst_w = self.config.width as usize;
            let dst_h = self.config.height as usize;
            if frame.pixels.len() != src_w * src_h * 4 {
                return Err(ScreenShareError::new("bad payload"));
            }
            let (y_plane, uv_plane) = self.nv12.split_at_mut(dst_w * dst_h);
            for y in 0..dst_h {
                for x in 0..dst_w {
                    let sx = x * src_w / dst_w;
                    let sy = y * src_h / dst_h;
                    let from = (sy * src_w + sx) * 4;
                    let (r, g, b) = (frame.pixels[from], frame.pixels[from + 1], frame.pixels[from + 2]);
                    let luma = ((66 * r as u32 + 129 * g as u32 + 25 * b as u32 + 128) >> 8) + 16;
                    y_plane[y * dst_w + x] = luma.clamp(16, 235) as u8;
                }
            }
            for y in 0..dst_h / 2 {
                for x in 0..dst_w / 2 {
                    let sx = (x * 2) * src_w / dst_w;
                    let sy = (y * 2) * src_h / dst_h;
                    let from = (sy * src_w + sx) * 4;
                    let (r, g, b) = (frame.pixels[from], frame.pixels[from + 1], frame.pixels[from + 2]);
                    let u = (((-38 * r as i32 - 74 * g as i32 + 112 * b as i32 + 128) >> 8) + 128).clamp(16, 240) as u8;
                    let v = (((112 * r as i32 - 94 * g as i32 - 18 * b as i32 + 128) >> 8) + 128).clamp(16, 240) as u8;
                    let to = (y * dst_w / 2 + x) * 2;
                    uv_plane[to] = u;
                    uv_plane[to + 1] = v;
                }
            }
            Ok(())
        }
    }
}
