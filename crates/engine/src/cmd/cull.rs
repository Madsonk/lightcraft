//! Assisted culling: score photos for focus and clipping, group similar shots taken close
//! together (bursts) and mark the sharpest of each group; optionally reject blurry ones and pick
//! each group's best. Scores are kept on the photos (rules / smart albums: `sharpness`,
//! `bestOfGroup`).

use lightcraft_catalog::{Analysis, Flag, Op, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, cmd};
use crate::{Result, Session};

/// Seconds between shots of the same burst at most (default of `burstGap`).
const BURST_GAP: f64 = 10.0;
/// Signature similarity for "the same scene" (default of `similarity`).
const SAME: f64 = 0.93;
/// The longest `burstGap` accepted: a day.
const MAX_GAP: f64 = 86_400.0;
/// In a burst that has faces: the share of a shot's score that comes from its faces (open, sharp
/// eyes) rather than from the whole frame's focus.
const FACE_WEIGHT: f32 = 0.6;
/// Subtracted from a shot with a closed eye, so it only wins a burst when every shot has one.
const CLOSED_PENALTY: f32 = 1000.0;

fn secs(iso: &str) -> Option<i64> {
    let d = lightcraft_meta::DateTime::parse_iso(iso)?;
    Some(d.unix_seconds())
}

fn analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || s.selection.ids.len() > 1 { s.targets(p)? } else { s.visible_cloned() };
    // a photo listed twice is one photo, not a burst of two look-alikes (first occurrence kept)
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<PhotoId> = ids.into_iter().filter(|id| seen.insert(*id)).collect();
    let reject_below = p.get("rejectBelow").and_then(Value::as_f64).map(|v| v as f32);
    let pick_best = p.get("pickBest").and_then(Value::as_bool).unwrap_or(false);
    let (gap, same) = burst_params(p)?;
    let reject_closed = p.get("rejectClosedEyes").and_then(Value::as_bool).unwrap_or(false);
    // faces: used when analyzed; `faces: true` (or rejecting closed eyes) analyzes the photos that
    // have none yet, which needs the model (`facemesh.model.download`)
    let want_faces = p.get("faces").and_then(Value::as_bool).unwrap_or(reject_closed);
    if want_faces {
        let (queued, _) = s
            .facemesh_enqueue(&ids, false)
            .map_err(|e| super::bad("photo.analyze", format!("{e} (faces / rejectClosedEyes need it: facemesh.model.download)")))?;
        if !s.facemesh.background {
            s.faces_wait(queued);
        }
    }
    // measure (thumbnail-level sources: fast, and enough for focus at 512 px)
    let mut rows: Vec<(PhotoId, Option<i64>, f32, f32, [f32; 64])> = Vec::new();
    let mut failed = Vec::new();
    // the sources are read and measured side by side (a burst of a few dozen raws would otherwise keep the
    // window waiting for seconds); what each photo learns from its file is applied afterwards, in order
    let jobs: Vec<(PhotoId, Option<i64>, crate::media::SourceRef)> = ids
        .into_iter()
        .filter_map(|id| {
            let ph = s.catalog.photo(id)?.clone();
            let t = ph.captured.as_deref().and_then(secs);
            Some((id, t, s.media.source_ref(&ph, crate::media::SourceLevel::Thumb)))
        })
        .collect();
    use rayon::prelude::*;
    type Loaded = (PhotoId, Option<i64>, std::result::Result<(crate::media::DecodedSource, f32, f32, [f32; 64]), String>);
    let loaded: Vec<Loaded> = jobs
        .into_par_iter()
        .map(|(id, t, source)| {
            let r = source.load_source().map(|r| {
                use lightcraft_pipeline::cull;
                let (sharp, clip, sig) = (cull::sharpness(&r.image), cull::clipped(&r.image), cull::signature(&r.image));
                (r, sharp, clip, sig)
            });
            (id, t, r)
        })
        .collect();
    for (id, t, r) in loaded {
        match r {
            Ok((r, sharp, clip, sig)) => {
                if let Some(info) = &r.info {
                    s.learn_as_shot_wb(id, info);
                }
                s.media.insert_source(id, crate::media::SourceLevel::Thumb, r);
                rows.push((id, t, sharp, clip, sig));
            }
            Err(e) => failed.push(json!([id.0, e])),
        }
    }
    // bursts: consecutive (by capture time) look-alike shots
    rows.sort_by_key(|r| (r.1.unwrap_or(i64::MAX), r.0));
    let mut group_of: Vec<Option<u32>> = vec![None; rows.len()];
    let mut next = 1u32;
    for i in 1..rows.len() {
        let (a, b) = (&rows[i - 1], &rows[i]);
        let close = matches!((a.1, b.1), (Some(x), Some(y)) if (y.saturating_sub(x)).unsigned_abs() as f64 <= gap);
        if close && lightcraft_pipeline::cull::similarity(&a.4, &b.4) >= same {
            let g = *group_of[i - 1].get_or_insert_with(|| {
                next += 1;
                next - 1
            });
            group_of[i] = Some(g);
        }
    }
    // a burst whose shots show faces (Analyze Faces) is judged on them (eyes open, eyes sharp) as
    // well as on focus; a shot with a closed eye only wins when every shot has one
    let summaries: Vec<Option<(usize, bool, f32)>> = rows
        .iter()
        .map(|r| {
            let faces = s.catalog.photo(r.0)?.faces()?;
            let judged: Vec<&lightcraft_catalog::FaceInfo> = faces.iter().filter(|f| f.eyes_judged()).collect();
            let quality = if judged.is_empty() { 0.0 } else { judged.iter().map(|f| f.quality()).sum::<f32>() / judged.len() as f32 };
            Some((judged.len(), judged.iter().any(|f| f.eyes_closed()), quality))
        })
        .collect();
    let mut group_has_faces: std::collections::HashSet<u32> = Default::default();
    for (i, g) in group_of.iter().enumerate() {
        if let (Some(g), Some((n, _, _))) = (g, &summaries[i])
            && *n > 0
        {
            group_has_faces.insert(*g);
        }
    }
    let shot_score = |i: usize| -> f32 {
        let sharp = rows[i].2;
        match group_of[i] {
            Some(g) if group_has_faces.contains(&g) => {
                let (_, closed, quality) = summaries[i].unwrap_or_default();
                (1.0 - FACE_WEIGHT) * sharp + FACE_WEIGHT * quality - if closed { CLOSED_PENALTY } else { 0.0 }
            }
            _ => sharp,
        }
    };
    let mut best: std::collections::HashMap<u32, (usize, f32)> = Default::default();
    for (i, g) in group_of.iter().enumerate() {
        if let Some(g) = g {
            let e = best.entry(*g).or_insert((i, f32::NEG_INFINITY));
            let sc = shot_score(i);
            if sc > e.1 {
                *e = (i, sc);
            }
        }
    }
    let mut ops = Vec::new();
    let (mut rejected, mut picked, mut closed_eyes) = (0, 0, 0);
    let mut out = Vec::new();
    for (i, (id, _, sharp, clip, _)) in rows.iter().enumerate() {
        let g = group_of[i];
        let is_best = g.is_some_and(|g| best.get(&g).is_some_and(|b| b.0 == i));
        ops.push(Op::SetAnalysis {
            id: *id,
            analysis: Some(Analysis { sharpness: *sharp, clipped: *clip, group: g, best: is_best, ..old_analysis(s, *id) }),
        });
        let flag = s.catalog.photo(*id).map(|p| p.flag).unwrap_or_default();
        let closed = summaries[i].is_some_and(|f| f.1);
        closed_eyes += closed as usize;
        if (reject_below.is_some_and(|t| *sharp < t) || (reject_closed && closed)) && flag != Flag::Reject {
            ops.push(Op::SetFlag { id: *id, flag: Flag::Reject });
            rejected += 1;
        } else if pick_best && is_best && flag == Flag::None {
            ops.push(Op::SetFlag { id: *id, flag: Flag::Pick });
            picked += 1;
        }
        out.push(json!({"id": id.0, "sharpness": (sharp * 10.0).round() / 10.0, "clipped": clip, "group": g, "best": is_best}));
    }
    if !ops.is_empty() {
        s.commit("Assisted Culling", Op::Batch { ops })?;
    }
    Ok(json!({"photos": out, "groups": best.len(), "rejected": rejected, "picked": picked, "closedEyes": closed_eyes, "failed": failed}))
}

/// `burstGap` (seconds, 0..=a day) and `similarity` (0..=1) of `photo.analyze`, refused before
/// anything is measured when they are not numbers in range.
fn burst_params(p: &Value) -> Result<(f64, f32)> {
    const C: &str = "photo.analyze";
    let num = |key: &str, d: f64, max: f64| -> Result<f64> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => match v.as_f64() {
                Some(x) if x.is_finite() && (0.0..=max).contains(&x) => Ok(x),
                _ => Err(super::bad(C, format!("`{key}` must be a number from 0 to {max}"))),
            },
        }
    };
    Ok((num("burstGap", BURST_GAP, MAX_GAP)?, num("similarity", SAME, 1.0)? as f32))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "library.findSimilar",
            "Find Similar Photos",
            ["Photo"],
            None,
            "{id?, similarity?: 0..1 (0.8), filter?: bool (true)} — photos that look like the active one (composition and tones), most similar first; filters the view to them (Clear Filters to go back) → {photos: [{id, similarity}]}. In the app the photos are read on a worker thread under an activity row: a control request is answered with this when the search ends, and from a menu the answer is {background: true, count}",
            super::similar::no_search_running,
            super::similar::run
        ),
        cmd!(
            "photo.analyze",
            "Assisted Culling",
            [],
            None,
            "{ids?, rejectBelow?: sharpness 0..100, pickBest?: bool, rejectClosedEyes?: bool, faces?: bool, burstGap?: seconds 0..86400 (10), similarity?: 0..1 (0.93)} — score the selected photos (else all in view) for focus (0..100) and clipping, group look-alike shots (signature similarity at least `similarity`) taken at most `burstGap` seconds apart and mark the sharpest of each; optionally reject blurry photos and pick each group's best; one undo step → {photos: [{id, sharpness, clipped, group, best}], groups, rejected, picked, closedEyes}",
            always,
            analyze
        ),
    ]
}

/// What a photo's analysis already holds besides the burst measurements (faces, AI verdict), so
/// that `photo.analyze` does not erase it.
fn old_analysis(s: &Session, id: PhotoId) -> Analysis {
    s.catalog
        .photo(id)
        .and_then(|p| p.analysis.clone())
        .map(|a| Analysis { faces: a.faces, verdict: a.verdict, ..Default::default() })
        .unwrap_or_default()
}
