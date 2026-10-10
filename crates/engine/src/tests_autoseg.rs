//! Automatic masks: model status / download guards, the missing-model message, the routing of the
//! Sky / Subject tiles, and (ignored by default: needs the real models) the whole path.

use serde_json::json;

use crate::Session;

#[test]
fn status_lists_both_models_and_the_download_needs_the_licence_to_be_acknowledged() {
    let mut s = Session::with_demo();
    let r = s.execute("autoseg.model.status", &json!({})).unwrap();
    assert_eq!(r["available"], cfg!(feature = "autoseg"));
    #[cfg(feature = "autoseg")]
    {
        let ids: Vec<&str> = r["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["segformer", "subject"]);
        assert!(r["models"][0]["licence"].as_str().unwrap().contains("non-commercial"));
        let e = s.execute("autoseg.model.download", &json!({"model": "subject"})).unwrap_err();
        assert!(e.to_string().contains("acknowledged"), "{e}");
        let e = s.execute("autoseg.model.download", &json!({"model": "nope", "acknowledged": true})).unwrap_err();
        assert!(e.to_string().contains("segformer"), "{e}");
    }
}

#[cfg(feature = "autoseg")]
#[test]
fn without_the_model_mask_auto_says_how_to_get_it_and_the_tiles_keep_the_classical_masks() {
    // an empty models folder: the developer's own models must not leak into the test
    let dir = std::env::temp_dir().join(format!("lc-autoseg-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    crate::autoseg::set_models_dir(Some(dir.clone()));
    let mut s = Session::with_demo();
    let e = s.execute("mask.auto", &json!({"kind": "sky"})).unwrap_err();
    assert!(e.to_string().contains("autoseg.model.download"), "{e}");
    // Sky tile: the classical mask while no model is installed
    s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
    let d = s.develop_of(s.active().unwrap()).unwrap();
    assert!(matches!(d.masks[0].components[0].shape, lightcraft_develop::MaskShape::Sky));
    assert!(s.execute("mask.auto", &json!({"kind": "wat"})).unwrap_err().to_string().contains("kind"));
    crate::autoseg::set_models_dir(None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// With the real models (`LIGHTCRAFT_AUTOSEG_DIR` pointing at them): a sky mask and a subject mask
/// are stored with the photo and render without the model.
#[cfg(feature = "autoseg")]
#[test]
#[ignore = "needs the real models: LIGHTCRAFT_AUTOSEG_DIR=/Volumes/Madson/ArtCraft/models/autoseg"]
fn real_models_make_stored_masks() {
    let mut s = Session::with_demo();
    for kind in ["sky", "people", "subject", "background"] {
        let r = s.execute("mask.auto", &json!({"kind": kind})).unwrap();
        assert!(r["seconds"].as_f64().unwrap() > 0.0, "{r}");
    }
    let d = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(d.masks.len(), 4);
    for m in &d.masks {
        let lightcraft_develop::MaskShape::Prompt { seg: Some(seg), .. } = &m.components[0].shape else { panic!("stored segmentation") };
        assert!(seg.logits().is_some());
    }
    assert!(d.masks[3].components[0].invert, "background = inverted subject");
}
