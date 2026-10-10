//! What the `people.*` commands do (feature `people`): opening the library's index, indexing,
//! the overview of persons, groups and suggestions, naming, confirming, merging, undo.
//!
//! A decision is recorded in the people file ([`lightcraft_people::State`]) and, for a named
//! person, as the keyword `People|<name>` on every photo of their confirmed faces — one undo step
//! of the session, folded with the others the decision needed — so the rest of LightCraft
//! (filters, smart albums, export, XMP) sees people as ordinary keywords. Nothing is tagged
//! without the user's yes, unless they set an auto-tag threshold.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use lightcraft_catalog::{MediaKind, PhotoId};
use lightcraft_people::cluster::{self, Group, Known};
use lightcraft_people::state::State;
use lightcraft_people::store::{FaceRef, FaceStore};
use lightcraft_people::{FAST_MODEL, MODEL, ModelInfo, key_of};
use serde_json::{Value, json};

use super::core::{Entry, Item, Slot, lock};
use super::views::{GroupView, Likely, Overview, SuggestionView, Thumb};
use super::{KEYWORD_ROOT, People};
use crate::cmd::{bad, bool_or, f64_or, str_param};
use crate::media::SourceLevel;
use crate::{EngineError, Result, Session};

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

/// The keyword that tags a person.
pub fn keyword_of(name: &str) -> String {
    format!("{KEYWORD_ROOT}|{}", name.trim().replace('|', "-"))
}

/// The keyword of the photos of faces marked "unknown person", when `tagUnknown` is on.
pub const UNKNOWN_KEYWORD: &str = "People|Unknown";

pub fn face_id(f: FaceRef) -> String {
    format!("{:016x}:{}", f.0, f.1)
}

pub fn parse_face(t: &str) -> Option<FaceRef> {
    let t = t.strip_prefix("f:").unwrap_or(t);
    let (k, i) = t.split_once(':')?;
    Some(FaceRef(u64::from_str_radix(k, 16).ok()?, i.parse().ok()?))
}

fn model_of(fast: bool) -> &'static ModelInfo {
    if fast { &FAST_MODEL } else { &MODEL }
}

// ------------------------------------------------------------------------------------ the slot

impl People {
    /// `<library>/people`, or `None` when the library isn't on disk.
    fn store_dir(library: Option<&crate::library::Library>) -> Option<PathBuf> {
        library.filter(|l| l.on_disk).map(|l| l.dir.join("people"))
    }
}

/// Open (or reopen, when the library or the model changed) the index and the decisions of the
/// session's library.
pub fn ensure_open(s: &mut Session) -> std::result::Result<(), String> {
    let dir = People::store_dir(s.library.as_ref());
    let slot_arc = s.people.core.slot.clone();
    let mut g = lock(&slot_arc);
    // the decisions come first: they say which model's index to read
    let state_path = dir.as_ref().map(|d| d.join("people.json"));
    if let Some(sl) = g.as_ref()
        && sl.dir == dir
    {
        return Ok(());
    }
    let (state, note) = state_path.as_deref().map_or((State::default(), None), State::load);
    let model = model_of(state.settings.fast_model);
    let path = dir.as_ref().map(|d| d.join(format!("faces-{}.bin", model.id)));
    let store = FaceStore::open(path.as_deref(), model.id).map_err(|e| format!("the people index: {e}"))?;
    *g = Some(Slot {
        dir,
        model,
        store,
        state,
        state_path,
        store_rev: 1,
        state_rev: 1,
        computed: None,
        history: Vec::new(),
        note,
        skipped: HashSet::new(),
    });
    Ok(())
}

fn with_slot<R>(s: &mut Session, f: impl FnOnce(&mut Slot) -> R) -> Result<R> {
    ensure_open(s).map_err(other)?;
    let arc = s.people.core.slot.clone();
    let mut g = lock(&arc);
    let slot = g.as_mut().ok_or_else(|| other("the people index is not open"))?;
    Ok(f(slot))
}

/// Remove from the grouping what the decisions already settled.
pub fn reconcile(sl: &mut Slot) {
    let Some(c) = sl.computed.as_mut() else { return };
    let confirmed: HashSet<FaceRef> = sl.state.persons.iter().flat_map(|p| p.faces.iter().copied()).collect();
    let ignored: HashSet<FaceRef> = sl.state.ignored.iter().copied().collect();
    let store = &sl.store;
    let gone = |slot: usize| store.meta(slot).is_none_or(|m| confirmed.contains(&m.face()) || ignored.contains(&m.face()));
    for g in &mut c.groups {
        g.members.retain(|m| !gone(*m));
    }
    c.groups.retain(|g| !g.members.is_empty());
    c.suggestions.retain(|sg| {
        !gone(sg.slot) && sl.state.person(sg.person).is_some() && store.meta(sg.slot).is_some_and(|m| !sl.state.is_rejected(m.face(), sg.person))
    });
}

fn save(sl: &mut Slot) -> Result<()> {
    sl.state_rev += 1;
    if let Some(p) = &sl.state_path {
        sl.state.save(p).map_err(|e| other(format!("saving the people file: {e}")))?;
    }
    Ok(())
}

fn remember(sl: &mut Slot, label: &str) {
    sl.history.push(Entry { label: label.to_string(), state: sl.state.clone(), committed: None });
    if sl.history.len() > 50 {
        sl.history.remove(0);
    }
}

// ------------------------------------------------------------------------------------ photos

/// Photos of the library by the key of their content.
fn key_map(s: &Session) -> HashMap<u64, Vec<PhotoId>> {
    let mut m: HashMap<u64, Vec<PhotoId>> = HashMap::new();
    for p in s.catalog.photos().filter(|p| p.in_library() && p.kind != MediaKind::Video) {
        m.entry(key_of(&crate::media::content_key(p))).or_default().push(p.id);
    }
    m
}

fn photos_of(map: &HashMap<u64, Vec<PhotoId>>, keys: impl IntoIterator<Item = u64>) -> Vec<PhotoId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for k in keys {
        for id in map.get(&k).into_iter().flatten() {
            if seen.insert(*id) {
                out.push(*id);
            }
        }
    }
    out.sort_by_key(|i| i.0);
    out
}

// ------------------------------------------------------------------------------------ views

fn thumbs_of(sl: &Slot, slots: &[usize], first: &HashMap<u64, PhotoId>, n: usize) -> Vec<Thumb> {
    slots
        .iter()
        .filter_map(|s| {
            let m = sl.store.meta(*s)?;
            Some(Thumb { photo: first.get(&m.key)?.0, face: face_id(m.face()), rect: m.rect })
        })
        .take(n)
        .collect()
}

fn mean_sim(sl: &Slot, slots: &[usize], centroid: &[f32]) -> f32 {
    if slots.is_empty() {
        return 0.0;
    }
    slots.iter().map(|s| cluster::dot(sl.store.vector(*s), centroid)).sum::<f32>() / slots.len() as f32
}

/// Which list a computed group belongs to: to name, "unknown person" (half or more of its faces
/// were marked so), or left for later (half or more of its faces were skipped).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Listed {
    Name,
    Unknown,
    Skipped,
}

pub(crate) fn listed(sl: &Slot, members: &[usize]) -> Listed {
    let faces: Vec<FaceRef> = members.iter().filter_map(|m| sl.store.meta(*m).map(|x| x.face())).collect();
    let n = faces.len();
    if n == 0 {
        return Listed::Name;
    }
    if faces.iter().filter(|f| sl.state.is_unknown(**f)).count() * 2 >= n {
        Listed::Unknown
    } else if faces.iter().filter(|f| sl.skipped.contains(f)).count() * 2 >= n {
        Listed::Skipped
    } else {
        Listed::Name
    }
}

/// The photos (by content key) of a person: those of their confirmed faces and those the user
/// filed under them as a whole.
fn person_keys(state: &State, pid: u32) -> HashSet<u64> {
    let mut keys: HashSet<u64> = state.person(pid).map(|p| p.faces.iter().map(|f| f.0).collect()).unwrap_or_default();
    keys.extend(state.photo_tags.iter().filter(|(_, p)| *p == pid).map(|(k, _)| *k));
    keys
}

/// Build the overview of the open index. `groups` / `suggestions` bound the lists.
pub fn overview(s: &mut Session, groups: usize, suggestions: usize, thumbs: usize) -> Result<Overview> {
    ensure_open(s).map_err(other)?;
    let map = key_map(s);
    let first: HashMap<u64, PhotoId> = map.iter().filter_map(|(k, v)| v.first().map(|p| (*k, *p))).collect();
    let computing = s.people.core.computing();
    let arc = s.people.core.slot.clone();
    let g = lock(&arc);
    let sl = g.as_ref().ok_or_else(|| other("the people index is not open"))?;
    let known = cluster::known_people(&sl.store, &sl.state);
    let mut ov = Overview { indexed_photos: sl.store.photo_count(), indexed_faces: sl.store.face_count(), computing, ..Default::default() };
    ov.revision = sl.store_rev * 1_000_000 + sl.state_rev;
    // persons
    for p in &sl.state.persons {
        let slots: Vec<usize> = p.faces.iter().filter_map(|f| sl.store.slot(*f)).collect();
        let mut best = slots.clone();
        best.sort_by(|a, b| sl.store.meta(*b).map_or(0.0, |m| m.quality()).total_cmp(&sl.store.meta(*a).map_or(0.0, |m| m.quality())));
        let present = photos_of(&map, person_keys(&sl.state, p.id)).len();
        ov.persons.push(GroupView {
            id: format!("p:{}", p.id),
            person: Some(p.id),
            name: p.name.clone(),
            keyword: p.name.as_deref().map(keyword_of),
            faces: slots.len(),
            photos: present,
            thumbs: thumbs_of(sl, &best, &first, thumbs),
            likely: Vec::new(),
            tightness: known.iter().find(|k| k.id == p.id).map_or(0.0, |k| mean_sim(sl, &slots, &k.centroid)),
        });
    }
    ov.persons.sort_by(|a, b| b.photos.cmp(&a.photos).then(a.id.cmp(&b.id)));
    // groups and suggestions
    if let Some(c) = &sl.computed {
        ov.grouped = true;
        ov.stale = c.store_rev != sl.store_rev;
        ov.left_out = c.left_out;
        for gr in &c.groups {
            let list = listed(sl, &gr.members);
            let target = match list {
                Listed::Name => {
                    ov.groups_total += 1;
                    (ov.groups.len() < groups).then_some(&mut ov.groups)
                }
                Listed::Unknown => {
                    ov.unknown_total += 1;
                    ov.unknown_faces += gr.members.len();
                    (ov.unknown.len() < groups).then_some(&mut ov.unknown)
                }
                Listed::Skipped => {
                    ov.skipped_groups += 1;
                    None
                }
            };
            let Some(target) = target else { continue };
            let keys: HashSet<u64> = gr.members.iter().filter_map(|x| sl.store.meta(*x).map(|m| m.key)).collect();
            target.push(GroupView {
                id: format!("g:{}", gr.members[0]),
                person: None,
                name: None,
                keyword: None,
                faces: gr.members.len(),
                photos: photos_of(&map, keys).len(),
                thumbs: thumbs_of(sl, &gr.members, &first, thumbs),
                likely: likely_for(&sl.state, &known, &gr.centroid, sl.state.settings.suggest - 0.1),
                tightness: mean_sim(sl, &gr.members, &gr.centroid),
            });
        }
        ov.suggestions_total = c.suggestions.len();
        for sg in c.suggestions.iter().take(suggestions) {
            let Some(m) = sl.store.meta(sg.slot) else { continue };
            let Some(photo) = first.get(&m.key) else { continue };
            ov.suggestions.push(SuggestionView {
                face: face_id(m.face()),
                photo: photo.0,
                rect: m.rect,
                person: sg.person,
                name: sl.state.person(sg.person).and_then(|p| p.name.clone()),
                score: sg.score,
                runner_up: sg.runner_up.map(|(person, score)| Likely { person, name: sl.state.person(person).and_then(|p| p.name.clone()), score }),
            });
        }
    }
    Ok(ov)
}

fn likely_for(state: &State, known: &[Known], centroid: &[f32], floor: f32) -> Vec<Likely> {
    let mut v: Vec<Likely> = known
        .iter()
        .map(|k| Likely { person: k.id, name: state.person(k.id).and_then(|p| p.name.clone()), score: cluster::dot(centroid, &k.centroid) })
        .filter(|l| l.score >= floor)
        .collect();
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    v.truncate(3);
    v
}

// ------------------------------------------------------------------------------------ targets

/// What a command names: a person, a group of faces, or one face.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Person(u32),
    Group(Vec<usize>),
    Face(FaceRef),
}

fn resolve(sl: &Slot, t: &str) -> std::result::Result<Target, String> {
    if let Some(id) = t.strip_prefix("p:") {
        let id: u32 = id.parse().map_err(|_| format!("`{t}` is not a person"))?;
        return sl.state.person(id).map(|_| Target::Person(id)).ok_or_else(|| format!("no person {id}"));
    }
    if let Some(slot) = t.strip_prefix("g:") {
        let slot: usize = slot.parse().map_err(|_| format!("`{t}` is not a group"))?;
        let c = sl.computed.as_ref().ok_or("the faces are not grouped yet: run people.refresh")?;
        return c
            .groups
            .iter()
            .find(|g| g.members.contains(&slot))
            .map(|g| Target::Group(g.members.clone()))
            .ok_or_else(|| format!("the group {t} changed: list the people again"));
    }
    if let Some(f) = parse_face(t) {
        return sl.store.slot(f).map(|_| Target::Face(f)).ok_or_else(|| format!("no such face {t}"));
    }
    Err(format!("`{t}` is not a person (p:N), a group (g:N) or a face (KEY:N)"))
}

fn faces_of(sl: &Slot, t: &Target) -> Vec<FaceRef> {
    match t {
        Target::Person(id) => sl.state.person(*id).map(|p| p.faces.clone()).unwrap_or_default(),
        Target::Group(slots) => slots.iter().filter_map(|s| sl.store.meta(*s).map(|m| m.face())).collect(),
        Target::Face(f) => vec![*f],
    }
}

fn target_param(sl: &Slot, p: &Value, key: &str, cmd: &str) -> Result<Target> {
    let t = str_param(p, key).ok_or_else(|| bad(cmd, format!("missing `{key}` (p:N, g:N or KEY:N from people.list)")))?;
    resolve(sl, t).map_err(|e| bad(cmd, e))
}

// ------------------------------------------------------------------------------------ keywords

/// Make the photos of `faces`' keys carry `kw` (add) or lose it (remove, unless `keep` still names
/// them). Returns how many photos changed.
fn tag(s: &mut Session, map: &HashMap<u64, Vec<PhotoId>>, keys: &HashSet<u64>, kw: &str, add: bool) -> Result<usize> {
    let ids = photos_of(map, keys.iter().copied());
    let changing: Vec<u64> = ids
        .iter()
        .filter(|id| s.catalog.photo(**id).is_some_and(|p| p.meta.keywords.iter().any(|k| k.eq_ignore_ascii_case(kw)) != add))
        .map(|i| i.0)
        .collect();
    if changing.is_empty() {
        return Ok(0);
    }
    let key = if add { "addKeywords" } else { "removeKeywords" };
    s.execute("photo.setMeta", &json!({"ids": changing, key: [kw]}))?;
    Ok(changing.len())
}

/// Bring the keyword of person `pid` in line with their confirmed faces: added to every photo of
/// theirs, removed from photos that no longer have one of their faces.
fn sync_person(s: &mut Session, pid: u32) -> Result<(usize, usize)> {
    let map = key_map(s);
    // the photos of their confirmed faces, plus those filed under them as a whole by hand: a photo
    // keeps the keyword as long as one of the two says so (a photo can hold several people)
    let (name, keys) = with_slot(s, |sl| (sl.state.person(pid).and_then(|p| p.name.clone()), person_keys(&sl.state, pid)))?;
    let Some(name) = name else { return Ok((0, 0)) };
    let kw = keyword_of(&name);
    let added = tag(s, &map, &keys, &kw, true)?;
    // photos that carry the keyword but have none of the person's faces
    let all: HashSet<u64> = map.keys().copied().collect();
    let stale: HashSet<u64> = all.difference(&keys).copied().collect();
    let removed = tag(s, &map, &stale, &kw, false)?;
    Ok((added, removed))
}

/// With `tagUnknown` on: the photos with a face marked "unknown person" carry `People|Unknown`, the
/// others don't. Does nothing when the setting is off.
fn sync_unknown(s: &mut Session) -> Result<(usize, usize)> {
    let (on, keys) = with_slot(s, |sl| (sl.state.settings.tag_unknown, sl.state.unknown.iter().map(|f| f.0).collect::<HashSet<u64>>()))?;
    if !on {
        return Ok((0, 0));
    }
    let map = key_map(s);
    let added = tag(s, &map, &keys, UNKNOWN_KEYWORD, true)?;
    let all: HashSet<u64> = map.keys().copied().collect();
    let stale: HashSet<u64> = all.difference(&keys).copied().collect();
    let removed = tag(s, &map, &stale, UNKNOWN_KEYWORD, false)?;
    Ok((added, removed))
}

/// Run `f` (a decision) as one undo step of the session and of the people file.
fn decision<R>(s: &mut Session, label: &str, f: impl FnOnce(&mut Session) -> Result<R>) -> Result<R> {
    with_slot(s, |sl| remember(sl, label))?;
    let start = s.undo.len();
    let r = f(s).and_then(|v| sync_unknown(s).map(|_| v));
    let added = s.undo.len().saturating_sub(start);
    if added > 1 {
        s.merge_undo(added, label);
    }
    let committed = (added > 0).then_some(s.undo.len());
    with_slot(s, |sl| {
        // a decision that changed nothing (people.sync with every photo already tagged) leaves no
        // step: `people.undo` must undo the decision before it, not an empty one
        let nothing_changed = added == 0 && sl.history.last().is_some_and(|e| e.state == sl.state);
        if let Some(e) = sl.history.last_mut() {
            e.committed = committed;
        }
        if r.is_err() || nothing_changed {
            sl.history.pop();
        }
    })?;
    r
}

// ------------------------------------------------------------------------------------ commands

pub fn status(s: &mut Session) -> Value {
    let opened = ensure_open(s);
    let arc = s.people.core.slot.clone();
    let (index, note) = match lock(&arc).as_ref() {
        Some(sl) => (
            json!({
                "photos": sl.store.photo_count(), "faces": sl.store.face_count(), "persons": sl.state.persons.len(),
                "named": sl.state.persons.iter().filter(|p| p.name.is_some()).count(), "unknownFaces": sl.state.unknown.len(),
                "skippedFaces": sl.skipped.len(), "grouped": sl.computed.is_some(),
                "groups": sl.computed.as_ref().map(|c| c.groups.len()), "suggestions": sl.computed.as_ref().map(|c| c.suggestions.len()),
                "model": sl.model.id, "file": sl.dir.as_ref().map(|d| d.display().to_string()),
                "settings": serde_json::to_value(&sl.state.settings).unwrap_or_default(),
            }),
            sl.note.clone(),
        ),
        None => (Value::Null, None),
    };
    let fast = lock(&arc).as_ref().is_some_and(|sl| sl.state.settings.fast_model);
    let m = model_of(fast);
    json!({
        "available": People::AVAILABLE,
        "model": {"id": m.id, "name": m.name, "installed": s.people.installed(m), "file": s.people.model_file(m).map(|f| f.display().to_string()), "sizeBytes": m.size,
                  "licence": m.licence, "licenceUrl": m.licence_url, "homepage": m.homepage, "loaded": s.people.core.loaded(),
                  "download": s.people.core.download_status()},
        "faceModel": {"installed": s.facemesh.installed(), "download": s.people.core.face_download_status()},
        "index": index,
        "job": s.people.core.job_status(),
        "computing": s.people.core.computing(),
        "error": opened.err(),
        "note": note,
    })
}

pub fn model_download(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.model.download";
    ensure_open(s).map_err(other)?;
    let fast = with_slot(s, |sl| sl.state.settings.fast_model)?;
    let m = model_of(fast);
    if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
        return Err(bad(
            C,
            format!(
                "{} is a {:.0} MB download from {}, published by {} under these terms: {} ({}): pass `acknowledged: true` once the user has agreed to download it. It is not part of LightCraft; everything else works without it.",
                m.name,
                m.size as f64 / 1e6,
                m.mirror,
                m.authors,
                m.licence,
                m.licence_url
            ),
        ));
    }
    let folder = s.people.model_folder().ok_or_else(|| other("there is no folder for the models on this system"))?.join(m.id);
    let mirrors = s.people.mirrors(m);
    let started = s.people.core.start_download(fast, mirrors, folder).map_err(other)?;
    // the face finder is a 3.7 MB file of its own
    if !s.facemesh.installed()
        && let Some(dir) = s.facemesh.model_dir()
    {
        let _ = s.people.core.start_face_download(dir, s.facemesh.mirrors());
    }
    Ok(json!({"started": started, "installed": s.people.installed(m), "downloading": s.people.core.download_status().running}))
}

fn scope(s: &Session, p: &Value) -> Vec<PhotoId> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() { s.targets(p) } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
    ids.into_iter().filter(|i| s.catalog.photo(*i).is_some_and(|q| q.kind != MediaKind::Video)).collect()
}

pub fn index(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    let (force, background) = (bool_or(p, "force", false), bool_or(p, "background", s.people.background));
    let fast = with_slot(s, |sl| sl.state.settings.fast_model)?;
    let m = model_of(fast);
    if !s.people.installed(m) {
        return Err(other(s.people.not_installed(m)));
    }
    if !s.facemesh.installed() {
        return Err(other(format!("{}: the face finder is also needed (facemesh.model.download)", crate::facemesh::NOT_INSTALLED)));
    }
    let model_file = s.people.model_file(m).ok_or_else(|| other("no folder for the models"))?;
    let face_file = s.facemesh.model_file().ok_or_else(|| other("no folder for the face model"))?;
    let edge = f64_or(p, "edge", s.people.edge.unwrap_or(super::core::DEFAULT_EDGE) as f64).clamp(320.0, 4096.0) as usize;
    let flip = bool_or(p, "flip", s.people.flip);
    let limit = f64_or(p, "limit", f64::MAX) as usize;
    let ids = scope(s, p);
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut already = 0;
    let have: HashSet<u64> = with_slot(s, |sl| sl.store.keys().collect())?;
    for id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        let key = key_of(&crate::media::content_key(&ph));
        if !seen.insert(key) {
            continue;
        }
        if have.contains(&key) && !force {
            already += 1;
            continue;
        }
        if items.len() >= limit {
            continue;
        }
        let source = s.media.source_ref(&ph, SourceLevel::Preview);
        items.push(Item { id, key, source });
    }
    let queued = items.len();
    if queued > 0 {
        s.people.core.start_job(items, m, model_file, face_file, edge, flip, background).map_err(other)?;
        s.people.was_running = true;
    }
    let st = s.people.core.job_status();
    if !background && queued > 0 {
        finish_run(s);
    }
    let photos = with_slot(s, |sl| (sl.store.photo_count(), sl.store.face_count()))?;
    Ok(json!({
        "queued": queued, "alreadyIndexed": already, "background": background && queued > 0,
        "done": if background { Value::Null } else { json!(st.done) }, "failed": st.failed, "faces": st.faces,
        "failures": st.failures.iter().take(20).map(|(i, e)| json!({"id": i, "error": e})).collect::<Vec<_>>(),
        "seconds": st.seconds, "phaseSeconds": st.phase_seconds,
        "secondsPerPhoto": if st.done > 0 && !background { json!(st.seconds / st.done as f64) } else { Value::Null },
        "error": st.error, "indexedPhotos": photos.0, "indexedFaces": photos.1,
    }))
}

/// A run ended: group the faces again (and tag what the auto-tag threshold allows).
fn finish_run(s: &mut Session) {
    s.people.was_running = false;
    let _ = s.people.core.start_compute(s.people.background);
}

/// Cheap; call it every frame. Returns whether the overview may have changed.
pub fn poll(s: &mut Session) -> bool {
    let running = s.people.core.job_running();
    let mut changed = false;
    if s.people.was_running && !running {
        finish_run(s);
        changed = true;
    }
    // faces are in the index but not grouped yet (the library was just opened): group them
    if !s.people.core.computing() && !running {
        let need =
            s.people.core.slot.try_lock().ok().is_some_and(|g| g.as_ref().is_some_and(|sl| sl.computed.is_none() && sl.store.face_count() > 0));
        if need {
            let _ = s.people.core.start_compute(true);
            changed = true;
        }
    }
    // auto-tag: suggestions above the user's threshold, once grouping is done
    if !s.people.core.computing() {
        let arc = s.people.core.slot.clone();
        let want = match arc.try_lock() {
            Ok(g) => g
                .as_ref()
                .and_then(|sl| sl.state.settings.auto_tag.zip(sl.computed.as_ref()))
                .is_some_and(|(t, c)| c.suggestions.iter().any(|sg| sg.score >= t)),
            Err(_) => false,
        };
        if want && auto_tag(s).is_ok() {
            changed = true;
        }
    }
    let rev = s
        .people
        .core
        .slot
        .try_lock()
        .ok()
        .and_then(|g| g.as_ref().map(|sl| sl.store_rev * 1_000_000 + sl.state_rev + u64::from(sl.computed.is_some())));
    if rev.is_some() && rev != s.people.seen_rev {
        s.people.seen_rev = rev;
        changed = true;
    }
    changed
}

fn auto_tag(s: &mut Session) -> Result<usize> {
    let picks: Vec<(FaceRef, u32)> = with_slot(s, |sl| {
        let t = sl.state.settings.auto_tag.unwrap_or(2.0);
        sl.computed
            .as_ref()
            .map(|c| c.suggestions.iter().filter(|sg| sg.score >= t).filter_map(|sg| sl.store.meta(sg.slot).map(|m| (m.face(), sg.person))).collect())
            .unwrap_or_default()
    })?;
    if picks.is_empty() {
        return Ok(0);
    }
    let n = picks.len();
    decision(s, &format!("Auto-tag {n} faces"), |s| {
        let mut persons: Vec<u32> = Vec::new();
        with_slot(s, |sl| {
            for (f, p) in &picks {
                sl.state.assign(*p, &[*f]);
                if !persons.contains(p) {
                    persons.push(*p);
                }
            }
            reconcile(sl);
            save(sl)
        })??;
        for p in persons {
            sync_person(s, p)?;
        }
        Ok(())
    })?;
    Ok(n)
}

pub fn refresh(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    let background = bool_or(p, "background", s.people.background);
    // "Group again" shows what was skipped for now
    with_slot(s, |sl| {
        if !sl.skipped.is_empty() {
            sl.skipped.clear();
            sl.state_rev += 1;
        }
    })?;
    let started = s.people.core.start_compute(background).map_err(other)?;
    let arc = s.people.core.slot.clone();
    let g = lock(&arc);
    let c = g.as_ref().and_then(|sl| sl.computed.as_ref());
    Ok(
        json!({"started": started, "background": background, "groups": c.map(|c| c.groups.len()), "suggestions": c.map(|c| c.suggestions.len()), "seconds": c.map(|c| c.seconds)}),
    )
}

fn ensure_grouped(s: &mut Session) -> Result<()> {
    let need = with_slot(s, |sl| sl.computed.is_none() && sl.store.face_count() > 0)?;
    if need && !s.people.core.computing() {
        s.people.core.start_compute(false).map_err(other)?;
    }
    Ok(())
}

pub fn list(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    if !s.people.background {
        ensure_grouped(s)?;
    }
    let groups = f64_or(p, "groups", 30.0).clamp(0.0, 1000.0) as usize;
    let suggestions = f64_or(p, "suggestions", 30.0).clamp(0.0, 1000.0) as usize;
    let thumbs = f64_or(p, "thumbs", 4.0).clamp(0.0, 20.0) as usize;
    let ov = overview(s, groups, suggestions, thumbs)?;
    let mut v = serde_json::to_value(&ov).map_err(|e| other(e.to_string()))?;
    if bool_or(p, "filter", false) {
        // nothing to filter here: people.photos does that
        v["note"] = json!("use people.photos {person|group, filter: true} to show a person's photos");
    }
    Ok(v)
}

pub fn photos(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.photos";
    ensure_open(s).map_err(other)?;
    let map = key_map(s);
    let keys: HashSet<u64> = with_slot(s, |sl| -> Result<HashSet<u64>> {
        let t = if p.get("person").is_some() { target_param(sl, p, "person", C)? } else { target_param(sl, p, "group", C)? };
        let mut keys: HashSet<u64> = faces_of(sl, &t).iter().map(|f| f.0).collect();
        if let Target::Person(id) = t {
            keys.extend(person_keys(&sl.state, id));
        }
        Ok(keys)
    })??;
    let ids = photos_of(&map, keys);
    if bool_or(p, "filter", false) {
        s.filter.only = ids.clone();
    }
    Ok(json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "count": ids.len()}))
}

pub fn name(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.name";
    ensure_open(s).map_err(other)?;
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?.replace('|', "-");
    if name.eq_ignore_ascii_case("unknown") {
        return Err(bad(C, "`Unknown` is the name of the keyword of unknown faces (People|Unknown): choose another name"));
    }
    let key = if p.get("group").is_some() { "group" } else { "person" };
    let (target, existing) =
        with_slot(s, |sl| -> Result<(Target, Option<u32>)> { Ok((target_param(sl, p, key, C)?, sl.state.by_name(&name).map(|x| x.id))) })??;
    let dry = bool_or(p, "dryRun", false);
    let map = key_map(s);
    let pid = decision(s, &format!("Name {name}"), |s| {
        if dry {
            return Err(other("dry run"));
        }
        let (pid, old_name, merged) = with_slot(s, |sl| {
            let mut old_name = None;
            let mut merged = None;
            let pid = match (&target, existing) {
                // the person is renamed to a name another person has: they become one
                (Target::Person(id), Some(e)) if *id != e => {
                    merged = Some((*id, sl.state.person(*id).and_then(|x| x.name.clone())));
                    e
                }
                (Target::Person(id), _) => {
                    old_name = sl.state.person(*id).and_then(|x| x.name.clone());
                    if let Some(x) = sl.state.person_mut(*id) {
                        x.name = Some(name.clone());
                    }
                    *id
                }
                (_, Some(e)) => e,
                (_, None) => sl.state.new_person(Some(name.clone())),
            };
            let mut faces = faces_of(sl, &target);
            // faces the user said are not this person stay out of a group named after them
            faces.retain(|f| !sl.state.is_rejected(*f, pid));
            if let Some((from, _)) = merged {
                sl.state.merge(from, pid);
            } else {
                sl.state.assign(pid, &faces);
            }
            reconcile(sl);
            save(sl)?;
            Ok::<_, EngineError>((pid, old_name, merged))
        })??;
        if let Some(old) = old_name.filter(|o| !o.eq_ignore_ascii_case(&name)) {
            // a renamed person: the keyword follows (merging with a keyword that exists)
            let _ = s.execute("keyword.rename", &json!({"from": keyword_of(&old), "to": keyword_of(&name)}));
        }
        if let Some((_, Some(old))) = merged
            && !old.eq_ignore_ascii_case(&name)
        {
            let _ = s.execute("keyword.rename", &json!({"from": keyword_of(&old), "to": keyword_of(&name)}));
        }
        sync_person(s, pid)?;
        Ok(pid)
    });
    let pid = match pid {
        Ok(p) => p,
        Err(EngineError::Other(m)) if m == "dry run" => {
            let (faces, keys) = with_slot(s, |sl| {
                let f = faces_of(sl, &target);
                (f.len(), f.iter().map(|x| x.0).collect::<HashSet<u64>>())
            })?;
            return Ok(
                json!({"dryRun": true, "name": name, "keyword": keyword_of(&name), "faces": faces, "photos": photos_of(&map, keys).len(), "existingPerson": existing}),
            );
        }
        Err(e) => return Err(e),
    };
    let (faces, keys) = with_slot(s, |sl| {
        let f = sl.state.person(pid).map(|x| x.faces.clone()).unwrap_or_default();
        (f.len(), f.iter().map(|x| x.0).collect::<HashSet<u64>>())
    })?;

    // the new person may make other faces look like them: group again
    let _ = s.people.core.start_compute(s.people.background);
    Ok(
        json!({"person": pid, "id": format!("p:{pid}"), "name": name, "keyword": keyword_of(&name), "faces": faces, "photos": photos_of(&map, keys).len()}),
    )
}

pub fn confirm(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.confirm";
    ensure_open(s).map_err(other)?;
    let answer = str_param(p, "answer").ok_or_else(|| bad(C, "missing `answer` (yes, no or unsure)"))?;
    if !matches!(answer, "yes" | "no" | "unsure") {
        return Err(bad(C, "`answer` is yes, no or unsure"));
    }
    // either a question from people.questions, or `face` and `person`/`group`
    let (a, b, qid) = match str_param(p, "question") {
        Some(q) => {
            let (a, b) = q.split_once('>').ok_or_else(|| bad(C, format!("`{q}` is not a question id")))?;
            (a.to_string(), b.to_string(), Some(q.to_string()))
        }
        None => {
            let a = str_param(p, "face").or_else(|| str_param(p, "a")).ok_or_else(|| bad(C, "give a `question`, or `face` and `person`"))?;
            let b = str_param(p, "person")
                .or_else(|| str_param(p, "group"))
                .or_else(|| str_param(p, "b"))
                .ok_or_else(|| bad(C, "missing `person` (p:N) or `group`"))?;
            (a.to_string(), b.to_string(), None)
        }
    };
    let (ta, tb) =
        with_slot(s, |sl| -> Result<(Target, Target)> { Ok((resolve(sl, &a).map_err(|e| bad(C, e))?, resolve(sl, &b).map_err(|e| bad(C, e))?)) })??;
    if answer == "unsure" {
        let id = qid.unwrap_or_else(|| format!("{a}>{b}"));
        with_slot(s, |sl| {
            if !sl.state.snoozed.contains(&id) {
                sl.state.snoozed.push(id.clone());
            }
            save(sl)
        })??;
        return Ok(json!({"answer": "unsure", "snoozed": id}));
    }
    let label = format!("Same person? {answer}");
    let yes = answer == "yes";
    let out = decision(s, &label, |s| {
        let touched: Vec<u32> = with_slot(s, |sl| -> Result<Vec<u32>> {
            let fa = faces_of(sl, &ta);
            let fb = faces_of(sl, &tb);
            let mut touched = Vec::new();
            match (&ta, &tb, yes) {
                // a face (or group) and a person
                (Target::Face(_) | Target::Group(_), Target::Person(pid), true) => {
                    sl.state.assign(*pid, &fa);
                    touched.push(*pid);
                }
                (Target::Face(_) | Target::Group(_), Target::Person(pid), false) => {
                    for f in &fa {
                        sl.state.reject(*f, *pid);
                    }
                    touched.push(*pid);
                }
                (Target::Person(x), Target::Person(y), true) => {
                    sl.state.merge(*x, *y);
                    touched.push(*y);
                }
                (Target::Person(x), Target::Person(y), false) => {
                    for f in sl.state.person(*x).map(|p| p.faces.clone()).unwrap_or_default().into_iter().take(1) {
                        sl.state.reject(f, *y);
                    }
                    for f in sl.state.person(*y).map(|p| p.faces.clone()).unwrap_or_default().into_iter().take(1) {
                        sl.state.reject(f, *x);
                    }
                }
                // two groups, a face and a group, two faces: one person, or two that stay apart
                (_, _, true) => {
                    let pid = sl.state.new_person(None);
                    let all: Vec<FaceRef> = fa.iter().chain(&fb).copied().collect();
                    sl.state.assign(pid, &all);
                    touched.push(pid);
                }
                (_, _, false) => {
                    let best = |f: &[FaceRef]| {
                        f.iter().copied().max_by(|x, y| {
                            let q = |r: &FaceRef| sl.store.slot(*r).and_then(|s| sl.store.meta(s)).map_or(0.0, |m| m.quality());
                            q(x).total_cmp(&q(y))
                        })
                    };
                    if let (Some(x), Some(y)) = (best(&fa), best(&fb)) {
                        sl.state.different.push((x, y));
                    }
                }
            }
            reconcile(sl);
            save(sl)?;
            Ok(touched)
        })??;
        for pid in &touched {
            sync_person(s, *pid)?;
        }
        Ok(touched)
    })?;
    // a "no" between groups changes how they would be grouped: group again
    if !yes {
        let _ = s.people.core.start_compute(s.people.background);
    }
    Ok(json!({"answer": answer, "persons": out}))
}

pub fn merge(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.merge";
    ensure_open(s).map_err(other)?;
    let (from, into) = with_slot(s, |sl| -> Result<(Target, Target)> { Ok((target_param(sl, p, "from", C)?, target_param(sl, p, "into", C)?)) })??;
    if from == into {
        return Err(bad(C, "`from` and `into` are the same"));
    }
    let pid = decision(s, "Merge people", |s| {
        let (pid, old) = with_slot(s, |sl| -> Result<(u32, Option<String>)> {
            let faces = faces_of(sl, &from);
            let old = match &from {
                Target::Person(f) => sl.state.person(*f).and_then(|x| x.name.clone()),
                _ => None,
            };
            let pid = match &into {
                Target::Person(i) => *i,
                _ => {
                    // into a group: the merged faces become a new person
                    let pid = sl.state.new_person(None);
                    sl.state.assign(pid, &faces_of(sl, &into));
                    pid
                }
            };
            if let Target::Person(f) = &from {
                sl.state.merge(*f, pid);
            } else {
                sl.state.assign(pid, &faces);
            }
            reconcile(sl);
            save(sl)?;
            Ok((pid, old))
        })??;
        // the folded person's keyword goes; the target's covers its photos now
        if let Some(old) = old {
            let target_name = with_slot(s, |sl| sl.state.person(pid).and_then(|x| x.name.clone()))?;
            if let Some(tn) = target_name {
                let _ = s.execute("keyword.rename", &json!({"from": keyword_of(&old), "to": keyword_of(&tn)}));
            } else {
                let _ = s.execute("keyword.delete", &json!({"keyword": keyword_of(&old)}));
            }
        }
        sync_person(s, pid)?;
        Ok(pid)
    })?;
    Ok(json!({"person": pid, "id": format!("p:{pid}")}))
}

pub fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    // naming a person that exists is people.name on `person`
    name(s, p)
}

/// The named person whose keyword `kw` is (`People|<name>`), if any. A photo dropped on that
/// keyword is filed under the person (see [`tag_photos`]).
pub fn person_of_keyword(s: &mut Session, kw: &str) -> Option<u32> {
    let rest = kw.strip_prefix(&format!("{KEYWORD_ROOT}|"))?;
    if rest.is_empty() || rest.contains('|') {
        return None;
    }
    with_slot(s, |sl| sl.state.by_name(rest).map(|x| x.id)).ok().flatten()
}

/// The faces a command names: `face` / `group` / `person` (one target each) and `faces` (a list),
/// as face references, without doubles.
fn faces_param(s: &mut Session, p: &Value, cmd: &str) -> Result<Vec<FaceRef>> {
    let mut ids: Vec<String> = ["face", "group", "person"].iter().filter_map(|k| str_param(p, k)).map(String::from).collect();
    if let Some(a) = p.get("faces").and_then(Value::as_array) {
        ids.extend(a.iter().filter_map(Value::as_str).map(String::from));
    }
    if ids.is_empty() {
        return Err(bad(cmd, "name the faces: `face`, `group` (g:N), `person` (p:N) or `faces` [KEY:N, …] (from people.list)"));
    }
    with_slot(s, |sl| -> Result<Vec<FaceRef>> {
        let mut out: Vec<FaceRef> = Vec::new();
        for t in &ids {
            let target = resolve(sl, t).map_err(|e| bad(cmd, e))?;
            for f in faces_of(sl, &target) {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        Ok(out)
    })?
}

/// The people (ids) who have one of `faces` among their confirmed faces.
fn owners(sl: &Slot, faces: &[FaceRef]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for f in faces {
        if let Some(pe) = sl.state.person_of(*f)
            && !out.contains(&pe.id)
        {
            out.push(pe.id);
        }
    }
    out
}

/// Unnamed people left without a face or a photo (after faces were moved away) are forgotten.
fn drop_empty_unnamed(sl: &mut Slot) {
    let tags: HashSet<u32> = sl.state.photo_tags.iter().map(|(_, p)| *p).collect();
    sl.state.persons.retain(|p| p.name.is_some() || !p.faces.is_empty() || tags.contains(&p.id));
}

/// "Not a real face": these faces are left out for good (a statue, a poster, a false detection).
/// The keywords of other people in the same photos are never touched; the keyword of the person
/// a face was taken from stays on a photo as long as another of their faces, or a photo filed
/// under them, is in it.
pub fn ignore(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.ignore";
    ensure_open(s).map_err(other)?;
    let faces = faces_param(s, p, C)?;
    let n = decision(s, "Not a real face", |s| {
        let touched = with_slot(s, |sl| -> Result<Vec<u32>> {
            let touched = owners(sl, &faces);
            sl.state.mark_ignored(&faces);
            drop_empty_unnamed(sl);
            reconcile(sl);
            save(sl)?;
            Ok(touched)
        })??;
        for pid in touched {
            sync_person(s, pid)?;
        }
        Ok(faces.len())
    })?;
    Ok(json!({"ignored": n}))
}

/// "Unknown person": the user looked at these faces and does not know them. They leave the groups
/// to name for a section of their own (`unknown` in people.list), where they can be named later.
/// No keyword, unless `people.settings {tagUnknown: true}` (then `People|Unknown`). `off: true`
/// puts them back among the groups to name.
pub fn unknown(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.unknown";
    ensure_open(s).map_err(other)?;
    let off = bool_or(p, "off", false);
    let faces = faces_param(s, p, C)?;
    let n = decision(s, if off { "Not unknown any more" } else { "Unknown person" }, |s| {
        let touched = with_slot(s, |sl| -> Result<Vec<u32>> {
            let touched = owners(sl, &faces);
            if off {
                sl.state.unknown.retain(|f| !faces.contains(f));
            } else {
                sl.state.mark_unknown(&faces);
                drop_empty_unnamed(sl);
            }
            reconcile(sl);
            save(sl)?;
            Ok(touched)
        })??;
        for pid in touched {
            sync_person(s, pid)?;
        }
        Ok(faces.len())
    })?;
    // they now group together (or with the groups they came back to)
    let _ = s.people.core.start_compute(s.people.background);
    Ok(json!({"unknown": n, "off": off}))
}

/// "Skip for now": hide these groups from the list to name until "Group again" (`people.refresh`)
/// or `clear`. Decides nothing, writes nothing: not an undo step.
pub fn skip(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.skip";
    ensure_open(s).map_err(other)?;
    let clear = bool_or(p, "clear", false);
    let faces = if clear && !["face", "group", "person", "faces"].iter().any(|k| p.get(*k).is_some()) { Vec::new() } else { faces_param(s, p, C)? };
    with_slot(s, |sl| {
        if clear {
            if faces.is_empty() {
                sl.skipped.clear();
            }
            for f in &faces {
                sl.skipped.remove(f);
            }
        } else {
            sl.skipped.extend(faces.iter().copied());
        }
        sl.state_rev += 1;
        json!({"skippedFaces": sl.skipped.len()})
    })
}

/// "This face is actually that person": reassign faces to a person (`to: "p:N"`), to the person a
/// group of faces would make (`"g:N"`), to "unknown person" (`"unknown"`) or to "not a real face"
/// (`"ignored"`). The person a face leaves is told "not them" (never suggested for them again).
/// Keywords follow at the level of the photo: the new person's keyword is added, and the old one's
/// is removed only from photos where none of their other faces, and no photo tag, remain: a
/// photo with several people keeps every other keyword. One undo step.
pub fn move_faces(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.move";
    ensure_open(s).map_err(other)?;
    let to = str_param(p, "to").ok_or_else(|| bad(C, "missing `to`: p:N, g:N, unknown or ignored"))?.to_string();
    match to.as_str() {
        "unknown" => return unknown(s, p),
        "ignored" | "notAFace" => return ignore(s, p),
        _ => {}
    }
    let faces: Vec<FaceRef> = {
        let mut q = p.clone();
        if let Some(o) = q.as_object_mut() {
            // the destination is not a source
            o.remove("person");
            o.remove("group");
        }
        faces_param(s, &q, C)?
    };
    let target = with_slot(s, |sl| resolve(sl, &to).map_err(|e| bad(C, e)))??;
    if let Target::Face(_) = target {
        return Err(bad(C, "`to` is a person (p:N), a group (g:N), unknown or ignored"));
    }
    let moving: Vec<FaceRef> = match &target {
        Target::Person(pid) => with_slot(s, |sl| faces.iter().copied().filter(|f| sl.state.person_of(*f).map(|x| x.id) != Some(*pid)).collect())?,
        // faces dropped on the group they are already in don't move
        Target::Group(_) => with_slot(s, |sl| {
            let own = faces_of(sl, &target);
            faces.iter().copied().filter(|f| !own.contains(f)).collect()
        })?,
        Target::Face(_) => faces.clone(),
    };
    if moving.is_empty() {
        return Ok(json!({"moved": 0, "person": to}));
    }
    let (pid, touched, added, removed) = decision(s, "Move faces", |s| {
        let (pid, touched) = with_slot(s, |sl| -> Result<(u32, Vec<u32>)> {
            let (pid, mut all) = match &target {
                Target::Person(id) => (*id, Vec::new()),
                Target::Group(_) => {
                    let g = faces_of(sl, &target);
                    (sl.state.new_person(None), g)
                }
                Target::Face(_) => return Err(bad(C, "not a destination")),
            };
            let mut touched = vec![pid];
            for f in &moving {
                if let Some(old) = sl.state.person_of(*f).map(|x| x.id).filter(|o| *o != pid) {
                    sl.state.reject(*f, old);
                    if !touched.contains(&old) {
                        touched.push(old);
                    }
                }
            }
            all.extend(moving.iter().copied());
            sl.state.assign(pid, &all);
            drop_empty_unnamed(sl);
            reconcile(sl);
            save(sl)?;
            Ok((pid, touched))
        })??;
        let (mut added, mut removed) = (0, 0);
        for t in &touched {
            let (a, r) = sync_person(s, *t)?;
            added += a;
            removed += r;
        }
        Ok((pid, touched, added, removed))
    })?;
    // the faces moved may change what looks like whom
    let _ = s.people.core.start_compute(s.people.background);
    let keyword = with_slot(s, |sl| sl.state.person(pid).and_then(|x| x.name.as_deref().map(keyword_of)))?;
    Ok(
        json!({"moved": moving.len(), "person": format!("p:{pid}"), "keyword": keyword, "keywordsAdded": added, "keywordsRemoved": removed, "touched": touched.iter().map(|t| format!("p:{t}")).collect::<Vec<_>>()}),
    )
}

fn selected_key_map(s: &Session, p: &Value) -> HashMap<u64, Vec<PhotoId>> {
    let mut m: HashMap<u64, Vec<PhotoId>> = HashMap::new();
    for id in s.targets(p) {
        if let Some(ph) = s.catalog.photo(id) {
            m.entry(key_of(&crate::media::content_key(ph))).or_default().push(id);
        }
    }
    m
}

fn named_person(s: &mut Session, p: &Value, cmd: &str) -> Result<(u32, String)> {
    let pid = with_slot(s, |sl| match target_param(sl, p, "person", cmd)? {
        Target::Person(id) => Ok(id),
        _ => Err(bad(cmd, "`person` must be p:N")),
    })??;
    let name = with_slot(s, |sl| sl.state.person(pid).and_then(|x| x.name.clone()))?
        .ok_or_else(|| bad(cmd, "this person has no name yet: name the group first (people.name)"))?;
    Ok((pid, name))
}

/// File whole photos under a person (a photo dropped on them): the photos get `People|<name>`
/// whether or not a face of theirs was found in them, and the photo is remembered as filed by hand
/// (`photo_tags` in people.json), so syncing the person, moving one of its faces to someone else
/// or regrouping never removes it. Other people's keywords on the photo are not touched; a photo
/// that already has the keyword is not changed twice. One undo step.
pub fn tag_photos(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.tagPhotos";
    ensure_open(s).map_err(other)?;
    let (pid, name) = named_person(s, p, C)?;
    let sel = selected_key_map(s, p);
    if sel.is_empty() {
        return Err(bad(C, "no photos: give `ids` or select some"));
    }
    let kw = keyword_of(&name);
    let keys: HashSet<u64> = sel.keys().copied().collect();
    let total: usize = sel.values().map(Vec::len).sum();
    let added = decision(s, &format!("Tag photos {name}"), |s| {
        with_slot(s, |sl| {
            for k in &keys {
                if !sl.state.has_photo_tag(*k, pid) {
                    sl.state.photo_tags.push((*k, pid));
                }
            }
            save(sl)
        })??;
        tag(s, &sel, &keys, &kw, true)
    })?;
    Ok(json!({"person": format!("p:{pid}"), "keyword": kw, "photos": total, "added": added, "alreadyTagged": total - added}))
}

/// Undo of [`tag_photos`] by hand: the photos are no longer filed under the person as a whole; the
/// keyword goes from those where none of the person's faces is.
pub fn untag_photos(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.untagPhotos";
    ensure_open(s).map_err(other)?;
    let (pid, name) = named_person(s, p, C)?;
    let sel = selected_key_map(s, p);
    let kw = keyword_of(&name);
    let keys: HashSet<u64> = sel.keys().copied().collect();
    let removed = decision(s, &format!("Untag photos {name}"), |s| {
        let faced: HashSet<u64> = with_slot(s, |sl| {
            sl.state.photo_tags.retain(|(k, x)| !(*x == pid && keys.contains(k)));
            save(sl).map(|_| sl.state.person(pid).map(|x| x.faces.iter().map(|f| f.0).collect()).unwrap_or_default())
        })??;
        let gone: HashSet<u64> = keys.difference(&faced).copied().collect();
        tag(s, &sel, &gone, &kw, false)
    })?;
    Ok(json!({"person": format!("p:{pid}"), "keyword": kw, "removed": removed}))
}

/// The faces of a person or a group, best first, each with the other faces of its photo and who
/// they are (so a viewer can tell which face of a group photo it drags).
pub fn members(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.members";
    ensure_open(s).map_err(other)?;
    let (limit, offset) = (f64_or(p, "limit", 60.0).clamp(1.0, 2000.0) as usize, f64_or(p, "offset", 0.0).max(0.0) as usize);
    let map = key_map(s);
    let first: HashMap<u64, PhotoId> = map.iter().filter_map(|(k, v)| v.first().map(|p| (*k, *p))).collect();
    with_slot(s, |sl| -> Result<Value> {
        let key = if p.get("person").is_some() { "person" } else { "group" };
        let t = target_param(sl, p, key, C)?;
        let mut faces: Vec<(FaceRef, usize, f32)> = faces_of(sl, &t)
            .into_iter()
            .filter_map(|f| sl.store.slot(f).map(|slot| (f, slot, sl.store.meta(slot).map_or(0.0, |m| m.quality()))))
            .collect();
        faces.sort_by(|a, b| b.2.total_cmp(&a.2));
        let total = faces.len();
        let rows: Vec<Value> = faces
            .into_iter()
            .skip(offset)
            .take(limit)
            .filter_map(|(f, slot, _)| {
                let m = sl.store.meta(slot)?;
                let photo = first.get(&f.0)?;
                let others: Vec<Value> = sl
                    .store
                    .slots_of(f.0)
                    .iter()
                    .filter(|x| **x != slot)
                    .filter_map(|x| sl.store.meta(*x))
                    .map(|o| {
                        let who = sl.state.person_of(o.face());
                        json!({"face": face_id(o.face()), "rect": o.rect, "person": who.map(|x| format!("p:{}", x.id)), "name": who.and_then(|x| x.name.clone()),
                               "unknown": sl.state.is_unknown(o.face()), "ignored": sl.state.ignored.contains(&o.face())})
                    })
                    .collect();
                Some(json!({"face": face_id(f), "photo": photo.0, "rect": m.rect, "others": others}))
            })
            .collect();
        Ok(json!({"faces": rows, "total": total}))
    })?
}

pub fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.delete";
    ensure_open(s).map_err(other)?;
    let pid = with_slot(s, |sl| match target_param(sl, p, "person", C)? {
        Target::Person(id) => Ok(id),
        _ => Err(bad(C, "`person` must be p:N")),
    })??;
    let n = decision(s, "Delete person", |s| {
        let map = key_map(s);
        let (name, keys) = with_slot(s, |sl| (sl.state.person(pid).and_then(|x| x.name.clone()), person_keys(&sl.state, pid)))?;
        with_slot(s, |sl| {
            sl.state.persons.retain(|x| x.id != pid);
            sl.state.rejected.retain(|(_, x)| *x != pid);
            sl.state.photo_tags.retain(|(_, x)| *x != pid);
            save(sl)
        })??;
        if let Some(n) = name {
            tag(s, &map, &keys, &keyword_of(&n), false)?;
        }
        Ok(keys.len())
    })?;
    // the faces are free again: group them
    let _ = s.people.core.start_compute(s.people.background);
    Ok(json!({"deleted": pid, "photos": n}))
}

pub fn suggest(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    if !s.people.background {
        ensure_grouped(s)?;
    }
    let limit = f64_or(p, "limit", 50.0).clamp(1.0, 5000.0) as usize;
    let min = p.get("minScore").and_then(Value::as_f64).map(|v| v as f32);
    let only: Option<HashSet<PhotoId>> = p.get("ids").map(|_| scope(s, p).into_iter().collect());
    let map = key_map(s);
    let first: HashMap<u64, PhotoId> = map.iter().filter_map(|(k, v)| v.first().map(|p| (*k, *p))).collect();
    let keep_key: Option<HashSet<u64>> = only.map(|set| map.iter().filter(|(_, v)| v.iter().any(|i| set.contains(i))).map(|(k, _)| *k).collect());
    let ov = overview(s, 0, usize::MAX / 2, 0)?;
    let rows: Vec<Value> = ov
        .suggestions
        .iter()
        .filter(|sg| min.is_none_or(|m| sg.score >= m))
        .filter(|sg| parse_face(&sg.face).is_some_and(|f| keep_key.as_ref().is_none_or(|k| k.contains(&f.0))))
        .take(limit)
        .map(|sg| {
            let ids: Vec<u64> = parse_face(&sg.face).and_then(|f| map.get(&f.0)).map(|v| v.iter().map(|i| i.0).collect()).unwrap_or_default();
            json!({"face": sg.face, "photo": sg.photo, "photos": ids, "rect": sg.rect, "person": format!("p:{}", sg.person), "name": sg.name,
                   "score": (sg.score * 1000.0).round() / 1000.0, "runnerUp": sg.runner_up, "ask": format!("{}>p:{}", sg.face, sg.person)})
        })
        .collect();
    let _ = first;
    Ok(json!({"suggestions": rows, "total": ov.suggestions_total, "grouped": ov.grouped, "computing": ov.computing}))
}

/// Questions worth asking, nearest to the thresholds first: a face that may be a known person, a
/// group that may be a known person, two groups that may be one person.
pub fn questions(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    if !s.people.background {
        ensure_grouped(s)?;
    }
    let limit = f64_or(p, "limit", 10.0).clamp(1.0, 200.0) as usize;
    let map = key_map(s);
    let first: HashMap<u64, PhotoId> = map.iter().filter_map(|(k, v)| v.first().map(|p| (*k, *p))).collect();
    let arc = s.people.core.slot.clone();
    let g = lock(&arc);
    let sl = g.as_ref().ok_or_else(|| other("the people index is not open"))?;
    let st = &sl.state.settings;
    let Some(c) = &sl.computed else { return Ok(json!({"questions": [], "grouped": false})) };
    let known = cluster::known_people(&sl.store, &sl.state);
    let snoozed: HashSet<&str> = sl.state.snoozed.iter().map(String::as_str).collect();
    let name_of = |id: u32| sl.state.person(id).and_then(|x| x.name.clone());
    let thumb =
        |slot: usize| sl.store.meta(slot).and_then(|m| first.get(&m.key).map(|ph| json!({"photo": ph.0, "face": face_id(m.face()), "rect": m.rect})));
    let mut qs: Vec<(f32, Value)> = Vec::new();
    // 1. faces that may be a known person
    for sg in c.suggestions.iter().filter(|sg| sg.score < st.suggest + 0.12) {
        let Some(m) = sl.store.meta(sg.slot) else { continue };
        let id = format!("{}>p:{}", face_id(m.face()), sg.person);
        if snoozed.contains(id.as_str()) {
            continue;
        }
        qs.push((
            (sg.score - st.suggest).abs(),
            json!({"id": id, "kind": "face", "face": thumb(sg.slot), "person": format!("p:{}", sg.person), "name": name_of(sg.person), "score": sg.score,
                   "personFaces": known.iter().find(|k| k.id == sg.person).map(|k| k.slots.iter().take(4).filter_map(|x| thumb(*x)).collect::<Vec<_>>())}),
        ));
    }
    // 2. groups that may be a known person, or one another
    let big: Vec<&Group> = c.groups.iter().filter(|g| g.members.len() >= 2 && listed(sl, &g.members) == Listed::Name).take(400).collect();
    for g in &big {
        for k in &known {
            let sc = cluster::dot(&g.centroid, &k.centroid);
            let id = format!("g:{}>p:{}", g.members[0], k.id);
            if sc >= st.suggest - 0.1 && sc < st.suggest + 0.12 && !snoozed.contains(id.as_str()) {
                qs.push((
                    (sc - st.suggest).abs(),
                    json!({"id": id, "kind": "group-person", "group": g.members.iter().take(4).filter_map(|x| thumb(*x)).collect::<Vec<_>>(), "groupId": format!("g:{}", g.members[0]),
                           "faces": g.members.len(), "person": format!("p:{}", k.id), "name": name_of(k.id), "score": sc,
                           "personFaces": k.slots.iter().take(4).filter_map(|x| thumb(*x)).collect::<Vec<_>>()}),
                ));
            }
        }
    }
    for (i, a) in big.iter().enumerate() {
        for b in big.iter().skip(i + 1) {
            let sc = cluster::dot(&a.centroid, &b.centroid);
            let id = format!("g:{}>g:{}", a.members[0], b.members[0]);
            if sc >= st.link - 0.12 && sc < st.link && !snoozed.contains(id.as_str()) {
                qs.push((
                    (sc - st.link).abs(),
                    json!({"id": id, "kind": "groups", "a": a.members.iter().take(4).filter_map(|x| thumb(*x)).collect::<Vec<_>>(), "b": b.members.iter().take(4).filter_map(|x| thumb(*x)).collect::<Vec<_>>(),
                           "facesA": a.members.len(), "facesB": b.members.len(), "score": sc}),
                ));
            }
        }
    }
    qs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = qs.len();
    Ok(json!({"questions": qs.into_iter().take(limit).map(|q| q.1).collect::<Vec<_>>(), "total": total}))
}

pub fn undo(s: &mut Session) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    let entry = with_slot(s, |sl| sl.history.pop())?.ok_or_else(|| other("nothing to undo in People"))?;
    let mut keywords_undone = false;
    if let Some(len) = entry.committed {
        if s.undo.len() == len {
            s.undo_step()?;
            keywords_undone = true;
        } else {
            with_slot(s, |sl| sl.history.push(Entry { label: entry.label.clone(), state: entry.state.clone(), committed: entry.committed }))?;
            return Err(other("other edits were made since that decision: undo them first (Edit > Undo), then People"));
        }
    }
    with_slot(s, |sl| {
        sl.state = entry.state;
        reconcile(sl);
        // the groups they came from are not known any more
        sl.computed = None;
        save(sl)
    })??;
    let _ = s.people.core.start_compute(s.people.background);
    Ok(json!({"undone": entry.label, "keywordsUndone": keywords_undone}))
}

pub fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    let changed_model = with_slot(s, |sl| -> Result<bool> {
        let st = &mut sl.state.settings;
        let before = st.fast_model;
        if let Some(v) = p.get("link").and_then(Value::as_f64) {
            st.link = (v as f32).clamp(0.2, 0.9);
        }
        if let Some(v) = p.get("suggest").and_then(Value::as_f64) {
            st.suggest = (v as f32).clamp(0.2, 0.9);
        }
        if p.get("autoTag").is_some() {
            st.auto_tag = p.get("autoTag").and_then(Value::as_f64).map(|v| (v as f32).clamp(0.4, 1.0));
        }
        if let Some(v) = p.get("minFacePx").and_then(Value::as_f64) {
            st.min_face_px = (v as f32).clamp(16.0, 400.0);
        }
        match str_param(p, "model") {
            Some("fast") => st.fast_model = true,
            Some("accurate") => st.fast_model = false,
            Some(m) => return Err(bad("people.settings", format!("model `{m}` is fast or accurate"))),
            None => {}
        }
        let changed = st.fast_model != before;
        save(sl)?;
        Ok(changed)
    })??;
    if let Some(on) = p.get("tagUnknown").and_then(Value::as_bool)
        && with_slot(s, |sl| sl.state.settings.tag_unknown != on)?
    {
        // switching it on tags the photos of the unknown faces, off takes the keyword away: one undo step
        decision(s, "Tag unknown faces", |s| {
            with_slot(s, |sl| {
                sl.state.settings.tag_unknown = on;
                save(sl)
            })??;
            if !on {
                let map = key_map(s);
                let all: HashSet<u64> = map.keys().copied().collect();
                tag(s, &map, &all, UNKNOWN_KEYWORD, false)?;
            }
            Ok(())
        })?;
    }
    if changed_model {
        // the other model has its own index: reopen
        let arc = s.people.core.slot.clone();
        *lock(&arc) = None;
        ensure_open(s).map_err(other)?;
    } else if p.get("link").is_some() || p.get("minFacePx").is_some() || p.get("suggest").is_some() {
        let _ = s.people.core.start_compute(s.people.background);
    }
    with_slot(s, |sl| json!({"settings": sl.state.settings, "model": sl.model.id}))
}

/// Tag every named person's photos again (after a library import, or an undo of keywords).
pub fn sync(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    let ids: Vec<u32> = with_slot(s, |sl| sl.state.persons.iter().filter(|x| x.name.is_some()).map(|x| x.id).collect())?;
    if bool_or(p, "dryRun", false) {
        return Ok(json!({"persons": ids.len(), "dryRun": true}));
    }
    let (mut added, mut removed) = (0, 0);
    decision(s, "Tag people", |s| {
        for id in ids {
            let (a, r) = sync_person(s, id)?;
            added += a;
            removed += r;
        }
        Ok(())
    })?;
    Ok(json!({"added": added, "removed": removed}))
}

/// The faces found in a photo and who they are (for a viewer that draws boxes).
pub fn faces(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_open(s).map_err(other)?;
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad("people.faces", "no photo"))?;
    let key = s.catalog.photo(id).map(|ph| key_of(&crate::media::content_key(ph))).ok_or_else(|| bad("people.faces", "no such photo"))?;
    with_slot(s, |sl| {
        let analysed = sl.store.has_photo(key);
        let faces: Vec<Value> = sl
            .store
            .slots_of(key)
            .iter()
            .filter_map(|x| sl.store.meta(*x))
            .map(|m| {
                let person = sl.state.person_of(m.face());
                json!({"face": face_id(m.face()), "rect": m.rect, "size": m.size, "quality": m.quality(), "person": person.map(|x| format!("p:{}", x.id)), "name": person.and_then(|x| x.name.clone()),
                       "ignored": sl.state.ignored.contains(&m.face()), "unknown": sl.state.is_unknown(m.face())})
            })
            .collect();
        // people the photo is filed under as a whole (dropped on them), whatever its faces say
        let filed: Vec<Value> = sl
            .state
            .photo_tags
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, pid)| json!({"person": format!("p:{pid}"), "name": sl.state.person(*pid).and_then(|x| x.name.clone())}))
            .collect();
        json!({"id": id.0, "analysed": analysed, "faces": faces, "filedUnder": filed})
    })
}

/// Add faces to the index without the models (for tests and tools): each photo with its faces
/// (embedding, box as fractions of the photo).
pub fn inject(s: &mut Session, photos: Vec<(PhotoId, Vec<(Vec<f32>, [f32; 4])>)>) {
    use lightcraft_people::store::FaceMeta;
    if ensure_open(s).is_err() {
        return;
    }
    let records: Vec<(u64, Vec<(FaceMeta, Vec<f32>)>)> = photos
        .into_iter()
        .map(|(id, faces)| {
            let key = key_of(&crate::media::content_key(s.catalog.photo(id).unwrap()));
            (key, faces.into_iter().map(|(v, rect)| (FaceMeta { key, idx: 0, rect, det: 0.95, size: 120.0, turn: 0.0 }, v)).collect())
        })
        .collect();
    with_slot(s, |sl| {
        sl.store.add(records).unwrap();
        sl.store_rev += 1;
    })
    .unwrap();
}
