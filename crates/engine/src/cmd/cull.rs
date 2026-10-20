//! Assisted culling: score photos for focus and clipping, group similar shots taken close
//! together (bursts) and mark the sharpest of each group; optionally reject blurry ones and pick
//! each group's best. Scores are kept on the photos (rules / smart albums: `sharpness`,
//! `bestOfGroup`). With faces analyzed (`photo.analyzeFaces`), a photo with a closed eye can be
//! rejected and a burst's best is chosen on the eyes as well as the focus.

use lightcraft_catalog::{Analysis, Flag, Op, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, cmd};
use crate::{Result, Session};

/// Seconds between shots of the same burst at most.
const BURST_GAP: i64 = 10;
/// Signature similarity for "the same scene".
const SAME: f32 = 0.93;
/// In a burst that has faces: the share of a shot's score that comes from its faces (open, sharp
/// eyes) rather than from the whole frame's focus.
const FACE_WEIGHT: f32 = 0.6;
/// Subtracted from a shot with a closed eye, so it only wins a burst when every shot has one.
const CLOSED_PENALTY: f32 = 1000.0;

const MEASUREMENT_VERSION: &str = "lightcraft-focus-clipping-thumb-v1";

/// Bounded, disposable measurements in prefs.json; group membership is always recomputed.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Measurement {
    sharpness: f32,
    clipped: f32,
    signature: Vec<f32>,
}

impl Measurement {
    pub(crate) fn valid(&self) -> bool {
        self.sharpness.is_finite()
            && (0.0..=100.0).contains(&self.sharpness)
            && self.clipped.is_finite()
            && (0.0..=1.0).contains(&self.clipped)
            && self.signature.len() == 64
            && self.signature.iter().all(|v| v.is_finite())
    }
}

fn measurement_key(s: &Session, id: PhotoId) -> Option<String> {
    let ph = s.catalog.photo(id)?;
    let stamp = match &ph.source {
        lightcraft_catalog::Source::Demo { .. } => "demo".to_string(),
        lightcraft_catalog::Source::File { path } => {
            let m = std::fs::metadata(path).ok()?;
            if !m.is_file() {
                return None;
            }
            let time = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            format!("{path}:{}:{time}", m.len())
        }
    };
    Some(format!("{MEASUREMENT_VERSION}:{}:{stamp}", crate::media::content_key(ph)))
}

/// What a photo's faces say about it as a shot.
#[derive(Clone, Copy, Default)]
struct FaceSummary {
    /// Faces whose eyes can be judged.
    judged: usize,
    closed: bool,
    /// Mean [`FaceInfo::quality`] of the judged faces, 0..100.
    quality: f32,
}

fn face_summary(s: &Session, id: PhotoId) -> Option<FaceSummary> {
    let faces = s.catalog.photo(id)?.faces()?;
    let judged: Vec<&lightcraft_catalog::FaceInfo> = faces.iter().filter(|f| f.eyes_judged()).collect();
    let quality = if judged.is_empty() { 0.0 } else { judged.iter().map(|f| f.quality()).sum::<f32>() / judged.len() as f32 };
    Some(FaceSummary { judged: judged.len(), closed: judged.iter().any(|f| f.eyes_closed()), quality })
}

fn secs(iso: &str) -> Option<i64> {
    let d = lightcraft_meta::DateTime::parse_iso(iso)?;
    Some(((d.year as i64 * 372 + d.month as i64 * 31 + d.day as i64) * 24 + d.hour as i64) * 3600 + d.minute as i64 * 60 + d.second as i64)
}

fn analyze(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.analyze";
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.checked_targets(C, p)? } else { s.visible_cloned() };
    for key in ["dryRun", "force", "pickBest", "rejectClosedEyes", "faces"] {
        if p.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(super::bad(C, format!("{key} must be a boolean")));
        }
    }
    if p.get("rejectBelow").is_some_and(|v| !v.as_f64().is_some_and(|n| n.is_finite() && (0.0..=100.0).contains(&n))) {
        return Err(super::bad(C, "rejectBelow must be a number 0..100"));
    }
    let dry_run = p.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let requested: Vec<u64> = ids.iter().map(|id| id.0).collect();
    let reject_below = p.get("rejectBelow").and_then(Value::as_f64).map(|v| v as f32);
    let pick_best = p.get("pickBest").and_then(Value::as_bool).unwrap_or(false);
    let reject_closed = p.get("rejectClosedEyes").and_then(Value::as_bool).unwrap_or(false);
    // faces: used when analyzed; `faces: true` (or rejecting closed eyes) analyzes the photos that
    // have none yet, which needs the model
    let want_faces = p.get("faces").and_then(Value::as_bool).unwrap_or(reject_closed);
    let mut faces_pending = 0;
    if want_faces && !dry_run {
        let (queued, _) = s
            .facemesh_enqueue(&ids, false)
            .map_err(|e| super::bad("photo.analyze", format!("{e} (faces / rejectClosedEyes need it: faces.model.download)")))?;
        if s.facemesh.background {
            faces_pending = queued;
        } else {
            s.faces_wait(queued);
        }
    }
    // measure (thumbnail-level sources: fast, and enough for focus at 512 px)
    let mut rows: Vec<(PhotoId, Option<i64>, f32, f32, [f32; 64])> = Vec::new();
    let mut failed = Vec::new();
    let mut cached = 0;
    let mut measured = 0;
    for id in ids {
        let key = measurement_key(s, id);
        let reused = key.as_ref().filter(|_| !force).and_then(|key| s.culling_cache.get(key)).filter(|m| m.valid()).cloned();
        if let Some(m) = reused {
            if let Ok(signature) = <[f32; 64]>::try_from(m.signature) {
                let t = s.catalog.photo(id).and_then(|p| p.captured.as_deref().and_then(secs));
                rows.push((id, t, m.sharpness, m.clipped, signature));
                cached += 1;
                continue;
            }
        }
        // Measure the original independently of the edited/denoised preview and decoded caches.
        let source = s.catalog.photo(id).map(|p| p.source.clone());
        let loaded = match source {
            Some(source) => s.media.origin_ref(&source, crate::media::SourceLevel::Thumb.max_edge()).load(),
            None => Err("no such photo".into()),
        };
        match loaded {
            Ok(src) => {
                let t = s.catalog.photo(id).and_then(|p| p.captured.as_deref().and_then(secs));
                use lightcraft_pipeline::cull;
                let (sharp, clip, signature) = (cull::sharpness(&src), cull::clipped(&src), cull::signature(&src));
                rows.push((id, t, sharp, clip, signature));
                measured += 1;
                if !dry_run
                    && key == measurement_key(s, id)
                    && let Some(key) = key
                {
                    if s.culling_cache.len() >= 1024 {
                        s.culling_cache.pop_first();
                    }
                    s.culling_cache.insert(key, Measurement { sharpness: sharp, clipped: clip, signature: signature.to_vec() });
                }
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
        let close = matches!((a.1, b.1), (Some(x), Some(y)) if (y - x).abs() <= BURST_GAP);
        if close && lightcraft_pipeline::cull::similarity(&a.4, &b.4) >= SAME {
            let g = *group_of[i - 1].get_or_insert_with(|| {
                next += 1;
                next - 1
            });
            group_of[i] = Some(g);
        }
    }
    let summaries: Vec<Option<FaceSummary>> = rows.iter().map(|r| face_summary(s, r.0)).collect();
    // a burst whose shots show faces is judged on them (eyes open, eyes sharp) as well as on focus
    let mut group_has_faces: std::collections::HashSet<u32> = Default::default();
    for (i, g) in group_of.iter().enumerate() {
        if let (Some(g), Some(f)) = (g, &summaries[i])
            && f.judged > 0
        {
            group_has_faces.insert(*g);
        }
    }
    let shot_score = |i: usize| -> f32 {
        let sharp = rows[i].2;
        match group_of[i] {
            Some(g) if group_has_faces.contains(&g) => {
                let f = summaries[i].unwrap_or_default();
                (1.0 - FACE_WEIGHT) * sharp + FACE_WEIGHT * f.quality - if f.closed { CLOSED_PENALTY } else { 0.0 }
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
            analysis: Some(Analysis {
                sharpness: *sharp,
                clipped: *clip,
                group: g,
                best: is_best,
                faces: s.catalog.photo(*id).and_then(|p| p.analysis.as_ref()).and_then(|a| a.faces.clone()),
            }),
        });
        let flag = s.catalog.photo(*id).map(|p| p.flag).unwrap_or_default();
        let fs = summaries[i];
        let closed = fs.is_some_and(|f| f.closed);
        closed_eyes += closed as usize;
        let low_focus = reject_below.is_some_and(|t| *sharp < t);
        let reject = low_focus || (reject_closed && closed);
        if reject && flag != Flag::Reject {
            ops.push(Op::SetFlag { id: *id, flag: Flag::Reject });
            rejected += 1;
        } else if pick_best && is_best && flag == Flag::None {
            ops.push(Op::SetFlag { id: *id, flag: Flag::Pick });
            picked += 1;
        }
        let mut reasons = Vec::new();
        if low_focus {
            reasons.push("focus_below_threshold");
        }
        if reject_closed && closed {
            reasons.push("closed_eyes");
        }
        if is_best {
            reasons.push("best_in_burst");
        }
        if fs.is_none_or(|f| f.judged == 0) {
            reasons.push("eyes_not_evaluated");
        }
        let suggested = if reject {
            "reject"
        } else if is_best {
            "pick"
        } else {
            "review"
        };
        let mut row = json!({"id": id.0, "sharpness": (sharp * 10.0).round() / 10.0, "clipped": clip, "group": g, "best": is_best,
            "suggestedFlag": suggested, "reasons": reasons, "measurementBasis": "original_thumbnail", "analysisVersion": MEASUREMENT_VERSION});
        if let (Some(f), Some(o)) = (fs, row.as_object_mut()) {
            o.insert("faces".into(), json!(f.judged));
            o.insert("eyesClosed".into(), json!(f.closed));
            o.insert("faceQuality".into(), json!((f.quality * 10.0).round() / 10.0));
        }
        out.push(row);
    }
    if !ops.is_empty() && !dry_run {
        s.commit("Assisted Culling", Op::Batch { ops })?;
        if measured > 0 {
            s.save_prefs()?;
        }
    }
    Ok(json!({"photos": out, "groups": best.len(), "rejected": if dry_run {0} else {rejected}, "picked": if dry_run {0} else {picked},
            "wouldReject": rejected, "wouldPick": picked, "dryRun": dry_run, "cached": cached, "measured": measured, "targetIds": requested,
            "closedEyes": closed_eyes, "facesPending": faces_pending, "failed": failed}))
}

/// Find Similar: photos that look like `id` (signatures from thumbnail-level sources, cached by
/// content), most similar first; the view is filtered to them.
fn find_similar(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.findSimilar";
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| super::bad(C, "no photo"))?;
    let min = p.get("similarity").and_then(Value::as_f64).unwrap_or(0.8) as f32;
    let all: Vec<PhotoId> = s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect();
    let sig = |s: &mut Session, id: PhotoId| -> Option<[f32; 64]> {
        let key = s.catalog.photo(id).map(|p| crate::media::content_key(p))?;
        if let Some(v) = s.signatures.get(&key) {
            return Some(*v);
        }
        let src = s.source_now(id, crate::media::SourceLevel::Thumb).ok()?;
        let v = lightcraft_pipeline::cull::signature(&src);
        s.signatures.insert(key, v);
        Some(v)
    };
    let me = sig(s, id).ok_or_else(|| super::bad(C, "can't read the photo"))?;
    let mut hits: Vec<(PhotoId, f32)> = Vec::new();
    for q in all {
        if let Some(v) = sig(s, q) {
            let sim = lightcraft_pipeline::cull::similarity(&me, &v);
            if sim >= min {
                hits.push((q, sim));
            }
        }
    }
    hits.sort_by(|a, b| b.1.total_cmp(&a.1));
    if p.get("filter").and_then(Value::as_bool).unwrap_or(true) {
        s.filter.only = hits.iter().map(|h| h.0).collect();
    }
    Ok(json!({"photos": hits.iter().map(|(i, v)| json!({"id": i.0, "similarity": (v * 1000.0).round() / 1000.0})).collect::<Vec<_>>()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "library.findSimilar",
            "Find Similar Photos",
            ["Photo"],
            None,
            "{id?, similarity?: 0..1 (0.8), filter?: bool (true)} — photos that look like the active one (composition and tones), most similar first; filters the view to them (Clear Filters to go back) → {photos: [{id, similarity}]}",
            always,
            find_similar
        ),
        cmd!(
            "photo.analyze",
            "Assisted Culling",
            [],
            None,
            "{id?|ids?, dryRun?: bool, force?: bool, rejectBelow?: 0..100, pickBest?: bool, rejectClosedEyes?: bool, faces?: bool} — focus/clipping on original thumbnails, burst suggestions with reasons. dryRun changes no analysis/flags and uses existing faces only. Bounded versioned measurements persist with library prefs and invalidate on file size/mtime; force recomputes. Suggestions are separate from flags; applying is one undo step → {photos, targetIds, groups, rejected, picked, wouldReject, wouldPick, dryRun, cached, measured, closedEyes, facesPending, failed}",
            always,
            analyze
        ),
    ]
}
