//! Duplicates and search by meaning. The model is never needed here (and never downloaded): exact
//! duplicates read content hashes, near-duplicates read perceptual hashes, and everything that
//! needs SigLIP says clearly that it isn't installed. The end-to-end run on the real model is
//! `real_model_end_to_end`, ignored unless `LIGHTCRAFT_SEARCH_DIR` names the model folder.

use std::path::Path;

use serde_json::{Value, json};

use crate::Session;
use crate::search::{LICENSE_NAME, MODEL_URL, NOT_INSTALLED};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-search-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A PNG of `w × h` pixels from `f(x, y)`.
fn write_png(path: &Path, w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) {
    let data: Vec<[u8; 4]> = (0..w * h)
        .map(|i| {
            let c = f(i % w, i / w);
            [c[0], c[1], c[2], 255]
        })
        .collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn import(s: &mut Session, dir: &Path) -> Vec<u64> {
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).collect()
}

fn groups(r: &Value) -> Vec<Vec<u64>> {
    r["groups"].as_array().unwrap().iter().map(|g| g["ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect()).collect()
}

#[test]
fn exact_duplicates_are_files_with_the_same_content() {
    let dir = temp_dir("exact");
    write_png(&dir.join("a.png"), 64, 48, |x, y| [(x * 4) as u8, (y * 5) as u8, 20]);
    write_png(&dir.join("b.png"), 64, 48, |x, y| [(y * 4) as u8, (x * 5) as u8, 90]);
    let mut s = Session::new().with_fs();
    let ids = import(&mut s, &dir);
    assert_eq!(ids.len(), 2);
    assert!(groups(&s.execute("library.findDuplicates", &json!({})).unwrap()).is_empty(), "two different photos are not duplicates");
    // Photo ▸ Duplicate writes a real copy of the file: same content, another file
    s.execute("photo.select", &json!({"id": ids[0]})).ok();
    let dup = s.execute("photo.duplicate", &json!({"ids": [ids[0]]})).unwrap()["ids"][0].as_u64().unwrap();
    let r = s.execute("library.findDuplicates", &json!({"mode": "exact"})).unwrap();
    let g = groups(&r);
    assert_eq!(g.len(), 1, "{r}");
    assert_eq!(g[0], vec![ids[0].min(dup), ids[0].max(dup)]);
    assert_eq!(r["photos"], 2);
    // restricted to other photos: nothing
    assert!(groups(&s.execute("library.findDuplicates", &json!({"ids": [ids[1]]})).unwrap()).is_empty());
    // grouping them into a stack
    let r = s.execute("library.findDuplicates", &json!({"stack": true})).unwrap();
    assert_eq!(r["stacked"], 1, "{r}");
    assert!(s.catalog.stack_of(lightcraft_catalog::PhotoId(dup)).is_some_and(|st| st.photos.len() == 2));
    let bad = s.execute("library.findDuplicates", &json!({"mode": "fuzzy"})).unwrap_err().to_string();
    assert!(bad.contains("exact"), "{bad}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_model_is_never_required_and_its_licence_is_shown_before_any_download() {
    let mut s = Session::new();
    s.searcher.dir = Some(temp_dir("nomodel").join("none"));
    let st = s.execute("search.model.status", &json!({})).unwrap();
    assert_eq!(st["installed"], false);
    assert_eq!(st["license"], LICENSE_NAME);
    assert!(st["licenseUrl"].as_str().unwrap().starts_with("https://"));
    assert!(st["sizeBytes"].as_u64().unwrap() > 1_000_000_000);
    // no consent, no download
    let e = s.execute("search.model.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains(LICENSE_NAME) && e.contains(MODEL_URL) && e.contains("acknowledged"), "{e}");
    assert!(!s.searcher.download_status().running);
    for (c, p) in [("search.index", json!({})), ("search.semantic", json!({"query": "a cat"})), ("search.tag", json!({"labels": ["cat"]}))] {
        let e = s.execute(c, &p).unwrap_err().to_string();
        assert!(e.contains(NOT_INSTALLED) || e.contains("not available in this build"), "{c}: {e}");
    }
    let st = s.execute("search.status", &json!({})).unwrap();
    assert_eq!(st["installed"], false);
}

#[cfg(feature = "search")]
mod with_search {
    use super::*;

    /// Photos of one scene (a gradient with a bright disc), lightly edited / re-exported, and a
    /// different scene.
    fn scene(dir: &Path) {
        let disc = |x: usize, y: usize, bx: f32| {
            let (dx, dy) = (x as f32 - bx, y as f32 - 24.0);
            if dx * dx + dy * dy < 120.0 { 255.0 } else { 0.0 }
        };
        write_png(&dir.join("burst1.png"), 96, 64, |x, y| {
            let v = (x as f32 * 2.0 + y as f32).min(255.0) * 0.6 + disc(x, y, 30.0) * 0.4;
            [v as u8, (v * 0.8) as u8, (v * 0.5) as u8]
        });
        // the next frame of the burst: a touch brighter, the disc moved by a pixel
        write_png(&dir.join("burst2.png"), 96, 64, |x, y| {
            let v = ((x as f32 * 2.0 + y as f32).min(255.0) * 0.6 + disc(x, y, 31.0) * 0.4 + 6.0).min(255.0);
            [v as u8, (v * 0.8) as u8, (v * 0.5) as u8]
        });
        // another size of the same frame
        write_png(&dir.join("burst3-small.png"), 48, 32, |x, y| {
            let (x, y) = (x * 2, y * 2);
            let v = (x as f32 * 2.0 + y as f32).min(255.0) * 0.6 + disc(x, y, 30.0) * 0.4;
            [v as u8, (v * 0.8) as u8, (v * 0.5) as u8]
        });
        write_png(&dir.join("other.png"), 96, 64, |x, y| {
            let v = if (x / 12 + y / 12) % 2 == 0 { 230 } else { 25 };
            [v, v, (255 - v as u16) as u8]
        });
    }

    #[test]
    fn near_duplicates_by_perceptual_hash_without_the_model() {
        let dir = temp_dir("near");
        scene(&dir);
        let mut s = Session::new().with_fs();
        s.searcher.dir = Some(dir.join("no-model-here"));
        let ids = import(&mut s, &dir);
        assert_eq!(ids.len(), 4);
        let by_name = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).unwrap().id.0;
        let r = s.execute("library.findDuplicates", &json!({"mode": "near"})).unwrap();
        let g = groups(&r);
        assert_eq!(g.len(), 1, "{r}");
        let mut want = vec![by_name(&s, "burst1.png"), by_name(&s, "burst2.png"), by_name(&s, "burst3-small.png")];
        want.sort_unstable();
        let mut got = g[0].clone();
        got.sort_unstable();
        assert_eq!(got, want, "{r}");
        // no embeddings (no model): the hash alone decided, and says so
        assert_eq!(r["groups"][0]["confirmed"], false);
        // a looser limit joins nothing else, a stricter one splits nothing that is identical
        let loose = s.execute("library.findDuplicates", &json!({"mode": "near", "maxDistance": 24})).unwrap();
        assert_eq!(groups(&loose).len(), 1, "{loose}");
        let none = s.execute("library.findDuplicates", &json!({"mode": "near", "maxDistance": 0})).unwrap();
        assert!(groups(&none).len() <= 1);
        // the view can be filtered to them
        s.execute("library.findDuplicates", &json!({"mode": "near", "filter": true})).unwrap();
        assert_eq!(s.filter.only.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hashes_survive_in_the_library_and_are_not_computed_twice() {
        let dir = temp_dir("persist");
        let lib = temp_dir("persist-lib");
        scene(&dir);
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        import(&mut s, &dir);
        s.execute("library.findDuplicates", &json!({"mode": "near"})).unwrap();
        let st = s.execute("search.status", &json!({})).unwrap();
        assert_eq!(st["index"]["hashed"], 4, "{st}");
        assert!(lib.join("search").join("hashes.bin").is_file());
        s.close_library().unwrap();
        drop(s);
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        let st = s.execute("search.status", &json!({})).unwrap();
        assert_eq!(st["index"]["hashed"], 4, "hashes are read back: {st}");
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&lib));
    }

    /// Downloads the real model from Hugging Face (1.5 GB, needs the network):
    /// `cargo test --release -p lightcraft-engine --features search real_download -- --ignored --nocapture`.
    #[test]
    #[ignore = "downloads 1.5 GB"]
    fn real_download() {
        let dir = temp_dir("download").join("model");
        let mut s = Session::new();
        s.searcher.dir = Some(dir.clone());
        let r = s.execute("search.model.download", &json!({"acknowledged": true})).unwrap();
        assert_eq!(r["started"], true, "{r}");
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let d = s.searcher.download_status();
            if !d.running {
                assert_eq!(d.error, None);
                break;
            }
        }
        assert!(s.searcher.installed());
        assert_eq!(std::fs::metadata(dir.join("model.safetensors")).unwrap().len(), 1_500_800_904);
        println!("downloaded to {}", dir.display());
    }

    /// Needs the real model: `LIGHTCRAFT_SEARCH_DIR=<folder> cargo test --release -p lightcraft-engine --features search real_model -- --ignored`.
    #[test]
    #[ignore = "needs the SigLIP files (LIGHTCRAFT_SEARCH_DIR)"]
    fn real_model_end_to_end() {
        let dir = temp_dir("real");
        scene(&dir);
        let lib = temp_dir("real-lib");
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        import(&mut s, &dir);
        let r = s.execute("search.index", &json!({})).unwrap();
        assert_eq!(r["queued"], 4, "{r}");
        assert_eq!(r["failed"], 0, "{r}");
        let again = s.execute("search.index", &json!({})).unwrap();
        assert_eq!(again["queued"], 0, "resumes: nothing left to do: {again}");
        let r = s.execute("search.semantic", &json!({"query": "a checkerboard pattern", "limit": 4})).unwrap();
        assert_eq!(r["photos"].as_array().unwrap().len(), 4);
        let near = s.execute("library.findDuplicates", &json!({"mode": "near"})).unwrap();
        assert_eq!(near["groups"][0]["confirmed"], true, "{near}");
        let tags = s.execute("search.tag", &json!({"labels": ["a checkerboard"], "threshold": 0.0, "dryRun": true})).unwrap();
        assert_eq!(tags["labels"][0]["matched"], 4, "{tags}");
    }
}
