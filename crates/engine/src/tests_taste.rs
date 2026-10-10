//! The taste model and the culling suggestions, on synthetic embeddings (no model needed): the
//! library is a few folders of tiny PNGs, the vectors are written straight into the in-memory
//! index, the stars follow a direction in the vector space.

use std::path::Path;

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-taste-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: usize) {
    let (w, h) = (32usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [((i % w) * 7 + seed) as u8, ((i / w) * 9 + seed * 3) as u8, (seed * 11) as u8, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn lcg(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
}

/// 6 folders of 16 photos. `quality` of photo i drives a direction of its vector; photos with
/// quality above the folder's cut are starred (1..3 stars) in the first 4 folders only; the last 2
/// folders have no stars at all (to be suggested from).
struct Lib {
    s: Session,
    /// (id, quality, folder)
    photos: Vec<(u64, f32, usize)>,
}

fn library(tag: &str) -> Lib {
    let root = temp_dir(tag);
    let mut s = Session::new().with_fs();
    let mut rng = 5u64;
    let mut photos = Vec::new();
    let mut n = 0;
    for f in 0..6 {
        let dir = root.join(format!("shoot{f}"));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..16 {
            write_png(&dir.join(format!("p{i}.png")), n);
            n += 1;
        }
        let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
        let ids: Vec<u64> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
        assert_eq!(ids.len(), 16);
        for id in ids {
            photos.push((id, lcg(&mut rng) + 0.5, f));
        }
    }
    // vectors: the quality moves every dimension a little along a fixed direction, on top of
    // noise; 768 wide like SigLIP's
    let dir: Vec<f32> = (0..768).map(|_| if lcg(&mut rng) > 0.0 { 1.0 } else { -1.0 }).collect();
    let mut entries = Vec::new();
    for (id, q, _) in &photos {
        let key = lightcraft_search::key_of(&crate::media::content_key(s.catalog.photo(crate::PhotoId(*id)).unwrap()));
        let mut v: Vec<f32> = (0..768).map(|i| 0.05 * lcg(&mut rng) + 0.04 * q * dir[i]).collect();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        entries.push((key, v));
    }
    s.searcher.with_store(None, |st| st.vectors.add(&entries).unwrap()).unwrap();
    for (id, q, f) in &photos {
        if *f < 4 && *q > 0.55 {
            let stars = if *q > 0.85 {
                3
            } else if *q > 0.7 {
                2
            } else {
                1
            };
            s.execute("photo.rate", &json!({"rating": stars, "ids": [id]})).unwrap();
        }
    }
    Lib { s, photos }
}

#[test]
fn training_reports_honest_numbers_and_suggestions_write_nothing() {
    let Lib { mut s, photos } = library("train");
    let before: Vec<u8> = photos.iter().map(|(id, _, _)| s.catalog.photo(crate::PhotoId(*id)).unwrap().rating).collect();
    let r = s.execute("taste.train", &json!({"extras": false, "lambdas": [1.0, 10.0, 100.0], "minRated": 10})).unwrap();
    assert!(r["counts"]["rated"].as_u64().unwrap() >= 10, "{r}");
    assert!(r["counts"]["implicit"].as_u64().unwrap() > 0, "unrated photos of the starred shoots count as not chosen: {r}");
    assert_eq!(r["metrics"]["split"], "shoot");
    assert!(r["metrics"]["aucChosen"].as_f64().unwrap() > 0.8, "{r}");
    assert!(r["metrics"]["topKHitRate"].as_f64().unwrap() > r["metrics"]["topKChance"].as_f64().unwrap(), "{r}");
    assert_eq!(s.execute("taste.status", &json!({})).unwrap()["trained"], true);

    // suggestions among the unrated photos of the two shoots that have no stars
    let ids: Vec<u64> = photos.iter().filter(|p| p.2 >= 4).map(|p| p.0).collect();
    let g = s.execute("culling.suggest", &json!({"ids": ids, "fraction": 0.25})).unwrap();
    let picks: Vec<u64> = g["picks"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    assert_eq!(picks.len(), 8, "{g}");
    let q = |id: u64| photos.iter().find(|p| p.0 == id).unwrap().1;
    let mean_pick = picks.iter().map(|i| q(*i)).sum::<f32>() / picks.len() as f32;
    let mean_all = ids.iter().map(|i| q(*i)).sum::<f32>() / ids.len() as f32;
    assert!(mean_pick > mean_all + 0.2, "picks {mean_pick} vs all {mean_all}");
    // nothing was written
    let after: Vec<u8> = photos.iter().map(|(id, _, _)| s.catalog.photo(crate::PhotoId(*id)).unwrap().rating).collect();
    assert_eq!(before, after);
    assert!(s.catalog.photos().all(|p| p.flag != lightcraft_catalog::Flag::Pick));
    // the view can be filtered to the picks
    let f = s.execute("culling.suggest", &json!({"ids": ids, "count": 3, "filter": true})).unwrap();
    assert_eq!(s.filter.only.len(), 3, "{f}");

    // accepting needs an explicit confirmation
    assert!(s.execute("culling.accept", &json!({"ids": picks})).is_err());
    let a = s.execute("culling.accept", &json!({"ids": picks, "mode": "pick", "confirm": true})).unwrap();
    assert_eq!(a["changed"], 8);
    assert_eq!(s.catalog.photos().filter(|p| p.flag == lightcraft_catalog::Flag::Pick).count(), 8);
    // undo puts it back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photos().all(|p| p.flag != lightcraft_catalog::Flag::Pick));
}

#[test]
fn without_enough_stars_it_says_so_and_without_a_model_it_refuses() {
    let mut s = Session::new().with_fs();
    let e = s.execute("culling.suggest", &json!({})).unwrap_err().to_string();
    assert!(e.contains("taste.train"), "{e}");
    let e = s.execute("taste.train", &json!({})).unwrap_err().to_string();
    assert!(e.contains("not enough"), "{e}");
}

#[test]
fn technical_features_are_cached_and_used() {
    let Lib { mut s, .. } = library("extras");
    let r = s.execute("taste.train", &json!({"extras": true, "lambdas": [10.0], "folds": 3, "minRated": 10})).unwrap();
    assert!(r["metrics"]["spearmanSharpnessOnly"].is_number(), "{r}");
}
