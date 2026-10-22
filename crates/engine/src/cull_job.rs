//! AI culling (`cull.*`): one verdict per photo of an album — highlight / selected / review /
//! rejected, with reasons and proposed stars — judged by `lightcraft-culler` from measurements
//! taken on the original's thumbnail-level source (focus, clipping, exposure, a look-alike
//! signature) and from the faces Analyze Faces already found.
//!
//! The measuring runs on a worker thread (rayon inside), reports progress and can be cancelled;
//! the session never waits on it unless the caller passes `wait`. Verdicts are written to the
//! photos' analysis when the run finishes (a measurement, not an undo step, like faces and focus);
//! writing ratings and flags is the separate, undoable `cull.apply`. Originals are only read.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lightcraft_catalog::{Analysis, Flag, Op, Photo, PhotoId};
use lightcraft_culler::{Bucket, Face, Genre, Options, Shot, Verdict};
use serde_json::{Value, json};

use crate::cmd::cull::{Measurement, measurement_key};
use crate::cmd::{CommandSpec, always, bad, cmd};
use crate::media::{SourceLevel, SourceRef};
use crate::{Result, Session};

/// Photos one run accepts unless `limit` raises it (a whole 27 000-photo catalogue is not an album).
const DEFAULT_LIMIT: usize = 5000;
/// Longest a waiting command (CLI, MCP, tests) waits.
const WAIT: Duration = Duration::from_secs(2 * 60 * 60);
/// Entries the measurement cache keeps (same bound as Assisted Culling).
const CACHE_MAX: usize = 1024;

/// The user's settings and the job in progress or the last one.
pub struct Culler {
    job: Option<Arc<Mutex<Job>>>,
    /// Defaults of `cull.run` (set with `cull.setGenre`).
    pub genre: Genre,
    pub grouping: f32,
    pub key_people: Vec<String>,
}

impl Default for Culler {
    fn default() -> Self {
        Culler { job: None, genre: Genre::General, grouping: 0.5, key_people: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Measuring,
    Done,
    Cancelled,
    Failed(String),
}

struct Job {
    phase: Phase,
    total: usize,
    options: Options,
    started: Instant,
    elapsed: Option<Duration>,
    failed: Vec<(u64, String)>,
    reused: usize,
    measured: usize,
    faces_missing: usize,
    /// Verdicts waiting to be written to the catalogue.
    results: Vec<(PhotoId, Verdict)>,
    fresh: Vec<(String, Measurement)>,
    stored: bool,
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
}

fn lock(j: &Arc<Mutex<Job>>) -> MutexGuard<'_, Job> {
    j.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the worker needs for one photo; built on the session thread.
struct Input {
    id: PhotoId,
    time: Option<i64>,
    aspect: f32,
    faces: Option<Vec<Face>>,
    source: SourceRef,
    basis: String,
    cached: Option<Measurement>,
}

fn measure(source: &SourceRef) -> std::result::Result<Measurement, String> {
    use lightcraft_pipeline::cull;
    let img = source.load()?;
    Ok(Measurement {
        sharpness: cull::sharpness(&img),
        clipped: cull::clipped(&img),
        signature: cull::signature(&img).to_vec(),
        exposure: Some(cull::exposure(&img)),
        color: Some(color_layout(&img)),
    })
}

/// Chroma (r, g shares) of the mean colour of each cell of a 4 × 4 grid, centred and unit norm: what the
/// luminance-only signature cannot tell (two dark plates on a black table differ by their colours).
fn color_layout(img: &lightcraft_raster::Rgb32f) -> Vec<f32> {
    const N: usize = 4;
    let (w, h) = (img.width.max(1), img.height.max(1));
    let mut out = Vec::with_capacity(2 * N * N);
    for cy in 0..N {
        for cx in 0..N {
            let (x0, x1, y0, y1) =
                (cx * w / N, ((cx + 1) * w / N).max(cx * w / N + 1).min(w), cy * h / N, ((cy + 1) * h / N).max(cy * h / N + 1).min(h));
            let (mut r, mut g, mut b, mut n) = (0f64, 0f64, 0f64, 0f64);
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some(c) = img.data.get(y * w + x) {
                        r += c[0] as f64;
                        g += c[1] as f64;
                        b += c[2] as f64;
                        n += 1.0;
                    }
                }
            }
            let s = (r + g + b).max(1e-6 * n.max(1.0));
            out.push((r / s) as f32);
            out.push((g / s) as f32);
        }
    }
    let mean = out.iter().sum::<f32>() / out.len().max(1) as f32;
    out.iter_mut().for_each(|v| *v -= mean);
    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
    out.iter_mut().for_each(|v| *v /= norm);
    out
}

/// The faces of a photo as the culler wants them; a face takes the name of the face region whose
/// centre falls in it (nearest centre wins).
fn faces_of(p: &Photo) -> Option<Vec<Face>> {
    let list = p.faces()?;
    let named: Vec<(f32, f32, &str)> = p
        .meta
        .regions
        .iter()
        .filter(|r| matches!(r.kind, lightcraft_meta::RegionKind::Face))
        .filter_map(|r| Some((((r.rect.x0 + r.rect.x1) / 2.0) as f32, ((r.rect.y0 + r.rect.y1) / 2.0) as f32, r.name.as_deref()?)))
        .collect();
    Some(
        list.iter()
            .map(|f| {
                let (cx, cy) = ((f.rect[0] + f.rect[2]) / 2.0, (f.rect[1] + f.rect[3]) / 2.0);
                let person = named
                    .iter()
                    .filter(|(x, y, _)| *x >= f.rect[0] && *x <= f.rect[2] && *y >= f.rect[1] && *y <= f.rect[3])
                    .min_by(|a, b| ((a.0 - cx).powi(2) + (a.1 - cy).powi(2)).total_cmp(&((b.0 - cx).powi(2) + (b.1 - cy).powi(2))))
                    .map(|n| n.2.to_string());
                Face { rect: f.rect, open: f.open, sharp: f.sharp, judged: f.eyes_judged(), person }
            })
            .collect(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn launch(job: Arc<Mutex<Job>>, inputs: Vec<Input>) -> std::result::Result<(), String> {
    let (progress, cancel, options) = {
        let j = lock(&job);
        (j.progress.clone(), j.cancel.clone(), j.options.clone())
    };
    std::thread::Builder::new()
        .name("cull".into())
        .spawn(move || {
            use rayon::prelude::*;
            let outcome = crate::guard::catch("AI culling", || {
                let rows: Vec<Option<(bool, std::result::Result<Measurement, String>)>> = inputs
                    .par_iter()
                    .map(|inp| {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                        let r = match &inp.cached {
                            Some(m) => (true, Ok(m.clone())),
                            None => (false, measure(&inp.source)),
                        };
                        progress.fetch_add(1, Ordering::Relaxed);
                        Some(r)
                    })
                    .collect();
                if cancel.load(Ordering::Relaxed) || rows.iter().any(Option::is_none) {
                    return None;
                }
                let (mut shots, mut ids, mut keys) = (Vec::new(), Vec::new(), Vec::new());
                let (mut failed, mut reused, mut measured, mut fresh) = (Vec::new(), 0usize, 0usize, Vec::new());
                let mut faces_missing = 0usize;
                for (inp, row) in inputs.iter().zip(rows) {
                    match row {
                        Some((was_cached, Ok(m))) => {
                            if was_cached {
                                reused += 1;
                            } else {
                                measured += 1;
                                fresh.push((inp.basis.clone(), m.clone()));
                            }
                            faces_missing += inp.faces.is_none() as usize;
                            shots.push(Shot {
                                id: inp.id.0,
                                aspect: inp.aspect,
                                time: inp.time,
                                sharpness: m.sharpness,
                                clipped: m.clipped,
                                exposure: m.exposure.unwrap_or([0.45, 0.1, 0.9]),
                                color: m.color.unwrap_or_default(),
                                signature: m.signature,
                                faces: inp.faces.clone(),
                                taste: None,
                            });
                            ids.push(inp.id);
                            keys.push(inp.basis.clone());
                        }
                        Some((_, Err(e))) => failed.push((inp.id.0, e)),
                        None => {}
                    }
                }
                let verdicts = lightcraft_culler::judge(&shots, &options);
                let results: Vec<(PhotoId, Verdict)> = ids
                    .into_iter()
                    .zip(verdicts)
                    .zip(keys)
                    .map(|((id, mut v), key)| {
                        v.basis = key;
                        (id, v)
                    })
                    .collect();
                Some((results, failed, reused, measured, fresh, faces_missing))
            });
            let mut j = lock(&job);
            j.elapsed = Some(j.started.elapsed());
            match outcome {
                Ok(Some((results, failed, reused, measured, fresh, faces_missing))) => {
                    j.results = results;
                    j.failed = failed;
                    j.reused = reused;
                    j.measured = measured;
                    j.fresh = fresh;
                    j.faces_missing = faces_missing;
                    j.phase = Phase::Done;
                }
                Ok(None) => j.phase = Phase::Cancelled,
                Err(e) => j.phase = Phase::Failed(e),
            }
        })
        .map(|_| ())
        .map_err(|e| format!("can't start the culling worker: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn launch(_: Arc<Mutex<Job>>, _: Vec<Input>) -> std::result::Result<(), String> {
    Err("AI culling needs the desktop app or the CLI".into())
}

impl Session {
    /// Write the verdicts of a finished run to the photos (not an undo step: a measurement).
    /// Called by every `cull.*` command; the desktop app can call it on its idle tick. Returns
    /// whether some photo changed.
    pub fn cull_poll(&mut self) -> bool {
        let Some(job) = self.cull.job.clone() else { return false };
        let (results, fresh) = {
            let mut j = lock(&job);
            if j.phase != Phase::Done || j.stored {
                return false;
            }
            j.stored = true;
            (std::mem::take(&mut j.results), std::mem::take(&mut j.fresh))
        };
        let mut changed = false;
        for (id, verdict) in results {
            let Some(p) = self.catalog.photo(id) else { continue };
            // the file changed while the run was going: the verdict would describe something else
            if !verdict.basis.is_empty() && measurement_key(self, id).as_deref() != Some(verdict.basis.as_str()) {
                continue;
            }
            let verdict = Some(Arc::new(verdict));
            let analysis = match &p.analysis {
                Some(a) => Analysis { verdict, ..a.clone() },
                None => Analysis { verdict, ..Default::default() },
            };
            let op = Op::SetAnalysis { id, analysis: Some(analysis) };
            if self.catalog.apply(op.clone()).is_ok() {
                self.pending_log.push(op);
                changed = true;
            }
        }
        for (key, m) in fresh {
            if self.culling_cache.len() >= CACHE_MAX {
                self.culling_cache.pop_first();
            }
            self.culling_cache.insert(key, m);
        }
        changed
    }
}

fn status_json(s: &Session) -> Value {
    let Some(job) = &s.cull.job else {
        return json!({"state": "idle", "genre": s.cull.genre.name(), "grouping": s.cull.grouping, "keyPeople": s.cull.key_people});
    };
    let j = lock(job);
    let done = j.progress.load(Ordering::Relaxed).min(j.total);
    let elapsed = j.elapsed.unwrap_or_else(|| j.started.elapsed());
    let secs = elapsed.as_secs_f64().max(1e-6);
    let mut out = json!({
        "state": match &j.phase { Phase::Measuring => "running", Phase::Done => "done", Phase::Cancelled => "cancelled", Phase::Failed(_) => "failed" },
        "total": j.total, "done": done, "percent": if j.total == 0 { 100.0 } else { (done as f64 / j.total as f64 * 1000.0).round() / 10.0 },
        "elapsedMs": (secs * 1000.0).round(), "photosPerSecond": ((done as f64 / secs) * 10.0).round() / 10.0,
        "genre": j.options.profile.genre.name(), "grouping": j.options.grouping, "keyPeople": j.options.key_people,
        "reused": j.reused, "measured": j.measured, "facesNotAnalyzed": j.faces_missing,
        "failed": j.failed.iter().map(|(id, e)| json!([id, e])).collect::<Vec<_>>(),
    });
    if let (Phase::Failed(e), Some(o)) = (&j.phase, out.as_object_mut()) {
        o.insert("error".into(), json!(e));
    }
    out
}

fn counts<'a>(verdicts: impl Iterator<Item = &'a Verdict>) -> Value {
    let (mut hi, mut sel, mut rev, mut rej) = (0, 0, 0, 0);
    let (mut scenes, mut groups) = (std::collections::HashSet::new(), std::collections::HashSet::new());
    for v in verdicts {
        match v.bucket {
            Bucket::Highlight => hi += 1,
            Bucket::Selected => sel += 1,
            Bucket::Review => rev += 1,
            Bucket::Rejected => rej += 1,
        }
        scenes.insert(v.scene);
        if let Some(g) = v.group {
            groups.insert((v.scene, g));
        }
    }
    json!({"highlight": hi, "selected": sel, "review": rev, "rejected": rej, "scenes": scenes.len(), "duplicateGroups": groups.len()})
}

fn row(s: &Session, id: PhotoId, v: &Verdict) -> Value {
    let p = s.catalog.photo(id);
    json!({"id": id.0, "name": p.map(|p| p.file_name.as_str()), "captured": p.and_then(|p| p.captured.as_deref()),
        "bucket": v.bucket.name(), "stars": v.stars, "score": v.score, "scene": v.scene, "group": v.group, "groupRank": v.group_rank, "groupSize": v.group_size,
        "reasons": v.reasons, "flags": v.flags()})
}

fn strings(p: &Value, key: &str, c: &str) -> Result<Option<Vec<String>>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(c, format!("{key} must be an array of strings"))))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(_) => Err(bad(c, format!("{key} must be an array of strings"))),
    }
}

fn options_from(s: &Session, p: &Value, c: &str) -> Result<Options> {
    let genre = match p.get("genre") {
        None | Some(Value::Null) => s.cull.genre,
        Some(v) => v.as_str().and_then(Genre::parse).ok_or_else(|| bad(c, "genre must be general, wedding, portrait, school or sport"))?,
    };
    let grouping = match p.get("grouping") {
        None | Some(Value::Null) => s.cull.grouping,
        Some(v) => v.as_f64().filter(|g| g.is_finite() && (0.0..=1.0).contains(g)).ok_or_else(|| bad(c, "grouping must be a number 0..1"))? as f32,
    };
    let mut o = Options::for_genre(genre);
    o.grouping = grouping;
    o.key_people = strings(p, "keyPeople", c)?.unwrap_or_else(|| s.cull.key_people.clone());
    Ok(o)
}

fn flag(p: &Value, key: &str, default: bool, c: &str) -> Result<bool> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_bool().ok_or_else(|| bad(c, format!("{key} must be a boolean"))),
    }
}

fn targets(s: &mut Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.checked_targets(c, p) } else { Ok(s.visible_cloned()) }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cull.run";
    s.cull_poll();
    if s.cull.job.as_ref().is_some_and(|j| lock(j).phase == Phase::Measuring) {
        return Err(bad(C, "a culling run is already in progress (cull.status, cull.cancel)"));
    }
    let options = options_from(s, p, C)?;
    let (force, wait, faces) = (flag(p, "force", false, C)?, flag(p, "wait", false, C)?, flag(p, "faces", false, C)?);
    let limit = match p.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(v) => v.as_u64().filter(|n| *n > 0).ok_or_else(|| bad(C, "limit must be a positive integer"))? as usize,
    };
    let ids: Vec<PhotoId> = targets(s, p, C)?
        .into_iter()
        .filter(|id| {
            s.catalog.photo(*id).is_some_and(|p| matches!(p.kind, lightcraft_catalog::MediaKind::Image | lightcraft_catalog::MediaKind::Raw))
        })
        .collect();
    if ids.is_empty() {
        return Err(bad(C, "no photos to cull (select an album or pass ids)"));
    }
    if ids.len() > limit {
        return Err(bad(C, format!("{} photos is more than one album (limit {limit}); pass ids for an album or a larger limit", ids.len())));
    }
    // faces: the ones Analyze Faces already found are always used; `faces: true` finds the rest
    if faces {
        let (queued, _) = s.facemesh_enqueue(&ids, false).map_err(|e| bad(C, format!("{e} (faces need the model: faces.model.download)")))?;
        if s.facemesh.background {
            // the verdicts would not see the faces yet; run again once they are in
        } else {
            s.faces_wait(queued);
        }
    }
    let mut inputs = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        let basis = measurement_key(s, id).unwrap_or_default();
        let cached = (!force)
            .then(|| s.culling_cache.get(&basis).filter(|m| m.valid() && m.exposure.is_some() && m.color.is_some()).cloned())
            .flatten()
            .filter(|_| !basis.is_empty());
        let source = s.media.origin_ref(&ph.source, SourceLevel::Thumb.max_edge());
        inputs.push(Input {
            id,
            aspect: if ph.height > 0 { ph.width as f32 / ph.height as f32 } else { 0.0 },
            time: ph.captured.as_deref().and_then(lightcraft_catalog::dates::iso_seconds),
            faces: faces_of(&ph),
            source,
            basis,
            cached,
        });
    }
    let job = Arc::new(Mutex::new(Job {
        phase: Phase::Measuring,
        total: inputs.len(),
        options,
        started: Instant::now(),
        elapsed: None,
        failed: Vec::new(),
        reused: 0,
        measured: 0,
        faces_missing: 0,
        results: Vec::new(),
        fresh: Vec::new(),
        stored: false,
        progress: Arc::new(AtomicUsize::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
    }));
    launch(job.clone(), inputs).map_err(|e| bad(C, e))?;
    s.cull.job = Some(job.clone());
    if wait {
        let deadline = Instant::now() + WAIT;
        while lock(&job).phase == Phase::Measuring && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        s.cull_poll();
        let mut st = status_json(s);
        if let Some(o) = st.as_object_mut() {
            o.insert("summary".into(), summary(s));
        }
        return Ok(st);
    }
    Ok(status_json(s))
}

/// Bucket counts of the verdicts stored on the library's photos.
fn summary(s: &Session) -> Value {
    counts(s.catalog.photos().filter_map(|p| p.analysis.as_ref().and_then(|a| a.verdict.as_deref())))
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    s.cull_poll();
    let mut st = status_json(s);
    if let Some(o) = st.as_object_mut()
        && st_done(&s.cull)
    {
        o.insert("summary".into(), summary(s));
    }
    Ok(st)
}

fn st_done(c: &Culler) -> bool {
    c.job.as_ref().is_some_and(|j| lock(j).phase == Phase::Done)
}

fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    if let Some(j) = &s.cull.job {
        lock(j).cancel.store(true, Ordering::Relaxed);
    }
    // the worker notices between two photos; give it a moment so the answer is usually final
    let deadline = Instant::now() + Duration::from_millis(500);
    while s.cull.job.as_ref().is_some_and(|j| lock(j).phase == Phase::Measuring) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(status_json(s))
}

fn verdict_of(s: &Session, id: PhotoId) -> Option<Arc<Verdict>> {
    s.catalog.photo(id).and_then(|p| p.analysis.as_ref()).and_then(|a| a.verdict.clone())
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cull.list";
    s.cull_poll();
    let ids = targets(s, p, C)?;
    let buckets = strings(p, "bucket", C)?;
    if let Some(b) = &buckets
        && let Some(bad_one) = b.iter().find(|x| !["highlight", "selected", "review", "rejected"].contains(&x.as_str()))
    {
        return Err(bad(C, format!("unknown bucket {bad_one} (highlight, selected, review, rejected)")));
    }
    let reason = match p.get("reason").and_then(Value::as_str) {
        None => None,
        Some(r) => Some(serde_json::from_value::<lightcraft_culler::Reason>(json!(r)).map_err(|_| bad(C, format!("unknown reason {r}")))?),
    };
    let min_stars = p.get("minStars").and_then(Value::as_u64).unwrap_or(0) as u8;
    let sort = p.get("sort").and_then(Value::as_str).unwrap_or("scene");
    if !["scene", "score", "time"].contains(&sort) {
        return Err(bad(C, "sort must be scene, score or time"));
    }
    let mut rows: Vec<(PhotoId, Arc<Verdict>, Option<i64>)> = ids
        .iter()
        .filter_map(|id| {
            Some((
                *id,
                verdict_of(s, *id)?,
                s.catalog.photo(*id).and_then(|p| p.captured.as_deref().and_then(lightcraft_catalog::dates::iso_seconds)),
            ))
        })
        .collect();
    let judged = rows.len();
    let all_counts = counts(rows.iter().map(|r| &*r.1));
    rows.retain(|(_, v, _)| {
        buckets.as_ref().is_none_or(|b| b.iter().any(|x| x == v.bucket.name()))
            && reason.is_none_or(|r| v.reasons.contains(&r))
            && v.stars >= min_stars
            && p.get("scene").and_then(Value::as_u64).is_none_or(|n| v.scene as u64 == n)
            && p.get("group").and_then(Value::as_u64).is_none_or(|n| v.group.map(u64::from) == Some(n))
    });
    match sort {
        "score" => rows.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0))),
        "time" => rows.sort_by_key(|r| (r.2.unwrap_or(i64::MAX), r.0)),
        _ => rows.sort_by_key(|r| (r.1.scene, r.2.unwrap_or(i64::MAX), r.0)),
    }
    // scenes of the listing, chronological
    let mut scenes: std::collections::BTreeMap<u32, (usize, Option<i64>, Option<i64>, [usize; 4])> = Default::default();
    for (_, v, t) in &rows {
        let e = scenes.entry(v.scene).or_insert((0, None, None, [0; 4]));
        e.0 += 1;
        e.1 = match (e.1, *t) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        e.2 = match (e.2, *t) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        e.3[match v.bucket {
            Bucket::Highlight => 0,
            Bucket::Selected => 1,
            Bucket::Review => 2,
            Bucket::Rejected => 3,
        }] += 1;
    }
    let matched = rows.len();
    let offset = p.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize;
    if flag(p, "filter", false, C)? {
        s.filter.only = rows.iter().map(|r| r.0).collect();
    }
    let page: Vec<Value> = rows.iter().skip(offset).take(limit).map(|(id, v, _)| row(s, *id, v)).collect();
    Ok(json!({
        "photos": page, "matched": matched, "judged": judged, "unjudged": ids.len() - judged, "counts": all_counts, "sort": sort,
        "scenes": scenes.iter().map(|(n, (count, from, to, b))| json!({"scene": n, "count": count, "from": from, "to": to,
            "highlight": b[0], "selected": b[1], "review": b[2], "rejected": b[3]})).collect::<Vec<_>>(),
    }))
}

fn explain(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cull.explain";
    s.cull_poll();
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(C, "no photo (pass id)"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(C, format!("no such photo {}", id.0)))?;
    let v = verdict_of(s, id).ok_or_else(|| bad(C, "this photo has no verdict yet (cull.run)"))?;
    let profile = lightcraft_culler::Profile::for_genre(v.genre);
    let dup_reject = profile.dup_reject;
    let lines: Vec<Value> = v
        .reasons
        .iter()
        .map(|r| json!({"reason": r, "flag": r.flag(), "severity": Verdict::severity_of(*r, dup_reject), "text": r.text()}))
        .collect();
    let mates: Vec<Value> = match v.group {
        Some(g) => s
            .catalog
            .photos()
            .filter_map(|q| {
                let w = q.analysis.as_ref()?.verdict.as_deref()?;
                (w.scene == v.scene && w.group == Some(g) && q.id != id)
                    .then(|| json!({"id": q.id.0, "name": q.file_name, "score": w.score, "groupRank": w.group_rank, "bucket": w.bucket.name()}))
            })
            .collect(),
        None => Vec::new(),
    };
    let headline = match v.bucket {
        Bucket::Highlight => "a highlight of the shoot".to_string(),
        Bucket::Selected => "selected".to_string(),
        Bucket::Review => "to review".to_string(),
        Bucket::Rejected => "rejected".to_string(),
    };
    Ok(json!({
        "id": id.0, "name": ph.file_name, "verdict": headline, "bucket": v.bucket.name(), "stars": v.stars, "score": v.score, "genre": v.genre.name(),
        "reasons": lines, "flags": v.flags(), "scene": v.scene, "group": v.group, "groupRank": v.group_rank, "groupSize": v.group_size, "groupMates": mates,
        "faces": v.faces, "measurements": {"sharpness": v.sharpness, "clipped": v.clipped, "exposure": {"mean": v.exposure[0], "p5": v.exposure[1], "p95": v.exposure[2]}},
        "profile": profile, "rulesVersion": v.version,
        "note": "stars are a proposal: nothing is written to the rating or the flag until cull.apply",
    }))
}

fn set_genre(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cull.setGenre";
    if p.get("genre").is_some() || p.get("grouping").is_some() || p.get("keyPeople").is_some() {
        let o = options_from(s, p, C)?;
        s.cull.genre = o.profile.genre;
        s.cull.grouping = o.grouping;
        s.cull.key_people = o.key_people;
    }
    let o = {
        let mut o = Options::for_genre(s.cull.genre);
        o.grouping = s.cull.grouping;
        o
    };
    Ok(json!({
        "genre": s.cull.genre.name(), "grouping": s.cull.grouping, "duplicateSimilarity": o.dup_threshold(), "keyPeople": s.cull.key_people,
        "profile": o.profile, "genres": Genre::ALL.iter().map(|g| g.name()).collect::<Vec<_>>(),
        "note": "applies to the next cull.run (measurements are cached, so running again is quick)",
    }))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cull.apply";
    s.cull_poll();
    let (stars, flags, overwrite) = (flag(p, "stars", true, C)?, flag(p, "flags", true, C)?, flag(p, "overwrite", false, C)?);
    let buckets = strings(p, "bucket", C)?;
    let ids = targets(s, p, C)?;
    let (mut ops, mut rated, mut picked, mut rejected) = (Vec::new(), 0, 0, 0);
    for id in ids {
        let (Some(v), Some(ph)) = (verdict_of(s, id), s.catalog.photo(id)) else { continue };
        if buckets.as_ref().is_some_and(|b| !b.iter().any(|x| x == v.bucket.name())) {
            continue;
        }
        if stars && (overwrite || ph.rating == 0) && ph.rating != v.stars {
            ops.push(Op::SetRating { id, rating: v.stars });
            rated += 1;
        }
        // a worse duplicate is not a failure: it gets one star but no Reject flag
        let failed = v.reasons.iter().any(|r| *r != lightcraft_culler::Reason::Duplicate && Verdict::severity_of(*r, false) == lightcraft_culler::Severity::Reject);
        let want = match v.bucket {
            Bucket::Highlight => Flag::Pick,
            Bucket::Rejected if failed => Flag::Reject,
            _ => Flag::None,
        };
        if flags && want != Flag::None && (overwrite || ph.flag == Flag::None) && ph.flag != want {
            ops.push(Op::SetFlag { id, flag: want });
            if want == Flag::Pick {
                picked += 1;
            } else {
                rejected += 1;
            }
        }
    }
    if !ops.is_empty() {
        s.commit("Apply AI culling", Op::Batch { ops })?;
    }
    Ok(json!({"rated": rated, "picked": picked, "rejected": rejected}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "cull.run",
            "AI Culling",
            [],
            None,
            "{id?|ids?, genre?: general|wedding|portrait|school|sport, grouping?: 0..1 (0.5), keyPeople?: [names], faces?: bool, force?: bool, limit?: n (5000), wait?: bool} — judges every photo of the album (default: the visible photos): highlight / selected / review / rejected with reasons and proposed stars, duplicate groups, shooting scenes. Runs in the background with progress (cull.status) and can be cancelled (cull.cancel); wait blocks until done. Reads the originals, never writes them; writes verdicts to the analysis (no rating or flag: cull.apply) → status {state, total, done, percent, photosPerSecond, …}",
            always,
            run
        ),
        cmd!(query "cull.status", "AI Culling progress", [], None, "{} → {state: idle|running|done|cancelled|failed, total, done, percent, elapsedMs, photosPerSecond, reused, measured, facesNotAnalyzed, failed, summary?}", always, status),
        cmd!(query "cull.cancel", "Cancel AI Culling", [], None, "{} — stops the run between two photos; nothing is written for a cancelled run", always, cancel),
        cmd!(
            query "cull.list",
            "AI Culling results",
            [],
            None,
            "{id?|ids?, bucket?: [highlight|selected|review|rejected], reason?: closedEyes|blurred|faceBlurred|duplicate|underexposed|…, minStars?, scene?, group?, sort?: scene|score|time (scene), offset?, limit? (500), filter?: bool} → {photos: [{id, name, bucket, stars, score, scene, group, groupRank, reasons, flags}], scenes: [{scene, count, from, to, highlight, selected, review, rejected}], counts, matched, judged}; filter narrows the view to the matches",
            always,
            list
        ),
        cmd!(query "cull.explain", "Explain a verdict", [], None, "{id?} → why the photo got its verdict: reasons with severity and plain text, faces (eyes, blur), duplicate group mates, measurements, the genre profile's thresholds", always, explain),
        cmd!(query "cull.setGenre", "AI Culling genre", [], None, "{genre?, grouping?: 0..1, keyPeople?: [names]} — sets the defaults of cull.run (no params: reads them) → {genre, grouping, duplicateSimilarity, keyPeople, profile, genres}", always, set_genre),
        cmd!(
            "cull.apply",
            "Apply AI culling",
            [],
            None,
            "{id?|ids?, bucket?: […], stars?: true, flags?: true, overwrite?: false} — writes the proposed stars (only to unrated photos unless overwrite) and Pick / Reject flags (highlights / rejected for a technical failure — a worse duplicate gets its star but no Reject flag —, only to unflagged photos unless overwrite); one undo step → {rated, picked, rejected}",
            always,
            apply
        ),
    ]
}
