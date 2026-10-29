//! Developer tool: run the denoiser or the upscaler on an image file and time it.
//!
//! `restore_bench denoise <weights.pth> in.png out.png [strength 0..1]`
//! `restore_bench upscale <weights.safetensors> in.png out.png [norm: none]`
//! `restore_bench noise in.png out.png <sigma 0..1> [seed]`  (adds Gaussian noise, for tests)
//! `restore_bench noisepg in.png out.png <shot a> <read b> [seed]`  (Poisson-Gaussian noise in linear light: variance a*x + b^2)
//! `restore_bench psnr a.png b.png`  (PSNR and SSIM of two images of the same size)
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::Instant;

use lightcraft_restore::{Denoiser, Image, Upscaler, blend, span};

fn load(p: &str) -> Image {
    let im = image::open(p).unwrap().to_rgb8();
    let (w, h) = im.dimensions();
    Image { width: w as usize, height: h as usize, data: im.as_raw().iter().map(|&v| v as f32 / 255.0).collect() }
}

fn save(p: &str, im: &Image) {
    let raw: Vec<u8> = im.data.iter().map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
    image::RgbImage::from_raw(im.width as u32, im.height as u32, raw).unwrap().save(p).unwrap();
}

fn psnr(a: &Image, b: &Image) -> f64 {
    let mse = a.data.iter().zip(&b.data).map(|(x, y)| ((x - y) as f64 * 255.0).powi(2)).sum::<f64>() / a.data.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-12)).log10()
}

/// Mean SSIM over the luma channel, 8 x 8 uniform windows with stride 4 (Wang et al. constants).
fn ssim(a: &Image, b: &Image) -> f64 {
    let luma = |im: &Image| -> Vec<f64> { im.data.chunks(3).map(|p| (0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]) as f64 * 255.0).collect() };
    let (la, lb) = (luma(a), luma(b));
    let (w, h) = (a.width, a.height);
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (mut sum, mut n) = (0.0, 0usize);
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut ma, mut mb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    let (p, q) = (la[(y + j) * w + x + i], lb[(y + j) * w + x + i]);
                    ma += p;
                    mb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let k = 64.0;
            let (ma, mb) = (ma / k, mb / k);
            let (va, vb, cov) = (saa / k - ma * ma, sbb / k - mb * mb, sab / k - ma * mb);
            sum += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
            x += 4;
        }
        y += 4;
    }
    sum / n.max(1) as f64
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let progress = |d: usize, t: usize| {
        eprint!("\rtile {d}/{t}");
        true
    };
    match a.get(1).map(String::as_str) {
        Some("denoise") => {
            let t = Instant::now();
            let net = Denoiser::load(Path::new(&a[2])).unwrap();
            eprintln!("loaded in {:.2}s on {:?}", t.elapsed().as_secs_f64(), net.device());
            let img = load(&a[3]);
            let t = Instant::now();
            let mut out = net.denoise(&img, &mut { progress }).unwrap();
            eprintln!("\n{}x{} denoised in {:.2}s", img.width, img.height, t.elapsed().as_secs_f64());
            if let Some(s) = a.get(5) {
                blend(&img, &mut out, s.parse().unwrap()).unwrap();
            }
            save(&a[4], &out);
        }
        Some("upscale") => {
            let norm = match a.get(5).map(String::as_str) {
                Some("none") => span::Norm::None,
                _ => span::Norm::REFERENCE,
            };
            let net = Upscaler::load_with(Path::new(&a[2]), norm).unwrap();
            let img = load(&a[3]);
            let t = Instant::now();
            let out = net.upscale(&img, &mut { progress }).unwrap();
            eprintln!("\n{}x{} -> {}x{} in {:.2}s on {:?}", img.width, img.height, out.width, out.height, t.elapsed().as_secs_f64(), net.device());
            save(&a[4], &out);
        }
        Some("noise") => {
            let mut img = load(&a[2]);
            let sigma: f32 = a[4].parse().unwrap();
            let mut s: u64 = a.get(5).map(|v| v.parse().unwrap()).unwrap_or(1);
            let mut rnd = || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f32 / (1u64 << 53) as f32
            };
            for v in img.data.iter_mut() {
                let g = (-2.0 * rnd().max(1e-9).ln()).sqrt() * (2.0 * std::f32::consts::PI * rnd()).cos();
                *v = (*v + sigma * g).clamp(0.0, 1.0);
            }
            save(&a[3], &img);
        }
        Some("noisepg") => {
            let mut img = load(&a[2]);
            let (sa, sb): (f32, f32) = (a[4].parse().unwrap(), a[5].parse().unwrap());
            let mut s: u64 = a.get(6).map(|v| v.parse().unwrap()).unwrap_or(1);
            let mut rnd = || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f32 / (1u64 << 53) as f32
            };
            let to_lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            let to_gam = |v: f32| if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            for v in img.data.iter_mut() {
                let g = (-2.0 * rnd().max(1e-9).ln()).sqrt() * (2.0 * std::f32::consts::PI * rnd()).cos();
                let l = to_lin(*v);
                *v = to_gam((l + (sa * l + sb * sb).sqrt() * g).clamp(0.0, 1.0));
            }
            save(&a[3], &img);
        }
        Some("psnr") => {
            let (x, y) = (load(&a[2]), load(&a[3]));
            println!("PSNR {:.2} dB  SSIM {:.4}", psnr(&x, &y), ssim(&x, &y));
        }
        _ => eprintln!("usage: see the file header"),
    }
}
