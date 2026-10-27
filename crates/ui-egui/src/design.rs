//! The design system in one place (UX refonte, October 2026). One accent (the blue of
//! [`Tokens::accent`]), one scale of spacing, three radii, three kinds of button, five states.
//! Screens that were touched by the refonte take their numbers from here instead of inventing
//! their own; the older screens keep `widgets::text_button` and friends, which draw the same
//! "secondary" look.
//!
//! * **Colour**: the surface greys and text greys are [`Tokens`]; the only saturated colour of
//!   the chrome is the accent, used for the one primary action of a screen and the current step.
//!   Data colours (the four verdict buckets, flags) are not chrome and live with their panels.
//! * **State**: [`State`] gives the colour and the glyph of ok / warning / error / info / busy.
//! * **Spacing**: [`SPACE`] (4, 8, 12, 16, 24); panels use 16 side margins, 8 between rows.
//! * **Type**: [`TEXT`] sizes (caption 11, body 12.5, title 14, heading 16); semibold for what is
//!   actionable or titles, regular for the rest.
//! * **Buttons**: [`Kind::Primary`] (accent fill: at most one per screen), [`Kind::Secondary`]
//!   (bordered), [`Kind::Quiet`] (no border: the third and later actions, "More…"). All 26 pt
//!   high, 6 pt radius, and addressable as `button:{id}` like every other button.
//! * **Empty state**: [`empty_state`] says what is missing and offers the one thing to do.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::theme::Tokens;
use crate::widgets::register;

/// The spacing scale, in points.
pub const SPACE: [f32; 5] = [4.0, 8.0, 12.0, 16.0, 24.0];
/// Corner radii: small (chips, fields), medium (buttons), large (cards, dialogs).
pub const RADIUS: [u8; 3] = [4, 6, 10];
/// Type sizes: caption, body, title, heading.
pub const TEXT: [f32; 4] = [11.0, 12.5, 14.0, 16.0];
/// Height of every button and of the step buttons of the journey bar.
pub const BUTTON_H: f32 = 26.0;

/// The three kinds of button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The one action of the screen that moves the work forward.
    Primary,
    Secondary,
    /// Something to reach, not to push: borderless, dim until hovered.
    Quiet,
}

/// What a message is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ok,
    Warning,
    Error,
    Info,
    Busy,
}

impl State {
    pub fn colour(self, t: &Tokens) -> Color32 {
        match self {
            State::Ok => Color32::from_rgb(0x5f, 0xb8, 0x7a),
            State::Warning => t.caution,
            State::Error => t.reject,
            State::Info | State::Busy => t.accent,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            State::Ok => "✓",
            State::Warning => "!",
            State::Error => "✕",
            State::Info => "i",
            State::Busy => "…",
        }
    }
}

/// A button of the given [`Kind`]. `enabled == false` draws it greyed and gives no click.
pub fn button(ui: &mut Ui, id: &str, label: &str, kind: Kind, enabled: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.semibold(TEXT[1] - 0.5), t.text);
    let size = vec2((galley.size().x + 2.0 * SPACE[2]).max(40.0), BUTTON_H);
    let (r, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let hovered = enabled && resp.hovered();
    let (fill, border, text) = match (kind, enabled) {
        (Kind::Primary, true) => (if hovered { t.accent.gamma_multiply(1.25) } else { t.accent }, t.accent, Color32::WHITE),
        (Kind::Secondary, true) => (if hovered { t.hover } else { t.button }, t.button_border, t.text),
        (Kind::Quiet, true) => {
            (if hovered { t.hover } else { Color32::TRANSPARENT }, Color32::TRANSPARENT, if hovered { t.text } else { t.text_label })
        }
        (_, false) => (t.button, t.divider, t.text_disabled),
    };
    let p = ui.painter();
    p.rect(r, CornerRadius::same(RADIUS[1]), fill, Stroke::new(1.0, border), StrokeKind::Inside);
    let colour = text;
    let g = p.layout_no_wrap(label.to_string(), t.semibold(TEXT[1] - 0.5), colour);
    p.galley(r.center() - g.size() / 2.0, g, colour);
    resp
}

/// A section title inside a panel: semibold, with the room above it the scale asks for.
pub fn section_title(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(SPACE[1]);
    ui.label(egui::RichText::new(crate::i18n::tr(text)).font(t.semibold(TEXT[2])).color(t.text));
    ui.add_space(SPACE[0]);
}

/// A line of text in the colour of a [`State`], with its glyph.
pub fn status_line(ui: &mut Ui, state: State, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(state.glyph()).font(t.semibold(TEXT[1])).color(state.colour(&t)));
        ui.label(egui::RichText::new(text).size(TEXT[0] + 0.5).color(t.text_label));
    });
}

/// What an empty list or panel says: a title, one line of help, and at most one action. Returns
/// `true` when the action was pressed. Registered as `empty:{id}` (and `button:{id}` for the action).
pub fn empty_state(ui: &mut Ui, id: &str, title: &str, hint: &str, action: Option<&str>) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut pressed = false;
    let top = ui.cursor().min;
    ui.add_space(SPACE[3]);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(title).font(t.semibold(TEXT[2])).color(t.text));
        ui.add_space(SPACE[0]);
        ui.label(egui::RichText::new(hint).size(TEXT[1] - 0.5).color(t.text_dim));
        if let Some(a) = action {
            ui.add_space(SPACE[1]);
            pressed = button(ui, id, a, Kind::Primary, true).clicked();
        }
    });
    ui.add_space(SPACE[3]);
    let bottom = ui.cursor().min;
    register(ui.ctx(), format!("empty:{id}"), Rect::from_min_max(top, pos2(top.x + ui.available_width(), bottom.y)));
    pressed
}

/// A small round numeral or glyph (the step markers of the journey bar): `filled` = the current step.
pub fn badge(ui: &Ui, centre: egui::Pos2, text: &str, filled: bool, dim: bool) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    let (fill, stroke, colour) = if filled {
        (t.accent, t.accent, Color32::WHITE)
    } else if dim {
        (Color32::TRANSPARENT, t.button_border, t.text_dim)
    } else {
        (Color32::TRANSPARENT, t.text_label, t.text)
    };
    p.circle(centre, 8.5, fill, Stroke::new(1.0, stroke));
    p.text(centre, Align2::CENTER_CENTER, text, t.semibold(TEXT[0]), colour);
}
