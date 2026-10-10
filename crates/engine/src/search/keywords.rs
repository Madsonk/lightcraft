//! Automatic keywords from a controlled vocabulary (feature `search`).
//!
//! Two sources, both on the SigLIP 2 embeddings the search index already holds:
//! - **zero-shot**: each vocabulary entry (a keyword like `Food|Dessert`, with a few English
//!   prompts) becomes a text embedding; a photo scores high for it when it sits unusually close
//!   to that text compared with the other photos of the library (a z-score of the cosine, which
//!   removes the per-prompt bias SigLIP's raw probabilities have) *and* the model's own
//!   probability is not negligible;
//! - **learned**: for keywords the user already uses (`keywords.learn`), a ridge probe fitted on
//!   the photos that carry it, cross-validated over whole folders, with the score above which it
//!   reached the target precision.
//!
//! Nothing is written without validation: `keywords.suggest` returns suggestions,
//! `keywords.accept` adds the chosen ones, and the `autoAccept` setting (off by default) lets
//! suggestions above its threshold through. The `People|…` and `Places|…` keyword spaces belong to
//! the people / places features and are never suggested here.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use lightcraft_catalog::{MediaKind, PhotoId};
use lightcraft_taste::ridge::{self, Ridge};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Searcher;
use crate::cmd::{bad, bool_or, f64_or, str_param};
use crate::{EngineError, Result, Session};

const BASE: &str = include_str!("vocab_base.json");

/// Keyword roots owned by other features: never suggested, never learned.
const RESERVED_ROOTS: [&str; 2] = ["People", "Places"];

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}
fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ---------- the vocabulary

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub keyword: String,
    #[serde(default)]
    pub fr: Option<String>,
    #[serde(default)]
    pub category: String,
    /// A noun phrase for the alt text ("a dessert").
    #[serde(default)]
    pub phrase: String,
    pub prompts: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct VocabFile {
    version: u32,
    entries: Vec<Entry>,
    /// Keywords of the built-in list the user switched off.
    removed: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub auto_accept: bool,
    pub auto_threshold: f64,
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_accept: false, auto_threshold: 0.9, language: "en".into() }
    }
}

fn store_dir(s: &Session) -> Option<PathBuf> {
    Searcher::store_dir(s.library.as_ref())
}

fn read_json<T: for<'a> Deserialize<'a> + Default>(dir: &Option<PathBuf>, name: &str) -> T {
    dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(name)).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_json<T: Serialize>(dir: &Option<PathBuf>, name: &str, v: &T) -> Result<()> {
    let Some(d) = dir else { return Ok(()) };
    std::fs::create_dir_all(d).map_err(|e| other(format!("{name}: {e}")))?;
    let p = d.join(name);
    let tmp = p.with_extension("part");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).map_err(|e| other(format!("{name}: {e}")))?;
    std::fs::rename(&tmp, &p).map_err(|e| other(format!("{name}: {e}")))
}

pub fn settings_of(s: &Session) -> Settings {
    read_json(&store_dir(s), "keywords-settings.json")
}

/// The built-in entries, then the user's (same keyword = the user's wins), minus the removed ones;
/// reserved roots are dropped.
pub fn entries_of(s: &Session) -> Vec<Entry> {
    let base: VocabFile = serde_json::from_str(BASE).unwrap_or_default();
    let user: VocabFile = read_json(&store_dir(s), "vocabulary.json");
    let mut by: BTreeMap<String, Entry> = BTreeMap::new();
    for e in base.entries.into_iter().chain(user.entries) {
        by.insert(e.keyword.to_lowercase(), e);
    }
    for r in &user.removed {
        by.remove(&r.to_lowercase());
    }
    by.into_values().filter(|e| !reserved(&e.keyword) && !e.prompts.is_empty()).collect()
}

fn reserved(k: &str) -> bool {
    let root = k.split('|').next().unwrap_or("");
    RESERVED_ROOTS.iter().any(|r| r.eq_ignore_ascii_case(root))
}

fn display(e: &Entry, lang: &str) -> String {
    if lang == "fr" { e.fr.clone().unwrap_or_else(|| e.keyword.clone()) } else { e.keyword.clone() }
}

pub fn vocabulary(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = store_dir(s);
    let mut user: VocabFile = read_json(&dir, "vocabulary.json");
    let mut changed = false;
    if let Some(a) = p.get("add").and_then(Value::as_array) {
        for v in a {
            let mut e: Entry =
                serde_json::from_value(v.clone()).map_err(|e| bad("keywords.vocabulary", format!("an entry needs `keyword` and `prompts`: {e}")))?;
            if reserved(&e.keyword) {
                return Err(bad("keywords.vocabulary", format!("`{}` is in a keyword space owned by the people / places features", e.keyword)));
            }
            if e.category.is_empty() {
                e.category = e.keyword.split('|').next().unwrap_or("").to_string();
            }
            user.entries.retain(|x| !x.keyword.eq_ignore_ascii_case(&e.keyword));
            user.removed.retain(|x| !x.eq_ignore_ascii_case(&e.keyword));
            user.entries.push(e);
            changed = true;
        }
    }
    if let Some(a) = p.get("remove").and_then(Value::as_array) {
        for k in a.iter().filter_map(Value::as_str) {
            user.entries.retain(|x| !x.keyword.eq_ignore_ascii_case(k));
            user.removed.push(k.to_string());
            changed = true;
        }
    }
    user.version = 1;
    if changed {
        write_json(&dir, "vocabulary.json", &user)?;
    }
    if let Some(l) = str_param(p, "language") {
        let mut st = settings_of(s);
        st.language = if l == "fr" { "fr".into() } else { "en".into() };
        write_json(&dir, "keywords-settings.json", &st)?;
    }
    let st = settings_of(s);
    let es = entries_of(s);
    let mut cats: BTreeMap<String, usize> = BTreeMap::new();
    es.iter().for_each(|e| *cats.entry(e.category.clone()).or_default() += 1);
    let learned = read_json::<ProbeFile>(&dir, "keyword-probes.json").probes.iter().filter(|p| p.kept).map(|p| p.keyword.clone()).collect::<Vec<_>>();
    Ok(json!({
        "language": st.language, "categories": cats, "total": es.len(),
        "entries": es.iter().map(|e| json!({"keyword": display(e, &st.language), "category": e.category, "prompts": e.prompts})).collect::<Vec<_>>(),
        "learned": learned,
    }))
}

pub fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = store_dir(s);
    let mut st = settings_of(s);
    let mut changed = false;
    if let Some(b) = p.get("autoAccept").and_then(Value::as_bool) {
        st.auto_accept = b;
        changed = true;
    }
    if let Some(t) = p.get("autoThreshold").and_then(Value::as_f64) {
        st.auto_threshold = t.clamp(0.5, 1.0);
        changed = true;
    }
    if let Some(l) = str_param(p, "language") {
        st.language = if l == "fr" { "fr".into() } else { "en".into() };
        changed = true;
    }
    if changed {
        if dir.is_none() {
            return Err(bad("keywords.settings", "these settings are kept in the library folder: the library is not on disk"));
        }
        write_json(&dir, "keywords-settings.json", &st)?;
    }
    Ok(json!({"autoAccept": st.auto_accept, "autoThreshold": st.auto_threshold, "language": st.language}))
}

// ---------- text embeddings

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct TextCache {
    /// SigLIP's learned logit scale and bias: probability = sigmoid(scale · cosine + bias).
    scale: f32,
    bias: f32,
    embeddings: HashMap<String, Vec<f32>>,
}

/// Centroid (mean of the prompt embeddings, renormalised) of each entry and the model's
/// `(scale, bias)`, cached on disk by prompt (so the model is only loaded when a prompt is new).
fn centroids(s: &mut Session, entries: &[Entry]) -> Result<(Vec<Vec<f32>>, (f32, f32))> {
    let dir = store_dir(s);
    let model = s.searcher.with_store(dir.clone(), |st| st.model.clone()).map_err(other)?;
    let cache_name = format!("vocab-text-{model}.json");
    let mut cache: TextCache = read_json(&dir, &cache_name);
    let mut need: Vec<String> = Vec::new();
    for e in entries {
        for pr in &e.prompts {
            if !cache.embeddings.contains_key(pr) && !need.contains(pr) {
                need.push(pr.clone());
            }
        }
    }
    if !need.is_empty() || cache.scale == 0.0 {
        for chunk in need.chunks(32) {
            let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let v = s.searcher.embed_texts(&refs).map_err(other)?;
            for (p, v) in chunk.iter().zip(v) {
                cache.embeddings.insert(p.clone(), v);
            }
        }
        // the model is loaded now: read its calibration off two probabilities (taken where the
        // sigmoid is not saturated)
        let (p0, p1) = (s.searcher.probability(0.05), s.searcher.probability(0.15));
        if let (Some(p0), Some(p1)) = (p0, p1) {
            let logit = |p: f32| (p.clamp(1e-7, 1.0 - 1e-7) / (1.0 - p.clamp(1e-7, 1.0 - 1e-7))).ln();
            cache.scale = (logit(p1) - logit(p0)) / 0.1;
            cache.bias = logit(p0) - cache.scale * 0.05;
        }
        write_json(&dir, &cache_name, &cache)?;
    }
    let cent = entries
        .iter()
        .map(|e| {
            let mut c = vec![0f32; 768];
            for pr in &e.prompts {
                if let Some(v) = cache.embeddings.get(pr) {
                    c.iter_mut().zip(v).for_each(|(a, b)| *a += b);
                }
            }
            let n = c.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            c.iter_mut().for_each(|x| *x /= n);
            c
        })
        .collect();
    Ok((cent, (cache.scale, cache.bias)))
}

// ---------- scoring

/// Where the zero-shot rule sits (calibrated on the archive, see docs/taste-and-keywords.md).
pub const Z_MIN: f32 = 1.0;
/// Minimum SigLIP probability for the text/photo pair.
pub const P_MIN: f32 = 0.001;
/// What a zero-shot match must reach to be added by the `autoAccept` setting: stricter than for
/// a suggestion (precision about 0.8 instead of 0.6 on the author's archive).
pub const AUTO_P_MIN: f32 = 0.006;

#[derive(Clone, Debug)]
struct Cand {
    keyword: String,
    score: f32,
    source: &'static str,
    category: String,
    /// SigLIP's probability for the pair (1 for learned probes).
    prob: f32,
}

/// Per entry, mean and standard deviation of the cosine over every vector of the index.
fn library_stats(s: &mut Session, cent: &[Vec<f32>]) -> Result<Vec<(f32, f32)>> {
    let dir = store_dir(s);
    s.searcher
        .with_store(dir, |st| {
            let vs: Vec<&[f32]> = st.vectors.iter().map(|(_, v)| v).collect();
            let n = vs.len().max(1) as f64;
            cent.par_iter()
                .map(|c| {
                    let cs: Vec<f64> = vs.iter().map(|v| lightcraft_search::index::dot(v, c) as f64).collect();
                    let m = cs.iter().sum::<f64>() / n;
                    let sd = (cs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n).sqrt();
                    (m as f32, (sd as f32).max(1e-4))
                })
                .collect()
        })
        .map_err(other)
}

/// Map a z-score to 0..1 so that the rule's threshold is 0.5.
fn zs_score(z: f32) -> f32 {
    1.0 / (1.0 + (-(z - Z_MIN) * 1.5).exp())
}

// ---------- learned probes

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ProbeFile {
    version: u32,
    embedding: String,
    probes: Vec<Probe>,
}

#[derive(Serialize, Deserialize)]
struct Probe {
    keyword: String,
    photos: usize,
    precision: Option<f64>,
    recall: Option<f64>,
    threshold: f32,
    kept: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ridge: Option<Ridge>,
}

/// The score the learned probe maps to 0..1 (0.5 at its threshold).
fn probe_score(pr: &Probe, v: &[f32]) -> Option<f32> {
    let r = pr.ridge.as_ref()?;
    let x = r.predict(v)?;
    Some(1.0 / (1.0 + (-(x - pr.threshold) * 8.0).exp()))
}

fn indexed_keywords(s: &Session, keys: &HashMap<u64, Vec<f32>>) -> BTreeMap<String, Vec<(PhotoId, u64)>> {
    let mut m: BTreeMap<String, Vec<(PhotoId, u64)>> = BTreeMap::new();
    for p in s.catalog.photos().filter(|p| p.in_library() && p.kind != MediaKind::Video) {
        let k = lightcraft_search::key_of(&crate::media::content_key(p));
        if keys.contains_key(&k) {
            for kw in &p.meta.keywords {
                m.entry(kw.clone()).or_default().push((p.id, k));
            }
        }
    }
    m
}

fn all_vectors(s: &mut Session) -> Result<(HashMap<u64, Vec<f32>>, String)> {
    let dir = store_dir(s);
    s.searcher.with_store(dir, |st| (st.vectors.iter().map(|(k, v)| (k, v.to_vec())).collect::<HashMap<_, _>>(), st.model.clone())).map_err(other)
}

pub fn learn(s: &mut Session, p: &Value) -> Result<Value> {
    let min_photos = f64_or(p, "minPhotos", 20.0).max(5.0) as usize;
    let target = f64_or(p, "precision", 0.8).clamp(0.5, 0.99);
    let folds = f64_or(p, "folds", 5.0).clamp(2.0, 10.0) as usize;
    let (vecs, model) = all_vectors(s)?;
    let by_kw = indexed_keywords(s, &vecs);
    let wanted: Option<Vec<String>> =
        p.get("keywords").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    // folder of every indexed photo (folds hold out whole folders)
    let mut group: HashMap<u64, u64> = HashMap::new();
    let mut indexed_photos = 0usize;
    for ph in s.catalog.photos().filter(|p| p.in_library() && p.kind != MediaKind::Video) {
        let k = lightcraft_search::key_of(&crate::media::content_key(ph));
        if vecs.contains_key(&k) {
            indexed_photos += 1;
            let g = match &ph.source {
                lightcraft_catalog::Source::File { path } => lightcraft_search::key_of(path.rsplit_once('/').map_or("", |x| x.0)),
                _ => 0,
            };
            group.insert(k, g);
        }
    }
    let library_photos = s.catalog.photos().filter(|p| p.in_library() && p.kind != MediaKind::Video).count().max(1);
    let keys: Vec<u64> = vecs.keys().copied().collect();
    let mut out = Vec::new();
    let mut probes = Vec::new();
    for (kw, members) in &by_kw {
        if wanted.as_ref().is_some_and(|w| !w.iter().any(|x| x.eq_ignore_ascii_case(kw))) {
            continue;
        }
        let mut pr = Probe {
            keyword: kw.clone(),
            photos: members.len(),
            precision: None,
            recall: None,
            threshold: 0.0,
            kept: false,
            reason: None,
            ridge: None,
        };
        let share_lib =
            s.catalog.photos().filter(|p| p.in_library() && p.meta.keywords.iter().any(|k| k == kw)).count() as f64 / library_photos as f64;
        if reserved(kw) {
            pr.reason = Some("People / Places keyword space".into());
        } else if members.len() < min_photos {
            pr.reason = Some(format!("fewer than {min_photos} indexed photos"));
        } else if share_lib > 0.4 {
            pr.reason = Some("on more than 40 % of the library: a probe would learn nothing".into());
        }
        if pr.reason.is_none() {
            let pos: HashSet<u64> = members.iter().map(|m| m.1).collect();
            let mut x: Vec<&[f32]> = Vec::new();
            let mut y: Vec<f32> = Vec::new();
            let mut g: Vec<u64> = Vec::new();
            for k in &keys {
                if let Some(v) = vecs.get(k) {
                    x.push(v);
                    y.push(pos.contains(k) as u8 as f32);
                    g.push(group.get(k).copied().unwrap_or(0));
                }
            }
            let n_pos = y.iter().filter(|v| **v > 0.5).count().max(1) as f32;
            let n_neg = (y.len() as f32 - n_pos).max(1.0);
            let w: Vec<f32> = y.iter().map(|v| if *v > 0.5 { (n_neg / n_pos).min(5.0) } else { 1.0 }).collect();
            let fold = lightcraft_taste::stats::group_folds(&g, folds, 7);
            let lam = 30.0;
            let mut oof = vec![f32::NAN; y.len()];
            for f in 0..folds {
                let tr: Vec<usize> = (0..y.len()).filter(|i| fold[*i] != f).collect();
                let xt: Vec<&[f32]> = tr.iter().map(|i| x[*i]).collect();
                let yt: Vec<f32> = tr.iter().map(|i| y[*i]).collect();
                let wt: Vec<f32> = tr.iter().map(|i| w[*i]).collect();
                if let Ok(m) = ridge::fit(&xt, &yt, &wt, lam) {
                    for i in (0..y.len()).filter(|i| fold[*i] == f) {
                        oof[i] = m.predict(x[i]).unwrap_or(f32::NAN);
                    }
                }
            }
            // the lowest threshold whose precision on held-out folders reaches the target
            let mut order: Vec<usize> = (0..y.len()).filter(|i| oof[*i].is_finite()).collect();
            order.sort_by(|a, b| oof[*b].total_cmp(&oof[*a]));
            let (mut tp, mut best) = (0usize, None);
            let total_pos = order.iter().filter(|i| y[**i] > 0.5).count().max(1);
            for (rank, i) in order.iter().enumerate() {
                tp += (y[*i] > 0.5) as usize;
                let prec = tp as f64 / (rank + 1) as f64;
                if prec >= target && rank + 1 >= 5 {
                    best = Some((oof[*i], prec, tp as f64 / total_pos as f64));
                }
            }
            if let Ok(m) = ridge::fit(&x, &y, &w, lam) {
                match best {
                    Some((thr, prec, rec)) => {
                        pr.threshold = thr;
                        pr.precision = Some(prec);
                        pr.recall = Some(rec);
                        pr.kept = true;
                    }
                    None => pr.reason = Some(format!("never reached {:.0} % precision on held-out folders", target * 100.0)),
                }
                pr.ridge = Some(m);
            }
        }
        out.push(json!({"keyword": kw, "photos": pr.photos, "precision": pr.precision.map(r3), "recall": pr.recall.map(r3), "threshold": r3(pr.threshold as f64), "kept": pr.kept, "reason": pr.reason}));
        probes.push(pr);
    }
    let dir = store_dir(s);
    let prev: ProbeFile = read_json(&dir, "keyword-probes.json");
    // keep earlier probes of keywords not re-learned now
    let mut all: Vec<Probe> = prev.probes.into_iter().filter(|o| !probes.iter().any(|n| n.keyword == o.keyword) && prev.embedding == model).collect();
    all.extend(probes);
    write_json(&dir, "keyword-probes.json", &ProbeFile { version: 1, embedding: model, probes: all })?;
    Ok(json!({"keywords": out, "indexedPhotos": indexed_photos, "precisionTarget": target, "folds": folds,
        "note": "Precision and recall are measured on the indexed photos only, on folders held out of the fit; if the indexed photos are a sample, the precision on the whole library is lower."}))
}

// ---------- suggestions

fn candidates(
    entries: &[Entry],
    cent: &[Vec<f32>],
    stats: &[(f32, f32)],
    probes: &[Probe],
    scale_prob: &dyn Fn(f32) -> Option<f32>,
    v: &[f32],
    lang: &str,
    p_min: f32,
) -> Vec<Cand> {
    let mut out = Vec::new();
    for ((e, c), (m, sd)) in entries.iter().zip(cent).zip(stats) {
        let cos = lightcraft_search::index::dot(v, c);
        let z = (cos - m) / sd;
        let pr = scale_prob(cos).unwrap_or(1.0);
        if pr >= p_min {
            let score = zs_score(z);
            if score > 0.01 {
                out.push(Cand { keyword: display(e, lang), score, source: "zero-shot", category: e.category.clone(), prob: pr });
            }
        }
    }
    for pr in probes.iter().filter(|p| p.kept) {
        if let Some(sc) = probe_score(pr, v) {
            out.push(Cand { keyword: pr.keyword.clone(), score: sc, source: "learned", category: "Archive".into(), prob: 1.0 });
        }
    }
    out
}

struct Prepared {
    entries: Vec<Entry>,
    cent: Vec<Vec<f32>>,
    stats: Vec<(f32, f32)>,
    probes: Vec<Probe>,
    lang: String,
    p_min: f32,
    scale: f32,
    bias: f32,
}

fn prepare(s: &mut Session, p: &Value) -> Result<Prepared> {
    s.searcher.model_dir().map_err(other)?;
    let entries = entries_of(s);
    let (cent, (scale, bias)) = centroids(s, &entries)?;
    let stats = library_stats(s, &cent)?;
    let dir = store_dir(s);
    let pf: ProbeFile = read_json(&dir, "keyword-probes.json");
    let model = s.searcher.with_store(dir, |st| st.model.clone()).map_err(other)?;
    let probes = if pf.embedding == model { pf.probes } else { Vec::new() };
    Ok(Prepared {
        entries,
        cent,
        stats,
        probes,
        lang: settings_of(s).language,
        p_min: f64_or(p, "minProbability", P_MIN as f64).clamp(0.0, 1.0) as f32,
        scale,
        bias,
    })
}

/// Suggestions per photo: `(id, existing keywords, candidates sorted by score)`.
fn suggest_for(
    s: &mut Session,
    prep: &Prepared,
    ids: &[PhotoId],
    include_existing: bool,
    threshold: f32,
    cats: &Option<Vec<String>>,
) -> Result<(Vec<(PhotoId, Vec<Cand>)>, usize)> {
    let dir = store_dir(s);
    let keyed: Vec<(PhotoId, u64, Vec<String>)> = ids
        .iter()
        .filter_map(|i| {
            s.catalog
                .photo(*i)
                .filter(|p| p.kind != MediaKind::Video)
                .map(|p| (*i, lightcraft_search::key_of(&crate::media::content_key(p)), p.meta.keywords.clone()))
        })
        .collect();
    let vs: Vec<(PhotoId, Vec<f32>, Vec<String>)> = s
        .searcher
        .with_store(dir, |st| keyed.iter().filter_map(|(i, k, kw)| st.vectors.get(*k).map(|v| (*i, v.to_vec(), kw.clone()))).collect())
        .map_err(other)?;
    let missing = keyed.len() - vs.len();
    let (sc, bi) = (prep.scale, prep.bias);
    let probe = move |c: f32| Some(1.0 / (1.0 + (-(c * sc + bi)).exp()));
    let mut out = Vec::new();
    for (id, v, have) in vs {
        let mut c = candidates(&prep.entries, &prep.cent, &prep.stats, &prep.probes, &probe, &v, &prep.lang, prep.p_min);
        c.retain(|x| {
            x.score >= threshold
                && cats.as_ref().is_none_or(|cs| cs.iter().any(|k| k.eq_ignore_ascii_case(&x.category)))
                && (include_existing || !have.iter().any(|h| h.eq_ignore_ascii_case(&x.keyword)))
        });
        c.sort_by(|a, b| b.score.total_cmp(&a.score));
        // one learned / zero-shot entry per keyword
        let mut seen = HashSet::new();
        c.retain(|x| seen.insert(x.keyword.clone()));
        out.push((id, c));
    }
    Ok((out, missing))
}

pub fn suggest(s: &mut Session, p: &Value) -> Result<Value> {
    let prep = prepare(s, p)?;
    let st = settings_of(s);
    let threshold = f64_or(p, "threshold", 0.5).clamp(0.0, 1.0) as f32;
    let limit = f64_or(p, "limit", 8.0).clamp(1.0, 50.0) as usize;
    let cats: Option<Vec<String>> =
        p.get("categories").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let ids = s.targets(p);
    let (res, missing) = suggest_for(s, &prep, &ids, bool_or(p, "includeExisting", false), threshold, &cats)?;
    let mut applied = 0usize;
    if st.auto_accept {
        let mut by_kw: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for (id, c) in &res {
            for x in c.iter().filter(|x| x.score as f64 >= st.auto_threshold && x.prob >= AUTO_P_MIN).take(limit) {
                by_kw.entry(x.keyword.clone()).or_default().push(id.0);
            }
        }
        for (kw, v) in by_kw {
            s.execute("photo.setMeta", &json!({"ids": v, "addKeywords": [kw]}))?;
            applied += v.len();
        }
    }
    Ok(json!({
        "photos": res.iter().map(|(id, c)| json!({"id": id.0, "suggestions": c.iter().take(limit).map(|x| json!({"keyword": x.keyword, "score": r3(x.score as f64), "source": x.source, "category": x.category})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "applied": applied,
        "autoAccept": st.auto_accept,
        "indexed": res.len(),
        "unindexed": missing,
        "note": "Suggestions only: `keywords.accept` adds the chosen ones.",
    }))
}

pub fn accept(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "keywords.accept";
    let mut by_kw: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    if let Some(items) = p.get("items").and_then(Value::as_array) {
        for it in items {
            let (Some(id), Some(k)) = (it.get("id").and_then(Value::as_u64), it.get("keyword").and_then(Value::as_str)) else {
                return Err(bad(C, "each item is {id, keyword}"));
            };
            by_kw.entry(k.to_string()).or_default().push(id);
        }
    } else if let Some(ks) = p.get("keywords").and_then(Value::as_array) {
        let ids: Vec<u64> = s.targets(p).iter().map(|i| i.0).collect();
        for k in ks.iter().filter_map(Value::as_str) {
            by_kw.insert(k.to_string(), ids.clone());
        }
    } else {
        return Err(bad(C, "pass `items: [{id, keyword}]` or `ids` + `keywords`"));
    }
    if by_kw.keys().any(|k| reserved(k)) {
        return Err(bad(C, "People / Places keywords are written by the people / places features"));
    }
    let mut changed = 0;
    let mut added = Vec::new();
    for (k, ids) in by_kw {
        let r = s.execute("photo.setMeta", &json!({"ids": ids, "addKeywords": [k]}))?;
        changed += r.get("changed").and_then(Value::as_u64).unwrap_or(ids.len() as u64);
        added.push(k);
    }
    Ok(json!({"changed": changed, "added": added}))
}

// ---------- alt text

fn alt_from(parts: &[(String, String)]) -> String {
    let pick = |cat: &str, n: usize| -> Vec<&str> { parts.iter().filter(|(c, _)| c == cat).map(|(_, p)| p.as_str()).take(n).collect() };
    let join = |v: Vec<&str>| match v.len() {
        0 => String::new(),
        1 => v[0].to_string(),
        _ => format!("{} and {}", v[..v.len() - 1].join(", "), v[v.len() - 1]),
    };
    let mut what: Vec<&str> = pick("Subject", 2);
    what.extend(pick("Food", 2));
    what.extend(pick("Object", 2));
    what.truncate(3);
    let scene = pick("Scene", 1);
    let mood = pick("Mood", 1);
    if what.is_empty() && scene.is_empty() {
        return String::new();
    }
    let mut t = String::from("Photo of ");
    t.push_str(&if what.is_empty() { scene.first().map_or(String::new(), |x| x.to_string()) } else { join(what) });
    if let (false, Some(sc)) = (parts.iter().all(|(c, _)| c != "Scene"), scene.first())
        && !t.ends_with(sc)
    {
        t.push_str(&format!(", {sc}"));
    }
    if let Some(m) = mood.first() {
        t.push_str(&format!(", {m}"));
    }
    t.push('.');
    if t.chars().count() > 125 { t.chars().take(124).collect::<String>() + "." } else { t }
}

pub fn alt_suggest(s: &mut Session, p: &Value) -> Result<Value> {
    let prep = prepare(s, p)?;
    let apply = bool_or(p, "apply", false);
    let ids = s.targets(p);
    let (res, missing) = suggest_for(s, &prep, &ids, true, 0.5, &None)?;
    let phrase: HashMap<String, (String, String)> =
        prep.entries.iter().map(|e| (display(e, &prep.lang), (e.category.clone(), e.phrase.clone()))).collect();
    let mut out = Vec::new();
    let mut applied = 0;
    for (id, c) in &res {
        let parts: Vec<(String, String)> =
            c.iter().filter(|x| x.source == "zero-shot").filter_map(|x| phrase.get(&x.keyword).cloned()).filter(|(_, p)| !p.is_empty()).collect();
        let alt = alt_from(&parts);
        let has = s.catalog.photo(*id).is_some_and(|p| !p.meta.alt_text.trim().is_empty());
        if apply && !alt.is_empty() && !has {
            s.execute("photo.setMeta", &json!({"ids": [id.0], "altText": alt}))?;
            applied += 1;
        }
        out.push(json!({"id": id.0, "altText": alt, "existing": has}));
    }
    Ok(json!({"photos": out, "applied": applied, "unindexed": missing, "method": "template over the photo's suggested keywords (no caption model)"}))
}

// ---------- evaluation

pub fn evaluate(s: &mut Session, p: &Value) -> Result<Value> {
    let prep = prepare(s, p)?;
    let threshold = f64_or(p, "threshold", 0.5) as f32;
    let (vecs, _) = all_vectors(s)?;
    let by_kw = indexed_keywords(s, &vecs);
    let wanted: Vec<String> = p
        .get("keywords")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_else(|| by_kw.keys().cloned().collect());
    let ids: Vec<PhotoId> = s.catalog.photos().filter(|q| q.in_library() && q.kind != MediaKind::Video).map(|q| q.id).collect();
    let (res, _) = suggest_for(s, &prep, &ids, true, threshold, &None)?;
    let mut out = Vec::new();
    for kw in wanted {
        let truth: HashSet<PhotoId> = by_kw.get(&kw).map(|v| v.iter().map(|x| x.0).collect()).unwrap_or_default();
        let known = prep.entries.iter().any(|e| display(e, &prep.lang).eq_ignore_ascii_case(&kw))
            || prep.probes.iter().any(|x| x.kept && x.keyword.eq_ignore_ascii_case(&kw));
        if !known {
            continue;
        }
        let (mut tp, mut fp) = (0usize, 0usize);
        let mut predicted: HashSet<PhotoId> = HashSet::new();
        for (id, c) in &res {
            if c.iter().any(|x| x.keyword.eq_ignore_ascii_case(&kw)) {
                predicted.insert(*id);
            }
        }
        for id in &predicted {
            if truth.contains(id) { tp += 1 } else { fp += 1 }
        }
        let fn_ = truth.iter().filter(|t| res.iter().any(|(i, _)| i == *t) && !predicted.contains(t)).count();
        let prec = (tp + fp > 0).then(|| tp as f64 / (tp + fp) as f64);
        let rec = (tp + fn_ > 0).then(|| tp as f64 / (tp + fn_) as f64);
        out.push(json!({"keyword": kw, "tp": tp, "fp": fp, "fn": fn_, "precision": prec.map(r3), "recall": rec.map(r3)}));
    }
    Ok(json!({"keywords": out, "threshold": threshold}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(c: &str, p: &str) -> (String, String) {
        (c.to_string(), p.to_string())
    }

    #[test]
    fn alt_text_is_a_template_over_the_tags_and_says_nothing_without_them() {
        assert_eq!(alt_from(&[]), "");
        assert_eq!(alt_from(&[part("Mood", "a calm scene")]), "");
        let t = alt_from(&[part("Subject", "a child"), part("Food", "a cake"), part("Scene", "a garden"), part("Mood", "a festive scene")]);
        assert_eq!(t, "Photo of a child and a cake, a garden, a festive scene.");
        assert_eq!(alt_from(&[part("Scene", "a beach")]), "Photo of a beach.");
        let long: Vec<(String, String)> = (0..3).map(|i| part("Subject", &format!("{} {i}", "very long description ".repeat(10)))).collect();
        assert!(alt_from(&long).chars().count() <= 125);
    }

    #[test]
    fn reserved_keyword_spaces_are_refused() {
        assert!(reserved("People|Allison"));
        assert!(reserved("places|Poitiers"));
        assert!(!reserved("Food|Dessert"));
        assert!(!reserved("Peoples"));
    }

    #[test]
    fn the_built_in_vocabulary_is_well_formed() {
        let v: VocabFile = serde_json::from_str(BASE).unwrap();
        assert!(v.entries.len() >= 100);
        let mut seen = HashSet::new();
        for e in &v.entries {
            assert!(seen.insert(e.keyword.to_lowercase()), "duplicate {}", e.keyword);
            assert!(!reserved(&e.keyword), "{}", e.keyword);
            assert!(!e.prompts.is_empty() && !e.phrase.is_empty() && e.fr.is_some(), "{}", e.keyword);
            assert!(e.keyword.starts_with(&format!("{}|", e.category)), "{}", e.keyword);
        }
    }
}
