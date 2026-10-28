//! Keep one shoot coherent: photos of the same series get the same *style*, adapted to each
//! photo's own exposure (the series v16 idea: same values, adapted to the light of each frame).

use crate::targets::{EXPOSURE, N_TARGETS, TARGETS, Targets};

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    match (v.get(n / 2), v.get(n.saturating_sub(1) / 2)) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        _ => 0.0,
    }
}

/// Pull the per-photo predictions of one series towards a shared style. `strength` 0 keeps every
/// prediction as it is, 1 gives every photo the series' median for each slider, except exposure:
/// the series' median *offset from the photo's own auto exposure* is shared, so a darker frame
/// still gets brightened more than a brighter one. `auto_exposure[i]` belongs to `photos[i]`.
pub fn harmonize(photos: &mut [Targets], auto_exposure: &[f32], strength: f32) {
    let s = strength.clamp(0.0, 1.0);
    if photos.len() < 2 || s == 0.0 || auto_exposure.len() != photos.len() {
        return;
    }
    for t in 0..N_TARGETS {
        if t == EXPOSURE {
            let mut offsets: Vec<f32> = photos.iter().zip(auto_exposure).map(|(p, a)| p.get(t).copied().unwrap_or(0.0) - a).collect();
            let shared = median(&mut offsets);
            for (p, a) in photos.iter_mut().zip(auto_exposure) {
                if let Some(v) = p.get_mut(t) {
                    *v += s * ((a + shared) - *v);
                }
            }
        } else {
            let mut col: Vec<f32> = photos.iter().map(|p| p.get(t).copied().unwrap_or(0.0)).collect();
            let shared = median(&mut col);
            for p in photos.iter_mut() {
                if let Some(v) = p.get_mut(t) {
                    *v += s * (shared - *v);
                }
            }
        }
    }
    for p in photos.iter_mut() {
        crate::targets::clamp(p);
    }
    let _ = TARGETS.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_strength_shares_values_but_keeps_exposure_adapted() {
        let mut a = [0.0; N_TARGETS];
        let mut b = [0.0; N_TARGETS];
        a[EXPOSURE] = 1.0; // auto said +0.8
        b[EXPOSURE] = 0.2; // auto said 0.0
        a[3] = 10.0;
        b[3] = 30.0;
        let mut v = [a, b];
        harmonize(&mut v, &[0.8, 0.0], 1.0);
        assert!((v[0][3] - v[1][3]).abs() < 1e-4, "contrast shared");
        // the darker frame (auto 0.8) is still brighter by the difference of the autos
        assert!(((v[0][EXPOSURE] - v[1][EXPOSURE]) - 0.8).abs() < 1e-4, "{:?}", (v[0][EXPOSURE], v[1][EXPOSURE]));
    }

    #[test]
    fn zero_strength_and_bad_lengths_change_nothing() {
        let mut v = [[1.0; N_TARGETS], [2.0; N_TARGETS]];
        let before = v;
        harmonize(&mut v, &[0.0, 0.0], 0.0);
        harmonize(&mut v, &[0.0], 1.0);
        assert_eq!(v, before);
    }
}
