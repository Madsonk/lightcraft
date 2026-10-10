//! The slow commands (web gallery, dust from a series, suggested keywords) as background tasks of
//! [`crate::tasks`], so the window keeps answering while they work: a progress panel with a Cancel
//! button, the result as a toast. Each one is "set up from the session on the UI thread, run on a
//! worker without it, apply on the UI thread" — the engine splits the command exactly there
//! (`cmd::gallery::plan`, `cmd::dust::plan_series`, `search::SuggestJob`), and the command run
//! directly (CLI, MCP, a script) still does all three in one go.
//!
//! They are asked for with `background: true` (the menus and buttons do), or, for the gallery, by
//! the menu item that has no folder yet. Without it the commands behave as before.

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::tasks::{self, Progress};

pub const GALLERY: &str = "Web Gallery";
pub const DUST: &str = "Dust From Series";
pub const SUGGEST: &str = "Suggested Keywords";
/// The second half of the dust task (the heal spots going on the photos), shown like a task.
pub const HEAL: &str = "Healing Dust";

/// The heal spots of a dust search going on the photos, a slice per frame ([`pump`]).
pub struct Healing {
    applier: lightcraft_engine::cmd::dust::Applier,
    stopping: bool,
}

impl Healing {
    pub fn new(applier: lightcraft_engine::cmd::dust::Applier) -> Healing {
        Healing { applier, stopping: false }
    }

    /// What Cancel does: the next frame drops the spots without writing them.
    pub fn cancel(&mut self) {
        self.stopping = true;
    }
}

/// Per frame: put the heal spots on the next few photos (about 15 ms of work), show the panel,
/// and when every photo is done commit them as one undo step. Cancel drops them: nothing is written.
pub fn pump(app: &mut LightcraftApp, ctx: &egui::Context) {
    pump_with(app, ctx, true)
}

/// [`pump`], drawing the panel or not (a script waiting for the end has no frame to draw in).
fn pump_with(app: &mut LightcraftApp, ctx: &egui::Context, draw: bool) {
    let Some(mut h) = app.healing.take() else { return };
    if h.stopping {
        remember(app, DUST, &Ok(json!({"cancelled": true})));
        return app.toast(ctx, crate::i18n::tr("Dust healing stopped: nothing was changed"));
    }
    if !h.applier.step(&mut app.session, std::time::Duration::from_millis(15)) {
        if draw {
            h.stopping = tasks::progress_window(ctx, HEAL, 0, HEAL, h.applier.done(), h.applier.total(), "", false);
            ctx.request_repaint();
        }
        app.healing = Some(h);
        return;
    }
    let r = h.applier.finish(&mut app.session).map_err(|e| e.to_string());
    let msg = match &r {
        Ok(v) => {
            let (spots, photos, added) = (v["spots"].as_u64().unwrap_or(0), v["photos"].as_u64().unwrap_or(0), v["added"].as_u64().unwrap_or(0));
            if added == 0 {
                format!("{spots} dust spot(s) found; every place was already healed")
            } else {
                format!("Healed {spots} dust spot(s) on {photos} photos ({added} heal spots, one undo step)")
            }
        }
        Err(e) => e.clone(),
    };
    remember(app, DUST, &r);
    app.toast(ctx, msg);
}

/// `ui.inspect` → `taskProgress` for the healing stage (the tasks report their own).
pub fn healing_json(app: &LightcraftApp) -> Option<Value> {
    app.healing
        .as_ref()
        .map(|h| json!({"label": HEAL, "done": h.applier.done(), "total": h.applier.total(), "current": "", "cancelling": h.stopping}))
}

/// `true` for `{background: true}`.
pub fn wanted(p: &Value) -> bool {
    p.get("background").and_then(Value::as_bool).unwrap_or(false)
}

/// What each finished task ended with, by label (`ui.inspect` → `lastTask`, and what `wait` returns).
fn remember(app: &mut LightcraftApp, label: &str, r: &Result<Value, String>) {
    app.last_task.insert(label.to_string(), r.clone().unwrap_or_else(|e| json!({"error": e})));
}

/// With `wait: true` (scripts and tests): run the task to its end and answer with its result.
fn maybe_wait(app: &mut LightcraftApp, p: &Value, label: &str, started: Value) -> Result<Value, String> {
    if !p.get("wait").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(started);
    }
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    if !tasks::wait(app, &ctx, std::time::Duration::from_secs(1800)) {
        return Err(format!("{label}: still running"));
    }
    while app.healing.is_some() {
        pump_with(app, &ctx, false);
    }
    match app.last_task.get(label) {
        Some(v) if v.get("error").is_some() => Err(v["error"].as_str().unwrap_or("failed").to_string()),
        Some(v) => Ok(v.clone()),
        None => Ok(Value::Null),
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 { one.into() } else { many.into() }
}

/// `gallery.export` in the background: the photos are rendered at every width on a worker, one at a
/// time; Cancel stops after the photo in hand and writes no page.
pub fn gallery(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    if app.tasks.is_running(GALLERY) {
        return Err(crate::i18n::tr("A web gallery is already being made").to_string());
    }
    let plan = lightcraft_engine::cmd::gallery::plan(&mut app.session, p).map_err(|e| e.to_string())?;
    let (n, dir) = (plan.photos(), plan.dir().to_string());
    let work = move |prog: &Progress| plan.run(&mut |done, name| prog.report(done, n, name)).map_err(|e| e.to_string());
    let done = move |app: &mut LightcraftApp, ctx: &egui::Context, r: Result<Value, String>| {
        let msg = match &r {
            Ok(v) if v["cancelled"] == true => format!(
                "Web gallery stopped: {} of {} photos made, no page written (the images stay in {dir})",
                v["photos"].as_u64().unwrap_or(0),
                v["total"].as_u64().unwrap_or(0),
            ),
            Ok(v) => {
                let n = v["photos"].as_u64().unwrap_or(0);
                format!("Web gallery: {n} photo{} in {dir}", plural(n, "", "s"))
            }
            Err(e) => e.clone(),
        };
        remember(app, GALLERY, &r);
        app.toast(ctx, msg);
    };
    tasks::spawn_with_progress(app, GALLERY, work, done)?;
    maybe_wait(app, p, GALLERY, json!({"background": true, "photos": n}))
}

/// `dust.apply` for a series in the background: the sample is rendered and searched on a worker
/// (Cancel stops it, nothing is changed), then the heal spots go on every photo in one undo step.
pub fn dust(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    if app.tasks.is_running(DUST) || app.healing.is_some() {
        return Err(crate::i18n::tr("Dust is already being searched for").to_string());
    }
    let plan = lightcraft_engine::cmd::dust::plan_series(&mut app.session, p).map_err(|e| e.to_string())?;
    let steps = plan.steps();
    let work = move |prog: &Progress| plan.run(&mut |done, total| prog.report(done, total, "")).map_err(|e| e.to_string());
    let done = |app: &mut LightcraftApp, ctx: &egui::Context, r: Result<lightcraft_engine::cmd::dust::DustFound, String>| {
        let (result, msg) = match r {
            Err(e) => (Err(e.clone()), e),
            Ok(f) if f.cancelled => (Ok(json!({"cancelled": true})), crate::i18n::tr("Dust search stopped: nothing was changed").to_string()),
            Ok(f) if f.spots.is_empty() => (Ok(json!({"spots": 0, "photos": 0, "added": 0})), format!("No dust found in {} photos", f.ids.len())),
            Ok(f) => {
                // the heal spots go on a slice of photos per frame (see `pump`), then one undo step
                app.healing = Some(Healing::new(lightcraft_engine::cmd::dust::Applier::from_found(&app.session, &f)));
                return;
            }
        };
        remember(app, DUST, &result);
        app.toast(ctx, msg);
    };
    tasks::spawn_with_progress(app, DUST, work, done)?;
    maybe_wait(app, p, DUST, json!({"background": true, "steps": steps}))
}

/// "Suggest" keywords for the selection in the background. Loading the model (about a minute the
/// first time) cannot be interrupted: Cancel only drops the answer when it comes.
pub fn suggest(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    if app.tasks.is_running(SUGGEST) {
        return Err(crate::i18n::tr("Keywords are already being suggested").to_string());
    }
    let job = lightcraft_engine::search::SuggestJob::new(&mut app.session, p)?;
    let ids: Vec<u64> = app.session.targets(p).iter().map(|i| i.0).collect();
    let n = ids.len();
    let note = crate::i18n::tr("Loading the search model, then looking at the photos").to_string();
    let work = move |prog: &Progress| {
        prog.report(0, 0, &note);
        let r = job.run();
        (r, prog.cancelled())
    };
    let done =
        move |app: &mut LightcraftApp, ctx: &egui::Context, (r, cancelled): (Result<lightcraft_engine::search::SuggestFound, String>, bool)| {
            if cancelled {
                remember(app, SUGGEST, &Ok(json!({"cancelled": true})));
                return app.toast(ctx, crate::i18n::tr("Suggested keywords: stopped, nothing was added"));
            }
            let r = r.and_then(|f| lightcraft_engine::search::SuggestJob::finish(&mut app.session, f));
            remember(app, SUGGEST, &r);
            match &r {
                Ok(v) => app.sorting.set_keywords(&ids, v),
                Err(e) => app.sorting.set_keyword_error(&ids, e),
            }
            // (the answer is in the Sorting Help window; a toast only when it is not open)
            if !matches!(app.ui.dialog, Some(crate::state::Dialog::SortingHelp { .. })) {
                let msg = match &r {
                    Ok(_) => crate::i18n::tr("Keyword suggestions are ready: open Photo › Sorting Help").to_string(),
                    Err(e) => e.clone(),
                };
                app.toast(ctx, msg);
            }
        };
    tasks::spawn_with_progress(app, SUGGEST, work, done)?;
    maybe_wait(app, p, SUGGEST, json!({"background": true, "photos": n}))
}
