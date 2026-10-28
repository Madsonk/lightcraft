//! "My style" and automatic framing off the interface thread: `style.train`, `style.apply`,
//! `autostraighten.suggest`, `autocrop.suggest`.
//!
//! Every job is split like the portrait and culling jobs are: **plan** on the caller's thread (what
//! to read, from the catalogue), **compute** on a worker thread (decoding, measuring, learning:
//! seconds to minutes, stoppable between two photos), **apply** on the caller's thread (one undo
//! step). The frontend polls `status` once a frame and never waits; the CLI, MCP and tests pass
//! `wait: true` and get the finished result.
//!
//! Nothing here edits a photo without an explicit `apply` / `mode: "apply"` plus `confirm: true`.
//! A suggestion is a [`DevelopSettings`] proposal the interface renders as Before / After
//! (`Session::style_proposal`, `Session::framing_proposal`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use lightcraft_autoframe::{self as framing, CropRequest, Subject};
use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::DevelopSettings;
use lightcraft_geom::{CropGeometry, Rect};
use lightcraft_style::{Config, Sample, StyleModel, targets};
use serde_json::{Value, json};
use web_time::Instant;

use crate::cmd::{CommandSpec, always, bad, cmd};
use crate::media::{SourceLevel, SourceRef};
use crate::{Result, Session};

/// Longest a waiting command (CLI, MCP, tests) waits.
const WAIT: Duration = Duration::from_secs(2 * 60 * 60);
/// Photos one run accepts.
const LIMIT: usize = 20_000;
/// The default name of a profile.
pub const DEFAULT_NAME: &str = "Mon style";

#[derive(Default)]
pub struct StyleLab {
    train: Option<Arc<Mutex<TrainJob>>>,
    plan: Option<Arc<Mutex<PlanJob>>>,
    frame: Option<Arc<Mutex<FrameJob>>>,
    /// Profiles that are not on disk (a library without a folder: tests, the demo).
    memory: BTreeMap<String, StyleModel>,
}

fn lock<T>(j: &Arc<Mutex<T>>) -> MutexGuard<'_, T> {
    j.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Running,
    Done,
    Cancelled,
    Failed(String),
}

fn phase_name(p: &Phase) -> &'static str {
    match p {
        Phase::Running => "running",
        Phase::Done => "done",
        Phase::Cancelled => "cancelled",
        Phase::Failed(_) => "failed",
    }
}

fn flag(p: &Value, key: &str, default: bool, c: &str) -> Result<bool> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_bool().ok_or_else(|| bad(c, format!("{key} must be a boolean"))),
    }
}

fn unit(p: &Value, key: &str, default: f32, c: &str) -> Result<f32> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
            .map(|n| n as f32)
            .ok_or_else(|| bad(c, format!("{key} must be a number 0..1"))),
    }
}

fn name_of(p: &Value, c: &str) -> Result<String> {
    match p.get("name").or_else(|| p.get("profile")) {
        None | Some(Value::Null) => Ok(DEFAULT_NAME.to_string()),
        Some(v) => {
            let n = v
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty() && n.chars().count() <= 60)
                .ok_or_else(|| bad(c, "name must be 1..60 characters"))?;
            Ok(n.to_string())
        }
    }
}

/// A file name for a profile.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "style".into() } else { s }
}

fn style_dir(s: &Session) -> Option<PathBuf> {
    crate::describe::ops::library_dir(s).map(|d| d.join("style"))
}

fn write_atomic(dir: &Path, name: &str, model: &StyleModel) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", file_stem(name)));
    let tmp = dir.join(format!("{}.json.tmp", file_stem(name)));
    std::fs::write(&tmp, model.to_json()).map_err(|e| format!("can't write the profile: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("can't save the profile: {e}"))
}

/// Every known profile, by name: those on disk, then those only in memory.
fn profiles(s: &Session) -> BTreeMap<String, StyleModel> {
    let mut out = s.style.memory.clone();
    if let Some(dir) = style_dir(s)
        && let Ok(rd) = std::fs::read_dir(&dir)
    {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            // a corrupt or foreign file is skipped, never an error for the others
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(m) = StyleModel::from_json(&text)
            {
                out.insert(m.name.clone(), m);
            }
        }
    }
    out
}

fn profile_row(m: &StyleModel, bytes: usize) -> Value {
    json!({
        "name": m.name, "trained": m.trained, "samples": m.samples, "shoots": m.groups, "bytes": bytes,
        "learned": m.summary.learned_targets, "targets": m.fits.len(),
        "skillVsNone": m.summary.skill_vs_none, "skillVsMedian": m.summary.skill_vs_median,
        "split": m.summary.split, "trainSeconds": m.summary.train_seconds,
        "sliders": m.fits.iter().map(|f| json!({
            "id": f.id, "learned": f.learned, "median": f.median, "usedShare": f.used_share,
            "maeModel": f.mae_model, "maeNone": f.mae_none, "maeMedian": f.mae_median,
        })).collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------------------------------------------------
// style.train

struct TrainJob {
    phase: Phase,
    name: String,
    total: usize,
    started: Instant,
    elapsed: Option<Duration>,
    /// Photos measured so far.
    measured: Arc<AtomicUsize>,
    /// Learning progress, 0..1 (f32 bits).
    learned: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    failed: Vec<(u64, String)>,
    unedited: usize,
    model: Option<StyleModel>,
    stored: bool,
    saved: Option<String>,
}

struct TrainInput {
    id: PhotoId,
    source: SourceRef,
    header: lightcraft_pipeline::SourceInfo,
    develop: Arc<DevelopSettings>,
    group: u64,
}

fn group_of(p: &Photo) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match &p.source {
        Source::File { path } => Path::new(path).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default().hash(&mut h),
        Source::Demo { scene } => scene.hash(&mut h),
    }
    h.finish()
}

/// Whether the edit moves any slider a style can learn (imported defaults and crops do not count).
fn moves_sliders(d: &DevelopSettings) -> bool {
    let info = lightcraft_pipeline::SourceInfo::default();
    targets::from_settings(d, &info).iter().zip(targets::TARGETS.iter()).any(|(v, sp)| v.abs() >= sp.dead_zone)
}

fn is_image(p: &Photo) -> bool {
    !p.deleted && matches!(p.kind, MediaKind::Image | MediaKind::Raw)
}

#[cfg(not(target_arch = "wasm32"))]
fn launch_train(
    job: Arc<Mutex<TrainJob>>,
    inputs: Vec<TrainInput>,
    cfg: Config,
    trained: String,
    dir: Option<PathBuf>,
) -> std::result::Result<(), String> {
    let (measured, learned, cancel, name) = {
        let j = lock(&job);
        (j.measured.clone(), j.learned.clone(), j.cancel.clone(), j.name.clone())
    };
    std::thread::Builder::new()
        .name("style-train".into())
        .spawn(move || {
            use rayon::prelude::*;
            let outcome = crate::guard::catch("style learning", || {
                let rows: Vec<Option<std::result::Result<Option<Sample>, String>>> = inputs
                    .par_iter()
                    .map(|inp| {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                        let r = inp.source.load_source().map(|d| {
                            let info = d.info_or(inp.header.clone());
                            let t = targets::from_settings(&inp.develop, &info);
                            let edited = t.iter().zip(targets::TARGETS.iter()).any(|(v, sp)| v.abs() >= sp.dead_zone);
                            edited.then(|| Sample { features: lightcraft_style::extract(&d.image, &info).values, targets: t, group: inp.group })
                        });
                        measured.fetch_add(1, Ordering::Relaxed);
                        Some(r)
                    })
                    .collect();
                if cancel.load(Ordering::Relaxed) || rows.iter().any(Option::is_none) {
                    return Err(lightcraft_style::Error::Cancelled);
                }
                let (mut samples, mut failed, mut unedited) = (Vec::new(), Vec::new(), 0usize);
                for (inp, row) in inputs.iter().zip(rows) {
                    match row {
                        Some(Ok(Some(s))) if !s.features.iter().all(|v| v.is_finite()) => {
                            failed.push((inp.id.0, "non-finite measurements".to_string()))
                        }
                        Some(Ok(Some(s))) if !s.targets.iter().all(|v| v.is_finite()) => {
                            failed.push((inp.id.0, "non-finite edit values".to_string()))
                        }
                        Some(Ok(Some(s))) => samples.push(s),
                        Some(Ok(None)) => unedited += 1,
                        Some(Err(e)) => failed.push((inp.id.0, e)),
                        None => {}
                    }
                }
                // keep what was read even when the learning then refuses (too few usable photos)
                {
                    let mut j = lock(&job);
                    j.failed = failed.clone();
                    j.unedited = unedited;
                }
                let mut progress = |f: f32| {
                    learned.store(f.to_bits(), Ordering::Relaxed);
                    !cancel.load(Ordering::Relaxed)
                };
                let t = Instant::now();
                let model = lightcraft_style::train(&samples, &name, &trained, &cfg, &mut progress)?;
                let mut model = model;
                model.summary.train_seconds = t.elapsed().as_secs_f32();
                Ok((model, failed, unedited))
            });
            let mut j = lock(&job);
            j.elapsed = Some(j.started.elapsed());
            match outcome {
                Ok(Ok((model, failed, unedited))) => {
                    j.failed = failed;
                    j.unedited = unedited;
                    if let Some(dir) = &dir {
                        match write_atomic(dir, &model.name, &model) {
                            Ok(()) => j.saved = Some(dir.join(format!("{}.json", file_stem(&model.name))).to_string_lossy().to_string()),
                            Err(e) => log::warn!("style profile not saved: {e}"),
                        }
                    }
                    j.model = Some(model);
                    j.phase = Phase::Done;
                }
                Ok(Err(lightcraft_style::Error::Cancelled)) => j.phase = Phase::Cancelled,
                Ok(Err(e)) => j.phase = Phase::Failed(e.to_string()),
                Err(e) => j.phase = Phase::Failed(e),
            }
        })
        .map(|_| ())
        .map_err(|e| format!("can't start the learning worker: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn launch_train(_: Arc<Mutex<TrainJob>>, _: Vec<TrainInput>, _: Config, _: String, _: Option<PathBuf>) -> std::result::Result<(), String> {
    Err("learning a style needs the desktop app or the CLI".into())
}

fn train_status(s: &Session) -> Value {
    let Some(job) = &s.style.train else { return json!({"state": "idle"}) };
    let j = lock(job);
    let measured = j.measured.load(Ordering::Relaxed).min(j.total);
    let learned = f32::from_bits(j.learned.load(Ordering::Relaxed));
    // reading the photos is most of the work: 85 % of the bar, then the learning
    let percent = if j.total == 0 { 100.0 } else { (measured as f64 / j.total as f64) * 85.0 + learned as f64 * 15.0 };
    let elapsed = j.elapsed.unwrap_or_else(|| j.started.elapsed());
    let mut out = json!({
        "state": phase_name(&j.phase), "name": j.name, "total": j.total, "measured": measured,
        "percent": if j.phase == Phase::Done { 100.0 } else { (percent * 10.0).round() / 10.0 },
        "elapsedMs": elapsed.as_millis() as u64, "unedited": j.unedited,
        "failed": j.failed.iter().take(20).map(|(id, e)| json!([id, e])).collect::<Vec<_>>(), "failedCount": j.failed.len(),
        "saved": j.saved,
    });
    if let Some(o) = out.as_object_mut() {
        if let Phase::Failed(e) = &j.phase {
            o.insert("error".into(), json!(e));
        }
        if let Some(m) = &j.model {
            o.insert("profile".into(), profile_row(m, m.to_json().len()));
        }
    }
    out
}

impl Session {
    /// Move a finished learning run's profile into the session. Called by every `style.*`
    /// command; the desktop app can call it on its idle tick. Returns whether a profile arrived.
    pub fn style_poll(&mut self) -> bool {
        let Some(job) = self.style.train.clone() else { return false };
        let model = {
            let mut j = lock(&job);
            if j.phase != Phase::Done || j.stored {
                return false;
            }
            j.stored = true;
            j.model.clone()
        };
        let Some(model) = model else { return false };
        if style_dir(self).is_none() {
            self.style.memory.insert(model.name.clone(), model);
        }
        true
    }

    /// The suggested develop settings of `id` from the last `style.apply` preview (for Before / After).
    pub fn style_proposal(&self, id: PhotoId) -> Option<DevelopSettings> {
        let plan = self.style.plan.as_ref()?;
        lock(plan).rows.iter().find(|r| r.id == id)?.proposed.clone()
    }

    /// The suggested develop settings of `id` from the last framing run (for the crop overlay).
    pub fn framing_proposal(&self, id: PhotoId) -> Option<DevelopSettings> {
        let job = self.style.frame.as_ref()?;
        lock(job).rows.iter().find(|r| r.id == id)?.proposed.clone()
    }

    /// Whether a style or framing job is running (the interface keeps repainting).
    pub fn style_busy(&self) -> bool {
        let running = |p: &Phase| *p == Phase::Running;
        self.style.train.as_ref().is_some_and(|j| running(&lock(j).phase))
            || self.style.plan.as_ref().is_some_and(|j| running(&lock(j).phase))
            || self.style.frame.as_ref().is_some_and(|j| running(&lock(j).phase))
    }
}

fn train(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "style.train";
    s.style_poll();
    if s.style.train.as_ref().is_some_and(|j| lock(j).phase == Phase::Running) {
        return Err(bad(C, "a style is already being learned (style.status, style.cancel)"));
    }
    let name = name_of(p, C)?;
    let wait = flag(p, "wait", false, C)?;
    let min_photos = match p.get("minPhotos") {
        None | Some(Value::Null) => 30usize,
        Some(v) => v.as_u64().filter(|n| (4..=100_000).contains(n)).ok_or_else(|| bad(C, "minPhotos must be 4..100000"))? as usize,
    };
    let folds = match p.get("folds") {
        None | Some(Value::Null) => 5usize,
        Some(v) => v.as_u64().filter(|n| (2..=10).contains(n)).ok_or_else(|| bad(C, "folds must be 2..10"))? as usize,
    };
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() {
        checked_targets(s, C, p)?
    } else {
        s.catalog.photos().filter(|ph| is_image(ph)).map(|ph| ph.id).collect()
    };
    // only photos that carry an edit are examples of a style; the pixels decide the rest
    let mut inputs = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        if !is_image(&ph) || !moves_sliders(&ph.develop) {
            continue;
        }
        if let Source::File { path } = &ph.source
            && !Path::new(path).exists()
        {
            continue;
        }
        if inputs.len() >= LIMIT {
            return Err(bad(C, format!("more than {LIMIT} edited photos: pass ids")));
        }
        let source = s.media.source_ref_uncached(&ph, SourceLevel::Thumb);
        inputs.push(TrainInput { id, source, header: s.source_info(id), develop: ph.develop.clone(), group: group_of(&ph) });
    }
    if inputs.len() < min_photos {
        return Err(bad(
            C,
            format!(
                "{} edited photos with an original found, at least {min_photos} are needed to learn a style (import your Lightroom catalogue or edit more photos; minPhotos lowers the bar, the result will say how little it is worth)",
                inputs.len()
            ),
        ));
    }
    let job = Arc::new(Mutex::new(TrainJob {
        phase: Phase::Running,
        name,
        total: inputs.len(),
        started: Instant::now(),
        elapsed: None,
        measured: Arc::new(AtomicUsize::new(0)),
        learned: Arc::new(AtomicU32::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
        failed: Vec::new(),
        unedited: 0,
        model: None,
        stored: false,
        saved: None,
    }));
    let cfg = Config { folds, min_samples: min_photos.min(12), ..Config::default() };
    launch_train(job.clone(), inputs, cfg, (s.clock)(), style_dir(s)).map_err(|e| bad(C, e))?;
    s.style.train = Some(job.clone());
    if wait {
        let deadline = Instant::now() + WAIT;
        while lock(&job).phase == Phase::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        s.style_poll();
    }
    Ok(train_status(s))
}

// ------------------------------------------------------------------------------------------------
// style.apply

struct PlanRow {
    id: PhotoId,
    before: Arc<DevelopSettings>,
    proposed: Option<DevelopSettings>,
    report: Value,
}

struct PlanJob {
    phase: Phase,
    profile: String,
    total: usize,
    started: Instant,
    elapsed: Option<Duration>,
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    rows: Vec<PlanRow>,
    applied: bool,
    strength: f32,
    consistency: f32,
    series: usize,
}

struct PlanInput {
    id: PhotoId,
    name: String,
    source: SourceRef,
    header: lightcraft_pipeline::SourceInfo,
    develop: Arc<DevelopSettings>,
    group: usize,
}

/// Which sliders a plan may touch (`only` lists target ids; absent: all the profile learned).
#[derive(Clone)]
struct Mask([bool; targets::N_TARGETS]);

#[cfg(not(target_arch = "wasm32"))]
fn launch_plan(
    job: Arc<Mutex<PlanJob>>,
    inputs: Vec<PlanInput>,
    model: StyleModel,
    mask: Mask,
    overwrite: bool,
    groups: usize,
) -> std::result::Result<(), String> {
    let (progress, cancel, strength, consistency) = {
        let j = lock(&job);
        (j.progress.clone(), j.cancel.clone(), j.strength, j.consistency)
    };
    std::thread::Builder::new()
        .name("style-plan".into())
        .spawn(move || {
            use rayon::prelude::*;
            let outcome = crate::guard::catch("style preview", || {
                type Measured = std::result::Result<(lightcraft_style::Features, lightcraft_pipeline::SourceInfo), String>;
                let measured: Vec<Option<Measured>> = inputs
                    .par_iter()
                    .map(|inp| {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                        let r = inp.source.load_source().map(|d| {
                            let info = d.info_or(inp.header.clone());
                            (lightcraft_style::extract(&d.image, &info), info)
                        });
                        progress.fetch_add(1, Ordering::Relaxed);
                        Some(r)
                    })
                    .collect();
                if cancel.load(Ordering::Relaxed) || measured.iter().any(Option::is_none) {
                    return None;
                }
                // one prediction per photo, then each series pulled towards a shared style
                let mut preds: Vec<Option<(targets::Targets, f32)>> = measured
                    .iter()
                    .map(|m| match m {
                        Some(Ok((f, _))) => model.predict(&f.values).map(|t| (t, f.auto_exposure)),
                        _ => None,
                    })
                    .collect();
                for g in 0..groups {
                    let members: Vec<usize> = inputs.iter().enumerate().filter(|(i, inp)| inp.group == g && preds.get(*i).is_some_and(Option::is_some)).map(|(i, _)| i).collect();
                    let mut ts: Vec<targets::Targets> = members.iter().filter_map(|i| preds.get(*i).and_then(|p| p.map(|x| x.0))).collect();
                    let auto: Vec<f32> = members.iter().filter_map(|i| preds.get(*i).and_then(|p| p.map(|x| x.1))).collect();
                    lightcraft_style::series::harmonize(&mut ts, &auto, consistency);
                    for (i, t) in members.iter().zip(ts) {
                        if let Some(Some(slot)) = preds.get_mut(*i) {
                            slot.0 = t;
                        }
                    }
                }
                let mut rows = Vec::with_capacity(inputs.len());
                for ((inp, m), pred) in inputs.iter().zip(measured).zip(preds) {
                    let base = json!({"id": inp.id.0, "name": inp.name, "series": inp.group});
                    let (mut row, info) = match (m, pred) {
                        (Some(Ok((_, info))), Some((t, _))) => (PlanRow { id: inp.id, before: inp.develop.clone(), proposed: None, report: base }, Some((info, t))),
                        (Some(Err(e)), _) => {
                            (PlanRow { id: inp.id, before: inp.develop.clone(), proposed: None, report: json!({"id": inp.id.0, "name": inp.name, "error": e}) }, None)
                        }
                        _ => (PlanRow { id: inp.id, before: inp.develop.clone(), proposed: None, report: json!({"id": inp.id.0, "name": inp.name, "error": "the profile does not fit this photo"}) }, None),
                    };
                    if let Some((info, mut t)) = info {
                        // what the photographer already did to this photo is kept unless asked otherwise
                        let current = targets::from_settings(&inp.develop, &info);
                        let touched: Vec<bool> = current.iter().zip(targets::TARGETS.iter()).map(|(v, sp)| v.abs() >= sp.dead_zone).collect();
                        let edited = touched.iter().any(|x| *x);
                        for (i, v) in t.iter_mut().enumerate() {
                            let allowed = mask.0.get(i).copied().unwrap_or(false) && (overwrite || !touched.get(i).copied().unwrap_or(false));
                            *v = if allowed { *v * strength } else { 0.0 };
                        }
                        let mut d = (*inp.develop).clone();
                        targets::apply(&mut d, &t, &info);
                        let changed = d != *inp.develop;
                        row.report = json!({
                            "id": inp.id.0, "name": inp.name, "series": inp.group, "alreadyEdited": edited, "changes": changed,
                            "sliders": targets::TARGETS.iter().zip(t.iter()).filter(|(sp, v)| v.abs() >= sp.dead_zone).map(|(sp, v)| json!({"id": sp.id, "value": (*v * 100.0).round() / 100.0})).collect::<Vec<_>>(),
                            "beforeHash": inp.develop.hash64(),
                        });
                        if changed {
                            row.proposed = Some(d);
                        }
                    }
                    rows.push(row);
                }
                Some(rows)
            });
            let mut j = lock(&job);
            j.elapsed = Some(j.started.elapsed());
            match outcome {
                Ok(Some(rows)) => {
                    j.rows = rows;
                    j.phase = Phase::Done;
                }
                Ok(None) => j.phase = Phase::Cancelled,
                Err(e) => j.phase = Phase::Failed(e),
            }
        })
        .map(|_| ())
        .map_err(|e| format!("can't start the preview worker: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn launch_plan(_: Arc<Mutex<PlanJob>>, _: Vec<PlanInput>, _: StyleModel, _: Mask, _: bool, _: usize) -> std::result::Result<(), String> {
    Err("applying a style needs the desktop app or the CLI".into())
}

fn plan_status(s: &Session) -> Value {
    let Some(job) = &s.style.plan else { return json!({"state": "idle"}) };
    let j = lock(job);
    let done = j.progress.load(Ordering::Relaxed).min(j.total);
    let elapsed = j.elapsed.unwrap_or_else(|| j.started.elapsed());
    let mut out = json!({
        "state": if j.applied { "applied" } else { phase_name(&j.phase) }, "profile": j.profile, "total": j.total, "done": done,
        "percent": if j.total == 0 { 100.0 } else { (done as f64 / j.total as f64 * 1000.0).round() / 10.0 },
        "elapsedMs": elapsed.as_millis() as u64, "strength": j.strength, "consistency": j.consistency, "series": j.series,
        "changes": j.rows.iter().filter(|r| r.proposed.is_some()).count(),
        "photos": j.rows.iter().map(|r| r.report.clone()).collect::<Vec<_>>(),
    });
    if let (Phase::Failed(e), Some(o)) = (&j.phase, out.as_object_mut()) {
        o.insert("error".into(), json!(e));
    }
    out
}

fn mask_from(p: &Value, model: &StyleModel, c: &str) -> Result<Mask> {
    let mut m = [false; targets::N_TARGETS];
    match p.get("only") {
        None | Some(Value::Null) => {
            // everything the profile learned; the sliders it could not predict stay untouched
            for (slot, f) in m.iter_mut().zip(&model.fits) {
                *slot = f.learned;
            }
        }
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(c, "only must be an array of slider ids"))?;
            for item in list {
                let id = item.as_str().ok_or_else(|| bad(c, "only must be an array of slider ids"))?;
                let i = targets::TARGETS.iter().position(|sp| sp.id == id).ok_or_else(|| bad(c, format!("no slider {id} (see style.list)")))?;
                if let Some(slot) = m.get_mut(i) {
                    *slot = true;
                }
            }
        }
    }
    Ok(Mask(m))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "style.apply";
    s.style_poll();
    let mode = p.get("mode").and_then(Value::as_str).unwrap_or("preview");
    match mode {
        "preview" => preview(s, p),
        "apply" => commit_plan(s, p),
        "undo" => {
            let label = s.undo.last().map(|e| e.label.clone());
            if label.as_deref() != Some("Apply my style") {
                return Err(bad(C, "the last change is not an applied style (nothing to undo here)"));
            }
            s.execute("edit.undo", &json!({}))?;
            if let Some(plan) = &s.style.plan {
                lock(plan).applied = false;
            }
            Ok(json!({"undone": true}))
        }
        _ => Err(bad(C, "mode must be preview, apply or undo")),
    }
}

fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "style.apply";
    if s.style.plan.as_ref().is_some_and(|j| lock(j).phase == Phase::Running) {
        return Err(bad(C, "a preview is already running (style.status, style.cancel)"));
    }
    let all = profiles(s);
    let name = name_of(p, C)?;
    let model = match all.get(&name) {
        Some(m) => m.clone(),
        None if p.get("name").is_none() && p.get("profile").is_none() && all.len() == 1 => {
            all.values().next().cloned().ok_or_else(|| bad(C, "no profile"))?
        }
        None => return Err(bad(C, format!("no style profile named \"{name}\" (style.list, style.train)"))),
    };
    let strength = unit(p, "strength", 1.0, C)?;
    let consistency = unit(p, "consistency", 0.7, C)?;
    let overwrite = flag(p, "overwrite", false, C)?;
    let wait = flag(p, "wait", false, C)?;
    let mask = mask_from(p, &model, C)?;
    let ids: Vec<PhotoId> = checked_targets(s, C, p)?;
    if ids.len() > LIMIT {
        return Err(bad(C, format!("at most {LIMIT} photos per preview")));
    }
    let photos: Vec<Arc<Photo>> = ids.iter().filter_map(|id| s.catalog.photo(*id).cloned()).filter(|ph| is_image(ph)).collect();
    if photos.is_empty() {
        return Err(bad(C, "no photos to style (videos and deleted photos are skipped)"));
    }
    let series = group_photos(photos.clone());
    let group_index: std::collections::HashMap<PhotoId, usize> =
        series.iter().enumerate().flat_map(|(g, ids)| ids.iter().map(move |id| (*id, g))).collect();
    let mut inputs = Vec::with_capacity(photos.len());
    for ph in &photos {
        let source = s.media.source_ref_uncached(ph, SourceLevel::Thumb);
        inputs.push(PlanInput {
            id: ph.id,
            name: ph.file_name.clone(),
            source,
            header: s.source_info(ph.id),
            develop: ph.develop.clone(),
            group: group_index.get(&ph.id).copied().unwrap_or(0),
        });
    }
    let job = Arc::new(Mutex::new(PlanJob {
        phase: Phase::Running,
        profile: model.name.clone(),
        total: inputs.len(),
        started: Instant::now(),
        elapsed: None,
        progress: Arc::new(AtomicUsize::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
        generation: s.library_generation(),
        rows: Vec::new(),
        applied: false,
        strength,
        consistency,
        series: series.len(),
    }));
    launch_plan(job.clone(), inputs, model, mask, overwrite, series.len()).map_err(|e| bad(C, e))?;
    s.style.plan = Some(job.clone());
    if wait {
        let deadline = Instant::now() + WAIT;
        while lock(&job).phase == Phase::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(plan_status(s))
}

fn commit_plan(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "style.apply";
    if !flag(p, "confirm", false, C)? {
        return Err(bad(C, "applying a style changes photos: pass confirm: true after looking at the preview"));
    }
    let shared = s.style.plan.clone().ok_or_else(|| bad(C, "no preview: run style.apply with mode \"preview\" first"))?;
    let (ops, changed, skipped) = {
        let st = lock(&shared);
        if st.phase != Phase::Done || st.applied {
            return Err(bad(C, "finish and review an unapplied preview first"));
        }
        if st.generation != s.library_generation() {
            return Err(bad(C, "the library changed; make a new preview"));
        }
        // a photo edited since the preview is left alone, the others are still applied
        let only: Option<Vec<u64>> = match p.get("ids") {
            Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_u64).collect()),
            _ => None,
        };
        let (mut ops, mut skipped) = (Vec::new(), 0usize);
        for row in &st.rows {
            let Some(d) = &row.proposed else { continue };
            if only.as_ref().is_some_and(|o| !o.contains(&row.id.0)) {
                continue;
            }
            let current = s.catalog.photo(row.id);
            if current.is_none_or(|c| c.deleted || c.develop != row.before) {
                skipped += 1;
                continue;
            }
            ops.extend(s.develop_op(row.id, d.clone(), "Apply my style"));
        }
        let changed = ops.len();
        (ops, changed, skipped)
    };
    if changed > 0 {
        s.commit("Apply my style", Op::Batch { ops })?;
    }
    lock(&shared).applied = true;
    Ok(json!({"changed": changed, "skippedEditedSincePreview": skipped}))
}

// ------------------------------------------------------------------------------------------------
// autostraighten.suggest / autocrop.suggest

#[derive(Clone, Copy, PartialEq)]
enum FrameKind {
    Straighten,
    Crop,
}

struct FrameRow {
    id: PhotoId,
    before: Arc<DevelopSettings>,
    proposed: Option<DevelopSettings>,
    report: Value,
}

struct FrameJob {
    kind: FrameKind,
    phase: Phase,
    total: usize,
    started: Instant,
    elapsed: Option<Duration>,
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    rows: Vec<FrameRow>,
    applied: bool,
}

struct FrameInput {
    id: PhotoId,
    name: String,
    source: SourceRef,
    header: lightcraft_pipeline::SourceInfo,
    develop: Arc<DevelopSettings>,
    faces: Vec<[f32; 4]>,
}

struct FrameOptions {
    kind: FrameKind,
    aspect: Option<(u32, u32)>,
    straighten_first: bool,
    min_confidence: f32,
}

fn parse_aspect(v: Option<&Value>, c: &str) -> Result<Option<(u32, u32)>> {
    let pair = |a: f64, b: f64| -> Result<Option<(u32, u32)>> {
        if a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && a / b > 0.1 && a / b < 10.0 {
            Ok(Some(((a * 100.0).round() as u32, (b * 100.0).round() as u32)))
        } else {
            Err(bad(c, "aspect out of range"))
        }
    };
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) if t == "original" => Ok(None),
        Some(Value::String(t)) => {
            let (a, b) = t.split_once(['x', ':']).ok_or_else(|| bad(c, "aspect: use original, WxH or [w, h]"))?;
            pair(a.trim().parse().map_err(|_| bad(c, "aspect: bad number"))?, b.trim().parse().map_err(|_| bad(c, "aspect: bad number"))?)
        }
        Some(Value::Array(a)) if a.len() == 2 => pair(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)),
        Some(_) => Err(bad(c, "aspect: use original, WxH or [w, h]")),
    }
}

fn frame_one(inp: &FrameInput, opts: &FrameOptions) -> FrameRow {
    let none = |report: Value| FrameRow { id: inp.id, before: inp.develop.clone(), proposed: None, report };
    let head = |extra: Value| {
        let mut v = json!({"id": inp.id.0, "name": inp.name});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };
    let decoded = match inp.source.load_source() {
        Ok(d) => d,
        Err(e) => return none(head(json!({"error": e}))),
    };
    let info = decoded.info_or(inp.header.clone());
    let src = &*decoded.image;
    // the suggestion is measured on the photo as it is now, without a crop of its own
    let base = DevelopSettings { crop: Default::default(), ..(*inp.develop).clone() };
    let st = framing::straighten(src, &info, &base);
    let angle_ok = st.as_ref().filter(|x| x.confidence >= opts.min_confidence && x.angle.abs() >= 0.1);
    match opts.kind {
        FrameKind::Straighten => {
            let Some(x) = st else {
                return none(head(json!({"suggestion": null, "reason": "no dominant horizon or vertical lines"})));
            };
            let usable = x.confidence >= opts.min_confidence;
            let mut d = (*inp.develop).clone();
            let (w, h) = (src.width as f64, src.height as f64);
            if usable && x.angle.abs() >= 0.1 {
                d.crop.geometry = lightcraft_geom::crop_fit_angle(w, h, x.angle, d.crop.aspect.map(|(a, b)| a as f64 / b.max(1) as f64));
            }
            let proposed = (usable && x.angle.abs() >= 0.1 && d != *inp.develop).then_some(d);
            FrameRow {
                id: inp.id,
                before: inp.develop.clone(),
                report: head(
                    json!({"suggestion": usable.then_some(json!({"angle": (x.angle * 100.0).round() / 100.0, "confidence": x.confidence, "kind": x.kind, "lines": x.lines, "horizonY": x.horizon_y})),
                    "reason": if !usable { json!("the detected lines disagree: not sure enough to suggest") } else if x.angle.abs() < 0.1 { json!("already level") } else { Value::Null }}),
                ),
                proposed,
            }
        }
        FrameKind::Crop => {
            let (w, h) = (src.width as f64, src.height as f64);
            let angle = if opts.straighten_first { angle_ok.map_or(0.0, |x| x.angle) } else { 0.0 };
            let mut subjects: Vec<Subject> = inp
                .faces
                .iter()
                .map(|f| Subject {
                    rect: Rect::new(f[0] as f64, f[1] as f64, f[2] as f64, f[3] as f64).inflate(0.01, 0.01),
                    weight: 1.0,
                    kind: "visage",
                })
                .collect();
            if subjects.is_empty()
                && let Some(sub) = framing::saliency_subject(src)
            {
                subjects.push(sub);
            }
            let aspect = opts.aspect.map(|(a, b)| a as f64 / b.max(1) as f64);
            let req = CropRequest {
                width: w,
                height: h,
                angle,
                aspect,
                subjects,
                horizon: st.as_ref().filter(|x| x.confidence >= 0.5).and_then(|x| x.horizon_y),
            };
            let Some(c) = framing::suggest_crop(&req) else {
                // no better framing, but a straighten alone may still be worth proposing
                let mut d = (*inp.develop).clone();
                if angle.abs() >= 0.1 {
                    d.crop.geometry = lightcraft_geom::crop_fit_angle(w, h, angle, aspect);
                    if let Some(a) = opts.aspect {
                        d.crop.aspect = Some(a);
                    }
                }
                let proposed = (d != *inp.develop).then_some(d);
                return FrameRow {
                    id: inp.id,
                    before: inp.develop.clone(),
                    report: head(json!({"suggestion": null, "angle": angle, "reason": "the framing is already good"})),
                    proposed,
                };
            };
            let mut d = (*inp.develop).clone();
            d.crop.geometry = CropGeometry { rect: Rect::new(c.rect[0], c.rect[1], c.rect[2], c.rect[3]), angle: c.angle };
            if let Some(a) = opts.aspect {
                d.crop.aspect = Some(a);
            }
            let proposed = (d != *inp.develop).then_some(d);
            FrameRow {
                id: inp.id,
                before: inp.develop.clone(),
                report: head(
                    json!({"suggestion": {"rect": c.rect, "angle": c.angle, "kept": c.kept, "score": c.score, "scoreFull": c.score_full, "reasons": c.reasons},
                    "straightenConfidence": st.as_ref().map(|x| x.confidence)}),
                ),
                proposed,
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn launch_frame(job: Arc<Mutex<FrameJob>>, inputs: Vec<FrameInput>, opts: FrameOptions) -> std::result::Result<(), String> {
    let (progress, cancel) = {
        let j = lock(&job);
        (j.progress.clone(), j.cancel.clone())
    };
    std::thread::Builder::new()
        .name("style-frame".into())
        .spawn(move || {
            use rayon::prelude::*;
            let outcome = crate::guard::catch("framing suggestion", || {
                let rows: Vec<Option<FrameRow>> = inputs
                    .par_iter()
                    .map(|inp| {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                        let r = frame_one(inp, &opts);
                        progress.fetch_add(1, Ordering::Relaxed);
                        Some(r)
                    })
                    .collect();
                if cancel.load(Ordering::Relaxed) || rows.iter().any(Option::is_none) {
                    None
                } else {
                    Some(rows.into_iter().flatten().collect::<Vec<_>>())
                }
            });
            let mut j = lock(&job);
            j.elapsed = Some(j.started.elapsed());
            match outcome {
                Ok(Some(rows)) => {
                    j.rows = rows;
                    j.phase = Phase::Done;
                }
                Ok(None) => j.phase = Phase::Cancelled,
                Err(e) => j.phase = Phase::Failed(e),
            }
        })
        .map(|_| ())
        .map_err(|e| format!("can't start the framing worker: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn launch_frame(_: Arc<Mutex<FrameJob>>, _: Vec<FrameInput>, _: FrameOptions) -> std::result::Result<(), String> {
    Err("framing suggestions need the desktop app or the CLI".into())
}

fn frame_status(s: &Session) -> Value {
    let Some(job) = &s.style.frame else { return json!({"state": "idle"}) };
    let j = lock(job);
    let done = j.progress.load(Ordering::Relaxed).min(j.total);
    let elapsed = j.elapsed.unwrap_or_else(|| j.started.elapsed());
    let mut out = json!({
        "state": if j.applied { "applied" } else { phase_name(&j.phase) },
        "kind": if j.kind == FrameKind::Crop { "crop" } else { "straighten" },
        "total": j.total, "done": done,
        "percent": if j.total == 0 { 100.0 } else { (done as f64 / j.total as f64 * 1000.0).round() / 10.0 },
        "elapsedMs": elapsed.as_millis() as u64,
        "suggestions": j.rows.iter().filter(|r| r.proposed.is_some()).count(),
        "photos": j.rows.iter().map(|r| r.report.clone()).collect::<Vec<_>>(),
    });
    if let (Phase::Failed(e), Some(o)) = (&j.phase, out.as_object_mut()) {
        o.insert("error".into(), json!(e));
    }
    out
}

fn suggest_frames(s: &mut Session, p: &Value, kind: FrameKind) -> Result<Value> {
    let c = if kind == FrameKind::Crop { "autocrop.suggest" } else { "autostraighten.suggest" };
    if s.style.frame.as_ref().is_some_and(|j| lock(j).phase == Phase::Running) {
        return Err(bad(c, "a framing run is already going (style.status, style.cancel)"));
    }
    let wait = flag(p, "wait", false, c)?;
    let aspect = parse_aspect(p.get("aspect"), c)?;
    let straighten_first = flag(p, "straighten", true, c)?;
    let min_confidence = unit(p, "minConfidence", 0.6, c)?;
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() || p.get("id").is_some() { checked_targets(s, c, p)? } else { s.active().into_iter().collect() };
    if ids.is_empty() {
        return Err(bad(c, "no photo selected: pass id or ids"));
    }
    if ids.len() > LIMIT {
        return Err(bad(c, format!("at most {LIMIT} photos per run")));
    }
    let mut inputs = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        if !is_image(&ph) {
            continue;
        }
        let source = s.media.source_ref_uncached(&ph, SourceLevel::Preview);
        inputs.push(FrameInput {
            id,
            name: ph.file_name.clone(),
            source,
            header: s.source_info(id),
            develop: ph.develop.clone(),
            faces: ph.faces().map(|f| f.iter().map(|x| x.rect).collect()).unwrap_or_default(),
        });
    }
    if inputs.is_empty() {
        return Err(bad(c, "no photos to analyse (videos and deleted photos are skipped)"));
    }
    let job = Arc::new(Mutex::new(FrameJob {
        kind,
        phase: Phase::Running,
        total: inputs.len(),
        started: Instant::now(),
        elapsed: None,
        progress: Arc::new(AtomicUsize::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
        generation: s.library_generation(),
        rows: Vec::new(),
        applied: false,
    }));
    launch_frame(job.clone(), inputs, FrameOptions { kind, aspect, straighten_first, min_confidence }).map_err(|e| bad(c, e))?;
    s.style.frame = Some(job.clone());
    if wait {
        let deadline = Instant::now() + WAIT;
        while lock(&job).phase == Phase::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(frame_status(s))
}

fn apply_frames(s: &mut Session, p: &Value, kind: FrameKind) -> Result<Value> {
    let c = if kind == FrameKind::Crop { "autocrop.apply" } else { "autostraighten.apply" };
    if !flag(p, "confirm", false, c)? {
        return Err(bad(c, "this changes photos: pass confirm: true after looking at the suggestion"));
    }
    let shared = s.style.frame.clone().ok_or_else(|| bad(c, "no suggestion: run the matching .suggest first"))?;
    let label = if kind == FrameKind::Crop { "Apply suggested crop" } else { "Apply suggested straighten" };
    let (ops, changed, skipped) = {
        let st = lock(&shared);
        if st.kind != kind {
            return Err(bad(c, "the last suggestion is of the other kind"));
        }
        if st.phase != Phase::Done || st.applied {
            return Err(bad(c, "finish and review an unapplied suggestion first"));
        }
        if st.generation != s.library_generation() {
            return Err(bad(c, "the library changed; make a new suggestion"));
        }
        let only: Option<Vec<u64>> = match p.get("ids") {
            Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_u64).collect()),
            _ => None,
        };
        let (mut ops, mut skipped) = (Vec::new(), 0usize);
        for row in &st.rows {
            let Some(d) = &row.proposed else { continue };
            if only.as_ref().is_some_and(|o| !o.contains(&row.id.0)) {
                continue;
            }
            if s.catalog.photo(row.id).is_none_or(|ph| ph.deleted || ph.develop != row.before) {
                skipped += 1;
                continue;
            }
            ops.extend(s.develop_op(row.id, d.clone(), label));
        }
        let n = ops.len();
        (ops, n, skipped)
    };
    if changed > 0 {
        s.commit(label, Op::Batch { ops })?;
    }
    lock(&shared).applied = true;
    Ok(json!({"changed": changed, "skippedEditedSincePreview": skipped}))
}

// ------------------------------------------------------------------------------------------------
// status, list, cancel, delete

fn status(s: &mut Session) -> Result<Value> {
    s.style_poll();
    Ok(json!({"train": train_status(s), "apply": plan_status(s), "framing": frame_status(s)}))
}

fn list(s: &mut Session) -> Result<Value> {
    s.style_poll();
    let all = profiles(s);
    let rows: Vec<Value> = all.values().map(|m| profile_row(m, m.to_json().len())).collect();
    // how many photos could teach a style today, so the empty state can say what is missing
    // (a photo counts when one of the learnable sliders is really moved: imported defaults do not)
    let edited = s.catalog.photos().filter(|p| is_image(p) && moves_sliders(&p.develop)).count();
    Ok(
        json!({"profiles": rows, "editedPhotos": edited, "sliders": targets::TARGETS.iter().map(|t| json!({"id": t.id, "label": t.label})).collect::<Vec<_>>()}),
    )
}

fn cancel(s: &mut Session) -> Result<Value> {
    if let Some(j) = &s.style.train {
        lock(j).cancel.store(true, Ordering::Relaxed);
    }
    if let Some(j) = &s.style.plan {
        lock(j).cancel.store(true, Ordering::Relaxed);
    }
    if let Some(j) = &s.style.frame {
        lock(j).cancel.store(true, Ordering::Relaxed);
    }
    Ok(json!({"cancelling": true}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "style.delete";
    let name = name_of(p, C)?;
    let existed_in_memory = s.style.memory.remove(&name).is_some();
    let mut removed = existed_in_memory;
    if let Some(dir) = style_dir(s) {
        let path = dir.join(format!("{}.json", file_stem(&name)));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| bad(C, format!("can't delete the profile: {e}")))?;
            removed = true;
        }
    }
    if !removed {
        return Err(bad(C, format!("no style profile named \"{name}\"")));
    }
    Ok(json!({"deleted": name}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "style.train",
            "Learn My Style",
            [],
            None,
            "{name?: \"Mon style\", ids? (default: every edited photo of the library whose original is there), minPhotos?: 4.. (30), folds?: 2..10 (5), wait?: bool (false)} — learn how you edit from the photos you already edited (their develop settings, e.g. imported from a Lightroom catalogue): exposure, white balance, contrast, highlights, shadows, whites, blacks, vibrance, saturation, tone curve. Runs on a worker thread with progress; measures on photos held out (whole shoots) against \"no adjustment\" and \"median adjustment\" and stores those numbers with the profile (<library>/style/<name>.json). Nothing is read from the network, no photo is changed → {state, percent, profile: {samples, shoots, learned, skillVsNone, skillVsMedian, sliders: [...]}}",
            always,
            train
        ),
        cmd!(
            "style.apply",
            "Apply My Style",
            [],
            None,
            "{mode?: \"preview\" (default: compute proposals on a worker, nothing changes) | \"apply\" (commit the last preview, needs confirm: true: ONE undo step) | \"undo\", name?, ids? (default: selection), strength?: 0..1 (1), consistency?: 0..1 (0.7: photos of one time-and-place series share the style, adapted to each photo's exposure), only?: [slider ids], overwrite?: bool (false: sliders you already moved are kept), wait?: bool (false), confirm?: bool} → status with one row per photo {sliders: [{id, value}], alreadyEdited, changes}",
            always,
            apply
        ),
        cmd!(
            query "style.status",
            "My Style Status",
            [],
            None,
            "{} → {train: {...}, apply: {...}, framing: {...}} progress of the running or last jobs; read-only",
            always,
            |s, _| status(s)
        ),
        cmd!(
            query "style.list",
            "My Style Profiles",
            [],
            None,
            "{} → {profiles: [{name, samples, shoots, learned, skillVsNone, skillVsMedian, bytes, sliders}], editedPhotos, sliders}",
            always,
            |s, _| list(s)
        ),
        cmd!("style.cancel", "Stop My Style", [], None, "{} → stop the running learning, preview or framing run", always, |s, _| cancel(s)),
        cmd!("style.delete", "Delete Style Profile", [], None, "{name} → remove a profile", always, delete),
        cmd!(
            "autostraighten.suggest",
            "Suggest Straighten",
            [],
            None,
            "{id?, ids?, minConfidence?: 0..1 (0.6), wait?: bool} — detect the horizon or plumb lines and propose a rotation; read-only (the interface shows it as a preview) → status with {suggestion: {angle, confidence, kind}} per photo",
            always,
            |s, p| suggest_frames(s, p, FrameKind::Straighten)
        ),
        cmd!(
            "autostraighten.apply",
            "Apply Suggested Straighten",
            [],
            None,
            "{confirm: true, ids?} — the only command that writes what autostraighten.suggest proposed; ONE undo step → {changed}",
            always,
            |s, p| apply_frames(s, p, FrameKind::Straighten)
        ),
        cmd!(
            "autocrop.suggest",
            "Suggest Crop",
            [],
            None,
            "{id?, ids?, aspect?: \"original\"|\"WxH\"|[w,h], straighten?: bool (true: the crop includes the straighten angle), minConfidence?: 0..1 (0.6), wait?: bool} — propose a crop from faces (Analyze Faces) or the salient subject: subject kept whole, near a third, horizon on a third; none when the framing is already good; read-only → status with {suggestion: {rect, angle, kept, reasons}} per photo",
            always,
            |s, p| suggest_frames(s, p, FrameKind::Crop)
        ),
        cmd!(
            "autocrop.apply",
            "Apply Suggested Crop",
            [],
            None,
            "{confirm: true, ids?} — the only command that writes what autocrop.suggest proposed; ONE undo step → {changed}",
            always,
            |s, p| apply_frames(s, p, FrameKind::Crop)
        ),
    ]
}

/// Split photos into time-and-place series (gap of at most 15 minutes, same place, near GPS);
/// a missing capture time stays alone. Same rule as the series grouping of the UX base.
fn group_photos(mut photos: Vec<Arc<Photo>>) -> Vec<Vec<PhotoId>> {
    photos.sort_by_key(|p| (p.captured.as_deref().and_then(lightcraft_catalog::dates::iso_seconds), p.id));
    let mut out: Vec<Vec<PhotoId>> = Vec::new();
    let mut previous: Option<Arc<Photo>> = None;
    for photo in photos {
        let same = previous.as_ref().is_some_and(|prev| {
            let time = prev
                .captured
                .as_deref()
                .and_then(lightcraft_catalog::dates::iso_seconds)
                .zip(photo.captured.as_deref().and_then(lightcraft_catalog::dates::iso_seconds))
                .is_some_and(|(a, b)| b >= a && b - a <= 900);
            let places = prev.meta.location.trim().eq_ignore_ascii_case(photo.meta.location.trim())
                && prev.meta.city.trim().eq_ignore_ascii_case(photo.meta.city.trim());
            let gps = match (prev.meta.gps, photo.meta.gps) {
                (Some((a, b)), Some((c, d))) => [a, b, c, d].iter().all(|v| v.is_finite()) && (a - c).abs() < 0.002 && (b - d).abs() < 0.002,
                _ => true,
            };
            time && places && gps
        });
        if same {
            if let Some(g) = out.last_mut() {
                g.push(photo.id);
            }
        } else {
            out.push(vec![photo.id]);
        }
        previous = Some(photo);
    }
    out
}

/// The photos a command targets (`ids`, `id` or the selection): at least one, all present, no duplicates.
fn checked_targets(s: &Session, command: &str, p: &Value) -> Result<Vec<PhotoId>> {
    if p.get("id").is_some_and(|v| !v.is_null()) && p.get("ids").is_some_and(|v| !v.is_null()) {
        return Err(bad(command, "give either id or ids, not both"));
    }
    let ids = s.targets(p).map_err(|e| bad(command, e.to_string()))?;
    if ids.is_empty() {
        return Err(bad(command, "no target photos"));
    }
    if let Some(id) = ids.iter().find(|id| s.catalog.photo(**id).is_none()) {
        return Err(bad(command, format!("no such photo {}", id.0)));
    }
    let mut seen = std::collections::HashSet::new();
    Ok(ids.into_iter().filter(|id| seen.insert(*id)).collect())
}
