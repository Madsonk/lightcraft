//! The portrait steps. Each one reads and edits the Lab planes of [`Work`], with sizes given as
//! fractions of the face width. Fields that move colour are computed on low-pass versions of the
//! planes and added as smooth deltas, so the texture of the skin survives.

use super::raster::{Poly, blobs, fill, max_filter, min_filter};
use super::{Kind, StepReport};
use crate::facegeo::{self, Face};
use crate::plane::{Plane, blur_weighted, smoothstep};

/// Weighted blur that widens its window where nothing of weight is near (a filled mark bigger
/// than the blur radius would otherwise get 0): 1x, 3x then 9x `sigma`; pixels still without
/// any support keep their own value.
fn robust_blur(v: &Plane, wt: &Plane, sigma: f32) -> Plane {
    let n = v.d.len();
    let mut out = v.clone();
    let mut done = vec![false; n];
    let vw = v.zip(wt, |a, b| a * b);
    for k in [1.0f32, 3.0, 9.0] {
        let (num, den) = (vw.gauss(sigma * k), wt.gauss(sigma * k));
        for i in 0..n {
            if !done[i] && den.d[i] > 0.02 {
                out.d[i] = num.d[i] / den.d[i];
                done[i] = true;
            }
        }
    }
    out
}
use crate::skin::{self, LabImage, Skin};

pub struct Work {
    pub lab: LabImage,
    orig: LabImage,
    pub face: Face,
    /// Face width in working pixels.
    pub fw: f32,
    /// Coverage of the face oval.
    oval: Plane,
    /// Coverage of the skin: the oval without eyes, brows and lips, softened.
    cov: Plane,
    skin: Skin,
}

impl Work {
    pub fn new(lab: LabImage, face: Face) -> Option<Work> {
        let (w, h) = (lab.w, lab.h);
        let fw = face.width;
        let oval = fill(&[&face.oval], w, h);
        let (le, re) = (face.left_eye(), face.right_eye());
        let brows = [brow(&face, true), brow(&face, false)];
        let lips = face.lips_outer();
        let mut holes: Vec<&Poly> = vec![&le, &re, &lips];
        holes.extend(brows.iter());
        let holes = fill(&holes, w, h).gauss(0.012 * fw).map(|v| (v * 3.0).min(1.0));
        let cov = oval.zip(&holes, |o, k| o * (1.0 - k)).gauss(0.01 * fw);
        let skin = skin::select(&lab, &cov, 0.01 * fw)?;
        let orig = LabImage { w, h, l: lab.l.clone(), a: lab.a.clone(), b: lab.b.clone() };
        Some(Work { lab, orig, face, fw, oval, cov, skin })
    }

    /// Edited minus original, per Lab channel.
    pub fn delta(&self) -> (Plane, Plane, Plane) {
        (self.lab.l.zip(&self.orig.l, |a, b| a - b), self.lab.a.zip(&self.orig.a, |a, b| a - b), self.lab.b.zip(&self.orig.b, |a, b| a - b))
    }

    fn px(&self, f: f32) -> f32 {
        (f * self.fw).max(0.0)
    }

    fn g(&self, p: &Plane, f: f32) -> Plane {
        p.gauss(self.px(f))
    }

    /// Run one step and report.
    pub fn apply(&mut self, kind: Kind, amt: f32) -> StepReport {
        let before = (self.lab.l.clone(), self.lab.a.clone(), self.lab.b.clone());
        let mut found = None;
        let mut skipped = None;
        match kind {
            Kind::Smooth => {
                let w = self.skin.weight.clone();
                self.smooth(&w, amt);
            }
            Kind::Blemishes => found = Some(self.blemishes(amt)),
            Kind::Freckles => self.freckles(amt),
            Kind::WrinklesEyes => {
                let r = self.region_eye_wrinkles();
                self.wrinkles(&r, amt);
            }
            Kind::WrinklesForehead => {
                let r = self.region_forehead();
                self.wrinkles(&r, amt);
            }
            Kind::WrinklesLines => {
                let r = self.region_smile_lines();
                self.wrinkles(&r, amt);
            }
            Kind::DarkCircles => self.dark_circles(amt),
            Kind::Shine => self.shine(amt),
            Kind::DoubleChin => self.double_chin(amt),
            Kind::Teeth => skipped = self.teeth(amt),
            Kind::Eyes => self.eyes(amt),
            Kind::Flyaways => found = Some(self.flyaways(amt)),
            Kind::NeckSmooth => {
                let n = self.neck();
                if n.d.iter().sum::<f32>() < 50.0 {
                    skipped = Some("no neck in the frame");
                } else {
                    self.smooth(&n, amt);
                }
            }
            Kind::NeckLight => {
                let n = self.neck();
                if n.d.iter().sum::<f32>() < 50.0 {
                    skipped = Some("no neck in the frame");
                } else {
                    self.neck_light(&n, amt);
                }
            }
        }
        let area: f32 = self.oval.d.iter().sum::<f32>().max(1.0);
        let moved: f32 = (0..self.lab.l.d.len())
            .filter(|&i| self.oval.d[i] > 0.5)
            .filter(|&i| {
                let d = (self.lab.l.d[i] - before.0.d[i]).abs()
                    + 0.6 * (self.lab.a.d[i] - before.1.d[i]).abs()
                    + 0.6 * (self.lab.b.d[i] - before.2.d[i]).abs();
                d > 2.0
            })
            .count() as f32;
        StepReport { kind, changed: (moved / area).min(1.0), found, skipped }
    }

    // ---- regions ---------------------------------------------------------------------------

    /// Distance along the face (forehead point to chin, in pixels) of a point.
    fn along(&self, p: [f32; 2]) -> f32 {
        let top = self.face.point(10).unwrap_or([0.0, 0.0]);
        (p[0] - top[0]) * self.face.down[0] + (p[1] - top[1]) * self.face.down[1]
    }

    /// 1 where the point is before `edge` (along the face), 0 after, with a soft transition.
    fn before_edge(&self, edge: f32, soft: f32) -> Plane {
        let (w, h) = (self.lab.w, self.lab.h);
        let top = self.face.point(10).unwrap_or([0.0, 0.0]);
        let d = self.face.down;
        let mut out = Plane::new(w, h, 0.0);
        for y in 0..h {
            for x in 0..w {
                let t = (x as f32 + 0.5 - top[0]) * d[0] + (y as f32 + 0.5 - top[1]) * d[1];
                out.d[y * w + x] = 1.0 - smoothstep(edge - soft, edge + soft, t);
            }
        }
        out
    }

    fn region_forehead(&self) -> Plane {
        let brows: Vec<f32> = [brow(&self.face, true), brow(&self.face, false)].iter().flatten().map(|p| self.along(*p)).collect();
        let Some(top_of_brows) = brows.iter().copied().reduce(f32::min) else { return Plane::new(self.lab.w, self.lab.h, 0.0) };
        let edge = top_of_brows - 0.03 * self.fw;
        self.before_edge(edge, 0.02 * self.fw).zip(&self.cov, |a, b| a * b)
    }

    fn region_eye_wrinkles(&self) -> Plane {
        let (w, h) = (self.lab.w, self.lab.h);
        let mut r = Plane::new(w, h, 0.0);
        for (outer, eye) in [(263usize, self.face.left_eye()), (33usize, self.face.right_eye())] {
            let (Some(c), false) = (self.face.point(outer), eye.is_empty()) else { continue };
            let mid = [eye.iter().map(|p| p[0]).sum::<f32>() / eye.len() as f32, eye.iter().map(|p| p[1]).sum::<f32>() / eye.len() as f32];
            let (dx, dy) = (c[0] - mid[0], c[1] - mid[1]);
            let n = dx.hypot(dy).max(1e-3);
            let ctr = [c[0] + dx / n * 0.045 * self.fw, c[1] + dy / n * 0.045 * self.fw];
            let d = facegeo::disc(w, h, ctr, 0.06 * self.fw, 0.05 * self.fw);
            r = r.zip(&d, f32::max);
        }
        // the band right under each eye too (fine lines of the lower lid)
        for left in [true, false] {
            if let Some(b) = self.face.under_eye(left, 0.05) {
                let f = fill(&[&b], w, h).gauss(0.008 * self.fw);
                r = r.zip(&f, |a, b| a.max(0.7 * b));
            }
        }
        r.zip(&self.cov, |a, b| a * b)
    }

    fn region_smile_lines(&self) -> Plane {
        let (w, h) = (self.lab.w, self.lab.h);
        let mut r = Plane::new(w, h, 0.0);
        for (wing, corner) in [(129usize, 61usize), (358, 291)] {
            let (Some(a), Some(b)) = (self.face.point(wing), self.face.point(corner)) else { continue };
            // the line sits a little outside the straight segment
            let mid = self.face.point(4).unwrap_or(a);
            let out = [b[0] - mid[0], 0.0];
            let n = out[0].abs().max(1e-3);
            let shift = 0.015 * self.fw * out[0] / n;
            let c = facegeo::capsule(w, h, [a[0] + shift, a[1]], [b[0] + shift, b[1]], 0.03 * self.fw, 0.03 * self.fw);
            r = r.zip(&c, f32::max);
        }
        r.zip(&self.cov, |a, b| a * b)
    }

    fn neck(&self) -> Plane {
        let (w, h) = (self.lab.w, self.lab.h);
        let Some(chin) = self.face.point(152) else { return Plane::new(w, h, 0.0) };
        let d = self.face.down;
        let ctr = [chin[0] + d[0] * 0.30 * self.fw, chin[1] + d[1] * 0.30 * self.fw];
        let (rx, ry) = (0.38 * self.fw, 0.36 * self.fw);
        // an ellipse, axes along and across the face
        let mut e = Plane::new(w, h, 0.0);
        let side = [-d[1], d[0]];
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5 - ctr[0], y as f32 + 0.5 - ctr[1]);
                let (u, v) = ((px * side[0] + py * side[1]) / rx, (px * d[0] + py * d[1]) / ry);
                let r = u.hypot(v);
                e.d[y * w + x] = 1.0 - smoothstep(0.85, 1.0, r);
            }
        }
        let m = self.skin.median;
        let like = |i: usize| {
            let (da, db) = (self.lab.a.d[i] - m[1], self.lab.b.d[i] - m[2]);
            let dl = (self.lab.l.d[i] - m[0]).abs();
            (1.0 - smoothstep(9.0, 20.0, da.hypot(db))) * (1.0 - smoothstep(28.0, 50.0, dl))
        };
        let mut out = Plane::new(w, h, 0.0);
        for i in 0..w * h {
            if e.d[i] > 0.0 {
                // below the jaw only: not the face itself
                out.d[i] = e.d[i] * (1.0 - self.oval.d[i]) * like(i);
            }
        }
        out.gauss(0.01 * self.fw)
    }

    // ---- steps -----------------------------------------------------------------------------

    /// Frequency-separation smoothing inside `weight`.
    fn smooth(&mut self, weight: &Plane, amt: f32) {
        let keep = 0.30;
        let sigma = 0.0055;
        let low = blur_weighted(&self.lab.l, weight, self.px(sigma));
        let detail = self.lab.l.zip(&low, |a, b| a - b);
        // a robust spread of the detail over the skin
        let vals: Vec<f32> = (0..detail.d.len()).step_by(3).filter(|&i| weight.d[i] > 0.6).map(|i| detail.d[i]).collect();
        let (_, mad) = crate::plane::median_mad(&vals);
        let sd = (1.4826 * mad).max(0.5);
        let n = self.lab.l.d.len();
        for i in 0..n {
            let wgt = weight.d[i];
            if wgt <= 0.0 {
                continue;
            }
            let dt = detail.d[i];
            // strong detail (edges, hairs, shadows lines) is left alone
            let gate = 1.0 - smoothstep(2.2 * sd, 5.0 * sd, dt.abs());
            let f = 1.0 - amt * (1.0 - keep) * gate;
            let target = low.d[i] + dt * f;
            self.lab.l.d[i] += (target - self.lab.l.d[i]) * wgt;
        }
        // colour: blotches go, the tone stays
        let la = blur_weighted(&self.lab.a, weight, self.px(0.012));
        let lb = blur_weighted(&self.lab.b, weight, self.px(0.012));
        for i in 0..n {
            let k = weight.d[i] * amt * 0.8;
            if k > 0.0 {
                self.lab.a.d[i] += (la.d[i] - self.lab.a.d[i]) * k;
                self.lab.b.d[i] += (lb.d[i] - self.lab.b.d[i]) * k;
            }
        }
    }

    /// Difference-of-Gaussians planes (small - large) of the three channels.
    fn dog(&self, s1: f32, s2: f32) -> (Plane, Plane, Plane) {
        let f = |p: &Plane| p.gauss(self.px(s1).max(0.6)).zip(&p.gauss(self.px(s2)), |a, b| a - b);
        (f(&self.lab.l), f(&self.lab.a), f(&self.lab.b))
    }

    fn inlier_spread(&self, p: &Plane) -> f32 {
        let vals: Vec<f32> = (0..p.d.len()).step_by(2).filter(|&i| self.skin.inlier.d[i] > 0.5).map(|i| p.d[i]).collect();
        let (_, mad) = crate::plane::median_mad(&vals);
        (1.4826 * mad).max(0.4)
    }

    /// Remove isolated marks: dark or red blobs, small and compact, on the skin.
    fn blemishes(&mut self, amt: f32) -> usize {
        let (dl, da, _) = self.dog(0.004, 0.014);
        let (sl, sa) = (self.inlier_spread(&dl), self.inlier_spread(&da));
        let k = 4.4 - 2.4 * amt;
        let (w, h) = (self.lab.w, self.lab.h);
        let mut cand = Plane::new(w, h, 0.0);
        for i in 0..w * h {
            if self.skin.weight.d[i] > 0.6 && (dl.d[i] < -k * sl || da.d[i] > k * sa * 1.2) {
                cand.d[i] = 1.0;
            }
        }
        let amin = ((0.0025 * self.fw).powi(2) * std::f32::consts::PI).max(3.0) as usize;
        let amax = ((0.03 * self.fw).powi(2) * std::f32::consts::PI) as usize;
        let mut mask = Plane::new(w, h, 0.0);
        let mut n = 0;
        for b in blobs(&cand, amax.max(amin + 1)) {
            if b.area < amin || b.aspect() > 3.2 || b.fill_ratio() < 0.35 {
                continue;
            }
            n += 1;
            for &i in &b.pixels {
                mask.d[i] = 1.0;
            }
        }
        if n == 0 {
            return 0;
        }
        let r = ((0.004 * self.fw).round() as usize).max(1);
        let grown = max_filter(&mask, r);
        let soft = grown.gauss(0.0025 * self.fw);
        // fill from the skin around, without the marks; keep a little grain
        let wt = self.skin.weight.zip(&grown, |s, g| s * (1.0 - g));
        let sig = self.px(0.010);
        let (fl, fa, fb) = (robust_blur(&self.lab.l, &wt, sig), robust_blur(&self.lab.a, &wt, sig), robust_blur(&self.lab.b, &wt, sig));
        let grain = 0.8 * self.inlier_spread(&self.hf_l());
        let strength = (amt * 1.4).min(1.0);
        for i in 0..w * h {
            let m = soft.d[i] * strength;
            if m > 0.0 {
                let noise = (hash(i as u32) - 0.5) * 2.0 * grain;
                self.lab.l.d[i] += (fl.d[i] + noise - self.lab.l.d[i]) * m;
                self.lab.a.d[i] += (fa.d[i] - self.lab.a.d[i]) * m;
                self.lab.b.d[i] += (fb.d[i] - self.lab.b.d[i]) * m;
            }
        }
        n
    }

    fn hf_l(&self) -> Plane {
        self.lab.l.zip(&self.lab.l.gauss(self.px(0.004).max(0.6)), |a, b| a - b)
    }

    /// Fade small dark and brownish spots without erasing them.
    fn freckles(&mut self, amt: f32) {
        let (dl, da, db) = self.dog(0.0025, 0.009);
        let sl = self.inlier_spread(&dl);
        let n = dl.d.len();
        for i in 0..n {
            let wgt = self.skin.weight.d[i];
            if wgt <= 0.0 {
                continue;
            }
            let d = -dl.d[i];
            // spots only: a dip of a couple of spreads, not the deep marks (blemishes) or edges
            let s = smoothstep(1.3 * sl, 3.0 * sl, d) * (1.0 - smoothstep(7.0 * sl, 12.0 * sl, d));
            let k = amt * 0.85 * s * wgt;
            if k > 0.0 {
                self.lab.l.d[i] += k * d;
                self.lab.a.d[i] -= k * da.d[i].max(0.0);
                self.lab.b.d[i] -= k * db.d[i].max(0.0);
            }
        }
    }

    /// Lighten the thin dark lines of a wrinkle region (the band-pass part only).
    fn wrinkles(&mut self, region: &Plane, amt: f32) {
        let (dl, _, _) = self.dog(0.003, 0.010);
        let sl = self.inlier_spread(&dl);
        let n = dl.d.len();
        for i in 0..n {
            let k = region.d[i] * self.cov.d[i];
            if k <= 0.0 {
                continue;
            }
            let line = (-dl.d[i]).max(0.0);
            // very deep lines are shadows of the face (nostril, lip crease), not wrinkles
            let keepmax = 1.0 - smoothstep(6.0 * sl, 10.0 * sl, line);
            self.lab.l.d[i] += amt * 0.9 * k * line * keepmax;
        }
    }

    fn dark_circles(&mut self, amt: f32) {
        let (w, h) = (self.lab.w, self.lab.h);
        let mut band = Plane::new(w, h, 0.0);
        for left in [true, false] {
            if let Some(b) = self.face.under_eye(left, 0.11) {
                band = band.zip(&fill(&[&b], w, h), f32::max);
            }
        }
        // a little outside the lid line so the shadow's edge is covered, then feathered
        let band = band.gauss(0.014 * self.fw);
        let eyes_out = {
            let (le, re) = (self.face.left_eye(), self.face.right_eye());
            fill(&[&le, &re], w, h).gauss(0.008 * self.fw).map(|v| (v * 2.0).min(1.0))
        };
        let region = band.zip(&self.oval, |b, o| b * o).zip(&eyes_out, |b, e| b * (1.0 - e));
        let reference = self.skin.median;
        let sig = self.px(0.012);
        let wt = region.map(|v| v.max(0.0001));
        let (ll, la, lb) = (blur_weighted(&self.lab.l, &wt, sig), blur_weighted(&self.lab.a, &wt, sig), blur_weighted(&self.lab.b, &wt, sig));
        for i in 0..w * h {
            let k = region.d[i];
            if k <= 0.0 {
                continue;
            }
            let lift = (reference[0] - ll.d[i]).clamp(0.0, 0.3 * reference[0]);
            self.lab.l.d[i] += amt * 0.9 * k * lift;
            // bluish / purple shadow back to the skin's own hue
            self.lab.a.d[i] += amt * 0.6 * k * (reference[1] - la.d[i]).clamp(-8.0, 8.0);
            self.lab.b.d[i] += amt * 0.6 * k * (reference[2] - lb.d[i]).clamp(-8.0, 8.0);
        }
    }

    fn shine(&mut self, amt: f32) {
        let m = self.skin.median;
        let sm_l = self.g(&self.lab.l, 0.012);
        let sm_a = self.g(&self.lab.a, 0.012);
        let sm_b = self.g(&self.lab.b, 0.012);
        let n = sm_l.d.len();
        for i in 0..n {
            let k = self.cov.d[i];
            if k <= 0.0 {
                continue;
            }
            let ex = sm_l.d[i] - m[0];
            let hot = smoothstep(5.0, 14.0, ex);
            if hot <= 0.0 {
                continue;
            }
            let w = amt * hot * k;
            // pull the smooth highlight down toward the skin's brightness; texture untouched
            self.lab.l.d[i] -= 0.85 * w * (sm_l.d[i] - (m[0] + 3.0)).max(0.0);
            // a specular highlight is desaturated: give the skin's colour back
            self.lab.a.d[i] += 0.5 * w * (m[1] - sm_a.d[i]);
            self.lab.b.d[i] += 0.5 * w * (m[2] - sm_b.d[i]);
        }
    }

    fn double_chin(&mut self, amt: f32) {
        let (w, h) = (self.lab.w, self.lab.h);
        let Some(chin) = self.face.point(152) else { return };
        let d = self.face.down;
        let ctr = [chin[0] + d[0] * 0.17 * self.fw, chin[1] + d[1] * 0.17 * self.fw];
        let side = [-d[1], d[0]];
        let (rx, ry) = (0.34 * self.fw, 0.2 * self.fw);
        let m = self.skin.median;
        let low = self.g(&self.lab.l, 0.02);
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5 - ctr[0], y as f32 + 0.5 - ctr[1]);
                let (u, v) = ((px * side[0] + py * side[1]) / rx, (px * d[0] + py * d[1]) / ry);
                let r = u.hypot(v);
                let e = 1.0 - smoothstep(0.7, 1.0, r);
                if e <= 0.0 {
                    continue;
                }
                let i = y * w + x;
                let (da, db) = (self.lab.a.d[i] - m[1], self.lab.b.d[i] - m[2]);
                let like = 1.0 - smoothstep(9.0, 20.0, da.hypot(db));
                let lift = (m[0] * 0.93 - low.d[i]).clamp(0.0, 0.25 * m[0]);
                self.lab.l.d[i] += amt * 0.8 * e * like * lift;
            }
        }
    }

    /// Whiten the teeth of an open mouth. `Some(reason)` when there is nothing to do.
    fn teeth(&mut self, amt: f32) -> Option<&'static str> {
        let (w, h) = (self.lab.w, self.lab.h);
        let inner = self.face.lips_inner();
        let area = poly_area(&inner);
        if area < 0.007 * self.fw * self.fw {
            return Some("mouth closed");
        }
        let cov = fill(&[&inner], w, h).gauss(0.004 * self.fw).map(|v| smoothstep(0.55, 0.95, v));
        let ls: Vec<f32> = (0..w * h).filter(|&i| cov.d[i] > 0.5).map(|i| self.lab.l.d[i]).collect();
        let q = crate::plane::quantile(&ls, 0.4);
        for i in 0..w * h {
            let k = cov.d[i];
            if k <= 0.0 {
                continue;
            }
            // teeth are light and not red (lips, gums and tongue are)
            let like = (1.0 - smoothstep(14.0, 24.0, self.lab.a.d[i])) * smoothstep(q - 6.0, q + 6.0, self.lab.l.d[i]) * k;
            if like <= 0.0 {
                continue;
            }
            let b = self.lab.b.d[i];
            if b > 3.0 {
                self.lab.b.d[i] -= amt * 0.85 * like * (b - 3.0);
            }
            self.lab.a.d[i] -= amt * 0.4 * like * self.lab.a.d[i].max(0.0);
            let l = self.lab.l.d[i];
            self.lab.l.d[i] = (l + amt * like * 7.0 * (1.0 - l / 100.0).max(0.0)).min(97.0).max(l);
        }
        None
    }

    fn eyes(&mut self, amt: f32) {
        let (w, h) = (self.lab.w, self.lab.h);
        let (le, re) = (self.face.left_eye(), self.face.right_eye());
        let opening = fill(&[&le, &re], w, h).gauss(0.003 * self.fw).map(|v| smoothstep(0.5, 0.95, v));
        // iris discs, to keep the whites' work off them and to give them contrast
        let mut iris = Plane::new(w, h, 0.0);
        for left in [true, false] {
            if let Some((c, r)) = self.face.iris(left) {
                iris = iris.zip(&facegeo::disc(w, h, c, r, 0.4 * r), f32::max);
            }
        }
        let local = self.g(&self.lab.l, 0.012);
        let hp = self.lab.l.zip(&local, |a, b| a - b);
        for i in 0..w * h {
            let k = opening.d[i];
            if k <= 0.0 {
                continue;
            }
            let l = self.lab.l.d[i];
            let white = (1.0 - iris.d[i]) * (1.0 - smoothstep(12.0, 22.0, self.lab.a.d[i].hypot(self.lab.b.d[i]))) * smoothstep(35.0, 60.0, l);
            if white > 0.0 {
                let t = amt * white * k;
                self.lab.l.d[i] = (l + t * 9.0 * (1.0 - l / 100.0)).min(97.0).max(l);
                self.lab.a.d[i] -= t * 0.5 * self.lab.a.d[i].max(0.0);
            }
            let ir = iris.d[i] * k;
            if ir > 0.0 {
                self.lab.l.d[i] += amt * ir * (0.9 * hp.d[i] + 2.5);
            }
        }
    }

    /// Remove thin hairs that cross smooth background, outside the face.
    fn flyaways(&mut self, amt: f32) -> usize {
        let (w, h) = (self.lab.w, self.lab.h);
        let r = ((0.010 * self.fw).round() as usize).max(1);
        // a ring around the head: outside the oval, within about half a face width of it
        let ring = {
            let outside = self.oval.map(|v| 1.0 - v);
            let near = max_filter(&self.oval, ((0.4 * self.fw) as usize).max(2).min(w.max(h)));
            outside.zip(&near, |o, n| o * n)
        };
        let l = &self.lab.l;
        let opened = max_filter(&min_filter(l, r), r);
        let closed = min_filter(&max_filter(l, r), r);
        let t = 22.0 - 14.0 * amt;
        let mut flag = Plane::new(w, h, 0.0);
        for i in 0..w * h {
            if ring.d[i] > 0.5 && (l.d[i] - opened.d[i] > t || closed.d[i] - l.d[i] > t) {
                flag.d[i] = 1.0;
            }
        }
        // only where the neighbourhood (flagged pixels left out) is smooth: a hair on a wall,
        // not the texture of the hair itself
        let grown = max_filter(&flag, 1);
        let bgw = grown.map(|v| 1.0 - v);
        let rr = self.px(0.05).max(3.0);
        let m1 = blur_weighted(&self.lab.l, &bgw, rr);
        let m2 = blur_weighted(&self.lab.l.map(|v| v * v), &bgw, rr);
        let mut mask = Plane::new(w, h, 0.0);
        let mut found = Plane::new(w, h, 0.0);
        for i in 0..w * h {
            if grown.d[i] > 0.5 {
                let sd = (m2.d[i] - m1.d[i] * m1.d[i]).max(0.0).sqrt();
                if sd < 9.0 {
                    mask.d[i] = 1.0;
                    found.d[i] = flag.d[i];
                }
            }
        }
        let n = blobs(&found, 4 * w * h / 5).iter().filter(|b| b.area >= 3).count();
        if n == 0 {
            return 0;
        }
        let soft = mask.gauss(0.002 * self.fw);
        let wt = bgw.zip(&ring, |b, rg| b * rg.max(0.3));
        let sig = self.px(0.008).max(1.5);
        let (fl, fa, fb) = (robust_blur(&self.lab.l, &wt, sig), robust_blur(&self.lab.a, &wt, sig), robust_blur(&self.lab.b, &wt, sig));
        for i in 0..w * h {
            let k = soft.d[i] * ring.d[i].min(1.0) * (amt * 1.4).min(1.0);
            if k > 0.0 {
                self.lab.l.d[i] += (fl.d[i] - self.lab.l.d[i]) * k;
                self.lab.a.d[i] += (fa.d[i] - self.lab.a.d[i]) * k;
                self.lab.b.d[i] += (fb.d[i] - self.lab.b.d[i]) * k;
            }
        }
        n
    }

    fn neck_light(&mut self, neck: &Plane, amt: f32) {
        let target = self.skin.median[0] * 0.97;
        let sig = self.px(0.02);
        let low = blur_weighted(&self.lab.l, neck, sig);
        for i in 0..self.lab.l.d.len() {
            let k = neck.d[i];
            if k > 0.0 {
                self.lab.l.d[i] += amt * 0.8 * k * (target - low.d[i]).clamp(0.0, 22.0);
            }
        }
    }
}

/// The brow polygon as a closed list (the person's left or right).
fn brow(face: &Face, left: bool) -> Poly {
    let idx = if left { &facegeo::LEFT_BROW } else { &facegeo::RIGHT_BROW };
    let top: Vec<[f32; 2]> = idx.iter().filter_map(|&i| face.point(i)).collect();
    // widen the line of points into a band along the face's vertical
    let off = 0.03 * face.width;
    let mut poly = top.clone();
    for p in top.iter().rev() {
        poly.push([p[0] + face.down[0] * off, p[1] + face.down[1] * off]);
    }
    poly
}

fn poly_area(p: &Poly) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (q, r) = (p[i], p[(i + 1) % p.len()]);
        a += q[0] * r[1] - r[0] * q[1];
    }
    a.abs() / 2.0
}

/// A deterministic 0..1 value for a pixel index (grain for filled marks).
fn hash(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xffff) as f32 / 65535.0
}
