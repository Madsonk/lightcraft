//! Annotations (`describe.*`): what a person writes wins over proposals, confirm / delete / undo,
//! the search "who + what + when" over photos and moments of videos (faces injected, no model),
//! proposals refused without the model, `video.frames` in the background, folder imports with
//! videos, and Video Catcher's videos brought into the catalog.

use std::path::Path;
use std::process::Command;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::describe::{Annotation, Store};
use crate::tests_video::people::{face, write_png};
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
    s.searcher.dir = Some(temp_dir("no-search-model"));
    write_png(&dir.join("p.png"), 10);
    let pid = ids(&import(&mut s, &dir.join("p.png")))[0];
    let e = s.execute("describe.suggest", &json!({"ids": [pid.0]})).unwrap_err().to_string();
    assert!(e.contains("not installed") || e.contains("not available"), "{e}");
    assert!(store(&s).all().is_empty(), "nothing invented");
    s.selection.ids.clear();
    s.selection.active = None;
    assert!(s.execute("describe.suggest", &json!({})).is_err());
    assert_eq!(s.execute("describe.status", &json!({})).unwrap()["running"], false);
}

#[cfg(feature = "people")]
#[test]
fn search_finds_who_and_what_at_the_same_moment_and_never_widens_in_silence() {
    use crate::people::ops::{inject, inject_keys};
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    s.searcher.dir = Some(temp_dir("no-search-model"));
    write_png(&dir.join("p.png"), 10);
    let photo = ids(&import(&mut s, &dir.join("p.png")))[0];
    let v = dir.join("clip.mp4");
    make_video(&v, 12, true);
    let vid = ids(&import(&mut s, &v))[0];
    s.execute("video.frames", &json!({"every": 2.0})).unwrap();
    let item = ck(&s, vid);
    let ms: Vec<u64> = s.frames_of_video(vid).iter().map(|(_, f)| f.ms).collect();
    assert_eq!(ms, vec![1000, 3000, 5000, 7000, 9000, 11000]);
    // Awa on the photo and at 1 s, 3 s, 9 s of the video; Bao at 9 s
    inject(&mut s, vec![(photo, vec![(face(1, 0), [0.1, 0.1, 0.4, 0.5])])]);
    let k = |ms| crate::video::frame_key(&item, ms);
    inject_keys(
        &mut s,
        vec![
            (k(1000), vec![(face(1, 1), [0.2, 0.2, 0.4, 0.5])]),
            (k(3000), vec![(face(1, 2), [0.2, 0.2, 0.4, 0.5])]),
            (k(9000), vec![(face(1, 3), [0.2, 0.2, 0.4, 0.5]), (face(2, 0), [0.6, 0.2, 0.8, 0.5])]),
            (k(11000), vec![(face(2, 1), [0.6, 0.2, 0.8, 0.5])]),
        ],
    )
    .unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    let group = |n: u64| l["groups"].as_array().unwrap().iter().find(|g| g["faces"] == n).unwrap()["id"].as_str().unwrap().to_string();
    let (ga, gb) = (group(4), group(2));
    s.execute("people.name", &json!({"group": ga, "name": "Awa Diallo"})).unwrap();
    s.execute("people.name", &json!({"group": gb, "name": "Bao"})).unwrap();
    // what happens: the person writes 1 s, Video Catcher described 9 s
    s.execute("describe.set", &json!({"id": vid.0, "ms": 1000, "action": "conduit une voiture", "scene": "route de campagne"})).unwrap();
    store(&s).add(vec![Annotation::new(&item, Some(9000), "salle de danse", "deux personnes dansent", "", "videocatcher", "t")]).unwrap();
    s.execute("describe.set", &json!({"id": photo.0, "scene": "Voiture garée devant la maison"})).unwrap();

    let q = |s: &mut Session, p: Value| s.execute("describe.search", &p).unwrap();
    // who + what, accents and word forms ignored
    let r = q(&mut s, json!({"query": "Awa conduisant une voiture"}));
    assert_eq!(r["people"], json!(["Awa Diallo"]), "{r}");
    assert_eq!(r["semantic"]["used"], false, "no model here: {r}");
    let hits: Vec<(u64, Option<u64>)> = r["results"].as_array().unwrap().iter().map(|x| (x["id"].as_u64().unwrap(), x["ms"].as_u64())).collect();
    assert!(hits.contains(&(vid.0, Some(1000))), "{r}");
    assert!(!hits.contains(&(photo.0, None)), "every word must be there: the photo has « voiture », not « conduit »: {r}");
    let r = q(&mut s, json!({"query": "Awa voiture"}));
    let hits: Vec<(u64, Option<u64>)> = r["results"].as_array().unwrap().iter().map(|x| (x["id"].as_u64().unwrap(), x["ms"].as_u64())).collect();
    assert!(hits.contains(&(photo.0, None)) && hits.contains(&(vid.0, Some(1000))), "{r}");
    assert!(!hits.iter().any(|h| h.1 == Some(9000)), "{r}");
    // Awa there but the words not found: apart, never mixed in
    let diag = &r["diagnostics"]["personPresentWhatNotDetected"];
    assert!(diag["total"].as_u64().unwrap() >= 1, "{r}");
    assert!(diag["examples"].as_array().unwrap().iter().any(|x| x["ms"] == 9000));
    // two people at the same moment (within the window): only 9 s (and 11 s: Awa is 2 s away)
    let r = q(&mut s, json!({"query": "danse", "people": ["Awa Diallo", "bao"], "window": 0}));
    let hits: Vec<Option<u64>> = r["results"].as_array().unwrap().iter().map(|x| x["ms"].as_u64()).collect();
    assert_eq!(hits, vec![Some(9000)], "{r}");
    let r = q(&mut s, json!({"people": ["Awa Diallo", "Bao"], "window": 2}));
    let mut hits: Vec<u64> = r["results"].as_array().unwrap().iter().filter_map(|x| x["ms"].as_u64()).collect();
    hits.sort();
    assert_eq!(hits, vec![7000, 9000, 11000], "both within 2 s of each of these: {r}");
    // a word nobody wrote: nothing, said plainly
    let r = q(&mut s, json!({"query": "Awa plongée sous-marine"}));
    assert_eq!(r["total"], 0, "{r}");
    // kinds
    let r = q(&mut s, json!({"query": "voiture", "kind": "photo"}));
    assert!(r["results"].as_array().unwrap().iter().all(|x| x["kind"] == "photo") && r["total"] == 1, "{r}");
    let r = q(&mut s, json!({"query": "voiture", "kind": "video"}));
    assert_eq!(r["total"], 1, "{r}");
    // the moment's words found but not its people
    let r = q(&mut s, json!({"query": "Bao conduit"}));
    assert_eq!(r["total"], 0, "{r}");
    assert_eq!(r["diagnostics"]["whatFoundPeopleNotRecognized"]["total"], 1, "{r}");
    // hostile or empty queries
    assert!(s.execute("describe.search", &json!({})).is_err());
    assert!(s.execute("describe.search", &json!({"query": "  le la "})).is_err());
    assert!(s.execute("describe.search", &json!({"people": ["Personne"]})).unwrap_err().to_string().contains("Personne"));
    assert!(s.execute("describe.search", &json!({"query": "x", "kind": "audio"})).is_err());
    let r = q(&mut s, json!({"query": "voiture", "limit": -5, "window": f64::MAX}));
    assert!(r["total"].as_u64().is_some());
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
#[cfg(feature = "people")]
#[test]
fn the_videos_view_data_lists_people_moments_and_the_player() {
    use crate::people::ops::inject_keys;
    if !have_ffmpeg() {
        return;
    }
    let (mut s, dir) = library();
    write_png(&dir.join("p.png"), 10);
    let photo = ids(&import(&mut s, &dir.join("p.png")))[0];
    let v = dir.join("clip.mp4");
    make_video(&v, 6, true);
    let vid = ids(&import(&mut s, &v))[0];
    let l = s.execute("video.list", &json!({})).unwrap();
    assert_eq!((l["total"].as_u64(), l["videos"][0]["frames"].as_u64(), l["videos"][0]["moments"].as_u64()), (Some(1), Some(0), Some(0)), "{l}");
    s.execute("video.frames", &json!({})).unwrap();
    let item = ck(&s, vid);
    inject_keys(
        &mut s,
        vec![
            (crate::video::frame_key(&item, 1000), vec![(face(1, 0), [0.2, 0.2, 0.4, 0.5])]),
            (crate::video::frame_key(&item, 3000), vec![(face(1, 1), [0.2, 0.2, 0.4, 0.5])]),
        ],
    )
    .unwrap();
    let g = s.execute("people.list", &json!({})).unwrap()["groups"][0]["id"].as_str().unwrap().to_string();
    s.execute("people.name", &json!({"group": g, "name": "Awa"})).unwrap();
    s.execute("describe.set", &json!({"id": vid.0, "ms": 3000, "action": "danse"})).unwrap();
    let l = s.execute("video.list", &json!({})).unwrap();
    let r = &l["videos"][0];
    assert_eq!(
        (r["frames"].as_u64(), r["facesLooked"].as_u64(), r["moments"].as_u64(), r["toConfirm"].as_u64()),
        (Some(3), Some(2), Some(1), Some(1)),
        "{r}"
    );
    assert_eq!(r["people"], json!(["Awa"]));
    assert!(Path::new(r["poster"].as_str().unwrap()).is_file());
    assert_eq!(s.execute("video.list", &json!({"person": "awa"})).unwrap()["total"], 1);
    assert_eq!(s.execute("video.list", &json!({"person": "Bao"})).unwrap()["total"], 0);
    assert_eq!(s.execute("video.list", &json!({"described": false})).unwrap()["total"], 0);
    assert_eq!(s.execute("video.list", &json!({"status": "confirmed"})).unwrap()["total"], 0);
    s.execute("describe.confirm", &json!({"id": vid.0, "ms": 3000})).unwrap();
    assert_eq!(s.execute("video.list", &json!({"status": "confirmed"})).unwrap()["total"], 1);
    assert_eq!(s.execute("video.list", &json!({"status": "toConfirm"})).unwrap()["total"], 0);
    let m = s.execute("video.moments", &json!({"id": vid.0})).unwrap();
    let ms: Vec<u64> = m["moments"].as_array().unwrap().iter().filter_map(|x| x["ms"].as_u64()).collect();
    assert_eq!(ms, vec![1000, 3000, 5000], "{m}");
    assert_eq!(m["moments"][1]["people"], json!(["Awa"]));
    assert_eq!(m["moments"][1]["annotation"]["status"], "confirmed");
    assert!(m["moments"][0]["faces"][0]["rect"].is_array());
    let p = s.execute("video.moments", &json!({"id": photo.0})).unwrap();
    assert_eq!((p["video"].as_bool(), p["moments"].as_array().unwrap().len()), (Some(false), 1), "{p}");
    assert!(s.execute("video.moments", &json!({"id": 999_999})).is_err());
    let o = s.execute("video.open", &json!({"id": vid.0, "ms": 83_503, "dryRun": true})).unwrap();
    assert_eq!(o["timecode"], "1:23");
    if o["atTime"] == true {
        assert!(o["args"].as_array().unwrap().iter().any(|a| a == "--start-time=83"), "{o}");
    }
    assert!(s.execute("video.open", &json!({"id": photo.0, "dryRun": true})).is_err(), "a photo is not played");
    // a short clip around a moment, made once
    let c = s.execute("video.clip", &json!({"id": vid.0, "ms": 3000, "seconds": 2})).unwrap();
    assert_eq!((c["startMs"].as_u64(), c["lengthMs"].as_u64(), c["cached"].as_bool()), (Some(2000), Some(2000), Some(false)), "{c}");
    let info = crate::video::ffmpeg::probe(Path::new(c["path"].as_str().unwrap())).unwrap();
    assert!(info.duration.is_some_and(|d| (1.5..=2.6).contains(&d)) && info.height <= 540, "{info:?}");
    assert_eq!(s.execute("video.clip", &json!({"id": vid.0, "ms": 3000, "seconds": 2})).unwrap()["cached"], true);
    // at the end of the video the clip starts earlier; hostile values are clamped
    let c = s.execute("video.clip", &json!({"id": vid.0, "ms": 1e300, "seconds": -4})).unwrap();
    assert_eq!((c["startMs"].as_u64(), c["lengthMs"].as_u64()), (Some(5000), Some(1000)), "{c}");
    assert!(s.execute("video.clip", &json!({"id": photo.0, "ms": 0})).is_err());
}

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
