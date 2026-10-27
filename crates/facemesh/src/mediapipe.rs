//! MediaPipe Face Landmarker (Google LLC, Apache-2.0) in plain Rust: the official
//! `face_landmarker.task` bundle's BlazeFace short-range detector and Face Mesh V2 (478 points)
//! run on the [`tflite`](crate::tflite) interpreter.
//!
//! The pipeline follows MediaPipe's published face landmarker graph (Apache-2.0): the detector
//! sees a 128×128 crop scaled to −1…1 and decodes 896 SSD anchors (strides 8, 16, 16, 16; two
//! anchors per cell per layer) with weighted non-maximum suppression; the mesh sees a 256×256
//! crop (0…1) of a square 1.5× the face, rotated so the eyes are level (detector keypoints 0 → 1,
//! then mesh points 33 → 263), and reports a presence score. Adapted from EffectCraft's
//! `effectcraft-segment` (MIT OR Apache-2.0).
//!
//! The short-range detector only finds faces that fill a good part of its 128 px window, so
//! [`FaceLandmarker::find_all`] runs it over windows of several sizes (the whole frame, then
//! smaller, overlapping ones) to find the small faces of a group photo too.

use crate::Result;
use crate::tflite::Model;
use crate::zip::zip_file;

pub const DETECTOR: &str = "face_detector.tflite";
pub const MESH: &str = "face_landmarks_detector.tflite";

const DET_SIZE: usize = 128;
const MESH_SIZE: usize = 256;
pub const MESH_POINTS: usize = 478;
/// Detection and presence thresholds (MediaPipe's defaults).
pub const MIN_SCORE: f32 = 0.5;
const NMS_IOU: f32 = 0.3;
/// Crops are this much larger than the face.
const ROI_SCALE: f32 = 1.5;
/// Mesh points whose direction levels the crop: the outer eye corners.
const EYE_LINE: [usize; 2] = [33, 263];
/// Most faces kept per frame.
const MAX_FACES: usize = 40;
/// Most detector windows per frame (each costs a few milliseconds).
const MAX_WINDOWS: usize = 96;

/// A frame: straight sRGB 0–1, row-major `w×h`.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub rgb: &'a [[f32; 3]],
    pub w: usize,
    pub h: usize,
}

/// A rotated square region of a frame: centre, side and angle (radians, the direction of the
/// square's x axis in the frame, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Roi {
    pub center: [f32; 2],
    pub size: f32,
    pub angle: f32,
}

impl Roi {
    /// Frame position of the normalised square position `(u, v)` (0–1).
    pub fn to_frame(&self, u: f32, v: f32) -> [f32; 2] {
        let (s, c) = self.angle.sin_cos();
        let (x, y) = ((u - 0.5) * self.size, (v - 0.5) * self.size);
        [self.center[0] + c * x - s * y, self.center[1] + s * x + c * y]
    }
}

/// Sample `roi` into an `n×n` RGB tensor (bilinear; zero outside the frame), each channel mapped
/// from 0–1 to `lo`–`hi`.
pub fn crop(f: Frame, roi: &Roi, n: usize, [lo, hi]: [f32; 2]) -> Vec<f32> {
    let (rgb, w, h) = (f.rgb, f.w, f.h);
    let mut out = vec![0.0f32; n * n * 3];
    if rgb.len() < w * h || n == 0 {
        return out;
    }
    let px = |x: i64, y: i64| -> [f32; 3] {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 { [0.0; 3] } else { rgb.get(y as usize * w + x as usize).copied().unwrap_or([0.0; 3]) }
    };
    for j in 0..n {
        for i in 0..n {
            let p = roi.to_frame((i as f32 + 0.5) / n as f32, (j as f32 + 0.5) / n as f32);
            let (x, y) = (p[0] - 0.5, p[1] - 0.5);
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
            for k in 0..3 {
                let v = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
                out[(j * n + i) * 3 + k] = lo + v * (hi - lo);
            }
        }
    }
    out
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// A detected face: box `[x0, y0, x1, y1]`, six keypoints (eyes, nose tip, mouth, ears; image
/// left first) and score, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub bbox: [f32; 4],
    pub keypoints: [[f32; 2]; 6],
    pub score: f32,
    /// The detector window it came from, and (after merging) how many different windows saw it.
    pub window: usize,
    pub votes: usize,
}

/// A face mesh: 478 points (frame pixels, `z` relative to the face, scaled like the width) and
/// the presence score.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub points: Vec<[f32; 3]>,
    pub score: f32,
    /// The detector's score for the detection this mesh was made from (0 when not from one).
    pub detection: f32,
    /// How far the mesh moved when it was computed again from its own points, over the face's
    /// size (≈ 0.01 on a face; a mesh fitted to a non-face wanders).
    pub drift: f32,
    /// Overlap (IoU) of the mesh's box with the detector's.
    pub agreement: f32,
    /// How many detector windows found this face.
    pub votes: usize,
}

impl Mesh {
    pub fn xy(&self, i: usize) -> Option<[f32; 2]> {
        self.points.get(i).map(|p| [p[0], p[1]])
    }

    /// The smallest box holding all points.
    pub fn bbox(&self) -> [f32; 4] {
        self.points.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, p| {
            [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
        })
    }
}

pub struct FaceLandmarker {
    detector: Model,
    mesh: Model,
    /// Anchor centres (normalised).
    anchors: Vec<[f32; 2]>,
}

/// SSD anchor centres: per stride group, `fm×fm` cells with two anchors per layer each.
fn anchors() -> Vec<[f32; 2]> {
    let mut out = vec![];
    for (stride, layers) in [(8usize, 1usize), (16, 3)] {
        let fm = DET_SIZE.div_ceil(stride);
        for y in 0..fm {
            for x in 0..fm {
                for _ in 0..2 * layers {
                    out.push([(x as f32 + 0.5) / fm as f32, (y as f32 + 0.5) / fm as f32]);
                }
            }
        }
    }
    out
}

pub fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let iw = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let ih = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// Weighted non-maximum suppression: each cluster of overlapping detections becomes their
/// score-weighted average, with the best score.
fn weighted_nms(mut dets: Vec<Detection>) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out = vec![];
    while let Some(top) = dets.first().copied() {
        let (cluster, rest): (Vec<Detection>, Vec<Detection>) = dets.into_iter().partition(|d| iou(&d.bbox, &top.bbox) > NMS_IOU);
        dets = rest;
        let total: f32 = cluster.iter().map(|d| d.score).sum();
        if total <= 0.0 {
            out.push(top);
            continue;
        }
        let mut windows: Vec<usize> = cluster.iter().map(|d| d.window).collect();
        windows.sort_unstable();
        windows.dedup();
        let mut avg = Detection { bbox: [0.0; 4], keypoints: [[0.0; 2]; 6], score: top.score, window: top.window, votes: windows.len() };
        for d in &cluster {
            let k = d.score / total;
            for i in 0..4 {
                avg.bbox[i] += d.bbox[i] * k;
            }
            for (a, p) in avg.keypoints.iter_mut().zip(&d.keypoints) {
                a[0] += p[0] * k;
                a[1] += p[1] * k;
            }
        }
        out.push(avg);
    }
    out
}

/// The crop a face's mesh is computed in: `bbox` grown to a square 1.5× its long side, turned so
/// `from → to` is level.
fn roi_of(bbox: [f32; 4], from: [f32; 2], to: [f32; 2]) -> Roi {
    Roi {
        center: [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0],
        size: (bbox[2] - bbox[0]).max(bbox[3] - bbox[1]) * ROI_SCALE,
        angle: (to[1] - from[1]).atan2(to[0] - from[0]),
    }
}

/// The detector windows for a `w×h` frame: the whole frame (zero-padded to a square), then
/// squares of 0.6 and 0.3 of the short side sliding over it with a third of overlap.
fn windows(w: usize, h: usize) -> Vec<Roi> {
    let (wf, hf) = (w as f32, h as f32);
    let (long, short) = (wf.max(hf), wf.min(hf));
    let mut out = vec![Roi { center: [wf / 2.0, hf / 2.0], size: long, angle: 0.0 }];
    for side in [short * 0.6, short * 0.3] {
        if side < 48.0 {
            continue;
        }
        let stride = side * 0.67;
        let (nx, ny) = (((wf - side) / stride).ceil().max(0.0) as usize + 1, ((hf - side) / stride).ceil().max(0.0) as usize + 1);
        for j in 0..ny {
            for i in 0..nx {
                let x0 = if nx > 1 { (wf - side) * i as f32 / (nx - 1) as f32 } else { (wf - side) / 2.0 };
                let y0 = if ny > 1 { (hf - side) * j as f32 / (ny - 1) as f32 } else { (hf - side) / 2.0 };
                out.push(Roi { center: [x0 + side / 2.0, y0 + side / 2.0], size: side, angle: 0.0 });
            }
        }
    }
    out.truncate(MAX_WINDOWS);
    out
}

impl FaceLandmarker {
    /// From the official `face_landmarker.task` bundle (a zip archive of `.tflite` models).
    pub fn from_task(bytes: &[u8]) -> Result<FaceLandmarker> {
        let detector = Model::read(&zip_file(bytes, DETECTOR)?)?;
        let mesh = Model::read(&zip_file(bytes, MESH)?)?;
        let anchors = anchors();
        if detector.input_shape(0) != Some(&[1, DET_SIZE, DET_SIZE, 3]) || detector.output(0).map(|o| o.0) != Some(&[1, anchors.len(), 16]) {
            return Err("unexpected face detector".into());
        }
        if mesh.input_shape(0) != Some(&[1, MESH_SIZE, MESH_SIZE, 3])
            || mesh.output(0).map(|o| o.0.iter().product::<usize>()) != Some(MESH_POINTS * 3)
        {
            return Err("unexpected face mesh".into());
        }
        Ok(FaceLandmarker { detector, mesh, anchors })
    }

    /// Faces in the square `roi` of a frame (frame pixels), best first.
    pub fn detect(&self, f: Frame, roi: &Roi) -> Result<Vec<Detection>> {
        Ok(weighted_nms(self.detect_raw(f, roi)?))
    }

    fn detect_raw(&self, f: Frame, roi: &Roi) -> Result<Vec<Detection>> {
        let x = crop(f, roi, DET_SIZE, [-1.0, 1.0]);
        let out = self.detector.run(&[&x])?;
        let (Some(reg), Some(cls)) = (out.first(), out.get(1)) else { return Err("face detector: missing outputs".into()) };
        let s = DET_SIZE as f32;
        let mut dets = vec![];
        for (i, a) in self.anchors.iter().enumerate() {
            let (Some(r), Some(&c)) = (reg.get(i * 16..i * 16 + 16), cls.get(i)) else { break };
            let score = sigmoid(c.clamp(-100.0, 100.0));
            if score < MIN_SCORE {
                continue;
            }
            let (cx, cy, bw, bh) = (r[0] / s + a[0], r[1] / s + a[1], r[2] / s, r[3] / s);
            let to = |u: f32, v: f32| roi.to_frame(u, v);
            let corners = [
                to(cx - bw / 2.0, cy - bh / 2.0),
                to(cx + bw / 2.0, cy - bh / 2.0),
                to(cx - bw / 2.0, cy + bh / 2.0),
                to(cx + bw / 2.0, cy + bh / 2.0),
            ];
            let mut keypoints = [[0.0f32; 2]; 6];
            for (k, p) in keypoints.iter_mut().enumerate() {
                *p = to(r[4 + 2 * k] / s + a[0], r[5 + 2 * k] / s + a[1]);
            }
            let bbox = corners.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            });
            dets.push(Detection { bbox, keypoints, score, window: 0, votes: 1 });
        }
        Ok(dets)
    }

    /// The face mesh in `roi`: 478 points (frame pixels) and the presence score.
    pub fn mesh(&self, f: Frame, roi: &Roi) -> Result<Mesh> {
        let x = crop(f, roi, MESH_SIZE, [0.0, 1.0]);
        let out = self.mesh.run(&[&x])?;
        let (Some(pts), Some(flag)) = (out.first(), out.get(1).and_then(|f| f.first())) else { return Err("face mesh: missing outputs".into()) };
        let s = MESH_SIZE as f32;
        let points = pts
            .as_chunks::<3>()
            .0
            .iter()
            .take(MESH_POINTS)
            .map(|p| {
                let q = roi.to_frame(p[0] / s, p[1] / s);
                [q[0], q[1], p[2] / s * roi.size]
            })
            .collect();
        Ok(Mesh { points, score: sigmoid(*flag), detection: 0.0, drift: 0.0, agreement: 0.0, votes: 0 })
    }

    /// The crop that frames a mesh found earlier.
    fn roi_of_mesh(m: &Mesh) -> Option<Roi> {
        let (a, b) = (m.xy(EYE_LINE[0])?, m.xy(EYE_LINE[1])?);
        Some(roi_of(m.bbox(), a, b))
    }

    /// The mesh from `roi`, refined once from its own points (a detector box is coarse).
    fn mesh_refined(&self, f: Frame, roi: &Roi) -> Result<Option<Mesh>> {
        let first = self.mesh(f, roi)?;
        if first.score < MIN_SCORE {
            return Ok(None);
        }
        let Some(r) = Self::roi_of_mesh(&first) else { return Ok(None) };
        let refined = self.mesh(f, &r)?;
        let mut m = if refined.score >= MIN_SCORE { refined } else { first.clone() };
        let b = m.bbox();
        let size = (b[2] - b[0]).max(b[3] - b[1]).max(1e-3);
        let n = m.points.len().max(1) as f32;
        m.drift = m.points.iter().zip(&first.points).map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()).sum::<f32>() / n / size;
        Ok(Some(m))
    }

    /// Every face of the frame, best first: the detector over windows of several sizes, then each
    /// face's mesh. Duplicates (one face seen by several windows) are merged.
    pub fn find_all(&self, f: Frame) -> Result<Vec<Mesh>> {
        if f.w < 16 || f.h < 16 || f.rgb.len() < f.w * f.h {
            return Ok(vec![]);
        }
        let mut dets = vec![];
        for (w, roi) in windows(f.w, f.h).iter().enumerate() {
            dets.extend(self.detect_raw(f, roi)?.into_iter().map(|d| Detection { window: w, ..d }));
        }
        let mut dets = weighted_nms(dets);
        dets.truncate(MAX_FACES);
        let mut meshes: Vec<Mesh> = vec![];
        for d in dets {
            let Some(mut m) = self.mesh_refined(f, &roi_of(d.bbox, d.keypoints[0], d.keypoints[1]))? else { continue };
            m.detection = d.score;
            m.agreement = iou(&d.bbox, &m.bbox());
            m.votes = d.votes;
            let b = m.bbox();
            // the same face again (through another detection)? keep the more confident mesh
            let dup = meshes.iter().position(|o| {
                let ob = o.bbox();
                let inter = (b[2].min(ob[2]) - b[0].max(ob[0])).max(0.0) * (b[3].min(ob[3]) - b[1].max(ob[1])).max(0.0);
                let smaller = ((b[2] - b[0]) * (b[3] - b[1])).min((ob[2] - ob[0]) * (ob[3] - ob[1])).max(1e-6);
                inter / smaller > 0.6
            });
            match dup {
                Some(i) => {
                    if meshes[i].score < m.score {
                        meshes[i] = m;
                    }
                }
                None => meshes.push(m),
            }
        }
        meshes.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(meshes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_the_detector_layout() {
        let a = anchors();
        assert_eq!(a.len(), 896);
        // Stride 8: 16×16 cells, two anchors each; then stride 16: 8×8 cells, six each.
        assert_eq!(a[0], [0.5 / 16.0, 0.5 / 16.0]);
        assert_eq!(a[1], a[0]);
        assert_eq!(a[2], [1.5 / 16.0, 0.5 / 16.0]);
        assert_eq!(a[512], [0.5 / 8.0, 0.5 / 8.0]);
        assert_eq!(a[517], a[512]);
        assert_eq!(a[518], [1.5 / 8.0, 0.5 / 8.0]);
    }

    #[test]
    fn nms_averages_overlapping_detections() {
        let d = |x: f32, s: f32| Detection { bbox: [x, 0.0, x + 10.0, 10.0], keypoints: [[x, 0.0]; 6], score: s, window: 0, votes: 1 };
        let out = weighted_nms(vec![d(0.0, 0.9), d(1.0, 0.9), d(50.0, 0.6)]);
        assert_eq!(out.len(), 2);
        assert!((out[0].bbox[0] - 0.5).abs() < 1e-5 && out[0].score == 0.9);
        assert_eq!(out[1].bbox[0], 50.0);
    }

    #[test]
    fn roi_maps_corners_and_rotates() {
        let r = Roi { center: [10.0, 20.0], size: 4.0, angle: 0.0 };
        assert_eq!(r.to_frame(0.0, 0.0), [8.0, 18.0]);
        assert_eq!(r.to_frame(1.0, 1.0), [12.0, 22.0]);
        let q = Roi { angle: std::f32::consts::FRAC_PI_2, ..r };
        let p = q.to_frame(1.0, 0.5);
        assert!((p[0] - 10.0).abs() < 1e-5 && (p[1] - 22.0).abs() < 1e-5);
    }

    #[test]
    fn windows_cover_the_frame_and_stay_inside_it() {
        let ws = windows(2560, 1707);
        assert!(ws.len() > 10 && ws.len() <= MAX_WINDOWS, "{}", ws.len());
        for r in &ws[1..] {
            assert!(r.center[0] - r.size / 2.0 >= -0.5 && r.center[0] + r.size / 2.0 <= 2560.5);
            assert!(r.center[1] - r.size / 2.0 >= -0.5 && r.center[1] + r.size / 2.0 <= 1707.5);
        }
        // a tiny frame gets only the whole-frame window
        assert_eq!(windows(40, 30).len(), 1);
    }

    /// The official bundle (`LIGHTCRAFT_FACE_LANDMARKER` = path to `face_landmarker.task`) on a
    /// fixed input matches TensorFlow Lite's own interpreter (`ai-edge-litert` 2.2.0, run once
    /// to record these sums, as EffectCraft did).
    #[test]
    fn matches_tensorflow_lite_on_the_official_weights() {
        let Some(path) = std::env::var_os("LIGHTCRAFT_FACE_LANDMARKER") else { return };
        let Ok(bytes) = std::fs::read(path) else { return };
        let Ok(m) = FaceLandmarker::from_task(&bytes) else { panic!("the bundle loads") };
        let input = |n: usize| -> Vec<f32> { (0..n).map(|i| ((i as f32) * 0.013).sin() * 0.5).collect() };
        let close = |got: &[f32], want: f64| {
            let s: f64 = got.iter().map(|v| *v as f64).sum();
            assert!((s - want).abs() <= 1e-5 * want.abs().max(1.0), "{s} vs {want}");
        };
        let Ok(d) = m.detector.run(&[&input(DET_SIZE * DET_SIZE * 3)]) else { panic!("detector runs") };
        close(&d[0], 54_202.040_273);
        close(&d[1], -3_061.866_384);
        let Ok(y) = m.mesh.run(&[&input(MESH_SIZE * MESH_SIZE * 3)]) else { panic!("mesh runs") };
        close(&y[0], 119_864.749_257);
        close(&y[1], -19.777_233);
    }
}
