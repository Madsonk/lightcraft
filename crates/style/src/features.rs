//! What the model sees of a photo: statistics of its unedited pixels on a small proxy.
//!
//! They must not depend on the photographer's edits (the edit is the target), so they are read
//! from the decoded source, before any develop setting. Reuses the pipeline's own estimators
//! (auto tone, auto white balance) so the model learns the photographer's *offset from auto*
//! rather than rediscovering exposure from scratch.

use lightcraft_color::luminance_2020;
use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::SourceInfo;
use lightcraft_pipeline::auto::{auto_tone, auto_wb, current_wb};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, fit};

pub const FEATURE_NAMES: [&str; 33] = [
    "raw",
    "auto_exposure",
    "auto_contrast",
    "auto_highlights",
    "auto_shadows",
    "auto_whites",
    "auto_blacks",
    "auto_wb_mired",
    "auto_wb_tint",
    "asshot_mired",
    "asshot_tint",
    "lum_p01",
    "lum_p05",
    "lum_p25",
    "lum_p50",
    "lum_p75",
    "lum_p95",
    "lum_p99",
    "lum_mean",
    "lum_std",
    "bright_share",
    "dark_share",
    "saturation_mean",
    "rg_log",
    "bg_log",
    "grid_tl",
    "grid_tc",
    "grid_tr",
    "grid_ml",
    "grid_mc",
    "grid_mr",
    "grid_bl",
    "grid_bc",
];

/// Number of values per photo.
pub const N_FEATURES: usize = FEATURE_NAMES.len();

/// A photo's feature vector plus the one measurement the series logic needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Features {
    pub values: Vec<f32>,
    /// What auto tone would set as exposure (EV): the photo's own brightness need.
    pub auto_exposure: f32,
}

fn percentile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f32 * q.clamp(0.0, 1.0)) as usize;
    sorted.get(i).copied().unwrap_or(0.0)
}

fn mired(kelvin: f64) -> f32 {
    (1.0e6 / kelvin.clamp(1500.0, 60000.0)) as f32
}

/// Measure `src` (the decoded, unedited source).
pub fn extract(src: &Rgb32f, info: &SourceInfo) -> Features {
    let base = DevelopSettings::default();
    let tone = auto_tone(src, info, &base);
    let (wb_t, wb_tint) = auto_wb(src, info);
    let (as_t, as_tint) = current_wb(info, &base);
    let img = fit(src, 96, 96, Filter::Box);
    let (w, h) = (img.width.max(1), img.height.max(1));
    let mut log_l: Vec<f32> = Vec::with_capacity(img.data.len());
    let (mut sat_sum, mut sat_n) = (0f64, 0usize);
    let (mut rg, mut bg, mut chroma_n) = (0f64, 0f64, 0usize);
    let (mut bright, mut dark) = (0usize, 0usize);
    let mut cells = [(0f64, 0usize); 9];
    for (i, c) in img.data.iter().enumerate() {
        let y = luminance_2020(*c);
        if !y.is_finite() {
            continue;
        }
        let l = (y.max(1e-5) / 0.18).log2().clamp(-10.0, 4.0);
        log_l.push(l);
        if y > 0.9 {
            bright += 1;
        }
        if y < 0.01 {
            dark += 1;
        }
        let mx = c[0].max(c[1]).max(c[2]);
        let mn = c[0].min(c[1]).min(c[2]);
        if mx > 0.02 && mx.is_finite() {
            sat_sum += ((mx - mn) / mx) as f64;
            sat_n += 1;
            if c[1] > 1e-4 && c[0] > 1e-4 && c[2] > 1e-4 {
                rg += (c[0] / c[1]).ln() as f64;
                bg += (c[2] / c[1]).ln() as f64;
                chroma_n += 1;
            }
        }
        let (x, yy) = (i % w, i / w);
        let cell = (yy * 3 / h).min(2) * 3 + (x * 3 / w).min(2);
        if let Some(slot) = cells.get_mut(cell) {
            slot.0 += l as f64;
            slot.1 += 1;
        }
    }
    let n = log_l.len().max(1) as f64;
    let mean = log_l.iter().map(|v| *v as f64).sum::<f64>() / n;
    let var = log_l.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / n;
    log_l.sort_by(f32::total_cmp);
    let grid = |i: usize| cells.get(i).map_or(0.0, |c| if c.1 > 0 { (c.0 / c.1 as f64) as f32 } else { 0.0 });
    let values = vec![
        if info.raw { 1.0 } else { 0.0 },
        tone.exposure as f32,
        tone.contrast as f32,
        tone.highlights as f32,
        tone.shadows as f32,
        tone.whites as f32,
        tone.blacks as f32,
        mired(wb_t),
        wb_tint as f32,
        mired(as_t),
        as_tint as f32,
        percentile(&log_l, 0.01),
        percentile(&log_l, 0.05),
        percentile(&log_l, 0.25),
        percentile(&log_l, 0.5),
        percentile(&log_l, 0.75),
        percentile(&log_l, 0.95),
        percentile(&log_l, 0.99),
        mean as f32,
        var.sqrt() as f32,
        bright as f32 / n as f32,
        dark as f32 / n as f32,
        if sat_n > 0 { (sat_sum / sat_n as f64) as f32 } else { 0.0 },
        if chroma_n > 0 { (rg / chroma_n as f64) as f32 } else { 0.0 },
        if chroma_n > 0 { (bg / chroma_n as f64) as f32 } else { 0.0 },
        grid(0),
        grid(1),
        grid(2),
        grid(3),
        grid(4),
        grid(5),
        grid(6),
        grid(7),
    ];
    Features { values, auto_exposure: tone.exposure as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_values_agree_and_are_finite() {
        let img = Rgb32f::from_fn(64, 48, |x, y| [0.05 + x as f32 * 0.01, 0.1 + y as f32 * 0.01, 0.08]);
        let f = extract(&img, &SourceInfo::default());
        assert_eq!(f.values.len(), N_FEATURES);
        assert!(f.values.iter().all(|v| v.is_finite()), "{:?}", f.values);
    }

    #[test]
    fn a_dark_photo_reads_darker_than_a_bright_one() {
        let dark = extract(&Rgb32f::from_fn(32, 32, |_, _| [0.01; 3]), &SourceInfo::default());
        let bright = extract(&Rgb32f::from_fn(32, 32, |_, _| [0.6; 3]), &SourceInfo::default());
        let p50 = FEATURE_NAMES.iter().position(|n| *n == "lum_p50").unwrap();
        assert!(dark.values[p50] < bright.values[p50]);
        assert!(dark.auto_exposure > bright.auto_exposure);
    }

    #[test]
    fn empty_and_tiny_images_do_not_panic() {
        let f = extract(&Rgb32f::from_fn(1, 1, |_, _| [f32::NAN, 0.0, 0.0]), &SourceInfo::default());
        assert_eq!(f.values.len(), N_FEATURES);
    }
}
