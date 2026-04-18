//! File-watching reloader that keeps a [`SharedFilter`] in sync
//! with a `.wasm` file on disk.
//!
//! **Tier 4 item 4.2, session C.** Watches the parent directory
//! of the configured WASM module path (not the file itself, so
//! macOS FSEvents and Linux inotify both observe writes via
//! atomic save / copy without a platform-specific special case)
//! via [`notify::RecommendedWatcher`]. Every event whose
//! `paths` list includes the target module triggers a
//! debounced recompile + [`SharedFilter::replace`] call.
//!
//! # Atomicity
//!
//! [`SharedFilter`] stores the active filter behind a
//! `Mutex<Box<dyn FragmentFilter>>`. Every [`FragmentFilter::apply`]
//! call holds the mutex for the duration of the guest
//! `on_fragment` invocation. [`SharedFilter::replace`] takes the
//! same mutex to swap the boxed pointer in place. Consequently:
//!
//! * Each in-flight `apply` finishes on the OLD module (its
//!   mutex guard already owns the `Box`).
//! * The very next `apply` after `replace` returns observes the
//!   NEW module.
//! * Readers never observe a partially constructed filter;
//!   `replace` never races with itself because it only runs on
//!   the single background thread this type owns.
//!
//! Future contributors broadening the filter to, e.g., an
//! `ArcSwap` or a lock-free surface MUST preserve the "each
//! `apply` finishes on the module it started on" invariant; see
//! [`FragmentFilter::apply`] for the contract.
//!
//! # Anti-scope
//!
//! * No rollback if the new module traps at runtime. The fail-
//!   open semantics in [`crate::WasmFilter::apply`] cover that
//!   case; the tap records the fragment as kept with the
//!   original payload.
//! * If the new module fails to **compile**, we log a warning
//!   and keep the previous module live; the reloader keeps
//!   watching for subsequent edits.
//! * The reloader does not surface reload events to the admin
//!   API. Operators verify a reload landed by watching the
//!   [`WasmFilterBridgeHandle`] counters change on subsequent
//!   fragments.
//!
//! [`FragmentFilter`]: crate::FragmentFilter
//! [`FragmentFilter::apply`]: crate::FragmentFilter::apply
//! [`WasmFilterBridgeHandle`]: crate::WasmFilterBridgeHandle

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{SharedFilter, WasmFilter};

/// Default quiet-period after a file-change event before the
/// reloader recompiles the module. Tuned on the short side
/// because the typical edit cycle is a single full-file
/// overwrite, so waiting longer only delays the reload without
/// coalescing more events. Overridable via
/// [`WasmFilterReloader::spawn_with_debounce`].
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(50);

/// Keeps a [`SharedFilter`] in sync with a `.wasm` file on disk.
///
/// Dropping the reloader stops the background worker and the
/// inner notify watcher; the [`SharedFilter`] remains usable
/// with the last successfully installed module.
pub struct WasmFilterReloader {
    path: PathBuf,
    // `Option` so `Drop` can take + drop the watcher BEFORE
    // joining the worker; dropping the watcher drops the
    // callback's `mpsc::Sender`, which closes the event
    // channel and causes the worker's `events.recv()` to
    // return `Err(Disconnected)`. Without this ordering the
    // worker would stay blocked on `recv()` and the join
    // below would deadlock.
    watcher: Option<RecommendedWatcher>,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: mpsc::Sender<()>,
}

impl std::fmt::Debug for WasmFilterReloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmFilterReloader").field("path", &self.path).finish()
    }
}

impl WasmFilterReloader {
    /// Spawn a reloader that re-compiles `path` on change and
    /// swaps the compiled module into `filter` via
    /// [`SharedFilter::replace`]. Uses [`DEFAULT_DEBOUNCE`] as
    /// the quiet window between an inbound change event and the
    /// recompile.
    pub fn spawn(path: impl AsRef<Path>, filter: SharedFilter) -> Result<Self> {
        Self::spawn_with_debounce(path, filter, DEFAULT_DEBOUNCE)
    }

    /// Like [`Self::spawn`] but with an explicit debounce window.
    /// Useful for tests that want to minimise wall-clock waiting
    /// between a file swap and the assertion that follows.
    pub fn spawn_with_debounce(path: impl AsRef<Path>, filter: SharedFilter, debounce: Duration) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let canon_target = std::fs::canonicalize(&path)
            .with_context(|| format!("canonicalizing WASM filter path {}", path.display()))?;
        let parent = canon_target
            .parent()
            .ok_or_else(|| anyhow!("WASM filter path has no parent directory: {}", path.display()))?
            .to_path_buf();

        let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let _ = event_tx.send(res);
        })
        .context("creating WASM filter watcher")?;
        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching WASM filter parent dir {}", parent.display()))?;

        let target_for_worker = canon_target.clone();
        let orig_path_for_worker = path.clone();
        let worker = thread::Builder::new()
            .name("lvqr-wasm-reload".into())
            .spawn(move || {
                run_reload_loop(
                    target_for_worker,
                    orig_path_for_worker,
                    filter,
                    event_rx,
                    shutdown_rx,
                    debounce,
                );
            })
            .context("spawning WASM reload worker thread")?;

        tracing::info!(
            path = %path.display(),
            debounce_ms = debounce.as_millis() as u64,
            "WASM filter reloader watching parent directory"
        );

        Ok(Self {
            path,
            watcher: Some(watcher),
            worker: Some(worker),
            shutdown: shutdown_tx,
        })
    }

    /// Path the reloader was configured to watch.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WasmFilterReloader {
    fn drop(&mut self) {
        // Order matters: send the shutdown signal, then drop
        // the watcher so its callback's `mpsc::Sender` is
        // released, which closes the event channel and wakes
        // the worker out of its blocking `recv()`. Only then
        // can we `join()` without deadlocking.
        let _ = self.shutdown.send(());
        let _ = self.watcher.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_reload_loop(
    target: PathBuf,
    orig_path: PathBuf,
    filter: SharedFilter,
    events: mpsc::Receiver<notify::Result<Event>>,
    shutdown: mpsc::Receiver<()>,
    debounce: Duration,
) {
    loop {
        if shutdown.try_recv().is_ok() {
            return;
        }
        let first = match events.recv() {
            Ok(ev) => ev,
            // Watcher dropped: event channel closed. Nothing
            // more to do.
            Err(_) => return,
        };
        let matched = match first {
            Ok(ev) => event_matches(&ev, &target),
            Err(e) => {
                tracing::warn!(error = %e, "WASM filter watcher reported an error");
                false
            }
        };
        if !matched {
            continue;
        }
        // Debounce: drain any follow-up events that arrive
        // within `debounce` of the first matching one. Editors
        // and `cp` frequently fire several events per save
        // (truncate + write + close), and we only want to
        // recompile once per logical change.
        let deadline = Instant::now() + debounce;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            if events.recv_timeout(deadline - now).is_err() {
                break;
            }
        }
        match WasmFilter::load(&orig_path) {
            Ok(new_filter) => {
                filter.replace(new_filter);
                tracing::info!(path = %orig_path.display(), "WASM filter reloaded from disk");
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %orig_path.display(),
                    "WASM filter reload failed; keeping previous module live"
                );
            }
        }
    }
}

fn event_matches(event: &Event, target: &Path) -> bool {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any => {}
        _ => return false,
    }
    event.paths.iter().any(|p| p == target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FragmentFilter;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags};
    use tempfile::TempDir;

    const WAT_NOOP: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            local.get 1))
    "#;

    const WAT_DROP: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            i32.const -1))
    "#;

    fn sample() -> Fragment {
        Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            1000,
            1000,
            3000,
            FragmentFlags::default(),
            Bytes::from_static(b"hello"),
        )
    }

    /// Wait up to `max` for `predicate` to return true, polling
    /// every 20 ms. Returns whether the predicate eventually
    /// matched. Used instead of a bare sleep so the tests do not
    /// rely on a single magic wall-clock duration across
    /// platforms.
    fn wait_until<F: FnMut() -> bool>(max: Duration, mut predicate: F) -> bool {
        let deadline = Instant::now() + max;
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        predicate()
    }

    #[test]
    fn reload_swaps_noop_to_drop_after_file_overwrite() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("filter.wasm");
        std::fs::write(&path, wat::parse_str(WAT_NOOP).unwrap()).unwrap();

        let filter = SharedFilter::new(WasmFilter::load(&path).unwrap());
        // Short debounce so the test does not spend its wall-
        // clock budget waiting on the coalescing window.
        let _reloader =
            WasmFilterReloader::spawn_with_debounce(&path, filter.clone(), Duration::from_millis(10)).unwrap();

        assert!(
            filter.apply(sample()).is_some(),
            "initial no-op filter must keep the fragment"
        );

        std::fs::write(&path, wat::parse_str(WAT_DROP).unwrap()).unwrap();

        let now_drops = wait_until(Duration::from_secs(5), || filter.apply(sample()).is_none());
        assert!(
            now_drops,
            "filter should drop fragments after the on-disk module was replaced with WAT_DROP"
        );
    }

    #[test]
    fn reload_with_invalid_wasm_keeps_previous_module_live() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("filter.wasm");
        std::fs::write(&path, wat::parse_str(WAT_NOOP).unwrap()).unwrap();

        let filter = SharedFilter::new(WasmFilter::load(&path).unwrap());
        let _reloader =
            WasmFilterReloader::spawn_with_debounce(&path, filter.clone(), Duration::from_millis(10)).unwrap();

        // Overwrite with junk bytes. WasmFilter::load must fail
        // to compile and the reloader must leave the previous
        // module installed.
        std::fs::write(&path, b"not wasm at all, just a lie").unwrap();
        // Give the watcher + debounce window a beat to process
        // the (rejected) reload.
        thread::sleep(Duration::from_millis(200));

        assert!(
            filter.apply(sample()).is_some(),
            "invalid reload must not replace the previous module; no-op filter should still keep"
        );
    }

    #[test]
    fn reloader_spawn_fails_on_nonexistent_path() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist.wasm");
        let filter = SharedFilter::new(WasmFilter::from_bytes(&wat::parse_str(WAT_NOOP).unwrap()).unwrap());
        let res = WasmFilterReloader::spawn(&missing, filter);
        assert!(res.is_err(), "spawn must error out when the target path does not exist");
    }
}
