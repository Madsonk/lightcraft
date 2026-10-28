//! Weighted ridge regression on standardised features, in closed form.

use serde::{Deserialize, Serialize};

use crate::{Error, Result, linalg};

/// A fitted ridge model: standardise each feature (`(x − mean) / scale`), then a dot product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ridge {
    pub mean: Vec<f32>,
    pub scale: Vec<f32>,
    pub weights: Vec<f32>,
    /// The (weighted) mean of the targets: what a photo with average features is predicted.
    pub bias: f32,
    /// The regularisation this model was fitted with (on standardised features).
    pub lambda: f32,
}

impl Ridge {
    pub fn dim(&self) -> usize {
        self.weights.len()
    }

    /// The predicted target for a feature vector (`None` when the length is wrong).
    pub fn predict(&self, x: &[f32]) -> Option<f32> {
        if x.len() != self.weights.len() || self.mean.len() != x.len() || self.scale.len() != x.len() {
            return None;
        }
        let mut s = self.bias as f64;
        for (((v, m), sc), w) in x.iter().zip(&self.mean).zip(&self.scale).zip(&self.weights) {
            s += ((*v - *m) / *sc) as f64 * *w as f64;
        }
        s.is_finite().then_some(s as f32)
    }
}

/// Column statistics and the normal equations, computed once for any number of `lambda`s.
struct Normal {
    mean: Vec<f32>,
    scale: Vec<f32>,
    /// `Zᵀ·W·Z` (row-major `d × d`).
    gram: Vec<f64>,
    /// `Zᵀ·W·(y − ȳ)`.
    rhs: Vec<f64>,
    ybar: f64,
    /// Σ weights.
    mass: f64,
}

fn normal(x: &[&[f32]], y: &[f32], weight: &[f32]) -> Result<Normal> {
    let d = x.first().map_or(0, |r| r.len());
    if x.len() != y.len() || x.len() != weight.len() || x.iter().any(|r| r.len() != d) {
        return Err(Error::Ragged);
    }
    let mass: f64 = weight.iter().map(|w| *w as f64).sum();
    if mass.is_nan() || mass <= 0.0 || d == 0 {
        return Err(Error::TooFew { have: 0, need: 1 });
    }
    let mut mean = vec![0f64; d];
    for (r, w) in x.iter().zip(weight) {
        for (m, v) in mean.iter_mut().zip(r.iter()) {
            *m += *w as f64 * *v as f64;
        }
    }
    mean.iter_mut().for_each(|m| *m /= mass);
    let mut var = vec![0f64; d];
    for (r, w) in x.iter().zip(weight) {
        for ((s, v), m) in var.iter_mut().zip(r.iter()).zip(&mean) {
            let c = *v as f64 - m;
            *s += *w as f64 * c * c;
        }
    }
    let scale: Vec<f32> = var.iter().map(|v| ((v / mass).sqrt() as f32).max(1e-6)).collect();
    let ybar: f64 = x.iter().zip(y).zip(weight).map(|((_, t), w)| *w as f64 * *t as f64).sum::<f64>() / mass;
    let mut gram = vec![0f64; d * d];
    let mut rhs = vec![0f64; d];
    let mut z = vec![0f64; d];
    for ((r, t), w) in x.iter().zip(y).zip(weight) {
        let w = *w as f64;
        for (((zj, v), m), s) in z.iter_mut().zip(r.iter()).zip(&mean).zip(&scale) {
            *zj = (*v as f64 - m) / *s as f64;
        }
        let dy = *t as f64 - ybar;
        for i in 0..d {
            let zi = w * z[i];
            rhs[i] += zi * dy;
            // lower triangle only
            let row = &mut gram[i * d..i * d + i + 1];
            for (g, zj) in row.iter_mut().zip(&z) {
                *g += zi * zj;
            }
        }
    }
    for i in 0..d {
        for j in i + 1..d {
            gram[i * d + j] = gram[j * d + i];
        }
    }
    Ok(Normal { mean: mean.iter().map(|m| *m as f32).collect(), scale, gram, rhs, ybar, mass })
}

/// Fit one model per `lambda` (the regularisation is multiplied by the weights' total mass, so
/// the same values mean the same thing whatever the number of photos).
pub fn fit_path(x: &[&[f32]], y: &[f32], weight: &[f32], lambdas: &[f32]) -> Result<Vec<Ridge>> {
    let n = normal(x, y, weight)?;
    let d = n.mean.len();
    let mut out = Vec::with_capacity(lambdas.len());
    for &l in lambdas {
        let mut a = n.gram.clone();
        let reg = l as f64 * n.mass;
        for i in 0..d {
            a[i * d + i] += reg;
        }
        let w = linalg::cholesky_solve(&mut a, d, &n.rhs).ok_or(Error::Singular)?;
        out.push(Ridge {
            mean: n.mean.clone(),
            scale: n.scale.clone(),
            weights: w.iter().map(|v| *v as f32).collect(),
            bias: n.ybar as f32,
            lambda: l,
        });
    }
    Ok(out)
}

/// Fit a single model.
pub fn fit(x: &[&[f32]], y: &[f32], weight: &[f32], lambda: f32) -> Result<Ridge> {
    fit_path(x, y, weight, &[lambda])?.into_iter().next().ok_or(Error::Singular)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic generator (the tests must not depend on a random crate).
    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
    }

    #[test]
    fn recovers_a_linear_relation() {
        let mut s = 1u64;
        let rows: Vec<Vec<f32>> = (0..200).map(|_| (0..4).map(|_| lcg(&mut s)).collect()).collect();
        let y: Vec<f32> = rows.iter().map(|r| 3.0 + 2.0 * r[0] - 1.0 * r[2]).collect();
        let x: Vec<&[f32]> = rows.iter().map(|r| r.as_slice()).collect();
        let m = fit(&x, &y, &vec![1.0; 200], 1e-6).unwrap();
        let p = m.predict(&[0.2, 0.0, -0.1, 0.4]).unwrap();
        assert!((p - (3.0 + 0.4 + 0.1)).abs() < 0.01, "{p}");
        assert!(m.predict(&[1.0]).is_none());
    }

    #[test]
    fn regularisation_shrinks_and_weights_count() {
        let mut s = 3u64;
        let rows: Vec<Vec<f32>> = (0..100).map(|_| (0..3).map(|_| lcg(&mut s)).collect()).collect();
        let y: Vec<f32> = rows.iter().map(|r| 4.0 * r[1]).collect();
        let x: Vec<&[f32]> = rows.iter().map(|r| r.as_slice()).collect();
        let ms = fit_path(&x, &y, &vec![1.0; 100], &[1e-4, 10.0]).unwrap();
        let n = |m: &Ridge| m.weights.iter().map(|w| w * w).sum::<f32>();
        assert!(n(&ms[1]) < n(&ms[0]) * 0.5);
        // a zero weight ignores the row
        let mut y2 = y.clone();
        y2[0] = 1000.0;
        let mut w = vec![1.0; 100];
        w[0] = 0.0;
        let a = fit(&x, &y2, &w, 1e-6).unwrap();
        let b = fit(&x[1..], &y[1..], &vec![1.0; 99], 1e-6).unwrap();
        assert!((a.weights[1] - b.weights[1]).abs() < 1e-3);
    }

    #[test]
    fn refuses_ragged_and_empty_input() {
        let a = [1.0f32, 2.0];
        let b = [1.0f32];
        assert_eq!(fit(&[&a, &b], &[1.0, 2.0], &[1.0, 1.0], 1.0).unwrap_err(), Error::Ragged);
        assert!(fit(&[], &[], &[], 1.0).is_err());
        // all-zero weights
        assert!(fit(&[&a], &[1.0], &[0.0], 1.0).is_err());
    }
}
