//! `depth_probe <model dir> <image> <out.png> [size]`: depth of one photo, timed.
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: depth_probe <model dir> <image> <out.png> [size] [cpu]");
        return;
    }
    let size = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(lightcraft_depth::DEFAULT_SIZE);
    let dev = if a.get(5).map(|s| s == "cpu").unwrap_or(false) { candle_core::Device::Cpu } else { lightcraft_depth::best_device() };
    let t = Instant::now();
    let m = lightcraft_depth::DepthModel::load_on(std::path::Path::new(&a[1]), dev).expect("load");
    println!("load {:.2}s", t.elapsed().as_secs_f64());
    let img = image::open(&a[2]).expect("open").to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    for run in 0..3 {
        let t = Instant::now();
        let d = m.estimate(img.as_raw(), w, h, size).expect("estimate");
        lightcraft_depth::sync(&m).ok();
        println!("run {run}: {}x{} -> {}x{} in {:.2}s", w, h, d.width, d.height, t.elapsed().as_secs_f64());
        if run == 2 {
            let (lo, hi) = d.data.iter().fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
            println!("range {lo:.3}..{hi:.3}");
            let px: Vec<u8> = d.data.iter().map(|v| (((v - lo) / (hi - lo).max(1e-6)) * 255.0) as u8).collect();
            image::GrayImage::from_raw(d.width as u32, d.height as u32, px).expect("img").save(&a[3]).expect("save");
        }
    }
}
