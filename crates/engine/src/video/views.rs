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

#[cfg(feature = "people")]
fn faces_at(s: &mut Session, key: u64) -> Vec<Value> {
    crate::people::ops::people_at(s, key)
}
#[cfg(not(feature = "people"))]
fn faces_at(_: &mut Session, _: u64) -> Vec<Value> {
    Vec::new()
}

#[cfg(feature = "people")]
fn analysed(s: &mut Session, keys: &[u64]) -> usize {
    crate::people::ops::analysed_count(s, keys)
}
#[cfg(not(feature = "people"))]
fn analysed(_: &mut Session, _: &[u64]) -> usize {
    0
}

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
    let mut all_people: BTreeMap<String, usize> = BTreeMap::new();
    for (id, ck, name, duration, captured) in videos {
        let frames = s.frames_of_video(id);
        let keys: Vec<u64> = frames.iter().map(|(k, _)| *k).collect();
        let looked = analysed(s, &keys);
        let mut people: Vec<String> = Vec::new();
        for k in &keys {
            for n in names(&faces_at(s, *k)) {
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
            "poster": frames.get(frames.len() / 2).map(|(_, f)| f.file.display().to_string()),
        }));
    }
    Ok(json!({"videos": rows, "people": all_people.keys().collect::<Vec<_>>(), "total": rows.len()}))
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
    for (ms, (key, frame)) in by_ms {
        let faces = faces_at(s, key);
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

/// `video.clip {id, ms, seconds?}`: a short clip around the moment (half before, half after), 540
/// lines, H.264 + AAC, kept in `<library>/video/<key>/clips/` (made once, then reused) — for a
/// player to show the moment without reading the whole video.
#[cfg(not(target_arch = "wasm32"))]
pub fn clip(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.clip";
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(C, "no video: pass `id`"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(C, "no such video"))?;
    let path = super::source_path(ph).ok_or_else(|| bad(C, "this is not a video of the library"))?.to_string();
    let ck = crate::media::content_key(ph);
    let duration_ms = super::ffmpeg::sane_duration(ph.duration).map(|d| (d * 1000.0) as u64);
    let root = super::cache_root(s).ok_or_else(|| bad(C, "the library is not on disk"))?;
    let ms = p.get("ms").and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map_or(0, |x| x.min(864_000_000.0) as u64);
    let len = (crate::cmd::f64_or(p, "seconds", CLIP_SECONDS).clamp(1.0, CLIP_MAX_SECONDS) * 1000.0) as u64;
    let mut start = ms.saturating_sub(len / 2);
    if let Some(d) = duration_ms {
        start = start.min(d.saturating_sub(len));
    }
    let dir = super::video_dir(&root, &ck).join("clips");
    let out = dir.join(format!("c{start:010}-{len}.mp4"));
    let cached = std::fs::metadata(&out).is_ok_and(|m| m.is_file() && m.len() > 0);
    if !cached {
        if !std::path::Path::new(&path).exists() {
            return Err(bad(C, format!("the video is not there (offline drive?): {path}")));
        }
        std::fs::create_dir_all(&dir).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
        super::ffmpeg::extract_clip(std::path::Path::new(&path), start, len, &out).map_err(|e| bad(C, e))?;
    }
    Ok(json!({
        "path": out.display().to_string(), "startMs": start, "lengthMs": len, "ms": ms, "cached": cached,
        "bytes": std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0), "timecode": timecode(ms),
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
