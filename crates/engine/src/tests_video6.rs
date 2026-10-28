//! VID-6: pictures cut at the changes of shot, failed pictures tried again, the lists of pictures
//! read off the window's thread, what is said in a video (needs `mlx_whisper` and macOS `say`; the
//! test skips without them) and the honest scores of the search by meaning (model-free parts).

use std::path::Path;
use std::process::Command;

use serde_json::json;

use crate::tests_video::{have_ffmpeg, ids, import, library, temp_dir};

/// Two shots: 3 s of black, then 12 s of FFmpeg's test pattern.
fn two_shots(path: &Path) {
    let ff = crate::video::ffmpeg::ffmpeg().unwrap();
    let st = Command::new(ff)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=320x240:d=3:r=10", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=12:size=320x240:rate=10")
        .args(["-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p[v]", "-map", "[v]", "-c:v", "mpeg4"])
        .arg(path)
        .status()
        .unwrap();
    assert!(st.success());
}

#[test]
fn pictures_are_cut_at_the_shots_and_a_long_shot_gets_more() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let v = dir.join("two-shots.mp4");
    two_shots(&v);
    let cuts = crate::video::scenes::detect(&v, 0.3).unwrap();
    assert!(cuts.iter().any(|c| (2_800..3_300).contains(c)), "the cut at 3 s is found: {cuts:?}");
    let id = *ids(&import(&mut s, &v)).first().unwrap();
    let r = s.execute("video.frames", &json!({"plan": "scenes", "max": 20})).unwrap();
    assert_eq!(r["failed"].as_u64(), Some(0), "{r}");
    let frames = s.frames_of_video(id);
    let ms: Vec<u64> = frames.iter().map(|(_, f)| f.ms).collect();
    assert!(ms.contains(&1_500), "the middle of the black shot: {ms:?}");
    let long: Vec<u64> = ms.iter().copied().filter(|t| *t > 3_300).collect();
    assert!(long.len() >= 2, "a 12 s shot has more than one picture: {ms:?}");
    assert!(s.execute("video.frames", &json!({"plan": "nope"})).is_err());
}

#[test]
fn a_failed_picture_is_tried_again_beside_itself() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let v = dir.join("clip.mp4");
    crate::tests_video::make_video(&v, 6, true);
    let id = *ids(&import(&mut s, &v)).first().unwrap();
    s.execute("video.frames", &json!({})).unwrap();
    let ck = crate::media::content_key(s.catalog.photo(id).unwrap());
    let vdir = crate::video::video_dir(&s.library.as_ref().unwrap().dir.join("video"), &ck);
    // pretend the picture at 3 s failed last time
    let mut m = crate::video::frames::Manifest::load(&vdir).unwrap();
    let gone = m.frames.iter().position(|f| f.ms == 3000).unwrap();
    let f = m.frames.remove(gone);
    std::fs::remove_file(vdir.join(&f.file)).unwrap();
    m.failed.push((3000, "no picture at 3000 ms".into()));
    m.save(&vdir).unwrap();
    let r = s.execute("video.frames", &json!({"retryFailed": true})).unwrap();
    assert_eq!(r["failed"].as_u64(), Some(0), "{r}");
    let after = crate::video::frames::Manifest::load(&vdir).unwrap();
    assert!(after.has(3000) && after.failed.is_empty(), "{:?} {:?}", after.frames.iter().map(|f| f.ms).collect::<Vec<_>>(), after.failed);
    // nothing failed any more: the second retry cuts nothing
    let r = s.execute("video.frames", &json!({"retryFailed": true})).unwrap();
    assert_eq!(r["cut"].as_u64(), Some(0), "{r}");
}

#[test]
fn the_lists_of_pictures_are_read_off_the_windows_thread() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let v = dir.join("clip.mp4");
    crate::tests_video::make_video(&v, 6, true);
    import(&mut s, &v);
    s.execute("video.frames", &json!({})).unwrap();
    // the remembered lists are stale after the run: the first answer is "not yet", then it is ready
    let first = s.video_frames_ready();
    let t0 = std::time::Instant::now();
    while !s.video_frames_ready() {
        assert!(t0.elapsed().as_secs() < 20, "never ready");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!first, "a first call that is ready would not have needed the thread");
    assert_eq!(s.video_frames().len(), 3);
    assert!(s.video_frames_ready(), "ready stays ready until something changes");
}

#[test]
fn the_search_floor_and_centre_are_checked() {
    let (mut s, _dir) = library();
    assert!(s.execute("describe.search", &json!({"query": "voiture", "centre": "sideways"})).is_err());
    assert!(s.execute("describe.search", &json!({"query": "voiture", "floor": "high"})).is_err());
    // without a model the other parameters change nothing and the search still answers
    let r = s.execute("describe.search", &json!({"query": "voiture", "centre": "text", "floor": 1.5})).unwrap();
    assert_eq!(r["semantic"]["used"], json!(false));
}

#[test]
fn what_is_said_in_a_video_is_found() {
    if !have_ffmpeg() || crate::video::speech::whisper().is_none() || !Command::new("say").arg("-v").arg("?").output().is_ok_and(|o| o.status.success()) {
        eprintln!("skipped: needs FFmpeg, mlx_whisper and `say`");
        return;
    }
    let (mut s, dir) = library();
    let aiff = dir.join("said.aiff");
    let st = Command::new("say").arg("-o").arg(&aiff).arg("The man drives a red van to the beach. Then he goes home.").status().unwrap();
    assert!(st.success());
    let v = dir.join("said.mp4");
    let ff = crate::video::ffmpeg::ffmpeg().unwrap();
    let st = Command::new(ff)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=8:size=320x240:rate=10", "-i"])
        .arg(&aiff)
        .args(["-shortest", "-pix_fmt", "yuv420p", "-c:v", "mpeg4", "-c:a", "aac"])
        .arg(&v)
        .status()
        .unwrap();
    assert!(st.success());
    let id = *ids(&import(&mut s, &v)).first().unwrap();
    s.execute("video.frames", &json!({})).unwrap();
    let r = s.execute("video.transcribe", &json!({"language": "en"})).unwrap();
    assert!(r["failed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["kept"].as_u64().unwrap() >= 1, "{r}");
    // the sentences are kept next to the pictures, and found by their words
    let r = s.execute("describe.search", &json!({"query": "red van beach", "kind": "video", "semantic": false})).unwrap();
    let rows = r["results"].as_array().unwrap();
    assert!(!rows.is_empty() && rows[0]["id"].as_u64() == Some(id.0), "{r}");
    assert!(rows[0]["speech"]["level"].as_u64().unwrap() <= 2, "{r}");
    // a second run keeps what is there; the text of a word that is not said finds nothing
    let again = s.execute("video.transcribe", &json!({"language": "en"})).unwrap();
    assert_eq!(again["transcribed"].as_u64(), Some(0), "{again}");
    let none = s.execute("describe.search", &json!({"query": "submarine", "kind": "video", "semantic": false})).unwrap();
    assert!(none["results"].as_array().unwrap().is_empty(), "{none}");
    let _ = temp_dir("unused");
}
