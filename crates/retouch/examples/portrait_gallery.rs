//! Contact sheet and timings of the portrait steps on the synthetic portrait:
//! `cargo run --release -p lightcraft-retouch --example portrait_gallery -- out.png [scale]`.
//! Each row is one slider at 80 %: the original and the result, side by side, cropped on the
//! part of the face the slider works on. `scale` (default 1) enlarges the portrait (a nearest
//! neighbour upscale) to time bigger faces.

use std::time::Instant;

use lightcraft_retouch::portrait::fixture::{W, mesh, portrait};
use lightcraft_retouch::portrait::{Crop, KINDS, Kind, retouch_face};

fn upscale(rgb: &[u8], k: usize) -> (Vec<u8>, usize) {
    let n = W * k;
    let mut out = vec![0u8; n * n * 3];
    for y in 0..n {
        for x in 0..n {
            let s = ((y / k) * W + x / k) * 3;
            out[(y * n + x) * 3..(y * n + x) * 3 + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (out, n)
}

fn region(kind: Kind) -> [usize; 4] {
    // x0, y0, x1, y1 in the 400 px portrait
    match kind {
        Kind::Teeth => [150, 235, 250, 285],
        Kind::Eyes | Kind::DarkCircles | Kind::WrinklesEyes => [110, 135, 290, 200],
        Kind::WrinklesForehead | Kind::Shine => [130, 70, 270, 135],
        Kind::Flyaways => [110, 10, 290, 80],
        Kind::NeckSmooth | Kind::NeckLight | Kind::DoubleChin => [100, 300, 300, 400],
        _ => [120, 180, 290, 250],
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out_path = args.next().unwrap_or_else(|| "portrait-gallery.png".into());
    let k: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1).clamp(1, 8);
    let (rgb0, _) = portrait(true);
    let (rgb, n) = upscale(&rgb0, k);
    let pts: Vec<[f32; 3]> = mesh(true).iter().map(|p| [p[0] * k as f32, p[1] * k as f32, p[2] * k as f32]).collect();
    let mut rows: Vec<(String, Vec<u8>, Vec<u8>, [usize; 4], f64)> = Vec::new();
    let mut total = 0.0;
    for info in KINDS {
        let mut out = rgb.clone();
        let t = Instant::now();
        let r = retouch_face(Crop { w: n, h: n, rgb: &mut out }, &pts, &[(info.kind, 0.8)]);
        let secs = t.elapsed().as_secs_f64();
        total += secs;
        println!(
            "{:<18} {:>7.3} s  {}",
            info.id,
            secs,
            r.map_or("no face".to_string(), |r| format!("changed {:.1} %", r.reports[0].changed * 100.0))
        );
        rows.push((info.id.to_string(), rgb.clone(), out, region(info.kind), secs));
    }
    let mut all = rgb.clone();
    let t = Instant::now();
    let steps: Vec<(Kind, f32)> = KINDS.iter().filter(|i| i.auto > 0.0).map(|i| (i.kind, i.auto)).collect();
    let _ = retouch_face(Crop { w: n, h: n, rgb: &mut all }, &pts, &steps);
    println!("auto recipe ({} steps) at {n}x{n}: {:.3} s (sum of single steps {total:.3} s)", steps.len(), t.elapsed().as_secs_f64());
    // contact sheet: each row = original | retouched, zoomed x2 on the region of interest
    let zoom = 2usize;
    let cell_w = 200 * zoom;
    let cell_h = 100 * zoom;
    let mut sheet = image::RgbImage::from_pixel((cell_w * 2 + 12) as u32, ((cell_h + 6) * (rows.len() + 1)) as u32, image::Rgb([30, 30, 30]));
    let mut put = |sheet: &mut image::RgbImage, src: &[u8], reg: [usize; 4], ox: usize, oy: usize| {
        for y in 0..cell_h {
            for x in 0..cell_w {
                let (sx, sy) = ((reg[0] + x / zoom).min(W - 1) * k, (reg[1] + y / zoom).min(W - 1) * k);
                let i = (sy * n + sx) * 3;
                sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb([src[i], src[i + 1], src[i + 2]]));
            }
        }
    };
    for (r, (_, a, b, reg, _)) in rows.iter().enumerate() {
        put(&mut sheet, a, *reg, 0, r * (cell_h + 6));
        put(&mut sheet, b, *reg, cell_w + 12, r * (cell_h + 6));
    }
    let last = rows.len();
    put(&mut sheet, &rgb, [100, 60, 300, 160], 0, last * (cell_h + 6));
    put(&mut sheet, &all, [100, 60, 300, 160], cell_w + 12, last * (cell_h + 6));
    sheet.save(&out_path).expect("write the sheet");
    println!("wrote {out_path} ({} rows: {})", rows.len() + 1, rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", "));
}
