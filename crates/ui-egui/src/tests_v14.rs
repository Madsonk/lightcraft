//! LightCraft v14: the French locale (complete for every panel of ours, the system language on the
//! first run), the Develop panel by task with its search, the left column of the Retouch space, the
//! Proposals panel, the command palette (⌘K), Window ▸ Panels and the typed toasts. Headless, like
//! `tests_v12`. `LIGHTCRAFT_TEST_SHOTS=<folder>` also saves screenshots.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::i18n::{Locale, set_language, tr};
use crate::state::{RightPanel, ToastKind, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The source files of our own panels (not upstream's): every display string in them must reach
/// `tr` / `tr_format!` and have its French line. `extract_strings.py` in the test folder keeps the same list.
const OUR_FILES: &[(&str, &str)] = &[
    ("home.rs", include_str!("home.rs")),
    ("classify.rs", include_str!("classify.rs")),
    ("quick_sort.rs", include_str!("quick_sort.rs")),
    ("workspaces.rs", include_str!("workspaces.rs")),
    ("context_bar.rs", include_str!("context_bar.rs")),
    ("create_space.rs", include_str!("create_space.rs")),
    ("edit_tasks.rs", include_str!("edit_tasks.rs")),
    ("retouch_left.rs", include_str!("retouch_left.rs")),
    ("palette.rs", include_str!("palette.rs")),
    ("panels/proposals.rs", include_str!("panels/proposals.rs")),
    ("flow.rs", include_str!("flow.rs")),
    ("panels/cull.rs", include_str!("panels/cull.rs")),
    ("panels/portrait.rs", include_str!("panels/portrait.rs")),
    ("panels/sorting_help.rs", include_str!("panels/sorting_help.rs")),
    ("panels/people_rec.rs", include_str!("panels/people_rec.rs")),
    ("panels/people_places.rs", include_str!("panels/people_places.rs")),
    ("panels/places.rs", include_str!("panels/places.rs")),
    ("panels/people.rs", include_str!("panels/people.rs")),
    ("panels/person.rs", include_str!("panels/person.rs")),
    ("panels/unnamed.rs", include_str!("panels/unnamed.rs")),
    ("panels/faces.rs", include_str!("panels/faces.rs")),
    ("panels/strip.rs", include_str!("panels/strip.rs")),
    ("panels/resume.rs", include_str!("panels/resume.rs")),
    ("tasks.rs", include_str!("tasks.rs")),
    ("panels/videos.rs", include_str!("panels/videos.rs")),
];

/// The string literal that opens right after `at` (`"…"`, with simple escapes), if any.
fn literal_after(text: &str, at: usize) -> Option<String> {
    let rest = text[at..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// Every literal passed to one of `prefixes` in `text` (the `(` of the call must follow the prefix).
fn literals(text: &str, prefixes: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for prefix in prefixes {
        let mut from = 0;
        while let Some(i) = text[from..].find(prefix) {
            let at = from + i + prefix.len();
            if let Some(s) = literal_after(text, at)
                && s.chars().filter(|c| c.is_alphabetic()).count() >= 2
            {
                out.push(s);
            }
            from = at;
        }
    }
    out
}

fn services() -> Services {
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    }
}

fn app_with(s: lightcraft_engine::Session) -> LightcraftApp {
    let mut app = LightcraftApp::new(s, services());
    app.ui.settings.workspaces = true;
    app.ui.view = ViewMode::PhotoGrid;
    app
}

fn demo() -> Headless {
    let mut h = Headless::new(app_with(lightcraft_engine::Session::with_demo()), [1500.0, 950.0], 1.0);
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn any_widget(h: &Headless, prefix: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w.starts_with(prefix))
}

fn run(h: &mut Headless, id: &str, p: Value) -> Result<Value, String> {
    let r = h.app.run(id, p);
    h.step();
    h.step();
    r
}

fn shot(h: &mut Headless, name: &str) {
    if let Ok(dir) = std::env::var("LIGHTCRAFT_TEST_SHOTS") {
        let _ = std::fs::create_dir_all(&dir);
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&dir).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

// ------------------------------------------------------------------ French

/// Every display string of our panels reaches `tr` and has its French line: a literal handed to a
/// label, button, tooltip or toast without `tr` is listed and fails the test, and so is a `tr("…")`
/// key the French catalog lacks.
#[test]
fn french_covers_every_string_of_our_panels() {
    let fr: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("../locales/fr.json")).unwrap();
    assert!(fr.len() > 2000, "the French catalog has {} messages", fr.len());
    // calls that must not take a bare literal (the same list as `extract_strings.py`)
    let raw_calls = [
        "ui.label(",
        "ui.button(",
        "ui.small_button(",
        "ui.heading(",
        "RichText::new(",
        "Button::new(",
        ".on_hover_text(",
        ".hint_text(",
        "app.toast(ctx, ",
        "app.toast(ui.ctx(), ",
        "app.toast_error(ctx, ",
        "app.toast_error(ui.ctx(), ",
        "ProgressBar::new(frac).text(",
    ];
    let mut bare: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for (name, text) in OUR_FILES {
        for s in literals(text, &raw_calls) {
            bare.push(format!("{name}: {s:?}"));
        }
        // keys: tr("…"), tr_format!("…"), and the label / tip tables of the strip and the context bar
        for s in literals(text, &["tr(", "tr_format!(", "label: ", "tip: ", "title: "]) {
            if s.contains('{') {
                continue; // a format message: checked by build.rs in every catalog
            }
            if !fr.contains_key(&s) {
                missing.push(format!("{name}: {s:?}"));
            }
        }
    }
    assert!(bare.is_empty(), "{} display string(s) of our panels bypass tr(): {bare:#?}", bare.len());
    assert!(missing.is_empty(), "{} string(s) of our panels have no French line: {missing:#?}", missing.len());
}

/// The vocabulary a French photographer expects, and the two faces of "Edit" (the menu, the panel).
#[test]
fn french_vocabulary_is_the_agreed_one() {
    set_language(Locale::Fr);
    for (en, fr) in [
        ("Library", "Bibliothèque"),
        ("Retouch", "Développement"),
        ("Create", "Créer"),
        ("Develop", "Développement"),
        ("Edit", "Édition"),
        ("Keywords", "Mots-clés"),
        ("People", "Personnes"),
        ("Places", "Lieux"),
        ("Filmstrip", "Pellicule"),
        ("Proposals", "Propositions"),
        ("Sharpening", "Netteté"),
        ("Exposure", "Exposition"),
        ("Workspaces", "Espaces de travail"),
    ] {
        assert_eq!(tr(en), fr, "{en}");
    }
    // a format message, with the plural suffix consumed the French way
    assert_eq!(crate::i18n::tr_format!("{} photo{}", 3, "s"), "3 photo(s)");
    set_language(Locale::En);
    assert_eq!(tr("Library"), "Library");
}

/// A French system opens LightCraft in French on the first run; the menu command, the settings
/// value and the control channel all reach the locale.
#[test]
fn french_is_reached_from_the_system_the_menu_and_the_settings() {
    for tag in ["fr", "fr-FR", "fr_CA.UTF-8", "fr-BE", "fr-CH"] {
        assert_eq!(Locale::parse_tag(tag), Some(Locale::Fr), "{tag}");
    }
    assert_eq!(crate::menus::language_from_command("app.language.french"), Some(Locale::Fr));
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default());
    app.run("app.language.french", json!({})).unwrap();
    assert_eq!(app.ui.language, Locale::Fr);
    assert_eq!(tr("File"), "Fichier");
    assert_eq!(crate::menubar::checked(&app, "app.language.french"), Some(true));
    let saved = serde_json::to_string(&app.ui).unwrap();
    assert!(saved.contains(r#""language":"fr""#));
    assert_eq!(serde_json::from_str::<crate::state::UiState>(&saved).unwrap().language, Locale::Fr);
    // the Language menu lists it, with its endonym
    assert!(crate::menus::ui_commands().any(|c| c.0 == "app.language.french" && c.1 == "Français" && c.3 == "Edit>Language"));
    set_language(Locale::En);
}

// ------------------------------------------------------------------ Develop by task

/// The task groups name real controls, each once; the search finds a control by its French name,
/// accents or not.
#[test]
fn develop_by_task_groups_real_controls_and_the_search_speaks_french() {
    let ids: Vec<&str> = lightcraft_develop::CONTROLS.iter().map(|c| c.id).collect();
    let mut seen: Vec<&str> = Vec::new();
    for task in crate::edit_tasks::TASKS {
        for c in task.controls {
            assert!(ids.contains(c), "task {}: unknown control {c}", task.id);
            assert!(!seen.contains(c), "control {c} is in two tasks");
            seen.push(c);
        }
    }
    assert!(seen.len() >= 55, "{} controls grouped by task", seen.len());
    set_language(Locale::En);
    assert!(crate::edit_tasks::search("expo").contains(&"light.exposure"));
    assert!(crate::edit_tasks::search("").is_empty());
    set_language(Locale::Fr);
    assert!(crate::edit_tasks::search("exposition").contains(&"light.exposure"));
    assert!(crate::edit_tasks::search("nettete").contains(&"detail.sharpenAmount"), "accents are optional");
    assert!(crate::edit_tasks::search("Netteté").contains(&"detail.sharpenAmount"));
    set_language(Locale::En);
}

/// In the window: the toggle switches the Develop panel to its task sections (remembered in the
/// settings), the search field narrows it to the matching sliders.
#[test]
fn develop_by_task_shows_its_sections_and_its_search_in_the_window() {
    let mut h = demo();
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(has(&h, "section:light"), "the panel by type first");
    assert!(has(&h, "button:editView-task"), "the toggle is offered");
    let r = h.request("ui.clickWidget", json!({"id": "button:editView-task"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.ui.edit_by_task);
    for id in ["section:task.base", "section:task.light", "section:task.color", "section:task.detail", "section:task.optics", "section:task.local"] {
        assert!(has(&h, id), "{id} on screen: {:?}", h.app.widgets.iter().map(|(w, _)| w).filter(|w| w.starts_with("section")).collect::<Vec<_>>());
    }
    assert!(has(&h, "field:editSearch"));
    shot(&mut h, "v14-develop-by-task.png");
    // the choice is saved with the UI settings
    assert!(serde_json::to_string(&h.app.ui).unwrap().contains(r#""editByTask":true"#));
    // search: only the matching sliders, no task sections
    h.app.ui.edit_search = "exposure".into();
    h.settle(SETTLE);
    assert!(
        has(&h, "slider:light.exposure") || any_widget(&h, "slider:light.exposure"),
        "{:?}",
        h.app.widgets.iter().map(|(w, _)| w).filter(|w| w.contains("exposure")).collect::<Vec<_>>()
    );
    assert!(!has(&h, "section:task.base"));
    shot(&mut h, "v14-develop-search.png");
}

// ------------------------------------------------------------------ Retouch space

/// The Retouch space opens with its left column (selection, versions, history) and the Proposals
/// panel is a panel of the strip with a Before / After pair; off, the classic window has neither.
#[test]
fn retouch_space_has_its_left_column_and_the_proposals_panel() {
    let mut h = demo();
    run(&mut h, "workspace.retouch", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(has(&h, "panel:retouch_left"), "{:?}", h.app.widgets.iter().map(|(w, _)| w).filter(|w| w.starts_with("panel")).collect::<Vec<_>>());
    assert!(has(&h, "button:developFit"), "the active-photo navigator");
    assert!(has(&h, "button:retouchVersionCreate"));
    assert!(has(&h, "button:ctxBeforeAfter"), "Before/After in one gesture");
    assert!(has(&h, "icon:portrait"), "the Portrait tool in the strip (the Proposals recipe is a button of its panel)");
    assert!(!has(&h, "icon:proposals"));
    assert!(!has(&h, "icon:versions"), "the left column has the versions and the history: one place each");
    run(&mut h, "panel.proposals", json!({})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, RightPanel::Proposals);
    assert!(has(&h, "button:proposalsPropose"));
    assert!(has(&h, "image:proposalsBefore") && has(&h, "image:proposalsAfter"));
    shot(&mut h, "v14-retouch-proposals.png");
    // a version made here shows in the column
    run(&mut h, "version.create", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(has(&h, "retouchVersion:0"));
    // Before/After: one click shows the original, one more the edit
    let r = h.request("ui.clickWidget", json!({"id": "button:ctxBeforeAfter"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(h.app.ui.before_after, crate::state::BeforeAfter::Original);
    let r = h.request("ui.clickWidget", json!({"id": "button:ctxBeforeAfter"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(h.app.ui.before_after, crate::state::BeforeAfter::Off);
    // workspaces off: the classic window, nothing of this
    h.app.ui.settings.workspaces = false;
    run(&mut h, "panel.versions", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(!has(&h, "panel:retouch_left"));
    assert!(!has(&h, "icon:proposals"));
    assert!(has(&h, "button:proposalsHeader"), "the section stays at the top of Versions in the classic window");
}

// ------------------------------------------------------------------ Command palette

/// ⌘K: the palette lists the menu commands, filters them, runs the chosen one and closes.
#[test]
fn the_command_palette_filters_and_runs_menu_commands() {
    let mut h = demo();
    let hits = crate::palette::hits(&h.app, "square grid");
    assert_eq!(hits.first().map(|x| x.id.as_str()), Some("view.squareGrid"), "{hits:?}");
    assert!(crate::palette::hits(&h.app, "").len() > 100, "every menu command");
    assert!(crate::palette::hits(&h.app, "zzzz-nothing").is_empty());
    assert!(crate::menus::ui_commands().any(|c| c.0 == "app.commandPalette" && c.2 == Some("Cmd+K")));
    let r = h.request("ui.key", json!({"key": "K", "cmd": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "field:commandPalette"), "{:?}", h.app.widgets.iter().map(|(w, _)| w).filter(|w| w.contains("alette")).collect::<Vec<_>>());
    let r = h.request("ui.text", json!({"text": "square grid"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "palette:view.squareGrid"));
    shot(&mut h, "v14-palette.png");
    let r = h.request("ui.key", json!({"key": "Enter"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.view, ViewMode::SquareGrid);
    assert!(!has(&h, "field:commandPalette"), "closed after running");
}

// ------------------------------------------------------------------ Panels, toasts

/// Window ▸ Panels toggles the left column, the filmstrip, the filter bar and the histogram, with a
/// check mark that says what is open.
#[test]
fn window_panels_toggle_and_are_checked() {
    let mut app = app_with(lightcraft_engine::Session::with_demo());
    for (id, read) in [
        ("window.panel.left", &(|a: &LightcraftApp| a.ui.left_panel) as &dyn Fn(&LightcraftApp) -> bool),
        ("window.panel.filmstrip", &|a: &LightcraftApp| a.ui.filmstrip),
        ("window.panel.filterBar", &|a: &LightcraftApp| a.ui.filter_bar),
        ("window.panel.histogram", &|a: &LightcraftApp| a.ui.histogram),
    ] {
        let before = read(&app);
        app.run(id, json!({})).unwrap();
        assert_eq!(read(&app), !before, "{id}");
        assert_eq!(crate::menubar::checked(&app, id), Some(!before), "{id}");
        app.run(id, json!({})).unwrap();
        assert_eq!(read(&app), before, "{id} back");
    }
    assert!(crate::menus::ui_commands().filter(|c| c.3 == "Window>Panels").count() >= 5);
}

/// A toast says what it is: information, success, or an error that stays long enough to read.
#[test]
fn toasts_carry_their_kind() {
    let mut app = app_with(lightcraft_engine::Session::with_demo());
    let ctx = egui::Context::default();
    app.toast(&ctx, "done");
    assert_eq!(app.ui.toast_kind, ToastKind::Info);
    app.toast_success(&ctx, "made");
    assert_eq!(app.ui.toast_kind, ToastKind::Success);
    app.toast_error(&ctx, "failed");
    assert_eq!(app.ui.toast_kind, ToastKind::Error);
    let (_, until, _) = app.ui.toast.clone().unwrap();
    assert!(until >= 6.0, "an error stays 6 s");
}

#[test]
fn workflow_development_scopes_presets_and_reset_to_active_photo() {
    let mut h = demo();
    h.app.ui.more_tools = true;
    let ids: Vec<_> = h.app.session.catalog.photos().filter(|p| p.in_library()).take(2).map(|p| p.id).collect();
    assert_eq!(ids.len(), 2);
    h.app.session.selection.ids = ids.clone();
    h.app.session.selection.active = Some(ids[0]);
    h.app.run("develop.set", json!({"ids": ids.iter().map(|id| id.0).collect::<Vec<_>>(), "control": "light.exposure", "value": 1})).unwrap();
    h.app.run("workspace.retouch", json!({})).unwrap();
    h.app.run("develop.reset", json!({})).unwrap();
    assert_eq!(h.app.session.develop_of(ids[0]).unwrap().light.exposure, 0.0);
    assert_eq!(h.app.session.develop_of(ids[1]).unwrap().light.exposure, 1.0);
    let before = h.app.session.develop_of(ids[1]).unwrap();
    let preset = h.app.session.presets.first().unwrap().id.clone();
    h.app.run("preset.apply", json!({"id": preset})).unwrap();
    assert_eq!(*h.app.session.develop_of(ids[1]).unwrap(), *before);
    assert_eq!(h.app.session.selection.ids, ids);
    assert!(h.app.ui.filmstrip);
    h.app.run("workspace.library", json!({})).unwrap();
    assert!(h.app.ui.filmstrip);
    h.app.run("workspace.step", json!({"step": "sort"})).unwrap();
    h.settle(SETTLE);
    assert!(has(&h, "button:batchAdjust:light.exposure:+"));
    assert!(!has(&h, "panel:quick_sort"));
}

#[test]
fn series_ui_preview_does_not_edit_and_apply_is_explicit() {
    let mut h = demo();
    h.app.ui.more_tools = true;
    let ids: Vec<_> = h.app.session.catalog.photos().filter(|p| p.in_library()).take(3).map(|p| p.id).collect();
    h.app.session.selection.ids = ids.clone();
    h.app.session.selection.active = Some(ids[0]);
    h.app.run("workspace.library", json!({})).unwrap();
    h.app.run("workspace.step", json!({"step":"sort"})).unwrap();
    h.settle(SETTLE);
    let before = h.app.session.catalog.to_snapshot();
    assert!(has(&h, "button:seriesPreview"));
    crate::tests_v12::reveal(&mut h, "button:seriesPreview");
    h.request("ui.hoverWidget", json!({"id":"button:seriesPreview"}), T);
    h.step();
    let click = h.request("ui.clickWidget", json!({"id":"button:seriesPreview"}), T);
    assert_eq!(click["ok"], true);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.step();
        if h.app.session.series.status()["state"] == "ready" {
            break;
        }
        assert!(std::time::Instant::now() < end, "preview button did not finish analysis");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.settle(SETTLE);
    assert_eq!(h.app.session.catalog.to_snapshot(), before);
    assert!(has(&h, "image:proposalsBefore"));
    assert!(has(&h, "image:proposalsAfter"));
    assert_eq!(h.app.session.active(), Some(ids[0]));
    crate::tests_v12::reveal(&mut h, "button:seriesApply");
    h.request("ui.hoverWidget", json!({"id":"button:seriesApply"}), T);
    h.step();
    let click = h.request("ui.clickWidget", json!({"id":"button:seriesApply"}), T);
    assert_eq!(click["ok"], true);
    h.step();
    assert_eq!(h.app.session.series.status()["state"], "applied");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.catalog.to_snapshot(), before);
}
