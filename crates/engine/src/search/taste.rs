//! Learn the user's taste from their stars and suggest which photos to pick (feature `search`).
//!
//! Features per photo: the SigLIP 2 embedding the search index already holds, plus five cheap
//! measurements of the thumbnail (focus, clipping, mean / dark / bright luminance) kept in
//! `<library>/search/taste-extras.bin`. Labels come from the catalog: stars (1–5), rejected
//! photos (0) and — optionally — unrated photos from shoots the user clearly went through
//! (counted as "not chosen", at a lower weight). The model (`taste.json`, next to the vectors)
//! is a ridge regression with its regularisation chosen by cross-validation over whole shoots.
//! It only ever *suggests*: `culling.accept` is the one command that writes stars or flags, and
//! it refuses without `confirm: true`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::{MediaKind, PhotoId, Source};
use lightcraft_search::records::RecordFile;
use lightcraft_taste::TasteModel;
use lightcraft_taste::eval::{Config, Kind, Sample};
use rayon::prelude::*;
use serde_json::{Value, json};

use super::Searcher;
use crate::cmd::{bad, bool_or, f64_or, str_param};
use crate::media::SourceLevel;
use crate::{EngineError, Result, Session};

/// Names of the measurements that follow the embedding in the feature vector.
pub const EXTRAS: [&str; 5] = ["sharpness", "clipped", "luma_mean", "luma_p05", "luma_p95"];
/// A folder counts as "gone through" when at least this many of its photos have stars...
pub const REVIEWED_MIN_RATED: usize = 5;
/// ...and they are at least this share of the folder.
pub const REVIEWED_MIN_SHARE: f64 = 0.10;
/// How much an unrated photo of a reviewed shoot counts, against 1 for a rated one.
pub const IMPLICIT_WEIGHT: f32 = 0.4;

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn model_path(dir: &Option<PathBuf>) -> Option<PathBuf> {
    dir.as_ref().map(|d| d.join("taste.json"))
}

/// The stored model of the library (or the in-memory one when the library is not on disk).
pub fn load_model(s: &Session) -> Option<Arc<TasteModel>> {
    let dir = Searcher::store_dir(s.library.as_ref());
    if let Some(p) = model_path(&dir) {
        let text = std::fs::read_to_string(&p).ok()?;
        return TasteModel::from_json(&text).ok().map(Arc::new);
    }
    s.searcher.taste_mem()
}

fn save_model(s: &Session, m: &TasteModel) -> Result<Option<PathBuf>> {
    let dir = Searcher::store_dir(s.library.as_ref());
    match model_path(&dir) {
        Some(p) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| other(format!("taste model: {e}")))?;
            }
            let tmp = p.with_extension("json.part");
            std::fs::write(&tmp, m.to_json()).map_err(|e| other(format!("taste model: {e}")))?;
            std::fs::rename(&tmp, &p).map_err(|e| other(format!("taste model: {e}")))?;
            Ok(Some(p))
        }
        None => {
            s.searcher.set_taste_mem(Arc::new(m.clone()));
            Ok(None)
        }
    }
}

// ---------- the measurements

const EXTRA_TAG: &str = "taste-extras-1";
const EXTRA_REC: usize = 8 + 5 * 4;

fn read_extras(path: &Path) -> HashMap<u64, [f32; 5]> {
    let mut out = HashMap::new();
    if let Ok((_f, bytes)) = RecordFile::open(path, b"LCTE", EXTRA_TAG, EXTRA_REC) {
        for r in bytes.as_chunks::<EXTRA_REC>().0 {
            let key = u64::from_le_bytes(r[..8].try_into().unwrap_or_default());
            let mut v = [0f32; 5];
            for (i, c) in r[8..].as_chunks::<4>().0.iter().enumerate().take(5) {
                v[i] = f32::from_le_bytes(*c);
            }
            out.insert(key, v);
        }
    }
    out
}

/// The measurements of `keyed` photos: from the cache, the rest computed from their thumbnails
/// (in parallel) and appended to it. Photos that cannot be read are missing from the result.
fn extras_for(s: &mut Session, keyed: &[(PhotoId, u64)], dir: &Option<PathBuf>) -> HashMap<u64, [f32; 5]> {
    let path = dir.as_ref().map(|d| d.join("taste-extras.bin"));
    let mut have = path.as_deref().map(read_extras).unwrap_or_default();
    let mut todo = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (id, k) in keyed {
        if !have.contains_key(k)
            && seen.insert(*k)
            && let Some(ph) = s.catalog.photo(*id).cloned()
        {
            todo.push((*k, s.media.source_ref(&ph, SourceLevel::Thumb)));
        }
    }
    let computed: Vec<(u64, [f32; 5])> = todo
        .par_iter()
        .filter_map(|(k, src)| {
            let img = src.load_source().ok()?.image;
            let e = lightcraft_pipeline::cull::exposure(&img);
            Some((*k, [lightcraft_pipeline::cull::sharpness(&img), lightcraft_pipeline::cull::clipped(&img), e[0], e[1], e[2]]))
        })
        .collect();
    if let Some(p) = &path
        && !computed.is_empty()
        && let Ok((mut f, _)) = RecordFile::open(p, b"LCTE", EXTRA_TAG, EXTRA_REC)
    {
        let mut out = Vec::with_capacity(computed.len() * EXTRA_REC);
        for (k, v) in &computed {
            out.extend_from_slice(&k.to_le_bytes());
            v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
        }
        let _ = f.append(&out);
    }
    have.extend(computed);
    have
}

// ---------- the labels

struct Labeled {
    id: PhotoId,
    key: u64,
    kind: Kind,
    target: f32,
    shoot: u64,
    /// Capture time in seconds (coarse), when known.
    time: Option<i64>,
}

fn folder_of(s: &Session, id: PhotoId) -> Option<String> {
    match &s.catalog.photo(id)?.source {
        Source::File { path } => Some(path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string())),
        Source::Demo { .. } => None,
    }
}

/// Folders the user has gone through: (folder → (photos, rated)), reviewed ones only.
fn reviewed_folders(s: &Session) -> BTreeMap<String, (usize, usize)> {
    let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for p in s.catalog.photos().filter(|p| p.in_library() && p.kind != MediaKind::Video) {
        if let Some(d) = folder_of(s, p.id) {
            let e = m.entry(d).or_default();
            e.0 += 1;
            e.1 += (p.rating > 0) as usize;
        }
    }
    m.retain(|_, (n, r)| *r >= REVIEWED_MIN_RATED && *r as f64 >= REVIEWED_MIN_SHARE * *n as f64);
    m
}

fn labels(s: &Session, ids: &[PhotoId], implicit: bool) -> Vec<Labeled> {
    let reviewed = if implicit { reviewed_folders(s) } else { BTreeMap::new() };
    let mut out = Vec::new();
    for id in ids {
        let Some(p) = s.catalog.photo(*id) else { continue };
        if !p.in_library() || p.kind == MediaKind::Video {
            continue;
        }
        let folder = folder_of(s, *id);
        let group = folder.as_deref().map_or(0, lightcraft_search::key_of);
        let key = lightcraft_search::key_of(&crate::media::content_key(p));
        let (kind, target) = if p.flag == lightcraft_catalog::Flag::Reject {
            (Kind::Rejected, 0.0)
        } else if p.rating > 0 {
            (Kind::Rated, p.rating as f32)
        } else if folder.as_ref().is_some_and(|d| reviewed.contains_key(d)) {
            (Kind::Implicit, 0.0)
        } else {
            continue;
        };
        let time = p.captured.as_deref().and_then(lightcraft_meta::DateTime::parse_iso).map(|d| {
            ((d.year as i64 * 372 + d.month as i64 * 31 + d.day as i64) * 24 + d.hour as i64) * 3600 + d.minute as i64 * 60 + d.second as i64
        });
        out.push(Labeled { id: *id, key, kind, target, shoot: group, time });
    }
    out
}

fn all_photos(s: &Session) -> Vec<PhotoId> {
    s.catalog.photos().filter(|q| q.in_library() && q.kind != MediaKind::Video).map(|q| q.id).collect()
}

fn feature_vec(emb: &[f32], ex: &[f32; 5]) -> Vec<f32> {
    let mut f = Vec::with_capacity(emb.len() + 5);
    f.extend_from_slice(emb);
    f.extend_from_slice(ex);
    f
}

// ---------- taste.train

pub fn train(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "taste.train";
    let t0 = std::time::Instant::now();
    let implicit = bool_or(p, "implicitNegatives", true);
    let use_extras = bool_or(p, "extras", false);
    let split = match str_param(p, "split") {
        Some("burst") => "burst",
        Some("photo") => "photo",
        _ => "shoot",
    };
    let ids = if p.get("ids").is_some() { s.targets(p)? } else { all_photos(s) };
    let lab = labels(s, &ids, implicit);
    let dir = Searcher::store_dir(s.library.as_ref());
    // embeddings of the labelled photos
    let mut missing: Vec<PhotoId> = Vec::new();
    let mut vecs: HashMap<u64, Vec<f32>> = HashMap::new();
    let fetch = |s: &mut Session, vecs: &mut HashMap<u64, Vec<f32>>, missing: &mut Vec<PhotoId>| -> Result<String> {
        missing.clear();
        s.searcher
            .with_store(dir.clone(), |st| {
                for l in &lab {
                    match st.vectors.get(l.key) {
                        Some(v) => {
                            vecs.entry(l.key).or_insert_with(|| v.to_vec());
                        }
                        None => missing.push(l.id),
                    }
                }
                st.model.clone()
            })
            .map_err(other)
    };
    let mut embedding = fetch(s, &mut vecs, &mut missing)?;
    if !missing.is_empty() && bool_or(p, "index", false) {
        let ids: Vec<u64> = missing.iter().map(|i| i.0).collect();
        s.execute("search.index", &json!({"ids": ids, "background": false}))?;
        embedding = fetch(s, &mut vecs, &mut missing)?;
    }
    let unindexed = missing.len();
    let usable: Vec<&Labeled> = lab.iter().filter(|l| vecs.contains_key(&l.key)).collect();
    let kd: Vec<(PhotoId, u64)> = usable.iter().map(|l| (l.id, l.key)).collect();
    let ex = if use_extras { extras_for(s, &kd, &dir) } else { HashMap::new() };
    let mut samples = Vec::new();
    for l in &usable {
        let Some(emb) = vecs.get(&l.key) else { continue };
        let e = if use_extras {
            match ex.get(&l.key) {
                Some(e) => *e,
                None => continue,
            }
        } else {
            [0.0; 5]
        };
        let f = if use_extras { feature_vec(emb, &e) } else { emb.clone() };
        let w = if l.kind == Kind::Implicit { IMPLICIT_WEIGHT } else { 1.0 };
        let group = match split {
            "burst" => l.time.map_or(l.id.0 ^ 0x5bd1_e995, |t| l.shoot ^ ((t / 300) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
            _ => l.shoot,
        };
        samples.push(Sample { features: f, target: l.target, weight: w, group, shoot: l.shoot, kind: l.kind });
    }
    let mut cfg = Config { sharpness_col: use_extras.then_some(768), ..Default::default() };
    if let Some(f) = p.get("folds").and_then(Value::as_u64) {
        cfg.folds = (f as usize).clamp(2, 10);
    }
    cfg.by_shoot = split != "photo";
    if let Some(m) = p.get("minRated").and_then(Value::as_u64) {
        cfg.min_rated = (m as usize).max(5);
    }
    if let Some(a) = p.get("lambdas").and_then(Value::as_array) {
        let l: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|v| v as f32).filter(|v| *v > 0.0).collect();
        if !l.is_empty() {
            cfg.lambdas = l;
        }
    }
    let extras: Vec<String> = if use_extras { EXTRAS.iter().map(|x| x.to_string()).collect() } else { Vec::new() };
    let dim = samples.first().map_or(0, |x| x.features.len()) - extras.len().min(samples.first().map_or(0, |x| x.features.len()));
    let now = (s.clock)();
    let mut model = lightcraft_taste::train(&samples, &embedding, dim, &extras, &now, &cfg)
        .map_err(|e| bad(C, format!("{e} ({unindexed} labelled photos have no embedding yet: `search.index`, or pass `index: true`)")))?;
    model.metrics.split = split.into();
    let save = bool_or(p, "save", true);
    let path = if save { save_model(s, &model)? } else { None };
    Ok(json!({
        "trained": model.trained,
        "counts": model.counts,
        "metrics": metrics_json(&model),
        "unindexedLabelled": unindexed,
        "saved": path.map(|p| p.display().to_string()),
        "seconds": r3(t0.elapsed().as_secs_f64()),
    }))
}

fn metrics_json(m: &TasteModel) -> Value {
    let x = &m.metrics;
    let o = |v: Option<f64>| v.map(r3);
    json!({
        "split": x.split, "folds": x.folds, "lambda": x.lambda,
        "spearmanRated": o(x.spearman_rated), "spearmanAll": o(x.spearman_all),
        "aucChosen": o(x.auc_chosen), "topKHitRate": o(x.top_k_hit_rate), "topKChance": o(x.top_k_chance),
        "spearmanSharpnessOnly": o(x.spearman_sharpness_only),
        "tried": x.tried.iter().map(|(l, s)| json!({"lambda": l, "criterion": o(*s)})).collect::<Vec<_>>(),
    })
}

pub fn status(s: &mut Session, _p: &Value) -> Result<Value> {
    let m = load_model(s);
    let ids = all_photos(s);
    let rated = ids.iter().filter(|i| s.catalog.photo(**i).is_some_and(|p| p.rating > 0)).count();
    Ok(json!({
        "available": true,
        "trained": m.is_some(),
        "model": m.as_ref().map(|m| json!({"trained": m.trained, "embedding": m.embedding, "counts": m.counts, "metrics": metrics_json(m)})),
        "ratedPhotos": rated,
        "reviewedFolders": reviewed_folders(s).len(),
    }))
}

// ---------- culling.suggest

struct Scored {
    id: PhotoId,
    score: f32,
}

/// Predicted stars of `ids` that have an embedding: `(scored, without_embedding)`.
fn score(s: &mut Session, model: &TasteModel, ids: &[PhotoId]) -> Result<(Vec<Scored>, usize)> {
    let dir = Searcher::store_dir(s.library.as_ref());
    let keyed: Vec<(PhotoId, u64)> = ids
        .iter()
        .filter(|i| s.catalog.photo(**i).is_some_and(|p| p.kind != MediaKind::Video))
        .filter_map(|i| s.catalog.photo(*i).map(|p| (*i, lightcraft_search::key_of(&crate::media::content_key(p)))))
        .collect();
    let (have, current): (Vec<(PhotoId, u64, Vec<f32>)>, String) = s
        .searcher
        .with_store(dir.clone(), |st| {
            (keyed.iter().filter_map(|(i, k)| st.vectors.get(*k).map(|v| (*i, *k, v.to_vec()))).collect(), st.model.clone())
        })
        .map_err(other)?;
    if current != model.embedding {
        return Err(other(format!("the taste model was trained on another search model ({}); run `taste.train` again", model.embedding)));
    }
    let kd: Vec<(PhotoId, u64)> = have.iter().map(|(i, k, _)| (*i, *k)).collect();
    let ex = if model.extras.is_empty() { HashMap::new() } else { extras_for(s, &kd, &dir) };
    let mut out = Vec::new();
    for (id, k, v) in have {
        let e: Vec<f32> = if model.extras.is_empty() {
            Vec::new()
        } else {
            match ex.get(&k) {
                Some(e) => e.to_vec(),
                None => continue,
            }
        };
        if let Some(sc) = model.predict(&v, &e) {
            out.push(Scored { id, score: sc });
        }
    }
    let missing = keyed.len() - out.len();
    Ok((out, missing))
}

pub fn suggest(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "culling.suggest";
    let model = load_model(s).ok_or_else(|| bad(C, "no taste model yet: `taste.train` learns it from the photos you rated"))?;
    let ids: Vec<PhotoId> = if p.get("ids").is_some() { s.targets(p)? } else { s.visible_cloned() };
    let unrated_only = bool_or(p, "unratedOnly", true);
    let fraction = f64_or(p, "fraction", 0.10).clamp(0.001, 1.0);
    let limit = f64_or(p, "limit", 100.0).clamp(1.0, 5000.0) as usize;
    let min_score = p.get("minScore").and_then(Value::as_f64).map(|v| v as f32);
    let (scored, without) = score(s, &model, &ids)?;
    let all_n = scored.len();
    // percentile among everything scored in this call
    let mut sorted: Vec<f32> = scored.iter().map(|x| x.score).collect();
    sorted.sort_by(f32::total_cmp);
    let pct = |v: f32| sorted.partition_point(|x| *x < v) as f64 / sorted.len().max(1) as f64;
    let mut cand: Vec<&Scored> = scored
        .iter()
        .filter(|x| !unrated_only || s.catalog.photo(x.id).is_some_and(|p| p.rating == 0 && p.flag == lightcraft_catalog::Flag::None))
        .collect();
    cand.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.0.cmp(&b.id.0)));
    let n_picks = match p.get("count").and_then(Value::as_u64) {
        Some(c) => c as usize,
        None => (cand.len() as f64 * fraction).ceil() as usize,
    };
    let picks: Vec<&Scored> = cand.iter().copied().filter(|x| min_score.is_none_or(|m| x.score >= m)).take(n_picks).collect();
    if bool_or(p, "filter", false) {
        s.filter.only = picks.iter().map(|x| x.id).collect();
    }
    let row = |x: &Scored| json!({"id": x.id.0, "score": r3(x.score as f64), "percentile": r3(pct(x.score)), "rating": s.catalog.photo(x.id).map_or(0, |p| p.rating)});
    Ok(json!({
        "scored": all_n,
        "withoutEmbedding": without,
        "candidates": cand.len(),
        "unratedOnly": unrated_only,
        "picks": picks.iter().map(|x| x.id.0).collect::<Vec<_>>(),
        "photos": cand.iter().take(limit).map(|x| row(x)).collect::<Vec<_>>(),
        "model": {"trained": model.trained, "rated": model.counts.rated, "spearmanRated": model.metrics.spearman_rated.map(r3), "topKHitRate": model.metrics.top_k_hit_rate.map(r3), "topKChance": model.metrics.top_k_chance.map(r3)},
        "note": "Suggestions only: nothing was written. `culling.accept {ids, mode, confirm: true}` applies them.",
    }))
}

/// `culling.accept`: the only command that writes what the model suggested.
pub fn accept(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "culling.accept";
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err(bad(C, "this writes real stars or flags: pass `confirm: true` once the user has agreed"));
    }
    let mode = p.get("mode").and_then(Value::as_str).unwrap_or("pick");
    let ids = s.targets(p)?;
    if ids.is_empty() {
        return Err(bad(C, "no photos: pass `ids` (e.g. the `picks` of culling.suggest)"));
    }
    match mode {
        "pick" => {
            s.execute("photo.flag", &json!({"flag": "pick", "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))?;
            Ok(json!({"mode": mode, "changed": ids.len()}))
        }
        "rating" => {
            let fixed = p.get("rating").and_then(Value::as_u64);
            let model = load_model(s).ok_or_else(|| bad(C, "no taste model"))?;
            let (scored, _) = score(s, &model, &ids)?;
            let mut by: BTreeMap<u8, Vec<u64>> = BTreeMap::new();
            for x in scored {
                let r = fixed.map_or((x.score.round() as i64).clamp(1, 5) as u8, |r| r.clamp(1, 5) as u8);
                by.entry(r).or_default().push(x.id.0);
            }
            let mut changed = 0;
            for (r, v) in &by {
                s.execute("photo.rate", &json!({"rating": r, "ids": v}))?;
                changed += v.len();
            }
            Ok(json!({"mode": mode, "changed": changed}))
        }
        o => Err(bad(C, format!("`mode` is `pick` or `rating`, not `{o}`"))),
    }
}
