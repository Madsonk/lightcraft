//! The left column of the Retouch space (LightCraft v14, wireframe 04): what the retoucher needs beside
//! the photo without opening a right-hand panel — the **selection** being retouched (count, position,
//! previous / next, a strip of its thumbnails), the photo's **versions** (with their look as a thumbnail;
//! click restores, nothing else changes) and its **history** (click goes back to that step). It takes
//! the place of the Library sources column only in the Retouch space with workspaces on; the width is
//! the shared `left_width`, resized by the same handle.

use egui::{Align2, Rect, RichText, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Whether this column replaces the Library sources column.
pub fn wants(app: &LightcraftApp) -> bool {
    crate::workspaces::enabled(app) && app.workspaces.kind == crate::workspaces::Space::Retouch
}

fn title(ui: &mut egui::Ui, text: &str, right: Option<String>) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 16.0, r.center().y + 2.0), Align2::LEFT_CENTER, tr(text), t.semibold(13.0), t.text);
    if let Some(right) = right {
        ui.painter().text(pos2(r.right() - 16.0, r.center().y + 2.0), Align2::RIGHT_CENTER, right, t.font(11.5), t.text_dim);
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.chrome).stroke(Stroke::new(1.0, t.divider));
    let reserve = if app.ui.right != crate::state::RightPanel::None { app.ui.right_width } else { 0.0 } + t.strip_w;
    let resized = crate::panels::resizable_side(ui, true, "retouch_left", frame, app.ui.left_width, crate::state::LEFT_WIDTH, reserve, |ui| {
        egui::ScrollArea::vertical().id_salt("retouch-left-scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().scroll.floating = false;
            ui.spacing_mut().item_spacing.y = 0.0;
            navigator(app, ui);
            presets(app, ui);
            versions(app, ui);
            history(app, ui);
        });
    });
    if let Some(w) = resized {
        app.ui.left_width = w;
    }
}

pub(crate) fn navigator(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    title(ui, "Navigator", None);
    if let Some(id) = app.session.active() {
        let width = (ui.available_width() - 32.0).max(1.0);
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::hover());
        let rect = Rect::from_center_size(area.center(), vec2(width, 154.0));
        crate::panels::grid::request_thumb(app, id, (width * ui.ctx().pixels_per_point()) as usize, 6);
        if let Some(tex) = app.renderer.thumb(id) {
            let fit = crate::panels::detail::fit_texture_rect(rect, tex.size);
            ui.painter().image(tex.tex.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        ui.horizontal(|ui| {
            for (widget, label, command, params) in
                [("developFit", "Fit", "view.zoomFit", json!({})), ("develop100", "100%", "view.zoom100", json!({}))]
            {
                if text_button(ui, widget, label, false).clicked() {
                    let _ = app.run(command, params);
                }
            }
        });
    }
}

fn presets(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(tr("Presets")).default_open(true).show(ui, |ui| {
        let presets: Vec<_> = app.session.presets.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        egui::ScrollArea::vertical().id_salt("develop-presets-list").max_height(180.0).show(ui, |ui| {
            for (id, name) in presets {
                if text_button(ui, &format!("developPreset:{id}"), &name, false).clicked() {
                    let _ = app.run("preset.apply", json!({"id": id, "amount": 100}));
                }
            }
        });
    });
}

fn versions(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = app.session.active() else { return };
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    title(ui, "Versions", Some(p.versions.len().to_string()));
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 8 }).show(ui, |ui| {
        if text_button(ui, "retouchVersionCreate", "Create Version", false)
            .on_hover_text(tr("Keeps this look under a name (Shift+V opens the panel)"))
            .clicked()
        {
            let _ = app.run("version.create", json!({}));
        }
        ui.add_space(6.0);
        if p.versions.is_empty() {
            ui.label(RichText::new(tr("No versions yet. Create one to keep this look.")).color(t.text_dim).size(11.5));
        }
        for (i, v) in p.versions.iter().enumerate().rev() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::click());
            register(ui.ctx(), format!("retouchVersion:{i}"), r);
            let current = *v.settings == *p.develop;
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, t.hover.gamma_multiply(0.7));
            }
            let tr_rect = Rect::from_min_size(r.min + vec2(0.0, 4.0), vec2(62.0, 42.0));
            ui.painter().rect_filled(tr_rect, 2.0, t.canvas);
            if let Some(job) = app.session.variant_job(id, &v.settings, 140)
                && let Some(tex) = app.renderer.variant(job)
            {
                ui.painter().image(tex.tex.id(), tr_rect, crate::panels::presets::cover_uv(tr_rect, tex.size), egui::Color32::WHITE);
            }
            let x = tr_rect.right() + 8.0;
            ui.painter().with_clip_rect(r.with_max_x(r.right() - 20.0)).text(
                pos2(x, r.top() + 16.0),
                Align2::LEFT_CENTER,
                &v.name,
                t.semibold(12.0),
                t.text,
            );
            ui.painter().text(pos2(x, r.top() + 34.0), Align2::LEFT_CENTER, tr(if v.auto { "automatic" } else { "named" }), t.font(10.5), t.text_dim);
            if current {
                paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 10.0, r.center().y), vec2(12.0, 12.0)), Icon::Check, t.accent);
            }
            if resp.hovered() && !current {
                app.hover_preview =
                    Some(crate::HoverPreview { label: crate::i18n::tr_format!("Version: {}", v.name), settings: (*v.settings).clone() });
            }
            if resp.on_hover_text(tr(if current { "The photo has these settings" } else { "Click to restore" })).clicked() && !current {
                let _ = app.run("version.restore", json!({"index": i}));
            }
        }
    });
    crate::widgets::divider(ui);
}

/// The history of the active photo, newest first; click goes back to that step (an undo step itself).
fn history(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = app.session.active() else { return };
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    title(ui, "History", Some(p.history.len().to_string()));
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 10 }).show(ui, |ui| {
        if p.history.is_empty() {
            ui.label(RichText::new(tr("No edits yet.")).color(t.text_dim).size(11.5));
        }
        for (i, h) in p.history.iter().enumerate().rev().take(40) {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            register(ui.ctx(), format!("retouchHistory:{i}"), r);
            if resp.hovered() {
                ui.painter().rect_filled(r, 3.0, t.hover);
            }
            paint(ui.painter(), Rect::from_min_size(r.min + vec2(0.0, 4.0), vec2(14.0, 14.0)), Icon::Clock, t.icon);
            ui.painter().with_clip_rect(r).text(
                pos2(r.left() + 20.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::history_label(&h.label, &app.session.presets),
                t.font(11.5),
                t.text_label,
            );
            if resp.on_hover_text(tr("Click to go back to this step")).clicked() {
                let _ = app.run("history.restore", json!({"index": i}));
            }
        }
    });
}
