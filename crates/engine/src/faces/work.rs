//! The face worker: one thread that loads the model on first use, decodes each photo it is sent
//! and finds and scores its faces. The session never waits on it: it sends [`Job`]s and takes
//! [`Outcome`]s back from a channel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::{FaceInfo, PhotoId};
use lightcraft_faces::{FaceLandmarker, mediapipe::Frame};
use lightcraft_raster::Rgb32f;
use rayon::prelude::*;

use super::Scan;
use crate::media::SourceRef;

/// One photo to analyze: where its pixels come from (decoded on the worker, not the UI thread).
pub(super) struct Job {
    pub id: PhotoId,
    pub source: SourceRef,
    /// The photo has no focus / clipping scores yet: measure them from the same pixels.
    pub need_scores: bool,
}

pub(super) struct Outcome {
    pub id: PhotoId,
    pub result: Result<Scan, String>,
}

struct Batch {
    file: PathBuf,
    generation: u64,
    jobs: Vec<Job>,
}

pub(super) struct Worker {
    tx: Mutex<Option<Sender<Batch>>>,
    results: (Sender<Outcome>, Receiver<Outcome>),
    pending: Arc<AtomicUsize>,
    generation: Arc<AtomicU64>,
    loaded: Arc<AtomicBool>,
}

impl Default for Worker {
    fn default() -> Self {
        Worker {
            tx: Mutex::new(None),
            results: channel(),
            pending: Arc::new(AtomicUsize::new(0)),
            generation: Arc::new(AtomicU64::new(0)),
            loaded: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Worker {
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    pub fn loaded(&self) -> bool {
        self.loaded.load(Ordering::SeqCst)
    }

    pub fn results(&self) -> &Receiver<Outcome> {
        &self.results.1
    }

    /// Forget the queued photos (the one being analyzed finishes; its result is dropped).
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Queue `jobs`; the model is read from `file` the first time.
    pub fn submit(&self, file: PathBuf, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        let n = jobs.len();
        let Ok(mut slot) = self.tx.lock() else { return };
        let mut batch = Batch { file, generation: self.generation.load(Ordering::SeqCst), jobs };
        self.pending.fetch_add(n, Ordering::SeqCst);
        // (a worker that died is replaced)
        for _ in 0..2 {
            if let Some(tx) = slot.as_ref() {
                match tx.send(batch) {
                    Ok(()) => return,
                    Err(e) => batch = e.0,
                }
            }
            let (tx, rx) = channel::<Batch>();
            let (out, pending, generation, loaded) = (self.results.0.clone(), self.pending.clone(), self.generation.clone(), self.loaded.clone());
            if std::thread::Builder::new().name("faces".into()).spawn(move || run(rx, out, pending, generation, loaded)).is_err() {
                break;
            }
            *slot = Some(tx);
        }
        // could not start a worker: the jobs fail
        self.pending.fetch_sub(n, Ordering::SeqCst);
        for j in batch.jobs {
            let _ = self.results.0.send(Outcome { id: j.id, result: Err("could not start the face worker".into()) });
        }
    }
}

fn run(rx: Receiver<Batch>, out: Sender<Outcome>, pending: Arc<AtomicUsize>, generation: Arc<AtomicU64>, loaded: Arc<AtomicBool>) {
    let mut model: Option<(PathBuf, Arc<FaceLandmarker>)> = None;
    while let Ok(batch) = rx.recv() {
        let m = match &model {
            Some((f, m)) if *f == batch.file => Ok(m.clone()),
            _ => load(&batch.file).inspect(|m| {
                model = Some((batch.file.clone(), m.clone()));
                loaded.store(true, Ordering::SeqCst);
            }),
        };
        for job in batch.jobs {
            if generation.load(Ordering::SeqCst) != batch.generation {
                pending.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let result = match &m {
                Ok(m) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyze(m, &job))).unwrap_or_else(|_| Err("the face model failed unexpectedly".into())),
                Err(e) => Err(e.clone()),
            };
            // (counted down after the result is sent: `pending == 0` means everything arrived)
            let _ = out.send(Outcome { id: job.id, result });
            pending.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Read the model file, after checking it is the published one (size and SHA-256).
fn load(file: &std::path::Path) -> Result<Arc<FaceLandmarker>, String> {
    super::download::verify(file)?;
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    FaceLandmarker::from_task(&bytes).map(Arc::new)
}

/// The photo as the model sees it: display sRGB 0–1. The source is linear Rec.2020; a source that
/// is dark overall (a raw file's unrendered sensor data) is brightened so faces keep their
/// contrast (the 95th percentile of luminance goes to 0.8, at most ×8).
pub(super) fn display(img: &Rgb32f) -> Vec<[f32; 3]> {
    let m = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB);
    let mut sample: Vec<f32> = img.data.iter().step_by(13).map(|c| lightcraft_color::luminance_2020(*c)).collect();
    let gain = if sample.is_empty() {
        1.0
    } else {
        let k = ((sample.len() as f32 * 0.95) as usize).min(sample.len() - 1);
        let (_, p95, _) = sample.select_nth_unstable_by(k, f32::total_cmp);
        if *p95 > 1e-4 && *p95 < 0.55 { (0.8 / *p95).min(8.0) } else { 1.0 }
    };
    img.data
        .par_iter()
        .map(|c| {
            let l = m.apply_f32([c[0] * gain, c[1] * gain, c[2] * gain]);
            l.map(|v| lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)))
        })
        .collect()
}

fn analyze(model: &FaceLandmarker, job: &Job) -> Result<Scan, String> {
    let src = job.source.load_source()?;
    let img = &src.image;
    let (w, h) = (img.width, img.height);
    let t = std::time::Instant::now();
    let rgb = display(img);
    let scores = lightcraft_faces::analyze(model, Frame { rgb: &rgb, w, h })?;
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let faces = scores
        .iter()
        .map(|f| FaceInfo {
            rect: [f.bbox[0] / wf, f.bbox[1] / hf, f.bbox[2] / wf, f.bbox[3] / hf].map(|v| v.clamp(0.0, 1.0)),
            score: f.score,
            open: [f.left.open, f.right.open],
            sharp: [f.left.sharpness, f.right.sharpness],
            turn: f.turn,
        })
        .collect();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let (sharpness, clipped) = if job.need_scores { (lightcraft_pipeline::cull::sharpness(img), lightcraft_pipeline::cull::clipped(img)) } else { (0.0, 0.0) };
    Ok(Scan { faces, sharpness, clipped, ms })
}
