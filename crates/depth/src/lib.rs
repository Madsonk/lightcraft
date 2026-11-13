//! Monocular depth estimation in pure Rust on candle: **Depth Anything V2 Small** (Yang et al.,
//! 2024; 24.8 M parameters, code and weights under the Apache License 2.0). The larger V2 models
//! (Base, Large, Giant) are CC-BY-NC-4.0 and are deliberately not supported.
//!
//! The model predicts *relative inverse depth* (larger = nearer, scale and shift unknown) from
//! one photo. [`DepthModel::estimate`] returns it at the model's working resolution (a multiple
//! of 14 px, short side 518 by default); `lightcraft-develop`'s `DepthMap` normalizes and
//! stores it, and the render pipeline upsamples it guided by the photo's own edges.
//!
//! On macOS the network runs on the GPU through Metal; elsewhere (for now) on the CPU. The
//! weights are not part of LightCraft: they are the official `depth-anything/Depth-Anything-V2-Small-hf`
//! checkpoint, downloaded only when the user asks ([`fetch`]).
//!
//! Ported from the Hugging Face `transformers` implementation (Apache-2.0); see `NOTICE`. This
//! crate is therefore licensed under the Apache License 2.0 only. No UI dependencies (L3).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg(not(target_arch = "wasm32"))]

pub mod fetch;
mod model;

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Image, Rgb32f};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("depth model: {0}")]
    Model(String),
    #[error("depth model file {0} not found")]
    Missing(std::path::PathBuf),
    #[error(transparent)]
    Candle(#[from] candle_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The checkpoint's file name in the model folder.
pub const WEIGHTS_FILE: &str = "model.safetensors";
/// Short side of the working resolution the model was trained around.
pub const DEFAULT_SIZE: usize = 518;
/// Most patches (tokens) one image may have: bounds time and memory on panoramas.
const MAX_PATCHES: usize = 6000;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Whether `dir` holds the checkpoint.
pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(WEIGHTS_FILE).is_file()
}

/// The best device here: Metal on macOS (when a GPU is available), else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

/// Relative inverse depth at the working resolution: larger = nearer, scale and shift unknown.
#[derive(Clone, Debug)]
pub struct RawDepth {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

pub struct DepthModel {
    net: model::Net,
    device: Device,
}

impl DepthModel {
    /// Load from a directory holding `model.safetensors`.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_on(dir, best_device())
    }

    pub fn load_on(dir: &Path, device: Device) -> Result<Self> {
        let file = dir.join(WEIGHTS_FILE);
        if !file.is_file() {
            return Err(Error::Missing(file));
        }
        let tensors = candle_core::safetensors::load(&file, &Device::Cpu)?;
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
        Ok(Self { net: model::Net::new(vb)?, device })
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The working size for a `w × h` photo with short side `size` (multiples of 14, long side
    /// bounded by the token budget).
    pub fn working_size(w: usize, h: usize, size: usize) -> (usize, usize) {
        let p = model::PATCH;
        let (w, h) = (w.max(1) as f64, h.max(1) as f64);
        let mut s = size.max(p) as f64 / w.min(h);
        let tokens = |s: f64| ((w * s / p as f64).round().max(1.0)) * ((h * s / p as f64).round().max(1.0));
        while tokens(s) > MAX_PATCHES as f64 && s > 0.01 {
            s *= 0.95;
        }
        let snap = |v: f64| ((v * s / p as f64).round().max(1.0) as usize) * p;
        (snap(w), snap(h))
    }

    /// Estimate the depth of an 8-bit RGB image (`w × h`, row-major, sRGB-encoded) whose short
    /// side is brought to `size` (see [`DEFAULT_SIZE`]).
    pub fn estimate(&self, rgb: &[u8], w: usize, h: usize, size: usize) -> Result<RawDepth> {
        if w == 0 || h == 0 || rgb.len() != w * h * 3 {
            return Err(Error::Model("bad image".into()));
        }
        let (tw, th) = Self::working_size(w, h, size);
        let src: Rgb32f = Image {
            width: w,
            height: h,
            data: rgb.as_chunks::<3>().0.iter().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect(),
        };
        let small = if (tw, th) == (w, h) { src } else { resize(&src, tw, th, Filter::Lanczos3) };
        let mut chw = vec![0f32; 3 * tw * th];
        for (i, p) in small.data.iter().enumerate() {
            for c in 0..3 {
                chw[c * tw * th + i] = (p[c].clamp(0.0, 1.0) - MEAN[c]) / STD[c];
            }
        }
        let pixels = Tensor::from_vec(chw, (1, 3, th, tw), &self.device)?;
        let data = self.net.forward(&pixels)?;
        Ok(RawDepth { width: tw, height: th, data })
    }
}

/// Wait for queued GPU work to finish (benchmarks).
pub fn sync(model: &DepthModel) -> Result<()> {
    Ok(model.device.synchronize()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The working size of the depth model: both sides are whole patches, and the patch count
    /// stays inside the token budget the model was exported with.
    #[test]
    fn working_size_snaps_to_patches_and_respects_the_budget() {
        let p = model::PATCH;
        for (w, h) in [(1000usize, 667usize), (3000, 2000), (12, 9), (1, 1)] {
            let (tw, th) = DepthModel::working_size(w, h, DEFAULT_SIZE);
            assert_eq!(tw % p, 0, "{w}×{h} -> {tw}×{th}: width not on a patch boundary");
            assert_eq!(th % p, 0, "{w}×{h} -> {tw}×{th}: height not on a patch boundary");
            assert!(tw >= p && th >= p);
            assert!((tw / p) * (th / p) <= MAX_PATCHES, "{w}×{h} -> {tw}×{th}: {} patches over the budget {}", (tw / p) * (th / p), MAX_PATCHES);
        }
    }

    #[test]
    fn working_size_keeps_the_aspect_and_grows_with_the_asked_size() {
        let small = DepthModel::working_size(3000, 2000, DEFAULT_SIZE / 2);
        let big = DepthModel::working_size(3000, 2000, DEFAULT_SIZE);
        assert!(big.0 >= small.0 && big.1 >= small.1, "small {small:?} big {big:?}");
        // 3:2 stays roughly 3:2 (within one patch of rounding on each side)
        let ratio = big.0 as f64 / big.1 as f64;
        assert!((ratio - 1.5).abs() < 0.05, "ratio {ratio}");
    }

    #[test]
    fn a_model_directory_is_recognised_by_its_weights_file() {
        let dir = std::env::temp_dir().join(format!("depth-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_model_dir(&dir), "empty directory is not a model");
        std::fs::write(dir.join(WEIGHTS_FILE), b"not really weights").unwrap();
        assert!(is_model_dir(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
