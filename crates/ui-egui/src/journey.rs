//! The journey bar (UX refonte): the one thread of the whole application, drawn in the context bar.
//!
//! ```text
//!  ① Import ✓ 30   ② Sort 12/30   ③ Retouch photos 4   ④ Export          [Retouch the 11 picks →]
//! ```
//!
//! The four stages map onto what already exists, nothing is removed: *Import* and *Sort* are the
//! Library space's steps (the archive bench *Classify* is one quiet button away), *Retouch photos* is the
//! Retouch space, *Export* is the Create space. Each step says how far the work has got, and the
//! right-hand end of the bar offers **the** next action, the one primary button of the screen.
//! See `docs/ux-flow.md`.

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::design;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::register;
use crate::workspaces::{Space, Step};

/// The four stages of the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Import,
    Sort,
    Retouch,
    Export,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Import, Stage::Sort, Stage::Retouch, Stage::Export];

    /// The photographer's word for it (an English key with a French line).
    pub fn label(self) -> &'static str {
        match self {
            Stage::Import => "Import",
            Stage::Sort => "Sort",
            Stage::Retouch => "Retouch photos",
            Stage::Export => "Export",
        }
    }

    /// The widget id: `button:ctx…` keeps the ids the first workspaces used.
    pub fn id(self) -> &'static str {
        match self {
            Stage::Import => "ctxImport",
            Stage::Sort => "ctxSort",
            Stage::Retouch => "ctxRetouch",
            Stage::Export => "ctxCreate",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Stage::Import => "Bring photos in from a card or a folder",
            Stage::Sort => "Decide fast: AI picks, ratings and flags (Ctrl+1)",
            Stage::Retouch => "Develop the selected photos: light, colour, portrait (Ctrl+2)",
            Stage::Export => "Make a gallery, a slideshow or a kit from these photos (Ctrl+3)",
        }
    }

    /// The command that opens the stage.
    pub fn open(self) -> (&'static str, Value) {
        match self {
            Stage::Import => ("workspace.step", json!({"step": "import"})),
            Stage::Sort => ("workspace.step", json!({"step": "sort"})),
            Stage::Retouch => ("workspace.retouchSelection", json!({})),
            Stage::Export => ("workspace.create", json!({})),
        }
    }
}

/// The stage on screen (`None`: the archive bench, or a space of the user's own).
pub fn current(app: &LightcraftApp) -> Option<Stage> {
    if !crate::workspaces::enabled(app) {
        return None;
    }
    match (app.workspaces.kind, app.workspaces.step) {
        (Space::Library, Step::Import) => Some(Stage::Import),
        (Space::Library, Step::Sort) => Some(Stage::Sort),
        (Space::Library, Step::Classify) => None,
        (Space::Retouch, _) => Some(Stage::Retouch),
        (Space::Create, _) => Some(Stage::Export),
    }
}

/// How far the work has got, counted at most once a second.
#[derive(Clone, Copy, Default, Debug)]
pub struct Facts {
    pub total: usize,
    pub visible: usize,
    /// Photos of the view with a rating or a flag.
    pub triaged: usize,
    /// Photos of the library that carry edits.
    pub edited: usize,
    at: f64,
}

pub fn facts(app: &mut LightcraftApp, ctx: &egui::Context) -> Facts {
    let key = egui::Id::new("journey-facts");
    let now = ctx.input(|i| i.time);
    let mut f: Facts = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
    let total = app.session.catalog.len();
    if now - f.at > 1.0 || f.total != total {
        let ids: Vec<_> = app.session.visible().iter().take(20_000).copied().collect();
        let (mut triaged, mut edited) = (0, 0);
        for id in &ids {
            if let Some(p) = app.session.catalog.photo(*id) {
                if p.rating > 0 || p.flag != lightcraft_catalog::Flag::None {
                    triaged += 1;
                }
                if p.is_edited() {
                    edited += 1;
                }
            }
        }
        f = Facts { total, visible: app.session.visible().len(), triaged, edited, at: now };
        ctx.data_mut(|d| d.insert_temp(key, f));
    }
    f
}

/// The small figure after a step's name.
fn figure(stage: Stage, f: &Facts, judged: usize) -> Option<String> {
    match stage {
        Stage::Import => (f.total > 0).then(|| f.total.to_string()),
        Stage::Sort => (f.visible > 0).then(|| format!("{}/{}", f.triaged.max(judged), f.visible)),
        Stage::Retouch => (f.edited > 0).then(|| f.edited.to_string()),
        Stage::Export => None,
    }
}

fn done(stage: Stage, f: &Facts) -> bool {
    match stage {
        Stage::Import => f.total > 0,
        Stage::Sort => f.visible > 0 && f.triaged * 2 >= f.visible,
        Stage::Retouch => f.edited > 0,
        Stage::Export => false,
    }
}

/// Draw the four steps from the cursor of `ui`; returns the stage pressed, if any.
pub fn steps(app: &LightcraftApp, ui: &mut egui::Ui, f: &Facts) -> Option<Stage> {
    let t = Tokens::get(ui.ctx());
    let now = current(app);
    let mut pressed = None;
    for (i, stage) in Stage::ALL.into_iter().enumerate() {
        let is_now = now == Some(stage);
        let label = tr(stage.label());
        let fig = figure(stage, f, crate::flow::counts(app).map_or(0, |c| c.iter().sum::<u64>() as usize));
        let font = if is_now { t.semibold(design::TEXT[1]) } else { t.font(design::TEXT[1]) };
        let colour = if is_now { t.text } else { t.text_label };
        let g = ui.painter().layout_no_wrap(label.to_string(), font, colour);
        let gf = fig.as_ref().map(|s| ui.painter().layout_no_wrap(s.clone(), t.font(design::TEXT[0]), t.text_dim));
        let w = 8.0 + 17.0 + 6.0 + g.size().x + gf.as_ref().map_or(0.0, |g| 6.0 + g.size().x) + 10.0;
        let (r, resp) = ui.allocate_exact_size(egui::vec2(w, design::BUTTON_H), egui::Sense::click());
        register(ui.ctx(), format!("button:{}", stage.id()), r);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, is_now, label));
        let resp = resp.on_hover_text(format!("{} · {}", label, tr(stage.tip())));
        if resp.hovered() && !is_now {
            ui.painter().rect_filled(r, egui::CornerRadius::same(design::RADIUS[1]), t.hover);
        }
        let ok = done(stage, f) && !is_now;
        design::badge(ui, egui::pos2(r.left() + 8.0 + 8.5, r.center().y), if ok { "✓" } else { ["1", "2", "3", "4"][i] }, is_now, !ok && !is_now);
        let mut x = r.left() + 8.0 + 17.0 + 6.0;
        ui.painter().galley(egui::pos2(x, r.center().y - g.size().y / 2.0), g.clone(), colour);
        x += g.size().x;
        if let Some(gf) = gf {
            ui.painter().galley(egui::pos2(x + 6.0, r.center().y - gf.size().y / 2.0), gf, t.text_dim);
        }
        if is_now {
            // the underline of the stage you are on: the accent, once
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(r.left() + 4.0, r.bottom() - 1.5), egui::vec2(r.width() - 8.0, 2.0)),
                1.0,
                t.accent,
            );
        }
        if resp.clicked() && !is_now {
            pressed = Some(stage);
        }
        if i + 1 < Stage::ALL.len() {
            let (c, _) = ui.allocate_exact_size(egui::vec2(10.0, design::BUTTON_H), egui::Sense::hover());
            ui.painter().text(c.center(), egui::Align2::CENTER_CENTER, "›", t.font(design::TEXT[2]), t.text_disabled);
        }
    }
    pressed
}

/// What the bar proposes next.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Open a stage.
    Go(Stage),
    /// Judge the photos of the view (AI picks).
    Judge,
    /// The AI is judging: say so, with the count.
    Judging(usize, usize),
    /// Send the kept photos (AI picks or the selection) to the retouch.
    RetouchPicks(usize),
}

pub fn next(app: &mut LightcraftApp, f: &Facts, stage: Stage) -> Option<Next> {
    if f.total == 0 {
        return (stage != Stage::Import).then_some(Next::Go(Stage::Import));
    }
    match stage {
        // the Import screen carries its own button for the import that is ready to run
        Stage::Import if app.workspaces.home.scanned => None,
        Stage::Import => Some(Next::Go(Stage::Sort)),
        Stage::Sort => {
            if app.flow.cull.running {
                let st = &app.flow.cull.status;
                return Some(Next::Judging(st["done"].as_u64().unwrap_or(0) as usize, st["total"].as_u64().unwrap_or(0) as usize));
            }
            let kept = crate::flow::counts(app).map_or(0, |c| (c[0] + c[1]) as usize);
            if crate::flow::counts(app).is_none() && f.triaged == 0 {
                Some(Next::Judge)
            } else if kept > 0 {
                Some(Next::RetouchPicks(kept))
            } else {
                Some(Next::Go(Stage::Retouch))
            }
        }
        Stage::Retouch => Some(Next::Go(Stage::Export)),
        Stage::Export => None,
    }
}

pub fn next_label(n: &Next) -> String {
    match n {
        Next::Go(Stage::Sort) => tr("Sort the photos →").to_string(),
        Next::Go(s) => format!("{} →", tr(s.label())),
        Next::Judge => tr("Judge the photos").to_string(),
        Next::Judging(d, t) => tr_format!("Judging {d} / {t}", d = d, t = t),
        Next::RetouchPicks(n) => tr_format!("Retouch the {n} picks →", n = n),
    }
}

/// Do it.
pub fn run(app: &mut LightcraftApp, ctx: &egui::Context, n: &Next) {
    match n {
        Next::Go(stage) => {
            let (cmd, p) = stage.open();
            let _ = app.run(cmd, p);
        }
        Next::Judge => {
            if let Err(e) = crate::flow::start_cull(app, ctx) {
                app.toast_error(ctx, e);
            }
        }
        Next::Judging(..) => {}
        Next::RetouchPicks(_) => {
            let ids: Vec<u64> = app
                .flow
                .cull
                .list
                .as_ref()
                .and_then(|l| l["photos"].as_array())
                .map(|a| {
                    a.iter().filter(|r| matches!(r["bucket"].as_str(), Some("highlight" | "selected"))).filter_map(|r| r["id"].as_u64()).collect()
                })
                .unwrap_or_default();
            if !ids.is_empty() {
                let _ = app.run("library.select", json!({"ids": ids}));
            }
            let _ = app.run("workspace.retouchSelection", json!({}));
        }
    }
}
