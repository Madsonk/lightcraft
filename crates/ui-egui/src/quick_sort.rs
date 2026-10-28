//! Library ▸ Sort: the Quick sort panel (LightCraft v12, phase 3 of the UX study), to the right of the
//! grid. It gathers what LightCraft already knows how to find — soft photos and bursts (Assisted
//! Culling, `photo.analyze`), closed eyes (Analyze Faces), identical files (content hash) — as four
//! lines with a count and a "Select" button, badges on the grid cells, and the way to stack the
//! bursts (`stack.group`). It creates no new analysis.
//!
//! **Everything is a suggestion.** The panel selects photos; it never rates, flags, rejects or deletes
//! by itself. Rating and flagging stay the keyboard's (`1`–`5`, `P`, `X`, `U`, the arrows) or the
//! three buttons under "This selection", which act on the selection only when clicked.
//!
//! The analysis of a big view is cut into small slices (a handful of photos per frame, never across
//! a burst) so the window keeps answering; it shows in the background-task list with a Cancel and is
//! one undo step when it ends.

use std::collections::{HashMap, HashSet, VecDeque};

use egui::{Rect, Stroke, pos2, vec2};
use lightcraft_catalog::{Flag, PhotoId};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::home::TaskRow;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};
use crate::workspaces::{self, Step};

/// Width of the panel.
pub const PANEL_W: f32 = 300.0;
/// Seconds between two shots of the same burst at most (the culling's own constant).
const BURST_GAP: i64 = 10;
/// A slice of the analysis holds at least this many photos (one: a slice is a burst, or a single photo, so a frame never decodes more than a burst) (then runs on to the next gap in time).
const SLICE_MIN: usize = 1;

/// How soft a photo has to be to be called soft: a share of the median focus score of the photos
/// that were scored (a fixed score would mean different things on a portrait and on a landscape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sensitivity {
    Low,
    #[default]
    Medium,
    High,
}

impl Sensitivity {
    pub fn factor(self) -> f32 {
        match self {
            Sensitivity::Low => 0.35,
            Sensitivity::Medium => 0.5,
            Sensitivity::High => 0.7,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Sensitivity::Low => "low",
            Sensitivity::Medium => "medium",
            Sensitivity::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Sensitivity> {
        [Sensitivity::Low, Sensitivity::Medium, Sensitivity::High].into_iter().find(|x| x.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// The analysis in progress.
pub struct Job {
    slices: VecDeque<Vec<u64>>,
    /// How many slices the analysis was cut into.
    slice_count: usize,
    /// Extra `photo.analyze` parameters for every slice (`rejectBelow`, `pickBest`: Sorting Help's options).
    extra: Value,
    rejected: usize,
    picked: usize,
    would_reject: usize,
    would_pick: usize,
    /// A bounded review list, distinct from stored analysis and flags.
    proposals: Vec<Value>,
    total: usize,
    done: usize,
    undo0: usize,
    started_ms: f64,
    cancelled: bool,
    error: Option<String>,
}

impl Job {
    /// Photos done so far (tests watch it move).
    pub fn progress_done(&self) -> usize {
        self.done
    }
}

/// One burst: look-alike shots taken within seconds of each other.
#[derive(Clone, Debug, PartialEq)]
pub struct Burst {
    pub ids: Vec<u64>,
    pub best: Option<u64>,
}

/// What the view holds, counted from what the photos already carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub total: usize,
    pub sorted: usize,
    /// Photos with a focus score.
    pub scored: usize,
    pub soft_below: f32,
    pub soft: Vec<u64>,
    pub closed_eyes: Vec<u64>,
    /// Photos whose faces were analysed.
    pub faces_done: usize,
    pub bursts: Vec<Burst>,
    /// Burst members that are not the best of their burst.
    pub burst_extras: Vec<u64>,
    /// Photos whose file content is the same as another photo's of the view.
    pub duplicates: Vec<u64>,
}

#[derive(Default)]
pub struct State {
    pub sensitivity: Sensitivity,
    pub job: Option<Job>,
    summary: Option<(SummaryKey, std::sync::Arc<Summary>)>,
    /// Per photo of the view: its burst (index, size, is the best), for the grid badges.
    burst_of: HashMap<u64, (usize, usize, bool)>,
    soft: HashSet<u64>,
    dup: HashSet<u64>,
    /// What the last analysis ended with (`ui.inspect`).
    pub last: Option<Value>,
}

type SummaryKey = (u64, u64, Sensitivity, usize);

fn secs(iso: &str) -> Option<i64> {
    let d = lightcraft_meta::DateTime::parse_iso(iso)?;
    Some(((d.year as i64 * 372 + d.month as i64 * 31 + d.day as i64) * 24 + d.hour as i64) * 3600 + d.minute as i64 * 60 + d.second as i64)
}

/// The panel is shown (Library space, Sort step).
pub fn visible(app: &LightcraftApp) -> bool {
    workspaces::step_of(app) == Some(Step::Sort)
        && matches!(app.ui.view, crate::state::ViewMode::PhotoGrid | crate::state::ViewMode::SquareGrid)
        && !app.ui.fullscreen
}

// -------------------------------------------------------------------------------------------- summary

/// Count what the photos in `ids` carry.
pub fn summarize(app: &LightcraftApp, ids: &[PhotoId], sens: Sensitivity) -> Summary {
    let cat = &app.session.catalog;
    let mut s = Summary { total: ids.len(), ..Default::default() };
    let mut scores: Vec<f32> = Vec::new();
    let mut in_group: Vec<(i64, u64, u32, bool)> = Vec::new();
    let mut by_hash: HashMap<&str, Vec<u64>> = HashMap::new();
    for id in ids {
        let Some(p) = cat.photo(*id) else { continue };
        s.sorted += usize::from(p.rating > 0 || p.flag != Flag::None);
        if let Some(a) = &p.analysis {
            s.scored += 1;
            scores.push(a.sharpness);
            if let Some(g) = a.group {
                in_group.push((p.captured.as_deref().and_then(secs).unwrap_or(i64::MAX), id.0, g, a.best));
            }
        }
        if p.faces().is_some() {
            s.faces_done += 1;
        }
        if p.has_closed_eyes() {
            s.closed_eyes.push(id.0);
        }
        if let Some(h) = p.content_hash.as_deref() {
            by_hash.entry(h).or_default().push(id.0);
        }
    }
    if !scores.is_empty() {
        scores.sort_by(f32::total_cmp);
        let median = scores[scores.len() / 2];
        s.soft_below = (median * sens.factor()).max(3.0);
        for id in ids {
            if cat.photo(*id).and_then(|p| p.analysis.as_ref()).is_some_and(|a| a.sharpness < s.soft_below) {
                s.soft.push(id.0);
            }
        }
    }
    // a burst = consecutive shots (by capture time) of one culling group, each within BURST_GAP of the
    // last. Groups are numbered per analysis slice, so the number alone does not tell two bursts apart.
    in_group.sort();
    let mut run: Vec<(i64, u64, bool)> = Vec::new();
    let mut run_group = 0u32;
    let flush = |run: &mut Vec<(i64, u64, bool)>, bursts: &mut Vec<Burst>| {
        if run.len() >= 2 {
            bursts.push(Burst { ids: run.iter().map(|r| r.1).collect(), best: run.iter().find(|r| r.2).map(|r| r.1) });
        }
        run.clear();
    };
    for (t, id, g, best) in in_group {
        let continues = run.last().is_some_and(|l| g == run_group && t != i64::MAX && l.0 != i64::MAX && (t - l.0).abs() <= BURST_GAP);
        if !continues {
            flush(&mut run, &mut s.bursts);
            run_group = g;
        }
        run.push((t, id, best));
    }
    flush(&mut run, &mut s.bursts);
    for b in &s.bursts {
        s.burst_extras.extend(b.ids.iter().copied().filter(|i| Some(*i) != b.best));
    }
    let mut groups: Vec<Vec<u64>> = by_hash.into_values().filter(|v| v.len() > 1).collect();
    groups.sort();
    s.duplicates = groups.into_iter().flatten().collect();
    s
}

/// The summary of the current view, recomputed only when the catalog, the view or the sensitivity changed.
fn current(app: &mut LightcraftApp) -> std::sync::Arc<Summary> {
    let ids = app.session.visible_cloned();
    let key: SummaryKey = (app.session.catalog.revision, app.session.library_generation(), app.workspaces.quick.sensitivity, ids.len());
    if let Some((k, s)) = &app.workspaces.quick.summary
        && *k == key
    {
        return s.clone();
    }
    let sens = app.workspaces.quick.sensitivity;
    let s = std::sync::Arc::new(summarize(app, &ids, sens));
    let q = &mut app.workspaces.quick;
    q.burst_of.clear();
    for (i, b) in s.bursts.iter().enumerate() {
        for id in &b.ids {
            q.burst_of.insert(*id, (i, b.ids.len(), Some(*id) == b.best));
        }
    }
    q.soft = s.soft.iter().copied().collect();
    q.dup = s.duplicates.iter().copied().collect();
    q.summary = Some((key, s.clone()));
    s
}

// ------------------------------------------------------------------------------------------- analysis

/// Analyse `ids` for focus and bursts (the existing `photo.analyze`), a slice per frame.
pub fn analyze_ids(app: &mut LightcraftApp, ids: Vec<u64>) -> Result<(), String> {
    analyze_with(app, ids, json!({}))
}

/// [`analyze_ids`] with extra `photo.analyze` parameters (Sorting Help's reject and pick options): the
/// same sliced, cancellable, one-undo-step analysis, so no button freezes the window on a big view.
pub fn analyze_with(app: &mut LightcraftApp, mut ids: Vec<u64>, extra: Value) -> Result<(), String> {
    if app.workspaces.quick.job.is_some() {
        return Err("an analysis is already running".into());
    }
    ids.retain(|i| app.session.catalog.photo(PhotoId(*i)).is_some());
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("no photo to analyze".into());
    }
    // by capture time; a slice is cut only where the next shot is more than BURST_GAP away (or has no time),
    // so no burst is ever split between two slices
    let time = |app: &LightcraftApp, id: u64| app.session.catalog.photo(PhotoId(id)).and_then(|p| p.captured.as_deref()).and_then(secs);
    let mut timed: Vec<(i64, u64)> = ids.iter().map(|i| (time(app, *i).unwrap_or(i64::MAX), *i)).collect();
    timed.sort();
    let mut slices: VecDeque<Vec<u64>> = VecDeque::new();
    let mut cur: Vec<u64> = Vec::new();
    let mut last = i64::MAX;
    for (t, id) in timed {
        let cut = cur.len() >= SLICE_MIN && (t == i64::MAX || last == i64::MAX || (t - last).abs() > BURST_GAP);
        if cut {
            slices.push_back(std::mem::take(&mut cur));
        }
        cur.push(id);
        last = t;
    }
    if !cur.is_empty() {
        slices.push_back(cur);
    }
    let total = ids.len();
    app.workspaces.quick.job = Some(Job {
        slice_count: slices.len(),
        extra,
        rejected: 0,
        picked: 0,
        would_reject: 0,
        would_pick: 0,
        proposals: Vec::new(),
        slices,
        total,
        done: 0,
        undo0: app.session.undo.len(),
        started_ms: crate::now_ms(),
        cancelled: false,
        error: None,
    });
    Ok(())
}

pub fn cancel(app: &mut LightcraftApp) {
    if let Some(j) = app.workspaces.quick.job.as_mut() {
        j.cancelled = true;
    }
}

/// The analysis as a background-task row.
pub fn progress(app: &LightcraftApp) -> Option<TaskRow> {
    app.workspaces.quick.job.as_ref().map(|j| TaskRow {
        key: "quick".into(),
        label: "Finding bursts and soft photos".into(),
        done: j.done,
        total: j.total,
        note: String::new(),
        cancelling: j.cancelled,
    })
}

/// Once a frame: the next slice of the analysis.
pub fn tick(app: &mut LightcraftApp, _now: f64) {
    let Some(mut job) = app.workspaces.quick.job.take() else { return };
    if job.cancelled || job.error.is_some() {
        finish(app, job);
        return;
    }
    match job.slices.pop_front() {
        Some(slice) => {
            let n = slice.len();
            let mut params = json!({"ids": slice});
            if let (Some(p), Some(x)) = (params.as_object_mut(), job.extra.as_object()) {
                p.extend(x.clone());
            }
            match app.session.execute("photo.analyze", &params) {
                Ok(v) => {
                    job.done += n;
                    job.rejected += v["rejected"].as_u64().unwrap_or(0) as usize;
                    job.picked += v["picked"].as_u64().unwrap_or(0) as usize;
                    job.would_reject += v["wouldReject"].as_u64().unwrap_or(0) as usize;
                    job.would_pick += v["wouldPick"].as_u64().unwrap_or(0) as usize;
                    if job.extra["dryRun"] == true
                        && let Some(rows) = v["photos"].as_array()
                    {
                        job.proposals.extend(rows.iter().take(200usize.saturating_sub(job.proposals.len())).cloned());
                    }
                }
                Err(e) => job.error = Some(e.to_string()),
            }
            app.workspaces.quick.job = Some(job);
        }
        None => finish(app, job),
    }
}

/// The analysis ended (or was stopped): its slices become one undo step.
fn finish(app: &mut LightcraftApp, job: Job) {
    let dry_run = job.extra["dryRun"] == true;
    let steps = app.session.undo.len().saturating_sub(job.undo0);
    if !dry_run {
        app.session.merge_undo(steps, "Find Bursts and Soft Photos");
    }
    let s = current_fresh(app);
    app.workspaces.quick.last = Some(json!({
        "photos": job.done, "of": job.total, "slices": job.slice_count, "rejected": job.rejected, "picked": job.picked, "cancelled": job.cancelled, "error": job.error,
        "dryRun": dry_run, "wouldReject": job.would_reject, "wouldPick": job.would_pick,
        "proposals": job.proposals,
        "seconds": (crate::now_ms() - job.started_ms) / 1000.0,
        "soft": s.soft.len(), "bursts": s.bursts.len(), "burstExtras": s.burst_extras.len(),
    }));
}

fn current_fresh(app: &mut LightcraftApp) -> std::sync::Arc<Summary> {
    app.workspaces.quick.summary = None;
    current(app)
}

// ------------------------------------------------------------------------------------------ commands

fn ids_of(v: &[u64]) -> Value {
    json!(v)
}

/// `workspace.sort.*`.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    match id {
        // analyse the photos of the view (or `ids`)
        "workspace.sort.analyze" => {
            let ids: Vec<u64> = match p.get("ids").and_then(Value::as_array) {
                Some(a) => a.iter().filter_map(Value::as_u64).collect(),
                None => app.session.visible_cloned().iter().map(|i| i.0).collect(),
            };
            let n = ids.len();
            analyze_ids(app, ids)?;
            Ok(json!({"started": true, "photos": n}))
        }
        "workspace.sort.cancel" => {
            cancel(app);
            Ok(Value::Null)
        }
        "workspace.sort.sensitivity" => {
            let v = p.get("value").and_then(Value::as_str).ok_or("missing `value` (low | medium | high)")?;
            app.workspaces.quick.sensitivity = Sensitivity::parse(v).ok_or("unknown sensitivity (low | medium | high)")?;
            Ok(Value::Null)
        }
        // select a group of suggestions: nothing is written
        "workspace.sort.select" => {
            let what = p.get("what").and_then(Value::as_str).ok_or("missing `what` (soft | closedEyes | burstExtras | duplicates | unsorted)")?;
            let s = current(app);
            let ids: Vec<u64> = match what {
                "soft" => s.soft.clone(),
                "closedEyes" => s.closed_eyes.clone(),
                "burstExtras" => s.burst_extras.clone(),
                "duplicates" => s.duplicates.clone(),
                "unsorted" => {
                    let vis = app.session.visible_cloned();
                    let cat = &app.session.catalog;
                    vis.iter().filter(|i| cat.photo(**i).is_some_and(|p| p.rating == 0 && p.flag == Flag::None)).map(|i| i.0).collect()
                }
                other => return Err(format!("unknown selection `{other}`")),
            };
            if ids.is_empty() {
                return Err(format!("nothing to select ({what})"));
            }
            app.run("library.select", json!({"ids": ids_of(&ids)}))?;
            Ok(json!({"selected": ids.len()}))
        }
        // stack every burst under its best photo (the existing stack.group, one undo step)
        "workspace.sort.stackBursts" => {
            let s = current(app);
            if s.bursts.is_empty() {
                return Err("no burst found (analyze the photos first)".into());
            }
            let undo0 = app.session.undo.len();
            let mut stacked = 0;
            for b in &s.bursts {
                let mut params = json!({"ids": b.ids, "collapsed": true});
                if let Some(best) = b.best {
                    params["top"] = json!(best);
                }
                if app.session.execute("stack.group", &params).is_ok() {
                    stacked += 1;
                }
            }
            let steps = app.session.undo.len().saturating_sub(undo0);
            app.session.merge_undo(steps, "Stack Bursts");
            Ok(json!({"stacked": stacked}))
        }
        // the next photo after the active one that has no rating and no flag
        "workspace.sort.next" => {
            let vis = app.session.visible_cloned();
            let start = app.session.active().and_then(|a| vis.iter().position(|i| *i == a)).map_or(0, |i| i + 1);
            let cat = &app.session.catalog;
            let next = vis
                .iter()
                .cycle()
                .skip(start)
                .take(vis.len())
                .find(|i| cat.photo(**i).is_some_and(|p| p.rating == 0 && p.flag == Flag::None))
                .copied();
            let Some(n) = next else { return Err("every photo of the view is rated or flagged".into()) };
            app.run("library.select", json!({"ids": [n.0]}))?;
            Ok(json!({"id": n.0}))
        }
        other => Err(format!("unknown sort command `{other}`")),
    }
}

// ----------------------------------------------------------------------------------------------- badges

/// The suggestion badges of a grid cell (soft, burst, same file), bottom left above the badge bar.
/// Closed eyes already have their own badge in the grid.
pub fn badges(app: &LightcraftApp, ui: &egui::Ui, id: PhotoId, img: Rect, offset: f32) {
    if !visible(app) {
        return;
    }
    let q = &app.workspaces.quick;
    let t = Tokens::get(ui.ctx());
    let mut pills: Vec<(String, &'static str, Option<String>)> = Vec::new();
    if q.soft.contains(&id.0) {
        pills.push(("Soft".into(), "soft", Some(crate::i18n::tr("Softer than most of the photos scored: it may be out of focus").into())));
    }
    if let Some((_, n, best)) = q.burst_of.get(&id.0) {
        let label = if *best { format!("Burst ×{n} · best") } else { format!("Burst ×{n}") };
        pills.push((
            label,
            "burst",
            Some(crate::i18n::tr("Look-alike shots taken within seconds: the sharpest, with open eyes, is marked best").into()),
        ));
    }
    if q.dup.contains(&id.0) {
        pills.push(("Same file".into(), "dup", Some(crate::i18n::tr("Another photo of the view has exactly the same file content").into())));
    }
    let p = ui.painter();
    let mut y = img.bottom() - 30.0 - 16.0 - offset;
    for (label, key, tip) in pills {
        let g = p.layout_no_wrap(label, t.semibold(9.5), egui::Color32::WHITE);
        let r = Rect::from_min_size(pos2(img.left() + 6.0, y), vec2(g.size().x + 14.0, 16.0));
        p.rect_filled(r, 8.0, egui::Color32::from_black_alpha(175));
        p.rect_stroke(r, 8.0, Stroke::new(1.0, if key == "soft" { t.caution } else { t.accent }), egui::StrokeKind::Inside);
        p.galley(pos2(r.left() + 7.0, r.center().y - g.size().y / 2.0), g, egui::Color32::WHITE);
        register(ui.ctx(), format!("badge:{key}:{}", id.0), r);
        if let Some(tip) = tip {
            ui.interact(r, egui::Id::new(("quick-badge", key, id.0)), egui::Sense::hover()).on_hover_text(tip);
        }
        y -= 18.0;
    }
}

// -------------------------------------------------------------------------------------------------- panel

fn line(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, count: usize, tip: &str, action: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{label}: {count}")).color(if count > 0 { t.text } else { t.text_dim }).size(12.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add_enabled_ui(count > 0, |ui| text_button(ui, id, crate::i18n::tr(action), false)).inner.on_hover_text(tip);
            clicked = r.clicked();
        });
    });
    clicked
}

/// The Quick sort panel, to the right of the grid.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !visible(app) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let s = current(app);
    let faces_installed = app.session.execute("facemesh.status", &json!({})).ok().is_some_and(|v| v["installed"].as_bool() == Some(true));
    let mut run: Option<(&'static str, Value)> = None;
    let mut other: Option<(&'static str, Value)> = None;
    register(ui.ctx(), "region:quickSort", ui.max_rect());
    crate::home::heading(ui, &t, "Quick sort");
    // how far along the sorting is
    let frac = if s.total > 0 { s.sorted as f32 / s.total as f32 } else { 0.0 };
    ui.label(
        egui::RichText::new(crate::i18n::tr_format!("Rated or flagged: {done} of {total}", done = s.sorted, total = s.total))
            .color(t.text_label)
            .size(12.0),
    );
    ui.add(egui::ProgressBar::new(frac).desired_height(6.0));
    ui.horizontal(|ui| {
        if text_button(ui, "quickNext", crate::i18n::tr("Next unsorted"), false)
            .on_hover_text(crate::i18n::tr("Select the next photo with no rating and no flag"))
            .clicked()
        {
            run = Some(("workspace.sort.next", json!({})));
        }
        if text_button(ui, "quickUnsorted", crate::i18n::tr("Select unsorted"), false).clicked() {
            run = Some(("workspace.sort.select", json!({"what": "unsorted"})));
        }
    });
    ui.add_space(10.0);
    crate::home::heading(ui, &t, "Suggestions");
    let running = app.workspaces.quick.job.is_some();
    if s.scored < s.total || running {
        let label = if running { "Analyzing…" } else { "Analyze this view" };
        let b = ui.add_enabled(!running && s.total > 0, egui::Button::new(crate::i18n::tr(label)));
        register(ui.ctx(), "button:quickAnalyze", b.rect);
        if b.clicked() {
            run = Some(("workspace.sort.analyze", json!({})));
        }
        let note = match &app.workspaces.quick.job {
            Some(j) => format!("{} of {} photos scored", j.done, j.total),
            None => format!("{} of {} photos scored: bursts and soft photos need it. It runs in the background.", s.scored, s.total),
        };
        ui.label(egui::RichText::new(note).color(t.text_dim).size(11.5));
    } else if text_button(ui, "quickReanalyze", crate::i18n::tr("Analyze again"), false).clicked() {
        run = Some(("workspace.sort.analyze", json!({})));
    }
    ui.add_space(4.0);
    if s.scored > 0 {
        if line(ui, &t, "quickSoft", "Soft photos", s.soft.len(), "Select the photos softer than the threshold below", "Select") {
            other = Some(("workspace.sort.select", json!({"what": "soft"})));
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("threshold {:.1}", s.soft_below)).color(t.text_dim).size(11.0));
            for (sens, label) in [(Sensitivity::Low, "Low"), (Sensitivity::Medium, "Medium"), (Sensitivity::High, "High")] {
                if text_button(ui, &format!("quickSens-{}", sens.name()), crate::i18n::tr(label), app.workspaces.quick.sensitivity == sens)
                    .on_hover_text(crate::i18n::tr("How soft counts as soft: a share of the median focus score"))
                    .clicked()
                {
                    run = Some(("workspace.sort.sensitivity", json!({"value": sens.name()})));
                }
            }
        });
        if line(
            ui,
            &t,
            "quickBursts",
            &format!("Bursts ({} groups) · extras", s.bursts.len()),
            s.burst_extras.len(),
            "Select the shots of each burst that are not the best",
            "Select",
        ) {
            other = Some(("workspace.sort.select", json!({"what": "burstExtras"})));
        }
        if !s.bursts.is_empty()
            && text_button(ui, "quickStack", &format!("Stack the {} bursts", s.bursts.len()), false)
                .on_hover_text(crate::i18n::tr("Group each burst into a stack with its best shot on top (one undo step)"))
                .clicked()
        {
            other = Some(("workspace.sort.stackBursts", json!({})));
        }
    }
    if s.faces_done > 0 {
        if line(ui, &t, "quickEyes", "Closed eyes", s.closed_eyes.len(), "Select the photos where a face has a closed eye", "Select") {
            other = Some(("workspace.sort.select", json!({"what": "closedEyes"})));
        }
    } else if faces_installed {
        if text_button(ui, "quickFaces", crate::i18n::tr("Analyze faces (closed eyes)"), false).clicked() {
            other = Some((
                "photo.analyzeFaces",
                json!({"ids": app.session.visible_cloned().iter().map(|i| i.0).collect::<Vec<_>>(), "background": true}),
            ));
        }
    } else {
        ui.label(egui::RichText::new(crate::i18n::tr("Closed eyes need the face model (Photo ▸ Analyze Faces).")).color(t.text_dim).size(11.5));
    }
    if line(ui, &t, "quickDup", "Same file", s.duplicates.len(), "Select the photos whose file is identical to another's", "Select") {
        other = Some(("workspace.sort.select", json!({"what": "duplicates"})));
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(crate::i18n::tr("These only select. Nothing is rated, rejected or deleted until you press a key or a button."))
            .color(t.text_dim)
            .size(11.5),
    );
    ui.add_space(10.0);
    let n = app.session.selection.ids.len();
    crate::home::heading(ui, &t, &format!("This selection ({n})"));
    ui.horizontal(|ui| {
        for (id, label, flag) in [("quickPick", "Pick (P)", "pick"), ("quickReject", "Reject (X)", "reject"), ("quickUnflag", "Unflag (U)", "none")] {
            if ui.add_enabled_ui(n > 0, |ui| text_button(ui, id, crate::i18n::tr(label), false)).inner.clicked() {
                other = Some(("photo.flag", json!({"flag": flag})));
            }
        }
    });
    ui.label(egui::RichText::new(crate::i18n::tr("Keys: 1-5 stars · P pick · X reject · U unflag · ← → next photo")).color(t.text_dim).size(11.5));
    for (id, params) in run.into_iter().chain(other) {
        if let Err(e) = app.run(id, params) {
            app.ui.status = e;
        }
    }
}

/// `ui.inspect` → `workspaces.quick`.
pub fn inspect(app: &LightcraftApp) -> Value {
    let q = &app.workspaces.quick;
    let s = q.summary.as_ref().map(|(_, s)| s.clone());
    json!({
        "visible": visible(app),
        "sensitivity": q.sensitivity.name(),
        "running": q.job.as_ref().map(|j| json!({"done": j.done, "total": j.total})),
        "last": q.last,
        "summary": s.map(|s| json!({
            "total": s.total, "sorted": s.sorted, "scored": s.scored, "softBelow": s.soft_below, "soft": s.soft.len(),
            "closedEyes": s.closed_eyes.len(), "bursts": s.bursts.len(), "burstExtras": s.burst_extras.len(), "duplicates": s.duplicates.len(),
        })),
    })
}
