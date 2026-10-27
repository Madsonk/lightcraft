//! Geometry of a face from the 478-point mesh (MediaPipe Face Mesh V2): polygons of the eyes,
//! lids, lips and oval, the iris discs, the nose ridge, a smooth depth surface and its convexity.
//! Points are in frame pixels; the z of a point is relative to the face and scaled like the width.

use crate::plane::Plane;

pub const OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234,
    127, 162, 21, 54, 103, 67, 109,
];
/// The person's left eye (the viewer's right) and right eye: eyelid contours.
pub const LEFT_EYE: [usize; 16] = [362, 382, 381, 380, 374, 373, 390, 249, 263, 466, 388, 387, 386, 385, 384, 398];
pub const RIGHT_EYE: [usize; 16] = [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246];
/// Lower lid, from the outer corner to the inner one.
pub const LEFT_LOWER_LID: [usize; 9] = [263, 249, 390, 373, 374, 380, 381, 382, 362];
pub const RIGHT_LOWER_LID: [usize; 9] = [33, 7, 163, 144, 145, 153, 154, 155, 133];
pub const LEFT_BROW: [usize; 10] = [300, 293, 334, 296, 336, 285, 295, 282, 283, 276];
pub const RIGHT_BROW: [usize; 10] = [70, 63, 105, 66, 107, 55, 65, 52, 53, 46];
pub const LIPS_OUTER: [usize; 20] = [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185];
pub const LIPS_INNER: [usize; 20] = [78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308, 415, 310, 311, 312, 13, 82, 81, 80, 191];
/// Iris: centre then four points of the rim (the right iris, then the left one).
pub const RIGHT_IRIS: [usize; 5] = [468, 469, 470, 471, 472];
pub const LEFT_IRIS: [usize; 5] = [473, 474, 475, 476, 477];
pub const NOSE_RIDGE: [usize; 5] = [168, 6, 197, 195, 5];
pub const NOSE_TIP: usize = 4;
pub const FOREHEAD: [usize; 4] = [151, 9, 8, 10];
pub const CHIN: [usize; 3] = [152, 175, 199];

pub type Poly = Vec<[f32; 2]>;

fn poly(p: &[[f32; 3]], idx: &[usize]) -> Option<Poly> {
    idx.iter().map(|&i| p.get(i).map(|q| [q[0], q[1]])).collect()
}

fn pt(p: &[[f32; 3]], i: usize) -> Option<[f32; 2]> {
    p.get(i).map(|q| [q[0], q[1]])
}

/// One face's mesh with the derived shapes.
pub struct Face {
    pub p: Vec<[f32; 3]>,
    pub oval: Poly,
    /// Width of the face (the oval's box), pixels.
    pub width: f32,
    /// Unit vector pointing down the face (forehead to chin).
    pub down: [f32; 2],
    pub bbox: [f32; 4],
}

impl Face {
    pub fn new(points: &[[f32; 3]]) -> Option<Face> {
        if points.len() < 468 {
            return None;
        }
        let oval = poly(points, &OVAL)?;
        let bb = oval.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]);
        let width = (bb[2] - bb[0]).max(1.0);
        let (top, chin) = (pt(points, 10)?, pt(points, 152)?);
        let (dx, dy) = (chin[0] - top[0], chin[1] - top[1]);
        let n = dx.hypot(dy).max(1e-3);
        Some(Face { p: points.to_vec(), oval, width, down: [dx / n, dy / n], bbox: bb })
    }

    pub fn has_iris(&self) -> bool {
        self.p.len() >= 478
    }

    pub fn left_eye(&self) -> Poly {
        poly(&self.p, &LEFT_EYE).unwrap_or_default()
    }
    pub fn right_eye(&self) -> Poly {
        poly(&self.p, &RIGHT_EYE).unwrap_or_default()
    }
    pub fn lips_outer(&self) -> Poly {
        poly(&self.p, &LIPS_OUTER).unwrap_or_default()
    }
    pub fn lips_inner(&self) -> Poly {
        poly(&self.p, &LIPS_INNER).unwrap_or_default()
    }

    /// An iris as (centre, radius) in pixels: the mean distance of its four rim points.
    pub fn iris(&self, left: bool) -> Option<([f32; 2], f32)> {
        if !self.has_iris() {
            return None;
        }
        let idx = if left { &LEFT_IRIS } else { &RIGHT_IRIS };
        let c = pt(&self.p, idx[0])?;
        let r = idx[1..].iter().filter_map(|&i| pt(&self.p, i)).map(|q| (q[0] - c[0]).hypot(q[1] - c[1])).sum::<f32>() / 4.0;
        (r > 0.5).then_some((c, r))
    }

    /// The skin band under an eye: the lower lid and the same line moved `depth` (fraction of the
    /// face width) down the face.
    pub fn under_eye(&self, left: bool, depth: f32) -> Option<Poly> {
        let lid = poly(&self.p, if left { &LEFT_LOWER_LID } else { &RIGHT_LOWER_LID })?;
        let d = depth * self.width;
        let mut out = lid.clone();
        for q in lid.iter().rev() {
            out.push([q[0] + self.down[0] * d, q[1] + self.down[1] * d]);
        }
        Some(out)
    }

    /// Points of the nose ridge, tip last.
    pub fn nose_line(&self) -> Vec<[f32; 2]> {
        let mut v: Vec<[f32; 2]> = NOSE_RIDGE.iter().filter_map(|&i| pt(&self.p, i)).collect();
        v.extend(pt(&self.p, NOSE_TIP));
        v
    }

    pub fn point(&self, i: usize) -> Option<[f32; 2]> {
        pt(&self.p, i)
    }

    /// The rectangle (frame pixels) the planes of this face are drawn in: the oval's box with a
    /// margin of `m` face widths, clipped to the frame.
    pub fn rect(&self, m: f32, fw: usize, fh: usize) -> [f32; 4] {
        let d = m * self.width;
        [(self.bbox[0] - d).max(0.0), (self.bbox[1] - d).max(0.0), (self.bbox[2] + d).min(fw as f32), (self.bbox[3] + d).min(fh as f32)]
    }
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

/// Coverage (0..1, 3 x 3 samples per pixel) of the union of `polys` on a `w x h` plane.
pub fn fill(polys: &[&Poly], w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    let polys: Vec<&&Poly> = polys.iter().filter(|p| p.len() >= 3).collect();
    if polys.is_empty() {
        return out;
    }
    let bb = polys
        .iter()
        .flat_map(|p| p.iter())
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]);
    let (x0, y0) = (bb[0].floor().max(0.0) as usize, bb[1].floor().max(0.0) as usize);
    let (x1, y1) = ((bb[2].ceil() as usize + 1).min(w), (bb[3].ceil() as usize + 1).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let mut n = 0;
            for sy in 0..3 {
                for sx in 0..3 {
                    let (fx, fy) = (x as f32 + (sx as f32 + 0.5) / 3.0, y as f32 + (sy as f32 + 0.5) / 3.0);
                    if polys.iter().any(|p| inside(p, fx, fy)) {
                        n += 1;
                    }
                }
            }
            out.d[y * w + x] = n as f32 / 9.0;
        }
    }
    out
}

/// A soft disc: 1 inside `r`, falling to 0 at `r + soft`.
pub fn disc(w: usize, h: usize, c: [f32; 2], r: f32, soft: f32) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    let reach = r + soft.max(0.5) + 1.0;
    let (x0, x1) = ((c[0] - reach).floor().max(0.0) as usize, ((c[0] + reach).ceil() as usize + 1).min(w));
    let (y0, y1) = ((c[1] - reach).floor().max(0.0) as usize, ((c[1] + reach).ceil() as usize + 1).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let d = (x as f32 + 0.5 - c[0]).hypot(y as f32 + 0.5 - c[1]);
            out.d[y * w + x] = 1.0 - crate::plane::smoothstep(r, r + soft.max(0.5), d);
        }
    }
    out
}

/// A soft elongated blob along the segment `a`-`b` (a capsule), `r` its radius, `soft` the fall.
pub fn capsule(w: usize, h: usize, a: [f32; 2], b: [f32; 2], r: f32, soft: f32) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    let reach = r + soft.max(0.5) + 1.0;
    let (x0, x1) = ((a[0].min(b[0]) - reach).floor().max(0.0) as usize, ((a[0].max(b[0]) + reach).ceil() as usize + 1).min(w));
    let (y0, y1) = ((a[1].min(b[1]) - reach).floor().max(0.0) as usize, ((a[1].max(b[1]) + reach).ceil() as usize + 1).min(h));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = (dx * dx + dy * dy).max(1e-6);
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]);
            let t = ((px * dx + py * dy) / l2).clamp(0.0, 1.0);
            let d = (px - t * dx).hypot(py - t * dy);
            out.d[y * w + x] = 1.0 - crate::plane::smoothstep(r, r + soft.max(0.5), d);
        }
    }
    out
}

/// Side of the depth grid.
pub const DEPTH_GRID: usize = 64;

/// The face's depth surface on a `DEPTH_GRID` square over `rect`: the mesh's z interpolated
/// (inverse distance, power 2) so that the near parts (nose, brow, cheekbones, chin) are high.
/// Returns "nearness" (minus z), in face-width units.
pub fn nearness(face: &Face, rect: [f32; 4]) -> Plane {
    let n = DEPTH_GRID;
    let (rw, rh) = ((rect[2] - rect[0]).max(1.0), (rect[3] - rect[1]).max(1.0));
    let mut g = Plane::new(n, n, 0.0);
    for gy in 0..n {
        for gx in 0..n {
            let (x, y) = (rect[0] + (gx as f32 + 0.5) / n as f32 * rw, rect[1] + (gy as f32 + 0.5) / n as f32 * rh);
            let (mut sw, mut sz) = (0.0f32, 0.0f32);
            for q in &face.p {
                let d2 = (q[0] - x).powi(2) + (q[1] - y).powi(2) + 4.0;
                let w = 1.0 / (d2 * d2.sqrt());
                sw += w;
                sz += w * (-q[2]);
            }
            g.d[gy * n + gx] = sz / sw / face.width;
        }
    }
    g
}

/// Convexity of the face on the depth grid: where the surface bulges toward the camera compared
/// with its surroundings (nose ridge, brow, cheekbones, chin: positive) and where it falls away
/// (eye sockets, the sides of the nose, under the cheekbones: negative). In face-width units.
pub fn convexity(near: &Plane, face: &Face, rect: [f32; 4]) -> Plane {
    let cell = (rect[2] - rect[0]).max(1.0) / near.w as f32;
    let small = near.gauss((0.05 * face.width / cell).max(0.8));
    let big = near.gauss((0.20 * face.width / cell).max(2.0));
    small.zip(&big, |a, b| a - b)
}

/// A grid plane over `rect` (frame pixels) drawn on a `w x h` frame, bilinear; 0 outside.
pub fn grid_to_frame(g: &Plane, rect: [f32; 4], w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    let (rw, rh) = ((rect[2] - rect[0]).max(1e-3), (rect[3] - rect[1]).max(1e-3));
    let (x0, x1) = (rect[0].floor().max(0.0) as usize, (rect[2].ceil() as usize).min(w));
    let (y0, y1) = (rect[1].floor().max(0.0) as usize, (rect[3].ceil() as usize).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let (u, v) = ((x as f32 + 0.5 - rect[0]) / rw, (y as f32 + 0.5 - rect[1]) / rh);
            out.d[y * w + x] = g.sample(u * g.w as f32, v * g.h as f32);
        }
    }
    out
}

/// The grid plane (side x side) over `rect` of a frame plane: the frame sampled at the grid's
/// cell centres (area-averaged by the box blur of the cell when the frame is finer).
pub fn frame_to_grid(frame: &Plane, rect: [f32; 4], side: usize) -> Plane {
    let (rw, rh) = ((rect[2] - rect[0]).max(1e-3), (rect[3] - rect[1]).max(1e-3));
    let cell = (rw.max(rh) / side as f32 / 2.0).floor() as usize;
    let src = if cell >= 1 { frame.box_blur(cell.min(8)) } else { frame.clone() };
    let mut g = Plane::new(side, side, 0.0);
    for gy in 0..side {
        for gx in 0..side {
            let (x, y) = (rect[0] + (gx as f32 + 0.5) / side as f32 * rw, rect[1] + (gy as f32 + 0.5) / side as f32 * rh);
            g.d[gy * side + gx] = src.sample(x, y);
        }
    }
    g
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic face mesh in a 400 x 400 frame: oval, eyes (with irises), brows, lips, a nose
    /// bulging toward the camera (negative z).
    pub fn fake_mesh() -> Vec<[f32; 3]> {
        let mut seed = 7u32;
        let mut r = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) & 0xffff) as f32 / 65535.0
        };
        // the unnamed points scatter over the face at z = 0
        let mut p: Vec<[f32; 3]> = (0..478).map(|_| [100.0 + r() * 200.0, 60.0 + r() * 280.0, 0.0]).collect();
        let ell = |cx: f32, cy: f32, rx: f32, ry: f32, n: usize, t0: f32, i: usize| {
            let t = t0 + i as f32 / n as f32 * std::f32::consts::TAU;
            [cx + rx * t.cos(), cy + ry * t.sin()]
        };
        for (i, &k) in OVAL.iter().enumerate() {
            let q = ell(200.0, 200.0, 110.0, 150.0, 36, -std::f32::consts::FRAC_PI_2, i);
            p[k] = [q[0], q[1], 20.0];
        }
        for (i, &k) in LEFT_EYE.iter().enumerate() {
            let q = ell(250.0, 160.0, 25.0, 9.0, 16, 0.0, i);
            p[k] = [q[0], q[1], 0.0];
        }
        for (i, &k) in RIGHT_EYE.iter().enumerate() {
            let q = ell(150.0, 160.0, 25.0, 9.0, 16, 0.0, i);
            p[k] = [q[0], q[1], 0.0];
        }
        for (i, &k) in LEFT_BROW.iter().enumerate() {
            p[k] = [225.0 + i as f32 * 5.0, 130.0, 0.0];
        }
        for (i, &k) in RIGHT_BROW.iter().enumerate() {
            p[k] = [125.0 + i as f32 * 5.0, 130.0, 0.0];
        }
        for (i, &k) in LIPS_OUTER.iter().enumerate() {
            let q = ell(200.0, 260.0, 45.0, 14.0, 20, 0.0, i);
            p[k] = [q[0], q[1], 0.0];
        }
        for (i, &k) in LIPS_INNER.iter().enumerate() {
            let q = ell(200.0, 260.0, 38.0, 3.0, 20, 0.0, i);
            p[k] = [q[0], q[1], 0.0];
        }
        // iris: centre + four rim points, radius 9
        for (c, idx) in [([250.0f32, 160.0f32], LEFT_IRIS), ([150.0, 160.0], RIGHT_IRIS)] {
            p[idx[0]] = [c[0], c[1], 0.0];
            for (k, &i) in idx[1..].iter().enumerate() {
                let t = k as f32 * std::f32::consts::FRAC_PI_2;
                p[i] = [c[0] + 9.0 * t.cos(), c[1] + 9.0 * t.sin(), 0.0];
            }
        }
        // the nose bulges: z falls to -120 on the ridge
        for (k, &i) in NOSE_RIDGE.iter().enumerate() {
            p[i] = [200.0, 150.0 + k as f32 * 18.0, -120.0];
        }
        p[NOSE_TIP] = [200.0, 240.0, -140.0];
        p[10] = [200.0, 50.0, 20.0];
        p[152] = [200.0, 350.0, 20.0];
        p
    }

    #[test]
    fn the_face_has_its_oval_a_down_vector_and_irises() {
        let f = Face::new(&fake_mesh()).expect("face");
        assert!((f.width - 220.0).abs() < 4.0 && f.down[1] > 0.99, "{} {:?}", f.width, f.down);
        let (c, r) = f.iris(true).expect("iris");
        assert!((c[0] - 250.0).abs() < 0.1 && (r - 9.0).abs() < 0.1);
        assert!(Face::new(&[[0.0; 3]; 10]).is_none());
    }

    #[test]
    fn fill_gives_a_soft_edge_and_a_band_lies_under_the_eye() {
        let f = Face::new(&fake_mesh()).expect("face");
        let oval = fill(&[&f.oval], 400, 400);
        assert!(oval.at(200, 200) == 1.0 && oval.at(5, 5) == 0.0);
        let band = f.under_eye(true, 0.08).expect("band");
        let cov = fill(&[&band], 400, 400);
        assert!(cov.at(250, 172) > 0.9, "just under the lid");
        assert!(cov.at(250, 140) == 0.0, "not above");
    }

    #[test]
    fn the_nose_is_convex_and_the_flanks_are_not() {
        let f = Face::new(&fake_mesh()).expect("face");
        let rect = f.rect(0.1, 400, 400);
        let near = nearness(&f, rect);
        let c = convexity(&near, &f, rect);
        let at = |x: f32, y: f32| {
            let (u, v) = ((x - rect[0]) / (rect[2] - rect[0]), (y - rect[1]) / (rect[3] - rect[1]));
            c.sample(u * c.w as f32, v * c.h as f32)
        };
        assert!(at(200.0, 200.0) > 0.0, "ridge {}", at(200.0, 200.0));
        assert!(at(200.0, 200.0) > at(130.0, 210.0), "ridge above flank");
    }

    #[test]
    fn grid_and_frame_planes_convert_back_and_forth() {
        let rect = [50.0, 60.0, 250.0, 260.0];
        let mut frame = Plane::new(300, 300, 0.0);
        for y in 100..180 {
            for x in 100..180 {
                frame.d[y * 300 + x] = 1.0;
            }
        }
        let g = frame_to_grid(&frame, rect, 64);
        assert!(g.sample(32.0, 32.0) > 0.9 && g.sample(2.0, 2.0) < 0.1);
        let back = grid_to_frame(&g, rect, 300, 300);
        assert!(back.at(140, 140) > 0.9 && back.at(10, 10) == 0.0);
    }

    #[test]
    fn discs_and_capsules_are_full_inside_and_empty_far() {
        let d = disc(50, 50, [25.0, 25.0], 8.0, 4.0);
        assert!(d.at(25, 25) == 1.0 && d.at(45, 45) == 0.0);
        let c = capsule(50, 50, [10.0, 25.0], [40.0, 25.0], 4.0, 3.0);
        assert!(c.at(25, 25) == 1.0 && c.at(25, 40) == 0.0);
    }
}
