//! Videos: import, pictures cut out of a video (`video.frames`: the cache, the key, resuming,
//! hostile files), faces of videos in People (with faces injected at the pictures' keys: no model
//! needed), annotations and the Video Catcher import. Tests that need FFmpeg (or `sqlite3`) skip
//! when it is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("lc-video-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn have_ffmpeg() -> bool {
    let ok = crate::video::ffmpeg::ffmpeg().is_some() && crate::video::ffmpeg::ffprobe().is_some();
    if !ok {
        eprintln!("skipped: FFmpeg is not installed");
    }
    ok
}

/// A synthetic video (FFmpeg's test pattern), `secs` long, with or without a video track.
fn make_video(path: &Path, secs: u32, video: bool) {
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

fn library() -> (Session, PathBuf) {
    let dir = temp_dir("lib");
    let mut s = Session::new().with_fs();
    s.people.dir = Some(temp_dir("models"));
    s.open_library(dir.join("lib"), false).unwrap();
    (s, dir)
}

fn import(s: &mut Session, path: &Path) -> Value {
    s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap()
}

fn ids(r: &Value) -> Vec<PhotoId> {
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
}

#[cfg(feature = "people")]
mod people {
    use super::*;
    use crate::people::ops::{inject, inject_keys};

    const DIM: usize = lightcraft_people::DIM;

    fn unit(v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.into_iter().map(|x| x / n).collect()
    }

    fn rnd(seed: u64) -> Vec<f32> {
        let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        unit(
            (0..DIM)
                .map(|_| {
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                })
                .collect(),
        )
    }

    fn face(who: u64, n: u64) -> Vec<f32> {
        let (a, b) = (rnd(100 + who), rnd(9000 + who * 31 + n));
        unit(a.iter().zip(&b).map(|(x, y)| x + 0.5 * y).collect())
    }

    fn write_png(path: &Path, seed: u8) {
        let data: Vec<[u8; 4]> = (0..64 * 48).map(|i| [(i % 64) as u8 * 3, (i / 64) as u8 * 4, seed, 255]).collect();
        let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A person named on photos is found in a video; naming, the keyword, the lists and moving a
    /// face work on the faces of the video like on any other.
    #[test]
    fn faces_of_a_video_join_the_people_of_the_photos() {
        if !have_ffmpeg() {
            return;
        }
        let (mut s, dir) = library();
        for i in 0..2u8 {
            write_png(&dir.join(format!("p{i}.png")), i * 40);
        }
        let photos = ids(&s
            .execute("library.import", &json!({"paths": [dir.join("p0.png").to_string_lossy(), dir.join("p1.png").to_string_lossy()]}))
            .unwrap());
        let v = dir.join("clip.mp4");
        make_video(&v, 6, true);
        let vid = ids(&import(&mut s, &v))[0];
        s.execute("video.frames", &json!({})).unwrap();
        let ck = crate::media::content_key(s.catalog.photo(vid).unwrap());
        // Awa on both photos and in the video at 1 s and 5 s; someone else at 3 s
        inject(&mut s, photos.iter().enumerate().map(|(i, id)| (*id, vec![(face(1, i as u64), [0.1, 0.1, 0.5, 0.5])])).collect());
        let k = |ms| crate::video::frame_key(&ck, ms);
        inject_keys(
            &mut s,
            vec![
                (k(1000), vec![(face(1, 10), [0.2, 0.2, 0.4, 0.5])]),
                (k(3000), vec![(face(2, 0), [0.5, 0.1, 0.7, 0.4])]),
                (k(5000), vec![(face(1, 11), [0.3, 0.2, 0.5, 0.5])]),
            ],
        )
        .unwrap();
        let l = s.execute("people.list", &json!({"thumbs": 10})).unwrap();
        let g = l["groups"].as_array().unwrap().iter().find(|g| g["faces"] == 4).unwrap_or_else(|| panic!("{l}"));
        assert_eq!(g["photos"], 3, "two photos and one video: {g}");
        let vt: Vec<&Value> = g["thumbs"].as_array().unwrap().iter().filter(|t| t.get("video").is_some()).collect();
        assert_eq!(vt.len(), 2, "{g}");
        assert!(vt.iter().all(|t| t["photo"] == vid.0 && t["video"]["video"] == vid.0));
        let mut ms: Vec<u64> = vt.iter().map(|t| t["video"]["ms"].as_u64().unwrap()).collect();
        ms.sort();
        assert_eq!(ms, vec![1000, 5000]);
        assert!(vt.iter().all(|t| Path::new(t["video"]["frame"].as_str().unwrap()).is_file()));
        let gid = g["id"].as_str().unwrap().to_string();
        s.execute("people.name", &json!({"group": gid, "name": "Awa"})).unwrap();
        let kw = |s: &Session, id: PhotoId| s.catalog.photo(id).unwrap().meta.keywords.clone();
        assert!(kw(&s, vid).contains(&"People|Awa".to_string()), "the video is filed under the person");
        assert!(photos.iter().all(|p| kw(&s, *p).contains(&"People|Awa".to_string())));
        // members and photos say when
        let m = s.execute("people.members", &json!({"person": "p:1"})).unwrap();
        let in_video: Vec<&Value> = m["faces"].as_array().unwrap().iter().filter(|f| f.get("video").is_some()).collect();
        assert_eq!(in_video.len(), 2, "{m}");
        let ph = s.execute("people.photos", &json!({"person": "p:1"})).unwrap();
        assert_eq!((ph["count"].as_u64(), ph["videos"].as_u64()), (Some(3), Some(1)), "{ph}");
        assert_eq!(ph["moments"].as_array().unwrap().iter().map(|m| m["ms"].as_u64().unwrap()).collect::<Vec<_>>(), vec![1000, 5000]);
        // the faces of the video, moment by moment
        let f = s.execute("people.faces", &json!({"id": vid.0})).unwrap();
        assert_eq!(f["faces"].as_array().unwrap().len(), 3, "{f}");
        assert_eq!(f["faces"][0]["name"], "Awa");
        // the face at 5 s is not Awa after all: it goes; the face at 1 s keeps the video filed under her
        let face5 = in_video.iter().find(|f| f["video"]["ms"] == 5000).unwrap()["face"].as_str().unwrap().to_string();
        s.execute("people.move", &json!({"faces": [face5], "to": "unknown"})).unwrap();
        assert!(kw(&s, vid).contains(&"People|Awa".to_string()));
        let face1 = in_video.iter().find(|f| f["video"]["ms"] == 1000).unwrap()["face"].as_str().unwrap().to_string();
        s.execute("people.ignore", &json!({"face": face1})).unwrap();
        assert!(!kw(&s, vid).contains(&"People|Awa".to_string()), "no face of Awa left in the video: {:?}", kw(&s, vid));
        s.execute("people.undo", &json!({})).unwrap();
        assert!(kw(&s, vid).contains(&"People|Awa".to_string()));
        // people.index sees the pictures of the video (it stops at the missing models, after counting them)
        let e = s.execute("people.index", &json!({})).unwrap_err().to_string();
        assert!(e.contains("not installed"), "{e}");
    }

    /// Video Catcher's database: its pictures and moments come in; the moments are annotations
    /// under the same keys as the faces, so "who" and "what" meet.
    #[test]
    fn video_catcher_pictures_and_moments_are_imported() {
        if !have_ffmpeg() || !Path::new("/usr/bin/sqlite3").exists() {
            eprintln!("skipped: needs FFmpeg and sqlite3");
            return;
        }
        let (mut s, dir) = library();
        let v = dir.join("clip.mp4");
        make_video(&v, 6, true);
        let vid = ids(&import(&mut s, &v))[0];
        let ck = crate::media::content_key(s.catalog.photo(vid).unwrap());
        // Video Catcher's own pictures
        let vc = dir.join("vc");
        std::fs::create_dir_all(&vc).unwrap();
        for (i, ms) in [250u64, 2750].iter().enumerate() {
            assert!(crate::video::ffmpeg::extract_frame(&v, *ms, 320, &vc.join(format!("frame_{:03}.jpg", i + 1))).is_ok());
        }
        let db = dir.join("video-catcher.sqlite3");
        let q = |p: &Path| p.display().to_string().replace('\'', "''");
        let sql = format!(
            "CREATE TABLE videos (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, filename TEXT NOT NULL, fingerprint TEXT NOT NULL, duration_ms INTEGER NOT NULL, width INTEGER, height INTEGER);
             CREATE TABLE frames (id INTEGER PRIMARY KEY, video_id INTEGER NOT NULL, ordinal INTEGER NOT NULL, timestamp_ms INTEGER NOT NULL, path TEXT NOT NULL UNIQUE, width INTEGER, height INTEGER);
             CREATE TABLE moments (id INTEGER PRIMARY KEY, frame_id INTEGER NOT NULL, video_id INTEGER NOT NULL, timestamp_ms INTEGER NOT NULL, machine_action TEXT DEFAULT '', machine_scene TEXT DEFAULT '', machine_description TEXT DEFAULT '', action_text TEXT DEFAULT '', scene_text TEXT DEFAULT '', description TEXT DEFAULT '', status TEXT DEFAULT 'pending', updated_at TEXT);
             INSERT INTO videos VALUES (1, '{}', 'clip.mp4', 'x', 6000, 320, 240), (2, '/elsewhere/other.mp4', 'other.mp4', 'y', 1000, 1, 1);
             INSERT INTO frames VALUES (1, 1, 1, 250, '{}', 320, 240), (2, 1, 2, 2750, '{}', 320, 240), (3, 1, 3, 4000, '/missing/frame.jpg', 1, 1);
             INSERT INTO moments VALUES (10, 1, 1, 250, '', '', '', 'Une femme danse', 'Salle de danse', 'Une femme en rose danse.', 'candidate', ''), (11, 2, 1, 2750, '', '', '', '', '', '', 'pending', '');",
            q(&v),
            q(&vc.join("frame_001.jpg")),
            q(&vc.join("frame_002.jpg"))
        );
        assert!(Command::new("/usr/bin/sqlite3").arg(&db).arg(sql).status().unwrap().success());
        let dry = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy(), "dryRun": true})).unwrap();
        assert_eq!((dry["matched"].as_u64(), dry["framesCopied"].as_u64(), dry["annotationsAdded"].as_u64()), (Some(1), Some(2), Some(1)), "{dry}");
        assert!(s.video_frames().is_empty(), "a dry run writes nothing");
        let r = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy()})).unwrap();
        assert_eq!(
            (
                r["videos"].as_u64(),
                r["matched"].as_u64(),
                r["unmatched"].as_u64(),
                r["framesCopied"].as_u64(),
                r["framesMissing"].as_u64(),
                r["moments"].as_u64(),
                r["momentsEmpty"].as_u64(),
                r["annotationsAdded"].as_u64()
            ),
            (Some(2), Some(1), Some(1), Some(2), Some(1), Some(2), Some(1), Some(1)),
            "{r}"
        );
        assert_eq!(s.frames_of_video(vid).iter().map(|(_, f)| f.ms).collect::<Vec<_>>(), vec![250, 2750]);
        // again: nothing new
        let r = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy()})).unwrap();
        assert_eq!(
            (r["framesKept"].as_u64(), r["annotationsAdded"].as_u64(), r["annotationsUnchanged"].as_u64()),
            (Some(2), Some(0), Some(1)),
            "{r}"
        );
        // a face at the moment's key: the annotation says who is there
        inject_keys(&mut s, vec![(crate::video::frame_key(&ck, 250), vec![(face(3, 0), [0.1, 0.1, 0.3, 0.3])])]).unwrap();
        let d = s.execute("describe.list", &json!({"id": vid.0})).unwrap();
        assert_eq!(d["total"], 1, "{d}");
        let a = &d["annotations"][0];
        assert_eq!(
            (a["ms"].as_u64(), a["scene"].as_str(), a["status"].as_str(), a["source"].as_str()),
            (Some(250), Some("Salle de danse"), Some("candidate"), Some("videocatcher")),
            "{a}"
        );
        assert_eq!(a["key"], format!("{:016x}", crate::video::frame_key(&ck, 250)));
        assert_eq!(a["people"].as_array().unwrap().len(), 1, "{a}");
        assert!(Path::new(a["frame"].as_str().unwrap()).is_file());
        // a file that is not SQLite, and one that is missing
        let junk = dir.join("junk.sqlite3");
        std::fs::write(&junk, b"SQLite format 3\0garbage").unwrap();
        assert!(s.execute("video.importVideoCatcher", &json!({"db": junk.to_string_lossy()})).is_err());
        assert!(s.execute("video.importVideoCatcher", &json!({"db": dir.join("nope").to_string_lossy()})).is_err());
        assert!(s.execute("video.importVideoCatcher", &json!({})).is_err());
    }
}
