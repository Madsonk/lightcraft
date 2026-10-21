//! Open eyes: take the eyes of the same person from another shot of the same series and put them
//! in the photo where they are closed. Nothing is invented: the pixels come from a real frame.
//!
//! Alignment is a similarity (scale, rotation, translation) fixed by the two corners of the eye in
//! both frames; the light and colour of the skin around the eye are matched by a smooth shift
//! measured on a ring around it; the join is a feathered mask. Every guard that fails says why
//! instead of producing a bad eye (reference eye not open, head turned differently, scale off).

use super::raster::{Poly, fill, max_filter};
use crate::facegeo::Face;
use crate::lab::{lab_to_rgb8, rgb8_to_lab};

/// Which eyes to take (the person's left is the image's right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    Both,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EyeReport {
    /// "left" or "right" (the person's).
    pub side: &'static str,
    /// Why nothing was done for this eye.
    pub skipped: Option<&'static str>,
    /// Scale of the reference relative to the photo (1 = same size) and its rotation, degrees.
    pub scale: f32,
    pub rotation_deg: f32,
    /// Eye aspect ratio before and after (lid gap over eye width): ≈ 0.3 open, ≈ 0 closed.
    pub open_before: f32,
    pub open_after: f32,
    /// Mean colour difference (ΔE76) between the reference and the photo on the thin band where they
    /// are joined, after the light/colour match: lower is a less visible seam.
    pub seam_de: f32,
}

pub struct Outcome {
    pub eyes: Vec<EyeReport>,
}

/// Lid gap over eye width of one eye of a mesh.
pub fn eye_aspect(face: &Face, left: bool) -> f32 {
    let (outer, inner, up, down) = if left { (263, 362, 386, 374) } else { (33, 133, 159, 145) };
    let (Some(a), Some(b), Some(u), Some(d)) = (face.point(outer), face.point(inner), face.point(up), face.point(down)) else { return 0.0 };
    let w = (a[0] - b[0]).hypot(a[1] - b[1]).max(1e-3);
    (u[0] - d[0]).hypot(u[1] - d[1]) / w
}

/// Yaw proxy: how far the nose tip sits from the middle of the eyes, in face widths.
fn yaw_proxy(face: &Face) -> f32 {
    let (Some(l), Some(r), Some(n)) = (face.point(263), face.point(33), face.point(4)) else { return 0.0 };
    ((n[0] - (l[0] + r[0]) / 2.0) / face.width.max(1.0)).clamp(-1.0, 1.0)
}

struct Lab {
    w: usize,
    h: usize,
    l: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
}

fn lab_of(rgb: &[u8], w: usize, h: usize) -> Lab {
    let n = w * h;
    let (mut l, mut a, mut b) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for px in rgb.chunks_exact(3).take(n) {
        let v = rgb8_to_lab([px[0], px[1], px[2]]);
        l.push(v[0]);
        a.push(v[1]);
        b.push(v[2]);
    }
    Lab { w, h, l, a, b }
}

impl Lab {
    fn sample(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        if !(x.is_finite() && y.is_finite()) || x < 0.5 || y < 0.5 || x > self.w as f32 - 0.5 || y > self.h as f32 - 0.5 {
            return None;
        }
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |v: &Vec<f32>| {
            let a = v[y0 * self.w + x0] * (1.0 - tx) + v[y0 * self.w + x1] * tx;
            let b = v[y1 * self.w + x0] * (1.0 - tx) + v[y1 * self.w + x1] * tx;
            a * (1.0 - ty) + b * ty
        };
        Some([at(&self.l), at(&self.a), at(&self.b)])
    }
}

/// A similarity from reference pixels to photo pixels.
struct Sim {
    s: f32,
    c: f32,
    sn: f32,
    t: [f32; 2],
}

impl Sim {
    /// Fixed by two point pairs (reference a→b mapped onto photo a→b).
    fn from_pairs(r: [[f32; 2]; 2], p: [[f32; 2]; 2]) -> Option<Sim> {
        let (rv, pv) = ([r[1][0] - r[0][0], r[1][1] - r[0][1]], [p[1][0] - p[0][0], p[1][1] - p[0][1]]);
        let (rl, pl) = (rv[0].hypot(rv[1]), pv[0].hypot(pv[1]));
        if rl < 1.0 || pl < 1.0 {
            return None;
        }
        let ang = pv[1].atan2(pv[0]) - rv[1].atan2(rv[0]);
        let s = pl / rl;
        let (sn, c) = ang.sin_cos();
        let rc = [(r[0][0] + r[1][0]) / 2.0, (r[0][1] + r[1][1]) / 2.0];
        let pc = [(p[0][0] + p[1][0]) / 2.0, (p[0][1] + p[1][1]) / 2.0];
        let mapped = [s * (c * rc[0] - sn * rc[1]), s * (sn * rc[0] + c * rc[1])];
        Some(Sim { s, c, sn, t: [pc[0] - mapped[0], pc[1] - mapped[1]] })
    }
    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [self.s * (self.c * p[0] - self.sn * p[1]) + self.t[0], self.s * (self.sn * p[0] + self.c * p[1]) + self.t[1]]
    }
    fn inverse(&self, p: [f32; 2]) -> [f32; 2] {
        let q = [p[0] - self.t[0], p[1] - self.t[1]];
        [(self.c * q[0] + self.sn * q[1]) / self.s, (-self.sn * q[0] + self.c * q[1]) / self.s]
    }
}

fn corners(face: &Face, left: bool) -> Option<[[f32; 2]; 2]> {
    let (o, i) = if left { (263, 362) } else { (33, 133) };
    Some([face.point(o)?, face.point(i)?])
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Put the eyes of the reference (`src`, mesh `spts`) into the photo (`dst`, mesh `dpts`, both in the
/// pixel coordinates of their own buffers). Pixels that move are rewritten.
pub fn transfer(
    dst: &mut [u8],
    dw: usize,
    dh: usize,
    dpts: &[[f32; 3]],
    src: &[u8],
    sw: usize,
    sh: usize,
    spts: &[[f32; 3]],
    sides: Sides,
) -> Option<Outcome> {
    if dst.len() < dw * dh * 3 || src.len() < sw * sh * 3 || dw < 8 || dh < 8 || sw < 8 || sh < 8 {
        return None;
    }
    let (dface, sface) = (Face::new(dpts)?, Face::new(spts)?);
    let (dl, sl) = (lab_of(dst, dw, dh), lab_of(src, sw, sh));
    let yaw_gap = (yaw_proxy(&dface) - yaw_proxy(&sface)).abs();
    let mut out = Outcome { eyes: Vec::new() };
    let mut edited: Vec<(usize, [f32; 3])> = Vec::new();
    for left in [true, false] {
        if (left && sides == Sides::Right) || (!left && sides == Sides::Left) {
            continue;
        }
        let side = if left { "left" } else { "right" };
        let mut rep = EyeReport {
            side,
            skipped: None,
            scale: 1.0,
            rotation_deg: 0.0,
            open_before: eye_aspect(&dface, left),
            open_after: eye_aspect(&dface, left),
            seam_de: 0.0,
        };
        let skip = |mut r: EyeReport, why: &'static str| {
            r.skipped = Some(why);
            r
        };
        let (Some(dc), Some(sc)) = (corners(&dface, left), corners(&sface, left)) else {
            out.eyes.push(skip(rep, "mesh incomplete"));
            continue;
        };
        let src_open = eye_aspect(&sface, left);
        if src_open < 0.2 {
            out.eyes.push(skip(rep, "reference eye is not open"));
            continue;
        }
        if yaw_gap > 0.12 {
            out.eyes.push(skip(rep, "head angle differs too much"));
            continue;
        }
        let ratio = dist(dc[0], dc[1]) / dist(sc[0], sc[1]).max(1e-3);
        if !(0.4..=2.5).contains(&ratio) {
            out.eyes.push(skip(rep, "face size differs too much"));
            continue;
        }
        let Some(sim) = Sim::from_pairs(sc, dc) else {
            out.eyes.push(skip(rep, "degenerate eye"));
            continue;
        };
        rep.scale = sim.s;
        rep.rotation_deg = sim.sn.atan2(sim.c).to_degrees();
        // region: the eye in the photo and the reference's eye mapped, grown by a third of the eye
        let ew = dist(dc[0], dc[1]);
        let dpoly: Poly = if left { dface.left_eye() } else { dface.right_eye() };
        let spoly: Poly = (if left { sface.left_eye() } else { sface.right_eye() }).iter().map(|p| sim.apply(*p)).collect();
        if dpoly.is_empty() || spoly.is_empty() {
            out.eyes.push(skip(rep, "mesh incomplete"));
            continue;
        }
        // work in a window around the eye
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in dpoly.iter().chain(spoly.iter()) {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let pad = 1.2 * ew;
        let (wx0, wy0) = (((x0 - pad).floor().max(0.0)) as usize, ((y0 - pad).floor().max(0.0)) as usize);
        let (wx1, wy1) = (((x1 + pad).ceil() as usize).min(dw), ((y1 + pad).ceil() as usize).min(dh));
        if wx1 <= wx0 + 4 || wy1 <= wy0 + 4 {
            out.eyes.push(skip(rep, "eye outside the frame"));
            continue;
        }
        let (ww, wh) = (wx1 - wx0, wy1 - wy0);
        let shift = |p: &Poly| -> Poly { p.iter().map(|q| [q[0] - wx0 as f32, q[1] - wy0 as f32]).collect() };
        let (dp, sp) = (shift(&dpoly), shift(&spoly));
        let union = fill(&[&dp, &sp], ww, wh);
        let grow = ((0.28 * ew).round() as usize).max(1);
        let core = max_filter(&union, grow);
        let mask = core.gauss((0.10 * ew).max(0.8));
        // the ring: just outside the region, where only skin is
        let outer = max_filter(&core, ((0.35 * ew).round() as usize).max(2));
        let mut ring: Vec<usize> = Vec::new();
        let mut seam: Vec<usize> = Vec::new();
        for i in 0..ww * wh {
            if outer.d[i] > 0.5 && core.d[i] < 0.5 {
                ring.push(i);
            }
            if mask.d[i] > 0.02 && mask.d[i] < 0.6 {
                seam.push(i);
            }
        }
        // a smooth shift of the reference toward the photo's skin: mean over the ring
        let (mut dm, mut sm, mut n) = ([0f32; 3], [0f32; 3], 0f32);
        for &i in &ring {
            let (x, y) = (wx0 + i % ww, wy0 + i / ww);
            let q = sim.inverse([x as f32 + 0.5, y as f32 + 0.5]);
            if let Some(sv) = sl.sample(q[0], q[1]) {
                let di = y * dw + x;
                for c in 0..3 {
                    dm[c] += [dl.l[di], dl.a[di], dl.b[di]][c];
                    sm[c] += sv[c];
                }
                n += 1.0;
            }
        }
        let off = if n >= 8.0 { [(dm[0] - sm[0]) / n, (dm[1] - sm[1]) / n, (dm[2] - sm[2]) / n] } else { [0.0; 3] };
        let off = [off[0].clamp(-25.0, 25.0), off[1].clamp(-12.0, 12.0), off[2].clamp(-12.0, 12.0)];
        let mut put: Vec<(usize, [f32; 3])> = Vec::new();
        for i in 0..ww * wh {
            let m = mask.d[i];
            if m <= 0.0 {
                continue;
            }
            let (x, y) = (wx0 + i % ww, wy0 + i / ww);
            let q = sim.inverse([x as f32 + 0.5, y as f32 + 0.5]);
            let Some(sv) = sl.sample(q[0], q[1]) else { continue };
            let di = y * dw + x;
            let d0 = [dl.l[di], dl.a[di], dl.b[di]];
            let s1 = [(sv[0] + off[0]).clamp(0.0, 100.0), sv[1] + off[1], sv[2] + off[2]];
            put.push((di, [d0[0] + (s1[0] - d0[0]) * m, d0[1] + (s1[1] - d0[1]) * m, d0[2] + (s1[2] - d0[2]) * m]));
        }
        // seam quality: reference (matched) against the photo, on the join band
        let (mut de, mut cnt) = (0f32, 0f32);
        for &i in &seam {
            let (x, y) = (wx0 + i % ww, wy0 + i / ww);
            let q = sim.inverse([x as f32 + 0.5, y as f32 + 0.5]);
            if let Some(sv) = sl.sample(q[0], q[1]) {
                let di = y * dw + x;
                let s1 = [sv[0] + off[0], sv[1] + off[1], sv[2] + off[2]];
                de += ((s1[0] - dl.l[di]).powi(2) + (s1[1] - dl.a[di]).powi(2) + (s1[2] - dl.b[di]).powi(2)).sqrt();
                cnt += 1.0;
            }
        }
        rep.seam_de = if cnt > 0.0 { de / cnt } else { 0.0 };
        // the transferred eye's own opening, measured on the reference
        rep.open_after = src_open;
        edited.extend(put);
        out.eyes.push(rep);
    }
    for (i, v) in edited {
        let px = lab_to_rgb8(v);
        dst[i * 3..i * 3 + 3].copy_from_slice(&px);
    }
    Some(out)
}
