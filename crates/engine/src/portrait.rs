//! Portrait retouching geometry: masks for the skin, the eyes and the teeth of a face, drawn from
//! the 478-point face mesh (MediaPipe Face Mesh V2, the same model Analyze Faces uses).
//!
//! The retouch itself is the existing local adjustments (texture / clarity down on the skin,
//! exposure / contrast up on the eyes…) applied through these masks, so it stays editable and
//! non-destructive. Here: the index lists of the mesh's regions, polygon filling and the
//! conversion to the stored logits of a [`lightcraft_develop::SegMask`]. Pure functions, tested
//! without a model.

/// Face oval, clockwise from the forehead.
pub const OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234,
    127, 162, 21, 54, 103, 67, 109,
];
/// The person's left eye (the viewer's right) and right eye, eyelid contour.
pub const LEFT_EYE: [usize; 16] = [362, 382, 381, 380, 374, 373, 390, 249, 263, 466, 388, 387, 386, 385, 384, 398];
pub const RIGHT_EYE: [usize; 16] = [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246];
pub const LEFT_BROW: [usize; 10] = [300, 293, 334, 296, 336, 285, 295, 282, 283, 276];
pub const RIGHT_BROW: [usize; 10] = [70, 63, 105, 66, 107, 55, 65, 52, 53, 46];
pub const LIPS_OUTER: [usize; 20] = [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185];
pub const LIPS_INNER: [usize; 20] = [78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308, 415, 310, 311, 312, 13, 82, 81, 80, 191];

/// Largest side of a stored patch.
pub const PATCH_SIDE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Skin,
    Eyes,
    Teeth,
    Lips,
}

impl Region {
    pub fn name(self) -> &'static str {
        match self {
            Region::Skin => "skin",
            Region::Eyes => "eyes",
            Region::Teeth => "teeth",
            Region::Lips => "lips",
        }
    }
}

type Poly = Vec<[f32; 2]>;

fn poly(points: &[[f32; 3]], idx: &[usize]) -> Option<Poly> {
    idx.iter().map(|&i| points.get(i).map(|p| [p[0], p[1]])).collect()
}

fn bbox(polys: &[&Poly]) -> [f32; 4] {
    polys
        .iter()
        .flat_map(|p| p.iter())
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])
}

fn area(p: &Poly) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (q, r) = (p[i], p[(i + 1) % p.len()]);
        a += q[0] * r[1] - r[0] * q[1];
    }
    a.abs() / 2.0
}

fn inside(p: &Poly, x: f32, y: f32) -> bool {
    let mut c = false;
    let mut j = p.len() - 1;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[j]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

/// Coverage (0..1, 3 x 3 samples per cell) of `polys` (union) over the grid of `side x side`
/// cells stretched on `rect` `[x0, y0, x1, y1]` (same units as the polygons).
pub fn coverage(polys: &[&Poly], rect: [f32; 4], side: usize) -> Vec<f32> {
    let (w, h) = ((rect[2] - rect[0]).max(1e-3), (rect[3] - rect[1]).max(1e-3));
    let mut out = vec![0f32; side * side];
    for gy in 0..side {
        for gx in 0..side {
            let mut n = 0;
            for sy in 0..3 {
                for sx in 0..3 {
                    let x = rect[0] + (gx as f32 + (sx as f32 + 0.5) / 3.0) / side as f32 * w;
                    let y = rect[1] + (gy as f32 + (sy as f32 + 0.5) / 3.0) / side as f32 * h;
                    if polys.iter().any(|p| inside(p, x, y)) {
                        n += 1;
                    }
                }
            }
            out[gy * side + gx] = n as f32 / 9.0;
        }
    }
    out
}

/// Grow (`r > 0`) or soften a coverage plane: a box blur of `r` cells, then a boost so that
/// anything the blur reached counts (a cheap dilation).
fn grow(c: &[f32], side: usize, r: usize) -> Vec<f32> {
    let b = lightcraft_blur(c, side, r);
    b.iter().map(|v| (v * 3.0).min(1.0)).collect()
}

fn lightcraft_blur(c: &[f32], side: usize, r: usize) -> Vec<f32> {
    crate::dust::box_blur(c, side, side, r)
}

fn logits(cov: &[f32]) -> Vec<f32> {
    cov.iter().map(|&p| lightcraft_logit(p)).collect()
}

fn lightcraft_logit(p: f32) -> f32 {
    let p = p.clamp(0.002, 0.998);
    (p / (1.0 - p)).ln()
}

/// One face's patch for a region: the grid's logits, over `rect` (normalized to the frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub side: usize,
    pub logits: Vec<f32>,
    pub rect: [f64; 4],
    /// Share of the patch that is selected (0 = the region is not visible on this face).
    pub fill: f32,
}

/// The patch of `region` for a face mesh (`points` in frame pixels of a `fw x fh` frame), or
/// `None` when the region is not there (mouth closed, mesh incomplete).
pub fn patch(points: &[[f32; 3]], fw: usize, fh: usize, region: Region) -> Option<Patch> {
    let oval = poly(points, &OVAL)?;
    let face = bbox(&[&oval]);
    let face_w = (face[2] - face[0]).max(1.0);
    let (le, re) = (poly(points, &LEFT_EYE)?, poly(points, &RIGHT_EYE)?);
    let (lb, rb) = (poly(points, &LEFT_BROW)?, poly(points, &RIGHT_BROW)?);
    let (lo, li) = (poly(points, &LIPS_OUTER)?, poly(points, &LIPS_INNER)?);
    let (rect, polys): ([f32; 4], Vec<&Poly>) = match region {
        Region::Skin => (face, vec![&oval]),
        Region::Eyes => (bbox(&[&le, &re]), vec![&le, &re]),
        Region::Teeth => (bbox(&[&li]), vec![&li]),
        Region::Lips => (bbox(&[&lo]), vec![&lo]),
    };
    // a margin so the feathered edge is inside the patch
    let m = match region {
        Region::Skin => 0.06 * face_w,
        _ => 0.10 * (rect[2] - rect[0]).max(0.02 * face_w),
    };
    let rect = [(rect[0] - m).max(0.0), (rect[1] - m).max(0.0), (rect[2] + m).min(fw as f32), (rect[3] + m).min(fh as f32)];
    if rect[2] - rect[0] < 4.0 || rect[3] - rect[1] < 4.0 {
        return None;
    }
    let side = PATCH_SIDE;
    let cells_per_px = side as f32 / (rect[2] - rect[0]).max(rect[3] - rect[1]);
    let cell = |frac_of_face: f32| ((frac_of_face * face_w * cells_per_px).round() as usize).max(1);
    let mut cov = coverage(&polys, rect, side);
    match region {
        Region::Skin => {
            // not the eyes, brows, lips: they keep their detail
            let holes = coverage(&[&le, &re, &lb, &rb, &lo], rect, side);
            let holes = grow(&holes, side, cell(0.02));
            for (c, h) in cov.iter_mut().zip(&holes) {
                *c *= 1.0 - h;
            }
            cov = lightcraft_blur(&cov, side, cell(0.025));
        }
        Region::Eyes => {
            cov = lightcraft_blur(&grow(&cov, side, cell(0.006)), side, cell(0.008));
        }
        Region::Teeth => {
            // the mouth must be open: the inner lips enclose a visible area
            if area(&li) < 0.004 * face_w * face_w {
                return None;
            }
            cov = lightcraft_blur(&cov, side, cell(0.004));
        }
        Region::Lips => {
            let inner = coverage(&[&li], rect, side);
            for (c, i) in cov.iter_mut().zip(&inner) {
                *c *= 1.0 - i;
            }
            cov = lightcraft_blur(&cov, side, cell(0.006));
        }
    }
    let fill = cov.iter().sum::<f32>() / cov.len() as f32;
    (fill > 0.001).then(|| Patch {
        side,
        logits: logits(&cov),
        rect: [f64::from(rect[0]) / fw as f64, f64::from(rect[1]) / fh as f64, f64::from(rect[2]) / fw as f64, f64::from(rect[3]) / fh as f64],
        fill,
    })
}

/// The local adjustments of a region for an amount 0..100 (the strength of the retouch).
pub fn adjustments(region: Region, amount: f64) -> lightcraft_develop::LocalAdjustments {
    use lightcraft_develop::LocalAdjustments;
    let k = (amount / 100.0).clamp(0.0, 1.0);
    match region {
        // smoothing: fine detail (texture) down a lot, mid-scale (clarity) a little, noise down
        Region::Skin => LocalAdjustments { texture: -80.0 * k, clarity: -25.0 * k, noise: 25.0 * k, ..Default::default() },
        // brighter, crisper eyes
        Region::Eyes => LocalAdjustments { exposure: 0.35 * k, contrast: 18.0 * k, clarity: 25.0 * k, sharpness: 30.0 * k, ..Default::default() },
        // whiter teeth: less yellow saturation, a touch brighter
        Region::Teeth => LocalAdjustments { exposure: 0.25 * k, saturation: -45.0 * k, ..Default::default() },
        Region::Lips => LocalAdjustments { saturation: 15.0 * k, ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic face: an ellipse of 36 points and small eye/mouth polygons at the right
    /// indices, in a 400 x 400 frame.
    fn fake_face(mouth_open: f32) -> Vec<[f32; 3]> {
        let mut p = vec![[0f32; 3]; 478];
        let ellipse = |cx: f32, cy: f32, rx: f32, ry: f32, n: usize, t0: f32, i: usize| {
            let t = t0 + i as f32 / n as f32 * std::f32::consts::TAU;
            [cx + rx * t.cos(), cy + ry * t.sin(), 0.0]
        };
        for (i, &k) in OVAL.iter().enumerate() {
            p[k] = ellipse(200.0, 200.0, 110.0, 150.0, 36, -std::f32::consts::FRAC_PI_2, i);
        }
        for (i, &k) in LEFT_EYE.iter().enumerate() {
            p[k] = ellipse(250.0, 160.0, 25.0, 9.0, 16, 0.0, i);
        }
        for (i, &k) in RIGHT_EYE.iter().enumerate() {
            p[k] = ellipse(150.0, 160.0, 25.0, 9.0, 16, 0.0, i);
        }
        for (i, &k) in LEFT_BROW.iter().enumerate() {
            p[k] = [225.0 + i as f32 * 5.0, 130.0, 0.0];
        }
        for (i, &k) in RIGHT_BROW.iter().enumerate() {
            p[k] = [125.0 + i as f32 * 5.0, 130.0, 0.0];
        }
        for (i, &k) in LIPS_OUTER.iter().enumerate() {
            p[k] = ellipse(200.0, 260.0, 45.0, 14.0 + mouth_open, 20, 0.0, i);
        }
        for (i, &k) in LIPS_INNER.iter().enumerate() {
            p[k] = ellipse(200.0, 260.0, 38.0, 1.0 + mouth_open, 20, 0.0, i);
        }
        p
    }

    fn at(p: &Patch, fw: usize, fh: usize, x: f32, y: f32) -> f32 {
        let (nx, ny) = (x as f64 / fw as f64, y as f64 / fh as f64);
        let gx = (((nx - p.rect[0]) / (p.rect[2] - p.rect[0])) * p.side as f64) as usize;
        let gy = (((ny - p.rect[1]) / (p.rect[3] - p.rect[1])) * p.side as f64) as usize;
        1.0 / (1.0 + (-p.logits[gy.min(p.side - 1) * p.side + gx.min(p.side - 1)]).exp())
    }

    #[test]
    fn skin_covers_cheeks_and_spares_eyes_brows_and_lips() {
        let f = fake_face(10.0);
        let p = patch(&f, 400, 400, Region::Skin).expect("skin");
        assert!(at(&p, 400, 400, 260.0, 215.0) > 0.9, "cheek");
        assert!(at(&p, 400, 400, 200.0, 215.0) > 0.9, "nose area");
        assert!(at(&p, 400, 400, 250.0, 160.0) < 0.2, "eye");
        assert!(at(&p, 400, 400, 200.0, 260.0) < 0.2, "mouth");
        assert!(at(&p, 400, 400, 20.0, 20.0) < 0.1, "outside the face");
    }

    #[test]
    fn eyes_cover_both_eyes_and_nothing_between() {
        let f = fake_face(10.0);
        let p = patch(&f, 400, 400, Region::Eyes).expect("eyes");
        assert!(at(&p, 400, 400, 250.0, 160.0) > 0.8);
        assert!(at(&p, 400, 400, 150.0, 160.0) > 0.8);
        assert!(at(&p, 400, 400, 200.0, 160.0) < 0.2);
    }

    #[test]
    fn teeth_need_an_open_mouth() {
        assert!(patch(&fake_face(0.0), 400, 400, Region::Teeth).is_none());
        let open = patch(&fake_face(12.0), 400, 400, Region::Teeth).expect("teeth");
        assert!(at(&open, 400, 400, 200.0, 260.0) > 0.7);
    }

    #[test]
    fn an_incomplete_mesh_gives_no_patch_and_amounts_scale_the_adjustments() {
        assert!(patch(&[[0.0; 3]; 10], 100, 100, Region::Skin).is_none());
        let (a, b) = (adjustments(Region::Skin, 100.0), adjustments(Region::Skin, 50.0));
        assert_eq!((a.texture, b.texture), (-80.0, -40.0));
        assert_eq!(adjustments(Region::Skin, 0.0), lightcraft_develop::LocalAdjustments::default());
        assert!(adjustments(Region::Eyes, 100.0).exposure > 0.0 && adjustments(Region::Teeth, 100.0).saturation < 0.0);
    }
}
