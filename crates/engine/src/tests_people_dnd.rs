//! People: "Unknown person", "Skip for now", "Not a real face", moving one face to someone else and
//! filing whole photos under a person, with photos that hold several people. Faces are injected
//! into the index (no model needed). The rule under test: faces belong to people, keywords belong
//! to photos — a keyword leaves a photo only when none of that person's faces, and no hand-filed
//! photo tag, remain in it, and nobody else's keyword is ever touched.

use std::path::Path;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::people::ops::inject;

const DIM: usize = lightcraft_people::DIM;
const R4: [f32; 4] = [0.1, 0.1, 0.4, 0.5];
const R4B: [f32; 4] = [0.6, 0.1, 0.9, 0.5];

fn temp_dir(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("lc-people-dnd-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let data: Vec<[u8; 4]> = (0..64 * 48).map(|i| [(i % 64) as u8 * 3, (i / 64) as u8 * 4, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

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

/// Eight photos, three identities (A = 1, B = 2, C = 3):
/// p0 A · p1 A · p2 A+B · p3 B · p4 B · p5 A+A (the same person twice) · p6 C · p7 B+C.
fn session() -> (Session, Vec<PhotoId>) {
    let dir = temp_dir("lib");
    for i in 0..8u8 {
        write_png(&dir.join(format!("p{i}.png")), i * 20);
    }
    let mut s = Session::new().with_fs();
    s.people.dir = Some(temp_dir("models"));
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut ids: Vec<PhotoId> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).map(PhotoId).collect();
    ids.sort_by_key(|i| i.0);
    assert_eq!(ids.len(), 8);
    let faces: Vec<Vec<(u64, u64, [f32; 4])>> = vec![
        vec![(1, 0, R4)],
        vec![(1, 1, R4)],
        vec![(1, 2, R4), (2, 3, R4B)],
        vec![(2, 4, R4)],
        vec![(2, 5, R4)],
        vec![(1, 6, R4), (1, 7, R4B)],
        vec![(3, 8, R4)],
        vec![(2, 9, R4), (3, 10, R4B)],
    ];
    let photos = ids.iter().zip(faces).map(|(id, fs)| (*id, fs.into_iter().map(|(w, n, r)| (face(w, n), r)).collect())).collect();
    inject(&mut s, photos);
    (s, ids)
}

fn kw(s: &Session, id: PhotoId) -> Vec<String> {
    let mut k = s.catalog.photo(id).unwrap().meta.keywords.clone();
    k.sort();
    k
}

fn has(s: &Session, id: PhotoId, k: &str) -> bool {
    kw(s, id).iter().any(|x| x == k)
}

/// The faces of a photo, in the order of the index: `[(face id, who)]`.
fn faces(s: &mut Session, id: PhotoId) -> Vec<(String, Value)> {
    let v = s.execute("people.faces", &json!({"id": id.0})).unwrap();
    v["faces"].as_array().unwrap().iter().map(|f| (f["face"].as_str().unwrap().to_string(), f.clone())).collect()
}

/// The group (`g:N`) any of whose listed faces is `face`, among the groups to name or the unknown ones.
fn group_with(s: &mut Session, face: &str) -> String {
    let l = s.execute("people.list", &json!({"groups": 100, "thumbs": 20})).unwrap();
    for key in ["groups", "unknown"] {
        for g in l[key].as_array().unwrap() {
            if g["thumbs"].as_array().unwrap().iter().any(|t| t["face"] == face) {
                return g["id"].as_str().unwrap().to_string();
            }
        }
    }
    panic!("no group with {face}: {l}");
}

/// Name the people: A on p0's face, B on p3's, C on p6's.
fn named(s: &mut Session, ids: &[PhotoId]) {
    for (photo, name) in [(0, "Awa"), (3, "Bao"), (6, "Cleo")] {
        let f = faces(s, ids[photo])[0].0.clone();
        let g = group_with(s, &f);
        s.execute("people.name", &json!({"group": g, "name": name})).unwrap();
    }
}

fn person(s: &mut Session, name: &str) -> String {
    let l = s.execute("people.list", &json!({})).unwrap();
    l["persons"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap_or_else(|| panic!("{name}: {l}"))["id"].as_str().unwrap().to_string()
}

fn people_json(s: &Session) -> Value {
    let p = s.library.as_ref().unwrap().dir.join("people").join("people.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn an_unknown_group_leaves_the_list_without_a_keyword_and_can_be_named_later() {
    let (mut s, ids) = session();
    let before = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((before["groupsTotal"].as_u64(), before["unknownTotal"].as_u64()), (Some(3), Some(0)), "{before}");
    let f = faces(&mut s, ids[6])[0].0.clone();
    let g = group_with(&mut s, &f);
    let r = s.execute("people.unknown", &json!({"group": g})).unwrap();
    assert_eq!(r["unknown"], 2, "{r}");
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((l["groupsTotal"].as_u64(), l["unknownTotal"].as_u64(), l["unknownFaces"].as_u64()), (Some(2), Some(1), Some(2)), "{l}");
    assert!(l["groups"].as_array().unwrap().iter().all(|g| g["faces"] != 2));
    // no keyword anywhere, no person created
    assert!(ids.iter().all(|i| kw(&s, *i).is_empty()));
    assert!(l["persons"].as_array().unwrap().is_empty());
    // the faces say so
    assert_eq!(faces(&mut s, ids[6])[0].1["unknown"], true);
    // Group again does not bring them back
    s.execute("people.refresh", &json!({"background": false})).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((l["groupsTotal"].as_u64(), l["unknownTotal"].as_u64()), (Some(2), Some(1)), "{l}");
    // later: name the unknown group
    let ug = l["unknown"][0]["id"].as_str().unwrap().to_string();
    s.execute("people.name", &json!({"group": ug, "name": "Cleo"})).unwrap();
    assert!(has(&s, ids[6], "People|Cleo") && has(&s, ids[7], "People|Cleo"));
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!(l["unknownTotal"], 0, "{l}");
    // undo steps back through naming, then the unknown mark
    s.execute("people.undo", &json!({})).unwrap();
    assert!(!has(&s, ids[6], "People|Cleo"));
    assert_eq!(s.execute("people.list", &json!({})).unwrap()["unknownTotal"], 1);
    s.execute("people.undo", &json!({})).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((l["groupsTotal"].as_u64(), l["unknownTotal"].as_u64()), (Some(3), Some(0)), "{l}");
    // "off" puts them back too
    let g = group_with(&mut s, &f);
    s.execute("people.unknown", &json!({"group": g})).unwrap();
    let ug = s.execute("people.list", &json!({})).unwrap()["unknown"][0]["id"].as_str().unwrap().to_string();
    s.execute("people.unknown", &json!({"group": ug, "off": true})).unwrap();
    assert_eq!(s.execute("people.list", &json!({})).unwrap()["unknownTotal"], 0);
}

#[test]
fn tag_unknown_is_off_by_default_and_when_on_marks_only_the_photos_of_unknown_faces() {
    let (mut s, ids) = session();
    named(&mut s, &ids);
    let st = s.execute("people.settings", &json!({})).unwrap();
    assert_eq!(st["settings"]["tagUnknown"], false, "{st}");
    // p7 holds B and C: C is unknown, B is not
    let c = faces(&mut s, ids[7])[1].0.clone();
    s.execute("people.unknown", &json!({"face": c})).unwrap();
    assert!(!has(&s, ids[7], "People|Unknown"), "off by default");
    assert!(has(&s, ids[7], "People|Bao"), "the known person of the photo keeps theirs");
    s.execute("people.settings", &json!({"tagUnknown": true})).unwrap();
    assert!(has(&s, ids[7], "People|Unknown") && has(&s, ids[7], "People|Bao"));
    // p6 (C only) is not unknown-marked (only p7's C face was): untouched
    assert!(!has(&s, ids[6], "People|Unknown"));
    // a new unknown face: its photo gets the keyword inside the same undo step
    let f6 = faces(&mut s, ids[6])[0].0.clone();
    s.execute("people.unknown", &json!({"face": f6})).unwrap();
    assert!(has(&s, ids[6], "People|Unknown"));
    s.execute("people.undo", &json!({})).unwrap();
    assert!(!has(&s, ids[6], "People|Unknown") && has(&s, ids[7], "People|Unknown"));
    // switching it off takes the keyword away from every photo (one undo step)
    s.execute("people.settings", &json!({"tagUnknown": false})).unwrap();
    assert!(ids.iter().all(|i| !has(&s, *i, "People|Unknown")));
    s.execute("people.undo", &json!({})).unwrap();
    assert!(has(&s, ids[7], "People|Unknown"));
    // nobody can be named Unknown (it is that keyword)
    let f7 = faces(&mut s, ids[7])[1].0.clone();
    let g = group_with(&mut s, &f7);
    assert!(s.execute("people.name", &json!({"group": g, "name": "unknown"})).is_err());
}

#[test]
fn skipping_hides_a_group_for_now_decides_nothing_and_group_again_brings_it_back() {
    let (mut s, ids) = session();
    let f = faces(&mut s, ids[6])[0].0.clone();
    let g = group_with(&mut s, &f);
    let undo_before = s.undo.len();
    let r = s.execute("people.skip", &json!({"group": g})).unwrap();
    assert_eq!(r["skippedFaces"], 2, "{r}");
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((l["groupsTotal"].as_u64(), l["skippedGroups"].as_u64()), (Some(2), Some(1)), "{l}");
    // nothing decided: no history, no keyword, no person, no undo step
    assert!(s.execute("people.undo", &json!({})).is_err());
    assert_eq!(s.undo.len(), undo_before);
    assert!(l["persons"].as_array().unwrap().is_empty() && l["unknownTotal"] == 0);
    // regrouping because of another decision keeps it hidden
    let f0 = faces(&mut s, ids[0])[0].0.clone();
    let g0 = group_with(&mut s, &f0);
    s.execute("people.name", &json!({"group": g0, "name": "Awa"})).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!(l["skippedGroups"], 1, "{l}");
    // "Group again" shows it
    s.execute("people.refresh", &json!({"background": false})).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!(l["skippedGroups"], 0, "{l}");
    assert!(group_with(&mut s, &f).starts_with("g:"));
    // clear shows it without regrouping
    let g = group_with(&mut s, &f);
    s.execute("people.skip", &json!({"group": g})).unwrap();
    s.execute("people.skip", &json!({"clear": true})).unwrap();
    assert_eq!(s.execute("people.list", &json!({})).unwrap()["skippedGroups"], 0);
}

#[test]
fn moving_one_face_changes_keywords_per_photo_never_dropping_a_person_still_in_the_photo() {
    let (mut s, ids) = session();
    named(&mut s, &ids);
    // p2 holds Awa and Bao
    assert_eq!(kw(&s, ids[2]), vec!["People|Awa".to_string(), "People|Bao".to_string()]);
    // p5 holds Awa twice
    assert_eq!(kw(&s, ids[5]), vec!["People|Awa".to_string()]);
    let (awa, bao, cleo) = (person(&mut s, "Awa"), person(&mut s, "Bao"), person(&mut s, "Cleo"));
    let undo0 = s.undo.len();

    // (1) one of p5's two Awa faces is really Bao: Bao is added, Awa stays (another Awa face is there)
    let p5 = faces(&mut s, ids[5]);
    let r = s.execute("people.move", &json!({"faces": [p5[1].0], "to": bao})).unwrap();
    assert_eq!(r["moved"], 1, "{r}");
    assert_eq!(kw(&s, ids[5]), vec!["People|Awa".to_string(), "People|Bao".to_string()]);
    // the face now belongs to Bao and Awa is told "not them" for it
    let f = faces(&mut s, ids[5]);
    assert_eq!(f[1].1["name"], "Bao");
    assert_eq!(f[0].1["name"], "Awa");
    // one undo step for the whole move
    assert_eq!(s.undo.len(), undo0 + 1);
    // the second Awa face moves too: now Awa leaves p5, the other photos keep theirs
    s.execute("people.move", &json!({"faces": [p5[0].0], "to": bao})).unwrap();
    assert_eq!(kw(&s, ids[5]), vec!["People|Bao".to_string()]);
    assert!(has(&s, ids[0], "People|Awa") && has(&s, ids[1], "People|Awa") && has(&s, ids[2], "People|Awa"));
    // undo, undo: back to Awa only
    s.execute("people.undo", &json!({})).unwrap();
    s.execute("people.undo", &json!({})).unwrap();
    assert_eq!(kw(&s, ids[5]), vec!["People|Awa".to_string()]);
    assert_eq!(s.undo.len(), undo0);

    // (2) p2's Bao face is really Cleo: p2 loses Bao (no other Bao face in it), keeps Awa
    let p2 = faces(&mut s, ids[2]);
    s.execute("people.move", &json!({"faces": [p2[1].0], "to": cleo})).unwrap();
    assert_eq!(kw(&s, ids[2]), vec!["People|Awa".to_string(), "People|Cleo".to_string()]);
    assert!(has(&s, ids[3], "People|Bao") && has(&s, ids[4], "People|Bao") && has(&s, ids[7], "People|Bao"));
    s.execute("people.undo", &json!({})).unwrap();
    assert_eq!(kw(&s, ids[2]), vec!["People|Awa".to_string(), "People|Bao".to_string()]);

    // moving to the person it already is does nothing and records nothing
    let n = s.undo.len();
    let r = s.execute("people.move", &json!({"faces": [p2[0].0], "to": awa})).unwrap();
    assert_eq!(r["moved"], 0);
    assert_eq!(s.undo.len(), n);
}

#[test]
fn unknown_and_not_a_real_face_on_one_face_never_remove_the_other_people_of_the_photo() {
    let (mut s, ids) = session();
    named(&mut s, &ids);
    // p2 = Awa + Bao. Awa's face there is "not a real face": Awa leaves p2 only (p2 has no other Awa face), Bao stays
    let p2 = faces(&mut s, ids[2]);
    s.execute("people.ignore", &json!({"face": p2[0].0})).unwrap();
    assert_eq!(kw(&s, ids[2]), vec!["People|Bao".to_string()]);
    assert!(has(&s, ids[0], "People|Awa") && has(&s, ids[1], "People|Awa") && has(&s, ids[5], "People|Awa"));
    let f = faces(&mut s, ids[2]);
    assert_eq!(f[0].1["ignored"], true);
    s.execute("people.undo", &json!({})).unwrap();
    assert_eq!(kw(&s, ids[2]), vec!["People|Awa".to_string(), "People|Bao".to_string()]);
    // p7 = Bao + Cleo: Cleo's face is unknown, Bao's keyword is not touched and no Unknown keyword appears
    let p7 = faces(&mut s, ids[7]);
    s.execute("people.unknown", &json!({"face": p7[1].0})).unwrap();
    assert_eq!(kw(&s, ids[7]), vec!["People|Bao".to_string()]);
    // p5 holds Awa twice: one of them "not a real face" leaves the keyword in place
    let p5 = faces(&mut s, ids[5]);
    s.execute("people.ignore", &json!({"face": p5[1].0})).unwrap();
    assert_eq!(kw(&s, ids[5]), vec!["People|Awa".to_string()]);
    // dropping a person's every face on "not a real face" via people.move works the same way
    let p3 = faces(&mut s, ids[3]);
    s.execute("people.move", &json!({"faces": [p3[0].0], "to": "ignored"})).unwrap();
    assert!(kw(&s, ids[3]).is_empty());
    assert!(has(&s, ids[4], "People|Bao"));
    // a photo with a known face and an unknown one keeps the known one's keyword
    assert_eq!(kw(&s, ids[7]), vec!["People|Bao".to_string()]);
}

#[test]
fn photos_filed_under_a_person_by_hand_survive_sync_and_a_face_moving_away() {
    let (mut s, ids) = session();
    named(&mut s, &ids);
    let awa = person(&mut s, "Awa");
    // p6 shows only Cleo: dropped on Awa as a whole photo
    let r = s.execute("people.tagPhotos", &json!({"person": awa, "ids": [ids[6].0, ids[2].0]})).unwrap();
    assert_eq!((r["photos"].as_u64(), r["added"].as_u64(), r["alreadyTagged"].as_u64()), (Some(2), Some(1), Some(1)), "{r}");
    assert_eq!(kw(&s, ids[6]), vec!["People|Awa".to_string(), "People|Cleo".to_string()], "Cleo's keyword is untouched");
    // no doubles: again adds nothing
    let r = s.execute("people.tagPhotos", &json!({"person": awa, "ids": [ids[6].0]})).unwrap();
    assert_eq!(r["added"], 0);
    assert_eq!(kw(&s, ids[6]).iter().filter(|k| *k == "People|Awa").count(), 1);
    // sync (and a regrouping) never removes it
    let sy = s.execute("people.sync", &json!({})).unwrap();
    assert_eq!(sy["removed"], 0, "{sy}");
    s.execute("people.refresh", &json!({"background": false})).unwrap();
    assert!(has(&s, ids[6], "People|Awa"));
    // p2 was tagged by hand too: Awa's face there moves to Bao, but the photo stays Awa's
    let p2 = faces(&mut s, ids[2]);
    let bao = person(&mut s, "Bao");
    s.execute("people.move", &json!({"faces": [p2[0].0], "to": bao})).unwrap();
    assert!(has(&s, ids[2], "People|Awa") && has(&s, ids[2], "People|Bao"));
    // and the filed photos are in the person's photos
    let ph = s.execute("people.photos", &json!({"person": awa})).unwrap();
    assert!(ph["ids"].as_array().unwrap().iter().any(|i| i.as_u64() == Some(ids[6].0)), "{ph}");
    // a face-derived keyword does go (p1 loses Awa when its only Awa face leaves)
    let p1 = faces(&mut s, ids[1]);
    s.execute("people.move", &json!({"faces": [p1[0].0], "to": "unknown"})).unwrap();
    assert!(!has(&s, ids[1], "People|Awa"));
    // untag by hand
    let r = s.execute("people.untagPhotos", &json!({"person": awa, "ids": [ids[6].0]})).unwrap();
    assert_eq!(r["removed"], 1, "{r}");
    assert!(!has(&s, ids[6], "People|Awa") && has(&s, ids[6], "People|Cleo"));
    // an unnamed person cannot be filed under
    let e = s.execute("people.tagPhotos", &json!({"person": "p:999", "ids": [ids[6].0]}));
    assert!(e.is_err());
    // deleting the person removes the hand-filed keyword as well
    s.execute("people.tagPhotos", &json!({"person": awa, "ids": [ids[7].0]})).unwrap();
    assert!(has(&s, ids[7], "People|Awa"));
    s.execute("people.delete", &json!({"person": awa})).unwrap();
    assert!(ids.iter().all(|i| !has(&s, *i, "People|Awa")));
}

#[test]
fn keyword_apply_adds_once_in_one_undo_step_and_a_person_keyword_files_the_photo_under_them() {
    let (mut s, ids) = session();
    named(&mut s, &ids);
    let n = s.undo.len();
    let sel: Vec<u64> = ids[..3].iter().map(|i| i.0).collect();
    let r = s.execute("keyword.apply", &json!({"keyword": "Places|France|Poitiers", "ids": sel})).unwrap();
    assert_eq!((r["added"].as_u64(), r["alreadyHad"].as_u64()), (Some(3), Some(0)), "{r}");
    assert_eq!(s.undo.len(), n + 1, "one undo step");
    assert!(ids[..3].iter().all(|i| has(&s, *i, "Places|France|Poitiers")));
    // photos 1..4: two already have it
    let sel: Vec<u64> = ids[1..4].iter().map(|i| i.0).collect();
    let r = s.execute("keyword.apply", &json!({"keyword": "Places|France|Poitiers", "ids": sel})).unwrap();
    assert_eq!((r["added"].as_u64(), r["alreadyHad"].as_u64()), (Some(1), Some(2)), "{r}");
    let r = s.execute("keyword.apply", &json!({"keyword": "places|france|poitiers", "ids": [ids[0].0]})).unwrap();
    assert_eq!(r["added"], 0, "the case does not matter");
    assert_eq!(kw(&s, ids[0]).iter().filter(|k| k.eq_ignore_ascii_case("Places|France|Poitiers")).count(), 1);
    // the hierarchy is the path: the parents are not added
    assert!(!has(&s, ids[0], "Places") && !has(&s, ids[0], "Places|France"));
    // a person's keyword = the photo is filed under them
    let r = s.execute("keyword.apply", &json!({"keyword": "People|Awa", "ids": [ids[6].0]})).unwrap();
    assert_eq!(r["filedUnderPerson"], true, "{r}");
    s.execute("people.sync", &json!({})).unwrap();
    assert!(has(&s, ids[6], "People|Awa"));
    // an unknown People| name is just a keyword
    let r = s.execute("keyword.apply", &json!({"keyword": "People|Nobody", "ids": [ids[6].0]})).unwrap();
    assert!(r["filedUnderPerson"].is_null() && r["added"] == 1, "{r}");
    assert!(s.execute("keyword.apply", &json!({"keyword": "", "ids": [ids[6].0]})).is_err());
    // albums: a regular one takes photos once, a smart one refuses
    let a = s.execute("album.create", &json!({"name": "Dropped"})).unwrap()["id"].as_u64().unwrap();
    let sel: Vec<u64> = ids[..3].iter().map(|i| i.0).collect();
    assert_eq!(s.execute("album.addPhotos", &json!({"id": a, "ids": sel})).unwrap()["added"], 3);
    assert_eq!(s.execute("album.addPhotos", &json!({"id": a, "ids": sel})).unwrap()["added"], 0);
}

#[test]
fn the_new_decisions_are_saved_with_the_library_and_an_old_people_file_still_loads() {
    let dir = temp_dir("persist");
    let img = temp_dir("persist-img");
    for i in 0..4u8 {
        write_png(&img.join(format!("p{i}.png")), i * 20);
    }
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [img.to_string_lossy()]})).unwrap();
    let mut ids: Vec<PhotoId> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).map(PhotoId).collect();
    ids.sort_by_key(|i| i.0);
    inject(
        &mut s,
        vec![(ids[0], vec![(face(1, 0), R4)]), (ids[1], vec![(face(1, 1), R4)]), (ids[2], vec![(face(3, 2), R4)]), (ids[3], vec![(face(3, 3), R4)])],
    );
    let f0 = faces(&mut s, ids[0])[0].0.clone();
    let g = group_with(&mut s, &f0);
    s.execute("people.name", &json!({"group": g, "name": "Awa"})).unwrap();
    let f2 = faces(&mut s, ids[2])[0].0.clone();
    let g = group_with(&mut s, &f2);
    s.execute("people.unknown", &json!({"group": g})).unwrap();
    s.execute("people.settings", &json!({"tagUnknown": true})).unwrap();
    let awa = person(&mut s, "Awa");
    s.execute("people.tagPhotos", &json!({"person": awa, "ids": [ids[3].0]})).unwrap();
    let j = people_json(&s);
    assert_eq!(j["unknown"].as_array().unwrap().len(), 2, "{j}");
    assert_eq!(j["photo_tags"].as_array().unwrap().len(), 1, "{j}");
    assert_eq!(j["settings"]["tagUnknown"], true);
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((l["unknownTotal"].as_u64(), l["unknownFaces"].as_u64()), (Some(1), Some(2)), "{l}");
    assert!(has(&s, ids[3], "People|Awa") && has(&s, ids[2], "People|Unknown"));
    // keywords stay after a sync of the reopened library
    s.execute("people.sync", &json!({})).unwrap();
    assert!(has(&s, ids[3], "People|Awa"));
    // a file from before these decisions (no unknown, no photoTags, no tagUnknown) loads as it was
    let p = s.library.as_ref().unwrap().dir.join("people").join("people.json");
    drop(s);
    let mut old = people_json_from(&p);
    for k in ["unknown", "photo_tags"] {
        old.as_object_mut().unwrap().remove(k);
    }
    old["settings"].as_object_mut().unwrap().remove("tagUnknown");
    std::fs::write(&p, serde_json::to_vec(&old).unwrap()).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!((l["unknownTotal"].as_u64(), l["persons"][0]["name"].as_str()), (Some(0), Some("Awa")), "{l}");
    assert_eq!(s.execute("people.settings", &json!({})).unwrap()["settings"]["tagUnknown"], false);
}

fn people_json_from(p: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn a_sync_that_changes_nothing_leaves_no_step_to_undo() {
    let (mut s, ids) = session();
    named(&mut s, &ids);
    // a decision to come back to: Cleo's only face is "unknown person", so the photo loses her keyword
    let f = faces(&mut s, ids[6])[0].0.clone();
    s.execute("people.unknown", &json!({"face": f})).unwrap();
    assert!(!has(&s, ids[6], "People|Cleo"));
    let steps = s.undo.len();
    let sy = s.execute("people.sync", &json!({})).unwrap();
    assert_eq!((sy["added"].as_u64(), sy["removed"].as_u64()), (Some(0), Some(0)), "{sy}");
    assert_eq!(s.undo.len(), steps, "no catalog step either");
    // `people.undo` takes back the decision before the sync, not an empty "Tag people"
    let r = s.execute("people.undo", &json!({})).unwrap();
    assert_ne!(r["undone"], "Tag people", "{r}");
    assert!(has(&s, ids[6], "People|Cleo"), "{r}");
    // a sync that does change something is still one undoable step
    let awa = person(&mut s, "Awa");
    s.execute("people.tagPhotos", &json!({"person": awa, "ids": [ids[6].0]})).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [ids[0].0], "removeKeywords": ["People|Awa"]})).unwrap();
    let sy = s.execute("people.sync", &json!({})).unwrap();
    assert_eq!(sy["added"], 1, "{sy}");
    let r = s.execute("people.undo", &json!({})).unwrap();
    assert_eq!(r["undone"], "Tag people", "{r}");
    assert!(!has(&s, ids[0], "People|Awa"));
}
