//! Developer tool: time the building blocks of the restoration networks on the CPU and on Metal
//! (which convolution shape is slow, what f16 buys), to choose and tune a faster denoiser.
//!
//! `op_bench [cpu|metal]`
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use candle_core::{DType, Device, Tensor};

fn time(name: &str, flops: f64, dev: &Device, mut f: impl FnMut() -> Tensor) {
    let sync = |t: &Tensor| {
        let _ = t
            .flatten_all()
            .unwrap()
            .narrow(0, 0, 1)
            .unwrap()
            .to_vec1::<f32>()
            .or_else(|_| t.flatten_all().unwrap().narrow(0, 0, 1).unwrap().to_dtype(DType::F32).unwrap().to_vec1::<f32>());
    };
    let t = f();
    sync(&t); // warm up (kernel compilation)
    let n = 3;
    let t0 = Instant::now();
    for _ in 0..n {
        let t = f();
        sync(&t);
    }
    let s = t0.elapsed().as_secs_f64() / n as f64;
    println!("{:<44} {:>8.1} ms  {:>7.2} GFLOP/s  ({})", name, s * 1e3, flops / s / 1e9, if dev.is_cpu() { "cpu" } else { "metal" });
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "metal".into());
    let dev = if which == "cpu" { Device::Cpu } else { Device::new_metal(0).unwrap() };
    for dt in [DType::F32, DType::F16] {
        let tag = if dt == DType::F32 { "f32" } else { "f16" };
        let hw = 512usize;
        for (c, o, k) in [(64usize, 64usize, 3usize), (64, 64, 1), (128, 128, 1), (64, 128, 1)] {
            let x = Tensor::randn(0f32, 1f32, (1, c, hw, hw), &dev).unwrap().to_dtype(dt).unwrap();
            let w = Tensor::randn(0f32, 0.1f32, (o, c, k, k), &dev).unwrap().to_dtype(dt).unwrap();
            let flops = 2.0 * (hw * hw) as f64 * (c * o * k * k) as f64;
            time(&format!("{tag} conv{k}x{k} {c}->{o} @{hw}"), flops, &dev, || x.conv2d(&w, k / 2, 1, 1, 1).unwrap());
            if k == 1 {
                // the same as a matrix product over pixels
                let xm = x.permute((0, 2, 3, 1)).unwrap().reshape((hw * hw, c)).unwrap().contiguous().unwrap();
                let wm = w.reshape((o, c)).unwrap().t().unwrap().contiguous().unwrap();
                time(&format!("{tag} matmul as 1x1 {c}->{o} @{hw}"), flops, &dev, || xm.matmul(&wm).unwrap());
            }
        }
        // depthwise 3x3
        for c in [64usize, 128] {
            let x = Tensor::randn(0f32, 1f32, (1, c, hw, hw), &dev).unwrap().to_dtype(dt).unwrap();
            let w = Tensor::randn(0f32, 0.1f32, (c, 1, 3, 3), &dev).unwrap().to_dtype(dt).unwrap();
            let flops = 2.0 * (hw * hw) as f64 * (c * 9) as f64;
            time(&format!("{tag} depthwise3x3 {c} @{hw}"), flops, &dev, || x.conv2d(&w, 1, 1, 1, c).unwrap());
        }
        // depthwise 3x3 as nine shifted multiply-adds on NHWC
        for c in [64usize, 128] {
            let x = Tensor::randn(0f32, 1f32, (1, hw, hw, c), &dev).unwrap().to_dtype(dt).unwrap();
            let w9: Vec<Tensor> = (0..9).map(|_| Tensor::randn(0f32, 0.1f32, (1, 1, 1, c), &dev).unwrap().to_dtype(dt).unwrap()).collect();
            let flops = 2.0 * (hw * hw) as f64 * (c * 9) as f64;
            time(&format!("{tag} depthwise3x3 shifted-adds {c} @{hw}"), flops, &dev, || {
                let p = x.pad_with_zeros(1, 1, 1).unwrap().pad_with_zeros(2, 1, 1).unwrap();
                let mut acc: Option<Tensor> = None;
                for dy in 0..3 {
                    for dx in 0..3 {
                        let t = p.narrow(1, dy, hw).unwrap().narrow(2, dx, hw).unwrap().broadcast_mul(&w9[dy * 3 + dx]).unwrap();
                        acc = Some(match acc {
                            Some(a) => (a + t).unwrap(),
                            None => t,
                        });
                    }
                }
                acc.unwrap()
            });
        }
        // depthwise 3x3 on a flat zero-padded [(H+2)(W+2), C] buffer: nine contiguous row slices
        for c in [64usize, 128] {
            let x = Tensor::randn(0f32, 1f32, (hw, hw, c), &dev).unwrap().to_dtype(dt).unwrap();
            let w9: Vec<Tensor> = (0..9).map(|_| Tensor::randn(0f32, 0.1f32, (1, c), &dev).unwrap().to_dtype(dt).unwrap()).collect();
            let flops = 2.0 * (hw * hw) as f64 * (c * 9) as f64;
            time(&format!("{tag} depthwise3x3 flat-slices {c} @{hw}"), flops, &dev, || {
                let p = x.pad_with_zeros(0, 1, 1).unwrap().pad_with_zeros(1, 1, 1).unwrap().reshape(((hw + 2) * (hw + 2), c)).unwrap();
                let z = Tensor::zeros((2, c), dt, &dev).unwrap();
                let flat = Tensor::cat(&[&p, &z], 0).unwrap();
                let n = hw * (hw + 2);
                let mut acc: Option<Tensor> = None;
                for k in 0..9 {
                    let t = flat.narrow(0, (k / 3) * (hw + 2) + k % 3, n).unwrap().broadcast_mul(&w9[k]).unwrap();
                    acc = Some(match acc {
                        Some(a) => (a + t).unwrap(),
                        None => t,
                    });
                }
                acc.unwrap().reshape((hw, hw + 2, c)).unwrap().narrow(1, 0, hw).unwrap().contiguous().unwrap()
            });
        }
        // dense 3x3 as one matrix product over nine shifted copies (im2col by concatenation), NHWC
        for (c, o) in [(64usize, 64usize), (128, 128)] {
            let x = Tensor::randn(0f32, 1f32, (1, hw, hw, c), &dev).unwrap().to_dtype(dt).unwrap();
            let wm = Tensor::randn(0f32, 0.1f32, (9 * c, o), &dev).unwrap().to_dtype(dt).unwrap();
            let flops = 2.0 * (hw * hw) as f64 * (c * o * 9) as f64;
            time(&format!("{tag} conv3x3 im2col-matmul {c}->{o} @{hw}"), flops, &dev, || {
                let p = x.pad_with_zeros(1, 1, 1).unwrap().pad_with_zeros(2, 1, 1).unwrap();
                let cols: Vec<Tensor> = (0..9).map(|k| p.narrow(1, k / 3, hw).unwrap().narrow(2, k % 3, hw).unwrap()).collect();
                let cat = Tensor::cat(&cols, 3).unwrap().reshape((hw * hw, 9 * c)).unwrap();
                cat.matmul(&wm).unwrap()
            });
            time(&format!("{tag} conv3x3 9 accumulated matmuls {c}->{o} @{hw}"), flops, &dev, || {
                let p = x.pad_with_zeros(1, 1, 1).unwrap().pad_with_zeros(2, 1, 1).unwrap();
                let mut acc: Option<Tensor> = None;
                for k in 0..9 {
                    let t = p.narrow(1, k / 3, hw).unwrap().narrow(2, k % 3, hw).unwrap().contiguous().unwrap().reshape((hw * hw, c)).unwrap();
                    let y = t.matmul(&wm.narrow(0, k * c, c).unwrap()).unwrap();
                    acc = Some(match acc {
                        Some(a) => (a + y).unwrap(),
                        None => y,
                    });
                }
                acc.unwrap()
            });
        }
        // batched small matmuls (window attention): 4096 windows of 64 tokens, 32-dim heads
        let q = Tensor::randn(0f32, 1f32, (4096 * 2, 64, 32), &dev).unwrap().to_dtype(dt).unwrap();
        let kt = Tensor::randn(0f32, 1f32, (4096 * 2, 32, 64), &dev).unwrap().to_dtype(dt).unwrap();
        time(&format!("{tag} bmm window attn (8192 x 64x32x64)"), 2.0 * 8192.0 * 64.0 * 32.0 * 64.0, &dev, || q.matmul(&kt).unwrap());
    }
}
