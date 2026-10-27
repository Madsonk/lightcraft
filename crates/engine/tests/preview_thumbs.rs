//! Grid thumbnails of unedited raws come from the camera's embedded preview; edited photos and files without a
//! usable preview render as before. Skipped when `corpus/raw` is absent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::import::{ImportMode, import};
use serde_json::json;

fn raws(n: usize) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/raw");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "arw" || e == "cr2")).collect();
    v.sort();
    v.truncate(n);
    v
}

#[test]
fn unedited_raw_thumbnails_use_the_camera_preview_and_edited_ones_do_not() {
    let files = raws(2);
    if files.len() < 2 {
        return;
    }
    let dir = std::env::temp_dir().join(format!("lc-preview-thumbs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for f in &files {
        std::fs::copy(f, dir.join(f.file_name().unwrap())).unwrap();
    }
    let mut s = lightcraft_engine::Session::new().with_fs();
    let rep = import(&mut s, &[dir.to_string_lossy().to_string()], ImportMode::Add).unwrap();
    assert_eq!(rep.imported.len(), 2, "{:?}", rep.failed);
    let (a, b) = (PhotoId(rep.imported[0]), PhotoId(rep.imported[1]));

    // unedited: the job asks the camera preview first, and its result fits the thumbnail size
    let job = s.thumb_job(a, 256).unwrap();
    assert!(job.embedded.is_some(), "an unedited raw takes its camera preview");
    let key_preview = job.cache.as_ref().unwrap().1;
    let r = job.run();
    let img = r.rendered.unwrap().image;
    assert!(img.width.max(img.height) <= 256 && img.width.min(img.height) > 0, "{}x{}", img.width, img.height);

    // the loupe never takes the camera preview: it is LightCraft's render
    assert!(s.loupe_job(a, 1600, 1600, true).unwrap().embedded.is_none());

    // a raw whose camera preview cannot be read renders from the raw as before, without an error
    let real = s.media.preview_loader.clone();
    s.media.preview_loader = Some(std::sync::Arc::new(|_: &str, _: usize| None));
    let job = s.thumb_job(a, 256).unwrap();
    assert!(job.embedded.is_some());
    let r = job.run();
    let img = r.rendered.expect("falls back to decoding the raw").image;
    assert!(img.width.max(img.height) <= 256 && img.width > 0);
    s.media.preview_loader = real;

    // the switch off renders from the raw, under another cache key
    s.media.preview_thumbs = false;
    let job = s.thumb_job(a, 256).unwrap();
    assert!(job.embedded.is_none());
    assert_ne!(job.cache.as_ref().unwrap().1, key_preview);
    s.media.preview_thumbs = true;

    // edited: the LightCraft render, never the camera's
    s.selection = lightcraft_engine::Selection::single(b);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.8, "ids": [b.0]})).unwrap();
    let job = s.thumb_job(b, 256).unwrap();
    assert!(job.embedded.is_none(), "an edited photo shows the LightCraft render");
    // and the other photo is still unedited
    assert!(s.thumb_job(a, 256).unwrap().embedded.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
