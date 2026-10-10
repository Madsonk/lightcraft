//! Float pixel planes: blurs, weighted blurs, sampling and robust statistics.

#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub d: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize, v: f32) -> Plane {
        Plane { w, h, d: vec![v; w * h] }
    }

    pub fn from_vec(w: usize, h: usize, d: Vec<f32>) -> Plane {
        assert_eq!(d.len(), w * h, "plane size");
        Plane { w, h, d }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.d[y * self.w + x]
    }

    /// Bilinear sample at pixel-centre coordinates (`x`, `y` in pixels; the centre of pixel 0 is 0.5).
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let (fx, fy) = ((x - 0.5).clamp(0.0, (self.w - 1) as f32), (y - 0.5).clamp(0.0, (self.h - 1) as f32));
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let a = self.at(x0, y0) * (1.0 - tx) + self.at(x1, y0) * tx;
        let b = self.at(x0, y1) * (1.0 - tx) + self.at(x1, y1) * tx;
        a * (1.0 - ty) + b * ty
    }

    pub fn map(&self, f: impl Fn(f32) -> f32) -> Plane {
        Plane { w: self.w, h: self.h, d: self.d.iter().map(|v| f(*v)).collect() }
    }

    pub fn zip(&self, o: &Plane, f: impl Fn(f32, f32) -> f32) -> Plane {
        assert_eq!((self.w, self.h), (o.w, o.h), "plane sizes differ");
        Plane { w: self.w, h: self.h, d: self.d.iter().zip(&o.d).map(|(a, b)| f(*a, *b)).collect() }
    }

    pub fn mean_where(&self, weight: &Plane, min: f32) -> f32 {
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for (v, w) in self.d.iter().zip(&weight.d) {
            if *w >= min {
                s += f64::from(*v);
                n += 1.0;
            }
        }
        if n == 0.0 { 0.0 } else { (s / n) as f32 }
    }

    /// Box blur of radius `r` pixels (window 2r+1), edges clamped.
    pub fn box_blur(&self, r: usize) -> Plane {
        if r == 0 {
            return self.clone();
        }
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0f32; w * h];
        let win = (2 * r + 1) as f32;
        for y in 0..h {
            let row = &self.d[y * w..(y + 1) * w];
            let mut acc: f32 = (0..=r).map(|i| row[i.min(w - 1)]).sum::<f32>() + row[0] * r as f32;
            for x in 0..w {
                tmp[y * w + x] = acc / win;
                let add = row[(x + r + 1).min(w - 1)];
                let sub = row[x.saturating_sub(r)];
                acc += add - sub;
            }
        }
        let mut out = vec![0f32; w * h];
        for x in 0..w {
            let mut acc: f32 = (0..=r).map(|i| tmp[i.min(h - 1) * w + x]).sum::<f32>() + tmp[x] * r as f32;
            for y in 0..h {
                out[y * w + x] = acc / win;
                let add = tmp[(y + r + 1).min(h - 1) * w + x];
                let sub = tmp[y.saturating_sub(r) * w + x];
                acc += add - sub;
            }
        }
        Plane { w, h, d: out }
    }

    /// Gaussian blur approximated by three box blurs (sigma in pixels).
    pub fn gauss(&self, sigma: f32) -> Plane {
        if sigma < 0.5 {
            return self.clone();
        }
        // three boxes of width n: variance 3 (n^2 - 1) / 12
        let n = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt()).round().max(1.0) as usize;
        let r = (n.saturating_sub(1)) / 2;
        self.box_blur(r).box_blur(r).box_blur(r)
    }
}

/// Weighted blur (normalized convolution): `blur(v * w) / blur(w)`. Where the weights are nil the
/// result is extrapolated from the neighbourhood (and 0 when nothing is near).
pub fn blur_weighted(v: &Plane, weight: &Plane, sigma: f32) -> Plane {
    let vw = v.zip(weight, |a, b| a * b);
    let num = vw.gauss(sigma);
    let den = weight.gauss(sigma);
    num.zip(&den, |n, d| if d > 1e-4 { n / d } else { 0.0 })
}

/// The `p`-quantile (0..1) of the values (sorts a copy); 0 for no values.
pub fn quantile(vals: &[f32], p: f32) -> f32 {
    if vals.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f32> = vals.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    let i = (p.clamp(0.0, 1.0) * (v.len() - 1) as f32).round() as usize;
    v[i]
}

/// Median and median absolute deviation.
pub fn median_mad(vals: &[f32]) -> (f32, f32) {
    let m = quantile(vals, 0.5);
    let dev: Vec<f32> = vals.iter().map(|v| (v - m).abs()).collect();
    (m, quantile(&dev, 0.5))
}

/// Smooth step from 0 at `lo` to 1 at `hi`.
pub fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    if hi <= lo {
        return if x >= hi { 1.0 } else { 0.0 };
    }
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_blur_keeps_a_constant_and_spreads_an_impulse() {
        let c = Plane::new(20, 15, 3.0).box_blur(4);
        assert!(c.d.iter().all(|v| (v - 3.0).abs() < 1e-4));
        let mut p = Plane::new(21, 21, 0.0);
        p.d[10 * 21 + 10] = 1.0;
        let b = p.box_blur(2);
        assert!((b.d.iter().sum::<f32>() - 1.0).abs() < 1e-4, "energy kept");
        assert!((b.at(10, 10) - 1.0 / 25.0).abs() < 1e-5 && b.at(13, 10) == 0.0);
    }

    #[test]
    fn gauss_has_about_the_asked_sigma() {
        let mut p = Plane::new(101, 1, 0.0);
        p.d[50] = 1.0;
        let g = p.gauss(6.0);
        let var: f32 = g.d.iter().enumerate().map(|(i, v)| v * (i as f32 - 50.0).powi(2)).sum();
        assert!((var.sqrt() - 6.0).abs() < 1.0, "sigma {}", var.sqrt());
    }

    #[test]
    fn weighted_blur_ignores_zero_weight_pixels() {
        let mut v = Plane::new(30, 30, 10.0);
        let mut w = Plane::new(30, 30, 1.0);
        for y in 10..20 {
            for x in 10..20 {
                v.d[y * 30 + x] = 1000.0;
                w.d[y * 30 + x] = 0.0;
            }
        }
        let b = blur_weighted(&v, &w, 3.0);
        assert!((b.at(15, 15) - 10.0).abs() < 0.5, "{}", b.at(15, 15));
    }

    #[test]
    fn quantile_median_and_sampling() {
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.5), 3.0);
        assert_eq!(median_mad(&[1.0, 2.0, 3.0, 4.0, 100.0]), (3.0, 1.0));
        let p = Plane::from_vec(2, 1, vec![0.0, 10.0]);
        assert!((p.sample(1.0, 0.5) - 5.0).abs() < 1e-4);
        assert!(smoothstep(0.0, 1.0, 0.5) == 0.5);
    }
}
