//! Library ▸ Import (LightCraft v12, phase 3 of the UX study): the home screen. It replaces the
//! import review *window* (a modal that dims the whole app until it is answered) with a page of the
//! Library space: where to resume, a drop zone for a card or a folder, the essential import settings
//! and the background tasks, all on one screen that never blocks the rest of the window.
//!
//! Nothing here imports by itself: the scan, the review of what was found and the import are the
//! existing ones (`import::open`, [`crate::import::ImportDialog`], `import::start`). The home screen
//! only hosts them in a panel and, once an import has started, opens the Sort step on the photos
//! that are arriving (the import runs on its worker thread and its batches join the catalog between
//! frames, as before). The classic modal is still one click away ("More options…") for the settings
//! the page does not show (folder template, DNG, metadata presets, what to do with deleted photos).
//!
//! With Settings ▸ Workspaces off none of this is reached: File ▸ Import opens the modal as before.

use std::collections::{BTreeMap, HashSet};

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::import::ImportDialog;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};
use crate::workspaces::{self, Step};

/// Width of the left (resume, tasks) and right (settings) columns.
const LEFT_W: f32 = 270.0;
const RIGHT_W: f32 = 350.0;
/// Candidates drawn in the preview (the rest is counted).
const PREVIEW_MAX: usize = 32;

/// What the import can start in the background once the photos are in, in the order the page lists
/// them. Each one is an existing command; the page only chains them on the new photos.
pub const AFTER: [(&str, &str); 5] = [
    ("previews", "Build previews for fast sorting"),
    ("culling", "Find bursts and soft photos (Assisted Culling)"),
    ("faces", "Read the faces (People model)"),
    ("places", "Put the places from the GPS"),
    ("keywords", "Suggest keywords (slow the first time)"),
];

/// The import that was started from this page, to time it and to chain the follow-ups.
#[derive(Clone, Debug)]
pub struct Started {
    pub clicked_ms: f64,
    pub files: usize,
    pub photos_before: usize,
    pub before: HashSet<u64>,
    /// Milliseconds from the click to the first imported photo being in the catalog.
    pub first_photo_ms: Option<f64>,
    pub after: [bool; 5],
}

/// One line of the background-task list.
#[derive(Clone, Debug)]
pub struct TaskRow {
    pub key: String,
    pub label: String,
    pub done: usize,
    pub total: usize,
    pub note: String,
    pub cancelling: bool,
}

/// Where the user can resume.
#[derive(Clone, Debug, PartialEq)]
pub struct Resume {
    /// The import day (`2026-10-10`) the photos came in.
    pub day: String,
    pub photos: usize,
    /// Rated or flagged.
    pub sorted: usize,
}

pub struct State {
    /// The import settings and the candidates of the last scan (the same struct as the modal's).
    pub d: ImportDialog,
    /// `d` holds the candidates of a scan (else only the settings).
    pub scanned: bool,
    /// Which follow-ups to run on the new photos (default: previews, culling, places).
    pub after: [bool; 5],
    pub started: Option<Started>,
    /// What the last import started here ended with (`ui.inspect`).
    pub last: Option<Value>,
    loaded: bool,
    resume: Option<((u64, u64), Vec<Resume>)>,
    /// The background tasks at the last refresh (every half second).
    pub tasks: Vec<TaskRow>,
    tasks_at: f64,
}

impl Default for State {
    fn default() -> Self {
        State {
            d: ImportDialog::default(),
            scanned: false,
            after: [true, true, false, true, false],
            started: None,
            last: None,
            loaded: false,
            resume: None,
            tasks: Vec::new(),
            tasks_at: f64::NEG_INFINITY,
        }
    }
}

// ----------------------------------------------------------------------------- remembered settings

/// The settings kept for the next import (not the keywords or the album: they belong to a shoot).
fn remembered(d: &ImportDialog, after: &[bool; 5]) -> Value {
    json!({
        "copy": d.copy, "moveFiles": d.move_files, "destination": d.destination, "organize": d.organize,
        "folderTemplate": d.folder_template, "rename": d.rename, "preset": d.preset, "metadataPreset": d.metadata_preset,
        "dng": d.dng, "after": after,
    })
}

fn restore(d: &mut ImportDialog, after: &mut [bool; 5], v: &Value) {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    d.copy = b("copy");
    d.move_files = d.copy && b("moveFiles");
    d.destination = s("destination");
    d.organize = s("organize");
    d.folder_template = s("folderTemplate");
    d.rename = s("rename");
    d.preset = s("preset");
    d.metadata_preset = s("metadataPreset");
    d.dng = b("dng");
    if let Some(a) = v.get("after").and_then(Value::as_array) {
        for (i, x) in a.iter().take(5).enumerate() {
            after[i] = x.as_bool().unwrap_or(after[i]);
        }
    }
}

// --------------------------------------------------------------------------------- scan and capture

/// A scan has ended (`import::poll_scan`): with workspaces on, what it found goes to the home screen
/// instead of a modal. Returns the dialog back when the home screen does not take it.
pub fn capture(app: &mut LightcraftApp, scanned: ImportDialog, copy: bool) -> Option<ImportDialog> {
    if !workspaces::enabled(app) {
        return Some(scanned);
    }
    ensure_loaded(app);
    let h = &mut app.workspaces.home;
    // the settings stay (that is the point of the page); the candidates are the new scan's
    h.d.candidates = scanned.candidates;
    h.d.checked = scanned.checked;
    h.d.trashed = scanned.trashed;
    h.d.sources = scanned.sources;
    h.d.on_deleted.clear();
    h.d.last_clicked = None;
    if copy {
        h.d.copy = true;
        h.d.move_files = false;
    }
    h.scanned = true;
    let _ = workspaces::set_step(app, Step::Import);
    None
}

fn ensure_loaded(app: &mut LightcraftApp) {
    if app.workspaces.home.loaded {
        return;
    }
    app.workspaces.home.loaded = true;
    if let Some(v) = app.workspaces.prefs.home.clone() {
        let h = &mut app.workspaces.home;
        restore(&mut h.d, &mut h.after, &v);
    }
}

/// Files dropped on the window while the home screen is the step in view: they are scanned and
/// reviewed here instead of being added at once. `false`: not ours (the usual drop handling runs).
pub fn on_drop(app: &mut LightcraftApp, paths: &[String]) -> bool {
    if workspaces::step_of(app) != Some(Step::Import) {
        return false;
    }
    if app.import.is_some() || app.scan.is_some() {
        return true;
    }
    scan(app, paths.to_vec()).is_ok()
}

fn scan(app: &mut LightcraftApp, paths: Vec<String>) -> Result<Value, String> {
    if paths.is_empty() {
        return Err("no path to scan".into());
    }
    ensure_loaded(app);
    workspaces::set_step(app, Step::Import)?;
    crate::import::open(app, paths)
}

// ------------------------------------------------------------------------------------------ commands

/// `workspace.home.*`.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    ensure_loaded(app);
    let strings = |k: &str| -> Vec<String> {
        match p.get(k) {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    };
    match id {
        // scan a folder, a card or photos and show what was found on the page
        "workspace.home.scan" => scan(app, strings("paths")),
        "workspace.home.device" => {
            let path = match p.get("path").and_then(Value::as_str) {
                Some(x) => x.to_string(),
                None => lightcraft_engine::devices::devices_now().into_iter().next().map(|d| d.path).ok_or("no camera or memory card found")?,
            };
            let r = scan(app, vec![path])?;
            if let Some(t) = &mut app.scan {
                t.copy = true;
            }
            Ok(r)
        }
        // the import settings, by name (a script can set what the page's fields set)
        "workspace.home.settings" => {
            let h = &mut app.workspaces.home;
            let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
            if let Some(m) = p.get("mode").and_then(Value::as_str) {
                match m {
                    "add" => (h.d.copy, h.d.move_files) = (false, false),
                    "copy" => (h.d.copy, h.d.move_files) = (true, false),
                    "move" => (h.d.copy, h.d.move_files) = (true, true),
                    other => return Err(format!("unknown mode `{other}` (add | copy | move)")),
                }
            }
            if let Some(v) = s("destination") {
                h.d.destination = v;
            }
            if let Some(v) = s("organize") {
                h.d.organize = v;
            }
            if let Some(v) = s("rename") {
                h.d.rename = v;
            }
            if let Some(v) = s("keywords") {
                h.d.keywords = v;
            }
            if let Some(v) = s("album") {
                h.d.album = None;
                h.d.new_album = v;
            }
            if let Some(v) = s("preset") {
                h.d.preset = v;
            }
            if let Some(a) = p.get("after").and_then(Value::as_object) {
                for (i, (key, _)) in AFTER.iter().enumerate() {
                    if let Some(b) = a.get(*key).and_then(Value::as_bool) {
                        h.after[i] = b;
                    }
                }
            }
            Ok(json!({"settings": remembered(&h.d, &h.after)}))
        }
        // import what is checked and open the Sort step on the photos as they arrive
        "workspace.home.import" => start(app),
        "workspace.home.moreOptions" => {
            let h = &mut app.workspaces.home;
            if !h.scanned || h.d.candidates.is_empty() {
                return Err("scan a folder or a card first".into());
            }
            app.ui.dialog = Some(crate::state::Dialog::Import { opts: Box::new(h.d.clone()) });
            Ok(json!({"dialog": "import"}))
        }
        "workspace.home.clear" => {
            let h = &mut app.workspaces.home;
            h.d.candidates.clear();
            h.d.checked.clear();
            h.d.trashed.clear();
            h.d.sources.clear();
            h.scanned = false;
            Ok(Value::Null)
        }
        // resume: where the user was (no `day`), or the photos that came in on `day`
        "workspace.home.resume" => {
            if let Some(day) = p.get("day").and_then(Value::as_str) {
                app.run("library.source", json!({"kind": "all"}))?;
                app.run("library.clearFilter", json!({}))?;
                app.run("library.filter", json!({"imported": day}))?;
            }
            workspaces::set_step(app, Step::Sort)
        }
        other => Err(format!("unknown home command `{other}`")),
    }
}

/// Import what is checked: the existing `import::start`, then the Sort step on the Recently Added
/// source so the first photos are on the grid as soon as their batch joins the catalog.
fn start(app: &mut LightcraftApp) -> Result<Value, String> {
    if app.import.is_some() {
        return Err("an import is already running".into());
    }
    let now = crate::now_ms();
    let d = app.workspaces.home.d.clone();
    let after = app.workspaces.home.after;
    let before: HashSet<u64> = app.session.catalog.photos().map(|p| p.id.0).collect();
    let photos_before = app.session.catalog.photos().filter(|p| p.in_library()).count();
    let r = crate::import::start(app, &d)?;
    let files = d.selected_paths().len();
    // remember the settings for the next time (written with workspaces.json)
    let v = remembered(&d, &after);
    app.workspaces.prefs.home = Some(v);
    workspaces::mark_dirty(app);
    app.workspaces.home.started = Some(Started { clicked_ms: now, files, photos_before, before, first_photo_ms: None, after });
    app.workspaces.home.scanned = false;
    let h = &mut app.workspaces.home;
    h.d.candidates.clear();
    h.d.checked.clear();
    h.d.trashed.clear();
    h.d.sources.clear();
    // Sort on what is arriving: Recently Added, no filter left over from before
    let _ = app.run("library.source", json!({"kind": "recentlyAdded"}));
    let _ = app.run("library.clearFilter", json!({}));
    workspaces::set_step(app, Step::Sort)?;
    Ok(json!({"started": r, "files": files}))
}

/// Once a frame: time the import started here, then chain the follow-ups when it ends; keep the list
/// of background tasks fresh while a page that shows it is in view.
pub fn tick(app: &mut LightcraftApp, now: f64) {
    if let Some(st) = app.workspaces.home.started.as_mut()
        && st.first_photo_ms.is_none()
        && app.session.catalog.photos().filter(|p| p.in_library()).count() > st.photos_before
    {
        st.first_photo_ms = Some(crate::now_ms() - st.clicked_ms);
    }
    if app.workspaces.home.started.is_some() && app.import.is_none() && app.scan.is_none() {
        finished(app);
    }
    let visible = matches!(workspaces::step_of(app), Some(Step::Import | Step::Classify));
    if visible && now - app.workspaces.home.tasks_at >= 0.5 {
        app.workspaces.home.tasks_at = now;
        app.workspaces.home.tasks = background_tasks(app);
    }
}

/// The import started here is over: record it and run the follow-ups the user ticked, on the photos
/// it added (and only those).
fn finished(app: &mut LightcraftApp) {
    let Some(st) = app.workspaces.home.started.take() else { return };
    let new: Vec<u64> = app.session.catalog.photos().filter(|p| p.in_library() && !st.before.contains(&p.id.0)).map(|p| p.id.0).collect();
    let mut started: Vec<&str> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if !new.is_empty() {
        for (i, (key, _)) in AFTER.iter().enumerate() {
            if !st.after[i] {
                continue;
            }
            let r = match *key {
                "previews" => app.run("library.buildPreviews", json!({"ids": new})),
                "culling" => crate::quick_sort::analyze_ids(app, new.clone()).map(|_| Value::Null),
                "faces" => app.run("faces.index", json!({"ids": new, "background": true})),
                "places" => app.run("metadata.reverseGeocode", json!({"ids": new})),
                "keywords" => Ok(Value::Null), // suggested keywords come back with semantic search (step 3)
                _ => Ok(Value::Null),
            };
            match r {
                Ok(_) => started.push(key),
                Err(e) => notes.push(format!("{key}: {e}")),
            }
        }
    }
    app.workspaces.home.last = Some(json!({
        "files": st.files, "imported": new.len(), "firstPhotoMs": st.first_photo_ms, "doneMs": crate::now_ms() - st.clicked_ms,
        "followUps": started, "notStarted": notes,
    }));
}

// ----------------------------------------------------------------------------------- background tasks

/// Every long job in flight, in one list: the import, the scan, the tasks of [`crate::tasks`], the
/// preview build, the face indexing and the quick-sort analysis.
pub fn background_tasks(app: &mut LightcraftApp) -> Vec<TaskRow> {
    let mut v = Vec::new();
    if let Some(t) = &app.import {
        let s = t.status();
        v.push(TaskRow {
            key: "import".into(),
            label: "Importing photos".into(),
            done: s["done"].as_u64().unwrap_or(0) as usize,
            total: s["total"].as_u64().unwrap_or(0) as usize,
            note: format!("{} added", s["imported"].as_u64().unwrap_or(0)),
            cancelling: s["cancelled"].as_bool() == Some(true),
        });
    }
    if let Some(t) = &app.scan {
        let s = t.status();
        v.push(TaskRow {
            key: "scan".into(),
            label: "Looking for photos".into(),
            done: s["done"].as_u64().unwrap_or(0) as usize,
            total: s["total"].as_u64().unwrap_or(0) as usize,
            note: String::new(),
            cancelling: false,
        });
    }
    let with_progress: Vec<Value> = app.tasks.progress_json().as_array().cloned().unwrap_or_default();
    for t in &with_progress {
        let label = t["label"].as_str().unwrap_or("").to_string();
        v.push(TaskRow {
            key: format!("task:{label}"),
            label,
            done: t["done"].as_u64().unwrap_or(0) as usize,
            total: t["total"].as_u64().unwrap_or(0) as usize,
            note: t["current"].as_str().unwrap_or("").to_string(),
            cancelling: t["cancelling"].as_bool() == Some(true),
        });
    }
    for label in app.tasks.labels() {
        if !with_progress.iter().any(|t| t["label"].as_str() == Some(label.as_str())) {
            v.push(TaskRow { key: format!("task:{label}"), label, done: 0, total: 0, note: String::new(), cancelling: false });
        }
    }
    if let Ok(p) = app.session.execute("library.previewProgress", &json!({}))
        && p["running"].as_bool() == Some(true)
    {
        v.push(TaskRow {
            key: "previews".into(),
            label: "Building previews".into(),
            done: p["done"].as_u64().unwrap_or(0) as usize,
            total: p["total"].as_u64().unwrap_or(0) as usize,
            note: p["what"].as_str().unwrap_or("").to_string(),
            cancelling: p["cancelled"].as_bool() == Some(true),
        });
    }
    if let Ok(st) = app.session.execute("people.status", &json!({}))
        && st["job"]["running"].as_bool() == Some(true)
    {
        v.push(TaskRow {
            key: "people".into(),
            label: "Reading the faces".into(),
            done: st["job"]["done"].as_u64().unwrap_or(0) as usize,
            total: st["job"]["total"].as_u64().unwrap_or(0) as usize,
            note: String::new(),
            cancelling: false,
        });
    }
    if let Some(q) = crate::quick_sort::progress(app) {
        v.push(q);
    }
    v
}

/// Stop the task `key` (the page's Cancel button).
pub fn cancel_task(app: &mut LightcraftApp, key: &str) {
    match key {
        "import" => {
            if let Some(t) = app.import.as_mut() {
                t.request_cancel();
            }
        }
        "scan" => app.scan = None,
        "previews" => {
            let _ = app.session.execute("library.cancelPreviews", &json!({}));
        }
        "people" => {
            let _ = app.session.execute("faces.pump", &json!({"cancel": true}));
        }
        "quick" => crate::quick_sort::cancel(app),
        k => {
            if let Some(label) = k.strip_prefix("task:") {
                app.tasks.cancel(label);
            }
        }
    }
}

/// The list of background tasks, with a progress bar and Cancel on each (`button:taskCancel:<key>`).
pub fn tasks_panel(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let rows = app.workspaces.home.tasks.clone();
    heading(ui, &t, "Background tasks");
    if rows.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("Nothing is running.")).color(t.text_dim).size(12.0));
        return;
    }
    let mut cancel: Option<String> = None;
    for r in &rows {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr(&r.label)).color(t.text).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if r.cancelling { "Stopping…" } else { "Cancel" };
                let b = text_button(ui, &format!("taskCancel:{}", r.key), crate::i18n::tr(label), false);
                if b.clicked() && !r.cancelling {
                    cancel = Some(r.key.clone());
                }
            });
        });
        if r.total > 0 {
            ui.add(egui::ProgressBar::new(r.done as f32 / r.total as f32).desired_height(6.0));
            ui.label(egui::RichText::new(format!("{} of {}  {}", r.done, r.total, r.note)).color(t.text_dim).size(11.0));
        } else {
            ui.add(egui::ProgressBar::new(0.0).animate(true).desired_height(6.0));
            if !r.note.is_empty() {
                ui.label(egui::RichText::new(&r.note).color(t.text_dim).size(11.0));
            }
        }
    }
    if let Some(k) = cancel {
        cancel_task(app, &k);
        app.workspaces.home.tasks_at = f64::NEG_INFINITY;
    }
}

// --------------------------------------------------------------------------------------------- resume

/// The import days of the library, newest first (cached while the catalog is unchanged).
fn resume_list(app: &mut LightcraftApp) -> Vec<Resume> {
    let key = (app.session.catalog.revision, app.session.library_generation());
    if let Some((k, v)) = &app.workspaces.home.resume
        && *k == key
    {
        return v.clone();
    }
    let mut days: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for p in app.session.catalog.photos().filter(|p| p.in_library()) {
        let day: String = p.imported.chars().take(10).collect();
        if day.len() < 10 {
            continue;
        }
        let e = days.entry(day).or_default();
        e.0 += 1;
        e.1 += usize::from(p.rating > 0 || p.flag != lightcraft_catalog::Flag::None);
    }
    let v: Vec<Resume> = days.into_iter().rev().take(5).map(|(day, (photos, sorted))| Resume { day, photos, sorted }).collect();
    app.workspaces.home.resume = Some((key, v.clone()));
    v
}

// -------------------------------------------------------------------------------------------- drawing

pub(crate) fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(egui::RichText::new(crate::i18n::tr(text)).font(t.semibold(14.0)).color(t.text));
    ui.add_space(4.0);
}

/// A clickable card with a title and a detail line; `id` is registered as `home:<id>`.
fn card(ui: &mut egui::Ui, id: &str, title: &str, detail: &str, action: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 58.0), Sense::click());
    register(ui, format!("home:{id}"), r);
    let fill = if resp.hovered() { t.hover } else { t.inset };
    ui.painter().rect(r, 6.0, fill, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    ui.painter().text(r.min + vec2(12.0, 14.0), Align2::LEFT_CENTER, title, t.semibold(13.0), t.text);
    ui.painter().text(r.min + vec2(12.0, 32.0), Align2::LEFT_CENTER, detail, t.font(11.5), t.text_dim);
    ui.painter().text(r.min + vec2(12.0, 47.0), Align2::LEFT_CENTER, format!("{action} →"), t.font(11.5), t.accent);
    resp.clicked()
}

/// The page: three columns inside the centre of the window.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    ensure_loaded(app);
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    register(ui, "region:home", full);
    let left = Rect::from_min_max(full.min, pos2(full.min.x + LEFT_W, full.max.y));
    let right_w = if full.width() > 1100.0 { RIGHT_W } else { 300.0 };
    let right = Rect::from_min_max(pos2(full.max.x - right_w, full.min.y), full.max);
    let centre = Rect::from_min_max(pos2(left.max.x, full.min.y), pos2(right.min.x, full.max.y));
    ui.painter().rect_filled(left, 0.0, t.chrome);
    ui.painter().rect_filled(right, 0.0, t.chrome);
    ui.painter().line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0, t.divider));
    ui.painter().line_segment([right.left_top(), right.left_bottom()], Stroke::new(1.0, t.divider));
    let pad = vec2(16.0, 14.0);
    ui.scope_builder(UiBuilder::new().max_rect(left.shrink2(pad)), |ui| left_column(app, ui));
    ui.scope_builder(UiBuilder::new().max_rect(centre.shrink2(pad)), |ui| centre_column(app, ui));
    ui.scope_builder(UiBuilder::new().max_rect(right.shrink2(pad)), |ui| right_column(app, ui));
}

fn left_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    heading(ui, &t, "Resume");
    let mut go: Option<Value> = None;
    let visible = app.session.visible().len();
    let source = crate::i18n::source_title(&app.session);
    if app.session.catalog.photos().any(|p| p.in_library()) {
        let detail = crate::i18n::tr_format!("{source} · {visible} photos", source = source, visible = visible);
        if card(ui, "resumeCurrent", "Where you left off", &detail, "Open in Sort") {
            go = Some(json!({}));
        }
        ui.add_space(6.0);
    }
    for (i, r) in resume_list(app).iter().enumerate() {
        let detail = crate::i18n::tr_format!(
            "{photos} photos · {sorted} sorted · {todo} to sort",
            photos = r.photos,
            sorted = r.sorted,
            todo = r.photos - r.sorted
        );
        if card(ui, &format!("resumeDay{i}"), &crate::i18n::tr_format!("Imported {day}", day = r.day), &detail, "Open in Sort") {
            go = Some(json!({"day": r.day}));
        }
        ui.add_space(6.0);
    }
    if let Some(p) = go {
        let _ = app.run("workspace.home.resume", p);
    }
    ui.add_space(10.0);
    tasks_panel(app, ui);
}

fn centre_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    heading(ui, &t, "Import photos");
    // the drop zone
    let w = ui.available_width();
    let (zone, _) = ui.allocate_exact_size(vec2(w, 150.0), Sense::hover());
    register(ui, "home:dropzone", zone);
    let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let colour = if hovering { t.accent } else { t.field_border };
    let c = [zone.left_top(), zone.right_top(), zone.right_bottom(), zone.left_bottom(), zone.left_top()];
    ui.painter().extend(egui::Shape::dashed_line(&c, Stroke::new(if hovering { 2.0 } else { 1.0 }, colour), 7.0, 5.0));
    let mut scan_paths: Option<Vec<String>> = None;
    let mut device = false;
    let scanning = app.scan.is_some();
    let importing = app.import.is_some();
    ui.scope_builder(UiBuilder::new().max_rect(zone.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
        ui.add_space(10.0);
        if scanning {
            let s = app.scan.as_ref().map(|s| s.status()).unwrap_or_default();
            let (done, total) = (s["done"].as_u64().unwrap_or(0), s["total"].as_u64().unwrap_or(0));
            let text = if total == 0 { "Looking for photos…".to_string() } else { format!("Reading photos… {done} of {total}") };
            ui.label(egui::RichText::new(text).font(t.semibold(14.0)).color(t.text));
            ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(260.0));
            return;
        }
        ui.label(egui::RichText::new(crate::i18n::tr("Drop a folder, a memory card or photos here")).font(t.semibold(15.0)).color(t.text));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(crate::i18n::tr("Photos the library already has are set aside before you import.")).color(t.text_dim).size(12.0),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // centre the row of buttons
            let devices = lightcraft_engine::devices::devices();
            ui.spacing_mut().item_spacing.x = 8.0;
            let n = 2 + devices.len().min(2);
            ui.add_space(((ui.available_width() - n as f32 * 150.0) / 2.0).max(0.0));
            if app.services.pick_folder.is_some() && text_button(ui, "homeChooseFolder", crate::i18n::tr("Choose a folder…"), false).clicked() {
                scan_paths = app.services.pick_folder.as_mut().and_then(|f| f()).map(|d| vec![d]);
            }
            if app.services.pick_files.is_some() && text_button(ui, "homeChoosePhotos", crate::i18n::tr("Choose photos…"), false).clicked() {
                scan_paths = app.services.pick_files.as_mut().map(|f| f()).filter(|v| !v.is_empty());
            }
            for (i, d) in devices.iter().take(2).enumerate() {
                if text_button(ui, &format!("homeDevice{i}"), &format!("From card “{}”", d.name), false).clicked() {
                    scan_paths = Some(vec![d.path.clone()]);
                    device = true;
                }
            }
        });
    });
    if let Some(paths) = scan_paths
        && !importing
    {
        let r = if device {
            app.run("workspace.home.device", json!({"path": paths[0]}))
        } else {
            app.run("workspace.home.scan", json!({"paths": paths}))
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
    ui.add_space(14.0);
    // what was found
    let h = &app.workspaces.home;
    if !h.scanned || h.d.candidates.is_empty() {
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "Nothing is imported until you press the blue button; a scan only lists what is there. Photos arrive on the grid as soon as the first ones are copied.",
            ))
            .color(t.text_dim)
            .size(12.0),
        );
        if let Some(last) = &h.last {
            ui.add_space(8.0);
            let first = last["firstPhotoMs"].as_f64().map(|m| format!("{:.1} s to the first photo", m / 1000.0)).unwrap_or_default();
            ui.label(egui::RichText::new(format!("Last import: {} photos · {first}", last["imported"])).color(t.text_label).size(12.0));
        }
        return;
    }
    let n = h.d.candidates.len();
    let sel = h.d.selected_paths().len();
    let dups = h.d.candidates.iter().filter(|c| c.duplicate.is_some()).count();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("Preview of what will be imported — {n} found · {sel} selected")).font(t.semibold(13.0)).color(t.text));
        if dups > 0 {
            ui.label(egui::RichText::new(format!("· {dups} already in the library")).color(t.text_dim));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if text_button(ui, "homeUncheckAll", crate::i18n::tr("Uncheck All"), false).clicked() {
                app.workspaces.home.d.checked.iter_mut().for_each(|c| *c = false);
            }
            if text_button(ui, "homeCheckAll", crate::i18n::tr("Check All"), false).clicked() {
                let d = &mut app.workspaces.home.d;
                for i in 0..n {
                    d.checked[i] = d.importable(i);
                }
            }
        });
    });
    ui.add_space(6.0);
    let cell = 104.0;
    let cols = ((ui.available_width() + 6.0) / (cell + 6.0)).floor().max(1.0) as usize;
    let shown = n.min(PREVIEW_MAX);
    let rows = shown.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * (cell + 24.0)), Sense::hover());
    // (the candidate cell writes into the dialog struct: take it out for the frame)
    let mut d = std::mem::take(&mut app.workspaces.home.d);
    for i in 0..shown {
        let (c, r) = (i % cols, i / cols);
        let rect = Rect::from_min_size(area.min + vec2(c as f32 * (cell + 6.0), r as f32 * (cell + 24.0)), vec2(cell, cell + 18.0));
        crate::import::candidate_cell(app, ui, &mut d, i, rect);
    }
    app.workspaces.home.d = d;
    if n > shown {
        ui.label(
            egui::RichText::new(format!("+ {} more files (all are imported unless unchecked in More options…)", n - shown))
                .color(t.text_dim)
                .size(12.0),
        );
    }
}

fn right_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    heading(ui, &t, "Import settings");
    let scanned = app.workspaces.home.scanned && !app.workspaces.home.d.candidates.is_empty();
    let mut d = std::mem::take(&mut app.workspaces.home.d);
    crate::import::field(ui, "Transfer", |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if text_button(ui, "homeAdd", crate::i18n::tr("Add in place"), !d.copy).clicked() {
            (d.copy, d.move_files) = (false, false);
        }
        let can = app.session.library.as_ref().is_some_and(|l| l.on_disk) || app.services.pick_folder.is_some();
        if ui.add_enabled_ui(can, |ui| text_button(ui, "homeCopy", crate::i18n::tr("Copy"), d.copy && !d.move_files)).inner.clicked() {
            (d.copy, d.move_files) = (true, false);
        }
        if ui.add_enabled_ui(can, |ui| text_button(ui, "homeMove", crate::i18n::tr("Move"), d.copy && d.move_files)).inner.clicked() {
            (d.copy, d.move_files) = (true, true);
        }
    });
    if d.copy {
        crate::import::field(ui, "Copy to", |ui| {
            let shown = if d.destination.trim().is_empty() { crate::i18n::tr("Library Originals").to_string() } else { d.destination.clone() };
            ui.add(egui::Label::new(egui::RichText::new(shown).color(t.text_label)).truncate());
            if app.services.pick_folder.is_some()
                && text_button(ui, "homeDest", crate::i18n::tr("Choose…"), false).clicked()
                && let Some(f) = app.services.pick_folder.as_mut().and_then(|f| f())
            {
                d.destination = f;
            }
        });
        crate::import::field(ui, "Folders", |ui| {
            let opts = [("date", "By day"), ("month", "By month"), ("flat", "One folder")];
            let key = if d.organize.is_empty() || d.organize == "custom" { "date".to_string() } else { d.organize.clone() };
            let cur = opts.iter().find(|o| o.0 == key).map_or(opts[0].1, |o| o.1);
            let named = crate::widgets::combo_box("home-organize").selected_text(crate::i18n::tr(cur)).show_ui(ui, |ui| {
                for (k, label) in opts {
                    if ui.selectable_label(key == k, crate::i18n::tr(label)).clicked() {
                        d.organize = k.to_string();
                    }
                }
            });
            crate::widgets::name_combo(&named.response, crate::i18n::tr("Organize into"));
        });
    }
    crate::import::album_field(app, ui, &mut d);
    crate::import::preset_field(app, ui, &mut d);
    crate::import::field(ui, "Keywords", |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut d.keywords).hint_text(crate::i18n::tr("comma, separated")).desired_width(f32::INFINITY));
        register(ui, "field:homeKeywords", r.rect);
    });
    app.workspaces.home.d = d;
    ui.add_space(10.0);
    heading(ui, &t, "While you start sorting (in the background)");
    let faces_ready = app.session.execute("people.status", &json!({})).ok().is_some_and(|s| s["model"]["installed"].as_bool() == Some(true));
    for (i, (key, label)) in AFTER.iter().enumerate() {
        let enabled = *key != "faces" || faces_ready;
        let mut on = app.workspaces.home.after[i] && enabled;
        let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, crate::i18n::tr(label)));
        register(ui, format!("check:homeAfter-{key}"), r.rect);
        if enabled {
            app.workspaces.home.after[i] = on;
        } else {
            r.on_disabled_hover_text(crate::i18n::tr("The People model is not installed (Photo → Sorting Help or People → Recognition set it up)."));
        }
    }
    ui.add_space(12.0);
    let sel = app.workspaces.home.d.selected_paths().len();
    let can = scanned && sel > 0 && app.import.is_none();
    let mover = app.workspaces.home.d.copy && app.workspaces.home.d.move_files;
    let label = match (mover, sel) {
        (false, 1) => crate::i18n::tr("Import 1 photo and open Sort").to_string(),
        (true, 1) => crate::i18n::tr("Move 1 photo and open Sort").to_string(),
        (false, n) => crate::i18n::tr_format!("Import {n} photos and open Sort", n = n),
        (true, n) => crate::i18n::tr_format!("Move {n} photos and open Sort", n = n),
    };
    let primary = crate::design::button(ui, "homeImport", &label, crate::design::Kind::Primary, can);
    if primary.clicked() {
        match app.run("workspace.home.import", json!({})) {
            Ok(_) => {}
            Err(e) => app.ui.status = e,
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled_ui(scanned, |ui| text_button(ui, "homeMore", crate::i18n::tr("More options…"), false))
            .inner
            .on_hover_text(crate::i18n::tr("Folder template, DNG, metadata presets and everything else the classic import window has"))
            .clicked()
        {
            let _ = app.run("workspace.home.moreOptions", json!({}));
        }
        if ui.add_enabled_ui(scanned, |ui| text_button(ui, "homeClear", crate::i18n::tr("Clear"), false)).inner.clicked() {
            let _ = app.run("workspace.home.clear", json!({}));
        }
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "Everything ticked here is remembered for the next import. Following steps run only on the photos this import adds.",
        ))
        .color(t.text_dim)
        .size(11.5),
    );
}

/// `ui.inspect` → `workspaces.home`.
pub fn inspect(app: &LightcraftApp) -> Value {
    let h = &app.workspaces.home;
    json!({
        "scanned": h.scanned,
        "candidates": h.d.candidates.len(),
        "selected": h.d.selected_paths().len(),
        "settings": remembered(&h.d, &h.after),
        "importing": h.started.as_ref().map(|s| json!({"files": s.files, "firstPhotoMs": s.first_photo_ms})),
        "last": h.last,
        "tasks": h.tasks.iter().map(|t| json!({"key": t.key, "done": t.done, "total": t.total})).collect::<Vec<_>>(),
    })
}
