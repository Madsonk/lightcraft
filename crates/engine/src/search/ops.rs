//! The search commands' work (feature `search`): indexing, search by meaning, zero-shot labels,
//! near-duplicates.

use std::collections::{HashMap, HashSet};

use lightcraft_catalog::{MediaKind, PhotoId};
use lightcraft_search::hash;
use rayon::prelude::*;
use serde_json::{Value, json};

use super::Searcher;
use super::core::Item;
use crate::cmd::{bad, bool_or, f64_or, str_param};
use crate::media::SourceLevel;
use crate::{EngineError, Result, Session};

/// Probability above which a photo gets a label by default. SigLIP's probabilities are low for
/// ordinary photos (a clear match often reads 0.02–0.4): calibrated on the test library, see
/// `docs/semantic-search.md`.
pub const DEFAULT_TAG_THRESHOLD: f64 = 0.02;

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

/// The photos a command works on: `ids` when given, else every photo of the library.
fn scope(s: &Session, p: &Value) -> Vec<PhotoId> {
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() { s.targets(p).unwrap_or_default() } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
    ids.into_iter().filter(|i| s.catalog.photo(*i).is_some_and(|q| q.kind != MediaKind::Video)).collect()
}

fn key(s: &Session, id: PhotoId) -> Option<u64> {
    s.catalog.photo(id).map(|p| lightcraft_search::key_of(&crate::media::content_key(p)))
}

/// `(id, key)` of `ids` (photos with the same content share a key).
fn keyed(s: &Session, ids: &[PhotoId]) -> Vec<(PhotoId, u64)> {
    ids.iter().filter_map(|i| key(s, *i).map(|k| (*i, k))).collect()
}

/// Decode jobs for the photos that miss what `want` asks for (first photo of each content).
fn items_for(s: &mut Session, keyed: &[(PhotoId, u64)], dir: Option<std::path::PathBuf>, vectors: bool, force: bool) -> Result<(Vec<Item>, usize)> {
    let needs: Vec<(PhotoId, u64, bool, bool)> = s
        .searcher
        .with_store(dir, |st| {
            let mut seen = HashSet::new();
            keyed
                .iter()
                .filter(|(_, k)| seen.insert(*k))
                .map(|(i, k)| (*i, *k, vectors && (force || !st.vectors.contains(*k)), force || st.hashes.get(*k).is_none()))
                .collect()
        })
        .map_err(other)?;
    let total = needs.len();
    let mut items = Vec::new();
    for (id, k, want_vector, want_hash) in needs {
        if !(want_vector || want_hash) {
            continue;
        }
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        let source = s.media.source_ref(&ph, SourceLevel::Thumb);
        items.push(Item { id, key: k, source, want_vector, want_hash });
    }
    Ok((items, total))
}

/// Embed the pictures that have no vector yet, waiting for it; how many were embedded.
pub(crate) fn embed_pictures(searcher: &Searcher, dir: Option<std::path::PathBuf>, pics: Vec<super::Picture>) -> std::result::Result<usize, String> {
    let mut seen = HashSet::new();
    let missing: Vec<super::Picture> =
        searcher.with_store(dir.clone(), |st| pics.into_iter().filter(|p| !st.vectors.contains(p.key) && seen.insert(p.key)).collect())?;
    let n = missing.len();
    if n > 0 {
        let items = missing.into_iter().map(|p| Item { id: p.id, key: p.key, source: p.source, want_vector: true, want_hash: false }).collect();
        searcher.start_job(dir, items, false)?;
    }
    Ok(n)
}

/// The vectors the search index has for `keys`.
pub(crate) fn vectors_of(searcher: &Searcher, dir: Option<std::path::PathBuf>, keys: &[u64]) -> std::result::Result<HashMap<u64, Vec<f32>>, String> {
    searcher.with_store(dir, |st| keys.iter().filter_map(|k| st.vectors.get(*k).map(|v| (*k, v.to_vec()))).collect())
}

/// The pictures cut out of the videos among `ids` (default: the library's), for the search index.
fn frame_items(s: &mut Session, p: &Value, dir: Option<std::path::PathBuf>, force: bool) -> Result<Vec<Item>> {
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() { s.targets(p)? } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
    let loader = s.media.file_loader.clone();
    let mut items = Vec::new();
    let videos: Vec<PhotoId> = ids.into_iter().filter(|i| s.catalog.photo(*i).is_some_and(|q| q.kind == MediaKind::Video)).collect();
    for id in videos {
        let Some(ck) = s.catalog.photo(id).map(|q| crate::media::content_key(q)) else { continue };
        for (_, f) in s.frames_of_video(id) {
            let source = crate::media::SourceRef::File {
                path: f.file.display().to_string(),
                max_edge: SourceLevel::Thumb.max_edge(),
                loader: loader.clone(),
                fallback: None,
                denoise: None,
                reduced: None,
                camera_profile: None,
                outliers: false,
            };
            items.push(Item { id, key: super::search_key(&ck, Some(f.ms)), source, want_vector: true, want_hash: false });
        }
    }
    let have: HashSet<u64> =
        s.searcher.with_store(dir, |st| items.iter().map(|i| i.key).filter(|k| st.vectors.contains(*k)).collect()).map_err(other)?;
    Ok(items.into_iter().filter(|i| force || !have.contains(&i.key)).collect())
}

pub fn index(s: &mut Session, p: &Value) -> Result<Value> {
    s.searcher.model_dir().map_err(other)?;
    let (force, background) = (bool_or(p, "force", false), bool_or(p, "background", false));
    let ids = scope(s, p);
    let dir = super::Searcher::store_dir(s.library.as_ref());
    let kd = keyed(s, &ids);
    let (mut items, distinct) = items_for(s, &kd, dir.clone(), true, force)?;
    // and the pictures cut out of videos (video.frames), each under its moment's key
    let mut video_frames = 0;
    let photos_queued = items.len();
    if bool_or(p, "videos", true) {
        let f = frame_items(s, p, dir.clone(), force)?;
        video_frames = f.len();
        items.extend(f);
    }
    let queued = items.len();
    if queued > 0 {
        s.searcher.start_job(dir.clone(), items, background).map_err(other)?;
    }
    let st = s.searcher.job_status();
    let vectors = s.searcher.with_store(dir, |st| st.vectors.len()).map_err(other)?;
    Ok(json!({
        "queued": queued,
        "alreadyIndexed": distinct - photos_queued.min(distinct),
        "background": background && queued > 0,
        "done": if background { Value::Null } else { json!(st.done) },
        "failed": st.failed,
        "failures": st.failures.iter().take(20).map(|(i, e)| json!({"id": i, "error": e})).collect::<Vec<_>>(),
        "seconds": st.seconds,
        "phaseSeconds": st.phase_seconds,
        "secondsPerPhoto": if st.done > 0 && !background { json!(st.seconds / st.done as f64) } else { Value::Null },
        "error": st.error,
        "vectors": vectors,
        "videoFrames": video_frames,
    }))
}

/// `{photos: [{id, score, probability}]}` for the photos whose vectors are closest to `query`.
pub fn semantic(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "search.semantic";
    let query = str_param(p, "query").map(str::trim).filter(|q| !q.is_empty()).ok_or_else(|| bad(C, "missing `query`"))?.to_string();
    let limit = f64_or(p, "limit", 50.0).clamp(1.0, 5000.0) as usize;
    let min = p.get("minScore").and_then(Value::as_f64).map(|v| v as f32);
    let ids = scope(s, p);
    let dir = Searcher::store_dir(s.library.as_ref());
    let mut by_key: HashMap<u64, Vec<PhotoId>> = HashMap::new();
    for (i, k) in keyed(s, &ids) {
        by_key.entry(k).or_default().push(i);
    }
    let q = s.searcher.embed_texts(&[&query]).map_err(other)?.pop().ok_or_else(|| other("no embedding"))?;
    let (top, indexed) = s
        .searcher
        .with_store(dir, |st| (st.vectors.top(&q, limit, &|k| by_key.contains_key(&k)), by_key.keys().filter(|k| st.vectors.contains(**k)).count()))
        .map_err(other)?;
    let mut photos = Vec::new();
    for (k, cos) in top {
        if min.is_some_and(|m| cos < m) {
            continue;
        }
        for id in by_key.get(&k).into_iter().flatten() {
            photos.push((*id, cos));
        }
    }
    photos.truncate(limit);
    if bool_or(p, "filter", false) {
        s.filter.only = photos.iter().map(|h| h.0).collect();
    }
    let prob = |c: f32| s.searcher.probability(c).map(|v| (v * 1000.0).round() / 1000.0);
    Ok(json!({
        "query": query,
        "photos": photos.iter().map(|(i, c)| json!({"id": i.0, "score": (c * 1000.0).round() / 1000.0, "probability": prob(*c)})).collect::<Vec<_>>(),
        "indexed": indexed,
        "unindexed": by_key.len() - indexed,
    }))
}

/// Zero-shot labels: every photo whose probability for a label reaches `threshold` gets the label
/// as a keyword.
pub fn tag(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "search.tag";
    let labels: Vec<String> = p
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    if labels.is_empty() {
        return Err(bad(C, "`labels` must list at least one label"));
    }
    let threshold = f64_or(p, "threshold", DEFAULT_TAG_THRESHOLD).clamp(0.0, 1.0) as f32;
    let template = str_param(p, "template").unwrap_or("a photo of {}").to_string();
    let dry = bool_or(p, "dryRun", false);
    let ids = scope(s, p);
    let dir = Searcher::store_dir(s.library.as_ref());
    let kd = keyed(s, &ids);
    let prompts: Vec<String> = labels.iter().map(|l| template.replace("{}", l)).collect();
    let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
    let text = s.searcher.embed_texts(&refs).map_err(other)?;
    // probability of each label for each indexed photo
    let (scores, unindexed): (Vec<(PhotoId, Vec<f32>)>, usize) = {
        let probs = |cos: f32| s.searcher.probability(cos).unwrap_or(0.0);
        s.searcher
            .with_store(dir, |st| {
                let mut out = Vec::new();
                let mut missing = 0;
                for (id, k) in &kd {
                    match st.vectors.get(*k) {
                        Some(v) => out.push((*id, text.iter().map(|t| probs(lightcraft_search::index::dot(v, t))).collect())),
                        None => missing += 1,
                    }
                }
                (out, missing)
            })
            .map_err(other)?
    };
    let mut report = Vec::new();
    for (li, label) in labels.iter().enumerate() {
        let mut hits: Vec<(PhotoId, f32)> = scores.iter().filter(|(_, v)| v[li] >= threshold).map(|(i, v)| (*i, v[li])).collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut best: Vec<(PhotoId, f32)> = scores.iter().map(|(i, v)| (*i, v[li])).collect();
        best.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut tagged = 0;
        if !dry && !hits.is_empty() {
            let r = s.execute("photo.setMeta", &json!({"ids": hits.iter().map(|h| h.0.0).collect::<Vec<_>>(), "addKeywords": [label]}))?;
            tagged = r.get("changed").and_then(Value::as_u64).unwrap_or(hits.len() as u64);
        }
        report.push(json!({
            "label": label,
            "matched": hits.len(),
            "tagged": tagged,
            "ids": hits.iter().take(100).map(|h| h.0.0).collect::<Vec<_>>(),
            "best": best.iter().take(5).map(|(i, v)| json!({"id": i.0, "probability": (v * 1000.0).round() / 1000.0})).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"labels": report, "threshold": threshold, "dryRun": dry, "scored": scores.len(), "unindexed": unindexed}))
}

/// A set of near-identical photos.
pub struct Group {
    pub ids: Vec<PhotoId>,
    /// Most differing hash bits between photos joined (0..=128).
    pub distance: u32,
    /// Lowest cosine between photos joined, when both have vectors.
    pub cosine: Option<f32>,
    /// Every join was confirmed by the embeddings (else the hash alone decided).
    pub confirmed: bool,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b] = a;
        }
    }
}

/// Near-duplicates among `ids`: photos whose perceptual hashes differ by at most `max_distance`
/// bits, kept when their embeddings (if both have them) are at least `min_cosine` alike. Hashes
/// are computed now for photos that lack them (no model needed).
pub fn near(s: &mut Session, ids: &[PhotoId], max_distance: u32, min_cosine: f32) -> Result<Vec<Group>> {
    let dir = Searcher::store_dir(s.library.as_ref());
    let kd = keyed(s, ids);
    let (items, _) = items_for(s, &kd, dir.clone(), false, false)?;
    if !items.is_empty() {
        s.searcher.start_job(dir.clone(), items, false).map_err(other)?;
        let st = s.searcher.job_status();
        if let Some(e) = st.error {
            return Err(other(e));
        }
    }
    let mut by_key: Vec<(u64, Vec<PhotoId>)> = Vec::new();
    let mut at: HashMap<u64, usize> = HashMap::new();
    for (i, k) in &kd {
        let slot = *at.entry(*k).or_insert_with(|| {
            by_key.push((*k, Vec::new()));
            by_key.len() - 1
        });
        by_key[slot].1.push(*i);
    }
    let rows: Vec<(usize, [u64; 2], Option<Vec<f32>>)> = s
        .searcher
        .with_store(dir, |st| {
            by_key.iter().enumerate().filter_map(|(n, (k, _))| st.hashes.get(*k).map(|h| (n, h, st.vectors.get(*k).map(<[f32]>::to_vec)))).collect()
        })
        .map_err(other)?;
    // every pair close in hash bits
    let pairs: Vec<(usize, usize, u32)> = (0..rows.len())
        .into_par_iter()
        .flat_map_iter(|i| {
            let hi = rows[i].1;
            rows[i + 1..].iter().enumerate().filter_map(move |(o, r)| {
                let d = hash::distance(hi, r.1);
                (d <= max_distance).then_some((i, i + 1 + o, d))
            })
        })
        .collect();
    let mut dsu = Dsu((0..rows.len()).collect());
    let mut edges: Vec<(usize, u32, Option<f32>)> = Vec::new();
    for (a, b, d) in pairs {
        let cos = match (&rows[a].2, &rows[b].2) {
            (Some(x), Some(y)) => Some(lightcraft_search::index::dot(x, y)),
            _ => None,
        };
        if cos.is_some_and(|c| c < min_cosine) {
            continue;
        }
        dsu.union(a, b);
        edges.push((a, d, cos));
    }
    let mut groups: HashMap<usize, Group> = HashMap::new();
    for (a, d, cos) in edges {
        let root = dsu.find(a);
        let g = groups.entry(root).or_insert(Group { ids: Vec::new(), distance: 0, cosine: None, confirmed: true });
        g.distance = g.distance.max(d);
        g.cosine = match (g.cosine, cos) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        };
        g.confirmed &= cos.is_some();
    }
    for (n, row) in rows.iter().enumerate() {
        let root = dsu.find(n);
        if let Some(g) = groups.get_mut(&root) {
            g.ids.extend(by_key[row.0].1.iter().copied());
        }
    }
    let mut out: Vec<Group> = groups.into_values().filter(|g| g.ids.len() >= 2).collect();
    for g in &mut out {
        g.ids.sort_by_key(|i| (s.catalog.photo(*i).and_then(|p| p.captured.clone()), i.0));
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.ids.len()));
    Ok(out)
}

/// `search.status`: the model, the index and the indexing run.
pub fn status(s: &Session) -> Value {
    let dir = Searcher::store_dir(s.library.as_ref());
    let ids: Vec<PhotoId> = s.catalog.photos().filter(|q| q.in_library() && q.kind != MediaKind::Video).map(|q| q.id).collect();
    let keys: HashSet<u64> = ids.iter().filter_map(|i| key(s, *i)).collect();
    let counts = s.searcher.with_store(dir.clone(), |st| {
        (
            st.model.clone(),
            st.vectors.len(),
            st.hashes.len(),
            keys.iter().filter(|k| st.vectors.contains(**k)).count(),
            keys.iter().filter(|k| st.hashes.get(**k).is_some()).count(),
        )
    });
    let index = match counts {
        Ok((model, vectors, hashes, have_v, have_h)) => json!({
            "model": model, "vectorsStored": vectors, "hashesStored": hashes,
            "photos": keys.len(), "indexed": have_v, "hashed": have_h, "unindexed": keys.len() - have_v,
            "file": dir.map(|d| d.display().to_string()),
        }),
        Err(e) => json!({"error": e}),
    };
    json!({"available": true, "installed": s.searcher.installed(), "dir": s.searcher.model_folder().map(|d| d.display().to_string()),
        "loaded": s.searcher.loaded(), "index": index, "job": s.searcher.job_status()})
}
