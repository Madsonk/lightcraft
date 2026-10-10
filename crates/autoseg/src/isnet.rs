//! Subject matting with an **ISNet** (the U²-Net family: nested U-shaped residual blocks,
//! "U²-Net: Going Deeper with Nested U-Structure for Salient Object Detection", Qin et al.
//! 2020; "Highly Accurate Dichotomous Image Segmentation", Qin et al. 2022), re-implemented on
//! candle from those papers and the layout of the RMBG-1.4 checkpoint (BRIA AI).
//!
//! Batch-norm layers are folded into the convolutions at load time.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Tensor};

use crate::{Error, Result};

type Weights = HashMap<String, Tensor>;

/// The network's side (the input is stretched to `SIDE x SIDE`).
pub const SIDE: usize = 1024;

struct RebnConv {
    w: Tensor,
    b: Tensor,
    dilation: usize,
}

impl RebnConv {
    fn load(w: &Weights, name: &str, inp: usize, out: usize, dilation: usize) -> Result<Self> {
        let g = |k: &str, shape: &[usize]| -> Result<Tensor> {
            let t = w.get(k).ok_or_else(|| Error::Model(format!("missing weight {k}")))?;
            if t.dims() != shape {
                return Err(Error::Model(format!("{k}: shape {:?}, expected {shape:?}", t.dims())));
            }
            Ok(t.clone())
        };
        let cw = g(&format!("{name}.conv_s1.weight"), &[out, inp, 3, 3])?;
        let cb = g(&format!("{name}.conv_s1.bias"), &[out])?;
        let gamma = g(&format!("{name}.bn_s1.weight"), &[out])?;
        let beta = g(&format!("{name}.bn_s1.bias"), &[out])?;
        let mean = g(&format!("{name}.bn_s1.running_mean"), &[out])?;
        let var = g(&format!("{name}.bn_s1.running_var"), &[out])?;
        let scale = (gamma / (var + 1e-5)?.sqrt()?)?;
        let fw = cw.broadcast_mul(&scale.reshape((out, 1, 1, 1))?)?.contiguous()?;
        let fb = (((cb - mean)? * &scale)? + beta)?;
        Ok(Self { w: fw, b: fb, dilation })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = x.conv2d(&self.w, self.dilation, 1, self.dilation, 1)?;
        Ok(y.broadcast_add(&self.b.reshape((1, (), 1, 1))?)?.relu()?)
    }
}

/// 2 x 2 max pooling with `ceil_mode`: an odd side keeps its last row / column (replicated).
fn pool(x: &Tensor) -> Result<Tensor> {
    let (_, _, h, w) = x.dims4()?;
    let mut x = x.clone();
    if h % 2 == 1 {
        x = x.pad_with_same(2, 0, 1)?;
    }
    if w % 2 == 1 {
        x = x.pad_with_same(3, 0, 1)?;
    }
    Ok(x.max_pool2d_with_stride(2, 2)?)
}

fn up_like(src: &Tensor, tar: &Tensor) -> Result<Tensor> {
    let (_, _, h, w) = tar.dims4()?;
    let (_, _, sh, sw) = src.dims4()?;
    if (h, w) == (sh, sw) { Ok(src.clone()) } else { Ok(src.upsample_bilinear2d(h, w, false)?) }
}

/// A residual U-block: `levels` encoder convolutions with pooling (`pooled` of them), a dilated
/// bottom, and a mirrored decoder. `dilated` blocks (RSU-4F) replace pooling with dilation.
fn dump(name: &str, t: &Tensor) {
    if let Some(dir) = std::env::var_os("LIGHTCRAFT_AUTOSEG_DUMP2") {
        let v = t.flatten_all().and_then(|t| t.to_vec1::<f32>()).unwrap_or_default();
        let bytes: Vec<u8> = v.iter().flat_map(|a| a.to_le_bytes()).collect();
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{name}.f32")), bytes);
    }
}

struct Rsu {
    name: String,
    rin: RebnConv,
    enc: Vec<RebnConv>,
    bottom: RebnConv,
    dec: Vec<RebnConv>,
    dilated: bool,
}

impl Rsu {
    /// `n` = 7, 6, 5, 4 for RSU-n; `dilated` for RSU-4F.
    fn load(w: &Weights, p: &str, n: usize, dilated: bool, inp: usize, mid: usize, out: usize) -> Result<Self> {
        let rin = RebnConv::load(w, &format!("{p}.rebnconvin"), inp, out, 1)?;
        let (enc, bottom, dec) = if dilated {
            let enc = vec![
                RebnConv::load(w, &format!("{p}.rebnconv1"), out, mid, 1)?,
                RebnConv::load(w, &format!("{p}.rebnconv2"), mid, mid, 2)?,
                RebnConv::load(w, &format!("{p}.rebnconv3"), mid, mid, 4)?,
            ];
            let bottom = RebnConv::load(w, &format!("{p}.rebnconv4"), mid, mid, 8)?;
            let dec = vec![
                RebnConv::load(w, &format!("{p}.rebnconv3d"), mid * 2, mid, 4)?,
                RebnConv::load(w, &format!("{p}.rebnconv2d"), mid * 2, mid, 2)?,
                RebnConv::load(w, &format!("{p}.rebnconv1d"), mid * 2, out, 1)?,
            ];
            (enc, bottom, dec)
        } else {
            // convs 1..n-1 with pooling after each but the last two; conv n dilated by 2
            let mut enc = vec![RebnConv::load(w, &format!("{p}.rebnconv1"), out, mid, 1)?];
            for i in 2..n {
                enc.push(RebnConv::load(w, &format!("{p}.rebnconv{i}"), mid, mid, 1)?);
            }
            let bottom = RebnConv::load(w, &format!("{p}.rebnconv{n}"), mid, mid, 2)?;
            let mut dec = Vec::new();
            for i in (1..n).rev() {
                let o = if i == 1 { out } else { mid };
                dec.push(RebnConv::load(w, &format!("{p}.rebnconv{i}d"), mid * 2, o, 1)?);
            }
            (enc, bottom, dec)
        };
        Ok(Self { name: p.to_string(), rin, enc, bottom, dec, dilated })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let hxin = self.rin.forward(x)?;
        dump(&format!("{}_in", self.name), &hxin);
        if self.dilated {
            let h1 = self.enc[0].forward(&hxin)?;
            let h2 = self.enc[1].forward(&h1)?;
            let h3 = self.enc[2].forward(&h2)?;
            let h4 = self.bottom.forward(&h3)?;
            let d3 = self.dec[0].forward(&Tensor::cat(&[&h4, &h3], 1)?)?;
            let d2 = self.dec[1].forward(&Tensor::cat(&[&d3, &h2], 1)?)?;
            let d1 = self.dec[2].forward(&Tensor::cat(&[&d2, &h1], 1)?)?;
            return Ok((d1 + hxin)?);
        }
        // encoder: conv_i, then pool, except after the last encoder conv
        let mut skips: Vec<Tensor> = Vec::new();
        let mut cur = hxin.clone();
        for (i, c) in self.enc.iter().enumerate() {
            let h = c.forward(&cur)?;
            dump(&format!("{}_h{}", self.name, i + 1), &h);
            skips.push(h.clone());
            // the last encoder conv feeds the bottom without pooling only for RSU-n's final level
            cur = if i + 1 < self.enc.len() { pool(&h)? } else { h };
        }
        let mut d = self.bottom.forward(&cur)?;
        dump(&format!("{}_bottom", self.name), &d);
        // decoder: dec[0] merges bottom with the last skip, then upsample to the previous skip
        let n = self.enc.len();
        for (k, dc) in self.dec.iter().enumerate() {
            let skip = &skips[n - 1 - k];
            let m = dc.forward(&Tensor::cat(&[&d, skip], 1)?)?;
            dump(&format!("{}_d{}", self.name, k), &m);
            d = if n - 1 - k > 0 { up_like(&m, &skips[n - 2 - k])? } else { m };
        }
        Ok((d + hxin)?)
    }
}

pub struct IsNet {
    conv_in_w: Tensor,
    conv_in_b: Tensor,
    stages: Vec<Rsu>,
    dec: Vec<Rsu>,
    side1: (Tensor, Tensor),
    dev: Device,
}

impl IsNet {
    /// Load `model.safetensors` from `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let file = dir.join("model.safetensors");
        let dev = crate::pick_device();
        let bytes = std::fs::read(&file).map_err(|e| Error::Io(file.display().to_string(), e))?;
        let raw = candle_core::safetensors::load_buffer(&bytes, &dev).map_err(|e| Error::Model(format!("{}: {e}", file.display())))?;
        let mut w = Weights::new();
        for (k, t) in raw {
            if !k.ends_with("num_batches_tracked") {
                w.insert(k, t.to_dtype(DType::F32)?);
            }
        }
        let g = |k: &str| w.get(k).cloned().ok_or_else(|| Error::Model(format!("missing weight {k}")));
        let stages = vec![
            Rsu::load(&w, "stage1", 7, false, 64, 32, 64)?,
            Rsu::load(&w, "stage2", 6, false, 64, 32, 128)?,
            Rsu::load(&w, "stage3", 5, false, 128, 64, 256)?,
            Rsu::load(&w, "stage4", 4, false, 256, 128, 512)?,
            Rsu::load(&w, "stage5", 4, true, 512, 256, 512)?,
            Rsu::load(&w, "stage6", 4, true, 512, 256, 512)?,
        ];
        let dec = vec![
            Rsu::load(&w, "stage5d", 4, true, 1024, 256, 512)?,
            Rsu::load(&w, "stage4d", 4, false, 1024, 128, 256)?,
            Rsu::load(&w, "stage3d", 5, false, 512, 64, 128)?,
            Rsu::load(&w, "stage2d", 6, false, 256, 32, 64)?,
            Rsu::load(&w, "stage1d", 7, false, 128, 16, 64)?,
        ];
        Ok(Self { conv_in_w: g("conv_in.weight")?, conv_in_b: g("conv_in.bias")?, stages, dec, side1: (g("side1.weight")?, g("side1.bias")?), dev })
    }

    pub fn device(&self) -> &Device {
        &self.dev
    }

    /// The foreground probability (0..1, `SIDE x SIDE`) of an RGB 8-bit image already stretched
    /// to `SIDE x SIDE`.
    pub fn matte(&self, rgb: &[u8]) -> Result<Vec<f32>> {
        if rgb.len() != SIDE * SIDE * 3 {
            return Err(Error::Image(format!("expected {SIDE} x {SIDE} pixels, got {} samples", rgb.len())));
        }
        let mut planar = vec![0f32; 3 * SIDE * SIDE];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                planar[c * SIDE * SIDE + i] = f32::from(px[c]) / 255.0 - 0.5;
            }
        }
        let x = Tensor::from_vec(planar, (1, 3, SIDE, SIDE), &self.dev)?;
        let hxin = x.conv2d(&self.conv_in_w, 1, 2, 1, 1)?.broadcast_add(&self.conv_in_b.reshape((1, (), 1, 1))?)?;
        let dbg = std::env::var_os("LIGHTCRAFT_AUTOSEG_DEBUG").is_some();
        let stat = |n: &str, t: &Tensor| {
            if let Some(dir) = std::env::var_os("LIGHTCRAFT_AUTOSEG_DUMP") {
                let v = t.flatten_all().and_then(|t| t.to_vec1::<f32>()).unwrap_or_default();
                let bytes: Vec<u8> = v.iter().flat_map(|a| a.to_le_bytes()).collect();
                let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{n}.f32")), bytes);
            }
            if dbg {
                let v = t.flatten_all().and_then(|t| t.to_vec1::<f32>()).unwrap_or_default();
                let m = (v.iter().map(|a| f64::from(*a)).sum::<f64>() / v.len().max(1) as f64) as f32;
                let sd = (v.iter().map(|a| f64::from(a - m).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt();
                eprintln!("{n}: {:?} mean {m:.4} std {sd:.4}", t.dims());
            }
        };
        stat("hxin", &hxin);
        let hx1 = self.stages[0].forward(&hxin)?;
        stat("hx1", &hx1);
        let hx2 = self.stages[1].forward(&pool(&hx1)?)?;
        stat("hx2", &hx2);
        let hx3 = self.stages[2].forward(&pool(&hx2)?)?;
        stat("hx3", &hx3);
        let hx4 = self.stages[3].forward(&pool(&hx3)?)?;
        stat("hx4", &hx4);
        let hx5 = self.stages[4].forward(&pool(&hx4)?)?;
        stat("hx5", &hx5);
        let hx6 = self.stages[5].forward(&pool(&hx5)?)?;
        stat("hx6", &hx6);
        let hx5d = self.dec[0].forward(&Tensor::cat(&[&up_like(&hx6, &hx5)?, &hx5], 1)?)?;
        stat("hx5d", &hx5d);
        let hx4d = self.dec[1].forward(&Tensor::cat(&[&up_like(&hx5d, &hx4)?, &hx4], 1)?)?;
        stat("hx4d", &hx4d);
        let hx3d = self.dec[2].forward(&Tensor::cat(&[&up_like(&hx4d, &hx3)?, &hx3], 1)?)?;
        stat("hx3d", &hx3d);
        let hx2d = self.dec[3].forward(&Tensor::cat(&[&up_like(&hx3d, &hx2)?, &hx2], 1)?)?;
        stat("hx2d", &hx2d);
        let hx1d = self.dec[4].forward(&Tensor::cat(&[&up_like(&hx2d, &hx1)?, &hx1], 1)?)?;
        stat("hx1d", &hx1d);
        let d1 = hx1d.conv2d(&self.side1.0, 1, 1, 1, 1)?.broadcast_add(&self.side1.1.reshape((1, (), 1, 1))?)?;
        let d1 = d1.upsample_bilinear2d(SIDE, SIDE, false)?;
        let p = candle_nn::ops::sigmoid(&d1)?.flatten_all()?.to_vec1::<f32>()?;
        // (min-max normalized, as the model's authors do)
        let (lo, hi) = p.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let span = (hi - lo).max(1e-6);
        Ok(p.into_iter().map(|v| (v - lo) / span).collect())
    }
}
