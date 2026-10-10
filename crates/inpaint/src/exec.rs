//! A small ONNX interpreter on candle: just the operators of the MI-GAN export, run on fixed
//! shapes. Values are *host* values (shape bookkeeping: tensor shapes, pads, constants; a few
//! numbers computed on the CPU in `f64`) or *device* tensors (the image data: Metal on macOS,
//! CPU elsewhere). Shape arithmetic never touches the device.

use std::collections::HashMap;

use candle_core::{DType, Device, Tensor};

use crate::onnx::{Graph, Node, RawTensor};
use crate::{Error, Result};

fn err(s: impl Into<String>) -> Error {
    Error::Model(s.into())
}

impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Error::Model(e.to_string())
    }
}

/// A small tensor held on the host.
#[derive(Clone, Debug)]
pub struct Host {
    pub dims: Vec<usize>,
    pub data: Vec<f64>,
}

/// Largest host tensor `ConstantOfShape` makes (bigger ones go to the device).
const HOST_MAX: usize = 4096;

#[derive(Clone)]
enum Val {
    Host(Host),
    Dev(Tensor),
}

impl Val {
    fn host(&self, what: &str) -> Result<&Host> {
        match self {
            Val::Host(h) => Ok(h),
            Val::Dev(_) => Err(err(format!("{what}: expected a constant"))),
        }
    }

    fn dims(&self) -> Vec<usize> {
        match self {
            Val::Host(h) => h.dims.clone(),
            Val::Dev(t) => t.dims().to_vec(),
        }
    }

    fn tensor(&self, dev: &Device) -> Result<Tensor> {
        match self {
            Val::Dev(t) => Ok(t.clone()),
            Val::Host(h) => Ok(Tensor::from_vec(h.data.iter().map(|v| *v as f32).collect::<Vec<f32>>(), h.dims.as_slice(), dev)?),
        }
    }
}

/// A loaded graph, ready to run.
pub struct Model {
    nodes: Vec<Node>,
    weights: HashMap<String, Val>,
    pub inputs: Vec<String>,
    outputs: Vec<String>,
    dev: Device,
}

impl Model {
    pub fn new(g: Graph, dev: Device) -> Result<Model> {
        let mut weights = HashMap::new();
        for (name, t) in g.initializers {
            weights.insert(name, to_val(t, &dev)?);
        }
        let mut m = Model { nodes: g.nodes, weights, inputs: g.inputs, outputs: g.outputs, dev };
        m.fold_batch_norms()?;
        Ok(m)
    }

    /// Fold every `Conv → BatchNormalization` pair (the BN reading only that convolution) into
    /// the convolution's weights and bias: the same values, one operator fewer per pair (a
    /// ResNet's inference gets about a quarter faster).
    fn fold_batch_norms(&mut self) -> Result<()> {
        let mut uses: HashMap<&str, usize> = HashMap::new();
        for n in &self.nodes {
            for x in &n.inputs {
                *uses.entry(x).or_default() += 1;
            }
        }
        for o in &self.outputs {
            *uses.entry(o).or_default() += 1;
        }
        let mut folds = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            let Some(next) = self.nodes.get(i + 1) else { break };
            let foldable = n.op == "Conv"
                && next.op == "BatchNormalization"
                && next.inputs.len() == 5
                && next.inputs.first() == n.outputs.first()
                && n.outputs.first().is_some_and(|o| uses.get(o.as_str()) == Some(&1))
                && n.inputs.get(1).is_some_and(|w| matches!(self.weights.get(w), Some(Val::Dev(_))))
                && next.inputs[1..].iter().all(|p| matches!(self.weights.get(p), Some(Val::Dev(_))));
            if foldable {
                folds.push(i);
            }
        }
        let mut remove = Vec::new();
        for &i in &folds {
            let (conv, bn) = (self.nodes[i].clone(), self.nodes[i + 1].clone());
            let w = self.weights[&conv.inputs[1]].tensor(&self.dev)?;
            let get = |k: usize| self.weights[&bn.inputs[k]].tensor(&self.dev);
            let (scale, beta, mean, var) = (get(1)?, get(2)?, get(3)?, get(4)?);
            let eps = bn.float("epsilon").unwrap_or(1e-5);
            let s = scale.broadcast_div(&(var + eps)?.sqrt()?)?;
            let c = s.elem_count();
            let w2 = w.broadcast_mul(&s.reshape((c, 1, 1, 1))?)?;
            let b0 = match conv.inputs.get(2).filter(|b| !b.is_empty()) {
                Some(b) => self.weights[b].tensor(&self.dev)?,
                None => Tensor::zeros(c, DType::F32, &self.dev)?,
            };
            let b2 = ((b0 - mean)?.broadcast_mul(&s)? + beta)?;
            let (wn, bnn) = (format!("{}__bn", conv.inputs[1]), format!("{}__bnb", conv.inputs[1]));
            self.weights.insert(wn.clone(), Val::Dev(w2));
            self.weights.insert(bnn.clone(), Val::Dev(b2));
            let node = &mut self.nodes[i];
            node.inputs = vec![conv.inputs[0].clone(), wn, bnn];
            node.outputs = bn.outputs.clone();
            remove.push(i + 1);
        }
        for i in remove.into_iter().rev() {
            self.nodes.remove(i);
        }
        Ok(())
    }

    pub fn device(&self) -> &Device {
        &self.dev
    }

    /// Run on named inputs (float tensors); returns the first output.
    pub fn run(&self, inputs: &[(&str, Tensor)]) -> Result<Tensor> {
        let mut env: HashMap<&str, Val> = HashMap::new();
        for (k, t) in inputs {
            env.insert(k, Val::Dev(t.clone()));
        }
        // (a value is dropped after its last use to keep the device's memory small)
        let mut last: HashMap<&str, usize> = HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            for x in &n.inputs {
                last.insert(x, i);
            }
        }
        let mut skip: Option<usize> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            if skip == Some(i) {
                continue;
            }
            let get = |name: &str| -> Result<Val> {
                if name.is_empty() {
                    return Err(err("missing optional input used"));
                }
                env.get(name).or_else(|| self.weights.get(name)).cloned().ok_or_else(|| err(format!("{}: input `{name}` is not computed", n.op)))
            };
            // LeakyRelu followed by a scalar Mul (the network's gain) is one fused step
            let fused = match (n.op.as_str(), self.nodes.get(i + 1)) {
                ("LeakyRelu", Some(m))
                    if m.op == "Mul" && m.inputs.first() == n.outputs.first() && last.get(n.outputs[0].as_str()) == Some(&(i + 1)) =>
                {
                    match (m.inputs.get(1).map(|c| get(c)), get(&n.inputs[0])) {
                        (Some(Ok(Val::Host(c))), Ok(Val::Dev(x))) if c.data.len() == 1 => Some((m, x, c.data[0] as f32)),
                        _ => None,
                    }
                }
                _ => None,
            };
            let outs = if let Some((m, x, gain)) = fused {
                // max(g·x, g·α·x) = g · leaky_relu(x) for g > 0
                let alpha = n.float("alpha").unwrap_or(0.01) as f32;
                if gain <= 0.0 {
                    return Err(err("a negative gain after LeakyRelu is not supported"));
                }
                skip = Some(i + 1);
                for o in &m.outputs {
                    last.insert(o, last.get(o.as_str()).copied().unwrap_or(i + 1));
                }
                vec![Val::Dev(x.affine(f64::from(gain), 0.0)?.maximum(&x.affine(f64::from(gain * alpha), 0.0)?)?)]
            } else {
                self.op(n, &get)?
            };
            let out_names = if skip == Some(i + 1) { &self.nodes[i + 1].outputs } else { &n.outputs };
            for (name, v) in out_names.iter().zip(outs) {
                env.insert(name, v);
            }
            for x in &n.inputs {
                if last.get(x.as_str()) == Some(&i) && !self.weights.contains_key(x) {
                    env.remove(x.as_str());
                }
            }
        }
        let out = self.outputs.first().ok_or_else(|| err("the graph has no output"))?;
        match env.get(out.as_str()) {
            Some(Val::Dev(t)) => Ok(t.clone()),
            Some(Val::Host(h)) => Ok(Val::Host(h.clone()).tensor(&self.dev)?),
            None => Err(err("the output was not computed")),
        }
    }

    fn op(&self, n: &Node, get: &dyn Fn(&str) -> Result<Val>) -> Result<Vec<Val>> {
        let input = |i: usize| -> Result<Val> { get(n.inputs.get(i).map(String::as_str).unwrap_or("")) };
        let dev = &self.dev;
        let one = |v: Val| Ok(vec![v]);
        match n.op.as_str() {
            "Constant" => {
                let t = n.tensor("value").ok_or_else(|| err("Constant without a tensor value"))?;
                one(Val::Host(Host { dims: t.dims.clone(), data: t.data.clone() }))
            }
            "Cast" => {
                let to = n.int("to").ok_or_else(|| err("Cast without `to`"))?;
                match input(0)? {
                    Val::Host(mut h) => {
                        match to {
                            // integers (int64, int32, uint8, int8): truncate toward zero
                            7 | 6 | 3 => h.data.iter_mut().for_each(|v| *v = v.trunc()),
                            2 => h.data.iter_mut().for_each(|v| *v = v.trunc().clamp(0.0, 255.0)),
                            _ => {}
                        }
                        one(Val::Host(h))
                    }
                    Val::Dev(t) => match to {
                        1 => one(Val::Dev(t)),
                        // uint8 results stay floats holding whole numbers in 0..255
                        2 => one(Val::Dev(t.clamp(0f32, 255f32)?)),
                        _ => Err(err(format!("Cast of image data to type {to} is not supported"))),
                    },
                }
            }
            "Shape" => {
                let d = input(0)?.dims();
                one(Val::Host(Host { dims: vec![d.len()], data: d.iter().map(|v| *v as f64).collect() }))
            }
            "Slice" => {
                let x = input(0)?;
                let list = |i: usize| -> Result<Option<Vec<i64>>> {
                    match n.inputs.get(i).filter(|s| !s.is_empty()) {
                        Some(s) => Ok(Some(get(s)?.host("Slice")?.data.iter().map(|v| *v as i64).collect())),
                        None => Ok(None),
                    }
                };
                let (starts, ends) = (list(1)?.ok_or_else(|| err("Slice: no starts"))?, list(2)?.ok_or_else(|| err("Slice: no ends"))?);
                let axes = list(3)?.unwrap_or_else(|| (0..starts.len() as i64).collect());
                let steps = list(4)?.unwrap_or_else(|| vec![1; starts.len()]);
                one(slice(&x, &starts, &ends, &axes, &steps)?)
            }
            "Concat" => {
                let axis = n.int("axis").ok_or_else(|| err("Concat without `axis`"))?;
                let vals: Vec<Val> = (0..n.inputs.len()).map(&input).collect::<Result<_>>()?;
                one(concat(&vals, axis, dev)?)
            }
            "Resize" => {
                let x = input(0)?;
                let Val::Dev(t) = x else { return Err(err("Resize of a constant is not supported")) };
                let (_, _, h, w) = t.dims4()?;
                let target = if let Some(s) = n.inputs.get(3).filter(|s| !s.is_empty()) {
                    let v = get(s)?;
                    let d = &v.host("Resize sizes")?.data;
                    (d.get(2).copied().unwrap_or(h as f64) as usize, d.get(3).copied().unwrap_or(w as f64) as usize)
                } else if let Some(s) = n.inputs.get(2).filter(|s| !s.is_empty()) {
                    let v = get(s)?;
                    let d = &v.host("Resize scales")?.data;
                    ((h as f64 * d.get(2).copied().unwrap_or(1.0)).floor() as usize, (w as f64 * d.get(3).copied().unwrap_or(1.0)).floor() as usize)
                } else {
                    return Err(err("Resize without sizes or scales"));
                };
                if target == (h, w) {
                    return one(Val::Dev(t));
                }
                match n.string("mode") {
                    Some("nearest")
                        if n.string("coordinate_transformation_mode") == Some("asymmetric")
                            && n.string("nearest_mode").unwrap_or("floor") == "floor" =>
                    {
                        one(Val::Dev(t.upsample_nearest2d(target.0, target.1)?))
                    }
                    Some("linear") if matches!(n.string("coordinate_transformation_mode"), Some("half_pixel" | "pytorch_half_pixel")) => {
                        one(Val::Dev(t.upsample_bilinear2d(target.0, target.1, false)?))
                    }
                    m => Err(err(format!("Resize {m:?} {:?} is not supported", n.string("coordinate_transformation_mode")))),
                }
            }
            "Round" => match input(0)? {
                Val::Host(mut h) => {
                    h.data.iter_mut().for_each(|v| *v = round_even(*v));
                    one(Val::Host(h))
                }
                Val::Dev(t) => one(Val::Dev(t.round()?)),
            },
            "Add" | "Sub" | "Mul" | "Div" => one(binary(&n.op, &input(0)?, &input(1)?, dev)?),
            "Conv" => {
                let Val::Dev(x) = input(0)? else { return Err(err("Conv of a constant")) };
                let w = input(1)?.tensor(dev)?;
                let pads = n.ints("pads").map(<[i64]>::to_vec).unwrap_or_else(|| vec![0; 4]);
                let strides = n.ints("strides").map(<[i64]>::to_vec).unwrap_or_else(|| vec![1, 1]);
                let dil = n.ints("dilations").map(<[i64]>::to_vec).unwrap_or_else(|| vec![1, 1]);
                if pads.iter().any(|p| *p != pads[0]) || strides[0] != strides[1] || dil[0] != dil[1] {
                    return Err(err("Conv with uneven padding, strides or dilations is not supported"));
                }
                let group = n.int("group").unwrap_or(1) as usize;
                let (c_in, wd) = (x.dim(1)?, w.dims().to_vec());
                let mut y = if group > 1 && group == c_in && wd.len() == 4 && wd[0] == c_in && wd[1] == 1 && dil[0] == 1 {
                    depthwise(&x, &w, pads[0] as usize, strides[0] as usize)?
                } else if group == 1 && wd.len() == 4 && wd[2] == 1 && wd[3] == 1 && pads[0] == 0 && strides[0] == 1 {
                    // a 1 × 1 convolution is a matrix product (no im2col copy)
                    let (b, _, h, wi) = x.dims4()?;
                    let wm = w.reshape((wd[0], wd[1]))?;
                    let cols = x.reshape((b, c_in, h * wi))?;
                    wm.broadcast_matmul(&cols)?.reshape((b, wd[0], h, wi))?
                } else {
                    x.conv2d(&w, pads[0] as usize, strides[0] as usize, dil[0] as usize, group)?
                };
                if n.inputs.len() > 2 && !n.inputs[2].is_empty() {
                    let b = input(2)?.tensor(dev)?;
                    y = y.broadcast_add(&b.reshape((1, b.elem_count(), 1, 1))?)?;
                }
                one(Val::Dev(y))
            }
            "BatchNormalization" => {
                // inference form: y = (x − mean) · scale / √(var + ε) + bias, per channel
                let Val::Dev(x) = input(0)? else { return Err(err("BatchNormalization of a constant")) };
                let (scale, bias, mean, var) = (input(1)?.tensor(dev)?, input(2)?.tensor(dev)?, input(3)?.tensor(dev)?, input(4)?.tensor(dev)?);
                let eps = n.float("epsilon").unwrap_or(1e-5);
                let s = scale.broadcast_div(&(var + eps)?.sqrt()?)?;
                let t = (bias - (mean * &s)?)?;
                let c = s.elem_count();
                // per-channel along axis 1, whatever the rank (images are 4-D, a feature vector 2-D)
                let mut shape = vec![1usize; x.rank()];
                if x.rank() > 1 {
                    shape[1] = c;
                } else {
                    shape[0] = c;
                }
                one(Val::Dev(x.broadcast_mul(&s.reshape(shape.as_slice())?)?.broadcast_add(&t.reshape(shape.as_slice())?)?))
            }
            "PRelu" => {
                let Val::Dev(x) = input(0)? else { return Err(err("PRelu of a constant")) };
                let slope = input(1)?.tensor(dev)?;
                let slope = if x.rank() == 4 && slope.elem_count() > 1 { slope.reshape((1, slope.elem_count(), 1, 1))? } else { slope };
                let neg = x.clamp(f32::MIN, 0f32)?;
                one(Val::Dev((x.relu()? + neg.broadcast_mul(&slope)?)?))
            }
            "Flatten" => {
                let Val::Dev(x) = input(0)? else { return Err(err("Flatten of a constant")) };
                let axis = usize::try_from(n.int("axis").unwrap_or(1)).unwrap_or(1).min(x.rank());
                let dims = x.dims().to_vec();
                let (a, b): (usize, usize) = (dims[..axis].iter().product(), dims[axis..].iter().product());
                one(Val::Dev(x.reshape((a, b))?))
            }
            "Gemm" => {
                let Val::Dev(a) = input(0)? else { return Err(err("Gemm of a constant")) };
                let mut b = input(1)?.tensor(dev)?;
                if n.int("transA").unwrap_or(0) != 0 {
                    return Err(err("Gemm with transA is not supported"));
                }
                if n.int("transB").unwrap_or(0) != 0 {
                    b = b.t()?;
                }
                let mut y = a.matmul(&b)?;
                let alpha = n.float("alpha").unwrap_or(1.0);
                if (alpha - 1.0).abs() > f64::EPSILON {
                    y = y.affine(alpha, 0.0)?;
                }
                if n.inputs.len() > 2 && !n.inputs[2].is_empty() {
                    let mut c = input(2)?.tensor(dev)?;
                    let beta = n.float("beta").unwrap_or(1.0);
                    if (beta - 1.0).abs() > f64::EPSILON {
                        c = c.affine(beta, 0.0)?;
                    }
                    y = y.broadcast_add(&c)?;
                }
                one(Val::Dev(y))
            }
            "LeakyRelu" => {
                let Val::Dev(x) = input(0)? else { return Err(err("LeakyRelu of a constant")) };
                one(Val::Dev(candle_nn::ops::leaky_relu(&x, n.float("alpha").unwrap_or(0.01))?))
            }
            "Clip" => {
                let Val::Dev(x) = input(0)? else { return Err(err("Clip of a constant")) };
                let bound = |i: usize, d: f64| -> Result<f64> {
                    match n.inputs.get(i).filter(|s| !s.is_empty()) {
                        Some(s) => Ok(get(s)?.host("Clip bound")?.data.first().copied().unwrap_or(d)),
                        None => Ok(d),
                    }
                };
                one(Val::Dev(x.clamp(bound(1, f64::MIN)? as f32, bound(2, f64::MAX)? as f32)?))
            }
            "Pad" => {
                let Val::Dev(x) = input(0)? else { return Err(err("Pad of a constant")) };
                let pads: Vec<i64> = input(1)?.host("Pad pads")?.data.iter().map(|v| *v as i64).collect();
                let rank = x.rank();
                if pads.len() != 2 * rank || n.string("mode").unwrap_or("constant") != "constant" {
                    return Err(err("only constant Pad of a full pad list is supported"));
                }
                if let Some(v) = n.inputs.get(2).filter(|s| !s.is_empty())
                    && get(v)?.host("Pad value")?.data.first().copied().unwrap_or(0.0) != 0.0
                {
                    return Err(err("Pad with a non-zero value is not supported"));
                }
                let mut y = x;
                for d in 0..rank {
                    let (a, b) = (pads[d], pads[rank + d]);
                    if a < 0 || b < 0 {
                        return Err(err("negative Pad is not supported"));
                    }
                    if a > 0 || b > 0 {
                        y = y.pad_with_zeros(d, a as usize, b as usize)?;
                    }
                }
                one(Val::Dev(y))
            }
            "ConstantOfShape" => {
                let shape: Vec<usize> = input(0)?.host("ConstantOfShape")?.data.iter().map(|v| *v as usize).collect();
                let value = n.tensor("value").and_then(|t| t.data.first().copied()).unwrap_or(0.0);
                let count: usize = shape.iter().product();
                if count <= HOST_MAX {
                    one(Val::Host(Host { data: vec![value; count], dims: shape }))
                } else {
                    one(Val::Dev(Tensor::full(value as f32, shape.as_slice(), dev)?))
                }
            }
            "Reshape" => {
                let x = input(0)?;
                let spec: Vec<i64> = input(1)?.host("Reshape shape")?.data.iter().map(|v| *v as i64).collect();
                let old = x.dims();
                let total: usize = old.iter().product();
                let mut dims: Vec<usize> = spec
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        if *v == 0 {
                            old.get(i).copied().unwrap_or(1)
                        } else if *v < 0 {
                            0
                        } else {
                            *v as usize
                        }
                    })
                    .collect();
                if let Some(i) = spec.iter().position(|v| *v < 0) {
                    let rest: usize = dims.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, v)| *v).product();
                    dims[i] = total.checked_div(rest).ok_or_else(|| err("Reshape by zero"))?;
                }
                match x {
                    Val::Host(h) => one(Val::Host(Host { dims, data: h.data })),
                    Val::Dev(t) => one(Val::Dev(t.reshape(dims.as_slice())?)),
                }
            }
            "Transpose" => {
                let perm: Vec<usize> = match n.ints("perm") {
                    Some(p) => p.iter().map(|v| *v as usize).collect(),
                    None => (0..input(0)?.dims().len()).rev().collect(),
                };
                match input(0)? {
                    Val::Dev(t) => one(Val::Dev(t.permute(perm)?.contiguous()?)),
                    Val::Host(h) => {
                        let nd = h.dims.len();
                        if perm.len() != nd {
                            return Err(err("Transpose: bad permutation"));
                        }
                        let new_dims: Vec<usize> = perm.iter().map(|p| h.dims[*p]).collect();
                        let strides: Vec<usize> = (0..nd).map(|i| h.dims[i + 1..].iter().product()).collect();
                        let mut data = Vec::with_capacity(h.data.len());
                        let mut idx = vec![0usize; nd];
                        for _ in 0..h.data.len() {
                            data.push(h.data[perm.iter().zip(&idx).map(|(p, i)| strides[*p] * i).sum::<usize>()]);
                            for d in (0..nd).rev() {
                                idx[d] += 1;
                                if idx[d] < new_dims[d] {
                                    break;
                                }
                                idx[d] = 0;
                            }
                        }
                        one(Val::Host(Host { dims: new_dims, data }))
                    }
                }
            }
            "Gather" => {
                let (d, i) = (input(0)?, input(1)?);
                let (d, i) = (d.host("Gather")?, i.host("Gather indices")?);
                let axis = n.int("axis").unwrap_or(0);
                if d.dims.len() != 1 || axis != 0 {
                    return Err(err("only Gather on a 1-D constant is supported"));
                }
                let data = i
                    .data
                    .iter()
                    .map(|v| if *v < 0.0 { d.data.len() as i64 + *v as i64 } else { *v as i64 })
                    .map(|j| d.data.get(j as usize).copied().ok_or_else(|| err("Gather out of range")))
                    .collect::<Result<Vec<f64>>>()?;
                one(Val::Host(Host { dims: i.dims.clone(), data }))
            }
            "Unsqueeze" => {
                let axes: Vec<i64> = match n.ints("axes") {
                    Some(a) => a.to_vec(),
                    None => input(1)?.host("Unsqueeze axes")?.data.iter().map(|v| *v as i64).collect(),
                };
                let x = input(0)?;
                let mut dims = x.dims();
                let out_rank = dims.len() + axes.len();
                let mut axes: Vec<usize> = axes.iter().map(|a| if *a < 0 { (out_rank as i64 + a) as usize } else { *a as usize }).collect();
                axes.sort_unstable();
                for a in axes {
                    dims.insert(a.min(dims.len()), 1);
                }
                match x {
                    Val::Host(h) => one(Val::Host(Host { dims, data: h.data })),
                    Val::Dev(t) => one(Val::Dev(t.reshape(dims.as_slice())?)),
                }
            }
            op => Err(err(format!("operator {op} is not supported"))),
        }
    }
}

/// A depthwise convolution (one `kh × kw` filter per channel) as a sum of shifted copies of the
/// input, each scaled per channel: a handful of whole-tensor operations instead of one tiny
/// convolution per channel, which is what the generic grouped path costs.
fn depthwise(x: &Tensor, w: &Tensor, pad: usize, stride: usize) -> Result<Tensor> {
    let (b, c, h, wd) = x.dims4()?;
    let (kh, kw) = (w.dim(2)?, w.dim(3)?);
    let (hp, wp) = (h + 2 * pad, wd + 2 * pad);
    if hp < kh || wp < kw || stride == 0 {
        return Err(err("depthwise: the input is smaller than the filter"));
    }
    let (oh, ow) = ((hp - kh) / stride + 1, (wp - kw) / stride + 1);
    let mut xp = x.clone();
    if pad > 0 {
        xp = xp.pad_with_zeros(2, pad, pad)?.pad_with_zeros(3, pad, pad)?;
    }
    // with a stride, taps are picked from a (rows/s, s, cols/s, s) view of the padded input
    let (ph, pw) = (hp.div_ceil(stride) * stride, wp.div_ceil(stride) * stride);
    let phased = if stride > 1 {
        let q = xp.pad_with_zeros(2, 0, ph - hp)?.pad_with_zeros(3, 0, pw - wp)?;
        Some(q.reshape((b, c, ph / stride, stride, pw / stride, stride))?)
    } else {
        None
    };
    let mut acc: Option<Tensor> = None;
    for ky in 0..kh {
        for kx in 0..kw {
            let tap = match &phased {
                None => xp.narrow(2, ky, oh)?.narrow(3, kx, ow)?,
                Some(v) => v
                    .narrow(2, ky / stride, oh)?
                    .narrow(3, ky % stride, 1)?
                    .narrow(4, kx / stride, ow)?
                    .narrow(5, kx % stride, 1)?
                    .reshape((b, c, oh, ow))?,
            };
            let f = w.narrow(2, ky, 1)?.narrow(3, kx, 1)?.reshape((1, c, 1, 1))?;
            let t = tap.broadcast_mul(&f)?;
            acc = Some(match acc {
                None => t,
                Some(a) => (a + t)?,
            });
        }
    }
    acc.ok_or_else(|| err("depthwise: empty filter"))
}

fn to_val(t: RawTensor, dev: &Device) -> Result<Val> {
    // weights go to the device; small tensors (shapes, constants) stay on the host
    if t.data.len() > 64 {
        let v: Vec<f32> = t.data.iter().map(|v| *v as f32).collect();
        Ok(Val::Dev(Tensor::from_vec(v, t.dims.as_slice(), dev)?.to_dtype(DType::F32)?))
    } else {
        Ok(Val::Host(Host { dims: t.dims, data: t.data }))
    }
}

fn round_even(v: f64) -> f64 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 && r % 2.0 != 0.0 { r - v.signum() } else { r }
}

fn norm_axis(a: i64, rank: usize) -> Result<usize> {
    let r = if a < 0 { rank as i64 + a } else { a };
    usize::try_from(r).ok().filter(|r| *r < rank).ok_or_else(|| err("axis out of range"))
}

fn slice(x: &Val, starts: &[i64], ends: &[i64], axes: &[i64], steps: &[i64]) -> Result<Val> {
    let dims = x.dims();
    // per axis: the indices kept
    let mut keep: Vec<Vec<usize>> = dims.iter().map(|d| (0..*d).collect()).collect();
    for k in 0..axes.len() {
        let a = norm_axis(axes[k], dims.len())?;
        let (d, step) = (dims[a] as i64, steps.get(k).copied().unwrap_or(1));
        if step == 0 {
            return Err(err("Slice with a zero step"));
        }
        let clamp = |v: i64, lo: i64, hi: i64| v.clamp(lo, hi);
        let norm = |v: i64| if v < 0 { v + d } else { v };
        let (s, e) = if step > 0 {
            (clamp(norm(starts[k]), 0, d), clamp(norm(ends[k]), 0, d))
        } else {
            (clamp(norm(starts[k]), -1, d - 1), clamp(norm(ends[k]), -1, d - 1))
        };
        let mut idx = Vec::new();
        let mut i = s;
        while (step > 0 && i < e) || (step < 0 && i > e) {
            idx.push(i as usize);
            i += step;
        }
        keep[a] = idx;
    }
    match x {
        Val::Host(h) => {
            let strides: Vec<usize> = (0..dims.len()).map(|i| dims[i + 1..].iter().product()).collect();
            let new_dims: Vec<usize> = keep.iter().map(Vec::len).collect();
            let mut data = Vec::new();
            let mut idx = vec![0usize; dims.len()];
            let total: usize = new_dims.iter().product();
            for _ in 0..total {
                data.push(h.data[idx.iter().enumerate().map(|(d, i)| keep[d][*i] * strides[d]).sum::<usize>()]);
                for d in (0..dims.len()).rev() {
                    idx[d] += 1;
                    if idx[d] < new_dims[d] {
                        break;
                    }
                    idx[d] = 0;
                }
            }
            Ok(Val::Host(Host { dims: new_dims, data }))
        }
        Val::Dev(t) => {
            let mut y = t.clone();
            for (a, idx) in keep.iter().enumerate() {
                if idx.len() != dims[a] {
                    let contiguous = idx.windows(2).all(|w| w[1] == w[0] + 1);
                    if !contiguous || idx.is_empty() {
                        return Err(err("Slice of image data must be a contiguous range"));
                    }
                    y = y.narrow(a, idx[0], idx.len())?;
                }
            }
            Ok(Val::Dev(y))
        }
    }
}

fn concat(vals: &[Val], axis: i64, dev: &Device) -> Result<Val> {
    let rank = vals.first().map(|v| v.dims().len()).ok_or_else(|| err("Concat of nothing"))?;
    let axis = norm_axis(axis, rank)?;
    if vals.iter().all(|v| matches!(v, Val::Host(_))) {
        let hs: Vec<&Host> = vals.iter().filter_map(|v| if let Val::Host(h) = v { Some(h) } else { None }).collect();
        if rank == 1 {
            return Ok(Val::Host(Host {
                dims: vec![hs.iter().map(|h| h.data.len()).sum()],
                data: hs.iter().flat_map(|h| h.data.iter().copied()).collect(),
            }));
        }
        return Err(err("Concat of constants is supported for 1-D only"));
    }
    let ts: Vec<Tensor> = vals.iter().map(|v| v.tensor(dev)).collect::<Result<_>>()?;
    Ok(Val::Dev(Tensor::cat(&ts, axis)?))
}

fn binary(op: &str, a: &Val, b: &Val, dev: &Device) -> Result<Val> {
    if let (Val::Host(x), Val::Host(y)) = (a, b) {
        // numpy broadcasting on small constants: scalars and equal lengths are all the graph needs
        let f = |p: f64, q: f64| match op {
            "Add" => p + q,
            "Sub" => p - q,
            "Mul" => p * q,
            _ => p / q,
        };
        let (n, dims) = if x.data.len() >= y.data.len() { (x.data.len(), x.dims.clone()) } else { (y.data.len(), y.dims.clone()) };
        let pick = |h: &Host, i: usize| {
            if h.data.len() == 1 {
                Ok(h.data[0])
            } else if h.data.len() == n {
                Ok(h.data[i])
            } else {
                Err(err("unsupported broadcast of constants"))
            }
        };
        let data = (0..n).map(|i| Ok(f(pick(x, i)?, pick(y, i)?))).collect::<Result<Vec<f64>>>()?;
        return Ok(Val::Host(Host { dims, data }));
    }
    let (x, y) = (a.tensor(dev)?, b.tensor(dev)?);
    Ok(Val::Dev(match op {
        "Add" => x.broadcast_add(&y)?,
        "Sub" => x.broadcast_sub(&y)?,
        "Mul" => x.broadcast_mul(&y)?,
        _ => x.broadcast_div(&y)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize, seed: f32) -> Vec<f32> {
        (0..n).map(|i| ((i as f32 * 0.37 + seed).sin() * 3.0).round() / 3.0).collect()
    }

    #[test]
    fn depthwise_matches_the_generic_grouped_convolution() {
        let dev = Device::Cpu;
        for (k, stride, pad, size) in [(3usize, 1usize, 1usize, 10usize), (4, 2, 1, 12), (4, 1, 0, 9), (3, 2, 1, 8)] {
            let c = 5;
            // a batch of two: the strided path must keep the batch axis
            let x = Tensor::from_vec(ramp(2 * c * size * size, 1.0), (2, c, size, size), &dev).unwrap();
            let w = Tensor::from_vec(ramp(c * k * k, 2.0), (c, 1, k, k), &dev).unwrap();
            let fast = depthwise(&x, &w, pad, stride).unwrap();
            let slow = x.conv2d(&w, pad, stride, 1, c).unwrap();
            assert_eq!(fast.dims(), slow.dims(), "k={k} s={stride} p={pad}");
            let d = (fast - slow).unwrap().abs().unwrap().flatten_all().unwrap().max(0).unwrap().to_scalar::<f32>().unwrap();
            assert!(d < 1e-4, "k={k} s={stride} p={pad}: {d}");
        }
    }

    #[test]
    fn host_shape_arithmetic_and_slices() {
        let h = |dims: Vec<usize>, data: Vec<f64>| Val::Host(Host { dims, data });
        // the pad list the network builds: reverse the rows of [[2,0],[1,0],[2,0],[1,0]] flattened
        let x = h(vec![4, 2], vec![2.0, 0.0, 1.0, 0.0, 2.0, 0.0, 1.0, 0.0]);
        let Val::Host(r) = slice(&x, &[-1], &[i64::MIN + 1], &[0], &[-1]).unwrap() else { panic!() };
        assert_eq!(r.data, vec![1.0, 0.0, 2.0, 0.0, 1.0, 0.0, 2.0, 0.0]);
        assert_eq!(r.dims, vec![4, 2]);
        assert_eq!(round_even(0.5), 0.0);
        assert_eq!(round_even(1.5), 2.0);
        assert_eq!(round_even(-2.5), -2.0);
        assert_eq!(round_even(2.4), 2.0);
    }
}
