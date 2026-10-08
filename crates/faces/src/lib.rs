//! Face analysis for LightCraft: find the faces of a photo and score how open and how sharp each
//! eye is (assisted culling: "eyes closed", the best shot of a burst).
//!
//! The model is Google's MediaPipe **Face Landmarker** bundle (`face_landmarker.task`, Apache-2.0):
//! the BlazeFace short-range detector and Face Mesh V2 (478 points). It runs on a small
//! TensorFlow Lite interpreter written in plain Rust ([`tflite`], on the [`nn`] kernels; no ML
//! framework, no C, no unsafe). The interpreter, kernels and the MediaPipe pipeline are adapted
//! from EffectCraft's `effectcraft-segment` (same authors, MIT OR Apache-2.0), where the
//! interpreter's outputs were checked against TensorFlow Lite's own.
//!
//! **The weights are never part of LightCraft**: the app downloads them on request, checked
//! against [`MODEL`]'s size and SHA-256 (see `docs/faces.md`).
//!
//! - [`mediapipe`]: detection (several window sizes, so small faces in groups are found) and the
//!   478-point mesh.
//! - [`eyes`]: eye openness (eye aspect ratio from the mesh's eyelid points) and eye sharpness
//!   (variance of the Laplacian on a normalized crop of each eye).
//! - [`analyze`]: both, for one photo.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod eyes;
pub mod mediapipe;
pub mod nn;
pub mod tflite;
pub mod zip;

pub use eyes::{EyeScore, FaceScore, analyze};
pub use mediapipe::FaceLandmarker;

pub type Result<T> = std::result::Result<T, String>;

/// The model file LightCraft uses and where it comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub name: &'static str,
    pub authors: &'static str,
    /// SPDX id of the weights' licence, as stated by the authors.
    pub licence: &'static str,
    /// The authors' statement of that licence (the model card).
    pub licence_url: &'static str,
    pub homepage: &'static str,
    /// Folder URL the file is downloaded from (`<mirror>/<file_name>`).
    pub mirror: &'static str,
    pub file_name: &'static str,
    pub size: u64,
    /// SHA-256 of the file (lower-case hex).
    pub sha256: &'static str,
}

pub const MODEL: ModelInfo = ModelInfo {
    name: "MediaPipe Face Landmarker",
    authors: "Google LLC (MediaPipe)",
    licence: "Apache-2.0",
    licence_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
    homepage: "https://developers.google.com/edge/mediapipe/solutions/vision/face_landmarker",
    mirror: "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1",
    file_name: "face_landmarker.task",
    size: 3_758_596,
    sha256: "64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff",
};
