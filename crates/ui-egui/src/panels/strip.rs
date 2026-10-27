//! The vertical tool strip at the far right (Presets, Edit, Crop, Remove, Masking, Red Eye …).

use egui::vec2;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::icon_button;

/// One tool of the strip: its widget id (`icon:{id}`), icon, short label under it, tooltip, and the
/// command that opens it.
struct Tool {
    id: &'static str,
    icon: Icon,
    label: &'static str,
    tip: &'static str,
    command: &'static str,
    panel: Option<RightPanel>,
    needs_photo: bool,
}

const RETOUCH: &[Tool] = &[
    Tool {
        id: "presets",
        icon: Icon::Presets,
        label: "Presets",
        tip: "Presets: one-click looks (Shift+P)",
        command: "panel.presets",
        panel: None,
        needs_photo: true,
    },
    Tool {
        id: "edit",
        icon: Icon::Sliders,
        label: "Develop",
        tip: "Develop: light, colour, effects, detail, optics (E)",
        command: "panel.edit",
        panel: Some(RightPanel::Edit),
        needs_photo: true,
    },
    Tool {
        id: "crop",
        icon: Icon::Crop,
        label: "Crop",
        tip: "Crop & Rotate: crop, straighten, aspect (C)",
        command: "panel.crop",
        panel: Some(RightPanel::Crop),
        needs_photo: true,
    },
    Tool {
        id: "remove",
        icon: Icon::Eraser,
        label: "Remove",
        tip: "Remove: heal, clone, dust spots; dust from a series and portrait retouch (H)",
        command: "panel.remove",
        panel: Some(RightPanel::Remove),
        needs_photo: true,
    },
    Tool {
        id: "masking",
        icon: Icon::Mask,
        label: "Masking",
        tip: "Masking: local adjustments on the subject, the sky, a brush or a gradient (M)",
        command: "panel.masking",
        panel: Some(RightPanel::Masking),
        needs_photo: true,
    },
    Tool {
        id: "redeye",
        icon: Icon::Eye,
        label: "Red Eye",
        tip: "Red Eye: fix red pupils",
        command: "panel.redeye",
        panel: Some(RightPanel::RedEye),
        needs_photo: true,
    },
];

/// Portrait retouch by sliders (lot D): shown with workspaces on.
const PORTRAIT: Tool = Tool {
    id: "portrait",
    icon: Icon::Eye,
    label: "Portrait",
    tip: "Portrait retouch: sliders by group, Before / After, auto retouch, open eyes (Shift+T)",
    command: "panel.portrait",
    panel: Some(RightPanel::Portrait),
    needs_photo: true,
};

/// "My style" and "Auto crop" (lot B): shown with workspaces on, next to Portrait.
const STYLE: Tool = Tool {
    id: "style",
    icon: Icon::Sliders,
    label: "Style",
    tip: "My style: learn how you edit, preview it on a shoot, auto straighten and crop",
    command: "panel.style",
    panel: Some(RightPanel::Style),
    needs_photo: false,
};

const HISTORY: &[Tool] = &[
    Tool {
        id: "versions",
        icon: Icon::Versions,
        label: "Versions",
        tip: "Versions: named snapshots of this photo's edits (Shift+V)",
        command: "panel.versions",
        panel: Some(RightPanel::Versions),
        needs_photo: true,
    },
    Tool {
        id: "activity",
        icon: Icon::Activity,
        label: "History",
        tip: "History & Activity: every step, undo any of them (Y)",
        command: "panel.activity",
        panel: Some(RightPanel::Activity),
        needs_photo: false,
    },
];

const ORGANIZE: &[Tool] = &[
    Tool {
        id: "keywords",
        icon: Icon::Tag,
        label: "Keywords",
        tip: "Keywords: add, paint and suggest keywords (K)",
        command: "panel.keywords",
        panel: Some(RightPanel::Keywords),
        needs_photo: true,
    },
    Tool {
        id: "info",
        icon: Icon::Info,
        label: "Info",
        tip: "Info: camera, lens, exposure, file, metadata (I)",
        command: "panel.info",
        panel: Some(RightPanel::Info),
        needs_photo: true,
    },
];

/// Height of a tool: the icon box and the label under it.
const TOOL_H: f32 = 48.0;

fn tool(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tool: &Tool, has_photo: bool) {
    let on = match tool.panel {
        Some(p) => app.ui.right == p || (p == RightPanel::Edit && app.ui.right == RightPanel::Profiles),
        None => app.ui.presets,
    };
    let enabled = has_photo || !tool.needs_photo;
    let (r, resp) = ui.allocate_exact_size(vec2(t.strip_w, TOOL_H), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let tip = crate::i18n::tr(tool.tip);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, tip));
    crate::widgets::register(ui.ctx(), format!("icon:{}", tool.id), r);
    let p = ui.painter();
    let sq = egui::Rect::from_center_size(egui::pos2(r.center().x, r.top() + 17.0), vec2(30.0, 30.0));
    if on {
        p.rect_filled(sq, 4.0, t.tool_active);
    } else if resp.hovered() && enabled {
        p.rect_filled(sq, 4.0, t.hover.gamma_multiply(0.7));
    }
    let color = if !enabled {
        t.text_disabled
    } else if on {
        t.pick
    } else if resp.hovered() {
        t.text
    } else {
        t.icon
    };
    crate::icons::paint(p, egui::Rect::from_center_size(sq.center(), vec2(20.0, 20.0)), tool.icon, color);
    // v14: a label one can read (10 pt, label colour) under every tool
    // the label shrinks (to 8 pt at most) rather than being cut off at the strip's edge ("Développement")
    let colour = if on { t.text } else { t.text_label };
    // a short word of its own when the full name does not fit the strip (the tooltip has the full name)
    let short_key = format!("tool:{}", tool.label);
    let text = match crate::i18n::tr(&short_key) {
        s if s == short_key => crate::i18n::tr(tool.label),
        s => s,
    };
    let natural = p.layout_no_wrap(text.to_string(), t.font(10.0), colour);
    let size = if natural.size().x > t.strip_w - 4.0 { (10.0 * (t.strip_w - 4.0) / natural.size().x).max(8.0) } else { 10.0 };
    p.text(egui::pos2(r.center().x, r.bottom() - 7.0), egui::Align2::CENTER_CENTER, text, t.font(size), colour);
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run(tool.command, json!({}));
    }
}

/// A group's small title.
fn caption(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(t.strip_w, 16.0), egui::Sense::hover());
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, crate::i18n::tr(text), t.semibold(9.5), t.text_dim);
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::right("tool_strip")
        .exact_size(t.strip_w)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.chrome)
                .inner_margin(egui::Margin { left: 0, right: 0, top: 6, bottom: 6 })
                .stroke(egui::Stroke::new(1.0, t.divider)),
        )
        .show(ui, |ui| {
            let has_photo = app.session.active().is_some();
            // the groups: retouch (what changes the photo), history (what it went through),
            // organize (what describes it); behaviour and shortcuts are the same as before
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                caption(ui, &t, "Retouch");
                for x in RETOUCH {
                    tool(app, ui, &t, x, has_photo);
                }
                if crate::workspaces::enabled(app) {
                    tool(app, ui, &t, &PORTRAIT, has_photo);
                    tool(app, ui, &t, &STYLE, has_photo);
                }
                // the Retouch space's left column already shows the versions and the history: one place each
                if !crate::retouch_left::wants(app) {
                    separator(ui, &t);
                    caption(ui, &t, "History");
                    for x in HISTORY {
                        tool(app, ui, &t, x, has_photo);
                    }
                }
                separator(ui, &t);
                caption(ui, &t, "Organize");
                for x in ORGANIZE {
                    tool(app, ui, &t, x, has_photo);
                }
            });
            // About, alone at the bottom
            let bottom = ui.max_rect().bottom();
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(egui::pos2(ui.max_rect().left(), bottom - 46.0), ui.max_rect().right_bottom())),
            );
            child.vertical_centered(|ui| {
                if icon_button(ui, "more", Icon::More, vec2(t.strip_w, 40.0), false, true, "About LightCraft and the project's links").clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::About);
                }
            });
        });
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(t.strip_w - 16.0, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.button_border.gamma_multiply(0.7));
}
