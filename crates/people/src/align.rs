//! Face alignment for ArcFace: the five landmarks of a face (eye centres, nose tip, mouth
//! corners) are mapped by the best similarity transform (rotation, uniform scale, translation)
//! onto the fixed positions of the ArcFace training crops, and the 112 × 112 crop is sampled
//! from the photo.
//!
//! The target positions are the ones InsightFace published for its 112 × 112 crops (a list of
//! ten numbers; MIT-licensed code, the numbers are data). The transform is the closed-form
//! least-squares fit of a 2-D similarity (Umeyama, 1991).

use lightcraft_faces::mediapipe::{Frame, Mesh};

/// Side of the aligned crop.
pub const SIDE: usize = 112;

/// Where the landmarks go in the crop: left eye, right eye, nose tip, left and right mouth
/// corner (left = left of the image).
pub const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

/// Face Mesh points: the iris centres (image-left and image-right eye are told apart by position),
/// the eyes' corners when the mesh has no iris, the nose tip and the mouth corners.
const IRIS: [usize; 2] = [468, 473];
const EYE_CORNERS: [[usize; 2]; 2] = [[33, 133], [362, 263]];
const NOSE_TIP: usize = 1;
const MOUTH: [usize; 2] = [61, 291];

/// The five ArcFace landmarks of a face mesh (frame pixels), `None` when the mesh is incomplete.
pub fn landmarks5(m: &Mesh) -> Option<[[f32; 2]; 5]> {
    let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let mut eyes = [0, 1].map(|k| m.xy(IRIS[k]).or_else(|| Some(mid(m.xy(EYE_CORNERS[k][0])?, m.xy(EYE_CORNERS[k][1])?))));
    let mut mouth = [m.xy(MOUTH[0])?, m.xy(MOUTH[1])?];
    let nose = m.xy(NOSE_TIP)?;
    let mut e = [eyes[0].take()?, eyes[1].take()?];
    // "left" is the left of the image, whichever way the mesh numbers its points
    if e[0][0] > e[1][0] {
        e.swap(0, 1);
    }
    if mouth[0][0] > mouth[1][0] {
        mouth.swap(0, 1);
    }
    Some([e[0], e[1], nose, mouth[0], mouth[1]])
}

/// The similarity `x' = a·x − b·y + tx`, `y' = b·x + a·y + ty` that best maps `src` onto `dst`
/// (least squares), as `[a, b, tx, ty]`.
pub fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [f32; 4] {
    let n = 5.0f32;
    let mean = |p: &[[f32; 2]; 5]| [p.iter().map(|q| q[0]).sum::<f32>() / n, p.iter().map(|q| q[1]).sum::<f32>() / n];
    let (ms, md) = (mean(src), mean(dst));
    let (mut num_a, mut num_b, mut den) = (0.0f32, 0.0f32, 0.0f32);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy, dx, dy) = (s[0] - ms[0], s[1] - ms[1], d[0] - md[0], d[1] - md[1]);
        num_a += sx * dx + sy * dy;
        num_b += sx * dy - sy * dx;
        den += sx * sx + sy * sy;
    }
    if den < 1e-9 {
        return [1.0, 0.0, md[0] - ms[0], md[1] - ms[1]];
    }
    let (a, b) = (num_a / den, num_b / den);
    [a, b, md[0] - (a * ms[0] - b * ms[1]), md[1] - (b * ms[0] + a * ms[1])]
}

/// Sample the aligned crop: RGB, channel-first (`3 × 112 × 112`), values `v · 2 − 1` (the
/// network's `(x − 127.5) / 127.5` on 8-bit values). Each output pixel averages `k × k` bilinear
/// samples (`k` grows with the shrink factor, so a large face is not aliased); pixels outside
/// the frame are black, as in the training crops.
pub fn crop(f: Frame, landmarks: &[[f32; 2]; 5]) -> Vec<f32> {
    let [a, b, tx, ty] = similarity(landmarks, &TEMPLATE);
    let s2 = (a * a + b * b).max(1e-12);
    // frame position of crop pixel (u, v): the inverse similarity
    let inv = |u: f32, v: f32| -> [f32; 2] {
        let (du, dv) = (u - tx, v - ty);
        [(a * du + b * dv) / s2, (-b * du + a * dv) / s2]
    };
    let k = ((1.0 / s2.sqrt()).ceil() as usize).clamp(1, 4);
    let px = |x: i64, y: i64| -> [f32; 3] {
        if x < 0 || y < 0 || x >= f.w as i64 || y >= f.h as i64 {
            [0.0; 3]
        } else {
            f.rgb.get(y as usize * f.w + x as usize).copied().unwrap_or([0.0; 3])
        }
    };
    let n = SIDE * SIDE;
    let mut out = vec![0.0f32; 3 * n];
    for j in 0..SIDE {
        for i in 0..SIDE {
            let mut acc = [0.0f32; 3];
            for sj in 0..k {
                for si in 0..k {
                    let u = i as f32 + (si as f32 + 0.5) / k as f32;
                    let v = j as f32 + (sj as f32 + 0.5) / k as f32;
                    let p = inv(u, v);
                    let (x, y) = (p[0] - 0.5, p[1] - 0.5);
                    let (x0, y0) = (x.floor(), y.floor());
                    let (fx, fy) = (x - x0, y - y0);
                    let (x0, y0) = (x0 as i64, y0 as i64);
                    let (c00, c10, c01, c11) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
                    for c in 0..3 {
                        acc[c] += (c00[c] * (1.0 - fx) + c10[c] * fx) * (1.0 - fy) + (c01[c] * (1.0 - fx) + c11[c] * fx) * fy;
                    }
                }
            }
            let kk = (k * k) as f32;
            for c in 0..3 {
                out[c * n + j * SIDE + i] = acc[c] / kk * 2.0 - 1.0;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_maps_to_itself_and_a_scaled_rotated_copy_is_recovered() {
        let [a, b, tx, ty] = similarity(&TEMPLATE, &TEMPLATE);
        assert!((a - 1.0).abs() < 1e-5 && b.abs() < 1e-5 && tx.abs() < 1e-3 && ty.abs() < 1e-3);
        // the template turned by 20°, scaled by 3 and moved: the fit sends it back
        let (s, c) = (20f32.to_radians().sin() * 3.0, 20f32.to_radians().cos() * 3.0);
        let moved: [[f32; 2]; 5] = TEMPLATE.map(|p| [c * p[0] - s * p[1] + 500.0, s * p[0] + c * p[1] + 300.0]);
        let [a, b, tx, ty] = similarity(&moved, &TEMPLATE);
        for (m, t) in moved.iter().zip(&TEMPLATE) {
            let (x, y) = (a * m[0] - b * m[1] + tx, b * m[0] + a * m[1] + ty);
            assert!((x - t[0]).abs() < 1e-2 && (y - t[1]).abs() < 1e-2, "{x} {y} vs {t:?}");
        }
    }

    #[test]
    fn the_crop_of_a_gradient_follows_the_transform() {
        // a frame whose red channel is x / 100: the crop's red must grow from left to right
        let (w, h) = (200usize, 200usize);
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|i| [(i % w) as f32 / w as f32, 0.5, 0.5]).collect();
        let lm = TEMPLATE.map(|p| [p[0] + 40.0, p[1] + 40.0]);
        let c = crop(Frame { rgb: &rgb, w, h }, &lm);
        let n = SIDE * SIDE;
        assert_eq!(c.len(), 3 * n);
        let row = 56 * SIDE;
        assert!(c[row + 10] < c[row + 100]);
        // identity placement: the crop's pixel (30, 56) is the frame's (70, 96)
        let want = (70.0 + 0.5) / w as f32 * 2.0 - 1.0;
        assert!((c[row + 30] - want).abs() < 0.02, "{} vs {want}", c[row + 30]);
    }
}
