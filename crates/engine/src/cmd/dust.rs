//! Persistent sensor dust across a series (`dust.detect`, `dust.apply`): spots that sit at the
//! same place on most photos of a shoot are dust on the sensor; `dust.apply` puts a heal spot on
//! each of them on every photo of the series, in one undo step. (`spot.findDust` looks at one
//! photo; this looks at the whole series, which is what tells dust from scenery.)

use std::sync::{Mutex, PoisonError};

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use super::masks::{SPOT_SIZE, pick_source};
use super::{CommandSpec, bad, bool_or, cmd, f64_or};
use crate::dust::{Accumulator, DustSpot, Params};
use crate::{Result, Session};

/// Long edge of the renders dust is looked for in.
const EDGE: usize = 2048;
/// Photos looked at by default (evenly spaced over the series).
const SAMPLE: usize = 40;

struct Found {
    key: u64,
    dims: (usize, usize),
    analysed: usize,
    skipped: usize,
    spots: Vec<DustSpot>,
    coverage: f64,
}

static LAST: Mutex<Option<Found>> = Mutex::new(None);

fn series_key(ids: &[PhotoId], limit: usize, sens: u64) -> u64 {
    ids.iter().fold(limit as u64 ^ sens.rotate_left(17), |h, id| (h ^ id.0).wrapping_mul(0x100_0000_01b3))
}

/// The job that renders the photo's luminance plane at the series' size, without its own edits
/// (sensor geometry: no crop, no lens correction), display-encoded.
fn luma_job(s: &mut Session, id: PhotoId) -> Result<crate::RenderJob> {
    let settings = DevelopSettings::default();
    s.preview_job(id, EDGE, EDGE, false, &settings).ok_or_else(|| bad("dust.detect", format!("no such photo {}", id.0)))
}

fn luma_run(job: crate::RenderJob) -> Result<(Vec<f32>, usize, usize)> {
    let img = job.run().rendered.map_err(|e| bad("dust.detect", e))?.image;
    let l = img.data.iter().map(|p| (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0).collect();
    Ok((l, img.width, img.height))
}

/// The photo's luminance plane at the series' size.
fn luma(s: &mut Session, id: PhotoId) -> Result<(Vec<f32>, usize, usize)> {
    luma_run(luma_job(s, id)?)
}

fn ids_of(s: &mut Session, p: &Value) -> Vec<PhotoId> {
    if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { s.visible_cloned() }
}

fn params_for(sensitivity: f64) -> Params {
    let k = (sensitivity / 100.0).clamp(0.0, 1.0) as f32;
    // 0 = only strong, obvious dust; 100 = faint spots too
    Params { min_depth: 0.025 - 0.02 * k, min_ratio: 0.75 - 0.3 * k, ..Params::default() }
}

/// An even sample of the series.
fn sample_of(ids: &[PhotoId], limit: usize) -> Vec<PhotoId> {
    if ids.len() <= limit { ids.to_vec() } else { (0..limit).map(|i| ids[i * ids.len() / limit]).collect() }
}

fn cached(key: u64) -> Option<Found> {
    LAST.lock().unwrap_or_else(PoisonError::into_inner).as_ref().filter(|f| f.key == key).map(|f| Found {
        key,
        dims: f.dims,
        analysed: f.analysed,
        skipped: f.skipped,
        spots: f.spots.clone(),
        coverage: f.coverage,
    })
}

/// The renders of a sample, one by one: the accumulator of what they have in common.
struct Collect {
    acc: Option<Accumulator>,
    analysed: usize,
    skipped: usize,
    sensitivity: f64,
}

impl Collect {
    fn new(sensitivity: f64) -> Self {
        Collect { acc: None, analysed: 0, skipped: 0, sensitivity }
    }

    fn add(&mut self, rendered: Result<(Vec<f32>, usize, usize)>) {
        let Ok((l, w, h)) = rendered else {
            self.skipped += 1;
            return;
        };
        let a = self.acc.get_or_insert_with(|| Accumulator::new(w, h, params_for(self.sensitivity)));
        if (a.w, a.h) != (w, h) {
            self.skipped += 1; // another orientation or camera: dust sits elsewhere in its frame
            return;
        }
        a.add(&l);
        self.analysed += 1;
    }

    fn finish(self, key: u64) -> Result<Found> {
        let a = self.acc.ok_or_else(|| bad("dust.detect", "no photo could be rendered"))?;
        let found = Found { key, dims: (a.w, a.h), analysed: self.analysed, skipped: self.skipped, spots: a.spots(), coverage: a.coverage() };
        *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(Found {
            key,
            dims: found.dims,
            analysed: found.analysed,
            skipped: found.skipped,
            spots: found.spots.clone(),
            coverage: found.coverage,
        });
        Ok(found)
    }
}

fn detect_series(s: &mut Session, ids: &[PhotoId], limit: usize, sensitivity: f64) -> Result<Found> {
    let key = series_key(ids, limit, sensitivity.to_bits());
    if let Some(f) = cached(key) {
        return Ok(f);
    }
    let mut c = Collect::new(sensitivity);
    for id in sample_of(ids, limit) {
        c.add(luma(s, id));
    }
    c.finish(key)
}

/// A search for persistent dust set up from the session: [`DustPlan::run`] renders the sample and
/// finds the spots without it, so the app can run it on a worker thread (progress, Cancel); the
/// heal spots are then put on the photos by `dust.apply` with the spots it found.
pub struct DustPlan {
    ids: Vec<PhotoId>,
    remove: bool,
    key: u64,
    sensitivity: f64,
    cached: Option<Found>,
    /// A render per photo of the sample (`None`: the photo is gone).
    jobs: Vec<Option<crate::RenderJob>>,
}

/// What [`DustPlan::run`] found.
pub struct DustFound {
    pub ids: Vec<PhotoId>,
    pub remove: bool,
    pub spots: Vec<DustSpot>,
    pub analysed: usize,
    pub skipped: usize,
    pub cancelled: bool,
}

impl DustFound {
    /// The parameters of `dust.apply` that put these spots on the series.
    pub fn apply_params(&self) -> Value {
        json!({"ids": self.ids.iter().map(|i| i.0).collect::<Vec<_>>(), "remove": self.remove,
               "spots": self.spots.iter().map(|d| json!({"x": d.x, "y": d.y, "radius": d.radius})).collect::<Vec<_>>()})
    }
}

impl DustPlan {
    /// Photos that will be rendered (0 when the answer is already known).
    pub fn steps(&self) -> usize {
        if self.cached.is_some() { 0 } else { self.jobs.len() }
    }

    /// Render the sample and find the dust. `progress(done, total)` is called before each photo;
    /// when it returns `false` the search stops (`cancelled`, no spots).
    pub fn run(self, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<DustFound> {
        let DustPlan { ids, remove, key, sensitivity, cached, jobs } = self;
        let done =
            |f: Found, cancelled: bool| DustFound { ids: ids.clone(), remove, spots: f.spots, analysed: f.analysed, skipped: f.skipped, cancelled };
        if let Some(f) = cached {
            return Ok(done(f, false));
        }
        let total = jobs.len();
        let mut c = Collect::new(sensitivity);
        for (i, job) in jobs.into_iter().enumerate() {
            if !progress(i, total) {
                return Ok(done(Found { key, dims: (0, 0), analysed: c.analysed, skipped: c.skipped, spots: Vec::new(), coverage: 0.0 }, true));
            }
            c.add(job.ok_or_else(|| bad("dust.detect", "no such photo")).and_then(luma_run));
        }
        progress(total, total);
        Ok(done(c.finish(key)?, false))
    }
}

/// Set up `dust.apply` for the app to run in the background (see [`DustPlan`]).
pub fn plan_series(s: &mut Session, p: &Value) -> Result<DustPlan> {
    const C: &str = "dust.apply";
    let ids = ids_of(s, p);
    if ids.len() < 3 {
        return Err(bad(C, "dust needs a series: at least 3 photos (select them, give `ids`, or filter the view to one shoot)"));
    }
    let limit = f64_or(p, "limit", SAMPLE as f64).clamp(3.0, 400.0) as usize;
    let sensitivity = f64_or(p, "sensitivity", 50.0);
    let key = series_key(&ids, limit, sensitivity.to_bits());
    let cached = cached(key);
    let jobs = if cached.is_some() { Vec::new() } else { sample_of(&ids, limit).into_iter().map(|id| luma_job(s, id).ok()).collect() };
    Ok(DustPlan { ids, remove: bool_or(p, "remove", false), key, sensitivity, cached, jobs })
}

fn spot_json(d: &DustSpot) -> Value {
    json!({"x": (d.x * 1e4).round() / 1e4, "y": (d.y * 1e4).round() / 1e4, "radius": (d.radius * 1e4).round() / 1e4, "seen": d.seen, "judged": d.judged})
}

fn detect(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "dust.detect";
    let ids = ids_of(s, p);
    if ids.len() < 3 {
        return Err(bad(C, "dust needs a series: at least 3 photos (select them, give `ids`, or filter the view to one shoot)"));
    }
    let limit = f64_or(p, "limit", SAMPLE as f64).clamp(3.0, 400.0) as usize;
    let f = detect_series(s, &ids, limit, f64_or(p, "sensitivity", 50.0))?;
    Ok(
        json!({"photos": ids.len(), "analysed": f.analysed, "skipped": f.skipped, "frame": [f.dims.0, f.dims.1], "judgedShare": (f.coverage * 1000.0).round() / 1000.0, "spots": f.spots.iter().map(spot_json).collect::<Vec<_>>()}),
    )
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "dust.apply";
    let ids = ids_of(s, p);
    if ids.len() < 3 && p.get("spots").is_none() {
        return Err(bad(C, "dust needs a series: at least 3 photos (or give the `spots` of dust.detect)"));
    }
    let spots: Vec<DustSpot> = match p.get("spots").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(|v| {
                Some(DustSpot { x: v.get("x")?.as_f64()?, y: v.get("y")?.as_f64()?, radius: v.get("radius")?.as_f64()?, seen: 0, judged: 0 })
            })
            .collect(),
        None => {
            let limit = f64_or(p, "limit", SAMPLE as f64).clamp(3.0, 400.0) as usize;
            detect_series(s, &ids, limit, f64_or(p, "sensitivity", 50.0))?.spots
        }
    };
    if spots.is_empty() {
        return Ok(json!({"spots": 0, "photos": 0, "added": 0}));
    }
    let mut a = Applier::new(s, ids, spots, bool_or(p, "remove", false));
    while !a.step(s, std::time::Duration::MAX) {}
    a.finish(s)
}

/// The heal spots of one dust search on one photo: the operation and how many spots it adds.
fn heal_photo(s: &mut Session, id: PhotoId, spots: &[DustSpot], mode: SpotMode) -> Option<(Op, usize)> {
    let ph = s.catalog.photo(id)?;
    let mut d = (*ph.develop).clone();
    let before = d.spots.len();
    for sp in spots {
        let size = (sp.radius * 2.2).clamp(SPOT_SIZE.0, SPOT_SIZE.1);
        // already healed there (by an earlier run or by hand)
        if d.spots.iter().any(|e| e.points.first().is_some_and(|q| (q.x - sp.x).hypot(q.y - sp.y) < size)) {
            continue;
        }
        let mut spot = Spot { mode, points: vec![Point::new(sp.x, sp.y)], size, feather: 50.0, opacity: 100.0, source_offset: None, patch: None };
        spot.source_offset = pick_source(s, id, &d, &spot, None);
        d.spots.push(spot);
    }
    let added = d.spots.len() - before;
    if added == 0 {
        return None;
    }
    s.develop_op(id, d, "Remove Dust").map(|op| (op, added))
}

/// The second half of `dust.apply` in steps: the heal spots go on the photos a few at a time (the
/// app does a slice per frame, with a progress panel and a Cancel), and nothing is written until
/// [`Applier::finish`] commits them all as one undo step. If the library changed meanwhile
/// (another edit, another undo), `finish` writes nothing rather than stack onto stale states.
pub struct Applier {
    ids: Vec<PhotoId>,
    next: usize,
    spots: Vec<DustSpot>,
    mode: SpotMode,
    ops: Vec<Op>,
    touched: usize,
    added: usize,
    revision: u64,
    undo_len: usize,
}

impl Applier {
    pub fn new(s: &Session, ids: Vec<PhotoId>, spots: Vec<DustSpot>, remove: bool) -> Applier {
        let mode = if remove { SpotMode::Remove } else { SpotMode::Heal };
        Applier { ids, next: 0, spots, mode, ops: Vec::new(), touched: 0, added: 0, revision: s.catalog.revision, undo_len: s.undo.len() }
    }

    /// Set up from what [`DustPlan::run`] found.
    pub fn from_found(s: &Session, f: &DustFound) -> Applier {
        Applier::new(s, f.ids.clone(), f.spots.clone(), f.remove)
    }

    pub fn total(&self) -> usize {
        self.ids.len()
    }

    pub fn done(&self) -> usize {
        self.next
    }

    /// Work on the next photos for about `budget`; `true` when every photo has been looked at.
    pub fn step(&mut self, s: &mut Session, budget: std::time::Duration) -> bool {
        let t0 = web_time::Instant::now();
        while self.next < self.ids.len() {
            let id = self.ids[self.next];
            self.next += 1;
            if let Some((op, n)) = heal_photo(s, id, &self.spots, self.mode) {
                self.ops.push(op);
                self.touched += 1;
                self.added += n;
            }
            if t0.elapsed() >= budget {
                break;
            }
        }
        self.next >= self.ids.len()
    }

    /// Commit what was made (one undo step) → what `dust.apply` returns.
    pub fn finish(self, s: &mut Session) -> Result<Value> {
        if s.catalog.revision != self.revision || s.undo.len() != self.undo_len {
            return Err(bad("dust.apply", "the library changed while the dust was being healed: nothing was written, run it again"));
        }
        if !self.ops.is_empty() {
            s.commit("Remove Dust", Op::Batch { ops: self.ops })?;
        }
        Ok(
            json!({"spots": self.spots.len(), "photos": self.touched, "added": self.added, "spotList": self.spots.iter().map(spot_json).collect::<Vec<_>>()}),
        )
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "dust.detect",
            "Find Persistent Dust",
            [],
            None,
            "{ids?, sensitivity?: 0..100 (50), limit?: photos looked at (40, evenly spaced)} — find sensor-dust spots that sit at the same place on most photos of a series (the selection, else the whole view; at least 3 photos, same size and orientation): dark soft discs on smooth areas (sky, walls, blurred backgrounds) seen in ≥ 60 % of the photos where the place can be judged → {photos, analysed, skipped, frame, judgedShare (share of the frame where dust could be judged at all: dark or busy areas cannot), spots: [{x, y, radius, seen, judged}]} (x/y/radius normalized to the uncropped photo). Changes nothing; dust.apply uses the same result",
            super::always,
            detect
        ),
        cmd!(
            "dust.apply",
            "Remove Dust From Series",
            ["Photo"],
            None,
            "{ids?, sensitivity?, limit?, spots?: [{x,y,radius}] (default: dust.detect), remove?: bool (heal by default)} — put a heal spot (source picked automatically) on each persistent dust spot on every photo of the series, skipping places already healed; one undo step → {spots, photos, added, spotList}",
            super::always,
            apply
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_puts_a_heal_spot_on_every_photo_once_and_is_one_undo_step() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(4).map(|i| i.0).collect();
        let spots = json!([{"x": 0.31, "y": 0.42, "radius": 0.006}, {"x": 0.7, "y": 0.2, "radius": 0.01}]);
        let n_spots = |s: &Session, id: u64| s.catalog.photo(PhotoId(id)).unwrap().develop.spots.len();
        let r = s.execute("dust.apply", &json!({"ids": ids, "spots": spots})).unwrap();
        assert_eq!((r["photos"].as_u64(), r["added"].as_u64()), (Some(4), Some(8)), "{r}");
        for id in &ids {
            let d = &s.catalog.photo(PhotoId(*id)).unwrap().develop;
            assert_eq!(d.spots.len(), 2);
            assert!(d.spots.iter().all(|sp| sp.mode == SpotMode::Heal && sp.source_offset.is_some()), "{:?}", d.spots);
            // the spot is as big as the dust, with a margin
            assert!((d.spots[0].size - 0.0132).abs() < 1e-9, "{}", d.spots[0].size);
        }
        // again: those places are already healed, nothing is added
        let r = s.execute("dust.apply", &json!({"ids": ids, "spots": spots})).unwrap();
        assert_eq!(r["added"], 0);
        // the whole series comes back with one undo
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(ids.iter().all(|id| n_spots(&s, *id) == 0));
    }

    #[test]
    fn a_series_is_needed_and_detect_reports_its_frame() {
        let mut s = Session::with_demo();
        let e = s.execute("dust.detect", &json!({"ids": [1, 2]})).unwrap_err();
        assert!(e.to_string().contains("at least 3 photos"), "{e}");
        let ids: Vec<u64> = s.visible_cloned().iter().take(5).map(|i| i.0).collect();
        let r = s.execute("dust.detect", &json!({"ids": ids, "limit": 5})).unwrap();
        assert_eq!(r["analysed"].as_u64().unwrap() + r["skipped"].as_u64().unwrap(), 5, "{r}");
        assert!(r["frame"][0].as_u64().unwrap() > 0 && r["spots"].is_array(), "{r}");
    }

    #[test]
    fn a_planned_search_finds_what_detect_finds_and_hands_dust_apply_its_spots() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(5).map(|i| i.0).collect();
        let plan = plan_series(&mut s, &json!({"ids": ids, "limit": 5, "sensitivity": 80})).unwrap();
        assert_eq!(plan.steps(), 5);
        let mut calls = Vec::new();
        let found = plan
            .run(&mut |done, total| {
                calls.push((done, total));
                true
            })
            .unwrap();
        assert_eq!(calls, vec![(0, 5), (1, 5), (2, 5), (3, 5), (4, 5), (5, 5)]);
        let direct = s.execute("dust.detect", &json!({"ids": ids, "limit": 5, "sensitivity": 80})).unwrap();
        assert!(!found.cancelled);
        assert_eq!(found.spots.len(), direct["spots"].as_array().unwrap().len());
        assert_eq!(found.analysed as u64 + found.skipped as u64, 5);
        // (the answer for this series is known now: a second plan renders nothing)
        assert_eq!(plan_series(&mut s, &json!({"ids": ids, "limit": 5, "sensitivity": 80})).unwrap().steps(), 0);
        // the parameters put the spots on the photos exactly as `dust.apply` does on its own
        let params = found.apply_params();
        assert_eq!(params["ids"].as_array().unwrap().len(), 5);
        assert_eq!(params["spots"].as_array().unwrap().len(), direct["spots"].as_array().unwrap().len());
        let a = s.execute("dust.apply", &params).unwrap();
        if a["added"].as_u64().unwrap_or(0) > 0 {
            s.execute("edit.undo", &json!({})).unwrap();
        }
        let b = s.execute("dust.apply", &json!({"ids": ids, "limit": 5, "sensitivity": 80})).unwrap();
        assert_eq!((&a["spots"], &a["added"]), (&b["spots"], &b["added"]), "the same series, the same result: {a} / {b}");
    }

    #[test]
    fn cancelling_a_planned_search_returns_no_spots() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(6).map(|i| i.0).collect();
        // a series nobody has looked at (another sensitivity than the other tests)
        let plan = plan_series(&mut s, &json!({"ids": ids, "limit": 6, "sensitivity": 11})).unwrap();
        assert_eq!(plan.steps(), 6);
        let found = plan.run(&mut |done, _| done < 3).unwrap();
        assert!(found.cancelled && found.spots.is_empty(), "stopped after 3 photos, nothing found");
        assert_eq!(found.analysed + found.skipped, 3);
    }

    #[test]
    fn healing_in_steps_gives_one_undo_step_and_refuses_a_library_that_changed_meanwhile() {
        let mut s = Session::with_demo();
        let ids: Vec<PhotoId> = s.visible_cloned().iter().take(4).copied().collect();
        let spots = vec![DustSpot { x: 0.31, y: 0.42, radius: 0.006, seen: 0, judged: 0 }];
        let n_spots = |s: &Session, id: PhotoId| s.catalog.photo(id).unwrap().develop.spots.len();
        let steps = s.undo.len();
        // a zero budget is one photo per step: nothing is written until the end
        let mut a = Applier::new(&s, ids.clone(), spots.clone(), false);
        assert_eq!((a.total(), a.done()), (4, 0));
        let mut calls = 0;
        while !a.step(&mut s, std::time::Duration::ZERO) {
            calls += 1;
            assert_eq!(a.done(), calls);
        }
        assert_eq!((calls, a.done()), (3, 4));
        assert!(ids.iter().all(|id| n_spots(&s, *id) == 0) && s.undo.len() == steps, "still nothing written");
        let r = a.finish(&mut s).unwrap();
        assert_eq!((r["photos"].as_u64(), r["added"].as_u64()), (Some(4), Some(4)), "{r}");
        assert!(ids.iter().all(|id| n_spots(&s, *id) == 1));
        assert_eq!(s.undo.len(), steps + 1, "one undo step");
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(ids.iter().all(|id| n_spots(&s, *id) == 0));
        // an edit made while the spots were being prepared: nothing is written on top of it
        let mut a = Applier::new(&s, ids.clone(), spots, false);
        while !a.step(&mut s, std::time::Duration::MAX) {}
        s.execute("photo.rate", &json!({"ids": [ids[0].0], "rating": 3})).unwrap();
        let e = a.finish(&mut s).unwrap_err().to_string();
        assert!(e.contains("changed while"), "{e}");
        assert!(ids.iter().all(|id| n_spots(&s, *id) == 0), "nothing written");
    }
}
