//! Background tasks for commands whose file-system work can take long on a slow or offline drive
//! (Find Missing Photos walking a folder, listing the auto-import folder…): the work runs on a
//! worker thread and its result is applied on the UI thread between frames, so the window keeps
//! answering. Imports, scans, exports and preview builds have their own tasks with progress. A task the user started
//! (Find Missing Photos) shows in the activity stack while it runs; quiet ones (the auto-import listing) don't.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::LightcraftApp;

/// Applies a task's result to the app (on the UI thread).
type Finish = Box<dyn FnOnce(&mut LightcraftApp, &egui::Context) + Send>;

struct Task {
    label: String,
    rx: Receiver<Finish>,
    /// Its row in the activity stack (none for quiet tasks); goes with the task.
    _guard: Option<lightcraft_engine::activity::TaskGuard>,
    /// Set for tasks that count their steps and can be stopped ([`spawn_with_progress`]).
    progress: Option<Progress>,
}

/// How far a long task has got, shared with its worker: the worker reports and asks whether it
/// may go on; the window shows the bar and a Cancel button that sets the flag. (Tasks that cannot
/// stop in the middle, like loading a model, report no steps and only stop being waited for.)
#[derive(Clone, Default)]
pub struct Progress {
    inner: Arc<ProgressInner>,
}

#[derive(Default)]
struct ProgressInner {
    /// 0: the number of steps is not known (a busy indicator, no bar).
    total: AtomicUsize,
    done: AtomicUsize,
    cancel: AtomicBool,
    note: Mutex<String>,
}

impl Progress {
    /// `done` of `total` steps are finished; `note` says what is being worked on. Returns `false`
    /// once the user has asked to stop.
    pub fn report(&self, done: usize, total: usize, note: &str) -> bool {
        self.inner.total.store(total, Ordering::Relaxed);
        self.inner.done.store(done, Ordering::Relaxed);
        if let Ok(mut n) = self.inner.note.lock() {
            note.clone_into(&mut n);
        }
        !self.cancelled()
    }

    pub fn cancelled(&self) -> bool {
        self.inner.cancel.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (usize, usize, String) {
        (
            self.inner.done.load(Ordering::Relaxed),
            self.inner.total.load(Ordering::Relaxed),
            self.inner.note.lock().map(|n| n.clone()).unwrap_or_default(),
        )
    }
}

/// The background tasks in flight.
#[derive(Default)]
pub struct Tasks {
    running: Vec<Task>,
    /// Repaints the window when a task finishes (set every frame).
    pub(crate) repaint: Option<egui::Context>,
}

impl Tasks {
    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// What is running (`ui.inspect` → `tasks`).
    pub fn labels(&self) -> Vec<String> {
        self.running.iter().map(|t| t.label.clone()).collect()
    }

    pub fn is_running(&self, label: &str) -> bool {
        self.running.iter().any(|t| t.label == label)
    }

    /// The tasks that report steps, for `ui.inspect` → `taskProgress`.
    pub fn progress_json(&self) -> Value {
        Value::Array(
            self.running
                .iter()
                .filter_map(|t| {
                    let p = t.progress.as_ref()?;
                    let (done, total, note) = p.snapshot();
                    Some(json!({"label": t.label, "done": done, "total": total, "current": note, "cancelling": p.cancelled()}))
                })
                .collect(),
        )
    }

    /// Ask the task called `label` to stop (`false`: no such task, or it cannot report).
    pub fn cancel(&self, label: &str) -> bool {
        self.running.iter().filter(|t| t.label == label).filter_map(|t| t.progress.as_ref()).map(Progress::cancel).count() > 0
    }
}

/// Run `work` on a worker thread, then `done(app, ctx, result)` on the UI thread (in the browser
/// build, which has no threads, `work` runs at once and `done` on the next frame). With `kind`, the
/// activity stack shows `label` meanwhile, without a count and without ✕ (the work can't be stopped).
pub fn spawn<T: Send + 'static>(
    app: &mut LightcraftApp,
    label: &str,
    kind: Option<&'static str>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut LightcraftApp, &egui::Context, T) + Send + 'static,
) -> Result<(), String> {
    let guard = kind.map(|k| app.session.activity.start(k, label, lightcraft_engine::activity::Cancel::No));
    start(app, label, guard, work, done)
}

/// [`spawn`] for a task with progress that can stop: its row (`kind`, `label`) adopts `cancel`,
/// the flag `work` checks, so the row's ✕ and `activity.cancel` stop it; `work` reports progress
/// through the handle it is given.
pub fn spawn_tracked<T: Send + 'static>(
    app: &mut LightcraftApp,
    label: &str,
    kind: &'static str,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    work: impl FnOnce(lightcraft_engine::activity::TaskHandle) -> T + Send + 'static,
    done: impl FnOnce(&mut LightcraftApp, &egui::Context, T) + Send + 'static,
) -> Result<(), String> {
    let guard = app.session.activity.start(kind, label, lightcraft_engine::activity::Cancel::Flag(cancel));
    let handle = guard.handle();
    start(app, label, Some(guard), move || work(handle), done)
}

/// Run `work` on a worker thread and list it in `app.tasks` with its activity row (if any).
fn start<T: Send + 'static>(
    app: &mut LightcraftApp,
    label: &str,
    guard: Option<lightcraft_engine::activity::TaskGuard>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut LightcraftApp, &egui::Context, T) + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = channel::<Finish>();
    let repaint = app.tasks.repaint.clone();
    let job = move || {
        let t = work();
        let _ = tx.send(Box::new(move |app: &mut LightcraftApp, ctx: &egui::Context| done(app, ctx, t)));
        if let Some(c) = repaint {
            c.request_repaint();
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name(format!("lc-task-{label}")).spawn(job).map_err(|e| format!("{label}: could not start: {e}"))?;
    #[cfg(target_arch = "wasm32")]
    job();
    app.tasks.running.push(Task { label: label.to_string(), rx, _guard: guard, progress: None });
    Ok(())
}

/// [`spawn`] for a long job that reports its steps and can be stopped: `work` gets the
/// [`Progress`] it reports to (its `report` returns `false` after Cancel), and the window shows a
/// progress panel with a Cancel button until it ends (see [`show`]).
pub fn spawn_with_progress<T: Send + 'static>(
    app: &mut LightcraftApp,
    label: &str,
    work: impl FnOnce(&Progress) -> T + Send + 'static,
    done: impl FnOnce(&mut LightcraftApp, &egui::Context, T) + Send + 'static,
) -> Result<(), String> {
    let progress = Progress::default();
    let p = progress.clone();
    spawn(app, label, None, move || work(&p), done)?;
    if let Some(t) = app.tasks.running.last_mut() {
        t.progress = Some(progress);
    }
    Ok(())
}

/// Per frame: the panel of the tasks that report steps (see [`progress_window`]).
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let mut slot = 0;
    for task in app.tasks.running.iter().filter(|t| t.progress.is_some()) {
        let Some(p) = &task.progress else { continue };
        let (done, total, note) = p.snapshot();
        if progress_window(ctx, &task.label, slot, &task.label, (done, total), &note, p.cancelled()) {
            p.cancel();
        }
        slot += 1;
    }
}

/// Apply finished tasks (called every frame).
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    app.tasks.repaint = Some(ctx.clone());
    let mut i = 0;
    while i < app.tasks.running.len() {
        match app.tasks.running[i].rx.try_recv() {
            Ok(finish) => {
                app.tasks.running.remove(i);
                finish(app, ctx);
            }
            Err(TryRecvError::Empty) => i += 1,
            Err(TryRecvError::Disconnected) => {
                // the worker died (a panic, already logged)
                let t = app.tasks.running.remove(i);
                app.toast(ctx, crate::i18n::tr_format!("{} failed", t.label));
            }
        }
    }
    if !app.tasks.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Wait for every task and apply it (tests, and commands asked to `wait`). `false` on timeout.
pub fn wait(app: &mut LightcraftApp, ctx: &egui::Context, timeout: std::time::Duration) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let t0 = std::time::Instant::now();
        loop {
            poll(app, ctx);
            if app.tasks.is_empty() {
                return true;
            }
            if t0.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        // tasks ran inline: their results are already waiting
        let _ = timeout;
        poll(app, ctx);
        app.tasks.is_empty()
    }
}

/// Wait for a specific task by label and apply finished tasks. Returns `true` once the labeled
/// task finishes (or is removed), or `false` on timeout. Other background tasks may continue running.
pub fn wait_for(app: &mut LightcraftApp, ctx: &egui::Context, label: &str, timeout: std::time::Duration) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let t0 = std::time::Instant::now();
        loop {
            poll(app, ctx);
            if !app.tasks.is_running(label) {
                return true;
            }
            if t0.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = timeout;
        poll(app, ctx);
        !app.tasks.is_running(label)
    }
}

/// A progress panel at the bottom left, `slot` panels above the first (the export's own panel is
/// below them): the label, a bar (a busy bar while `total` is 0), what is in hand and Cancel.
/// Returns `true` when Cancel was pressed this frame.
pub fn progress_window(ctx: &egui::Context, key: &str, slot: usize, label: &str, (done, total): (usize, usize), note: &str, stopping: bool) -> bool {
    let t = crate::theme::Tokens::get(ctx);
    let mut cancel = false;
    egui::Window::new(format!("task-{key}"))
        .title_bar(false)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0, -130.0 - 72.0 * slot as f32])
        .fixed_size([320.0, 64.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let text = if total > 0 {
                    format!("{} · {} / {total}", crate::i18n::tr(label), (done + 1).min(total))
                } else {
                    format!("{}…", crate::i18n::tr(label))
                };
                ui.label(egui::RichText::new(text).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    cancel =
                        crate::widgets::text_button(ui, &format!("taskCancel:{key}"), if stopping { "Stopping…" } else { "Cancel" }, false).clicked();
                });
            });
            if total > 0 {
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(300.0));
            } else {
                ui.add(egui::ProgressBar::new(0.0).animate(true).desired_width(300.0));
            }
            ui.label(egui::RichText::new(note).size(11.0).color(t.text_dim));
        });
    cancel
}
