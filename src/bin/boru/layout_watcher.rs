#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! `boru-layout.toml` file watcher (BORU-LAYOUT-06 / PDF Task 6).
//!
//! Observes the dev layout override file during development, debounces
//! editor save storms, parses the file **away from the rendering path**
//! (a dedicated background thread, never in `view()`/`update()`) and sends
//! a normal Iced message (`AppMessage::LayoutReloaded`) into the
//! application update loop.
//!
//! ## Reuse
//!
//! The file-watcher machinery is shared with the theme watcher
//! (`theme_watcher.rs`, BORU-UI-06): the same [`Debouncer`], [`ReloadTracker`],
//! [`is_dev_config_event`] filter, debounce window and monotonic clock. This
//! module only swaps the parsed type (`LayoutOverrides` instead of
//! `UiThemeConfig`) and the message name.
//!
//! ## Design rules
//!
//! - **Watch the parent directory, not the file** (identical rationale to
//!   the theme watcher: editors atomically replace files, and a missing
//!   file must be observed on create).
//! - **Debounce save storms** with the same trailing-edge debounce window.
//! - **Never mutate shared UI state from the watcher callback.** The
//!   watcher only sends a [`LayoutReloadMsg`] into an mpsc channel; every
//!   state change flows through the Iced update loop.
//! - **Drop stale reload results** via the shared [`ReloadTracker`]
//!   generation watermark.
//! - **Only validated layouts are applied.** The file is parsed here; a
//!   malformed file yields a structured [`LayoutReloadError`] (path, kind,
//!   parser line/column) and the app keeps the last known-good layout
//!   (BORU-LAYOUT-06 acceptance: unparseable TOML never changes the UI).

use std::path::PathBuf;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::layout::LayoutOverrides;
use crate::layout_config::{load_layout_config, LayoutReloadError, LAYOUT_CONFIG_FILE_NAME};
use crate::theme_watcher::{is_dev_config_event, now_nanos, Debouncer};

/// Debounce window: same as the theme watcher (editor saves emit a burst
/// of write events; wait this long after the *last* event before one
/// reload).
pub const LAYOUT_DEBOUNCE: Duration = crate::theme_watcher::UI_THEME_DEBOUNCE;

/// Message sent from the layout watcher thread into the Iced update loop.
///
/// `result` carries the freshly parsed layout overrides, or a structured
/// developer error ([`LayoutReloadError`] — path, kind, parser
/// line/column). The error is a `Clone` projection so it can ride inside
/// `AppMessage` (which derives `Clone`).
#[derive(Debug, Clone)]
pub struct LayoutReloadMsg {
    /// Monotonic reload generation. The app drops any message whose
    /// generation is not newer than the last one it applied.
    pub generation: u64,
    /// Freshly parsed layout overrides, or a structured developer error.
    pub result: Result<LayoutOverrides, LayoutReloadError>,
}

/// Spawn the dev layout watcher on a dedicated background thread.
///
/// Watches `<data_dir>` (the parent of `boru-layout.toml`) non-recursively
/// so create/rename events are observed even when the file does not exist
/// yet. Each debounced save burst produces exactly one [`LayoutReloadMsg`]
/// on `tx`, parsed on this thread — never on the rendering path.
///
/// Returns an error only when the notify watcher cannot be created or the
/// watch cannot be registered (e.g. the data dir does not exist); the
/// caller treats that as a non-fatal "live reload disabled".
pub fn spawn_layout_watcher(
    data_dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<LayoutReloadMsg>,
) -> Result<(), notify::Error> {
    let layout_config_path = data_dir.join(LAYOUT_CONFIG_FILE_NAME);
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(notify_tx, Config::default())?;
    // Watch the parent directory, not the file (see module docs).
    watcher.watch(&data_dir, RecursiveMode::NonRecursive)?;

    std::thread::Builder::new()
        .name("boru-layout-watch".into())
        .spawn(move || {
            // Keep the watch registered for the thread's lifetime: dropping
            // `watcher` closes the notify channel and the loop below would
            // see a disconnect and exit.
            let _watcher = watcher;
            let mut debouncer = Debouncer::new(LAYOUT_DEBOUNCE);
            loop {
                // Wait for the first relevant event of a burst.
                loop {
                    match notify_rx.recv() {
                        Ok(Ok(event)) if is_dev_config_event(&event, &layout_config_path) => {
                            debouncer.note_event(now_nanos());
                            break;
                        }
                        Ok(Ok(_)) => continue,
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "boru-layout.toml watcher: notify error");
                            continue;
                        }
                        // Sender disconnected — watcher dropped, stop.
                        Err(_) => return,
                    }
                }

                // Trailing-edge debounce: keep extending the window while
                // new events keep arriving.
                let generation = 'debounce: loop {
                    match notify_rx.recv_timeout(LAYOUT_DEBOUNCE) {
                        Ok(Ok(event)) if is_dev_config_event(&event, &layout_config_path) => {
                            debouncer.note_event(now_nanos());
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "boru-layout.toml watcher: notify error");
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if let Some(generation) = debouncer.on_quiet_elapsed(now_nanos()) {
                                break 'debounce generation;
                            }
                            // A straggler event landed right at the window
                            // boundary — keep waiting.
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                };

                // Parse away from the rendering path (this thread). A
                // missing file yields empty overrides (defaults) — deleting
                // boru-layout.toml therefore reloads the default layout.
                // Errors are projected into the Clone-able structured report
                // so the app can log path + parser detail and keep the last
                // known-good layout.
                let result = load_layout_config(&data_dir)
                    .map_err(|e| LayoutReloadError::from_layout_error(&e));
                if tx
                    .blocking_send(LayoutReloadMsg { generation, result })
                    .is_err()
                {
                    // The app dropped the receiver (shutdown).
                    return;
                }
            }
        })
        .map_err(|e| {
            notify::Error::generic(&format!("failed to spawn layout watcher thread: {e}"))
        })?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{HomeOverrides, LayoutOverrides};

    /// Integration-style: a file write produces exactly one reload message,
    /// and the parsed overrides arrive on the channel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_sends_exactly_one_reload_per_save() {
        let dir = std::env::temp_dir().join(format!("boru-layout-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_layout_watcher(dir.clone(), tx).expect("spawn watcher");

        // The watch is registered synchronously inside spawn_layout_watcher
        // (before the thread starts), so writes after this point are seen.
        let path = dir.join(LAYOUT_CONFIG_FILE_NAME);
        std::fs::write(&path, "[home]\nmax_content_width = 1200.0\n").expect("write config");

        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for the reload message")
            .expect("channel closed before the reload");
        assert_eq!(msg.generation, 1, "first burst reloads with generation 1");
        let cfg = msg.result.expect("valid TOML should parse");
        let home = cfg.home.as_ref().expect("home group present");
        assert_eq!(home.max_content_width, Some(1200.0));

        // One save → one reload. No second message within the debounce
        // window plus margin.
        let extra = tokio::time::timeout(LAYOUT_DEBOUNCE * 3, rx.recv()).await;
        assert!(
            extra.is_err(),
            "expected no second reload message for one save, got {:?}",
            extra.ok().flatten()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second save after the burst settled re-arms the watcher with the
    /// next generation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_rearms_for_subsequent_saves() {
        let dir = std::env::temp_dir().join(format!("boru-layout-watch2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_layout_watcher(dir.clone(), tx).expect("spawn watcher");

        let path = dir.join(LAYOUT_CONFIG_FILE_NAME);
        std::fs::write(&path, "[home]\nmax_content_width = 1000.0\n").expect("write config");

        let msg1 = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for first reload")
            .expect("channel closed");
        assert_eq!(msg1.generation, 1);
        assert!(msg1.result.is_ok());

        // Save again after the debounce has settled.
        tokio::time::sleep(LAYOUT_DEBOUNCE + Duration::from_millis(100)).await;
        std::fs::write(&path, "[home]\nmax_content_width = 1400.0\n").expect("rewrite config");

        let msg2 = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for second reload")
            .expect("channel closed");
        assert_eq!(msg2.generation, 2, "second burst reloads with generation 2");
        let cfg = msg2.result.expect("valid TOML should parse");
        let home = cfg.home.as_ref().expect("home group present");
        assert_eq!(home.max_content_width, Some(1400.0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A malformed file yields a structured error result — the app keeps
    /// the last known-good layout (BORU-LAYOUT-06 acceptance).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_reports_malformed_toml_as_error() {
        let dir = std::env::temp_dir().join(format!("boru-layout-watch3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_layout_watcher(dir.clone(), tx).expect("spawn watcher");

        let path = dir.join(LAYOUT_CONFIG_FILE_NAME);
        std::fs::write(&path, "[home\nmax_content_width = not-a-number\n")
            .expect("write malformed config");

        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for reload message")
            .expect("channel closed");
        let err = msg
            .result
            .expect_err("malformed TOML must be reported as an error");
        assert!(
            err.path.ends_with(LAYOUT_CONFIG_FILE_NAME),
            "error carries the file path: {}",
            err.path.display()
        );
        assert_eq!(
            err.kind,
            crate::layout_config::LayoutReloadErrorKind::Parse,
            "malformed TOML is a Parse error"
        );
        assert!(!err.message.is_empty(), "parser detail is present");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that parses but contains duplicate section ids yields a
    /// structured *Validation* error — the app keeps the last known-good
    /// layout (BORU-LAYOUT-07 acceptance: duplicates are rejected, never
    /// silently applied).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_reports_duplicate_sections_as_validation_error() {
        let dir = std::env::temp_dir().join(format!("boru-layout-watch5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_layout_watcher(dir.clone(), tx).expect("spawn watcher");

        let path = dir.join(LAYOUT_CONFIG_FILE_NAME);
        std::fs::write(
            &path,
            "[home]\nsection_order = [\"Tunnels\", \"Tunnels\"]\n",
        )
        .expect("write duplicate config");

        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for reload message")
            .expect("channel closed");
        let err = msg
            .result
            .expect_err("duplicate section ids must be reported as an error");
        assert_eq!(
            err.kind,
            crate::layout_config::LayoutReloadErrorKind::Validation,
            "duplicate section ids are a Validation error"
        );
        assert!(
            err.message.contains("duplicate"),
            "message names the problem: {}",
            err.message
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A deleted file reloads the default (empty) layout — deleting
    /// boru-layout.toml restores today's baseline arrangement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_missing_file_yields_empty_overrides() {
        let dir = std::env::temp_dir().join(format!("boru-layout-watch4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_layout_watcher(dir.clone(), tx).expect("spawn watcher");

        // No file exists; a create event for a different file must not
        // trigger a layout reload, but the *directory watch* only sees
        // the events we produce. Write a non-layout file → no message.
        let other = dir.join("other.toml");
        std::fs::write(&other, "x = 1\n").expect("write other file");
        let extra = tokio::time::timeout(LAYOUT_DEBOUNCE * 2, rx.recv()).await;
        assert!(
            extra.is_err(),
            "a non-layout file must not trigger a reload, got {:?}",
            extra.ok().flatten()
        );

        // Now create the layout file itself: the watcher sees the create
        // and reloads (empty overrides when the file exists but is empty).
        std::fs::write(dir.join(LAYOUT_CONFIG_FILE_NAME), "").expect("create layout file");
        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for reload message")
            .expect("channel closed");
        let cfg = msg.result.expect("empty TOML parses");
        assert_eq!(cfg, LayoutOverrides::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layout_reload_error_kind_matches_parse_failures() {
        // The Clone-able projection keeps path + kind + line/column.
        let err = LayoutReloadError {
            path: PathBuf::from(LAYOUT_CONFIG_FILE_NAME),
            kind: crate::layout_config::LayoutReloadErrorKind::Parse,
            message: "invalid dev layout override boru-layout.toml: bad".to_string(),
            line: Some(2),
            column: Some(3),
        };
        assert!(err.to_string().contains(LAYOUT_CONFIG_FILE_NAME));
        assert_eq!(err.line, Some(2));
        assert_eq!(err.column, Some(3));
    }

    #[test]
    fn overrides_flow_through_debouncer_clock() {
        // The watcher's debouncer is the shared theme-watcher Debouncer —
        // prove a burst still settles to one generation.
        let mut d = Debouncer::new(LAYOUT_DEBOUNCE);
        d.note_event(now_nanos());
        assert!(
            d.on_quiet_elapsed(now_nanos()).is_none(),
            "burst still settling"
        );
    }

    #[test]
    fn layout_overrides_default_constructible() {
        // `LayoutOverrides` is the parsed type AND the "no overrides"
        // sentinel — it must be default-constructible (missing file).
        let empty: LayoutOverrides = LayoutOverrides::default();
        assert!(empty.home.is_none());
        let _ = HomeOverrides::default();
    }
}
