//! Eye scores from a face mesh: how open each eye is, and how sharp it is.
//!
//! # Openness
//!
//! The *eye aspect ratio* (Soukupová & Čech, 2016) of the mesh's eyelid points. With the six
//! points of an eye `p1 … p6` (outer corner, two upper-lid points, inner corner, two lower-lid
//! points, going round the eye), `EAR = (|p2 − p6| + |p3 − p5|) / (2 · |p1 − p4|)`: the lid gap
//! over the eye's width. About 0.3 for an open eye, near 0 for a closed one, and it does not
//! change when the head is tilted. MediaPipe face mesh points used (numbers of its canonical
//! 478-point mesh; "left" is the left of the image):
//!
//! | eye | p1 outer | p2 | p3 | p4 inner | p5 | p6 |
//! |---|---|---|---|---|---|---|
//! | left | 33 | 160 | 158 | 133 | 153 | 144 |
//! | right | 263 | 387 | 385 | 362 | 380 | 373 |
//!
//! (The right eye lists its points in the order that pairs each upper-lid point with the
//! lower-lid point under it: 387 ↔ 373 at the outer side, 385 ↔ 380 at the inner side.)
//!
//! # Sharpness
//!
//! The variance of the Laplacian of the luminance on a crop of the eye, sampled at a fixed size
//! (48 × 48 px for a square 1.8 × the eye's width, rotated with the head), so a small eye and a
//! large one are measured on the same scale: a crisp eye (lashes, iris edge, catch-light) has a
//! large variance, a blurred or moved one a small one. The score maps the variance to 0 .. 100
//! (see [`SHARP_SCALE`]).

use crate::mediapipe::{Frame, Mesh, Roi};
use crate::{FaceLandmarker, Result};

/// Mesh points of the left eye of the image: outer corner, two upper-lid points, inner corner,
/// two lower-lid points.
pub const LEFT_EYE: [usize; 6] = [33, 160, 158, 133, 153, 144];
/// The right eye of the image, in the same order.
pub const RIGHT_EYE: [usize; 6] = [263, 387, 385, 362, 380, 373];
/// Nose tip and the outermost face points either side (image left, image right).
const NOSE: usize = 1;
const CHEEKS: [usize; 2] = [234, 454];

/// Side of the sampled eye crop (pixels).
const PATCH: usize = 48;
/// Side of the crop in eye widths.
const PATCH_EYES: f32 = 1.8;
/// Laplacian variance (of luminance 0–1 on the 48 px crop) at which the score reaches 63.
pub const SHARP_SCALE: f32 = 0.004;
/// Faces narrower than this (pixels) are not reported: too small to judge eyes.
pub const MIN_FACE_PX: f32 = 40.0;
/// A face needs at least this detector score, and its mesh may move at most this much (over the
/// face's size) when computed again from its own points: what separates a face from the dessert,
/// ear or dark patch the detector sometimes takes for one. Chosen on the labelled photos of
/// `docs/faces.md`: of 32 detections that were not faces, 6 pass; of 108 that were, 4 are lost
/// (the faintest ones).
pub const MIN_DETECTION: f32 = 0.70;
pub const MAX_DRIFT: f32 = 0.06;

/// One eye.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EyeScore {
    /// Eye aspect ratio (0 closed … ~0.35 wide open).
    pub open: f32,
    /// Sharpness 0 .. 100.
    pub sharpness: f32,
    /// The Laplacian variance the sharpness comes from (for calibration).
    pub detail: f32,
}

/// One face of a photo, in frame pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceScore {
    /// `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    /// The mesh model's face presence score, 0 .. 1.
    pub score: f32,
    /// The left and right eye of the image.
    pub left: EyeScore,
    pub right: EyeScore,
    /// Head turn: −1 (nose against the image-left cheek) … 1; ≈ 0 when facing the camera.
    pub turn: f32,
    /// The detector's score (0 .. 1) for this face.
    pub detection: f32,
    /// Outer eye corners' distance over the cheek-to-cheek width: ≈ 0.7 on a face seen from the
    /// front (the mesh of an ear or a blob has other proportions).
    pub proportion: f32,
    /// See [`Mesh::drift`] and [`Mesh::agreement`].
    pub drift: f32,
    pub agreement: f32,
    /// How many detector windows found the face.
    pub votes: usize,
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Eye aspect ratio of the eye with mesh points `idx` (see the module docs).
pub fn eye_aspect_ratio(m: &Mesh, idx: [usize; 6]) -> Option<f32> {
    let p = |k: usize| m.xy(idx[k]);
    let (p1, p2, p3, p4, p5, p6) = (p(0)?, p(1)?, p(2)?, p(3)?, p(4)?, p(5)?);
    let w = dist(p1, p4);
    (w > 1e-3).then(|| (dist(p2, p6) + dist(p3, p5)) / (2.0 * w))
}

/// Luminance (0–1, of the encoded values) of a pixel.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The luminance of `roi` sampled at `n × n` (each output pixel averages `k × k` bilinear
/// samples, so shrinking a large crop does not alias).
fn patch(f: Frame, roi: &Roi, n: usize) -> Vec<f32> {
    let k = ((roi.size / n as f32).ceil() as usize).clamp(1, 6);
    let at = |x: f32, y: f32| -> f32 {
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let px = |xi: i64, yi: i64| -> f32 {
            let (xc, yc) = (xi.clamp(0, f.w as i64 - 1) as usize, yi.clamp(0, f.h as i64 - 1) as usize);
            f.rgb.get(yc * f.w + xc).map(|c| luma(*c)).unwrap_or(0.0)
        };
        let (x0, y0) = (x0 as i64, y0 as i64);
        (px(x0, y0) * (1.0 - fx) + px(x0 + 1, y0) * fx) * (1.0 - fy) + (px(x0, y0 + 1) * (1.0 - fx) + px(x0 + 1, y0 + 1) * fx) * fy
    };
    let mut out = vec![0.0f32; n * n];
    for j in 0..n {
        for i in 0..n {
            let mut acc = 0.0;
            for sj in 0..k {
                for si in 0..k {
                    let u = (i as f32 + (si as f32 + 0.5) / k as f32) / n as f32;
                    let v = (j as f32 + (sj as f32 + 0.5) / k as f32) / n as f32;
                    let p = roi.to_frame(u, v);
                    acc += at(p[0], p[1]);
                }
            }
            out[j * n + i] = acc / (k * k) as f32;
        }
    }
    out
}

/// Variance of the 4-neighbour Laplacian over the inside of an `n × n` luminance patch.
pub fn laplacian_variance(l: &[f32], n: usize) -> f32 {
    if n < 3 || l.len() < n * n {
        return 0.0;
    }
    let (mut sum, mut sum2, mut count) = (0.0f64, 0.0f64, 0usize);
    for y in 1..n - 1 {
        for x in 1..n - 1 {
            let v = (4.0 * l[y * n + x] - l[y * n + x - 1] - l[y * n + x + 1] - l[(y - 1) * n + x] - l[(y + 1) * n + x]) as f64;
            sum += v;
            sum2 += v * v;
            count += 1;
        }
    }
    let c = count.max(1) as f64;
    ((sum2 / c) - (sum / c).powi(2)).max(0.0) as f32
}

/// Variance of the Laplacian of the eye with mesh points `idx`, tilted by `angle` radians (see
/// the module docs).
pub fn eye_detail(f: Frame, m: &Mesh, idx: [usize; 6], angle: f32) -> Option<f32> {
    let (a, b) = (m.xy(idx[0])?, m.xy(idx[3])?);
    let side = dist(a, b) * PATCH_EYES;
    if side < 4.0 {
        return None;
    }
    let pts: Vec<[f32; 2]> = idx.iter().filter_map(|&i| m.xy(i)).collect();
    let n = pts.len().max(1) as f32;
    let center = [pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[1]).sum::<f32>() / n];
    let l = patch(f, &Roi { center, size: side, angle }, PATCH);
    Some(laplacian_variance(&l, PATCH))
}

/// Sharpness 0 .. 100 from the Laplacian variance of an eye.
pub fn sharpness_score(detail: f32) -> f32 {
    100.0 * (1.0 - (-detail / SHARP_SCALE).exp())
}

/// The scores of one face mesh; `None` when it is too small or the mesh is incomplete.
pub fn score_face(f: Frame, m: &Mesh) -> Option<FaceScore> {
    let bbox = m.bbox();
    if (bbox[2] - bbox[0]).max(bbox[3] - bbox[1]) < MIN_FACE_PX || m.detection < MIN_DETECTION || m.drift > MAX_DRIFT {
        return None;
    }
    let (a, b) = (m.xy(33)?, m.xy(263)?);
    // the head's roll: the direction of the line through the outer eye corners
    let angle = (b[1] - a[1]).atan2(b[0] - a[0]);
    let eye = |idx: [usize; 6]| -> Option<EyeScore> {
        let detail = eye_detail(f, m, idx, angle)?;
        Some(EyeScore { open: eye_aspect_ratio(m, idx)?, sharpness: sharpness_score(detail), detail })
    };
    let turn = match (m.xy(NOSE), m.xy(CHEEKS[0]), m.xy(CHEEKS[1])) {
        (Some(n), Some(l), Some(r)) => {
            let (dl, dr) = (dist(n, l), dist(n, r));
            if dl + dr > 1e-3 { (dl - dr) / (dl + dr) } else { 0.0 }
        }
        _ => 0.0,
    };
    let proportion = match (m.xy(CHEEKS[0]), m.xy(CHEEKS[1])) {
        (Some(l), Some(r)) if dist(l, r) > 1e-3 => dist(a, b) / dist(l, r),
        _ => 0.0,
    };
    Some(FaceScore {
        bbox,
        score: m.score,
        left: eye(LEFT_EYE)?,
        right: eye(RIGHT_EYE)?,
        turn,
        detection: m.detection,
        proportion,
        drift: m.drift,
        agreement: m.agreement,
        votes: m.votes,
    })
}

/// Find the faces of a photo (sRGB, 0–1) and score their eyes, best face first.
pub fn analyze(model: &FaceLandmarker, f: Frame) -> Result<Vec<FaceScore>> {
    Ok(model.find_all(f)?.iter().filter_map(|m| score_face(f, m)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mesh with one eye of width 30 whose lids are `gap` apart (the other eye mirrored).
    fn mesh(gap: f32) -> Mesh {
        let mut pts = vec![[0.0f32; 3]; 478];
        let mut set = |i: usize, x: f32, y: f32| pts[i] = [x, y, 0.0];
        // left eye: corners at x 0 and 30, upper lid at y −gap/2, lower at +gap/2
        set(33, 0.0, 0.0);
        set(133, 30.0, 0.0);
        set(160, 10.0, -gap / 2.0);
        set(158, 20.0, -gap / 2.0);
        set(144, 10.0, gap / 2.0);
        set(153, 20.0, gap / 2.0);
        // right eye
        set(263, 100.0, 0.0);
        set(362, 70.0, 0.0);
        set(387, 90.0, -gap / 2.0);
        set(385, 80.0, -gap / 2.0);
        set(373, 90.0, gap / 2.0);
        set(380, 80.0, gap / 2.0);
        Mesh { points: pts, score: 1.0, ..Default::default() }
    }

    #[test]
    fn aspect_ratio_is_lid_gap_over_eye_width() {
        let open = mesh(10.0);
        assert!((eye_aspect_ratio(&open, LEFT_EYE).unwrap() - 10.0 / 30.0).abs() < 1e-5);
        assert!((eye_aspect_ratio(&open, RIGHT_EYE).unwrap() - 10.0 / 30.0).abs() < 1e-5);
        assert!(eye_aspect_ratio(&mesh(0.0), LEFT_EYE).unwrap() < 1e-6);
        assert!(eye_aspect_ratio(&Mesh::default(), LEFT_EYE).is_none());
    }

    #[test]
    fn laplacian_variance_drops_when_the_patch_is_blurred() {
        let n = 48;
        let mut s = 12345u32;
        let noise: Vec<f32> = (0..n * n)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1u32 << 24) as f32
            })
            .collect();
        let blur: Vec<f32> = (0..n * n)
            .map(|i| {
                let (x, y) = (i % n, i / n);
                let mut a = 0.0;
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let (xx, yy) = ((x as i32 + dx).clamp(0, n as i32 - 1) as usize, (y as i32 + dy).clamp(0, n as i32 - 1) as usize);
                        a += noise[yy * n + xx];
                    }
                }
                a / 25.0
            })
            .collect();
        assert!(laplacian_variance(&noise, n) > 20.0 * laplacian_variance(&blur, n));
        assert_eq!(laplacian_variance(&[0.5; 48 * 48], n), 0.0);
    }

    #[test]
    fn patch_resamples_a_ramp() {
        let (w, h) = (96, 96);
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|i| [(i % w) as f32 / w as f32; 3]).collect();
        let f = Frame { rgb: &rgb, w, h };
        let p = patch(f, &Roi { center: [48.0, 48.0], size: 96.0, angle: 0.0 }, 16);
        assert!(p[0] < p[15] && (p[7] - 0.5).abs() < 0.05);
    }
}
