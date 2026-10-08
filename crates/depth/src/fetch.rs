//! Downloading the Depth Anything V2 Small weights, only when the user asked for it. The
//! downloader (mirrors, resuming, checks, pure-Rust HTTPS) is the `lightcraft-fetch` crate; this
//! module says what the model needs and where it may come from.

pub use lightcraft_fetch::*;

use std::path::Path;

/// Size of the official `model.safetensors` (99.2 MB).
pub const WEIGHTS_SIZE: u64 = 99_173_660;
/// SHA-256 of the official `depth-anything/Depth-Anything-V2-Small-hf` `model.safetensors`.
pub const WEIGHTS_SHA256: &str = "3152477ce0d8d6978d76b995120de97cb5b928701fd0f817769f59e249a16b70";

pub const FILES: &[FileSpec] = &[FileSpec { name: crate::WEIGHTS_FILE, size: Some(WEIGHTS_SIZE), sha256: Some(WEIGHTS_SHA256), max: WEIGHTS_SIZE }];

/// Where the model comes from by default: the official repository on Hugging Face (public, no
/// token; the SHA-256 above is checked before the file is used).
pub const DEFAULT_MIRRORS: &[&str] = &["https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf/resolve/main"];

/// Environment variable with extra mirrors (base URLs separated by commas or spaces), tried first.
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_DEPTH_MIRRORS";

/// The model's licence (shown before downloading): Apache-2.0 for the Small model.
pub const LICENSE_NAME: &str = "Apache License 2.0";
pub const LICENSE_URL: &str = "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf";

pub fn mirrors(env: Option<&str>, file: Option<&Path>) -> Vec<String> {
    lightcraft_fetch::mirrors(env, file, DEFAULT_MIRRORS)
}
