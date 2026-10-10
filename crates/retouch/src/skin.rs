//! Skin measurement and harmonization fields.
//!
//! Colour is corrected on its low-frequency part only (the blotches, the cast, the dark circles'
//! hue), never on the pores: the planes here are blurred to a few percent of the face width and
//! the fine detail of the photo is never read back. Luminance keeps its volumes: the fields work
//! on the Lab `a`/`b` axes (and, for the cleanup dodge & burn, on one mid-frequency band of `L`).

use crate::lab::{de00, rgb8_to_lab};
use crate::plane::{Plane, blur_weighted, median_mad, quantile, smoothstep};

/// A developed photo as Lab planes.
pub struct LabImage {
    pub w: usize,
    pub h: usize,
    pub l: Plane,
    pub a: Plane,
    pub b: Plane,
}

impl LabImage {
    pub fn from_rgba(w: usize, h: usize, px: &[[u8; 4]]) -> LabImage {
        let (mut l, mut a, mut b) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h), Vec::with_capacity(w * h));
        for p in px.iter().take(w * h) {
            let v = rgb8_to_lab([p[0], p[1], p[2]]);
            l.push(v[0]);
            a.push(v[1]);
            b.push(v[2]);
        }
        LabImage { w, h, l: Plane::from_vec(w, h, l), a: Plane::from_vec(w, h, a), b: Plane::from_vec(w, h, b) }
    }

    #[inline]
    pub fn lab_at(&self, i: usize) -> [f32; 3] {
        [self.l.d[i], self.a.d[i], self.b.d[i]]
    }
}

/// The skin pixels of one face: soft weights, robust statistics.
pub struct Skin {
    /// 0..1: coverage of the face's skin region times the inlier test, softened at the edges.
    pub weight: Plane,
    /// 0 or 1: the pixels the statistics were made on (no highlights, hair, beard, deep shadow).
    pub inlier: Plane,
    pub n: usize,
    pub median: [f32; 3],
    pub mad: [f32; 3],
}

/// Select the skin of a face from its region coverage `cov` (0..1 per pixel). `soft_px`: blur
/// radius of the inlier mask's edge. `None` when fewer than 300 pixels qualify.
pub fn select(img: &LabImage, cov: &Plane, soft_px: f32) -> Option<Skin> {
    let n_px = img.w * img.h;
    let cand: Vec<usize> = (0..n_px).filter(|&i| cov.d[i] >= 0.85).collect();
    if cand.len() < 300 {
        return None;
    }
    let med = |idx: &[usize], f: &dyn Fn(usize) -> f32| {
        let v: Vec<f32> = idx.iter().map(|&i| f(i)).collect();
        median_mad(&v)
    };
    let mut keep = cand.clone();
    let mut stats = [(0.0, 0.0); 3];
    for _round in 0..3 {
        let (l, a, b) = (med(&keep, &|i| img.l.d[i]), med(&keep, &|i| img.a.d[i]), med(&keep, &|i| img.b.d[i]));
        stats = [l, a, b];
        let sl = (1.4826 * l.1).max(5.0);
        let sa = (1.4826 * a.1).max(2.5);
        let sb = (1.4826 * b.1).max(2.5);
        let chroma_med = a.0.hypot(b.0);
        keep = cand
            .iter()
            .copied()
            .filter(|&i| {
                let (dl, da, db) = (img.l.d[i] - l.0, img.a.d[i] - a.0, img.b.d[i] - b.0);
                let c = img.a.d[i].hypot(img.b.d[i]);
                dl > -2.8 * sl
                    && dl < 2.5 * sl
                    && da.abs() <= (3.5 * sa).max(14.0)
                    && db.abs() <= (3.5 * sb).max(14.0)
                    && !(dl > 10.0 && c < 0.75 * chroma_med)
            })
            .collect();
        if keep.len() < 300 {
            return None;
        }
    }
    let mut inlier = Plane::new(img.w, img.h, 0.0);
    for &i in &keep {
        inlier.d[i] = 1.0;
    }
    let soft = inlier.gauss(soft_px.max(0.5));
    let weight = cov.zip(&soft, |c, s| c * s);
    Some(Skin { weight, inlier, n: keep.len(), median: [stats[0].0, stats[1].0, stats[2].0], mad: [stats[0].1, stats[1].1, stats[2].1] })
}

impl Skin {
    /// The same pixels measured on another render of the photo (after an edit): the weights are
    /// kept, the median and spread are taken again over the same inlier pixels.
    pub fn restat(&self, img: &LabImage) -> Skin {
        let idx: Vec<usize> = (0..img.w * img.h).filter(|&i| self.inlier.d[i] > 0.5).collect();
        let m = |f: &dyn Fn(usize) -> f32| median_mad(&idx.iter().map(|&i| f(i)).collect::<Vec<_>>());
        let (l, a, b) = (m(&|i| img.l.d[i]), m(&|i| img.a.d[i]), m(&|i| img.b.d[i]));
        Skin { weight: self.weight.clone(), inlier: self.inlier.clone(), n: self.n, median: [l.0, a.0, b.0], mad: [l.1, a.1, b.1] }
    }
}

/// Low-frequency `a` and `b` of the skin (weighted blur, sigma in pixels).
pub fn lowpass_ab(img: &LabImage, skin: &Skin, sigma: f32) -> (Plane, Plane) {
    (blur_weighted(&img.a, &skin.inlier, sigma), blur_weighted(&img.b, &skin.inlier, sigma))
}

/// What is measured on a face's skin.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub n: usize,
    pub median: [f32; 3],
    pub chroma: f32,
    pub hue_deg: f32,
    /// Mean CIEDE2000 between the low-frequency colour of each skin pixel and the face's median
    /// colour, luminosity excluded (each pixel keeps the median lightness).
    pub de_color_mean: f32,
    pub de_color_p95: f32,
    /// Spread (standard deviation) of the mid-frequency band of L on the skin.
    pub l_band_sd: f32,
}

pub fn metrics(img: &LabImage, skin: &Skin, sigma_lp: f32, sigma_big: f32) -> Metrics {
    let (la, lb) = lowpass_ab(img, skin, sigma_lp);
    let m = skin.median;
    let mut des = Vec::new();
    for i in 0..img.w * img.h {
        if skin.inlier.d[i] > 0.5 {
            des.push(de00([m[0], la.d[i], lb.d[i]], m));
        }
    }
    let mean = if des.is_empty() { 0.0 } else { des.iter().sum::<f32>() / des.len() as f32 };
    let band = l_band(img, skin, sigma_lp, sigma_big);
    let vals: Vec<f32> = (0..img.w * img.h).filter(|&i| skin.inlier.d[i] > 0.5).map(|i| band.d[i]).collect();
    let sd = if vals.is_empty() {
        0.0
    } else {
        let mu = vals.iter().sum::<f32>() / vals.len() as f32;
        (vals.iter().map(|v| (v - mu).powi(2)).sum::<f32>() / vals.len() as f32).sqrt()
    };
    Metrics {
        n: skin.n,
        median: m,
        chroma: m[1].hypot(m[2]),
        hue_deg: crate::lab::hue_deg(m),
        de_color_mean: mean,
        de_color_p95: quantile(&des, 0.95),
        l_band_sd: sd,
    }
}

/// The mid-frequency band of L on the skin: blur(`sigma_small`) minus blur(`sigma_big`), the part
/// the cleanup dodge & burn evens out (under-eye shadows, patches, folds), not the pores and not
/// the face's volumes.
pub fn l_band(img: &LabImage, skin: &Skin, sigma_small: f32, sigma_big: f32) -> Plane {
    let s = blur_weighted(&img.l, &skin.inlier, sigma_small);
    let b = blur_weighted(&img.l, &skin.inlier, sigma_big);
    s.zip(&b, |a, c| a - c)
}

/// Variance of the high-frequency detail of L on the skin (pores, hairs, fine texture): L minus
/// its blur of `sigma` pixels, over the inlier pixels.
pub fn hf_variance(img: &LabImage, skin: &Skin, sigma: f32) -> f32 {
    let lp = blur_weighted(&img.l, &skin.inlier, sigma);
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for i in 0..img.w * img.h {
        if skin.inlier.d[i] > 0.5 {
            let d = f64::from(img.l.d[i] - lp.d[i]);
            s += d * d;
            n += 1.0;
        }
    }
    if n == 0.0 { 0.0 } else { (s / n) as f32 }
}

/// Like [`hf_variance`] on the logarithm of the luminance (the log of Y, from L): a pore keeps its
/// relative contrast when the skin around it is lightened or darkened, so a dodge or a burn
/// leaves this number alone while a smoothing lowers it.
pub fn hf_variance_log(img: &LabImage, skin: &Skin, sigma: f32) -> f32 {
    let logy = img.l.map(|l| 3.0 * ((l + 16.0) / 116.0).max(1e-3).ln());
    let lp = blur_weighted(&logy, &skin.inlier, sigma);
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for i in 0..img.w * img.h {
        if skin.inlier.d[i] > 0.5 {
            let d = f64::from(logy.d[i] - lp.d[i]);
            s += d * d;
            n += 1.0;
        }
    }
    if n == 0.0 { 0.0 } else { (s / n) as f32 }
}

/// A wanted colour shift in Lab units, per pixel.
pub struct Field {
    pub da: Plane,
    pub db: Plane,
}

/// The shift that brings the skin's low-frequency colour `ku` of the way to its own median
/// (uniformity inside the photo) and the median `ks` of the way to `target` (a colour shared with
/// the rest of a series). The series part is capped at `max_shift` Lab units (a real change of
/// light is only partly corrected).
pub fn harmonize_field(lp: &(Plane, Plane), own: [f32; 2], target: [f32; 2], ku: f32, ks: f32, max_shift: f32) -> Field {
    let (mut sa, mut sb) = (target[0] - own[0], target[1] - own[1]);
    let mag = sa.hypot(sb);
    let cap = max_shift / ks.max(1e-3);
    if mag > cap {
        let k = cap / mag;
        sa *= k;
        sb *= k;
    }
    let (ca, cb) = (ks * sa, ks * sb);
    let da = lp.0.map(|v| -ku * (v - own[0]) + ca);
    let db = lp.1.map(|v| -ku * (v - own[1]) + cb);
    Field { da, db }
}

/// A signed field as soft masks (alpha 0..1, always positive): `neg` where the field wants to go
/// down, `pos` where it wants to go up; alpha = |field| / `dref`, so that one local adjustment
/// of size `dref` per mask rebuilds the field.
pub struct SignedMasks {
    pub dref: f32,
    pub pos: Plane,
    pub neg: Plane,
}

pub fn signed_masks(field: &Plane, weight: &Plane, dref: f32) -> SignedMasks {
    let pos = field.zip(weight, |f, w| w * (f / dref).clamp(0.0, 1.0));
    let neg = field.zip(weight, |f, w| w * (-f / dref).clamp(0.0, 1.0));
    SignedMasks { dref, pos, neg }
}

/// The reference size of a pair of fields: the 98th percentile of their magnitudes inside the
/// skin, at least `min`.
pub fn dref_of(fields: &[&Plane], inlier: &Plane, min: f32) -> f32 {
    let mut v = Vec::new();
    for f in fields {
        v.extend((0..f.d.len()).filter(|&i| inlier.d[i] > 0.5).map(|i| f.d[i].abs()));
    }
    quantile(&v, 0.98).max(min)
}

/// Cleanup dodge & burn field: bring the mid-frequency band of L `k` of the way to zero. Soft
/// limit at `limit` L units so that an outlier (a spot, a fold) is never dodged to white.
pub fn cleanup_field(band: &Plane, k: f32, limit: f32) -> Plane {
    band.map(|v| {
        let t = (v / limit).clamp(-3.0, 3.0);
        -k * limit * t.tanh()
    })
}

/// Vectorscope: angle (degrees) of an sRGB colour (0..1 per channel) in the Cb/Cr plane
/// (BT.601), 0 = +Cb. Skin tones sit close to [`SKIN_LINE_DEG`] whatever the person.
pub fn vectorscope_deg(rgb: [f32; 3]) -> f32 {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let cb = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    let a = cr.atan2(cb).to_degrees();
    if a < 0.0 { a + 360.0 } else { a }
}

/// The skin line of the vectorscope (the angle skin tones gather around).
pub const SKIN_LINE_DEG: f32 = 123.0;

/// Is the difference between two photos' skin a cast over the whole picture rather than a real
/// difference of skin? `skin_shift` and `neutral_shift` are the changes (Lab `a`, `b`) of the
/// skin median and of the neutral parts (greys, whites, walls) between two photos. When the
/// neutrals moved in the same direction and by a comparable amount, it is a cast.
pub fn is_cast(skin_shift: [f32; 2], neutral_shift: [f32; 2]) -> bool {
    let (ms, mn) = (skin_shift[0].hypot(skin_shift[1]), neutral_shift[0].hypot(neutral_shift[1]));
    if ms < 1.0 || mn < 1.0 {
        return false;
    }
    let cos = (skin_shift[0] * neutral_shift[0] + skin_shift[1] * neutral_shift[1]) / (ms * mn);
    cos > 0.7 && mn > 0.5 * ms
}

/// A soft edge for how far a pixel's colour is from `median`, used by tests and tools: 0 inside
/// `lo`, 1 beyond `hi` (CIEDE2000, luminosity excluded).
pub fn deviation(lab: [f32; 3], median: [f32; 3], lo: f32, hi: f32) -> f32 {
    smoothstep(lo, hi, de00([median[0], lab[1], lab[2]], median))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic face: a 200 x 200 image, skin ellipse with texture, a red patch, a hair band.
    fn face(redness: f32) -> (LabImage, Plane) {
        let (w, h) = (200usize, 200usize);
        let mut px = vec![[20u8, 20, 20, 255]; w * h];
        let mut cov = Plane::new(w, h, 0.0);
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 16) & 0xff) as f32 / 255.0 - 0.5
        };
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = ((x as f32 - 100.0) / 80.0, (y as f32 - 100.0) / 90.0);
                if dx * dx + dy * dy < 1.0 {
                    cov.d[y * w + x] = 1.0;
                    // a dark brown skin, texture of +-4 levels
                    let n = noise() * 8.0;
                    let mut c = [110.0 + n, 72.0 + n, 55.0 + n];
                    // a red cheek patch
                    let (px_, py_) = ((x as f32 - 140.0) / 18.0, (y as f32 - 110.0) / 18.0);
                    if px_ * px_ + py_ * py_ < 1.0 {
                        c[0] += redness;
                        c[1] -= redness * 0.4;
                        c[2] -= redness * 0.4;
                    }
                    // a hair band across the forehead (excluded by the trimming)
                    if y < 35 {
                        c = [25.0, 20.0, 18.0];
                    }
                    px[y * w + x] = [c[0].clamp(0.0, 255.0) as u8, c[1].clamp(0.0, 255.0) as u8, c[2].clamp(0.0, 255.0) as u8, 255];
                }
            }
        }
        (LabImage::from_rgba(w, h, &px), cov)
    }

    #[test]
    fn selection_drops_hair_and_a_red_patch_stays_in() {
        let (img, cov) = face(20.0);
        let s = select(&img, &cov, 1.5).expect("skin");
        assert!(s.inlier.d[20 * 200 + 100] < 0.5, "the hair band is out");
        assert!(s.inlier.d[110 * 200 + 140] > 0.5, "the red patch is skin to correct");
        assert!(s.median[0] > 20.0 && s.median[1] > 5.0, "{:?}", s.median);
    }

    #[test]
    fn the_harmonize_field_pulls_a_patch_to_the_median_and_keeps_the_rest() {
        let (img, cov) = face(20.0);
        let skin = select(&img, &cov, 1.5).expect("skin");
        let lp = lowpass_ab(&img, &skin, 5.0);
        let own = [skin.median[1], skin.median[2]];
        let f = harmonize_field(&lp, own, own, 1.0, 0.0, 6.0);
        let i = 110 * 200 + 140;
        assert!(f.da.d[i] < -2.0, "less red on the patch: {}", f.da.d[i]);
        let j = 130 * 200 + 60;
        assert!(f.da.d[j].abs() < 1.0 && f.db.d[j].abs() < 1.0, "the rest is left");
    }

    #[test]
    fn a_series_target_shifts_the_whole_face_and_is_capped() {
        let (img, cov) = face(0.0);
        let skin = select(&img, &cov, 1.5).expect("skin");
        let lp = lowpass_ab(&img, &skin, 5.0);
        let own = [skin.median[1], skin.median[2]];
        let f = harmonize_field(&lp, [own[0], own[1]], [own[0] + 20.0, own[1]], 0.0, 1.0, 6.0);
        let j = 130 * 200 + 60;
        assert!((f.da.d[j] - 6.0).abs() < 0.2, "capped at 6: {}", f.da.d[j]);
    }

    #[test]
    fn signed_masks_rebuild_the_field_from_two_alphas() {
        let field = Plane::from_vec(4, 1, vec![-4.0, -1.0, 0.0, 3.0]);
        let w = Plane::new(4, 1, 1.0);
        let m = signed_masks(&field, &w, 4.0);
        assert_eq!(m.neg.d, vec![1.0, 0.25, 0.0, 0.0]);
        assert_eq!(m.pos.d, vec![0.0, 0.0, 0.0, 0.75]);
        let rebuilt: Vec<f32> = (0..4).map(|i| (m.pos.d[i] - m.neg.d[i]) * m.dref).collect();
        assert_eq!(rebuilt, vec![-4.0, -1.0, 0.0, 3.0]);
    }

    #[test]
    fn metrics_see_the_blotch_and_texture_variance_is_measured_on_fine_detail() {
        let (img, cov) = face(20.0);
        let skin = select(&img, &cov, 1.5).expect("skin");
        let m = metrics(&img, &skin, 5.0, 18.0);
        let (flat, c2) = face(0.0);
        let s2 = select(&flat, &c2, 1.5).expect("skin");
        let m2 = metrics(&flat, &s2, 5.0, 18.0);
        assert!(m.de_color_mean > m2.de_color_mean + 0.2, "{} vs {}", m.de_color_mean, m2.de_color_mean);
        assert!(hf_variance(&img, &skin, 2.0) > 0.1, "texture exists");
    }

    #[test]
    fn the_cleanup_field_evens_the_band_and_limits_outliers() {
        let band = Plane::from_vec(3, 1, vec![-6.0, 0.0, 60.0]);
        let f = cleanup_field(&band, 0.5, 6.0);
        assert!(f.d[0] > 1.5 && f.d[1] == 0.0 && f.d[2] < 0.0 && f.d[2] > -3.1, "{:?}", f.d);
    }

    #[test]
    fn the_vectorscope_puts_skin_near_the_skin_line_and_flags_casts() {
        for rgb in [[0.78, 0.55, 0.42], [0.42, 0.28, 0.2], [0.95, 0.8, 0.7]] {
            let a = vectorscope_deg(rgb);
            assert!((a - SKIN_LINE_DEG).abs() < 14.0, "{rgb:?}: {a}");
        }
        assert!(is_cast([3.0, 4.0], [2.5, 3.5]) && !is_cast([3.0, 4.0], [-2.0, 0.5]) && !is_cast([3.0, 4.0], [0.2, 0.1]));
    }
}
