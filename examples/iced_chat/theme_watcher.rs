//! `boru-ui.toml` file watcher (BORU-UI-06 / PDF Task 6).
//!
//! Observes the dev theme override file during development, debounces
//! editor save storms, parses the file **away from the rendering path**
//! (a dedicated background thread, never in `view()`/`update()`) and sends
//! a normal Iced message (`AppMessage::UiThemeReloaded`) into the
//! application update loop.
//!
//! ## Design rules
//!
//! - **Watch the parent directory**, not the file: `notify` drops a
//!   file-level watch when an editor atomically replaces the file
//!   (temp-write + rename), and a missing file would never be observed on
//!   create. Watching `<data_dir>` non-recursively covers write / create /
//!   rename / remove of `boru-ui.toml`.
//! - **Debounce save storms.** A single editor save can emit many events
//!   (create temp, write temp, rename, metadata, …). Only the *trailing
//!   edge* of a burst (no new event for [`UI_THEME_DEBOUNCE`]) triggers
//!   one reload.
//! - **Never mutate shared UI state from the watcher callback.** The
//!   watcher only sends a [`UiThemeReloadMsg`] into an mpsc channel; every
//!   state change flows through the Iced update loop.
//! - **Drop stale reload results.** Each fired reload carries an increasing
//!   `generation`; the app-side [`ReloadTracker`] drops any result older
//!   than the last one it accepted (a slow parse racing a newer save).
//!
//! Applying the parsed config to the live theme is BORU-UI-07's job; this
//! module only delivers + tracks reloads.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::theme_config::{load_ui_theme_config, UiThemeConfig, UI_CONFIG_FILE_NAME};

/// Debounce window: editor saves emit a burst of write events; wait this
/// long after the *last* event before triggering one reload.
pub const UI_THEME_DEBOUNCE: Duration = Duration::from_millis(300);

/// Message sent from the watcher thread into the Iced update loop.
///
/// `result` carries the freshly parsed config, or a developer-facing error
/// string (the typed [`UiThemeConfigError`](crate::theme_config::UiThemeConfigError)
/// is not `Clone`, and `AppMessage` derives `Clone`).
#[derive(Debug, Clone)]
pub struct UiThemeReloadMsg {
    /// Monotonic reload generation. The app drops any message whose
    /// generation is not newer than the last one it applied.
    pub generation: u64,
    /// Freshly parsed config, or an error description.
    pub result: Result<UiThemeConfig, String>,
}

/// Filter: is this notify event relevant to `boru-ui.toml`?
///
/// Only write / create / rename / remove events matter — access and pure
/// metadata events are noise. Paths are matched by file name because a
/// rename event carries both the old and the new path.
pub fn is_ui_config_event(event: &Event, ui_config_path: &Path) -> bool {
    let relevant_kind = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Modify(notify::event::ModifyKind::Data(_))
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
            | EventKind::Remove(_)
    );
    if !relevant_kind {
        return false;
    }
    let Some(file_name) = ui_config_path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    event
        .paths
        .iter()
        .any(|p| p.file_name().and_then(|n| n.to_str()) == Some(file_name))
}

/// Pure trailing-edge debounce for file events.
///
/// [`Debouncer::note_event`] records the most recent event time;
/// [`Debouncer::on_quiet_elapsed`] reports when the burst has settled (no
/// event within the debounce window) and hands out the next reload
/// generation. Timestamps are monotonic nanoseconds so the logic is
/// unit-testable without wall clocks.
#[derive(Debug)]
pub struct Debouncer {
    debounce: Duration,
    /// Monotonic nanosecond timestamp of the most recent file event.
    last_event_ns: Option<u64>,
    /// Generation for the next fired reload (1-based).
    next_generation: u64,
}

impl Debouncer {
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            last_event_ns: None,
            next_generation: 1,
        }
    }

    /// Record a file event at `now_ns` (monotonic nanoseconds).
    pub fn note_event(&mut self, now_ns: u64) {
        self.last_event_ns = Some(now_ns);
    }

    /// Called when the debounce timer elapses at `now_ns`. Returns the
    /// generation of the reload to fire if the burst has settled (the last
    /// event is older than the debounce window), or `None` if a newer event
    /// is still inside the window (keep waiting).
    pub fn on_quiet_elapsed(&mut self, now_ns: u64) -> Option<u64> {
        let last = self.last_event_ns?;
        if now_ns.saturating_sub(last) < self.debounce.as_nanos() as u64 {
            return None;
        }
        self.last_event_ns = None;
        let generation = self.next_generation;
        self.next_generation += 1;
        Some(generation)
    }
}

/// Tracks which reload generation the app has already accepted so stale
/// reload results (a save that raced a newer one) are dropped.
#[derive(Debug, Clone, Default)]
pub struct ReloadTracker {
    last_applied_generation: u64,
}

impl ReloadTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when `generation` is newer than the last accepted one.
    pub fn should_apply(&self, generation: u64) -> bool {
        generation > self.last_applied_generation
    }

    /// Record that `generation` was accepted (monotonic — never goes back).
    pub fn mark_applied(&mut self, generation: u64) {
        self.last_applied_generation = self.last_applied_generation.max(generation);
    }

    /// Last generation accepted (for tests / diagnostics).
    pub fn last_applied(&self) -> u64 {
        self.last_applied_generation
    }
}

/// Monotonic nanosecond timestamp, stable within one process.
fn now_nanos() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    start.elapsed().as_nanos() as u64
}

/// Spawn the dev theme watcher on a dedicated background thread.
///
/// Watches `<data_dir>` (the parent of `boru-ui.toml`) non-recursively so
/// create/rename events are observed even when the file does not exist yet.
/// Each debounced save burst produces exactly one [`UiThemeReloadMsg`] on
/// `tx`, parsed on this thread — never on the rendering path.
///
/// Returns an error only when the notify watcher cannot be created or the
/// watch cannot be registered (e.g. the data dir does not exist); the
/// caller treats that as a non-fatal "live reload disabled".
pub fn spawn_ui_theme_watcher(
    data_dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<UiThemeReloadMsg>,
) -> Result<(), notify::Error> {
    let ui_config_path = data_dir.join(UI_CONFIG_FILE_NAME);
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(notify_tx, Config::default())?;
    // Watch the parent directory, not the file (see module docs).
    watcher.watch(&data_dir, RecursiveMode::NonRecursive)?;

    std::thread::Builder::new()
        .name("boru-ui-theme-watch".into())
        .spawn(move || {
            // Keep the watch registered for the thread's lifetime: dropping
            // `watcher` closes the notify channel and the loop below would
            // see a disconnect and exit.
            let _watcher = watcher;
            let mut debouncer = Debouncer::new(UI_THEME_DEBOUNCE);
            loop {
                // Wait for the first relevant event of a burst.
                loop {
                    match notify_rx.recv() {
                        Ok(Ok(event)) if is_ui_config_event(&event, &ui_config_path) => {
                            debouncer.note_event(now_nanos());
                            break;
                        }
                        Ok(Ok(_)) => continue,
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "boru-ui.toml watcher: notify error");
                            continue;
                        }
                        // Sender disconnected — watcher dropped, stop.
                        Err(_) => return,
                    }
                }

                // Trailing-edge debounce: keep extending the window while
                // new events keep arriving.
                let generation = 'debounce: loop {
                    match notify_rx.recv_timeout(UI_THEME_DEBOUNCE) {
                        Ok(Ok(event)) if is_ui_config_event(&event, &ui_config_path) => {
                            debouncer.note_event(now_nanos());
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "boru-ui.toml watcher: notify error");
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
                // missing file yields an empty config (defaults) — deleting
                // boru-ui.toml therefore reloads the default theme.
                let result = load_ui_theme_config(&data_dir).map_err(|e| e.to_string());
                if tx
                    .blocking_send(UiThemeReloadMsg { generation, result })
                    .is_err()
                {
                    // The app dropped the receiver (shutdown).
                    return;
                }
            }
        })
        .map_err(|e| notify::Error::generic(&format!("failed to spawn watcher thread: {e}")))?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RenameMode};

    fn ns(ms: u64) -> u64 {
        ms * 1_000_000
    }

    #[test]
    fn debouncer_fires_once_after_quiet_window() {
        let mut d = Debouncer::new(UI_THEME_DEBOUNCE);
        d.note_event(ns(1_000));
        // Exactly the debounce window after the event → settle.
        assert_eq!(
            d.on_quiet_elapsed(ns(1_000) + UI_THEME_DEBOUNCE.as_nanos() as u64),
            Some(1)
        );
        // Firing clears the burst; another quiet-elapse without a new event
        // does not fire again.
        assert_eq!(d.on_quiet_elapsed(ns(9_999)), None);
    }

    #[test]
    fn debouncer_keeps_waiting_while_events_continue() {
        let mut d = Debouncer::new(UI_THEME_DEBOUNCE);
        let t0 = ns(1_000);
        d.note_event(t0);
        // A second event 100 ms later — still inside the 300 ms window.
        d.note_event(t0 + ns(100));
        // 100 ms after the *last* event → not settled yet.
        assert_eq!(d.on_quiet_elapsed(t0 + ns(200)), None);
        // 400 ms after the last event → settled, one reload.
        assert_eq!(d.on_quiet_elapsed(t0 + ns(500)), Some(1));
    }

    #[test]
    fn debouncer_generations_increase_per_burst() {
        let mut d = Debouncer::new(UI_THEME_DEBOUNCE);
        let t0 = ns(1_000);
        d.note_event(t0);
        assert_eq!(d.on_quiet_elapsed(t0 + UI_THEME_DEBOUNCE.as_nanos() as u64), Some(1));
        // A new burst gets the next generation.
        d.note_event(t0 + ns(10_000));
        assert_eq!(
            d.on_quiet_elapsed(t0 + ns(10_000) + UI_THEME_DEBOUNCE.as_nanos() as u64),
            Some(2)
        );
    }

    #[test]
    fn reload_tracker_drops_stale_results() {
        let mut t = ReloadTracker::new();
        assert!(t.should_apply(1));
        t.mark_applied(1);
        // Same generation (duplicate delivery) and older generations drop.
        assert!(!t.should_apply(1));
        assert!(!t.should_apply(0));
        // Newer generation applies; the tracker never goes backwards.
        assert!(t.should_apply(2));
        t.mark_applied(2);
        assert!(!t.should_apply(1));
        assert!(!t.should_apply(2));
        // Marking an older generation cannot regress the watermark.
        t.mark_applied(1);
        assert_eq!(t.last_applied(), 2);
        assert!(t.should_apply(3));
    }

    #[test]
    fn is_ui_config_event_matches_filename_and_kinds() {
        let path = PathBuf::from("/data/boru-ui.toml");

        let event = |paths: Vec<&str>, kind: EventKind| Event {
            kind,
            paths: paths.into_iter().map(PathBuf::from).collect(),
            ..Event::default()
        };

        // Write (data change) → match.
        assert!(is_ui_config_event(
            &event(
                vec!["/data/boru-ui.toml"],
                EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
            ),
            &path,
        ));
        // Create → match.
        assert!(is_ui_config_event(
            &event(
                vec!["/data/boru-ui.toml"],
                EventKind::Create(CreateKind::File),
            ),
            &path,
        ));
        // Rename (old + new paths; only the new one matches) → match.
        assert!(is_ui_config_event(
            &event(
                vec!["/data/boru-ui.toml.tmp", "/data/boru-ui.toml"],
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            ),
            &path,
        ));
        // Remove → match.
        assert!(is_ui_config_event(
            &event(vec!["/data/boru-ui.toml"], EventKind::Remove(notify::event::RemoveKind::File)),
            &path,
        ));

        // A different file never matches.
        assert!(!is_ui_config_event(
            &event(vec!["/data/other.toml"], EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content))),
            &path,
        ));
        // Access / metadata-only events are noise.
        assert!(!is_ui_config_event(
            &event(vec!["/data/boru-ui.toml"], EventKind::Access(notify::event::AccessKind::Read)),
            &path,
        ));
        assert!(!is_ui_config_event(
            &event(vec!["/data/boru-ui.toml"], EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::Any))),
            &path,
        ));
    }

    /// Integration-style: a file write produces exactly one reload message,
    /// and the parsed config arrives on the channel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_sends_exactly_one_reload_per_save() {
        let dir = std::env::temp_dir().join(format!("boru-ui-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_ui_theme_watcher(dir.clone(), tx).expect("spawn watcher");

        // The watch is registered synchronously inside spawn_ui_theme_watcher
        // (before the thread starts), so writes after this point are seen.
        let path = dir.join(UI_CONFIG_FILE_NAME);
        std::fs::write(&path, "[sidebar]\nwidth = 270.0\n").expect("write config");

        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for the reload message")
            .expect("channel closed before the reload");
        assert_eq!(msg.generation, 1, "first burst reloads with generation 1");
        let cfg = msg.result.expect("valid TOML should parse");
        let sidebar = cfg.sidebar.as_ref().expect("sidebar group present");
        assert_eq!(sidebar.width, Some(270.0));

        // One save → one reload. No second message within the debounce
        // window plus margin.
        let extra = tokio::time::timeout(UI_THEME_DEBOUNCE * 3, rx.recv()).await;
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
        let dir = std::env::temp_dir().join(format!("boru-ui-watch2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_ui_theme_watcher(dir.clone(), tx).expect("spawn watcher");

        let path = dir.join(UI_CONFIG_FILE_NAME);
        std::fs::write(&path, "[radii]\nmd = 12.0\n").expect("write config");

        let msg1 = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for first reload")
            .expect("channel closed");
        assert_eq!(msg1.generation, 1);
        assert!(msg1.result.is_ok());

        // Save again after the debounce has settled.
        tokio::time::sleep(UI_THEME_DEBOUNCE + Duration::from_millis(100)).await;
        std::fs::write(&path, "[radii]\nmd = 20.0\n").expect("rewrite config");

        let msg2 = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for second reload")
            .expect("channel closed");
        assert_eq!(msg2.generation, 2, "second burst reloads with generation 2");
        let cfg = msg2.result.expect("valid TOML should parse");
        let radii = cfg.radii.as_ref().expect("radii group present");
        assert_eq!(radii.md, Some(20.0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A malformed file yields an error result — the app keeps the last
    /// known-good theme (BORU-UI-18's reporting builds on this).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_reports_malformed_toml_as_error() {
        let dir = std::env::temp_dir().join(format!("boru-ui-watch3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_ui_theme_watcher(dir.clone(), tx).expect("spawn watcher");

        let path = dir.join(UI_CONFIG_FILE_NAME);
        std::fs::write(&path, "[sidebar\nwidth = not-a-number\n").expect("write malformed config");

        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for reload message")
            .expect("channel closed");
        assert!(msg.result.is_err(), "malformed TOML must be reported as an error");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
