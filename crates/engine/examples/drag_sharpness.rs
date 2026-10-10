//! Sharpness and cost of the loupe while a slider is dragged.
//!
//! `cargo run --release -p lightcraft-engine --example drag_sharpness -- photo.CR2 [EDGE]`
//!
//! The loupe shows a texture drawn at the display's real pixel size (`EDGE`, the long edge in device
//! pixels: 2560 on a Retina laptop). While a slider is dragged it asks for a *draft*. Older builds
//! rendered that draft at 60 % of the display size and stretched it (bilinear, like the GPU sampler);
//! this example measures that against a draft at the display size, and against the settled (full
//! quality) render, with the variance of the Laplacian of the luminance as the sharpness metric (higher =
//! sharper; the ratio to the settled render is what matters) and the warm-cache time of one drag step.
//!
//! `N=9` runs (minimum is reported), `OUT=dir` writes the three renders as PPM files.
use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::DevelopSettings;
use lightcraft_engine::media::develop;
use lightcraft_pipeline::{Quality, RenderRequest, StageCache};
use lightcraft_raster::Rgba8;

fn luma(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

/// Variance of the 4-neighbour Laplacian of the luminance (interior pixels).
fn laplacian_variance(img: &Rgba8) -> f64 {
    let (w, h) = (img.width, img.height);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let l: Vec<f64> = img.data.iter().map(|p| luma(*p)).collect();
    let (mut sum, mut sum2, mut n) = (0.0, 0.0, 0.0);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let v = l[i - 1] + l[i + 1] + l[i - w] + l[i + w] - 4.0 * l[i];
            sum += v;
            sum2 += v * v;
            n += 1.0;
        }
    }
    let mean = sum / n;
    sum2 / n - mean * mean
}

/// Bilinear resize to `w × h` (what the GPU does when a smaller texture is drawn into a bigger rect).
fn stretch(img: &Rgba8, w: usize, h: usize) -> Rgba8 {
    let mut out = img.clone();
    out.width = w;
    out.height = h;
    out.data = vec![[0, 0, 0, 255]; w * h];
    let (sx, sy) = (img.width as f64 / w as f64, img.height as f64 / h as f64);
    for y in 0..h {
        let fy = ((y as f64 + 0.5) * sy - 0.5).clamp(0.0, (img.height - 1) as f64);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(img.height - 1);
        for x in 0..w {
            let fx = ((x as f64 + 0.5) * sx - 0.5).clamp(0.0, (img.width - 1) as f64);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(img.width - 1);
            let mut px = [0u8; 4];
            for c in 0..4 {
                let a = f64::from(img.data[y0 * img.width + x0][c]) * (1.0 - tx) + f64::from(img.data[y0 * img.width + x1][c]) * tx;
                let b = f64::from(img.data[y1 * img.width + x0][c]) * (1.0 - tx) + f64::from(img.data[y1 * img.width + x1][c]) * tx;
                px[c] = (a * (1.0 - ty) + b * ty).round() as u8;
            }
            out.data[y * w + x] = px;
        }
    }
    out
}

fn ppm(img: &Rgba8, path: &std::path::Path) {
    let mut v = format!("P6\n{} {}\n255\n", img.width, img.height).into_bytes();
    for p in &img.data {
        v.extend_from_slice(&p[..3]);
    }
    let _ = std::fs::write(path, v);
}

fn best(n: usize, mut f: impl FnMut()) -> f64 {
    let mut b = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        f();
        b = b.min(t.elapsed().as_secs_f64() * 1e3);
    }
    b
}

fn settings() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.3;
    s.light.highlights = -40.0;
    s.light.shadows = 30.0;
    s.effects.clarity = 15.0;
    s.effects.texture = 10.0;
    s.detail.nr_luminance = 30.0;
    s.detail.sharpen_amount = 40.0;
    s
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: drag_sharpness photo [EDGE]");
        return;
    };
    let edge: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(2560);
    let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(7);
    let bytes = std::fs::read(path).expect("read");
    let (img, info) = lightcraft_engine::files::load_bytes(&bytes, 2560).expect("decode");
    println!("{path}: loupe source {}×{}, display edge {edge} px", img.width, img.height);
    let src = Arc::new(img);
    let gpu = lightcraft_engine::gpu::available();
    println!("gpu: {}", lightcraft_engine::gpu::adapter_name().filter(|_| gpu).unwrap_or_else(|| "none".into()));
    let s = settings();
    let full = RenderRequest::fit(edge, edge);
    let draft = |e: usize| RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(e, e) };
    let small = (edge as f64 * 0.6) as usize;
    for g in if gpu { vec![false, true] } else { vec![false] } {
        println!("--- {}", if g { "GPU" } else { "CPU" });
        let rest = develop(&src, &info, &s, &full, None, g).image;
        let old_small = develop(&src, &info, &s, &draft(small), None, g).image;
        let old = stretch(&old_small, rest.width, rest.height);
        let new = develop(&src, &info, &s, &draft(edge), None, g).image;
        let v_rest = laplacian_variance(&rest);
        for (name, im) in [("settled (full quality)", &rest), ("drag, old (draft 60% stretched)", &old), ("drag, new (draft at display size)", &new)]
        {
            let v = laplacian_variance(im);
            println!("{name:<36} {}×{}  laplacian variance {v:>9.2}  = {:>5.1} % of settled", im.width, im.height, 100.0 * v / v_rest);
        }
        let (mut mx, mut sum) = (0u8, 0u64);
        for (a, b) in rest.data.iter().zip(&new.data) {
            for c in 0..3 {
                let d = a[c].abs_diff(b[c]);
                mx = mx.max(d);
                sum += u64::from(d);
            }
        }
        println!("settled vs drag-new: max |Δ| {mx} LSB, mean |Δ| {:.4} LSB", sum as f64 / (rest.data.len() * 3) as f64);
        if let Ok(dir) = std::env::var("OUT") {
            let d = std::path::Path::new(&dir);
            let tag = if g { "gpu" } else { "cpu" };
            ppm(&rest, &d.join(format!("settled_{tag}.ppm")));
            ppm(&old, &d.join(format!("drag_old_{tag}.ppm")));
            ppm(&new, &d.join(format!("drag_new_{tag}.ppm")));
        }
        // one drag step with a warm stage cache (highlights slider), per render size
        let mut k = 0.0;
        for (name, req) in [("full  display size", full), ("draft 60 %", draft(small)), ("draft display size", draft(edge))] {
            let cache = StageCache::default();
            drop(develop(&src, &info, &s, &req, Some(&cache), g));
            let ms = best(n, || {
                let mut t = s.clone();
                k += 1.0;
                t.light.highlights = -40.0 + k % 50.0;
                drop(develop(&src, &info, &t, &req, Some(&cache), g));
            });
            println!("drag step highlights, {name:<20} {ms:>7.1} ms");
        }
        for (name, req) in [("full  display size", full), ("draft 60 %", draft(small)), ("draft display size", draft(edge))] {
            let cache = StageCache::default();
            drop(develop(&src, &info, &s, &req, Some(&cache), g));
            let ms = best(n, || {
                let mut t = s.clone();
                k += 1.0;
                t.light.exposure = 0.3 + (k % 50.0) * 0.01;
                drop(develop(&src, &info, &t, &req, Some(&cache), g));
            });
            println!("drag step exposure,   {name:<20} {ms:>7.1} ms");
        }
    }
}
