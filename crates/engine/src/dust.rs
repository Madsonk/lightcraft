//! Persistent sensor dust: spots that sit at the same place on every photo of a series.
//!
//! Dust on the sensor shows as small soft dark discs that do not move from one frame to the
//! next, while everything the camera sees does. [`Accumulator`] looks at each photo (a
//! luminance plane of a fixed size), marks the pixels that are a little darker than their
//! surroundings *in smooth areas* (sky, walls, out-of-focus backgrounds: where dust can be
//! judged at all), and counts, per pixel, how often that happened out of how often the pixel
//! could be judged. Pixels that are dark in most of the photos where they could be judged are
//! dust; neighbouring ones are merged into spots ([`Accumulator::spots`]).
//!
//! The engine commands `dust.detect` / `dust.apply` (see `cmd/dust.rs`) feed it renders and put
//! the spots on the photos as heal spots.

/// A dust spot: centre and radius as fractions of the plane's width / long edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DustSpot {
    pub x: f64,
    pub y: f64,
    /// Fraction of the long edge.
    pub radius: f64,
    /// Photos in which the spot was seen / in which its place could be judged.
    pub seen: u32,
    pub judged: u32,
}

/// Tunables (the defaults are what the commands use).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Radius (px) of the neighbourhood the spot must be darker than.
    pub background: usize,
    /// Smallest darkening (fraction of full scale) that counts.
    pub min_depth: f32,
    /// Share of the photos judged at a place in which it must be dark.
    pub min_ratio: f32,
    /// Photos in which a place must have been judged (at least).
    pub min_judged: u32,
    /// ... and at least this share of the series (a place judged in 3 of 40 photos is no evidence).
    pub min_judged_share: f32,
    /// Spot area limits (px of the plane).
    pub min_area: usize,
    pub max_area: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self { background: 16, min_depth: 0.012, min_ratio: 0.6, min_judged: 3, min_judged_share: 0.3, min_area: 3, max_area: 2500 }
    }
}

pub struct Accumulator {
    pub w: usize,
    pub h: usize,
    hit: Vec<u16>,
    judged: Vec<u16>,
    pub photos: u32,
    p: Params,
}

/// Box blur (mean) of radius `r`, edges clamped, via a running sum on each axis.
pub fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut s: f32 = (0..=r).map(|i| row[i.min(w - 1)]).sum::<f32>() + row[0] * r as f32;
        for x in 0..w {
            tmp[y * w + x] = s / n;
            let add = row[(x + r + 1).min(w - 1)];
            let sub = row[x.saturating_sub(r)];
            s += add - sub;
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        let col = |y: usize| tmp[y.min(h - 1) * w + x];
        let mut s: f32 = (0..=r).map(col).sum::<f32>() + tmp[x] * r as f32;
        for y in 0..h {
            out[y * w + x] = s / n;
            s += col(y + r + 1) - tmp[y.saturating_sub(r) * w + x];
        }
    }
    out
}

impl Accumulator {
    pub fn new(w: usize, h: usize, p: Params) -> Self {
        Self { w, h, hit: vec![0; w * h], judged: vec![0; w * h], photos: 0, p }
    }

    /// Add one photo: `luma` is `w * h` values in 0..1, display-encoded.
    pub fn add(&mut self, luma: &[f32]) {
        let (w, h, r) = (self.w, self.h, self.p.background);
        if luma.len() != w * h {
            return;
        }
        let m = box_blur(luma, w, h, 1);
        let bg = box_blur(luma, w, h, r);
        // fine texture (noise, detail) and the large-scale variation around each pixel
        let tex_in: Vec<f32> = luma.iter().zip(&m).map(|(a, b)| (a - b).abs()).collect();
        let tex = box_blur(&tex_in, w, h, 3);
        let var_in: Vec<f32> = m.iter().zip(&bg).map(|(a, b)| (a - b).abs()).collect();
        let var = box_blur(&var_in, w, h, r);
        for i in 0..w * h {
            // judged: the surroundings are smooth, whatever the pixel itself does
            if var[i] > 0.025 || tex[i] > 0.02 || bg[i] < 0.04 || bg[i] > 0.97 {
                continue;
            }
            self.judged[i] = self.judged[i].saturating_add(1);
            let depth = bg[i] - m[i];
            if depth > self.p.min_depth.max(2.5 * tex[i]) {
                self.hit[i] = self.hit[i].saturating_add(1);
            }
        }
        self.photos += 1;
    }

    /// The share of the frame that could be judged in enough photos to find dust there at all
    /// (dark areas, busy areas and edges cannot be judged: dust on them is not seen).
    pub fn coverage(&self) -> f64 {
        let need = self.p.min_judged.max((self.p.min_judged_share * self.photos as f32).ceil() as u32);
        self.judged.iter().filter(|&&j| u32::from(j) >= need).count() as f64 / (self.w * self.h).max(1) as f64
    }

    /// The persistent spots found so far.
    pub fn spots(&self) -> Vec<DustSpot> {
        let (w, h) = (self.w, self.h);
        let mut mask = vec![false; w * h];
        let need = self.p.min_judged.max((self.p.min_judged_share * self.photos as f32).ceil() as u32);
        for i in 0..w * h {
            let j = u32::from(self.judged[i]);
            mask[i] = j >= need && f32::from(self.hit[i]) >= self.p.min_ratio * j as f32;
        }
        // close one-pixel gaps, then label 8-connected components
        let mut grown = mask.clone();
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                if !mask[y * w + x] {
                    let n = mask[y * w + x - 1] as u8 + mask[y * w + x + 1] as u8 + mask[(y - 1) * w + x] as u8 + mask[(y + 1) * w + x] as u8;
                    grown[y * w + x] = n >= 3;
                }
            }
        }
        let long = w.max(h) as f64;
        let mut seen = vec![false; w * h];
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for s in 0..w * h {
            if !grown[s] || seen[s] {
                continue;
            }
            let (mut sx, mut sy, mut n) = (0f64, 0f64, 0usize);
            let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0usize, usize::MAX, 0usize);
            let (mut hits, mut judged) = (0u32, 0u32);
            stack.push(s);
            seen[s] = true;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                sx += x as f64;
                sy += y as f64;
                n += 1;
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
                hits = hits.max(u32::from(self.hit[i]));
                judged = judged.max(u32::from(self.judged[i]));
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let j = ny as usize * w + nx as usize;
                        if grown[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
            if n < self.p.min_area || n > self.p.max_area {
                continue;
            }
            // a dust disc is compact: a long thin run is an edge or a wire, not dust
            let (bw, bh) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
            if bw.max(bh) > 3.0 * bw.min(bh) + 2.0 || (n as f64) < 0.4 * bw * bh {
                continue;
            }
            let radius = (bw.max(bh) / 2.0 + 1.0).max((n as f64 / std::f64::consts::PI).sqrt());
            out.push(DustSpot {
                x: (sx / n as f64 + 0.5) / w as f64,
                y: (sy / n as f64 + 0.5) / h as f64,
                radius: radius / long,
                seen: hits,
                judged,
            });
        }
        out.sort_by(|a, b| b.seen.cmp(&a.seen).then(a.y.total_cmp(&b.y)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth gradient sky with noise, plus (optionally) clouds that differ per photo.
    fn photo(seed: u32, w: usize, h: usize, dust: &[(f32, f32, f32)]) -> Vec<f32> {
        let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(12345);
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) - 0.5
        };
        let mut v = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let sky = 0.55 + 0.2 * (y as f32 / h as f32) + 0.01 * rnd();
                // a different bright blob per photo (scene content moves)
                let (cx, cy) = (((seed * 37) % w as u32) as f32, ((seed * 53) % h as u32) as f32);
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let cloud = 0.15 * (-d * d / 900.0).exp();
                v[y * w + x] = sky + cloud;
            }
        }
        for &(dx, dy, dr) in dust {
            let (cx, cy, r) = (dx * w as f32, dy * h as f32, dr * w as f32);
            for y in 0..h {
                for x in 0..w {
                    let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                    if d < r * 1.6 {
                        v[y * w + x] -= 0.05 * (-(d * d) / (2.0 * (r * 0.7).powi(2))).exp();
                    }
                }
            }
        }
        v
    }

    #[test]
    fn spots_fixed_across_photos_are_found_and_moving_content_is_not() {
        let (w, h) = (400, 300);
        let dust = [(0.30, 0.40, 0.012), (0.72, 0.65, 0.016)];
        let mut acc = Accumulator::new(w, h, Params::default());
        for seed in 1..=8 {
            acc.add(&photo(seed, w, h, &dust));
        }
        let spots = acc.spots();
        assert_eq!(spots.len(), 2, "{spots:?}");
        for d in dust {
            let near = spots.iter().find(|s| (s.x - f64::from(d.0)).abs() < 0.01 && (s.y - f64::from(d.1)).abs() < 0.012);
            assert!(near.is_some(), "no spot near {d:?} in {spots:?}");
        }
        assert!(spots.iter().all(|s| s.seen >= 6 && s.judged <= 8 && s.radius > 0.004));
    }

    #[test]
    fn a_clean_series_has_no_spots_and_too_few_photos_find_nothing() {
        let (w, h) = (300, 200);
        let mut acc = Accumulator::new(w, h, Params::default());
        for seed in 1..=6 {
            acc.add(&photo(seed, w, h, &[]));
        }
        assert!(acc.spots().is_empty());
        let mut two = Accumulator::new(w, h, Params::default());
        for seed in 1..=2 {
            two.add(&photo(seed, w, h, &[(0.5, 0.5, 0.015)]));
        }
        assert!(two.spots().is_empty(), "min_judged is 3");
    }

    #[test]
    fn box_blur_of_a_constant_is_the_constant() {
        let v = box_blur(&vec![0.4; 30 * 20], 30, 20, 5);
        assert!(v.iter().all(|x| (x - 0.4).abs() < 1e-5));
    }
}
