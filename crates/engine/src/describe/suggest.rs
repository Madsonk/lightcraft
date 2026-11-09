//! `describe.suggest`: proposals (scene, action) for chosen photos or videos, by SigLIP 2 zero-shot
//! (the semantic search model, `crate::search`): each picture is compared with a list of French
//! scenes and actions, and the best of each above a threshold becomes a `candidate` annotation,
//! `source: siglip`. Nothing is proposed without the model (a clear error says so), nothing is
//! confirmed, and only the photos or videos asked for are looked at — never the whole library.
//!
//! The lists are built in ([`SCENES`], [`ACTIONS`]) and extended by the user's
//! `<config>/describe-vocabulary.json` (`{"scenes": [...], "actions": [...], "replace": false}`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Value, json};

#[cfg(feature = "search")]
use super::{Annotation, Store};
#[cfg(feature = "search")]
use crate::cmd::{bad, bool_or, f64_or};
use crate::{EngineError, Result, Session};

/// Where the user's own scenes and actions are kept.
pub const VOCABULARY_FILE: &str = "describe-vocabulary.json";
/// Who writes these proposals.
pub const SOURCE: &str = "siglip";
/// Probability a label needs to be proposed (SigLIP's probabilities are low; see `search.tag`).
pub const DEFAULT_THRESHOLD: f64 = 0.02;
/// Most pictures one call looks at.
pub const MAX_PICTURES: usize = 2000;

pub const SCENES: &[&str] = &[
    "une plage",
    "la montagne",
    "une forêt",
    "une rue en ville",
    "une ville la nuit",
    "l'intérieur d'une maison",
    "une cuisine",
    "une salle de bain",
    "une chambre",
    "un bureau",
    "l'intérieur d'une voiture",
    "une route",
    "un parking",
    "un restaurant",
    "une terrasse de café",
    "une boîte de nuit",
    "une scène de concert",
    "une salle de sport",
    "un studio photo",
    "un jardin ou un parc",
    "une piscine",
    "la mer",
    "la campagne",
    "un marché",
    "un magasin",
    "un stade",
    "un aéroport",
    "une gare",
    "un mariage",
    "une fête",
    "la neige",
    "une salle de danse",
];

pub const ACTIONS: &[&str] = &[
    "conduire une voiture",
    "parler face à la caméra",
    "danser",
    "chanter",
    "manger",
    "boire un verre",
    "cuisiner",
    "marcher",
    "courir",
    "faire du vélo",
    "nager",
    "rire",
    "se brosser les dents",
    "poser pour une photo",
    "faire un selfie",
    "jouer de la musique",
    "travailler sur un ordinateur",
    "lire",
    "dormir",
    "faire du sport",
    "jouer avec un enfant",
    "se serrer dans les bras",
    "applaudir",
    "filmer avec une caméra",
    "téléphoner",
    "donner une interview",
    "faire la fête",
    "regarder un écran",
];

/// The scenes and actions, the user's file merged in.
pub fn vocabulary(file: Option<&std::path::Path>) -> (Vec<String>, Vec<String>, Option<String>) {
    let (mut scenes, mut actions): (Vec<String>, Vec<String>) =
        (SCENES.iter().map(|x| x.to_string()).collect(), ACTIONS.iter().map(|x| x.to_string()).collect());
    let mut note = None;
    if let Some(f) = file
        && let Ok(bytes) = std::fs::read(f)
    {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => {
                let list = |k: &str| -> Vec<String> {
                    v.get(k)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty() && x.len() <= 200)
                        .take(500)
                        .collect()
                };
                if v.get("replace").and_then(Value::as_bool) == Some(true) {
                    scenes.clear();
                    actions.clear();
                }
                for (dst, add) in [(&mut scenes, list("scenes")), (&mut actions, list("actions"))] {
                    for a in add {
                        if !dst.contains(&a) {
                            dst.push(a);
                        }
                    }
                }
            }
            Err(e) => note = Some(format!("{} could not be read ({e}): the built-in lists are used", f.display())),
        }
    }
    (scenes, actions, note)
}

/// A `describe.suggest` run.
#[derive(Default)]
pub struct Job {
    pub running: AtomicBool,
    pub cancel: AtomicBool,
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub written: AtomicU64,
    text: Mutex<(Option<String>, Vec<Value>)>,
}

impl Job {
    fn lock(&self) -> std::sync::MutexGuard<'_, (Option<String>, Vec<Value>)> {
        self.text.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn report(&self) -> Value {
        let g = self.lock();
        json!({
            "running": self.running.load(Ordering::SeqCst), "cancelled": self.cancel.load(Ordering::SeqCst),
            "total": self.total.load(Ordering::SeqCst), "done": self.done.load(Ordering::SeqCst),
            "written": self.written.load(Ordering::SeqCst), "error": g.0, "proposals": g.1,
        })
    }
}

/// One picture to describe.
#[cfg(feature = "search")]
struct Moment {
    item: String,
    ms: Option<u64>,
    id: u64,
    skey: u64,
}

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

#[cfg(not(feature = "search"))]
pub fn suggest(_: &mut Session, _: &Value) -> Result<Value> {
    Err(other(format!("{} Proposals for scenes and actions need it; nothing was written.", crate::search::UNAVAILABLE)))
}

#[cfg(feature = "search")]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `describe.suggest {ids?, ms?, threshold?, background?, limit?, scenes?, actions?}`.
#[cfg(feature = "search")]
pub fn suggest(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "describe.suggest";
    s.searcher.model_dir().map_err(|e| other(format!("{e} Proposals for scenes and actions need it; nothing was written.")))?;
    let has_ids = p.get("ids").is_some() || p.get("id").is_some();
    if !has_ids && s.selection.ids.is_empty() && s.selection.active.is_none() {
        return Err(bad(C, "choose the photos or videos to describe (`ids`, or a selection): the whole library is never described at once"));
    }
    let ids = s.targets(p)?;
    let limit = f64_or(p, "limit", 500.0).clamp(1.0, MAX_PICTURES as f64) as usize;
    let only_ms = p.get("ms").and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map(|x| x as u64);
    let threshold = f64_or(p, "threshold", DEFAULT_THRESHOLD).clamp(0.0, 1.0) as f32;
    let background = bool_or(p, "background", false);
    // the pictures: a photo itself, a video's pictures (or the one nearest `ms`)
    let mut moments = Vec::new();
    let mut pics = Vec::new();
    let mut videos_without = 0;
    let loader = s.media.file_loader.clone();
    for id in ids {
        if moments.len() >= limit {
            break;
        }
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        let ck = crate::media::content_key(&ph);
        if ph.kind == lightcraft_catalog::MediaKind::Video {
            let mut fr = s.frames_of_video(id);
            if fr.is_empty() {
                videos_without += 1;
                continue;
            }
            if let Some(t) = only_ms {
                fr.sort_by_key(|(_, f)| f.ms.abs_diff(t));
                fr.truncate(1);
            }
            for (_, f) in fr.into_iter().take(limit.saturating_sub(moments.len())) {
                let skey = crate::search::search_key(&ck, Some(f.ms));
                let source = crate::media::SourceRef::File {
                    path: f.file.display().to_string(),
                    max_edge: crate::media::SourceLevel::Thumb.max_edge(),
                    loader: loader.clone(),
                    fallback: None,
                    denoise: None,
                    reduced: None,
                    camera_profile: None,
                    outliers: false,
                };
                pics.push(crate::search::Picture { id, key: skey, source });
                moments.push(Moment { item: ck.clone(), ms: Some(f.ms), id: id.0, skey });
            }
        } else {
            let skey = crate::search::search_key(&ck, None);
            let source = s.media.source_ref(&ph, crate::media::SourceLevel::Thumb);
            pics.push(crate::search::Picture { id, key: skey, source });
            moments.push(Moment { item: ck, ms: None, id: id.0, skey });
        }
    }
    if moments.is_empty() {
        return Err(bad(
            C,
            if videos_without > 0 { "these videos have no pictures yet: video.frames first" } else { "no photo or video to describe" },
        ));
    }
    let (mut scenes, mut actions, note) = vocabulary(crate::config::config_dir().map(|d| d.join(VOCABULARY_FILE)).as_deref());
    for (k, dst) in [("scenes", &mut scenes), ("actions", &mut actions)] {
        if let Some(a) = p.get(k).and_then(Value::as_array) {
            *dst = a.iter().filter_map(Value::as_str).map(str::to_string).filter(|x| !x.trim().is_empty()).take(500).collect();
        }
    }
    if scenes.is_empty() && actions.is_empty() {
        return Err(bad(C, "the lists of scenes and actions are empty"));
    }
    let job = s.describe.job.clone();
    if job.running.swap(true, Ordering::SeqCst) {
        return Err(other("proposals are already being made (describe.status; describe.cancel stops it)"));
    }
    job.cancel.store(false, Ordering::SeqCst);
    job.total.store(moments.len() as u64, Ordering::SeqCst);
    job.done.store(0, Ordering::SeqCst);
    job.written.store(0, Ordering::SeqCst);
    *job.lock() = (None, Vec::new());
    let searcher = s.searcher.share();
    let dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("search"));
    let lib = super::ops::library_dir(s);
    let now = (s.clock)();
    let work_job = job.clone();
    let work = move || {
        let job = work_job;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&job, &searcher, dir, lib, pics, moments, &scenes, &actions, threshold, &now)
        }));
        let error = match r {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some("making the proposals failed unexpectedly".into()),
        };
        job.lock().0 = error;
        job.running.store(false, Ordering::SeqCst);
    };
    let handle = std::thread::Builder::new().name("describe-suggest".into()).spawn(work).map_err(|e| {
        job.running.store(false, Ordering::SeqCst);
        other(format!("could not start: {e}"))
    })?;
    if !background {
        let _ = handle.join();
    }
    let mut v = job.report();
    v["background"] = json!(background);
    v["videosWithoutFrames"] = json!(videos_without);
    v["note"] = json!(note);
    Ok(v)
}

#[cfg(feature = "search")]
#[allow(clippy::too_many_arguments)]
fn run(
    job: &Job,
    searcher: &crate::search::Searcher,
    dir: Option<std::path::PathBuf>,
    lib: Option<std::path::PathBuf>,
    pics: Vec<crate::search::Picture>,
    moments: Vec<Moment>,
    scenes: &[String],
    actions: &[String],
    threshold: f32,
    now: &str,
) -> std::result::Result<(), String> {
    let prompts: Vec<String> = scenes
        .iter()
        .map(|x| format!("une photo de {x}"))
        .chain(actions.iter().map(|x| format!("une photo d'une personne en train de {x}")))
        .collect();
    let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
    let text = searcher.embed_texts(&refs)?;
    if job.cancel.load(Ordering::SeqCst) {
        return Ok(());
    }
    crate::search::embed_pictures(searcher, dir.clone(), pics)?;
    let keys: Vec<u64> = moments.iter().map(|m| m.skey).collect();
    let vectors = crate::search::vectors_of(searcher, dir, &keys)?;
    let mut store = Store::open(lib.as_deref());
    let mut new = Vec::new();
    let mut shown = Vec::new();
    for m in &moments {
        if job.cancel.load(Ordering::SeqCst) {
            break;
        }
        job.done.fetch_add(1, Ordering::SeqCst);
        let Some(v) = vectors.get(&m.skey) else { continue };
        let probs: Vec<f32> = text.iter().map(|t| searcher.probability(dot(v, t)).unwrap_or(0.0)).collect();
        let best = |range: std::ops::Range<usize>, names: &[String]| -> Option<(String, f32)> {
            range
                .zip(names)
                .filter_map(|(i, n)| probs.get(i).map(|p| (n.clone(), *p)))
                .filter(|(_, p)| *p >= threshold)
                .max_by(|a, b| a.1.total_cmp(&b.1))
        };
        let scene = best(0..scenes.len(), scenes);
        let action = best(scenes.len()..scenes.len() + actions.len(), actions);
        if scene.is_none() && action.is_none() {
            continue;
        }
        let score = scene.iter().chain(action.iter()).map(|x| x.1).fold(0.0f32, f32::max);
        let mut a = Annotation::new(
            &m.item,
            m.ms,
            scene.as_ref().map_or("", |x| x.0.as_str()),
            action.as_ref().map_or("", |x| x.0.as_str()),
            "",
            SOURCE,
            now,
        );
        a.score = Some((score * 1000.0).round() / 1000.0);
        if store.get(&m.item, m.ms, SOURCE).is_some_and(|old| old.same_text(&a)) {
            continue;
        }
        if shown.len() < 50 {
            shown.push(json!({"id": m.id, "ms": m.ms, "scene": a.scene, "action": a.action, "score": a.score}));
        }
        new.push(a);
    }
    job.written.store(new.len() as u64, Ordering::SeqCst);
    job.lock().1 = shown;
    store.add(new)
}

/// `describe.cancel`.
pub fn cancel(s: &mut Session) -> Value {
    let running = s.describe.job.running.load(Ordering::SeqCst);
    if running {
        s.describe.job.cancel.store(true, Ordering::SeqCst);
        s.searcher.cancel_job();
    }
    json!({"cancelled": running})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_vocabulary_extends_or_replaces_the_lists() {
        let d = std::env::temp_dir().join(format!("lc-vocab-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join(VOCABULARY_FILE);
        let (s0, a0, n) = vocabulary(Some(&f));
        assert!(n.is_none() && s0.len() == SCENES.len() && a0.len() == ACTIONS.len());
        std::fs::write(&f, r#"{"scenes": ["un atelier", "une plage"], "actions": ["souder"]}"#).unwrap();
        let (s1, a1, _) = vocabulary(Some(&f));
        assert_eq!((s1.len(), a1.len()), (SCENES.len() + 1, ACTIONS.len() + 1));
        std::fs::write(&f, r#"{"replace": true, "scenes": ["un atelier"], "actions": [3, null]}"#).unwrap();
        let (s2, a2, _) = vocabulary(Some(&f));
        assert_eq!((s2, a2.len()), (vec!["un atelier".to_string()], 0));
        std::fs::write(&f, b"{not json").unwrap();
        let (s3, _, n) = vocabulary(Some(&f));
        assert!(n.is_some() && s3.len() == SCENES.len());
        let _ = std::fs::remove_dir_all(&d);
    }
}
