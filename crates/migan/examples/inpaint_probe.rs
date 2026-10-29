//! `inpaint_probe <model dir> <image> <out.png> <shape>…` with shapes `e:cx,cy,rx,ry` (ellipse),
//! `r:x0,y0,x1,y1` (rectangle), in pixels. Writes `[selection | result | crop of the fill]`.
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: inpaint_probe <model dir> <image> <out.png> e:cx,cy,rx,ry|r:x0,y0,x1,y1 …");
        return;
    }
    let t = Instant::now();
    let m = lightcraft_migan::Inpainter::load(std::path::Path::new(&a[1])).expect("load");
    println!("load {:.2}s", t.elapsed().as_secs_f64());
    let img = image::open(&a[2]).expect("open").to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut mask = vec![0u8; w * h];
    for s in &a[4..] {
        let (k, v) = s.split_once(':').expect("shape");
        let n: Vec<f32> = v.split(',').map(|x| x.parse().expect("num")).collect();
        for y in 0..h {
            for x in 0..w {
                let inside = match k {
                    "e" => ((x as f32 - n[0]) / n[2]).powi(2) + ((y as f32 - n[1]) / n[3]).powi(2) <= 1.0,
                    _ => x as f32 >= n[0] && x as f32 <= n[2] && y as f32 >= n[1] && y as f32 <= n[3],
                };
                if inside {
                    mask[y * w + x] = 255;
                }
            }
        }
    }
    let mut last = None;
    for run in 0..2 {
        let t = Instant::now();
        let f = m.fill(img.as_raw(), w, h, &mask, &Default::default()).expect("fill");
        println!("fill {run}: crop {}x{} at ({}, {}) in {:.2}s", f.width, f.height, f.x, f.y, t.elapsed().as_secs_f64());
        last = Some(f);
    }
    let f = last.expect("fill");
    let mut out = img.clone();
    let mut sel = img.clone();
    for y in 0..f.height {
        for x in 0..f.width {
            let (px, py) = (f.x + x, f.y + y);
            let al = f32::from(f.alpha[y * f.width + x]) / 255.0;
            let o = out.get_pixel_mut(px as u32, py as u32);
            for c in 0..3 {
                o[c] = (f32::from(f.rgb[(y * f.width + x) * 3 + c]) * al + f32::from(o[c]) * (1.0 - al)).round() as u8;
            }
        }
    }
    for (i, mv) in mask.iter().enumerate() {
        if *mv > 0 {
            let p = sel.get_pixel_mut((i % w) as u32, (i / w) as u32);
            p[0] = (p[0] / 2).saturating_add(120);
            p[1] /= 2;
            p[2] /= 2;
        }
    }
    // side by side, cropped around the fill (at most 900 px wide each)
    let crop = |im: &image::RgbImage| image::imageops::crop_imm(im, f.x as u32, f.y as u32, f.width as u32, f.height as u32).to_image();
    let (c0, c1) = (crop(&sel), crop(&out));
    let mut sheet = image::RgbImage::new(c0.width() * 2, c0.height());
    image::imageops::replace(&mut sheet, &c0, 0, 0);
    image::imageops::replace(&mut sheet, &c1, c0.width() as i64, 0);
    sheet.save(&a[3]).expect("save");
    out.save(a[3].replace(".png", "_full.png")).ok();
}
