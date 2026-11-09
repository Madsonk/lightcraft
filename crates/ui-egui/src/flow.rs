//! The IMPORT → CULL → EDIT → RETOUCH flow (lot D, LightCraft × Aftershoot): the state and the
//! background work behind the "AI picks" section of the Library sidebar (`panels/cull.rs`), the
//! portrait retouch panel (`panels/portrait.rs`), the one-click button, the guided tour and the
//! "next time, try the key…" hints. It owns no engine logic: everything goes through the commands
//! `cull.*` and `portrait.*` (`app.run`), exactly as the CLI and MCP do.
//!
//! Never freezes the window: the cull runs in the engine's own thread (`cull.status` is polled
//! ten times a second); the retouch of an album is one photo per frame with a progress panel and a
//! Stop button between two photos (the retouch of one photo itself runs on the UI thread, see
//! `docs/ui-flux.md` → limits).

use std::collections::BTreeMap;

use egui::{Align2, RichText, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::portrait_job::{PortraitPreview, PortraitRun};
use lightcraft_engine::portrait_stack as ps;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::text_button;

/// Everything the flow remembers between frames (not saved: the verdicts live in the catalog).
#[derive(Default)]
pub struct Flow {
    pub cull: Cull,
    pub portrait: Portrait,
    /// The album retouch / one-click run in progress.
    pub job: Option<Job>,
    /// The one-click confirmation window is open (what it will do, in numbers).
    pub confirm: Option<Confirm>,
    /// What the last run did, with the button that undoes all of it at once.
    pub summary: Option<Summary>,
    /// The guided tour step on screen.
    pub tour: Option<usize>,
}

#[derive(Default)]
pub struct Cull {
    pub status: Value,
    pub running: bool,
    /// `cull.list` of the whole view at the time of the last run (rows, scenes, counts).
    pub list: Option<Value>,
    /// Ids the list covers (so a filter can be applied without asking again).
    pub all_ids: Vec<u64>,
    pub dirty: bool,
    /// The bucket / scene the view is narrowed to (the view's `only` list is ours while set).
    pub bucket: Option<String>,
    pub scene: Option<u64>,
    pub person: Option<String>,
    pub genre: String,
    pub grouping: f32,
    pub key_people: Vec<String>,
    /// People of the library (`p:N`, name), loaded on demand.
    pub people: Vec<(String, String)>,
    pub people_loaded: bool,
    pub last_poll: f64,
    /// Show the duplicate groups.
    pub dup_open: bool,
}

#[derive(Default)]
pub struct Portrait {
    pub loaded_for: Option<u64>,
    pub sliders: Vec<Slider>,
    pub amounts: BTreeMap<String, f32>,
    pub background_mode: String,
    /// Before / After of the last preview.
    pub pair: Option<Pair>,
    /// The preview being made on a worker (the window keeps answering).
    pub preview: Option<PortraitPreview>,
    /// Apply / Auto retouch of the active photo being made on a worker; `true` = auto.
    pub apply: Option<(PortraitRun, bool)>,
    pub want_eyes: bool,
    pub eyes: Option<Pair>,
    /// One line about the last action ("2 faces · 38 % of the face changed").
    pub note: String,
    pub error: String,
    pub busy_frames: u8,
}

#[derive(Clone)]
pub struct Slider {
    pub id: String,
    pub label: String,
    pub group: String,
    pub auto: f32,
}

pub struct Pair {
    pub id: u64,
    pub before: egui::TextureHandle,
    pub after: egui::TextureHandle,
    pub size: [usize; 2],
    pub info: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Culling,
    Retouching,
}

/// An album retouch or the whole one-click run.
pub struct Job {
    pub one_click: bool,
    pub phase: Phase,
    /// The retouch itself, on a worker, one photo at a time.
    pub run: Option<PortraitRun>,
    pub total: usize,
    pub done: usize,
    pub undo0: usize,
    pub stop: bool,
    pub judged: Value,
    pub applied: Value,
    pub started: web_time::Instant,
}

pub struct Confirm {
    pub photos: usize,
    pub with_selection: bool,
}

pub struct Summary {
    pub one_click: bool,
    pub lines: Vec<String>,
    pub undone: bool,
    pub stopped: bool,
}

// ------------------------------------------------------------------------------------------ hints

/// "Next time, try the key X": shown once per `id`, only when the action was done with the mouse.
pub fn tip(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, key: &str) {
    if key.is_empty() || app.ui.flow_tips.iter().any(|t| t == id) {
        return;
    }
    app.ui.flow_tips.push(id.to_string());
    app.toast_for(ctx, tr_format!("Next time, try the {key} key", key = key), 4.5);
}

/// The key of a UI command as the user's keymap has it.
pub fn key_of(app: &LightcraftApp, command: &str) -> String {
    crate::shortcuts::shortcut_of(&app.ui.settings.keymap, command).unwrap_or_default().to_string()
}

// ------------------------------------------------------------------------------------------ cull

fn visible_ids(app: &mut LightcraftApp) -> Vec<u64> {
    app.session.visible().iter().map(|i| i.0).collect()
}

/// Start AI Culling on the photos of the view (background; progress in the panel and the overlay).
pub fn start_cull(app: &mut LightcraftApp, ctx: &egui::Context) -> Result<(), String> {
    if app.flow.cull.running {
        return Ok(());
    }
    let ids = if app.flow.cull.bucket.is_some() || app.flow.cull.scene.is_some() || app.flow.cull.person.is_some() {
        app.flow.cull.all_ids.clone()
    } else {
        visible_ids(app)
    };
    if ids.is_empty() {
        return Err(tr("There are no photos to judge").to_string());
    }
    let c = &app.flow.cull;
    let mut p = json!({"ids": ids, "limit": ids.len().max(5000)});
    if !c.genre.is_empty() {
        p["genre"] = json!(c.genre);
        p["grouping"] = json!(c.grouping);
    }
    if !c.key_people.is_empty() {
        p["keyPeople"] = json!(c.key_people);
    }
    app.run("cull.run", p)?;
    let c = &mut app.flow.cull;
    c.running = true;
    c.all_ids = ids;
    c.last_poll = 0.0;
    ctx.request_repaint();
    Ok(())
}

pub fn cancel_cull(app: &mut LightcraftApp) {
    let _ = app.run("cull.cancel", json!({}));
}

/// Narrow the view to one bucket, one scene or one person (or back to everything).
pub fn set_filter(app: &mut LightcraftApp, bucket: Option<&str>, scene: Option<u64>) {
    let c = &mut app.flow.cull;
    c.bucket = bucket.map(str::to_string);
    c.scene = scene;
    c.person = None;
    if bucket.is_none() && scene.is_none() {
        app.session.filter.only.clear();
        return;
    }
    let mut p = json!({"ids": c.all_ids, "filter": true, "limit": 0});
    if let Some(b) = bucket {
        p["bucket"] = json!([b]);
    }
    if let Some(s) = scene {
        p["scene"] = json!(s);
    }
    let _ = app.run("cull.list", p);
}

pub fn set_person_filter(app: &mut LightcraftApp, person: Option<&str>) {
    app.flow.cull.bucket = None;
    app.flow.cull.scene = None;
    app.flow.cull.person = person.map(str::to_string);
    app.session.filter.only.clear();
    app.session.filter.person = person.map(str::to_string);
}

/// The people named on faces in the library (`people` crate-free: the catalog's named face
/// regions, which is where the People view of 0.14.0 writes its names), as `(id, name)`.
pub fn load_people(app: &mut LightcraftApp) {
    if app.flow.cull.people_loaded {
        return;
    }
    app.flow.cull.people_loaded = true;
    app.flow.cull.people = app.session.catalog.people().into_iter().map(|p| (p.name.clone(), p.name)).collect();
}

/// How many of each bucket the last run found, `(highlight, selected, review, rejected)`.
pub fn counts(app: &LightcraftApp) -> Option<[u64; 4]> {
    let c = app.flow.cull.list.as_ref()?;
    let n = |k: &str| c["counts"][k].as_u64().unwrap_or(0);
    Some([n("highlight"), n("selected"), n("review"), n("rejected")])
}

/// The duplicate groups of the last run: `(scene, group)` → members `(id, name, rank, score, bucket)`.
pub fn duplicate_groups(app: &LightcraftApp) -> BTreeMap<(u64, u64), Vec<(u64, String, u64, f64, String)>> {
    let mut out: BTreeMap<(u64, u64), Vec<(u64, String, u64, f64, String)>> = BTreeMap::new();
    let Some(rows) = app.flow.cull.list.as_ref().and_then(|l| l["photos"].as_array()) else { return out };
    for r in rows {
        let (Some(g), Some(id)) = (r["group"].as_u64(), r["id"].as_u64()) else { continue };
        if r["groupSize"].as_u64().unwrap_or(0) < 2 {
            continue;
        }
        out.entry((r["scene"].as_u64().unwrap_or(0), g)).or_default().push((
            id,
            r["name"].as_str().unwrap_or("").to_string(),
            r["groupRank"].as_u64().unwrap_or(0),
            r["score"].as_f64().unwrap_or(0.0),
            r["bucket"].as_str().unwrap_or("").to_string(),
        ));
    }
    for v in out.values_mut() {
        v.sort_by_key(|m| m.2);
    }
    out
}

/// Keep `keep` of a duplicate group: Pick for it, Reject for the others, one undo step.
pub fn keep_one(app: &mut LightcraftApp, ctx: &egui::Context, keep: u64, others: &[u64]) {
    let undo0 = app.session.undo.len();
    let _ = app.run("photo.flag", json!({"flag": "pick", "ids": [keep]}));
    if !others.is_empty() {
        let _ = app.run("photo.flag", json!({"flag": "reject", "ids": others}));
    }
    let n = app.session.undo.len().saturating_sub(undo0);
    app.session.merge_undo(n, "Keep one of a duplicate group");
    app.toast_success(ctx, tr_format!("Kept 1, rejected {n}", n = others.len()));
    tip(app, ctx, "keep-one", "P");
}

/// Write the proposed stars and flags (one undo step).
pub fn apply_cull(app: &mut LightcraftApp, ctx: &egui::Context) {
    let ids = app.flow.cull.all_ids.clone();
    match app.run("cull.apply", json!({"ids": ids})) {
        Ok(v) => {
            app.toast_success(
                ctx,
                tr_format!(
                    "{rated} rated, {picked} picked, {rejected} rejected: one undo step",
                    rated = v["rated"].as_u64().unwrap_or(0),
                    picked = v["picked"].as_u64().unwrap_or(0),
                    rejected = v["rejected"].as_u64().unwrap_or(0)
                ),
            );
            let key = crate::shortcuts::shortcut_of(&app.ui.settings.keymap, "edit.undo").unwrap_or("Cmd+Z").to_string();
            tip(app, ctx, "cull-apply", &key);
        }
        Err(e) => app.toast_error(ctx, e),
    }
}

// --------------------------------------------------------------------------------------- portrait

pub fn load_portrait(app: &mut LightcraftApp) {
    let active = app.session.active().map(|i| i.0);
    if app.flow.portrait.loaded_for == active && !app.flow.portrait.sliders.is_empty() {
        return;
    }
    let Ok(v) = app.run("portrait.list", json!({})) else { return };
    let p = &mut app.flow.portrait;
    p.loaded_for = active;
    p.sliders.clear();
    p.amounts.clear();
    if p.background_mode.is_empty() {
        p.background_mode = "blur".into();
    }
    for s in v["sliders"].as_array().into_iter().flatten() {
        let (Some(id), Some(label), Some(group)) = (s["id"].as_str(), s["label"].as_str(), s["group"].as_str()) else { continue };
        let current = s["current"]["amount"].as_f64().unwrap_or(0.0) as f32;
        p.amounts.insert(id.to_string(), current);
        p.sliders.push(Slider { id: id.into(), label: label.into(), group: group.into(), auto: s["auto"].as_f64().unwrap_or(0.0) as f32 });
    }
    p.pair = None;
    p.eyes = None;
    p.note.clear();
    p.error.clear();
}

/// The `portrait.apply` / `portrait.preview` parameters for the sliders as they are now.
pub fn portrait_params(app: &LightcraftApp) -> Value {
    let p = &app.flow.portrait;
    let mut o = serde_json::Map::new();
    for s in &p.sliders {
        let a = p.amounts.get(&s.id).copied().unwrap_or(0.0).round();
        if s.id == "background" {
            if a > 0.0 {
                o.insert("background".into(), json!({"mode": p.background_mode, "amount": a}));
            }
        } else {
            o.insert(s.id.clone(), json!(a));
        }
    }
    Value::Object(o)
}

fn texture_of(ctx: &egui::Context, name: &str, path: &str) -> Option<(egui::TextureHandle, [usize; 2])> {
    let bytes = std::fs::read(path).ok()?;
    let t = lightcraft_codecs::decode_thumbnail(&bytes, 1024).ok()?;
    let (w, h) = (t.image.width as usize, t.image.height as usize);
    let color = crate::render::color_image(&t.image);
    Some((ctx.load_texture(name, color, egui::TextureOptions::LINEAR), [w, h]))
}

fn scratch(name: &str) -> String {
    std::env::temp_dir().join(format!("lightcraft-flow-{}-{name}", std::process::id())).to_string_lossy().into_owned()
}

/// The steps the sliders ask for, as the engine takes them.
fn slider_steps(app: &LightcraftApp) -> Result<Vec<lightcraft_develop::PortraitStep>, String> {
    let p = portrait_params(app);
    ps::steps_from_params(&p, "portrait").map_err(|e| e.to_string())
}

/// Start the Before / After preview of the active photo on a worker: the window keeps answering and
/// [`tick`] picks the result up.
pub fn start_preview(app: &mut LightcraftApp) {
    let Some(id) = app.session.active() else { return };
    let steps = match slider_steps(app) {
        Ok(s) if s.iter().any(|st| st.amount > 0.0) => s,
        Ok(_) => {
            app.flow.portrait.error = tr("Move a slider first, or press Auto retouch").to_string();
            return;
        }
        Err(e) => {
            app.flow.portrait.error = e;
            return;
        }
    };
    app.flow.portrait.error.clear();
    match PortraitPreview::start(&mut app.session, id, steps, false, 4, ps::PREVIEW_EDGE) {
        Ok(pv) => app.flow.portrait.preview = Some(pv),
        Err(e) => app.flow.portrait.error = e.to_string(),
    }
}

/// Per frame: the preview and the single-photo apply, if they are on a worker.
fn pump_portrait(app: &mut LightcraftApp, ctx: &egui::Context) {
    if let Some(pv) = &mut app.flow.portrait.preview
        && let Some(r) = pv.pump(&mut app.session)
    {
        app.flow.portrait.preview = None;
        let id = app.session.active().map_or(0, |i| i.0);
        match r {
            Ok(d) => {
                let size = [d.after.width, d.after.height];
                let opts = egui::TextureOptions::LINEAR;
                let before = ctx.load_texture("portrait-before", crate::render::color_image(&d.before), opts);
                let after = ctx.load_texture("portrait-after", crate::render::color_image(&d.after), opts);
                let pr = &mut app.flow.portrait;
                pr.pair = Some(Pair { id, before, after, size, info: String::new() });
                pr.note = tr_format!("{faces} face(s) found", faces = d.faces);
                pr.error.clear();
            }
            Err(e) => app.flow.portrait.error = e.to_string(),
        }
    }
    let finished = match &mut app.flow.portrait.apply {
        Some((run, _)) => run.pump(&mut app.session),
        None => false,
    };
    if finished && let Some((run, auto)) = app.flow.portrait.apply.take() {
        let _ = auto;
        let (retouched, _, failed) = run.counts();
        if let Some(e) = run.photos.iter().find_map(|p| p["error"].as_str()) {
            app.flow.portrait.error = e.to_string();
        } else if retouched == 0 && !run.cancelled {
            app.flow.portrait.error = tr("No face found in this photo").to_string();
        } else if retouched > 0 && failed == 0 {
            let faces = run.photos.first().and_then(|p| p["faces"].as_u64()).unwrap_or(0);
            app.toast_success(ctx, tr_format!("Portrait retouch applied ({faces} face(s)): one undo step", faces = faces));
            let key = crate::shortcuts::shortcut_of(&app.ui.settings.keymap, "edit.undo").unwrap_or("Cmd+Z").to_string();
            tip(app, ctx, "portrait-apply", &key);
        }
        app.flow.portrait.loaded_for = None;
        app.flow.portrait.pair = None;
        load_portrait(app);
    }
}

pub fn run_eyes_now(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(id) = app.session.active().map(|i| i.0) else { return };
    let after = scratch("eyes-after.png");
    let params = json!({"id": id, "edge": 1024, "path": after});
    let mut result = app.run("portrait.openEyes", params.clone());
    // the faces of the burst must have been analysed (eye openness): do it once, then ask again
    if let Err(e) = &result
        && e.contains("analyzeFaces")
    {
        let ids: Vec<u64> = visible_ids(app).into_iter().take(60).collect();
        let _ = app.run("photo.analyzeFaces", json!({"ids": ids}));
        result = app.run("portrait.openEyes", params);
    }
    match result {
        Ok(v) => {
            let before = v["before"].as_str().map(str::to_string).unwrap_or_default();
            let after = v["after"].as_str().map(str::to_string).unwrap_or(after);
            match (texture_of(ctx, "eyes-before", &before), texture_of(ctx, "eyes-after", &after)) {
                (Some((b, size)), Some((a, _))) => {
                    let info = tr_format!(
                        "Eyes open: {before} → {after}",
                        before = v["openBefore"].as_f64().map_or("?".to_string(), |x| format!("{x:.2}")),
                        after = v["openAfter"].as_f64().map_or("?".to_string(), |x| format!("{x:.2}"))
                    );
                    app.flow.portrait.eyes = Some(Pair { id, before: b, after: a, size, info });
                    app.flow.portrait.error.clear();
                }
                _ => app.flow.portrait.error = tr("The preview could not be read").to_string(),
            }
        }
        Err(e) => {
            app.flow.portrait.eyes = None;
            app.flow.portrait.error = if e.contains("no face with closed eyes") {
                tr("No closed eyes found in this photo").to_string()
            } else if e.contains("no reference photo") {
                tr("No other shot of the burst has this person's eyes open").to_string()
            } else {
                e
            };
        }
    }
}

/// Store the sliders' retouch (or the default recipe) on the active photo on a worker; one undo step.
pub fn apply_portrait(app: &mut LightcraftApp, auto: bool) {
    let Some(id) = app.session.active() else { return };
    if app.flow.portrait.apply.is_some() || app.flow.job.is_some() {
        return;
    }
    let steps = if auto {
        ps::auto_steps()
    } else {
        match slider_steps(app) {
            Ok(s) => s,
            Err(e) => {
                app.flow.portrait.error = e;
                return;
            }
        }
    };
    app.flow.portrait.error.clear();
    app.flow.portrait.apply = Some((PortraitRun::new(vec![id], steps, false, 4), auto));
}

pub fn confirm_eyes(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(id) = app.session.active().map(|i| i.0) else { return };
    match app.run("portrait.openEyes", json!({"id": id, "apply": true, "confirmed": true})) {
        Ok(_) => {
            app.flow.portrait.eyes = None;
            app.toast_success(ctx, tr("Eyes opened from another shot of the burst: one undo step"));
        }
        Err(e) => app.flow.portrait.error = e,
    }
}

// ---------------------------------------------------------------------------------- album and one click

/// Photos a retouch run would touch: the selection when several are selected, else the view.
pub fn targets(app: &mut LightcraftApp) -> (Vec<u64>, bool) {
    if app.session.selection.ids.len() > 1 { (app.session.selection.ids.iter().map(|i| i.0).collect(), true) } else { (visible_ids(app), false) }
}

fn new_job(app: &LightcraftApp, one_click: bool, phase: Phase, run: Option<PortraitRun>) -> Job {
    Job {
        one_click,
        phase,
        total: run.as_ref().map_or(0, |r| r.total),
        run,
        done: 0,
        undo0: app.session.undo.len(),
        stop: false,
        judged: Value::Null,
        applied: Value::Null,
        started: web_time::Instant::now(),
    }
}

/// The portrait steps for the current slider values, or an error if none are set.
pub fn current_portrait_steps(app: &LightcraftApp) -> Result<Vec<lightcraft_develop::PortraitStep>, String> {
    let steps = slider_steps(app)?;
    if steps.iter().any(|st| st.amount > 0.0) {
        Ok(steps)
    } else {
        Err(tr("Move a slider first, or use Auto retouch").to_string())
    }
}

/// Retouch `ids` with explicit `steps`, one photo at a time on a worker (the window never waits).
pub fn start_retouch_with_steps(app: &mut LightcraftApp, ids: Vec<u64>, steps: Vec<lightcraft_develop::PortraitStep>) {
    if app.flow.job.is_some() || ids.is_empty() || steps.is_empty() {
        return;
    }
    app.flow.summary = None;
    app.flow.portrait.error.clear();
    let run = PortraitRun::new(ids.into_iter().map(PhotoId).collect(), steps, false, 4);
    app.flow.job = Some(new_job(app, false, Phase::Retouching, Some(run)));
}

/// Auto-retouch the photos of `ids`: one photo at a time on a worker, the window never waits.
pub fn start_retouch(app: &mut LightcraftApp, ids: Vec<u64>) {
    start_retouch_with_steps(app, ids, ps::auto_steps());
}

pub fn ask_one_click(app: &mut LightcraftApp) {
    if app.flow.job.is_some() || app.flow.cull.running {
        return;
    }
    let (ids, with_selection) = targets(app);
    app.flow.summary = None;
    app.flow.confirm = Some(Confirm { photos: ids.len(), with_selection });
}

/// The user said yes: judge, apply the stars and flags, retouch the selected ones.
pub fn run_one_click(app: &mut LightcraftApp, ctx: &egui::Context) {
    let (ids, _) = targets(app);
    app.flow.confirm = None;
    app.flow.cull.all_ids = ids.clone();
    let job = new_job(app, true, Phase::Culling, None);
    app.flow.job = Some(job);
    if let Err(e) = start_cull(app, ctx) {
        app.flow.job = None;
        app.toast_error(ctx, e);
    }
}

fn finish_job(app: &mut LightcraftApp, ctx: &egui::Context, stopped: bool) {
    let Some(job) = app.flow.job.take() else { return };
    let (retouched, no_face, failed) = job.run.as_ref().map_or((0, 0, 0), |r| r.counts());
    let steps = app.session.undo.len().saturating_sub(job.undo0);
    app.session.merge_undo(steps, if job.one_click { "Cull and retouch in one click" } else { "Portrait retouch" });
    let secs = job.started.elapsed().as_secs_f64();
    let mut lines = Vec::new();
    if job.one_click {
        let j = &job.judged;
        lines.push(tr_format!(
            "{n} photos judged: {hi} highlights, {sel} selected, {rev} to review, {rej} rejected",
            n = j["judged"].as_u64().unwrap_or(0),
            hi = j["counts"]["highlight"].as_u64().unwrap_or(0),
            sel = j["counts"]["selected"].as_u64().unwrap_or(0),
            rev = j["counts"]["review"].as_u64().unwrap_or(0),
            rej = j["counts"]["rejected"].as_u64().unwrap_or(0)
        ));
        lines.push(tr_format!(
            "Stars and flags written: {rated} rated, {picked} picked, {rejected} rejected",
            rated = job.applied["rated"].as_u64().unwrap_or(0),
            picked = job.applied["picked"].as_u64().unwrap_or(0),
            rejected = job.applied["rejected"].as_u64().unwrap_or(0)
        ));
    }
    lines.push(tr_format!("{n} photos retouched, {nf} without a face, {fl} failed", n = retouched, nf = no_face, fl = failed));
    lines.push(format!("{secs:.0} s"));
    app.flow.summary = Some(Summary { one_click: job.one_click, lines, undone: false, stopped });
    app.flow.cull.dirty = true;
    app.flow.portrait.loaded_for = None;
    ctx.request_repaint();
}

/// Undo everything the last run did, in one step.
pub fn undo_all(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.run("edit.undo", json!({})).is_ok() {
        if let Some(s) = &mut app.flow.summary {
            s.undone = true;
        }
        app.flow.cull.dirty = true;
        app.flow.portrait.loaded_for = None;
        app.toast_success(ctx, tr("Everything the run did is undone"));
    }
}

// ------------------------------------------------------------------------------------------ frame

/// Per frame, after the tasks: follow the cull, refresh its list, advance the job.
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    if app.flow.cull.running && now - app.flow.cull.last_poll > 0.1 {
        app.flow.cull.last_poll = now;
        if let Ok(st) = app.run("cull.status", json!({})) {
            let state = st["state"].as_str().unwrap_or("idle").to_string();
            app.flow.cull.status = st.clone();
            if state != "running" {
                app.flow.cull.running = false;
                app.flow.cull.dirty = true;
                match state.as_str() {
                    "done" => {
                        let n = st["total"].as_u64().unwrap_or(0);
                        if app.flow.job.is_none() {
                            app.toast_success(ctx, tr_format!("AI Culling done: {n} photos judged", n = n));
                        }
                    }
                    "failed" => {
                        let e = st["error"].as_str().unwrap_or("").to_string();
                        app.toast_error(ctx, e);
                        app.flow.job = None;
                    }
                    _ => {
                        app.flow.job = None;
                    }
                }
            }
        }
    }
    if app.flow.cull.running || app.flow.job.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    // the list behind the counts, the duplicate groups and the scenes
    if app.flow.cull.dirty && !app.flow.cull.running {
        app.flow.cull.dirty = false;
        let ids = if app.flow.cull.all_ids.is_empty() { visible_ids(app) } else { app.flow.cull.all_ids.clone() };
        if !ids.is_empty() {
            let filtered = app.flow.cull.bucket.is_some() || app.flow.cull.scene.is_some();
            // asking must not touch the view's own narrowing
            if let Ok(l) = app.run("cull.list", json!({"ids": ids, "limit": 50000}))
                && (l["judged"].as_u64().unwrap_or(0) > 0 || !filtered)
            {
                app.flow.cull.list = Some(l);
                app.flow.cull.all_ids = ids;
            }
        }
    }
    step_job(app, ctx);
    pump_portrait(app, ctx);
    if app.flow.portrait.preview.is_some() || app.flow.portrait.apply.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn step_job(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(job) = &mut app.flow.job else { return };
    if job.stop {
        if let Some(run) = &mut job.run {
            run.cancel();
        }
        finish_job(app, ctx, true);
        return;
    }
    match job.phase {
        Phase::Culling => {
            if app.flow.cull.running || app.flow.cull.status["state"].as_str() != Some("done") {
                return;
            }
            // the judging is finished: write the proposals, then take the selected ones
            let ids = app.flow.cull.all_ids.clone();
            let applied = app.run("cull.apply", json!({"ids": ids})).unwrap_or(Value::Null);
            let list = app.run("cull.list", json!({"ids": ids, "limit": 50000})).unwrap_or(Value::Null);
            let picked: Vec<PhotoId> = list["photos"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|r| matches!(r["bucket"].as_str(), Some("highlight" | "selected")))
                        .filter_map(|r| r["id"].as_u64())
                        .map(PhotoId)
                        .collect()
                })
                .unwrap_or_default();
            app.flow.cull.list = Some(list.clone());
            if let Some(job) = &mut app.flow.job {
                job.applied = applied;
                job.judged = list;
                job.total = picked.len();
                job.run = Some(PortraitRun::new(picked, ps::auto_steps(), false, 4));
                job.phase = Phase::Retouching;
            }
        }
        Phase::Retouching => {
            let Some(run) = &mut job.run else { return };
            let finished = run.pump(&mut app.session);
            job.done = run.done;
            job.total = run.total;
            if finished {
                finish_job(app, ctx, false);
            }
        }
    }
}

// --------------------------------------------------------------------------------------- overlays

/// Progress, the one-click confirmation, the summary and the tour bubbles (above everything).
pub fn overlays(app: &mut LightcraftApp, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    // the cull progress: shown in whichever space the user is in
    if app.flow.cull.running {
        let st = &app.flow.cull.status;
        let (done, total) = (st["done"].as_u64().unwrap_or(0) as usize, st["total"].as_u64().unwrap_or(0) as usize);
        let note = if app.flow.job.is_some() { tr("Step 1 of 3: judging the photos").to_string() } else { String::new() };
        if crate::tasks::progress_window(ctx, "aiCulling", 1, "AI Culling", (done, total), &note, false) {
            if app.flow.job.is_some() {
                app.flow.job = None;
            }
            cancel_cull(app);
        }
    } else if let Some(job) = &app.flow.job {
        let note = if job.one_click { tr("Step 3 of 3: retouching the selected photos").to_string() } else { String::new() };
        let (done, total) = (job.done, job.total);
        let label = if job.one_click { "Cull and retouch in one click" } else { "Portrait retouch" };
        if crate::tasks::progress_window(ctx, "portraitJob", 1, label, (done, total), &note, false)
            && let Some(job) = &mut app.flow.job
        {
            job.stop = true;
        }
    }
    // the confirmation
    if let Some(c) = app.flow.confirm.as_ref().map(|c| (c.photos, c.with_selection)) {
        let mut go = false;
        let mut cancel = false;
        egui::Window::new("one-click-confirm")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, [0.0, -40.0])
            .fixed_size([440.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(tr("Cull and retouch in one click")).font(t.semibold(15.0)).color(t.text));
                ui.add_space(6.0);
                ui.label(RichText::new(if c.1 { tr("On the selected photos:") } else { tr("On the photos of this view:") }).color(t.text_label));
                for line in [
                    tr_format!("1. LightCraft judges the {n} photos (sharpness, eyes, exposure, duplicates).", n = c.0),
                    tr("2. It writes the proposed stars and Pick / Reject flags.").to_string(),
                    tr("3. It retouches the highlights and the selected photos with the default portrait recipe.").to_string(),
                ] {
                    ui.label(RichText::new(line).color(t.text));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr("Originals are never touched. Everything can be undone in one step afterwards.")).size(11.5).color(t.text_dim),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    go = crate::design::button(ui, "oneClickGo", tr("Start"), crate::design::Kind::Primary, c.0 > 0).clicked();
                    cancel = text_button(ui, "oneClickCancel", "Cancel", false).clicked();
                });
            });
        crate::widgets::register_geometry(ctx, "region:oneClickConfirm", ctx.content_rect());
        if cancel {
            app.flow.confirm = None;
        }
        if go {
            run_one_click(app, ctx);
        }
    }
    // the summary
    if app.flow.summary.is_some() {
        let mut undo = false;
        let mut close = false;
        if let Some(s) = &app.flow.summary {
            egui::Window::new("flow-summary")
                .title_bar(false)
                .resizable(false)
                .collapsible(false)
                .anchor(Align2::CENTER_CENTER, [0.0, -40.0])
                .fixed_size([460.0, 0.0])
                .show(ctx, |ui| {
                    let title = match (s.one_click, s.stopped) {
                        (true, false) => "Cull and retouch: done",
                        (true, true) => "Cull and retouch: stopped",
                        (false, false) => "Portrait retouch: done",
                        (false, true) => "Portrait retouch: stopped",
                    };
                    ui.label(RichText::new(tr(title)).font(t.semibold(15.0)).color(t.text));
                    ui.add_space(6.0);
                    for l in &s.lines {
                        ui.label(RichText::new(l.as_str()).color(t.text));
                    }
                    if s.undone {
                        ui.label(RichText::new(tr("Undone.")).color(t.text_dim));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if !s.undone {
                            undo = text_button(ui, "summaryUndo", "Undo everything", false).clicked();
                        }
                        close = crate::design::button(ui, "summaryClose", tr("Close"), crate::design::Kind::Primary, true).clicked();
                    });
                });
        }
        if undo {
            undo_all(app, ctx);
        }
        if close {
            app.flow.summary = None;
        }
    }
    tour(app, ctx);
}

// -------------------------------------------------------------------------------------------- tour

const TOUR: &[(&str, &str)] = &[
    ("1 · Import", "Step 1 of the journey bar: bring a card or a folder in. The photos open in Sort."),
    ("2 · Cull", "In Sort, the AI picks section judges the whole album: nothing is written until you apply it."),
    ("3 · Read the verdicts", "Click Best shots, Selected, To review or Rejected to narrow the grid. Hover a photo to see why."),
    ("4 · Duplicates", "Look-alikes are grouped: keep the best one in a click, the others are rejected (one undo)."),
    ("5 · Edit", "The journey bar offers the next step: Retouch the picks opens the Develop space. The filmstrip stays, the selection stays."),
    ("6 · Retouch", "Portrait retouch: sliders by group, Before / After, Auto retouch, open closed eyes."),
    ("7 · One click", "Cull and retouch in one click does steps 2 to 6 on the album, and one Undo takes it all back."),
];

pub fn start_tour(app: &mut LightcraftApp) {
    app.flow.tour = Some(0);
}

fn tour(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(step) = app.flow.tour else { return };
    let t = Tokens::get(ctx);
    let (title, text) = TOUR[step.min(TOUR.len() - 1)];
    let (mut next, mut prev, mut stop) = (false, false, false);
    egui::Window::new("flow-tour")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(Align2::RIGHT_TOP, [-24.0, 96.0])
        .fixed_size([320.0, 0.0])
        .frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(2.0, t.accent)).inner_margin(12).corner_radius(8))
        .show(ctx, |ui| {
            ui.label(RichText::new(tr(title)).font(t.semibold(14.0)).color(t.accent));
            ui.add_space(4.0);
            ui.label(RichText::new(tr(text)).color(t.text));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} / {}", step + 1, TOUR.len())).size(11.0).color(t.text_dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let last = step + 1 >= TOUR.len();
                    next = crate::design::button(ui, "tourNext", if last { tr("Done") } else { tr("Next") }, crate::design::Kind::Primary, true)
                        .clicked();
                    if step > 0 {
                        prev = text_button(ui, "tourPrev", "Back", false).clicked();
                    }
                    stop = text_button(ui, "tourSkip", "Skip", false).clicked();
                });
            });
        });
    let _ = vec2(0.0, 0.0);
    if stop || (next && step + 1 >= TOUR.len()) {
        app.flow.tour = None;
        app.ui.flow_tour_done = true;
    } else if next {
        app.flow.tour = Some(step + 1);
    } else if prev {
        app.flow.tour = Some(step.saturating_sub(1));
    }
}
