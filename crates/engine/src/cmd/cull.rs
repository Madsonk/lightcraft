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
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { s.visible_cloned() };
    let reject_below = p.get("rejectBelow").and_then(Value::as_f64).map(|v| v as f32);
    let pick_best = p.get("pickBest").and_then(Value::as_bool).unwrap_or(false);
    let reject_closed = p.get("rejectClosedEyes").and_then(Value::as_bool).unwrap_or(false);
    // faces: used when analyzed; `faces: true` (or rejecting closed eyes) analyzes the photos that
    // have none yet, which needs the model
    let want_faces = p.get("faces").and_then(Value::as_bool).unwrap_or(reject_closed);
    let mut faces_pending = 0;
    if want_faces {
        let (queued, _) = s.faces_enqueue(&ids, false).map_err(|e| super::bad("photo.analyze", format!("{e} (faces / rejectClosedEyes need it: faces.model.download)")))?;
        if s.faces.background {
            faces_pending = queued;
        } else {
            s.faces_wait(queued);
        }
    }
    // measure (thumbnail-level sources: fast, and enough for focus at 512 px)
    let mut rows: Vec<(PhotoId, Option<i64>, f32, f32, [f32; 64])> = Vec::new();
    let mut failed = Vec::new();
    for id in ids {
        match s.source_now(id, crate::media::SourceLevel::Thumb) {
            Ok(src) => {
                let t = s.catalog.photo(id).and_then(|p| p.captured.as_deref().and_then(secs));
                use lightcraft_pipeline::cull;
                rows.push((id, t, cull::sharpness(&src), cull::clipped(&src), cull::signature(&src)));
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
        ops.push(Op::SetAnalysis { id: *id, analysis: Some(Analysis { sharpness: *sharp, clipped: *clip, group: g, best: is_best, faces: s.catalog.photo(*id).and_then(|p| p.analysis.as_ref()).and_then(|a| a.faces.clone()) }) });
        let flag = s.catalog.photo(*id).map(|p| p.flag).unwrap_or_default();
        let fs = summaries[i];
        let closed = fs.is_some_and(|f| f.closed);
        closed_eyes += closed as usize;
        if (reject_below.is_some_and(|t| *sharp < t) || (reject_closed && closed)) && flag != Flag::Reject {
            ops.push(Op::SetFlag { id: *id, flag: Flag::Reject });
            rejected += 1;
        } else if pick_best && is_best && flag == Flag::None {
            ops.push(Op::SetFlag { id: *id, flag: Flag::Pick });
            picked += 1;
        }
        let mut row = json!({"id": id.0, "sharpness": (sharp * 10.0).round() / 10.0, "clipped": clip, "group": g, "best": is_best});
        if let (Some(f), Some(o)) = (fs, row.as_object_mut()) {
            o.insert("faces".into(), json!(f.judged));
            o.insert("eyesClosed".into(), json!(f.closed));
            o.insert("faceQuality".into(), json!((f.quality * 10.0).round() / 10.0));
        }
        out.push(row);
    }
    if !ops.is_empty() {
        s.commit("Assisted Culling", Op::Batch { ops })?;
    }
    Ok(json!({"photos": out, "groups": best.len(), "rejected": rejected, "picked": picked, "closedEyes": closed_eyes, "facesPending": faces_pending, "failed": failed}))
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
            "{ids?, rejectBelow?: sharpness 0..100, pickBest?: bool, rejectClosedEyes?: bool, faces?: bool} — score the selected photos (else all in view) for focus (0..100) and clipping, group look-alike shots taken within 10 s and mark the best of each (the sharpest; when the shots have faces, the one with open, sharp eyes: see photo.analyzeFaces); optionally reject blurry photos, reject photos with a closed eye (`rejectClosedEyes`, which analyzes faces first: needs the face model) and pick each group's best; one undo step → {photos: [{id, sharpness, clipped, group, best, faces?, eyesClosed?, faceQuality?}], groups, rejected, picked, closedEyes, facesPending}",
            always,
            analyze
        ),
    ]
}
