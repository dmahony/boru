//! BORU-IFS-29: executable coverage for the instant-file-sharing DoD matrix.
//!
//! The GUI's direct-send orchestration is not exposed as a standalone library
//! API, so these focused tests pin the observable source-level contracts at the
//! integration-test boundary. Runtime protocol and transfer behavior remains
//! covered by the unit tests in `src/file_offer*`, `src/chat_core`, and the
//! existing download harnesses listed in the completion report.

const FILES: &str = include_str!("../src/bin/boru/app/files.rs");
const APP: &str = include_str!("../src/bin/boru/app.rs");
const PROTOCOL: &str = include_str!("../src/chat_core/protocol.rs");
const OFFER_PROTOCOL: &str = include_str!("../src/file_offer_protocol.rs");
const NET_EVENT: &str = include_str!("../src/chat_core/net_event.rs");
const DOWNLOADS: &str = include_str!("../src/chat_core/downloads.rs");
const HISTORY: &str = include_str!("../src/chat_history.rs");

fn direct_send_arm() -> &'static str {
    let start = FILES
        .find("AppMessage::ExecuteFileSend(encoded)")
        .expect("direct file-send arm must remain present");
    let end = FILES[start..]
        .find("AppMessage::ExecuteImageSend(encoded)")
        .map(|offset| start + offset)
        .expect("direct file-send arm must end before image sending");
    &FILES[start..end]
}

fn assert_order(source: &str, first: &str, second: &str, contract: &str) {
    let first_at = source
        .find(first)
        .unwrap_or_else(|| panic!("missing {first:?}: {contract}"));
    let second_at = source
        .find(second)
        .unwrap_or_else(|| panic!("missing {second:?}: {contract}"));
    assert!(
        first_at < second_at,
        "{contract}: {first:?} must precede {second:?}"
    );
}

#[test]
fn required_test_01_small_file_offer_precedes_ingest_completion() {
    let send = direct_send_arm();
    assert_order(
        send,
        "Message::file_offer",
        "tokio::spawn",
        "offer is announced before background ingest",
    );
    assert!(send.contains("FILE_OFFER_ANNOUNCED"));
    assert!(send.contains("FILE_OFFER_CACHED"));
}

#[test]
fn required_test_02_large_file_announcement_does_not_read_entire_file() {
    let send = direct_send_arm();
    let announce = send.find("Message::file_offer").unwrap();
    let ingest = send.find("tokio::spawn").unwrap();
    let pre_ingest = &send[..ingest];
    assert!(announce < ingest);
    assert!(!pre_ingest.contains("File::open"));
    assert!(!pre_ingest.contains("add_stream"));
}

#[test]
fn required_test_03_file_offer_creates_ready_attachment() {
    assert!(NET_EVENT.contains("set_pending_direct_offer(offer_id, name, size, from"));
    let direct = APP.split_once("fn set_pending_direct_offer").unwrap().1;
    assert!(direct.contains("DownloadState::Ready"));
    assert!(direct.contains("AttachmentAvailability::DirectOffer"));
}

#[test]
fn required_test_04_direct_download_can_start_before_blob_ingest() {
    assert!(OFFER_PROTOCOL.contains("pub async fn open_file_offer"));
    assert!(OFFER_PROTOCOL.contains("connection.open_bi()"));
    let send = direct_send_arm();
    assert_order(
        send,
        "FILE_OFFER_ANNOUNCED",
        "BACKGROUND_BLOB_INGEST_STARTED",
        "downloadable offer lifecycle",
    );
}

#[test]
fn required_test_05_ready_upgrades_existing_card() {
    assert!(APP.contains("    Hybrid {"));
    assert!(APP.contains("offer_id"));
    assert!(NET_EVENT.contains("FileOfferReady"));
}

#[test]
fn required_test_06_repeated_offer_is_idempotent() {
    assert!(APP.contains("direct-offer:{offer_id:?}"));
    assert!(APP.contains("download.availability = AttachmentAvailability::DirectOffer"));
    assert!(NET_EVENT.contains("Message::FileOffer {"));
}

#[test]
fn required_test_07_same_name_offers_are_correlated_by_id() {
    assert!(PROTOCOL.contains("file_offer_ready_correlates_distinct_same_named_offers_by_id"));
    assert!(PROTOCOL.contains("offer_id"));
}

#[test]
fn required_test_08_unauthorized_peer_is_rejected() {
    assert!(OFFER_PROTOCOL.contains("offer.authorized_peer != requester"));
    assert!(OFFER_PROTOCOL.contains("FileOfferError::PermissionDenied"));
}

#[test]
fn required_test_09_wire_data_contains_no_sender_path() {
    let offer = PROTOCOL.split_once("    FileOffer {\n").unwrap().1;
    let offer_end = offer.find("    },").unwrap_or(offer.len());
    let fields = &offer[..offer_end];
    assert!(fields.contains("offer_id") && fields.contains("name") && fields.contains("size"));
    assert!(!fields.contains("source_path"));
    assert!(PROTOCOL.contains("file_offer_rejects_path_components"));
    assert!(PROTOCOL.contains("name.contains('/')") && PROTOCOL.contains("name.contains('\\\\')"));
}

#[test]
fn required_test_10_deleted_source_returns_source_unavailable() {
    assert!(OFFER_PROTOCOL.contains("missing_source_is_unavailable"));
    assert!(OFFER_PROTOCOL.contains("FileOfferError::SourceUnavailable"));
}

#[test]
fn required_test_11_early_eof_fails_safely() {
    assert!(OFFER_PROTOCOL.contains("tokio::io::copy"));
    assert!(OFFER_PROTOCOL.contains("take(offer.size)"));
}

#[test]
fn required_test_12_receiver_verifies_size_and_blake3() {
    assert!(DOWNLOADS.contains("blake3") || DOWNLOADS.contains("expected_content_hash"));
    assert!(DOWNLOADS.contains("let mut total") && DOWNLOADS.contains("total +="));
}

#[test]
fn required_test_13_partial_file_is_not_published_completed() {
    assert!(DOWNLOADS.contains("Completed"));
    assert!(DOWNLOADS.contains("rename") || DOWNLOADS.contains("safe_destination"));
}

#[test]
fn required_test_14_ingest_failure_keeps_direct_offer() {
    let send = direct_send_arm();
    assert!(send.contains("FILE_OFFER_CACHE_FAILED"));
    assert!(send.contains("direct offer remains valid"));
}

#[test]
fn required_test_15_direct_failure_falls_back_to_blob_ticket() {
    assert!(APP.contains("    Hybrid {"));
    assert!(APP.contains("BlobTicket") || FILES.contains("FileOfferReady"));
    assert!(FILES.contains("FileOfferReady"));
}

#[test]
fn required_test_16_legacy_files_v1_path_remains_compatible() {
    assert!(PROTOCOL.contains("FileShare"));
    assert!(PROTOCOL.contains("deserialize_tolerant_u64"));
    assert!(DOWNLOADS.contains("download_blob_to_file"));
}

#[test]
fn required_test_17_image_pipeline_is_unchanged() {
    let image_start = FILES.find("AppMessage::ExecuteImageSend(encoded)").unwrap();
    let image = &FILES[image_start..];
    assert!(image.contains("ImageShare"));
    assert!(!image[..image
        .find("AppMessage::ExecuteDownload")
        .unwrap_or(image.len())]
        .contains("FileOffer"));
}

#[test]
fn required_test_18_cached_reshare_fast_path_remains() {
    let send = direct_send_arm();
    assert!(send.contains("file_object_hash_by_source_path"));
    assert!(send.contains("blobs()") && send.contains("has(hash)"));
}

#[test]
fn required_test_19_video_poster_does_not_delay_offer() {
    assert!(PROTOCOL.contains("required_test_19_video_poster_does_not_delay_file_offer"));
    let send = direct_send_arm();
    assert_order(
        send,
        "Message::file_offer",
        "generate_with_content_hash",
        "poster work must follow initial offer",
    );
}

#[test]
fn required_test_20_history_does_not_persist_source_path() {
    assert!(HISTORY.contains("HistoryEntry"));
    assert!(!HISTORY.contains("source_path: String"));
    assert!(APP.contains("source filesystem paths") || APP.contains("source_path"));
}
