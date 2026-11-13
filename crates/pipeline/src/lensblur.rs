//! Lens Blur: a disc-shaped aperture blur whose size follows the photo's depth.
//!
//! The photo is split into *layers* by signed circle of confusion (negative = behind the focal
//! plane, positive = in front), each pixel belonging to the two nearest layers. Every layer is
//! blurred with a disc of its own radius (premultiplied by its membership, so a layer only
//! spreads its own pixels), and the layers are composited back to front. That is what keeps the
//! in-focus subject sharp, lets a blurred foreground bleed over the sharp area behind it, and
//! avoids the halo a plain "blur by depth" puts around a sharp subject. The final division by
//! the accumulated coverage extends the visible neighbours into what the blur uncovers.
//! Runs in scene-linear light, so highlights keep their weight in the bokeh.

use lightcraft_develop::{DepthMap, LensBlur};
use lightcraft_raster::{Plane, Rgb32f};

use crate::geometry::Frame;
use crate::masks::guided_cross;

/// Largest blur radius, as a fraction of the long edge, at Amount 100.
pub const MAX_RADIUS: f64 = 0.02;
/// Largest radius the disc kernel handles at working resolution (larger radii are blurred on a
/// reduced copy).
const KERNEL_RADIUS: f32 = 5.0;
/// Layers on each side of the focal plane.
const MAX_LAYERS: usize = 10;

/// The depth plane at output resolution: the stored map sampled at each output pixel's place in
/// the photo, then snapped to the photo's own edges (guided filter on `guide`'s luminance) so a
/// depth edge meets the object's edge, not a blurred version of it.
pub fn depth_plane(depth: &DepthMap, frame: &Frame, w: usize, h: usize, guide: &Rgb32f) -> Plane {
    let m = frame.out_to_norm(w, h);
    let mut p = Plane::new(w, h);
    crate::for_rows(&mut p.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let n = m.apply(lightcraft_geom::Point::new(x as f64 + 0.5, y as f64 + 0.5));
            *v = depth.at(n.x, n.y);
        }
    });
    let g = Plane {
        width: w,
        height: h,
        data: guide.data.iter().map(|c| (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).max(0.0).sqrt().min(1.5)).collect(),
    };
    let sigma = (0.004 * w.max(h) as f32).max(1.0);
    guided_cross(&g, &p, sigma, 1e-3)
}

/// Blur radius in pixels at Amount `amount` (0..100) for a render `px_per_long` pixels per unit of
/// the photo's long edge.
pub fn max_radius_px(amount: f64, px_per_long: f64) -> f32 {
    (amount / 100.0 * MAX_RADIUS * px_per_long) as f32
}

/// The signed circle of confusion (pixels; positive = nearer than the focal plane) of depth `d`
/// (0 far … 1 near).
pub fn signed_coc(d: f32, lb: &LensBlur, radius: f32) -> f32 {
    let f = (lb.focal_distance / 100.0).clamp(0.0, 1.0) as f32;
    let half = (lb.focal_range / 200.0).clamp(0.0, 0.5) as f32;
    let off = d - f;
    let s = if off > half {
        off - half
    } else if off < -half {
        off + half
    } else {
        0.0
    };
    // half the depth range of distance from the focal band = the full blur
    (s / 0.5).clamp(-1.0, 1.0) * radius
}

/// Blur `img` (scene-linear, output resolution) by `depth` (from [`depth_plane`]).
pub fn apply(img: &mut Rgb32f, depth: &Plane, lb: &LensBlur, px_per_long: f64) {
    let radius = max_radius_px(lb.amount, px_per_long);
    if !lb.enabled || radius < 0.6 || depth.data.len() != img.data.len() {
        return;
    }
    let (w, h) = (img.width, img.height);
    let layers = ((radius / 1.5).round() as usize).clamp(2, MAX_LAYERS);
    // layer radii (pixels), on each side: 0, then growing steps
    let radii: Vec<f32> = (0..=layers).map(|j| radius * (j as f32 / layers as f32).powf(1.6)).collect();
    let coc: Vec<f32> = depth.data.iter().map(|d| signed_coc(*d, lb, radius)).collect();
    // each pixel's continuous position along the signed layer axis (-layers … layers): it
    // belongs to the two layers around it, in proportion
    let pos: Vec<f32> = coc
        .iter()
        .map(|c| {
            let a = c.abs();
            let j = radii.iter().rposition(|r| *r <= a).unwrap_or(0).min(layers - 1);
            let u = j as f32 + ((a - radii[j]) / (radii[j + 1] - radii[j]).max(1e-6)).clamp(0.0, 1.0);
            if *c < 0.0 { -u } else { u }
        })
        .collect();
    let mut acc = vec![[0f32; 4]; w * h]; // premultiplied colour, coverage
    // back (most negative) to front
    for k in -(layers as i32)..=layers as i32 {
        let weight: Vec<f32> = pos.iter().map(|u| (1.0 - (u - k as f32).abs()).max(0.0)).collect();
        if weight.iter().all(|v| *v == 0.0) {
            continue;
        }
        let r = radii[k.unsigned_abs() as usize];
        let mut layer: Vec<[f32; 4]> = img.data.iter().zip(&weight).map(|(c, wk)| [c[0] * wk, c[1] * wk, c[2] * wk, *wk]).collect();
        if r >= 0.5 {
            layer = disc_blur(&layer, w, h, r);
        }
        for (a, l) in acc.iter_mut().zip(&layer) {
            let keep = 1.0 - l[3].clamp(0.0, 1.0);
            for c in 0..3 {
                a[c] = a[c] * keep + l[c];
            }
            a[3] = a[3] * keep + l[3];
        }
    }
    for (c, a) in img.data.iter_mut().zip(&acc) {
        if a[3] > 1e-4 {
            *c = [a[0] / a[3], a[1] / a[3], a[2] / a[3]];
        }
    }
}

/// A disc of radius `r` (pixels) over premultiplied RGBA: directly for small radii, on a reduced
/// copy for large ones.
fn disc_blur(src: &[[f32; 4]], w: usize, h: usize, r: f32) -> Vec<[f32; 4]> {
    let f = (r / KERNEL_RADIUS).ceil().max(1.0) as usize;
    if f == 1 {
        return disc_conv(src, w, h, r);
    }
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    // box-reduce
    let mut small = vec![[0f32; 4]; sw * sh];
    crate::for_rows(&mut small, sw, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut s, mut n) = ([0f32; 4], 0f32);
            for yy in (y * f)..((y + 1) * f).min(h) {
                for xx in (x * f)..((x + 1) * f).min(w) {
                    let p = src[yy * w + xx];
                    for c in 0..4 {
                        s[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            *o = s.map(|v| v / n.max(1.0));
        }
    });
    let blurred = disc_conv(&small, sw, sh, r / f as f32);
    // bilinear back up (cell centres)
    let mut out = vec![[0f32; 4]; w * h];
    crate::for_rows(&mut out, w, |y, row| {
        let fy = ((y as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let (a, b, c, d) = (blurred[y0 * sw + x0], blurred[y0 * sw + x1], blurred[y1 * sw + x0], blurred[y1 * sw + x1]);
            for k in 0..4 {
                o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
            }
        }
    });
    out
}

/// Direct convolution with an anti-aliased disc (weight 1 inside, a one-pixel ramp at the edge).
fn disc_conv(src: &[[f32; 4]], w: usize, h: usize, r: f32) -> Vec<[f32; 4]> {
    let ri = (r + 0.5).ceil() as i32;
    let mut taps: Vec<(i32, i32, f32)> = Vec::new();
    let mut total = 0.0;
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            let wt = (r + 0.5 - d).clamp(0.0, 1.0);
            if wt > 0.0 {
                taps.push((dx, dy, wt));
                total += wt;
            }
        }
    }
    let inv = 1.0 / total.max(1e-6);
    let mut out = vec![[0f32; 4]; w * h];
    crate::for_rows(&mut out, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = [0f32; 4];
            for &(dx, dy, wt) in &taps {
                let xx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                let yy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                let p = src[yy * w + xx];
                s[0] += p[0] * wt;
                s[1] += p[1] * wt;
                s[2] += p[2] * wt;
                s[3] += p[3] * wt;
            }
            *o = s.map(|v| v * inv);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lb(amount: f64, focus: f64, range: f64) -> LensBlur {
        LensBlur { enabled: true, amount, focal_distance: focus, focal_range: range, ..LensBlur::default() }
    }

    #[test]
    fn coc_is_zero_in_the_focal_band_and_signed_outside() {
        let l = lb(100.0, 50.0, 20.0);
        assert_eq!(signed_coc(0.5, &l, 10.0), 0.0);
        assert_eq!(signed_coc(0.58, &l, 10.0), 0.0);
        assert!(signed_coc(0.9, &l, 10.0) > 0.0, "nearer than the focus: positive");
        assert!(signed_coc(0.1, &l, 10.0) < 0.0, "farther: negative");
        assert_eq!(signed_coc(1.0, &l, 10.0), 10.0 * ((0.5f32 - 0.1) / 0.5));
    }

    #[test]
    fn sharp_subject_stays_sharp_and_background_blurs_without_a_halo() {
        // a bright square (near) on a checkered background (far); focus on the square
        let (w, h) = (96, 96);
        let mut img = Rgb32f::new(w, h);
        let mut depth = Plane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let inside = (32..64).contains(&x) && (32..64).contains(&y);
                img.data[y * w + x] = if inside {
                    [0.9, 0.2, 0.2]
                } else if (x / 4 + y / 4) % 2 == 0 {
                    [0.1, 0.1, 0.1]
                } else {
                    [0.5, 0.5, 0.5]
                };
                depth.data[y * w + x] = if inside { 0.9 } else { 0.1 };
            }
        }
        let orig = img.clone();
        // amount 100 on a 96 px photo: radius 0.02 × 96 ≈ 1.9; use px_per_long to get ~8 px
        apply(&mut img, &depth, &lb(100.0, 90.0, 10.0), 400.0);
        // the subject's interior is untouched
        assert_eq!(img.data[48 * w + 48], orig.data[48 * w + 48]);
        assert_eq!(img.data[40 * w + 40], orig.data[40 * w + 40]);
        // the checkers far from the square are smoothed to their mean
        let far = img.data[5 * w + 5];
        assert!((far[0] - 0.3).abs() < 0.12, "{far:?}");
        // no red leaks into the background beside the square
        let beside = img.data[48 * w + 20];
        assert!(beside[0] - beside[1] < 0.05, "halo: {beside:?}");
    }
}
