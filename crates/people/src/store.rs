//! The face index: one record per face found (box, scores, 512-number embedding as 16-bit
//! floats), keyed by the photo's content key, in an append-only file next to the catalog (see
//! [`crate::records`]). A photo is written whole, in one append: its records say how many faces
//! it has, so a record set cut short by a crash is ignored and the photo is analysed again. A
//! photo with no face gets one marker record, so it is not looked at again. Scanning a photo
//! again replaces its earlier faces.

use std::collections::HashMap;
use std::path::Path;

use half::f16;

use crate::DIM;
use crate::records::RecordFile;

const REC: usize = 8 + 2 + 2 + 2 + 2 + 16 + 12 + DIM * 2;

/// A face: the photo's key and the face's number in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FaceRef(pub u64, pub u16);

/// What the index keeps of a face besides its embedding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMeta {
    pub key: u64,
    pub idx: u16,
    /// `[x0, y0, x1, y1]` as fractions of the (upright) photo.
    pub rect: [f32; 4],
    /// The detector's score.
    pub det: f32,
    /// Long side of the face in pixels of the picture it was found in.
    pub size: f32,
    /// Head turn, −1 … 1.
    pub turn: f32,
}

impl FaceMeta {
    pub fn face(&self) -> FaceRef {
        FaceRef(self.key, self.idx)
    }

    /// How good a picture of the person this is, 0 – 1: sharp detector score, big, facing the camera.
    pub fn quality(&self) -> f32 {
        let size = (self.size / 110.0).clamp(0.0, 1.0);
        let frontal = (1.0 - self.turn.abs() * 1.2).clamp(0.1, 1.0);
        self.det.clamp(0.0, 1.0) * size.sqrt() * frontal
    }
}

/// The faces of a library.
pub struct FaceStore {
    metas: Vec<FaceMeta>,
    vecs: Vec<f32>,
    alive: Vec<bool>,
    photos: HashMap<u64, Vec<usize>>,
    file: Option<RecordFile>,
}

fn put_record(out: &mut Vec<u8>, key: u64, idx: u16, n: u16, flags: u16, m: Option<&FaceMeta>, v: Option<&[f32]>) {
    out.extend_from_slice(&key.to_le_bytes());
    out.extend_from_slice(&idx.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    let rect = m.map_or([0.0; 4], |m| m.rect);
    for x in rect {
        out.extend_from_slice(&x.to_le_bytes());
    }
    for x in [m.map_or(0.0, |m| m.det), m.map_or(0.0, |m| m.size), m.map_or(0.0, |m| m.turn)] {
        out.extend_from_slice(&x.to_le_bytes());
    }
    for i in 0..DIM {
        let x = v.and_then(|v| v.get(i)).copied().unwrap_or(0.0);
        out.extend_from_slice(&f16::from_f32(x).to_le_bytes());
    }
}

fn f32_at(r: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(r[at..at + 4].try_into().unwrap_or_default())
}

impl FaceStore {
    /// An index kept in `path` (`None`: memory only) for embeddings made by `tag`.
    pub fn open(path: Option<&Path>, tag: &str) -> std::io::Result<Self> {
        let mut me = Self { metas: Vec::new(), vecs: Vec::new(), alive: Vec::new(), photos: HashMap::new(), file: None };
        if let Some(p) = path {
            let (file, bytes) = RecordFile::open(p, b"LCPF", tag, REC)?;
            me.load(&bytes);
            me.file = Some(file);
        }
        Ok(me)
    }

    fn load(&mut self, bytes: &[u8]) {
        // per photo, the records of its last complete scan
        let mut pending: HashMap<u64, (u16, Vec<(FaceMeta, Vec<f32>)>)> = HashMap::new();
        for r in bytes.as_chunks::<REC>().0 {
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
            let meta = FaceMeta {
                key,
                idx,
                rect: [f32_at(r, 16), f32_at(r, 20), f32_at(r, 24), f32_at(r, 28)],
                det: f32_at(r, 32),
                size: f32_at(r, 36),
                turn: f32_at(r, 40),
            };
            let v: Vec<f32> = r[44..].as_chunks::<2>().0.iter().map(|c| f16::from_le_bytes(*c).to_f32()).collect();
            if usize::from(idx) == faces.len() && usize::from(*want) > faces.len() {
                faces.push((meta, v));
            }
        }
        let mut keys: Vec<u64> = pending.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            if let Some((n, faces)) = pending.remove(&key)
                && faces.len() == usize::from(n)
            {
                self.insert(key, faces);
            }
        }
    }

    fn insert(&mut self, key: u64, faces: Vec<(FaceMeta, Vec<f32>)>) {
        for s in self.photos.remove(&key).unwrap_or_default() {
            if let Some(a) = self.alive.get_mut(s) {
                *a = false;
            }
        }
        let mut slots = Vec::with_capacity(faces.len());
        for (m, v) in faces {
            slots.push(self.metas.len());
            self.metas.push(m);
            self.vecs.extend_from_slice(&v);
            self.alive.push(true);
        }
        self.photos.insert(key, slots);
    }

    /// Record the faces of photos (each photo's list may be empty): kept in memory and appended
    /// to the file, one photo's records together.
    pub fn add(&mut self, photos: Vec<(u64, Vec<(FaceMeta, Vec<f32>)>)>) -> std::io::Result<()> {
        let mut out = Vec::new();
        for (key, faces) in &photos {
            let n = faces.len().min(usize::from(u16::MAX)) as u16;
            if faces.is_empty() {
                put_record(&mut out, *key, 0, 0, 1, None, None);
            }
            for (i, (m, v)) in faces.iter().enumerate().take(usize::from(n)) {
                let m = FaceMeta { key: *key, idx: i as u16, ..*m };
                put_record(&mut out, *key, i as u16, n, 0, Some(&m), Some(v));
            }
        }
        for (key, mut faces) in photos {
            faces.truncate(usize::from(u16::MAX));
            for (i, f) in faces.iter_mut().enumerate() {
                f.0.key = key;
                f.0.idx = i as u16;
                f.1 = f.1.iter().map(|x| f16::from_f32(*x).to_f32()).collect();
            }
            self.insert(key, faces);
        }
        match &mut self.file {
            Some(f) => f.append(&out),
            None => Ok(()),
        }
    }

    /// A copy of the faces held, without the file (for work that must not hold the live index).
    pub fn snapshot(&self) -> FaceStore {
        FaceStore { metas: self.metas.clone(), vecs: self.vecs.clone(), alive: self.alive.clone(), photos: self.photos.clone(), file: None }
    }

    /// Photos analysed (with or without faces).
    pub fn photo_count(&self) -> usize {
        self.photos.len()
    }

    /// Faces held.
    pub fn face_count(&self) -> usize {
        self.photos.values().map(Vec::len).sum()
    }

    /// Whether the photo was analysed.
    pub fn has_photo(&self, key: u64) -> bool {
        self.photos.contains_key(&key)
    }

    /// Slots of the faces of a photo.
    pub fn slots_of(&self, key: u64) -> &[usize] {
        self.photos.get(&key).map_or(&[], Vec::as_slice)
    }

    pub fn slot(&self, f: FaceRef) -> Option<usize> {
        self.slots_of(f.0).iter().copied().find(|s| self.metas.get(*s).is_some_and(|m| m.idx == f.1))
    }

    pub fn meta(&self, slot: usize) -> Option<&FaceMeta> {
        self.metas.get(slot).filter(|_| self.alive.get(slot).copied().unwrap_or(false))
    }

    pub fn vector(&self, slot: usize) -> &[f32] {
        self.vecs.get(slot * DIM..(slot + 1) * DIM).unwrap_or(&[])
    }

    /// Slots of every live face.
    pub fn slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.alive.iter().enumerate().filter(|(_, a)| **a).map(|(i, _)| i)
    }

    /// Keys of the analysed photos.
    pub fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.photos.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(key: u64, v0: f32) -> (FaceMeta, Vec<f32>) {
        let mut v = vec![0.0; DIM];
        v[0] = v0;
        (FaceMeta { key, idx: 0, rect: [0.1, 0.2, 0.3, 0.4], det: 0.9, size: 80.0, turn: 0.1 }, v)
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lc-people-{name}-{}", std::process::id()))
    }

    #[test]
    fn photos_persist_are_replaced_and_a_torn_write_is_ignored() {
        let p = tmp("store");
        let _ = std::fs::remove_file(&p);
        let mut s = FaceStore::open(Some(&p), "t").unwrap();
        s.add(vec![(1, vec![face(1, 0.5), face(1, 0.25)]), (2, vec![]), (3, vec![face(3, 1.0)])]).unwrap();
        assert_eq!((s.photo_count(), s.face_count()), (3, 3));
        assert!(s.has_photo(2) && s.slots_of(2).is_empty());
        // scanning photo 1 again replaces its faces
        s.add(vec![(1, vec![face(1, 0.75)])]).unwrap();
        assert_eq!((s.photo_count(), s.face_count()), (3, 2));
        drop(s);
        let s = FaceStore::open(Some(&p), "t").unwrap();
        assert_eq!((s.photo_count(), s.face_count()), (3, 2));
        let slot = s.slot(FaceRef(1, 0)).unwrap();
        assert!((s.vector(slot)[0] - 0.75).abs() < 1e-3);
        assert_eq!(s.meta(slot).unwrap().rect, [0.1, 0.2, 0.3, 0.4]);
        drop(s);
        // a photo whose records were cut after the first of two faces: dropped (and the file keeps working)
        let mut b = std::fs::read(&p).unwrap();
        let mut one = Vec::new();
        put_record(&mut one, 9, 0, 2, 0, Some(&face(9, 0.1).0), Some(&face(9, 0.1).1));
        b.extend_from_slice(&one);
        std::fs::write(&p, &b).unwrap();
        let s = FaceStore::open(Some(&p), "t").unwrap();
        assert!(!s.has_photo(9) && s.has_photo(1));
        drop(s);
        // another model: the old faces are not used
        assert_eq!(FaceStore::open(Some(&p), "other").unwrap().photo_count(), 0);
        let _ = std::fs::remove_file(&p);
    }
}
