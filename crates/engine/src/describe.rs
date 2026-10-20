//! Annotations: what a photo, or a moment of a video, shows — **scene**, **action** (what is
//! happening, what people do) and a free **description** — in one store shared by photos and
//! videos: `<library>/describe/annotations.jsonl` (see `docs/describe.md`).
//!
//! - **One line per annotation**, JSON, appended (never rewritten in place), next to the catalog and
//!   never in its journal. A later line for the same `(item, ms, source)` replaces the earlier one;
//!   a line cut short by a crash, or unreadable, is skipped.
//! - **The key** is the one of the face index: `item` is the catalog content key of a photo or a
//!   video; `ms` is absent for a whole photo (key = `key_of(item)`) and set for a moment of a video
//!   (key = `key_of("<item>@<ms>")`, [`crate::video::frame_key`]). Who is in the picture is not
//!   copied here: the faces under the same key say it (`<library>/people/`).
//! - **Status**: `candidate` (written by a program: Video Catcher, a model) or `confirmed` (a
//!   person said yes). A program never writes `confirmed`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file, in `<library>/describe/`.
pub const FILE: &str = "annotations.jsonl";
/// Longest text kept in one field (characters).
pub const MAX_TEXT: usize = 4000;
/// Most lines read from the file (a cap on a damaged or hostile file).
const MAX_LINES: usize = 2_000_000;
/// Longest line read.
const MAX_LINE: usize = 64 << 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Proposed by a program; waits for a person.
    #[default]
    Candidate,
    /// A person said it is right.
    Confirmed,
}

/// What a photo, or a moment of a video, shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// `key_of(item)` or `key_of("<item>@<ms>")`, 16 hex digits: the key of the face index.
    pub key: String,
    /// Content key of the catalog item (photo or video).
    pub item: String,
    /// Milliseconds into the video; absent for a whole photo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(default)]
    pub scene: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: Status,
    /// Who wrote it: `videocatcher`, `manual`, a model's id…
    #[serde(default)]
    pub source: String,
    /// Where it came from inside the source (`videocatcher:moment:250`), if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// When it was written (ISO 8601).
    #[serde(default)]
    pub at: String,
}

fn clip(s: &str) -> String {
    let t = s.trim();
    match t.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => t.get(..i).unwrap_or(t).to_string(),
        None => t.to_string(),
    }
}

impl Annotation {
    /// An annotation of `item` (at `ms` for a moment of a video), its key derived.
    pub fn new(item: &str, ms: Option<u64>, scene: &str, action: &str, description: &str, source: &str, at: &str) -> Annotation {
        let key = match ms {
            Some(ms) => crate::video::frame_key(item, ms),
            None => crate::video::item_key(item),
        };
        Annotation {
            key: format!("{key:016x}"),
            item: item.into(),
            ms,
            scene: clip(scene),
            action: clip(action),
            description: clip(description),
            status: Status::Candidate,
            source: source.trim().into(),
            origin: None,
            at: at.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.scene.is_empty() && self.action.is_empty() && self.description.is_empty()
    }

    /// The same words (status and texts), whoever wrote them and when.
    pub fn same_text(&self, o: &Annotation) -> bool {
        (self.scene.as_str(), self.action.as_str(), self.description.as_str(), self.status)
            == (o.scene.as_str(), o.action.as_str(), o.description.as_str(), o.status)
    }

    fn slot(&self) -> (String, Option<u64>, String) {
        (self.item.clone(), self.ms, self.source.clone())
    }
}

/// The annotations of a library.
#[derive(Default)]
pub struct Store {
    path: Option<PathBuf>,
    items: Vec<Annotation>,
    by_slot: HashMap<(String, Option<u64>, String), usize>,
    /// Lines skipped while reading (cut short, unreadable).
    pub skipped: usize,
}

impl Store {
    /// The store of the library at `dir` (`<dir>/describe/annotations.jsonl`); `None`: memory only.
    pub fn open(library_dir: Option<&Path>) -> Store {
        let path = library_dir.map(|d| d.join("describe").join(FILE));
        let mut s = Store { path: path.clone(), ..Default::default() };
        if let Some(bytes) = path.as_deref().and_then(|p| std::fs::read(p).ok()) {
            s.read(&bytes);
        }
        s
    }

    fn read(&mut self, bytes: &[u8]) {
        for line in bytes.split(|b| *b == b'\n').take(MAX_LINES) {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match (line.len() <= MAX_LINE).then(|| serde_json::from_slice::<Annotation>(line).ok()).flatten() {
                Some(a) => self.put(a),
                None => self.skipped += 1,
            }
        }
    }

    fn put(&mut self, a: Annotation) {
        match self.by_slot.get(&a.slot()) {
            Some(i) => {
                if let Some(x) = self.items.get_mut(*i) {
                    *x = a;
                }
            }
            None => {
                self.by_slot.insert(a.slot(), self.items.len());
                self.items.push(a);
            }
        }
    }

    /// Every annotation (the latest of each `(item, ms, source)`).
    pub fn all(&self) -> &[Annotation] {
        &self.items
    }

    /// The annotations of a catalog item (a photo, or every moment of a video), in time order.
    pub fn of_item(&self, item: &str) -> Vec<&Annotation> {
        let mut v: Vec<&Annotation> = self.items.iter().filter(|a| a.item == item).collect();
        v.sort_by_key(|a| a.ms);
        v
    }

    /// The annotation `source` wrote for this slot, if any.
    pub fn get(&self, item: &str, ms: Option<u64>, source: &str) -> Option<&Annotation> {
        self.by_slot.get(&(item.to_string(), ms, source.to_string())).and_then(|i| self.items.get(*i))
    }

    /// Append annotations (kept in memory and written to the file, all together).
    pub fn add(&mut self, new: Vec<Annotation>) -> Result<(), String> {
        if new.is_empty() {
            return Ok(());
        }
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let mut out = Vec::new();
            // a line cut short by a crash is closed first, so it stays alone and is skipped
            let torn = std::fs::read(p).ok().is_some_and(|b| b.last().is_some_and(|c| *c != b'\n'));
            if torn {
                out.push(b'\n');
            }
            for a in &new {
                serde_json::to_writer(&mut out, a).map_err(|e| e.to_string())?;
                out.push(b'\n');
            }
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).map_err(|e| format!("{}: {e}", p.display()))?;
            f.write_all(&out).map_err(|e| format!("{}: {e}", p.display()))?;
            f.sync_data().map_err(|e| format!("{}: {e}", p.display()))?;
        }
        for a in new {
            self.put(a);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_persist_replace_and_survive_damage() {
        let d = std::env::temp_dir().join(format!("lc-describe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut s = Store::open(Some(&d));
        assert!(s.all().is_empty());
        let a = Annotation::new("hash1", Some(250), "Mur graffiti", "Courroie qui pend", "Une courroie pend.", "videocatcher", "2026-10-20T10:00:00");
        let photo = Annotation::new("hash2", None, "Plage", "", "", "manual", "2026-10-20T10:00:00");
        assert_eq!(a.key, format!("{:016x}", crate::video::frame_key("hash1", 250)));
        assert_eq!(photo.key, format!("{:016x}", crate::video::item_key("hash2")));
        s.add(vec![a.clone(), photo.clone()]).unwrap();
        // a crash in the middle of a line, then a new line: the torn one is skipped
        let f = d.join("describe").join(FILE);
        let mut b = std::fs::read(&f).unwrap();
        b.extend_from_slice(b"{\"key\":\"00\",\"item\":\"hash3\",\"sce");
        std::fs::write(&f, &b).unwrap();
        let mut s = Store::open(Some(&d));
        assert_eq!((s.all().len(), s.skipped), (2, 1));
        let mut a2 = a.clone();
        a2.description = "Autre".into();
        s.add(vec![a2.clone()]).unwrap();
        let s = Store::open(Some(&d));
        assert_eq!(s.all().len(), 2);
        assert_eq!(s.get("hash1", Some(250), "videocatcher").unwrap().description, "Autre");
        assert_eq!(s.of_item("hash2").len(), 1);
        // hostile lines: not JSON, wrong types, huge
        std::fs::write(&f, [b"null\n[1,2]\n{\"ms\":-1}\n".as_slice(), &vec![b'x'; MAX_LINE + 10], b"\n"].concat()).unwrap();
        let s = Store::open(Some(&d));
        assert!(s.all().is_empty());
        assert_eq!(s.skipped, 4);
        let long = "é".repeat(MAX_TEXT + 50);
        assert_eq!(Annotation::new("x", None, &long, "", "", "m", "").scene.chars().count(), MAX_TEXT);
        let _ = std::fs::remove_dir_all(&d);
    }
}
