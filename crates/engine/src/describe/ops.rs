//! What the `describe.*` commands do: write, confirm, delete and undo what a person says a photo
//! or a moment of a video shows; list it with who is there.

use serde_json::{Value, json};

use super::{Annotation, MANUAL, Status, Store};
use crate::cmd::{bad, str_param};
use crate::{EngineError, Result, Session};

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

/// One slot as it was before a step (`None`: empty), for `describe.undo`.
type Before = ((String, Option<u64>, String), Option<Annotation>);

/// What the session keeps for annotations: the undo steps, and the proposal run.
#[derive(Default)]
pub struct Describe {
    history: Vec<(String, Vec<Before>)>,
    /// `describe.suggest` in progress (or the last one).
    pub job: std::sync::Arc<super::suggest::Job>,
}

pub(crate) fn library_dir(s: &Session) -> Option<std::path::PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone())
}

pub(crate) fn open(s: &Session) -> Store {
    Store::open(library_dir(s).as_deref())
}

/// The photo or video a command names (`id`, else the active photo), its content key, and the
/// moment (`ms`, videos only).
pub(crate) fn target(s: &Session, p: &Value, cmd: &str) -> Result<(lightcraft_catalog::PhotoId, String, Option<u64>)> {
    let id = p.get("id").and_then(Value::as_u64).map(lightcraft_catalog::PhotoId).or(s.active()).ok_or_else(|| bad(cmd, "no photo: pass `id`"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(cmd, format!("no photo {}", id.0)))?;
    let ms = match p.get("ms") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0 && *x < 864_000_000.0)
                .ok_or_else(|| bad(cmd, "`ms` must be milliseconds into the video"))? as u64,
        ),
    };
    if ms.is_some() && ph.kind != lightcraft_catalog::MediaKind::Video {
        return Err(bad(cmd, "`ms` is for a moment of a video; this is a photo"));
    }
    Ok((id, crate::media::content_key(ph), ms))
}

fn remember(s: &mut Session, label: &str, before: Vec<Before>) {
    s.describe.history.push((label.to_string(), before));
    if s.describe.history.len() > 100 {
        s.describe.history.remove(0);
    }
}

fn text_param(p: &Value, key: &str, cmd: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) => Ok(Some(t.clone())),
        Some(_) => Err(bad(cmd, format!("`{key}` must be text"))),
    }
}

/// `describe.set`: what the person typed, for the fields given (an empty string clears one).
pub fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "describe.set";
    let (id, item, ms) = target(s, p, C)?;
    let fields = [text_param(p, "scene", C)?, text_param(p, "action", C)?, text_param(p, "description", C)?];
    if fields.iter().all(Option::is_none) {
        return Err(bad(C, "give at least one of `scene`, `action`, `description`"));
    }
    let mut store = open(s);
    let prev = store.get(&item, ms, MANUAL).cloned();
    let base = prev.clone().unwrap_or_default();
    let [scene, action, description] = fields;
    let mut a = Annotation::new(
        &item,
        ms,
        scene.as_deref().unwrap_or(&base.scene),
        action.as_deref().unwrap_or(&base.action),
        description.as_deref().unwrap_or(&base.description),
        MANUAL,
        &(s.clock)(),
    );
    // editing what one confirmed keeps it confirmed: the person wrote it
    a.status = base.status;
    let empty = a.is_empty();
    a.deleted = empty;
    store.add(vec![a]).map_err(other)?;
    remember(s, "Describe", vec![((item.clone(), ms, MANUAL.into()), prev)]);
    Ok(json!({"id": id.0, "ms": ms, "annotation": store.effective(&item, ms), "cleared": empty}))
}

/// `describe.confirm`: a person says the description is right (what they typed, else the
/// proposal shown). Only this command writes `confirmed`.
pub fn confirm(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "describe.confirm";
    let (id, item, ms) = target(s, p, C)?;
    let mut store = open(s);
    let e =
        store.effective(&item, ms).ok_or_else(|| bad(C, "nothing is written for this photo or moment: describe.set or describe.suggest first"))?;
    let prev = store.get(&item, ms, MANUAL).cloned();
    let mut a = Annotation::new(&item, ms, &e.scene, &e.action, &e.description, MANUAL, &(s.clock)());
    a.status = Status::Confirmed;
    store.add(vec![a]).map_err(other)?;
    remember(s, "Confirm description", vec![((item.clone(), ms, MANUAL.into()), prev)]);
    Ok(json!({"id": id.0, "ms": ms, "annotation": store.effective(&item, ms)}))
}

/// `describe.delete`: take back what the person wrote (`source: manual`, the default), one
/// program's proposal (`source: siglip`…), or everything of the moment (`source: all`).
pub fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "describe.delete";
    let (id, item, ms) = target(s, p, C)?;
    let source = str_param(p, "source").unwrap_or(MANUAL).trim().to_string();
    let mut store = open(s);
    let gone: Vec<Annotation> =
        store.all().into_iter().filter(|a| a.item == item && a.ms == ms && (source == "all" || a.source == source)).cloned().collect();
    if gone.is_empty() {
        return Ok(json!({"id": id.0, "ms": ms, "deleted": 0}));
    }
    let before: Vec<Before> = gone.iter().map(|a| ((a.item.clone(), a.ms, a.source.clone()), Some(a.clone()))).collect();
    let now = (s.clock)();
    let marks: Vec<Annotation> = gone.iter().map(|a| Annotation { deleted: true, at: now.clone(), ..a.clone() }).collect();
    store.add(marks).map_err(other)?;
    remember(s, "Delete description", before);
    Ok(json!({"id": id.0, "ms": ms, "deleted": gone.len(), "annotation": store.effective(&item, ms)}))
}

/// `describe.undo`: the last set / confirm / delete of this session, taken back.
pub fn undo(s: &mut Session) -> Result<Value> {
    let Some((label, before)) = s.describe.history.pop() else { return Ok(json!({"undone": Value::Null})) };
    let mut store = open(s);
    let now = (s.clock)();
    let mut back = Vec::new();
    for ((item, ms, source), prev) in before {
        back.push(match prev {
            Some(a) => Annotation { at: now.clone(), deleted: false, ..a },
            None => {
                let mut a = Annotation::new(&item, ms, "", "", "", &source, &now);
                a.deleted = true;
                a
            }
        });
    }
    store.add(back).map_err(other)?;
    Ok(json!({"undone": label}))
}

/// Who is there at `key` (named people of the face index), as JSON.
fn people_at(_: &mut Session, _: u64) -> Value {
    json!([])
}

/// `describe.list`: what a photo shows, or each moment of a video, every source together.
pub fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "describe.list";
    let (id, item, _) = target(s, &json!({"id": p.get("id")}), C)?;
    let store = open(s);
    let frames: std::collections::HashMap<u64, String> = s.frames_of_video(id).into_iter().map(|(k, f)| (k, f.file.display().to_string())).collect();
    let mut rows = Vec::new();
    for (it, ms) in store.moments().into_iter().filter(|(i, _)| *i == item) {
        let Some(e) = store.effective(&it, ms) else { continue };
        let key = match ms {
            Some(ms) => crate::video::frame_key(&it, ms),
            None => crate::video::item_key(&it),
        };
        let mut v = json!(e);
        v["people"] = people_at(s, key);
        if let Some(f) = frames.get(&key) {
            v["frame"] = json!(f);
        }
        let hints: Vec<&String> = e.proposals.iter().flat_map(|a| a.hints.iter()).collect();
        if !hints.is_empty() {
            v["hints"] = json!(hints);
        }
        rows.push((ms, v));
    }
    rows.sort_by_key(|(ms, _)| *ms);
    Ok(json!({"id": id.0, "total": rows.len(), "annotations": rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>(), "skippedLines": store.skipped}))
}
