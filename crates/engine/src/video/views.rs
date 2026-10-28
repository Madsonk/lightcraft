//! What the Videos view lists, built here so a frontend only draws: the library's videos with who
//! is in them and how far their analysis went (`video.list`), the moments of one video or the one
//! "moment" of a photo with their pictures, people and descriptions (`video.moments`), and opening
//! a moment in the system's video player (`video.open`).

use std::collections::BTreeMap;

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::{Value, json};

use crate::cmd::{bad, bool_or, str_param};
use crate::describe::{Status, fold};
use crate::{Result, Session};

/// The named people among faces (`[{name, …}]`), each once, in order of appearance.
fn names(faces: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in faces.iter().filter_map(|f| f["name"].as_str()) {
        if !out.iter().any(|x| x == n) {
            out.push(n.to_string());
        }
    }
    out
}

/// `video.list {person?, described?: bool, status?: confirmed | toConfirm, text?}`.
pub fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let person = str_param(p, "person").map(fold).filter(|x| !x.is_empty());
    let described = p.get("described").and_then(Value::as_bool);
    let status = str_param(p, "status");
    let text = str_param(p, "text").map(fold).filter(|x| !x.is_empty());
    let store = crate::describe::ops::open(s);
    let videos: Vec<(PhotoId, String, String, Option<f64>, String)> = s
        .catalog
        .photos()
        .filter(|q| q.in_library() && q.kind == MediaKind::Video)
        .map(|q| (q.id, crate::media::content_key(q), q.file_name.clone(), q.duration, q.captured.clone().unwrap_or_default()))
        .collect();
    let mut rows = Vec::new();
    let reader = super::faces::reader(s);
    let mut all_people: BTreeMap<String, usize> = BTreeMap::new();
    // the pictures of every video from the remembered lists (read once, until they change), not one
    // `frames.json` per video per call: that froze the window on a cold disk
    let mut keys_of: std::collections::HashMap<PhotoId, Vec<(u64, u64, std::path::PathBuf)>> = std::collections::HashMap::new();
    for (k, f) in s.video_frames().iter() {
        for id in &f.ids {
            keys_of.entry(*id).or_default().push((f.ms, *k, f.file.clone()));
        }
    }
    for (id, ck, name, duration, captured) in videos {
        let mut frames = keys_of.remove(&id).unwrap_or_default();
        frames.sort_unstable();
        let keys: Vec<u64> = frames.iter().map(|(_, k, _)| *k).collect();
        let looked = reader.analysed(&keys);
        let mut people: Vec<String> = Vec::new();
        for k in &keys {
            for n in names(&reader.faces_at(*k)) {
                if !people.contains(&n) {
                    people.push(n);
                }
            }
        }
        let moments: Vec<_> = store.moments().into_iter().filter(|(i, _)| *i == ck).filter_map(|(i, ms)| store.effective(&i, ms)).collect();
        let confirmed = moments.iter().filter(|e| e.status == Status::Confirmed).count();
        for n in &people {
            *all_people.entry(n.clone()).or_default() += 1;
        }
        if person.as_ref().is_some_and(|w| !people.iter().any(|n| fold(n) == *w)) {
            continue;
        }
        if described.is_some_and(|d| d != !moments.is_empty()) {
            continue;
        }
        match status {
            Some("confirmed") if confirmed == 0 => continue,
            Some("toConfirm") if moments.len() == confirmed => continue,
            _ => {}
        }
        if let Some(t) = &text {
            let hay = fold(&format!(
                "{name} {} {}",
                people.join(" "),
                moments.iter().map(|e| format!("{} {} {}", e.scene, e.action, e.description)).collect::<Vec<_>>().join(" ")
            ));
            if !t.split(' ').all(|w| hay.contains(w)) {
                continue;
            }
        }
        rows.push(json!({
            "id": id.0, "name": name, "duration": duration, "captured": captured,
            "frames": frames.len(), "facesLooked": looked, "people": people,
            "moments": moments.len(), "confirmed": confirmed, "toConfirm": moments.len() - confirmed,
            "poster": frames.get(frames.len() / 2).map(|(_, _, f)| f.display().to_string()),
        }));
    }
    Ok(json!({"videos": rows, "people": all_people.keys().collect::<Vec<_>>(), "total": rows.len()}))
}

/// `video.person {name, limit?}`: the pictures of videos where `name` is suggested (by the photos'
/// named faces) → `{name, total, videos, faces: [{video, file, ms, frame, rect}]}`, by video then time.
pub fn person(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.person";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?;
    let want = fold(name);
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(200).clamp(1, 5000) as usize;
    let reader = super::faces::reader(s);
    let analysed: std::collections::HashSet<u64> = reader.keys().into_iter().collect();
    let frames = s.video_frames();
    let mut found: Vec<(u64, u64, String, Value)> = Vec::new();
    for (k, f) in frames.iter().filter(|(k, _)| analysed.contains(*k)) {
        let Some(id) = f.ids.first() else { continue };
        for face in reader.faces_at(*k) {
            if face["name"].as_str().is_some_and(|n| fold(n) == want) {
                found.push((id.0, f.ms, f.file.display().to_string(), face["rect"].clone()));
            }
        }
    }
    found.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let videos = found.iter().map(|x| x.0).collect::<std::collections::BTreeSet<_>>().len();
    let total = found.len();
    let faces: Vec<Value> = found
        .into_iter()
        .take(limit)
        .map(|(id, ms, frame, rect)| {
            let file = s.catalog.photo(PhotoId(id)).map(|q| q.file_name.clone());
            json!({"video": id, "file": file, "ms": ms, "frame": frame, "rect": rect})
        })
        .collect();
    Ok(json!({"name": name, "total": total, "videos": videos, "faces": faces}))
}

/// `video.moments {id}`: for a video, each of its pictures and described moments in time order;
/// for a photo, its one "moment". Each: `{ms, frame, faces: [{face, rect, person, name}], people,
/// annotation: {scene, action, description, status, from, proposals} | null}`.
pub fn moments(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.moments";
    let (id, item, _) = crate::describe::ops::target(s, &json!({"id": p.get("id")}), C)?;
    let video = s.catalog.photo(id).is_some_and(|q| q.kind == MediaKind::Video);
    let store = crate::describe::ops::open(s);
    let mut by_ms: BTreeMap<Option<u64>, (u64, Option<String>)> = BTreeMap::new();
    if video {
        for (k, f) in s.frames_of_video(id) {
            by_ms.insert(Some(f.ms), (k, Some(f.file.display().to_string())));
        }
        for (_, ms) in store.moments().into_iter().filter(|(i, _)| *i == item) {
            by_ms.entry(ms).or_insert_with(|| (ms.map_or_else(|| crate::video::item_key(&item), |m| crate::video::frame_key(&item, m)), None));
        }
    } else {
        by_ms.insert(None, (crate::video::item_key(&item), None));
    }
    let mut rows = Vec::new();
    let reader = super::faces::reader(s);
    for (ms, (key, frame)) in by_ms {
        let faces = reader.faces_at(key);
        rows.push(json!({
            "ms": ms, "frame": frame, "people": names(&faces), "faces": faces,
            "annotation": store.effective(&item, ms),
        }));
    }
    let ph = s.catalog.photo(id);
    Ok(json!({
        "id": id.0, "video": video, "name": ph.map(|q| q.file_name.clone()), "duration": ph.and_then(|q| q.duration),
        "moments": rows,
    }))
}

/// Seconds as `m:ss` (or `h:mm:ss`).
pub fn timecode(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// The program and arguments that open `path` at `ms` (VLC when it is installed, else the
/// system's default player, which starts at the beginning).
pub fn player_command(path: &str, ms: u64) -> (String, Vec<String>, bool) {
    let secs = format!("{}", ms / 1000);
    #[cfg(target_os = "macos")]
    {
        let vlc =
            ["/Applications/VLC.app", "/Applications/Artcraft/VLC.app"].iter().find(|p| std::path::Path::new(p).is_dir()).map(|p| p.to_string());
        if let Some(vlc) = vlc {
            return ("/usr/bin/open".into(), vec!["-a".into(), vlc, "--args".into(), format!("--start-time={secs}"), path.into()], true);
        }
        ("/usr/bin/open".into(), vec![path.into()], false)
    }
    #[cfg(target_os = "windows")]
    {
        let vlc = r"C:\Program Files\VideoLAN\VLC\vlc.exe";
        if std::path::Path::new(vlc).is_file() {
            return (vlc.into(), vec![format!("--start-time={secs}"), path.into()], true);
        }
        ("explorer".into(), vec![path.into()], false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(vlc) = ["/usr/bin/vlc", "/snap/bin/vlc"].iter().find(|p| std::path::Path::new(p).is_file()) {
            return (vlc.to_string(), vec![format!("--start-time={secs}"), path.into()], true);
        }
        ("xdg-open".into(), vec![path.into()], false)
    }
}

/// `video.open {id, ms?, dryRun?}`: the video in the system's player, at `ms` when the player can.
pub fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.open";
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(C, "no video: pass `id`"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(C, "no such video"))?;
    let path = super::source_path(ph).ok_or_else(|| bad(C, "this is not a video of the library"))?.to_string();
    let ms = p.get("ms").and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map_or(0, |x| x.min(864_000_000.0) as u64);
    if !std::path::Path::new(&path).exists() {
        return Err(bad(C, format!("the video is not there (offline drive?): {path}")));
    }
    let (program, args, at_time) = player_command(&path, ms);
    if !bool_or(p, "dryRun", false) {
        #[cfg(not(target_arch = "wasm32"))]
        std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| bad(C, format!("could not open the player: {e}")))?;
    }
    Ok(json!({"program": program, "args": args, "atTime": at_time, "timecode": timecode(ms)}))
}

/// Seconds a clip lasts by default, and at most.
pub const CLIP_SECONDS: f64 = 8.0;
const CLIP_MAX_SECONDS: f64 = 30.0;

/// A clip to make (or reuse): the video, where it goes, from when and how long. Built on the UI
/// thread (no file is touched), made anywhere ([`ClipJob::run`]).
#[derive(Clone, Debug)]
pub struct ClipJob {
    pub source: std::path::PathBuf,
    pub out: std::path::PathBuf,
    pub start_ms: u64,
    pub len_ms: u64,
    pub ms: u64,
}

/// The clip `video.clip {id, ms, seconds?}` asks for.
pub fn clip_job(s: &Session, p: &Value) -> Result<ClipJob> {
    const C: &str = "video.clip";
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(C, "no video: pass `id`"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(C, "no such video"))?;
    let path = super::source_path(ph).ok_or_else(|| bad(C, "this is not a video of the library"))?.to_string();
    let ck = crate::media::content_key(ph);
    #[cfg(not(target_arch = "wasm32"))]
    let duration_ms = super::ffmpeg::sane_duration(ph.duration).map(|d| (d * 1000.0) as u64);
    #[cfg(target_arch = "wasm32")]
    let duration_ms: Option<u64> = None;
    let root = super::cache_root(s).ok_or_else(|| bad(C, "the library is not on disk"))?;
    let ms = p.get("ms").and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map_or(0, |x| x.min(864_000_000.0) as u64);
    let len = (crate::cmd::f64_or(p, "seconds", CLIP_SECONDS).clamp(1.0, CLIP_MAX_SECONDS) * 1000.0) as u64;
    let mut start = ms.saturating_sub(len / 2);
    if let Some(d) = duration_ms {
        start = start.min(d.saturating_sub(len));
    }
    let out = super::video_dir(&root, &ck).join("clips").join(format!("c{start:010}-{len}.mp4"));
    Ok(ClipJob { source: path.into(), out, start_ms: start, len_ms: len, ms })
}

impl ClipJob {
    /// Already made.
    pub fn cached(&self) -> bool {
        std::fs::metadata(&self.out).is_ok_and(|m| m.is_file() && m.len() > 0)
    }

    /// Make the clip unless it is there.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run(&self) -> std::result::Result<(), String> {
        if self.cached() {
            return Ok(());
        }
        if !self.source.exists() {
            return Err(format!("the video is not there (offline drive?): {}", self.source.display()));
        }
        if let Some(dir) = self.out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        super::ffmpeg::extract_clip(&self.source, self.start_ms, self.len_ms, &self.out).map(|_| ())
    }
}

/// `video.clip {id, ms, seconds?}`: a short clip around the moment (half before, half after), 540
/// lines, H.264 + AAC, kept in `<library>/video/<key>/clips/` (made once, then reused) — for a
/// player to show the moment without reading the whole video.
#[cfg(not(target_arch = "wasm32"))]
pub fn clip(s: &mut Session, p: &Value) -> Result<Value> {
    let job = clip_job(s, p)?;
    let cached = job.cached();
    job.run().map_err(|e| bad("video.clip", e))?;
    Ok(json!({
        "path": job.out.display().to_string(), "startMs": job.start_ms, "lengthMs": job.len_ms, "ms": job.ms, "cached": cached,
        "bytes": std::fs::metadata(&job.out).map(|m| m.len()).unwrap_or(0), "timecode": timecode(job.ms),
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn timecodes() {
        assert_eq!(super::timecode(0), "0:00");
        assert_eq!(super::timecode(83_503), "1:23");
        assert_eq!(super::timecode(3_723_000), "1:02:03");
        assert!(!super::timecode(u64::MAX).is_empty());
    }
}
