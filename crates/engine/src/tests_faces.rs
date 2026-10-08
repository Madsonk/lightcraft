//! Analyze Faces: the model-free parts (the filters, assisted culling's use of faces, what is
//! stored). The model itself is exercised by `tests_faces_model` when the weights are around.

use std::sync::Arc;

use lightcraft_catalog::{Analysis, FaceInfo, Flag, Op, PhotoId};
use serde_json::{Value, json};

use crate::Session;

/// An error about the model: not installed (a build with Analyze Faces) or not available (without).
fn about_the_model(e: &str) -> bool {
    e.contains(crate::faces::NOT_INSTALLED) || e.contains("can't analyze faces")
}

fn face(open: f32, sharp: f32) -> FaceInfo {
    FaceInfo { rect: [0.3, 0.2, 0.6, 0.6], score: 0.95, open: [open, open], sharp: [sharp, sharp], turn: 0.0 }
}

fn set_faces(s: &mut Session, id: u64, faces: Option<Vec<FaceInfo>>) {
    let prev = s.catalog.photo(PhotoId(id)).and_then(|p| p.analysis.clone()).unwrap_or(Analysis { sharpness: 50.0, ..Default::default() });
    let analysis = Analysis { faces: faces.map(Arc::new), ..prev };
    s.catalog.apply(Op::SetAnalysis { id: PhotoId(id), analysis: Some(analysis) }).unwrap();
}

fn ids_of(s: &mut Session, filter: Value) -> Vec<u64> {
    s.execute("library.clearFilter", &json!({})).unwrap();
    s.execute("library.filter", &filter).unwrap();
    let mut v: Vec<u64> = s.visible_cloned().iter().map(|i| i.0).collect();
    v.sort();
    v
}

#[test]
fn without_the_model_analyze_faces_says_how_to_get_it() {
    let mut s = Session::with_demo();
    s.faces.dir = Some(std::env::temp_dir().join("lightcraft-no-such-face-model"));
    let e = s.execute("photo.analyzeFaces", &json!({})).unwrap_err().to_string();
    assert!(about_the_model(&e), "{e}");
    // the download needs the user's yes
    let e = s.execute("faces.model.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains("acknowledged"), "{e}");
    let st = s.execute("faces.status", &json!({})).unwrap();
    assert_eq!(st["installed"], false);
    assert_eq!(st["license"], crate::faces::LICENSE_NAME);
}

#[test]
fn library_filters_find_closed_eyes_and_faces() {
    let mut s = Session::with_demo();
    let all: Vec<u64> = ids_of(&mut s, json!({}));
    let (a, b, c, d) = (all[0], all[1], all[2], all[3]);
    set_faces(&mut s, a, Some(vec![face(0.32, 80.0), face(0.30, 70.0)])); // two faces, eyes open
    set_faces(&mut s, b, Some(vec![face(0.31, 80.0), face(0.05, 70.0)])); // one blinking
    set_faces(&mut s, c, Some(vec![])); // analyzed, nobody there
    // d: never analyzed
    assert_eq!(ids_of(&mut s, json!({"faces": "any"})), vec![a, b]);
    assert_eq!(ids_of(&mut s, json!({"faces": "none"})), vec![c]);
    assert_eq!(ids_of(&mut s, json!({"faces": "closedEyes"})), vec![b]);
    assert_eq!(ids_of(&mut s, json!({"faces": "eyesOpen"})), vec![a]);
    assert!(ids_of(&mut s, json!({"faces": "unanalyzed"})).contains(&d));
    assert!(!ids_of(&mut s, json!({"faces": "unanalyzed"})).contains(&a));
    // the search token and the smart-album rules
    assert_eq!(ids_of(&mut s, json!({"text": "eyes:closed"})), vec![b]);
    let rule = |field: &str, op: &str, value: Value| json!({"ruleSet": {"rules": [{"field": field, "op": op, "value": value}]}});
    assert_eq!(ids_of(&mut s, rule("eyesClosed", "is", json!(true))), vec![b]);
    assert_eq!(ids_of(&mut s, rule("faceCount", "gte", json!(2))), vec![a, b]);
}

#[test]
fn a_turned_or_tiny_face_is_not_judged() {
    let mut turned = face(0.02, 80.0);
    turned.turn = 0.9;
    let mut tiny = face(0.02, 80.0);
    tiny.rect = [0.5, 0.5, 0.51, 0.52];
    assert!(!turned.eyes_closed() && !tiny.eyes_closed());
    assert!(face(0.02, 80.0).eyes_closed() && !face(0.28, 80.0).eyes_closed());
}

/// A scene as a PNG, optionally blurred a little.
fn write_scene(path: &std::path::Path, blur: usize) {
    let img = lightcraft_scenes::demo_library()[0].render(480, 320);
    let (w, h) = (img.width, img.height);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let (mut acc, mut n) = ([0f32; 3], 0.0);
            for yy in y.saturating_sub(blur)..(y + blur + 1).min(h) {
                for xx in x.saturating_sub(blur)..(x + blur + 1).min(w) {
                    let c = img.data[yy * w + xx];
                    acc = [acc[0] + c[0], acc[1] + c[1], acc[2] + c[2]];
                    n += 1.0;
                }
            }
            out.data[y * w + x] = acc.map(|v| v / n);
        }
    }
    let data: Vec<[u8; 4]> = out
        .data
        .iter()
        .map(|c| {
            let e = |v: f32| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
            [e(c[0]), e(c[1]), e(c[2]), 255]
        })
        .collect();
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }), &Default::default()).unwrap();
    std::fs::write(path, png).unwrap();
}

/// Assisted culling picks the burst's shot with open, sharp eyes over a slightly sharper frame
/// whose subject blinked, and rejects closed eyes on request.
#[test]
fn culling_chooses_open_eyes_over_a_slightly_sharper_frame() {
    let dir = std::env::temp_dir().join(format!("lc-faces-cull-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_scene(&dir.join("a_blink.png"), 0);
    write_scene(&dir.join("b_open.png"), 1);
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let ids: Vec<u64> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    let by_name = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).unwrap().id;
    let (blink, open) = (by_name(&s, "a_blink.png"), by_name(&s, "b_open.png"));
    for (i, p) in [blink, open].iter().enumerate() {
        s.commit("t", Op::SetCaptured { id: *p, captured: Some(format!("2026-05-01T10:00:0{i}")) }).unwrap();
    }
    set_faces(&mut s, blink.0, Some(vec![face(0.04, 90.0)]));
    set_faces(&mut s, open.0, Some(vec![face(0.31, 70.0)]));
    // without faces the sharper frame is the best of the burst
    let r = s.execute("photo.analyze", &json!({"ids": ids, "faces": false})).unwrap();
    assert_eq!(r["groups"], 1, "{r}");
    // faces are used when they are there: the open-eyed frame wins, and the blink is rejected
    let r = s.execute("photo.analyze", &json!({"ids": ids, "pickBest": true, "rejectClosedEyes": false})).unwrap();
    let flag = |s: &Session, id: PhotoId| s.catalog.photo(id).unwrap().flag;
    assert_eq!(flag(&s, open), Flag::Pick, "{r}");
    assert_eq!(flag(&s, blink), Flag::None);
    assert_eq!(r["closedEyes"], 1);
    // rejecting closed eyes: the blink goes (no model needed: its photos already have faces)
    s.faces.dir = Some(dir.join("no-model"));
    let r = s.execute("photo.analyze", &json!({"ids": ids, "rejectClosedEyes": true})).unwrap();
    assert_eq!(flag(&s, blink), Flag::Reject, "{r}");
    assert_eq!(r["rejected"], 1);
    // a photo that has no faces yet needs the model to be analyzed first, and it isn't there
    s.catalog.apply(Op::SetAnalysis { id: open, analysis: None }).unwrap();
    let e = s.execute("photo.analyze", &json!({"ids": ids, "rejectClosedEyes": true})).unwrap_err().to_string();
    assert!(about_the_model(&e), "{e}");
    // the faces survive the culling's own rewrite of the analysis
    assert!(s.catalog.photo(blink).unwrap().faces().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Photos analyzed before faces existed read as before, and an analysis with faces keeps the
/// older shape for readers that don't know the field.
#[test]
fn analysis_is_compatible_both_ways() {
    let old: Analysis = serde_json::from_str(r#"{"sharpness":42.0,"clipped":0.0,"group":null,"best":false}"#).unwrap();
    assert!(old.faces.is_none());
    let none = serde_json::to_value(&old).unwrap();
    assert!(none.get("faces").is_none(), "no new key unless faces were analyzed: {none}");
    let with = Analysis { faces: Some(Arc::new(vec![face(0.3, 60.0)])), ..old };
    let json = serde_json::to_value(&with).unwrap();
    // an older build's `Analysis` (no `faces` field) reads it, ignoring what it doesn't know
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OldAnalysis {
        sharpness: f32,
        clipped: f32,
        group: Option<u32>,
        best: bool,
    }
    let o: OldAnalysis = serde_json::from_value(json).unwrap();
    assert_eq!(o.sharpness, 42.0);
    assert_eq!(Flag::default(), Flag::None);
}

/// The engine's copies of the model's size and file name (it can't name the crate when built
/// without Analyze Faces) are the crate's.
#[cfg(feature = "faces")]
#[test]
fn model_constants_match_the_crate() {
    assert_eq!(crate::faces::MODEL_BYTES, lightcraft_faces::MODEL.size);
    assert_eq!(crate::faces::MODEL_FILE, lightcraft_faces::MODEL.file_name);
}
