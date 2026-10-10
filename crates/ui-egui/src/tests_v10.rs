//! LightCraft v10: the Proposals section of the Versions panel (the proposed retouch). Headless, on
//! the demo library: proposing writes nothing, every step has its switch and its explanation,
//! Apply writes ordinary settings in one undo step, Apply with variants leaves Before, Light,
//! Medium and Strong in Versions.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo(size: [f32; 2], ui: Value) -> Headless {
    let services = Services { png: None, pick_folder: Some(Box::new(|| None)), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, size, 1.0);
    let r = h.request("ui.set", ui, T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

#[test]
fn the_proposals_section_proposes_without_writing_then_applies_in_one_undo_step() {
    let mut h = demo([1400.0, 1000.0], json!({"view": "detail", "right": "versions"}));
    assert!(has(&h, "button:proposalsHeader"), "the section is at the top of the Versions panel");
    assert!(!has(&h, "button:proposalsPropose"), "closed until opened");
    click(&mut h, "button:proposalsHeader");
    for id in ["button:proposalsImpact0", "button:proposalsImpact1", "button:proposalsImpact2", "button:proposalsPropose"] {
        assert!(has(&h, id), "{id}");
    }
    let id = h.app.session.active().expect("active photo");
    let before = h.app.session.develop_of(id).expect("develop");
    let steps = h.app.session.undo.len();
    click(&mut h, "button:proposalsPropose");
    h.settle(SETTLE);
    // eight steps, each with its switch
    for s in ["camera", "clean", "skin_color", "dnb_cleanup", "dnb_sculpt", "eyes", "color", "finish"] {
        assert!(has(&h, &format!("check:proposals:{s}")), "switch of step {s}");
    }
    assert!(*h.app.session.develop_of(id).expect("develop") == *before && h.app.session.undo.len() == steps, "proposing writes nothing");
    assert!(has(&h, "button:proposalsApply") && has(&h, "button:proposalsApplyVariants"));
    click(&mut h, "button:proposalsApply");
    h.settle(SETTLE);
    assert_eq!(h.app.session.undo.len(), steps + 1, "one undo step");
    assert!(h.app.session.develop_of(id).expect("develop").masks.iter().all(|m| m.name.starts_with("Retouch: ")), "ordinary named masks");
}

#[test]
fn apply_with_variants_leaves_before_light_medium_strong_in_versions() {
    let mut h = demo([1400.0, 1000.0], json!({"view": "detail", "right": "versions"}));
    click(&mut h, "button:proposalsHeader");
    click(&mut h, "button:proposalsPropose");
    h.settle(SETTLE);
    click(&mut h, "button:proposalsApplyVariants");
    h.settle(SETTLE);
    let id = h.app.session.active().expect("active photo");
    let names: Vec<String> = h.app.session.catalog.photo(id).expect("photo").versions.iter().map(|v| v.name.clone()).collect();
    for n in ["Before", "Light", "Medium", "Strong"] {
        assert!(names.iter().any(|x| x.ends_with(n)), "{n} in {names:?}");
    }
}
