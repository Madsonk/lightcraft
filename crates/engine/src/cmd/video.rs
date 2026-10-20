//! Videos: `video.*` — cut pictures out of the catalog's videos (so People recognizes the faces in
//! them) and bring in what Video Catcher already did. See `crate::video`, `crate::describe`, `docs/people.md`,
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
            "{ids? (default: every video of the library), background?: bool (false: wait; true: return at once, follow video.status → job, stop with video.cancel), every?: seconds between pictures (default: 12 per video, one every 2 s for a short one), max?: n per video (120, at most 240), force?: bool (cut again), maxBytes?: disk for all the library's pictures (4 GiB)} — cut pictures (JPEG, 1280 px) out of the videos with FFmpeg into `<library>/video/<key>/` (never next to the video, never in the catalog), listed in its `frames.json`; an interrupted run resumes (pictures already there are kept). Then `people.index` finds the faces in them → {videos, cut, kept, failed, bytes, overBudget, results: [{id, planned, cut, kept, failed, error}]}",
            always,
            frames
        ),
        cmd!(
            query "video.list",
            "Videos",
            [],
            None,
            "{person?: name, described?: bool, status?: confirmed | toConfirm, text?} — the library's videos with who is in them (named people of their pictures), how far their analysis went (pictures cut, pictures looked at for faces, moments described, confirmed) → {videos: [{id, name, duration, captured, frames, facesLooked, people, moments, confirmed, toConfirm, poster}], people, total}",
            always,
            crate::video::views::list
        ),
        cmd!(
            query "video.moments",
            "Moments of a Video",
            [],
            None,
            "{id} — a video's pictures and described moments in time order (a photo: its one moment), each with its picture, its faces and who they are, and its description (value, status, sources, proposals) → {id, video, name, duration, moments: [{ms, frame, faces, people, annotation}]}",
            always,
            crate::video::views::moments
        ),
        cmd!(
            query "video.open",
            "Play Video",
            [],
            None,
            "{id, ms?, dryRun?} — open the video in the system's player at `ms` (VLC when installed, `--start-time`; otherwise the default player, from the start) → {program, args, atTime, timecode}",
            always,
            crate::video::views::open
        ),
        cmd!(
            query "video.cancel",
            "Stop Cutting Pictures",
            [],
            None,
            "{} — stop video.frames after the pictures being cut (what was cut is kept; running it again resumes) → {cancelled}",
            always,
            cancel
        ),
        cmd!(
            query "video.importVideoCatcher",
            "Import from Video Catcher",
            [],
            None,
            "{db: path of video-catcher.sqlite3, createMissing?: bool (also `videos`; false: only the catalog's videos; true: its videos the catalog lacks are imported first, by file, read only), dryRun?: bool, maxBytes?} — read (never write) a Video Catcher database (and its files): its pictures of the catalog's videos (matched by path) are copied into `<library>/video/<key>/`, its described moments (scene, action, description) become candidate annotations in `<library>/describe/` → {videos, matched, unmatched, framesCopied, framesKept, framesMissing, moments, annotationsAdded, annotationsUnchanged}",
            always,
            import_videocatcher
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
fn cancel(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"cancelled": false}))
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
        "job": s.video.job.report(false),
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn frames(s: &mut Session, p: &Value) -> Result<Value> {
    use std::sync::atomic::{AtomicU64, Ordering};

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
    let background = bool_or(p, "background", false);
    let max_bytes = f64_or(p, "maxBytes", DEFAULT_MAX_BYTES as f64).clamp(0.0, 1e15) as u64;
    let ids: Vec<lightcraft_catalog::PhotoId> =
        if p.get("ids").is_some() { s.targets(p) } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
    // the work, detached from the session (one content once, even with virtual copies)
    let mut seen = std::collections::HashSet::new();
    let mut jobs = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id) else { continue };
        let Some(path) = source_path(ph) else { continue };
        let ck = crate::media::content_key(ph);
        if !seen.insert(ck.clone()) {
            continue;
        }
        jobs.push((id, std::path::PathBuf::from(path), ck, ph.duration));
    }
    let job = s.video.job.clone();
    if !job.begin(jobs.len()) {
        return Err(other("pictures are already being cut (see video.status; video.cancel stops it)"));
    }
    let work_job = job.clone();
    let work = move || {
        let job = work_job;
        let budget = AtomicU64::new(max_bytes.saturating_sub(crate::video::frames_bytes(&root)));
        let pool = match rayon::ThreadPoolBuilder::new().num_threads(3).build() {
            Ok(p) => p,
            Err(e) => {
                job.finish(Some(e.to_string()));
                return;
            }
        };
        pool.install(|| {
            use rayon::prelude::*;
            jobs.par_iter().for_each(|(id, path, ck, duration)| {
                if job.cancel.load(Ordering::SeqCst) {
                    return;
                }
                let cut = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // a video catalogued without its duration (or a damaged one): ask the file
                    let duration =
                        crate::video::ffmpeg::sane_duration(*duration).or_else(|| crate::video::ffmpeg::probe(path).ok().and_then(|i| i.duration));
                    let times = frames::plan(duration, every, max);
                    let ms = duration.map(|d| (d * 1000.0) as u64);
                    frames::extract(path, ck, ms, &video_dir(&root, ck), &times, force, &budget, &job.cancel)
                }))
                .unwrap_or_else(|_| frames::Cut { error: Some("cutting the pictures failed unexpectedly".into()), ..Default::default() });
                job.record(id.0, cut);
            });
        });
        job.finish(None);
    };
    let handle = std::thread::Builder::new().name("video-frames".into()).spawn(work).map_err(|e| {
        job.finish(Some(e.to_string()));
        other(format!("could not start cutting the pictures: {e}"))
    })?;
    if background {
        return Ok(json!({"background": true, "videos": job.total.load(Ordering::SeqCst)}));
    }
    let _ = handle.join();
    s.video.rev += 1;
    Ok(job.report(true))
}

#[cfg(not(target_arch = "wasm32"))]
fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"cancelled": s.video.job.cancel_run()}))
}

#[cfg(not(target_arch = "wasm32"))]
fn import_videocatcher(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.importVideoCatcher";
    let db = str_param(p, "db").filter(|d| !d.trim().is_empty()).ok_or_else(|| bad(C, "missing `db` (the path of video-catcher.sqlite3)"))?;
    let dry = bool_or(p, "dryRun", false);
    let max_bytes = f64_or(p, "maxBytes", DEFAULT_MAX_BYTES as f64).clamp(0.0, 1e15) as u64;
    let create = bool_or(p, "createMissing", bool_or(p, "videos", false));
    let r = crate::video::videocatcher::import(s, std::path::Path::new(db), dry, max_bytes, create).map_err(|e| bad(C, e))?;
    Ok(r.json(dry))
}
