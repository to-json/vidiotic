//! Watch a shader file for edits and signal the render loop to recompile.
//! Watches the *parent directory*, not the file: editors save via write-temp +
//! atomic rename, which a direct file watch would miss after the first save.

use std::path::{Path, PathBuf};

use notify::{EventKind, RecursiveMode, Watcher};

pub struct ShaderWatcher {
    _watcher: notify::RecommendedWatcher, // kept alive; dropping stops events
    rx: std::sync::mpsc::Receiver<()>,
    path: PathBuf,
}

impl ShaderWatcher {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let target = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("shader path has no file name"))?
            .to_owned();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let relevant = matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_));
                if relevant
                    && ev
                        .paths
                        .iter()
                        .any(|p| p.file_name() == Some(target.as_os_str()))
                {
                    let _ = tx.send(());
                }
            }
        })?;
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
        let dir = parent.unwrap_or_else(|| Path::new("."));
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        Ok(ShaderWatcher {
            _watcher: watcher,
            rx,
            path: path.to_path_buf(),
        })
    }

    /// Drain pending events; returns true if the shader changed since last check.
    pub fn dirty(&self) -> bool {
        let mut changed = false;
        while self.rx.try_recv().is_ok() {
            changed = true;
        }
        changed
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
