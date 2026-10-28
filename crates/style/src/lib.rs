//! "My style": learn how a photographer edits, then suggest those edits on new photos.
//!
//! Everything is local and deterministic: no network, no model weights, no random start. A photo
//! is described by a few dozen numbers measured on its *unedited* pixels ([`features`]); the
//! photographer's edits become a vector of develop sliders ([`targets`]); a ridge regression per
//! slider ([`model`], [`train`]) maps one to the other. A slider the data cannot predict better
//! than the photographer's own median is not predicted at all (the model says so), and the
//! numbers it was judged on (leave-shoots-out cross-validation, against "no adjustment" and
//! "median adjustment") are stored with it. [`series`] keeps one shoot coherent.
//!
//! No UI, no I/O, no unsafe code (L3).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod features;
pub mod folds;
pub mod linalg;
pub mod model;
pub mod ridge;
pub mod series;
pub mod synth;
pub mod targets;
pub mod train;

pub use features::{FEATURE_NAMES, Features, extract};
pub use model::{StyleModel, TargetFit};
pub use targets::{N_TARGETS, TARGETS, Targets};
pub use train::{Config, Sample, train};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("not enough edited photos to learn a style: {have} usable, at least {need} needed")]
    TooFew { have: usize, need: usize },
    #[error("the stored style profile is invalid: {0}")]
    Invalid(String),
    #[error("the photos' features have different lengths")]
    Ragged,
    #[error("the system could not be solved (degenerate features)")]
    Singular,
    #[error("learning stopped")]
    Cancelled,
    #[error("{0}")]
    Fit(String),
}

pub type Result<T> = std::result::Result<T, Error>;
