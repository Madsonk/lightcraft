//! The trained taste model as it is stored next to the library (`taste.json`), with the
//! cross-validated numbers it was judged on, so what the user is told about it is what was
//! measured and not a guess.

use serde::{Deserialize, Serialize};

use crate::ridge::Ridge;
use crate::{Error, Result};

/// Version of the file layout (bumped when the features change meaning).
pub const FORMAT: u32 = 1;

/// How many photos the model learned from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Counts {
    /// Photos the user gave stars (1..5).
    pub rated: usize,
    /// Photos the user rejected.
    pub rejected: usize,
    /// Unrated photos from shoots the user clearly went through (counted as "not chosen", with a
    /// lower weight).
    pub implicit: usize,
    /// Shoots (folders) among them.
    pub groups: usize,
    /// Stars the rated photos got: how many with 1, 2, 3, 4, 5.
    pub stars: [usize; 5],
}

/// How well the model does on photos it did not learn from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Metrics {
    pub folds: usize,
    /// How the folds were cut: `"shoot"` (whole folders held out), `"burst"` (whole 5-minute
    /// windows of a folder held out) or `"photo"` (at random: twins leak, an upper bound).
    pub split: String,
    /// Spearman correlation between predicted and actual stars on the rated photos held out,
    /// computed inside each held-out fold and averaged.
    pub spearman_rated: Option<f64>,
    /// The same on every labelled photo held out (rated, rejected, implicit "not chosen").
    pub spearman_all: Option<f64>,
    /// Chance that a chosen photo outscores a not-chosen one (per held-out fold, averaged).
    pub auc_chosen: Option<f64>,
    /// In each held-out shoot with R chosen photos: the share of the R best-scored that really
    /// were chosen (averaged over photos), and what picking at random would give.
    pub top_k_hit_rate: Option<f64>,
    pub top_k_chance: Option<f64>,
    /// Spearman of a baseline that only looks at sharpness (to show what the learning adds).
    pub spearman_sharpness_only: Option<f64>,
    /// The regularisation chosen and the criterion each candidate got (AUC of chosen vs not chosen
    /// when the data has not-chosen photos, else Spearman on the rated ones).
    pub lambda: f32,
    pub tried: Vec<(f32, Option<f64>)>,
}

/// The stored model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteModel {
    pub format: u32,
    /// The embedding model whose vectors it expects (`siglip-<size>`): another one invalidates it.
    pub embedding: String,
    /// Length of the embedding part of the feature vector.
    pub embedding_dim: usize,
    /// Names of the extra features that follow the embedding.
    pub extras: Vec<String>,
    pub ridge: Ridge,
    pub counts: Counts,
    pub metrics: Metrics,
    /// ISO 8601 (from the engine's clock).
    pub trained: String,
    /// The predictions' spread over the training photos: the 10th, 50th and 90th percentile of
    /// the predicted stars, to turn a score into "better than N % of what you shot".
    pub spread: [f32; 3],
}

impl TasteModel {
    /// Check a model read from disk: right layout, lengths agree.
    pub fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::Invalid(format!("format {} (this build reads {FORMAT})", self.format)));
        }
        let d = self.embedding_dim + self.extras.len();
        if self.ridge.dim() != d || self.ridge.mean.len() != d || self.ridge.scale.len() != d {
            return Err(Error::Invalid("the feature lengths disagree".into()));
        }
        if self.ridge.scale.iter().any(|s| !(*s > 0.0 && s.is_finite())) {
            return Err(Error::Invalid("a feature scale is not positive".into()));
        }
        Ok(())
    }

    /// Predicted stars (0..5, not rounded) for a photo's embedding and extra measurements.
    pub fn predict(&self, embedding: &[f32], extras: &[f32]) -> Option<f32> {
        if embedding.len() != self.embedding_dim || extras.len() != self.extras.len() {
            return None;
        }
        let mut f = Vec::with_capacity(embedding.len() + extras.len());
        f.extend_from_slice(embedding);
        f.extend_from_slice(extras);
        self.ridge.predict(&f).map(|v| v.clamp(0.0, 5.0))
    }

    /// Where a predicted score falls among the training photos' predictions, 0..1 (a rough
    /// percentile from three anchor points).
    pub fn percentile(&self, score: f32) -> f32 {
        let [p10, p50, p90] = self.spread;
        let lerp = |a: f32, b: f32, x: f32, lo: f32, hi: f32| if b > a { lo + (hi - lo) * ((x - a) / (b - a)).clamp(0.0, 1.0) } else { lo };
        if score < p10 {
            0.1 * (score / p10.max(1e-6)).clamp(0.0, 1.0)
        } else if score < p50 {
            lerp(p10, p50, score, 0.1, 0.5)
        } else if score < p90 {
            lerp(p50, p90, score, 0.5, 0.9)
        } else {
            0.9 + 0.1 * ((score - p90) / (5.0 - p90).max(1e-6)).clamp(0.0, 1.0)
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let m: Self = serde_json::from_str(s).map_err(|e| Error::Invalid(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> TasteModel {
        TasteModel {
            format: FORMAT,
            embedding: "e".into(),
            embedding_dim: 2,
            extras: vec!["sharp".into()],
            ridge: Ridge { mean: vec![0.0; 3], scale: vec![1.0; 3], weights: vec![1.0, 0.0, 0.5], bias: 2.0, lambda: 1.0 },
            counts: Counts::default(),
            metrics: Metrics::default(),
            trained: "2026-10-09T00:00:00".into(),
            spread: [1.0, 2.0, 3.0],
        }
    }

    #[test]
    fn predicts_clamps_and_round_trips() {
        let m = tiny();
        assert_eq!(m.predict(&[1.0, 0.0], &[2.0]), Some(4.0));
        assert_eq!(m.predict(&[10.0, 0.0], &[2.0]), Some(5.0));
        assert_eq!(m.predict(&[1.0], &[2.0]), None);
        let back = TasteModel::from_json(&m.to_json()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn rejects_a_model_with_the_wrong_layout() {
        let mut m = tiny();
        m.ridge.weights.pop();
        assert!(TasteModel::from_json(&m.to_json()).is_err());
        let mut m = tiny();
        m.format = 99;
        assert!(m.validate().is_err());
        assert!(TasteModel::from_json("{").is_err());
    }

    #[test]
    fn percentile_is_monotone() {
        let m = tiny();
        let ps: Vec<f32> = [0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0].iter().map(|s| m.percentile(*s)).collect();
        assert!(ps.windows(2).all(|w| w[0] <= w[1]), "{ps:?}");
        assert!((m.percentile(2.0) - 0.5).abs() < 1e-6);
    }
}
