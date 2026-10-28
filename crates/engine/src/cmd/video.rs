//! Videos: `video.*` — cut pictures out of the catalog's videos (so People recognizes the faces in
//! them) and bring in what Video Catcher already did. See `crate::video`, `crate::describe`, `docs/people.md`,
//! `docs/describe.md`.

use serde_json::{Value, json};

use super::{CommandSpec, always, cmd};
#[cfg(not(target_arch = "wasm32"))]
use super::{bad, bool_or, f64_or, str_param};
use crate::{EngineError, Result, Session};

/// Disk the pictures of a library's videos may take by default.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
            "{jobOnly?: bool (only `job`, reads no file: what a window polls)} — whether FFmpeg is there (`ffmpeg`, `ffprobe`: found on PATH, in /opt/homebrew/bin, /usr/local/bin, or LIGHTCRAFT_FFMPEG / LIGHTCRAFT_FFPROBE), the library's videos and how many have their pictures cut, the pictures' folder and its size → {ffmpeg, ffprobe, videos, withFrames, frames, bytes, folder}",
            always,
            status
        ),
        cmd!(
            query "video.frames",
            "Cut Pictures from Videos",
            [],
            None,
            "{ids? (default: every video of the library), background?: bool (false: wait; true: return at once, follow video.status → job, stop with video.cancel), every?: seconds between pictures (default: 12 per video, one every 2 s for a short one), max?: n per video (120, at most 240), plan?: even | scenes (scenes: one picture per shot and more in a long shot, found by reading the whole video; shots not found → even), sceneThreshold? (0.3), maxGap?: seconds a shot may go without a picture (8), retryFailed?: bool (only the moments a previous run could not cut, and the ones 0.7 s beside them), force?: bool (cut again), maxBytes?: disk for all the library's pictures (4 GiB)} — cut pictures (JPEG, 1280 px) out of the videos with FFmpeg into `<library>/video/<key>/` (never next to the video, never in the catalog), listed in its `frames.json`; an interrupted run resumes (pictures already there are kept). Then `people.index` finds the faces in them → {videos, cut, kept, failed, bytes, overBudget, results: [{id, planned, cut, kept, failed, error}]}",
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
            query "video.person",
            "Faces of a Person in Videos",
            [],
            None,
            "{name, limit?: n (200)} — the pictures of videos where this person is suggested by the photos' named faces (People ▸ Recognition), by video then time → {name, total, videos, faces: [{video, file, ms, frame, rect}]}",
            always,
            crate::video::views::person
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
            query "video.clip",
            "Clip of a Moment",
            [],
            None,
            "{id, ms, seconds?: 1..30 (8)} — a short clip around the moment (half before, half after; 540 lines, H.264 + AAC, MP4), made once by FFmpeg and kept in `<library>/video/<key>/clips/`, for a player → {path, startMs, lengthMs, ms, cached, bytes, timecode}",
            always,
            clip
        ),
        cmd!(
            query "video.unreadable",
            "Unreadable Videos",
            [],
            None,
            "{} — the videos imports could not read, with their cause (`empty`: 0 bytes; `noIndex`: no moov, the file was cut short — not repairable without a sound file of the same camera; `noPicture`; `missing`; `other`) and folder → {videos: [{path, folder, name, cause, error, bytes}], total}",
            always,
            unreadable
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
            query "video.transcribe",
            "Transcribe Videos",
            [],
            None,
            "{ids? (default: every video of the library), language?: auto | fr | en… (auto), model?: (mlx-community/whisper-large-v3-turbo), force?: bool (listen again), background?: bool (false: wait; true: return at once, follow video.status → speech, stop with video.cancel)} — listen to the videos' soundtracks with `mlx_whisper` (installed by `pip install mlx-whisper`; one video at a time) and keep the sentences, cleaned of Whisper's loops and stock phrases, in `<library>/video/<key>/speech.json`; a video already transcribed is kept. `describe.search` then also finds what is said → {videos, transcribed, kept, failed: [{id, error}]}",
            always,
            transcribe
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
fn transcribe(_: &mut Session, _: &Value) -> Result<Value> {
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
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::video::{cache_root, ffmpeg};
    // `jobOnly`: what a window polls every few seconds — no file read at all (the counts below read
    // every video's list of pictures, which froze the window on a slow disk)
    if bool_or(p, "jobOnly", false) {
        return Ok(json!({"available": true, "job": s.video.job.report(false), "speech": s.video.speech.report()}));
    }
    let root = cache_root(s);
    let videos: Vec<lightcraft_catalog::PhotoId> =
        s.catalog.photos().filter(|p| p.in_library() && p.kind == lightcraft_catalog::MediaKind::Video).map(|p| p.id).collect();
    // counted from the remembered pictures (read once, until the videos or their pictures change)
    let mut per: std::collections::HashMap<lightcraft_catalog::PhotoId, usize> = std::collections::HashMap::new();
    for f in s.video_frames().values() {
        for id in &f.ids {
            *per.entry(*id).or_default() += 1;
        }
    }
    let n: usize = videos.iter().map(|id| per.get(id).copied().unwrap_or(0)).sum();
    let with = videos.iter().filter(|id| per.get(id).is_some_and(|c| *c > 0)).count();
    Ok(json!({
        "available": true,
        "ffmpeg": ffmpeg::ffmpeg().map(|p| p.display().to_string()), "ffprobe": ffmpeg::ffprobe().map(|p| p.display().to_string()),
        "videos": videos.len(), "withFrames": with, "frames": n,
        "bytes": root.as_deref().map(crate::video::frames_bytes), "folder": root.map(|r| r.display().to_string()),
        "job": s.video.job.report(false), "speech": s.video.speech.report(),
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
    // `plan`: "even" (the default: about twelve pictures spread over the video) or "scenes" (one
    // picture per shot, more in a long shot; reads the whole video to find the cuts)
    let by_scenes = match str_param(p, "plan").unwrap_or("even") {
        "even" => false,
        "scenes" => true,
        _ => return Err(bad(C, "`plan` is even or scenes")),
    };
    let threshold = f64_or(p, "sceneThreshold", crate::video::scenes::THRESHOLD);
    let max_gap = (f64_or(p, "maxGap", crate::video::scenes::MAX_GAP_MS as f64 / 1000.0).clamp(1.0, 600.0) * 1000.0) as u64;
    let retry_failed = bool_or(p, "retryFailed", false);
    let background = bool_or(p, "background", false);
    let max_bytes = f64_or(p, "maxBytes", DEFAULT_MAX_BYTES as f64).clamp(0.0, 1e15) as u64;
    let ids: Vec<lightcraft_catalog::PhotoId> =
        if p.get("ids").is_some() { s.targets(p)? } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
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
                    let ms = duration.map(|d| (d * 1000.0) as u64);
                    let mut note = None;
                    if retry_failed {
                        // only the moments the last run could not cut, and their neighbours
                        let dir = video_dir(&root, ck);
                        let times =
                            frames::Manifest::load(&dir).filter(|m| m.content_key == *ck).map(|m| frames::retry_times(&m)).unwrap_or_default();
                        if times.is_empty() {
                            return frames::Cut::default();
                        }
                        let cut = frames::extract(path, ck, ms, &dir, &times, false, &budget, &job.cancel);
                        frames::resolve_failed(&dir);
                        return cut;
                    }
                    let times = match (by_scenes, ms) {
                        (true, Some(len)) => match crate::video::scenes::detect(path, threshold) {
                            Ok(cuts) => crate::video::scenes::plan(len, &cuts, max_gap, max),
                            Err(e) => {
                                note = Some(format!("shots not found ({e}): evenly spread pictures instead"));
                                frames::plan(duration, every, max)
                            }
                        },
                        _ => frames::plan(duration, every, max),
                    };
                    let mut cut = frames::extract(path, ck, ms, &video_dir(&root, ck), &times, force, &budget, &job.cancel);
                    if cut.error.is_none() {
                        cut.note = note;
                    }
                    cut
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
fn clip(s: &mut Session, p: &Value) -> Result<Value> {
    crate::video::views::clip(s, p)
}
#[cfg(target_arch = "wasm32")]
fn clip(_: &mut Session, _: &Value) -> Result<Value> {
    Err(other("videos are not available in this build"))
}

#[cfg(not(target_arch = "wasm32"))]
fn unreadable(s: &mut Session, _: &Value) -> Result<Value> {
    let list = crate::video::cache_root(s).map(|r| crate::video::unreadable::load(&r)).unwrap_or_default();
    let rows: Vec<Value> = list
        .iter()
        .map(|e| {
            let p = std::path::Path::new(&e.path);
            json!({"path": e.path, "folder": p.parent().map(|f| f.display().to_string()), "name": p.file_name().map(|n| n.to_string_lossy().to_string()),
                   "cause": e.cause, "error": e.error, "bytes": e.bytes, "exists": p.exists()})
        })
        .collect();
    Ok(json!({"total": rows.len(), "videos": rows}))
}
#[cfg(target_arch = "wasm32")]
fn unreadable(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"total": 0, "videos": []}))
}

#[cfg(not(target_arch = "wasm32"))]
fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    let (a, b) = (s.video.job.cancel_run(), s.video.speech.cancel_run());
    Ok(json!({"cancelled": a || b}))
}

#[cfg(not(target_arch = "wasm32"))]
fn transcribe(s: &mut Session, p: &Value) -> Result<Value> {
    use std::sync::atomic::Ordering;

    use crate::video::{cache_root, source_path, speech, video_dir};
    const C: &str = "video.transcribe";
    let root = cache_root(s).ok_or_else(|| bad(C, "the library is not on disk: its videos' sentences need a folder"))?;
    if crate::video::ffmpeg::ffmpeg().is_none() || speech::whisper().is_none() {
        return Err(other(format!(
            "ffmpeg or mlx_whisper was not found (install FFmpeg and `pip install mlx-whisper`, or set {} / {})",
            crate::video::ffmpeg::FFMPEG_ENV,
            speech::WHISPER_ENV
        )));
    }
    let language = str_param(p, "language").unwrap_or("auto").trim().to_lowercase();
    if language.is_empty() || language.len() > 12 || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(bad(C, "`language` is auto or a language code (fr, en…)"));
    }
    let model = str_param(p, "model").unwrap_or(speech::DEFAULT_MODEL).to_string();
    let force = bool_or(p, "force", false);
    let background = bool_or(p, "background", false);
    let ids: Vec<lightcraft_catalog::PhotoId> = if p.get("ids").is_some() {
        s.targets(p)?
    } else {
        s.catalog.photos().filter(|q| q.in_library() && q.kind == lightcraft_catalog::MediaKind::Video).map(|q| q.id).collect()
    };
    let mut seen = std::collections::HashSet::new();
    let mut jobs = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id).filter(|q| q.kind == lightcraft_catalog::MediaKind::Video) else { continue };
        let Some(path) = source_path(ph) else { continue };
        let ck = crate::media::content_key(ph);
        if !seen.insert(ck.clone()) {
            continue;
        }
        let dir = video_dir(&root, &ck);
        if !force && speech::Speech::load(&dir).is_some() {
            continue;
        }
        jobs.push((id, std::path::PathBuf::from(path), ck, dir));
    }
    let job = s.video.speech.clone();
    if !job.begin(jobs.len()) {
        return Err(other("videos are already being transcribed (see video.status → speech; video.cancel stops it)"));
    }
    let work_job = job.clone();
    let work = move || {
        let job = work_job;
        for (id, path, ck, dir) in &jobs {
            if job.cancel.load(Ordering::SeqCst) {
                break;
            }
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| speech::transcribe(path, ck, dir, &model, &language)))
                .unwrap_or_else(|_| Err("transcription failed unexpectedly".into()));
            job.record(id.0, r.map(|sp| sp.utterances.len()));
        }
        job.finish(None);
    };
    let handle = std::thread::Builder::new().name("video-speech".into()).spawn(work).map_err(|e| {
        job.finish(Some(e.to_string()));
        other(format!("could not start the transcription: {e}"))
    })?;
    if background {
        return Ok(json!({"background": true, "videos": job.total.load(Ordering::SeqCst)}));
    }
    let _ = handle.join();
    let r = job.report();
    Ok(json!({"videos": r["total"], "transcribed": r["done"], "kept": r["sentences"], "failed": r["failed"]}))
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
