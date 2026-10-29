//! SegFormer (Xie et al. 2021, "SegFormer: Simple and Efficient Design for Semantic Segmentation
//! with Transformers") re-implemented on candle from the paper and the checkpoint's layout:
//! a four-stage Mix Transformer encoder (overlapping patch embeddings, spatial-reduction
//! attention, Mix-FFN with a depthwise convolution) and an all-MLP decoder.
//!
//! The checkpoint is NVIDIA's `segformer-b2-finetuned-ade-512-512` (150 ADE20K classes).

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::ops;

use crate::{Error, Result};

const DEPTHS: [usize; 4] = [3, 4, 6, 3];
const DIMS: [usize; 4] = [64, 128, 320, 512];
const HEADS: [usize; 4] = [1, 2, 5, 8];
const SR: [usize; 4] = [8, 4, 2, 1];
const PATCH: [usize; 4] = [7, 3, 3, 3];
const STRIDE: [usize; 4] = [4, 2, 2, 2];
const DECODER: usize = 768;
pub const CLASSES: usize = 150;
const LN_EPS: f32 = 1e-6;

/// ADE20K class indices the masks use.
pub mod class {
    pub const SKY: usize = 2;
    pub const PERSON: usize = 12;
    pub const TREE: usize = 4;
    pub const GRASS: usize = 9;
    pub const WATER: usize = 21;
    pub const SEA: usize = 26;
    pub const MOUNTAIN: usize = 16;
    pub const BUILDING: usize = 1;
    pub const PLANT: usize = 17;
    pub const FIELD: usize = 29;
    pub const SAND: usize = 46;
    pub const RIVER: usize = 60;
    pub const HILL: usize = 68;
    pub const LAKE: usize = 128;
    pub const ROCK: usize = 34;
}

/// A named group of ADE20K classes, for the Landscape mask.
pub fn landscape_classes(name: &str) -> Option<&'static [usize]> {
    use class::{BUILDING, FIELD, GRASS, HILL, LAKE, MOUNTAIN, PERSON, PLANT, RIVER, ROCK, SAND, SEA, SKY, TREE, WATER};
    Some(match name {
        "sky" => &[SKY],
        "people" | "person" => &[PERSON],
        "vegetation" => &[TREE, GRASS, PLANT, FIELD],
        "water" => &[WATER, SEA, RIVER, LAKE],
        "mountain" => &[MOUNTAIN, HILL, ROCK],
        "buildings" | "building" => &[BUILDING],
        "ground" => &[SAND, FIELD, GRASS],
        _ => return None,
    })
}

type Weights = HashMap<String, Tensor>;

fn get(w: &Weights, name: &str, shape: &[usize]) -> Result<Tensor> {
    let t = w.get(name).ok_or_else(|| Error::Model(format!("missing weight {name}")))?;
    if t.dims() != shape {
        return Err(Error::Model(format!("{name}: shape {:?}, expected {shape:?}", t.dims())));
    }
    Ok(t.clone())
}

struct Linear {
    w: Tensor,
    b: Tensor,
}

impl Linear {
    fn load(w: &Weights, name: &str, inp: usize, out: usize) -> Result<Self> {
        Ok(Self { w: get(w, &format!("{name}.weight"), &[out, inp])?.t()?.contiguous()?, b: get(w, &format!("{name}.bias"), &[out])? })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(x.broadcast_matmul(&self.w)?.broadcast_add(&self.b)?)
    }
}

struct Norm {
    w: Tensor,
    b: Tensor,
}

impl Norm {
    fn load(w: &Weights, name: &str, c: usize) -> Result<Self> {
        Ok(Self { w: get(w, &format!("{name}.weight"), &[c])?, b: get(w, &format!("{name}.bias"), &[c])? })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(ops::layer_norm(&x.contiguous()?, &self.w, &self.b, LN_EPS)?)
    }
}

struct Conv {
    w: Tensor,
    b: Option<Tensor>,
    pad: usize,
    stride: usize,
    groups: usize,
}

impl Conv {
    #[allow(clippy::too_many_arguments)]
    fn load(w: &Weights, name: &str, inp: usize, out: usize, k: usize, stride: usize, pad: usize, groups: usize, bias: bool) -> Result<Self> {
        Ok(Self {
            w: get(w, &format!("{name}.weight"), &[out, inp / groups, k, k])?,
            b: if bias { Some(get(w, &format!("{name}.bias"), &[out])?) } else { None },
            pad,
            stride,
            groups,
        })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = x.conv2d(&self.w, self.pad, self.stride, 1, self.groups)?;
        Ok(match &self.b {
            Some(b) => y.broadcast_add(&b.reshape((1, (), 1, 1))?)?,
            None => y,
        })
    }
}

struct Block {
    ln1: Norm,
    q: Linear,
    k: Linear,
    v: Linear,
    sr: Option<(Conv, Norm)>,
    out: Linear,
    ln2: Norm,
    fc1: Linear,
    dw: Conv,
    fc2: Linear,
    heads: usize,
}

impl Block {
    fn load(w: &Weights, stage: usize, i: usize) -> Result<Self> {
        let c = DIMS[stage];
        let p = format!("segformer.encoder.block.{stage}.{i}");
        let sr = if SR[stage] > 1 {
            Some((
                Conv::load(w, &format!("{p}.attention.self.sr"), c, c, SR[stage], SR[stage], 0, 1, true)?,
                Norm::load(w, &format!("{p}.attention.self.layer_norm"), c)?,
            ))
        } else {
            None
        };
        Ok(Self {
            ln1: Norm::load(w, &format!("{p}.layer_norm_1"), c)?,
            q: Linear::load(w, &format!("{p}.attention.self.query"), c, c)?,
            k: Linear::load(w, &format!("{p}.attention.self.key"), c, c)?,
            v: Linear::load(w, &format!("{p}.attention.self.value"), c, c)?,
            sr,
            out: Linear::load(w, &format!("{p}.attention.output.dense"), c, c)?,
            ln2: Norm::load(w, &format!("{p}.layer_norm_2"), c)?,
            fc1: Linear::load(w, &format!("{p}.mlp.dense1"), c, 4 * c)?,
            dw: Conv::load(w, &format!("{p}.mlp.dwconv.dwconv"), 4 * c, 4 * c, 3, 1, 1, 4 * c, true)?,
            fc2: Linear::load(w, &format!("{p}.mlp.dense2"), 4 * c, c)?,
            heads: HEADS[stage],
        })
    }

    /// `x`: `[b, h * w, c]`.
    fn forward(&self, x: &Tensor, h: usize, w: usize) -> Result<Tensor> {
        let (b, n, c) = x.dims3()?;
        let d = c / self.heads;
        let y = self.ln1.forward(x)?;
        let q = self.q.forward(&y)?.reshape((b, n, self.heads, d))?.transpose(1, 2)?.contiguous()?;
        let kv = match &self.sr {
            Some((conv, ln)) => {
                let img = y.transpose(1, 2)?.contiguous()?.reshape((b, c, h, w))?;
                let r = conv.forward(&img)?;
                ln.forward(&r.flatten_from(2)?.transpose(1, 2)?.contiguous()?)?
            }
            None => y,
        };
        let m = kv.dim(1)?;
        let k = self.k.forward(&kv)?.reshape((b, m, self.heads, d))?.transpose(1, 2)?.contiguous()?;
        let v = self.v.forward(&kv)?.reshape((b, m, self.heads, d))?.transpose(1, 2)?.contiguous()?;
        let att = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * (1.0 / (d as f64).sqrt()))?;
        let att = ops::softmax_last_dim(&att)?;
        let o = att.matmul(&v)?.transpose(1, 2)?.contiguous()?.reshape((b, n, c))?;
        let x = (x + self.out.forward(&o)?)?;
        let y = self.ln2.forward(&x)?;
        let y = self.fc1.forward(&y)?;
        let hid = y.dim(2)?;
        let img = y.transpose(1, 2)?.contiguous()?.reshape((b, hid, h, w))?;
        let y = self.dw.forward(&img)?.flatten_from(2)?.transpose(1, 2)?.contiguous()?.gelu_erf()?;
        Ok((x + self.fc2.forward(&y)?)?)
    }
}

struct Stage {
    embed: Conv,
    embed_ln: Norm,
    blocks: Vec<Block>,
    ln: Norm,
}

struct Bn {
    scale: Tensor,
    shift: Tensor,
}

pub struct SegFormer {
    stages: Vec<Stage>,
    linear_c: Vec<Linear>,
    fuse: Conv,
    bn: Bn,
    classifier: Conv,
    dev: Device,
}

impl SegFormer {
    /// Load `pytorch_model.bin` from `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let file = dir.join("pytorch_model.bin");
        let dev = crate::pick_device();
        let tensors = candle_core::pickle::read_all(&file).map_err(|e| Error::Model(format!("{}: {e}", file.display())))?;
        let mut w = Weights::new();
        for (k, t) in tensors {
            w.insert(k, t.to_dtype(DType::F32)?.to_device(&dev)?);
        }
        let mut stages = Vec::new();
        for s in 0..4 {
            let prev = if s == 0 { 3 } else { DIMS[s - 1] };
            let p = format!("segformer.encoder.patch_embeddings.{s}");
            stages.push(Stage {
                embed: Conv::load(&w, &format!("{p}.proj"), prev, DIMS[s], PATCH[s], STRIDE[s], PATCH[s] / 2, 1, true)?,
                embed_ln: Norm::load(&w, &format!("{p}.layer_norm"), DIMS[s])?,
                blocks: (0..DEPTHS[s]).map(|i| Block::load(&w, s, i)).collect::<Result<_>>()?,
                ln: Norm::load(&w, &format!("segformer.encoder.layer_norm.{s}"), DIMS[s])?,
            });
        }
        let linear_c = (0..4).map(|i| Linear::load(&w, &format!("decode_head.linear_c.{i}.proj"), DIMS[i], DECODER)).collect::<Result<_>>()?;
        let fuse = Conv::load(&w, "decode_head.linear_fuse", 4 * DECODER, DECODER, 1, 1, 0, 1, false)?;
        let g = get(&w, "decode_head.batch_norm.weight", &[DECODER])?;
        let be = get(&w, "decode_head.batch_norm.bias", &[DECODER])?;
        let mean = get(&w, "decode_head.batch_norm.running_mean", &[DECODER])?;
        let var = get(&w, "decode_head.batch_norm.running_var", &[DECODER])?;
        let scale = (g / (var + 1e-5)?.sqrt()?)?;
        let shift = (be - (mean * &scale)?)?;
        let bn = Bn { scale: scale.reshape((1, (), 1, 1))?, shift: shift.reshape((1, (), 1, 1))? };
        let classifier = Conv::load(&w, "decode_head.classifier", DECODER, CLASSES, 1, 1, 0, 1, true)?;
        Ok(Self { stages, linear_c, fuse, bn, classifier, dev })
    }

    pub fn device(&self) -> &Device {
        &self.dev
    }

    /// Class logits `[CLASSES][h/4 * w/4]` (plane per class) of an RGB 8-bit image `w x h`
    /// (both multiples of 32), and the logit grid's size.
    pub fn forward(&self, rgb: &[u8], w: usize, h: usize) -> Result<(Vec<f32>, usize, usize)> {
        if rgb.len() != w * h * 3 || !w.is_multiple_of(32) || !h.is_multiple_of(32) || w == 0 || h == 0 {
            return Err(Error::Image(format!("{w} x {h} image with {} samples (sides must be multiples of 32)", rgb.len())));
        }
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let mut planar = vec![0f32; 3 * w * h];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                planar[c * w * h + i] = (f32::from(px[c]) / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let mut x = Tensor::from_vec(planar, (1, 3, h, w), &self.dev)?;
        let mut feats: Vec<Tensor> = Vec::new();
        for (s, st) in self.stages.iter().enumerate() {
            let y = st.embed.forward(&x)?;
            let (b, c, hh, ww) = y.dims4()?;
            let mut t = st.embed_ln.forward(&y.flatten_from(2)?.transpose(1, 2)?.contiguous()?)?;
            for blk in &st.blocks {
                t = blk.forward(&t, hh, ww)?;
            }
            t = st.ln.forward(&t)?;
            x = t.transpose(1, 2)?.contiguous()?.reshape((b, c, hh, ww))?;
            let _ = s;
            feats.push(x.clone());
        }
        let (_, _, h0, w0) = feats[0].dims4()?;
        let mut ups = Vec::new();
        for (i, f) in feats.iter().enumerate() {
            let (b, c, hh, ww) = f.dims4()?;
            let t = f.flatten_from(2)?.transpose(1, 2)?.contiguous()?;
            let t = self.linear_c[i].forward(&t)?.transpose(1, 2)?.contiguous()?.reshape((b, DECODER, hh, ww))?;
            let _ = c;
            ups.push(if (hh, ww) == (h0, w0) { t } else { t.upsample_bilinear2d(h0, w0, false)? });
        }
        ups.reverse();
        let cat = Tensor::cat(&ups, 1)?;
        let y = self.fuse.forward(&cat)?;
        let y = y.broadcast_mul(&self.bn.scale)?.broadcast_add(&self.bn.shift)?.relu()?;
        let logits = self.classifier.forward(&y)?;
        let v = logits.flatten_all()?.to_vec1::<f32>()?;
        Ok((v, w0, h0))
    }
}

/// Probability of "belongs to `classes`" at every cell of the logit grid, as the logit of the
/// softmax mass of those classes (`ln(sum exp(selected) / sum exp(others))`).
pub fn class_logit(logits: &[f32], cells: usize, classes: &[usize]) -> Vec<f32> {
    let mut out = vec![0f32; cells];
    for (i, o) in out.iter_mut().enumerate() {
        let (mut sel, mut oth) = (0f64, 0f64);
        let mx = (0..CLASSES).map(|c| logits[c * cells + i]).fold(f32::MIN, f32::max);
        for c in 0..CLASSES {
            let e = f64::from((logits[c * cells + i] - mx).exp());
            if classes.contains(&c) {
                sel += e;
            } else {
                oth += e;
            }
        }
        *o = ((sel.max(1e-12) / oth.max(1e-12)).ln() as f32).clamp(-15.0, 15.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_logit_is_positive_where_the_selected_class_wins() {
        let cells = 2;
        let mut l = vec![0f32; CLASSES * cells];
        l[class::SKY * cells] = 8.0; // cell 0: sky
        l[3 * cells + 1] = 8.0; // cell 1: floor
        let v = class_logit(&l, cells, &[class::SKY]);
        assert!(v[0] > 2.0, "{v:?}");
        assert!(v[1] < -4.0, "{v:?}");
    }

    #[test]
    fn groups_are_named() {
        assert_eq!(landscape_classes("sky"), Some(&[class::SKY][..]));
        assert!(landscape_classes("nope").is_none());
    }
}
