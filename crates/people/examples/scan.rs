//! Developer tool: find, align and embed the faces of image files; writes the aligned crops as
//! PNG and one JSON line per face (box, landmarks, embedding head) — used to check the
//! interpreter against ONNX Runtime and to measure speed.
//!
//! `LIGHTCRAFT_FACE_LANDMARKER=…/face_landmarker.task LIGHTCRAFT_ARCFACE=…/w600k_r50.onnx cargo run
//! --release -p lightcraft-people --example scan -- [--edge 1600] [--out DIR] [--full] a.jpg …`
//! `--full` prints the whole embedding.

use image::{ImageDecoder, ImageReader};
use lightcraft_faces::{FaceLandmarker, mediapipe::Frame};
use lightcraft_people::{align, detect, recog::Recognizer};

fn main() {
    let (mut edge, mut out, mut full, mut files) = (1600u32, None::<String>, false, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--edge" => edge = args.next().and_then(|v| v.parse().ok()).unwrap_or(edge),
            "--out" => out = args.next(),
            "--full" => full = true,
            _ => files.push(a),
        }
    }
    let task = std::env::var("LIGHTCRAFT_FACE_LANDMARKER").expect("LIGHTCRAFT_FACE_LANDMARKER");
    let onnx = std::env::var("LIGHTCRAFT_ARCFACE").expect("LIGHTCRAFT_ARCFACE");
    let lm = FaceLandmarker::from_task(&std::fs::read(task).expect("task")).expect("landmarker");
    let t = std::time::Instant::now();
    let rec = Recognizer::load(std::path::Path::new(&onnx)).expect("recognizer");
    eprintln!("model loaded in {:.2}s", t.elapsed().as_secs_f64());
    for f in files {
        let t0 = std::time::Instant::now();
        let load = || -> Result<image::DynamicImage, String> {
            let mut d = ImageReader::open(&f)
                .map_err(|e| e.to_string())?
                .with_guessed_format()
                .map_err(|e| e.to_string())?
                .into_decoder()
                .map_err(|e| e.to_string())?;
            let o = d.orientation().map_err(|e| e.to_string())?;
            let mut img = image::DynamicImage::from_decoder(d).map_err(|e| e.to_string())?;
            img.apply_orientation(o);
            Ok(img)
        };
        let img = match load() {
            Ok(i) => i,
            Err(e) => {
                println!("{}", serde_json::json!({"file": f, "error": e}));
                continue;
            }
        };
        let img = if img.width().max(img.height()) > edge {
            let s = edge as f32 / img.width().max(img.height()) as f32;
            img.resize(
                ((img.width() as f32 * s).round() as u32).max(1),
                ((img.height() as f32 * s).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };
        let rgb8 = img.to_rgb8();
        let (w, h) = (rgb8.width() as usize, rgb8.height() as usize);
        let px: Vec<[f32; 3]> = rgb8.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
        let t_load = t0.elapsed().as_secs_f64();
        let t1 = std::time::Instant::now();
        let faces = match detect::faces_of(&lm, Frame { rgb: &px, w, h }) {
            Ok(v) => v,
            Err(e) => {
                println!("{}", serde_json::json!({"file": f, "error": e.to_string()}));
                continue;
            }
        };
        let t_det = t1.elapsed().as_secs_f64();
        let crops: Vec<Vec<f32>> = faces.iter().map(|d| align::crop(Frame { rgb: &px, w, h }, &d.landmarks)).collect();
        let t2 = std::time::Instant::now();
        let embs = rec.embed(&crops).expect("embed");
        let t_emb = t2.elapsed().as_secs_f64();
        for (i, ((d, c), e)) in faces.iter().zip(&crops).zip(&embs).enumerate() {
            if let Some(dir) = &out {
                let _ = std::fs::create_dir_all(dir);
                let n = align::SIDE * align::SIDE;
                let mut im = image::RgbImage::new(align::SIDE as u32, align::SIDE as u32);
                for (k, p) in im.pixels_mut().enumerate() {
                    *p = image::Rgb([0, 1, 2].map(|ch| ((c[ch * n + k] + 1.0) * 127.5).round().clamp(0.0, 255.0) as u8));
                }
                let stem = std::path::Path::new(&f).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = im.save(format!("{dir}/{stem}_{i}.png"));
                // the crop as raw floats, for ONNX Runtime
                let bytes: Vec<u8> = c.iter().flat_map(|v| v.to_le_bytes()).collect();
                let _ = std::fs::write(format!("{dir}/{stem}_{i}.f32"), bytes);
            }
            println!(
                "{}",
                serde_json::json!({"file": f, "i": i, "bbox": d.bbox, "lm": d.landmarks, "det": d.det, "size": d.size, "turn": d.turn,
                   "emb": if full { serde_json::json!(e) } else { serde_json::json!(&e[..6]) }, "tLoad": t_load, "tDet": t_det, "tEmb": t_emb / embs.len() as f64, "w": w, "h": h})
            );
        }
        if faces.is_empty() {
            println!("{}", serde_json::json!({"file": f, "faces": 0, "tLoad": t_load, "tDet": t_det}));
        }
    }
}
