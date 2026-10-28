//! The slow commands that run as background tasks of [`crate::tasks`], so the window keeps
//! answering: a progress panel with a Cancel button, the result kept for `ui.inspect` and for a
//! script that asks to `wait`. Today: suggested keywords (the search model reads the photos). The
//! engine splits the command exactly at the right places (`search::SuggestJob`: set up from the
//! session on the UI thread, run on a worker without it, apply on the UI thread), and the command
//! run directly (CLI, MCP, a script) still does all three in one go.
//!
//! It is asked for with `background: true` (the menus and buttons do).

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::tasks::{self, Progress};

pub const SUGGEST: &str = "Suggested Keywords";

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
    match app.last_task.get(label) {
        Some(v) if v.get("error").is_some() => Err(v["error"].as_str().unwrap_or("failed").to_string()),
        Some(v) => Ok(v.clone()),
        None => Ok(Value::Null),
    }
}

/// "Suggest" keywords for the selection in the background. Loading the model (about a minute the
/// first time) cannot be interrupted: Cancel only drops the answer when it comes.
pub fn suggest(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    if app.tasks.is_running(SUGGEST) {
        return Err(crate::i18n::tr("Keywords are already being suggested").to_string());
    }
    let job = lightcraft_engine::search::SuggestJob::new(&mut app.session, p)?;
    let ids: Vec<u64> = app.session.targets(p).unwrap_or_default().iter().map(|i| i.0).collect();
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
