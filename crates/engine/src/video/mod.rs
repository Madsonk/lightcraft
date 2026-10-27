//! Videos: what LightCraft knows of them besides their catalog entry (`MediaKind::Video`, duration).
//!
//! **Pictures of a video** ([`frames`]): a few pictures are cut out of each video ("frames", about
//! twelve per video by default, like Video Catcher) by FFmpeg run as a helper program
//! ([`ffmpeg`]; never linked, LightCraft stays pure Rust), and kept as JPEGs in
//! `<library>/video/<video key>/` with a small `frames.json` that lists them — never next to the
//! source, never in the catalog journal. The folder is a cache: deleting it loses work that can be
//! redone, nothing else.
//!
//! **One key for a moment of a video** ([`frame_key`]): `key_of("<content key>@<ms>")`. The face
//! index (`<library>/people/`) stores the faces of a picture of a video under that key, exactly as
//! it stores those of a photo under `key_of("<content key>")`; the annotation store
//! (`<library>/describe/`, [`crate::describe`]) uses the same key, so "who" (faces) and "what"
//! (scene, action, description) of one moment meet without being copied into each other.

pub mod faces;
#[cfg(not(target_arch = "wasm32"))]
pub mod ffmpeg;
pub mod frames;
#[cfg(not(target_arch = "wasm32"))]
pub mod unreadable;
#[cfg(not(target_arch = "wasm32"))]
pub mod videocatcher;
pub mod views;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::{MediaKind, PhotoId, Source};

/// File extensions LightCraft recognizes as videos (lower case). Folder imports do not pick them up
/// (yet); a video named explicitly (`library.import {paths: [file]}`) is catalogued.
pub const EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mkv", "avi", "mts", "m2ts", "webm", "3gp", "mpg", "mpeg", "wmv", "insv"];

/// Whether `path` names a video by its extension.
pub fn is_video_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// The face-index / annotation key of the picture of a video at `ms` milliseconds.
pub fn frame_key(content_key: &str, ms: u64) -> u64 {
    lightcraft_people_key(&format!("{content_key}@{ms}"))
}

/// The key of a whole catalog item (photo or video).
pub fn item_key(content_key: &str) -> u64 {
    lightcraft_people_key(content_key)
}

/// SipHash-1-3 with fixed keys (the same function as `lightcraft_people::key_of`, kept here so the
/// key exists in builds without face recognition).
fn lightcraft_people_key(s: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = siphasher::sip::SipHasher13::new_with_keys(0x4c43_5045_4f50_4c45, 0);
    h.write(s.as_bytes());
    h.finish()
}

/// `<library>/video`, when the library is on disk.
pub fn cache_root(s: &crate::Session) -> Option<PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("video"))
}

/// The folder of one video's pictures.
pub fn video_dir(root: &Path, content_key: &str) -> PathBuf {
    root.join(format!("{:016x}", item_key(content_key)))
}

/// Bytes of the pictures kept under `root` (as their lists say).
pub fn frames_bytes(root: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(root) else { return 0 };
    rd.flatten().take(1_000_000).filter_map(|e| frames::Manifest::load(&e.path())).map(|m| m.bytes()).sum()
}

/// A picture of a video, as the rest of LightCraft finds it.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameAt {
    /// The catalog items with this content (a video and its virtual copies).
    pub ids: Vec<PhotoId>,
    pub ms: u64,
    pub file: PathBuf,
}

/// The pictures of the library's videos by their key, built from the `frames.json` files, and
/// remembered until the videos or their pictures change.
#[derive(Default)]
pub struct Videos {
    cache: Option<(u64, Arc<HashMap<u64, FrameAt>>)>,
    /// Changes whenever pictures are added or removed.
    pub rev: u64,
    /// The `video.frames` run in progress (or the last one).
    pub job: Arc<FramesJob>,
    /// The faces found in the pictures (see [`faces`]).
    pub faces: faces::VideoFaces,
}

/// A `video.frames` run: progress, what each video gave, and its stop button.
#[derive(Default)]
pub struct FramesJob {
    pub running: std::sync::atomic::AtomicBool,
    pub cancel: std::sync::atomic::AtomicBool,
    pub total: std::sync::atomic::AtomicU64,
    pub done: std::sync::atomic::AtomicU64,
    /// Runs finished (the pictures of the library may have changed).
    pub runs: std::sync::atomic::AtomicU64,
    results: std::sync::Mutex<(Vec<(u64, frames::Cut)>, Option<String>)>,
}

impl FramesJob {
    fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<(u64, frames::Cut)>, Option<String>)> {
        self.results.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Start a run of `total` videos; false when one is running.
    pub fn begin(&self, total: usize) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        if self.running.swap(true, SeqCst) {
            return false;
        }
        self.cancel.store(false, SeqCst);
        self.total.store(total as u64, SeqCst);
        self.done.store(0, SeqCst);
        *self.lock() = (Vec::new(), None);
        true
    }

    pub fn record(&self, id: u64, cut: frames::Cut) {
        self.done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lock().0.push((id, cut));
    }

    pub fn finish(&self, error: Option<String>) {
        use std::sync::atomic::Ordering::SeqCst;
        self.lock().1 = error;
        self.runs.fetch_add(1, SeqCst);
        self.running.store(false, SeqCst);
    }

    /// Ask the run to stop; true when one was running.
    pub fn cancel_run(&self) -> bool {
        let running = self.running.load(std::sync::atomic::Ordering::SeqCst);
        if running {
            self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        running
    }

    /// What the run did (each video's result with `results`).
    pub fn report(&self, results: bool) -> serde_json::Value {
        use std::sync::atomic::Ordering::SeqCst;
        let g = self.lock();
        let r = &g.0;
        let sum = |f: fn(&frames::Cut) -> usize| r.iter().map(|(_, c)| f(c)).sum::<usize>();
        let mut v = serde_json::json!({
            "running": self.running.load(SeqCst), "cancelled": self.cancel.load(SeqCst),
            "total": self.total.load(SeqCst), "done": self.done.load(SeqCst), "videos": r.len(),
            "cut": sum(|c| c.cut), "kept": sum(|c| c.kept), "failed": sum(|c| c.failed),
            "bytes": r.iter().map(|(_, c)| c.bytes).sum::<u64>(), "overBudget": r.iter().any(|(_, c)| c.over_budget),
            "unreadable": r.iter().filter(|(_, c)| c.error.is_some() && c.cut + c.kept == 0).map(|(i, _)| *i).collect::<Vec<_>>(),
            "error": g.1,
        });
        if results {
            v["results"] = r
                .iter()
                .map(|(id, c)| {
                    let mut x = serde_json::json!(c);
                    x["id"] = serde_json::json!(id);
                    x
                })
                .collect();
        }
        v
    }
}

fn signature(s: &crate::Session, root: &Path, rev: u64) -> u64 {
    use std::hash::Hasher;
    let mut h = siphasher::sip::SipHasher13::new();
    h.write(root.as_os_str().as_encoded_bytes());
    h.write_u64(rev);
    h.write_u64(s.video.job.runs.load(std::sync::atomic::Ordering::SeqCst));
    for p in s.catalog.photos().filter(|p| p.in_library() && p.kind == MediaKind::Video) {
        h.write_u64(p.id.0);
        h.write(crate::media::content_key(p).as_bytes());
    }
    h.finish()
}

/// Lists of pictures read at once when the remembered ones are rebuilt.
const LOAD_THREADS: usize = 32;

impl crate::Session {
    /// The pictures of the library's videos, by key (empty without a library on disk).
    pub fn video_frames(&mut self) -> Arc<HashMap<u64, FrameAt>> {
        let Some(root) = cache_root(self) else { return Arc::default() };
        let sig = signature(self, &root, self.video.rev);
        if let Some((s, m)) = &self.video.cache
            && *s == sig
        {
            return m.clone();
        }
        let mut by_content: HashMap<String, Vec<PhotoId>> = HashMap::new();
        for p in self.catalog.photos().filter(|p| p.in_library() && p.kind == MediaKind::Video) {
            by_content.entry(crate::media::content_key(p)).or_default().push(p.id);
        }
        // the lists are read side by side: on a cold or busy disk each open waits tens of
        // milliseconds, and 355 of them one after the other kept the Videos view blank for 30 s
        let jobs: Vec<(String, Vec<PhotoId>)> = by_content.into_iter().collect();
        let load = |chunk: &[(String, Vec<PhotoId>)]| -> Vec<(String, Vec<PhotoId>, PathBuf, frames::Manifest)> {
            chunk
                .iter()
                .filter_map(|(ck, ids)| {
                    let dir = video_dir(&root, ck);
                    let m = frames::Manifest::load(&dir)?;
                    (m.content_key == *ck).then(|| (ck.clone(), ids.clone(), dir, m))
                })
                .collect()
        };
        let threads = if cfg!(target_arch = "wasm32") { 1 } else { jobs.len().clamp(1, LOAD_THREADS) };
        let loaded = if threads == 1 {
            load(&jobs)
        } else {
            let per = jobs.len().div_ceil(threads);
            std::thread::scope(|sc| {
                let handles: Vec<_> = jobs.chunks(per).map(|chunk| sc.spawn(|| load(chunk))).collect();
                handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            })
        };
        let mut map = HashMap::new();
        for (ck, ids, dir, m) in loaded {
            for f in m.frames {
                map.insert(frame_key(&ck, f.ms), FrameAt { ids: ids.clone(), ms: f.ms, file: dir.join(&f.file) });
            }
        }
        let m = Arc::new(map);
        self.video.cache = Some((sig, m.clone()));
        m
    }

    /// The pictures of one video, in time order.
    pub fn frames_of_video(&mut self, id: PhotoId) -> Vec<(u64, FrameAt)> {
        let Some(p) = self.catalog.photo(id).filter(|p| p.kind == MediaKind::Video) else { return Vec::new() };
        let ck = crate::media::content_key(p);
        let Some(root) = cache_root(self) else { return Vec::new() };
        let dir = video_dir(&root, &ck);
        let Some(m) = frames::Manifest::load(&dir) else { return Vec::new() };
        if m.content_key != ck {
            return Vec::new();
        }
        let mut v: Vec<(u64, FrameAt)> =
            m.frames.into_iter().map(|f| (frame_key(&ck, f.ms), FrameAt { ids: vec![id], ms: f.ms, file: dir.join(&f.file) })).collect();
        v.sort_by_key(|(_, f)| f.ms);
        v
    }
}

/// The source path of a catalog video.
pub fn source_path(p: &lightcraft_catalog::Photo) -> Option<&str> {
    match &p.source {
        Source::File { path } if p.kind == MediaKind::Video => Some(path),
        _ => None,
    }
}

/// A content key for a video that does not need the whole file read (videos are large, often on a
/// network share): size, then 1 MiB from the start, the middle and the end. Not a 32-digit
/// [`lightcraft_preview::hash_bytes`], so Import ▸ Copy compares a copy byte for byte instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn sampled_hash(path: &Path) -> Result<String, String> {
    use std::io::{Read, Seek, SeekFrom};
    const PART: u64 = 1 << 20;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut h = lightcraft_preview::Hasher128::new();
    h.u64(len);
    let mut buf = vec![0u8; PART as usize];
    for at in [0, (len / 2).saturating_sub(PART / 2), len.saturating_sub(PART)] {
        f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        let n = f.by_ref().take(PART).read(&mut buf).map_err(|e| e.to_string())?;
        h.update(buf.get(..n).unwrap_or(&[]));
    }
    Ok(format!("vs1-{}", h.finish()))
}

/// Probe a video for import: size and duration from `ffprobe`, the content key from samples.
#[cfg(not(target_arch = "wasm32"))]
pub fn probe_for_import(path: &str) -> Result<crate::media::ProbeInfo, String> {
    let p = Path::new(path);
    let file_size = std::fs::metadata(p).map_err(|e| format!("{path}: {e}"))?.len();
    if file_size == 0 {
        return Err("empty file (0 bytes)".into());
    }
    let info = ffmpeg::probe(p)?;
    let format = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_else(|| "VIDEO".into());
    // `2026-07-22T10:11:12.000000Z` → `2026-07-22T10:11:12`
    let captured = info.created.as_deref().and_then(|c| c.get(..19)).filter(|c| c.as_bytes().get(10) == Some(&b'T')).map(str::to_string);
    Ok(crate::media::ProbeInfo {
        width: info.width,
        height: info.height,
        format,
        kind: MediaKind::Video,
        file_size,
        captured,
        content_hash: Some(sampled_hash(p)?),
        duration: info.duration,
        ..Default::default()
    })
}

/// A poster picture of a video for the grid (1 s in, else its first picture), decoded like a JPEG.
#[cfg(not(target_arch = "wasm32"))]
pub fn poster_bytes(path: &str, max_edge: usize) -> Result<Vec<u8>, String> {
    let edge = u32::try_from(max_edge.clamp(64, 2560)).unwrap_or(2560);
    let p = Path::new(path);
    if std::fs::metadata(p).map_err(|e| format!("{path}: {e}"))?.len() == 0 {
        return Err("empty file (0 bytes)".into());
    }
    ffmpeg::frame_bytes(p, 1000, edge).or_else(|_| ffmpeg::frame_bytes(p, 0, edge))
}

#[cfg(target_arch = "wasm32")]
pub fn probe_for_import(_: &str) -> Result<crate::media::ProbeInfo, String> {
    Err("videos are not supported in this build".into())
}

#[cfg(target_arch = "wasm32")]
pub fn poster_bytes(_: &str, _: usize) -> Result<Vec<u8>, String> {
    Err("videos are not supported in this build".into())
}

/// JPEG bytes as sRGB RGBA, at most `max` pixels on the long side: `(width, height, pixels)`.
pub fn jpeg_rgba(bytes: &[u8], max: u32) -> Option<(usize, usize, Vec<u8>)> {
    let d =
        lightcraft_codecs::decode(bytes, lightcraft_codecs::DecodeOptions { max_size: Some((max.max(1), max.max(1))), ..Default::default() }).ok()?;
    let img = d.to_srgb8();
    Some((img.width, img.height, img.as_bytes()))
}

/// A picture of a moment (a JPEG of `<library>/video/`) as sRGB RGBA bytes, at most `max` pixels
/// on its long side, for a frontend to show: `(width, height, pixels)`. `None` when it cannot be
/// read (missing, damaged).
pub fn picture_rgba(path: &Path, max: u32) -> Option<(usize, usize, Vec<u8>)> {
    let len = std::fs::metadata(path).ok()?.len();
    if len == 0 || len > 64 << 20 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let d = lightcraft_codecs::decode(&bytes, lightcraft_codecs::DecodeOptions { max_size: Some((max.max(1), max.max(1))), ..Default::default() })
        .ok()?;
    let img = d.to_srgb8();
    Some((img.width, img.height, img.as_bytes()))
}

#[cfg(test)]
mod tests {

    #[test]
    fn video_paths_by_extension() {
        assert!(super::is_video_path("/a/GOPR0953.MP4") && super::is_video_path("x.mov"));
        assert!(!super::is_video_path("/a/b.jpg") && !super::is_video_path("mp4") && !super::is_video_path(""));
    }
}
