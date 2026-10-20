//! Videos: `video.*` — cut pictures out of the catalog's videos (so People recognizes the faces in
//! them), bring in what Video Catcher already did, and `describe.list` (the annotations of a photo
//! or of the moments of a video). See `crate::video`, `crate::describe`, `docs/people.md`,
//! `docs/describe.md`.

use serde_json::{Value, json};

use super::{CommandSpec, always, cmd};
#[cfg(not(target_arch = "wasm32"))]
use super::{bad, bool_or, f64_or, str_param};
use crate::{EngineError, Result, Session};

/// Disk the pictures of a library's videos may take by default.
pub const DEFAULT_MAX_BYTES: u64 = 4 << 30;

#[allow(dead_code)]
fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "video.status",
            "Video Status",
            [],
            None,
            "{} — whether FFmpeg is there (`ffmpeg`, `ffprobe`: found on PATH, in /opt/homebrew/bin, /usr/local/bin, or LIGHTCRAFT_FFMPEG / LIGHTCRAFT_FFPROBE), the library's videos and how many have their pictures cut, the pictures' folder and its size → {ffmpeg, ffprobe, videos, withFrames, frames, bytes, folder}",
            always,
            status
        ),
        cmd!(
            query "video.frames",
            "Cut Pictures from Videos",
            [],
            None,
            "{ids? (default: every video of the library), every?: seconds between pictures (default: 12 per video, one every 2 s for a short one), max?: n per video (120, at most 240), force?: bool (cut again), maxBytes?: disk for all the library's pictures (4 GiB)} — cut pictures (JPEG, 1280 px) out of the videos with FFmpeg into `<library>/video/<key>/` (never next to the video, never in the catalog), listed in its `frames.json`; an interrupted run resumes (pictures already there are kept). Then `people.index` finds the faces in them → {videos, cut, kept, failed, bytes, overBudget, results: [{id, planned, cut, kept, failed, error}]}",
            always,
            frames
        ),
        cmd!(
            query "video.importVideoCatcher",
            "Import from Video Catcher",
            [],
            None,
            "{db: path of video-catcher.sqlite3, dryRun?: bool, maxBytes?} — read (never write) a Video Catcher database: its pictures of the catalog's videos (matched by path) are copied into `<library>/video/<key>/`, its described moments (scene, action, description) become candidate annotations in `<library>/describe/` → {videos, matched, unmatched, framesCopied, framesKept, framesMissing, moments, annotationsAdded, annotationsUnchanged}",
            always,
            import_videocatcher
        ),
        cmd!(
            query "describe.list",
            "Annotations",
            [],
            None,
            "{id?} (default: the active photo) — what a photo, or each moment of a video, shows: scene, action, description, status (candidate | confirmed), source, and who is there (from the faces found under the same key; nothing copied) → {id, annotations: [{key, ms, scene, action, description, status, source, people: [{face, person, name}], frame}], total}",
            always,
            describe_list
        ),
    ]
}

#[cfg(target_arch = "wasm32")]
fn status(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"available": false}))
}
#[cfg(target_arch = "wasm32")]
fn frames(_: &mut Session, _: &Value) -> Result<Value> {
    Err(other("videos are not available in this build"))
}
#[cfg(target_arch = "wasm32")]
fn import_videocatcher(_: &mut Session, _: &Value) -> Result<Value> {
    Err(other("videos are not available in this build"))
}

#[cfg(not(target_arch = "wasm32"))]
fn status(s: &mut Session, _: &Value) -> Result<Value> {
    use crate::video::{cache_root, ffmpeg};
    let root = cache_root(s);
    let videos: Vec<lightcraft_catalog::PhotoId> =
        s.catalog.photos().filter(|p| p.in_library() && p.kind == lightcraft_catalog::MediaKind::Video).map(|p| p.id).collect();
    let mut with = 0;
    let mut n = 0;
    for id in &videos {
        let f = s.frames_of_video(*id).len();
        n += f;
        with += usize::from(f > 0);
    }
    Ok(json!({
        "available": true,
        "ffmpeg": ffmpeg::ffmpeg().map(|p| p.display().to_string()), "ffprobe": ffmpeg::ffprobe().map(|p| p.display().to_string()),
        "videos": videos.len(), "withFrames": with, "frames": n,
        "bytes": root.as_deref().map(crate::video::frames_bytes), "folder": root.map(|r| r.display().to_string()),
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn frames(s: &mut Session, p: &Value) -> Result<Value> {
    use std::sync::atomic::{AtomicBool, AtomicU64};

    use crate::video::{cache_root, frames, source_path, video_dir};
    const C: &str = "video.frames";
    let root = cache_root(s).ok_or_else(|| bad(C, "the library is not on disk: its videos' pictures need a folder"))?;
    if crate::video::ffmpeg::ffmpeg().is_none() {
        return Err(other(format!(
            "{}: `ffmpeg` was not found (install FFmpeg, or set {})",
            crate::video::ffmpeg::NOT_FOUND,
            crate::video::ffmpeg::FFMPEG_ENV
        )));
    }
    let every = p.get("every").and_then(Value::as_f64);
    let max = f64_or(p, "max", 120.0).clamp(1.0, frames::MAX_FRAMES as f64) as usize;
    let force = bool_or(p, "force", false);
    let max_bytes = f64_or(p, "maxBytes", DEFAULT_MAX_BYTES as f64).clamp(0.0, 1e15) as u64;
    let ids: Vec<lightcraft_catalog::PhotoId> =
        if p.get("ids").is_some() { s.targets(p) } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
    // the work, detached from the session (one content once, even with virtual copies)
    let mut seen = std::collections::HashSet::new();
    let mut jobs = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id) else { continue };
        let Some(path) = source_path(ph) else { continue };
        let ck = crate::media::content_key(ph);
        if !seen.insert(ck.clone()) {
            continue;
        }
        jobs.push((id, std::path::PathBuf::from(path), ck, ph.duration));
    }
    let budget = AtomicU64::new(max_bytes.saturating_sub(crate::video::frames_bytes(&root)));
    let cancel = AtomicBool::new(false);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build().map_err(|e| other(e.to_string()))?;
    let results: Vec<(u64, frames::Cut)> = pool.install(|| {
        use rayon::prelude::*;
        jobs.par_iter()
            .map(|(id, path, ck, duration)| {
                // a video catalogued without its duration (or a damaged one): ask the file
                let duration =
                    crate::video::ffmpeg::sane_duration(*duration).or_else(|| crate::video::ffmpeg::probe(path).ok().and_then(|i| i.duration));
                let times = frames::plan(duration, every, max);
                let ms = duration.map(|d| (d * 1000.0) as u64);
                let cut = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    frames::extract(path, ck, ms, &video_dir(&root, ck), &times, force, &budget, &cancel)
                }))
                .unwrap_or_else(|_| frames::Cut { error: Some("cutting the pictures failed unexpectedly".into()), ..Default::default() });
                (id.0, cut)
            })
            .collect()
    });
    s.video.rev += 1;
    for (id, c) in &results {
        if c.error.is_some() && c.cut + c.kept == 0 {
            skipped.push(*id);
        }
    }
    let sum = |f: fn(&frames::Cut) -> usize| results.iter().map(|(_, c)| f(c)).sum::<usize>();
    Ok(json!({
        "videos": results.len(), "cut": sum(|c| c.cut), "kept": sum(|c| c.kept), "failed": sum(|c| c.failed),
        "bytes": results.iter().map(|(_, c)| c.bytes).sum::<u64>(), "overBudget": results.iter().any(|(_, c)| c.over_budget),
        "folder": root.display().to_string(), "unreadable": skipped,
        "results": results.iter().map(|(id, c)| { let mut v = json!(c); v["id"] = json!(id); v }).collect::<Vec<_>>(),
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn import_videocatcher(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.importVideoCatcher";
    let db = str_param(p, "db").filter(|d| !d.trim().is_empty()).ok_or_else(|| bad(C, "missing `db` (the path of video-catcher.sqlite3)"))?;
    let dry = bool_or(p, "dryRun", false);
    let max_bytes = f64_or(p, "maxBytes", DEFAULT_MAX_BYTES as f64).clamp(0.0, 1e15) as u64;
    let r = crate::video::videocatcher::import(s, std::path::Path::new(db), dry, max_bytes).map_err(|e| bad(C, e))?;
    Ok(r.json(dry))
}

fn describe_list(s: &mut Session, p: &Value) -> Result<Value> {
    let id =
        p.get("id").and_then(Value::as_u64).map(lightcraft_catalog::PhotoId).or(s.active()).ok_or_else(|| super::bad("describe.list", "no photo"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| super::bad("describe.list", "no such photo"))?;
    let item = crate::media::content_key(ph);
    let dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
    let store = crate::describe::Store::open(dir.as_deref());
    let frames: std::collections::HashMap<u64, String> = s.frames_of_video(id).into_iter().map(|(k, f)| (k, f.file.display().to_string())).collect();
    let mut rows = Vec::new();
    for a in store.of_item(&item) {
        let mut v = json!(a);
        let key = u64::from_str_radix(&a.key, 16).unwrap_or(0);
        #[cfg(feature = "people")]
        {
            v["people"] = json!(crate::people::ops::people_at(s, key));
        }
        if let Some(f) = frames.get(&key) {
            v["frame"] = json!(f);
        }
        rows.push(v);
    }
    Ok(json!({"id": id.0, "total": rows.len(), "annotations": rows, "skippedLines": store.skipped}))
}
