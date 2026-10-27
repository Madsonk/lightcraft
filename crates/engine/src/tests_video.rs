//! Videos: import, pictures cut out of a video (`video.frames`: the cache, the key, resuming,
//! hostile files), faces of videos in People (with faces injected at the pictures' keys: no model
//! needed), annotations and the Video Catcher import. Tests that need FFmpeg (or `sqlite3`) skip
//! when it is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::{Value, json};

use crate::Session;

pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("lc-video-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn have_ffmpeg() -> bool {
    let ok = crate::video::ffmpeg::ffmpeg().is_some() && crate::video::ffmpeg::ffprobe().is_some();
    if !ok {
        eprintln!("skipped: FFmpeg is not installed");
    }
    ok
}

/// A synthetic video (FFmpeg's test pattern), `secs` long, with or without a video track.
pub(crate) fn make_video(path: &Path, secs: u32, video: bool) {
    let ff = crate::video::ffmpeg::ffmpeg().unwrap();
    let mut c = Command::new(ff);
    c.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"]);
    if video {
        c.arg(format!("testsrc=duration={secs}:size=320x240:rate=10")).args(["-pix_fmt", "yuv420p", "-c:v", "mpeg4"]);
    } else {
        c.arg(format!("sine=frequency=440:duration={secs}")).args(["-c:a", "aac"]);
    }
    let st = c.arg(path).status().unwrap();
    assert!(st.success(), "ffmpeg could not make {}", path.display());
}

pub(crate) fn library() -> (Session, PathBuf) {
    let dir = temp_dir("lib");
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    (s, dir)
}

pub(crate) fn import(s: &mut Session, path: &Path) -> Value {
    s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap()
}

pub(crate) fn ids(r: &Value) -> Vec<PhotoId> {
    r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).map(PhotoId).collect()
}

#[test]
fn hostile_video_files_are_refused_without_a_panic() {
    let (mut s, dir) = library();
    let empty = dir.join("empty.mp4");
    std::fs::write(&empty, b"").unwrap();
    let text = dir.join("text.mov");
    std::fs::write(&text, b"this is not a video at all").unwrap();
    let mut paths = vec![empty, text];
    if have_ffmpeg() {
        let audio = dir.join("audio.mp4");
        make_video(&audio, 1, false);
        let good = dir.join("good.mp4");
        make_video(&good, 2, true);
        let bytes = std::fs::read(&good).unwrap();
        let cut = dir.join("cut.mp4");
        std::fs::write(&cut, &bytes[..bytes.len() / 3]).unwrap();
        paths.extend([audio, cut]);
    }
    for p in &paths {
        let r = import(&mut s, p);
        assert!(ids(&r).is_empty() || p.ends_with("cut.mp4"), "{} imported: {r}", p.display());
    }
    // a truncated file that still probes: its pictures fail one by one, nothing crashes
    let r = s.execute("video.frames", &json!({}));
    if have_ffmpeg() {
        let r = r.unwrap();
        assert!(r["videos"].as_u64().unwrap() <= 1, "{r}");
    }
}

#[test]
fn pictures_of_a_video_are_cut_cached_keyed_and_resumed() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let v = dir.join("clip.mp4");
    make_video(&v, 6, true);
    let id = *ids(&import(&mut s, &v)).first().expect("the video is imported");
    let ph = s.catalog.photo(id).unwrap().clone();
    assert_eq!((ph.kind, ph.width, ph.height), (MediaKind::Video, 320, 240));
    assert!((ph.duration.unwrap() - 6.0).abs() < 0.2, "{:?}", ph.duration);
    assert!(ph.content_hash.as_deref().unwrap().starts_with("vs1-"));
    // the grid shows a poster
    let thumb = s.thumb_job(id, 128).unwrap().run();
    assert!(thumb.loaded.is_some(), "no poster");

    let r = s.execute("video.frames", &json!({})).unwrap();
    assert_eq!((r["cut"].as_u64(), r["kept"].as_u64(), r["failed"].as_u64()), (Some(3), Some(0), Some(0)), "{r}");
    let ck = crate::media::content_key(&ph);
    let root = s.library.as_ref().unwrap().dir.join("video");
    let vdir = crate::video::video_dir(&root, &ck);
    let m = crate::video::frames::Manifest::load(&vdir).unwrap();
    assert_eq!(m.frames.iter().map(|f| f.ms).collect::<Vec<_>>(), vec![1000, 3000, 5000]);
    assert!(m.complete && m.content_key == ck);
    for f in &m.frames {
        let b = std::fs::read(vdir.join(&f.file)).unwrap();
        assert_eq!(&b[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
    }
    // nothing next to the source
    assert_eq!(std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path().is_file()).count(), 1);
    // keyed by content + time
    let frames = s.video_frames();
    assert_eq!(frames.len(), 3);
    let at = frames.get(&crate::video::frame_key(&ck, 3000)).unwrap();
    assert_eq!((at.ids.clone(), at.ms), (vec![id], 3000));
    // again: nothing to cut
    let r = s.execute("video.frames", &json!({})).unwrap();
    assert_eq!((r["cut"].as_u64(), r["kept"].as_u64()), (Some(0), Some(3)), "{r}");
    // an interrupted run (the last picture not listed): only it is cut
    let mut m = crate::video::frames::Manifest::load(&vdir).unwrap();
    let last = m.frames.pop().unwrap();
    std::fs::remove_file(vdir.join(&last.file)).unwrap();
    m.complete = false;
    m.save(&vdir).unwrap();
    let r = s.execute("video.frames", &json!({})).unwrap();
    assert_eq!((r["cut"].as_u64(), r["kept"].as_u64()), (Some(1), Some(2)), "{r}");
    // every 1 s, at most 4, and a budget of 0 bytes stops at once
    let r = s.execute("video.frames", &json!({"every": 1.0, "max": 4})).unwrap();
    assert_eq!((r["cut"].as_u64(), r["kept"].as_u64()), (Some(4), Some(0)), "{r}");
    let r = s.execute("video.frames", &json!({"every": 0.5, "maxBytes": 0})).unwrap();
    assert_eq!(r["overBudget"], true, "{r}");
    assert_eq!(r["cut"], 0);
    // a damaged list: the pictures are cut again
    std::fs::write(vdir.join(crate::video::frames::MANIFEST), b"{oops").unwrap();
    s.video.rev += 1;
    assert!(s.video_frames().is_empty());
    let r = s.execute("video.frames", &json!({})).unwrap();
    assert_eq!(r["cut"], 3, "{r}");
    let st = s.execute("video.status", &json!({})).unwrap();
    assert_eq!((st["videos"].as_u64(), st["withFrames"].as_u64(), st["frames"].as_u64()), (Some(1), Some(1), Some(3)), "{st}");
    // what a window polls: the job only, no list of pictures read
    std::fs::write(vdir.join(crate::video::frames::MANIFEST), b"{oops").unwrap();
    let st = s.execute("video.status", &json!({"jobOnly": true})).unwrap();
    assert_eq!(st["job"]["running"], false, "{st}");
    assert!(st.get("frames").is_none() && st.get("bytes").is_none(), "{st}");
}

/// `force` cuts LightCraft's own pictures again and never removes nor replaces those brought in
/// from Video Catcher; a forced run stopped midway leaves a whole list whose every picture is there.
#[test]
fn force_keeps_video_catchers_pictures_and_a_stopped_run_leaves_a_whole_list() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let v = dir.join("clip.mp4");
    make_video(&v, 6, true);
    let id = ids(&import(&mut s, &v))[0];
    let ck = crate::media::content_key(s.catalog.photo(id).unwrap());
    let vdir = crate::video::video_dir(&s.library.as_ref().unwrap().dir.join("video"), &ck);
    std::fs::create_dir_all(&vdir).unwrap();
    // Video Catcher's picture at 1 s (its own bytes), as its import lists it
    let vc = vdir.join(crate::video::frames::file_name(1000));
    assert!(crate::video::ffmpeg::extract_frame(&v, 4000, 160, &vc).is_ok());
    let vc_bytes = std::fs::read(&vc).unwrap();
    let mut m = crate::video::frames::Manifest::new(&ck, &v.to_string_lossy(), Some(6000), "videocatcher");
    m.frames.push(crate::video::frames::Frame {
        ms: 1000,
        file: crate::video::frames::file_name(1000),
        bytes: vc_bytes.len() as u64,
        origin: "videocatcher".into(),
    });
    m.save(&vdir).unwrap();
    s.video.rev += 1;
    let r = s.execute("video.frames", &json!({"force": true})).unwrap();
    assert_eq!((r["cut"].as_u64(), r["kept"].as_u64()), (Some(2), Some(1)), "{r}");
    assert_eq!(std::fs::read(&vc).unwrap(), vc_bytes, "Video Catcher's picture is untouched");
    let m = crate::video::frames::Manifest::load(&vdir).unwrap();
    assert_eq!(
        m.frames.iter().map(|f| (f.ms, f.origin.as_str())).collect::<Vec<_>>(),
        vec![(1000, "videocatcher"), (3000, "ffmpeg"), (5000, "ffmpeg")]
    );
    // again with force: LightCraft's own two are cut again, Video Catcher's is still kept
    let r = s.execute("video.frames", &json!({"force": true})).unwrap();
    assert_eq!((r["cut"].as_u64(), r["kept"].as_u64()), (Some(2), Some(1)), "{r}");
    assert_eq!(std::fs::read(&vc).unwrap(), vc_bytes);
    // a forced run stopped at once (in the background), then again stopped later: the list is whole
    for _ in 0..3 {
        s.execute("video.frames", &json!({"force": true, "every": 0.5, "background": true})).unwrap();
        s.execute("video.cancel", &json!({})).unwrap();
        while s.execute("video.status", &json!({})).unwrap()["job"]["running"] == true {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let m = crate::video::frames::Manifest::load(&vdir).expect("the list is readable after a stop");
        for f in &m.frames {
            assert!(vdir.join(&f.file).is_file(), "{} listed but missing", f.file);
        }
        assert!(m.frames.iter().any(|f| f.ms == 1000 && f.origin == "videocatcher"));
        assert_eq!(std::fs::read(&vc).unwrap(), vc_bytes);
        assert!(std::fs::read_dir(&vdir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part")), "no half-written file left");
    }
}
