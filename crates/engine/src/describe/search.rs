//! `describe.search`: "who + what + where + when" over photos and moments of videos.
//!
//! A query is split into the **people** it names (people named in People ▸ Recognition; a full
//! name, or a first name only one person has) and the **words** left. A photo or a moment of a
//! video is a result when
//! - **every** person asked for is there at that moment — one of their confirmed faces is in the
//!   photo, or in a picture of the video within `window` seconds of the moment; and
//! - the words match **what** it shows: the annotation's scene, action and description (accents and
//!   case ignored, word beginnings: « conduit » finds « conduisant »), or — when the semantic search
//!   model is installed and the pictures are indexed — the meaning of the picture (SigLIP).
//!
//! Nothing is widened in silence (Video Catcher's rule): moments where the people are there but the
//! words are not found come back apart, as `personPresentWhatNotDetected`, and moments that match the
//! words without the people as `whatFoundPeopleNotRecognized`. "Not detected" is never a proof of
//! absence: a face too small, turned away, or not confirmed yet is not counted.

use std::collections::{HashMap, HashSet};

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::{Value, json};

#[cfg(feature = "search")]
use super::honest;
use super::honest::Centre;
use super::{Effective, fold};
use crate::cmd::{bad, bool_or, f64_or, str_param};
use crate::video::speech::{self, Level, Speech, same_word};
use crate::{Result, Session};

/// Words that say nothing about a picture (French and English).
const STOP: &[&str] = &[
    "le", "la", "les", "l", "un", "une", "des", "de", "du", "d", "et", "ou", "a", "au", "aux", "en", "dans", "sur", "sous", "avec", "par", "pour",
    "qui", "que", "quoi", "est", "sont", "se", "s", "sa", "son", "ses", "il", "elle", "ils", "elles", "on", "y", "ce", "cet", "cette", "the", "an",
    "of", "in", "on", "with", "and", "or", "is", "are", "at", "to", "his", "her", "their", "photo", "video", "moment",
];

/// Default seconds around a moment of a video in which a person's face counts as "there".
pub const DEFAULT_WINDOW: f64 = 6.0;
/// Default probability for a match by meaning.
pub const DEFAULT_MIN_PROBABILITY: f64 = 0.02;

/// The query's words matched in `text` (both folded).
fn matched<'a>(terms: &'a [String], text: &str) -> Vec<&'a str> {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    terms.iter().filter(|t| words.iter().any(|w| same_word(t, w))).map(String::as_str).collect()
}

/// A photo, or a moment of a video.
struct Moment {
    id: PhotoId,
    item: String,
    ms: Option<u64>,
    /// Face-index key of the picture.
    fkey: u64,
    /// Search-index key of the picture.
    skey: u64,
    frame: Option<String>,
}

/// Split `query` into the people it names and the words left.
fn people_in(query: &str, people: &[(u32, String)]) -> (Vec<(u32, String)>, String) {
    let mut q = format!(" {} ", fold(query));
    let mut found: Vec<(u32, String)> = Vec::new();
    let mut by_len: Vec<&(u32, String)> = people.iter().collect();
    by_len.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    for (id, name) in &by_len {
        let full = format!(" {} ", fold(name));
        if full.trim().is_empty() {
            continue;
        }
        if q.contains(&full) {
            q = q.replace(&full, " ");
            found.push((*id, name.clone()));
        }
    }
    // a first name alone, when one named person has it
    for (id, name) in &by_len {
        let Some(first) = fold(name).split(' ').next().map(str::to_string).filter(|f| f.chars().count() >= 3) else { continue };
        let same = people.iter().filter(|(_, n)| fold(n).split(' ').next() == Some(first.as_str())).count();
        let w = format!(" {first} ");
        if same == 1 && q.contains(&w) && !found.iter().any(|(f, _)| f == id) {
            q = q.replace(&w, " ");
            found.push((*id, name.clone()));
        }
    }
    (found, q.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The people the photos name (numbered by first appearance) and, by picture key, who the faces
/// of the videos are suggested to be.
fn named(s: &mut Session) -> (Vec<(u32, String)>, HashMap<u64, Vec<(u32, String)>>) {
    let reader = crate::video::faces::reader(s);
    let keys: Vec<u64> = reader.keys();
    let mut people: Vec<(u32, String)> = Vec::new();
    let mut by_key: HashMap<u64, Vec<(u32, String)>> = HashMap::new();
    for k in keys {
        for f in reader.faces_at(k) {
            let Some(name) = f["name"].as_str() else { continue };
            let id = match people.iter().find(|(_, n)| n == name) {
                Some((i, _)) => *i,
                None => {
                    let i = people.len() as u32;
                    people.push((i, name.to_string()));
                    i
                }
            };
            let row = by_key.entry(k).or_default();
            if !row.iter().any(|(i, _)| *i == id) {
                row.push((id, name.to_string()));
            }
        }
    }
    (people, by_key)
}

/// What the meaning of the query says about each picture.
#[cfg_attr(not(feature = "search"), allow(dead_code))]
struct Meaning {
    /// What the pictures are ranked by: the cosine with the (moved) query, minus the picture's noise
    /// ceiling when `normalise` is asked.
    cos: HashMap<u64, f32>,
    /// Cosine with the query as the model gives it (the probability is read from this one).
    raw: HashMap<u64, f32>,
    probability: Box<dyn Fn(f32) -> f32>,
    /// A match must beat the picture's noise ceiling by this margin, when a floor is asked.
    floor: Option<f32>,
    /// The pictures that do, when a floor is asked.
    above: HashSet<u64>,
}

impl Meaning {
    fn passes(&self, key: u64) -> bool {
        self.floor.is_none() || self.above.contains(&key)
    }
}

/// The options of the meaning side of a search.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "search"), allow(dead_code))]
struct Honest {
    centre: Centre,
    /// Margin over the noise ceiling (cosine units), if a floor is asked.
    floor: Option<f32>,
    /// Rank by the margin over the noise ceiling instead of the plain cosine.
    normalise: bool,
}

/// The cosine of each search key with the meaning of `texts` (their embeddings averaged), moved as
/// `h.centre` says, and the noise floor when one is asked; `Err` says why there is none.
#[cfg(feature = "search")]
fn meaning(s: &mut Session, texts: &[String], keys: &[u64], h: Honest) -> std::result::Result<Meaning, String> {
    s.searcher.model_dir()?;
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let all = s.searcher.embed_texts(&refs)?;
    let first = all.first().ok_or("no embedding")?;
    let mut q = vec![0.0f32; first.len()];
    for v in &all {
        for (a, b) in q.iter_mut().zip(v) {
            *a += b;
        }
    }
    honest::unit(&mut q);
    let dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("search"));
    let vectors = crate::search::vectors_of(&s.searcher, dir, keys)?;
    let calib = if h.centre != Centre::None || h.floor.is_some() || h.normalise { Some(s.searcher.calibration()?) } else { None };
    let (text_mean, image_mean) = match &calib {
        Some(c) => {
            let views: Vec<&[f32]> = vectors.values().map(Vec::as_slice).collect();
            (c.text_mean.clone(), honest::mean(&views))
        }
        None => (Vec::new(), Vec::new()),
    };
    let moved = honest::moved(&q, h.centre, &text_mean, &image_mean);
    let plain: HashMap<u64, f32> = vectors.iter().map(|(k, v)| (*k, honest::dot(v, &moved))).collect();
    let raw: HashMap<u64, f32> =
        if h.centre == Centre::None { plain.clone() } else { vectors.iter().map(|(k, v)| (*k, honest::dot(v, &q))).collect() };
    let mut cos = plain.clone();
    let mut above = HashSet::new();
    if let Some(c) = calib.filter(|_| h.floor.is_some() || h.normalise) {
        let noise: Vec<Vec<f32>> = c.noise.iter().map(|n| honest::moved(n, h.centre, &text_mean, &image_mean)).collect();
        for (k, v) in &vectors {
            let margin = plain[k] - honest::ceiling(&noise, v);
            if h.floor.is_some_and(|m| margin > m) {
                above.insert(*k);
            }
            if h.normalise {
                cos.insert(*k, margin);
            }
        }
    }
    let shared = s.searcher.share();
    Ok(Meaning { cos, raw, probability: Box::new(move |c| shared.probability(c).unwrap_or(0.0)), floor: h.floor, above })
}
#[cfg(not(feature = "search"))]
fn meaning(_: &mut Session, _: &[String], _: &[u64], _: Honest) -> std::result::Result<Meaning, String> {
    Err(crate::search::UNAVAILABLE.into())
}

/// Where a moment of a video is near what is said, by level: `(level, ms of the sentence, text)`.
type SpeechHit = Option<(Level, u64, String)>;

/// For each moment, the best place where every word of the query is said near it (level 1: one
/// sentence, 2: within a few seconds, 3: somewhere in the video, given to its closest moment).
fn speech_per_moment(s: &mut Session, moments: &[Moment], terms: &[String], on: bool, window_ms: u64) -> Vec<SpeechHit> {
    let mut out: Vec<SpeechHit> = vec![None; moments.len()];
    let Some(root) = crate::video::cache_root(s).filter(|_| on && !terms.is_empty()) else { return out };
    let mut by_item: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, m) in moments.iter().enumerate().filter(|(_, m)| m.ms.is_some()) {
        by_item.entry(m.item.as_str()).or_default().push(i);
    }
    for (item, idxs) in by_item {
        let Some(sp) = Speech::load(&crate::video::video_dir(&root, item)) else { continue };
        let hits = speech::find(&sp.utterances, terms, speech::WINDOW_MS);
        let Some(first) = hits.first() else { continue };
        if first.level == Level::Video {
            // somewhere in the video: its closest moment carries it
            if let Some(&i) = idxs.iter().min_by_key(|i| speech::distance_ms(first, moments[**i].ms.unwrap_or(0))) {
                out[i] = Some((Level::Video, first.t0, first.text.clone()));
            }
            continue;
        }
        for i in idxs {
            let ms = moments[i].ms.unwrap_or(0);
            out[i] = hits
                .iter()
                .filter(|h| h.level != Level::Video)
                .map(|h| (speech::distance_ms(h, ms), h))
                .filter(|(d, _)| *d <= window_ms)
                .min_by_key(|(d, h)| (h.level, *d))
                .map(|(_, h)| (h.level, h.t0, h.text.clone()));
        }
    }
    out
}

/// How a French query is put to the semantic model: in French, in English through the
/// [`super::glossary`], or both (their meanings averaged).
pub const DEFAULT_LANG: &str = "both";
/// A picture counts as matching by meaning when it is among this many closest to the query.
pub const DEFAULT_SEMANTIC_TOP: usize = 20;
/// How the query vector is moved before it is compared (measured with `examples/describe_bench.rs`:
/// centring on the neutral captions is neutral for the fusion, `gap` hurts — see docs/describe.md).
pub const DEFAULT_CENTRE: Centre = Centre::None;
/// A meaning match must beat the picture's noise ceiling by this much (`None` = no floor).
pub const DEFAULT_FLOOR: Option<f32> = Some(0.04);
/// Whether pictures are ranked by their margin over the noise ceiling instead of the plain cosine.
pub const DEFAULT_NORMALISE: bool = false;
/// Reciprocal rank fusion constant (text rank and meaning rank).
const RRF_K: f32 = 60.0;

fn row(m: &Moment, e: Option<&Effective>, present: &[(u32, String)], words: &[&str], prob: Option<f32>, cos: Option<f32>, said: &SpeechHit) -> Value {
    json!({
        "speech": said.as_ref().map(|(l, ms, t)| json!({"level": *l as u8, "ms": ms, "text": t})),
        "id": m.id.0, "kind": if m.ms.is_some() { "video" } else { "photo" }, "ms": m.ms, "frame": m.frame,
        "scene": e.map(|e| e.scene.clone()), "action": e.map(|e| e.action.clone()), "description": e.map(|e| e.description.clone()),
        "status": e.map(|e| e.status), "from": e.map(|e| e.from.clone()),
        "people": present.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>(),
        "matched": {"words": words, "probability": prob.map(|p| (p * 1000.0).round() / 1000.0), "cosine": cos.map(|c| (c * 1000.0).round() / 1000.0)},
    })
}

/// Every photo, and every moment of every video (its pictures and its annotated moments), for `kind`
/// (`all`, `photo` or `video`), with the pictures' times of each video.
fn collect_moments(s: &mut Session, store: &super::Store, kind: &str) -> (Vec<Moment>, HashMap<PhotoId, Vec<(u64, u64)>>) {
    let mut moments: Vec<Moment> = Vec::new();
    let mut video_frames: HashMap<PhotoId, Vec<(u64, u64)>> = HashMap::new();
    let catalog: Vec<(PhotoId, String, bool)> =
        s.catalog.photos().filter(|q| q.in_library()).map(|q| (q.id, crate::media::content_key(q), q.kind == MediaKind::Video)).collect();
    let annotated: HashMap<String, Vec<u64>> = store.moments().into_iter().fold(HashMap::new(), |mut m, (item, ms)| {
        if let Some(ms) = ms {
            m.entry(item).or_default().push(ms);
        }
        m
    });
    // the pictures of every video, from the cached map (not one list file read per video)
    let all_frames = s.video_frames();
    let mut per_video: HashMap<PhotoId, Vec<(u64, crate::video::FrameAt)>> = HashMap::new();
    for (k, f) in all_frames.iter() {
        for id in &f.ids {
            per_video.entry(*id).or_default().push((*k, f.clone()));
        }
    }
    for v in per_video.values_mut() {
        v.sort_by_key(|(_, f)| f.ms);
    }
    for (id, ck, video) in catalog {
        if video && kind != "photo" {
            let fr = per_video.remove(&id).unwrap_or_default();
            let mut seen = HashSet::new();
            for (k, f) in &fr {
                seen.insert(f.ms);
                moments.push(Moment {
                    id,
                    item: ck.clone(),
                    ms: Some(f.ms),
                    fkey: *k,
                    skey: crate::search::search_key(&ck, Some(f.ms)),
                    frame: Some(f.file.display().to_string()),
                });
            }
            for ms in annotated.get(&ck).into_iter().flatten().filter(|ms| !seen.contains(ms)) {
                moments.push(Moment {
                    id,
                    item: ck.clone(),
                    ms: Some(*ms),
                    fkey: crate::video::frame_key(&ck, *ms),
                    skey: crate::search::search_key(&ck, Some(*ms)),
                    frame: None,
                });
            }
            video_frames.insert(id, fr.iter().map(|(k, f)| (f.ms, *k)).collect());
        } else if !video && kind != "video" {
            moments.push(Moment {
                id,
                fkey: crate::video::item_key(&ck),
                skey: crate::search::search_key(&ck, None),
                item: ck,
                ms: None,
                frame: None,
            });
        }
    }
    (moments, video_frames)
}

/// `describe.search {query, people?, limit?, kind?: photo|video|all, window?, semantic?, minProbability?}`.
pub fn search(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "describe.search";
    let query = str_param(p, "query").unwrap_or("").trim().to_string();
    let limit = f64_or(p, "limit", 50.0).clamp(1.0, 2000.0) as usize;
    let window_ms = (f64_or(p, "window", DEFAULT_WINDOW).clamp(0.0, 600.0) * 1000.0) as u64;
    let min_prob = f64_or(p, "minProbability", DEFAULT_MIN_PROBABILITY).clamp(0.0, 1.0) as f32;
    let kind = str_param(p, "kind").unwrap_or("all");
    if !matches!(kind, "all" | "photo" | "video") {
        return Err(bad(C, "`kind` is photo, video or all"));
    }
    let fields: Vec<String> = match p.get("fields").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).filter(|f| matches!(*f, "scene" | "action" | "description")).map(str::to_string).collect(),
        None => vec!["scene".into(), "action".into(), "description".into()],
    };
    let lang = str_param(p, "lang").unwrap_or(DEFAULT_LANG);
    if !matches!(lang, "fr" | "en" | "both") {
        return Err(bad(C, "`lang` is fr, en or both"));
    }
    let semantic_top = f64_or(p, "semanticTop", DEFAULT_SEMANTIC_TOP as f64).clamp(0.0, 1e9) as usize;
    let use_min_prob = p.get("minProbability").is_some();
    let use_text = bool_or(p, "text", true);
    // every word of the query must be found (the default) or any of them (`textAll: false`)
    let text_all = bool_or(p, "textAll", true);
    let (people, by_key) = named(s);
    let (mut required, rest) = people_in(&query, &people);
    let mut unknown_people = Vec::new();
    for n in p.get("people").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        match people.iter().find(|(_, x)| fold(x) == fold(n)) {
            Some(x) if !required.iter().any(|(r, _)| *r == x.0) => required.push(x.clone()),
            Some(_) => {}
            None => unknown_people.push(n.to_string()),
        }
    }
    let terms: Vec<String> = rest.split(' ').filter(|w| w.chars().count() >= 2 && !STOP.contains(w)).map(str::to_string).collect();
    if required.is_empty() && terms.is_empty() {
        return Err(bad(
            C,
            if unknown_people.is_empty() {
                "say who or what to look for (`query`, `people`)".to_string()
            } else {
                format!("nobody named {} in People: name them first (people.name)", unknown_people.join(", "))
            },
        ));
    }
    // every photo, and every moment of every video (its pictures and its annotated moments)
    let store = super::ops::open(s);
    let (moments, video_frames) = collect_moments(s, &store, kind);
    // what, by meaning
    let use_meaning = bool_or(p, "semantic", true) && !terms.is_empty();
    let centre = match str_param(p, "centre") {
        None => DEFAULT_CENTRE,
        Some(c) => Centre::parse(c).ok_or_else(|| bad(C, "`centre` is none, text or gap"))?,
    };
    // `floor`: false = no floor, a number = the margin over the picture's noise ceiling (true = 0),
    // absent = the default
    let floor = match p.get("floor") {
        None => DEFAULT_FLOOR,
        Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(0.0),
        Some(v) => Some(v.as_f64().ok_or_else(|| bad(C, "`floor` is a number or a boolean"))?.clamp(-1.0, 1.0) as f32),
    };
    let honest_opts = Honest { centre, floor, normalise: bool_or(p, "normalise", DEFAULT_NORMALISE) };
    let (found, semantic) = if use_meaning {
        let keys: Vec<u64> = moments.iter().map(|m| m.skey).collect();
        let english = super::glossary::to_english(&terms.join(" "));
        let texts = match lang {
            "fr" => vec![format!("une photo de {rest}")],
            "en" => vec![format!("a photo of {english}")],
            _ => vec![format!("une photo de {rest}"), format!("a photo of {english}")],
        };
        match meaning(s, &texts, &keys, honest_opts) {
            Ok(m) => {
                let n = m.cos.len();
                let centre_name = match centre {
                    Centre::None => "none",
                    Centre::Text => "text",
                    Centre::Gap => "gap",
                };
                let mut v = json!({"used": true, "indexed": n, "lang": lang, "english": english, "centre": centre_name});
                if use_min_prob {
                    v["minProbability"] = json!(min_prob);
                } else {
                    v["top"] = json!(semantic_top);
                }
                v["normalise"] = json!(honest_opts.normalise);
                if let Some(f) = m.floor {
                    v["floor"] = json!((f * 10000.0).round() / 10000.0);
                    v["nothingByMeaning"] = json!(m.above.is_empty());
                }
                (Some(m), v)
            }
            Err(e) => (None, json!({"used": false, "why": e})),
        }
    } else {
        (None, json!({"used": false, "why": if terms.is_empty() { "no words to look for" } else { "semantic: false" }}))
    };
    let empty = HashMap::new();
    let cos = found.as_ref().map_or(&empty, |m| &m.cos);
    // the rank of each picture by meaning (1 = closest) among those above the floor
    let mut by_cos: Vec<(u64, f32)> = cos.iter().filter(|(k, _)| found.as_ref().is_none_or(|m| m.passes(**k))).map(|(k, c)| (*k, *c)).collect();
    by_cos.sort_by(|a, b| b.1.total_cmp(&a.1));
    let sem_rank: HashMap<u64, usize> = by_cos.iter().enumerate().map(|(i, (k, _))| (*k, i + 1)).collect();
    // what is said near each moment (every word of the query, in three levels)
    let said = speech_per_moment(s, &moments, &terms, use_text && bool_or(p, "speech", true), window_ms);
    // the text score of each moment, then its rank among the moments the words reach
    let texts: Vec<(Option<Effective>, Vec<&str>)> = moments
        .iter()
        .enumerate()
        .map(|(mi, m)| {
            let e = store.effective(&m.item, m.ms);
            let text = e
                .as_ref()
                .filter(|_| use_text)
                .map(|e| {
                    let mut t = String::new();
                    for f in &fields {
                        t.push_str(match f.as_str() {
                            "scene" => &e.scene,
                            "action" => &e.action,
                            _ => &e.description,
                        });
                        t.push(' ');
                    }
                    fold(&t)
                })
                .unwrap_or_default();
            let mut words = matched(&terms, &text);
            if said[mi].is_some() {
                words = terms.iter().map(String::as_str).collect();
            }
            (e, words)
        })
        .collect();
    // more words first; for equal words, what is said in one sentence before what is said in a window
    let weight = |i: usize, w: &[&str]| w.len() * 4 + said[i].as_ref().map_or(3, |(l, _, _)| 3 - *l as usize + 1).min(3);
    let mut text_order: Vec<(usize, usize)> =
        texts.iter().enumerate().filter(|(_, (_, w))| !w.is_empty()).map(|(i, (_, w))| (i, weight(i, w))).collect();
    text_order.sort_by_key(|x| std::cmp::Reverse(x.1));
    let text_rank: HashMap<usize, usize> = text_order.iter().enumerate().map(|(r, (i, _))| (*i, r + 1)).collect();
    let mut results: Vec<(f32, Value)> = Vec::new();
    let mut present_only: Vec<(f32, Value)> = Vec::new();
    let mut what_only: Vec<(f32, Value)> = Vec::new();
    for (mi, m) in moments.iter().enumerate() {
        // who is there: the picture's faces, and for a video those within the window
        let mut present: Vec<(u32, String)> = by_key.get(&m.fkey).cloned().unwrap_or_default();
        if let Some(ms) = m.ms {
            for (fms, k) in video_frames.get(&m.id).into_iter().flatten() {
                if fms.abs_diff(ms) <= window_ms {
                    for x in by_key.get(k).into_iter().flatten() {
                        if !present.iter().any(|(i, _)| *i == x.0) {
                            present.push(x.clone());
                        }
                    }
                }
            }
        }
        let people_ok = required.iter().all(|(r, _)| present.iter().any(|(i, _)| i == r));
        let Some((e, words)) = texts.get(mi).map(|(e, w)| (e.clone(), w.clone())) else { continue };
        // every word must be there: « plongée sous-marine » is not « contre-plongée »
        let text_ok = !terms.is_empty() && if text_all { words.len() == terms.len() } else { !words.is_empty() };
        let prob = found.as_ref().and_then(|f| f.raw.get(&m.skey).map(|c| (f.probability)(*c)));
        let srank = sem_rank.get(&m.skey).copied();
        // by meaning: among the closest pictures (a rank, not an absolute probability, which SigLIP
        // keeps very low for French sentences), or above `minProbability` when one is asked for
        let above = found.as_ref().is_some_and(|f| f.passes(m.skey));
        let sem_ok = above && if use_min_prob { prob.is_some_and(|p| p >= min_prob) } else { srank.is_some_and(|r| r <= semantic_top) };
        let what_ok = terms.is_empty() || text_ok || sem_ok;
        // reciprocal rank fusion of the two orders
        let score = text_rank.get(&mi).map_or(0.0, |r| 1.0 / (RRF_K + *r as f32)) + srank.map_or(0.0, |r| 1.0 / (RRF_K + r as f32));
        match (people_ok, what_ok) {
            (true, true) => {
                results.push((score, row(m, e.as_ref(), &present, &words, prob, found.as_ref().and_then(|f| f.raw.get(&m.skey).copied()), &said[mi])))
            }
            (true, false) if !required.is_empty() => present_only
                .push((score, row(m, e.as_ref(), &present, &words, prob, found.as_ref().and_then(|f| f.raw.get(&m.skey).copied()), &said[mi]))),
            (false, true) if !required.is_empty() && !terms.is_empty() => what_only
                .push((score, row(m, e.as_ref(), &present, &words, prob, found.as_ref().and_then(|f| f.raw.get(&m.skey).copied()), &said[mi]))),
            _ => {}
        }
    }
    let top = |mut v: Vec<(f32, Value)>, n: usize| -> (usize, Vec<Value>) {
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        (v.len(), v.into_iter().take(n).map(|x| x.1).collect())
    };
    let (total, results) = top(results, limit);
    if bool_or(p, "filter", false) {
        let mut seen = HashSet::new();
        s.filter.only = results.iter().filter_map(|r| r["id"].as_u64()).map(PhotoId).filter(|i| seen.insert(*i)).collect();
    }
    let (n_present, present_only) = top(present_only, 20);
    let (n_what, what_only) = top(what_only, 20);
    Ok(json!({
        "query": query, "people": required.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>(), "words": terms,
        "semantic": semantic, "window": window_ms as f64 / 1000.0, "searched": moments.len(),
        "total": total, "results": results,
        "diagnostics": {
            "personPresentWhatNotDetected": {"total": n_present, "examples": present_only},
            "whatFoundPeopleNotRecognized": {"total": n_what, "examples": what_only},
            "unknownPeople": unknown_people,
            "note": "not detected is not a proof of absence: a face too small, turned away or not confirmed is not counted; a picture not indexed has no meaning to compare",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_names_people_and_words() {
        let people = vec![(1, "Madson Kurtis".to_string()), (2, "Awa".to_string()), (3, "Élodie Martin".to_string())];
        let (p, rest) = people_in("Madson conduit une voiture", &people);
        assert_eq!((p.iter().map(|x| x.0).collect::<Vec<_>>(), rest.as_str()), (vec![1], "conduit une voiture"));
        let (p, rest) = people_in("elodie martin et Awa à la plage", &people);
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(rest, "et a la plage");
        let (p, _) = people_in("une awale", &people);
        assert!(p.is_empty(), "a word that starts like a name is not the name");
        let terms: Vec<String> = ["conduit", "voiture"].iter().map(|x| x.to_string()).collect();
        assert_eq!(matched(&terms, &fold("Homme conduisant en parlant, VOITURE en route")), vec!["conduit", "voiture"]);
        assert!(matched(&terms, &fold("Salle de danse")).is_empty());
        let danse = vec!["danse".to_string()];
        assert!(matched(&danse, "elle est dans la salle").is_empty() && matched(&danse, "ils dansent").len() == 1);
        assert_eq!(fold("  Œuvre — Éclairée !! "), "oeuvre eclairee");
        assert_eq!(fold(""), "");
    }
}
