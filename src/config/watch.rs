//! Hot-reload: watch the config file and atomically swap the running config
//! (and restart monitors) whenever it changes and still validates.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::config;
use crate::state::AppState;

/// Spawn a background watcher for `path`. Invalid edits are logged and ignored,
/// leaving the previous good config running.
pub fn spawn(state: Arc<AppState>, path: PathBuf) {
    // Captured so the (non-async) watcher thread can spawn tokio tasks on reload.
    let handle = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("config watcher init failed: {e}");
                return;
            }
        };

        // Watch the parent directory: editors often replace the file on save,
        // which can drop a watch placed directly on the file.
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            tracing::error!("failed to watch {}: {e}", dir.display());
            return;
        }
        tracing::info!("watching {} for changes", path.display());

        loop {
            match rx.recv() {
                Ok(Ok(event)) => {
                    // Only react to events touching our specific file.
                    if !event
                        .paths
                        .iter()
                        .any(|p| p.file_name() == path.file_name())
                    {
                        continue;
                    }
                    // Debounce: drain any rapid follow-up events.
                    while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
                    let _enter = handle.enter();
                    reload(&state, &path);
                }
                Ok(Err(e)) => tracing::debug!("watch event error: {e}"),
                Err(_) => break, // sender dropped
            }
        }
    });
}

fn reload(state: &Arc<AppState>, path: &Path) {
    match config::load(path) {
        Ok(cfg) => {
            state.config.store(Arc::new(cfg));
            crate::monitor::spawn(state.clone());
            tracing::info!("configuration reloaded from {}", path.display());
        }
        Err(e) => tracing::warn!("ignoring invalid config change: {e:#}"),
    }
}
