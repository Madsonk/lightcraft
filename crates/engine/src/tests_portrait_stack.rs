//! The portrait stack end to end, with a synthetic portrait and a synthetic face finder and
//! matte standing in for the models (which are not part of the tests).

use std::path::{Path, PathBuf};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::SpotMode;
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::portrait::fixture::{W, mesh, portrait};
use serde_json::{Value, json};

use crate::cmd::portrait_stack::{apply_one, batch_with, preview_with};
use crate::portrait_stack::{self as ps, Meshes, Sources};
use crate::{Result, Session};

fn write_png(path: &Path, rgb: &[u8], w: usize, h: usize) {
    let data: Vec<[u8; 4]> = rgb.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2], 255]).collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    std::fs::write(path, png).unwrap();
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-portrait-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A session with `a.png` and `b.png` (the synthetic portrait) and `c.png` (no face: a flat grey).
fn session(tag: &str) -> (Session, Vec<PhotoId>, PathBuf) {
    let dir = folder(tag);
    let (rgb, _) = portrait(true);
    write_png(&dir.join("a.png"), &rgb, W, W);
    // (not the same bytes as a.png: identical files are not imported twice)
    let mut rgb_b = rgb.clone();
    rgb_b[0] ^= 1;
    write_png(&dir.join("b.png"), &rgb_b, W, W);
    write_png(&dir.join("c.png"), &vec![120u8; W * W * 3], W, W);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut ids: Vec<(String, PhotoId)> = s.catalog.photos().map(|p| (p.file_name.clone(), p.id)).collect();
    ids.sort();
    (s, ids.into_iter().map(|x| x.1).collect(), dir)
}

fn faces_of(s: &mut Session, id: PhotoId) -> Result<Meshes> {
    // only the portraits have a face
    let name = s.catalog.photo(id).unwrap().file_name.clone();
    if name == "c.png" {
        return Ok(Vec::new());
    }
    Ok(vec![mesh(true).iter().map(|p| [p[0] / W as f32, p[1] / W as f32, p[2] / W as f32]).collect()])
}

fn matte_of(_: &mut Session, _: PhotoId, w: usize, h: usize) -> Result<Plane> {
    let mut m = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 / w as f32 - 0.5) / 0.30, (y as f32 / h as f32 - 0.5) / 0.42);
            m.d[y * w + x] = if dx * dx + dy * dy < 1.0 { 1.0 } else { 0.0 };
        }
    }
    Ok(m)
}

fn steps(v: Value) -> Vec<lightcraft_develop::PortraitStep> {
    ps::steps_from_params(&v, "t").unwrap()
}

fn rendered(s: &mut Session, id: PhotoId) -> lightcraft_raster::Rgba8 {
    let job = s.render_job(id, W, W, false, true).unwrap();
    job.run().rendered.unwrap().image
}

fn luma(img: &lightcraft_raster::Rgba8, x: usize, y: usize) -> f32 {
    let p = img.data[y * img.width + x];
    0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])
}

#[test]
fn the_stack_bakes_a_patch_keeps_the_file_and_undoes_in_one_step() {
    let (mut s, ids, dir) = session("stack");
    let id = ids[0];
    let file = dir.join("a.png");
    let before_bytes = std::fs::read(&file).unwrap();
    let before = rendered(&mut s, id);
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    let r = apply_one(&mut s, id, steps(json!({"blemishes": 90, "dark_circles": 70, "smooth": 40})), false, 4, &mut src).unwrap();
    assert_eq!(r["faces"], 1, "{r}");
    let d = s.develop_of(id).unwrap();
    assert_eq!(d.portrait.steps.len(), 3);
    assert!(d.portrait.patch.is_some() && d.portrait.key != 0);
    let spot = d.spots.iter().find(|sp| sp.mode == SpotMode::Inpaint).expect("the patch is placed by a spot");
    assert_eq!(spot.patch, d.portrait.patch);
    assert!(d.portrait.step("blemishes").unwrap().changed.is_some());
    // the original file is byte for byte what it was
    assert_eq!(std::fs::read(&file).unwrap(), before_bytes);
    // the render shows the retouch: the blemish at (225, 215) is less dark than before
    let after = rendered(&mut s, id);
    let around = luma(&before, 239, 229);
    let (b, a) = (luma(&before, 225, 215), luma(&after, 225, 215));
    assert!((b - around).abs() > 6.0, "the mark was there: {b} vs {around}");
    assert!((a - around).abs() < 0.5 * (b - around).abs(), "and is gone: {a} vs {around}");
    // the background corner is untouched
    assert_eq!(before.data[380 * W + 10], after.data[380 * W + 10]);
    // running it again replaces the patch (one spot), it does not stack them
    apply_one(&mut s, id, steps(json!({"blemishes": 50})), false, 4, &mut src).unwrap();
    let d = s.develop_of(id).unwrap();
    assert_eq!(d.spots.iter().filter(|sp| sp.mode == SpotMode::Inpaint).count(), 1);
    assert_eq!(d.portrait.steps.len(), 3, "the other steps stay");
    assert_eq!(d.portrait.step("blemishes").unwrap().amount, 50.0);
    // undo: back to the first run; undo again: nothing
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().portrait.step("blemishes").unwrap().amount, 90.0);
    s.execute("edit.undo", &json!({})).unwrap();
    let d = s.develop_of(id).unwrap();
    assert!(d.portrait.is_empty() && d.spots.is_empty());
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(!s.develop_of(id).unwrap().portrait.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_step_can_be_removed_and_the_others_run_again() {
    let (mut s, ids, dir) = session("remove");
    let id = ids[0];
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    apply_one(&mut s, id, steps(json!({"blemishes": 90, "shine": 80})), false, 4, &mut src).unwrap();
    apply_one(&mut s, id, steps(json!({"blemishes": 0})), false, 4, &mut src).unwrap();
    let d = s.develop_of(id).unwrap();
    assert!(d.portrait.step("blemishes").is_none() && d.portrait.step("shine").is_some());
    apply_one(&mut s, id, steps(json!({"shine": 0})), false, 4, &mut src).unwrap();
    let d = s.develop_of(id).unwrap();
    assert!(d.portrait.is_empty() && d.spots.is_empty(), "an empty stack leaves no trace");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_photo_without_a_face_is_skipped_not_failed_and_the_album_goes_on() {
    let (mut s, ids, dir) = session("batch");
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    s.selection.ids = ids.clone();
    let r = batch_with(&mut s, &json!({"auto": true, "chunk": 2}), &mut src).unwrap();
    assert_eq!((r["total"].as_u64(), r["done"].as_u64()), (Some(3), Some(2)), "{r}");
    assert_eq!(r["next"], 2);
    let r2 = batch_with(&mut s, &json!({"auto": true, "chunk": 2, "offset": 2}), &mut src).unwrap();
    assert_eq!(r2["next"], Value::Null, "{r2}");
    assert_eq!(r2["photos"][0]["skipped"], "no face found", "{r2}");
    for id in &ids[..2] {
        assert!(!s.develop_of(*id).unwrap().portrait.is_empty());
    }
    assert!(s.develop_of(ids[2]).unwrap().portrait.is_empty());
    assert!(r["secondsPerPhoto"].as_f64().unwrap() > 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_batch_stops_between_two_photos_when_cancelled() {
    let (mut s, ids, dir) = session("cancel");
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    s.selection.ids = ids.clone();
    ps::request_cancel();
    // a continued batch (offset > 0) does not clear the flag
    let r = batch_with(&mut s, &json!({"auto": true, "offset": 1}), &mut src).unwrap();
    assert_eq!(r["cancelled"], true, "{r}");
    assert_eq!(r["photos"].as_array().unwrap().len(), 0);
    // a new batch starts clean
    let r = batch_with(&mut s, &json!({"auto": true}), &mut src).unwrap();
    assert_eq!(r["cancelled"], false, "{r}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_preview_writes_two_pictures_and_changes_nothing() {
    let (mut s, ids, dir) = session("preview");
    let id = ids[0];
    s.selection.ids = vec![id];
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    let out = dir.join("out").join("p-after.png");
    let r = preview_with(&mut s, &json!({"id": id.0, "blemishes": 90, "edge": 400, "path": out.to_string_lossy()}), &mut src).unwrap();
    assert!(out.is_file() && Path::new(r["before"].as_str().unwrap()).is_file(), "{r}");
    assert!(s.develop_of(id).unwrap().portrait.is_empty(), "nothing stored");
    assert!(r["seconds"]["retouch"].as_f64().unwrap() >= 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_background_is_replaced_behind_the_subject_matte() {
    let (mut s, ids, dir) = session("bg");
    let id = ids[0];
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    let before = rendered(&mut s, id);
    apply_one(&mut s, id, steps(json!({"background": {"mode": "color", "color": "#2040c0", "amount": 100}})), false, 4, &mut src).unwrap();
    let after = rendered(&mut s, id);
    let corner = after.data[10 * W + 10];
    assert!(corner[2] > corner[0] + 60, "blue background: {corner:?}");
    assert_ne!(before.data[10 * W + 10], corner);
    // the subject (the face centre) keeps its colour
    let (b, a) = (before.data[200 * W + 200], after.data[200 * W + 200]);
    assert!((i32::from(b[0]) - i32::from(a[0])).abs() < 8, "{b:?} {a:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn changing_the_white_balance_marks_the_pixels_as_out_of_date() {
    let (mut s, ids, dir) = session("stale");
    let id = ids[0];
    let (mut f, mut m) = (faces_of, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    apply_one(&mut s, id, steps(json!({"smooth": 50})), false, 4, &mut src).unwrap();
    let d = s.develop_of(id).unwrap();
    assert_eq!(d.portrait.key, ps::linear_key(&d));
    let mut d2 = (*d).clone();
    d2.wb.temp += 800.0;
    assert_ne!(d2.portrait.key, ps::linear_key(&d2));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bad_steps_are_refused_with_a_message() {
    let e = ps::steps_from_params(&json!({"steps": [{"kind": "wings", "amount": 10}]}), "t").unwrap_err().to_string();
    assert!(e.contains("unknown step") && e.contains("blemishes"), "{e}");
    let e = ps::steps_from_params(&json!({"background": {"mode": "color", "color": "red"}}), "t").unwrap_err().to_string();
    assert!(e.contains("#rrggbb"), "{e}");
    let e = ps::steps_from_params(&json!({"background": {"mode": "paint"}}), "t").unwrap_err().to_string();
    assert!(e.contains("blur, color or dim"), "{e}");
    // amounts are clamped, not trusted
    let st = ps::steps_from_params(&json!({"smooth": 900, "teeth": -4}), "t").unwrap();
    assert_eq!((st[0].amount, st[1].amount), (100.0, 0.0));
    assert!(ps::auto_steps().iter().all(|s| lightcraft_retouch::portrait::info_of(&s.kind).is_some()));
}

#[test]
fn the_commands_are_registered() {
    for c in [
        "portrait.list",
        "portrait.apply",
        "portrait.preview",
        "portrait.reset",
        "portrait.history",
        "portrait.batch",
        "portrait.auto",
        "portrait.cancel",
        "portrait.remove",
    ] {
        assert!(crate::cmd::find_command(c).is_some(), "{c}");
    }
    let mut s = Session::with_demo();
    let r = s.execute("portrait.list", &json!({})).unwrap();
    assert_eq!(r["sliders"].as_array().unwrap().len(), 15);
}

// ---- open eyes -----------------------------------------------------------------------------

use lightcraft_catalog::{Analysis, FaceInfo, Op};
use lightcraft_retouch::portrait::fixture::{mesh_closed, portrait_closed, shifted};

fn norm(m: &[[f32; 3]]) -> Vec<[f32; 3]> {
    m.iter().map(|p| [p[0] / W as f32, p[1] / W as f32, p[2] / W as f32]).collect()
}

fn set_faces(s: &mut Session, id: PhotoId, open: f32, group: Option<u32>) {
    let face = FaceInfo { rect: [0.25, 0.1, 0.75, 0.9], score: 0.95, open: [open, open], sharp: [80.0, 80.0], turn: 0.0 };
    let analysis = Analysis { faces: Some(std::sync::Arc::new(vec![face])), sharpness: 50.0, group, ..Default::default() };
    s.catalog.apply(Op::SetAnalysis { id, analysis: Some(analysis) }).unwrap();
}

/// `a.png`: the face with closed eyes; `b.png`: the same scene a moment later, open eyes, a bit
/// shifted and darker; both in the same burst, analyzed.
fn eyes_session(tag: &str, with_reference: bool) -> (Session, PhotoId, Option<PhotoId>, PathBuf) {
    let dir = folder(tag);
    write_png(&dir.join("a.png"), &portrait_closed(), W, W);
    let (open, _) = portrait(true);
    let (refimg, _) = shifted(&open, &mesh(true), 9, -6, 0.94);
    if with_reference {
        write_png(&dir.join("b.png"), &refimg, W, W);
    }
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let find = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).map(|p| p.id);
    let (a, b) = (find(&s, "a.png").unwrap(), find(&s, "b.png"));
    set_faces(&mut s, a, 0.02, Some(1));
    if let Some(b) = b {
        set_faces(&mut s, b, 0.32, Some(1));
    }
    s.selection.ids = vec![a];
    s.selection.active = Some(a);
    (s, a, b, dir)
}

fn eye_meshes(s: &mut Session, id: PhotoId) -> Result<Meshes> {
    let name = s.catalog.photo(id).unwrap().file_name.clone();
    let m = if name == "a.png" { mesh_closed() } else { shifted(&portrait(true).0, &mesh(true), 9, -6, 0.94).1 };
    Ok(vec![norm(&m)])
}

#[test]
fn open_eyes_previews_first_applies_only_when_confirmed_and_keeps_the_original() {
    use crate::cmd::portrait_stack::open_eyes_with;
    let (mut s, a, b, dir) = eyes_session("eyes", true);
    let b = b.unwrap();
    let before_bytes = std::fs::read(dir.join("a.png")).unwrap();
    let (mut f, mut m) = (eye_meshes, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    // applying before any preview is refused
    let e = open_eyes_with(&mut s, &json!({"apply": true, "confirmed": true}), &mut src).unwrap_err().to_string();
    assert!(e.contains("preview"), "{e}");
    // the preview finds the reference by itself, writes two pictures, changes nothing
    let out = dir.join("o").join("e-after.png");
    let r = open_eyes_with(&mut s, &json!({"edge": 400, "path": out.to_string_lossy()}), &mut src).unwrap();
    assert_eq!(r["source"], b.0, "{r}");
    assert_eq!(r["needsConfirmation"], true);
    assert!(out.is_file() && std::path::Path::new(r["before"].as_str().unwrap()).is_file());
    let eyes = r["eyes"].as_array().unwrap();
    assert_eq!(eyes.len(), 2, "{r}");
    assert!(eyes.iter().all(|e| e["skipped"].is_null() && e["openAfter"].as_f64().unwrap() > 0.3), "{r}");
    assert!(s.develop_of(a).unwrap().portrait.is_empty(), "a preview stores nothing");
    // apply without the confirmation: refused
    let e = open_eyes_with(&mut s, &json!({"apply": true}), &mut src).unwrap_err().to_string();
    assert!(e.contains("preview"), "{e}");
    // confirmed: stored as a step, one undo, the file untouched, the iris is there in the render
    let before = rendered(&mut s, a);
    open_eyes_with(&mut s, &json!({"apply": true, "confirmed": true}), &mut src).unwrap();
    let d = s.develop_of(a).unwrap();
    assert!(d.portrait.step("open_eyes").is_some() && d.portrait.patch.is_some());
    assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), before_bytes);
    let after = rendered(&mut s, a);
    assert!(luma(&before, 150, 154) > luma(&after, 150, 154) + 40.0, "{} -> {}", luma(&before, 150, 154), luma(&after, 150, 154));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(a).unwrap().portrait.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_usable_reference_it_says_so_instead_of_inventing_eyes() {
    use crate::cmd::portrait_stack::open_eyes_with;
    let (mut s, _, _, dir) = eyes_session("noref", false);
    let (mut f, mut m) = (eye_meshes, matte_of);
    let mut src = Sources { faces: &mut f, matte: &mut m };
    let e = open_eyes_with(&mut s, &json!({}), &mut src).unwrap_err().to_string();
    assert!(e.contains("no reference photo"), "{e}");
    let e = open_eyes_with(&mut s, &json!({"level": 2}), &mut src).unwrap_err().to_string();
    assert!(e.contains("not available"), "{e}");
    let r = open_eyes_with(&mut s, &json!({"level": 3, "face": 0}), &mut src).unwrap();
    assert_eq!(r["sent"], false);
    assert_eq!(r["needsConsent"], true);
    assert!(std::path::Path::new(r["willLeaveTheMac"]["file"].as_str().unwrap()).is_file());
    let _ = std::fs::remove_dir_all(dir);
}

/// Real faces, real model: `LC_PORTRAIT_DIR=<folder of JPEGs (a copy)> LC_PORTRAIT_OUT=<folder>
/// LIGHTCRAFT_FACES_DIR=<folder with face_landmarker.task> cargo test … real_faces -- --ignored --nocapture`.
/// Previews (before/after) go to the output folder; nothing is written next to the photos.
#[cfg(feature = "faces")]
#[test]
#[ignore = "needs the face model and a folder of photos"]
fn real_faces_preview_and_apply_timings() {
    let (Some(dir), Some(out)) = (std::env::var_os("LC_PORTRAIT_DIR"), std::env::var_os("LC_PORTRAIT_OUT")) else { return };
    let (dir, out) = (PathBuf::from(dir), PathBuf::from(out));
    std::fs::create_dir_all(&out).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut photos: Vec<(String, PhotoId)> = s.catalog.photos().map(|p| (p.file_name.clone(), p.id)).collect();
    photos.sort();
    for (name, id) in photos {
        let path = out.join(format!("{}-after.png", name.trim_end_matches(".jpg")));
        let t = std::time::Instant::now();
        let r = s.execute("portrait.preview", &json!({"id": id.0, "auto": true, "edge": 1400, "path": path.to_string_lossy()}));
        let pv = t.elapsed().as_secs_f64();
        match r {
            Ok(v) => {
                let t = std::time::Instant::now();
                let a = s.execute("portrait.apply", &json!({"id": id.0, "auto": true}));
                let ap = t.elapsed().as_secs_f64();
                let steps: Vec<String> = v["steps"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| {
                                format!(
                                    "{}:{}%{}",
                                    x["step"].as_str().unwrap_or("?"),
                                    (x["changed"].as_f64().unwrap_or(0.0) * 100.0).round(),
                                    x["found"].as_u64().map_or(String::new(), |n| format!("({n})"))
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                println!(
                    "{name}: faces={} preview={pv:.1}s apply={ap:.1}s {} steps: {}",
                    v["faces"],
                    if a.is_ok() { "applied" } else { "apply failed" },
                    steps.join(" ")
                );
                if let Ok(a) = a {
                    println!("   apply detail: {}", a["photos"][0]["seconds"]);
                }
                let _ = s.execute("portrait.reset", &json!({"id": id.0}));
            }
            Err(e) => println!("{name}: ERROR {e}"),
        }
    }
}
