//! Bring in what Video Catcher already did for videos of the catalog: the pictures it cut (copied
//! into `<library>/video/<key>/`, listed as from `videocatcher`) and the moments it described
//! (scene, action, description: candidates in the annotation store, [`crate::describe`]).
//!
//! Its SQLite file is only read (with its committed WAL), never written. A Video Catcher video is
//! matched to a catalog video by its path. Nothing of the catalog is changed.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::{Value, json};

use super::frames::{self, Frame, Manifest};
use crate::describe::{Annotation, Status, Store};

type Row = BTreeMap<String, Value>;

fn text<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).unwrap_or("")
}

fn int(r: &Row, k: &str) -> Option<i64> {
    r.get(k).and_then(Value::as_i64)
}

fn ms(r: &Row, k: &str) -> Option<u64> {
    int(r, k).and_then(|v| u64::try_from(v).ok()).filter(|v| *v < 864_000_000)
}

/// A picture file worth copying: there, not huge, a JPEG.
fn jpeg_ok(p: &Path) -> Option<u64> {
    use std::io::Read;
    let len = std::fs::metadata(p).ok().filter(|m| m.is_file())?.len();
    if !(4..=32 << 20).contains(&len) {
        return None;
    }
    let mut head = [0u8; 3];
    std::fs::File::open(p).ok()?.read_exact(&mut head).ok()?;
    (head == [0xFF, 0xD8, 0xFF]).then_some(len)
}

/// What an import found and did.
#[derive(Default)]
pub struct Report {
    pub videos: usize,
    pub matched: usize,
    pub unmatched: Vec<String>,
    pub frames_copied: usize,
    pub frames_kept: usize,
    pub frames_missing: usize,
    pub moments: usize,
    pub moments_empty: usize,
    pub annotations_added: usize,
    pub annotations_unchanged: usize,
    pub over_budget: bool,
    /// Videos of the database brought into the catalog (`createMissing`).
    pub created: usize,
    /// Videos of the database whose file is not there (offline share, moved).
    pub offline: usize,
    pub import_failed: Vec<String>,
    /// People Video Catcher linked to moments: kept as hints, never as identities.
    pub person_hints: usize,
    /// Videos of the database that are a copy of another one (same fingerprint), matched to it.
    pub duplicates: usize,
    /// A dry run with `createMissing`: the files that would be imported.
    pub missing_paths: Vec<String>,
}

impl Report {
    pub fn json(&self, dry: bool) -> Value {
        json!({
            "dryRun": dry, "videos": self.videos, "matched": self.matched, "unmatched": self.unmatched.len(),
            "unmatchedExamples": self.unmatched.iter().take(10).collect::<Vec<_>>(),
            "framesCopied": self.frames_copied, "framesKept": self.frames_kept, "framesMissing": self.frames_missing,
            "moments": self.moments, "momentsEmpty": self.moments_empty,
            "annotationsAdded": self.annotations_added, "annotationsUnchanged": self.annotations_unchanged,
            "overBudget": self.over_budget,
            "created": self.created, "offline": self.offline, "importFailed": self.import_failed.len(),
            "importFailedExamples": self.import_failed.iter().take(10).collect::<Vec<_>>(),
            "personHints": self.person_hints, "duplicates": self.duplicates, "missingPaths": self.missing_paths,
        })
    }
}

/// The database's videos the catalog lacks, imported by file (or only counted when `dry`).
fn create_missing_videos(s: &mut crate::Session, videos: &[Row], dry: bool, r: &mut Report) {
    let known: std::collections::HashSet<String> =
        s.catalog.photos().filter(|p| p.in_library()).filter_map(|p| super::source_path(p).map(str::to_string)).collect();
    let mut todo = Vec::new();
    for v in videos {
        let path = text(v, "path");
        if path.is_empty() || known.contains(path) {
            continue;
        }
        if std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
            todo.push(path.to_string());
        } else {
            r.offline += 1;
        }
    }
    if dry {
        r.created = todo.len();
        r.missing_paths = todo.clone();
    } else {
        for batch in todo.chunks(25) {
            match s.execute("library.import", &json!({"paths": batch})) {
                Ok(v) => {
                    r.created += v["imported"].as_array().map_or(0, Vec::len);
                    for f in v["failed"].as_array().into_iter().flatten() {
                        r.import_failed.push(f.to_string());
                    }
                }
                Err(e) => r.import_failed.push(e.to_string()),
            }
        }
    }
}

/// Import from the Video Catcher database `db` into the session's library.
pub fn import(s: &mut crate::Session, db: &Path, dry: bool, max_bytes: u64, create_missing: bool) -> Result<Report, String> {
    let root = super::cache_root(s).ok_or("the library is not on disk")?;
    let lib = s.library.as_ref().map(|l| l.dir.clone()).ok_or("no library")?;
    let mut t = crate::lightroom_catalog::read_sqlite_tables(db, &["videos", "frames", "moments", "persons", "moment_people"])?;
    let mut take = |n: &str| t.remove(n).unwrap_or_default();
    let (videos, frame_rows, moments, persons, moment_people) =
        (take("videos"), take("frames"), take("moments"), take("persons"), take("moment_people"));
    if videos.is_empty() {
        return Err(format!("{} has no Video Catcher videos (table `videos` empty or missing)", db.display()));
    }
    let mut r = Report { videos: videos.len(), ..Default::default() };
    // the database's videos the catalog lacks: imported by file (read only: ffprobe and three
    // samples of each file), a batch at a time; files already catalogued are skipped, so an
    // interrupted run simply resumes
    if create_missing {
        create_missing_videos(s, &videos, dry, &mut r);
    }
    // catalog videos by path
    let mut by_path: HashMap<String, (PhotoId, String)> = HashMap::new();
    for p in s.catalog.photos().filter(|p| p.in_library() && p.kind == MediaKind::Video) {
        if let Some(path) = super::source_path(p) {
            by_path.insert(path.to_string(), (p.id, crate::media::content_key(p)));
            by_path.entry(path.to_lowercase()).or_insert_with(|| (p.id, crate::media::content_key(p)));
        }
    }
    // Video Catcher id → content key
    let mut vc: HashMap<i64, String> = HashMap::new();
    for v in &videos {
        let (Some(id), path) = (int(v, "id"), text(v, "path")) else { continue };
        match by_path.get(path).or_else(|| by_path.get(&path.to_lowercase())) {
            Some((_, ck)) => {
                r.matched += 1;
                vc.insert(id, ck.clone());
            }
            None => r.unmatched.push(path.to_string()),
        }
    }
    // a file Video Catcher knows twice (same fingerprint, two paths) is catalogued once (the import
    // skips a duplicate): its other copy's moments belong to the same video
    let by_fp: HashMap<String, String> = videos
        .iter()
        .filter_map(|v| Some((text(v, "fingerprint").to_string(), vc.get(&int(v, "id")?)?.clone())))
        .filter(|(f, _)| !f.is_empty())
        .collect();
    for v in &videos {
        let Some(id) = int(v, "id") else { continue };
        if vc.contains_key(&id) {
            continue;
        }
        if let Some(ck) = by_fp.get(text(v, "fingerprint")) {
            r.duplicates += 1;
            r.unmatched.retain(|p| p != text(v, "path"));
            vc.insert(id, ck.clone());
        }
    }
    // pictures
    let mut per_video: HashMap<i64, Vec<&Row>> = HashMap::new();
    for f in &frame_rows {
        if let Some(v) = int(f, "video_id").filter(|v| vc.contains_key(v)) {
            per_video.entry(v).or_default().push(f);
        }
    }
    let mut budget = max_bytes.saturating_sub(super::frames_bytes(&root));
    for (vid, rows) in per_video {
        let Some(ck) = vc.get(&vid) else { continue };
        let dir = super::video_dir(&root, ck);
        let path = videos.iter().find(|v| int(v, "id") == Some(vid)).map(|v| text(v, "path").to_string()).unwrap_or_default();
        let duration = videos.iter().find(|v| int(v, "id") == Some(vid)).and_then(|v| ms(v, "duration_ms"));
        let mut m = match Manifest::load(&dir) {
            Some(m) if m.content_key == *ck => m,
            _ => Manifest::new(ck, &path, duration, "videocatcher"),
        };
        let mut changed = false;
        for f in rows {
            let (Some(at), src) = (ms(f, "timestamp_ms"), PathBuf::from(text(f, "path"))) else { continue };
            if m.has(at) {
                r.frames_kept += 1;
                continue;
            }
            let Some(len) = jpeg_ok(&src) else {
                r.frames_missing += 1;
                continue;
            };
            if len > budget {
                r.over_budget = true;
                break;
            }
            if !dry {
                let name = frames::file_name(at);
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let tmp = dir.join(format!("{name}.part"));
                std::fs::copy(&src, &tmp).map_err(|e| format!("{}: {e}", src.display()))?;
                std::fs::rename(&tmp, dir.join(&name)).map_err(|e| format!("{}: {e}", dir.display()))?;
                m.frames.push(Frame { ms: at, file: name, bytes: len, origin: "videocatcher".into() });
                changed = true;
            }
            budget = budget.saturating_sub(len);
            r.frames_copied += 1;
        }
        if changed {
            m.frames.sort_by_key(|f| f.ms);
            if m.source != "videocatcher" && !m.source.contains("videocatcher") {
                m.source = format!("{}+videocatcher", m.source);
            }
            m.save(&dir)?;
        }
    }
    if r.frames_copied > 0 && !dry {
        s.video.rev += 1;
    }
    // people Video Catcher put on moments: hints only (its names, its matches), never identities;
    // who is there comes from LightCraft's own face index
    let names: HashMap<i64, String> = persons.iter().filter_map(|p| Some((int(p, "id")?, text(p, "name").to_string()))).collect();
    let mut hints: HashMap<i64, Vec<String>> = HashMap::new();
    for mp in &moment_people {
        if let (Some(m), Some(name)) = (int(mp, "moment_id"), int(mp, "person_id").and_then(|p| names.get(&p))) {
            hints.entry(m).or_default().push(format!("videocatcher:{name}"));
            r.person_hints += 1;
        }
    }
    // moments → annotations
    let mut store = Store::open(Some(&lib));
    let now = (s.clock)();
    let mut new = Vec::new();
    for mo in &moments {
        let (Some(vid), Some(at)) = (int(mo, "video_id"), ms(mo, "timestamp_ms")) else { continue };
        let Some(ck) = vc.get(&vid) else { continue };
        r.moments += 1;
        let pick = |a: &str, b: &str| if text(mo, a).trim().is_empty() { text(mo, b).to_string() } else { text(mo, a).to_string() };
        let mut a = Annotation::new(
            ck,
            Some(at),
            &pick("scene_text", "machine_scene"),
            &pick("action_text", "machine_action"),
            &pick("description", "machine_description"),
            "videocatcher",
            &now,
        );
        if a.is_empty() {
            r.moments_empty += 1;
            continue;
        }
        // a person confirmed it in Video Catcher; anything else stays a candidate
        a.status = if text(mo, "status") == "confirmed" { Status::Confirmed } else { Status::Candidate };
        a.origin = int(mo, "id").map(|id| format!("videocatcher:moment:{id}"));
        a.hints = int(mo, "id").and_then(|id| hints.get(&id)).cloned().unwrap_or_default();
        if store.get(ck, Some(at), "videocatcher").is_some_and(|old| old.same_text(&a)) {
            r.annotations_unchanged += 1;
            continue;
        }
        new.push(a);
    }
    r.annotations_added = new.len();
    if !dry {
        store.add(new)?;
    }
    Ok(r)
}
