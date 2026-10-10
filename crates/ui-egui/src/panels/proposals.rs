//! Proposals: the proposed retouch ("Natural skin") at the top of the Versions panel. Pressing
//! Propose plans the eight ordered steps for the selected photos and writes nothing; every step
//! has its switch and a one-line explanation with measured figures; Apply writes ordinary
//! editable settings in one undo step, and "Apply with 3 variants" also leaves Before, Light,
//! Medium and Strong in Versions below. Same commands as the CLI: `retouch.propose`,
//! `retouch.apply`, `retouch.clear`.

use std::collections::BTreeMap;

use egui::RichText;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// What the section remembers between frames.
#[derive(Clone, Default)]
struct State {
    /// 0 Light, 1 Medium, 2 Strong.
    impact: usize,
    steps: BTreeMap<String, bool>,
    result: Option<Value>,
    open: bool,
}

const IMPACTS: [(&str, &str); 3] = [("Light", "light"), ("Medium", "medium"), ("Strong", "strong")];

fn ids_of(app: &LightcraftApp) -> Vec<u64> {
    let s = &app.session;
    if s.selection.ids.is_empty() { s.active().map(|i| i.0).into_iter().collect() } else { s.selection.ids.iter().map(|i| i.0).collect() }
}

fn params(st: &State, ids: &[u64]) -> Value {
    json!({"ids": ids, "recipe": "natural-skin", "impact": IMPACTS[st.impact.min(2)].1, "steps": st.steps})
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
        egui::Button::new(RichText::new(format!("{} Proposals · Natural skin", if st.open { "▾" } else { "▸" })).font(t.semibold(12.0))).frame(false),
    );
    register(ui.ctx(), "button:proposalsHeader", head.rect);
    if head.clicked() {
        st.open = !st.open;
    }
    if st.open {
        ui.label(
            RichText::new("Proposing writes nothing. Apply writes ordinary settings you can edit, in one undo step.").color(t.text_dim).size(10.5),
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
        ui.horizontal_wrapped(|ui| {
            let r = ui.add_enabled(!ids.is_empty(), egui::Button::new("Propose"));
            register(ui.ctx(), "button:proposalsPropose", r.rect);
            if r.clicked() {
                match app.run("retouch.propose", params(&st, &ids)) {
                    Ok(v) => st.result = Some(v),
                    Err(e) => app.toast(ui.ctx(), format!("Propose: {e}")),
                }
            }
            let has = st.result.is_some();
            let r = ui.add_enabled(has, egui::Button::new("Apply"));
            register(ui.ctx(), "button:proposalsApply", r.rect);
            if r.clicked() {
                match app.run("retouch.apply", params(&st, &ids)) {
                    Ok(_) => app.toast(ui.ctx(), "Natural skin applied: one undo step".to_string()),
                    Err(e) => app.toast(ui.ctx(), format!("Apply: {e}")),
                }
            }
            let r = ui.add_enabled(has, egui::Button::new("Apply with 3 variants"));
            register(ui.ctx(), "button:proposalsApplyVariants", r.rect);
            if r.clicked() {
                let mut p = params(&st, &ids);
                p["variants"] = json!(true);
                match app.run("retouch.apply", p) {
                    Ok(_) => app.toast(ui.ctx(), "Applied; Before, Light, Medium and Strong are in Versions".to_string()),
                    Err(e) => app.toast(ui.ctx(), format!("Apply: {e}")),
                }
            }
        });
        if st.result.is_some() {
            let r = ui.small_button("Remove the retouch");
            register(ui.ctx(), "button:proposalsClear", r.rect);
            if r.clicked() {
                let _ = app.run("retouch.clear", json!({"ids": ids}));
                st.result = None;
            }
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
                match app.run("retouch.propose", params(&st, &ids)) {
                    Ok(v) => st.result = Some(v),
                    Err(e) => app.toast(ui.ctx(), format!("Propose: {e}")),
                }
            }
        }
    }
    ui.add_space(6.0);
    ui.separator();
    ui.data_mut(|d| d.insert_temp(key, st));
    let _ = text_button;
}
