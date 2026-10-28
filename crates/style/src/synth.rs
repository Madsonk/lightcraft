//! Photos and photographers made up on purpose, so the learning can be checked against a known
//! truth (and the tests need no real picture). The "photographer" edits as a fixed function of
//! what the photo looks like, plus noise, and one slider is pure noise: the model must learn the
//! first and leave the second alone.

use lightcraft_pipeline::SourceInfo;
use lightcraft_raster::Rgb32f;

use crate::features::{self, FEATURE_NAMES};
use crate::targets::{N_TARGETS, Targets};
use crate::train::Sample;

/// A tiny deterministic generator (no dependency, reproducible).
pub struct Rng(pub u64);

impl Rng {
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32) / (1u64 << 31) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
    /// Roughly normal (sum of uniforms).
    pub fn normal(&mut self) -> f32 {
        (0..4).map(|_| self.next_f32()).sum::<f32>() - 2.0
    }
}

/// A made-up scene: a sky/ground split, a few blobs, a colour cast, an exposure and a contrast.
pub fn image(seed: u64) -> Rgb32f {
    let mut r = Rng(seed.wrapping_mul(0x9e3779b97f4a7c15) ^ 0xdead_beef);
    let gain = 2f32.powf(r.range(-2.2, 1.2));
    let cast = [r.range(0.8, 1.25), 1.0, r.range(0.8, 1.25)];
    let gamma = r.range(0.7, 1.4);
    let horizon = r.range(0.3, 0.7);
    let sky = [r.range(0.2, 0.5), r.range(0.3, 0.6), r.range(0.5, 0.9)];
    let ground = [r.range(0.1, 0.4), r.range(0.1, 0.4), r.range(0.05, 0.3)];
    let blobs: Vec<(f32, f32, f32, [f32; 3])> =
        (0..3).map(|_| (r.range(0.1, 0.9), r.range(0.1, 0.9), r.range(0.05, 0.2), [r.range(0.0, 1.0), r.range(0.0, 1.0), r.range(0.0, 1.0)])).collect();
    Rgb32f::from_fn(96, 64, |x, y| {
        let (u, v) = (x as f32 / 96.0, y as f32 / 64.0);
        let mut c = if v < horizon { sky } else { ground };
        for (bx, by, br, bc) in &blobs {
            if (u - bx).powi(2) + (v - by).powi(2) < br * br {
                c = *bc;
            }
        }
        let mut out = [0f32; 3];
        for ((o, v), k) in out.iter_mut().zip(c).zip(cast) {
            *o = (v.powf(gamma) * gain * k).clamp(1e-4, 4.0);
        }
        out
    })
}

fn f(feat: &[f32], name: &str) -> f32 {
    FEATURE_NAMES.iter().position(|n| *n == name).and_then(|i| feat.get(i)).copied().unwrap_or(0.0)
}

/// The sliders the made-up photographer would set for a photo with these features.
pub fn photographer(feat: &[f32], noise: &mut Rng) -> Targets {
    let mut t = [0f32; N_TARGETS];
    t[0] = 0.7 * f(feat, "auto_exposure") + 0.15 + 0.08 * noise.normal();
    t[1] = 0.6 * (f(feat, "auto_wb_mired") - f(feat, "asshot_mired")) + 6.0 + 2.0 * noise.normal();
    t[2] = 0.5 * (f(feat, "auto_wb_tint") - f(feat, "asshot_tint")) + 1.5 * noise.normal();
    t[3] = 14.0 + 0.8 * f(feat, "auto_contrast") + 3.0 * noise.normal();
    t[4] = 0.7 * f(feat, "auto_highlights") - 8.0 + 3.0 * noise.normal();
    t[5] = 0.6 * f(feat, "auto_shadows") + 12.0 + 3.0 * noise.normal();
    t[6] = 0.5 * f(feat, "auto_whites") + 2.0 * noise.normal();
    t[7] = 0.5 * f(feat, "auto_blacks") - 5.0 + 2.0 * noise.normal();
    t[8] = 22.0 - 40.0 * f(feat, "saturation_mean") + 3.0 * noise.normal();
    t[9] = -4.0 + 2.0 * noise.normal();
    t[10] = 0.0;
    t[11] = 6.0 + 4.0 * f(feat, "lum_mean").clamp(-3.0, 3.0) * 0.3 + 2.0 * noise.normal();
    t[12] = 0.0;
    t[13] = 9.0 * noise.normal(); // pure noise: not learnable
    t
}

/// `n` made-up edited photos in `shoots` shoots.
pub fn samples(n: usize, shoots: usize, seed: u64) -> Vec<Sample> {
    let mut noise = Rng(seed ^ 0x51ed);
    (0..n)
        .map(|i| {
            let img = image(seed.wrapping_add(i as u64 * 7919));
            let fe = features::extract(&img, &SourceInfo::default());
            let targets = photographer(&fe.values, &mut noise);
            Sample { features: fe.values, targets, group: (i % shoots.max(1)) as u64 }
        })
        .collect()
}
