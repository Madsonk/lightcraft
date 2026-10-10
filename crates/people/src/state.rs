//! What the user decided: the people they named or confirmed, the faces they said are not a
//! given person or not worth showing. Small, human-readable (`people.json` next to the face
//! index), written whole through a temporary file. The face index itself never holds a decision,
//! so deleting it loses only work that can be redone.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store::FaceRef;

const FORMAT: u32 = 1;

/// A person: faces the user confirmed are theirs. The name can come later.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Person {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub faces: Vec<FaceRef>,
}

/// Thresholds and switches (cosine similarities of ArcFace embeddings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Two faces are put in one group from this similarity up.
    pub link: f32,
    /// A face is suggested as a known person from this similarity up.
    pub suggest: f32,
    /// Above this, a face is tagged as the person without asking (`None`: always ask).
    pub auto_tag: Option<f32>,
    /// Faces below this size (pixels) are left out of the groups.
    pub min_face_px: f32,
    /// Use the light MobileFaceNet instead of the ResNet-50 (about 6 times faster, less exact).
    pub fast_model: bool,
    /// Give the photos of faces marked "unknown person" the keyword `People|Unknown` (off by
    /// default: an unknown face is only remembered as seen).
    pub tag_unknown: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { link: 0.50, suggest: 0.45, auto_tag: None, min_face_px: crate::detect::MIN_FACE_PX, fast_model: false, tag_unknown: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub format: u32,
    pub next_id: u32,
    pub persons: Vec<Person>,
    /// "This face is not that person".
    pub rejected: Vec<(FaceRef, u32)>,
    /// "These two faces are not the same person".
    pub different: Vec<(FaceRef, FaceRef)>,
    /// Faces to leave out (not a face, or not wanted).
    pub ignored: Vec<FaceRef>,
    /// Questions answered "not sure": not asked again.
    pub snoozed: Vec<String>,
    /// Faces the user looked at and does not know ("unknown person"): out of the list of groups to
    /// name, kept in a section of their own where they can be named later. No keyword unless
    /// `settings.tag_unknown`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<FaceRef>,
    /// Photos the user filed under a person as a whole (a photo dropped on them), by the key of the
    /// photo's content: `(photo key, person)`. Unlike the keyword of a confirmed face, these are
    /// never removed by re-syncing the person, nor by moving one face of the photo elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photo_tags: Vec<(u64, u32)>,
    pub settings: Settings,
}

impl Default for State {
    fn default() -> Self {
        Self {
            format: FORMAT,
            next_id: 1,
            persons: Vec::new(),
            rejected: Vec::new(),
            different: Vec::new(),
            ignored: Vec::new(),
            snoozed: Vec::new(),
            unknown: Vec::new(),
            photo_tags: Vec::new(),
            settings: Settings::default(),
        }
    }
}

impl State {
    /// Read `path`; a missing or unreadable file is an empty state (the error is returned
    /// too, so the caller can say the file was set aside).
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read(path) {
            Ok(b) => match serde_json::from_slice::<State>(&b) {
                Ok(s) => (s, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(_) => (Self::default(), None),
        }
    }

    /// Write the state (candidate serialized and parsed back first, then renamed into place).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        serde_json::from_slice::<State>(&text).map_err(|e| format!("the people file would not read back: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn person(&self, id: u32) -> Option<&Person> {
        self.persons.iter().find(|p| p.id == id)
    }

    pub fn person_mut(&mut self, id: u32) -> Option<&mut Person> {
        self.persons.iter_mut().find(|p| p.id == id)
    }

    /// The person with this name (any case), if any.
    pub fn by_name(&self, name: &str) -> Option<&Person> {
        self.persons.iter().find(|p| p.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name.trim())))
    }

    pub fn person_of(&self, f: FaceRef) -> Option<&Person> {
        self.persons.iter().find(|p| p.faces.contains(&f))
    }

    pub fn new_person(&mut self, name: Option<String>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.persons.push(Person { id, name, faces: Vec::new() });
        id
    }

    /// Make `faces` the person's (taken from any other person). An explicit assignment overrides
    /// an earlier "not this person" for the same face.
    pub fn assign(&mut self, id: u32, faces: &[FaceRef]) {
        for p in &mut self.persons {
            if p.id != id {
                p.faces.retain(|f| !faces.contains(f));
            }
        }
        self.rejected.retain(|(r, p)| !(*p == id && faces.contains(r)));
        if let Some(p) = self.person_mut(id) {
            for f in faces {
                if !p.faces.contains(f) {
                    p.faces.push(*f);
                }
            }
        }
        self.ignored.retain(|f| !faces.contains(f));
        self.unknown.retain(|f| !faces.contains(f));
    }

    /// Faces the user does not know: they leave every person and the ignored ones (the person
    /// they were taken from is told "not them", so they are not suggested for them again).
    pub fn mark_unknown(&mut self, faces: &[FaceRef]) {
        for p in &mut self.persons {
            let from = p.id;
            let leaving: Vec<FaceRef> = p.faces.iter().copied().filter(|f| faces.contains(f)).collect();
            p.faces.retain(|f| !faces.contains(f));
            for f in leaving {
                if !self.rejected.contains(&(f, from)) {
                    self.rejected.push((f, from));
                }
            }
        }
        self.ignored.retain(|f| !faces.contains(f));
        for f in faces {
            if !self.unknown.contains(f) {
                self.unknown.push(*f);
            }
        }
    }

    /// Faces that are not faces (a statue, a poster, a false detection): left out for good.
    pub fn mark_ignored(&mut self, faces: &[FaceRef]) {
        for p in &mut self.persons {
            p.faces.retain(|f| !faces.contains(f));
        }
        self.unknown.retain(|f| !faces.contains(f));
        for f in faces {
            if !self.ignored.contains(f) {
                self.ignored.push(*f);
            }
        }
    }

    pub fn is_unknown(&self, f: FaceRef) -> bool {
        self.unknown.contains(&f)
    }

    /// The photo (by content key) is filed under the person as a whole.
    pub fn has_photo_tag(&self, key: u64, person: u32) -> bool {
        self.photo_tags.contains(&(key, person))
    }

    /// Say a face is not this person (removing it from them if it was).
    pub fn reject(&mut self, f: FaceRef, id: u32) {
        if let Some(p) = self.person_mut(id) {
            p.faces.retain(|x| *x != f);
        }
        if !self.rejected.contains(&(f, id)) {
            self.rejected.push((f, id));
        }
    }

    pub fn is_rejected(&self, f: FaceRef, id: u32) -> bool {
        self.rejected.iter().any(|(r, p)| *r == f && *p == id)
    }

    /// Fold person `from` into `into`.
    pub fn merge(&mut self, from: u32, into: u32) {
        if from == into {
            return;
        }
        let faces = self.person(from).map(|p| p.faces.clone()).unwrap_or_default();
        self.assign(into, &faces);
        self.persons.retain(|p| p.id != from);
        for (_, p) in &mut self.rejected {
            if *p == from {
                *p = into;
            }
        }
        for (_, p) in &mut self.photo_tags {
            if *p == from {
                *p = into;
            }
        }
        self.photo_tags.sort_unstable();
        self.photo_tags.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_rejection_and_merge() {
        let mut s = State::default();
        let (a, b) = (s.new_person(Some("Awa".into())), s.new_person(None));
        let f = |n: u64| FaceRef(n, 0);
        s.assign(a, &[f(1), f(2)]);
        s.assign(b, &[f(3), f(2)]);
        assert_eq!(s.person(a).unwrap().faces, vec![f(1)]);
        assert_eq!(s.person_of(f(2)).unwrap().id, b);
        // "not Awa": f(1) leaves her; saying yes later overrides it
        s.reject(f(1), a);
        assert!(s.person(a).unwrap().faces.is_empty() && s.is_rejected(f(1), a));
        s.assign(a, &[f(1)]);
        assert_eq!(s.person(a).unwrap().faces, vec![f(1)]);
        assert!(!s.is_rejected(f(1), a));
        s.merge(b, a);
        assert_eq!(s.persons.len(), 1);
        let mut got = s.person(a).unwrap().faces.clone();
        got.sort();
        assert_eq!(got, vec![f(1), f(2), f(3)]);
        assert_eq!(s.by_name(" awa ").unwrap().id, a);
    }

    #[test]
    fn unknown_ignored_and_photo_tags_follow_the_faces_and_the_merge() {
        let mut s = State::default();
        let (a, b) = (s.new_person(Some("Awa".into())), s.new_person(Some("Bao".into())));
        let f = |n: u64| FaceRef(n, 0);
        s.assign(a, &[f(1), f(2)]);
        s.assign(b, &[f(3)]);
        // unknown: the face leaves its person (who is told "not them"), nobody else is touched
        s.mark_unknown(&[f(2), f(9)]);
        assert_eq!(s.person(a).unwrap().faces, vec![f(1)]);
        assert!(s.is_unknown(f(2)) && s.is_unknown(f(9)) && s.is_rejected(f(2), a) && !s.is_rejected(f(9), a));
        assert_eq!(s.person(b).unwrap().faces, vec![f(3)]);
        // naming it later takes it out of the unknown ones, and clears the "not them" for the new person
        s.assign(b, &[f(2)]);
        assert!(!s.is_unknown(f(2)) && s.is_unknown(f(9)));
        // not a real face is exclusive with unknown and with a person
        s.mark_ignored(&[f(9), f(3)]);
        assert!(!s.is_unknown(f(9)) && s.ignored.contains(&f(9)) && !s.person(b).unwrap().faces.contains(&f(3)));
        s.mark_unknown(&[f(9)]);
        assert!(s.is_unknown(f(9)) && !s.ignored.contains(&f(9)));
        // a photo filed under Bao follows the merge into Awa, once
        s.photo_tags.push((77, b));
        s.photo_tags.push((77, a));
        s.merge(b, a);
        assert_eq!(s.photo_tags, vec![(77, a)]);
        assert!(s.has_photo_tag(77, a) && !s.has_photo_tag(77, b));
    }

    #[test]
    fn the_file_round_trips_and_an_old_file_without_new_fields_loads() {
        let p = std::env::temp_dir().join(format!("lc-people-state-{}.json", std::process::id()));
        let mut s = State::default();
        let a = s.new_person(Some("Awa".into()));
        s.assign(a, &[FaceRef(7, 1)]);
        s.save(&p).unwrap();
        let (back, err) = State::load(&p);
        assert!(err.is_none());
        assert_eq!(back, s);
        std::fs::write(&p, br#"{"persons":[{"id":3,"faces":[[5,0]]}]}"#).unwrap();
        let (old, err) = State::load(&p);
        assert!(err.is_none() && old.persons[0].id == 3 && old.settings.link > 0.0);
        // a file from before unknown people / hand-filed photos: those are empty and off
        assert!(old.unknown.is_empty() && old.photo_tags.is_empty() && !old.settings.tag_unknown);
        // and an empty state does not write the new fields at all (older versions read the file as before)
        let text = serde_json::to_string(&State::default()).unwrap();
        assert!(!text.contains("photo_tags") && !text.contains("\"unknown\""), "{text}");
        std::fs::write(&p, b"not json").unwrap();
        assert!(State::load(&p).1.is_some());
        let _ = std::fs::remove_file(&p);
    }
}
