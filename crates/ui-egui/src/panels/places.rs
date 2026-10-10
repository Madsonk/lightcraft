//! Places: where the photos were taken. The list of places with photo counts (a click shows that place's photos),
//! Geotag (a preview of what the GPS positions resolve to, then Apply), naming a place for the selection, and the
//! places suggested for photos without GPS (to confirm one by one: nothing is written without a click).
//!
//! The work is the engine's `places.*` commands; this panel only calls them.

use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::Filter;
use lightcraft_catalog::places::{PlaceNode, PlaceSummary};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

const PAD: f32 = 20.0;
const HEADER_H: f32 = 44.0;
const ROW_H: f32 = 26.0;
/// Suggestions at or above this confidence are what "Confirm all" writes.
pub const CONFIRM_ALL_FROM: f64 = 0.8;

/// The panel's state: what the last preview and suggestion run returned, and the place being typed.
#[derive(Default)]
pub struct PlacesUi {
    pub preview: Option<Value>,
    pub suggestions: Option<Value>,
    pub name: String,
    pub message: String,
    summary: Option<(u64, Filter, Arc<PlaceSummary>)>,
}

impl PlacesUi {
    /// The places among the photos the filter lets through (its own `place` aside), recomputed when the catalog or the
    /// filter changes.
    pub fn summary(&mut self, cat: &lightcraft_catalog::Catalog, filter: &Filter) -> Arc<PlaceSummary> {
        let key = Filter { place: None, ..filter.clone() };
        match &self.summary {
            Some((r, f, s)) if *r == cat.revision && *f == key => s.clone(),
            _ => {
                let s = Arc::new(cat.places_in(&key));
                self.summary = Some((cat.revision, key, s.clone()));
                s
            }
        }
    }
}

fn selected(app: &LightcraftApp) -> Vec<u64> {
    app.session.targets(&json!({})).into_iter().map(|i| i.0).collect()
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let mut st = std::mem::take(&mut app.caches.places);
    let summary = st.summary(&app.session.catalog, &app.session.filter);
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    ui.painter().text(pos2(head.left() + PAD, head.center().y), Align2::LEFT_CENTER, "Places", t.semibold(15.0), t.text);
    let counts = format!("{} placed ({} from GPS only) · {} without a place", summary.placed, summary.from_gps, summary.unplaced);
    ui.painter().text(pos2(head.right() - PAD, head.center().y), Align2::RIGHT_CENTER, counts, t.font(13.0), t.text_dim);
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    super::chips::show(app, ui, &chips);
    let status = app.session.execute("places.status", &json!({})).unwrap_or_default();
    let installed = status["data"]["installed"].as_bool().unwrap_or(false);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        let w = ui.available_width();
        let left_w = ((w - PAD * 3.0) * 0.5).max(260.0);
        ui.horizontal_top(|ui| {
            ui.add_space(PAD);
            ui.vertical(|ui| {
                ui.set_width(left_w);
                list(app, ui, &summary, left_w);
            });
            ui.add_space(PAD);
            ui.vertical(|ui| {
                ui.set_width((w - left_w - PAD * 3.0).max(260.0));
                actions(app, ui, &mut st, &summary, &status, installed);
            });
        });
        ui.add_space(PAD);
    });
    app.caches.places = st;
}

fn row(ui: &mut egui::Ui, id: &str, indent: f32, label: &str, count: &str, hint: &str, active: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    register(ui.ctx(), id.to_string(), r);
    let p = ui.painter();
    if active {
        p.rect_filled(r, 4.0, t.tool_active);
        p.rect_stroke(r, 4.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 4.0, t.hover);
    }
    p.text(pos2(r.left() + 8.0 + indent, r.center().y), Align2::LEFT_CENTER, label, t.font(13.0), t.text);
    p.text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, count, t.font(12.0), t.text_dim);
    if !hint.is_empty() {
        let w = p.layout_no_wrap(count.to_string(), t.font(12.0), Color32::WHITE).size().x;
        p.text(pos2(r.right() - 16.0 - w, r.center().y), Align2::RIGHT_CENTER, hint, t.font(11.0), t.text_dim);
    }
    resp
}

fn list(app: &mut LightcraftApp, ui: &mut egui::Ui, sum: &PlaceSummary, _w: f32) {
    let t = Tokens::get(ui.ctx());
    let active = app.session.filter.place.clone();
    if sum.tree.is_empty() && sum.unplaced == 0 {
        ui.label(egui::RichText::new(crate::i18n::tr("No photos here")).color(t.text_dim));
        return;
    }
    let mut chosen: Option<Value> = None;
    for (depth, n) in sum.flatten() {
        let on = active.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&n.path));
        let hint = if n.from_gps > 0 && n.from_gps == n.count { "GPS" } else { "" };
        let resp = row(ui, &format!("place:{}", n.path), depth as f32 * 16.0, &n.name, &n.count.to_string(), hint, on).on_hover_text(path_label(n));
        if resp.clicked() {
            chosen = Some(json!({"place": n.path}));
        }
    }
    if sum.unplaced > 0 {
        ui.add_space(6.0);
        let on = active.as_deref().is_some_and(|a| a.eq_ignore_ascii_case("none"));
        if row(ui, "place:none", 0.0, crate::i18n::tr("No place"), &sum.unplaced.to_string(), "", on).clicked() {
            chosen = Some(json!({"place": "none"}));
        }
    }
    if let Some(p) = chosen {
        let _ = app.run("library.filter", p);
        let _ = app.run("view.photoGrid", json!({}));
    }
}

fn path_label(n: &PlaceNode) -> String {
    n.path.replacen("Places|", "", 1).replace('|', " › ")
}

fn actions(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &mut PlacesUi, sum: &PlaceSummary, status: &Value, installed: bool) {
    let t = Tokens::get(ui.ctx());
    let heading = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::i18n::tr(s)).font(t.semibold(13.0)).color(t.text));
        ui.add_space(2.0);
    };
    let dim = |ui: &mut egui::Ui, s: String| {
        ui.label(egui::RichText::new(s).font(t.font(12.0)).color(t.text_dim));
    };

    heading(ui, "Geotag from GPS");
    if !installed {
        let why = status["data"]["error"].as_str().unwrap_or("the places data is not installed").to_string();
        dim(ui, format!("Places data not found: {why}"));
    }
    dim(ui, format!("{} photos have a GPS position and no place keyword yet.", sum.gps_untagged));
    ui.horizontal(|ui| {
        let b = ui.add_enabled(installed, egui::Button::new(crate::i18n::tr("Preview Geotag")));
        register(ui.ctx(), "button:placesPreview", b.rect);
        if b.clicked() {
            match app.run("places.geotag", json!({"apply": false, "limit": 12})) {
                Ok(v) => {
                    st.message.clear();
                    st.preview = Some(v);
                }
                Err(e) => st.message = e,
            }
        }
        let n = st.preview.as_ref().and_then(|p| p["wouldWrite"].as_u64()).unwrap_or(0);
        let a = ui.add_enabled(n > 0, egui::Button::new(format!("{} {n}", crate::i18n::tr("Apply to"))));
        register(ui.ctx(), "button:placesApply", a.rect);
        if a.clicked() {
            match app.run("places.geotag", json!({"apply": true, "limit": 0})) {
                Ok(v) => {
                    st.message = format!("Geotagged {} photos (Edit > Undo takes it back).", v["written"]);
                    st.preview = None;
                }
                Err(e) => st.message = e,
            }
        }
    });
    if let Some(p) = &st.preview {
        let g = |k: &str| p[k].as_u64().unwrap_or(0);
        dim(
            ui,
            format!(
                "{} photos would get a place · {} already have one · {} without GPS · {} too far from any place",
                g("resolved"),
                g("alreadyPlaced"),
                g("withoutGps"),
                g("unresolved")
            ),
        );
        for x in p["places"].as_array().into_iter().flatten().take(8) {
            let label = x["place"].as_str().unwrap_or("").replacen("Places|", "", 1).replace('|', " › ");
            dim(ui, format!("   {label}: {}", x["count"]));
        }
        if let Some(first) = p["photos"].as_array().and_then(|a| a.first()) {
            dim(
                ui,
                format!(
                    "   e.g. {} → {} ({} km)",
                    first["fileName"].as_str().unwrap_or(""),
                    first["place"].as_str().unwrap_or(""),
                    first["distanceKm"]
                ),
            );
        }
    }

    ui.horizontal(|ui| {
        let mut auto = status["settings"]["autoGeotag"].as_bool().unwrap_or(false);
        let c = ui.checkbox(&mut auto, crate::i18n::tr("Geotag new photos on import"));
        register(ui.ctx(), "check:placesAuto", c.rect);
        if c.changed() {
            let _ = app.run("places.settings", json!({"autoGeotag": auto}));
        }
    });

    heading(ui, "Name a place");
    let sel = selected(app);
    ui.horizontal(|ui| {
        let e = ui.add(egui::TextEdit::singleline(&mut st.name).hint_text("Country|Region|City").desired_width(220.0));
        register(ui.ctx(), "input:placesName", e.rect);
        let ok = !sel.is_empty() && !st.name.trim().is_empty();
        let b = ui.add_enabled(ok, egui::Button::new(format!("{} ({})", crate::i18n::tr("Set for selection"), sel.len())));
        register(ui.ctx(), "button:placesName", b.rect);
        if b.clicked() {
            match app.run("places.name", json!({"place": st.name.trim()})) {
                Ok(v) => st.message = format!("{} photos set to {}.", v["changed"], v["place"].as_str().unwrap_or("")),
                Err(e) => st.message = e,
            }
        }
    });

    heading(ui, "Suggestions for photos without GPS");
    dim(ui, "From photos of the same camera taken minutes apart, and from photos that look alike. Nothing is written until you confirm.".into());
    ui.horizontal(|ui| {
        let b = ui.button(crate::i18n::tr(if sel.is_empty() { "Find Suggestions (library)" } else { "Find Suggestions (selection)" }));
        register(ui.ctx(), "button:placesSuggest", b.rect);
        if b.clicked() {
            let params = if sel.is_empty() { json!({}) } else { json!({"ids": sel}) };
            match app.run("places.suggest", params) {
                Ok(v) => {
                    st.message = v["notes"].as_array().and_then(|n| n.first()).and_then(Value::as_str).unwrap_or("").to_string();
                    st.suggestions = Some(v);
                }
                Err(e) => st.message = e,
            }
        }
        let n_high = st.suggestions.as_ref().map_or(0, |s| {
            s["suggestions"].as_array().map_or(0, |a| a.iter().filter(|x| x["confidence"].as_f64().unwrap_or(0.0) >= CONFIRM_ALL_FROM).count())
        });
        let all = ui.add_enabled(
            n_high > 0,
            egui::Button::new(format!("{} ≥ {}% ({n_high})", crate::i18n::tr("Confirm all"), (CONFIRM_ALL_FROM * 100.0) as u32)),
        );
        register(ui.ctx(), "button:placesConfirmAll", all.rect);
        if all.clicked() {
            let ids: Vec<u64> = st
                .suggestions
                .as_ref()
                .and_then(|s| s["suggestions"].as_array())
                .map(|a| a.iter().filter(|x| x["confidence"].as_f64().unwrap_or(0.0) >= CONFIRM_ALL_FROM).filter_map(|x| x["id"].as_u64()).collect())
                .unwrap_or_default();
            if app.run("places.confirm", json!({"ids": ids})).is_ok() {
                st.message = format!("Confirmed {} suggestions.", ids.len());
                if let Some(s) = st.suggestions.as_mut().and_then(|s| s["suggestions"].as_array_mut()) {
                    s.retain(|x| x["id"].as_u64().is_none_or(|i| !ids.contains(&i)));
                }
            }
        }
    });
    if let Some(s) = &st.suggestions {
        let (c, n) = (s["candidates"].as_u64().unwrap_or(0), s["suggested"].as_u64().unwrap_or(0));
        let un = s["unindexed"].as_u64().unwrap_or(0);
        dim(
            ui,
            format!(
                "{n} suggestions for {c} photos without a place{}",
                if un > 0 { format!(" ({un} not indexed yet for look-alikes: run Search ▸ Index)") } else { String::new() }
            ),
        );
    }
    let rows: Vec<Value> = st.suggestions.as_ref().and_then(|s| s["suggestions"].as_array()).cloned().unwrap_or_default();
    let mut acted: Option<(u64, bool)> = None;
    for x in rows.iter().take(40) {
        let id = x["id"].as_u64().unwrap_or(0);
        ui.horizontal(|ui| {
            let conf = (x["confidence"].as_f64().unwrap_or(0.0) * 100.0).round();
            let src = if x["source"] == "time" { "by time" } else { "by look" };
            let place = x["place"].as_str().unwrap_or("").replacen("Places|", "", 1).replace('|', " › ");
            let name = x["fileName"].as_str().unwrap_or("");
            ui.label(egui::RichText::new(format!("{name}  →  {place}  ({conf}% {src})")).font(t.font(12.0)).color(t.text));
            let y = ui.small_button(crate::i18n::tr("Confirm"));
            register(ui.ctx(), format!("button:placesConfirm:{id}"), y.rect);
            if y.clicked() {
                acted = Some((id, true));
            }
            let n = ui.small_button(crate::i18n::tr("Dismiss"));
            if n.clicked() {
                acted = Some((id, false));
            }
        });
    }
    if let Some((id, yes)) = acted {
        let cmd = if yes { "places.confirm" } else { "places.dismiss" };
        let _ = app.run(cmd, json!({"ids": [id]}));
        if let Some(s) = st.suggestions.as_mut().and_then(|s| s["suggestions"].as_array_mut()) {
            s.retain(|x| x["id"].as_u64() != Some(id));
        }
    }
    if rows.len() > 40 {
        dim(ui, format!("… and {} more", rows.len() - 40));
    }
    if !st.message.is_empty() {
        ui.add_space(6.0);
        dim(ui, st.message.clone());
    }
    ui.add_space(14.0);
    dim(ui, "Place data from GeoNames (geonames.org), CC BY 4.0.".into());
    let _ = Rect::NOTHING;
}
