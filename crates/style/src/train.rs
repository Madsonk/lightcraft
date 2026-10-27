//! Learning: one ridge regression per slider, judged by leave-shoots-out cross-validation.
//!
//! The regularisation is chosen by an inner cross-validation *inside* each training fold, so the
//! reported error is not tuned on the photos it is measured on. A slider that does not beat the
//! photographer's own median in that outer cross-validation is marked not learned.

use lightcraft_taste::ridge;
use lightcraft_taste::stats::group_folds;

use crate::features::N_FEATURES;
use crate::model::{FORMAT, StyleModel, Summary, TargetFit};
use crate::targets::{N_TARGETS, TARGETS, Targets};
use crate::{Error, Result};

/// One edited photo: its unedited features, the sliders its edit amounts to, and its shoot.
#[derive(Clone, Debug)]
pub struct Sample {
    pub features: Vec<f32>,
    pub targets: Targets,
    /// Any id shared by the photos of one shoot (folder): they never straddle a fold.
    pub group: u64,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub folds: usize,
    pub seed: u64,
    pub lambdas: Vec<f32>,
    pub min_samples: usize,
    /// A slider counts as learned only if its model error is below this share of the median's.
    pub gate: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self { folds: 5, seed: 7, lambdas: vec![3.0, 10.0, 30.0, 100.0, 300.0, 1000.0, 3000.0], min_samples: 12, gate: 0.97 }
    }
}

fn median(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let n = s.len();
    match (s.get(n / 2), s.get(n.saturating_sub(1) / 2)) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        _ => 0.0,
    }
}

fn mae(pred: &[f32], truth: &[f32]) -> f32 {
    if truth.is_empty() {
        return 0.0;
    }
    pred.iter().zip(truth).map(|(p, t)| (p - t).abs()).sum::<f32>() / truth.len() as f32
}

fn rows<'a>(samples: &'a [Sample], idx: &[usize]) -> Vec<&'a [f32]> {
    idx.iter().filter_map(|i| samples.get(*i)).map(|s| s.features.as_slice()).collect()
}

fn column(samples: &[Sample], idx: &[usize], t: usize) -> Vec<f32> {
    idx.iter().filter_map(|i| samples.get(*i)).map(|s| s.targets.get(t).copied().unwrap_or(0.0)).collect()
}

/// Pick the regularisation for target `t` on the samples `idx` by an inner grouped cross-validation.
fn pick_lambda(samples: &[Sample], idx: &[usize], t: usize, cfg: &Config) -> f32 {
    let groups: Vec<u64> = idx.iter().filter_map(|i| samples.get(*i)).map(|s| s.group).collect();
    let k = cfg.folds.clamp(2, 4);
    let fold = group_folds(&groups, k, cfg.seed ^ 0x9e37);
    let mut best = (f32::MAX, cfg.lambdas.first().copied().unwrap_or(100.0));
    let mut err = vec![0f32; cfg.lambdas.len()];
    let mut n = 0usize;
    for f in 0..k {
        let tr: Vec<usize> = idx.iter().zip(&fold).filter(|(_, g)| **g != f).map(|(i, _)| *i).collect();
        let te: Vec<usize> = idx.iter().zip(&fold).filter(|(_, g)| **g == f).map(|(i, _)| *i).collect();
        if tr.len() < 4 || te.is_empty() {
            continue;
        }
        let (x, y) = (rows(samples, &tr), column(samples, &tr, t));
        let w = vec![1.0f32; y.len()];
        let Ok(path) = ridge::fit_path(&x, &y, &w, &cfg.lambdas) else { continue };
        let truth = column(samples, &te, t);
        for (li, r) in path.iter().enumerate() {
            let p: Vec<f32> = te.iter().filter_map(|i| samples.get(*i)).map(|s| r.predict(&s.features).unwrap_or(0.0)).collect();
            if let Some(e) = err.get_mut(li) {
                *e += mae(&p, &truth) * te.len() as f32;
            }
        }
        n += te.len();
    }
    if n > 0 {
        for (li, e) in err.iter().enumerate() {
            if *e < best.0 {
                best = (*e, cfg.lambdas.get(li).copied().unwrap_or(100.0));
            }
        }
    }
    best.1
}

/// Learn a style. `progress(done_fraction)` is called between units of work; returning `false`
/// stops the learning with [`Error::Cancelled`].
pub fn train(samples: &[Sample], name: &str, trained: &str, cfg: &Config, progress: &mut dyn FnMut(f32) -> bool) -> Result<StyleModel> {
    let usable: Vec<Sample> = samples
        .iter()
        .filter(|s| s.features.len() == N_FEATURES && s.features.iter().all(|v| v.is_finite()) && s.targets.iter().all(|v| v.is_finite()))
        .cloned()
        .collect();
    if usable.len() < cfg.min_samples {
        return Err(Error::TooFew { have: usable.len(), need: cfg.min_samples });
    }
    let groups: Vec<u64> = usable.iter().map(|s| s.group).collect();
    let mut distinct = groups.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let by_shoot = distinct.len() >= cfg.folds.max(2);
    let k = cfg.folds.clamp(2, usable.len());
    let fold = if by_shoot { group_folds(&groups, k, cfg.seed) } else { (0..usable.len()).map(|i| i % k).collect() };
    let all: Vec<usize> = (0..usable.len()).collect();
    let total_steps = (N_TARGETS * (k + 1)) as f32;
    let mut step = 0f32;
    let mut fits = Vec::with_capacity(N_TARGETS);
    for (t, spec) in TARGETS.iter().enumerate() {
        let truth_all = column(&usable, &all, t);
        let mut oof = vec![0f32; usable.len()];
        let mut oof_median = vec![0f32; usable.len()];
        for f in 0..k {
            if !progress(step / total_steps) {
                return Err(Error::Cancelled);
            }
            step += 1.0;
            let tr: Vec<usize> = all.iter().zip(&fold).filter(|(_, g)| **g != f).map(|(i, _)| *i).collect();
            let te: Vec<usize> = all.iter().zip(&fold).filter(|(_, g)| **g == f).map(|(i, _)| *i).collect();
            if tr.len() < 4 || te.is_empty() {
                continue;
            }
            let med = median(&column(&usable, &tr, t));
            let lambda = pick_lambda(&usable, &tr, t, cfg);
            let (x, y) = (rows(&usable, &tr), column(&usable, &tr, t));
            let w = vec![1.0f32; y.len()];
            let r = ridge::fit(&x, &y, &w, lambda).map_err(|e| Error::Fit(e.to_string()))?;
            for i in te {
                if let Some(s) = usable.get(i) {
                    let p = r.predict(&s.features).unwrap_or(med).clamp(spec.min, spec.max);
                    if let Some(o) = oof.get_mut(i) {
                        *o = p;
                    }
                    if let Some(o) = oof_median.get_mut(i) {
                        *o = med;
                    }
                }
            }
        }
        if !progress(step / total_steps) {
            return Err(Error::Cancelled);
        }
        step += 1.0;
        let med_all = median(&truth_all);
        let lambda = pick_lambda(&usable, &all, t, cfg);
        let (x, y) = (rows(&usable, &all), truth_all.clone());
        let w = vec![1.0f32; y.len()];
        let r = ridge::fit(&x, &y, &w, lambda).map_err(|e| Error::Fit(e.to_string()))?;
        let mae_model = mae(&oof, &truth_all);
        let mae_median = mae(&oof_median, &truth_all);
        let mae_none = mae(&vec![0.0; truth_all.len()], &truth_all);
        let used = truth_all.iter().filter(|v| v.abs() >= spec.dead_zone).count() as f32 / truth_all.len().max(1) as f32;
        fits.push(TargetFit {
            id: spec.id.to_string(),
            learned: mae_model < cfg.gate * mae_median && mae_median > 1e-6,
            median: med_all,
            used_share: used,
            ridge: Some(r),
            mae_model,
            mae_none,
            mae_median,
        });
    }
    let moved: Vec<&TargetFit> = fits.iter().filter(|f| f.mae_none > 1e-3 && f.used_share >= 0.1).collect();
    let skill = |f: fn(&TargetFit) -> f32| {
        if moved.is_empty() {
            0.0
        } else {
            moved.iter().map(|t| 1.0 - t.mae_model / f(t).max(1e-6)).sum::<f32>() / moved.len() as f32
        }
    };
    let summary = Summary {
        folds: k,
        split: if by_shoot { "shoot" } else { "photo" }.into(),
        skill_vs_none: skill(|t| t.mae_none),
        skill_vs_median: skill(|t| t.mae_median),
        learned_targets: fits.iter().filter(|f| f.learned).count(),
        train_seconds: 0.0,
    };
    progress(1.0);
    Ok(StyleModel {
        format: FORMAT,
        name: name.to_string(),
        features: crate::features::FEATURE_NAMES.iter().map(|s| s.to_string()).collect(),
        fits,
        samples: usable.len(),
        groups: distinct.len(),
        summary,
        trained: trained.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    fn id(name: &str) -> usize {
        TARGETS.iter().position(|t| t.id == name).unwrap()
    }

    fn fit(n: usize, shoots: usize, seed: u64) -> StyleModel {
        train(&synth::samples(n, shoots, seed), "test", "2026-10-27T00:00:00", &Config::default(), &mut |_| true).unwrap()
    }

    #[test]
    fn learns_a_known_photographer_and_leaves_the_noise_alone() {
        let m = fit(150, 15, 3);
        assert_eq!(m.summary.split, "shoot");
        assert!(m.fits[id("exposure")].learned, "exposure follows auto exposure");
        assert!(m.fits[id("shadows")].learned);
        assert!(!m.fits[id("curve_shadows")].learned, "pure noise must not be learned: {:?}", m.fits[id("curve_shadows")]);
        assert!(m.summary.skill_vs_none > 0.3, "{:?}", m.summary);
        // on photos it never saw the model beats both baselines on the learned sliders
        let fresh = synth::samples(60, 6, 99);
        for name in ["exposure", "shadows", "contrast"] {
            let i = id(name);
            let (mut e_model, mut e_median, mut e_none) = (0.0, 0.0, 0.0);
            for s in &fresh {
                let p = m.predict(&s.features).unwrap()[i];
                e_model += (p - s.targets[i]).abs();
                e_median += (m.fits[i].median - s.targets[i]).abs();
                e_none += s.targets[i].abs();
            }
            assert!(e_model < 0.7 * e_median && e_model < e_none, "{name}: model {e_model} median {e_median} none {e_none}");
        }
    }

    #[test]
    fn the_profile_round_trips_and_stays_small() {
        let m = fit(60, 8, 5);
        let back = StyleModel::from_json(&m.to_json()).unwrap();
        assert_eq!(back, m);
        assert!(m.to_json().len() < 60_000, "{} bytes", m.to_json().len());
        assert!(StyleModel::from_json("{").is_err());
        let mut bad = m.clone();
        bad.fits.pop();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn too_few_photos_stop_early_and_cancel_is_honoured() {
        let few = synth::samples(5, 5, 1);
        assert_eq!(train(&few, "x", "t", &Config::default(), &mut |_| true), Err(Error::TooFew { have: 5, need: 12 }));
        let many = synth::samples(40, 8, 2);
        assert_eq!(train(&many, "x", "t", &Config::default(), &mut |_| false), Err(Error::Cancelled));
    }

    #[test]
    fn broken_samples_are_skipped_not_fatal() {
        let mut s = synth::samples(40, 8, 4);
        s[0].features[3] = f32::NAN;
        s[1].targets[0] = f32::INFINITY;
        s[2].features.pop();
        let m = train(&s, "x", "t", &Config::default(), &mut |_| true).unwrap();
        assert_eq!(m.samples, 37);
    }

    #[test]
    fn one_shoot_only_falls_back_to_photo_folds_and_says_so() {
        let m = fit(40, 1, 6);
        assert_eq!(m.summary.split, "photo");
    }
}
