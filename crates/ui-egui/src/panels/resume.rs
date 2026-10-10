//! The "Resume" bar: LightCraft keeps the last view when it opens (Settings ▸ Open in ▸ Last view),
//! and says so — where it is, which photo it was on — with the two things one usually does next:
//! carry on, or import. A discreet bar over the top of the photo area (no layout change), gone at
//! the first thing the user does (a photo chosen, an edit, a dialog) or after half a minute.
//!
//! Only the desktop app arms it at launch ([`State::arm`]); everything else (the headless harness,
//! the browser build, tests) never shows it unless asked (`ui.set` has no switch for it: the
//! control channel arms it with the `view.resumeBar` command).

use egui::{Align2, RichText};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::ViewMode;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// How long the bar stays when nothing is done.
const SECONDS: f64 = 30.0;

#[derive(Default)]
pub struct State {
    armed: bool,
    dismissed: bool,
    /// When it first showed, and what the app looked like then (anything that changes ends it).
    shown: Option<Shown>,
}

struct Shown {
    at: f64,
    undo: usize,
    active: Option<u64>,
    selection: usize,
    source: String,
    photos: usize,
    file: Option<String>,
    view: ViewMode,
}

impl State {
    /// Show the bar when the first frame has a library with photos in it.
    pub fn arm(&mut self) {
        self.armed = true;
        self.dismissed = false;
        self.shown = None;
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    /// `ui.inspect` → `resume`.
    pub fn to_json(&self) -> Value {
        match (&self.shown, self.armed, self.dismissed) {
            (Some(s), true, false) => {
                json!({"visible": true, "source": s.source, "photos": s.photos, "photo": s.file, "view": format!("{:?}", s.view)})
            }
            _ => json!({"visible": false, "armed": self.armed, "dismissed": self.dismissed}),
        }
    }
}

/// Per frame, after the panels.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.resume.armed || app.resume.dismissed || !app.ui.settings.resume_bar {
        return;
    }
    let now = ctx.input(|i| i.time);
    let active = app.session.active().map(|p| p.0);
    let (undo, selection) = (app.session.undo.len(), app.session.selection.ids.len());
    let Some(s) = &app.resume.shown else {
        // the first frame: what is there to resume?
        let photos = app.session.catalog.len();
        if photos == 0 {
            app.resume.dismissed = true;
            return;
        }
        let file = app.session.active().and_then(|id| app.session.catalog.photo(id)).map(|p| p.file_name.clone());
        app.resume.shown =
            Some(Shown { at: now, undo, active, selection, source: crate::i18n::source_title(&app.session), photos, file, view: app.ui.view });
        return;
    };
    // the user got going: the bar has done its job
    if now - s.at > SECONDS || undo != s.undo || active != s.active || selection != s.selection || app.ui.view != s.view {
        app.resume.dismissed = true;
        return;
    }
    if app.ui.dialog.is_some() || app.ui.fullscreen {
        return;
    }
    let (source, photos, file) = (s.source.clone(), s.photos, s.file.clone());
    let t = Tokens::get(ctx);
    let mut action: Option<&str> = None;
    egui::Area::new(egui::Id::new("resume-bar")).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, t.top_bar_h + 12.0]).show(
        ctx,
        |ui| {
            let r = egui::Frame::new()
                .fill(t.chrome)
                .stroke(egui::Stroke::new(1.0, t.button_border))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let what = match &file {
                            Some(f) => format!("Welcome back · {source} · {photos} photos · you were on {f}"),
                            None => format!("Welcome back · {source} · {photos} photos"),
                        };
                        ui.label(RichText::new(what).size(12.5).color(t.text_label));
                        let label = if file.is_some() { "Resume editing" } else { "Continue" };
                        if text_button(ui, "resumeContinue", label, false).clicked() {
                            action = Some("continue");
                        }
                        if text_button(ui, "resumeImport", "Import Photos…", false).clicked() {
                            action = Some("import");
                        }
                        if text_button(ui, "resumeDismiss", "Hide", false).clicked() {
                            action = Some("dismiss");
                        }
                    });
                });
            register(ui.ctx(), "resume:bar", r.response.rect);
        },
    );
    match action {
        Some("continue") => {
            // back to the photo it was on, in the loupe, unless it already is
            if app.session.active().is_some() && app.ui.view != ViewMode::Detail {
                let _ = app.run("view.detail", json!({}));
            }
            app.resume.dismissed = true;
        }
        Some("import") => {
            app.resume.dismissed = true;
            let _ = app.run("file.addPhotos", json!({}));
        }
        Some(_) => app.resume.dismissed = true,
        None => ctx.request_repaint_after(std::time::Duration::from_secs(1)),
    }
}
