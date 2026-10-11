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

/// Frames until the proposal has been planned (one stage per frame). Returns how many frames
/// showed the progress bar's Cancel button, i.e. painted between two stages.
fn planned(h: &mut Headless) -> usize {
    let t0 = std::time::Instant::now();
    let mut painted = 0;
    while !has(h, "check:proposals:camera") && t0.elapsed() < SETTLE {
        if has(h, "button:proposalsCancel") {
            painted += 1;
        }
        h.step();
    }
    assert!(has(h, "check:proposals:camera"), "the proposal was planned");
    painted
}

#[test]
fn the_proposals_section_proposes_without_writing_then_creates_a_copy_and_leaves_the_original() {
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
    let photos = h.app.session.catalog.photos().count();
    click(&mut h, "button:proposalsPropose");
    // v13: the planning is staged, the window paints between the stages (the plan, then the checks)
    let painted = planned(&mut h);
    assert!(painted >= 3, "the progress bar was painted between stages, {painted} frames");
    assert!(!has(&h, "button:proposalsCancel"), "the progress bar went when the plan was ready");
    // eight steps, each with its switch
    for s in ["camera", "clean", "skin_color", "dnb_cleanup", "dnb_sculpt", "eyes", "color", "finish"] {
        assert!(has(&h, &format!("check:proposals:{s}")), "switch of step {s}");
    }
    assert!(*h.app.session.develop_of(id).expect("develop") == *before && h.app.session.undo.len() == steps, "proposing writes nothing");
    // the effect is visible before validating: the loupe's temporary look, nothing written
    let edited_before = h.app.session.catalog.photo(id).expect("photo").is_edited();
    click(&mut h, "button:proposalsPreview");
    h.step();
    assert!(h.app.hover_preview.as_ref().is_some_and(|p| p.label.contains("Proposal")), "the proposal is shown on the photo");
    assert!(h.app.session.undo.len() == steps && h.app.session.catalog.photos().count() == photos, "the preview writes nothing");
    // v13: a visible answer, and the photo is not marked as edited by looking
    assert!(h.app.ui.toast.as_ref().is_some_and(|(m, ..)| m.contains("not saved")), "a toast says the preview is not saved: {:?}", h.app.ui.toast);
    assert_eq!(h.app.session.catalog.photo(id).expect("photo").is_edited(), edited_before, "previewing does not mark the photo edited");
    assert!(*h.app.session.develop_of(id).expect("develop") == *before);
    click(&mut h, "button:proposalsPreview");
    h.step();
    assert!(h.app.hover_preview.is_none(), "flipped off: the original");
    assert!(h.app.ui.toast.as_ref().is_some_and(|(m, ..)| m.contains("original")), "{:?}", h.app.ui.toast);
    click(&mut h, "button:proposalsPreview");
    h.step();
    // the controls of the section do not overlap: an accessibility hit test on a button's centre finds it
    let rects: Vec<(String, egui::Rect)> = h.app.widgets.iter().filter(|(w, _)| w.starts_with("button:proposals")).cloned().collect();
    for (a, ra) in &rects {
        for (b, rb) in &rects {
            assert!(a == b || !ra.contains(rb.center()), "{a} covers the centre of {b}");
        }
    }
    assert!(has(&h, "button:proposalsApply") && has(&h, "button:proposalsApplyVariants"));
    click(&mut h, "button:proposalsApply");
    h.settle(SETTLE);
    assert_eq!(h.app.session.undo.len(), steps + 1, "one undo step");
    // the ORIGINAL is untouched; a copy named after the impact carries the retouch
    assert!(*h.app.session.develop_of(id).expect("develop") == *before, "the original keeps its settings");
    assert_eq!(h.app.session.catalog.photos().count(), photos + 1, "one copy");
    let copy = h.app.session.catalog.photos().find(|p| p.copy_of == Some(id)).expect("a copy of the original").clone();
    assert_eq!(copy.copy_name.as_deref(), Some("Natural skin – Light"));
    assert!(copy.develop.masks.iter().any(|m| m.name.starts_with("Retouch: ")), "the copy has the named masks");
    assert!(h.app.session.catalog.stack_of(id).is_some_and(|st| st.photos.contains(&copy.id)), "stacked with the original");
    // "Remove the retouch copies" removes only the copy
    click(&mut h, "button:proposalsClear");
    h.settle(SETTLE);
    assert!(h.app.session.catalog.photo(copy.id).unwrap().deleted, "the copy went");
    assert!(!h.app.session.catalog.photo(id).unwrap().deleted && *h.app.session.develop_of(id).unwrap() == *before, "the original stayed, untouched");
}

#[test]
fn three_copies_are_named_light_medium_strong_and_the_original_gets_nothing() {
    let mut h = demo([1400.0, 1000.0], json!({"view": "detail", "right": "versions"}));
    click(&mut h, "button:proposalsHeader");
    click(&mut h, "button:proposalsPropose");
    planned(&mut h);
    let id = h.app.session.active().expect("active photo");
    let before = h.app.session.develop_of(id).expect("develop");
    click(&mut h, "button:proposalsApplyVariants");
    h.settle(SETTLE);
    let mut names: Vec<String> = h.app.session.catalog.photos().filter(|p| p.copy_of == Some(id)).filter_map(|p| p.copy_name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["Natural skin – Light", "Natural skin – Medium", "Natural skin – Strong"]);
    assert!(h.app.session.catalog.photo(id).unwrap().versions.is_empty(), "no versions written on the original");
    assert!(*h.app.session.develop_of(id).unwrap() == *before, "the original is untouched");
}
