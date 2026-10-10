//! Cross-validation of the taste model and the final fit.
//!
//! The folds hold out *whole shoots* (a shoot is any id the caller groups photos by — the
//! folder): the frames of one burst look alike, so cutting the folds at random would let the
//! model be tested on a twin of what it learned and flatter itself. Everything reported is
//! measured on photos of shoots the model never saw.

use crate::model::{Counts, FORMAT, Metrics, TasteModel};
use crate::ridge::{self, Ridge};
use crate::{Error, Result, stats};

/// What a training photo is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The user gave it stars.
    Rated,
    /// The user rejected it.
    Rejected,
    /// Unrated, but from a shoot the user clearly went through: "not chosen".
    Implicit,
}

/// One labelled photo.
#[derive(Clone, Debug)]
pub struct Sample {
    pub features: Vec<f32>,
    /// Stars (rated), 0 (rejected or not chosen).
    pub target: f32,
    pub weight: f32,
    /// What the folds keep together (a shoot, a burst, or the photo itself).
    pub group: u64,
    /// The shoot (folder) it belongs to, for the per-shoot numbers.
    pub shoot: u64,
    pub kind: Kind,
}

/// The settings of a training run.
#[derive(Clone, Debug)]
pub struct Config {
    pub lambdas: Vec<f32>,
    pub folds: usize,
    pub seed: u64,
    /// At least this many photos must be rated.
    pub min_rated: usize,
    /// Cut the folds by `Sample::group` (`true`) or photo by photo at random (`false`; only for comparison).
    pub by_shoot: bool,
    /// The column of the log-sharpness feature (for the baseline), if there is one.
    pub sharpness_col: Option<usize>,
}

impl Default for Config {
    fn default() -> Self {
        Self { lambdas: vec![0.3, 1.0, 3.0, 10.0, 30.0, 100.0, 300.0], folds: 5, seed: 20261009, min_rated: 30, by_shoot: true, sharpness_col: None }
    }
}

/// The report of a cross-validation (an alias to keep the public name short).
pub type CvReport = Metrics;

/// The fold of every sample (whole shoots together when `cfg.by_shoot`).
pub fn fold_of(samples: &[Sample], cfg: &Config) -> Vec<usize> {
    let group: Vec<u64> = if cfg.by_shoot { samples.iter().map(|s| s.group).collect() } else { (0..samples.len() as u64).collect() };
    stats::group_folds(&group, cfg.folds, cfg.seed)
}

/// Out-of-fold predictions: for every lambda, the prediction for each sample made by a model that
/// never saw its fold.
pub fn out_of_fold(samples: &[Sample], fold: &[usize], cfg: &Config) -> Result<Vec<Vec<f32>>> {
    let mut oof = vec![vec![f32::NAN; samples.len()]; cfg.lambdas.len()];
    for k in 0..cfg.folds.max(1) {
        let train: Vec<&Sample> = samples.iter().zip(fold).filter(|(_, f)| **f != k).map(|(s, _)| s).collect();
        if train.is_empty() {
            continue;
        }
        let x: Vec<&[f32]> = train.iter().map(|s| s.features.as_slice()).collect();
        let y: Vec<f32> = train.iter().map(|s| s.target).collect();
        let w: Vec<f32> = train.iter().map(|s| s.weight).collect();
        let models = ridge::fit_path(&x, &y, &w, &cfg.lambdas)?;
        for (li, m) in models.iter().enumerate() {
            for (i, s) in samples.iter().enumerate() {
                if fold[i] == k {
                    oof[li][i] = m.predict(&s.features).unwrap_or(f32::NAN);
                }
            }
        }
    }
    Ok(oof)
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Metrics of one set of out-of-fold predictions. Rank metrics are computed **inside each held-out
/// fold and then averaged**: with whole shoots held out, folds differ in their average stars, and
/// pooling the predictions of different models would reward (or punish) that difference instead of
/// the ranking of photos.
pub fn measure(samples: &[Sample], pred: &[f32], fold: &[usize], sharpness_col: Option<usize>) -> Metrics {
    let k = fold.iter().copied().max().map_or(0, |m| m + 1);
    let (mut sp_rated, mut sp_all, mut aucs) = (Vec::new(), Vec::new(), Vec::new());
    for f in 0..k {
        let idx: Vec<usize> = (0..samples.len()).filter(|i| fold[*i] == f && pred[*i].is_finite()).collect();
        let rated: Vec<usize> = idx.iter().copied().filter(|i| samples[*i].kind == Kind::Rated).collect();
        if rated.len() >= 10 {
            let (p, t): (Vec<f32>, Vec<f32>) = rated.iter().map(|i| (pred[*i], samples[*i].target)).unzip();
            sp_rated.extend(stats::spearman(&p, &t));
        }
        if idx.len() >= 10 {
            let (p, t): (Vec<f32>, Vec<f32>) = idx.iter().map(|i| (pred[*i], samples[*i].target)).unzip();
            sp_all.extend(stats::spearman(&p, &t));
        }
        let (p, c): (Vec<f32>, Vec<bool>) = idx.iter().map(|i| (pred[*i], samples[*i].kind == Kind::Rated)).unzip();
        aucs.extend(stats::auc(&p, &c));
    }
    // top-k hit rate per shoot
    let mut by_group: std::collections::BTreeMap<u64, Vec<(f32, bool)>> = Default::default();
    for (s, p) in samples.iter().zip(pred) {
        if p.is_finite() {
            by_group.entry(s.shoot).or_default().push((*p, s.kind == Kind::Rated));
        }
    }
    let (mut hits, mut asked, mut chance) = (0usize, 0usize, 0.0f64);
    for v in by_group.values() {
        let r = v.iter().filter(|x| x.1).count();
        if r < 2 || r >= v.len() {
            continue;
        }
        let (s, rel): (Vec<f32>, Vec<bool>) = v.iter().copied().unzip();
        hits += stats::hits_at_k(&s, &rel, r);
        asked += r;
        chance += (r * r) as f64 / v.len() as f64;
    }
    let (top_k_hit_rate, top_k_chance) = if asked > 0 { (Some(hits as f64 / asked as f64), Some(chance / asked as f64)) } else { (None, None) };
    // the same baseline: sharpness alone, per fold
    let spearman_sharpness_only = sharpness_col.and_then(|c| {
        let mut v = Vec::new();
        for f in 0..k {
            let (a, b): (Vec<f32>, Vec<f32>) = samples
                .iter()
                .zip(fold)
                .filter(|(s, ff)| **ff == f && s.kind == Kind::Rated)
                .filter_map(|(s, _)| s.features.get(c).map(|v| (*v, s.target)))
                .unzip();
            if a.len() >= 10 {
                v.extend(stats::spearman(&a, &b));
            }
        }
        mean(&v)
    });
    Metrics {
        spearman_rated: mean(&sp_rated),
        spearman_all: mean(&sp_all),
        auc_chosen: mean(&aucs),
        top_k_hit_rate,
        top_k_chance,
        spearman_sharpness_only,
        ..Default::default()
    }
}

fn counts(samples: &[Sample]) -> Counts {
    let mut c = Counts::default();
    let mut groups = std::collections::BTreeSet::new();
    for s in samples {
        groups.insert(s.shoot);
        match s.kind {
            Kind::Rated => {
                c.rated += 1;
                let i = (s.target.round() as i64 - 1).clamp(0, 4) as usize;
                c.stars[i] += 1;
            }
            Kind::Rejected => c.rejected += 1,
            Kind::Implicit => c.implicit += 1,
        }
    }
    c.groups = groups.len();
    c
}

/// Cross-validate every lambda of `cfg`, pick the best — by the AUC of chosen vs not-chosen photos
/// when there are not-chosen photos, else by the Spearman correlation on the rated ones — and
/// report it. Returns the report and the out-of-fold predictions of the chosen lambda.
pub fn cross_validate(samples: &[Sample], cfg: &Config) -> Result<(Metrics, Vec<f32>)> {
    let rated = samples.iter().filter(|s| s.kind == Kind::Rated).count();
    if rated < cfg.min_rated {
        return Err(Error::TooFew { have: rated, need: cfg.min_rated });
    }
    let fold = fold_of(samples, cfg);
    let oof = out_of_fold(samples, &fold, cfg)?;
    let mut best: Option<(usize, f64)> = None;
    let mut tried = Vec::new();
    for (li, pred) in oof.iter().enumerate() {
        let m = measure(samples, pred, &fold, cfg.sharpness_col);
        let crit = m.auc_chosen.or(m.spearman_rated);
        tried.push((cfg.lambdas[li], crit));
        if let Some(c) = crit
            && best.is_none_or(|b| c > b.1)
        {
            best = Some((li, c));
        }
    }
    let (li, _) = best.ok_or(Error::TooFew { have: rated, need: cfg.min_rated })?;
    let mut m = measure(samples, &oof[li], &fold, cfg.sharpness_col);
    m.folds = cfg.folds;
    m.split = if cfg.by_shoot { "grouped" } else { "photo" }.into();
    m.lambda = cfg.lambdas[li];
    m.tried = tried;
    Ok((m, oof[li].clone()))
}

fn quantile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f32 * q).round() as usize;
    sorted.get(i).copied().unwrap_or(0.0)
}

/// Cross-validate, then fit the final model on every labelled photo with the chosen lambda.
pub fn train(samples: &[Sample], embedding: &str, embedding_dim: usize, extras: &[String], trained: &str, cfg: &Config) -> Result<TasteModel> {
    let (metrics, oof) = cross_validate(samples, cfg)?;
    let x: Vec<&[f32]> = samples.iter().map(|s| s.features.as_slice()).collect();
    let y: Vec<f32> = samples.iter().map(|s| s.target).collect();
    let w: Vec<f32> = samples.iter().map(|s| s.weight).collect();
    let ridge: Ridge = ridge::fit(&x, &y, &w, metrics.lambda)?;
    let mut spread: Vec<f32> = oof.into_iter().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 5.0)).collect();
    spread.sort_by(f32::total_cmp);
    let model = TasteModel {
        format: FORMAT,
        embedding: embedding.into(),
        embedding_dim,
        extras: extras.to_vec(),
        ridge,
        counts: counts(samples),
        metrics,
        trained: trained.into(),
        spread: [quantile(&spread, 0.1), quantile(&spread, 0.5), quantile(&spread, 0.9)],
    };
    model.validate()?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
    }

    /// 20 shoots of 30 photos; the stars depend on features 0 and 1 (plus noise), the others are noise.
    fn data(noise: f32) -> Vec<Sample> {
        let mut s = 11u64;
        let mut out = Vec::new();
        for g in 0..20u64 {
            for _ in 0..30 {
                let f: Vec<f32> = (0..8).map(|_| lcg(&mut s)).collect();
                let q = 2.0 * f[0] + f[1] + noise * lcg(&mut s);
                let (kind, target) = if q > 0.6 { (Kind::Rated, if q > 1.0 { 3.0 } else { 1.0 }) } else { (Kind::Implicit, 0.0) };
                out.push(Sample { features: f, target, weight: if kind == Kind::Rated { 1.0 } else { 0.5 }, group: g, shoot: g, kind });
            }
        }
        out
    }

    #[test]
    fn learns_a_taste_it_has_not_seen_and_says_how_well() {
        let samples = data(0.2);
        let cfg = Config { lambdas: vec![0.01, 1.0, 100.0], min_rated: 10, ..Default::default() };
        let (m, oof) = cross_validate(&samples, &cfg).unwrap();
        assert_eq!(oof.len(), samples.len());
        assert!(m.spearman_all.unwrap() > 0.6, "{m:?}");
        assert!(m.auc_chosen.unwrap() > 0.9, "{m:?}");
        assert!(m.top_k_hit_rate.unwrap() > m.top_k_chance.unwrap() + 0.2, "{m:?}");
        assert_eq!(m.split, "grouped");
    }

    #[test]
    fn pure_noise_scores_about_chance() {
        let mut samples = data(0.2);
        // stars unrelated to the features
        let mut s = 99u64;
        for x in &mut samples {
            x.kind = if lcg(&mut s) > 0.3 { Kind::Rated } else { Kind::Implicit };
            x.target = if x.kind == Kind::Rated { 1.0 + (lcg(&mut s) + 0.5).round() } else { 0.0 };
        }
        let cfg = Config { lambdas: vec![10.0, 100.0], min_rated: 10, ..Default::default() };
        let (m, _) = cross_validate(&samples, &cfg).unwrap();
        assert!(m.auc_chosen.unwrap() < 0.62, "{m:?}");
    }

    #[test]
    fn refuses_too_few_rated_photos_and_trains_a_valid_model() {
        let samples = data(0.1);
        let few: Vec<Sample> = samples
            .iter()
            .filter(|s| s.kind != Kind::Rated)
            .cloned()
            .chain(samples.iter().filter(|s| s.kind == Kind::Rated).take(3).cloned())
            .collect();
        assert!(matches!(cross_validate(&few, &Config::default()), Err(Error::TooFew { .. })));
        let names: Vec<String> = vec![];
        let cfg = Config { lambdas: vec![1.0], min_rated: 10, ..Default::default() };
        let m = train(&samples, "e", 8, &names, "now", &cfg).unwrap();
        assert!(m.counts.rated > 50 && m.counts.groups == 20);
        let p_hi = m.predict(&[0.4, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], &[]).unwrap();
        let p_lo = m.predict(&[-0.4, -0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], &[]).unwrap();
        assert!(p_hi > p_lo + 0.5, "{p_hi} {p_lo}");
        assert!(m.spread[0] <= m.spread[1] && m.spread[1] <= m.spread[2]);
    }
}
