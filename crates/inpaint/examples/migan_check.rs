//! `migan_check <model dir> <image.jpg> <out.raw> [cpu]`: runs the model on the crop of `image` at
//! (700, 400), 512 × 512, with a hole at rows 200..300, columns 220..330 (the case the reference
//! output was made with by onnxruntime), writes the planar result (CHW, u8) and prints timings.
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: migan_check <model dir> <image.jpg> <out.raw> [cpu]");
        return;
    }
    let dev = if a.get(4).is_some_and(|s| s == "cpu") { candle_core::Device::Cpu } else { lightcraft_inpaint::best_device() };
    let t = Instant::now();
    let m = lightcraft_inpaint::Inpainter::load_on(std::path::Path::new(&a[1]), dev).expect("load");
    println!("load {:.2}s on {}", t.elapsed().as_secs_f64(), if m.device().is_metal() { "metal" } else { "cpu" });
    let img = image::open(&a[2]).expect("open").to_rgb8();
    let crop = image::imageops::crop_imm(&img, 700, 400, 512, 512).to_image();
    let mut known = vec![255u8; 512 * 512];
    for y in 200..300 {
        for x in 220..330 {
            known[y * 512 + x] = 0;
        }
    }
    for run in 0..4 {
        let t = Instant::now();
        let out = m.run_square(crop.as_raw(), &known).expect("run");
        println!("run {run}: {:.3}s", t.elapsed().as_secs_f64());
        if run == 0 {
            let mut planar = vec![0u8; 3 * 512 * 512];
            for i in 0..512 * 512 {
                for c in 0..3 {
                    planar[c * 512 * 512 + i] = out[i * 3 + c];
                }
            }
            std::fs::write(&a[3], planar).expect("write");
        }
    }
}
