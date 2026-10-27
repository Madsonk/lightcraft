//! The face model download as a background task the session starts, watches and cancels without
//! ever blocking: progress lives in atomics, text behind a mutex only ever `try_lock`ed by the
//! session. The downloader itself (mirrors, resuming, size and SHA-256 checks, pure-Rust HTTPS) is
//! `lightcraft-fetch`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lightcraft_fetch::{FileSpec, Options, Progress};

/// What the session sees of a download.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    pub running: bool,
    pub done: u64,
    pub total: u64,
    pub file: String,
    /// The last download's error (cleared when a new one starts).
    pub error: Option<String>,
    /// The last download finished and the file is in place.
    pub finished: bool,
}

#[derive(Default)]
struct Shared {
    running: AtomicBool,
    cancel: AtomicBool,
    finished: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    error: Mutex<Option<String>>,
}

/// Clears `running` when the download thread ends, however it ends.
struct Running(Arc<Shared>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct Downloader {
    shared: Arc<Shared>,
}

fn spec() -> FileSpec<'static> {
    let m = lightcraft_facemesh::MODEL;
    FileSpec { name: m.file_name, size: Some(m.size), sha256: Some(m.sha256), max: m.size }
}

impl Downloader {
    pub fn status(&self) -> Status {
        let s = &self.shared;
        Status {
            running: s.running.load(Ordering::SeqCst),
            done: s.done.load(Ordering::SeqCst),
            total: s.total.load(Ordering::SeqCst),
            file: lightcraft_facemesh::MODEL.file_name.to_string(),
            error: s.error.try_lock().ok().and_then(|e| e.clone()),
            finished: s.finished.load(Ordering::SeqCst),
        }
    }

    /// Ask a running download to stop (it keeps its partial file to resume later).
    pub fn cancel(&self) -> bool {
        let running = self.shared.running.load(Ordering::SeqCst);
        if running {
            self.shared.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    /// Download the model from `mirrors` into `dir` on a background thread. False when one is
    /// already running.
    pub fn start(&self, mirrors: Vec<String>, dir: PathBuf) -> Result<bool, String> {
        let s = self.shared.clone();
        if s.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        s.cancel.store(false, Ordering::SeqCst);
        s.finished.store(false, Ordering::SeqCst);
        s.done.store(0, Ordering::SeqCst);
        s.total.store(lightcraft_facemesh::MODEL.size, Ordering::SeqCst);
        if let Ok(mut e) = s.error.lock() {
            *e = None;
        }
        let guard = Running(s.clone());
        let spawned = std::thread::Builder::new().name("faces-download".into()).spawn(move || {
            let s = guard.0.clone();
            let mut on_progress = |p: &Progress| {
                s.done.store(p.done, Ordering::SeqCst);
                s.total.store(p.total, Ordering::SeqCst);
            };
            let files = [spec()];
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lightcraft_fetch::download(&files, &mirrors, &dir, &Options::default(), &s.cancel, &mut on_progress)
            }));
            let error = match r {
                Ok(Ok(())) => {
                    s.finished.store(true, Ordering::SeqCst);
                    log::info!("face model downloaded to {}", dir.display());
                    None
                }
                Ok(Err(e)) => Some(e.to_string()),
                Err(_) => Some("the download failed unexpectedly".to_string()),
            };
            if let Some(e) = &error {
                log::warn!("face model download: {e}");
            }
            if let Ok(mut slot) = s.error.lock() {
                *slot = error;
            }
            drop(guard);
        });
        match spawned {
            Ok(_) => Ok(true),
            Err(e) => Err(format!("could not start the download: {e}")),
        }
    }
}

/// Check that `file` is the published model (exact size and SHA-256).
pub fn verify(file: &std::path::Path) -> Result<(), String> {
    let dir = file.parent().ok_or("the model file has no folder")?;
    lightcraft_fetch::download(&[spec()], &[], dir, &Options::default(), &AtomicBool::new(false), &mut |_| {}).map_err(|_| {
        format!(
            "{} is not the published {} (expected {} bytes, SHA-256 {}): delete it and download the model again",
            file.display(),
            lightcraft_facemesh::MODEL.name,
            lightcraft_facemesh::MODEL.size,
            lightcraft_facemesh::MODEL.sha256
        )
    })
}
