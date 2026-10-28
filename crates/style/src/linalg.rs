//! Just enough linear algebra for ridge regression: a Cholesky solve of a symmetric positive
//! definite system (row-major, `f64`).

/// Solve `a · x = b` for a symmetric positive definite `n × n` matrix `a` (row-major). `a` is
/// overwritten. `None` when the matrix is not positive definite (or the sizes don't match).
pub fn cholesky_solve(a: &mut [f64], n: usize, b: &[f64]) -> Option<Vec<f64>> {
    if a.len() != n.checked_mul(n)? || b.len() != n {
        return None;
    }
    // a = L·Lᵀ, L stored in the lower triangle
    for j in 0..n {
        let mut d = *a.get(j * n + j)?;
        for k in 0..j {
            let l = *a.get(j * n + k)?;
            d -= l * l;
        }
        if !(d.is_finite() && d > 1e-12) {
            return None;
        }
        let d = d.sqrt();
        *a.get_mut(j * n + j)? = d;
        for i in j + 1..n {
            let mut s = *a.get(i * n + j)?;
            for k in 0..j {
                s -= *a.get(i * n + k)? * *a.get(j * n + k)?;
            }
            *a.get_mut(i * n + j)? = s / d;
        }
    }
    // forward: L·y = b
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = *b.get(i)?;
        for k in 0..i {
            s -= *a.get(i * n + k)? * *y.get(k)?;
        }
        *y.get_mut(i)? = s / *a.get(i * n + i)?;
    }
    // backward: Lᵀ·x = y
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = *y.get(i)?;
        for k in i + 1..n {
            s -= *a.get(k * n + i)? * *x.get(k)?;
        }
        *x.get_mut(i)? = s / *a.get(i * n + i)?;
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_small_system() {
        // [[4,1],[1,3]]·x = [1,2]  →  x = [1/11, 7/11]
        let mut a = vec![4.0, 1.0, 1.0, 3.0];
        let x = cholesky_solve(&mut a, 2, &[1.0, 2.0]).unwrap();
        assert!((x[0] - 1.0 / 11.0).abs() < 1e-12 && (x[1] - 7.0 / 11.0).abs() < 1e-12);
    }

    #[test]
    fn refuses_what_is_not_positive_definite_or_ill_sized() {
        assert!(cholesky_solve(&mut [1.0, 2.0, 2.0, 1.0], 2, &[1.0, 1.0]).is_none());
        assert!(cholesky_solve(&mut [1.0, 0.0, 0.0], 2, &[1.0, 1.0]).is_none());
        assert!(cholesky_solve(&mut [1.0, 0.0, 0.0, 1.0], 2, &[1.0]).is_none());
        assert!(cholesky_solve(&mut [f64::NAN, 0.0, 0.0, 1.0], 2, &[1.0, 1.0]).is_none());
    }
}
