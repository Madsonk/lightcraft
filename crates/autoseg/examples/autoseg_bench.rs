//! Developer tool: run SegFormer / the matting network on an image file, time it, save the masks.
//!
//! `autoseg_bench segformer <model dir> in.jpg out_prefix` writes `<prefix>_sky.png`, `<prefix>_person.png`
//! `autoseg_bench subject <model dir> in.jpg out.png`
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::Instant;

use lightcraft_autoseg::{isnet, segformer};

fn save_gray(p: &str, w: u32, h: u32, v: &[f32]) {
    let raw: Vec<u8> = v.iter().map(|x| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
    image::GrayImage::from_raw(w, h, raw).unwrap().save(p).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[3]).unwrap().to_rgb8();
    match a[1].as_str() {
        "segformer" => {
            // short side 512, sides multiples of 32
            let (w, h) = img.dimensions();
            let s = 512.0 / w.min(h) as f32;
            let (tw, th) = ((((w as f32 * s) / 32.0).round() as u32).max(1) * 32, (((h as f32 * s) / 32.0).round() as u32).max(1) * 32);
            let r = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
            let t = Instant::now();
            let m = segformer::SegFormer::load(Path::new(&a[2])).unwrap();
            println!("load {:.2}s device {:?}", t.elapsed().as_secs_f64(), m.device().is_cpu());
            let t = Instant::now();
            let (l, gw, gh) = m.forward(r.as_raw(), tw as usize, th as usize).unwrap();
            println!("forward {:.2}s grid {gw}x{gh}", t.elapsed().as_secs_f64());
            let t = Instant::now();
            let _ = m.forward(r.as_raw(), tw as usize, th as usize).unwrap();
            println!("forward (warm) {:.2}s", t.elapsed().as_secs_f64());
            for (name, cls) in [("sky", vec![segformer::class::SKY]), ("person", vec![segformer::class::PERSON])] {
                let lg = segformer::class_logit(&l, gw * gh, &cls);
                let p: Vec<f32> = lg.iter().map(|x| 1.0 / (1.0 + (-x).exp())).collect();
                let cov = p.iter().filter(|v| **v > 0.5).count() as f32 / p.len() as f32;
                println!("{name}: coverage {:.1}%", cov * 100.0);
                save_gray(&format!("{}_{name}.png", a[4]), gw as u32, gh as u32, &p);
            }
            // top classes
            let cells = gw * gh;
            let mut hist = vec![0usize; segformer::CLASSES];
            for i in 0..cells {
                let (mut b, mut bv) = (0, f32::MIN);
                for c in 0..segformer::CLASSES {
                    if l[c * cells + i] > bv {
                        bv = l[c * cells + i];
                        b = c;
                    }
                }
                hist[b] += 1;
            }
            let mut top: Vec<(usize, usize)> = hist.into_iter().enumerate().collect();
            top.sort_by_key(|x| std::cmp::Reverse(x.1));
            println!("top classes: {:?}", &top[..5]);
        }
        "subject" => {
            let r = image::imageops::resize(&img, isnet::SIDE as u32, isnet::SIDE as u32, image::imageops::FilterType::Triangle);
            r.save(format!("{}.in.png", a[4])).unwrap();
            let t = Instant::now();
            let m = isnet::IsNet::load(Path::new(&a[2])).unwrap();
            println!("load {:.2}s cpu={}", t.elapsed().as_secs_f64(), m.device().is_cpu());
            let t = Instant::now();
            let p = m.matte(r.as_raw()).unwrap();
            println!("matte {:.2}s", t.elapsed().as_secs_f64());
            let cov = p.iter().filter(|v| **v > 0.5).count() as f32 / p.len() as f32;
            println!("foreground {:.1}%", cov * 100.0);
            save_gray(&a[4], isnet::SIDE as u32, isnet::SIDE as u32, &p);
        }
        _ => panic!("segformer|subject"),
    }
}
