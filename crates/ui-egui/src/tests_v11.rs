//! LightCraft v11: workspaces (Library, Retouch, Create) and the context bar. Headless, on the demo
//! library: changing space keeps the selection and the filter, each space is a remembered panel
//! arrangement that survives a restart, Reset puts the factory one back, own workspaces can be
//! saved and deleted, the switch off gives the classic window, and the Create kit is a folder of
//! images plus a manifest.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::workspaces::{self, Space};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo_with(services: Services) -> Headless {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    // the crate's tests start in the classic window; these are about the workspaces
    app.ui.settings.workspaces = true;
    let mut h = Headless::new(app, [1500.0, 950.0], 1.0);
    h.settle(SETTLE);
    h
}

fn demo() -> Headless {
    demo_with(Services { png: None, pick_folder: Some(Box::new(|| None)), ..Default::default() })
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

fn key(h: &mut Headless, k: &str, ctrl: bool) {
    let r = h.request("ui.key", json!({"key": k, "ctrl": ctrl}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn run(h: &mut Headless, id: &str, p: Value) -> Result<Value, String> {
    let r = h.app.run(id, p);
    h.step();
    h.step();
    r
}

/// Select `n` photos and filter the view; returns (selection, filter text).
fn pick(h: &mut Headless, n: usize) -> Vec<u64> {
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(n).map(|p| p.0).collect();
    h.app.run("library.select", json!({"ids": ids})).unwrap();
    h.step();
    ids
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "lc-v11-{tag}-{}-{}",
        std::process::id(),
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn changing_space_never_empties_the_selection_or_the_filter() {
    let mut h = demo();
    h.app.run("library.filter", json!({"kind": "image"})).unwrap();
    h.step();
    let ids = pick(&mut h, 3);
    assert_eq!(ids.len(), 3);
    let filter = h.app.session.filter.clone();
    let source = h.app.session.source;
    assert_ne!(filter, Default::default(), "a filter is on");
    for id in ["workspace.retouch", "workspace.create", "workspace.library", "workspace.retouch", "workspace.library"] {
        run(&mut h, id, json!({})).unwrap();
        let sel: Vec<u64> = h.app.session.selection.ids.iter().map(|p| p.0).collect();
        assert_eq!(sel, ids, "selection after {id}");
        assert_eq!(h.app.session.filter, filter, "filter after {id}");
        assert_eq!(h.app.session.source, source);
    }
    // Retouch selection: same thing through the button
    click(&mut h, "button:ctxRetouch");
    assert_eq!(h.app.workspaces.current, "Retouch");
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert_eq!(h.app.session.selection.ids.iter().map(|p| p.0).collect::<Vec<_>>(), ids);
    assert_eq!(h.app.session.filter, filter);
    // and the way back
    click(&mut h, "button:ctxBack");
    assert_eq!(h.app.workspaces.current, "Library");
    assert_eq!(h.app.session.selection.ids.iter().map(|p| p.0).collect::<Vec<_>>(), ids);
    // only the explicit Clear empties them
    click(&mut h, "button:ctxClear");
    // (LightCraft always keeps one active photo: Deselect All leaves it as the whole selection)
    assert_eq!(h.app.session.selection.ids.len(), 1, "{:?}", h.app.session.selection);
    assert_eq!(h.app.session.filter, Default::default());
}

#[test]
fn the_context_bar_says_source_filter_selection_and_active_photo() {
    let mut h = demo();
    assert!(has(&h, "region:contextBar") && has(&h, "context:summary"));
    let all = h.app.session.visible_cloned().len();
    let s = crate::context_bar::summary(&mut h.app);
    assert_eq!((s.selected, s.visible), (h.app.session.selection.ids.len(), all));
    assert!(s.filter.is_empty());
    h.app.run("library.filter", json!({"rating": 1})).unwrap();
    let ids = pick(&mut h, 2);
    let s = crate::context_bar::summary(&mut h.app);
    assert_eq!(s.selected, ids.len());
    assert_eq!(s.filter.len(), 1, "{:?}", s.filter);
    assert!(s.active.is_some());
    let j = h.request("ui.inspect", json!({}), T)["result"]["workspaces"].clone();
    assert_eq!(j["current"], "Library");
    assert_eq!(j["context"]["selected"], 2, "{j}");
}

#[test]
fn ctrl_1_2_3_open_the_spaces_and_each_arranges_the_panels() {
    let mut h = demo();
    h.app.run("library.filter", json!({})).unwrap();
    pick(&mut h, 2);
    let active = h.app.session.active().expect("active photo");
    let rating = h.app.session.catalog.photo(active).unwrap().rating;
    key(&mut h, "2", true);
    assert_eq!(h.app.workspaces.current, "Retouch");
    assert_eq!(h.app.session.catalog.photo(active).unwrap().rating, rating, "Ctrl+2 opens a space, it does not rate the photo");
    assert_eq!(
        (h.app.ui.view, h.app.ui.right, h.app.ui.left_panel, h.app.ui.filmstrip),
        (ViewMode::Detail, RightPanel::Edit, true, true),
        "v14: Retouch has its left column"
    );
    key(&mut h, "3", true);
    assert_eq!(h.app.workspaces.current, "Create");
    assert!(h.app.ui.left_panel && h.app.ui.right == RightPanel::None);
    assert!(has(&h, "panel:create"), "the Create panel is on screen");
    assert!(workspaces::hides_strip(&h.app));
    key(&mut h, "1", true);
    assert_eq!(h.app.workspaces.current, "Library");
    assert!(h.app.ui.filter_bar && h.app.ui.left_panel && h.app.ui.right == RightPanel::Keywords);
    assert!(has(&h, "button:space-library") && has(&h, "button:space-retouch") && has(&h, "button:space-create"));
    // the selector buttons do the same
    click(&mut h, "button:space-retouch");
    assert_eq!(h.app.workspaces.current, "Retouch");
    // the plain digit keys are still ratings (Ctrl is what opens a space)
    key(&mut h, "3", false);
    assert_eq!(h.app.workspaces.current, "Retouch");
}

#[test]
fn a_space_remembers_how_it_was_left_and_survives_a_restart() {
    let dir = temp_dir("persist");
    let file = dir.join(workspaces::FILE);
    let mut h = demo();
    workspaces::attach(&mut h.app, Some(file.clone()));
    pick(&mut h, 2);
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    // the user changes the panel, then leaves
    h.app.ui.right = RightPanel::Masking;
    h.step();
    run(&mut h, "workspace.library", json!({})).unwrap();
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    assert_eq!(h.app.ui.right, RightPanel::Masking, "Retouch was left with Masking open");
    run(&mut h, "workspace.create", json!({})).unwrap();
    workspaces::flush(&mut h.app, 1e9, true);
    assert!(file.exists(), "workspaces.json written");
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["current"], "Create", "{saved}");
    assert!(saved["saved"].as_array().unwrap().iter().any(|s| s["name"] == "Retouch" && s["layout"]["right"] == "masking"), "{saved}");
    // a new run of the app: the space last used reopens, and Retouch still has Masking
    let mut h2 = demo();
    workspaces::attach(&mut h2.app, Some(file.clone()));
    assert_eq!(h2.app.workspaces.current, "Create");
    assert_eq!(h2.app.workspaces.kind, Space::Create);
    assert!(h2.app.ui.left_panel && h2.app.ui.right == RightPanel::None);
    pick(&mut h2, 2);
    run(&mut h2, "workspace.retouch", json!({})).unwrap();
    assert_eq!(h2.app.ui.right, RightPanel::Masking);
    // a file that is not ours is ignored, not fatal
    std::fs::write(&file, b"not json").unwrap();
    let mut h3 = demo();
    workspaces::attach(&mut h3.app, Some(file));
    assert_eq!(h3.app.workspaces.current, "Library");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reset_puts_the_factory_arrangement_back() {
    let mut h = demo();
    pick(&mut h, 2);
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    h.app.ui.right = RightPanel::Info;
    h.app.ui.filmstrip = false;
    h.step();
    assert!(h.app.workspaces.prefs.saved.iter().any(|s| s.name == "Retouch"), "the change is remembered");
    run(&mut h, "workspace.reset", json!({})).unwrap();
    assert_eq!((h.app.ui.right, h.app.ui.filmstrip), (RightPanel::Edit, true));
    assert!(h.app.workspaces.prefs.saved.is_empty(), "nothing left but the factory arrangement");
    h.step();
    assert!(h.app.workspaces.prefs.saved.is_empty(), "and the next frame does not record it again");
}

#[test]
fn an_own_workspace_is_saved_listed_in_the_menu_and_deleted() {
    let mut h = demo();
    pick(&mut h, 2);
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    h.app.ui.right = RightPanel::Versions;
    h.step();
    assert!(run(&mut h, "workspace.saveAs", json!({"name": "  "})).is_err(), "a name is needed");
    assert!(run(&mut h, "workspace.saveAs", json!({"name": "retouch"})).is_err(), "a built-in name is taken");
    run(&mut h, "workspace.saveAs", json!({"name": "Portraits"})).unwrap();
    assert_eq!(h.app.workspaces.current, "Portraits");
    assert_eq!(h.app.workspaces.kind, Space::Retouch);
    assert!(run(&mut h, "workspace.saveAs", json!({"name": "portraits"})).is_err(), "the name exists");
    // it is in Window ▸ Workspaces, checked, with the three spaces and the layout commands
    let bar = crate::menubar::menu_bar(&h.app);
    let window = &bar.iter().find(|(t, _)| t == "Window").unwrap().1;
    let children = window
        .iter()
        .find_map(|n| match n {
            crate::menubar::MenuNode::Submenu { label, children } if label == "Workspaces" => Some(children),
            _ => None,
        })
        .expect("Workspaces submenu");
    let items: Vec<(String, bool)> = children
        .iter()
        .filter_map(|n| match n {
            crate::menubar::MenuNode::Item { id, label, checked, .. } => Some((format!("{id}:{label}"), *checked == Some(true))),
            _ => None,
        })
        .collect();
    for want in [
        "workspace.library:Library",
        "workspace.retouch:Retouch",
        "workspace.create:Create",
        "workspace.switch:Portraits",
        "workspace.reset:Reset Workspace",
        "workspace.saveAs:Save Workspace As…",
    ] {
        assert!(items.iter().any(|(i, _)| i == want), "{want} in {items:?}");
    }
    assert!(items.iter().any(|(i, c)| i == "workspace.switch:Portraits" && *c), "the open one is checked: {items:?}");
    // another space, then back to it: its own arrangement
    run(&mut h, "workspace.library", json!({})).unwrap();
    run(&mut h, "workspace.switch", json!({"name": "Portraits"})).unwrap();
    assert_eq!((h.app.ui.right, h.app.workspaces.kind), (RightPanel::Versions, Space::Retouch));
    // a built-in space can't be deleted, an own one can
    assert!(run(&mut h, "workspace.delete", json!({"name": "Create"})).is_err());
    run(&mut h, "workspace.delete", json!({})).unwrap();
    assert_eq!(h.app.workspaces.current, "Library");
    assert!(h.app.workspaces.find("Portraits").is_none());
}

#[test]
fn switched_off_the_window_is_the_classic_one() {
    let on = demo();
    let top_on = on.app.canvas_rect.expect("canvas").top();
    assert!(has(&on, "region:contextBar"));
    let mut h = demo();
    let r = h.request("ui.set", json!({"settings": {"workspaces": false}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    for id in ["region:contextBar", "button:space-library", "button:space-retouch", "button:space-create", "context:summary"] {
        assert!(!has(&h, id), "{id} is gone");
    }
    assert!(h.app.canvas_rect.unwrap().top() < top_on, "no bar under the top bar: {} vs {top_on}", h.app.canvas_rect.unwrap().top());
    // the shortcuts and commands do nothing (and say why), the panels stay as they were
    let before = (h.app.ui.view, h.app.ui.right, h.app.ui.left_panel);
    key(&mut h, "2", true);
    assert_eq!((h.app.ui.view, h.app.ui.right, h.app.ui.left_panel), before);
    assert!(run(&mut h, "workspace.retouch", json!({})).unwrap_err().contains("off"));
    // the menu has no Workspaces entry, the classic tool strip is there
    let bar = crate::menubar::menu_bar(&h.app);
    let window = &bar.iter().find(|(t, _)| t == "Window").unwrap().1;
    assert!(!window.iter().any(|n| matches!(n, crate::menubar::MenuNode::Submenu { label, .. } if label == "Workspaces")));
    assert!(!workspaces::hides_strip(&h.app) && !workspaces::create_center(&h.app));
    // nothing is recorded while it is off
    h.app.ui.right = RightPanel::Info;
    h.step();
    assert!(h.app.workspaces.prefs.saved.is_empty());
    // and back on
    h.app.ui.settings.workspaces = true;
    h.step();
    h.step();
    assert!(has(&h, "region:contextBar"));
}

#[test]
fn the_kit_is_a_folder_of_images_with_a_manifest() {
    let out = temp_dir("kit");
    let base = out.clone();
    let services = Services {
        png: None,
        pick_folder: Some(Box::new(|| None)),
        write_shared: Some(std::sync::Arc::new(|p: &str, bytes: &[u8]| {
            if let Some(d) = std::path::Path::new(p).parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(p, bytes).map_err(|e| e.to_string())
        })),
        ..Default::default()
    };
    let mut h = demo_with(services);
    let ids = pick(&mut h, 3);
    let previous = h.app.session.last_export.clone();
    let r = run(&mut h, "create.kit", json!({"dir": base.to_str().unwrap(), "name": "Test kit", "target": "designcraft", "longEdge": 64})).unwrap();
    assert_eq!(r["photos"], 3, "{r}");
    let kit = std::path::PathBuf::from(r["dir"].as_str().unwrap());
    let t0 = std::time::Instant::now();
    while (h.app.export.is_some() || h.app.workspaces.create.pending.is_some()) && t0.elapsed() < Duration::from_secs(90) {
        h.step();
    }
    assert!(h.app.workspaces.create.pending.is_none(), "finished");
    let manifest: Value = serde_json::from_slice(&std::fs::read(kit.join("manifest.json")).expect("manifest.json")).unwrap();
    assert_eq!(manifest["kit"], "lightcraft-kit");
    assert_eq!(manifest["target"], "designcraft");
    assert_eq!(manifest["count"], 3, "{manifest}");
    let images = manifest["images"].as_array().unwrap();
    for (i, im) in images.iter().enumerate() {
        assert_eq!(im["order"], i + 1);
        let f = kit.join(im["file"].as_str().unwrap());
        assert!(f.exists(), "{f:?}");
        assert!(f.file_name().unwrap().to_string_lossy().starts_with(&format!("{:03}_", i + 1)));
        assert!(im["width"].as_u64().is_some_and(|w| w <= 64), "{im}");
        assert!(im["altText"].as_str().is_some_and(|t| !t.is_empty()));
    }
    assert!(kit.join("README.txt").exists());
    assert_eq!(std::fs::read_dir(kit.join("images")).unwrap().count(), 3);
    assert_eq!(h.app.session.last_export, previous, "Export with Previous is not changed by a kit");
    assert_eq!(h.app.session.selection.ids.iter().map(|p| p.0).collect::<Vec<_>>(), ids);
    // the kit goes in its own new folder each time
    let again = run(&mut h, "create.kit", json!({"dir": base.to_str().unwrap(), "name": "Test kit", "longEdge": 64})).unwrap();
    assert_ne!(again["dir"], r["dir"]);
    let t0 = std::time::Instant::now();
    while (h.app.export.is_some() || h.app.workspaces.create.pending.is_some()) && t0.elapsed() < Duration::from_secs(90) {
        h.step();
    }
    assert!(run(&mut h, "create.kit", json!({"dir": base.to_str().unwrap(), "target": "nope"})).is_err());
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn the_create_panel_offers_the_gallery_the_kit_and_the_slideshow() {
    let mut h = demo();
    run(&mut h, "workspace.create", json!({})).unwrap();
    for id in [
        "button:createTabGallery",
        "button:createTabKit",
        "button:createTabSlideshow",
        "field:createTitle",
        "check:createWidth-480",
        "check:createWidth-2400",
        "button:createGallery",
    ] {
        assert!(has(&h, id), "{id}");
    }
    click(&mut h, "button:createTabKit");
    for id in ["button:createTarget-0", "button:createTarget-3", "field:createKitName", "button:createEdge-1", "button:createKit"] {
        assert!(has(&h, id), "{id}");
    }
    click(&mut h, "button:createTarget-1");
    assert_eq!(h.app.workspaces.create.target, crate::create_space::Target::Pdf);
    click(&mut h, "button:createTabSlideshow");
    assert!(has(&h, "button:createSlideshow"));
    // the photos are the usual grid beside the panel
    assert!(h.app.widgets.iter().any(|(w, _)| w.starts_with("thumb:")), "photos shown");
}

#[test]
fn the_selector_the_search_field_and_the_icons_never_overlap() {
    for width in [900.0, 1100.0, 1500.0] {
        for native_menu in [false, true] {
            let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
            app.ui.settings.workspaces = true;
            app.native_menu = native_menu;
            app.integrated_titlebar = native_menu;
            let mut h = Headless::new(app, [width, 700.0], 1.0);
            h.settle(SETTLE);
            let rect = |id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no {id} at {width}"));
            let (create, search, filter, bell) = (rect("button:space-create"), rect("field:search"), rect("icon:filter"), rect("icon:bell"));
            // the selector is in the top bar, or (a window too narrow for it there) in the context bar below
            let same_row = create.top() < search.bottom() && search.top() < create.bottom();
            assert!(!same_row || create.right() < search.left(), "{width}/{native_menu}: selector {create:?} over the search field {search:?}");
            assert_eq!(same_row, h.app.workspaces.selector_in_top_bar, "{width}/{native_menu}");
            assert!(search.right() < filter.left(), "{width}/{native_menu}: search {search:?} over the filter icon {filter:?}");
            assert!(filter.right() <= bell.left(), "{width}/{native_menu}: filter icon {filter:?} over the bell {bell:?}");
            if !native_menu && h.app.workspaces.selector_in_top_bar {
                // the in-window menus end before the selector starts
                let library = rect("button:space-library");
                let title = h.app.widgets.iter().filter(|(w, _)| w.starts_with("menu")).map(|(_, r)| r.right()).fold(0.0f32, f32::max);
                assert!(title <= library.left() + 0.5, "{width}: menus end at {title}, selector starts at {}", library.left());
            }
        }
    }
}

#[test]
fn the_first_run_opens_the_library_arrangement_and_a_later_run_the_space_last_used() {
    let dir = temp_dir("first");
    let file = dir.join(workspaces::FILE);
    let mut h = demo();
    // the window as an older ui.json left it: the loupe with Edit open and no sources column
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    h.app.ui.left_panel = false;
    workspaces::attach(&mut h.app, Some(file.clone()));
    h.step();
    assert_eq!((h.app.ui.view, h.app.ui.right, h.app.ui.left_panel, h.app.ui.filter_bar), (ViewMode::PhotoGrid, RightPanel::Keywords, true, true));
    assert!(h.app.workspaces.prefs.saved.is_empty(), "nothing is remembered until the user changes something");
    // the Retouch space is the factory one the first time too (not what the window was before)
    pick(&mut h, 2);
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    assert_eq!(
        (h.app.ui.view, h.app.ui.right, h.app.ui.left_panel, h.app.ui.filmstrip),
        (ViewMode::Detail, RightPanel::Edit, true, true),
        "v14: Retouch has its left column"
    );
    run(&mut h, "workspace.library", json!({})).unwrap();
    assert_eq!((h.app.ui.view, h.app.ui.right, h.app.ui.left_panel), (ViewMode::PhotoGrid, RightPanel::Keywords, true));
    let _ = std::fs::remove_dir_all(dir);
}
