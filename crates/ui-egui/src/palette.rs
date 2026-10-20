//! The command palette (LightCraft v14, ⌘K): type a few letters, see the commands whose name, menu or
//! shortcut matches, press Enter to run the highlighted one. It lists exactly the menu model
//! ([`crate::menus::menu_entries`]): every menu entry, with its shortcut and whether it is enabled,
//! so nothing is reachable here that isn't in a menu. The palette is a small floating window over the
//! canvas; Esc or a click outside closes it. Everything else is one UI command, `app.commandPalette`.

use egui::{Align2, Key, RichText, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::theme::Tokens;
use crate::widgets::register;

/// What the palette remembers between frames.
#[derive(Clone, Default)]
pub struct State {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// Focus the field on the next frame (just opened).
    focus: bool,
}

const SHOWN: usize = 12;

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: String,
    pub label: String,
    pub menu: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
}

/// Accent- and case-insensitive text for matching.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The commands matching `query`, best first: a label that starts with the words, then a label that
/// contains them, then a menu path or shortcut that does. Every word must match somewhere.
pub fn hits(app: &LightcraftApp, query: &str) -> Vec<Hit> {
    let words: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
    let mut scored: Vec<(i32, Hit)> = crate::menus::menu_entries(app)
        .into_iter()
        .filter(|e| !e.menu.is_empty() && !e.label.is_empty())
        .filter_map(|e| {
            let label = tr(&e.label).to_string();
            let menu = e.menu.iter().map(|m| tr(m)).collect::<Vec<_>>().join(" ▸ ");
            let hay = (fold(&label), fold(&menu), e.shortcut.as_deref().map(fold).unwrap_or_default());
            let mut score = 0;
            for w in &words {
                score += if hay.0.starts_with(w.as_str()) || hay.0.split_whitespace().any(|part| part.starts_with(w.as_str())) {
                    30
                } else if hay.0.contains(w.as_str()) {
                    20
                } else if hay.1.contains(w.as_str()) {
                    10
                } else if hay.2.contains(w.as_str()) {
                    8
                } else {
                    return None;
                };
            }
            if !e.enabled {
                score -= 5;
            }
            Some((score, Hit { id: e.id, label, menu, shortcut: e.shortcut, enabled: e.enabled }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.dedup_by(|a, b| a.1.id == b.1.id);
    scored.into_iter().map(|(_, h)| h).collect()
}

/// `app.commandPalette`: open (or close) the palette.
pub fn toggle(app: &mut LightcraftApp) -> bool {
    let st = &mut app.palette;
    st.open = !st.open;
    st.focus = st.open;
    st.query.clear();
    st.selected = 0;
    st.open
}

/// Draw the palette when it is open; run the chosen command.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let mut st = std::mem::take(&mut app.palette);
    if !st.open {
        return;
    }
    let t = Tokens::get(ctx);
    let list = hits(app, &st.query);
    let shown = list.len().min(SHOWN);
    st.selected = st.selected.min(shown.saturating_sub(1));
    let mut run: Option<String> = None;
    let mut close = false;
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.5).clamp(360.0, 620.0);
    let resp = egui::Window::new("command-palette")
        .id(egui::Id::new("command-palette-window"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_size(vec2(width, 0.0))
        .anchor(Align2::CENTER_TOP, vec2(0.0, screen.height() * 0.14))
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.chrome).corner_radius(8.0).inner_margin(12.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .hint_text(tr("Type a command, a menu or a shortcut…"))
                    .desired_width(f32::INFINITY)
                    .font(t.font(15.0))
                    .id_salt("command-palette-field"),
            );
            register(ctx, "field:commandPalette", field.rect);
            if st.focus {
                field.request_focus();
                st.focus = false;
            }
            if field.changed() {
                st.selected = 0;
            }
            let (enter, esc, down, up) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, Key::Enter),
                    i.consume_key(egui::Modifiers::NONE, Key::Escape),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                )
            });
            if esc {
                close = true;
            }
            if down && shown > 0 {
                st.selected = (st.selected + 1) % shown;
            }
            if up && shown > 0 {
                st.selected = (st.selected + shown - 1) % shown;
            }
            if shown == 0 {
                ui.label(
                    RichText::new(tr(if st.query.trim().is_empty() { "Every menu command is here: type to filter." } else { "No command matches." }))
                        .color(t.text_dim)
                        .size(12.0),
                );
            }
            for (n, hit) in list.iter().take(shown).enumerate() {
                let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), egui::Sense::click());
                register(ctx, format!("palette:{}", hit.id), r);
                let selected = n == st.selected;
                if selected {
                    ui.painter().rect_filled(r, 4.0, t.accent.gamma_multiply(0.85));
                } else if row.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.hover);
                }
                let fg = if !hit.enabled {
                    t.text_disabled
                } else if selected {
                    egui::Color32::WHITE
                } else {
                    t.text
                };
                let dim = if selected { egui::Color32::from_white_alpha(170) } else { t.text_dim };
                ui.painter().text(egui::pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, &hit.label, t.semibold(13.0), fg);
                let label_w = ui.painter().layout_no_wrap(hit.label.clone(), t.semibold(13.0), fg).size().x;
                ui.painter().with_clip_rect(r.with_max_x(r.right() - 90.0)).text(
                    egui::pos2(r.left() + 18.0 + label_w, r.center().y),
                    Align2::LEFT_CENTER,
                    &hit.menu,
                    t.font(11.0),
                    dim,
                );
                if let Some(sc) = &hit.shortcut {
                    ui.painter().text(egui::pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, sc, t.font(11.0), dim);
                }
                if row.clicked() {
                    run = Some(hit.id.clone());
                }
            }
            if enter && let Some(hit) = list.get(st.selected) {
                run = Some(hit.id.clone());
            }
            ui.label(RichText::new(tr("↑ ↓ choose · Enter runs · Esc closes")).color(t.text_dim).size(10.5));
        });
    // a click outside closes it
    if let Some(resp) = resp
        && ctx.input(|i| i.pointer.any_click())
        && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| resp.response.rect.contains(p)))
    {
        close = true;
    }
    if let Some(id) = run {
        close = true;
        if let Some(hit) = list.iter().find(|h| h.id == id) {
            if hit.enabled {
                match app.run(&id, json!({})) {
                    Ok(_) => {}
                    Err(e) => app.toast_error(ctx, e),
                }
            } else {
                app.toast(ctx, crate::i18n::tr_format!("“{name}” is not available right now", name = hit.label.clone()));
            }
        }
    }
    if close {
        st.open = false;
        st.query.clear();
        st.selected = 0;
    }
    app.palette = st;
}
