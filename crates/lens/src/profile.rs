//! From a lens calibration to the polynomials the render pipeline applies.
//!
//! The database gives, per focal length, a radial distortion model, per focal length a lateral chromatic
//! aberration model and per focal length / aperture / focus distance a vignetting polynomial. The pipeline's lens
//! stage takes, per colour plane, `source = d·(k0 + k1·r² + k2·r⁴ + k3·r⁶)` and a vignetting gain
//! `1 + k0·r² + … + k4·r¹⁰`, with `r` the distance from the image centre over the image's half-diagonal. So the
//! calibration is evaluated at the shot's focal length, aperture and distance (interpolating between calibrated
//! values), re-expressed in the image's own radius, and fitted by least squares onto those polynomials. The fit
//! error is returned and tested; nothing here guesses data the calibration does not have.

use crate::{DistModel, Distortion, LensEntry, Query, Tca, TcaModel, Vignetting};

/// Radial samples for the fits: `r = i / SAMPLES` over the image (centre to corner).
const SAMPLES: usize = 48;
/// The brightness the vignetting model may fall to before the gain is capped (3 stops).
const MIN_BRIGHTNESS: f64 = 0.125;

/// The corrections of one photo, ready to become the pipeline's embedded-lens data.
#[derive(Clone, Debug, PartialEq)]
pub struct LensCorrection {
    /// Per colour plane (R, G, B) the coefficients `[k0, k1, k2, k3]` of the source/destination radius ratio as a
    /// polynomial in `r²`, scaled so that the image corner keeps its position.
    pub planes: Option<[[f64; 4]; 3]>,
    /// Coefficients `[k0 … k4]` of the vignetting gain minus one, as a polynomial in `r²` (powers 1 to 5).
    pub vignette: Option<[f64; 5]>,
    /// Calibration frame crop factor over the camera's: the image radius `r` is `crop_ratio × r_lens`.
    pub crop_ratio: f64,
    /// Largest absolute error of the polynomial fits against the exact models (ratio units / gain units).
    pub fit_error: f64,
}

impl LensCorrection {
    /// Source/destination radius ratio of plane `ch` at image radius `r`.
    pub fn ratio(&self, ch: usize, r: f64) -> f64 {
        match &self.planes {
            Some(p) => {
                let k = p[ch.min(2)];
                let r2 = r * r;
                k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]))
            }
            None => 1.0,
        }
    }
    /// Vignetting gain at image radius `r`.
    pub fn gain(&self, r: f64) -> f64 {
        match &self.vignette {
            Some(k) => {
                let r2 = r * r;
                1.0 + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))))
            }
            None => 1.0,
        }
    }
}

/// Least-squares fit of `y ≈ Σ_j c_j · r^(2(first + j))` for `j < terms` over the `(r, y)` samples. `None` when
/// the system is degenerate.
pub fn fit_even(samples: &[(f64, f64)], first: usize, terms: usize) -> Option<Vec<f64>> {
    if terms == 0 || terms > 8 || samples.len() < terms {
        return None;
    }
    let basis = |r: f64, j: usize| r.powi(2 * (first + j) as i32);
    let n = terms;
    let mut a = vec![vec![0.0; n + 1]; n];
    for &(r, y) in samples {
        for i in 0..n {
            let bi = basis(r, i);
            for j in 0..n {
                a[i][j] += bi * basis(r, j);
            }
            a[i][n] += bi * y;
        }
    }
    // Gauss-Jordan with partial pivoting
    for col in 0..n {
        let piv = (col..n).max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap_or(std::cmp::Ordering::Equal))?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        let d = a[col][col];
        for v in a[col].iter_mut() {
            *v /= d;
        }
        for row in 0..n {
            if row != col {
                let f = a[row][col];
                if f != 0.0 {
                    for k in col..=n {
                        let t = a[col][k];
                        a[row][k] -= f * t;
                    }
                }
            }
        }
    }
    Some((0..n).map(|i| a[i][n]).collect())
}

/// Position of `x` among sorted keys: the two neighbours and the weight of the second (clamped at the ends).
fn bracket(keys: &[f64], x: f64) -> Option<(usize, usize, f64)> {
    let last = keys.len().checked_sub(1)?;
    if keys.len() == 1 || x <= keys[0] {
        return Some((0, 0, 0.0));
    }
    if x >= keys[last] {
        return Some((last, last, 0.0));
    }
    let hi = keys.iter().position(|k| *k >= x)?;
    let lo = hi.saturating_sub(1);
    let span = keys[hi] - keys[lo];
    Some((lo, hi, if span > 0.0 { (x - keys[lo]) / span } else { 0.0 }))
}

fn distortion_ratio(d: &Distortion, r: f64) -> f64 {
    let r = r.max(1e-9);
    let [k1, k2, k3] = d.k;
    match d.model {
        DistModel::Ptlens => k1 * r.powi(3) + k2 * r * r + k3 * r + (1.0 - k1 - k2 - k3),
        DistModel::Poly3 => 1.0 - k1 + k1 * r * r,
        DistModel::Poly5 => 1.0 + k1 * r * r + k2 * r.powi(4),
    }
}

fn tca_ratios(t: &Tca, rd: f64) -> (f64, f64) {
    match t.model {
        TcaModel::Linear => (t.k[0], t.k[1]),
        TcaModel::Poly3 => {
            let [vr, vb, cr, cb, br, bb] = t.k;
            (br * rd * rd + cr * rd + vr, bb * rd * rd + cb * rd + vb)
        }
    }
}

/// Entries of one focal length each, sorted by focal length (the first of equal ones wins).
fn by_focal<T: Clone>(items: &[T], focal: impl Fn(&T) -> f64) -> Vec<T> {
    let mut v: Vec<T> = items.to_vec();
    v.sort_by(|a, b| focal(a).partial_cmp(&focal(b)).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup_by(|b, a| (focal(a) - focal(b)).abs() < 1e-9);
    v
}

/// Brightness at lens radius `r` (the vignetting model).
fn brightness(k: [f64; 3], r: f64) -> f64 {
    let r2 = r * r;
    1.0 + r2 * (k[0] + r2 * (k[1] + r2 * k[2]))
}

/// The vignetting entries nearest to focus distance `distance` (log scale; none = infinity).
fn at_distance(v: &[Vignetting], distance: Option<f64>) -> Vec<Vignetting> {
    let target = distance.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(1000.0).clamp(0.01, 1000.0);
    let best = v.iter().map(|e| e.distance).min_by(|a, b| {
        let da = (a.max(1e-3) / target).ln().abs();
        let db = (b.max(1e-3) / target).ln().abs();
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    match best {
        Some(d) => v.iter().filter(|e| (e.distance - d).abs() < 1e-9).cloned().collect(),
        None => Vec::new(),
    }
}

/// Interpolated brightness model at (focal, aperture): a function of lens radius.
fn vignette_model(entries: &[Vignetting], focal: f64, aperture: f64) -> Option<impl Fn(f64) -> f64 + use<>> {
    let mut focals: Vec<f64> = entries.iter().map(|e| e.focal).collect();
    focals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    focals.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let (f0, f1, tf) = bracket(&focals, focal)?;
    // per focal: sorted (aperture, k) list
    let list = |f: f64| -> Vec<(f64, [f64; 3])> {
        let mut v: Vec<(f64, [f64; 3])> = entries.iter().filter(|e| (e.focal - f).abs() < 1e-9).map(|e| (e.aperture.max(0.1), e.k)).collect();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        v.dedup_by(|b, a| (a.0 - b.0).abs() < 1e-9);
        v
    };
    let (l0, l1) = (list(focals[f0]), list(focals[f1]));
    let at = |l: &[(f64, [f64; 3])]| -> Option<((usize, usize, f64), Vec<f64>)> {
        let keys: Vec<f64> = l.iter().map(|e| e.0.log2()).collect();
        Some((bracket(&keys, aperture.max(0.1).log2())?, keys))
    };
    let ((a0, a1, ta), _) = at(&l0)?;
    let ((b0, b1, tb), _) = at(&l1)?;
    let (k00, k01, k10, k11) = (l0.get(a0)?.1, l0.get(a1)?.1, l1.get(b0)?.1, l1.get(b1)?.1);
    Some(move |r: f64| {
        let v0 = brightness(k00, r) * (1.0 - ta) + brightness(k01, r) * ta;
        let v1 = brightness(k10, r) * (1.0 - tb) + brightness(k11, r) * tb;
        v0 * (1.0 - tf) + v1 * tf
    })
}

/// Build the corrections of `lens` for a camera of crop factor `cam_crop` and the shot `q`. Also reports which
/// of (distortion, lateral CA, vignetting) the result contains.
pub(crate) fn build_correction(lens: &LensEntry, cam_crop: f64, q: &Query) -> (Option<LensCorrection>, (bool, bool, bool)) {
    let focal = q.focal.filter(|f| *f > 0.0).or(lens.focal.map(|f| f.0));
    let crop_ratio = lens.crop / cam_crop;
    if !crop_ratio.is_finite() || crop_ratio <= 0.0 {
        return (None, (false, false, false));
    }
    // --- distortion + lateral CA → per-plane polynomials
    let dist = if lens.kind.is_none() { by_focal(&lens.distortion, |d| d.focal) } else { Vec::new() };
    let tca = by_focal(&lens.tca, |t| t.focal);
    let dist_at = focal.and_then(|f| bracket(&dist.iter().map(|d| d.focal).collect::<Vec<_>>(), f));
    let tca_at = focal.and_then(|f| bracket(&tca.iter().map(|t| t.focal).collect::<Vec<_>>(), f));
    let green = |rl: f64| -> f64 {
        match dist_at {
            Some((a, b, t)) => match (dist.get(a), dist.get(b)) {
                (Some(x), Some(y)) => distortion_ratio(x, rl) * (1.0 - t) + distortion_ratio(y, rl) * t,
                _ => 1.0,
            },
            None => 1.0,
        }
    };
    let rb = |rl: f64| -> (f64, f64) {
        match tca_at {
            Some((a, b, t)) => match (tca.get(a), tca.get(b)) {
                (Some(x), Some(y)) => {
                    let rd = rl * green(rl);
                    let (xr, xb) = tca_ratios(x, rd);
                    let (yr, yb) = tca_ratios(y, rd);
                    (xr * (1.0 - t) + yr * t, xb * (1.0 - t) + yb * t)
                }
                _ => (1.0, 1.0),
            },
            None => (1.0, 1.0),
        }
    };
    let has_dist = dist_at.is_some();
    let has_tca = tca_at.is_some();
    let mut fit_error = 0.0f64;
    let mut planes = None;
    if has_dist || has_tca {
        let corner = green(crop_ratio).max(1e-6); // the image corner keeps its position
        let samples: Vec<[(f64, f64); 3]> = (0..=SAMPLES)
            .map(|i| {
                let r = i as f64 / SAMPLES as f64;
                let rl = r * crop_ratio;
                let g = green(rl);
                let (cr, cb) = rb(rl);
                [(r, g * cr / corner), (r, g / corner), (r, g * cb / corner)]
            })
            .collect();
        let mut out = [[0.0; 4]; 3];
        let mut ok = true;
        for (ch, o) in out.iter_mut().enumerate() {
            let pts: Vec<(f64, f64)> = samples.iter().map(|s| s[ch]).collect();
            match fit_even(&pts, 0, 4) {
                Some(c) => {
                    o.copy_from_slice(&c);
                    for &(r, y) in &pts {
                        let r2 = r * r;
                        let v = c[0] + r2 * (c[1] + r2 * (c[2] + r2 * c[3]));
                        fit_error = fit_error.max((v - y).abs());
                    }
                }
                None => ok = false,
            }
        }
        if ok {
            planes = Some(out);
        }
    }
    // --- vignetting
    let mut vignette = None;
    if let Some(ap) = q.aperture.filter(|a| *a > 0.0)
        && !lens.vignetting.is_empty()
        && let Some(f) = focal
    {
        let entries = at_distance(&lens.vignetting, q.distance);
        if let Some(model) = vignette_model(&entries, f, ap) {
            let pts: Vec<(f64, f64)> = (0..=SAMPLES)
                .map(|i| {
                    let r = i as f64 / SAMPLES as f64;
                    (r, 1.0 / model(r * crop_ratio).max(MIN_BRIGHTNESS) - 1.0)
                })
                .collect();
            if let Some(c) = fit_even(&pts, 1, 5)
                && c.len() == 5
            {
                let mut k = [0.0; 5];
                k.copy_from_slice(&c);
                for &(r, y) in &pts {
                    let r2 = r * r;
                    let v = r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))));
                    fit_error = fit_error.max(((v + 1.0) - (y + 1.0)).abs() / (y + 1.0));
                }
                vignette = Some(k);
            }
        }
    }
    let flags = (has_dist && planes.is_some(), has_tca && planes.is_some(), vignette.is_some());
    if planes.is_none() && vignette.is_none() {
        return (None, (false, false, false));
    }
    (Some(LensCorrection { planes, vignette, crop_ratio, fit_error }), flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_recovers_an_even_polynomial() {
        let pts: Vec<(f64, f64)> = (0..=40)
            .map(|i| {
                let r = i as f64 / 40.0;
                let r2 = r * r;
                (r, 1.02 - 0.05 * r2 + 0.03 * r2 * r2 - 0.01 * r2 * r2 * r2)
            })
            .collect();
        let c = fit_even(&pts, 0, 4).unwrap();
        for (a, b) in c.iter().zip([1.02, -0.05, 0.03, -0.01]) {
            assert!((a - b).abs() < 1e-8, "{c:?}");
        }
        assert!(fit_even(&pts, 0, 0).is_none());
        assert!(fit_even(&pts[..2], 0, 4).is_none());
    }

    #[test]
    fn brackets_clamp_and_interpolate() {
        let k = [24.0, 35.0, 70.0];
        assert_eq!(bracket(&k, 10.0), Some((0, 0, 0.0)));
        assert_eq!(bracket(&k, 200.0), Some((2, 2, 0.0)));
        let (a, b, t) = bracket(&k, 52.5).unwrap();
        assert_eq!((a, b), (1, 2));
        assert!((t - 0.5).abs() < 1e-12);
        assert_eq!(bracket(&[], 1.0), None);
    }

    fn lens() -> LensEntry {
        LensEntry {
            maker: "T".into(),
            names: vec!["T 50mm f/1.8".into()],
            mounts: vec!["M".into()],
            crop: 1.0,
            focal: Some((50.0, 50.0)),
            aperture: Some((1.8, 22.0)),
            distortion: vec![Distortion { focal: 50.0, model: DistModel::Ptlens, k: [0.0, -0.02, 0.0] }],
            tca: vec![Tca { focal: 50.0, model: TcaModel::Poly3, k: [1.0002, 0.9998, 0.0, 0.0, 0.0, 0.0] }],
            vignetting: vec![
                Vignetting { focal: 50.0, aperture: 1.8, distance: 1000.0, k: [-1.0, 0.8, -0.3] },
                Vignetting { focal: 50.0, aperture: 8.0, distance: 1000.0, k: [-0.1, 0.0, 0.0] },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn corrections_follow_the_models() {
        let q = Query { focal: Some(50.0), aperture: Some(1.8), ..Default::default() };
        let (c, flags) = build_correction(&lens(), 1.0, &q);
        let c = c.unwrap();
        assert_eq!(flags, (true, true, true));
        assert!(c.fit_error < 2e-3, "fit error {}", c.fit_error);
        // corner keeps its position (green plane ratio 1), the centre is magnified by the barrel model: b < 0 → d > 1
        assert!((c.ratio(1, 1.0) - 1.0).abs() < 2e-3, "{}", c.ratio(1, 1.0));
        assert!(c.ratio(1, 0.0) > 1.01);
        // red and blue differ from green by the TCA scale
        assert!(c.ratio(0, 0.5) > c.ratio(1, 0.5) && c.ratio(2, 0.5) < c.ratio(1, 0.5));
        // wide open: corner falls to 1 + (-1 + 0.8 - 0.3) = 0.5 of the centre → gain 2
        assert!((c.gain(1.0) - 2.0).abs() < 0.05, "{}", c.gain(1.0));
        assert!((c.gain(0.0) - 1.0).abs() < 1e-9);
        // stopped down to f/8: less gain; between apertures: in between
        let q8 = Query { aperture: Some(8.0), ..q.clone() };
        let g8 = build_correction(&lens(), 1.0, &q8).0.unwrap().gain(1.0);
        let q4 = Query { aperture: Some(3.8), ..q.clone() };
        let g4 = build_correction(&lens(), 1.0, &q4).0.unwrap().gain(1.0);
        assert!(g8 < g4 && g4 < c.gain(1.0), "{g8} {g4}");
    }

    #[test]
    fn a_smaller_sensor_sees_only_the_centre_of_the_calibration() {
        // APS-C camera (crop 1.5) with a lens calibrated on full frame: image radius 1 = lens radius 2/3
        let q = Query { focal: Some(50.0), aperture: Some(1.8), ..Default::default() };
        let (c, _) = build_correction(&lens(), 1.5, &q);
        let c = c.unwrap();
        assert!((c.crop_ratio - 2.0 / 3.0).abs() < 1e-12);
        // brightness at the image corner is the lens brightness at r = 2/3
        let v = 1.0 - 1.0 * (4.0 / 9.0) + 0.8 * (16.0 / 81.0) - 0.3 * (64.0 / 729.0);
        assert!((c.gain(1.0) - 1.0 / v).abs() < 0.03, "{} vs {}", c.gain(1.0), 1.0 / v);
    }

    #[test]
    fn no_data_at_the_shot_means_no_correction_not_a_made_up_one() {
        let mut l = lens();
        l.distortion.clear();
        l.tca.clear();
        l.vignetting.clear();
        let q = Query { focal: Some(50.0), aperture: Some(1.8), ..Default::default() };
        assert_eq!(build_correction(&l, 1.0, &q), (None, (false, false, false)));
        // aperture unknown: vignetting skipped, distortion still there
        let q = Query { focal: Some(50.0), aperture: None, ..Default::default() };
        let (c, flags) = build_correction(&lens(), 1.0, &q);
        assert_eq!(flags, (true, true, false));
        assert!(c.unwrap().vignette.is_none());
    }

    #[test]
    fn fisheye_lenses_keep_their_projection() {
        let mut l = lens();
        l.kind = Some("fisheye".into());
        let q = Query { focal: Some(50.0), aperture: Some(1.8), ..Default::default() };
        let (c, flags) = build_correction(&l, 1.0, &q);
        assert_eq!(flags, (false, true, true));
        assert!(c.is_some());
    }
}
