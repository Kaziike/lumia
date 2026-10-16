use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::SystemTime;
use wm_proto::ReloadTarget;

/// File modification event emitted by `AssetWatcher` when `.css` or `.html` files change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetReloadEvent {
    pub target: ReloadTarget,
    pub surface: Option<String>,
    pub path: PathBuf,
}

/// Watches an `assets/` directory for `.css` and `.html` changes using `notify`
/// plus a deterministic mtime-polling fallback.
pub struct AssetWatcher {
    watch_dir: PathBuf,
    _watcher: Option<RecommendedWatcher>,
    rx: Option<Receiver<notify::Result<Event>>>,
    known_mtimes: HashMap<PathBuf, SystemTime>,
}

impl AssetWatcher {
    pub fn new(watch_dir: impl Into<PathBuf>) -> Self {
        let watch_dir = watch_dir.into();
        let mut known_mtimes = HashMap::new();
        Self::scan_mtimes(&watch_dir, &mut known_mtimes);

        let (tx, rx) = mpsc::channel();
        let mut watcher_opt = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .ok();

        if let Some(watcher) = watcher_opt.as_mut() {
            if watch_dir.exists() {
                let _ = watcher.watch(&watch_dir, RecursiveMode::Recursive);
            }
        }

        Self {
            watch_dir,
            _watcher: watcher_opt,
            rx: Some(rx),
            known_mtimes,
        }
    }

    pub fn watch_dir(&self) -> &Path {
        &self.watch_dir
    }

    /// Polls for any modified `.css` or `.html` files without blocking.
    pub fn poll_events(&mut self) -> Vec<AssetReloadEvent> {
        let mut changed_paths: Vec<PathBuf> = Vec::new();

        // 1. Drain notify channel events
        if let Some(rx) = &self.rx {
            while let Ok(Ok(event)) = rx.try_recv() {
                if event.kind.is_modify() || event.kind.is_create() {
                    for path in event.paths {
                        if Self::classify_path(&path).is_some() && !changed_paths.contains(&path) {
                            changed_paths.push(path);
                        }
                    }
                }
            }
        }

        // 2. Also check mtime changes as a reliable fallback
        let mut current_mtimes = HashMap::new();
        Self::scan_mtimes(&self.watch_dir, &mut current_mtimes);
        for (path, mtime) in &current_mtimes {
            let is_modified = match self.known_mtimes.get(path) {
                Some(prev) => mtime > prev,
                None => true,
            };
            if is_modified && !changed_paths.contains(path) {
                changed_paths.push(path.clone());
            }
        }
        self.known_mtimes = current_mtimes;

        let mut events = Vec::new();
        for path in changed_paths {
            if let Some((target, surface)) = Self::classify_path(&path) {
                events.push(AssetReloadEvent {
                    target,
                    surface,
                    path,
                });
            }
        }
        events
    }

    /// Convenience helper returning deduplicated `ReloadTarget`s since last poll.
    pub fn poll_reload_targets(&mut self) -> Vec<ReloadTarget> {
        let mut targets = Vec::new();
        for ev in self.poll_events() {
            if !targets.contains(&ev.target) {
                targets.push(ev.target);
            }
        }
        targets
    }

    pub fn classify_path(path: &Path) -> Option<(ReloadTarget, Option<String>)> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let target = match ext.as_str() {
            "css" => ReloadTarget::Css,
            "html" | "htm" => ReloadTarget::Html,
            _ => return None,
        };

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let surface = match stem.as_deref() {
            Some("theme" | "global" | "common" | "variables" | "style" | "main") => None,
            other => other.map(|s| s.to_string()),
        };

        Some((target, surface))
    }

    fn scan_mtimes(dir: &Path, out: &mut HashMap<PathBuf, SystemTime>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::scan_mtimes(&path, out);
            } else if Self::classify_path(&path).is_some() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        out.insert(path, mtime);
                    }
                }
            }
        }
    }
}
