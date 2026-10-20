//! Keep the three library vocabularies coherent without pretending every keyword is a person or
//! a place. `People|…` and `Places|…` are structured keyword branches; ordinary descriptive
//! keywords remain ordinary keywords.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use lightcraft_catalog::{Op, PhotoId, keywords, places};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    id: u64,
    file_name: String,
    from: String,
    to: String,
    kind: &'static str,
    evidence: &'static str,
    #[serde(skip)]
    person: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Ambiguous {
    id: u64,
    file_name: String,
    keyword: String,
    choices: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Audit {
    revision: u64,
    candidates: Vec<Candidate>,
    ambiguous: Vec<Ambiguous>,
    generic_keywords: Vec<String>,
    photos_scanned: usize,
    safe_changes: usize,
    people_changes: usize,
    place_changes: usize,
}

fn folded(s: &str) -> String {
    s.trim().to_lowercase()
}

fn place_path(parts: impl IntoIterator<Item = String>) -> Option<String> {
    let mut out = vec![places::ROOT.to_string()];
    for part in parts {
        let clean = keywords::clean(&part);
        if !clean.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&clean)) {
            out.push(clean);
        }
    }
    (out.len() > 1).then(|| out.join("|"))
}

fn named_people(s: &mut Session) -> HashMap<String, (u32, String)> {
    #[cfg(feature = "people")]
    {
        return crate::people::ops::overview(s, 0, 0, 0)
            .ok()
            .into_iter()
            .flat_map(|o| o.persons)
            .filter_map(|p| p.person.zip(p.name))
            .map(|(id, name)| (folded(&name), (id, name)))
            .collect();
    }
    #[cfg(not(feature = "people"))]
    {
        let _ = s;
        HashMap::new()
    }
}

fn audit(s: &mut Session) -> Audit {
    let people = named_people(s);
    let photos: Vec<_> = s.catalog.photos().filter(|p| !p.deleted && !p.local).cloned().collect();
    let mut known_places: HashMap<String, BTreeSet<String>> = HashMap::new();
    for photo in &photos {
        for path in photo.meta.keywords.iter().filter(|k| places::is_place_keyword(k)).cloned().chain(places::gps_place(photo)) {
            if let Some(leaf) = path.split('|').next_back().map(folded).filter(|x| !x.is_empty()) {
                known_places.entry(leaf).or_default().insert(path);
            }
        }
    }

    let mut candidates = Vec::new();
    let mut ambiguous = Vec::new();
    let mut generic = BTreeMap::<String, String>::new();
    for photo in &photos {
        let meta_path = place_path([photo.meta.country.clone(), photo.meta.state.clone(), photo.meta.city.clone(), photo.meta.location.clone()]);
        let meta_parts: HashSet<String> = [&photo.meta.country, &photo.meta.state, &photo.meta.city, &photo.meta.location]
            .into_iter()
            .map(|x| folded(x))
            .filter(|x| !x.is_empty())
            .collect();
        for kw in &photo.meta.keywords {
            if kw.contains('|') || kw.trim().is_empty() {
                continue;
            }
            let key = folded(kw);
            let person = people.get(&key).cloned();
            let place = if meta_parts.contains(&key) {
                meta_path.clone().map(|p| (p, "photo metadata"))
            } else {
                known_places.get(&key).filter(|p| p.len() == 1).and_then(|p| p.first().cloned()).map(|p| (p, "unique place path"))
            };
            match (person, place) {
                (Some((pid, name)), None) => candidates.push(Candidate {
                    id: photo.id.0,
                    file_name: photo.file_name.clone(),
                    from: kw.clone(),
                    to: format!("People|{name}"),
                    kind: "person",
                    evidence: "known person",
                    person: Some(pid),
                }),
                (None, Some((to, evidence))) => candidates.push(Candidate {
                    id: photo.id.0,
                    file_name: photo.file_name.clone(),
                    from: kw.clone(),
                    to,
                    kind: "place",
                    evidence,
                    person: None,
                }),
                (Some((_, name)), Some((place, _))) => ambiguous.push(Ambiguous {
                    id: photo.id.0,
                    file_name: photo.file_name.clone(),
                    keyword: kw.clone(),
                    choices: vec![format!("People|{name}"), place],
                }),
                (None, None) => {
                    generic.entry(key).or_insert_with(|| kw.clone());
                }
            }
        }
    }
    candidates.sort_by(|a, b| a.file_name.cmp(&b.file_name).then_with(|| a.from.cmp(&b.from)));
    ambiguous.sort_by(|a, b| a.file_name.cmp(&b.file_name).then_with(|| a.keyword.cmp(&b.keyword)));
    let people_changes = candidates.iter().filter(|c| c.kind == "person").count();
    let place_changes = candidates.len() - people_changes;
    Audit {
        revision: s.catalog.revision,
        safe_changes: candidates.len(),
        candidates,
        ambiguous,
        generic_keywords: generic.into_values().collect(),
        photos_scanned: photos.len(),
        people_changes,
        place_changes,
    }
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let report = audit(s);
    if let Some(expected) = p.get("expectedRevision").and_then(Value::as_u64)
        && expected != report.revision
    {
        return Err(bad("classification.harmonize", "the library changed after the audit; run the audit again"));
    }
    if report.candidates.is_empty() {
        return Ok(json!({"changed": 0, "people": 0, "places": 0, "ambiguous": report.ambiguous.len()}));
    }

    #[cfg(feature = "people")]
    {
        let mut by_person = BTreeMap::<u32, Vec<u64>>::new();
        for c in report.candidates.iter().filter(|c| c.kind == "person") {
            if let Some(pid) = c.person {
                by_person.entry(pid).or_default().push(c.id);
            }
        }
        for (pid, ids) in by_person {
            crate::people::ops::tag_photos(s, &json!({"person": format!("p:{pid}"), "ids": ids}))?;
        }
    }

    let mut by_photo = BTreeMap::<u64, Vec<&Candidate>>::new();
    for c in &report.candidates {
        by_photo.entry(c.id).or_default().push(c);
    }
    let mut ops = Vec::new();
    for (id, changes) in by_photo {
        let id = PhotoId(id);
        let Some(photo) = s.catalog.photo(id) else { continue };
        let mut meta = photo.meta.clone();
        for c in changes {
            meta.keywords.retain(|k| !k.eq_ignore_ascii_case(&c.from));
            if !meta.keywords.iter().any(|k| k.eq_ignore_ascii_case(&c.to)) {
                meta.keywords.push(c.to.clone());
            }
        }
        meta.keywords.sort_by_key(|k| k.to_lowercase());
        meta.keywords.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        ops.push(Op::SetMeta { id, meta: Box::new(meta) });
    }
    if !ops.is_empty() {
        s.commit("Harmonize classification", Op::Batch { ops })?;
    }
    Ok(json!({
        "changed": report.safe_changes,
        "people": report.people_changes,
        "places": report.place_changes,
        "ambiguous": report.ambiguous.len(),
        "genericKeywordsPreserved": report.generic_keywords.len(),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "classification.audit", "Audit Classification", [], None, "{} -> safe People/Places migrations, ambiguities and preserved descriptive keywords; read-only", always, |s, _| {
            Ok(serde_json::to_value(audit(s)).unwrap_or_default())
        }),
        cmd!(
            "classification.harmonize",
            "Harmonize Classification",
            [],
            None,
            "{expectedRevision?} — applies only safe migrations from flat keywords to People|… or Places|…; ambiguous and descriptive keywords are preserved",
            always,
            apply
        ),
    ]
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{Photo, Source};

    use super::*;

    fn add(s: &mut Session, name: &str, keywords: &[&str], country: &str, state: &str, city: &str) -> PhotoId {
        let id = s.catalog.alloc_photo_id();
        let mut photo = Photo::new(id, Source::File { path: format!("/tmp/{name}") }, name, "jpg", 100, 80, "2026-01-01T00:00:00");
        photo.meta.keywords = keywords.iter().map(|x| x.to_string()).collect();
        photo.meta.country = country.into();
        photo.meta.state = state.into();
        photo.meta.city = city.into();
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        id
    }

    #[test]
    fn descriptive_keywords_stay_generic_and_safe_places_are_previewed_then_applied() {
        let mut s = Session::new();
        let id = add(&mut s, "field.jpg", &["Paris", "agriculture", "manège"], "France", "Île-de-France", "Paris");
        let before = s.catalog.revision;
        let report = s.execute("classification.audit", &json!({})).unwrap();
        assert_eq!(s.catalog.revision, before, "audit is read-only");
        assert_eq!(report["safeChanges"], 1);
        assert_eq!(report["candidates"][0]["to"], "Places|France|Île-de-France|Paris");
        assert!(report["genericKeywords"].as_array().unwrap().iter().any(|x| x == "agriculture"));
        assert!(report["genericKeywords"].as_array().unwrap().iter().any(|x| x == "manège"));

        s.execute("classification.harmonize", &json!({"expectedRevision": report["revision"]})).unwrap();
        let keys = &s.catalog.photo(id).unwrap().meta.keywords;
        assert!(keys.iter().any(|x| x == "Places|France|Île-de-France|Paris"));
        assert!(keys.iter().any(|x| x == "agriculture"));
        assert!(keys.iter().any(|x| x == "manège"));
        assert!(!keys.iter().any(|x| x == "Paris"));
    }

    #[test]
    fn stale_audit_is_rejected() {
        let mut s = Session::new();
        let id = add(&mut s, "field.jpg", &["Paris"], "France", "", "Paris");
        let report = s.execute("classification.audit", &json!({})).unwrap();
        let mut meta = s.catalog.photo(id).unwrap().meta.clone();
        meta.keywords.push("night".into());
        s.commit("External metadata edit", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
        let err = s.execute("classification.harmonize", &json!({"expectedRevision": report["revision"]})).unwrap_err();
        assert!(err.to_string().contains("changed after the audit"));
    }
}
