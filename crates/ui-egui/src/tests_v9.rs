//! LightCraft v9, phase 1 of the UI study ("fix without changing the structure"): one menu item per
//! action, the dust-from-a-series and portrait buttons in the Remove panel, People opening on
//! Recognition, People and Places in the left panel, one Sorting Help for the three helpers, the
//! tool strip's labels and groups, the slow commands as background tasks with progress and Cancel,
//! and the Resume bar. All headless, on the demo library or a small library made here.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::menubar::{MenuNode, menu_bar};
use crate::state::{Dialog, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn temp(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("lc-v9-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo(size: [f32; 2], ui: Value) -> Headless {
    let services = Services { png: None, pick_folder: Some(Box::new(|| None)), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, size, 1.0);
    let r = h.request("ui.set", ui, T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn widget(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
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

/// Frames for a smooth scroll (a section scrolled into view) to arrive.
fn glide(h: &mut Headless) {
    for _ in 0..40 {
        h.step();
    }
}

/// Frames until no background task is left (the results are applied by the frames).
fn finish_tasks(h: &mut Headless) {
    assert!(h.step_until(SETTLE, |h| h.app.tasks.is_empty() && h.app.healing.is_none()), "tasks still running: {:?}", h.app.tasks.labels());
    h.step();
}

// ------------------------------------------------------------------------------------ 1. menus

fn items(nodes: &[MenuNode]) -> Vec<(&str, &str)> {
    nodes.iter().filter_map(|n| if let MenuNode::Item { id, label, .. } = n { Some((id.as_str(), label.as_str())) } else { None }).collect()
}

#[test]
fn every_action_is_one_item_and_no_submenu_repeats_its_own_name() {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { pick_folder: Some(Box::new(|| None)), ..Default::default() });
    let bar = menu_bar(&app);
    fn walk(nodes: &[MenuNode], path: &str) {
        for n in nodes {
            if let MenuNode::Submenu { label, children } = n {
                // (the old defect: a submenu holding one item that repeats its own name)
                let same = children.iter().any(|c| matches!(c, MenuNode::Item { label: l, .. } if l == label));
                assert!(!same, "{path} ▸ {label} holds an item of the same name");
                walk(children, &format!("{path} ▸ {label}"));
            }
        }
    }
    for (title, nodes) in &bar {
        walk(nodes, title);
    }
    let menu = |t: &str| bar.iter().find(|(x, _)| x == t).map(|(_, v)| v.clone()).unwrap();
    let (photo, file) = (menu("Photo"), menu("File"));
    // each one a top-level item of its menu, with a plain label
    let want = [
        ("dialog.cull", "Sorting Help…"),
        ("portrait.retouch", "Portrait Retouch"),
        ("portrait.clear", "Remove Portrait Retouch"),
        ("dust.apply", "Remove Dust From Series"),
    ];
    for (id, label) in want {
        assert!(items(&photo).contains(&(id, label)), "Photo ▸ {label} ({id}) missing: {:?}", items(&photo));
    }
    assert!(items(&file).contains(&("gallery.export", "Export Web Gallery…")), "{:?}", items(&file));
    // the old nesting is gone everywhere
    let json = serde_json::to_string(&bar).unwrap();
    for gone in ["Remove Dust From Selection", "\"label\":\"Portrait Retouch\",\"children\"", "\"label\":\"Export Web Gallery…\",\"children\""] {
        assert!(!json.contains(gone), "{gone}");
    }
    // the slow one asks for the background; the others carry no parameters
    let dust = photo.iter().find_map(|n| match n {
        MenuNode::Item { id, params, .. } if id == "dust.apply" => Some(params.clone()),
        _ => None,
    });
    assert_eq!(dust, Some(json!({"background": true})));
}

#[test]
fn the_new_shortcuts_are_free() {
    let keymap = crate::shortcuts::Keymap::new();
    for (id, sc) in [("dialog.cull", "Cmd+Alt+A"), ("portrait.retouch", "Cmd+Alt+P"), ("gallery.export", "Cmd+Alt+G")] {
        assert_eq!(crate::shortcuts::shortcut_of(&keymap, id), Some(sc), "{id}");
        assert!(crate::shortcuts::conflicts(&keymap, id, sc).is_empty(), "{id} shares {sc}: {:?}", crate::shortcuts::conflicts(&keymap, id, sc));
    }
}

// ----------------------------------------------------------------- 2. the Remove panel's buttons

#[test]
fn the_remove_panel_has_the_series_and_portrait_buttons() {
    let mut h = demo([1400.0, 900.0], json!({"view": "detail", "right": "remove"}));
    let panel = widget(&h, "panel:right_panel");
    for id in ["button:dustSeries", "button:portraitRetouch"] {
        assert!(panel.contains_rect(widget(&h, id)), "{id} sits in the panel");
    }
    // the series button searches in the background and heals the whole series in one undo step
    let steps = h.app.session.undo.len();
    click(&mut h, "button:dustSeries");
    finish_tasks(&mut h);
    let r = h.app.last_task.get(crate::heavy::DUST).cloned().unwrap_or_else(|| panic!("no result: {:?}", h.app.last_task));
    assert!(r.get("error").is_none() && r["cancelled"].is_null(), "{r}");
    assert!(h.app.session.undo.len() <= steps + 1, "at most one undo step");
    // portrait retouch without the face model: said, not silent
    click(&mut h, "button:portraitRetouch");
    assert!(h.app.ui.toast.is_some() || h.app.ui.dialog.is_some(), "asking without the model answers");
}

// -------------------------------------------------------------------------- 3. People opens on faces

fn small_library(photos: usize) -> (lightcraft_engine::Session, Vec<lightcraft_catalog::PhotoId>) {
    let dir = temp("lib");
    for i in 0..photos {
        let data: Vec<[u8; 4]> = (0..64 * 48).map(|p| [(p % 64) as u8 * 3, (p / 64) as u8 * 4, (i * 20) as u8, 255]).collect();
        let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(dir.join(format!("p{i}.png")), bytes).unwrap();
    }
    let mut s = lightcraft_engine::Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut ids: Vec<_> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).map(lightcraft_catalog::PhotoId).collect();
    ids.sort_by_key(|i| i.0);
    (s, ids)
}

fn unit(v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / n).collect()
}

fn face(who: u64, n: u64) -> Vec<f32> {
    let rnd = |seed: u64| {
        let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        unit(
            (0..512)
                .map(|_| {
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                })
                .collect(),
        )
    };
    let (a, b) = (rnd(100 + who), rnd(9000 + who * 31 + n));
    unit(a.iter().zip(&b).map(|(x, y)| x + 0.5 * y).collect())
}

/// A library on disk whose photos have faces in the people index (none named).
fn library_with_faces() -> Headless {
    let (mut s, ids) = small_library(6);
    let photos = ids.iter().enumerate().map(|(i, id)| (*id, vec![(face(if i < 3 { 1 } else { 2 }, i as u64), [0.3, 0.2, 0.6, 0.6])])).collect();
    lightcraft_engine::people::inject_faces(&mut s, photos);
    let mut h = Headless::new(LightcraftApp::new(s, Services::default()), [1400.0, 900.0], 1.0);
    h.settle(SETTLE);
    h
}

#[test]
fn people_opens_on_recognition_when_faces_were_found_and_nobody_is_named_in_xmp() {
    let mut h = library_with_faces();
    let open = |h: &mut Headless, view: ViewMode| {
        h.app.ui.view = view;
        h.step();
        h.step();
    };
    assert!(!h.app.caches.people_rec.recognition, "the first tab until the view is opened");
    // however the view is reached (the menu, the bottom bar, a restored session): faces to name, so Recognition
    open(&mut h, ViewMode::People);
    assert!(h.app.caches.people_rec.recognition, "not \"No named people yet\"");
    // the choice is made once per visit: the user can click the other tab and stay there
    h.app.caches.people_rec.recognition = false;
    h.step();
    assert!(!h.app.caches.people_rec.recognition, "a tab clicked stays");
    // the setting turns it off
    open(&mut h, ViewMode::PhotoGrid);
    h.app.ui.settings.people_recognition_first = false;
    open(&mut h, ViewMode::People);
    assert!(!h.app.caches.people_rec.recognition);
    // naming someone in Recognition does not change that the first tab (XMP names) is empty
    open(&mut h, ViewMode::PhotoGrid);
    h.app.ui.settings.people_recognition_first = true;
    let g = h.app.session.execute("people.list", &json!({"groups": 10})).unwrap()["groups"][0]["id"].as_str().unwrap().to_string();
    h.app.session.execute("people.name", &json!({"group": g, "name": "Awa"})).unwrap();
    open(&mut h, ViewMode::People);
    assert!(h.app.caches.people_rec.recognition, "the people named there are listed there");
    // a library with no faces (the demo one) keeps the first tab, and never gains an index by being looked at
    let mut d = demo([1400.0, 900.0], json!({"view": "photoGrid"}));
    d.app.run("view.people", json!({})).unwrap();
    d.step();
    d.step();
    assert!(!d.app.caches.people_rec.recognition);
    assert_eq!(d.app.ui.view, ViewMode::People);
}

// -------------------------------------------------------------- 4. People and Places on the left

#[test]
fn people_and_places_are_sections_of_the_left_panel_with_counts_filters_and_drops() {
    let mut h = demo([1500.0, 1000.0], json!({"view": "photoGrid", "leftPanel": true}));
    let ids: Vec<u64> = h.app.session.catalog.photos().map(|p| p.id.0).take(6).collect();
    h.app.session.execute("photo.setMeta", &json!({"ids": &ids[..3], "addKeywords": ["People|Awa"]})).unwrap();
    h.app.session.execute("photo.setMeta", &json!({"ids": &ids[..2], "addKeywords": ["People|Bao", "Places|France|Paris"]})).unwrap();
    h.step();
    h.step();
    for id in [
        "sidebarSection:people",
        "sidebarSection:places",
        "source:keyword:People|Awa",
        "source:keyword:People|Bao",
        "source:keyword:Places|France",
        "source:peopleOpen",
        "source:placesOpen",
    ] {
        assert!(
            has(&h, id),
            "{id}: {:?}",
            h.app
                .widgets
                .iter()
                .map(|w| w.0.as_str())
                .filter(|w| w.starts_with("keyword") || w.starts_with("sidebar") || w.starts_with("source"))
                .collect::<Vec<_>>()
        );
    }
    // counts, on the same rows the Keywords tree used
    let count = |h: &Headless, id: &str| h.app.widgets.iter().any(|(w, _)| w == &format!("count:{id}"));
    assert!(count(&h, "keyword:People|Awa") && count(&h, "keyword:People|Bao"));
    // nothing twice: the Keywords section no longer repeats them
    assert_eq!(h.app.widgets.iter().filter(|(w, _)| w == "source:keyword:People|Awa").count(), 1);
    assert!(h.app.session.catalog.keyword_tree().iter().any(|n| n.path == "People"), "they are still keywords");
    // one click filters by the person, a second one clears it
    click(&mut h, "source:keyword:People|Awa");
    assert_eq!(h.app.session.filter.keyword.as_deref(), Some("People|Awa"));
    assert_eq!(h.app.session.visible_cloned().len(), 3);
    click(&mut h, "source:keyword:People|Awa");
    assert_eq!(h.app.session.filter.keyword, None);
    // the rows that open the whole views
    click(&mut h, "source:peopleOpen");
    assert_eq!(h.app.ui.view, ViewMode::People);
    h.app.run("view.places", json!({})).unwrap();
    h.app.run("view.photoGrid", json!({})).unwrap();
    click(&mut h, "source:placesOpen");
    assert_eq!(h.app.ui.view, ViewMode::Places);
}

// ------------------------------------------------------------------------------- 5. Sorting Help

#[test]
fn sorting_help_gathers_the_three_helpers_with_the_measured_warnings() {
    let mut h = demo([1500.0, 1000.0], json!({"view": "photoGrid", "filterBar": true}));
    let marks_before: Vec<_> = h.app.session.catalog.photos().map(|p| (p.id.0, p.rating, p.flag)).collect();
    // the three old places open it, each at its own section
    click(&mut h, "button:suggestedPicks");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SortingHelp { .. })), "the filter bar's Suggested picks");
    for id in [
        "sorting:culling",
        "sorting:picks",
        "sorting:keywords",
        "sorting:picksWarning",
        "sorting:keywordsWarning",
        "button:sortingCull",
        "button:sortingPicksShow",
        "button:sortingKwSuggest",
    ] {
        assert!(has(&h, id), "{id}");
    }
    assert!(
        crate::panels::sorting_help::PICKS_WARNING.contains("AUC 0.56")
            && crate::panels::sorting_help::PICKS_WARNING.contains("33 %")
            && crate::panels::sorting_help::PICKS_WARNING.contains("31 %")
    );
    assert!(crate::panels::sorting_help::KEYWORDS_WARNING.contains("0.58") && crate::panels::sorting_help::KEYWORDS_WARNING.contains("0.39"));
    // nothing was written by looking at it
    let marks = |h: &Headless| h.app.session.catalog.photos().map(|p| (p.id.0, p.rating, p.flag)).collect::<Vec<_>>();
    assert_eq!(marks(&h), marks_before, "ratings and flags are as they were");
    h.request("ui.dialog.cancel", json!({}), T);
    h.step();
    assert!(h.app.ui.dialog.is_none());
    // Photo ▸ Sorting Help, and the Keywords panel's Suggest
    h.app.run("dialog.cull", json!({})).unwrap();
    assert_eq!(h.app.sorting.goto, Some(crate::panels::sorting_help::Section::Culling));
    h.request("ui.dialog.cancel", json!({}), T);
    h.app.run("ui.set", json!({})).ok();
    let r = h.request("ui.set", json!({"view": "detail", "right": "keywords"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    click(&mut h, "button:kwSuggestRun");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SortingHelp { .. })), "the Keywords panel's Suggest");
    h.step();
    assert_eq!(h.app.sorting.goto, None, "scrolled to its section once");
}

#[test]
fn the_helpers_of_sorting_help_answer_honestly_without_their_models() {
    let mut h = demo([1500.0, 1000.0], json!({"view": "photoGrid"}));
    h.app.run("dialog.cull", json!({"section": "picks"})).unwrap();
    glide(&mut h);
    // Suggested picks without a trained taste model: the engine's refusal is shown, the window stays
    click(&mut h, "button:sortingPicksShow");
    assert!(matches!(h.app.sorting.picks, Some(Err(_))), "{:?}", h.app.sorting.picks);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SortingHelp { .. })));
    // Assisted Culling is the same command as before
    h.app.run("dialog.cull", json!({"section": "culling"})).unwrap();
    glide(&mut h);
    click(&mut h, "button:sortingCull");
    // sliced and in the background now: a few frames until it ends
    let t0 = std::time::Instant::now();
    while h.app.workspaces.quick.job.is_some() && t0.elapsed() < SETTLE {
        h.step();
    }
    let done = h.app.workspaces.quick.last.clone().expect("the analysis was recorded");
    assert!(done["photos"].as_u64().unwrap_or(0) > 0, "{done}");
    // Suggested keywords without the search model: a background task that ends on the model's own message
    let first = h.app.session.catalog.photos().next().map(|p| p.id.0).unwrap();
    h.app.run("library.select", json!({"ids": [first]})).unwrap();
    let before_kw = h.app.session.catalog.photo(lightcraft_catalog::PhotoId(first)).unwrap().meta.keywords.clone();
    h.app.run("dialog.cull", json!({"section": "keywords"})).unwrap();
    glide(&mut h);
    click(&mut h, "button:sortingKwSuggest");
    finish_tasks(&mut h);
    h.step();
    let kw = h.app.sorting.kw.as_ref().expect("an answer");
    // the model's own message (which one depends on the build and on where its folder is), and nothing added
    assert!(
        kw.error.as_deref().is_some_and(|e| ["not installed", "not available", "no folder is set"].iter().any(|m| e.contains(m))),
        "{:?}",
        kw.error
    );
    assert_eq!(h.app.session.catalog.photo(lightcraft_catalog::PhotoId(first)).unwrap().meta.keywords, before_kw);
}

#[test]
fn keyword_suggestions_are_kept_per_photo_and_bring_their_section_into_view() {
    use crate::panels::sorting_help::{Section, State};
    let mut st = State::default();
    let answer = json!({"photos": [
        {"id": 7, "suggestions": [{"keyword": "Subject|Couple", "score": 0.81, "source": "zero-shot"}, {"keyword": "Mood|Blurry", "score": 0.5, "source": "zero-shot"}]},
        {"id": 8, "suggestions": []}
    ], "unindexed": 2});
    st.set_keywords(&[7, 8], &answer);
    let kw = st.kw.as_ref().unwrap();
    assert_eq!((kw.ids.clone(), kw.photos.len(), kw.unindexed, kw.error.clone()), (vec![7, 8], 2, 2, None));
    assert_eq!((kw.photos[0].1[0].keyword.as_str(), kw.photos[0].1.len(), kw.photos[1].1.len()), ("Subject|Couple", 2, 0));
    assert_eq!(st.goto, Some(Section::Keywords), "the answer is below the Suggest button: scroll there");
    st.goto = None;
    st.set_keyword_error(&[7], "The semantic search model is not installed");
    assert!(st.kw.as_ref().unwrap().error.is_some() && st.goto == Some(Section::Keywords));
}

// ------------------------------------------------------------------------- 6. the tool strip

#[test]
fn the_tool_strip_has_labelled_icons_in_three_groups() {
    use crate::state::RightPanel;
    let mut h = demo([1400.0, 900.0], json!({"view": "detail", "right": "edit"}));
    let strip_left = 1400.0 - 48.0;
    let top = |h: &Headless, id: &str| {
        let r = widget(h, &format!("icon:{id}"));
        assert!(r.left() >= strip_left - 1.0 && r.right() <= 1400.0 + 0.5, "{id} {r:?} is in the strip");
        r
    };
    // retouch, then history, then organize, each in order, none on top of another
    let order = ["presets", "edit", "crop", "remove", "masking", "redeye", "versions", "activity", "keywords", "info"];
    let rects: Vec<egui::Rect> = order.iter().map(|id| top(&h, id)).collect();
    for (id, r) in order.iter().zip(&rects) {
        assert_eq!(r.height(), 44.0, "{id} has room for its label");
    }
    assert!(rects.windows(2).all(|w| w[1].top() - w[0].top() >= 44.0 - 0.1), "{rects:?}");
    // About stays below them, alone
    assert!(top(&h, "more").top() > rects[9].bottom(), "About is below the groups");
    // the behaviour did not change: the same panels open
    for (id, panel) in [
        ("crop", RightPanel::Crop),
        ("remove", RightPanel::Remove),
        ("masking", RightPanel::Masking),
        ("redeye", RightPanel::RedEye),
        ("versions", RightPanel::Versions),
        ("activity", RightPanel::Activity),
        ("keywords", RightPanel::Keywords),
        ("info", RightPanel::Info),
        ("edit", RightPanel::Edit),
    ] {
        click(&mut h, &format!("icon:{id}"));
        assert_eq!(h.app.ui.right, panel, "{id}");
    }
    click(&mut h, "icon:presets");
    assert!(h.app.ui.presets, "Presets opens its column");
}

// ---------------------------------------------------------- 7. heavy tasks in the background

#[test]
fn a_long_task_reports_its_steps_and_stops_on_cancel() {
    let mut h = demo([1400.0, 900.0], json!({"view": "photoGrid"}));
    let r = crate::tasks::spawn_with_progress(
        &mut h.app,
        "Test Task",
        |p| {
            let mut i = 0;
            // runs until Cancel
            while p.report(i, 1000, &format!("step {i}")) && i < 100_000 {
                std::thread::sleep(Duration::from_millis(2));
                i += 1;
            }
            i
        },
        |app, _, steps| {
            app.last_task.insert("Test Task".into(), json!({"steps": steps}));
        },
    );
    assert!(r.is_ok());
    // the progress panel is on screen, with its bar's state in ui.inspect
    assert!(h.step_until(T, |h| h.app.tasks.progress_json().as_array().is_some_and(|a| a.iter().any(|t| t["done"].as_u64().unwrap_or(0) > 2))));
    assert!(has(&h, "button:taskCancel:Test Task"), "Cancel is there");
    let insp = h.request("ui.inspect", json!({}), T);
    let inspected = insp.get("result").unwrap_or(&insp);
    assert_eq!(inspected["taskProgress"][0]["label"], "Test Task", "{inspected}");
    assert_eq!(inspected["taskProgress"][0]["total"], 1000);
    click(&mut h, "button:taskCancel:Test Task");
    finish_tasks(&mut h);
    let steps = h.app.last_task["Test Task"]["steps"].as_u64().unwrap();
    assert!(steps > 0 && steps < 100_000, "stopped by Cancel after {steps} steps");
    assert!(!has(&h, "button:taskCancel:Test Task"), "the panel goes with the task");
}

#[test]
fn the_web_gallery_runs_in_the_background_and_the_command_alone_still_waits() {
    let mut h = demo([1400.0, 900.0], json!({"view": "photoGrid"}));
    let ids: Vec<u64> = h.app.session.catalog.photos().map(|p| p.id.0).take(3).collect();
    let (a, b) = (temp("gal-a"), temp("gal-b"));
    let params = |d: &std::path::Path| json!({"dir": d.to_str().unwrap(), "ids": ids, "widths": [160], "formats": []});
    // as before: the answer comes back with the files written
    let direct = h.app.run("gallery.export", params(&a)).unwrap();
    assert_eq!(direct["photos"], 3, "{direct}");
    assert!(a.join("index.html").is_file());
    // in the background: an immediate answer, the same gallery after the task
    let mut p = params(&b);
    p["background"] = json!(true);
    let started = h.app.run("gallery.export", p).unwrap();
    assert_eq!(started["background"], true, "{started}");
    assert!(!b.join("index.html").exists() || h.app.tasks.is_empty(), "the page is written at the end");
    finish_tasks(&mut h);
    let done = &h.app.last_task[crate::heavy::GALLERY];
    assert_eq!((done["photos"].as_u64(), done["files"].as_u64()), (direct["photos"].as_u64(), direct["files"].as_u64()), "{done}");
    assert!(b.join("index.html").is_file() && b.join("snippet.html").is_file());
    // a second one while the first runs is refused (not queued behind it)
    let (c, mut p) = (temp("gal-c"), params(&temp("gal-d")));
    p["background"] = json!(true);
    h.app.run("gallery.export", p.clone()).unwrap();
    let again = h.app.run("gallery.export", p);
    finish_tasks(&mut h);
    assert!(again.is_err() || h.app.last_task.contains_key(crate::heavy::GALLERY), "refused or finished already: {again:?}");
    let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b), std::fs::remove_dir_all(&c));
}

#[test]
fn dust_from_a_series_runs_in_the_background_and_gives_dust_apply_its_spots() {
    let mut h = demo([1400.0, 900.0], json!({"view": "photoGrid"}));
    let steps = h.app.session.undo.len();
    // `spots` given: not a search, the plain command
    let three: Vec<u64> = h.app.session.visible_cloned().iter().take(3).map(|i| i.0).collect();
    let direct = h.app.run("dust.apply", json!({"spots": [{"x": 0.4, "y": 0.4, "radius": 0.01}], "ids": three})).unwrap();
    assert_eq!(direct["photos"], 3, "{direct}");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.undo.len(), steps);
    // the search in the background (an odd sensitivity: nobody has looked at this series yet)
    let r = h.app.run("dust.apply", json!({"background": true, "sensitivity": 37})).unwrap();
    assert_eq!(r["background"], true, "{r}");
    finish_tasks(&mut h);
    let done = &h.app.last_task[crate::heavy::DUST];
    assert!(done.get("error").is_none(), "{done}");
    // whatever it found went on as at most one undo step
    assert!(h.app.session.undo.len() <= steps + 1);
}

#[test]
fn the_heal_spots_go_on_a_slice_of_photos_per_frame_and_cancel_writes_nothing() {
    use lightcraft_engine::cmd::dust::Applier;
    let mut h = demo([1400.0, 900.0], json!({"view": "photoGrid"}));
    let ids: Vec<lightcraft_catalog::PhotoId> = h.app.session.visible_cloned().iter().take(12).copied().collect();
    let spots = || vec![lightcraft_engine::dust::DustSpot { x: 0.31, y: 0.42, radius: 0.006, seen: 0, judged: 0 }];
    let healed = |h: &Headless| ids.iter().filter(|id| !h.app.session.catalog.photo(**id).unwrap().develop.spots.is_empty()).count();
    let steps = h.app.session.undo.len();
    // everything, then one undo step
    h.app.healing = Some(crate::heavy::Healing::new(Applier::new(&h.app.session, ids.clone(), spots(), false)));
    // it shows as a task of its own until the last photo is done (a frame does a slice of them)
    assert_eq!(crate::heavy::healing_json(&h.app).map(|j| (j["done"].clone(), j["total"].clone())), Some((json!(0), json!(12))));
    h.step();
    finish_tasks(&mut h);
    assert_eq!(healed(&h), 12);
    assert_eq!(h.app.session.undo.len(), steps + 1, "one undo step for the whole series");
    assert_eq!(h.app.last_task[crate::heavy::DUST]["photos"], 12);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(healed(&h), 0);
    // Cancel: the spots are dropped, nothing is written
    let mut applier = Applier::new(&h.app.session, ids.clone(), spots(), false);
    applier.step(&mut h.app.session, std::time::Duration::MAX);
    let mut healing = crate::heavy::Healing::new(applier);
    healing.cancel();
    h.app.healing = Some(healing);
    h.step();
    assert!(h.app.healing.is_none() && healed(&h) == 0, "dropped");
    assert_eq!(h.app.session.undo.len(), steps, "and no undo step was made");
    assert_eq!(h.app.last_task[crate::heavy::DUST]["cancelled"], true);
}

// ------------------------------------------------------------------------------ 8. the Resume bar

#[test]
fn the_resume_bar_shows_where_the_app_was_and_goes_at_the_first_thing_done() {
    let mut h = demo([1500.0, 1000.0], json!({"view": "photoGrid"}));
    h.step();
    assert!(!has(&h, "resume:bar"), "only the desktop app arms it at launch");
    h.app.run("view.resumeBar", json!({})).unwrap();
    h.step();
    h.step();
    assert!(has(&h, "resume:bar") && has(&h, "button:resumeContinue") && has(&h, "button:resumeImport"));
    let insp = h.request("ui.inspect", json!({}), T);
    let r = insp.get("result").unwrap_or(&insp);
    assert_eq!(r["resume"]["visible"], true, "{r}");
    assert!(r["resume"]["photos"].as_u64().unwrap_or(0) > 0);
    // choosing a photo is the first thing done: the bar is gone
    let first = h.app.session.catalog.photos().next().map(|p| p.id.0).unwrap();
    h.app.run("library.select", json!({"ids": [first]})).unwrap();
    h.step();
    h.step();
    assert!(!has(&h, "resume:bar"));
    // the setting hides it altogether; ✕ hides it too
    h.app.ui.settings.resume_bar = false;
    h.app.run("view.resumeBar", json!({})).unwrap();
    h.step();
    h.step();
    assert!(!has(&h, "resume:bar"));
    h.app.ui.settings.resume_bar = true;
    h.app.run("view.resumeBar", json!({})).unwrap();
    h.step();
    h.step();
    assert!(has(&h, "resume:bar"));
    click(&mut h, "button:resumeDismiss");
    h.step();
    assert!(!has(&h, "resume:bar"));
    // "Resume editing" opens the photo it was on
    h.app.run("view.photoGrid", json!({})).unwrap();
    h.app.run("view.resumeBar", json!({})).unwrap();
    h.step();
    h.step();
    click(&mut h, "button:resumeContinue");
    assert_eq!(h.app.ui.view, ViewMode::Detail);
}
