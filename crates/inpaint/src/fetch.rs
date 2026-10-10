//! Downloading the MI-GAN model, only when the user asked for it (see `lightcraft-fetch`).

pub use lightcraft_fetch::*;

use std::path::Path;

/// Size of `migan.onnx` (29.5 MB).
pub const MODEL_SIZE: u64 = 29_546_882;
/// SHA-256 of `migan.onnx` from `andraniksargsyan/migan` (the ONNX export of MI-GAN's author).
pub const MODEL_SHA256: &str = "593eba0b7e04730f1b61c0a3cbca68d97d8d6a7ff5c6a44a7b9d7fcd880fc5ae";

pub const FILES: &[FileSpec] = &[FileSpec { name: crate::MODEL_FILE, size: Some(MODEL_SIZE), sha256: Some(MODEL_SHA256), max: MODEL_SIZE }];

/// Where the model comes from by default (public, no token; the SHA-256 is checked before use).
pub const DEFAULT_MIRRORS: &[&str] = &["https://huggingface.co/andraniksargsyan/migan/resolve/main"];

/// Environment variable with extra mirrors (base URLs separated by commas or spaces), tried first.
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_INPAINT_MIRRORS";

pub const LICENSE_NAME: &str = "MIT License (Picsart AI Research)";
pub const LICENSE_URL: &str = "https://github.com/Picsart-AI-Research/MI-GAN/blob/main/LICENSE-WEIGHTS";

pub fn mirrors(env: Option<&str>, file: Option<&Path>) -> Vec<String> {
    lightcraft_fetch::mirrors(env, file, DEFAULT_MIRRORS)
}
