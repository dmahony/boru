//! Linux ScreenCast backend.
//!
//! Two layers live here:
//!
//! 1. [`PortalCapture`] — the portal/PipeWire state machine and bounded frame
//!    queue (kept for API compatibility and tests).
//! 2. [`LinuxPortalCapture`] — the REAL capture backend: an
//!    xdg-desktop-portal ScreenCast client (zbus) that obtains portal
//!    consent and negotiates a PipeWire stream, plus a dlopen-based PipeWire
//!    client that consumes buffers and feeds them into the CPU frame path.
//!    This mirrors the fail-closed connect pattern of
//!    `remote_input::LinuxPortalRemoteInput::connect()`.
//!
//! The PipeWire client is deliberately dlopen-based (`libpipewire-0.3.so.0`,
//! a runtime dependency present on any desktop with xdg-desktop-portal) so
//! building does not require PipeWire development headers. When the session
//! bus, portal, or PipeWire is unavailable the factory fails closed and the
//! caller falls back to the synthetic [`TestPatternCapture`].
#![allow(missing_docs)]

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::screen_share::{
    capture::FrameSink, CapturedFrame, PixelFormat, ScreenCapture, ScreenShareError,
    TestPatternCapture,
};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};

/// State of the XDG ScreenCast portal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalState {
    Idle,
    Selecting,
    Streaming,
    Ending,
    Ended,
}

/// A portal-approved Linux capture session fed by PipeWire buffers.
#[derive(Debug)]
pub struct PortalCapture {
    state: PortalState,
    sink: FrameSink,
    format: Option<(u32, u32, PixelFormat)>,
    pending_events: VecDeque<PortalEvent>,
}

/// Lifecycle and format events emitted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalEvent {
    SourcePickerOpened,
    SourceSelected,
    FormatChanged { width: u32, height: u32 },
    Ended,
}

impl PortalCapture {
    /// Create an idle session with a bounded frame queue.
    pub fn new(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            state: PortalState::Idle,
            sink: FrameSink::new(queue_capacity)?,
            format: None,
            pending_events: VecDeque::new(),
        })
    }
    /// Request the native XDG Desktop Portal source picker.
    pub fn begin_selection(&mut self) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Idle {
            return Err(ScreenShareError::new("portal session is already active"));
        }
        self.state = PortalState::Selecting;
        self.pending_events
            .push_back(PortalEvent::SourcePickerOpened);
        Ok(())
    }
    /// Handle a portal cancellation without leaving PipeWire resources alive.
    pub fn cancel(&mut self) {
        if matches!(self.state, PortalState::Selecting | PortalState::Streaming) {
            self.state = PortalState::Ended;
            self.pending_events.push_back(PortalEvent::Ended);
        }
    }
    /// Mark the portal stream as selected and ready to receive PipeWire buffers.
    pub fn source_selected(&mut self) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Selecting {
            return Err(ScreenShareError::new(
                "portal source was not being selected",
            ));
        }
        self.state = PortalState::Streaming;
        self.pending_events.push_back(PortalEvent::SourceSelected);
        Ok(())
    }
    /// Normalize one PipeWire BGRA/RGBA buffer and enqueue it.
    pub fn push_pipewire_frame(&mut self, frame: CapturedFrame) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Streaming {
            return Err(ScreenShareError::new(
                "PipeWire frame received outside streaming state",
            ));
        }
        let current = (frame.width, frame.height, frame.pixel_format);
        if self.format.map(|f| (f.0, f.1)) != Some((frame.width, frame.height)) {
            if self.format.is_some() {
                self.pending_events.push_back(PortalEvent::FormatChanged {
                    width: frame.width,
                    height: frame.height,
                });
            }
            self.format = Some(current);
        }
        self.sink.push(frame);
        Ok(())
    }
    /// Signal that the OS/portal closed the stream.
    pub fn stream_closed(&mut self) {
        self.state = PortalState::Ended;
        self.pending_events.push_back(PortalEvent::Ended);
    }
    /// Read the next lifecycle event.
    pub fn next_event(&mut self) -> Option<PortalEvent> {
        self.pending_events.pop_front()
    }
    /// Return bounded queue diagnostics: captured, encoded, dropped.
    pub fn counters(&self) -> (u64, u64, u64) {
        self.sink.counters()
    }
    /// Current portal state.
    pub fn state(&self) -> PortalState {
        self.state
    }
}

impl ScreenCapture for PortalCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        Ok(self.sink.pop_latest())
    }
}

// ── Real XDG Desktop Portal ScreenCast + PipeWire capture ───────────────────
//
// ScreenCast flow (org.freedesktop.portal.ScreenCast on the session bus):
//   1. CreateSession(session_handle_token) → session object path.
//   2. SelectSources(session, {types}) — monitor sources on X11 auto-select
//      the primary monitor; on Wayland the compositor shows the picker.
//   3. Start(session, "", {handle_token}) — blocks until a source is chosen,
//      returns a PipeWire node id.
//   4. Connect a PipeWire INPUT stream to that node and consume buffers.
// The PipeWire client is dlopen'd at runtime; all PipeWire objects live on a
// dedicated thread so raw pointers never cross threads.

/// Format state negotiated with the PipeWire stream. Read by the main thread.
#[derive(Debug, Clone, Copy)]
struct NegotiatedFormat {
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
}

impl Default for NegotiatedFormat {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixel_format: PixelFormat::Bgra8,
        }
    }
}

/// The real Linux capture backend: portal consent + PipeWire stream.
#[derive(Debug)]
pub struct LinuxPortalCapture {
    portal: PortalCapture,
    frames: Receiver<CapturedFrame>,
    events: Receiver<PortalEvent>,
    format: Arc<Mutex<NegotiatedFormat>>,
}

impl LinuxPortalCapture {
    /// Portal timeout for the interactive `Start` call. A desktop user is
    /// expected to pick a source; headless/CI environments fail closed.
    pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(20);

    /// Establish a full ScreenCast session: portal consent, PipeWire stream,
    /// and the capture object that yields real desktop frames. Fails closed
    /// (Err) when no session bus, portal, or PipeWire server is reachable.
    pub async fn connect() -> Result<Self, ScreenShareError> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| ScreenShareError::new(format!("no session bus: {e}")))?;
        let portal = (
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        );
        let token = format!("boru_{:016x}", rand::random::<u64>());
        let options: std::collections::HashMap<&str, zbus::zvariant::Value> = [(
            "session_handle_token",
            zbus::zvariant::Value::from(token.as_str()),
        )]
        .into_iter()
        .collect();
        let reply = connection
            .call_method(Some(portal.0), portal.1, Some(portal.2), "CreateSession", &options)
            .await
            .map_err(|e| ScreenShareError::new(format!("portal CreateSession failed: {e}")))?;
        let session: zbus::zvariant::OwnedObjectPath = reply
            .body()
            .deserialize()
            .map_err(|e| ScreenShareError::new(format!("portal session reply malformed: {e}")))?;

        // Monitor sources (1); Wayland shows the compositor picker, X11 picks
        // the primary monitor automatically.
        let select_options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            [("types", zbus::zvariant::Value::U32(1))].into_iter().collect();
        let _ = connection
            .call_method(Some(portal.0), portal.1, Some(portal.2), "SelectSources", &(session.clone(), select_options))
            .await
            .map_err(|e| ScreenShareError::new(format!("portal SelectSources failed: {e}")))?;

        // Start blocks until the user picks a source on Wayland; bound it so
        // headless environments fail closed instead of hanging the session.
        let start_token = format!("boru_start_{:016x}", rand::random::<u64>());
        let start_options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            [("handle_token", zbus::zvariant::Value::from(start_token.as_str()))]
                .into_iter()
                .collect();
        // Portal requests complete asynchronously: Start returns a request
        // object path and emits Response(u32, a{sv}) on that path.  Waiting
        // for the method reply body here would never yield the stream list.
        let request_path: zbus::zvariant::OwnedObjectPath = tokio::time::timeout(
            Self::PORTAL_TIMEOUT,
            connection.call_method(Some(portal.0), portal.1, Some(portal.2), "Start", &(session, "", start_options)),
        )
        .await
        .map_err(|_| ScreenShareError::new("portal Start timed out (no source selected)"))?
        .map_err(|e| ScreenShareError::new(format!("portal Start failed: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| ScreenShareError::new(format!("portal Start request malformed: {e}")))?;
        let request = zbus::Proxy::new(
            &connection,
            portal.0,
            request_path.as_str(),
            "org.freedesktop.portal.Request",
        )
        .await
        .map_err(|e| ScreenShareError::new(format!("portal request proxy failed: {e}")))?;
        let mut responses = request
            .receive_signal("Response")
            .await
            .map_err(|e| ScreenShareError::new(format!("portal response subscription failed: {e}")))?;
        let response = tokio::time::timeout(Self::PORTAL_TIMEOUT, n0_future::StreamExt::next(&mut responses))
            .await
            .map_err(|_| ScreenShareError::new("portal Start timed out (no response)"))?
            .ok_or_else(|| ScreenShareError::new("portal response stream closed"))?;
        let (response_code, body): (u32, zbus::zvariant::OwnedValue) = response
            .body()
            .deserialize()
            .map_err(|e| ScreenShareError::new(format!("portal response malformed: {e}")))?;
        if response_code != 0 {
            return Err(ScreenShareError::new(format!("portal source selection rejected ({response_code})")));
        }
        let node_id = extract_stream_node_id(&body)
            .ok_or_else(|| ScreenShareError::new("portal Start reply missing stream node id"))?;

        Self::from_node_id(node_id)
    }

    /// Connect the PipeWire stream to an already-negotiated portal node id.
    fn from_node_id(node_id: u32) -> Result<Self, ScreenShareError> {
        let (frame_tx, frames) = sync_channel::<CapturedFrame>(4);
        let (event_tx, events) = sync_channel::<PortalEvent>(4);
        let format = Arc::new(Mutex::new(NegotiatedFormat::default()));
        PipeWireClient::connect(node_id, frame_tx, event_tx, format.clone())
            .map_err(|e| ScreenShareError::new(format!("PipeWire capture failed: {e}")))?;

        let mut portal = PortalCapture::new(4)?;
        portal.source_selected()?;
        Ok(Self {
            portal,
            frames,
            events,
            format,
        })
    }

    /// Read the next lifecycle event from the PipeWire thread.
    pub fn poll_event(&mut self) -> Option<PortalEvent> {
        self.events.try_recv().ok()
    }

    /// Current negotiated frame size, if the stream has produced one.
    pub fn negotiated_size(&self) -> Option<(u32, u32)> {
        let f = self.format.lock().unwrap();
        if f.width > 0 && f.height > 0 {
            Some((f.width, f.height))
        } else {
            None
        }
    }

    /// Current portal state.
    pub fn state(&self) -> PortalState {
        self.portal.state()
    }
}

impl ScreenCapture for LinuxPortalCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        // Drain lifecycle events first so format changes are observed before
        // the frame that triggered them.
        while let Ok(event) = self.events.try_recv() {
            match event {
                PortalEvent::Ended => {
                    self.portal.stream_closed();
                    return Err(ScreenShareError::new("portal stream ended"));
                }
                _ => {}
            }
        }
        // Return the newest queued frame, dropping stale ones.
        let mut latest: Option<CapturedFrame> = None;
        while let Ok(frame) = self.frames.try_recv() {
            latest = Some(frame);
        }
        if let Some(frame) = &latest {
            let mut fmt = self.format.lock().unwrap();
            if fmt.width == 0 {
                *fmt = NegotiatedFormat {
                    width: frame.width,
                    height: frame.height,
                    pixel_format: frame.pixel_format,
                };
            }
            if fmt.width != frame.width || fmt.height != frame.height {
                fmt.width = frame.width;
                fmt.height = frame.height;
            }
            drop(fmt);
            let _ = self.portal.push_pipewire_frame(frame.clone());
        }
        Ok(self.portal.sink.pop_latest())
    }
}

impl Drop for LinuxPortalCapture {
    fn drop(&mut self) {
        self.portal.stream_closed();
    }
}

// ── PipeWire dlopen client ───────────────────────────────────────────────────

const PW_LIB: &str = "libpipewire-0.3.so.0";

/// Minimal pw_buffer mirror (layout matches `struct pw_buffer` in stream.h).
#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
    user_data: *mut c_void,
    size: u64,
    requested: u64,
}

/// Minimal spa_buffer mirror (layout matches `struct spa_buffer` in buffer.h).
#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

/// Minimal spa_data mirror (layout matches `struct spa_data` in buffer.h).
#[repr(C)]
struct SpaData {
    type_: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

/// Minimal spa_chunk mirror (layout matches `struct spa_chunk` in buffer.h).
#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

/// PipeWire stream events table (layout matches `struct pw_stream_events`).
#[repr(C)]
struct PwStreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// Owned PipeWire objects and the function table. Lives on the capture thread.
struct PipeWireCtx {
    library: libloading::Library,
    pw: Pw,
    main_loop: *mut c_void,
    context: *mut c_void,
    core: *mut c_void,
    stream: *mut c_void,
    /// The format pod bytes handed to pw_stream_connect must outlive the stream.
    params: Vec<u8>,
}

// SAFETY: raw pointers are only dereferenced on the thread that owns `ctx`.
unsafe impl Send for PipeWireCtx {}

/// Per-stream callback payload, passed as `pw_stream_new_simple` user data.
struct StreamUserData {
    ctx: *mut PipeWireCtx,
    frame_tx: SyncSender<CapturedFrame>,
    event_tx: SyncSender<PortalEvent>,
    format: Arc<Mutex<NegotiatedFormat>>,
}

// SAFETY: as for PipeWireCtx — all access happens on the capture thread.
unsafe impl Send for StreamUserData {}

/// Function table for the PipeWire ABI we use.
struct Pw {
    init: unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char),
    main_loop_new: unsafe extern "C" fn(props: *const c_void) -> *mut c_void,
    main_loop_get_loop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    main_loop_run: unsafe extern "C" fn(*mut c_void) -> i32,
    main_loop_quit: unsafe extern "C" fn(*mut c_void) -> i32,
    main_loop_destroy: unsafe extern "C" fn(*mut c_void),
    context_new: unsafe extern "C" fn(loop_: *mut c_void, props: *const c_void, user_data_size: usize) -> *mut c_void,
    context_connect: unsafe extern "C" fn(*mut c_void, props: *mut c_void, user_data_size: usize) -> *mut c_void,
    context_destroy: unsafe extern "C" fn(*mut c_void),
    core_disconnect: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_new_simple: unsafe extern "C" fn(
        loop_: *mut c_void,
        name: *const c_char,
        props: *mut c_void,
        events: *const PwStreamEvents,
        data: *mut c_void,
    ) -> *mut c_void,
    stream_connect: unsafe extern "C" fn(
        stream: *mut c_void,
        direction: u32,
        target_id: u32,
        flags: u32,
        params: *const *const c_void,
        n_params: u32,
    ) -> i32,
    stream_destroy: unsafe extern "C" fn(*mut c_void),
    stream_disconnect: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_dequeue_buffer: unsafe extern "C" fn(*mut c_void) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(*mut c_void, *mut PwBuffer) -> i32,
    properties_new: unsafe extern "C" fn(key: *const c_char, ...) -> *mut c_void,
    properties_set: unsafe extern "C" fn(*mut c_void, key: *const c_char, value: *const c_char) -> i32,
    properties_free: unsafe extern "C" fn(*mut c_void),
}

impl Pw {
    fn load(library: &libloading::Library) -> Result<Self, ScreenShareError> {
        macro_rules! sym {
            ($name:literal) => {
                unsafe {
                    *library
                        .get::<unsafe extern "C" fn()>(concat!($name, "\0").as_bytes())
                        .map_err(|e| ScreenShareError::new(format!("symbol {} missing: {e}", $name)))?
                }
            };
        }
        Ok(Self {
            init: unsafe { std::mem::transmute(sym!("pw_init")) },
            main_loop_new: unsafe { std::mem::transmute(sym!("pw_main_loop_new")) },
            main_loop_get_loop: unsafe { std::mem::transmute(sym!("pw_main_loop_get_loop")) },
            main_loop_run: unsafe { std::mem::transmute(sym!("pw_main_loop_run")) },
            main_loop_quit: unsafe { std::mem::transmute(sym!("pw_main_loop_quit")) },
            main_loop_destroy: unsafe { std::mem::transmute(sym!("pw_main_loop_destroy")) },
            context_new: unsafe { std::mem::transmute(sym!("pw_context_new")) },
            context_connect: unsafe { std::mem::transmute(sym!("pw_context_connect")) },
            context_destroy: unsafe { std::mem::transmute(sym!("pw_context_destroy")) },
            core_disconnect: unsafe { std::mem::transmute(sym!("pw_core_disconnect")) },
            stream_new_simple: unsafe { std::mem::transmute(sym!("pw_stream_new_simple")) },
            stream_connect: unsafe { std::mem::transmute(sym!("pw_stream_connect")) },
            stream_destroy: unsafe { std::mem::transmute(sym!("pw_stream_destroy")) },
            stream_disconnect: unsafe { std::mem::transmute(sym!("pw_stream_disconnect")) },
            stream_dequeue_buffer: unsafe { std::mem::transmute(sym!("pw_stream_dequeue_buffer")) },
            stream_queue_buffer: unsafe { std::mem::transmute(sym!("pw_stream_queue_buffer")) },
            properties_new: unsafe { std::mem::transmute(sym!("pw_properties_new")) },
            properties_set: unsafe { std::mem::transmute(sym!("pw_properties_set")) },
            properties_free: unsafe { std::mem::transmute(sym!("pw_properties_free")) },
        })
    }
}

struct PipeWireClient;

impl PipeWireClient {
    /// Connect a capture stream to the given portal node and spawn the
    /// PipeWire main loop on a background thread.
    fn connect(
        node_id: u32,
        frame_tx: SyncSender<CapturedFrame>,
        event_tx: SyncSender<PortalEvent>,
        format: Arc<Mutex<NegotiatedFormat>>,
    ) -> Result<(), ScreenShareError> {
        // SAFETY: every raw pointer below is created and used on the spawned
        // thread. `ctx` is boxed and its pointer handed to the thread; the
        // stream events borrow the same context for their whole lifetime,
        // which ends when the loop quits.
        unsafe {
            let library = libloading::Library::new(PW_LIB)
                .map_err(|e| ScreenShareError::new(format!("cannot load {PW_LIB}: {e}")))?;
            let pw = Pw::load(&library)?;
            let mut argc = 0i32;
            let mut argv: *mut *mut c_char = std::ptr::null_mut();
            (pw.init)(&mut argc, &mut argv);

            let main_loop = (pw.main_loop_new)(std::ptr::null());
            if main_loop.is_null() {
                return Err(ScreenShareError::new("pw_main_loop_new failed"));
            }
            let loop_ = (pw.main_loop_get_loop)(main_loop);
            let context = (pw.context_new)(loop_, std::ptr::null(), 0);
            if context.is_null() {
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::new("pw_context_new failed"));
            }
            let core = (pw.context_connect)(context, std::ptr::null_mut(), 0);
            if core.is_null() {
                (pw.context_destroy)(context);
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::new(
                    "pw_context_connect failed (is PipeWire running?)",
                ));
            }

            let props = make_stream_properties(&pw)?;
            let params = build_format_params();

            let ctx = Box::into_raw(Box::new(PipeWireCtx {
                library,
                pw,
                main_loop,
                context,
                core,
                stream: std::ptr::null_mut(),
                params,
            }));

            let user_data = Box::into_raw(Box::new(StreamUserData {
                ctx,
                frame_tx,
                event_tx,
                format: format.clone(),
            }));

            let events = PwStreamEvents {
                version: 2,
                destroy: None,
                state_changed: Some(stream_state_changed),
                control_info: None,
                io_changed: None,
                param_changed: Some(stream_param_changed),
                add_buffer: None,
                remove_buffer: None,
                process: Some(stream_process),
                drained: None,
                command: None,
                trigger_done: None,
            };

            let stream = ((*ctx).pw.stream_new_simple)(
                loop_,
                c"boru-screen-capture".as_ptr(),
                props,
                &events,
                user_data as *mut c_void,
            );
            if stream.is_null() {
                ((*ctx).pw.properties_free)(props);
                drop(Box::from_raw(user_data));
                drop(Box::from_raw(ctx));
                return Err(ScreenShareError::new("pw_stream_new_simple failed"));
            }
            (*ctx).stream = stream;

            // Advertise the formats we can consume: BGRx (preferred), BGRA,
            // RGBA. The portal converts its native format to one of these.
            let flags = PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS;
            let result = ((*ctx).pw.stream_connect)(
                stream,
                PW_DIRECTION_INPUT,
                node_id,
                flags,
                [(*ctx).params.as_ptr() as *const c_void].as_ptr(),
                1,
            );
            if result < 0 {
                ((*ctx).pw.stream_destroy)(stream);
                drop(Box::from_raw(user_data));
                drop(Box::from_raw(ctx));
                return Err(ScreenShareError::new(format!(
                    "pw_stream_connect failed: {result}"
                )));
            }

            // `thread::spawn` requires every captured value to be Send. Raw
            // pointers are not, so carry them as usize (Send) and reconstruct
            // on the thread; the boxed objects stay alive until the thread
            // drops them.
            let ctx_addr = ctx as usize;
            let user_addr = user_data as usize;
            std::thread::Builder::new()
                .name("boru-pipewire-capture".into())
                .spawn(move || {
                    run_pipewire_thread(ctx_addr as *mut PipeWireCtx, user_addr as *mut StreamUserData)
                })
                .map_err(|e| ScreenShareError::new(format!("spawn pipewire thread: {e}")))?;

            Ok(())
        }
    }
}

/// Build the PipeWire stream properties (null-terminated varargs call).
unsafe fn make_stream_properties(pw: &Pw) -> Result<*mut c_void, ScreenShareError> {
    let media_type = CString::new("media.type").unwrap();
    let video = CString::new("Video").unwrap();
    let category = CString::new("media.category").unwrap();
    let capture = CString::new("Capture").unwrap();
    let role = CString::new("media.role").unwrap();
    let screen = CString::new("Screen").unwrap();
    let node_name = CString::new("node.name").unwrap();
    let node_value = CString::new("boru-screen-capture").unwrap();
    let props = (pw.properties_new)(
        media_type.as_ptr(),
        video.as_ptr(),
        category.as_ptr(),
        capture.as_ptr(),
        role.as_ptr(),
        screen.as_ptr(),
        node_name.as_ptr(),
        node_value.as_ptr(),
        std::ptr::null::<c_char>(),
    );
    if props.is_null() {
        return Err(ScreenShareError::new("pw_properties_new failed"));
    }
    Ok(props)
}

/// Run the PipeWire main loop until quit; forwards frames and events.
fn run_pipewire_thread(ctx: *mut PipeWireCtx, user_data: *mut StreamUserData) {
    unsafe {
        let _ = ((*ctx).pw.main_loop_run)((*ctx).main_loop);
        let _ = ((*ctx).pw.stream_disconnect)((*ctx).stream);
        let _ = ((*ctx).pw.stream_destroy)((*ctx).stream);
        let _ = ((*ctx).pw.core_disconnect)((*ctx).core);
        let _ = ((*ctx).pw.context_destroy)((*ctx).context);
        let _ = ((*ctx).pw.main_loop_destroy)((*ctx).main_loop);
        drop(Box::from_raw(user_data));
        drop(Box::from_raw(ctx));
    }
}

unsafe extern "C" fn stream_state_changed(
    data: *mut c_void,
    _old: i32,
    _state: i32,
    _error: *const c_char,
) {
    let _ = data;
}

unsafe extern "C" fn stream_param_changed(
    data: *mut c_void,
    id: u32,
    param: *const c_void,
) {
    // SPA_PARAM_Format (4) carries the negotiated geometry/format.
    if id != 4 || param.is_null() {
        return;
    }
    let user = data as *mut StreamUserData;
    let Some((width, height, pixel_format)) = parse_format_pod(param) else {
        return;
    };
    let mut fmt = (*user).format.lock().unwrap();
    if fmt.width != width || fmt.height != height || fmt.pixel_format != pixel_format {
        *fmt = NegotiatedFormat {
            width,
            height,
            pixel_format,
        };
        let _ = (*user)
            .event_tx
            .try_send(PortalEvent::FormatChanged { width, height });
    }
}

unsafe extern "C" fn stream_process(data: *mut c_void) {
    let user = data as *mut StreamUserData;
    let ctx = (*user).ctx;
    let pw = &(*ctx).pw;
    let buffer = (pw.stream_dequeue_buffer)((*ctx).stream);
    if buffer.is_null() {
        return;
    }
    let spa = (*buffer).buffer;
    if !spa.is_null() && (*spa).n_datas > 0 {
        let dat = (*spa).datas;
        if !dat.is_null() && !(*dat).data.is_null() {
            let chunk = (*dat).chunk;
            let offset = if chunk.is_null() { 0 } else { (*chunk).offset as usize };
            let size = if chunk.is_null() || (*chunk).size == 0 {
                (*dat).maxsize as usize
            } else {
                (*chunk).size as usize
            };
            let src = std::slice::from_raw_parts((*dat).data as *const u8, size);
            let payload = src[offset.min(size)..].to_vec();
            let fmt = *(*user).format.lock().unwrap();
            if fmt.width > 0 && fmt.height > 0 {
                let expected = fmt.width as usize * fmt.height as usize * 4;
                if payload.len() >= expected {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;
                    let frame = CapturedFrame {
                        timestamp_us: now,
                        width: fmt.width,
                        height: fmt.height,
                        pixel_format: fmt.pixel_format,
                        pixels: payload[..expected].to_vec(),
                        gpu_handle: None,
                    };
                    let _ = (*user).frame_tx.try_send(frame);
                }
            }
        }
    }
    (pw.stream_queue_buffer)((*ctx).stream, buffer);
}

const PW_DIRECTION_INPUT: u32 = 0;

const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;

// SPA pod/type constants (spa/include/spa/param/param-types.h,
// spa/include/spa/param/format.h, spa/include/spa/param/video/raw.h).
const SPA_TYPE_Object: u32 = 16;
const SPA_TYPE_Id: u32 = 3;
const SPA_TYPE_Choice: u32 = 20;
const SPA_TYPE_Rectangle: u32 = 10;
const SPA_POD_OBJECT_TYPE_Format: u32 = 4;
const SPA_FORMAT_mediaType: u32 = 0x10001;
const SPA_FORMAT_mediaSubtype: u32 = 0x10002;
const SPA_FORMAT_VIDEO_format: u32 = 0x20001;
const SPA_FORMAT_VIDEO_size: u32 = 0x20003;
const SPA_MEDIA_TYPE_VIDEO: u32 = 1;
const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;
const SPA_VIDEO_FORMAT_BGRx: u32 = 7;
const SPA_VIDEO_FORMAT_RGBA: u32 = 10;
const SPA_VIDEO_FORMAT_BGRA: u32 = 11;

/// Map a negotiated SPA video format id to the CPU pixel format we encode.
fn spa_format_to_pixel_format(format_id: u32) -> Option<PixelFormat> {
    match format_id {
        SPA_VIDEO_FORMAT_BGRx | SPA_VIDEO_FORMAT_BGRA => Some(PixelFormat::Bgra8),
        SPA_VIDEO_FORMAT_RGBA => Some(PixelFormat::Rgba8),
        _ => None,
    }
}

/// Build the SPA format object pod advertising BGRx/BGRA/RGBA.
///
/// Layout (all little-endian, 8-byte aligned):
///   pod header { u32 body_size, u32 type = Object }
///   object body { u32 type = ParamFormat, u32 id = Format }
///   prop { u32 key, u32 flags, pod value }
fn build_format_params() -> Vec<u8> {
    let mut pod: Vec<u8> = Vec::new();
    // Placeholder header: size patched once the body is known.
    pod.extend_from_slice(&[0, 0, 0, 0]);
    pod.extend_from_slice(&SPA_TYPE_Object.to_le_bytes());
    pod.extend_from_slice(&SPA_POD_OBJECT_TYPE_Format.to_le_bytes());
    pod.extend_from_slice(&4u32.to_le_bytes()); // id = SPA_PARAM_Format
    push_prop_id(&mut pod, SPA_FORMAT_mediaType, SPA_MEDIA_TYPE_VIDEO);
    push_prop_id(&mut pod, SPA_FORMAT_mediaSubtype, SPA_MEDIA_SUBTYPE_RAW);
    push_prop_choice_id(
        &mut pod,
        SPA_FORMAT_VIDEO_format,
        &[SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRA, SPA_VIDEO_FORMAT_RGBA],
    );
    push_prop_rectangle(&mut pod, SPA_FORMAT_VIDEO_size, 640, 360);
    let body_size = pod.len() as u32 - 8;
    pod[0..4].copy_from_slice(&body_size.to_le_bytes());
    pod
}

fn push_prop_id(pod: &mut Vec<u8>, key: u32, value: u32) {
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&4u32.to_le_bytes()); // value pod body size
    pod.extend_from_slice(&SPA_TYPE_Id.to_le_bytes());
    pod.extend_from_slice(&value.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // value padding
}

fn push_prop_rectangle(pod: &mut Vec<u8>, key: u32, width: u32, height: u32) {
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&8u32.to_le_bytes()); // value pod body size
    pod.extend_from_slice(&SPA_TYPE_Rectangle.to_le_bytes());
    pod.extend_from_slice(&width.to_le_bytes());
    pod.extend_from_slice(&height.to_le_bytes());
}

fn push_prop_choice_id(pod: &mut Vec<u8>, key: u32, values: &[u32]) {
    let n = values.len();
    // Choice body: kind + flags + child Id pod + alternative values.
    let value_body = 16 + 4 * n;
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&(value_body as u32).to_le_bytes());
    pod.extend_from_slice(&SPA_TYPE_Choice.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // choice type: Enum
    pod.extend_from_slice(&0u32.to_le_bytes()); // choice flags
    pod.extend_from_slice(&4u32.to_le_bytes()); // child pod size
    pod.extend_from_slice(&SPA_TYPE_Id.to_le_bytes());
    pod.extend_from_slice(&values[0].to_le_bytes()); // default = first format
    for v in &values[1..] {
        pod.extend_from_slice(&v.to_le_bytes());
    }
    // The value pod is 8-byte aligned before the next property.
    while pod.len() % 8 != 0 {
        pod.push(0);
    }
}

/// Parse a SPA format object pod into (width, height, pixel_format).
fn parse_format_pod(pod: *const c_void) -> Option<(u32, u32, PixelFormat)> {
    if pod.is_null() {
        return None;
    }
    // SAFETY: the pod is owned by PipeWire and stays valid for the callback.
    let head = unsafe { std::slice::from_raw_parts(pod as *const u8, 8) };
    if head.len() < 8 || u32::from_le_bytes(head[4..8].try_into().ok()?) != SPA_TYPE_Object {
        return None;
    }
    let total = u32::from_le_bytes(head[0..4].try_into().ok()?) as usize;
    // Clamp reads to the declared pod body so a short pod cannot overrun.
    let body = unsafe { std::slice::from_raw_parts(pod.add(8) as *const u8, total) };
    if body.len() < 8 {
        return None;
    }
    // body[0..4] = object type (ParamFormat), body[4..8] = id; props follow.
    let mut offset = 8usize;
    let mut format_id: Option<u32> = None;
    let mut size: Option<(u32, u32)> = None;
    while offset + 16 <= body.len() {
        let key = u32::from_le_bytes(body[offset..offset + 4].try_into().ok()?);
        let value_body_size =
            u32::from_le_bytes(body[offset + 8..offset + 12].try_into().ok()?) as usize;
        let value_type = u32::from_le_bytes(body[offset + 12..offset + 16].try_into().ok()?);
        // Value pod header: body size at offset+16, type at offset+20, data at
        // offset+20; value data starts at offset+16.
        let value_data = &body[offset + 16..];
        match (key, value_type) {
            (SPA_FORMAT_VIDEO_format, SPA_TYPE_Choice) => {
                // choice body: type(4) flags(4) child pod(size+type) value...
                // The chosen value is the child pod value (offset 16 within the
                // choice value) when the child is an Id.
                if value_body_size >= 20 && value_data.len() >= 20 {
                    let child_type = u32::from_le_bytes(value_data[12..16].try_into().ok()?);
                    if child_type == SPA_TYPE_Id {
                        format_id = Some(u32::from_le_bytes(value_data[16..20].try_into().ok()?));
                    }
                }
            }
            (SPA_FORMAT_VIDEO_format, SPA_TYPE_Id) => {
                if value_data.len() >= 4 {
                    format_id = Some(u32::from_le_bytes(value_data[0..4].try_into().ok()?));
                }
            }
            (SPA_FORMAT_VIDEO_size, SPA_TYPE_Rectangle) => {
                if value_data.len() >= 8 {
                    let w = u32::from_le_bytes(value_data[0..4].try_into().ok()?);
                    let h = u32::from_le_bytes(value_data[4..8].try_into().ok()?);
                    size = Some((w, h));
                }
            }
            _ => {}
        }
        let value_pod_size = (8 + value_body_size + 7) & !7;
        offset += 8 + value_pod_size;
    }
    let format_id = format_id?;
    let (width, height) = size?;
    let pixel_format = spa_format_to_pixel_format(format_id)?;
    Some((width, height, pixel_format))
}

/// Extract the first stream node id from a portal Start reply body.
///
/// The reply is a dictionary `{ "streams": [ { "node_id": u32, ... }, ... ] }`.
/// zvariant 5 does not implement `TryFrom<&Value>` for Vec/HashMap, so walk
/// the Value enum directly instead of downcasting.
fn extract_stream_node_id(body: &zbus::zvariant::Value) -> Option<u32> {
    use zbus::zvariant::Value;
    let streams_key = "streams".to_string();
    let node_key = "node_id".to_string();
    let Value::Dict(dict) = body else { return None };
    let streams = dict.get::<String, Value>(&streams_key).ok()??;
    let Value::Array(array) = streams else { return None };
    for item in array.iter() {
        let Value::Dict(stream) = item else { continue };
        let node = stream.get::<String, Value>(&node_key).ok()??;
        let Value::U32(node_id) = node else { continue };
        return Some(node_id);
    }
    None
}

// ── Direct X11 capture backend ─────────────────────────────────────────────

/// Direct X11 capture: grabs the root window via `GetImage` and converts the
/// ZPixmap buffer to RGBA8. This is the no-portal fallback — it makes real
/// desktop sharing work on any X11 display without xdg-desktop-portal or
/// PipeWire. Pixels are interpreted through the root visual's channel masks,
/// so both LSBFirst (BGRX, typical x86) and MSBFirst (XRGB) servers convert
/// correctly. An XShm fast path can replace the per-frame GetImage copy later
/// without changing this interface.
pub struct X11Capture {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    width: u32,
    height: u32,
    depth: u8,
    lsb_first: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    timestamp_us: u64,
}

impl X11Capture {
    /// Connect to `$DISPLAY` and describe the root window. Fails closed when
    /// no display is reachable or the root visual is not 24/32-bit.
    pub fn connect() -> Result<Self, ScreenShareError> {
        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| ScreenShareError::new(format!("X11 connect failed: {e}")))?;
        // Copy everything out of the borrowed setup/screen/visual data before
        // moving `conn` into the struct (the setup borrows the connection).
        let (root, width, height, depth, lsb_first, red_mask, green_mask, blue_mask) = {
            let setup = conn.setup();
            let screen = &setup.roots[screen_num];
            let depth = screen.root_depth;
            if !matches!(depth, 24 | 32) {
                return Err(ScreenShareError::new(format!(
                    "unsupported X11 root depth {depth} (need a 24 or 32-bit visual)"
                )));
            }
            let visual = screen
                .allowed_depths
                .iter()
                .flat_map(|d| d.visuals.iter())
                .find(|v| v.visual_id == screen.root_visual)
                .ok_or_else(|| ScreenShareError::new("X11 root visual not found"))?;
            (
                screen.root,
                screen.width_in_pixels as u32,
                screen.height_in_pixels as u32,
                depth,
                setup.image_byte_order == ImageOrder::LSB_FIRST,
                visual.red_mask,
                visual.green_mask,
                visual.blue_mask,
            )
        };
        Ok(Self {
            conn,
            root,
            width,
            height,
            depth,
            lsb_first,
            red_mask,
            green_mask,
            blue_mask,
            timestamp_us: 0,
        })
    }
}

impl ScreenCapture for X11Capture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        // Refresh geometry every frame (the screen can resize); the capture
        // buffer is rebuilt only when the size actually changed.
        let geometry = self
            .conn
            .get_geometry(self.root)
            .map_err(|e| ScreenShareError::new(format!("X11 get_geometry failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 get_geometry reply failed: {e}")))?;
        let width = geometry.width as u32;
        let height = geometry.height as u32;
        if width == 0 || height == 0 {
            return Ok(None);
        }
        self.width = width;
        self.height = height;
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, width as u16, height as u16, u32::MAX)
            .map_err(|e| ScreenShareError::new(format!("X11 GetImage failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 GetImage reply failed: {e}")))?;
        let pixels = convert_zpixmap_rgba(
            &reply.data,
            width as usize,
            height as usize,
            self.depth,
            self.lsb_first,
            self.red_mask,
            self.green_mask,
            self.blue_mask,
        )?;
        let timestamp_us = self.timestamp_us;
        self.timestamp_us = self.timestamp_us.saturating_add(33_333);
        CapturedFrame::cpu(timestamp_us, width, height, PixelFormat::Rgba8, pixels).map(Some)
    }
}

/// Convert an X11 ZPixmap `GetImage` buffer into RGBA8 using the root
/// visual's channel masks. Depth 24/32 visuals pack every pixel into 32 bits
/// (the high byte is padding for depth 24); the pixel value is reassembled in
/// the server's image byte order before the masks are applied, which makes the
/// conversion correct for both LSBFirst (BGRX on x86) and MSBFirst (XRGB)
/// servers.
fn convert_zpixmap_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    depth: u8,
    lsb_first: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
) -> Result<Vec<u8>, ScreenShareError> {
    let bpp = if matches!(depth, 24 | 32) { 4 } else { 0 };
    if bpp == 0 {
        return Err(ScreenShareError::new(format!(
            "unsupported ZPixmap depth {depth}"
        )));
    }
    let expected = width * height * bpp;
    if data.len() < expected {
        return Err(ScreenShareError::new(format!(
            "X11 image buffer too small: {} bytes for {width}x{height}@{depth}",
            data.len()
        )));
    }
    if red_mask == 0 || green_mask == 0 || blue_mask == 0 {
        return Err(ScreenShareError::new("X11 visual channel masks are empty"));
    }
    let red_shift = red_mask.trailing_zeros();
    let green_shift = green_mask.trailing_zeros();
    let blue_shift = blue_mask.trailing_zeros();
    let red_max = red_mask >> red_shift;
    let green_max = green_mask >> green_shift;
    let blue_max = blue_mask >> blue_shift;
    let mut out = Vec::with_capacity(width * height * 4);
    for chunk in data.chunks_exact(bpp).take(width * height) {
        let pixel = if lsb_first {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        let r = (((pixel & red_mask) >> red_shift) as u32 * 255) / red_max;
        let g = (((pixel & green_mask) >> green_shift) as u32 * 255) / green_max;
        let b = (((pixel & blue_mask) >> blue_shift) as u32 * 255) / blue_max;
        out.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
    }
    Ok(out)
}

// ── Selection factory ────────────────────────────────────────────────────────

/// The capture source chosen by [`create_capture_source`].
pub enum ActiveCapture {
    /// A real portal/PipeWire capture with its negotiated geometry.
    Portal(LinuxPortalCapture),
    /// A direct X11 GetImage capture of the root window.
    X11(X11Capture),
    /// Synthetic fallback (demo/CI path) with the given geometry.
    TestPattern(TestPatternCapture, (u32, u32)),
}

impl ActiveCapture {
    /// Capture the next frame, if one is ready.
    pub fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        match self {
            ActiveCapture::Portal(capture) => capture.capture(),
            ActiveCapture::X11(capture) => capture.capture(),
            ActiveCapture::TestPattern(capture, _) => capture.capture(),
        }
    }

    /// Active capture geometry for codec configuration.
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            ActiveCapture::Portal(capture) => {
                capture.negotiated_size().unwrap_or((DEMO_WIDTH, DEMO_HEIGHT))
            }
            ActiveCapture::X11(capture) => (capture.width, capture.height),
            ActiveCapture::TestPattern(_, size) => *size,
        }
    }

    /// Whether the synthetic fallback is active (viewer/UI diagnostics).
    pub fn is_test_pattern(&self) -> bool {
        matches!(self, ActiveCapture::TestPattern(..))
    }

    /// Human-readable backend name for startup diagnostics.
    pub fn backend_name(&self) -> &'static str {
        match self {
            ActiveCapture::Portal(_) => "portal",
            ActiveCapture::X11(_) => "x11",
            ActiveCapture::TestPattern(..) => "test-pattern",
        }
    }
}

const DEMO_WIDTH: u32 = 640;
const DEMO_HEIGHT: u32 = 360;
const DEMO_FPS: u32 = 15;

/// Try the real platform capture first, then the direct X11 backend, falling
/// back to the synthetic test pattern. `force_fallback` is a test hook;
/// production callers pass `false`.
pub async fn create_capture_source(force_fallback: bool) -> ActiveCapture {
    #[cfg(target_os = "linux")]
    {
        if !force_fallback {
            if let Ok(capture) = LinuxPortalCapture::connect().await {
                return ActiveCapture::Portal(capture);
            }
            if let Ok(capture) = X11Capture::connect() {
                return ActiveCapture::X11(capture);
            }
        }
    }
    ActiveCapture::TestPattern(
        TestPatternCapture::new(DEMO_WIDTH, DEMO_HEIGHT).unwrap(),
        (DEMO_WIDTH, DEMO_HEIGHT),
    )
}

/// Active capture dimensions used for codec configuration and pointer mapping.
pub fn capture_dimensions(capture: &ActiveCapture) -> (u32, u32) {
    capture.dimensions()
}

/// Default frame rate for the real capture path (15 fps keeps encode cost low).
pub const CAPTURE_FPS: u32 = DEMO_FPS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_ends_selection() {
        let mut c = PortalCapture::new(2).unwrap();
        c.begin_selection().unwrap();
        c.cancel();
        assert_eq!(c.state(), PortalState::Ended);
    }

    #[test]
    fn format_pod_builder_produces_parsable_object() {
        let pod = build_format_params();
        assert!(pod.len() > 32);
        // Header: body size + Object type.
        assert_eq!(u32::from_le_bytes(pod[4..8].try_into().unwrap()), SPA_TYPE_Object);
        let parsed = parse_format_pod(pod.as_ptr() as *const c_void);
        // The pod advertises BGRx first; parse returns the first value.
        let (width, height, pixel_format) = parsed.expect("pod must parse");
        assert_eq!((width, height), (640, 360));
        assert_eq!(pixel_format, PixelFormat::Bgra8);
    }

    #[test]
    fn create_capture_source_falls_back_to_test_pattern() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut capture = rt.block_on(create_capture_source(true));
        assert!(capture.is_test_pattern());
        assert_eq!(capture.dimensions(), (DEMO_WIDTH, DEMO_HEIGHT));
        let frame = capture.capture().unwrap().unwrap();
        assert_eq!(frame.pixel_format, PixelFormat::Rgba8);
        assert_eq!(frame.pixels.len(), (DEMO_WIDTH * DEMO_HEIGHT * 4) as usize);
    }

    #[test]
    fn portal_capture_rejects_frames_outside_streaming() {
        let mut c = PortalCapture::new(2).unwrap();
        let frame = CapturedFrame::cpu(0, 2, 2, PixelFormat::Bgra8, vec![0; 16]).unwrap();
        assert!(c.push_pipewire_frame(frame).is_err());
        c.begin_selection().unwrap();
        c.source_selected().unwrap();
        let frame = CapturedFrame::cpu(0, 2, 2, PixelFormat::Bgra8, vec![0; 16]).unwrap();
        assert!(c.push_pipewire_frame(frame).is_ok());
        assert_eq!(c.state(), PortalState::Streaming);
    }

    #[test]
    fn parse_rejects_non_object_pod() {
        assert!(parse_format_pod(std::ptr::null()).is_none());
    }

    #[test]
    fn zpixmap_lsb_first_bgrx_converts_to_rgba() {
        // Depth 24, LSBFirst (x86): pixel bytes are B,G,R,X. Two pixels:
        // (0x30,0x20,0x10) → RGB(0x10,0x20,0x30) and (0xAA,0xBB,0xCC) → RGB(0xCC,0xBB,0xAA).
        let data = [0x30, 0x20, 0x10, 0x00, 0xAA, 0xBB, 0xCC, 0x00];
        let out = convert_zpixmap_rgba(
            &data, 2, 1, 24, true, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF,
        )
        .unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0x30, 255, 0xCC, 0xBB, 0xAA, 255]);
    }

    #[test]
    fn zpixmap_msb_first_xrgb_converts_to_rgba() {
        // Depth 24, MSBFirst (big-endian): pixel bytes are X,R,G,B.
        let data = [0x00, 0x10, 0x20, 0x30];
        let out = convert_zpixmap_rgba(
            &data, 1, 1, 24, false, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF,
        )
        .unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0x30, 255]);
    }

    #[test]
    fn zpixmap_respects_nonstandard_channel_masks() {
        // 5-6-5 masks (R=0xF800, G=0x07E0, B=0x001F). A pixel with
        // R=0x1F,G=0x3F,B=0x1F packs as 0xF800|0x07E0|0x001F = 0xFFFF;
        // LSBFirst bytes are [0xFF, 0xFF, 0x00, 0x00].
        let data = [0xFF, 0xFF, 0x00, 0x00];
        let out = convert_zpixmap_rgba(
            &data, 1, 1, 24, true, 0x0000_F800, 0x0000_07E0, 0x0000_001F,
        )
        .unwrap();
        // R: 31/31*255 = 255; G: 63/63*255 = 255; B: 255.
        assert_eq!(out, vec![255, 255, 255, 255]);
    }

    #[test]
    fn zpixmap_rejects_unsupported_depth_and_short_buffer() {
        assert!(
            convert_zpixmap_rgba(&[0; 8], 2, 1, 16, true, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF)
                .is_err()
        );
        assert!(
            convert_zpixmap_rgba(&[0; 4], 2, 1, 24, true, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF)
                .is_err()
        );
        assert!(
            convert_zpixmap_rgba(&[0; 8], 2, 1, 24, true, 0, 0, 0x0000_00FF).is_err()
        );
    }
}
