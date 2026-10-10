//! Keyword suggestions without the model: the vocabulary commands, the learned probes on
//! synthetic embeddings, the accept / settings contract. (The zero-shot part needs SigLIP's text
//! tower and is measured on real photos, see docs/taste-and-keywords.md.)

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-kw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &std::path::Path, seed: usize) {
    let (w, h) = (16usize, 12usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [((i % w) * 9 + seed) as u8, ((i / w) * 7 + seed * 5) as u8, (seed * 3) as u8, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn lcg(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
}

#[test]
fn vocabulary_can_be_extended_but_not_into_the_people_or_places_spaces() {
    let mut s = Session::new().with_fs();
    let r = s.execute("keywords.vocabulary", &json!({})).unwrap();
    assert!(r["total"].as_u64().unwrap() >= 100, "{r}");
    assert_eq!(r["language"], "en");
    let e = s.execute("keywords.vocabulary", &json!({"add": [{"keyword": "People|Someone", "prompts": ["a person"]}]})).unwrap_err().to_string();
    assert!(e.contains("people / places"), "{e}");
    // no library on disk: an extension is accepted in the call but there is nowhere to keep it
    let e = s.execute("keywords.accept", &json!({"items": [{"id": 1, "keyword": "Places|Poitiers"}]})).unwrap_err().to_string();
    assert!(e.contains("People / Places"), "{e}");
    assert!(s.execute("keywords.accept", &json!({})).is_err());
}

#[test]
fn settings_default_to_no_automatic_keywords() {
    let mut s = Session::new().with_fs();
    let r = s.execute("keywords.settings", &json!({})).unwrap();
    assert_eq!(r["autoAccept"], false);
    assert!(r["autoThreshold"].as_f64().unwrap() >= 0.8);
    // in-memory library: nowhere to keep a change, and it says so
    assert!(s.execute("keywords.settings", &json!({"autoAccept": true})).is_err());
}

#[test]
fn accepting_keywords_adds_them_in_one_undoable_step_per_keyword() {
    let dir = temp_dir("accept");
    for i in 0..3 {
        write_png(&dir.join(format!("p{i}.png")), i);
    }
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let ids: Vec<u64> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    let a = s.execute("keywords.accept", &json!({"ids": ids, "keywords": ["Food|Dessert"]})).unwrap();
    assert_eq!(a["added"][0], "Food|Dessert");
    assert!(ids.iter().all(|i| s.catalog.photo(crate::PhotoId(*i)).unwrap().meta.keywords.iter().any(|k| k == "Food|Dessert")));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(ids.iter().all(|i| s.catalog.photo(crate::PhotoId(*i)).unwrap().meta.keywords.is_empty()));
}

#[test]
fn a_learned_probe_finds_the_keyword_it_was_taught_and_holds_out_folders() {
    let root = temp_dir("learn");
    let mut s = Session::new().with_fs();
    let mut rng = 9u64;
    let dirk: Vec<f32> = (0..768).map(|_| if lcg(&mut rng) > 0.0 { 1.0 } else { -1.0 }).collect();
    let mut entries = Vec::new();
    let mut tagged = Vec::new();
    let mut n = 0;
    for f in 0..8 {
        let dir = root.join(format!("shoot{f}"));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..20 {
            write_png(&dir.join(format!("p{i}.png")), n);
            n += 1;
        }
        let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
        for (k, id) in r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).enumerate() {
            let has = k % 3 == 0;
            let key = lightcraft_search::key_of(&crate::media::content_key(s.catalog.photo(crate::PhotoId(id)).unwrap()));
            let mut v: Vec<f32> = (0..768).map(|j| 0.05 * lcg(&mut rng) + if has { 0.03 * dirk[j] } else { 0.0 }).collect();
            let nrm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter_mut().for_each(|x| *x /= nrm);
            entries.push((key, v));
            if has {
                tagged.push(id);
            }
        }
    }
    s.searcher.with_store(None, |st| st.vectors.add(&entries).unwrap()).unwrap();
    s.execute("keywords.accept", &json!({"ids": tagged, "keywords": ["Cousins"]})).unwrap();
    let r = s.execute("keywords.learn", &json!({"minPhotos": 20, "precision": 0.8})).unwrap();
    let row = r["keywords"].as_array().unwrap().iter().find(|k| k["keyword"] == "Cousins").unwrap();
    assert_eq!(row["kept"], true, "{r}");
    assert!(row["precision"].as_f64().unwrap() >= 0.8 && row["recall"].as_f64().unwrap() > 0.5, "{r}");
}
