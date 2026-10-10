//! Object removal by inpainting: **MI-GAN** (Sargsyan et al., ICCV 2023; Picsart AI Research,
//! code and weights under the MIT licence) trained on Places2 (scenes), run from the ONNX export
//! of its author.
//!
//! The network fills a 512 × 512 image where a mask says so and keeps the known pixels. For a
//! photo of any size [`Inpainter::fill`] crops a square of context around the area, runs the
//! model at 512 px and returns the fill at the crop's own resolution with a feathered alpha
//! (so the area's border blends in); the caller composites it. The model file is not part of
//! LightCraft: it is downloaded only when the user asks ([`fetch`]).
//!
//! The ONNX file is read by a small protobuf decoder ([`onnx`]) and run by a small interpreter of
//! the dozen operators it uses ([`exec`]) on candle's CPU backend: pure Rust,
//! no C, C++ or assembly anywhere (tract was tried first: its build assembles hand-written
//! kernels through the `cc` crate).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg(not(target_arch = "wasm32"))]

mod exec;
pub mod fetch;
mod onnx;

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Image, Plane, Rgb32f};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("inpainting model: {0}")]
    Model(String),
    #[error("inpainting model file {0} not found")]
    Missing(std::path::PathBuf),
    #[error("nothing selected to fill")]
    EmptyMask,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The device the model runs on: the CPU. (Metal was measured at 6–8 s for one fill against 4.4 s on the
/// CPU of the same M4: candle's Metal convolutions go through a generic im2col copy, and this
/// network is mostly full-resolution convolutions.)
pub fn best_device() -> Device {
    Device::Cpu
}

/// The ONNX file's name in the model folder.
pub const MODEL_FILE: &str = "migan.onnx";
/// The side of the model's square input.
pub const SIDE: usize = 512;

/// A loaded ONNX graph, for other models that use the same interpreter (face recognition).
pub use exec::Model as OnnxModel;

/// Read an ONNX file's bytes into a graph ready to run on `device` (the operators this crate's
/// interpreter knows: see `exec`).
pub fn load_onnx(bytes: &[u8], device: Device) -> Result<OnnxModel> {
    exec::Model::new(onnx::parse(bytes)?, device)
}

pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(MODEL_FILE).is_file()
}

pub struct Inpainter {
    model: exec::Model,
}

/// The fill of a rectangle of the photo.
#[derive(Clone, Debug)]
pub struct Fill {
    /// The rectangle of the photo this covers, in pixels.
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    /// Interleaved 8-bit RGB (sRGB-encoded like the input), `width × height × 3`.
    pub rgb: Vec<u8>,
    /// Where to use it: the selection grown a little and feathered, 255 = the fill.
    pub alpha: Vec<u8>,
}

/// Settings of one fill.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Context around the selection's box, as a multiple of its longer side (the model sees
    /// this much of the surroundings).
    pub context: f32,
    /// How far the selection grows before filling, as a fraction of the crop's side (removes
    /// the halo around a hand-drawn selection).
    pub grow: f32,
    /// Feather of the border between the fill and the photo, as a fraction of the crop's side.
    pub feather: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { context: 2.2, grow: 0.006, feather: 0.008 }
    }
}

/// The model keeps the known pixels but drifts in tone inside the hole (a smooth gradient comes
/// back lighter or darker than around it). Measure the jump between the fill's edge and the
/// photo just outside it, spread that offset smoothly over the hole (normalized convolution of
/// the edge jumps: a membrane-like interpolation), and add it to the fill.
fn match_tone(fill: &mut Rgb32f, orig: &Rgb32f, hole: &Plane) {
    let (w, h) = (fill.width, fill.height);
    let side = w.max(h) as f32;
    let edge_sigma = (0.004 * side).max(1.0);
    let inner = gaussian(hole, edge_sigma);
    let outer = gaussian(hole, edge_sigma * 2.0);
    // the hole's inner border band, and the ring of known pixels just outside it
    let band =
        Plane { width: w, height: h, data: hole.data.iter().zip(&inner.data).map(|(m, g)| if *m > 0.5 && *g < 0.9 { 1.0 } else { 0.0 }).collect() };
    let ring =
        Plane { width: w, height: h, data: hole.data.iter().zip(&outer.data).map(|(m, g)| if *m < 0.5 && *g > 0.04 { 1.0 } else { 0.0 }).collect() };
    if band.data.iter().all(|v| *v == 0.0) || ring.data.iter().all(|v| *v == 0.0) {
        return;
    }
    let sig = (0.006 * side).max(1.5);
    let ring_w = gaussian(&ring, sig);
    let ring_c = gaussian(&Rgb32f { width: w, height: h, data: orig.data.iter().zip(&ring.data).map(|(c, r)| c.map(|v| v * r)).collect() }, sig);
    // jump at each band pixel: the photo's colour just outside minus the fill's
    let jump = Rgb32f {
        width: w,
        height: h,
        data: (0..w * h)
            .map(|i| {
                if band.data[i] > 0.0 && ring_w.data[i] > 1e-4 {
                    [0, 1, 2].map(|c| ring_c.data[i][c] / ring_w.data[i] - fill.data[i][c])
                } else {
                    [0.0; 3]
                }
            })
            .collect(),
    };
    let weight =
        Plane { width: w, height: h, data: (0..w * h).map(|i| if band.data[i] > 0.0 && ring_w.data[i] > 1e-4 { 1.0 } else { 0.0 }).collect() };
    let spread = (0.12 * side).max(4.0);
    let (jb, wb) = (gaussian(&jump, spread), gaussian(&weight, spread));
    for i in 0..w * h {
        if hole.data[i] > 0.0 && wb.data[i] > 1e-5 {
            for c in 0..3 {
                fill.data[i][c] += jb.data[i][c] / wb.data[i];
            }
        }
    }
}

impl Inpainter {
    /// Load `migan.onnx` from `dir` (on the CPU, see [`best_device`]).
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_on(dir, best_device())
    }

    pub fn load_on(dir: &Path, device: Device) -> Result<Self> {
        let file = dir.join(MODEL_FILE);
        if !file.is_file() {
            return Err(Error::Missing(file));
        }
        let bytes = std::fs::read(&file).map_err(|e| Error::Model(format!("{}: {e}", file.display())))?;
        let model = exec::Model::new(onnx::parse(&bytes)?, device)?;
        if model.inputs.len() != 2 {
            return Err(Error::Model("the model should take an image and a mask".into()));
        }
        Ok(Self { model })
    }

    pub fn device(&self) -> &Device {
        self.model.device()
    }

    /// The model on one square `SIDE × SIDE` image: `rgb` interleaved, `known` 255 where the
    /// pixel is kept and 0 where it is invented. Known pixels come back unchanged.
    pub fn run_square(&self, rgb: &[u8], known: &[u8]) -> Result<Vec<u8>> {
        let n = SIDE * SIDE;
        if rgb.len() != 3 * n || known.len() != n {
            return Err(Error::Model("the model takes 512 × 512 images".into()));
        }
        let mut chw = vec![0f32; 3 * n];
        for (i, p) in rgb.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                chw[c * n + i] = f32::from(p[c]);
            }
        }
        let dev = self.model.device();
        let image = Tensor::from_vec(chw, (1, 3, SIDE, SIDE), dev)?;
        let mask = Tensor::from_vec(known.iter().map(|v| f32::from(*v)).collect::<Vec<f32>>(), (1, 1, SIDE, SIDE), dev)?;
        let (i0, i1) = (self.model.inputs[0].as_str(), self.model.inputs[1].as_str());
        let out = self.model.run(&[(i0, image), (i1, mask)])?.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?;
        if out.len() != 3 * n {
            return Err(Error::Model(format!("unexpected output of {} values", out.len())));
        }
        let mut res = vec![0u8; 3 * n];
        for i in 0..n {
            for c in 0..3 {
                res[i * 3 + c] = out[c * n + i].round().clamp(0.0, 255.0) as u8;
            }
        }
        Ok(res)
    }

    /// Fill the area `mask` (255 = remove, row-major `w × h`) of the 8-bit RGB image `rgb`.
    pub fn fill(&self, rgb: &[u8], w: usize, h: usize, mask: &[u8], opts: &Options) -> Result<Fill> {
        if w == 0 || h == 0 || rgb.len() != w * h * 3 || mask.len() != w * h {
            return Err(Error::Model("bad image or mask".into()));
        }
        // the selection's box
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        for (i, m) in mask.iter().enumerate() {
            if *m > 127 {
                let (x, y) = (i % w, i / w);
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return Err(Error::EmptyMask);
        }
        // a crop with context around it, inside the photo
        let side = (((x1 - x0).max(y1 - y0) as f32 * opts.context.max(1.2)) as usize).max(96);
        let (cw, ch) = (side.min(w), side.min(h));
        let place = |lo: usize, hi: usize, len: usize, full: usize| (((lo + hi) / 2).saturating_sub(len / 2)).min(full - len);
        let (cx, cy) = (place(x0, x1, cw, w), place(y0, y1, ch, h));
        // crop, stretched to the model's square
        let crop: Rgb32f = Image {
            width: cw,
            height: ch,
            data: (0..cw * ch)
                .map(|i| {
                    let o = ((cy + i / cw) * w + cx + i % cw) * 3;
                    [rgb[o] as f32, rgb[o + 1] as f32, rgb[o + 2] as f32]
                })
                .collect(),
        };
        let hole = Plane {
            width: cw,
            height: ch,
            data: (0..cw * ch).map(|i| if mask[(cy + i / cw) * w + cx + i % cw] > 127 { 1.0 } else { 0.0 }).collect(),
        };
        // grow the selection (a max filter by blurring and thresholding) so no halo of the
        // removed object stays, and feather the result
        let grow_px = (opts.grow * cw.max(ch) as f32).max(1.0);
        let grown = gaussian(&hole, grow_px).map(|v| if v > 0.12 { 1.0 } else { 0.0 });
        let sq = |img: &Rgb32f, f: Filter| if (img.width, img.height) == (SIDE, SIDE) { img.clone() } else { resize(img, SIDE, SIDE, f) };
        let small = sq(&crop, Filter::Lanczos3);
        let small_hole = resize(&grown, SIDE, SIDE, Filter::Bilinear);
        let known: Vec<u8> = small_hole.data.iter().map(|v| if *v > 0.05 { 0 } else { 255 }).collect();
        let small_rgb: Vec<u8> = small.data.iter().flat_map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8)).collect();
        let out = self.run_square(&small_rgb, &known)?;
        let out_img: Rgb32f = Image {
            width: SIDE,
            height: SIDE,
            data: out.as_chunks::<3>().0.iter().map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]).collect(),
        };
        let mut back = if (cw, ch) == (SIDE, SIDE) { out_img } else { resize(&out_img, cw, ch, Filter::Lanczos3) };
        match_tone(&mut back, &crop, &grown);
        let feather = (opts.feather * cw.max(ch) as f32).max(0.8);
        let alpha = gaussian(&grown, feather);
        Ok(Fill {
            x: cx,
            y: cy,
            width: cw,
            height: ch,
            rgb: back.data.iter().flat_map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8)).collect(),
            alpha: alpha.data.iter().map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8).collect(),
        })
    }
}
