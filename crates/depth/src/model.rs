//! The Depth Anything V2 network: a DINOv2 ViT-S/14 backbone and a DPT neck + head.
//!
//! Modified work (Apache License 2.0, §4(b)): ported by the LightCraft contributors in 2026 from
//! the Python/PyTorch code of Hugging Face Transformers (`models/depth_anything/modeling_depth_anything.py`
//! and `models/dinov2/modeling_dinov2.py`), Copyright The HuggingFace Team, itself an
//! implementation of Depth Anything V2 (Yang et al., 2024, Apache-2.0 for the Small model);
//! translated to Rust on candle and restructured. See NOTICE.

use candle_core::{Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig, LayerNorm, Linear, VarBuilder};

use crate::{Error, Result};

pub const PATCH: usize = 14;
const HIDDEN: usize = 384;
const HEADS: usize = 6;
const HEAD_DIM: usize = HIDDEN / HEADS;
const LAYERS: usize = 12;
/// Layers whose output feeds the neck (`out_indices` 3, 6, 9, 12).
const TAPS: [usize; 4] = [3, 6, 9, 12];
const NECK: [usize; 4] = [48, 96, 192, 384];
const FUSION: usize = 64;
const HEAD_HIDDEN: usize = 32;
/// Side of the position table the checkpoint stores (518 / 14).
const POS_GRID: usize = 37;
const EPS: f64 = 1e-6;

fn ln(vb: VarBuilder) -> Result<LayerNorm> {
    Ok(candle_nn::layer_norm(HIDDEN, EPS, vb)?)
}

struct Block {
    norm1: LayerNorm,
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    scale1: Tensor,
    norm2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    scale2: Tensor,
}

impl Block {
    fn new(vb: VarBuilder) -> Result<Self> {
        let a = vb.pp("attention");
        Ok(Self {
            norm1: ln(vb.pp("norm1"))?,
            q: candle_nn::linear(HIDDEN, HIDDEN, a.pp("attention.query"))?,
            k: candle_nn::linear(HIDDEN, HIDDEN, a.pp("attention.key"))?,
            v: candle_nn::linear(HIDDEN, HIDDEN, a.pp("attention.value"))?,
            o: candle_nn::linear(HIDDEN, HIDDEN, a.pp("output.dense"))?,
            scale1: vb.get(HIDDEN, "layer_scale1.lambda1")?,
            norm2: ln(vb.pp("norm2"))?,
            fc1: candle_nn::linear(HIDDEN, 4 * HIDDEN, vb.pp("mlp.fc1"))?,
            fc2: candle_nn::linear(4 * HIDDEN, HIDDEN, vb.pp("mlp.fc2"))?,
            scale2: vb.get(HIDDEN, "layer_scale2.lambda1")?,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, n, _) = x.dims3()?;
        let y = self.norm1.forward(x)?;
        let split = |t: Tensor| -> Result<Tensor> { Ok(t.reshape((b, n, HEADS, HEAD_DIM))?.transpose(1, 2)?.contiguous()?) };
        let (q, k, v) = (split(self.q.forward(&y)?)?, split(self.k.forward(&y)?)?, split(self.v.forward(&y)?)?);
        let att = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * (HEAD_DIM as f64).powf(-0.5))?;
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let o = att.matmul(&v)?.transpose(1, 2)?.reshape((b, n, HIDDEN))?;
        let o = self.o.forward(&o)?;
        let x = (x + o.broadcast_mul(&self.scale1)?)?;
        let y = self.fc2.forward(&self.fc1.forward(&self.norm2.forward(&x)?)?.gelu_erf()?)?;
        Ok((x + y.broadcast_mul(&self.scale2)?)?)
    }
}

/// How a neck stage resizes its projected features.
enum Resize {
    Up(ConvTranspose2d),
    Same,
    Down(Conv2d),
}

struct Reassemble {
    project: Conv2d,
    resize: Resize,
}

struct Residual {
    c1: Conv2d,
    c2: Conv2d,
}

impl Residual {
    fn new(vb: VarBuilder) -> Result<Self> {
        let cfg = Conv2dConfig { padding: 1, ..Default::default() };
        Ok(Self {
            c1: candle_nn::conv2d(FUSION, FUSION, 3, cfg, vb.pp("convolution1"))?,
            c2: candle_nn::conv2d(FUSION, FUSION, 3, cfg, vb.pp("convolution2"))?,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = self.c1.forward(&x.relu()?)?;
        let y = self.c2.forward(&y.relu()?)?;
        Ok((y + x)?)
    }
}

struct Fusion {
    projection: Conv2d,
    r1: Residual,
    r2: Residual,
}

impl Fusion {
    fn forward(&self, x: &Tensor, residual: Option<&Tensor>, size: Option<(usize, usize)>) -> Result<Tensor> {
        let mut x = x.clone();
        if let Some(r) = residual {
            let (_, _, h, w) = x.dims4()?;
            let r = if r.dims4()?.2 != h || r.dims4()?.3 != w { r.upsample_bilinear2d(h, w, false)? } else { r.clone() };
            x = (x + self.r1.forward(&r)?)?;
        }
        let x = self.r2.forward(&x)?;
        let (h, w) = x.dims4().map(|d| (d.2, d.3))?;
        let (th, tw) = size.unwrap_or((h * 2, w * 2));
        Ok(self.projection.forward(&x.upsample_bilinear2d(th, tw, true)?)?)
    }
}

pub struct Net {
    patch: Conv2d,
    cls: Tensor,
    /// `[POS_GRID², HIDDEN]` patch position embeddings, on the CPU for resampling.
    pos_patches: Vec<f32>,
    pos_cls: Tensor,
    blocks: Vec<Block>,
    norm: LayerNorm,
    reassemble: Vec<Reassemble>,
    convs: Vec<Conv2d>,
    fusion: Vec<Fusion>,
    head1: Conv2d,
    head2: Conv2d,
    head3: Conv2d,
}

impl Net {
    /// `vb` at the checkpoint's root (`backbone.*`, `neck.*`, `head.*`).
    pub fn new(vb: VarBuilder) -> Result<Self> {
        let bb = vb.pp("backbone");
        let emb = bb.pp("embeddings");
        let cfg = Conv2dConfig { stride: PATCH, ..Default::default() };
        let patch = candle_nn::conv2d(3, HIDDEN, PATCH, cfg, emb.pp("patch_embeddings.projection"))?;
        let pos = emb.get((1, 1 + POS_GRID * POS_GRID, HIDDEN), "position_embeddings")?;
        let pos_cls = pos.narrow(1, 0, 1)?;
        let pos_patches = pos.narrow(1, 1, POS_GRID * POS_GRID)?.flatten_all()?.to_vec1::<f32>()?;
        let blocks = (0..LAYERS).map(|i| Block::new(bb.pp(format!("encoder.layer.{i}")))).collect::<Result<Vec<_>>>()?;
        let neck = vb.pp("neck");
        let mut reassemble = Vec::new();
        for (i, (&c, factor)) in NECK.iter().zip([4.0f32, 2.0, 1.0, 0.5]).enumerate() {
            let l = neck.pp(format!("reassemble_stage.layers.{i}"));
            let project = candle_nn::conv2d(HIDDEN, c, 1, Default::default(), l.pp("projection"))?;
            let resize = if factor > 1.0 {
                let k = factor as usize;
                Resize::Up(candle_nn::conv_transpose2d(c, c, k, ConvTranspose2dConfig { stride: k, ..Default::default() }, l.pp("resize"))?)
            } else if factor == 1.0 {
                Resize::Same
            } else {
                Resize::Down(candle_nn::conv2d(c, c, 3, Conv2dConfig { stride: 2, padding: 1, ..Default::default() }, l.pp("resize"))?)
            };
            reassemble.push(Reassemble { project, resize });
        }
        let cfg3 = Conv2dConfig { padding: 1, ..Default::default() };
        let convs = NECK
            .iter()
            .enumerate()
            .map(|(i, &c)| Ok(candle_nn::conv2d_no_bias(c, FUSION, 3, cfg3, neck.pp(format!("convs.{i}")))?))
            .collect::<Result<Vec<_>>>()?;
        let fusion = (0..4)
            .map(|i| {
                let l = neck.pp(format!("fusion_stage.layers.{i}"));
                Ok(Fusion {
                    projection: candle_nn::conv2d(FUSION, FUSION, 1, Default::default(), l.pp("projection"))?,
                    r1: Residual::new(l.pp("residual_layer1"))?,
                    r2: Residual::new(l.pp("residual_layer2"))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let head = vb.pp("head");
        Ok(Self {
            patch,
            cls: emb.get((1, 1, HIDDEN), "cls_token")?,
            pos_patches,
            pos_cls,
            blocks,
            norm: ln(bb.pp("layernorm"))?,
            reassemble,
            convs,
            fusion,
            head1: candle_nn::conv2d(FUSION, FUSION / 2, 3, cfg3, head.pp("conv1"))?,
            head2: candle_nn::conv2d(FUSION / 2, HEAD_HIDDEN, 3, cfg3, head.pp("conv2"))?,
            head3: candle_nn::conv2d(HEAD_HIDDEN, 1, 1, Default::default(), head.pp("conv3"))?,
        })
    }

    /// `pixels`: `[1, 3, H, W]` normalized with the ImageNet statistics, `H` and `W` multiples of
    /// 14. Returns the relative inverse depth `[H, W]` (larger = nearer), row-major.
    pub fn forward(&self, pixels: &Tensor) -> Result<Vec<f32>> {
        let (_, _, hh, ww) = pixels.dims4()?;
        if hh % PATCH != 0 || ww % PATCH != 0 || hh == 0 || ww == 0 {
            return Err(Error::Model(format!("input {ww}×{hh} is not a multiple of {PATCH}")));
        }
        let (gh, gw) = (hh / PATCH, ww / PATCH);
        let dev = pixels.device();
        let x = self.patch.forward(pixels)?.flatten_from(2)?.transpose(1, 2)?; // [1, N, C]
        let pos = Tensor::from_vec(resample_pos(&self.pos_patches, gh, gw), (1, gh * gw, HIDDEN), dev)?;
        let x = Tensor::cat(&[self.cls.to_device(dev)?.broadcast_add(&self.pos_cls.to_device(dev)?)?, x.broadcast_add(&pos)?], 1)?;
        let mut x = x;
        let mut taps = Vec::with_capacity(4);
        for (i, b) in self.blocks.iter().enumerate() {
            x = b.forward(&x)?;
            if TAPS.contains(&(i + 1)) {
                taps.push(self.norm.forward(&x)?);
            }
        }
        let mut feats = Vec::with_capacity(4);
        for (t, r) in taps.iter().zip(&self.reassemble) {
            let f = t.narrow(1, 1, gh * gw)?.reshape((1, gh, gw, HIDDEN))?.permute((0, 3, 1, 2))?.contiguous()?;
            let f = r.project.forward(&f)?;
            let f = match &r.resize {
                Resize::Up(c) => c.forward(&f)?,
                Resize::Same => f,
                Resize::Down(c) => c.forward(&f)?,
            };
            feats.push(f);
        }
        let feats: Vec<Tensor> = feats.iter().zip(&self.convs).map(|(f, c)| Ok(c.forward(f)?)).collect::<Result<_>>()?;
        // fuse from the coarsest map to the finest
        let rev: Vec<&Tensor> = feats.iter().rev().collect();
        let size_of = |t: &Tensor| t.dims4().map(|d| (d.2, d.3));
        let mut fused = self.fusion[0].forward(rev[0], None, Some(size_of(rev[1])?))?;
        for i in 1..4 {
            let size = if i < 3 { Some(size_of(rev[i + 1])?) } else { None };
            fused = self.fusion[i].forward(&fused, Some(rev[i]), size)?;
        }
        let y = self.head1.forward(&fused)?.upsample_bilinear2d(hh, ww, true)?;
        let y = self.head2.forward(&y)?.relu()?;
        let y = self.head3.forward(&y)?.relu()?;
        Ok(y.flatten_all()?.to_vec1::<f32>()?)
    }
}

/// The `POS_GRID × POS_GRID` position table resampled to `gh × gw` (bicubic, PyTorch's
/// `align_corners = false` convention with A = −0.75 and replicated borders), `[gh·gw, HIDDEN]`.
fn resample_pos(table: &[f32], gh: usize, gw: usize) -> Vec<f32> {
    if gh == POS_GRID && gw == POS_GRID {
        return table.to_vec();
    }
    fn cubic(t: f32) -> [f32; 4] {
        const A: f32 = -0.75;
        let k = |x: f32| {
            let x = x.abs();
            if x <= 1.0 {
                ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
            } else if x < 2.0 {
                (((x - 5.0) * x + 8.0) * x - 4.0) * A
            } else {
                0.0
            }
        };
        [k(t + 1.0), k(t), k(1.0 - t), k(2.0 - t)]
    }
    let taps = |dst: usize, n: usize| -> Vec<(isize, [f32; 4])> {
        (0..dst)
            .map(|i| {
                let s = (i as f32 + 0.5) * POS_GRID as f32 / n as f32 - 0.5;
                let f = s.floor();
                (f as isize, cubic(s - f))
            })
            .collect::<Vec<_>>()
    };
    let (ty, tx) = (taps(gh, gh), taps(gw, gw));
    let at = |y: isize, x: isize, c: usize| {
        table[(y.clamp(0, POS_GRID as isize - 1) as usize * POS_GRID + x.clamp(0, POS_GRID as isize - 1) as usize) * HIDDEN + c]
    };
    let mut out = vec![0f32; gh * gw * HIDDEN];
    for (j, (y0, wy)) in ty.iter().enumerate() {
        for (i, (x0, wx)) in tx.iter().enumerate() {
            for c in 0..HIDDEN {
                let mut acc = 0.0;
                for (a, wa) in wy.iter().enumerate() {
                    for (b, wb) in wx.iter().enumerate() {
                        acc += wa * wb * at(*y0 - 1 + a as isize, *x0 - 1 + b as isize, c);
                    }
                }
                out[(j * gw + i) * HIDDEN + c] = acc;
            }
        }
    }
    out
}
