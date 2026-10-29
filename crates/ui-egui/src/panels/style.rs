//! "My style" and "Auto crop" in the Retouch step (tool strip ▸ Style): learn how you edit from
//! the photos you already edited, preview that style on a shoot (Before / After, nothing stored),
//! apply it as ONE undo step; propose a straighten and a crop, again as a preview first.
//! All through `style.*`, `autostraighten.*` and `autocrop.*` (see `docs/style-ai.md`); the heavy
//! work runs on worker threads and this panel only polls their status once a frame.

use egui::RichText;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::design::{self, Kind, State};
use crate::i18n::tr;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Photos a style needs before learning is worth trying.
const MIN_PHOTOS: u64 = 30;

const ASPECTS: [(&str, &str); 5] = [("original", "Original"), ("1x1", "1 × 1"), ("4x5", "4 × 5"), ("3x2", "3 × 2"), ("16x9", "16 × 9")];

fn temp<T: Clone + Send + Sync + 'static>(ui: &egui::Ui, key: &str, default: T) -> T {
    ui.data(|d| d.get_temp::<T>(egui::Id::new(key))).unwrap_or(default)
}

fn put<T: Clone + Send + Sync + 'static>(ui: &egui::Ui, key: &str, v: T) {
    ui.data_mut(|d| d.insert_temp(egui::Id::new(key), v));
}

/// The photos the commands act on: the selection, else the photo in front.
fn targets(app: &LightcraftApp) -> Vec<u64> {
    let ids: Vec<u64> = app.session.selection.ids.iter().map(|i| i.0).collect();
    if ids.is_empty() { app.session.active().map(|i| vec![i.0]).unwrap_or_default() } else { ids }
}

fn bar(ui: &mut egui::Ui, percent: f64) {
    ui.add(egui::ProgressBar::new((percent / 100.0).clamp(0.0, 1.0) as f32).desired_height(8.0));
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    super::right::header(ui, "My style");
    register(ui, "region:stylePanel", ui.max_rect());
    let status = app.run("style.status", json!({})).unwrap_or(Value::Null);
    let listing = app.run("style.list", json!({})).unwrap_or(Value::Null);
    let busy = ["train", "apply", "framing"].iter().any(|k| status[k]["state"] == "running");
    if busy {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
    }
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 12 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        my_style(app, ui, &t, &status, &listing);
        ui.add_space(10.0);
        ui.separator();
        auto_crop(app, ui, &t, &status);
    });
}

fn my_style(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, status: &Value, listing: &Value) {
    let edited = listing["editedPhotos"].as_u64().unwrap_or(0);
    let profiles = listing["profiles"].as_array().cloned().unwrap_or_default();
    let train = &status["train"];
    // ---- learning in progress
    if train["state"] == "running" {
        design::status_line(ui, State::Busy, tr("Learning how you edit…"));
        bar(ui, train["percent"].as_f64().unwrap_or(0.0));
        ui.label(
            RichText::new(format!("{} / {}", train["measured"].as_u64().unwrap_or(0), train["total"].as_u64().unwrap_or(0)))
                .size(11.0)
                .color(t.text_dim),
        );
        if design::button(ui, "styleStop", tr("Stop"), Kind::Quiet, true).clicked() {
            let _ = app.run("style.cancel", json!({}));
        }
        return;
    }
    if train["state"] == "failed"
        && let Some(e) = train["error"].as_str()
    {
        design::status_line(ui, State::Error, e);
    }
    // ---- empty: nothing learned yet
    let Some(profile) = profiles.first() else {
        ui.label(
            RichText::new(tr("Learns from the photos you already edited how you set exposure, white balance, contrast and colour, then suggests it on new photos. Everything stays on this Mac."))
                .size(11.5)
                .color(t.text_dim),
        );
        if edited < MIN_PHOTOS {
            design::status_line(ui, State::Info, &format!("{edited} {} {}", tr("edited photos found, at least"), MIN_PHOTOS));
            ui.label(
                RichText::new(tr("Import your Lightroom catalogue (File ▸ Import Lightroom Catalog) or edit more photos, then come back."))
                    .size(11.0)
                    .color(t.text_dim),
            );
        } else if design::button(ui, "styleTrain", tr("Learn my style"), Kind::Primary, true).clicked() {
            let _ = app.run("style.train", json!({}));
        }
        return;
    };
    // ---- a profile exists
    let samples = profile["samples"].as_u64().unwrap_or(0);
    let learned = profile["learned"].as_u64().unwrap_or(0);
    let total = profile["targets"].as_u64().unwrap_or(0);
    design::status_line(
        ui,
        State::Ok,
        &format!("{} · {} {} · {learned}/{total} {}", profile["name"].as_str().unwrap_or(""), samples, tr("photos"), tr("settings learned")),
    );
    let none = profile["skillVsNone"].as_f64().unwrap_or(0.0);
    let median = profile["skillVsMedian"].as_f64().unwrap_or(0.0);
    ui.label(
        RichText::new(format!(
            "{} {:+.0} % {} · {:+.0} % {}",
            tr("On photos it never saw:"),
            none * 100.0,
            tr("closer than leaving them as they are"),
            median * 100.0,
            tr("than your average setting")
        ))
        .size(11.0)
        .color(if none > 0.1 { t.text_label } else { t.caution }),
    );
    if samples < 200 {
        ui.label(RichText::new(tr("Few examples: the more photos you edited, the better the suggestions.")).size(11.0).color(t.caution));
    }
    // ---- preview on the selection
    let ids = targets(app);
    let n = ids.len();
    let strength: f32 = temp(ui, "styleStrength", 1.0f32);
    let consistency: f32 = temp(ui, "styleConsistency", 0.7f32);
    let mut s = strength * 100.0;
    let mut c = consistency * 100.0;
    // label above, slider as wide as the panel: a slider with its text beside it widens the whole panel
    ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(80.0);
    ui.label(RichText::new(tr("Strength")).size(11.5).color(t.text_label));
    if ui.add(egui::Slider::new(&mut s, 0.0..=100.0).integer()).changed() {
        put(ui, "styleStrength", s / 100.0);
    }
    ui.label(RichText::new(tr("Series consistency")).size(11.5).color(t.text_label));
    if ui
        .add(egui::Slider::new(&mut c, 0.0..=100.0).integer())
        .on_hover_text(tr("Photos of one shoot get the same style, adapted to each photo's exposure"))
        .changed()
    {
        put(ui, "styleConsistency", c / 100.0);
    }
    let apply = &status["apply"];
    let running = apply["state"] == "running";
    if running {
        design::status_line(ui, State::Busy, tr("Preparing the preview…"));
        bar(ui, apply["percent"].as_f64().unwrap_or(0.0));
        if design::button(ui, "styleStopPlan", tr("Stop"), Kind::Quiet, true).clicked() {
            let _ = app.run("style.cancel", json!({}));
        }
    } else {
        let label = format!("{} ({n})", tr("Preview on the selection"));
        if design::button(ui, "stylePreview", &label, Kind::Secondary, n > 0).clicked() {
            let r = app.run("style.apply", json!({"mode": "preview", "ids": ids, "strength": s / 100.0, "consistency": c / 100.0}));
            if let Err(e) = r {
                app.toast(ui.ctx(), e);
            }
        }
    }
    if n == 0 {
        ui.label(RichText::new(tr("Select the photos of a shoot to try your style on them.")).size(11.0).color(t.text_dim));
    }
    let state = apply["state"].as_str().unwrap_or("idle");
    if state == "done" || state == "applied" {
        let rows = apply["photos"].as_array().cloned().unwrap_or_default();
        let changes = apply["changes"].as_u64().unwrap_or(0);
        let skipped = rows.iter().filter(|r| r["alreadyEdited"] == true && r["changes"] == false).count();
        let failed = rows.iter().filter(|r| r["error"].is_string()).count();
        design::status_line(
            ui,
            if changes > 0 { State::Info } else { State::Warning },
            &format!(
                "{changes} {} · {} {} · {} {}",
                tr("photos would change"),
                apply["series"].as_u64().unwrap_or(0),
                tr("series"),
                skipped,
                tr("already edited, kept")
            ),
        );
        if failed > 0 {
            design::status_line(ui, State::Warning, &format!("{failed} {}", tr("photos could not be read")));
        }
        // Before / After of the photo in front (else the first one that changes)
        let review: Option<u64> = app
            .session
            .active()
            .map(|i| i.0)
            .filter(|i| app.session.style_proposal(lightcraft_catalog::PhotoId(*i)).is_some())
            .or_else(|| rows.iter().find(|r| r["changes"] == true).and_then(|r| r["id"].as_u64()));
        if let Some(id) = review {
            let after = app.session.style_proposal(lightcraft_catalog::PhotoId(id));
            pair(app, ui, id, after);
            if let Some(row) = rows.iter().find(|r| r["id"].as_u64() == Some(id)) {
                let line: Vec<String> = row["sliders"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .take(6)
                            .map(|x| format!("{} {:+.2}", tr(slider_label(x["id"].as_str().unwrap_or(""))), x["value"].as_f64().unwrap_or(0.0)))
                            .collect()
                    })
                    .unwrap_or_default();
                ui.label(RichText::new(line.join(" · ")).size(10.5).color(t.text_dim));
            }
        }
        ui.horizontal_wrapped(|ui| {
            if state == "done" && changes > 0 {
                let label = format!("{} ({changes})", tr("Apply my style"));
                if design::button(ui, "styleApply", &label, Kind::Primary, true).clicked() {
                    match app.run("style.apply", json!({"mode": "apply", "confirm": true})) {
                        Ok(v) => {
                            app.toast(ui.ctx(), format!("{} {}", v["changed"].as_u64().unwrap_or(0), tr("photos styled — Undo reverts all of them")))
                        }
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
            }
            if state == "applied" && design::button(ui, "styleUndo", tr("Undo my style"), Kind::Secondary, true).clicked() {
                let _ = app.run("style.apply", json!({"mode": "undo"}));
            }
        });
    }
    ui.add_space(2.0);
    if text_button(ui, "styleRetrain", tr("Learn again"), false)
        .on_hover_text(tr("Replaces this profile with a new one from your edited photos"))
        .clicked()
    {
        let _ = app.run("style.train", json!({}));
    }
    egui::CollapsingHeader::new(RichText::new(tr("What was measured")).font(t.semibold(11.5))).id_salt("styleMeasured").default_open(false).show(ui, |ui| {
        let sliders = profile["sliders"].as_array().cloned().unwrap_or_default();
        for sl in sliders {
            let id = sl["id"].as_str().unwrap_or("");
            let on = sl["learned"] == true;
            ui.label(
                RichText::new(format!(
                    "{} {} · {:.1} {} {:.1}",
                    if on { "✓" } else { "·" },
                    tr(slider_label(id)),
                    sl["maeModel"].as_f64().unwrap_or(0.0),
                    tr("vs"),
                    sl["maeNone"].as_f64().unwrap_or(0.0)
                ))
                .size(10.5)
                .color(if on { t.text_label } else { t.text_dim }),
            );
        }
        ui.label(RichText::new(tr("Mean error on unseen shoots, model vs leaving the slider at 0. A dot: the data could not do better than your median, so that slider is left alone.")).size(10.5).color(t.text_dim));
    });
}

fn slider_label(id: &str) -> &'static str {
    match id {
        "exposure" => "Exposure",
        "temp" => "Temperature",
        "tint" => "Tint",
        "contrast" => "Contrast",
        "highlights" => "Highlights",
        "shadows" => "Shadows",
        "whites" => "Whites",
        "blacks" => "Blacks",
        "vibrance" => "Vibrance",
        "saturation" => "Saturation",
        "curve_highlights" => "Curve: highlights",
        "curve_lights" => "Curve: lights",
        "curve_darks" => "Curve: darks",
        "curve_shadows" => "Curve: shadows",
        _ => "Setting",
    }
}

fn auto_crop(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, status: &Value) {
    design::section_title(ui, "Auto crop");
    ui.label(RichText::new(tr("Suggests a straighten and a crop. Nothing changes until you accept.")).size(11.5).color(t.text_dim));
    let ids = targets(app);
    let aspect: String = temp(ui, "styleAspect", "original".to_string());
    ui.horizontal_wrapped(|ui| {
        for (key, label) in ASPECTS {
            if text_button(ui, &format!("cropAspect-{key}"), label, aspect == key).clicked() {
                put(ui, "styleAspect", key.to_string());
            }
        }
    });
    let fr = &status["framing"];
    let running = fr["state"] == "running";
    ui.horizontal_wrapped(|ui| {
        if design::button(ui, "autoStraighten", tr("Suggest straighten"), Kind::Secondary, !ids.is_empty() && !running).clicked() {
            let _ = app.run("autostraighten.suggest", json!({"ids": ids}));
        }
        if design::button(ui, "autoCrop", tr("Suggest crop"), Kind::Secondary, !ids.is_empty() && !running).clicked() {
            let _ = app.run("autocrop.suggest", json!({"ids": ids, "aspect": aspect}));
        }
    });
    if ids.is_empty() {
        ui.label(RichText::new(tr("Select a photo first.")).size(11.0).color(t.text_dim));
    }
    if running {
        design::status_line(ui, State::Busy, tr("Looking at the photos…"));
        bar(ui, fr["percent"].as_f64().unwrap_or(0.0));
        if design::button(ui, "autoStop", tr("Stop"), Kind::Quiet, true).clicked() {
            let _ = app.run("style.cancel", json!({}));
        }
        return;
    }
    let state = fr["state"].as_str().unwrap_or("idle");
    if !matches!(state, "done" | "applied") {
        return;
    }
    let kind = fr["kind"].as_str().unwrap_or("crop");
    let rows = fr["photos"].as_array().cloned().unwrap_or_default();
    let n = fr["suggestions"].as_u64().unwrap_or(0);
    if n == 0 {
        let why = rows.first().and_then(|r| r["reason"].as_str()).unwrap_or("");
        design::status_line(
            ui,
            State::Ok,
            &if why.is_empty() { tr("Nothing to suggest: framing and horizon look fine.").to_string() } else { why.to_string() },
        );
        return;
    }
    design::status_line(ui, State::Info, &format!("{n} {}", tr("suggestions")));
    let review =
        app.session.active().map(|i| i.0).filter(|i| app.session.framing_proposal(lightcraft_catalog::PhotoId(*i)).is_some()).or_else(|| {
            rows.iter().find_map(|r| {
                let id = r["id"].as_u64()?;
                app.session.framing_proposal(lightcraft_catalog::PhotoId(id)).map(|_| id)
            })
        });
    if let Some(id) = review {
        let after = app.session.framing_proposal(lightcraft_catalog::PhotoId(id));
        pair(app, ui, id, after);
        if let Some(row) = rows.iter().find(|r| r["id"].as_u64() == Some(id)) {
            let sug = &row["suggestion"];
            let mut bits = Vec::new();
            if let Some(a) = sug["angle"].as_f64() {
                bits.push(format!("{:+.1}°", a));
            }
            if let Some(cf) = sug["confidence"].as_f64() {
                bits.push(format!("{} {:.0} %", tr("confidence"), cf * 100.0));
            }
            if let Some(rs) = sug["reasons"].as_array() {
                bits.extend(rs.iter().filter_map(|x| x.as_str().map(str::to_string)));
            }
            ui.label(RichText::new(bits.join(" · ")).size(10.5).color(t.text_dim));
        }
    }
    if state == "done" {
        let (cmd, label) = if kind == "crop" { ("autocrop.apply", "Accept the crop") } else { ("autostraighten.apply", "Accept the straighten") };
        if design::button(ui, "autoAccept", tr(label), Kind::Primary, true).clicked() {
            match app.run(cmd, json!({"confirm": true})) {
                Ok(v) => app.toast(ui.ctx(), format!("{} {}", v["changed"].as_u64().unwrap_or(0), tr("photos reframed — Undo reverts all of them"))),
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
    } else if design::button(ui, "autoUndo", tr("Undo"), Kind::Secondary, !app.session.undo.is_empty()).clicked() {
        let _ = app.run("edit.undo", json!({}));
    }
}

/// Before / After of a photo: its current look and the proposed one, side by side (nothing is stored).
fn pair(app: &mut LightcraftApp, ui: &mut egui::Ui, active: u64, after: Option<lightcraft_develop::DevelopSettings>) {
    use egui::{Align2, Rect, pos2, vec2};
    let t = Tokens::get(ui.ctx());
    let id = lightcraft_catalog::PhotoId(active);
    let w = ui.available_width();
    let gap = 8.0;
    let tile = vec2(((w - gap) / 2.0).max(60.0), ((w - gap) / 2.0 * 0.66).max(40.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, tile.y + 18.0), egui::Sense::hover());
    let before = app.session.develop_of(id).map(|d| (*d).clone()).unwrap_or_default();
    let looks: [(&str, Option<lightcraft_develop::DevelopSettings>, &str); 2] =
        [("Before", Some(before), "styleBefore"), ("After", after, "styleAfter")];
    for (i, (label, settings, wid)) in looks.into_iter().enumerate() {
        let rect = Rect::from_min_size(r.min + vec2(i as f32 * (tile.x + gap), 0.0), tile);
        crate::widgets::register(ui, format!("image:{wid}"), rect);
        ui.painter().rect_filled(rect, 3.0, t.canvas);
        match settings {
            Some(s) => {
                if let Some(job) = app.session.variant_job(id, &s, 200)
                    && let Some(tex) = app.renderer.variant(job)
                {
                    ui.painter().image(tex.tex.id(), rect, super::presets::cover_uv(rect, tex.size), egui::Color32::WHITE);
                } else {
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "…", t.font(14.0), t.text_dim);
                    ui.ctx().request_repaint();
                }
            }
            None => {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, tr("No proposal"), t.font(11.0), t.text_dim);
            }
        }
        ui.painter().text(pos2(rect.left(), rect.bottom() + 9.0), Align2::LEFT_CENTER, tr(label), t.semibold(11.0), t.text_label);
    }
}
