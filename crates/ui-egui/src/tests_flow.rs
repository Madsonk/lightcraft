//! Lot D: the cull → retouch flow in the window (AI picks section, verdict dots, one click, portrait
//! panel, tour, hints). Headless, like `tests_v14`. `LIGHTCRAFT_TEST_SHOTS=<folder>` saves screenshots.

use std::time::Duration;

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn services() -> crate::Services {
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    crate::Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    }
}

fn demo() -> Headless {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services());
    app.ui.settings.workspaces = true;
    app.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(app, [1500.0, 950.0], 1.0);
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn run(h: &mut Headless, id: &str, p: Value) -> Result<Value, String> {
    let r = h.app.run(id, p);
    h.step();
    h.step();
    r
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn shot(h: &mut Headless, name: &str) {
    if let Ok(dir) = std::env::var("LIGHTCRAFT_TEST_SHOTS") {
        let _ = std::fs::create_dir_all(&dir);
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&dir).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

fn library_sort(h: &mut Headless) {
    run(h, "workspace.library", json!({})).unwrap();
    run(h, "workspace.step", json!({"step": "sort"})).unwrap();
    h.settle(SETTLE);
}

/// Judge the demo album from the window: runs in the background, the four buckets get counts, the
/// grid gets dots, a bucket narrows the view and "Show all" restores it.
#[test]
fn ai_picks_judges_in_the_background_and_narrows_the_grid() {
    let mut h = demo();
    library_sort(&mut h);
    assert!(has(&h, "region:aiPicks"), "the section is in the Sort sidebar");
    assert!(has(&h, "button:cullRun"));
    let all = h.app.session.visible().len();
    assert!(all >= 3, "{all} demo photos");
    click(&mut h, "button:cullRun");
    assert!(h.app.flow.cull.running || h.app.flow.cull.list.is_some(), "started");
    assert!(h.step_until(Duration::from_secs(120), |h| !h.app.flow.cull.running && h.app.flow.cull.list.is_some()), "the run finished");
    h.settle(SETTLE);
    let c = crate::flow::counts(&h.app).expect("counts");
    assert_eq!(c.iter().sum::<u64>() as usize, all, "every photo judged: {c:?}");
    for b in ["highlight", "selected", "review", "rejected"] {
        assert!(has(&h, &format!("button:cullBucket-{b}")), "{b}");
    }
    assert!(h.app.widgets.iter().any(|(w, _)| w.starts_with("verdict:")), "dots on the thumbnails");
    shot(&mut h, "flux-1-ai-picks.png");
    // narrow to the biggest bucket, then back
    let (i, n) = c.iter().enumerate().max_by_key(|(_, n)| **n).map(|(i, n)| (i, *n)).unwrap();
    let key = ["highlight", "selected", "review", "rejected"][i];
    click(&mut h, &format!("button:cullBucket-{key}"));
    h.settle(SETTLE);
    assert_eq!(h.app.session.visible().len() as u64, n, "{key} narrows the grid");
    shot(&mut h, "flux-2-bucket-filter.png");
    click(&mut h, "button:cullShowAll");
    h.settle(SETTLE);
    assert_eq!(h.app.session.visible().len(), all);
    assert!(h.app.session.filter.only.is_empty());
}

/// Cancelling a cull leaves the window answering and nothing written.
#[test]
fn ai_picks_can_be_stopped() {
    let mut h = demo();
    library_sort(&mut h);
    let ratings: Vec<_> = h.app.session.visible().to_vec().iter().filter_map(|id| h.app.session.catalog.photo(*id).map(|p| p.rating)).collect();
    crate::flow::start_cull(&mut h.app, &egui::Context::default()).unwrap();
    crate::flow::cancel_cull(&mut h.app);
    assert!(h.step_until(Duration::from_secs(60), |h| !h.app.flow.cull.running));
    let after: Vec<_> = h.app.session.visible().to_vec().iter().filter_map(|id| h.app.session.catalog.photo(*id).map(|p| p.rating)).collect();
    assert_eq!(ratings, after, "a cull never writes ratings");
}

/// One click: confirmation, the run, the summary, and "Undo everything" restores the stars and flags.
#[test]
fn one_click_confirms_runs_summarises_and_undoes_in_one_step() {
    let mut h = demo();
    library_sort(&mut h);
    let before: Vec<_> =
        h.app.session.visible().to_vec().iter().filter_map(|id| h.app.session.catalog.photo(*id).map(|p| (p.rating, p.flag))).collect();
    click(&mut h, "button:oneClick");
    assert!(h.app.flow.confirm.is_some(), "asks first");
    assert!(has(&h, "button:oneClickGo") && has(&h, "button:oneClickCancel"));
    shot(&mut h, "flux-3-one-click-confirm.png");
    click(&mut h, "button:oneClickCancel");
    assert!(h.app.flow.confirm.is_none() && h.app.flow.job.is_none(), "cancel does nothing");
    click(&mut h, "button:oneClick");
    click(&mut h, "button:oneClickGo");
    assert!(h.step_until(Duration::from_secs(300), |h| h.app.flow.summary.is_some()), "the run ends with a summary");
    h.settle(SETTLE);
    let s = h.app.flow.summary.as_ref().unwrap();
    assert!(s.lines.len() >= 3, "{:?}", s.lines);
    shot(&mut h, "flux-4-one-click-summary.png");
    assert!(has(&h, "button:summaryUndo"));
    click(&mut h, "button:summaryUndo");
    let after: Vec<_> =
        h.app.session.visible().to_vec().iter().filter_map(|id| h.app.session.catalog.photo(*id).map(|p| (p.rating, p.flag))).collect();
    assert_eq!(before, after, "one undo took the stars and flags back");
    click(&mut h, "button:summaryClose");
    assert!(h.app.flow.summary.is_none());
}

/// The portrait panel: groups and sliders on screen, a preview of a photo without a face says so
/// instead of failing, Undo / Redo exist.
#[test]
fn portrait_panel_shows_groups_sliders_and_explains_a_missing_face() {
    let mut h = demo();
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    run(&mut h, "panel.portrait", json!({})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, RightPanel::Portrait);
    assert!(has(&h, "region:portraitPanel"));
    assert!(has(&h, "button:portraitAuto") && has(&h, "button:portraitPreview") && has(&h, "button:portraitApply"));
    assert!(has(&h, "slider:portrait.blemishes"), "the Skin group is open with its sliders");
    h.app.flow.portrait.amounts.insert("blemishes".into(), 60.0);
    click(&mut h, "button:portraitPreview");
    assert!(h.step_until(Duration::from_secs(60), |h| !h.app.flow.portrait.want_preview));
    h.settle(SETTLE);
    // the demo photos are not portraits: either a pair or a reason, never a panic
    assert!(h.app.flow.portrait.pair.is_some() || !h.app.flow.portrait.error.is_empty(), "{}", h.app.flow.portrait.note);
    shot(&mut h, "flux-5-portrait-panel.png");
}

/// The tour walks its seven bubbles and remembers it was seen; the hint is shown once.
#[test]
fn the_tour_is_relaunchable_and_the_hint_comes_once() {
    let mut h = demo();
    assert!(!h.app.ui.flow_tour_done);
    run(&mut h, "flow.tour", json!({})).unwrap();
    assert_eq!(h.app.flow.tour, Some(0));
    shot(&mut h, "flux-6-tour.png");
    for _ in 0..7 {
        click(&mut h, "button:tourNext");
    }
    assert!(h.app.flow.tour.is_none() && h.app.ui.flow_tour_done);
    run(&mut h, "flow.tour", json!({})).unwrap();
    assert_eq!(h.app.flow.tour, Some(0), "relaunchable");
    click(&mut h, "button:tourSkip");
    assert!(h.app.flow.tour.is_none());
    let ctx = egui::Context::default();
    crate::flow::tip(&mut h.app, &ctx, "t", "P");
    assert!(h.app.ui.toast.is_some());
    h.app.ui.toast = None;
    crate::flow::tip(&mut h.app, &ctx, "t", "P");
    assert!(h.app.ui.toast.is_none(), "once");
    assert!(serde_json::to_string(&h.app.ui).unwrap().contains(r#""flowTips":["t"]"#));
}
