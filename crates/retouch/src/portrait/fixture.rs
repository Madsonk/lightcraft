//! A synthetic portrait (no photo of a real person): a skin-coloured oval with pores, eyes, brows,
//! lips and an open mouth with teeth, with planted defects (a blemish, freckles, dark circles, shine,
//! forehead lines, a stray hair), and the matching face mesh. Used by the tests, the benchmarks
//! and the visual checks of the portrait steps.

use crate::facegeo::{self};
use crate::lab::{self, rgb8_to_lab};

pub const W: usize = 400;

fn base_mesh() -> Vec<[f32; 3]> {
    let mut seed = 7u32;
    let mut r = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 8) & 0xffff) as f32 / 65535.0
    };
    // the unnamed points scatter over the face at z = 0
    let mut p: Vec<[f32; 3]> = (0..478).map(|_| [100.0 + r() * 200.0, 60.0 + r() * 280.0, 0.0]).collect();
    let ell = |cx: f32, cy: f32, rx: f32, ry: f32, n: usize, t0: f32, i: usize| {
        let t = t0 + i as f32 / n as f32 * std::f32::consts::TAU;
        [cx + rx * t.cos(), cy + ry * t.sin()]
    };
    for (i, &k) in facegeo::OVAL.iter().enumerate() {
        let q = ell(200.0, 200.0, 110.0, 150.0, 36, -std::f32::consts::FRAC_PI_2, i);
        p[k] = [q[0], q[1], 20.0];
    }
    for (i, &k) in facegeo::LEFT_EYE.iter().enumerate() {
        let q = ell(250.0, 160.0, 25.0, 9.0, 16, 0.0, i);
        p[k] = [q[0], q[1], 0.0];
    }
    for (i, &k) in facegeo::RIGHT_EYE.iter().enumerate() {
        let q = ell(150.0, 160.0, 25.0, 9.0, 16, 0.0, i);
        p[k] = [q[0], q[1], 0.0];
    }
    for (i, &k) in facegeo::LEFT_BROW.iter().enumerate() {
        p[k] = [225.0 + i as f32 * 5.0, 130.0, 0.0];
    }
    for (i, &k) in facegeo::RIGHT_BROW.iter().enumerate() {
        p[k] = [125.0 + i as f32 * 5.0, 130.0, 0.0];
    }
    for (i, &k) in facegeo::LIPS_OUTER.iter().enumerate() {
        let q = ell(200.0, 260.0, 45.0, 14.0, 20, 0.0, i);
        p[k] = [q[0], q[1], 0.0];
    }
    for (i, &k) in facegeo::LIPS_INNER.iter().enumerate() {
        let q = ell(200.0, 260.0, 38.0, 3.0, 20, 0.0, i);
        p[k] = [q[0], q[1], 0.0];
    }
    // iris: centre + four rim points, radius 9
    for (c, idx) in [([250.0f32, 160.0f32], facegeo::LEFT_IRIS), ([150.0, 160.0], facegeo::RIGHT_IRIS)] {
        p[idx[0]] = [c[0], c[1], 0.0];
        for (k, &i) in idx[1..].iter().enumerate() {
            let t = k as f32 * std::f32::consts::FRAC_PI_2;
            p[i] = [c[0] + 9.0 * t.cos(), c[1] + 9.0 * t.sin(), 0.0];
        }
    }
    // the nose bulges: z falls to -120 on the ridge
    for (k, &i) in facegeo::NOSE_RIDGE.iter().enumerate() {
        p[i] = [200.0, 150.0 + k as f32 * 18.0, -120.0];
    }
    p[facegeo::NOSE_TIP] = [200.0, 240.0, -140.0];
    p[10] = [200.0, 50.0, 20.0];
    p[152] = [200.0, 350.0, 20.0];
    p
}

/// Deterministic noise in -1..1.
fn noise(x: usize, y: usize, seed: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663) ^ seed.wrapping_mul(83_492_791);
    v ^= v >> 13;
    v = v.wrapping_mul(0x5bd1_e995);
    v ^= v >> 15;
    ((v & 0xffff) as f32 / 32767.5) - 1.0
}

/// The mesh with an open mouth and the nose wings the smile-line region needs.
pub fn mesh(open_mouth: bool) -> Vec<[f32; 3]> {
    let mut p = base_mesh();
    if open_mouth {
        for (i, &k) in facegeo::LIPS_INNER.iter().enumerate() {
            let t = i as f32 / 20.0 * std::f32::consts::TAU;
            p[k] = [200.0 + 38.0 * t.cos(), 260.0 + 11.0 * t.sin(), 0.0];
        }
    } else {
        for (i, &k) in facegeo::LIPS_INNER.iter().enumerate() {
            let t = i as f32 / 20.0 * std::f32::consts::TAU;
            p[k] = [200.0 + 38.0 * t.cos(), 260.0 + 1.0 * t.sin(), 0.0];
        }
    }
    p[129] = [172.0, 232.0, 0.0];
    p[358] = [228.0, 232.0, 0.0];
    p
}

pub struct Planted {
    pub blemish: [usize; 2],
    pub freckles: Vec<[usize; 2]>,
}

/// The portrait: returns (rgb, planted defect positions).
pub fn portrait(open_mouth: bool) -> (Vec<u8>, Planted) {
    let mut rgb = vec![0u8; W * W * 3];
    let skin = [196.0f32, 150.0, 128.0];
    let mut lab_px = vec![[0f32; 3]; W * W];
    let inside = |x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) < 1.0;
    let blemish = [225usize, 215];
    let freckles: Vec<[usize; 2]> = (0..14).map(|i| [150 + (i % 7) * 16, 200 + (i / 7) * 14]).collect();
    for y in 0..W {
        for x in 0..W {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let n = noise(x, y, 1);
            let mut c = if inside(fx, fy, 200.0, 200.0, 110.0, 150.0) {
                // skin with pores
                let k = 1.0 + 0.04 * n;
                [skin[0] * k, skin[1] * k, skin[2] * k]
            } else {
                // wall
                let k = 1.0 + 0.01 * n;
                [120.0 * k, 130.0 * k, 135.0 * k]
            };
            // eyes: white then iris
            for ex in [150.0, 250.0] {
                if inside(fx, fy, ex, 160.0, 25.0, 9.0) {
                    c = [236.0, 232.0, 230.0];
                    if (fx - ex).hypot(fy - 160.0) < 9.0 {
                        c = [70.0, 50.0, 40.0];
                    }
                }
            }
            // brows
            if (130.0..275.0).contains(&fx) && (127.0..134.0).contains(&fy) && inside(fx, fy, 200.0, 130.0, 80.0, 6.0) && (fx - 200.0).abs() > 20.0 {
                c = [60.0, 40.0, 30.0];
            }
            // lips and mouth
            if inside(fx, fy, 200.0, 260.0, 45.0, 14.0) {
                c = [170.0, 70.0, 80.0];
            }
            if open_mouth && inside(fx, fy, 200.0, 260.0, 38.0, 11.0) {
                c = if fy > 268.0 { [120.0, 50.0, 55.0] } else { [232.0, 220.0, 170.0] };
            }
            let i = y * W + x;
            lab_px[i] = rgb8_to_lab([c[0].clamp(0.0, 255.0) as u8, c[1].clamp(0.0, 255.0) as u8, c[2].clamp(0.0, 255.0) as u8]);
        }
    }
    let darken = |lab_px: &mut Vec<[f32; 3]>, cx: usize, cy: usize, r: f32, dl: f32, da: f32| {
        for y in cy.saturating_sub(8)..cy + 8 {
            for x in cx.saturating_sub(8)..cx + 8 {
                let d = ((x as f32 - cx as f32).hypot(y as f32 - cy as f32) / r).min(1.0);
                let k = 1.0 - d * d;
                lab_px[y * W + x][0] += dl * k;
                lab_px[y * W + x][1] += da * k;
            }
        }
    };
    darken(&mut lab_px, blemish[0], blemish[1], 5.0, -16.0, 14.0);
    for f in &freckles {
        darken(&mut lab_px, f[0], f[1], 2.5, -9.0, 2.0);
    }
    // dark circles: bluish shade under both eyes
    for ex in [150.0f32, 250.0] {
        for y in 170..192 {
            for x in (ex as usize - 28)..(ex as usize + 28) {
                let k = (1.0 - ((x as f32 - ex) / 28.0).powi(2)).max(0.0) * (1.0 - ((y as f32 - 180.0) / 12.0).powi(2)).max(0.0);
                lab_px[y * W + x][0] -= 14.0 * k;
                lab_px[y * W + x][2] -= 6.0 * k;
            }
        }
    }
    // shine: a bright, desaturated patch on the forehead
    for y in 85..115 {
        for x in 170..230 {
            let k = (1.0 - ((x as f32 - 200.0) / 30.0).powi(2)).max(0.0) * (1.0 - ((y as f32 - 100.0) / 15.0).powi(2)).max(0.0);
            lab_px[y * W + x][0] += 18.0 * k;
            lab_px[y * W + x][1] -= 8.0 * k;
            lab_px[y * W + x][2] -= 8.0 * k;
        }
    }
    // forehead wrinkles: two thin dark lines
    for &ly in &[96usize, 110] {
        for x in 150..250 {
            lab_px[ly * W + x][0] -= 7.0;
            lab_px[(ly + 1) * W + x][0] -= 3.0;
        }
    }
    // a stray hair: a thin dark curve over the wall above the head
    for t in 0..120 {
        let (x, y) = (150 + t, 40 - (t as f32 * 0.08) as usize);
        lab_px[y * W + x][0] -= 40.0;
    }
    for (i, l) in lab_px.iter().enumerate() {
        let px = lab::lab_to_rgb8(*l);
        rgb[i * 3..i * 3 + 3].copy_from_slice(&px);
    }
    (rgb, Planted { blemish, freckles })
}

/// The same face with both eyes closed: the eye openings painted over with skin and a lash line.
pub fn portrait_closed() -> Vec<u8> {
    let (mut rgb, _) = portrait(true);
    let skin = lab::lab_to_rgb8(rgb8_to_lab([196, 150, 128]));
    for ex in [150.0f32, 250.0] {
        for y in 140..180usize {
            for x in (ex as usize - 30)..(ex as usize + 30) {
                let (dx, dy) = ((x as f32 + 0.5 - ex) / 27.0, (y as f32 + 0.5 - 160.0) / 11.0);
                if dx * dx + dy * dy < 1.0 {
                    let i = (y * W + x) * 3;
                    // skin, with a thin dark lash line along the middle
                    let lash = (y as f32 + 0.5 - 161.0).abs() < 1.3 && dx.abs() < 0.9;
                    let c = if lash { [60u8, 40, 35] } else { skin };
                    rgb[i..i + 3].copy_from_slice(&c);
                }
            }
        }
    }
    rgb
}

/// The mesh of [`portrait_closed`]: the eyelid contours collapse onto the lash line.
pub fn mesh_closed() -> Vec<[f32; 3]> {
    let mut p = mesh(true);
    for (idx, cy) in [(&facegeo::LEFT_EYE[..], 160.0f32), (&facegeo::RIGHT_EYE[..], 160.0)] {
        for &k in idx {
            p[k][1] = cy + (p[k][1] - cy) * 0.12;
        }
    }
    p
}

/// A second frame of the same scene: shifted by (`dx`, `dy`) pixels and scaled in brightness by
/// `gain` (edges clamped), with the mesh moved along.
pub fn shifted(rgb: &[u8], mesh: &[[f32; 3]], dx: i32, dy: i32, gain: f32) -> (Vec<u8>, Vec<[f32; 3]>) {
    let mut out = vec![0u8; W * W * 3];
    for y in 0..W {
        for x in 0..W {
            let sx = (x as i32 - dx).clamp(0, W as i32 - 1) as usize;
            let sy = (y as i32 - dy).clamp(0, W as i32 - 1) as usize;
            for c in 0..3 {
                out[(y * W + x) * 3 + c] = (f32::from(rgb[(sy * W + sx) * 3 + c]) * gain).clamp(0.0, 255.0) as u8;
            }
        }
    }
    (out, mesh.iter().map(|p| [p[0] + dx as f32, p[1] + dy as f32, p[2]]).collect())
}
