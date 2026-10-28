//! Learn a photographer's taste from the photos they rated, in pure Rust.
//!
//! The model is deliberately small: a ridge regression (closed form, no iteration, no random
//! initialisation, so a run is reproducible) on a feature vector per photo — the unit SigLIP 2
//! embedding the search crate already computes, plus a few technical measurements (focus,
//! clipping, exposure). It is trained on the user's own stars, never on anybody else's, and it
//! only ever *suggests*: nothing here touches a rating.
//!
//! - [`ridge`]: weighted ridge regression with standardised features.
//! - [`stats`]: Spearman correlation, ROC AUC, top-k precision, grouped k-fold splits.
//! - [`model`]: the trained model as it is stored (JSON), with the cross-validated metrics.
//! - [`eval`]: cross-validation of the whole thing and the choice of the regularisation.
//!
//! No UI, no model weights, no unsafe code (L3).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod eval;
pub mod linalg;
pub mod model;
pub mod ridge;
pub mod stats;

pub use eval::{CvReport, Sample, cross_validate, train};
pub use model::TasteModel;
pub use ridge::Ridge;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("not enough rated photos to learn from: {have} usable, at least {need} needed")]
    TooFew { have: usize, need: usize },
    #[error("the photos' features have different lengths")]
    Ragged,
    #[error("the system could not be solved (degenerate features)")]
    Singular,
    #[error("the stored taste model is invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
