//! Annotations (`describe.*`): what a person writes wins over proposals, confirm / delete / undo,
//! the search "who + what + when" over photos and moments of videos (faces injected, no model),
//! proposals refused without the model, `video.frames` in the background, folder imports with
//! videos, and Video Catcher's videos brought into the catalog.

fn write_png(path: &Path, seed: u8) {
    let data: Vec<[u8; 4]> = (0..64 * 48).map(|i| [(i % 64) as u8 * 3, (i / 64) as u8 * 4, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

use std::path::Path;
use std::process::Command;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::describe::{Annotation, Store};
use crate::tests_video::{have_ffmpeg, ids, import, library, make_video, temp_dir};

fn store(s: &Session) -> Store {
    Store::open(Some(&s.library.as_ref().unwrap().dir))
}

fn ck(s: &Session, id: PhotoId) -> String {
    crate::media::content_key(s.catalog.photo(id).unwrap())
}

#[test]
fn what_a_person_writes_wins_and_confirm_delete_undo_work() {
    let (mut s, dir) = library();
    write_png(&dir.join("p.png"), 10);
    let pid = ids(&import(&mut s, &dir.join("p.png")))[0];
    let item = ck(&s, pid);
    // a program's proposal first
    let mut proposal = Annotation::new(&item, None, "une plage", "nager", "", "siglip", "t0");
    proposal.score = Some(0.3);
    store(&s).add(vec![proposal]).unwrap();
    let l = s.execute("describe.list", &json!({"id": pid.0})).unwrap();
    assert_eq!((l["annotations"][0]["scene"].as_str(), l["annotations"][0]["status"].as_str()), (Some("une plage"), Some("candidate")), "{l}");
    // the person writes the scene: it wins, the proposal stays apart
    let r = s.execute("describe.set", &json!({"id": pid.0, "scene": "Lac de Vassivière"})).unwrap();
    assert_eq!(r["annotation"]["scene"], "Lac de Vassivière", "{r}");
    assert_eq!(r["annotation"]["action"], "nager", "a field the person left empty still shows the proposal");
    assert_eq!(r["annotation"]["proposals"][0]["scene"], "une plage");
    assert_eq!(r["annotation"]["status"], "candidate", "writing is not confirming");
    let r = s.execute("describe.confirm", &json!({"id": pid.0})).unwrap();
    assert_eq!(r["annotation"]["status"], "confirmed", "{r}");
    assert_eq!(store(&s).get(&item, None, "manual").unwrap().action, "nager", "the confirmed value is the person's own copy");
    // undo the confirmation, then delete what the person wrote: the proposal shows again
    s.execute("describe.undo", &json!({})).unwrap();
    assert_eq!(s.execute("describe.list", &json!({"id": pid.0})).unwrap()["annotations"][0]["status"], "candidate");
    let r = s.execute("describe.delete", &json!({"id": pid.0})).unwrap();
    assert_eq!((r["deleted"].as_u64(), r["annotation"]["scene"].as_str()), (Some(1), Some("une plage")), "{r}");
    s.execute("describe.undo", &json!({})).unwrap();
    assert_eq!(s.execute("describe.list", &json!({"id": pid.0})).unwrap()["annotations"][0]["scene"], "Lac de Vassivière");
    // persists across sessions
    let lib = s.library.as_ref().unwrap().dir.clone();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert_eq!(s.execute("describe.list", &json!({"id": pid.0})).unwrap()["annotations"][0]["scene"], "Lac de Vassivière");
    // hostile parameters
    for bad in [
        json!({"id": pid.0, "ms": 1000, "scene": "x"}),
        json!({"id": pid.0, "scene": 3}),
        json!({"id": pid.0}),
        json!({"id": 999_999, "scene": "x"}),
        json!({"id": pid.0, "ms": -1, "scene": "x"}),
        json!({"id": pid.0, "ms": "NaN", "scene": "x"}),
    ] {
        assert!(s.execute("describe.set", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("describe.confirm", &json!({"id": 999_999})).is_err());
    assert_eq!(s.execute("describe.undo", &json!({})).unwrap()["undone"], Value::Null);
}

#[test]
fn proposals_need_the_model_and_a_choice_of_photos() {
    let (mut s, dir) = library();
    write_png(&dir.join("p.png"), 10);
    let pid = ids(&import(&mut s, &dir.join("p.png")))[0];
    let e = s.execute("describe.suggest", &json!({"ids": [pid.0]})).unwrap_err().to_string();
    assert!(e.contains("not installed") || e.contains("not available") || e.contains("no folder is set"), "{e}");
    assert!(store(&s).all().is_empty(), "nothing invented");
    s.selection.ids.clear();
    s.selection.active = None;
    assert!(s.execute("describe.suggest", &json!({})).is_err());
    assert_eq!(s.execute("describe.status", &json!({})).unwrap()["running"], false);
}

#[test]
fn pictures_are_cut_in_the_background_and_folders_bring_videos_only_when_asked() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let folder = dir.join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    write_png(&folder.join("p.png"), 3);
    make_video(&folder.join("a.mp4"), 4, true);
    make_video(&folder.join("b.mov"), 4, true);
    let r = s.execute("library.import", &json!({"paths": [folder.to_string_lossy()]})).unwrap();
    assert_eq!(ids(&r).len(), 1, "by default a folder brings its photos only: {r}");
    let r = s.execute("library.import", &json!({"paths": [folder.to_string_lossy()], "videos": true})).unwrap();
    assert_eq!(ids(&r).len(), 2, "the videos, the photo is already there: {r}");
    // again: nothing new (an interrupted import of a big folder simply resumes)
    assert!(ids(&s.execute("library.import", &json!({"paths": [folder.to_string_lossy()], "videos": true})).unwrap()).is_empty());
    let r = s.execute("video.frames", &json!({"background": true})).unwrap();
    assert_eq!(r["background"], true, "{r}");
    let t = std::time::Instant::now();
    loop {
        let st = s.execute("video.status", &json!({})).unwrap();
        if st["job"]["running"] == false {
            assert_eq!((st["job"]["videos"].as_u64(), st["withFrames"].as_u64()), (Some(2), Some(2)), "{st}");
            break;
        }
        assert!(t.elapsed().as_secs() < 120, "{st}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(s.video_frames().len(), 4, "the cache sees the run's pictures");
    assert_eq!(s.execute("video.cancel", &json!({})).unwrap()["cancelled"], false);
    // a cancelled run keeps what it cut
    let r = s.execute("video.frames", &json!({"background": true, "force": true, "every": 0.5})).unwrap();
    assert_eq!(r["background"], true);
    s.execute("video.cancel", &json!({})).unwrap();
    while s.execute("video.status", &json!({})).unwrap()["job"]["running"] == true {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(s.execute("video.frames", &json!({})).is_ok(), "a new run starts after a cancelled one");
}

#[test]
fn video_catchers_videos_missing_from_the_catalog_are_imported_when_asked() {
    if !have_ffmpeg() || !Path::new("/usr/bin/sqlite3").exists() {
        return;
    }
    let (mut s, dir) = library();
    let v = dir.join("vc clip.mp4");
    make_video(&v, 3, true);
    let db = dir.join("vc.sqlite3");
    let sql = format!(
        "CREATE TABLE videos (id INTEGER PRIMARY KEY, path TEXT, filename TEXT, fingerprint TEXT, duration_ms INTEGER);
         CREATE TABLE frames (id INTEGER PRIMARY KEY, video_id INTEGER, ordinal INTEGER, timestamp_ms INTEGER, path TEXT);
         CREATE TABLE moments (id INTEGER PRIMARY KEY, frame_id INTEGER, video_id INTEGER, timestamp_ms INTEGER, action_text TEXT, scene_text TEXT, description TEXT, machine_action TEXT, machine_scene TEXT, machine_description TEXT, status TEXT);
         CREATE TABLE persons (id INTEGER PRIMARY KEY, name TEXT);
         CREATE TABLE moment_people (moment_id INTEGER, person_id INTEGER, source TEXT);
         INSERT INTO videos VALUES (1, '{}', 'vc clip.mp4', 'f', 3000), (2, '/nowhere/gone.mp4', 'gone.mp4', 'g', 1000), (3, '/nowhere/copy of clip.mp4', 'copy.mp4', 'f', 3000);
         INSERT INTO moments VALUES (5, 1, 1, 500, 'parle', 'studio', '', '', '', '', 'candidate');
         INSERT INTO persons VALUES (7, 'Madson');
         INSERT INTO moment_people VALUES (5, 7, 'confirmed_cluster');",
        v.display().to_string().replace('\'', "''")
    );
    assert!(Command::new("/usr/bin/sqlite3").arg(&db).arg(sql).status().unwrap().success());
    let r = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy()})).unwrap();
    assert_eq!((r["matched"].as_u64(), r["created"].as_u64()), (Some(0), Some(0)), "without the option nothing is imported: {r}");
    let r = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy(), "createMissing": true, "dryRun": true})).unwrap();
    assert_eq!((r["created"].as_u64(), r["offline"].as_u64()), (Some(1), Some(2)), "{r}");
    assert_eq!(s.catalog.photos().count(), 0, "a dry run imports nothing");
    let r = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy(), "createMissing": true})).unwrap();
    assert_eq!(
        (r["created"].as_u64(), r["matched"].as_u64(), r["annotationsAdded"].as_u64(), r["personHints"].as_u64()),
        (Some(1), Some(1), Some(1), Some(1)),
        "{r}"
    );
    assert_eq!((r["duplicates"].as_u64(), r["unmatched"].as_u64()), (Some(1), Some(1)), "the copy (same fingerprint) is the same video: {r}");
    let vid = s.catalog.photos().next().unwrap().id;
    let l = s.execute("describe.list", &json!({"id": vid.0})).unwrap();
    assert_eq!(l["annotations"][0]["hints"], json!(["videocatcher:Madson"]), "a hint, not a person: {l}");
    assert_eq!(l["annotations"][0]["people"], json!([]));
    // again: nothing more
    let r = s.execute("video.importVideoCatcher", &json!({"db": db.to_string_lossy(), "createMissing": true})).unwrap();
    assert_eq!((r["created"].as_u64(), r["annotationsAdded"].as_u64()), (Some(0), Some(0)), "{r}");
}

/// What the Videos view draws: the list (people, analysis, filters), a video's moments, a photo's
/// one moment, and the player's command line.

/// Videos an import could not read are listed with their cause; a file that imports later leaves
/// the list.
#[test]
fn unreadable_videos_are_listed_with_their_cause() {
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    let folder = dir.join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("empty.mp4"), b"").unwrap();
    make_video(&folder.join("good.mp4"), 2, true);
    let good = std::fs::read(folder.join("good.mp4")).unwrap();
    // a transfer cut short: the start of an MP4 without its index
    std::fs::write(folder.join("cut.mp4"), &good[..good.len() / 4]).unwrap();
    s.execute("library.import", &json!({"paths": [folder.to_string_lossy()], "videos": true})).unwrap();
    let u = s.execute("video.unreadable", &json!({})).unwrap();
    let causes: std::collections::HashMap<String, String> =
        u["videos"].as_array().unwrap().iter().map(|v| (v["name"].as_str().unwrap().to_string(), v["cause"].as_str().unwrap().to_string())).collect();
    assert_eq!(causes.get("empty.mp4").map(String::as_str), Some("empty"), "{u}");
    assert!(causes.contains_key("cut.mp4") && !causes.contains_key("good.mp4"), "{u}");
    assert!(u["videos"][0]["folder"].as_str().unwrap().ends_with("shoot"));
    // the file is repaired (here: replaced by a sound copy) and imported: it leaves the list
    std::fs::write(folder.join("cut.mp4"), {
        let mut b = good.clone();
        b.extend_from_slice(b"different content");
        b
    })
    .unwrap();
    s.execute("library.import", &json!({"paths": [folder.join("cut.mp4").to_string_lossy()]})).unwrap();
    let u = s.execute("video.unreadable", &json!({})).unwrap();
    assert_eq!(u["total"], 1, "{u}");
}
