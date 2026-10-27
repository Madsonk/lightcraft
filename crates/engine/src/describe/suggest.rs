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

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

pub fn suggest(_: &mut Session, _: &Value) -> Result<Value> {
    Err(other(format!("{} Proposals for scenes and actions need it; nothing was written.", crate::search::UNAVAILABLE)))
}

/// `describe.suggest {ids?, ms?, threshold?, background?, limit?, scenes?, actions?}`.

/// `describe.cancel`.
pub fn cancel(s: &mut Session) -> Value {
    let running = s.describe.job.running.load(Ordering::SeqCst);
    if running {
        s.describe.job.cancel.store(true, Ordering::SeqCst);
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
