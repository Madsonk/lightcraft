//! Proposals: the proposed retouch ("Natural skin") at the top of the Versions panel. Pressing
//! Propose plans the eight ordered steps for the selected photos and writes nothing: the planning is
//! **staged** (`retouch.propose {stage: true}`: the plan, then one quality check per frame), the window
//! paints a progress bar, a "Computing…" line and a busy cursor between the stages and the button is
//! disabled meanwhile (the session is not shareable: a stage still runs on the UI thread, so the window
//! pauses for the length of one stage, not of the whole plan); every step has its switch and a one-line
//! explanation with measured figures; **Preview on the photo** shows the planned retouch in the loupe
//! without writing it (a toast says so; flip it off to see the original). Validating **never touches the
//! original**: "Create retouched copy" makes a virtual copy named "Natural skin – Medium" (or the impact
//! chosen), "Create 3 copies" makes Light, Medium and Strong, all stacked with the original; "Remove the
//! retouch copies" takes only those copies away. Same commands as the CLI: `retouch.propose`,
//! `retouch.apply` (copies by default), `retouch.clear`. Each control sits on its own row: a wrapped
//! label in the same row as a button gets an accessibility frame that covers the button (macOS AX hit
//! tests then land on the text).

use std::collections::BTreeMap;

use egui::RichText;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// A proposal being planned: one photo per frame, so the window shows progress instead of freezing.
#[derive(Clone, Default)]
struct Pending {
    queue: Vec<u64>,
    done: Vec<Value>,
    total: usize,
    /// Frames drawn since it started (the first draws the progress bar before any planning).
    frames: u32,
    /// Stages run so far on the photo at the head of the queue (0 = its plan is not made yet).
    stages: u32,
    /// What the last stage was ("plan", "check"), for the progress line.
    stage: String,
}

/// What the section remembers between frames.
#[derive(Clone, Default)]
struct State {
    /// 0 Light, 1 Medium, 2 Strong.
    impact: usize,
    steps: BTreeMap<String, bool>,
    result: Option<Value>,
    open: bool,
    pending: Option<Pending>,
    /// Show the planned retouch on the photo (nothing is written).
    preview: bool,
    last_copies: usize,
}

const IMPACTS: [(&str, &str); 3] = [("Light", "light"), ("Medium", "medium"), ("Strong", "strong")];

fn ids_of(app: &LightcraftApp) -> Vec<u64> {
    let s = &app.session;
    if s.selection.ids.is_empty() { s.active().map(|i| i.0).into_iter().collect() } else { s.selection.ids.iter().map(|i| i.0).collect() }
}

fn params(st: &State, ids: &[u64]) -> Value {
    json!({"ids": ids, "recipe": "natural-skin", "impact": IMPACTS[st.impact.min(2)].1, "steps": st.steps})
}

/// Validate: virtual copies, the original untouched. One call per photo (the plan of each is cached by the
/// proposal), the undo steps merged into one. Returns how many copies were made.
fn apply_copies(app: &mut LightcraftApp, ctx: &egui::Context, st: &State, ids: &[u64], variants: bool) -> usize {
    let undo0 = app.session.undo.len();
    let mut made = 0;
    let mut shown: Vec<u64> = Vec::new();
    for id in ids {
        let mut p = params(st, &[*id]);
        p["copy"] = json!(true);
        if variants {
            p["variants"] = json!(true);
        }
        match app.run("retouch.apply", p) {
            Ok(v) => {
                made += v["copies"].as_array().map_or(0, Vec::len);
                shown.extend(app.session.selection.ids.iter().map(|i| i.0));
            }
            Err(e) => app.toast(ctx, format!("Create copy: {e}")),
        }
    }
    let steps = app.session.undo.len().saturating_sub(undo0);
    app.session.merge_undo(steps, "Natural Skin Retouch");
    if made > 0 {
        let _ = app.run("library.select", json!({"ids": shown}));
        app.toast(
            ctx,
            format!("{made} retouched cop{} made, stacked with the original (untouched): one undo step", if made == 1 { "y" } else { "ies" }),
        );
    }
    made
}

pub fn section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    // the panel's room ends before the tool strip: keep the text inside it (this section only)
    ui.scope(|ui| {
        ui.set_max_width((ui.available_width() - 34.0).max(140.0));
        body(app, ui);
    });
}

fn body(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let key = egui::Id::new("proposals-state");
    let mut st: State = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    ui.add_space(6.0);
    let head = ui.add(
        egui::Button::new(RichText::new(format!("{} Proposals · Natural skin", if st.open { "v" } else { ">" })).font(t.semibold(12.0))).frame(false),
    );
    register(ui.ctx(), "button:proposalsHeader", head.rect);
    if head.clicked() {
        st.open = !st.open;
    }
    if st.open {
        ui.label(
            RichText::new("Proposing writes nothing. Validating makes a retouched COPY: the original is never touched.").color(t.text_dim).size(10.5),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (i, (label, _)) in IMPACTS.iter().enumerate() {
                let r = ui.selectable_label(st.impact == i, *label);
                register(ui.ctx(), format!("button:proposalsImpact{i}"), r.rect);
                if r.clicked() && st.impact != i {
                    st.impact = i;
                    st.result = None;
                }
            }
        });
        let ids = ids_of(app);
        ui.add_space(4.0);
        // planning, one stage per frame: the progress bar, the "Computing…" line and the busy cursor are
        // painted BEFORE the first stage runs (frames == 1 draws and computes nothing)
        if let Some(mut pend) = st.pending.take() {
            pend.frames += 1;
            let n = pend.total - pend.queue.len();
            let frac = (n as f32 + (pend.stages.min(8) as f32) / 9.0) / pend.total.max(1) as f32;
            ui.add(egui::ProgressBar::new(frac).text(format!("Planning {} of {}…", (n + 1).min(pend.total), pend.total)));
            ui.label(
                RichText::new(if pend.frames <= 1 {
                    "Computing… the window pauses for a few seconds per step".to_string()
                } else {
                    format!(
                        "Computing… {} (stage {}). The window pauses for a few seconds per stage.",
                        if pend.stage == "check" { "quality checks" } else { "the plan" },
                        pend.stages + 1
                    )
                })
                .color(t.text_dim)
                .size(10.5),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
            let cancel = ui.small_button("Cancel");
            register(ui.ctx(), "button:proposalsCancel", cancel.rect);
            if cancel.clicked() {
                // dropped: nothing was written
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
            } else {
                if pend.frames > 1
                    && let Some(id) = pend.queue.first().copied()
                {
                    let mut p = params(&st, &[id]);
                    p["stage"] = json!(true);
                    match app.run("retouch.propose", p) {
                        Ok(v) if v["done"] == true => {
                            // everything is cached: the plain call answers at once
                            pend.queue.remove(0);
                            pend.stages = 0;
                            let mut p = params(&st, &[id]);
                            p["settings"] = json!(true);
                            match app.run("retouch.propose", p) {
                                Ok(v) => pend.done.extend(v["photos"].as_array().cloned().unwrap_or_default()),
                                Err(e) => app.toast(ui.ctx(), format!("Propose: {e}")),
                            }
                        }
                        Ok(v) => {
                            pend.stages += 1;
                            pend.stage = v["stage"].as_str().unwrap_or("").to_string();
                        }
                        Err(e) => {
                            pend.queue.remove(0);
                            pend.stages = 0;
                            app.toast(ui.ctx(), format!("Propose: {e}"));
                        }
                    }
                }
                if pend.queue.is_empty() {
                    st.result = Some(json!({"photos": pend.done, "written": false}));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                } else {
                    st.pending = Some(pend);
                }
            }
            ui.ctx().request_repaint();
        }
        let planning = st.pending.is_some();
        ui.horizontal_wrapped(|ui| {
            let r = ui.add_enabled(!ids.is_empty() && !planning, egui::Button::new("Propose"));
            register(ui.ctx(), "button:proposalsPropose", r.rect);
            if r.clicked() {
                st.pending = Some(Pending { total: ids.len(), queue: ids.clone(), ..Default::default() });
                st.result = None;
            }
        });
        let has = st.result.is_some();
        // the button and its status line are two rows on purpose (see the module note)
        let r = ui.add_enabled(has, egui::Button::new("Preview on the photo").selected(st.preview));
        register(ui.ctx(), "button:proposalsPreview", r.rect);
        if r.clicked() {
            st.preview = !st.preview;
            app.toast(
                ui.ctx(),
                if st.preview {
                    "Proposal preview: shown on the photo, not saved (the photo is unchanged)"
                } else {
                    "Preview off: showing the original"
                },
            );
        }
        ui.label(RichText::new(if st.preview { "showing the proposal (not written)" } else { "showing the original" }).color(t.text_dim).size(10.5));
        ui.horizontal_wrapped(|ui| {
            let r = ui.add_enabled(has, egui::Button::new(format!("Create retouched copy ({})", IMPACTS[st.impact.min(2)].0)));
            register(ui.ctx(), "button:proposalsApply", r.rect);
            if r.clicked() {
                st.last_copies = apply_copies(app, ui.ctx(), &st, &ids, false);
                st.preview = false;
            }
            let r = ui.add_enabled(has, egui::Button::new("Create 3 copies"));
            register(ui.ctx(), "button:proposalsApplyVariants", r.rect);
            if r.clicked() {
                st.last_copies = apply_copies(app, ui.ctx(), &st, &ids, true);
                st.preview = false;
            }
        });
        let r = ui.small_button("Remove the retouch copies");
        register(ui.ctx(), "button:proposalsClear", r.rect);
        if r.clicked() {
            match app.run("retouch.clear", json!({"ids": ids})) {
                Ok(v) => {
                    let n = v["copiesRemoved"].as_u64().unwrap_or(0);
                    let m = v["removed"].as_u64().unwrap_or(0);
                    app.toast(
                        ui.ctx(),
                        if n > 0 { format!("{n} retouch copies moved to Recently Deleted") } else { format!("{m} retouch masks removed") },
                    );
                }
                Err(e) => app.toast(ui.ctx(), format!("Remove: {e}")),
            }
            st.result = None;
            st.preview = false;
        }
        // the proposal on the photo, nothing written: the loupe's temporary look
        if st.preview
            && let Some(active) = app.session.active().map(|i| i.0)
            && let Some(settings) = st
                .result
                .as_ref()
                .and_then(|r| r["photos"].as_array())
                .and_then(|a| a.iter().find(|p| p["id"].as_u64() == Some(active)))
                .and_then(|p| serde_json::from_value::<lightcraft_develop::DevelopSettings>(p["settings"].clone()).ok())
        {
            app.hover_preview = Some(crate::HoverPreview { label: "Proposal: Natural skin".into(), settings });
        }
        // the plan of the active photo
        let active = app.session.active().map(|i| i.0);
        let photo = st
            .result
            .as_ref()
            .and_then(|r| r["photos"].as_array())
            .and_then(|a| a.iter().find(|p| p["id"].as_u64() == active).or(a.first()))
            .cloned();
        if let Some(p) = photo {
            ui.add_space(6.0);
            let mut changed = false;
            for step in p["steps"].as_array().cloned().unwrap_or_default() {
                let id = step["id"].as_str().unwrap_or("").to_string();
                let available = step["available"].as_bool().unwrap_or(false);
                let mut on = step["enabled"].as_bool().unwrap_or(false);
                ui.horizontal(|ui| {
                    let label = format!("{}. {}", step["n"].as_u64().unwrap_or(0), step["label"].as_str().unwrap_or(""));
                    let r = ui.add_enabled(available, egui::Checkbox::new(&mut on, label));
                    register(ui.ctx(), format!("check:proposals:{id}"), r.rect);
                    if r.changed() {
                        st.steps.insert(id.clone(), on);
                        changed = true;
                    }
                });
                let text = step["explain"].as_str().unwrap_or("");
                if !text.is_empty() {
                    ui.label(RichText::new(text).color(t.text_dim).size(10.5));
                }
                for w in step["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    ui.label(RichText::new(format!("⚠ {w}")).color(egui::Color32::from_rgb(230, 170, 60)).size(10.5));
                }
            }
            if let Some(r) = p["autoReduced"].as_object() {
                ui.label(
                    RichText::new(format!("Dose reduced to {} to stay natural: {}", r["to"], r["reason"].as_str().unwrap_or("")))
                        .color(egui::Color32::from_rgb(230, 170, 60))
                        .size(10.5),
                );
            }
            let q = &p["qc"];
            ui.label(
                RichText::new(format!(
                    "Checks: skin texture {} (tolerance 5 %), halo {} L, outside the zones: {} pixels changed",
                    q["textureChange"].as_f64().map_or("n/a".into(), |v| format!("{:+.1} %", v * 100.0)),
                    q["halo"],
                    q["outsidePixelsChanged"]
                ))
                .color(t.text_dim)
                .size(10.0),
            );
            if changed {
                st.pending = Some(Pending { total: ids.len(), queue: ids.clone(), ..Default::default() });
            }
        }
    }
    ui.add_space(6.0);
    ui.separator();
    ui.data_mut(|d| d.insert_temp(key, st));
    let _ = text_button;
}
