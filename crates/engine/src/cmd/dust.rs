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

/// The photo's luminance plane at the series' size, rendered without its own edits (sensor
/// geometry: no crop, no lens correction), display-encoded.
fn luma(s: &mut Session, id: PhotoId) -> Result<(Vec<f32>, usize, usize)> {
    let settings = DevelopSettings::default();
    let job = s.preview_job(id, EDGE, EDGE, false, &settings).ok_or_else(|| bad("dust.detect", format!("no such photo {}", id.0)))?;
    let img = job.run().rendered.map_err(|e| bad("dust.detect", e))?.image;
    let l = img.data.iter().map(|p| (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0).collect();
    Ok((l, img.width, img.height))
}

fn ids_of(s: &mut Session, p: &Value) -> Vec<PhotoId> {
    if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { s.visible_cloned() }
}

fn params_for(sensitivity: f64) -> Params {
    let k = (sensitivity / 100.0).clamp(0.0, 1.0) as f32;
    // 0 = only strong, obvious dust; 100 = faint spots too
    Params { min_depth: 0.025 - 0.02 * k, min_ratio: 0.75 - 0.3 * k, ..Params::default() }
}

fn detect_series(s: &mut Session, ids: &[PhotoId], limit: usize, sensitivity: f64) -> Result<Found> {
    let key = series_key(ids, limit, sensitivity.to_bits());
    if let Some(f) = LAST.lock().unwrap_or_else(PoisonError::into_inner).as_ref().filter(|f| f.key == key) {
        return Ok(Found { key, dims: f.dims, analysed: f.analysed, skipped: f.skipped, spots: f.spots.clone(), coverage: f.coverage });
    }
    // an even sample of the series
    let pick: Vec<PhotoId> = if ids.len() <= limit { ids.to_vec() } else { (0..limit).map(|i| ids[i * ids.len() / limit]).collect() };
    let mut acc: Option<Accumulator> = None;
    let (mut analysed, mut skipped) = (0usize, 0usize);
    for id in pick {
        let Ok((l, w, h)) = luma(s, id) else {
            skipped += 1;
            continue;
        };
        let a = acc.get_or_insert_with(|| Accumulator::new(w, h, params_for(sensitivity)));
        if (a.w, a.h) != (w, h) {
            skipped += 1; // another orientation or camera: dust sits elsewhere in its frame
            continue;
        }
        a.add(&l);
        analysed += 1;
    }
    let a = acc.ok_or_else(|| bad("dust.detect", "no photo could be rendered"))?;
    let found = Found { key, dims: (a.w, a.h), analysed, skipped, spots: a.spots(), coverage: a.coverage() };
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(Found { key, dims: found.dims, analysed, skipped, spots: found.spots.clone(), coverage: found.coverage });
    Ok(found)
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
    let mode = if bool_or(p, "remove", false) { SpotMode::Remove } else { SpotMode::Heal };
    let (mut ops, mut touched, mut added) = (Vec::new(), 0usize, 0usize);
    for id in &ids {
        let Some(ph) = s.catalog.photo(*id) else { continue };
        let mut d = (*ph.develop).clone();
        let before = d.spots.len();
        for sp in &spots {
            let size = (sp.radius * 2.2).clamp(SPOT_SIZE.0, SPOT_SIZE.1);
            // already healed there (by an earlier run or by hand)
            if d.spots.iter().any(|e| e.points.first().is_some_and(|q| (q.x - sp.x).hypot(q.y - sp.y) < size)) {
                continue;
            }
            let mut spot = Spot { mode, points: vec![Point::new(sp.x, sp.y)], size, feather: 50.0, opacity: 100.0, source_offset: None, patch: None };
            spot.source_offset = pick_source(s, *id, &d, &spot, None);
            d.spots.push(spot);
        }
        if d.spots.len() > before {
            added += d.spots.len() - before;
            touched += 1;
            if let Some(op) = s.develop_op(*id, d, "Remove Dust") {
                ops.push(op);
            }
        }
    }
    if !ops.is_empty() {
        s.commit("Remove Dust", Op::Batch { ops })?;
    }
    Ok(json!({"spots": spots.len(), "photos": touched, "added": added, "spotList": spots.iter().map(spot_json).collect::<Vec<_>>()}))
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
            ["Photo", "Remove Dust From Selection"],
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
}
