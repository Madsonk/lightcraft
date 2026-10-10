//! People ▸ Recognition, headless: the view lists the groups of faces found, naming one tags its
//! photos, and the doubtful cases are asked as questions. Faces are injected into the index (no
//! model needed); the real models are exercised by the engine's and the CLI's runs.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);
const DIM: usize = 512;

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

#[test]
fn the_recognition_view_lists_groups_and_naming_one_tags_its_photos() {
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let services = Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    };
    let mut s = lightcraft_engine::Session::with_demo();
    let ids: Vec<_> = s.catalog.photos().map(|p| p.id).take(8).collect();
    assert!(ids.len() >= 8, "the demo library has photos");
    // person A on the first four photos, person B on the next four
    let photos = ids.iter().enumerate().map(|(i, id)| (*id, vec![(face(if i < 4 { 1 } else { 2 }, i as u64), [0.3, 0.2, 0.6, 0.6])])).collect();
    lightcraft_engine::people::inject_faces(&mut s, photos);
    let app = LightcraftApp::new(s, services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.app.ui.view = ViewMode::People;
    h.app.caches.people_rec.recognition = true;
    h.settle(SETTLE);
    h.step();
    let r = h.request("ui.set", json!({"view": "people"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    // the faces are grouped in the background (a frame starts it), then the view lists the groups
    let t0 = std::time::Instant::now();
    while !h.app.caches.people_rec.overview().is_some_and(|o| o.grouped) && t0.elapsed() < SETTLE {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    let ov = h.app.caches.people_rec.overview().cloned().expect("the view built its overview");
    assert_eq!((ov.indexed_faces, ov.groups.len()), (8, 2), "{ov:?}");
    assert!(ov.persons.is_empty());
    let shot = std::env::var("LIGHTCRAFT_TEST_SHOTS").ok();
    if let Some(dir) = &shot {
        let r =
            h.request("ui.screenshot", json!({"path": std::path::Path::new(dir).join("people_groups.png").to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    // naming a group (what the Name button does) tags its photos and moves it to the people
    let group = ov.groups[0].id.clone();
    let r = h.app.run("people.name", json!({"group": group, "name": "Awa"})).unwrap();
    assert_eq!(r["keyword"], "People|Awa");
    h.settle(SETTLE);
    h.step();
    let ov = h.app.caches.people_rec.overview().cloned().unwrap();
    assert_eq!((ov.persons.len(), ov.persons[0].name.as_deref()), (1, Some("Awa")), "{ov:?}");
    assert_eq!(ov.persons[0].faces, 4);
    let tagged = h.app.session.catalog.photos().filter(|p| p.meta.keywords.iter().any(|k| k == "People|Awa")).count();
    assert_eq!(tagged, 4);
    if let Some(dir) = &shot {
        let r =
            h.request("ui.screenshot", json!({"path": std::path::Path::new(dir).join("people_named.png").to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    // the first tab is still the XMP one, and switching back and forth keeps working
    h.app.caches.people_rec.recognition = false;
    h.step();
    h.app.caches.people_rec.recognition = true;
    h.step();
    // undo: the keyword goes, the group is back
    h.app.run("people.undo", json!({})).unwrap();
    h.settle(SETTLE);
    h.step();
    assert_eq!(h.app.session.catalog.photos().filter(|p| p.meta.keywords.iter().any(|k| k == "People|Awa")).count(), 0);
}
