//! Automatic masks in pure Rust on candle: **SegFormer** (ADE20K, 150 classes: sky, person,
//! vegetation, water, building…) for the Sky / People / Landscape masks, and an **ISNet**
//! matting network (RMBG-1.4) for the Subject mask. Both run on the GPU through Metal when that
//! is faster (see [`pick_device`]), else on the CPU.
//!
//! The weights are never part of LightCraft: the app downloads them on request ([`files`]).
//! Images enter as interleaved 8-bit sRGB already stretched to the size the network wants; the
//! engine does the resizing and the conversion of the result into stored logits.

#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod files;
pub mod isnet;
pub mod segformer;

use candle_core::Device;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("model: {0}")]
    Model(String),
    #[error("image: {0}")]
    Image(String),
    #[error("{0}")]
    Candle(#[from] candle_core::Error),
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The device the networks run on: Metal on macOS, else the CPU; `LIGHTCRAFT_AUTOSEG_DEVICE=cpu`
/// forces the CPU. **Not the CPU by default on purpose:** candle 0.9.2's CPU `conv2d` returns wrong
/// values for some shapes (measured: a 3x3 convolution of a 32 x 32 map with 32 or 64 channels, as in
/// SegFormer's stages on a square photo and in the matting network's deeper levels, differs from
/// PyTorch's by up to 3 %; Metal agrees to 1e-6 on every shape tried), so the CPU is only for machines
/// without a GPU (see `docs/autoseg.md`).
pub fn pick_device() -> Device {
    let force_cpu = std::env::var("LIGHTCRAFT_AUTOSEG_DEVICE").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
    #[cfg(target_os = "macos")]
    if !force_cpu && let Ok(d) = Device::new_metal(0) {
        return d;
    }
    let _ = force_cpu;
    Device::Cpu
}

/// `ln(p / (1 - p))`, clamped so a certain pixel stays finite.
pub fn logit(p: f32) -> f32 {
    let p = p.clamp(1e-4, 1.0 - 1e-4);
    (p / (1.0 - p)).ln()
}
