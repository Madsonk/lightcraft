//! "My style" and automatic framing end to end on the procedural demo library: a made-up
//! photographer with a known rule edits the photos, `style.train` learns it, `style.apply`
//! previews it, applies it as ONE undo step and undoes it; framing suggestions change nothing
//! until accepted.

use crate::Session;
use crate::media::SourceLevel;
use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_style::synth::{Rng, photographer};
use lightcraft_style::{extract, targets};
use serde_json::{Value, json};

/// Give every demo photo the edit the made-up photographer would make; returns the photo ids.
fn edit_like_the_photographer(s: &mut Session) -> Vec<PhotoId> {
    let ids: Vec<PhotoId> = s.visible_cloned();
    let mut noise = Rng(11);
    for id in &ids {
        let src = s.source_now(*id, SourceLevel::Thumb).unwrap();
        let info = s.source_info(*id);
        let f = extract(&src, &info);
        let t = photographer(&f.values, &mut noise);
        let mut d = DevelopSettings::default();
        targets::apply(&mut d, &t, &info);
        s.set_develop(*id, d, "photographer").unwrap();
    }
    ids
}

fn wait_idle(s: &mut Session) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let st = s.execute("style.status", &json!({})).unwrap();
        if ["train", "apply", "framing"].iter().all(|k| st[k]["state"] != "running") {
            return st;
        }
        assert!(std::time::Instant::now() < deadline, "job timeout");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn too_few_edited_photos_is_a_clear_error_and_nothing_is_stored() {
    let mut s = Session::with_demo();
    let before = s.catalog.to_snapshot();
    let e = s.execute("style.train", &json!({"wait": true})).unwrap_err().to_string();
    assert!(e.contains("edited photos"), "{e}");
    assert_eq!(s.catalog.to_snapshot(), before);
    let list = s.execute("style.list", &json!({})).unwrap();
    assert_eq!(list["profiles"].as_array().unwrap().len(), 0);
    assert!(list["editedPhotos"].as_u64().unwrap() < 12, "the demo library has too few edited photos to learn from");
    assert!(s.execute("style.apply", &json!({"mode": "preview", "wait": true})).is_err(), "no profile yet");
}

#[test]
fn train_preview_apply_and_undo_in_one_step() {
    let mut s = Session::with_demo();
    let ids = edit_like_the_photographer(&mut s);
    assert!(ids.len() >= 12, "the demo library is big enough: {}", ids.len());
    let r = s.execute("style.train", &json!({"name": "Test", "minPhotos": 8, "folds": 3, "wait": true})).unwrap();
    assert_eq!(r["state"], "done", "{r}");
    let list = s.execute("style.list", &json!({})).unwrap();
    let profile = &list["profiles"][0];
    assert_eq!(profile["name"], "Test");
    assert!(profile["skillVsNone"].as_f64().unwrap() > 0.0, "{profile}");
    assert!(profile["learned"].as_u64().unwrap() >= 3, "{profile}");

    // unedited copies of a few photos: the style is previewed on them without any change
    let target: Vec<PhotoId> = ids.iter().copied().take(4).collect();
    for id in &target {
        s.set_develop(*id, DevelopSettings::default(), "reset").unwrap();
    }
    let snapshot = s.catalog.to_snapshot();
    let undo = s.undo.len();
    let p = s.execute("style.apply", &json!({"mode": "preview", "ids": target.iter().map(|i| i.0).collect::<Vec<_>>(), "wait": true})).unwrap();
    assert_eq!(p["state"], "done", "{p}");
    assert!(p["changes"].as_u64().unwrap() >= 1, "{p}");
    assert_eq!(s.catalog.to_snapshot(), snapshot, "a preview changes nothing");
    assert_eq!(s.undo.len(), undo);
    assert!(s.style_proposal(target[0]).is_some() || p["changes"].as_u64().unwrap() < target.len() as u64);

    assert!(s.execute("style.apply", &json!({"mode": "apply"})).is_err(), "needs confirm");
    let a = s.execute("style.apply", &json!({"mode": "apply", "confirm": true})).unwrap();
    assert_eq!(a["changed"], p["changes"]);
    assert_eq!(s.undo.len(), undo + 1, "ONE undo step");
    assert!(s.execute("style.apply", &json!({"mode": "apply", "confirm": true})).is_err(), "a plan applies once");
    s.execute("style.apply", &json!({"mode": "undo"})).unwrap();
    assert_eq!(s.catalog.to_snapshot(), snapshot);
    assert!(s.execute("style.apply", &json!({"mode": "undo"})).is_err(), "nothing left to undo");
}

#[test]
fn a_photo_edited_after_the_preview_is_left_alone() {
    let mut s = Session::with_demo();
    let ids = edit_like_the_photographer(&mut s);
    s.execute("style.train", &json!({"minPhotos": 8, "folds": 3, "wait": true})).unwrap();
    let target: Vec<PhotoId> = ids.iter().copied().take(3).collect();
    for id in &target {
        s.set_develop(*id, DevelopSettings::default(), "reset").unwrap();
    }
    let p = s.execute("style.apply", &json!({"ids": target.iter().map(|i| i.0).collect::<Vec<_>>(), "wait": true})).unwrap();
    if p["changes"].as_u64().unwrap() == 0 {
        return;
    }
    let mut d = DevelopSettings::default();
    d.light.exposure = 0.9;
    s.set_develop(target[0], d.clone(), "manual").unwrap();
    s.execute("style.apply", &json!({"mode": "apply", "confirm": true})).unwrap();
    assert_eq!(s.develop_of(target[0]).unwrap().light.exposure, 0.9, "manual edit survives");
}

#[test]
fn sliders_already_moved_are_kept_unless_overwrite() {
    let mut s = Session::with_demo();
    let ids = edit_like_the_photographer(&mut s);
    s.execute("style.train", &json!({"minPhotos": 8, "folds": 3, "wait": true})).unwrap();
    let id = ids[0];
    let kept = s.develop_of(id).unwrap();
    let p = s.execute("style.apply", &json!({"ids": [id.0], "wait": true})).unwrap();
    let row = &p["photos"][0];
    assert_eq!(row["alreadyEdited"], true);
    if let Some(prop) = s.style_proposal(id) {
        assert_eq!(prop.light.exposure, kept.light.exposure, "exposure was already moved");
    }
}

#[test]
fn cancel_stops_the_learning_without_a_profile() {
    let mut s = Session::with_demo();
    edit_like_the_photographer(&mut s);
    s.execute("style.train", &json!({"minPhotos": 8, "folds": 3})).unwrap();
    s.execute("style.cancel", &json!({})).unwrap();
    let st = wait_idle(&mut s);
    assert!(matches!(st["train"]["state"].as_str(), Some("cancelled" | "done")), "{st}");
    if st["train"]["state"] == "cancelled" {
        assert_eq!(s.execute("style.list", &json!({})).unwrap()["profiles"].as_array().unwrap().len(), 0);
    }
}

#[test]
fn framing_suggestions_change_nothing_until_accepted_then_one_undo() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(4).collect();
    let snapshot = s.catalog.to_snapshot();
    let undo = s.undo.len();
    let idv: Vec<u64> = ids.iter().map(|i| i.0).collect();
    for cmd in ["autostraighten.suggest", "autocrop.suggest"] {
        let r = s.execute(cmd, &json!({"ids": idv, "wait": true})).unwrap();
        assert_eq!(r["state"], "done", "{cmd}: {r}");
        assert_eq!(r["photos"].as_array().unwrap().len(), ids.len());
        assert_eq!(s.catalog.to_snapshot(), snapshot, "{cmd} is read-only");
        assert_eq!(s.undo.len(), undo);
    }
    let r = s.execute("autocrop.suggest", &json!({"ids": idv, "aspect": "1x1", "wait": true})).unwrap();
    assert!(s.execute("autocrop.apply", &json!({})).is_err(), "needs confirm");
    assert!(s.execute("autostraighten.apply", &json!({"confirm": true})).is_err(), "other kind of suggestion");
    let n = r["suggestions"].as_u64().unwrap();
    let a = s.execute("autocrop.apply", &json!({"confirm": true})).unwrap();
    assert_eq!(a["changed"].as_u64().unwrap(), n);
    if n > 0 {
        assert_eq!(s.undo.len(), undo + 1, "ONE undo step");
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.catalog.to_snapshot(), snapshot);
    }
}

#[test]
fn bad_parameters_are_errors_not_panics() {
    let mut s = Session::with_demo();
    for (cmd, p) in [
        ("style.train", json!({"minPhotos": "x"})),
        ("style.train", json!({"folds": 99})),
        ("style.train", json!({"name": ""})),
        ("style.apply", json!({"mode": "explode"})),
        ("style.apply", json!({"strength": 3})),
        ("style.delete", json!({"name": "nope"})),
        ("autocrop.suggest", json!({"ids": [999999999], "wait": true})),
        ("autocrop.suggest", json!({"aspect": "banana"})),
        ("autostraighten.suggest", json!({"minConfidence": 2})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
}

/// Real data (not run by default): `LC_LRCAT=<copy of a .lrcat> cargo test -p lightcraft-engine
/// real_catalog_report -- --ignored --nocapture`. Imports the catalogue read-only into an empty
/// session, learns a style from the edited photos whose original is on disk and prints the
/// measured error against "no adjustment" and "median adjustment".
#[test]
#[ignore]
fn real_catalog_report() {
    let Ok(path) = std::env::var("LC_LRCAT") else { return };
    let mut s = Session::new().with_fs();
    let r = s.execute("library.importLightroom", &json!({"path": path})).unwrap();
    println!("import: {}", r.to_string().chars().take(400).collect::<String>());
    let list = s.execute("style.list", &json!({})).unwrap();
    println!("edited photos with their original: {}", list["editedPhotos"]);
    let t0 = std::time::Instant::now();
    let r = s.execute("style.train", &json!({"name": "Madson", "minPhotos": 12, "folds": 5, "wait": true}));
    println!("train wall time {:.1}s", t0.elapsed().as_secs_f32());
    let r = match r {
        Ok(v) => v,
        Err(e) => {
            println!("train failed: {e}");
            return;
        }
    };
    println!("train result: {}", r.to_string().chars().take(600).collect::<String>());
    let st = s.execute("style.status", &json!({})).unwrap();
    println!("status: {}", st["train"].to_string().chars().take(600).collect::<String>());
    let list = s.execute("style.list", &json!({})).unwrap();
    let p = &list["profiles"][0];
    println!(
        "samples {} shoots {} split {} learned {}/{} skillVsNone {:.3} skillVsMedian {:.3} bytes {} trainSeconds {:.2}",
        p["samples"], p["shoots"], p["split"], p["learned"], p["targets"], p["skillVsNone"].as_f64().unwrap_or(0.0), p["skillVsMedian"].as_f64().unwrap_or(0.0), p["bytes"], p["trainSeconds"].as_f64().unwrap_or(0.0)
    );
    println!("{:<18}{:>8}{:>10}{:>10}{:>10}{:>8}", "slider", "learned", "cv model", "cv median", "cv none", "used%");
    for f in p["sliders"].as_array().unwrap() {
        println!(
            "{:<18}{:>8}{:>10.2}{:>10.2}{:>10.2}{:>8.0}",
            f["id"].as_str().unwrap_or(""),
            f["learned"],
            f["maeModel"].as_f64().unwrap_or(0.0),
            f["maeMedian"].as_f64().unwrap_or(0.0),
            f["maeNone"].as_f64().unwrap_or(0.0),
            100.0 * f["usedShare"].as_f64().unwrap_or(0.0)
        );
    }
}
