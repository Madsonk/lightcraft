//! The recognizer, the face index and the indexing run (feature `people`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lightcraft_catalog::PhotoId;
use lightcraft_facemesh::FaceLandmarker;
use lightcraft_facemesh::mediapipe::Frame;
use lightcraft_fetch::FileSpec;
use lightcraft_people::cluster::{Group, Suggestion};
use lightcraft_people::recog::Recognizer;
use lightcraft_people::state::State;
use lightcraft_people::store::{FaceMeta, FaceStore};
use lightcraft_people::{FAST_MODEL, MODEL, ModelInfo};
use rayon::prelude::*;

use crate::media::SourceRef;
use crate::segment::DownloadStatus;
use crate::segment::download::Downloader;

/// Longest side of the picture the faces are looked for in (pixels).
pub const DEFAULT_EDGE: usize = 1600;
/// Photos decoded together; their faces are recognized together.
const BATCH: usize = 12;

const fn spec(m: &ModelInfo) -> FileSpec<'_> {
    FileSpec { name: m.file_name, size: Some(m.size), sha256: Some(m.sha256), max: m.size }
}

pub static FILES_R50: [FileSpec; 1] = [spec(&MODEL)];
pub static FILES_FAST: [FileSpec; 1] = [spec(&FAST_MODEL)];

/// One photo to look at.
pub struct Item {
    pub id: PhotoId,
    pub key: u64,
    pub source: SourceRef,
}

/// A decision and the state before it, for Undo.
pub struct Entry {
    pub label: String,
    pub state: State,
    /// Length of the session's undo stack after the step's keyword changes (`None`: none).
    pub committed: Option<usize>,
}

/// The faces found, grouped (see [`lightcraft_people::cluster`]), as of some revision of the index.
pub struct Computed {
    pub store_rev: u64,
    pub groups: Vec<Group>,
    pub suggestions: Vec<Suggestion>,
    pub left_out: usize,
    pub seconds: f64,
}

/// The index and the decisions of one library, for one model.
pub struct Slot {
    pub dir: Option<PathBuf>,
    pub model: &'static ModelInfo,
    pub store: FaceStore,
    pub state: State,
    pub state_path: Option<PathBuf>,
    /// Changes whenever faces are added.
    pub store_rev: u64,
    /// Changes whenever a decision is recorded.
    pub state_rev: u64,
    pub computed: Option<Computed>,
    pub history: Vec<Entry>,
    /// A note for the user from loading (a people file that could not be read).
    pub note: Option<String>,
}

#[derive(Default)]
struct JobText {
    error: Option<String>,
    failures: Vec<(PhotoId, String)>,
}

/// An indexing run in progress (or the last one).
#[derive(Default)]
pub struct Job {
    running: AtomicBool,
    cancel: AtomicBool,
    total: AtomicU64,
    done: AtomicU64,
    failed: AtomicU64,
    faces: AtomicU64,
    millis: AtomicU64,
    /// Time spent loading the models, decoding, finding faces, recognizing, writing (ms).
    phases: [AtomicU64; 5],
    text: Mutex<JobText>,
}

/// What a run looks like from outside.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub running: bool,
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    pub faces: u64,
    pub seconds: f64,
    pub phase_seconds: PhaseSeconds,
    pub error: Option<String>,
    pub failures: Vec<(u64, String)>,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct PhaseSeconds {
    pub load: f64,
    pub decode: f64,
    pub detect: f64,
    pub recognize: f64,
    pub write: f64,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Running(Arc<Job>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

/// Grouping in the background.
#[derive(Default)]
struct Compute {
    running: AtomicBool,
    /// Grouping was asked for while one ran: it starts again when that one ends.
    again: AtomicBool,
}

/// What the session shares with the worker threads.
#[derive(Default)]
pub struct Core {
    pub slot: Arc<Mutex<Option<Slot>>>,
    recognizer: Arc<Mutex<Option<(PathBuf, Arc<Recognizer>)>>>,
    landmarker: Arc<Mutex<Option<(PathBuf, Arc<FaceLandmarker>)>>>,
    job: Arc<Job>,
    compute: Arc<Compute>,
    download: Downloader,
    download_faces: Downloader,
}

/// The picture shrunk so its long side is at most `edge` (area average of integer blocks, then a
/// bilinear step), straight sRGB 0–1.
pub fn shrink(rgb: Vec<[f32; 3]>, w: usize, h: usize, edge: usize) -> (Vec<[f32; 3]>, usize, usize) {
    let long = w.max(h);
    if edge == 0 || long <= edge || w == 0 || h == 0 {
        return (rgb, w, h);
    }
    let scale = edge as f32 / long as f32;
    let (nw, nh) = (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1));
    // each output pixel averages the block of source pixels it covers (box filter, fractional edges ignored)
    let (bx, by) = (w as f32 / nw as f32, h as f32 / nh as f32);
    let out: Vec<[f32; 3]> = (0..nh)
        .into_par_iter()
        .flat_map_iter(|j| {
            let (y0, y1) = ((j as f32 * by) as usize, (((j + 1) as f32 * by).ceil() as usize).clamp(1, h));
            let rgb = &rgb;
            (0..nw).map(move |i| {
                let (x0, x1) = ((i as f32 * bx) as usize, (((i + 1) as f32 * bx).ceil() as usize).clamp(1, w));
                let mut acc = [0.0f32; 3];
                let mut n = 0.0f32;
                for y in y0..y1.max(y0 + 1).min(h) {
                    for x in x0..x1.max(x0 + 1).min(w) {
                        let p = rgb[y * w + x];
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                        n += 1.0;
                    }
                }
                [acc[0] / n.max(1.0), acc[1] / n.max(1.0), acc[2] / n.max(1.0)]
            })
        })
        .collect();
    (out, nw, nh)
}

/// Decode a photo for the face tools: display sRGB 0–1, at most `edge` on the long side.
fn decode(it: &Item, edge: usize) -> Result<(Vec<[f32; 3]>, usize, usize), String> {
    let mut source = it.source.clone();
    if let SourceRef::File { max_edge, .. } = &mut source {
        *max_edge = (*max_edge).min(edge.max(512));
    }
    let src = source.load_source()?;
    let (w, h) = (src.image.width, src.image.height);
    let rgb = crate::facemesh::display(&src.image);
    Ok(shrink(rgb, w, h, edge))
}

struct Run {
    job: Arc<Job>,
    slot: Arc<Mutex<Option<Slot>>>,
    recognizer: Arc<Mutex<Option<(PathBuf, Arc<Recognizer>)>>>,
    landmarker: Arc<Mutex<Option<(PathBuf, Arc<FaceLandmarker>)>>>,
    face_file: PathBuf,
    model_file: PathBuf,
    edge: usize,
    flip: bool,
}

fn verify(file: &Path, m: &ModelInfo) -> Result<(), String> {
    let dir = file.parent().ok_or("the model file has no folder")?;
    let s = spec(m);
    lightcraft_fetch::download(&[s], &[], dir, &lightcraft_fetch::Options::default(), &AtomicBool::new(false), &mut |_| {}).map_err(|_| {
        format!(
            "{} is not the published {} (expected {} bytes, SHA-256 {}): delete it and download the model again",
            file.display(),
            m.name,
            m.size,
            m.sha256
        )
    })
}

impl Run {
    fn recognizer(&self, m: &ModelInfo) -> Result<Arc<Recognizer>, String> {
        let mut g = lock(&self.recognizer);
        if let Some((f, r)) = g.as_ref()
            && *f == self.model_file
        {
            return Ok(r.clone());
        }
        verify(&self.model_file, m)?;
        let r = Arc::new(Recognizer::load(&self.model_file).map_err(|e| e.to_string())?);
        *g = Some((self.model_file.clone(), r.clone()));
        Ok(r)
    }

    fn landmarker(&self) -> Result<Arc<FaceLandmarker>, String> {
        let mut g = lock(&self.landmarker);
        if let Some((f, r)) = g.as_ref()
            && *f == self.face_file
        {
            return Ok(r.clone());
        }
        crate::facemesh::verify_model(&self.face_file)?;
        let bytes = std::fs::read(&self.face_file).map_err(|e| format!("{}: {e}", self.face_file.display()))?;
        let r = Arc::new(FaceLandmarker::from_task(&bytes)?);
        *g = Some((self.face_file.clone(), r.clone()));
        Ok(r)
    }

    fn go(&self, items: Vec<Item>, model: &'static ModelInfo) -> Result<(), String> {
        let phase = |i: usize, t: std::time::Instant| self.job.phases[i].fetch_add(t.elapsed().as_millis() as u64, Ordering::SeqCst);
        let t = std::time::Instant::now();
        let lm = self.landmarker()?;
        let rec = self.recognizer(model)?;
        phase(0, t);
        for chunk in items.chunks(BATCH) {
            if self.job.cancel.load(Ordering::SeqCst) {
                break;
            }
            let t = std::time::Instant::now();
            let decoded: Vec<Result<(Vec<[f32; 3]>, usize, usize), String>> = chunk.par_iter().map(|it| decode(it, self.edge)).collect();
            phase(1, t);
            // faces of each photo (the interpreter spreads one picture over the cores)
            let t = std::time::Instant::now();
            let mut per_photo: Vec<(usize, Vec<(FaceMeta, Vec<f32>)>)> = Vec::new();
            let mut crops: Vec<Vec<f32>> = Vec::new();
            for (n, d) in decoded.into_iter().enumerate() {
                let it = &chunk[n];
                match d {
                    Err(e) => self.fail(it.id, e),
                    Ok((rgb, w, h)) => match lightcraft_people::detect::faces_of(&lm, Frame { rgb: &rgb, w, h }) {
                        Err(e) => self.fail(it.id, e.to_string()),
                        Ok(faces) => {
                            let mut metas = Vec::new();
                            for f in faces {
                                let (wf, hf) = (w as f32, h as f32);
                                let rect = [f.bbox[0] / wf, f.bbox[1] / hf, f.bbox[2] / wf, f.bbox[3] / hf].map(|v| v.clamp(0.0, 1.0));
                                metas.push((FaceMeta { key: it.key, idx: 0, rect, det: f.det, size: f.size, turn: f.turn }, Vec::new()));
                                crops.push(lightcraft_people::align::crop(Frame { rgb: &rgb, w, h }, &f.landmarks));
                            }
                            per_photo.push((n, metas));
                        }
                    },
                }
            }
            phase(2, t);
            let t = std::time::Instant::now();
            let mut vectors = rec.embed(&crops).map_err(|e| e.to_string())?;
            if self.flip {
                let flipped: Vec<Vec<f32>> = crops.iter().map(|c| lightcraft_people::recog::flipped(c)).collect();
                let other = rec.embed(&flipped).map_err(|e| e.to_string())?;
                for (v, o) in vectors.iter_mut().zip(other) {
                    let sum: Vec<f32> = v.iter().zip(&o).map(|(a, b)| a + b).collect();
                    *v = lightcraft_people::recog::normalized(&sum);
                }
            }
            phase(3, t);
            let mut it = vectors.into_iter();
            let mut records = Vec::new();
            for (n, metas) in per_photo {
                let faces: Vec<(FaceMeta, Vec<f32>)> = metas.into_iter().map(|(m, _)| (m, it.next().unwrap_or_default())).collect();
                self.job.faces.fetch_add(faces.len() as u64, Ordering::SeqCst);
                records.push((chunk[n].key, faces));
            }
            let t = std::time::Instant::now();
            {
                let mut g = lock(&self.slot);
                let slot = g.as_mut().ok_or("the people index is not open")?;
                slot.store.add(records).map_err(|e| format!("writing the people index: {e}"))?;
                slot.store_rev += 1;
            }
            phase(4, t);
            self.job.done.fetch_add(chunk.len() as u64, Ordering::SeqCst);
        }
        Ok(())
    }

    fn fail(&self, id: PhotoId, e: String) {
        self.job.failed.fetch_add(1, Ordering::SeqCst);
        let mut t = lock(&self.job.text);
        if t.failures.len() < 100 {
            t.failures.push((id, e));
        }
    }
}

impl Core {
    pub fn job_status(&self) -> JobStatus {
        let j = &self.job;
        let t = lock(&j.text);
        let p = |i: usize| j.phases[i].load(Ordering::SeqCst) as f64 / 1000.0;
        JobStatus {
            running: j.running.load(Ordering::SeqCst),
            total: j.total.load(Ordering::SeqCst),
            done: j.done.load(Ordering::SeqCst),
            failed: j.failed.load(Ordering::SeqCst),
            faces: j.faces.load(Ordering::SeqCst),
            seconds: j.millis.load(Ordering::SeqCst) as f64 / 1000.0,
            phase_seconds: PhaseSeconds { load: p(0), decode: p(1), detect: p(2), recognize: p(3), write: p(4) },
            error: t.error.clone(),
            failures: t.failures.iter().map(|(i, e)| (i.0, e.clone())).collect(),
        }
    }

    pub fn job_running(&self) -> bool {
        self.job.running.load(Ordering::SeqCst)
    }

    pub fn cancel_job(&self) -> bool {
        let running = self.job_running();
        if running {
            self.job.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    pub fn loaded(&self) -> bool {
        lock(&self.recognizer).is_some()
    }

    pub fn unload(&self) -> bool {
        let a = lock(&self.recognizer).take().is_some();
        let b = lock(&self.landmarker).take().is_some();
        a || b
    }

    pub fn computing(&self) -> bool {
        self.compute.running.load(Ordering::SeqCst)
    }

    /// Look at `items` (decode, find faces, recognize) and add the faces to the open index, on a
    /// worker thread; with `background` false this waits for it.
    #[allow(clippy::too_many_arguments)]
    pub fn start_job(
        &self,
        items: Vec<Item>,
        model: &'static ModelInfo,
        model_file: PathBuf,
        face_file: PathBuf,
        edge: usize,
        flip: bool,
        background: bool,
    ) -> Result<(), String> {
        let job = self.job.clone();
        if job.running.swap(true, Ordering::SeqCst) {
            return Err("an indexing run is already in progress (see people.status; people.cancel stops it)".into());
        }
        let guard = Running(job.clone());
        job.cancel.store(false, Ordering::SeqCst);
        job.total.store(items.len() as u64, Ordering::SeqCst);
        for a in [&job.done, &job.failed, &job.faces, &job.millis] {
            a.store(0, Ordering::SeqCst);
        }
        job.phases.iter().for_each(|p| p.store(0, Ordering::SeqCst));
        *lock(&job.text) = JobText::default();
        let run = Run {
            job,
            slot: self.slot.clone(),
            recognizer: self.recognizer.clone(),
            landmarker: self.landmarker.clone(),
            face_file,
            model_file,
            edge,
            flip,
        };
        let work = move || {
            let t0 = std::time::Instant::now();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.go(items, model)));
            let error = match r {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("the people indexing run failed unexpectedly".to_string()),
            };
            if let Some(e) = &error {
                log::warn!("people index: {e}");
            }
            lock(&run.job.text).error = error;
            run.job.millis.store(t0.elapsed().as_millis() as u64, Ordering::SeqCst);
            drop(guard);
        };
        let handle =
            std::thread::Builder::new().name("people-index".into()).spawn(work).map_err(|e| format!("could not start the indexing run: {e}"))?;
        if !background {
            let _ = handle.join();
        }
        Ok(())
    }

    /// Group the faces of the open index (a snapshot of it is grouped, so the index stays
    /// available) and keep the result in the slot. With `background` false this waits.
    pub fn start_compute(&self, background: bool) -> Result<bool, String> {
        if self.compute.running.swap(true, Ordering::SeqCst) {
            if background {
                // the run in progress started before this change: it runs once more when it ends
                self.compute.again.store(true, Ordering::SeqCst);
                return Ok(false);
            }
            while self.compute.running.swap(true, Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let (slot, compute) = (self.slot.clone(), self.compute.clone());
        let handle = std::thread::Builder::new().name("people-group".into()).spawn(move || {
            loop {
                compute.again.store(false, Ordering::SeqCst);
                let snap = {
                    let g = lock(&slot);
                    g.as_ref().map(|sl| (sl.store.snapshot(), sl.state.clone(), sl.store_rev))
                };
                let Some((store, state, rev)) = snap else { break };
                let t = std::time::Instant::now();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lightcraft_people::cluster::group(&store, &state)));
                if let Ok(g) = r {
                    let mut sl = lock(&slot);
                    if let Some(s) = sl.as_mut() {
                        let c = Computed {
                            store_rev: rev,
                            groups: g.groups,
                            suggestions: g.suggestions,
                            left_out: g.left_out,
                            seconds: t.elapsed().as_secs_f64(),
                        };
                        s.computed = Some(c);
                        super::ops::reconcile(s);
                    }
                }
                if !compute.again.load(Ordering::SeqCst) {
                    break;
                }
            }
            compute.running.store(false, Ordering::SeqCst);
        });
        match handle {
            Ok(h) => {
                if !background {
                    let _ = h.join();
                }
                Ok(true)
            }
            Err(e) => {
                self.compute.running.store(false, Ordering::SeqCst);
                Err(format!("could not start grouping: {e}"))
            }
        }
    }

    pub fn download_status(&self) -> DownloadStatus {
        let s = self.download.status();
        DownloadStatus { running: s.running, done: s.done, total: s.total, file: s.file, error: s.error, finished: s.finished }
    }

    pub fn face_download_status(&self) -> DownloadStatus {
        let s = self.download_faces.status();
        DownloadStatus { running: s.running, done: s.done, total: s.total, file: s.file, error: s.error, finished: s.finished }
    }

    pub fn start_download(&self, fast: bool, mirrors: Vec<String>, dir: PathBuf) -> Result<bool, String> {
        let files: &'static [FileSpec] = if fast { &FILES_FAST } else { &FILES_R50 };
        self.download.start(files, mirrors, dir, lightcraft_fetch::Options::default())
    }

    pub fn start_face_download(&self, dir: PathBuf, mirrors: Vec<String>) -> Result<bool, String> {
        static F: [FileSpec; 1] = [FileSpec {
            name: lightcraft_facemesh::MODEL.file_name,
            size: Some(lightcraft_facemesh::MODEL.size),
            sha256: Some(lightcraft_facemesh::MODEL.sha256),
            max: lightcraft_facemesh::MODEL.size,
        }];
        self.download_faces.start(&F, mirrors, dir, lightcraft_fetch::Options::default())
    }

    pub fn cancel_download(&self) -> bool {
        let a = self.download.cancel();
        let b = self.download_faces.cancel();
        a || b
    }
}
