//! Faces of a photo ready to be recognized: the Face Landmarker meshes (from `lightcraft-faces`)
//! that really are faces, each with its five alignment landmarks, box, and what says how good
//! a picture of the person it is (size, head turn).

use lightcraft_faces::FaceLandmarker;
use lightcraft_faces::eyes::{MAX_DRIFT, MIN_DETECTION};
use lightcraft_faces::mediapipe::{Frame, Mesh};

use crate::align::landmarks5;

/// Faces narrower than this (pixels of the analysed picture) are not recognized: ArcFace is
/// unreliable on them and they would make groups of their own.
pub const MIN_FACE_PX: f32 = 40.0;
/// Mesh points at the face's outer cheeks and nose tip (for the head turn).
const CHEEKS: [usize; 2] = [234, 454];
const NOSE: usize = 1;

/// A face found in a photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detected {
    /// `[x0, y0, x1, y1]` in pixels of the analysed picture.
    pub bbox: [f32; 4],
    pub landmarks: [[f32; 2]; 5],
    /// The detector's score, 0 – 1.
    pub det: f32,
    /// Long side of the box (pixels).
    pub size: f32,
    /// Head turn: −1 … 1, ≈ 0 facing the camera.
    pub turn: f32,
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn turn_of(m: &Mesh) -> f32 {
    match (m.xy(NOSE), m.xy(CHEEKS[0]), m.xy(CHEEKS[1])) {
        (Some(n), Some(l), Some(r)) => {
            let (dl, dr) = (dist(n, l), dist(n, r));
            if dl + dr > 1e-3 { (dl - dr) / (dl + dr) } else { 0.0 }
        }
        _ => 0.0,
    }
}

/// The recognizable faces of `f`, best first.
pub fn faces_of(model: &FaceLandmarker, f: Frame) -> crate::Result<Vec<Detected>> {
    let meshes = model.find_all(f).map_err(crate::Error::Detect)?;
    Ok(meshes
        .iter()
        .filter(|m| m.detection >= MIN_DETECTION && m.drift <= MAX_DRIFT)
        .filter_map(|m| {
            let b = m.bbox();
            let size = (b[2] - b[0]).max(b[3] - b[1]);
            (size >= MIN_FACE_PX).then_some(())?;
            Some(Detected { bbox: b, landmarks: landmarks5(m)?, det: m.detection, size, turn: turn_of(m) })
        })
        .collect())
}
