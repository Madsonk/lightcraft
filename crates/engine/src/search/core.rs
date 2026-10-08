//! The model, the index and the indexing run (feature `search`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lightcraft_catalog::PhotoId;
use lightcraft_fetch::FileSpec;
use lightcraft_search::{Embedder, HashStore, VectorIndex, hash, preprocess::preprocess};
use rayon::prelude::*;

use super::Searcher;
use crate::media::SourceRef;
use crate::segment::DownloadStatus;
use crate::segment::download::Downloader;

/// The SigLIP 2 files of `google/siglip2-base-patch16-224` at one fixed revision, pinned by size and
/// SHA-256 (Hugging Face's LFS hashes, checked again on the files as downloaded).
pub const FILES: &[FileSpec] = &[
    FileSpec {
        name: "preprocessor_config.json",
        size: Some(394),
        sha256: Some("9b36b57ebaf20f09bf4c22100ccc21877ea6bfe5aead0c00c59f8af8ccefacfc"),
        max: 394,
    },
    FileSpec {
        name: "tokenizer.json",
        size: Some(34_363_039),
        sha256: Some("cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322"),
        max: 34_363_039,
    },
    FileSpec {
        name: "model.safetensors",
        size: Some(1_500_800_904),
        sha256: Some("612923381c76ec5a9bed335d1c48827e3f2e506ac31b044b63b2031fadee6a0b"),
        max: 1_500_800_904,
    },
];

/// Where the files come from, in order (base URLs; file `f` is at `<base>/<f>`): the model's
/// public repository at the revision [`FILES`] was pinned on. Not gated: no account is needed.
pub const DEFAULT_MIRRORS: &[&str] = &["https://huggingface.co/google/siglip2-base-patch16-224/resolve/75de2d55ec2d0b4efc50b3e9ad70dba96a7b2fa2"];

/// Photos decoded and embedded per step.
const BATCH: usize = 32;

/// One photo to decode.
pub struct Item {
    pub id: PhotoId,
    pub key: u64,
    pub source: SourceRef,
    pub want_vector: bool,
    pub want_hash: bool,
}

/// The vectors and hashes of one library, for one model.
pub struct Store {
    pub dir: Option<PathBuf>,
    pub model: String,
    pub vectors: VectorIndex,
    pub hashes: HashStore,
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
    millis: AtomicU64,
    /// Time spent loading the model, decoding photos, embedding them and writing (milliseconds).
    phases: [AtomicU64; 4],
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
    pub seconds: f64,
    /// Where `seconds` went: loading the model, decoding the photos, embedding, writing the index.
    pub phase_seconds: PhaseSeconds,
    pub error: Option<String>,
    pub failures: Vec<(u64, String)>,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct PhaseSeconds {
    pub load: f64,
    pub decode: f64,
    pub embed: f64,
    pub write: f64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Clears `running` when the run ends, however it ends.
struct Running(Arc<Job>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

/// What a run shares with the session.
#[derive(Default)]
pub struct Core {
    embedder: Arc<Mutex<Option<Arc<Embedder>>>>,
    /// One caller on the GPU at a time (a query while a run embeds waits for the batch).
    gpu: Arc<Mutex<()>>,
    store: Arc<Mutex<Option<Store>>>,
    job: Arc<Job>,
    download: Downloader,
}

/// 8-bit sRGB of a decoded source (the sources are linear).
fn srgb8(img: &lightcraft_raster::Rgb32f) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() * 3);
    for p in &img.data {
        for c in p {
            out.push((lightcraft_color::transfer::linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8);
        }
    }
    out
}

/// A photo ready for the model: preprocessed pixels (if wanted) and its hash.
struct Prepared {
    key: u64,
    pixels: Option<Vec<f32>>,
    hash: Option<[u64; 2]>,
}

fn prepare(it: &Item, resample: Option<lightcraft_search::Resample>) -> Result<Prepared, (PhotoId, String)> {
    let fail = |e: String| (it.id, e);
    let src = it.source.load_source().map_err(fail)?;
    let (w, h) = (src.image.width, src.image.height);
    let rgb = srgb8(&src.image);
    let hash = if it.want_hash { hash::dhash(&rgb, w, h) } else { None };
    let pixels = match (it.want_vector, resample) {
        (true, Some(r)) => Some(preprocess(&rgb, w, h, r).ok_or_else(|| (it.id, "the decoded photo is empty".to_string()))?),
        _ => None,
    };
    Ok(Prepared { key: it.key, pixels, hash })
}

/// The model in `slot`, loaded from `dir` now if it isn't.
fn load_embedder(slot: &Mutex<Option<Arc<Embedder>>>, dir: &Path) -> Result<Arc<Embedder>, String> {
    let mut g = lock(slot);
    if let Some(e) = g.as_ref() {
        return Ok(e.clone());
    }
    let e = Arc::new(Embedder::load(dir).map_err(|e| e.to_string())?);
    *g = Some(e.clone());
    Ok(e)
}

fn open_store(g: &mut Option<Store>, model_dir: Option<&Path>, dir: Option<PathBuf>) -> Result<(), String> {
    let model = model_dir.map(lightcraft_search::model_id).unwrap_or_else(|| "none".into());
    if g.as_ref().is_some_and(|s| s.dir == dir && s.model == model) {
        return Ok(());
    }
    let err = |e: std::io::Error| format!("the search index: {e}");
    let vectors_path = dir.as_ref().filter(|_| model_dir.is_some()).map(|d| d.join(format!("vectors-{model}.bin")));
    let vectors = VectorIndex::open(vectors_path.as_deref(), &model, 768).map_err(err)?;
    let hashes = HashStore::open(dir.as_ref().map(|d| d.join("hashes.bin")).as_deref()).map_err(err)?;
    *g = Some(Store { dir, model, vectors, hashes });
    Ok(())
}

/// Everything a run needs, owned (it runs on its own thread).
struct Run {
    job: Arc<Job>,
    store: Arc<Mutex<Option<Store>>>,
    embedder: Arc<Mutex<Option<Arc<Embedder>>>>,
    gpu: Arc<Mutex<()>>,
    model_dir: Option<PathBuf>,
    dir: Option<PathBuf>,
}

impl Run {
    fn go(&self, items: Vec<Item>) -> Result<(), String> {
        let phase = |i: usize, t: std::time::Instant| self.job.phases[i].fetch_add(t.elapsed().as_millis() as u64, Ordering::SeqCst);
        let t = std::time::Instant::now();
        let model = if items.iter().any(|i| i.want_vector) {
            let d = self.model_dir.as_deref().ok_or("the semantic search model is not installed")?;
            Some(load_embedder(&self.embedder, d)?)
        } else {
            None
        };
        phase(0, t);
        let resample = model.as_ref().map(|m| m.resample());
        for chunk in items.chunks(BATCH) {
            if self.job.cancel.load(Ordering::SeqCst) {
                break;
            }
            let t = std::time::Instant::now();
            let results: Vec<Result<Prepared, (PhotoId, String)>> = chunk.par_iter().map(|it| prepare(it, resample)).collect();
            phase(1, t);
            let mut ready = Vec::new();
            for r in results {
                match r {
                    Ok(p) => ready.push(p),
                    Err(f) => {
                        self.job.failed.fetch_add(1, Ordering::SeqCst);
                        let mut t = lock(&self.job.text);
                        if t.failures.len() < 100 {
                            t.failures.push(f);
                        }
                    }
                }
            }
            let mut vectors = Vec::new();
            let t = std::time::Instant::now();
            if let Some(m) = &model {
                let with_pixels: Vec<&Prepared> = ready.iter().filter(|p| p.pixels.is_some()).collect();
                let px: Vec<Vec<f32>> = with_pixels.iter().filter_map(|p| p.pixels.clone()).collect();
                let embedded = {
                    let _gpu = lock(&self.gpu);
                    m.embed_images(&px).map_err(|e| e.to_string())?
                };
                vectors = with_pixels.iter().map(|p| p.key).zip(embedded).collect::<Vec<_>>();
            }
            phase(2, t);
            let hashes: Vec<(u64, [u64; 2])> = ready.iter().filter_map(|p| p.hash.map(|h| (p.key, h))).collect();
            let t = std::time::Instant::now();
            {
                let mut g = lock(&self.store);
                open_store(&mut g, self.model_dir.as_deref(), self.dir.clone())?;
                let st = g.as_mut().ok_or("the search index could not be opened")?;
                st.vectors.add(&vectors).map_err(|e| format!("writing the index: {e}"))?;
                st.hashes.add(&hashes).map_err(|e| format!("writing the hashes: {e}"))?;
            }
            phase(3, t);
            self.job.done.fetch_add(chunk.len() as u64, Ordering::SeqCst);
        }
        Ok(())
    }
}

impl Searcher {
    /// `<library>/search`, or `None` when the library isn't on disk.
    pub(crate) fn store_dir(library: Option<&crate::library::Library>) -> Option<PathBuf> {
        library.filter(|l| l.on_disk).map(|l| l.dir.join("search"))
    }

    fn installed_model(&self) -> Option<PathBuf> {
        self.model_folder().filter(|d| lightcraft_search::is_model_dir(d))
    }

    /// Run `f` on the library's store (opened now if it isn't, or if the library or the model
    /// changed).
    pub fn with_store<R>(&self, dir: Option<PathBuf>, f: impl FnOnce(&mut Store) -> R) -> Result<R, String> {
        let mut g = lock(&self.core.store);
        open_store(&mut g, self.installed_model().as_deref(), dir)?;
        match g.as_mut() {
            Some(s) => Ok(f(s)),
            None => Err("the search index could not be opened".into()),
        }
    }

    /// The model, loaded now if it isn't (a minute on a slow disk, a few seconds on an SSD).
    pub fn embedder(&self) -> Result<Arc<Embedder>, String> {
        let dir = self.model_dir()?;
        load_embedder(&self.core.embedder, &dir)
    }

    /// Whether the model is in memory.
    pub fn loaded(&self) -> bool {
        lock(&self.core.embedder).is_some()
    }

    /// Free the model's memory (it loads again when needed).
    pub fn unload(&self) -> bool {
        lock(&self.core.embedder).take().is_some()
    }

    /// Unit embeddings of `texts`.
    pub fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let e = self.embedder()?;
        let _gpu = lock(&self.core.gpu);
        e.embed_texts(texts).map_err(|e| e.to_string())
    }

    /// The probability the model gives a photo and a caption with this cosine.
    pub fn probability(&self, cosine: f32) -> Option<f32> {
        lock(&self.core.embedder).as_ref().map(|e| e.probability(cosine))
    }

    pub fn job_status(&self) -> JobStatus {
        let j = &self.core.job;
        let t = lock(&j.text);
        JobStatus {
            running: j.running.load(Ordering::SeqCst),
            total: j.total.load(Ordering::SeqCst),
            done: j.done.load(Ordering::SeqCst),
            failed: j.failed.load(Ordering::SeqCst),
            seconds: j.millis.load(Ordering::SeqCst) as f64 / 1000.0,
            phase_seconds: {
                let p = |i: usize| j.phases[i].load(Ordering::SeqCst) as f64 / 1000.0;
                PhaseSeconds { load: p(0), decode: p(1), embed: p(2), write: p(3) }
            },
            error: t.error.clone(),
            failures: t.failures.iter().map(|(i, e)| (i.0, e.clone())).collect(),
        }
    }

    pub fn job_running(&self) -> bool {
        self.core.job.running.load(Ordering::SeqCst)
    }

    /// Stop the run after the batch in flight (what it did is kept).
    pub fn cancel_job(&self) -> bool {
        let running = self.job_running();
        if running {
            self.core.job.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    /// Decode `items`, embed and/or hash them, and append the results to the store of `dir`, on a
    /// worker thread; with `background` false this waits for it. Photos that can't be read are
    /// listed in the status, the others are kept.
    pub fn start_job(&self, dir: Option<PathBuf>, items: Vec<Item>, background: bool) -> Result<(), String> {
        let job = self.core.job.clone();
        if job.running.swap(true, Ordering::SeqCst) {
            return Err("an indexing run is already in progress (see search.status; search.cancel stops it)".into());
        }
        let guard = Running(job.clone());
        job.cancel.store(false, Ordering::SeqCst);
        job.total.store(items.len() as u64, Ordering::SeqCst);
        job.done.store(0, Ordering::SeqCst);
        job.failed.store(0, Ordering::SeqCst);
        job.millis.store(0, Ordering::SeqCst);
        job.phases.iter().for_each(|p| p.store(0, Ordering::SeqCst));
        *lock(&job.text) = JobText::default();
        let run = Run {
            job,
            store: self.core.store.clone(),
            embedder: self.core.embedder.clone(),
            gpu: self.core.gpu.clone(),
            model_dir: self.installed_model(),
            dir,
        };
        let work = move || {
            let t0 = std::time::Instant::now();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.go(items)));
            let error = match r {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("the indexing run failed unexpectedly".to_string()),
            };
            if let Some(e) = &error {
                log::warn!("search index: {e}");
            }
            lock(&run.job.text).error = error;
            run.job.millis.store(t0.elapsed().as_millis() as u64, Ordering::SeqCst);
            drop(guard);
        };
        let handle =
            std::thread::Builder::new().name("search-index".into()).spawn(work).map_err(|e| format!("could not start the indexing run: {e}"))?;
        if !background {
            let _ = handle.join();
        }
        Ok(())
    }

    /// Where the SigLIP files are downloaded from, in order (the user's mirrors first).
    pub fn mirrors(&self) -> Vec<String> {
        let env = std::env::var(super::MIRRORS_ENV).ok();
        lightcraft_fetch::mirrors(env.as_deref(), self.mirrors_file.as_deref(), DEFAULT_MIRRORS)
    }

    pub fn download_status(&self) -> DownloadStatus {
        let s = self.core.download.status();
        DownloadStatus { running: s.running, done: s.done, total: s.total, file: s.file, error: s.error, finished: s.finished }
    }

    /// Start downloading the model (on a background thread; see `search.model.download`).
    /// `Ok(false)` when it is installed or downloading already.
    pub fn start_download(&self) -> Result<bool, String> {
        let dir = self.model_folder().ok_or("no folder is set for the semantic search model")?;
        if self.installed() {
            return Ok(false);
        }
        let mirrors = self.mirrors();
        if mirrors.is_empty() {
            return Err(format!("no download location is configured for the model (set {})", super::MIRRORS_ENV));
        }
        self.core.download.start(FILES, mirrors, dir, lightcraft_fetch::Options::default())
    }

    pub fn cancel_download(&self) -> bool {
        self.core.download.cancel()
    }
}
