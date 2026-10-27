//! The stored style profile (`style/<name>.json` next to the library), with the numbers it was
//! judged on so what the photographer is told about it is what was measured.

use lightcraft_taste::Ridge;
use serde::{Deserialize, Serialize};

use crate::features::N_FEATURES;
use crate::targets::{self, N_TARGETS, TARGETS, Targets};
use crate::{Error, Result};

pub const FORMAT: u32 = 1;

/// What one slider got from the learning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetFit {
    pub id: String,
    /// `false`: the data did not predict this slider better than the photographer's median, so the
    /// profile only suggests the median (and `apply` can skip it).
    pub learned: bool,
    /// The photographer's median value of this slider over the training photos.
    pub median: f32,
    /// Share of training photos where the slider is moved at all.
    pub used_share: f32,
    pub ridge: Option<Ridge>,
    /// Mean absolute error on photos held out (whole shoots held out), in slider units: the
    /// profile, no adjustment at all, and the photographer's median.
    pub mae_model: f32,
    pub mae_none: f32,
    pub mae_median: f32,
}

/// The overall numbers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Summary {
    pub folds: usize,
    /// How the folds were cut: `"shoot"` (whole folders held out) or `"photo"`.
    pub split: String,
    /// Average over the sliders the photographer really moves of `1 − mae_model / mae_none` and
    /// `1 − mae_model / mae_median`: positive means better than the baseline.
    pub skill_vs_none: f32,
    pub skill_vs_median: f32,
    pub learned_targets: usize,
    pub train_seconds: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleModel {
    pub format: u32,
    pub name: String,
    pub features: Vec<String>,
    pub fits: Vec<TargetFit>,
    /// Edited photos learned from, and shoots (folders) among them.
    pub samples: usize,
    pub groups: usize,
    pub summary: Summary,
    /// ISO 8601 (from the engine's clock).
    pub trained: String,
}

impl StyleModel {
    pub fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::Invalid(format!("format {} (this build reads {FORMAT})", self.format)));
        }
        if self.fits.len() != N_TARGETS || self.features.len() != N_FEATURES {
            return Err(Error::Invalid("the profile does not match this build's sliders".into()));
        }
        for (f, spec) in self.fits.iter().zip(TARGETS.iter()) {
            if f.id != spec.id {
                return Err(Error::Invalid(format!("slider {} where {} expected", f.id, spec.id)));
            }
            if let Some(r) = &f.ridge {
                let ok = r.dim() == N_FEATURES
                    && r.mean.len() == N_FEATURES
                    && r.scale.len() == N_FEATURES
                    && r.scale.iter().all(|s| *s > 0.0 && s.is_finite())
                    && r.weights.iter().all(|w| w.is_finite());
                if !ok {
                    return Err(Error::Invalid(format!("slider {} has a broken model", f.id)));
                }
            }
        }
        Ok(())
    }

    /// The sliders suggested for a photo's features; `None` when the length is wrong. Sliders the
    /// data could not learn get the photographer's median.
    pub fn predict(&self, features: &[f32]) -> Option<Targets> {
        if features.len() != N_FEATURES || self.fits.len() != N_TARGETS {
            return None;
        }
        let mut out = [0f32; N_TARGETS];
        for (slot, f) in out.iter_mut().zip(&self.fits) {
            *slot = match (&f.ridge, f.learned) {
                (Some(r), true) => r.predict(features).unwrap_or(f.median),
                _ => f.median,
            };
        }
        targets::clamp(&mut out);
        Some(out)
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
