//! Recognizing the people of a photo library.
//!
//! - [`detect`]: the faces of a photo (the MediaPipe Face Landmarker of `lightcraft-facemesh`) with
//!   five alignment landmarks each.
//! - [`align`]: the landmarks are mapped onto the ArcFace template and a 112 × 112 crop is sampled.
//! - [`recog`]: InsightFace's ArcFace network turns a crop into 512 numbers (pure Rust, ONNX run
//!   by `lightcraft-inpaint`'s interpreter on candle).
//! - [`store`]: the index, one record per face, an append-only file next to the catalog.
//! - [`cluster`]: groups the faces of one person, and tells which known person a face looks like.
//! - [`state`]: the user's decisions (names, confirmations, "not this person").
//!
//! **The weights are never part of LightCraft**: they are InsightFace's models, released for
//! non-commercial research, which the user downloads on request after agreeing to their terms
//! (see [`MODEL`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod align;
pub mod cluster;
pub mod detect;
mod records;
pub mod state;
pub mod store;

#[cfg(not(target_arch = "wasm32"))]
pub mod recog;

pub use state::{Person, Settings, State};
pub use store::{FaceMeta, FaceRef, FaceStore};

/// Length of a face embedding.
pub const DIM: usize = 512;

/// 64-bit key of a photo's content key (SipHash-1-3 with fixed keys: the same on every run and
/// machine). Photos with the same content (virtual copies) share it, and so share their faces.
pub fn key_of(content_key: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = siphasher::sip::SipHasher13::new_with_keys(0x4c43_5045_4f50_4c45, 0);
    h.write(content_key.as_bytes());
    h.finish()
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("face recognition model: {0}")]
    Model(String),
    #[error("face detection: {0}")]
    Detect(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A recognition model and where it comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Short id, also the tag of the index it makes (a different model has a different index).
    pub id: &'static str,
    pub name: &'static str,
    pub authors: &'static str,
    /// The weights' licence as stated by their authors.
    pub licence: &'static str,
    pub licence_url: &'static str,
    pub homepage: &'static str,
    /// Folder URL the file is downloaded from (`<mirror>/<file_name>`), pinned to a commit.
    pub mirror: &'static str,
    /// Name of the file, in the mirror and in the model's folder (`<people folder>/<id>/<file_name>`).
    pub file_name: &'static str,
    pub size: u64,
    /// SHA-256 of the file (lower-case hex).
    pub sha256: &'static str,
}

/// The default model: ArcFace ResNet-50 trained on WebFace600K (`w600k_r50`, the `buffalo_l` pack).
pub const MODEL: ModelInfo = ModelInfo {
    id: "arcface-w600k-r50",
    name: "InsightFace ArcFace ResNet-50 (buffalo_l, w600k_r50)",
    authors: "InsightFace (Jia Guo, Jiankang Deng et al.)",
    licence: "InsightFace models: non-commercial research purposes only",
    licence_url: "https://github.com/deepinsight/insightface/tree/master/python-package#license",
    homepage: "https://github.com/deepinsight/insightface",
    mirror: "https://huggingface.co/public-data/insightface/resolve/33c1063c49c785b7652d3fd529f86fa4f149392b/models/buffalo_l",
    file_name: "w600k_r50.onnx",
    size: 174_383_860,
    sha256: "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43",
};

/// The light model: ArcFace MobileFaceNet trained on WebFace600K (`w600k_mbf`, the `buffalo_s` pack).
pub const FAST_MODEL: ModelInfo = ModelInfo {
    id: "arcface-w600k-mbf",
    name: "InsightFace ArcFace MobileFaceNet (buffalo_s, w600k_mbf)",
    authors: "InsightFace (Jia Guo, Jiankang Deng et al.)",
    licence: "InsightFace models: non-commercial research purposes only",
    licence_url: "https://github.com/deepinsight/insightface/tree/master/python-package#license",
    homepage: "https://github.com/deepinsight/insightface",
    mirror: "https://huggingface.co/immich-app/buffalo_s/resolve/0ff1751885575e62e084dff70549ce24a11fa5dc/recognition",
    file_name: "model.onnx",
    size: 13_616_099,
    sha256: "9cc6e4a75f0e2bf0b1aed94578f144d15175f357bdc05e815e5c4a02b319eb4f",
};
