//! Small raster helpers for the portrait operations: scanline polygon fill, sliding-window
//! extrema, connected components and bilinear resampling of float planes.

use crate::plane::Plane;

pub type Poly = Vec<[f32; 2]>;

/// Coverage (0..1) of the union of `polys` on a `w x h` plane: four sub-scanlines per row and
/// exact horizontal coverage of the spans (fast enough for multi-megapixel crops).
pub fn fill(polys: &[&Poly], w: usize, h: usize) -> Plane {
    let polys: Vec<&&Poly> = polys.iter().filter(|p| p.len() >= 3).collect();
    exact_sum(polys.as_slice(), w, h)
}

fn exact_sum(polys: &[&&Poly], w: usize, h: usize) -> Plane {
    const SUB: usize = 4;
    let mut out = Plane::new(w, h, 0.0);
    let mut xs: Vec<f32> = Vec::new();
    let mut spans: Vec<(f32, f32)> = Vec::new();
    for y in 0..h {
        for s in 0..SUB {
            let yy = y as f32 + (s as f32 + 0.5) / SUB as f32;
            spans.clear();
            for p in polys {
                xs.clear();
                let n = p.len();
                for i in 0..n {
                    let (a, b) = (p[i], p[(i + 1) % n]);
                    if (a[1] <= yy) != (b[1] <= yy) {
                        let t = (yy - a[1]) / (b[1] - a[1]);
                        xs.push(a[0] + t * (b[0] - a[0]));
                    }
                }
                xs.sort_by(f32::total_cmp);
                spans.extend(xs.chunks_exact(2).map(|c| (c[0].max(0.0), c[1].min(w as f32))).filter(|(a, b)| b > a));
            }
            if spans.is_empty() {
                continue;
            }
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            // union of the spans
            let mut merged: Vec<(f32, f32)> = Vec::with_capacity(spans.len());
            for &(a, b) in &spans {
                match merged.last_mut() {
                    Some(l) if a <= l.1 => l.1 = l.1.max(b),
                    _ => merged.push((a, b)),
                }
            }
            for (x0, x1) in merged {
                let (i0, i1) = (x0.floor() as usize, (x1.ceil() as usize).min(w));
                for i in i0..i1 {
                    let cov = (x1.min(i as f32 + 1.0) - x0.max(i as f32)).max(0.0);
                    out.d[y * w + i] += cov / SUB as f32;
                }
            }
        }
    }
    out.d.iter_mut().for_each(|v| *v = v.min(1.0));
    out
}

/// Sliding-window maximum of radius `r` along rows, then columns (a square structuring element).
pub fn max_filter(p: &Plane, r: usize) -> Plane {
    if r == 0 {
        return p.clone();
    }
    let (w, h) = (p.w, p.h);
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        window_extreme(&p.d[y * w..(y + 1) * w], r, true, |x, v| tmp[y * w + x] = v);
    }
    let mut out = vec![0f32; w * h];
    let mut col = vec![0f32; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = tmp[y * w + x];
        }
        window_extreme(&col, r, true, |y, v| out[y * w + x] = v);
    }
    Plane { w, h, d: out }
}

pub fn min_filter(p: &Plane, r: usize) -> Plane {
    let neg = p.map(|v| -v);
    max_filter(&neg, r).map(|v| -v)
}

/// Monotonic-deque window extreme of radius `r` (window `2r + 1`, clamped at the ends).
fn window_extreme(line: &[f32], r: usize, max: bool, mut put: impl FnMut(usize, f32)) {
    use std::collections::VecDeque;
    let n = line.len();
    let mut dq: VecDeque<usize> = VecDeque::new();
    let better = |a: f32, b: f32| if max { a >= b } else { a <= b };
    let mut next = 0usize;
    for i in 0..n {
        let hi = (i + r).min(n - 1);
        while next <= hi {
            while let Some(&b) = dq.back() {
                if better(line[next], line[b]) {
                    dq.pop_back();
                } else {
                    break;
                }
            }
            dq.push_back(next);
            next += 1;
        }
        while let Some(&f) = dq.front() {
            if f + r < i {
                dq.pop_front();
            } else {
                break;
            }
        }
        if let Some(&f) = dq.front() {
            put(i, line[f]);
        }
    }
}

/// One 4-connected component of a binary mask.
#[derive(Clone, Debug)]
pub struct Blob {
    pub area: usize,
    pub bbox: [usize; 4],
    pub pixels: Vec<usize>,
}

impl Blob {
    pub fn bbox_w(&self) -> usize {
        self.bbox[2] - self.bbox[0] + 1
    }
    pub fn bbox_h(&self) -> usize {
        self.bbox[3] - self.bbox[1] + 1
    }
    /// Long side over short side of the bounding box.
    pub fn aspect(&self) -> f32 {
        let (a, b) = (self.bbox_w() as f32, self.bbox_h() as f32);
        a.max(b) / a.min(b).max(1.0)
    }
    /// Share of the bounding box the blob fills.
    pub fn fill_ratio(&self) -> f32 {
        self.area as f32 / (self.bbox_w() * self.bbox_h()) as f32
    }
}

/// The 4-connected components of the pixels where `mask > 0.5`, ignoring blobs bigger than
/// `max_area` (they are not collected, only skipped).
pub fn blobs(mask: &Plane, max_area: usize) -> Vec<Blob> {
    let (w, h) = (mask.w, mask.h);
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] || mask.d[start] <= 0.5 {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut pixels = Vec::new();
        let mut bb = [usize::MAX, usize::MAX, 0, 0];
        let mut too_big = false;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x), bb[3].max(y)];
            if !too_big {
                pixels.push(i);
                if pixels.len() > max_area {
                    too_big = true;
                    pixels = Vec::new();
                }
            }
            let mut push = |j: usize| {
                if !seen[j] && mask.d[j] > 0.5 {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if !too_big {
            out.push(Blob { area: pixels.len(), bbox: bb, pixels });
        }
    }
    out
}

/// Bilinear resample of a plane to `nw x nh`.
pub fn resize(p: &Plane, nw: usize, nh: usize) -> Plane {
    if (p.w, p.h) == (nw, nh) {
        return p.clone();
    }
    if nw == 0 || nh == 0 || p.w == 0 || p.h == 0 {
        return Plane::new(nw, nh, 0.0);
    }
    // shrinking by more than 2x: average first so thin features are not skipped
    let src = if p.w >= 2 * nw || p.h >= 2 * nh { box_down(p, (p.w / nw).max(1), (p.h / nh).max(1)) } else { p.clone() };
    let (sx, sy) = (src.w as f32 / nw as f32, src.h as f32 / nh as f32);
    let mut out = Plane::new(nw, nh, 0.0);
    for y in 0..nh {
        for x in 0..nw {
            out.d[y * nw + x] = src.sample((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
        }
    }
    out
}

fn box_down(p: &Plane, fx: usize, fy: usize) -> Plane {
    let (nw, nh) = ((p.w / fx).max(1), (p.h / fy).max(1));
    let mut out = Plane::new(nw, nh, 0.0);
    for y in 0..nh {
        for x in 0..nw {
            let (mut s, mut n) = (0.0f32, 0.0f32);
            for j in y * fy..((y + 1) * fy).min(p.h) {
                for i in x * fx..((x + 1) * fx).min(p.w) {
                    s += p.d[j * p.w + i];
                    n += 1.0;
                }
            }
            out.d[y * nw + x] = if n > 0.0 { s / n } else { 0.0 };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_covers_a_rectangle_exactly_and_unions_overlaps() {
        let a: Poly = vec![[2.0, 2.0], [8.0, 2.0], [8.0, 6.0], [2.0, 6.0]];
        let b: Poly = vec![[5.0, 4.0], [11.5, 4.0], [11.5, 8.0], [5.0, 8.0]];
        let f = fill(&[&a, &b], 14, 10);
        assert_eq!(f.at(3, 3), 1.0);
        assert_eq!(f.at(6, 5), 1.0, "overlap counts once");
        assert!((f.at(11, 5) - 0.5).abs() < 1e-5, "half pixel at x = 11.5: {}", f.at(11, 5));
        assert_eq!(f.at(0, 0), 0.0);
        assert!((f.d.iter().sum::<f32>() - (24.0 + 26.0 - 6.0 + 0.0)).abs() < 1.0);
    }

    #[test]
    fn extrema_and_blobs_and_resize() {
        let mut p = Plane::new(9, 1, 0.0);
        p.d[4] = 5.0;
        let m = max_filter(&p, 2);
        assert_eq!(m.d, vec![0.0, 0.0, 5.0, 5.0, 5.0, 5.0, 5.0, 0.0, 0.0]);
        assert!(min_filter(&m, 2).d.iter().all(|v| *v <= 5.0));
        let mut mask = Plane::new(8, 8, 0.0);
        for (x, y) in [(1, 1), (2, 1), (1, 2), (6, 6)] {
            mask.d[y * 8 + x] = 1.0;
        }
        let bl = blobs(&mask, 100);
        assert_eq!(bl.len(), 2);
        assert!(bl.iter().any(|b| b.area == 3) && bl.iter().any(|b| b.area == 1));
        assert!(blobs(&mask, 2).len() == 1, "bigger blobs skipped");
        let big = resize(&Plane::new(40, 40, 3.0), 10, 10);
        assert!(big.d.iter().all(|v| (v - 3.0).abs() < 1e-4));
    }
}
