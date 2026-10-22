//! Portrait retouch off the interface thread: `portrait.apply` and `portrait.preview` need the
//! session only at the two ends, so a run is split like the gallery and the dust search are
//! (`heavy.rs`): **plan** on the caller's thread (settings, the render jobs, the face model: a few
//! milliseconds), **compute** on a worker thread (decode, face mesh, the steps, the baking: seconds,
//! and stoppable before every face and between two steps), **apply** on the caller's thread (store
//! the patch and the stack: one undo step per photo).
//!
//! [`PortraitRun`] drives an album one photo at a time; [`PortraitPreview`] makes the Before /
//! After pair of one photo. Both are polled (`pump`) by the frontend once a frame and never wait.
//! The commands (`portrait.apply`, `portrait.batch`, `portrait.preview`) keep their synchronous
//! behaviour for the CLI, MCP and tests: they use the same pieces in one go.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, PortraitStep};
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

use crate::cmd::bad;
use crate::cmd::portrait_stack::{Prepared, finish_apply, prepare};
use crate::media::RenderJob;
use crate::portrait_stack::{self as ps, CoreInput, Outcome};
use crate::{Result, Session};

/// What the worker needs: no session, no UI.
pub struct Work {
    linear: RenderJob,
    base: DevelopSettings,
    edge: usize,
    /// The developed picture the face model and the subject matte look at (face and background steps only).
    developed: Option<RenderJob>,
    #[cfg(feature = "faces")]
    model: Option<Arc<lightcraft_facemesh::FaceLandmarker>>,
    steps: Vec<PortraitStep>,
    max_faces: usize,
    /// Face meshes given instead of found (tests, scripts that already know the faces).
    fixed: Option<ps::Meshes>,
}

/// What the caller keeps to apply the worker's result.
pub struct Applier {
    pub id: PhotoId,
    base: DevelopSettings,
    steps: Vec<PortraitStep>,
    /// Hash of the photo's develop settings when the run was planned: a photo edited since is left alone.
    settings_key: u64,
}

pub enum Planned {
    Work(Box<Work>, Applier),
    /// Nothing to compute: the stack was empty (the result is already reported).
    Done(Value),
    /// A step that needs the session all along (open eyes): use the synchronous path.
    Sync,
}

fn needs_session(steps: &[PortraitStep]) -> bool {
    steps.iter().any(|st| st.enabled && st.amount > 0.0 && st.kind == ps::OPEN_EYES)
}

/// Plan the apply of `new` on photo `id` at `edge`.
pub fn plan(s: &mut Session, id: PhotoId, new: Vec<PortraitStep>, replace: bool, max_faces: usize, edge: usize) -> Result<Planned> {
    plan_fixed(s, id, new, replace, max_faces, edge, None)
}

/// [`plan`] with the faces given (`None`: the face model finds them on the worker).
pub fn plan_fixed(
    s: &mut Session,
    id: PhotoId,
    new: Vec<PortraitStep>,
    replace: bool,
    max_faces: usize,
    edge: usize,
    fixed: Option<ps::Meshes>,
) -> Result<Planned> {
    const C: &str = "portrait.apply";
    let settings_key = s.develop_of(id).map_or(0, |d| d.hash64());
    let (base, steps) = match prepare(s, id, new, replace)? {
        Prepared::Early(v) => return Ok(Planned::Done(v)),
        Prepared::Go { base, steps } => (base, steps),
    };
    plan_steps(s, id, base, steps, max_faces, edge, settings_key, fixed, C)
}

/// Plan the rendering of `steps` over `base` (the previews do not store anything).
#[allow(clippy::too_many_arguments)]
pub fn plan_steps(
    s: &mut Session,
    id: PhotoId,
    base: DevelopSettings,
    steps: Vec<PortraitStep>,
    max_faces: usize,
    edge: usize,
    settings_key: u64,
    fixed: Option<ps::Meshes>,
    c: &str,
) -> Result<Planned> {
    if needs_session(&steps) {
        return Ok(Planned::Sync);
    }
    let on = |k: &str| steps.iter().any(|st| st.enabled && st.amount > 0.0 && st.kind == k);
    let face_steps = ps::ORDER_IDS.iter().any(|k| on(k));
    let background = on(ps::BACKGROUND);
    let linear = s.render_job(id, edge, edge, false, false).ok_or_else(|| bad(c, "no such photo"))?;
    let developed = if (face_steps && fixed.is_none()) || background { Some(crate::cmd::autoseg::developed_job(s, id, c)?) } else { None };
    #[cfg(feature = "faces")]
    let model = if face_steps && fixed.is_none() { Some(crate::cmd::portrait::landmarker(s)?) } else { None };
    #[cfg(not(feature = "faces"))]
    if face_steps && fixed.is_none() {
        return Err(bad(c, "face analysis is not available in this build"));
    }
    let work = Work {
        linear,
        base: base.clone(),
        edge,
        developed,
        #[cfg(feature = "faces")]
        model,
        steps: steps.clone(),
        max_faces,
        fixed,
    };
    Ok(Planned::Work(Box::new(work), Applier { id, base, steps, settings_key }))
}

/// The worker half: everything that does not need the session. `cancel` is looked at before each
/// stage, each face and each step; a stopped run is an error [`ps::is_cancelled`] knows.
pub fn compute(work: Work, cancel: &AtomicBool) -> Result<Outcome> {
    const C: &str = "portrait.apply";
    let stop = || cancel.load(Ordering::Relaxed);
    let t = web_time::Instant::now();
    let source = work.linear.source.load_source().map_err(|e| bad(C, e))?;
    let info = source.info_or(work.linear.info);
    // never bigger than the file itself (the pipeline would scale a small photo up to the edge)
    let edge = work.edge.min(source.image.width.max(source.image.height)).max(16);
    let lin = lightcraft_pipeline::linear_stage(&source.image, &info, &work.base, edge);
    drop(source);
    let (w, h) = (lin.width, lin.height);
    let base = lightcraft_pipeline::inpaint::to_srgb8(&lin);
    if stop() {
        return Err(ps::cancelled());
    }
    let dev: Option<Rgba8> = match work.developed {
        Some(job) => Some(job.run().rendered.map_err(|e| bad(C, e))?.image),
        None => None,
    };
    if stop() {
        return Err(ps::cancelled());
    }
    #[cfg(feature = "faces")]
    let meshes = match (&work.fixed, &work.model, &dev) {
        (Some(m), _, _) => m.clone(),
        (None, Some(model), Some(img)) => crate::cmd::portrait_stack::meshes_of(model, img)?,
        _ => Vec::new(),
    };
    #[cfg(not(feature = "faces"))]
    let meshes: ps::Meshes = work.fixed.clone().unwrap_or_default();
    if stop() {
        return Err(ps::cancelled());
    }
    let out = base.clone();
    let input = CoreInput {
        base,
        out,
        w,
        h,
        steps: &work.steps,
        meshes: &meshes,
        max_faces: work.max_faces,
        eye_reports: Vec::new(),
        eyes_changed: None,
        started: t,
    };
    let mut matte = |mw: usize, mh: usize| match &dev {
        Some(img) => crate::cmd::portrait_stack::matte_of_image(img, mw, mh),
        None => Err(bad(C, "no picture for the subject matte")),
    };
    ps::render_core(input, &mut matte, &stop)
}

/// Apply the worker's result on the caller's thread. A photo whose settings changed since the plan
/// is not touched (its new edits would be lost): `{"skipped": "changed"}`.
pub fn finish(s: &mut Session, a: Applier, o: Outcome) -> Result<Value> {
    let now = s.develop_of(a.id).map_or(0, |d| d.hash64());
    if now != a.settings_key {
        return Ok(json!({"id": a.id.0, "skipped": "the photo was edited while it was being retouched: press the button again"}));
    }
    finish_apply(s, a.id, &a.base, a.steps, o)
}

fn spawn_compute(work: Work, cancel: Arc<AtomicBool>) -> Receiver<Result<Outcome>> {
    let (tx, rx) = channel();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let spawned = std::thread::Builder::new().name("lc-portrait".into()).spawn({
            let tx = tx.clone();
            move || {
                let _ = tx.send(compute(work, &cancel));
            }
        });
        if let Err(e) = spawned {
            let _ = tx.send(Err(crate::EngineError::Other(format!("could not start the retouch: {e}"))));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tx.send(compute(work, &cancel));
    }
    rx
}

struct Running {
    id: PhotoId,
    applier: Applier,
    rx: Receiver<Result<Outcome>>,
}

/// An album retouched one photo at a time without blocking the caller.
pub struct PortraitRun {
    queue: VecDeque<PhotoId>,
    new: Vec<PortraitStep>,
    replace: bool,
    max_faces: usize,
    running: Option<Running>,
    cancel: Arc<AtomicBool>,
    fixed_faces: Option<Arc<dyn Fn(PhotoId) -> ps::Meshes + Send + Sync>>,
    pub total: usize,
    pub done: usize,
    /// The report of each photo (`portrait.apply`'s shape, or `{id, error}`).
    pub photos: Vec<Value>,
    pub cancelled: bool,
    pub finished: bool,
}

impl PortraitRun {
    pub fn new(ids: Vec<PhotoId>, new: Vec<PortraitStep>, replace: bool, max_faces: usize) -> PortraitRun {
        PortraitRun {
            total: ids.len(),
            queue: ids.into(),
            new,
            replace,
            max_faces,
            running: None,
            cancel: Arc::new(AtomicBool::new(false)),
            fixed_faces: None,
            done: 0,
            photos: Vec::new(),
            cancelled: false,
            finished: false,
        }
    }

    /// Give the faces of each photo instead of finding them with the face model (tests).
    pub fn with_faces(mut self, f: Arc<dyn Fn(PhotoId) -> ps::Meshes + Send + Sync>) -> PortraitRun {
        self.fixed_faces = Some(f);
        self
    }

    /// Stop now: the photo being computed is dropped (its worker notices within a step), the ones
    /// done stay. Takes effect at once for the caller.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancelled = true;
        self.queue.clear();
        self.running = None;
        self.finished = true;
    }

    /// Photos retouched (with a face), without a face, and failed.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for p in &self.photos {
            if p.get("error").is_some() {
                c.2 += 1;
            } else if p.get("faces").and_then(Value::as_u64).is_some_and(|f| f > 0) {
                c.0 += 1;
            } else {
                c.1 += 1;
            }
        }
        c
    }

    /// Advance without waiting: apply a finished photo, start the next. `true` once everything is done.
    pub fn pump(&mut self, s: &mut Session) -> bool {
        if self.finished {
            return true;
        }
        if let Some(r) = &self.running {
            match r.rx.try_recv() {
                Err(TryRecvError::Empty) => return false,
                Ok(res) => {
                    let Some(r) = self.running.take() else { return false };
                    self.done += 1;
                    match res {
                        Ok(o) => self.photos.push(finish(s, r.applier, o).unwrap_or_else(|e| json!({"id": r.id.0, "error": e.to_string()}))),
                        Err(e) if ps::is_cancelled(&e) => {}
                        Err(e) => self.photos.push(json!({"id": r.id.0, "error": e.to_string()})),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    let Some(r) = self.running.take() else { return false };
                    self.done += 1;
                    self.photos.push(json!({"id": r.id.0, "error": "the retouch stopped unexpectedly"}));
                }
            }
        }
        while let Some(id) = self.queue.pop_front() {
            match plan_fixed(s, id, self.new.clone(), self.replace, self.max_faces, ps::EDGE, self.fixed_faces.as_ref().map(|f| f(id))) {
                Ok(Planned::Work(work, applier)) => {
                    let rx = spawn_compute(*work, self.cancel.clone());
                    self.running = Some(Running { id, applier, rx });
                    return false;
                }
                Ok(Planned::Done(v)) => {
                    self.done += 1;
                    self.photos.push(v);
                }
                Ok(Planned::Sync) => {
                    self.done += 1;
                    let r = crate::cmd::portrait_stack::sync_apply(s, id, self.new.clone(), self.replace, self.max_faces);
                    self.photos.push(r.unwrap_or_else(|e| json!({"id": id.0, "error": e.to_string()})));
                }
                Err(e) => {
                    self.done += 1;
                    self.photos.push(json!({"id": id.0, "error": e.to_string()}));
                }
            }
        }
        self.finished = self.running.is_none();
        self.finished
    }
}

/// The Before / After of one photo, made off the caller's thread in two stages: the retouch, then
/// the two renders of the whole develop (with and without the patch).
pub struct PortraitPreview {
    stage: Stage,
    cancel: Arc<AtomicBool>,
    id: PhotoId,
    edge: usize,
}

enum Stage {
    Retouch { rx: Receiver<Result<Outcome>>, applier: Applier },
    Render { rx: Receiver<Result<(Rgba8, Rgba8)>>, faces: usize, report: Value, retouch_seconds: f64 },
    Gone,
}

pub struct PreviewDone {
    pub before: Rgba8,
    pub after: Rgba8,
    pub faces: usize,
    pub steps: Value,
}

impl PortraitPreview {
    /// `new` sliders on top of the photo's stack (or alone with `replace`), at `edge`.
    pub fn start(s: &mut Session, id: PhotoId, new: Vec<PortraitStep>, replace: bool, max_faces: usize, edge: usize) -> Result<PortraitPreview> {
        const C: &str = "portrait.preview";
        let d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
        let mut steps = if replace { Vec::new() } else { d.portrait.steps.clone() };
        ps::merge(&mut steps, new);
        steps.retain(|st| st.amount > 0.0);
        if steps.is_empty() {
            return Err(bad(C, "no steps: give slider amounts"));
        }
        let base = ps::base_settings(&d);
        let cancel = Arc::new(AtomicBool::new(false));
        match plan_steps(s, id, base, steps, max_faces, edge, d.hash64(), None, C)? {
            Planned::Work(work, applier) => {
                let rx = spawn_compute(*work, cancel.clone());
                Ok(PortraitPreview { stage: Stage::Retouch { rx, applier }, cancel, id, edge })
            }
            _ => Err(bad(C, "the preview of this retouch needs the synchronous command (open eyes)")),
        }
    }

    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.stage = Stage::Gone;
    }

    /// `None` while working; `Some(result)` once (then the preview is spent).
    pub fn pump(&mut self, s: &mut Session) -> Option<Result<PreviewDone>> {
        const C: &str = "portrait.preview";
        match std::mem::replace(&mut self.stage, Stage::Gone) {
            Stage::Gone => None,
            Stage::Retouch { rx, applier } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.stage = Stage::Retouch { rx, applier };
                    None
                }
                Err(TryRecvError::Disconnected) => Some(Err(bad(C, "the retouch stopped unexpectedly"))),
                Ok(Err(e)) => Some(Err(e)),
                Ok(Ok(o)) => {
                    let cached = o.patch.clone().map(|pt| s.cache_patch(pt));
                    let after_settings = ps::with_stack(&applier.base, applier.steps.clone(), &o, cached.as_deref());
                    let (id, edge) = (self.id, self.edge);
                    let (Some(jb), Some(ja)) =
                        (s.preview_job(id, edge, edge, true, &applier.base), s.preview_job(id, edge, edge, true, &after_settings))
                    else {
                        return Some(Err(bad(C, "no such photo")));
                    };
                    let (tx, rx2) = channel();
                    let run = move || {
                        let r = (|| -> Result<(Rgba8, Rgba8)> {
                            let b = jb.run().rendered.map_err(|e| bad(C, e))?.image;
                            let a = ja.run().rendered.map_err(|e| bad(C, e))?.image;
                            Ok((b, a))
                        })();
                        let _ = tx.send(r);
                    };
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if let Err(e) = std::thread::Builder::new().name("lc-portrait-preview".into()).spawn(run) {
                            return Some(Err(crate::EngineError::Other(format!("could not start the preview: {e}"))));
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    run();
                    self.stage = Stage::Render { rx: rx2, faces: o.faces, report: ps::report_json(&o), retouch_seconds: o.seconds };
                    None
                }
            },
            Stage::Render { rx, faces, report, retouch_seconds } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.stage = Stage::Render { rx, faces, report, retouch_seconds };
                    None
                }
                Err(TryRecvError::Disconnected) => Some(Err(bad(C, "the preview stopped unexpectedly"))),
                Ok(Err(e)) => Some(Err(e)),
                Ok(Ok((before, after))) => {
                    let _ = retouch_seconds;
                    Some(Ok(PreviewDone { before, after, faces, steps: report }))
                }
            },
        }
    }
}
