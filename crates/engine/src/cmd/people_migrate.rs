//! `people.migrate`: bring the people of our earlier face index (`<library>/people/people.json` and
//! `faces-<model>.bin`, made by the fork's `lightcraft-people`) into 0.14.0's own model, without
//! losing anything.
//!
//! 0.14.0 keeps a person as the **name of face regions in the catalogue** (`Meta.regions`, one
//! undo step in the journal); LightCraft never writes `mwg-rs:Regions` into an original or a
//! sidecar, and this command does not either (it sets `skip_auto_write`). So:
//!
//! - a named person's every face becomes a named face region of its photo (an existing region
//!   overlapping the face by IoU ≥ 0.3 is named instead of adding a second one; a name already
//!   there is never replaced: a different one is listed as a conflict);
//! - an unnamed group's faces become unnamed face regions, which 0.14.0's own unnamed-faces view
//!   groups again;
//! - what has no equivalent (rejections, "not the same", ignored faces, "unknown", settings, the
//!   exact grouping) stays in `people/people.json`, copied once to `people/legacy-people.json`, and
//!   is read back by `people.legacy`;
//! - the default is a dry run; applying first copies `people/` and the catalogue files to
//!   `<library>/migration-backups/<time>/`.
//!
//! The report counts everything so that "nothing lost" can be checked against the file.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_geom::Rect;
use lightcraft_meta::{Region, RegionKind};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

/// An existing region this much overlapped by a legacy face is the same face.
const SAME_FACE: f64 = 0.3;
/// Size of one record of the legacy `faces-<model>.bin`: key, index, count, flags, pad, box, three scores, 512 half floats.
const REC: usize = 8 + 2 + 2 + 2 + 2 + 16 + 12 + 512 * 2;

fn people_dir(s: &Session, c: &str) -> Result<PathBuf> {
    let lib = s.library.as_ref().filter(|l| l.on_disk).ok_or_else(|| bad(c, "the library is not a folder on disk"))?;
    Ok(lib.dir.join("people"))
}

fn face_ref(v: &Value) -> Option<(u64, u16)> {
    let a = v.as_array()?;
    Some((a.first()?.as_u64()?, u16::try_from(a.get(1)?.as_u64()?).ok()?))
}

/// The box of every face of the legacy index, by (photo key, face number): the last complete scan
/// of each photo, as the fork's index read it.
pub(crate) fn legacy_boxes(dir: &Path) -> HashMap<(u64, u16), [f32; 4]> {
    let mut best: Option<(u64, PathBuf)> = None;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with("faces-") && name.ends_with(".bin") {
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((len, p));
            }
        }
    }
    let Some((_, path)) = best else { return HashMap::new() };
    let Ok(bytes) = std::fs::read(&path) else { return HashMap::new() };
    let u32_at = |at: usize| bytes.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let (Some(rec_len), Some(tag_len)) = (u32_at(8), u32_at(12)) else { return HashMap::new() };
    if bytes.get(..4) != Some(b"LCPF") || rec_len as usize != REC {
        return HashMap::new();
    }
    let start = 16 + tag_len as usize;
    let f32_at = |r: &[u8], at: usize| r.get(at..at + 4).and_then(|b| b.try_into().ok()).map(f32::from_le_bytes).unwrap_or(0.0);
    let mut pending: HashMap<u64, (u16, Vec<[f32; 4]>)> = HashMap::new();
    let mut out = HashMap::new();
    let records = bytes.get(start..).unwrap_or(&[]);
    for r in (0..records.len() / REC).filter_map(|i| records.get(i * REC..(i + 1) * REC)) {
        let key = u64::from_le_bytes(r[..8].try_into().unwrap_or_default());
        let idx = u16::from_le_bytes(r[8..10].try_into().unwrap_or_default());
        let n = u16::from_le_bytes(r[10..12].try_into().unwrap_or_default());
        if idx == 0 {
            pending.insert(key, (n, Vec::new()));
        }
        let Some((want, faces)) = pending.get_mut(&key) else { continue };
        if n == 0 {
            continue;
        }
        if usize::from(idx) == faces.len() && usize::from(*want) > faces.len() {
            faces.push([f32_at(r, 16), f32_at(r, 20), f32_at(r, 24), f32_at(r, 28)]);
        }
        if faces.len() == usize::from(*want) {
            for (i, b) in faces.iter().enumerate() {
                out.insert((key, i as u16), *b);
            }
        }
    }
    out
}

fn iou(a: &Rect, b: &Rect) -> f64 {
    let (w, h) = ((a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0), (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0));
    let inter = w * h;
    let union = (a.x1 - a.x0) * (a.y1 - a.y0) + (b.x1 - b.x0) * (b.y1 - b.y0) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            n += copy_dir(&src, &dst)?;
        } else {
            n += std::fs::copy(&src, &dst)?;
        }
    }
    Ok(n)
}

/// What a run found and (unless `dryRun`) did.
#[derive(Default)]
struct Tally {
    persons: usize,
    named_persons: usize,
    faces: usize,
    no_record: Vec<(u64, u16)>,
    no_photo: Vec<(u64, u16)>,
    named_existing: usize,
    added_named: usize,
    already_named: usize,
    conflicts: Vec<Value>,
    unnamed_existing: usize,
    added_unnamed: usize,
    photos: usize,
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "people.migrate";
    let dry = p.get("dryRun").and_then(Value::as_bool).unwrap_or(true);
    let dir = people_dir(s, C)?;
    let file = dir.join("people.json");
    let raw = std::fs::read(&file).map_err(|e| bad(C, format!("{}: {e}", file.display())))?;
    let legacy: Value = serde_json::from_slice(&raw).map_err(|e| bad(C, format!("{}: not readable: {e}", file.display())))?;
    let persons = legacy["persons"].as_array().cloned().unwrap_or_default();
    let boxes = legacy_boxes(&dir);
    // photo key → catalogue photos (a video's virtual copies and a photo's copies share a key)
    let mut by_key: HashMap<u64, Vec<PhotoId>> = HashMap::new();
    for ph in s.catalog.photos().filter(|q| q.in_library()) {
        by_key.entry(crate::video::item_key(&crate::media::content_key(ph))).or_default().push(ph.id);
    }
    let mut t = Tally::default();
    let mut work: BTreeMap<PhotoId, lightcraft_catalog::Meta> = BTreeMap::new();
    let mut expected: HashMap<String, usize> = HashMap::new();
    for person in &persons {
        t.persons += 1;
        let name = person["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
        t.named_persons += name.is_some() as usize;
        for f in person["faces"].as_array().into_iter().flatten().filter_map(face_ref) {
            t.faces += 1;
            let Some(b) = boxes.get(&f) else {
                t.no_record.push(f);
                continue;
            };
            let Some(ids) = by_key.get(&f.0) else {
                t.no_photo.push(f);
                continue;
            };
            let rect = Rect { x0: f64::from(b[0].clamp(0.0, 1.0)), y0: f64::from(b[1].clamp(0.0, 1.0)), x1: f64::from(b[2].clamp(0.0, 1.0)), y1: f64::from(b[3].clamp(0.0, 1.0)) };
            for id in ids {
                let Some(photo) = s.catalog.photo(*id) else { continue };
                let meta = work.entry(*id).or_insert_with(|| photo.meta.clone());
                let hit = meta
                    .regions
                    .iter_mut()
                    .filter(|r| r.kind == RegionKind::Face)
                    .map(|r| (iou(&r.rect, &rect), r))
                    .filter(|(o, _)| *o >= SAME_FACE)
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, r)| r);
                let mut conflict = false;
                match (hit, &name) {
                    (Some(r), Some(n)) => match r.name.as_deref() {
                        None => {
                            r.name = Some(n.clone());
                            t.named_existing += 1;
                        }
                        Some(x) if x.eq_ignore_ascii_case(n) => t.already_named += 1,
                        Some(x) => {
                            conflict = true;
                            t.conflicts.push(json!({"photo": id.0, "legacy": n, "catalogue": x}));
                        }
                    },
                    (Some(_), None) => t.unnamed_existing += 1,
                    (None, _) => {
                        meta.regions.push(Region { rect, kind: RegionKind::Face, name: name.clone(), description: None });
                        if name.is_some() {
                            t.added_named += 1;
                        } else {
                            t.added_unnamed += 1;
                        }
                    }
                }
                // a conflict keeps the catalogue's name and is reported on its own
                if let (Some(n), false) = (&name, conflict) {
                    *expected.entry(n.to_lowercase()).or_default() += 1;
                }
            }
        }
    }
    // only the photos whose regions changed are written
    let changed: Vec<(PhotoId, lightcraft_catalog::Meta)> =
        work.into_iter().filter(|(id, m)| s.catalog.photo(*id).is_some_and(|ph| ph.meta.regions != m.regions)).collect();
    t.photos = changed.len();
    let mut backup = Value::Null;
    if !dry && !changed.is_empty() {
        let lib = s.library.as_ref().map(|l| l.dir.clone()).ok_or_else(|| bad(C, "no library"))?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let to = lib.join("migration-backups").join(format!("{stamp}"));
        copy_dir(&dir, &to.join("people")).map_err(|e| bad(C, format!("backup of people/ failed, nothing was changed: {e}")))?;
        for f in ["catalog.snap", "catalog.log", "prefs.json", "view.json", "presets.json"] {
            let src = lib.join(f);
            if src.is_file() {
                std::fs::copy(&src, to.join(f)).map_err(|e| bad(C, format!("backup of {f} failed, nothing was changed: {e}")))?;
            }
        }
        let legacy_copy = dir.join("legacy-people.json");
        if !legacy_copy.exists() {
            std::fs::write(&legacy_copy, &raw).map_err(|e| bad(C, format!("{}: {e}", legacy_copy.display())))?;
        }
        let ops: Vec<Op> = changed.into_iter().map(|(id, meta)| Op::SetMeta { id, meta: Box::new(meta) }).collect();
        s.commit("Migrate People", Op::Batch { ops })?;
        s.skip_auto_write = true;
        backup = json!(to.display().to_string());
    }
    // what the catalogue holds now, per name, against what the file promised
    let mut found: HashMap<String, usize> = HashMap::new();
    for ph in s.catalog.photos().filter(|q| q.in_library()) {
        for r in ph.meta.regions.iter().filter(|r| r.kind == RegionKind::Face) {
            if let Some(n) = &r.name {
                *found.entry(n.to_lowercase()).or_default() += 1;
            }
        }
    }
    let short: Vec<Value> = expected
        .iter()
        .filter(|(n, c)| found.get(*n).copied().unwrap_or(0) < **c)
        .map(|(n, c)| json!({"name": n, "expectedAtLeast": c, "found": found.get(n).copied().unwrap_or(0)}))
        .collect();
    let count = |k: &str| legacy[k].as_array().map_or(0, Vec::len);
    Ok(json!({
        "dryRun": dry,
        "file": file.display().to_string(),
        "backup": backup,
        "legacy": {"persons": t.persons, "named": t.named_persons, "faces": t.faces, "rejected": count("rejected"),
            "different": count("different"), "ignored": count("ignored"), "unknown": count("unknown")},
        "faces": {"withRecord": t.faces - t.no_record.len(), "noRecord": t.no_record.len(), "photoNotInCatalogue": t.no_photo.len(),
            "namedExistingRegion": t.named_existing, "addedNamed": t.added_named, "alreadyNamed": t.already_named,
            "unnamedExistingRegion": t.unnamed_existing, "addedUnnamed": t.added_unnamed, "conflicts": t.conflicts.len()},
        "photosChanged": t.photos,
        "conflicts": t.conflicts.iter().take(50).collect::<Vec<_>>(),
        "noRecordExamples": t.no_record.iter().take(20).map(|f| json!([f.0, f.1])).collect::<Vec<_>>(),
        "photoNotInCatalogueExamples": t.no_photo.iter().take(20).map(|f| json!([f.0, f.1])).collect::<Vec<_>>(),
        "kept": "rejections, 'not the same', ignored and unknown faces, settings and the exact grouping stay in people/people.json (and people/legacy-people.json): read them with people.legacy",
        "verification": {"namesInFile": expected.len(), "namesShort": short.len(), "short": short.into_iter().take(20).collect::<Vec<_>>()},
    }))
}

/// `people.legacy`: what the earlier people index decided and 0.14.0 cannot hold.
fn legacy(s: &mut Session, _: &Value) -> Result<Value> {
    const C: &str = "people.legacy";
    let dir = people_dir(s, C)?;
    let file = if dir.join("legacy-people.json").is_file() { dir.join("legacy-people.json") } else { dir.join("people.json") };
    let v: Value = serde_json::from_slice(&std::fs::read(&file).map_err(|e| bad(C, format!("{}: {e}", file.display())))?)
        .map_err(|e| bad(C, format!("{}: not readable: {e}", file.display())))?;
    let persons: Vec<Value> = v["persons"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| json!({"id": x["id"], "name": x["name"], "faces": x["faces"].as_array().map_or(0, Vec::len)}))
        .collect();
    Ok(json!({"file": file.display().to_string(), "persons": persons, "unnamed": persons.iter().filter(|p| p["name"].is_null()).count(),
        "rejected": v["rejected"], "different": v["different"], "ignored": v["ignored"], "unknown": v["unknown"], "settings": v["settings"]}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "people.migrate",
            "Migrate People",
            [],
            None,
            "{dryRun?: bool (true)} — brings the people of the earlier index (people/people.json + faces-<model>.bin) into the catalogue as named (or unnamed) face regions, one undo step; originals and sidecars are never written. Applying first copies people/ and the catalogue files to migration-backups/<time>/. → counts of persons, named, faces, rejections… and what was added, named, already there, in conflict or missing",
            always,
            run
        ),
        cmd!(
            query "people.legacy",
            "Legacy People",
            [],
            None,
            "{} — what the earlier people index kept that 0.14.0 has no place for (groups, rejections, not-the-same, ignored, unknown, settings)",
            always,
            legacy
        ),
    ]
}
