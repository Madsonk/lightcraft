//! Proposals: the proposed retouch ("Natural skin"). With workspaces on it is a panel of its own in
//! the tool strip (`panel.proposals`); off, it stays a section at the top of Versions. Pressing
//! Propose plans the eight ordered steps for the selected photos (one photo per frame, with a progress
//! bar) and writes nothing. The panel shows the result the way a photographer reads it: a **Before /
//! After pair** of the active photo, one line that sums the plan up, the impact (Light / Medium /
//! Strong), one primary button, and the step-by-step details folded under "Details". **Preview on the
//! photo** shows the planned retouch in the loupe without writing it. Validating **never touches the
//! original**: "Create retouched copy" makes a virtual copy named "Natural skin – Medium" (or the impact
//! chosen), "Create 3 copies" makes Light, Medium and Strong, all stacked with the original; "Remove the
//! retouch copies" takes only those copies away. Same commands as the CLI: `retouch.propose`,
//! `retouch.apply {copy}`, `retouch.clear`.

use std::collections::BTreeMap;

use egui::{Align2, Rect, RichText, pos2, vec2};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
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
}

/// What the section remembers between frames.
#[derive(Clone, Default)]
struct State {
    /// 0 Light, 1 Medium, 2 Strong.
    impact: usize,
    steps: BTreeMap<String, bool>,
    result: Option<Value>,
    open: bool,
    details: bool,
    pending: Option<Pending>,
    /// Show the planned retouch on the photo (nothing is written).
    preview: bool,
    last_copies: usize,
}

const IMPACTS: [(&str, &str); 3] = [("Light", "light"), ("Medium", "medium"), ("Strong", "strong")];

fn ids_of(app: &LightcraftApp) -> Vec<u64> {
    let s = &app.session;
    if crate::retouch_left::wants(app) || s.selection.ids.is_empty() {
        s.active().map(|i| i.0).into_iter().collect()
    } else {
        s.selection.ids.iter().map(|i| i.0).collect()
    }
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
            Err(e) => app.toast_error(ctx, tr_format!("Create copy: {e}", e = e)),
        }
    }
    let steps = app.session.undo.len().saturating_sub(undo0);
    app.session.merge_undo(steps, "Natural Skin Retouch");
    if made > 0 {
        let _ = app.run("library.select", json!({"ids": shown}));
        app.toast_success(
            ctx,
            tr_format!(
                "{made} retouched cop{} made, stacked with the original (untouched): one undo step",
                if made == 1 { "y" } else { "ies" },
                made = made
            ),
        );
    }
    made
}

/// The panel of its own (tool strip ▸ Proposals, workspaces on).
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    super::right::header(ui, "Proposals");
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 8 }).show(ui, |ui| {
        let key = egui::Id::new("proposals-state");
        let mut st: State = ui.data(|d| d.get_temp(key)).unwrap_or_default();
        st.open = true;
        body(app, ui, &mut st);
        ui.data_mut(|d| d.insert_temp(key, st));
    });
}

/// The section at the top of Versions (the classic window, workspaces off).
pub fn section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let key = egui::Id::new("proposals-state");
    let mut st: State = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    ui.add_space(6.0);
    let head = ui.add(
        egui::Button::new(RichText::new(format!("{} {}", if st.open { "v" } else { ">" }, tr("Proposals · Natural skin"))).font(t.semibold(12.0)))
            .frame(false),
    );
    register(ui.ctx(), "button:proposalsHeader", head.rect);
    if head.clicked() {
        st.open = !st.open;
    }
    if st.open {
        body(app, ui, &mut st);
    }
    ui.add_space(6.0);
    ui.separator();
    ui.data_mut(|d| d.insert_temp(key, st));
}

/// Before / After of the active photo: the current look and the planned one, side by side.
pub(crate) fn pair(app: &mut LightcraftApp, ui: &mut egui::Ui, active: u64, after: Option<lightcraft_develop::DevelopSettings>) {
    let t = Tokens::get(ui.ctx());
    let id = lightcraft_catalog::PhotoId(active);
    let w = ui.available_width();
    let gap = 8.0;
    let tile = vec2(((w - gap) / 2.0).max(60.0), ((w - gap) / 2.0 * 0.66).max(40.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, tile.y + 18.0), egui::Sense::hover());
    let before = app.session.develop_of(id).map(|d| (*d).clone()).unwrap_or_default();
    let looks: [(&str, Option<lightcraft_develop::DevelopSettings>, &str); 2] =
        [("Before", Some(before), "proposalsBefore"), ("After", after, "proposalsAfter")];
    for (i, (label, settings, wid)) in looks.into_iter().enumerate() {
        let tr_rect = Rect::from_min_size(r.min + vec2(i as f32 * (tile.x + gap), 0.0), tile);
        register(ui.ctx(), format!("image:{wid}"), tr_rect);
        ui.painter().rect_filled(tr_rect, 3.0, t.canvas);
        match settings {
            Some(s) => {
                if let Some(job) = app.session.variant_job(id, &s, 200)
                    && let Some(tex) = app.renderer.variant(job)
                {
                    ui.painter().image(tex.tex.id(), tr_rect, super::presets::cover_uv(tr_rect, tex.size), egui::Color32::WHITE);
                } else {
                    ui.painter().text(tr_rect.center(), Align2::CENTER_CENTER, "…", t.font(14.0), t.text_dim);
                    ui.ctx().request_repaint();
                }
            }
            None => {
                ui.painter().text(tr_rect.center(), Align2::CENTER_CENTER, tr("Propose first"), t.font(11.0), t.text_dim);
            }
        }
        ui.painter().text(pos2(tr_rect.left(), tr_rect.bottom() + 9.0), Align2::LEFT_CENTER, tr(label), t.semibold(11.0), t.text_label);
    }
}

/// "5 of 8 steps · Medium · skin texture −30 %": the plan in one line.
fn summary(p: &Value, impact: &str) -> String {
    let steps = p["steps"].as_array().cloned().unwrap_or_default();
    let on = steps.iter().filter(|s| s["enabled"].as_bool().unwrap_or(false) && s["available"].as_bool().unwrap_or(false)).count();
    let texture = p["qc"]["textureChange"].as_f64().map(|v| format!(" · {}", tr_format!("skin texture {:+.0} %", v * 100.0))).unwrap_or_default();
    tr_format!("{on} of {total} steps · {impact}{texture}", on = on, total = steps.len(), impact = tr(impact), texture = texture)
}

fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &mut State) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
    let ids = ids_of(app);
    let active = app.session.active().map(|i| i.0);
    // the plan of the active photo
    let photo =
        st.result.as_ref().and_then(|r| r["photos"].as_array()).and_then(|a| a.iter().find(|p| p["id"].as_u64() == active).or(a.first())).cloned();
    let after = photo.as_ref().and_then(|p| serde_json::from_value::<lightcraft_develop::DevelopSettings>(p["settings"].clone()).ok());
    if let Some(active) = active {
        pair(app, ui, active, after.clone());
    } else {
        ui.label(RichText::new(tr("Select a photo, then press Propose.")).color(t.text_dim).size(12.0));
    }
    // impact
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("Impact")).color(t.text_dim).size(12.0));
        for (i, (label, _)) in IMPACTS.iter().enumerate() {
            let r = text_button(ui, &format!("proposalsImpact{i}"), label, st.impact == i);
            if r.clicked() && st.impact != i {
                st.impact = i;
                st.result = None;
            }
        }
    });
    // planning, a photo per frame
    if let Some(mut pend) = st.pending.take() {
        pend.frames += 1;
        let frac = (pend.total - pend.queue.len()) as f32 / pend.total.max(1) as f32;
        ui.add(egui::ProgressBar::new(frac).text(tr_format!("Planning {} of {}…", pend.total - pend.queue.len() + 1, pend.total)));
        if text_button(ui, "proposalsCancel", "Cancel", false).clicked() {
            // dropped: nothing was written
        } else {
            if pend.frames > 1
                && let Some(id) = pend.queue.first().copied()
            {
                pend.queue.remove(0);
                let mut p = params(st, &[id]);
                p["settings"] = json!(true);
                match app.run("retouch.propose", p) {
                    Ok(v) => pend.done.extend(v["photos"].as_array().cloned().unwrap_or_default()),
                    Err(e) => app.toast_error(ui.ctx(), tr_format!("Propose: {e}", e = e)),
                }
            }
            if pend.queue.is_empty() {
                st.result = Some(json!({"photos": pend.done, "written": false}));
            } else {
                st.pending = Some(pend);
            }
        }
        ui.ctx().request_repaint();
    }
    let planning = st.pending.is_some();
    let has = st.result.is_some();
    // one primary button per state: Propose before a plan exists, Create the copy once it does
    ui.horizontal_wrapped(|ui| {
        let propose = if has {
            text_button(ui, "proposalsPropose", "Propose again", false)
        } else {
            crate::context_bar::primary_button(ui, "proposalsPropose", tr("Propose"), !ids.is_empty() && !planning)
        };
        if propose.on_hover_text(tr("Plans the recipe on the selected photos. Nothing is written.")).clicked() && !ids.is_empty() && !planning {
            st.pending = Some(Pending { total: ids.len(), queue: ids.clone(), ..Default::default() });
            st.result = None;
        }
        if has {
            let r = text_button(ui, "proposalsPreview", "Preview on the photo", st.preview)
                .on_hover_text(tr("Shows the planned retouch in the loupe without writing it; press again to see the original"));
            if r.clicked() {
                st.preview = !st.preview;
            }
        }
    });
    if let Some(p) = &photo {
        ui.label(RichText::new(summary(p, IMPACTS[st.impact.min(2)].0)).color(t.text_label).size(12.0));
        for w in p["steps"].as_array().into_iter().flatten().flat_map(|s| s["warnings"].as_array().into_iter().flatten()).filter_map(Value::as_str) {
            ui.label(RichText::new(format!("⚠ {w}")).color(t.caution).size(11.0));
        }
        if let Some(r) = p["autoReduced"].as_object() {
            ui.label(
                RichText::new(tr_format!(
                    "Dose reduced to {to} to stay natural: {reason}",
                    to = r["to"].to_string(),
                    reason = r["reason"].as_str().unwrap_or("")
                ))
                .color(t.caution)
                .size(11.0),
            );
        }
    }
    if has {
        ui.horizontal_wrapped(|ui| {
            let label = tr_format!("Create retouched copy ({impact})", impact = tr(IMPACTS[st.impact.min(2)].0));
            if crate::context_bar::primary_button(ui, "proposalsApply", &label, true)
                .on_hover_text(tr("A virtual copy stacked with the original, which stays untouched (one undo step)"))
                .clicked()
            {
                st.last_copies = apply_copies(app, ui.ctx(), st, &ids, false);
                st.preview = false;
            }
            if text_button(ui, "proposalsApplyVariants", "Create 3 copies", false)
                .on_hover_text(tr("Light, Medium and Strong, stacked with the original"))
                .clicked()
            {
                st.last_copies = apply_copies(app, ui.ctx(), st, &ids, true);
                st.preview = false;
            }
        });
    }
    // details: the steps, folded
    if let Some(p) = photo {
        let head =
            ui.add(egui::Button::new(RichText::new(format!("{} {}", if st.details { "v" } else { ">" }, tr("Details"))).size(12.0)).frame(false));
        register(ui.ctx(), "button:proposalsDetails", head.rect);
        if head.clicked() {
            st.details = !st.details;
        }
        if st.details {
            let mut changed = false;
            for step in p["steps"].as_array().cloned().unwrap_or_default() {
                let id = step["id"].as_str().unwrap_or("").to_string();
                let available = step["available"].as_bool().unwrap_or(false);
                let mut on = step["enabled"].as_bool().unwrap_or(false);
                let label = format!("{}. {}", step["n"].as_u64().unwrap_or(0), tr(step["label"].as_str().unwrap_or("")));
                let r = ui.add_enabled(available, egui::Checkbox::new(&mut on, label));
                register(ui.ctx(), format!("check:proposals:{id}"), r.rect);
                if r.changed() {
                    st.steps.insert(id.clone(), on);
                    changed = true;
                }
                let text = step["explain"].as_str().unwrap_or("");
                if !text.is_empty() {
                    ui.label(RichText::new(text).color(t.text_dim).size(10.5));
                }
            }
            let q = &p["qc"];
            ui.label(
                RichText::new(tr_format!(
                    "Checks: skin texture {texture} (tolerance 5 %), halo {halo} L, outside the zones: {pixels} pixels changed",
                    texture = q["textureChange"].as_f64().map_or("n/a".into(), |v| format!("{:+.1} %", v * 100.0)),
                    halo = q["halo"].to_string(),
                    pixels = q["outsidePixelsChanged"].to_string()
                ))
                .color(t.text_dim)
                .size(10.0),
            );
            if changed {
                st.pending = Some(Pending { total: ids.len(), queue: ids.clone(), ..Default::default() });
            }
        }
    }
    let r = ui.add(egui::Button::new(RichText::new(tr("Remove the retouch copies")).size(11.0)).frame(false));
    register(ui.ctx(), "button:proposalsClear", r.rect);
    if r.on_hover_text(tr("Moves the retouch copies of these photos to Recently Deleted; the originals stay")).clicked() {
        match app.run("retouch.clear", json!({"ids": ids})) {
            Ok(v) => {
                let n = v["copiesRemoved"].as_u64().unwrap_or(0);
                let m = v["removed"].as_u64().unwrap_or(0);
                app.toast(
                    ui.ctx(),
                    if n > 0 {
                        tr_format!("{n} retouch copies moved to Recently Deleted", n = n)
                    } else {
                        tr_format!("{m} retouch masks removed", m = m)
                    },
                );
            }
            Err(e) => app.toast_error(ui.ctx(), tr_format!("Remove: {e}", e = e)),
        }
        st.result = None;
        st.preview = false;
    }
    ui.label(
        RichText::new(tr("Proposing writes nothing. Validating makes a retouched COPY: the original is never touched.")).color(t.text_dim).size(10.5),
    );
    // the proposal on the photo, nothing written: the loupe's temporary look
    if st.preview
        && let Some(settings) = after
    {
        app.hover_preview = Some(crate::HoverPreview { label: tr("Proposal: Natural skin").into(), settings });
    }
}
