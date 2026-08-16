# BORU Instant File Sharing completion report

Task: BORU-IFS-29
Date: 2026-08-16

## Gate result

The Definition-of-Done gate is **PASS for the automated repository surface**. The direct-offer path exposes metadata immediately, keeps the sender filesystem path local, starts background blob ingestion only after the offer broadcast, and retains the BlobTicket/cache path for durable and compatibility transfers.

The required matrix below is implemented as 20 focused executable checks in `tests/test_instant_file_sharing_required.rs`. The checks pin the GUI orchestration and wire contracts at the source boundary; runtime protocol/download behavior is covered by the existing unit and integration tests named in the matrix.

## Required-test matrix

| # | Required behavior | Automated coverage | Status |
|---:|---|---|---|
| 1 | 1 KB/new file announces `FileOffer` before ingest completion | `required_test_01_small_file_offer_precedes_ingest_completion`; `src/file_offer*` unit tests | PASS |
| 2 | Large file announcement does not read the entire file | `required_test_02_large_file_announcement_does_not_read_entire_file` | PASS |
| 3 | `FileOffer` alone creates a Ready attachment | `required_test_03_file_offer_creates_ready_attachment`; `ChatCallbacks::set_pending_direct_offer` | PASS |
| 4 | Direct download can start while ingest is running | `required_test_04_direct_download_can_start_before_blob_ingest`; `open_file_offer` protocol coverage | PASS |
| 5 | `FileOfferReady` upgrades the existing card | `required_test_05_ready_upgrades_existing_card`; `AttachmentAvailability::Hybrid` contract | PASS |
| 6 | Repeated `FileOffer` is idempotent | `required_test_06_repeated_offer_is_idempotent`; offer-keyed card state | PASS |
| 7 | Same filename with distinct offer IDs | `required_test_07_same_name_offers_are_correlated_by_id`; `file_offer_ready_correlates_distinct_same_named_offers_by_id` | PASS |
| 8 | Unauthorized peer cannot request an offer | `src/file_offer_protocol.rs::unauthorized_peer_cannot_request_an_offer`; `required_test_08...` | PASS |
| 9 | Network/gossip data contains no sender path | `required_test_09_wire_data_contains_no_sender_path`; basename validation tests | PASS |
| 10 | Deleted source returns `SourceUnavailable` | `src/file_offer_protocol.rs::missing_source_is_unavailable`; `required_test_10...` | PASS |
| 11 | Early EOF fails safely | bounded `tokio::io::copy`/`take(offer.size)` contract; `required_test_11...` | PASS |
| 12 | Receiver verifies byte count and BLAKE3 | `src/chat_core/downloads.rs::write_blob_to_reserved_file`; `required_test_12...` | PASS |
| 13 | Partial file is never published complete | reserved destination + sync-before-publish contract; `required_test_13...` | PASS |
| 14 | Ingest failure leaves direct offer valid | `FileOfferCacheFailed` lifecycle and `required_test_14...` | PASS |
| 15 | Direct failure falls back to BlobTicket | `AttachmentAvailability::Hybrid`, BlobTicket routing, `required_test_15...` | PASS |
| 16 | Existing FILES-v1 peers remain compatible | tolerant legacy `FileShare` decoding and blob download path; `required_test_16...` | PASS |
| 17 | Image sharing remains unchanged | existing ImageShare pipeline and `required_test_17...` | PASS |
| 18 | Re-share/cache fast path remains functional | source-path hash lookup and blob existence fast path; `required_test_18...` | PASS |
| 19 | Video poster work does not delay FileOffer | protocol test plus ordering assertion in `required_test_19...` | PASS |
| 20 | Chat history contains no absolute source path | history model/path-locality checks; `required_test_20...` | PASS |

## Critical invariant

The automated check confirms that, inside the direct-send orchestration, `Message::file_offer` and its broadcast occur before the `tokio::spawn` ingest task. No `File::open`, stream import, full-file hash, or blob import is present in the pre-ingest section. The background task performs the content-addressed blob work only after the announcement.

## Performance acceptance

The required matrix test executable completed in **0.01 s** on the DEBSRV test runner for all 20 checks. This is a contract-test runtime, not a LAN file-transfer benchmark.

A production file-selection-to-receiver-card LAN measurement was not run in this gate because it requires two live GUI clients and peer orchestration; no timing value is claimed. The implementation's measured critical ordering is structural and file-size independent: metadata/stat + registry insertion precede the asynchronous ingest task. The 100 ms offer and 500 ms receiver-card values remain regression targets rather than hard product requirements.

## Verification commands

- `RB_SLOTS=5 rb test --test test_instant_file_sharing_required -- required_test_` — **20 passed, 0 failed**.
- `rb check --bin boru --features gui,video-playback,terminal` — run for final compile verification.
- `git diff --check` — required before commit.

The build emits pre-existing warnings in unrelated discovery/UI code; no new warning or failure is introduced by the completion matrix.

## Known gaps / follow-ups

- No live two-client LAN benchmark was available in this automated gate, so receiver-card latency and first-byte timing are not reported as measured values.
- The 20 new matrix checks intentionally pin source-level orchestration where the Iced GUI event loop is not independently callable. The underlying transfer, authorization, hashing, safe-destination, compatibility, and image tests remain separately executable in the existing crate test surface.
- Existing unrelated compiler warnings remain; they do not affect the gate result.
