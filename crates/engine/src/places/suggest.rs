//! Places for photos that have none: by neighbours in time (the same camera, a few minutes from a photo that has a
//! place) and by looks (the photos most like it, among those whose place is known: SigLIP embeddings from the search
//! index). A suggestion is only ever proposed; `places.confirm` writes it.

use std::collections::HashMap;

use lightcraft_catalog::{Photo, PhotoId, keywords};
use serde_json::{Value, json};

use super::{other, place_keywords, resolve};
use crate::{Result, Session};

/// A proposed place.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// The keyword, `Places|France|New Aquitaine|Poitiers`.
    pub place: String,
    /// `time` or `visual`.
    pub source: &'static str,
    /// 0..1. Not a probability: see `docs/places.md` for how each source computes it.
    pub confidence: f32,
    /// The photos it rests on: neighbour in time (seconds apart) or look-alike (cosine similarity).
    pub evidence: Vec<(PhotoId, f64)>,
    /// Another place the evidence also points to (neighbours before and after that disagree).
    pub conflict: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SuggestOptions {
    pub window_secs: i64,
    pub same_camera: bool,
    pub time: bool,
    pub visual: bool,
    /// Look-alikes consulted per photo.
    pub k: usize,
    /// Lowest confidence kept; `None` = the default of each source ([`MIN_TIME`], [`MIN_LOOK`]).
    pub min_confidence: Option<f32>,
    pub max_km: f64,
}

impl Default for SuggestOptions {
    fn default() -> Self {
        SuggestOptions {
            window_secs: 30 * 60,
            same_camera: true,
            time: true,
            visual: true,
            k: 7,
            min_confidence: None,
            max_km: super::DEFAULT_MAX_KM,
        }
    }
}

/// Seconds since 1970 of an ISO 8601 local time (`2026-04-12T14:03:22…`), ignoring any zone or fraction.
pub fn parse_secs(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let s = b.get(from..from + len)?;
        if s.iter().all(u8::is_ascii_digit) { std::str::from_utf8(s).ok()?.parse().ok() } else { None }
    };
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hh, mm, ss) = (num(11, 2).unwrap_or(0), num(14, 2).unwrap_or(0), num(17, 2).unwrap_or(0));
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // days from civil (Howard Hinnant's algorithm)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The place of a photo that has one: its deepest place keyword, else what its GPS resolves to.
pub fn known_place(p: &Photo, max_km: f64) -> Option<String> {
    let kw = place_keywords(p).into_iter().max_by_key(|k| k.matches(keywords::SEP).count());
    if let Some(k) = kw {
        return Some(k.clone());
    }
    let (lat, lon) = p.meta.gps?;
    resolve(lat, lon, max_km).ok().flatten().map(|r| r.keyword)
}

/// Whether the photo already has a place or a position (nothing to suggest).
fn is_placed(p: &Photo) -> bool {
    p.meta.gps.is_some() || !place_keywords(p).is_empty()
}

struct Anchor {
    secs: i64,
    id: PhotoId,
    place: String,
}

/// Suggestions from neighbours in time.
fn by_time(s: &Session, scope: &[PhotoId], o: &SuggestOptions) -> HashMap<PhotoId, Suggestion> {
    let mut anchors: HashMap<String, Vec<Anchor>> = HashMap::new();
    for p in s.catalog.photos().filter(|p| p.in_library()) {
        let Some(secs) = p.captured.as_deref().and_then(parse_secs) else { continue };
        let Some(place) = known_place(p, o.max_km) else { continue };
        let cam = if o.same_camera { p.meta.camera.clone() } else { String::new() };
        anchors.entry(cam).or_default().push(Anchor { secs, id: p.id, place });
    }
    for v in anchors.values_mut() {
        v.sort_by_key(|a| (a.secs, a.id));
    }
    let mut out = HashMap::new();
    for id in scope {
        let Some(p) = s.catalog.photo(*id) else { continue };
        if !p.in_library() || is_placed(p) {
            continue;
        }
        let Some(secs) = p.captured.as_deref().and_then(parse_secs) else { continue };
        let cam = if o.same_camera { p.meta.camera.clone() } else { String::new() };
        let Some(list) = anchors.get(&cam) else { continue };
        let i = list.partition_point(|a| a.secs <= secs);
        let before = i.checked_sub(1).and_then(|j| list.get(j)).filter(|a| secs - a.secs <= o.window_secs);
        let after = list.get(i).filter(|a| a.secs - secs <= o.window_secs);
        let gap = |a: &Anchor| (a.secs - secs).abs();
        let (best, other) = match (before, after) {
            (Some(b), Some(a)) => {
                if gap(b) <= gap(a) {
                    (b, Some(a))
                } else {
                    (a, Some(b))
                }
            }
            (Some(x), None) | (None, Some(x)) => (x, None),
            (None, None) => continue,
        };
        let window = o.window_secs.max(1) as f32;
        let mut conf = 0.9 * (1.0 - gap(best) as f32 / window);
        let mut conflict = None;
        let mut evidence = vec![(best.id, gap(best) as f64)];
        if let Some(a) = other {
            evidence.push((a.id, gap(a) as f64));
            if a.place == best.place {
                conf = (conf + 0.08).min(0.95);
            } else {
                conf *= 0.5;
                conflict = Some(a.place.clone());
            }
        }
        out.insert(*id, Suggestion { place: best.place.clone(), source: "time", confidence: conf.clamp(0.0, 1.0), evidence, conflict });
    }
    out
}

/// Suggestions from look-alikes (needs the search model and an index of the photos involved).
#[cfg(feature = "search")]
fn by_look(s: &mut Session, scope: &[PhotoId], o: &SuggestOptions) -> Result<(HashMap<PhotoId, Suggestion>, usize)> {
    use lightcraft_catalog::MediaKind;
    use rayon::prelude::*;
    s.searcher.model_dir().map_err(other)?;
    let dir = crate::search::Searcher::store_dir(s.library.as_ref());
    let key = |p: &Photo| lightcraft_search::key_of(&crate::media::content_key(p));
    // reference photos: a known place, one per content
    let mut refs: Vec<(u64, PhotoId, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in s.catalog.photos().filter(|p| p.in_library() && p.kind != MediaKind::Video) {
        if let Some(place) = known_place(p, o.max_km)
            && seen.insert(key(p))
        {
            refs.push((key(p), p.id, place));
        }
    }
    let cands: Vec<(PhotoId, u64)> = scope
        .iter()
        .filter_map(|id| s.catalog.photo(*id))
        .filter(|p| p.in_library() && p.kind != MediaKind::Video && !is_placed(p))
        .map(|p| (p.id, key(p)))
        .collect();
    let k = o.k.max(1);
    let (found, unindexed) = s
        .searcher
        .with_store(dir, |st| {
            let reference: Vec<(&[f32], &(u64, PhotoId, String))> = refs.iter().filter_map(|r| st.vectors.get(r.0).map(|v| (v, r))).collect();
            let mut unindexed = 0;
            let mut work: Vec<(PhotoId, &[f32])> = Vec::new();
            for (id, kk) in &cands {
                match st.vectors.get(*kk) {
                    Some(v) => work.push((*id, v)),
                    None => unindexed += 1,
                }
            }
            let res: Vec<(PhotoId, Suggestion)> = work
                .par_iter()
                .filter_map(|(id, v)| {
                    let mut sims: Vec<(f32, usize)> =
                        reference.iter().enumerate().map(|(i, (rv, _))| (lightcraft_search::index::dot(v, rv), i)).collect();
                    sims.sort_by(|a, b| b.0.total_cmp(&a.0));
                    sims.truncate(k);
                    let top = sims.first()?.0;
                    let neighbours: Vec<(f32, &(u64, PhotoId, String))> =
                        sims.iter().filter_map(|(c, i)| reference.get(*i).map(|(_, r)| (*c, *r))).collect();
                    vote(&neighbours, top).map(|sg| (*id, sg))
                })
                .collect();
            (res, unindexed)
        })
        .map_err(other)?;
    Ok((found.into_iter().collect(), unindexed))
}

#[cfg(not(feature = "search"))]
fn by_look(_: &mut Session, _: &[PhotoId], _: &SuggestOptions) -> Result<(HashMap<PhotoId, Suggestion>, usize)> {
    Err(other(crate::search::UNAVAILABLE))
}

/// The deepest place holding at least [`MIN_SHARE`] of the neighbours' weight, with a confidence that also grows with how
/// alike the closest neighbour is.
#[cfg(feature = "search")]
fn vote(neigh: &[(f32, &(u64, PhotoId, String))], top: f32) -> Option<Suggestion> {
    const MIN_SHARE: f32 = 0.6;
    // neighbours within ~0.05 cosine of the best weigh the most
    let w: Vec<f32> = neigh.iter().map(|(c, _)| ((c - top) / 0.05).exp()).collect();
    let total: f32 = w.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut share: HashMap<String, f32> = HashMap::new();
    for ((_, r), w) in neigh.iter().zip(&w) {
        let mut path = String::new();
        for (i, part) in r.2.split(keywords::SEP).enumerate() {
            if i > 0 {
                path.push(keywords::SEP);
            }
            path.push_str(part);
            // the root and the country level only: `Places|France` and below
            if i >= 1 {
                *share.entry(path.clone()).or_default() += w / total;
            }
        }
    }
    let (place, sh) = share
        .into_iter()
        .filter(|(_, s)| *s >= MIN_SHARE)
        .max_by(|a, b| a.0.matches(keywords::SEP).count().cmp(&b.0.matches(keywords::SEP).count()).then(a.1.total_cmp(&b.1)))?;
    let strength = ((top - LOOK_FLOOR) / (LOOK_FULL - LOOK_FLOOR)).clamp(0.0, 1.0);
    let evidence = neigh.iter().filter(|(_, r)| keywords::is_under(&r.2, &place)).map(|(c, r)| (r.1, f64::from(*c))).collect();
    Some(Suggestion { place, source: "visual", confidence: (sh * strength).clamp(0.0, 1.0), evidence, conflict: None })
}

/// Cosine similarity of the closest look-alike below which a suggestion has no strength, and from which it has all of it.
#[cfg(feature = "search")]
pub const LOOK_FLOOR: f32 = 0.78;
#[cfg(feature = "search")]
pub const LOOK_FULL: f32 = 0.92;

/// Default lowest confidence kept for suggestions by time (99 % exact in the measurements of `docs/places.md`, so low).
pub const MIN_TIME: f32 = 0.2;
/// Default lowest confidence kept for suggestions by look (below it, photos from other days were mostly wrong).
pub const MIN_LOOK: f32 = 0.5;

/// Compute suggestions for `scope`, keep the best per photo above `min_confidence`, remember them in the session.
pub fn suggest(s: &mut Session, scope: &[PhotoId], o: &SuggestOptions) -> Result<Value> {
    let mut notes: Vec<String> = Vec::new();
    let mut best: HashMap<PhotoId, Suggestion> = HashMap::new();
    let mut unindexed = 0;
    if o.time {
        best = by_time(s, scope, o);
    }
    if o.visual {
        match by_look(s, scope, o) {
            Ok((v, un)) => {
                unindexed = un;
                for (id, sg) in v {
                    if best.get(&id).is_none_or(|b| b.confidence < sg.confidence) {
                        best.insert(id, sg);
                    }
                }
            }
            Err(e) => notes.push(format!("visual suggestions skipped: {e}")),
        }
    }
    best.retain(|id, sg| {
        let floor = o.min_confidence.unwrap_or(if sg.source == "time" { MIN_TIME } else { MIN_LOOK });
        sg.confidence >= floor && !s.places.dismissed.contains(id)
    });
    s.places.suggestions.retain(|id, _| !scope.contains(id));
    for (id, sg) in &best {
        s.places.suggestions.insert(*id, sg.clone());
    }
    let mut list: Vec<(&PhotoId, &Suggestion)> = best.iter().collect();
    list.sort_by(|a, b| b.1.confidence.total_cmp(&a.1.confidence).then(a.0.cmp(b.0)));
    let rows: Vec<Value> = list
        .iter()
        .map(|(id, sg)| {
            json!({
                "id": id.0,
                "fileName": s.catalog.photo(**id).map(|p| p.file_name.clone()),
                "place": sg.place,
                "source": sg.source,
                "confidence": (sg.confidence * 1000.0).round() / 1000.0,
                "evidence": sg.evidence.iter().map(|(i, v)| json!({"id": i.0, "value": (v * 1000.0).round() / 1000.0})).collect::<Vec<_>>(),
                "conflict": sg.conflict,
            })
        })
        .collect();
    let mut by_source: HashMap<&str, usize> = HashMap::new();
    for sg in best.values() {
        *by_source.entry(sg.source).or_default() += 1;
    }
    let candidates = scope.iter().filter_map(|i| s.catalog.photo(*i)).filter(|p| p.in_library() && !is_placed(p)).count();
    Ok(json!({
        "candidates": candidates,
        "suggested": rows.len(),
        "bySource": by_source,
        "unindexed": unindexed,
        "suggestions": rows,
        "notes": notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::parse_secs;

    #[test]
    fn iso_times() {
        assert_eq!(parse_secs("1970-01-01T00:00:00"), Some(0));
        assert_eq!(parse_secs("2026-10-09T12:30:15"), Some(1_791_549_015));
        assert_eq!(parse_secs("2026-10-09T12:30:15.250+02:00"), Some(1_791_549_015));
        assert_eq!(parse_secs("2026-10-09"), Some(1_791_504_000));
        assert_eq!(parse_secs("2026-13-09T00:00:00"), None);
        assert_eq!(parse_secs("garbage"), None);
        assert_eq!(parse_secs(""), None);
    }
}
