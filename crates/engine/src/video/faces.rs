//! Faces of the pictures of videos, on 0.14.0's own face models (`lightcraft-faces`: YuNet finds the
//! faces and gives their landmarks, the chosen recognition model embeds them).
//!
//! A video is not a catalogue photo, so its faces do not go through the photo index. For each
//! picture of a video (see [`super::frames`]) the faces are found, aligned and embedded here, kept
//! in `<library>/video/faces-<model tag>.json` under the picture's key ([`super::frame_key`]) and
//! named by comparing them with the **named faces of the photos** (the people of the People view):
//! the same bars as `faces.suggest` (the model's threshold and margin). Nothing is ever copied into
//! a photo and nobody is named by the machine alone: a name shown on a moment is a suggestion read
//! from the photos' confirmed names.
//!
//! Needs both face models installed (Settings ▸ Faces) and the pictures cut (`video.frames`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_faces::align::{Rgb, align_to_template};
use lightcraft_faces::matching;
use lightcraft_faces::yunet::Options;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

const MIN_SCORE: f32 = 0.6;
const DEFAULT_THRESHOLD: f32 = 0.45;
const DEFAULT_MARGIN: f32 = 0.06;

/// One face of one picture: its box (fractions of the picture) and its embedding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoFace {
    pub rect: [f32; 4],
    pub emb: Vec<f32>,
}

/// The faces found so far, by picture key, for one recognition model.
#[derive(Default, Serialize, Deserialize)]
struct Faces {
    tag: String,
    by_key: HashMap<u64, Vec<VideoFace>>,
}

/// State kept in the session: the faces, the named gallery, the run in progress.
#[derive(Default)]
pub struct VideoFaces {
    store: Arc<Mutex<Faces>>,
    pub job: Arc<Job>,
    /// The named faces of the photos, by (index size, number of named faces): rebuilt when it changes.
    gallery: Option<((usize, usize), Arc<Vec<(String, Arc<[f32]>)>>)>,
    /// The recognition model's bars (threshold, margin) for the tag in `store`.
    bars: Option<(f32, f32)>,
}

#[derive(Default)]
pub struct Job {
    pub running: AtomicBool,
    pub cancel: AtomicBool,
    pub total: AtomicU64,
    pub done: AtomicU64,
    /// Pictures with at least one face found in the run.
    pub with_faces: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Job {
    fn error(&self) -> MutexGuard<'_, Option<String>> {
        self.error.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn lock(m: &Mutex<Faces>) -> MutexGuard<'_, Faces> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn file_of(s: &Session, tag: &str) -> Option<std::path::PathBuf> {
    let safe: String = tag.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    super::cache_root(s).map(|r| r.join(format!("faces-{safe}.json")))
}

/// Make sure the store is the one of the current recognition model (loaded from its file).
fn open(s: &mut Session, tag: &str, bars: (f32, f32)) {
    s.video.faces.bars = Some(bars);
    let store = s.video.faces.store.clone();
    if lock(&store).tag == tag {
        return;
    }
    let loaded = file_of(s, tag)
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice::<Faces>(&b).ok())
        .filter(|f| f.tag == tag)
        .unwrap_or_else(|| Faces { tag: tag.to_string(), by_key: HashMap::new() });
    *lock(&store) = loaded;
}

fn save(s: &Session, store: &Mutex<Faces>) {
    let f = lock(store);
    let Some(path) = file_of(s, &f.tag) else { return };
    if let (Some(dir), Ok(bytes)) = (path.parent(), serde_json::to_vec(&*f)) {
        let _ = std::fs::create_dir_all(dir);
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn recognizer(s: &mut Session, c: &str) -> Result<(Arc<lightcraft_faces::runtime::Embedder>, String)> {
    let e = crate::cmd::face_recognize::recognizer(s, c)?;
    let tag = s.faces.embedder.as_ref().map(|(t, _)| t.clone()).ok_or_else(|| bad(c, "no recognition model is loaded"))?;
    Ok((e, tag))
}

/// The named faces of the photos the People view knows, as `(name, embedding)`.
fn gallery(s: &mut Session) -> Arc<Vec<(String, Arc<[f32]>)>> {
    let named = s.catalog.photos().filter(|p| p.in_library()).map(|p| p.meta.regions.len()).sum::<usize>();
    let stamp = (s.faces.index.len(), named);
    if let Some((st, g)) = &s.video.faces.gallery
        && *st == stamp
    {
        return g.clone();
    }
    let g: Vec<(String, Arc<[f32]>)> = s
        .catalog
        .photos()
        .filter(|p| p.in_library())
        .flat_map(|p| {
            p.meta
                .regions
                .iter()
                .filter(|r| r.kind == lightcraft_meta::RegionKind::Face)
                .filter_map(|r| Some((r.name.clone()?, s.faces.index.get(p.id.0, &r.rect)?.clone())))
                .collect::<Vec<_>>()
        })
        .collect();
    let g = Arc::new(g);
    s.video.faces.gallery = Some((stamp, g.clone()));
    g
}

/// Who is at `key`: `[{face, rect, person: null, name}]`, `name` a suggestion from the photos' names.
pub fn faces_at(s: &mut Session, key: u64) -> Vec<Value> {
    prepare_reading(s);
    let found: Vec<VideoFace> = lock(&s.video.faces.store).by_key.get(&key).cloned().unwrap_or_default();
    if found.is_empty() {
        return Vec::new();
    }
    let g = gallery(s);
    let refs: Vec<(&str, &[f32])> = g.iter().map(|(n, e)| (n.as_str(), &e[..])).collect();
    let (threshold, margin) = s.video.faces.bars.unwrap_or((DEFAULT_THRESHOLD, DEFAULT_MARGIN));
    found
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let name = matching::suggest(&matching::rank(&f.emb, &refs), threshold, margin).map(|x| x.name);
            json!({"face": i, "rect": f.rect, "person": Value::Null, "name": name})
        })
        .collect()
}

/// Every picture key faces were looked for in.
pub fn keys(s: &mut Session) -> Vec<u64> {
    prepare_reading(s);
    lock(&s.video.faces.store).by_key.keys().copied().collect()
}

/// How many of `keys` have been looked at for faces (with or without a result).
pub fn analysed(s: &mut Session, keys: &[u64]) -> usize {
    prepare_reading(s);
    let f = lock(&s.video.faces.store);
    keys.iter().filter(|k| f.by_key.contains_key(k)).count()
}

/// Make the store of the chosen model ready for reading, when a model is installed (nothing happens
/// otherwise: the lists then show no faces).
pub fn prepare_reading(s: &mut Session) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok((e, tag)) = recognizer(s, "video.faces") {
        let bars = (e.manifest().thresholds.match_cosine.unwrap_or(DEFAULT_THRESHOLD), DEFAULT_MARGIN);
        open(s, &tag, bars);
    }
}

/// `video.faces {ids?, force?, background?}`: look for faces in the pictures of these videos.
#[cfg(not(target_arch = "wasm32"))]
fn run(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.faces";
    let (embedder, tag) = recognizer(s, C)?;
    let detector = crate::cmd::face_detect::detector(s).map_err(|e| e.into_error(C))?;
    let bars = (embedder.manifest().thresholds.match_cosine.unwrap_or(DEFAULT_THRESHOLD), DEFAULT_MARGIN);
    open(s, &tag, bars);
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let background = p.get("background").and_then(Value::as_bool).unwrap_or(false);
    let ids: Vec<lightcraft_catalog::PhotoId> = if p.get("ids").is_some() {
        s.targets(p)?
    } else {
        s.catalog.photos().filter(|q| q.in_library() && q.kind == lightcraft_catalog::MediaKind::Video).map(|q| q.id).collect()
    };
    let mut found: Vec<(u64, std::path::PathBuf)> = Vec::new();
    for id in &ids {
        for (key, f) in s.frames_of_video(*id) {
            found.push((key, f.file));
        }
    }
    let mut work: Vec<(u64, std::path::PathBuf)> = Vec::new();
    {
        let have = lock(&s.video.faces.store);
        let mut seen = std::collections::HashSet::new();
        for (key, file) in found {
            if seen.insert(key) && (force || !have.by_key.contains_key(&key)) {
                work.push((key, file));
            }
        }
    }
    let job = s.video.faces.job.clone();
    if job.running.swap(true, Ordering::SeqCst) {
        return Err(bad(C, "faces are already being looked for: wait or video.facesCancel"));
    }
    job.cancel.store(false, Ordering::SeqCst);
    job.total.store(work.len() as u64, Ordering::SeqCst);
    job.done.store(0, Ordering::SeqCst);
    job.with_faces.store(0, Ordering::SeqCst);
    *job.error() = None;
    let store = s.video.faces.store.clone();
    let total = work.len();
    let body = {
        let job = job.clone();
        let store = store.clone();
        move || {
            for (key, file) in work {
                if job.cancel.load(Ordering::SeqCst) {
                    break;
                }
                let faces = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| faces_of(&file, &detector, &embedder))).unwrap_or_default();
                if !faces.is_empty() {
                    job.with_faces.fetch_add(1, Ordering::SeqCst);
                }
                lock(&store).by_key.insert(key, faces);
                job.done.fetch_add(1, Ordering::SeqCst);
            }
            job.running.store(false, Ordering::SeqCst);
        }
    };
    if background {
        let spawned = std::thread::Builder::new().name("video-faces".into()).spawn(body);
        if let Err(e) = spawned {
            job.running.store(false, Ordering::SeqCst);
            return Err(bad(C, format!("could not start the run: {e}")));
        }
    } else {
        body();
        save(s, &store);
    }
    Ok(json!({"pictures": total, "background": background, "model": tag}))
}

/// The faces of one picture file: found by YuNet, aligned by its landmarks, embedded.
#[cfg(not(target_arch = "wasm32"))]
fn faces_of(file: &std::path::Path, detector: &lightcraft_faces::yunet::Detector, embedder: &lightcraft_faces::runtime::Embedder) -> Vec<VideoFace> {
    let Ok(bytes) = std::fs::read(file) else { return Vec::new() };
    let Ok(t) = lightcraft_codecs::decode_thumbnail(&bytes, 1280) else { return Vec::new() };
    let (w, h) = (t.image.width, t.image.height);
    let rgb: Vec<u8> = t.image.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
    let Ok(found) = detector.detect(&rgb, w, h, &Options { score: 0.5, ..Options::default() }) else { return Vec::new() };
    let img = Rgb { data: &rgb, width: w, height: h };
    let (iw, ih) = embedder.input_size();
    let kept: Vec<_> = found.iter().filter(|f| f.score >= MIN_SCORE).collect();
    let crops: Vec<Option<Vec<u8>>> =
        kept.iter().map(|f| align_to_template(&img, &f.landmarks.map(|(x, y)| (x * w as f32, y * h as f32)), iw, ih)).collect();
    let live: Vec<&[u8]> = crops.iter().filter_map(|c| c.as_deref()).collect();
    let mut embedded = embedder.embed_batch(&live).into_iter();
    kept.iter()
        .zip(crops)
        .filter_map(|(f, c)| {
            let emb = c.and_then(|_| embedded.next()?.ok())?;
            Some(VideoFace { rect: [f.x0.clamp(0.0, 1.0), f.y0.clamp(0.0, 1.0), f.x1.clamp(0.0, 1.0), f.y1.clamp(0.0, 1.0)], emb })
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn run(_: &mut Session, _: &Value) -> Result<Value> {
    Err(bad("video.faces", "faces of videos need the desktop app"))
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let j = &s.video.faces.job;
    let running = j.running.load(Ordering::SeqCst);
    let (done, total) = (j.done.load(Ordering::SeqCst), j.total.load(Ordering::SeqCst));
    if !running && done > 0 && done == total {
        // a finished run is written once, by whoever asks first
        let store = s.video.faces.store.clone();
        let tag = lock(&store).tag.clone();
        if !tag.is_empty() && !tag.starts_with('!') {
            save(s, &store);
        }
    }
    let j = &s.video.faces.job;
    Ok(json!({"job": {"running": running, "done": done, "total": total, "withFaces": j.with_faces.load(Ordering::SeqCst),
        "cancelled": j.cancel.load(Ordering::SeqCst), "error": j.error().clone()}}))
}

fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    let j = &s.video.faces.job;
    let running = j.running.load(Ordering::SeqCst);
    if running {
        j.cancel.store(true, Ordering::SeqCst);
    }
    Ok(json!({"cancelled": running}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "video.faces",
            "Find Faces in Videos",
            [],
            None,
            "{ids?, force?: bool, background?: bool} — finds, aligns and embeds the faces of the pictures cut from these videos (all videos when no ids) with the installed YuNet and recognition model (Settings ▸ Faces), and keeps them beside the library. Moments then show the people the photos name (a suggestion, never a confirmation) → {pictures, background, model}",
            always,
            run
        ),
        cmd!(
            query "video.facesStatus",
            "Video Faces Status",
            [],
            None,
            "{} — the faces run: → {job: {running, done, total, withFaces, cancelled, error}}",
            always,
            status
        ),
        cmd!(
            query "video.facesCancel",
            "Stop Finding Faces in Videos",
            [],
            None,
            "{} — stops the run after the picture in hand → {cancelled}",
            always,
            cancel
        ),
    ]
}
