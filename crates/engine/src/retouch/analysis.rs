//! Looking at a photo for the retouch: developed frames (cached), faces with their skin, who they
//! are (People). Everything here only reads; nothing is written to the library.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_retouch::facegeo::Face;
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::skin::LabImage;

use super::PREFIX;
use crate::cmd::bad;
use crate::portrait::{Patch, Region};
use crate::{Result, Session};

/// Long edge of the frames the retouch looks at.
pub const EDGE: usize = 1280;

/// A developed photo (uncropped, oriented, display-encoded) and its Lab planes.
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<[u8; 4]>,
    pub lab: LabImage,
}

type FrameKey = (u64, u64, usize);

fn frames() -> &'static Mutex<Vec<(FrameKey, Arc<Frame>)>> {
    static C: Mutex<Vec<(FrameKey, Arc<Frame>)>> = Mutex::new(Vec::new());
    &C
}

/// The photo's settings without the masks this module made: what "before" means.
pub fn base_develop(s: &Session, id: PhotoId, c: &str) -> Result<DevelopSettings> {
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
    d.masks.retain(|m| !m.name.starts_with(PREFIX));
    Ok(d)
}

/// Render `d` for `id` at the analysis size (cached by photo, settings and size).
pub fn render(s: &mut Session, id: PhotoId, d: &DevelopSettings, edge: usize, c: &str) -> Result<Arc<Frame>> {
    let key = (id.0, d.hash64(), edge);
    {
        let g = frames().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, f)) = g.iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
    }
    let job = s.preview_job(id, edge, edge, false, d).ok_or_else(|| bad(c, "no such photo"))?;
    let img = job.run().rendered.map_err(|e| bad(c, e))?.image;
    let lab = LabImage::from_rgba(img.width, img.height, &img.data);
    let f = Arc::new(Frame { w: img.width, h: img.height, rgba: img.data, lab });
    let mut g = frames().lock().unwrap_or_else(PoisonError::into_inner);
    g.push((key, f.clone()));
    if g.len() > 6 {
        g.remove(0);
    }
    Ok(f)
}

/// A face found in a frame.
pub struct Found {
    pub face: Face,
    /// The soft coverage of the face's skin region (eyes, brows and lips left out), over the frame.
    pub cov: Plane,
    /// The person's name when the People index knows this face.
    pub person: Option<String>,
}

/// Draw a stored patch over a `w x h` frame as a plane (0..1).
pub fn rasterize(p: &Patch, w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    let rect = [p.rect[0] * w as f64, p.rect[1] * h as f64, p.rect[2] * w as f64, p.rect[3] * h as f64];
    let (rw, rh) = ((rect[2] - rect[0]).max(1e-6), (rect[3] - rect[1]).max(1e-6));
    let grid = Plane::from_vec(p.side, p.side, p.logits.iter().map(|l| 1.0 / (1.0 + (-l).exp())).collect());
    let (x0, x1) = (rect[0].floor().max(0.0) as usize, (rect[2].ceil() as usize).min(w));
    let (y0, y1) = (rect[1].floor().max(0.0) as usize, (rect[3].ceil() as usize).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let (u, v) = ((x as f64 + 0.5 - rect[0]) / rw, (y as f64 + 0.5 - rect[1]) / rh);
            out.d[y * w + x] = grid.sample((u * p.side as f64) as f32, (v * p.side as f64) as f32);
        }
    }
    out
}

type FacesKey = (u64, u64, usize);

fn face_cache() -> &'static Mutex<Vec<(FacesKey, Arc<Vec<Found>>)>> {
    static C: Mutex<Vec<(FacesKey, Arc<Vec<Found>>)>> = Mutex::new(Vec::new());
    &C
}

/// The faces of a frame (biggest first, at most `max`), with skin coverage and, when the People
/// index knows them, a name. `Err` when the face model is not installed.
pub fn faces(s: &mut Session, id: PhotoId, d: &DevelopSettings, frame: &Frame, max: usize, c: &str) -> Result<Arc<Vec<Found>>> {
    let key: FacesKey = (id.0, d.hash64(), max);
    {
        let g = face_cache().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, f)) = g.iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
    }
    let mut meshes = meshes(s, frame, c)?;
    meshes.sort_by(|a, b| {
        let (ba, bb) = (a.bbox(), b.bbox());
        ((bb[2] - bb[0]) * (bb[3] - bb[1])).total_cmp(&((ba[2] - ba[0]) * (ba[3] - ba[1])))
    });
    meshes.truncate(max.clamp(1, crate::portrait_max_detail()));
    let people = people_of(s, id);
    let (w, h) = (frame.w, frame.h);
    let mut out = Vec::new();
    for m in meshes {
        let Some(face) = Face::new(&m.points) else { continue };
        let Some(patch) = crate::portrait::patch(&m.points, w, h, Region::Skin) else { continue };
        let b = m.bbox();
        let bbox = [b[0] / w as f32, b[1] / h as f32, b[2] / w as f32, b[3] / h as f32];
        let (cx, cy) = ((bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0);
        let person = people.iter().find(|(r, _)| cx >= r[0] && cx <= r[2] && cy >= r[1] && cy <= r[3]).and_then(|(_, n)| n.clone());
        out.push(Found { cov: rasterize(&patch, w, h), face, person });
    }
    let out = Arc::new(out);
    let mut g = face_cache().lock().unwrap_or_else(PoisonError::into_inner);
    g.push((key, out.clone()));
    if g.len() > 6 {
        g.remove(0);
    }
    Ok(out)
}

#[cfg(feature = "faces")]
fn meshes(s: &mut Session, frame: &Frame, c: &str) -> Result<Vec<lightcraft_facemesh::mediapipe::Mesh>> {
    use lightcraft_facemesh::mediapipe::Frame as F;
    let model = crate::cmd::portrait::landmarker(s).map_err(|e| bad(c, e.to_string()))?;
    let rgb: Vec<[f32; 3]> = frame.rgba.iter().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
    let found = model.find_all(F { rgb: &rgb, w: frame.w, h: frame.h }).map_err(|e| bad(c, e))?;
    // the same test as Analyze Faces and People: a plate or a wall must not be taken for a face
    use lightcraft_facemesh::eyes::{MAX_DRIFT, MIN_DETECTION, MIN_FACE_PX};
    Ok(found
        .into_iter()
        .filter(|m| {
            let b = m.bbox();
            m.detection >= MIN_DETECTION && m.drift <= MAX_DRIFT && (b[2] - b[0]).max(b[3] - b[1]) >= MIN_FACE_PX
        })
        .collect())
}

#[cfg(not(feature = "faces"))]
fn meshes(_: &mut Session, _: &Frame, c: &str) -> Result<Vec<MeshStub>> {
    Err(bad(c, "face analysis is not available in this build"))
}

#[cfg(not(feature = "faces"))]
pub struct MeshStub {
    pub points: Vec<[f32; 3]>,
}
#[cfg(not(feature = "faces"))]
impl MeshStub {
    fn bbox(&self) -> [f32; 4] {
        [0.0; 4]
    }
}

/// `(box as fractions, name)` of the faces the photo's People regions name (empty without any).
fn people_of(s: &mut Session, id: PhotoId) -> Vec<([f32; 4], Option<String>)> {
    let Some(p) = s.catalog.photo(id) else { return Vec::new() };
    p.meta.regions
        .iter()
        .filter(|r| r.kind == lightcraft_meta::RegionKind::Face)
        .map(|r| ([r.rect.x0 as f32, r.rect.y0 as f32, r.rect.x1 as f32, r.rect.y1 as f32], r.name.clone()))
        .collect()
}
