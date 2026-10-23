//! The space selector (top bar) and the context bar under it: the one line that says what you are
//! working on — source, filter, selection, active photo — and that travels unchanged between the
//! spaces. It offers the way on ("Retouch selection (14) →") and the way back; changing space never
//! empties the selection or the filter, and the only way to empty them is the explicit "Clear" button.

use egui::{Align, Layout, Rect, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::design::{self, Kind};
use crate::i18n::tr;
use crate::theme::Tokens;
use crate::widgets::register;
use crate::workspaces::{self, Space, Step};

/// Height of the context bar.
pub const HEIGHT: f32 = 40.0;

/// What the context bar says.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// The library source (All Photos, an album, a folder…).
    pub source: String,
    /// One entry per active filter (`Rating ≥ 3★`, a search…); empty = no filter.
    pub filter: Vec<String>,
    pub selected: usize,
    /// Photos in the current view (source + filter).
    pub visible: usize,
    /// The photo the panels act on.
    pub active: Option<String>,
}

pub fn summary(app: &mut LightcraftApp) -> Summary {
    let s = &mut app.session;
    Summary {
        source: crate::i18n::source_title(s),
        filter: lightcraft_engine::filter_chips(&s.filter, &s.catalog).into_iter().map(|c| c.label).collect(),
        selected: s.selection.ids.len(),
        visible: s.visible().len(),
        active: s.active().and_then(|id| s.catalog.photo(id)).map(|p| p.file_name.clone()),
    }
}

impl Summary {
    pub fn to_json(&self) -> Value {
        json!({"source": self.source, "filter": self.filter, "selected": self.selected, "visible": self.visible, "active": self.active})
    }
}

/// The space selector in the top bar, starting at `left`, centred on `cy`. Returns its right edge.
/// The three spaces are buttons; a space of the user's own that is open shows as a fourth.
pub fn selector(app: &mut LightcraftApp, ui: &mut egui::Ui, left: f32, cy: f32) -> f32 {
    let t = Tokens::get(ui.ctx());
    // the three built-in spaces are the stages of the journey bar now; only a space of the user's own shows here
    if Space::ALL.iter().any(|s| s.name() == app.workspaces.current) {
        return left;
    }
    let mut items: Vec<(String, String, String, bool)> = Space::ALL
        .iter()
        .map(|s| {
            (s.name().to_string(), format!("space-{}", s.name().to_lowercase()), format!("{} ({})", crate::i18n::tr(s.tip()), s.shortcut()), true)
        })
        .collect();
    if !Space::ALL.iter().any(|s| s.name() == app.workspaces.current) {
        items.push((
            app.workspaces.current.clone(),
            "space-custom".into(),
            crate::i18n::tr("A workspace of your own (Window ▸ Workspaces)").to_string(),
            false,
        ));
    }
    let mut x = left;
    let mut clicked: Option<String> = None;
    for (name, id, tip, builtin) in items {
        let label = if builtin { crate::i18n::tr(if name == "Retouch" { "Develop" } else { &name }).to_string() } else { name.clone() };
        let galley = ui.painter().layout_no_wrap(label, t.semibold(12.5), t.text);
        let w = (galley.size().x + 26.0).max(70.0);
        let r = Rect::from_min_size(pos2(x, cy - 13.0), vec2(w, 26.0));
        let resp = ui.interact(r, egui::Id::new(("space", &id)), Sense::click()).on_hover_text(tip);
        register(ui.ctx(), format!("button:{id}"), r);
        let active = name == app.workspaces.current;
        let fill = if active {
            t.accent
        } else if resp.hovered() {
            t.hover
        } else {
            t.button
        };
        let colour = if active { egui::Color32::WHITE } else { t.text_label };
        ui.painter().rect(r, 5.0, fill, Stroke::new(1.0, if active { t.accent } else { t.button_border }), StrokeKind::Inside);
        ui.painter().galley(r.center() - galley.size() / 2.0, galley.clone(), colour);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, name.as_str()));
        if resp.clicked() {
            clicked = Some(name);
        }
        x += w + 4.0;
    }
    if let Some(name) = clicked {
        let _ = app.run("workspace.switch", json!({"name": name}));
    }
    x - 4.0
}

/// The width the selector needs (for the top bar to leave it room).
pub fn selector_width(app: &LightcraftApp, ui: &egui::Ui) -> f32 {
    let t = Tokens::get(ui.ctx());
    if Space::ALL.iter().any(|s| s.name() == app.workspaces.current) {
        return 0.0;
    }
    let mut names: Vec<String> = Space::ALL.iter().map(|s| crate::i18n::tr(s.name()).to_string()).collect();
    if !Space::ALL.iter().any(|s| s.name() == app.workspaces.current) {
        names.push(app.workspaces.current.clone());
    }
    names.iter().map(|n| (ui.painter().layout_no_wrap(n.clone(), t.semibold(12.5), t.text).size().x + 26.0).max(70.0) + 4.0).sum::<f32>() - 4.0
}

/// A filled button for the step that comes next.
pub(crate) fn primary_button(ui: &mut egui::Ui, id: &str, label: &str, enabled: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.semibold(12.0), egui::Color32::WHITE);
    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 24.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let fill = match (enabled, resp.hovered()) {
        (false, _) => t.button,
        (true, true) => t.accent.gamma_multiply(1.25),
        (true, false) => t.accent,
    };
    ui.painter().rect_filled(r, 5.0, fill);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, if enabled { egui::Color32::WHITE } else { t.text_disabled });
    resp
}

/// The context bar: the journey (four steps and the next action) and, dimmer, what the work is
/// about (source, filter, selection, active photo). Only with workspaces on.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !workspaces::enabled(app) || app.ui.fullscreen {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let sum = summary(app);
    app.workspaces.context = Some(sum.clone());
    let kind = app.workspaces.kind;
    let selector_in_top = app.workspaces.selector_in_top_bar;
    let facts = crate::journey::facts(app, ui.ctx());
    let stage = crate::journey::current(app);
    let next = stage.and_then(|s| crate::journey::next(app, &facts, s));
    let mut run: Option<(&'static str, Value)> = None;
    let mut pressed_stage = None;
    let mut do_next = false;
    egui::Panel::top("context_bar")
        .exact_size(HEIGHT)
        .frame(egui::Frame::NONE.fill(t.inset).stroke(Stroke::new(1.0, t.divider)).inner_margin(egui::Margin {
            left: if app.integrated_titlebar { 12 } else { 10 },
            right: 12,
            top: 0,
            bottom: 0,
        }))
        .show(ui, |ui| {
            let full = ui.max_rect();
            register(ui.ctx(), "region:contextBar", full);
            // from the right: the next action (the one primary button of the screen), then the quiet ones
            let mut right = ui.new_child(UiBuilder::new().max_rect(full).layout(Layout::right_to_left(Align::Center)));
            right.spacing_mut().item_spacing.x = design::SPACE[1];
            if let Some(n) = &next {
                let enabled = !matches!(n, crate::journey::Next::Judging(..));
                if design::button(&mut right, "journeyNext", &crate::journey::next_label(n), Kind::Primary, enabled).clicked() {
                    do_next = true;
                }
            }
            if (!sum.filter.is_empty() || sum.selected > 0)
                && design::button(&mut right, "ctxClear", tr("Clear"), Kind::Quiet, true)
                    .on_hover_text(crate::i18n::tr("Clear the filter and the selection (the active photo stays selected)"))
                    .clicked()
            {
                run = Some(("workspace.clearContext", json!({})));
            }
            if kind == Space::Library
                && app.workspaces.step != Step::Classify
                && design::button(&mut right, "ctxClassify", tr("Archive"), Kind::Quiet, true)
                    .on_hover_text(crate::i18n::tr("The archive bench: health counters, “same person?” questions, groups, places, keywords"))
                    .clicked()
            {
                run = Some(("workspace.step", json!({"step": "classify"})));
            }
            if kind == Space::Library
                && app.workspaces.step == Step::Classify
                && design::button(&mut right, "ctxSort", tr("Back to sorting"), Kind::Quiet, true).clicked()
            {
                run = Some(("workspace.step", json!({"step": "sort"})));
            }
            if kind == Space::Retouch {
                // v14: Before / After in one gesture (the same as holding `\`)
                let showing = app.ui.before_after == crate::state::BeforeAfter::Original;
                if design::button(
                    &mut right,
                    "ctxBeforeAfter",
                    if showing { tr("Showing: before") } else { tr("Before/After") },
                    Kind::Secondary,
                    true,
                )
                .on_hover_text(crate::i18n::tr(
                    "Show the photo before any edit; press again for the edited photo (hold \\ for a glance, Y for side by side)",
                ))
                .clicked()
                {
                    run = Some(("view.showOriginal", json!({})));
                }
            }
            // from the left: a custom space's selector (when the top bar has no room), the steps, then the context
            let left_rect = Rect::from_min_max(full.min, pos2((right.min_rect().left() - 12.0).max(full.left() + 60.0), full.max.y));
            let mut left = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
            left.spacing_mut().item_spacing.x = 0.0;
            if !selector_in_top {
                let x = left.cursor().left();
                let end = selector(app, &mut left, x, full.center().y);
                left.add_space((end - x + 8.0).max(0.0));
            }
            pressed_stage = crate::journey::steps(app, &mut left, &facts);
            left.add_space(design::SPACE[3]);
            // source · filter · selection · active photo, on one line that gives way at the right
            let dim = t.text_dim;
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let part = |job: &mut egui::text::LayoutJob, label: String, value: String, strong: bool| {
                let font = egui::FontId::proportional(design::TEXT[0] + 0.5);
                job.append(&label, 0.0, egui::TextFormat { font_id: font.clone(), color: dim, ..Default::default() });
                job.append(&value, 0.0, egui::TextFormat { font_id: font, color: if strong { t.text } else { t.text_label }, ..Default::default() });
            };
            part(&mut job, String::new(), sum.source.clone(), false);
            if !sum.filter.is_empty() {
                part(&mut job, format!("  ·  {}", crate::i18n::tr("Filter ")), sum.filter.join(" + "), true);
            }
            if sum.selected > 0 {
                part(&mut job, "  ·  ".to_string(), crate::i18n::tr_format!("{sel} selected of {all}", sel = sum.selected, all = sum.visible), true);
            }
            if let Some(a) = &sum.active {
                part(&mut job, format!("  ·  {}", crate::i18n::tr("Photo ")), a.clone(), false);
            }
            let r = left.add(egui::Label::new(job).truncate().selectable(false));
            register(ui.ctx(), "context:summary", r.rect);
        });
    if let Some(stage) = pressed_stage {
        let (cmd, p) = stage.open();
        let _ = app.run(cmd, p);
    }
    if do_next && let Some(n) = &next {
        crate::journey::run(app, ui.ctx(), n);
    }
    if let Some((id, params)) = run {
        let _ = app.run(id, params);
    }
}
