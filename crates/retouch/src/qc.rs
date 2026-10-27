//! Quality checks on a retouch: texture, halos, what changed outside the treated zones. Each
//! returns a number in the unit its threshold is written in, so a report can say it with figures.

use crate::plane::Plane;

/// Highest tolerated change of the skin's high-frequency variance (share, 0.05 = 5 %).
pub const TEXTURE_TOLERANCE: f32 = 0.05;
/// Highest tolerated halo (L units) around a subject.
pub const HALO_TOLERANCE: f32 = 1.0;
/// Highest tolerated change of the skin's median lightness (L units) from the colour step.
pub const SKIN_L_TOLERANCE: f32 = 1.2;

/// Relative change of the high-frequency variance: `after / before - 1` (0 when nothing to compare).
pub fn texture_change(before_var: f32, after_var: f32) -> f32 {
    if before_var <= 1e-9 { 0.0 } else { after_var / before_var - 1.0 }
}

/// Halo of a burn or dodge around a subject: the distance (L units) of the lightness change just
/// outside the subject's edge from the interval between the change deep inside and the change far
/// outside. A smooth transition lies inside the interval (0); a dark or bright rim sticks out.
pub fn halo(before_l: &Plane, after_l: &Plane, subject: &Plane) -> f32 {
    let (w, h) = (before_l.w, before_l.h);
    let edge = w.max(h) as f32;
    let bin = subject.map(|v| if v > 0.5 { 1.0 } else { 0.0 });
    let near = bin.gauss((0.012 * edge).max(1.0));
    let far = bin.gauss((0.06 * edge).max(3.0));
    let (mut sr, mut nr, mut sf, mut nf, mut si, mut ni) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for i in 0..w * h {
        let d = f64::from(after_l.d[i] - before_l.d[i]);
        if bin.d[i] < 0.5 && near.d[i] > 0.04 && near.d[i] < 0.45 {
            sr += d;
            nr += 1.0;
        } else if bin.d[i] < 0.5 && far.d[i] < 0.005 {
            sf += d;
            nf += 1.0;
        } else if bin.d[i] > 0.5 && near.d[i] > 0.97 {
            si += d;
            ni += 1.0;
        }
    }
    if nr < 20.0 || nf < 20.0 || ni < 20.0 {
        return 0.0;
    }
    let (ring, far_m, in_m) = ((sr / nr) as f32, (sf / nf) as f32, (si / ni) as f32);
    let (lo, hi) = (far_m.min(in_m), far_m.max(in_m));
    if ring < lo {
        lo - ring
    } else if ring > hi {
        ring - hi
    } else {
        0.0
    }
}

/// Largest per-channel difference (0..255) between two renders at the pixels where no mask
/// acts (`union` alpha is 0), and how many of those pixels differ at all.
pub fn outside_change(before: &[[u8; 4]], after: &[[u8; 4]], union: &Plane) -> (u8, usize) {
    let (mut mx, mut n) = (0u8, 0usize);
    for ((b, a), u) in before.iter().zip(after).zip(&union.d) {
        if *u <= 0.0 {
            let d = (0..3).map(|c| b[c].abs_diff(a[c])).max().unwrap_or(0);
            if d > 0 {
                n += 1;
            }
            mx = mx.max(d);
        }
    }
    (mx, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_change_is_relative() {
        assert!((texture_change(10.0, 10.3) - 0.03).abs() < 1e-5);
        assert_eq!(texture_change(0.0, 5.0), 0.0);
    }

    fn scene() -> (Plane, Plane) {
        // a subject disc in the middle of a 200 x 200 frame
        let mut s = Plane::new(200, 200, 0.0);
        for y in 0..200 {
            for x in 0..200 {
                let (dx, dy) = (x as f32 - 100.0, y as f32 - 100.0);
                if dx * dx + dy * dy < 40.0 * 40.0 {
                    s.d[y * 200 + x] = 1.0;
                }
            }
        }
        (Plane::new(200, 200, 50.0), s)
    }

    #[test]
    fn a_soft_burn_has_no_halo_and_a_rim_does() {
        let (before, subject) = scene();
        // soft: the background burn follows the subject's edge smoothly (-5 far, 0 inside)
        let soft = subject.gauss(8.0);
        let after_soft = before.zip(&soft, |b, s| b - 5.0 * (1.0 - s));
        assert!(halo(&before, &after_soft, &subject) < 0.3, "{}", halo(&before, &after_soft, &subject));
        // a dark rim just outside the subject
        let ring = subject.gauss(2.0).zip(&subject, |g, s| if s < 0.5 { g } else { 0.0 });
        let after_rim = after_soft.zip(&ring, |a, r| a - 40.0 * r);
        assert!(halo(&before, &after_rim, &subject) > 1.0, "{}", halo(&before, &after_rim, &subject));
        // nothing changed: 0
        assert_eq!(halo(&before, &before, &subject), 0.0);
    }

    #[test]
    fn outside_change_counts_only_where_no_mask_acts() {
        let before = vec![[10u8, 20, 30, 255]; 4];
        let mut after = before.clone();
        after[0] = [12, 20, 30, 255]; // inside the mask
        after[3] = [10, 20, 33, 255]; // outside: changed by 3
        let union = Plane::from_vec(4, 1, vec![0.5, 0.0, 0.0, 0.0]);
        assert_eq!(outside_change(&before, &after, &union), (3, 1));
        assert_eq!(outside_change(&before, &before, &union), (0, 0));
    }
}
