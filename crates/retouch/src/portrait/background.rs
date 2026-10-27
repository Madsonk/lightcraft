//! Background replacement from a subject matte: blur, flat colour, dimming or brightening, a
//! drop shadow, zoom and rotation of the background, and an edge that is cleaned of the old
//! background's colour. The matte comes from a segmentation model in the engine; here it is just
//! a plane of 0..1 (1 = subject).

use super::raster::{max_filter, resize};
use crate::plane::{Plane, blur_weighted, smoothstep};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Gaussian blur of the old background (the subject's colours are not pulled in).
    Blur,
    /// A flat colour.
    Color([u8; 3]),
    /// The old background, darker (`dim < 0`) or brighter (`dim > 0`).
    Dim,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub mode: Mode,
    /// 0..1: blur size.
    pub amount: f32,
    /// -1..1 for [`Mode::Dim`]; also applied (lightly) to the other modes.
    pub dim: f32,
    /// 0..1 shadow of the subject on the background.
    pub shadow: f32,
    /// 1.0 = none; 1.3 = background enlarged by 30 %.
    pub zoom: f32,
    pub rotate_deg: f32,
    /// 0..1: how hard the matte's edge is made.
    pub edge: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params { mode: Mode::Blur, amount: 0.5, dim: 0.0, shadow: 0.0, zoom: 1.0, rotate_deg: 0.0, edge: 0.5 }
    }
}

/// Tighten the matte's edge: `edge` 0 keeps it, 1 makes a narrow transition.
pub fn refine_matte(m: &Plane, edge: f32) -> Plane {
    let width = 0.5 - 0.42 * edge.clamp(0.0, 1.0);
    m.map(|v| smoothstep(0.5 - width, 0.5 + width, v.clamp(0.0, 1.0)))
}

/// Composite: the subject of `rgb` (interleaved 8-bit, `w x h`) over the new background.
/// Returns the new pixels (same size). `matte` must have the image's size.
pub fn replace(rgb: &[u8], w: usize, h: usize, matte: &Plane, p: &Params) -> Option<Vec<u8>> {
    if matte.w != w || matte.h != h || rgb.len() < w * h * 3 || w == 0 || h == 0 {
        return None;
    }
    let m = refine_matte(matte, p.edge);
    let long = w.max(h) as f32;
    let chan = |c: usize| Plane::from_vec(w, h, (0..w * h).map(|i| f32::from(rgb[i * 3 + c])).collect());
    let src = [chan(0), chan(1), chan(2)];

    // the subject's colour, cleaned at the edge: semi-transparent pixels take the colour of the
    // subject's inside
    let inside = m.map(|v| smoothstep(0.85, 1.0, v));
    let sig_fg = (0.0025 * long).max(1.0);
    let fg: Vec<Plane> = src
        .iter()
        .map(|s| {
            let est = blur_weighted(s, &inside, sig_fg);
            let k = m.map(|v| 3.0 * v * (1.0 - v));
            let mut out = s.clone();
            for i in 0..out.d.len() {
                out.d[i] += (est.d[i] - out.d[i]) * k.d[i];
            }
            out
        })
        .collect();

    // the new background layer
    let bg: Vec<Plane> = match p.mode {
        Mode::Color(c) => (0..3).map(|k| Plane::new(w, h, f32::from(c[k]))).collect(),
        Mode::Blur | Mode::Dim => {
            let wbg = max_filter(&m, ((0.003 * long) as usize).max(1)).map(|v| 1.0 - v);
            let sigma = if p.mode == Mode::Blur { (0.004 + 0.036 * p.amount) * long } else { 0.0 };
            src.iter()
                .map(|s| {
                    let base = if sigma > 0.5 { blurred(s, &wbg, sigma) } else { s.clone() };
                    transform(&base, p.zoom, p.rotate_deg)
                })
                .collect()
        }
    };
    let gain = 2f32.powf(p.dim.clamp(-1.0, 1.0) * if p.mode == Mode::Dim { 1.6 } else { 0.8 });
    let shadow = if p.shadow > 0.0 {
        let off = (0.012 * long) as isize;
        let mut moved = Plane::new(w, h, 0.0);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as isize - off, y as isize - 2 * off);
                if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    moved.d[y * w + x] = m.d[sy as usize * w + sx as usize];
                }
            }
        }
        Some(moved.gauss(0.014 * long).map(|v| v * p.shadow.clamp(0.0, 1.0) * 0.6))
    } else {
        None
    };
    let mut out = vec![0u8; w * h * 3];
    for i in 0..w * h {
        let a = m.d[i];
        let s = shadow.as_ref().map_or(0.0, |s| s.d[i]);
        for c in 0..3 {
            let b = bg[c].d[i] * gain * (1.0 - s);
            let v = fg[c].d[i] * a + b * (1.0 - a);
            out[i * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    Some(out)
}

/// Blur at a quarter of the resolution when the radius is big (the result is blurry anyway).
fn blurred(s: &Plane, wbg: &Plane, sigma: f32) -> Plane {
    if sigma < 6.0 {
        return blur_weighted(s, wbg, sigma);
    }
    let f = 4usize;
    let (sw, sh) = ((s.w / f).max(1), (s.h / f).max(1));
    let small = blur_weighted(&resize(s, sw, sh), &resize(wbg, sw, sh), sigma / f as f32);
    resize(&small, s.w, s.h)
}

/// Zoom (about the centre) and rotate a plane, bilinear, edges clamped.
fn transform(p: &Plane, zoom: f32, deg: f32) -> Plane {
    if (zoom - 1.0).abs() < 1e-3 && deg.abs() < 1e-3 {
        return p.clone();
    }
    let (w, h) = (p.w, p.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let z = zoom.max(0.2);
    let (s, c) = (-deg.to_radians()).sin_cos();
    let mut out = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 + 0.5 - cx) / z, (y as f32 + 0.5 - cy) / z);
            out.d[y * w + x] = p.sample(cx + dx * c - dy * s, cy + dx * s + dy * c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> (Vec<u8>, Plane) {
        let (w, h) = (80usize, 60usize);
        let mut rgb = vec![0u8; w * h * 3];
        let mut m = Plane::new(w, h, 0.0);
        for y in 0..h {
            for x in 0..w {
                let inside = (x as f32 - 40.0).powi(2) / 400.0 + (y as f32 - 30.0).powi(2) / 400.0 < 1.0;
                let c = if inside {
                    [200u8, 140, 110]
                } else if (x / 4 + y / 4) % 2 == 0 {
                    [30, 90, 40]
                } else {
                    [90, 160, 80]
                };
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
                m.d[y * w + x] = if inside { 1.0 } else { 0.0 };
            }
        }
        (rgb, m)
    }

    #[test]
    fn a_colour_replaces_the_background_and_the_subject_stays() {
        let (rgb, m) = scene();
        let out = replace(&rgb, 80, 60, &m, &Params { mode: Mode::Color([10, 20, 200]), ..Params::default() }).expect("out");
        let at = |x: usize, y: usize| [out[(y * 80 + x) * 3], out[(y * 80 + x) * 3 + 1], out[(y * 80 + x) * 3 + 2]];
        assert_eq!(at(40, 30), [200, 140, 110]);
        assert_eq!(at(2, 2), [10, 20, 200]);
    }

    #[test]
    fn blur_flattens_the_background_pattern_without_pulling_in_the_subject() {
        let (rgb, m) = scene();
        let out = replace(&rgb, 80, 60, &m, &Params { mode: Mode::Blur, amount: 0.8, ..Params::default() }).expect("out");
        let g = |x: usize, y: usize| i32::from(out[(y * 80 + x) * 3 + 1]);
        assert!((g(2, 2) - g(6, 2)).abs() < 40, "checker flattened: {} {}", g(2, 2), g(6, 2));
        // near the subject the background does not turn skin-coloured (red channel stays low)
        let r = i32::from(out[(30 * 80 + 12) * 3]);
        assert!(r < 110, "no subject colour in the background: {r}");
    }

    #[test]
    fn dimming_zoom_and_shadow_run() {
        let (rgb, m) = scene();
        let dark = replace(&rgb, 80, 60, &m, &Params { mode: Mode::Dim, dim: -0.8, shadow: 0.8, zoom: 1.2, rotate_deg: 5.0, ..Params::default() })
            .expect("out");
        assert!(dark[0] < rgb[0] + 1 || dark[1] < rgb[1], "background darker");
        assert!(replace(&rgb, 10, 10, &m, &Params::default()).is_none(), "size mismatch refused");
    }
}
