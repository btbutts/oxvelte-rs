//! Filesystem watch worker for the future long-running/service execution
//! mode (editor integration over stdio).
//!
//! One general worker for the whole lint pipeline — deliberately NOT
//! rule-specific. Rules stay parser/lint-triggered per file; this worker
//! wraps the same per-file lint call the one-shot CLI uses. Each `notify`
//! event maps to a re-lint unit (parse → rules), with exactly one
//! rule-aware addition: `svelte/no-conflicting-module-names` reports on
//! sibling file *names*, so a rename/delete of `Foo.svelte.<ext>` must also
//! re-lint the collision partner `Foo.svelte` (and vice versa) — the
//! diagnostic may live on the sibling the event did not touch.
//!
//! Not yet wired to a CLI subcommand (that, and the JSON stdio protocol,
//! belong to the separate editor-extension project). The worker ships
//! tested rather than CLI-exercised — see
//! `docs/research/test/no-conflicting-module-names.md`.

use crate::linter::rules::no_conflicting_module_names::MODULE_EXTENSIONS;
use notify::{Event, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// Map one changed path to the full set of files that must be re-linted so
/// every side of a potential component/module collision is re-evaluated.
///
/// - `Foo.svelte` touched → itself plus every existing `Foo.svelte.<ext>`
///   sibling (a new component can *create* a collision; a deleted one must
///   *clear* the modules' diagnostics).
/// - `Foo.svelte.<ext>` touched → itself plus the `Foo.svelte` component,
///   when it exists (a removed/renamed module must clear the component's
///   diagnostic).
/// - Anything else → just the path itself; the lint pipeline ignores
///   non-lintable files downstream.
pub fn relint_targets(changed: &Path) -> Vec<PathBuf> {
    let Some(name) = changed.file_name().and_then(|n| n.to_str()) else {
        return vec![changed.to_path_buf()];
    };

    if name.ends_with(".svelte") {
        let mut targets = vec![changed.to_path_buf()];
        for ext in MODULE_EXTENSIONS {
            let candidate = changed.with_file_name(format!("{name}{ext}"));
            if candidate.is_file() {
                targets.push(candidate);
            }
        }
        return targets;
    }

    if let Some(ext) = MODULE_EXTENSIONS.iter().find(|ext| name.ends_with(**ext)) {
        let base = &name[..name.len() - ext.len()];
        if base.ends_with(".svelte") {
            let component = changed.with_file_name(base);
            let mut targets = vec![changed.to_path_buf()];
            if component.is_file() {
                targets.push(component);
            }
            return targets;
        }
    }

    vec![changed.to_path_buf()]
}

/// Coalesces bursty filesystem events (editors save in bursts) into
/// deduplicated batches. Paths accumulate until the quiet window since the
/// oldest pending path elapses, then flush together.
pub struct Debouncer {
    window: Duration,
    pending: BTreeSet<PathBuf>,
    oldest_at: Option<Instant>,
}

impl Debouncer {
    pub fn new(window: Duration) -> Self {
        Debouncer {
            window,
            pending: BTreeSet::new(),
            oldest_at: None,
        }
    }

    /// Record a path. When the window since the oldest pending path has
    /// elapsed, the accumulated batch flushes immediately and the path
    /// starts a new batch.
    pub fn push(&mut self, path: PathBuf) -> Option<Vec<PathBuf>> {
        if self.oldest_at.is_none() {
            self.oldest_at = Some(Instant::now());
        }
        let elapsed = self.oldest_at.is_some_and(|t| t.elapsed() >= self.window);
        if elapsed {
            let flushed = self.take_pending();
            self.oldest_at = Some(Instant::now());
            self.pending.insert(path);
            return Some(flushed);
        }
        self.pending.insert(path);
        None
    }

    /// Drain whatever is still pending (called on a quiet-period timeout).
    pub fn take_pending(&mut self) -> Vec<PathBuf> {
        self.oldest_at = None;
        std::mem::take(&mut self.pending).into_iter().collect()
    }
}

/// Watch `roots` recursively and invoke `handler` with debounced batches of
/// re-lint targets. Blocks until the watcher channel disconnects (i.e. the
/// watcher is dropped or errors out); intended to run on a dedicated
/// thread — `notify` delivers events over a channel, so no async runtime
/// is needed.
pub fn watch<F>(roots: &[PathBuf], debounce_window: Duration, mut handler: F) -> notify::Result<()>
where
    F: FnMut(Vec<PathBuf>),
{
    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })?;
    for root in roots {
        watcher.watch(root, RecursiveMode::Recursive)?;
    }

    let mut debouncer = Debouncer::new(debounce_window);
    loop {
        match rx.recv_timeout(debounce_window) {
            Ok(event) => {
                for path in event.paths {
                    for target in relint_targets(&path) {
                        if let Some(batch) = debouncer.push(target) {
                            handler(batch);
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let batch = debouncer.take_pending();
                if !batch.is_empty() {
                    handler(batch);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CASE_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_case(files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxvelte-watch-{}-{}",
            std::process::id(),
            CASE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), "").unwrap();
        }
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn component_event_pulls_in_existing_modules() {
        let dir = temp_case(&["Foo.svelte", "Foo.svelte.ts", "Foo.svelte.js"]);
        let targets = relint_targets(&dir.join("Foo.svelte"));
        assert_eq!(targets.len(), 3);
        assert!(targets.contains(&dir.join("Foo.svelte")));
        assert!(targets.contains(&dir.join("Foo.svelte.ts")));
        assert!(targets.contains(&dir.join("Foo.svelte.js")));
        cleanup(&dir);
    }

    #[test]
    fn module_event_pulls_in_component() {
        let dir = temp_case(&["Foo.svelte", "Foo.svelte.ts"]);
        let targets = relint_targets(&dir.join("Foo.svelte.ts"));
        assert_eq!(targets, vec![dir.join("Foo.svelte.ts"), dir.join("Foo.svelte")]);
        cleanup(&dir);
    }

    #[test]
    fn deleted_file_event_still_reaches_the_sibling() {
        // Only the component remains; the module was just deleted.
        let dir = temp_case(&["Foo.svelte"]);
        let targets = relint_targets(&dir.join("Foo.svelte.ts"));
        assert_eq!(targets, vec![dir.join("Foo.svelte.ts"), dir.join("Foo.svelte")]);
        cleanup(&dir);
    }

    #[test]
    fn unrelated_files_map_to_themselves() {
        let dir = temp_case(&["foo-state.ts", "README.md"]);
        assert_eq!(
            relint_targets(&dir.join("foo-state.ts")),
            vec![dir.join("foo-state.ts")]
        );
        // `svelte.ts` is not a component; no sibling expansion.
        assert_eq!(relint_targets(&dir.join("README.md")), vec![dir.join("README.md")]);
        cleanup(&dir);
    }

    #[test]
    fn declaration_files_do_not_expand() {
        // Stripping `.ts` from `Foo.svelte.d.ts` leaves `Foo.svelte.d`,
        // which is not a component — no pair to re-lint.
        let dir = temp_case(&["Foo.svelte", "Foo.svelte.d.ts"]);
        assert_eq!(
            relint_targets(&dir.join("Foo.svelte.d.ts")),
            vec![dir.join("Foo.svelte.d.ts")]
        );
        cleanup(&dir);
    }

    #[test]
    fn debouncer_batches_within_window_and_flushes_after() {
        let mut debouncer = Debouncer::new(Duration::from_millis(30));
        assert_eq!(debouncer.push(PathBuf::from("/a")), None);
        assert_eq!(debouncer.push(PathBuf::from("/b")), None);
        // Duplicate path collapses into the same batch.
        assert_eq!(debouncer.push(PathBuf::from("/a")), None);
        std::thread::sleep(Duration::from_millis(40));
        let flushed = debouncer.push(PathBuf::from("/c"));
        assert_eq!(flushed, Some(vec![PathBuf::from("/a"), PathBuf::from("/b")]));
        // `/c` stays pending until drained.
        assert_eq!(debouncer.take_pending(), vec![PathBuf::from("/c")]);
        assert!(debouncer.take_pending().is_empty());
    }
}
