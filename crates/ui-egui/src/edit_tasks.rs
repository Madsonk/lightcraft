//! The Develop panel **by task** (LightCraft v14, phase 3 of the UX study): the same 114 controls as
//! the panel by type (Light, Color, Effects…), regrouped the way a retoucher works — **Base** (profile,
//! white balance, exposure), **Light** (tones, curve), **Color** (vibrance, mixer, grading, calibration),
//! **Detail** (texture, sharpening, noise, grain), **Optics** (lens, geometry, vignette) and **Local**
//! (the tools that touch a part of the photo) — plus a search field that finds any control by its name.
//! The sections fold and their state is remembered with the other Develop sections (`open_sections`,
//! ids `task.*`). Every slider is drawn by [`crate::panels::edit::control`], so a value changed here is
//! the same `develop.set` as in the panel by type. The toggle lives in the Develop header and is kept in
//! `ui.json` (`edit_by_task`); it is only offered with workspaces on.

use egui::{RichText, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{CONTROLS, DevelopSettings};
use serde_json::json;

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::panels::edit::{self, control};
use crate::theme::Tokens;
use crate::widgets::{divider, flyout_row, register, section_header, text_button};

/// A task group: id (section state `task.<id>`), title, and the controls it holds (used by the search
/// as well, so a control belongs to exactly one task).
pub struct Task {
    pub id: &'static str,
    pub title: &'static str,
    pub controls: &'static [&'static str],
}

pub const TASKS: &[Task] = &[
    Task { id: "base", title: "Base", controls: &["profile.amount", "wb.temp", "wb.tint", "light.exposure", "light.contrast"] },
    Task {
        id: "light",
        title: "Light",
        controls: &[
            "light.highlights",
            "light.shadows",
            "light.whites",
            "light.blacks",
            "curve.highlights",
            "curve.lights",
            "curve.darks",
            "curve.shadows",
            "effects.dehaze",
        ],
    },
    Task {
        id: "color",
        title: "Color",
        controls: &[
            "color.vibrance",
            "color.saturation",
            "calibration.shadowsTint",
            "calibration.redHue",
            "calibration.redSat",
            "calibration.greenHue",
            "calibration.greenSat",
            "calibration.blueHue",
            "calibration.blueSat",
        ],
    },
    Task {
        id: "detail",
        title: "Detail",
        controls: &[
            "effects.texture",
            "effects.clarity",
            "detail.sharpenAmount",
            "detail.sharpenRadius",
            "detail.sharpenDetail",
            "detail.sharpenMasking",
            "detail.nrLuminance",
            "detail.nrDetail",
            "detail.nrContrast",
            "detail.nrColor",
            "detail.nrColorDetail",
            "detail.nrColorSmoothness",
            "grain.amount",
            "grain.size",
            "grain.roughness",
        ],
    },
    Task {
        id: "optics",
        title: "Optics",
        controls: &[
            "optics.distortion",
            "optics.vignetting",
            "optics.vignettingMidpoint",
            "optics.defringePurple",
            "optics.defringeGreen",
            "optics.caRed",
            "optics.caBlue",
            "geometry.vertical",
            "geometry.horizontal",
            "geometry.rotate",
            "geometry.aspect",
            "geometry.scale",
            "geometry.offsetX",
            "geometry.offsetY",
            "crop.angle",
            "vignette.amount",
            "vignette.midpoint",
            "vignette.roundness",
            "vignette.feather",
            "vignette.highlights",
        ],
    },
    Task { id: "local", title: "Local", controls: &[] },
];

/// Whether the Develop panel shows by task: workspaces on and the toggle on.
pub fn wants(app: &LightcraftApp) -> bool {
    crate::workspaces::enabled(app) && app.ui.edit_by_task
}

/// Accent- and case-insensitive containment, so "nettete" finds "Netteté".
fn fold(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            'à' | 'â' | 'ä' => Some('a'),
            'é' | 'è' | 'ê' | 'ë' => Some('e'),
            'î' | 'ï' => Some('i'),
            'ô' | 'ö' => Some('o'),
            'ù' | 'û' | 'ü' => Some('u'),
            'ç' => Some('c'),
            c if c.is_alphanumeric() || c == ' ' => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// The controls whose translated label (or id) contains the query, in panel order.
pub fn search(query: &str) -> Vec<&'static str> {
    let q = fold(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    CONTROLS
        .iter()
        .filter(|c| fold(tr(c.label)).contains(&q) || c.id.to_ascii_lowercase().contains(&q) || fold(task_title_of(c.id)).contains(&q))
        .map(|c| c.id)
        .collect()
}

fn task_title_of(control: &str) -> &'static str {
    TASKS.iter().find(|t| t.controls.contains(&control)).map(|t| tr(t.title)).unwrap_or("")
}

/// The header shared with the panel by type: the title, Auto / B&W / Reset, and the two view buttons.
fn header(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(hr.left() + 24.0, hr.bottom() - 10.0), egui::Align2::LEFT_CENTER, tr("Develop"), t.semibold(15.0), t.text);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if text_button(ui, "auto", "Auto", false).clicked() {
                let _ = app.run("develop.auto", json!({}));
                app.toast(ui.ctx(), tr("Auto settings applied"));
            }
            if text_button(ui, "bw", "B&W", crate::is_bw(d)).clicked() {
                let _ = app.run("develop.treatment", json!({}));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_button(ui, "reset", "Reset", false).on_hover_text(tr("Reset all edits (Cmd+Shift+R)")).clicked() {
                    let _ = app.run("develop.reset", json!({}));
                }
            });
        });
        ui.add_space(8.0);
        view_toggle(app, ui);
        ui.add_space(4.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut app.ui.edit_search)
                .hint_text(tr("Search a control…"))
                .desired_width(ui.available_width() - 28.0)
                .id_salt("edit-search"),
        );
        register(ui.ctx(), "field:editSearch", r.rect);
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.ui.edit_search.clear();
        }
    });
}

/// "By task | By type", drawn in both Develop headers (workspaces on).
pub fn view_toggle(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !crate::workspaces::enabled(app) {
        return;
    }
    let by_task = app.ui.edit_by_task;
    if let Some(i) = crate::widgets::segmented(ui, "editView", &[("By task", "task"), ("By type", "type")], Some(if by_task { 0 } else { 1 }), 2) {
        app.ui.edit_by_task = i == 0;
    }
}

/// The Develop panel by task.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = app.session.develop_of(id).unwrap_or_default();
    let mut d = d;
    if d.wb.mode == lightcraft_develop::WbMode::AsShot && app.session.catalog.photo(id).is_some_and(|p| p.relative_wb()) {
        let wb = &mut std::sync::Arc::make_mut(&mut d).wb;
        wb.temp = 6500.0;
        wb.tint = 0.0;
    }
    header(app, ui, &d);
    divider(ui);
    // a search: the matching controls, flat, each with its task as a caption
    let found = search(&app.ui.edit_search);
    if !app.ui.edit_search.trim().is_empty() {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 8 }).show(ui, |ui| {
            if found.is_empty() {
                ui.label(RichText::new(tr("No control has that name.")).color(t.text_dim).size(12.0));
            } else {
                ui.label(
                    RichText::new(crate::i18n::tr_format!("{n} control{} found", if found.len() == 1 { "" } else { "s" }, n = found.len()))
                        .color(t.text_dim)
                        .size(11.0),
                );
            }
        });
        let mut last = "";
        for c in found {
            let task = task_title_of(c);
            if task != last {
                edit::sub_title(ui, task);
                last = task;
            }
            control(app, ui, &d, c, true);
        }
        return;
    }
    let mut run: Option<&'static str> = None;
    for task in TASKS {
        let sid = format!("task.{}", task.id);
        let open = app.ui.section_open(&sid);
        let (resp, _) = section_header(ui, &sid, task.title, open, None);
        if resp.clicked() {
            app.ui.toggle_section(&sid);
        }
        if open {
            match task.id {
                "base" => {
                    edit::profile_row(app, ui, &d);
                    for c in task.controls {
                        control(app, ui, &d, c, true);
                    }
                }
                "light" => {
                    for c in &task.controls[..4] {
                        control(app, ui, &d, c, true);
                    }
                    let open = app.ui.flyout_open("curve");
                    if flyout_row(ui, "curve", "Curve", Icon::Curve, open).clicked() {
                        app.ui.toggle_flyout("curve");
                    }
                    if open {
                        edit::curve_editor(app, ui, id, &d);
                    }
                    control(app, ui, &d, "effects.dehaze", true);
                }
                "color" => {
                    control(app, ui, &d, "color.vibrance", true);
                    control(app, ui, &d, "color.saturation", true);
                    let open = app.ui.flyout_open("mixer");
                    if flyout_row(ui, "mixer", if crate::is_bw(&d) { "B&W Mixer" } else { "Color Mixer" }, Icon::Circle, open).clicked() {
                        app.ui.toggle_flyout("mixer");
                    }
                    if open {
                        edit::mixer(app, ui, &d);
                    }
                    let open = app.ui.flyout_open("grading");
                    if flyout_row(ui, "grading", "Color Grading", Icon::Presets, open).clicked() {
                        app.ui.toggle_flyout("grading");
                    }
                    if open {
                        edit::grading(app, ui, &d);
                    }
                    let open = app.ui.flyout_open("calibration");
                    if flyout_row(ui, "calibration", "Calibration", Icon::Sliders, open).clicked() {
                        app.ui.toggle_flyout("calibration");
                    }
                    if open {
                        for c in &task.controls[2..] {
                            control(app, ui, &d, c, true);
                        }
                    }
                }
                "detail" => {
                    for c in &task.controls[..2] {
                        control(app, ui, &d, c, true);
                    }
                    edit::sub_title(ui, "Sharpening");
                    for c in &task.controls[2..6] {
                        control(app, ui, &d, c, *c == "detail.sharpenAmount" || d.detail.sharpen_amount > 0.0);
                    }
                    edit::sub_title(ui, "Noise Reduction");
                    for c in &task.controls[6..12] {
                        control(app, ui, &d, c, true);
                    }
                    edit::sub_title(ui, "Grain");
                    for c in &task.controls[12..] {
                        control(app, ui, &d, c, *c == "grain.amount" || d.grain.amount != 0.0);
                    }
                }
                "optics" => {
                    edit::sub_title(ui, "Lens Corrections");
                    for c in &task.controls[..7] {
                        control(app, ui, &d, c, true);
                    }
                    edit::sub_title(ui, "Geometry");
                    for c in &task.controls[7..15] {
                        control(app, ui, &d, c, true);
                    }
                    edit::sub_title(ui, "Vignette");
                    for c in &task.controls[15..] {
                        control(app, ui, &d, c, *c == "vignette.amount" || d.vignette.amount != 0.0);
                    }
                }
                _ => {
                    // Local: the tools that touch a part of the photo, one button each
                    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 10 }).show(ui, |ui| {
                        ui.label(
                            RichText::new(tr("Tools that change one part of the photo. Each opens its own panel.")).color(t.text_dim).size(11.0),
                        );
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for (wid, label, cmd, tip) in [
                                ("localMasking", "Masking", "panel.masking", "Masking: subject, sky, brush, gradients (M)"),
                                ("localRemove", "Remove", "panel.remove", "Remove: heal, clone, dust spots (H)"),
                                ("localRedEye", "Red Eye", "panel.redeye", "Red Eye: fix red pupils"),
                                ("localCrop", "Crop", "panel.crop", "Crop & Rotate (C)"),
                                (
                                    "localProposals",
                                    "Proposals",
                                    "panel.proposals",
                                    "Proposals: the proposed portrait retouch, previewed before any copy is made",
                                ),
                            ] {
                                if text_button(ui, wid, label, false).on_hover_text(tr(tip)).clicked() {
                                    run = Some(cmd);
                                }
                            }
                        });
                        let n = d.masks.len();
                        if n > 0 {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(crate::i18n::tr_format!("{n} mask{} on this photo", if n == 1 { "" } else { "s" }, n = n))
                                    .color(t.text_label)
                                    .size(11.5),
                            );
                        }
                    });
                }
            }
        }
        divider(ui);
    }
    if let Some(cmd) = run {
        let _ = app.run(cmd, json!({}));
    }
}
