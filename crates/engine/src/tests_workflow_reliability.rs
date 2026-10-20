use crate::Session;
use serde_json::json;

fn scratch(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("lc-workflow-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn image(path: &std::path::Path) {
    let img = lightcraft_raster::Rgba8::from_fn(48, 32, |x, y| [(x * 5) as u8, (y * 7) as u8, 90, 255]);
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn workflow_selection_guard_refuses_stale_edits_and_bad_targets() {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    let expected = serde_json::to_value(&s.selection).unwrap();
    s.execute("library.select", &json!({"ids": [ids[1].0], "active": ids[1].0})).unwrap();
    let before = s.catalog.to_snapshot();
    let e = s.execute("photo.rate", &json!({"rating": 5, "expectedSelection": expected})).unwrap_err().to_string();
    assert!(e.contains("selection changed"));
    assert_eq!(s.catalog.to_snapshot(), before);
    let expected = serde_json::to_value(&s.selection).unwrap();
    s.execute("photo.rate", &json!({"rating": 5, "expectedSelection": expected})).unwrap();
    assert_eq!(s.catalog.photo(ids[1]).unwrap().rating, 5);
    for p in [json!({"ids": []}), json!({"ids": ["bad"]}), json!({"id": 999999}), json!({"id": ids[0].0, "ids": [ids[1].0]})] {
        assert!(s.export_params(&p).is_err(), "{p}");
    }
    s.export_presets.push(crate::export::ExportPreset { name: "Legacy".into(), params: json!({"id": ids[0].0, "format": "png"}) });
    let settings = s.export_params(&json!({"preset": "Legacy"})).unwrap();
    assert!(settings.get("id").is_none());
    assert_eq!(settings["format"], "png");
}

#[test]
fn workflow_culling_preview_preserves_catalog_and_cache_persists() {
    let dir = scratch("culling");
    let src = dir.join("input.png");
    image(&src);
    let lib = dir.join("library");
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    let before = s.catalog.to_snapshot();
    let undo = s.undo.len();
    let r = s.execute("photo.analyze", &json!({"id": id.0, "dryRun": true, "rejectBelow": 100})).unwrap();
    assert_eq!(r["rejected"], 0);
    assert!(r["photos"][0]["reasons"].is_array());
    assert_eq!(s.catalog.to_snapshot(), before);
    assert_eq!(s.undo.len(), undo);
    assert!(s.culling_cache.is_empty());
    let r = s.execute("photo.analyze", &json!({"id": id.0})).unwrap();
    assert_eq!(r["measured"], 1);
    assert_eq!(r["targetIds"], json!([id.0]));
    s.close_library().unwrap();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("photo.analyze", &json!({"id": id.0})).unwrap();
    assert_eq!(r["cached"], 1, "{r}");
    let r = s.execute("photo.analyze", &json!({"id": id.0, "force": true})).unwrap();
    assert_eq!(r["cached"], 0);
    // A changed source is never served from the persistent measurement cache.
    let mut bytes = std::fs::read(&src).unwrap();
    bytes.extend_from_slice(b"trailing-data");
    std::fs::write(&src, bytes).unwrap();
    let r = s.execute("photo.analyze", &json!({"id": id.0})).unwrap();
    assert_eq!(r["cached"], 0);
    assert!(s.execute("photo.analyze", &json!({"ids": "bad"})).is_err());
    assert!(s.execute("photo.analyze", &json!({"rejectBelow": -1})).is_err());
    drop(s);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn workflow_missing_plan_is_read_only_then_relink_is_undoable() {
    let dir = scratch("relink");
    let src = dir.join("input.png");
    image(&src);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src]})).unwrap();
    let moved = dir.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::rename(&src, moved.join("input.png")).unwrap();
    let diagnosis = s.execute("library.diagnoseFiles", &json!({})).unwrap();
    assert_eq!(diagnosis["counts"]["missing"], 1);
    let before = s.catalog.to_snapshot();
    let plan = s.execute("library.findMissing", &json!({"folder": moved, "dryRun": true})).unwrap();
    assert_eq!(plan["found"].as_array().unwrap().len(), 1);
    assert_eq!(s.catalog.to_snapshot(), before);
    s.execute("library.findMissing", &plan).unwrap();
    assert_eq!(s.execute("library.diagnoseFiles", &json!({})).unwrap()["counts"]["available"], 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.to_snapshot(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn workflow_export_receipt_detects_changed_and_invalid_output() {
    use crate::export::{Destination, ExportOptions, export_batch, write_file};
    let dir = scratch("export");
    let mut s = Session::with_demo().with_fs();
    let id = s.active().unwrap();
    let opts = ExportOptions::from_json(&json!({"format": "png", "longEdge": 48}));
    let to = Destination { dir: dir.to_string_lossy().to_string(), exact: None };
    let files = export_batch(&mut s, &[id], &opts, &to, &mut write_file, &|p| std::path::Path::new(p).exists()).unwrap();
    let receipt = &files[0];
    assert_eq!(receipt["photo"], id.0);
    assert_eq!(s.execute("export.verify", receipt).unwrap()["verified"], true);
    let mut bad = receipt.clone();
    bad["width"] = json!(9999);
    assert!(s.execute("export.verify", &bad).is_err());
    std::fs::write(receipt["path"].as_str().unwrap(), b"broken").unwrap();
    assert!(s.execute("export.verify", receipt).is_err());
    assert!(s.execute("export.verify", &json!({"path": receipt["path"]})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn workflow_lut_profile_batch_has_explicit_targets_and_one_undo() {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    let profile = crate::presets::PROFILES.iter().find(|p| p.id != s.develop_of(ids[0]).unwrap().profile.id).unwrap().id;
    let old = s.develop_of(ids[0]).unwrap();
    let other = s.develop_of(ids[2]).unwrap();
    let undo = s.undo.len();
    s.execute("develop.profile", &json!({"id": profile, "ids": [ids[0].0, ids[1].0]})).unwrap();
    assert_eq!(s.develop_of(ids[0]).unwrap().profile.id, profile);
    assert_eq!(s.develop_of(ids[1]).unwrap().profile.id, profile);
    assert_eq!(*s.develop_of(ids[2]).unwrap(), *other);
    assert_eq!(s.undo.len(), undo + 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(*s.develop_of(ids[0]).unwrap(), *old);
    for targets in [json!([]), json!(["bad"]), json!([999999])] {
        let before = s.catalog.to_snapshot();
        assert!(s.execute("develop.profile", &json!({"id": profile, "ids": targets})).is_err());
        assert_eq!(s.catalog.to_snapshot(), before);
    }
}
