//! Analyze Faces: find the faces of photos and score each eye's openness and sharpness
//! (`lightcraft-faces`, cargo feature `faces`; the desktop app and the CLI enable it).
//!
//! **Nothing requires the model.** It is Google's MediaPipe Face Landmarker bundle (3.8 MB,
//! Apache-2.0, see `docs/faces.md`) and is not part of LightCraft: the user downloads it when they
//! first run Analyze Faces and agree to (`faces.model.download`), or puts the file in the model
//! folder themselves. Without it the command fails with a clear "not installed" error; renders,
//! exports and every other feature never touch it (the result is plain data in the photo's
//! [`Analysis`](lightcraft_catalog::Analysis)).
//!
//! The work runs on one worker thread ([`work`]) that decodes each photo (a source level the
//! caller picks, the preview by default), finds faces and scores them. The session sends it
//! `(photo, source)` pairs and takes the results back in [`Session::faces_poll`], committing each
//! photo as it arrives (not an undo step: it is a measurement, like the focus score), so an
//! interrupted run keeps what it did and running the command again resumes with the photos that
//! have no faces yet. With [`Faces::background`] (the desktop app) the command returns at once;
//! without it (CLI, MCP, tests) it waits for the results.

#[cfg(feature = "faces")]
mod download;
#[cfg(feature = "faces")]
mod work;
#[cfg(feature = "faces")]
pub(crate) use work::display;

/// Check that `file` is the published face model (exact size and SHA-256).
#[cfg(feature = "faces")]
pub(crate) fn verify_model(file: &std::path::Path) -> Result<(), String> {
    download::verify(file)
}

use std::path::PathBuf;

use lightcraft_catalog::{FaceInfo, PhotoId};
#[cfg(feature = "faces")]
use lightcraft_catalog::{Analysis, Op};

use crate::Session;
use crate::media::SourceLevel;

/// The model's licence (the weights' own, not LightCraft's): shown before downloading.
pub const LICENSE_NAME: &str = "Apache-2.0 (Google, MediaPipe)";
pub const LICENSE_URL: &str = "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf";
/// Download size of the model.
pub const MODEL_BYTES: u64 = 3_758_596;
/// The model's file name in the model folder.
pub const MODEL_FILE: &str = "face_landmarker.task";
/// How errors about a missing model start (the UI offers the download on it).
pub const NOT_INSTALLED: &str = "The face model is not installed";
/// Longest a waiting command (CLI, MCP) waits for the model.
#[cfg_attr(not(feature = "faces"), allow(dead_code))]
const WAIT: std::time::Duration = std::time::Duration::from_secs(60 * 60);
/// Environment variable naming the model folder.
pub const DIR_ENV: &str = "LIGHTCRAFT_FACES_DIR";
/// Environment variable with extra download locations (base URLs separated by commas or spaces).
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_FACES_MIRRORS";

/// What a finished photo is for [`Session::faces_poll`].
#[derive(Debug, Default)]
pub struct Polled {
    /// Some photo got its faces.
    pub changed: bool,
    /// Errors and notices from background work, for the user.
    pub messages: Vec<String>,
}

/// Where Analyze Faces is: photos queued, done and failed in the current run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
}

/// What a finished photo yields: its faces, and the focus and clipping scores when it had no
/// analysis yet.
#[cfg_attr(not(feature = "faces"), allow(dead_code))]
pub(crate) struct Scan {
    pub faces: Vec<FaceInfo>,
    pub sharpness: f32,
    pub clipped: f32,
    /// Milliseconds the model took.
    pub ms: f64,
}

/// The model (on its worker thread), the download, and the run in progress.
pub struct Faces {
    /// The model folder (`face_landmarker.task` inside); `None`: [`DIR_ENV`], else
    /// `<config>/models/faces`.
    pub dir: Option<PathBuf>,
    /// The user's list of download mirrors (one base URL per line), if any.
    pub mirrors_file: Option<PathBuf>,
    /// Run in the background and apply the results in [`Session::faces_poll`] (the desktop app);
    /// otherwise commands wait for their results (CLI, MCP, tests).
    pub background: bool,
    /// Source level the photos are analyzed from.
    pub level: SourceLevel,
    pub(crate) progress: Progress,
    messages: Vec<String>,
    #[cfg(feature = "faces")]
    worker: work::Worker,
    #[cfg(feature = "faces")]
    download: download::Downloader,
}

impl Default for Faces {
    fn default() -> Self {
        Faces {
            dir: None,
            mirrors_file: None,
            background: false,
            level: SourceLevel::Preview,
            progress: Progress::default(),
            messages: Vec::new(),
            #[cfg(feature = "faces")]
            worker: work::Worker::default(),
            #[cfg(feature = "faces")]
            download: download::Downloader::default(),
        }
    }
}

impl Faces {
    /// Whether this build can analyze faces at all.
    pub const AVAILABLE: bool = cfg!(feature = "faces");

    /// The model folder.
    pub fn model_dir(&self) -> Option<PathBuf> {
        self.dir
            .clone()
            .or_else(|| std::env::var_os(DIR_ENV).map(PathBuf::from))
            .or_else(|| crate::camera_profiles::config_dir().map(|d| d.join("models").join("faces")))
    }

    /// The model file (it may not exist).
    pub fn model_file(&self) -> Option<PathBuf> {
        self.model_dir().map(|d| d.join(MODEL_FILE))
    }

    /// Whether the model file is in place (right size; its SHA-256 is checked when it loads).
    pub fn installed(&self) -> bool {
        Self::AVAILABLE && self.model_file().and_then(|f| std::fs::metadata(f).ok()).is_some_and(|m| m.is_file() && m.len() == MODEL_BYTES)
    }

    /// Photos queued or running.
    pub fn busy(&self) -> bool {
        #[cfg(feature = "faces")]
        return self.worker.pending() > 0;
        #[cfg(not(feature = "faces"))]
        false
    }

    /// Whether the model is in memory.
    pub fn loaded(&self) -> bool {
        #[cfg(feature = "faces")]
        return self.worker.loaded();
        #[cfg(not(feature = "faces"))]
        false
    }

    /// The current (or last) run.
    pub fn progress(&self) -> Progress {
        self.progress
    }

    /// The download locations, in order: [`MIRRORS_ENV`], the mirrors file, then Google's model
    /// storage (the weights' official location).
    #[cfg(feature = "faces")]
    pub fn mirrors(&self) -> Vec<String> {
        lightcraft_fetch::mirrors(std::env::var(MIRRORS_ENV).ok().as_deref(), self.mirrors_file.as_deref(), &[lightcraft_faces::MODEL.mirror])
    }
    #[cfg(not(feature = "faces"))]
    pub fn mirrors(&self) -> Vec<String> {
        Vec::new()
    }

    /// The download's progress.
    pub fn download_status(&self) -> DownloadStatus {
        #[cfg(feature = "faces")]
        return self.download.status();
        #[cfg(not(feature = "faces"))]
        DownloadStatus::default()
    }

    /// Start downloading the model in the background (after the user agreed). False when a
    /// download is already running.
    pub fn start_download(&self) -> Result<bool, String> {
        #[cfg(feature = "faces")]
        {
            let dir = self.model_dir().ok_or("there is no folder for the model on this system")?;
            return self.download.start(self.mirrors(), dir);
        }
        #[cfg(not(feature = "faces"))]
        Err("this build of LightCraft can't analyze faces".into())
    }

    /// Stop a running download (what has arrived is kept, and a new download resumes from it).
    pub fn cancel_download(&self) -> bool {
        #[cfg(feature = "faces")]
        return self.download.cancel();
        #[cfg(not(feature = "faces"))]
        false
    }

    /// Drop the queued photos (the one being analyzed finishes).
    pub fn cancel(&mut self) {
        #[cfg(feature = "faces")]
        self.worker.cancel();
        self.progress = Progress::default();
    }
}

#[cfg(not(feature = "faces"))]
mod download {
    #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
    pub struct Status {
        pub running: bool,
        pub done: u64,
        pub total: u64,
        pub file: String,
        pub error: Option<String>,
        pub finished: bool,
    }
}
pub use download::Status as DownloadStatus;

impl Session {
    /// Queue `ids` for analysis (those without faces yet, unless `force`). Returns (queued,
    /// already analyzed).
    pub fn faces_enqueue(&mut self, ids: &[PhotoId], force: bool) -> Result<(usize, usize), String> {
        #[cfg(feature = "faces")]
        {
            let (mut queued, mut skipped) = (0, 0);
            let level = self.faces.level;
            let mut jobs = Vec::new();
            for &id in ids {
                let Some(p) = self.catalog.photo(id).cloned() else { continue };
                if !force && p.faces().is_some() {
                    skipped += 1;
                    continue;
                }
                if !matches!(p.kind, lightcraft_catalog::MediaKind::Image | lightcraft_catalog::MediaKind::Raw) {
                    continue;
                }
                let source = self.media.source_ref(&p, level);
                jobs.push(work::Job { id, source, need_scores: p.analysis.is_none() });
                queued += 1;
            }
            // (the model is only needed when there is something to analyze)
            if queued == 0 {
                return Ok((0, skipped));
            }
            if !self.faces.installed() {
                return Err(NOT_INSTALLED.to_string());
            }
            let file = self.faces.model_file().ok_or("there is no folder for the model on this system")?;
            if self.faces.progress.total == self.faces.progress.done + self.faces.progress.failed {
                self.faces.progress = Progress::default();
            }
            self.faces.progress.total += queued;
            self.faces.worker.submit(file, jobs);
            return Ok((queued, skipped));
        }
        #[cfg(not(feature = "faces"))]
        {
            let todo = ids.iter().filter(|&&id| self.catalog.photo(id).is_some_and(|p| force || p.faces().is_none())).count();
            if todo == 0 {
                return Ok((0, ids.len()));
            }
            Err("this build of LightCraft can't analyze faces".into())
        }
    }

    /// Apply the finished photos (in the order they arrive) and collect messages. Cheap when
    /// nothing is pending: call it every frame.
    pub fn faces_poll(&mut self) -> Polled {
        #[cfg_attr(not(feature = "faces"), allow(unused_mut))]
        let mut polled = Polled { messages: std::mem::take(&mut self.faces.messages), ..Default::default() };
        #[cfg(feature = "faces")]
        {
            let done: Vec<work::Outcome> = self.faces.worker.results().try_iter().collect();
            for o in done {
                self.faces_apply(o, &mut polled);
            }
        }
        polled
    }

    #[cfg(feature = "faces")]
    fn faces_apply(&mut self, o: work::Outcome, polled: &mut Polled) {
        match o.result {
            Ok(scan) => {
                self.faces.progress.done += 1;
                self.faces_set(o.id, scan);
                polled.changed = true;
            }
            Err(e) => {
                self.faces.progress.failed += 1;
                polled.messages.push(format!("Faces: {e}"));
            }
        }
    }

    /// Record a photo's faces in its analysis (not an undo step: a measurement).
    #[cfg(feature = "faces")]
    fn faces_set(&mut self, id: PhotoId, scan: Scan) {
        let Some(p) = self.catalog.photo(id) else { return };
        let faces = Some(std::sync::Arc::new(scan.faces));
        let analysis = match &p.analysis {
            Some(a) => Analysis { faces, ..a.clone() },
            None => Analysis { faces, sharpness: scan.sharpness, clipped: scan.clipped, group: None, best: false },
        };
        let op = Op::SetAnalysis { id, analysis: Some(analysis) };
        if self.catalog.apply(op.clone()).is_ok() {
            self.pending_log.push(op);
        }
    }
}

/// What one photo of a waiting run gave.
#[derive(Debug)]
pub struct Row {
    pub id: PhotoId,
    pub result: Result<RowOk, String>,
}

#[derive(Debug)]
pub struct RowOk {
    pub faces: usize,
    pub eyes_closed: bool,
    pub ms: f64,
}

impl Session {
    /// Wait for `n` queued photos (CLI, MCP, tests), applying each as it arrives. Stops early
    /// when the worker goes quiet (nothing pending) or [`WAIT`] passes.
    pub fn faces_wait(&mut self, n: usize) -> Vec<Row> {
        #[cfg_attr(not(feature = "faces"), allow(unused_mut))]
        let mut rows = Vec::new();
        #[cfg(feature = "faces")]
        {
            let deadline = std::time::Instant::now() + WAIT;
            while rows.len() < n && std::time::Instant::now() < deadline {
                match self.faces.worker.results().recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(o) => {
                        let id = o.id;
                        let result = match &o.result {
                            Ok(scan) => Ok(RowOk {
                                faces: scan.faces.len(),
                                eyes_closed: scan.faces.iter().any(FaceInfo::eyes_closed),
                                ms: scan.ms,
                            }),
                            Err(e) => Err(e.clone()),
                        };
                        let mut polled = Polled::default();
                        self.faces_apply(o, &mut polled);
                        rows.push(Row { id, result });
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) if self.faces.busy() => {}
                    Err(_) => break,
                }
            }
        }
        #[cfg(not(feature = "faces"))]
        let _ = n;
        rows
    }
}
