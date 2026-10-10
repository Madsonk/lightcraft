//! People: grouping, naming, confirming, undo and keywords, with faces injected into the index
//! (no model needed). The run on the real models is `real_models_end_to_end`, ignored unless
//! `LIGHTCRAFT_PEOPLE_DIR` and `LIGHTCRAFT_FACES_DIR` name the model folders.

use std::path::Path;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::people::ops::inject;

const DIM: usize = lightcraft_people::DIM;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    // (tests run in parallel: every call gets its own folder)
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("lc-people-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
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

/// A face of identity `who`, a little different each time.
fn face(who: u64, n: u64) -> Vec<f32> {
    let (a, b) = (rnd(100 + who), rnd(9000 + who * 31 + n));
    unit(a.iter().zip(&b).map(|(x, y)| x + 0.5 * y).collect())
}

/// Six photos: 0-2 show person A, 3-5 person B, photo 2 also shows B.
fn session() -> (Session, Vec<PhotoId>) {
    let dir = temp_dir("lib");
    for i in 0..6u8 {
        write_png(&dir.join(format!("p{i}.png")), i * 20);
    }
    let mut s = Session::new().with_fs();
    s.people.dir = Some(temp_dir("models"));
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let ids: Vec<PhotoId> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).map(PhotoId).collect();
    assert_eq!(ids.len(), 6);
    let r4 = [0.1, 0.1, 0.4, 0.5];
    let mut photos = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let mut faces = vec![(face(if i < 3 { 1 } else { 2 }, i as u64), r4)];
        if i == 2 {
            faces.push((face(2, 77), [0.6, 0.1, 0.9, 0.5]));
        }
        photos.push((*id, faces));
    }
    inject(&mut s, photos);
    (s, ids)
}

fn kw(s: &Session, id: PhotoId) -> Vec<String> {
    s.catalog.photo(id).unwrap().meta.keywords.clone()
}

#[test]
fn faces_are_grouped_then_named_and_the_photos_get_the_keyword() {
    let (mut s, ids) = session();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!(l["indexedFaces"], 7, "{l}");
    let groups = l["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{l}");
    assert_eq!(groups[0]["faces"], 4);
    assert_eq!(groups[1]["faces"], 3);
    // nothing is tagged by grouping
    assert!(ids.iter().all(|i| kw(&s, *i).is_empty()));
    // a dry run says what would happen, and changes nothing
    let g = groups[1]["id"].as_str().unwrap().to_string();
    let d = s.execute("people.name", &json!({"group": g, "name": "Awa", "dryRun": true})).unwrap();
    assert_eq!((d["faces"].as_u64(), d["photos"].as_u64()), (Some(3), Some(3)), "{d}");
    assert!(ids.iter().all(|i| kw(&s, *i).is_empty()));
    let r = s.execute("people.name", &json!({"group": g, "name": "Awa"})).unwrap();
    assert_eq!(r["keyword"], "People|Awa");
    for i in 0..3 {
        assert_eq!(kw(&s, ids[i]), vec!["People|Awa".to_string()], "photo {i}");
    }
    assert!(kw(&s, ids[4]).is_empty());
    // undo takes the keywords and the person back
    let u = s.execute("people.undo", &json!({})).unwrap();
    assert_eq!(u["keywordsUndone"], true);
    assert!(ids.iter().all(|i| kw(&s, *i).is_empty()));
    assert!(s.execute("people.list", &json!({})).unwrap()["persons"].as_array().unwrap().is_empty());
    assert!(s.execute("people.undo", &json!({})).is_err());
}

#[test]
fn a_new_photo_is_suggested_never_tagged_until_confirmed() {
    let (mut s, ids) = session();
    // name person A from the faces of photos 0 and 1 only
    let l = s.execute("people.list", &json!({})).unwrap();
    let a = l["groups"][1]["id"].as_str().unwrap().to_string();
    s.execute("people.name", &json!({"group": a, "name": "Bao"})).unwrap();
    // then a sixth photo of Bao arrives
    let dir = temp_dir("more");
    write_png(&dir.join("new.png"), 200);
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let new = PhotoId(r["imported"][0].as_u64().unwrap());
    crate::people::ops::inject(&mut s, vec![(new, vec![(face(1, 500), [0.2, 0.2, 0.5, 0.6])])]);
    s.execute("people.refresh", &json!({})).unwrap();
    let sg = s.execute("people.suggest", &json!({})).unwrap();
    let list = sg["suggestions"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{sg}");
    assert_eq!(list[0]["name"], "Bao");
    assert!(kw(&s, new).is_empty(), "suggested, not tagged");
    // "no": never suggested for Bao again
    let ask = list[0]["ask"].as_str().unwrap().to_string();
    let (face_ref, person) = ask.split_once('>').unwrap();
    s.execute("people.confirm", &json!({"face": face_ref, "person": person, "answer": "no"})).unwrap();
    s.execute("people.refresh", &json!({})).unwrap();
    assert!(s.execute("people.suggest", &json!({})).unwrap()["suggestions"].as_array().unwrap().is_empty());
    assert!(kw(&s, new).is_empty());
    // the face is now in a group of its own; saying yes there joins it to Bao
    let l = s.execute("people.list", &json!({})).unwrap();
    assert!(l["groups"].as_array().unwrap().iter().any(|g| g["faces"] == 1), "{l}");
    s.execute("people.confirm", &json!({"face": face_ref, "person": person, "answer": "yes"})).unwrap();
    assert_eq!(kw(&s, new), vec!["People|Bao".to_string()]);
    assert_eq!(kw(&s, ids[0]), vec!["People|Bao".to_string()]);
}

#[test]
fn merging_renaming_and_deleting_keep_the_keywords_in_line() {
    let (mut s, ids) = session();
    let l = s.execute("people.list", &json!({})).unwrap();
    let (g0, g1) = (l["groups"][0]["id"].as_str().unwrap().to_string(), l["groups"][1]["id"].as_str().unwrap().to_string());
    let p0 = s.execute("people.name", &json!({"group": g0, "name": "Dia"})).unwrap()["id"].as_str().unwrap().to_string();
    let p1 = s.execute("people.name", &json!({"group": g1, "name": "Awa"})).unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(kw(&s, ids[4]), vec!["People|Dia".to_string()]);
    // rename: the keyword follows on every photo
    s.execute("people.rename", &json!({"person": p0, "name": "Dia K"})).unwrap();
    assert_eq!(kw(&s, ids[4]), vec!["People|Dia K".to_string()]);
    // photo 2 shows both people
    let mut both = kw(&s, ids[2]);
    both.sort();
    assert_eq!(both, vec!["People|Awa".to_string(), "People|Dia K".to_string()]);
    // merge Awa into Dia K: one keyword, one person
    s.execute("people.merge", &json!({"from": p1, "into": p0})).unwrap();
    assert_eq!(kw(&s, ids[0]), vec!["People|Dia K".to_string()]);
    assert_eq!(s.execute("people.list", &json!({})).unwrap()["persons"].as_array().unwrap().len(), 1);
    // delete: faces are free, keywords gone
    s.execute("people.delete", &json!({"person": p0})).unwrap();
    assert!(ids.iter().all(|i| kw(&s, *i).is_empty()));
    // an unknown reference is an error that says what to give
    let e = s.execute("people.name", &json!({"group": "x", "name": "N"})).unwrap_err().to_string();
    assert!(e.contains("g:N"), "{e}");
}

#[test]
fn questions_cover_the_doubtful_and_unsure_is_not_asked_again() {
    let (mut s, _) = session();
    s.execute("people.settings", &json!({"link": 0.9})).unwrap();
    let q = s.execute("people.questions", &json!({"limit": 5})).unwrap();
    let list = q["questions"].as_array().unwrap();
    // with a very strict link the two A/B-alike groups split: pairs between groups may be asked
    for x in list {
        assert!(x["id"].as_str().unwrap().contains('>'));
    }
    if let Some(first) = list.first() {
        let id = first["id"].as_str().unwrap().to_string();
        s.execute("people.confirm", &json!({"question": id, "answer": "unsure"})).unwrap();
        let again = s.execute("people.questions", &json!({"limit": 50})).unwrap();
        assert!(again["questions"].as_array().unwrap().iter().all(|x| x["id"] != json!(id)));
    }
    // out-of-range settings are clamped, an unknown model refused
    assert!(s.execute("people.settings", &json!({"model": "huge"})).is_err());
    let st = s.execute("people.settings", &json!({"autoTag": 0.7, "link": 5.0})).unwrap();
    assert!(st["settings"]["link"].as_f64().unwrap() <= 0.9 + 1e-6);
    assert!((st["settings"]["autoTag"].as_f64().unwrap() - 0.7).abs() < 1e-6);
}

#[test]
fn without_the_models_indexing_says_what_to_do_and_download_needs_consent() {
    let mut s = Session::new();
    s.people.dir = Some(temp_dir("nomodels").join("none"));
    let e = s.execute("people.index", &json!({})).unwrap_err().to_string();
    assert!(e.contains(crate::people::NOT_INSTALLED) && e.contains("people.model.download"), "{e}");
    let st = s.execute("people.status", &json!({})).unwrap();
    assert_eq!(st["model"]["installed"], false);
    assert!(st["model"]["licence"].as_str().unwrap().contains("non-commercial"));
    let e = s.execute("people.model.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains("acknowledged"), "{e}");
}

#[test]
fn the_decisions_survive_reopening_the_library_folder() {
    let dir = temp_dir("persist");
    let img = temp_dir("persist-img");
    for i in 0..3u8 {
        write_png(&img.join(format!("p{i}.png")), i * 20);
    }
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    assert!(s.library.as_ref().unwrap().on_disk);
    let r = s.execute("library.import", &json!({"paths": [img.to_string_lossy()]})).unwrap();
    let ids: Vec<PhotoId> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).map(PhotoId).collect();
    assert_eq!(ids.len(), 3);
    inject(&mut s, ids.iter().enumerate().map(|(i, id)| (*id, vec![(face(1, i as u64), [0.1, 0.1, 0.5, 0.5])])).collect());
    let l = s.execute("people.list", &json!({})).unwrap();
    let g = l["groups"][0]["id"].as_str().unwrap().to_string();
    s.execute("people.name", &json!({"group": g, "name": "Awa"})).unwrap();
    let folder = s.library.as_ref().unwrap().dir.join("people");
    assert!(folder.join("people.json").is_file() && std::fs::read_to_string(folder.join("people.json")).unwrap().contains("Awa"));
    assert!(std::fs::read_dir(&folder).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("faces-")));
    drop(s);
    // a new session on the same library: the people and the faces are there, nothing is looked at again
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let l = s.execute("people.list", &json!({})).unwrap();
    assert_eq!(l["indexedFaces"], 3, "{l}");
    assert_eq!(l["persons"][0]["name"], "Awa", "{l}");
    assert_eq!(l["persons"][0]["faces"], 3);
    for id in &ids {
        assert!(s.catalog.photo(*id).unwrap().meta.keywords.contains(&"People|Awa".to_string()));
    }
}
