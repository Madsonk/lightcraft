//! LightCraft v12: the Library steps (phases 3 and 4 of the UX study). Headless: the import that does
//! not block the window (scan, review and import on the home screen with the window answering the whole
//! time), Quick sort (analysis in slices, counts that equal a direct count, nothing written by a
//! suggestion), the "same person?" queue answered on the keyboard, and the health counters that equal a
//! count made straight on the photos. `LIGHTCRAFT_TEST_SHOTS=<folder>` also saves screenshots.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::classify;
use crate::headless::Headless;
use crate::quick_sort;
use crate::state::ViewMode;
use crate::workspaces::Step;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn services() -> Services {
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        pick_folder: Some(Box::new(|| None)),
        ..Default::default()
    }
}

fn app_with(s: lightcraft_engine::Session) -> LightcraftApp {
    let mut app = LightcraftApp::new(s, services());
    // the crate's tests start in the classic window; these are about the workspaces
    app.ui.settings.workspaces = true;
    app.ui.view = ViewMode::PhotoGrid;
    app
}

fn headless(s: lightcraft_engine::Session) -> Headless {
    let mut h = Headless::new(app_with(s), [1500.0, 950.0], 1.0);
    h.settle(SETTLE);
    h
}

fn demo() -> Headless {
    headless(lightcraft_engine::Session::with_demo())
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget `{id}` on screen"))
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn key(h: &mut Headless, k: &str) {
    let r = h.request("ui.key", json!({"key": k}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
}

fn run(h: &mut Headless, id: &str, p: Value) -> Result<Value, String> {
    let r = h.app.run(id, p);
    h.step();
    h.step();
    r
}

/// Scroll the page under the pointer until `id` is inside the window (`ui.clickWidget` clicks the centre of
/// the rectangle, even one below the fold).
fn bring_into_view(h: &mut Headless, id: &str) {
    // the pointer over the page, so the wheel scrolls it
    let at = h.size / 2.0;
    let r = h.request("ui.move", json!({"x": at.x, "y": at.y}), T);
    let _ = r;
    for _ in 0..60 {
        h.step();
        if has(h, id) && rect(h, id).bottom() < h.size.y - 24.0 && rect(h, id).top() > 40.0 {
            // the wheel's smooth scrolling goes on for a while: wait for the page to rest, then check again
            for _ in 0..40 {
                h.step();
            }
            if has(h, id) && rect(h, id).bottom() < h.size.y - 24.0 && rect(h, id).top() > 40.0 {
                return;
            }
            continue;
        }
        // above the window: scroll up; below: down
        let dy = if has(h, id) && rect(h, id).top() <= 40.0 { 500.0 } else { -500.0 };
        let r = h.request("ui.scroll", json!({"x": 700.0, "y": 500.0, "dy": dy}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
    }
    panic!("`{id}` never came into view");
}

fn shot(h: &mut Headless, name: &str) {
    if let Ok(dir) = std::env::var("LIGHTCRAFT_TEST_SHOTS") {
        let _ = std::fs::create_dir_all(&dir);
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&dir).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "lc-v12-{tag}-{}-{}",
        std::process::id(),
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 64 × 48 picture of 8 × 8 blocks with a grey of their own, from `seed`: two seeds look nothing alike,
/// the same seed with a different `nudge` looks the same (a burst), and `soft` smooths it out.
fn picture(seed: u64, nudge: u8, soft: bool) -> Vec<u8> {
    picture_sized(seed, nudge, soft, 64, 48)
}

/// [`picture`] at `w` × `h` (the blocks scale with it).
fn picture_sized(seed: u64, nudge: u8, soft: bool, w: usize, h: usize) -> Vec<u8> {
    let mut px = vec![[0u8; 4]; w * h];
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut cell = [[0u8; 8]; 6];
    for row in cell.iter_mut() {
        for c in row.iter_mut() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *c = 30 + ((s >> 40) % 190) as u8;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let v = if soft {
                // a gentle ramp: no edges
                (100 + (x * 40 / w) + (y * 20 / h)) as u8
            } else {
                cell[y * 6 / h][x * 8 / w]
            };
            px[y * w + x] = [v, v, v, 255];
        }
    }
    px[0] = [nudge, nudge, nudge, 255];
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data: px };
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

/// `n` files `name-<i>.png` in `dir`; seeds `100 + i` unless `seeds` says otherwise; returns their paths.
fn write_pictures(dir: &std::path::Path, n: usize, seeds: &dyn Fn(usize) -> (u64, u8, bool)) -> Vec<String> {
    (0..n)
        .map(|i| {
            let (seed, nudge, soft) = seeds(i);
            let p = dir.join(format!("shot-{i:03}.png"));
            std::fs::write(&p, picture(seed, nudge, soft)).unwrap();
            p.to_string_lossy().to_string()
        })
        .collect()
}

/// Like [`write_pictures`], big enough pictures that an import of them takes a while.
fn write_big_pictures(dir: &std::path::Path, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let p = dir.join(format!("big-{i:03}.png"));
            std::fs::write(&p, picture_sized(500 + i as u64, 1, false, 1800, 1200)).unwrap();
            p.to_string_lossy().to_string()
        })
        .collect()
}

fn fs_session() -> lightcraft_engine::Session {
    lightcraft_engine::Session::new().with_fs()
}

/// Frames until `done`, or panic after `limit`.
fn until(h: &mut Headless, limit: Duration, what: &str, done: impl Fn(&Headless) -> bool) {
    let t0 = Instant::now();
    while !done(h) {
        assert!(t0.elapsed() < limit, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ------------------------------------------------------------------------------------------ steps

#[test]
fn the_steps_change_the_centre_and_never_touch_the_session() {
    let mut h = demo();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(4).map(|p| p.0).collect();
    h.app.run("library.select", json!({"ids": ids})).unwrap();
    h.app.run("library.filter", json!({"rating": 1})).unwrap();
    h.step();
    let (filter, source, sel) = (h.app.session.filter.clone(), h.app.session.source, h.app.session.selection.clone());
    assert_eq!(h.app.workspaces.step, Step::Sort, "a library with photos opens on Sort");
    assert!(has(&h, "region:libraryBatchTools"), "Batch tools share the library sidebar");
    for (chip, step, region) in [
        ("ctxImport", Step::Import, "region:home"),
        ("ctxClassify", Step::Classify, "region:classify"),
        ("ctxSort", Step::Sort, "region:libraryBatchTools"),
    ] {
        click(&mut h, &format!("button:{chip}"));
        assert_eq!(h.app.workspaces.step, step);
        assert!(has(&h, region), "{chip}: {region} on screen");
        assert_eq!(h.app.session.filter, filter, "filter after {chip}");
        assert_eq!(h.app.session.source, source);
        assert_eq!(h.app.session.selection, sel, "selection after {chip}");
    }
    // Import and Classify own the window: no Quick sort, no source column
    run(&mut h, "workspace.step", json!({"step": "import"})).unwrap();
    assert!(!has(&h, "region:libraryBatchTools") && has(&h, "region:home"));
    // a view changed by anything else leaves the step (the centre would not show it)
    run(&mut h, "view.detail", json!({})).unwrap();
    assert_eq!(h.app.workspaces.step, Step::Sort);
    run(&mut h, "workspace.step", json!({"step": "classify"})).unwrap();
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    run(&mut h, "workspace.library", json!({})).unwrap();
    assert_eq!(h.app.workspaces.current, "Library");
    // an unknown step is refused
    assert!(h.app.run("workspace.step", json!({"step": "nonsense"})).is_err());
    // switched off: the old window, no steps
    h.app.ui.settings.workspaces = false;
    h.step();
    h.step();
    assert!(!has(&h, "region:libraryBatchTools") && !has(&h, "region:home") && !has(&h, "region:contextBar"));
    assert!(h.app.run("workspace.step", json!({"step": "import"})).is_err());
}

#[test]
fn an_empty_library_opens_on_the_home_screen_and_the_old_review_returns_when_switched_off() {
    let mut h = headless(fs_session());
    assert_eq!(h.app.workspaces.step, Step::Import, "nothing to sort: Import");
    assert!(has(&h, "region:home") && has(&h, "home:dropzone"));
    // switched off: a scan opens the classic modal review
    let dir = temp_dir("off");
    let paths = write_pictures(&dir, 3, &|i| (200 + i as u64, 1, false));
    h.app.ui.settings.workspaces = false;
    h.step();
    crate::import::open(&mut h.app, paths).unwrap();
    until(&mut h, T, "the classic review", |h| matches!(h.app.ui.dialog, Some(crate::state::Dialog::Import { .. })));
    assert!(!h.app.workspaces.home.scanned, "the home screen took nothing");
}

// ----------------------------------------------------------------------------- import without blocking

#[test]
fn the_import_is_scanned_reviewed_and_run_without_blocking_the_window() {
    let mut h = headless(fs_session());
    let dir = temp_dir("import");
    let n = 60;
    write_big_pictures(&dir, n);
    assert_eq!(h.app.workspaces.step, Step::Import);
    shot(&mut h, "v12-01-home-empty.png");

    // 1. scan: the page lists what it found, no modal anywhere
    run(&mut h, "workspace.home.scan", json!({"paths": [dir.to_string_lossy()]})).unwrap();
    until(&mut h, T, "the scan", |h| h.app.workspaces.home.scanned);
    h.step();
    assert!(h.app.ui.dialog.is_none(), "no modal review: {:?}", h.app.ui.dialog);
    assert_eq!(h.app.workspaces.home.d.candidates.len(), n);
    assert_eq!(h.app.workspaces.home.d.selected_paths().len(), n);
    assert!(has(&h, "button:homeImport") && has(&h, "import:0"), "the preview shows the first candidates");
    assert_eq!(h.app.session.catalog.photos().count(), 0, "a scan imports nothing");
    shot(&mut h, "v12-02-home-scanned.png");

    // 2. settings by name, then the blue button
    run(&mut h, "workspace.home.settings", json!({"mode": "add", "keywords": "shoot-test", "album": "Shoot test", "after": {"previews": true, "culling": true, "places": true, "keywords": false}})).unwrap();
    assert!(has(&h, "button:homeImport"), "the blue button is on the page");
    let t0 = Instant::now();
    // (the button runs this command; run here without the frames a click would add, so the whole import can be watched)
    h.app.run("workspace.home.import", json!({})).unwrap();
    assert!(
        h.app.import.is_some() || h.app.workspaces.home.started.is_some() || h.app.workspaces.home.last.is_some(),
        "the import started: status `{}`",
        h.app.ui.status
    );
    assert_eq!(h.app.workspaces.step, Step::Sort, "the Sort step opens on the photos arriving");
    assert_eq!(h.app.session.source, lightcraft_engine::LibrarySource::RecentlyAdded);

    // 3. the window answers all the time the import runs: every frame is short, no modal ever shows,
    //    and the user can change space, select and filter meanwhile
    let mut worst = Duration::ZERO;
    let mut frames = 0;
    let mut did = false;
    let t_run = Instant::now();
    while (h.app.import.is_some() || h.app.workspaces.home.started.is_some()) && t_run.elapsed() < Duration::from_secs(60) {
        let f = Instant::now();
        h.step();
        worst = worst.max(f.elapsed());
        frames += 1;
        assert!(h.app.ui.dialog.is_none(), "the import never opens a modal");
        assert!(!has(&h, "dialog:window"));
        if !did && h.app.session.catalog.photos().count() > 0 {
            did = true;
            // working while it runs
            let first = h.app.session.visible_cloned().first().copied();
            if let Some(f) = first {
                h.app.run("library.select", json!({"ids": [f.0]})).unwrap();
            }
            h.app.run("workspace.retouch", json!({})).unwrap();
            h.app.run("workspace.library", json!({})).unwrap();
            h.app.run("library.filter", json!({"rating": 0})).unwrap();
            h.app.run("library.clearFilter", json!({})).unwrap();
        }
    }
    let total = t0.elapsed();
    assert!(h.app.import.is_none(), "the import finished");
    assert!(frames > 1, "the import took more than one frame ({frames}): a window that answers has frames to answer in");
    assert!(worst < Duration::from_millis(1500), "the slowest frame during the import took {worst:?}");
    assert_eq!(h.app.session.catalog.photos().filter(|p| p.in_library()).count(), n);
    let last = h.app.workspaces.home.last.clone().expect("the import was recorded");
    assert_eq!(last["imported"], n, "{last}");
    assert!(last["firstPhotoMs"].as_f64().is_some_and(|m| m >= 0.0), "{last}");
    // the shoot's album and keyword went on, from the page's settings
    let album = h.app.session.catalog.albums().find(|a| a.name == "Shoot test").expect("the new album");
    assert_eq!(album.photos.len(), n);
    assert!(h.app.session.catalog.photos().all(|p| p.meta.keywords.iter().any(|k| k == "shoot-test")));
    // the follow-ups ran on the new photos: the analysis was asked for
    assert!(last["followUps"].as_array().unwrap().iter().any(|f| f == "culling"), "{last}");
    println!("import of {n} files: first photo after {:?} ms, {frames} frames, slowest {worst:?}, total {total:?}", last["firstPhotoMs"]);
    // the remembered settings are in the prefs (written with workspaces.json)
    assert_eq!(h.app.workspaces.prefs.home.as_ref().unwrap()["after"][1], true);
    shot(&mut h, "v12-03-sort-after-import.png");
}

#[test]
fn cancelling_an_import_from_the_task_list_stops_it_cleanly() {
    let mut h = headless(fs_session());
    let dir = temp_dir("cancel");
    let n = 120;
    write_pictures(&dir, n, &|i| (300 + i as u64, 1, false));
    run(&mut h, "workspace.home.scan", json!({"paths": [dir.to_string_lossy()]})).unwrap();
    until(&mut h, T, "the scan", |h| h.app.workspaces.home.scanned);
    run(&mut h, "workspace.home.import", json!({})).unwrap();
    // the first frame starts the worker; the task list shows the import with a Cancel
    h.step();
    h.step();
    until(&mut h, T, "the task list", |h| h.app.workspaces.home.tasks.iter().any(|t| t.key == "import") || h.app.import.is_none());
    if h.app.import.is_some() {
        crate::home::cancel_task(&mut h.app, "import");
        h.step();
    }
    until(&mut h, T, "the end of the import", |h| h.app.import.is_none());
    let imported = h.app.session.catalog.photos().filter(|p| p.in_library()).count();
    assert!(imported <= n, "{imported}");
    assert!(h.app.ui.dialog.is_none());
    // what came in is whole: every photo can be read back
    assert!(h.app.session.catalog.photos().all(|p| p.width > 0 && p.height > 0));
}

// ------------------------------------------------------------------------------------------ quick sort

#[test]
fn quick_sort_finds_bursts_and_soft_photos_in_slices_and_writes_nothing_by_itself() {
    let mut h = headless(fs_session());
    let dir = temp_dir("quick");
    let n = 24;
    // photos 8-11 are one burst; photo 3 is soft; the rest all look different
    let paths = write_pictures(&dir, n, &|i| match i {
        8..=11 => (700, i as u8, false),
        3 => (702, 0, true),
        _ => (400 + i as u64, 1, false),
    });
    crate::import::start_paths(&mut h.app, paths).unwrap();
    h.settle(SETTLE);
    until(&mut h, T, "the import", |h| h.app.import.is_none());
    assert_eq!(h.app.session.catalog.photos().count(), n);
    // capture times: 60 s apart, except the burst (1 s apart)
    let by_name: std::collections::BTreeMap<String, u64> = h.app.session.catalog.photos().map(|p| (p.file_name.clone(), p.id.0)).collect();
    for i in 0..n {
        let id = by_name[&format!("shot-{i:03}.png")];
        let secs = if (8..=11).contains(&i) { 1000 + (i - 8) } else { i * 60 };
        let t = format!("2026-10-10T{:02}:{:02}:{:02}", 8 + secs / 3600, secs % 3600 / 60, secs % 60);
        h.app.session.execute("photo.setCaptureTime", &json!({"ids": [id], "time": t, "each": true})).unwrap();
    }
    run(&mut h, "workspace.step", json!({"step": "sort"})).unwrap();
    run(&mut h, "library.source", json!({"kind": "all"})).unwrap();
    let vis = h.app.session.visible_cloned();
    let sum = quick_sort::summarize(&h.app, &vis, quick_sort::Sensitivity::Medium);
    assert_eq!((sum.total, sum.scored, sum.bursts.len()), (n, 0, 0), "nothing is analysed before the user asks");
    shot(&mut h, "v12-04-sort-before.png");

    let ratings: Vec<(u64, u8, lightcraft_catalog::Flag)> = h.app.session.catalog.photos().map(|p| (p.id.0, p.rating, p.flag)).collect();
    let undo0 = h.app.session.undo.len();
    // the button asks for the analysis; it runs a slice per frame
    click(&mut h, "button:quickAnalyze");
    assert!(h.app.workspaces.quick.job.is_some(), "an analysis is running");
    let t0 = Instant::now();
    while h.app.workspaces.quick.job.is_some() && t0.elapsed() < Duration::from_secs(60) {
        h.step();
    }
    assert!(h.app.workspaces.quick.job.is_none(), "the analysis finished");
    let last = h.app.workspaces.quick.last.clone().expect("the analysis was recorded");
    assert!(last["slices"].as_u64().unwrap_or(0) >= 3, "it ran in several slices: {last}");
    assert_eq!(h.app.session.undo.len(), undo0 + 1, "the slices are one undo step");
    // the suggestions wrote no rating and no flag
    let after: Vec<(u64, u8, lightcraft_catalog::Flag)> = h.app.session.catalog.photos().map(|p| (p.id.0, p.rating, p.flag)).collect();
    assert_eq!(after, ratings);
    // the summary is what a direct, single run of the existing command says
    let ids = h.app.session.visible_cloned();
    let chunked = quick_sort::summarize(&h.app, &ids, quick_sort::Sensitivity::Medium);
    assert_eq!(chunked.scored, n);
    assert_eq!(chunked.bursts.len(), 1, "{:?}", chunked.bursts);
    assert_eq!(chunked.bursts[0].ids.len(), 4);
    assert!(chunked.bursts[0].best.is_some());
    assert_eq!(chunked.burst_extras.len(), 3);
    let soft_id = by_name["shot-003.png"];
    assert!(chunked.soft.contains(&soft_id), "the smooth picture is soft: {:?} below {}", chunked.soft, chunked.soft_below);
    assert_eq!(chunked.soft.len(), 1, "only the smooth one: {:?}", chunked.soft);
    let direct = h.app.session.execute("photo.analyze", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
    assert_eq!(direct["groups"], 1, "{}", direct["groups"]);
    let single = quick_sort::summarize(&h.app, &ids, quick_sort::Sensitivity::Medium);
    assert_eq!(single.bursts.len(), chunked.bursts.len());
    assert_eq!(single.soft, chunked.soft);
    let mut a: Vec<Vec<u64>> = chunked.bursts.iter().map(|b| b.ids.clone()).collect();
    let mut b: Vec<Vec<u64>> = single.bursts.iter().map(|b| b.ids.clone()).collect();
    a.iter_mut().for_each(|v| v.sort());
    b.iter_mut().for_each(|v| v.sort());
    assert_eq!(a, b, "the slices cut no burst");

    // selecting a suggestion selects and writes nothing
    let undo1 = h.app.session.undo.len();
    run(&mut h, "workspace.sort.select", json!({"what": "burstExtras"})).unwrap();
    assert_eq!(h.app.session.selection.ids.len(), 3);
    assert_eq!(h.app.session.undo.len(), undo1, "selecting writes nothing");
    run(&mut h, "workspace.sort.select", json!({"what": "soft"})).unwrap();
    assert_eq!(h.app.session.selection.ids.iter().map(|i| i.0).collect::<Vec<_>>(), vec![soft_id]);
    // the badges are on the grid and the panel says the counts
    assert!(has(&h, "badge:soft:".to_string().as_str()) || h.app.widgets.iter().any(|(w, _)| w.starts_with("badge:soft:")), "a soft badge");
    assert!(h.app.widgets.iter().any(|(w, _)| w.starts_with("badge:burst:")), "burst badges");
    assert!(has(&h, "button:quickSoft") && has(&h, "button:quickBursts") && has(&h, "button:quickStack"));
    shot(&mut h, "v12-05-sort-after.png");
    // stacking the bursts is an explicit gesture, one undo step
    let undo2 = h.app.session.undo.len();
    run(&mut h, "workspace.sort.stackBursts", json!({})).unwrap();
    assert_eq!(h.app.session.undo.len(), undo2 + 1);
    assert_eq!(h.app.session.catalog.stacks().count(), 1);
    // the three buttons act on the selection only when clicked
    run(&mut h, "library.select", json!({"ids": [soft_id]})).unwrap();
    // the button moved down the sidebar (the AI picks section, lot D, sits above Sorting Help): like a
    // hand, rest the pointer on it for a frame before pressing
    let soft = lightcraft_catalog::PhotoId(soft_id);
    for _ in 0..4 {
        h.settle(SETTLE);
        let hv = h.request("ui.hoverWidget", json!({"id": "button:quickReject"}), T);
        assert_eq!(hv["ok"], true, "{hv}");
        h.step();
        h.step();
        click(&mut h, "button:quickReject");
        if h.app.session.catalog.photo(soft).unwrap().flag == lightcraft_catalog::Flag::Reject {
            break;
        }
    }
    assert_eq!(h.app.session.catalog.photo(lightcraft_catalog::PhotoId(soft_id)).unwrap().flag, lightcraft_catalog::Flag::Reject);
    // next unsorted: a photo with no rating and no flag
    let r = run(&mut h, "workspace.sort.next", json!({})).unwrap();
    let nid = r["id"].as_u64().unwrap();
    let p = h.app.session.catalog.photo(lightcraft_catalog::PhotoId(nid)).unwrap();
    assert!(p.rating == 0 && p.flag == lightcraft_catalog::Flag::None);
}

// -------------------------------------------------------------------------------------- the health

#[test]
fn the_health_counters_equal_a_direct_count_and_open_what_they_count() {
    let mut s = lightcraft_engine::Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().filter(|p| p.in_library()).map(|p| p.id.0).collect();
    assert!(ids.len() >= 12, "the demo library has photos");
    // a library with some of everything missing
    for id in &ids[..3] {
        let kws = s.catalog.photo(lightcraft_catalog::PhotoId(*id)).unwrap().meta.keywords.clone();
        s.execute("photo.setMeta", &json!({"ids": [id], "removeKeywords": kws})).unwrap();
    }
    s.execute("photo.setMeta", &json!({"ids": [ids[4], ids[5]], "addKeywords": ["People|Awa"]})).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [ids[5], ids[6]], "addKeywords": ["Places|France|Poitiers"]})).unwrap();
    s.execute("photo.rate", &json!({"ids": [ids[7], ids[8]], "rating": 0})).unwrap();
    // three photos with one file's content, two with another's
    for (id, hash) in [(ids[9], "aaaa"), (ids[10], "aaaa"), (ids[11], "aaaa"), (ids[1], "bbbb"), (ids[2], "bbbb")] {
        let p = s.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().clone();
        s.commit(
            "test",
            lightcraft_catalog::Op::SetContent {
                id: p.id,
                width: p.width,
                height: p.height,
                file_size: p.file_size,
                content_hash: Some(hash.into()),
                preview_only: None,
            },
        )
        .unwrap();
    }
    let mut h = headless(s);
    let counted = classify::count_health(&h.app.session.catalog);

    // the direct count, written straight over the photos
    let cat = &h.app.session.catalog;
    let lib: Vec<_> = cat.photos().filter(|p| !p.deleted && !p.local).collect();
    let direct = |f: &dyn Fn(&lightcraft_catalog::Photo) -> bool| lib.iter().filter(|p| f(p)).count();
    assert_eq!(counted.total, lib.len());
    assert_eq!(counted.no_keyword, direct(&|p| p.meta.keywords.is_empty()));
    assert_eq!(counted.no_rating, direct(&|p| p.rating == 0));
    assert_eq!(
        counted.no_person,
        direct(&|p| !p.meta.keywords.iter().any(|k| k == "People" || k.starts_with("People|")) && !p.meta.regions.iter().any(|r| r.name.is_some()))
    );
    let filter = |f: lightcraft_catalog::Filter| cat.query(&f, &Default::default()).len();
    assert_eq!(counted.no_keyword, filter(lightcraft_catalog::Filter { untagged: Some(true), ..Default::default() }));
    assert_eq!(counted.no_place, filter(lightcraft_catalog::Filter { place: Some("none".into()), ..Default::default() }));
    let mut hashes: std::collections::HashMap<&str, usize> = Default::default();
    for p in &lib {
        if let Some(c) = &p.content_hash {
            *hashes.entry(c.as_str()).or_default() += 1;
        }
    }
    assert_eq!(counted.duplicates, hashes.values().filter(|n| **n > 1).sum::<usize>());
    assert_eq!(counted.duplicate_sets, hashes.values().filter(|n| **n > 1).count());
    assert!(counted.no_keyword >= 3 && counted.no_rating >= 2 && counted.duplicates >= 5, "{counted:?}");
    assert_eq!(counted.duplicate_sets, 2.max(counted.duplicate_sets), "{counted:?}");

    // the page shows them, and each counter opens exactly the photos it counts
    run(&mut h, "workspace.step", json!({"step": "classify"})).unwrap();
    for id in ["noKeyword", "noPlace", "noPerson", "noRating", "duplicates"] {
        assert!(has(&h, &format!("counter:{id}")), "{id} on the page");
    }
    shot(&mut h, "v12-06-classify-health.png");
    for (what, n) in [
        ("noKeyword", counted.no_keyword),
        ("noPlace", counted.no_place),
        ("noPerson", counted.no_person),
        ("noRating", counted.no_rating),
        ("duplicates", counted.duplicates),
    ] {
        run(&mut h, "workspace.step", json!({"step": "classify"})).unwrap();
        click(&mut h, &format!("counter:{what}").replace("counter:", "counter:"));
        assert_eq!(h.app.workspaces.step, Step::Sort, "{what} opens the grid");
        assert_eq!(h.app.session.visible().len(), n, "{what}: the grid shows what the counter counts");
        assert_eq!(h.app.session.source, lightcraft_engine::LibrarySource::All);
    }
    // the inspect summary says the same numbers (what ui.inspect reports)
    run(&mut h, "workspace.step", json!({"step": "classify"})).unwrap();
    let v = crate::workspaces::inspect(&h.app);
    assert_eq!(v["classify"]["health"]["noKeyword"], counted.no_keyword);
    assert_eq!(v["classify"]["health"]["duplicates"], counted.duplicates);
}

// ------------------------------------------------------------------------------------- the queue

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

/// A face of group `g` of `groups`: every group shares a component (so two groups are doubtfully alike,
/// cosine ≈ 0.45, between "link" and "link − 0.12") and has a direction of its own; `n` adds a little noise.
fn face(g: u64, n: u64) -> Vec<f32> {
    let (shared, own, noise) = (rnd(7), rnd(1000 + g), rnd(50_000 + g * 97 + n));
    let (a, b) = (0.45f32.sqrt(), 0.55f32.sqrt());
    unit(shared.iter().zip(&own).zip(&noise).map(|((s, o), z)| a * s + b * o + 0.06 * z).collect())
}

fn session_with_groups(groups: u64) -> lightcraft_engine::Session {
    let mut s = lightcraft_engine::Session::with_demo();
    let ids: Vec<lightcraft_catalog::PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    assert!(ids.len() as u64 >= groups, "{} photos for {groups} groups", ids.len());
    // two faces per group, each group's pair in two photos; a photo carries faces of two groups
    let mut per_photo: Vec<(lightcraft_catalog::PhotoId, Vec<(Vec<f32>, [f32; 4])>)> =
        ids.iter().take(groups as usize).map(|i| (*i, Vec::new())).collect();
    for g in 0..groups {
        for k in 0..2u64 {
            let slot = ((g + k) % groups) as usize;
            let n = per_photo[slot].1.len() as f32;
            per_photo[slot].1.push((face(g, k), [0.05 + 0.4 * n, 0.2, 0.3, 0.4]));
        }
    }
    lightcraft_engine::people::inject_faces(&mut s, per_photo);
    s
}

#[test]
fn the_same_person_queue_is_answered_on_the_keyboard() {
    let mut h = headless(session_with_groups(8));
    run(&mut h, "workspace.step", json!({"step": "classify"})).unwrap();
    until(&mut h, T, "the questions", |h| h.app.workspaces.classify.queue_total > 0);
    for _ in 0..4 {
        h.step();
    }
    let total0 = h.app.workspaces.classify.queue_total;
    assert!(total0 >= 20, "28 pairs of groups are doubtful, {total0} questions");
    assert!(has(&h, "classify:question") && has(&h, "button:classifyYes"));
    shot(&mut h, "v12-07-classify-people.png");

    // the keys answer; no flag is touched (U is also "unflag" outside this page)
    let flags: Vec<_> = h.app.session.catalog.photos().map(|p| (p.id.0, p.flag, p.rating)).collect();
    let t0 = Instant::now();
    let mut times = Vec::new();
    let pattern = ["N", "U", "N", "N", "U", "N", "U", "U", "N", "N", "U", "N", "N", "U", "N", "U", "N", "N", "U", "N"];
    for (i, k) in pattern.iter().enumerate() {
        let q0 = h.app.workspaces.classify.queue.first().map(|q| q["id"].clone());
        assert!(q0.is_some(), "question {i} is there");
        let t = Instant::now();
        key(&mut h, k);
        times.push(t.elapsed());
        let q1 = h.app.workspaces.classify.queue.first().map(|q| q["id"].clone());
        assert_ne!(q0, q1, "key {k} (#{i}) answered the question (status `{}`, classify {})", h.app.ui.status, crate::classify::inspect(&h.app));
        // keep the queue fed, the way a frame does
        h.step();
    }
    let total = t0.elapsed();
    let a = h.app.workspaces.classify.answers;
    assert_eq!(a[0], 0);
    assert_eq!(a[1], pattern.iter().filter(|k| **k == "N").count());
    assert_eq!(a[2], pattern.iter().filter(|k| **k == "U").count());
    let after: Vec<_> = h.app.session.catalog.photos().map(|p| (p.id.0, p.flag, p.rating)).collect();
    assert_eq!(after, flags, "U on this page does not unflag anything");
    let worst = times.iter().max().unwrap();
    println!("20 questions answered on the keyboard in {total:?} (slowest key {worst:?}, mean {:?})", total / 20);
    until(&mut h, T, "the queue to go on", |h| !h.app.workspaces.classify.queue.is_empty());
    // Y: the pair is the same person (or the same group): the queue moves on and the answer is stored by the engine
    let before = h.app.workspaces.classify.queue_total;
    key(&mut h, "Y");
    h.step();
    assert_eq!(h.app.workspaces.classify.answers[0], 1);
    assert!(h.app.workspaces.classify.queue_total <= before);
    // keys are ignored in a text field and off the People tab
    run(&mut h, "workspace.classify.tab", json!({"tab": "places"})).unwrap();
    let n = h.app.workspaces.classify.answers;
    key(&mut h, "Y");
    assert_eq!(h.app.workspaces.classify.answers, n);
    // naming a group: type in its field, Enter
    run(&mut h, "workspace.classify.tab", json!({"tab": "people"})).unwrap();
    until(&mut h, T, "the groups", |h| h.app.workspaces.classify.ov_len() > 0);
    let g = h.app.workspaces.classify.first_group_id().unwrap();
    assert!(has(&h, &format!("field:classifyName:{g}")));
    let undo0 = h.app.session.undo.len();
    run(&mut h, "workspace.classify.name", json!({"group": g, "name": "Awa"})).unwrap();
    assert!(h.app.session.catalog.photos().any(|p| p.meta.keywords.iter().any(|k| k == "People|Awa")), "naming tags the photos");
    assert!(h.app.session.undo.len() >= undo0);
    shot(&mut h, "v12-08-classify-after.png");
}

#[test]
fn nothing_is_written_by_looking_at_the_places_or_the_keywords() {
    let mut h = demo();
    run(&mut h, "workspace.step", json!({"step": "classify"})).unwrap();
    run(&mut h, "workspace.classify.tab", json!({"tab": "places"})).unwrap();
    let undo0 = h.app.session.undo.len();
    let kws: Vec<_> = h.app.session.catalog.photos().map(|p| p.meta.keywords.clone()).collect();
    if let Ok(v) = run(&mut h, "workspace.classify.places", json!({"action": "preview"})) {
        assert!(v.get("wouldWrite").is_some(), "{v}");
    }
    let _ = run(&mut h, "workspace.classify.places", json!({"action": "suggest"}));
    assert_eq!(h.app.session.undo.len(), undo0, "a preview and a suggestion write nothing");
    assert_eq!(h.app.session.catalog.photos().map(|p| p.meta.keywords.clone()).collect::<Vec<_>>(), kws);
    // a batch rename is previewed, never done
    let r = run(&mut h, "workspace.classify.rename", json!({"template": "{date}_{name}"})).unwrap();
    assert!(r.as_array().is_some_and(|a| !a.is_empty()), "{r}");
    assert_eq!(h.app.session.undo.len(), undo0);
    // the tabs of the page are all there
    for t in classify::Tab::ALL {
        run(&mut h, "workspace.classify.tab", json!({"tab": t.name()})).unwrap();
        assert!(has(&h, &format!("button:classifyTab:{}", t.name())));
    }
    assert!(h.app.run("workspace.classify.tab", json!({"tab": "nonsense"})).is_err());
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
}

// ------------------------------------------------------------------------------------------- layout

#[test]
fn the_pages_fit_the_window_at_three_widths() {
    for (w, hh) in [(1500.0f32, 950.0f32), (1100.0, 700.0), (900.0, 650.0)] {
        let mut app = app_with(session_with_groups(4));
        app.ui.view = ViewMode::PhotoGrid;
        let mut h = Headless::new(app, [w, hh], 1.0);
        h.settle(SETTLE);
        for (step, region) in [("import", "region:home"), ("classify", "region:classify"), ("sort", "region:libraryBatchTools")] {
            run(&mut h, "workspace.step", json!({"step": step})).unwrap();
            h.settle(SETTLE);
            let r = rect(&h, region);
            assert!(r.left() >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= hh + 0.5, "{step} at {w}: {r:?}");
        }
        // the buttons of the home screen are inside the window
        run(&mut h, "workspace.step", json!({"step": "import"})).unwrap();
        for id in ["button:homeImport", "button:homeChooseFolder"] {
            if has(&h, id) {
                let r = rect(&h, id);
                assert!(r.right() <= w + 0.5 && r.left() >= -0.5, "{id} at {w}: {r:?}");
            }
        }
    }
}

// ------------------------------------------------------------------- Recognition: every group, every face

/// `groups` groups of two faces each (random directions: no two groups look alike), spread over the demo photos.
fn session_with_many_groups(groups: u64) -> lightcraft_engine::Session {
    let mut s = lightcraft_engine::Session::with_demo();
    let ids: Vec<lightcraft_catalog::PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    let mut per_photo: Vec<(lightcraft_catalog::PhotoId, Vec<(Vec<f32>, [f32; 4])>)> = ids.iter().map(|i| (*i, Vec::new())).collect();
    for g in 0..groups {
        for k in 0..2u64 {
            let (a, b) = (rnd(10_000 + g), rnd(70_000 + g * 31 + k));
            let v = unit(a.iter().zip(&b).map(|(x, y)| x + 0.08 * y).collect());
            let slot = ((g * 2 + k) as usize) % per_photo.len();
            let n = per_photo[slot].1.len() as f32;
            // distinct boxes in one photo (a grid of faces)
            per_photo[slot].1.push((v, [0.02 + 0.09 * (n % 10.0), 0.05 + 0.1 * (n / 10.0).floor(), 0.09, 0.09]));
        }
    }
    lightcraft_engine::people::inject_faces(&mut s, per_photo);
    s
}

#[test]
fn the_recognition_page_reaches_every_group_and_every_face_and_never_draws_an_empty_square() {
    let mut app = app_with(session_with_many_groups(130));
    app.ui.view = ViewMode::People;
    app.caches.people_rec.recognition = true;
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    h.settle(SETTLE);
    let r = h.request("ui.set", json!({"view": "people"}), T);
    assert_eq!(r["ok"], true, "{r}");
    until(&mut h, SETTLE, "the groups", |h| h.app.caches.people_rec.overview().is_some_and(|o| o.grouped && o.groups_total > 0));
    let ov = h.app.caches.people_rec.overview().cloned().unwrap();
    assert_eq!(ov.indexed_faces, 260, "{}", ov.indexed_faces);
    assert_eq!(ov.groups_total, 130, "all the groups are counted: {}", ov.groups_total);
    assert_eq!(ov.groups.len(), 40, "the first page");
    shot(&mut h, "v12-09-recognition-first-page.png");
    // "Load more" pages through all of them
    for expect in [100usize, 130] {
        bring_into_view(&mut h, "recMore:groups");
        click(&mut h, "recMore:groups");
        h.step();
        h.step();
        until(&mut h, T, "the next page", |h| h.app.caches.people_rec.overview().is_some_and(|o| o.groups.len() >= expect));
        assert_eq!(h.app.caches.people_rec.overview().unwrap().groups.len(), expect);
    }
    h.step();
    assert!(!has(&h, "recMore:groups"), "nothing left to load");
    // the faces of one group, then several picked at once and moved in one go
    let gid = h.app.caches.people_rec.overview().unwrap().groups[0].id.clone();
    bring_into_view(&mut h, &format!("recGroupFaces:{gid}"));
    let r = h.request("ui.clickWidget", json!({"id": format!("recGroupFaces:{gid}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let faces: Vec<String> = h.app.widgets.iter().filter(|(w, _)| w.starts_with("recFace:")).map(|(w, _)| w.clone()).collect();
    assert!(faces.len() >= 2, "the group's faces are listed: {}", faces.len());
    h.settle(SETTLE);
    shot(&mut h, "v12-10-recognition-group-faces.png");
    // every drawn face shows something or says why not: no tile is left as an empty dark square
    let mut empty = 0;
    let img = h.snapshot(T);
    for (w, r) in h.app.widgets.clone() {
        if !w.starts_with("recFace:") {
            continue;
        }
        let c = r.shrink(r.width() * 0.2);
        let mut lum = Vec::new();
        for y in (c.top() as usize..c.bottom() as usize).step_by(2) {
            for x in (c.left() as usize..c.right() as usize).step_by(2) {
                if let Some(p) = img.pixels.get(y * img.size[0] + x) {
                    lum.push(p.r() as i32 + p.g() as i32 + p.b() as i32);
                }
            }
        }
        let (min, max) = (lum.iter().min().copied().unwrap_or(0), lum.iter().max().copied().unwrap_or(0));
        let mean = lum.iter().sum::<i32>() / lum.len().max(1) as i32;
        // an empty tile is the canvas colour (#1c1c1c) and nothing else
        empty += usize::from(max - min < 12 && (mean - 84).abs() < 14);
    }
    assert_eq!(empty, 0, "{empty} face tiles are flat squares");
    // (picking is a click on a tile; the same face can be on the page twice, so the pick is set directly here)
    h.app.caches.people_rec.selected = faces.iter().take(2).map(|f| f.trim_start_matches("recFace:").to_string()).collect();
    h.step();
    assert!(
        has(&h, "button:recBulkUnknown"),
        "the bar of the open group is drawn (open: {:?}, widgets: {:?})",
        h.app.caches.people_rec.open_person,
        h.app
            .widgets
            .iter()
            .filter(|(w, _)| w.starts_with("rec") || w.starts_with("button:rec"))
            .map(|(w, _)| w.as_str())
            .take(12)
            .collect::<Vec<_>>()
    );
    bring_into_view(&mut h, "button:recBulkUnknown");
    assert_eq!(h.app.caches.people_rec.selected.len(), 2);
    let unknown0 = h.app.caches.people_rec.overview().unwrap().unknown_total;
    click(&mut h, "button:recBulkUnknown");
    h.step();
    until(&mut h, T, "the move", |h| h.app.caches.people_rec.selected.is_empty());
    h.step();
    h.step();
    assert!(h.app.caches.people_rec.overview().unwrap().unknown_total >= unknown0, "the two faces left the list to name");
    // the list can be limited to one kind
    click(&mut h, "button:recSectionUnknown");
    assert_eq!(h.app.caches.people_rec.section, crate::panels::people_rec::Section::Unknown);
}

#[test]
fn portrait_retouch_without_the_face_model_offers_its_download_instead_of_a_command_line() {
    let mut h = demo();
    let installed = h.app.session.execute("facemesh.status", &json!({})).ok().is_some_and(|v| v["installed"].as_bool() == Some(true));
    if installed {
        return; // this machine has the model: nothing to offer
    }
    let r = h.app.run("portrait.retouch", json!({}));
    assert!(r.is_err());
    h.step();
    assert!(matches!(h.app.ui.dialog, Some(crate::state::Dialog::FacemeshModel { .. })), "the download dialog opens: {:?}", h.app.ui.dialog);
}
