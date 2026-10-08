//! Developer tool: run the face analysis on image files and print one JSON line per file.
//!
//! `LIGHTCRAFT_FACE_LANDMARKER=/path/face_landmarker.task cargo run --release -p lightcraft-faces
//! --example scan -- [--edge 2560] a.jpg b.jpg …` — used to calibrate the thresholds in
//! `lightcraft-catalog` (`EYE_CLOSED_BELOW` …) on real photos. Raw per-eye numbers, in pixels.

use image::{ImageDecoder, ImageReader};
use lightcraft_faces::{FaceLandmarker, analyze, mediapipe::Frame};

fn main() {
    let mut edge = 2560u32;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--edge" {
            edge = args.next().and_then(|v| v.parse().ok()).unwrap_or(edge);
        } else {
            files.push(a);
        }
    }
    let Some(task) = std::env::var_os("LIGHTCRAFT_FACE_LANDMARKER") else { return eprintln!("set LIGHTCRAFT_FACE_LANDMARKER to face_landmarker.task") };
    let Ok(bytes) = std::fs::read(task) else { return eprintln!("can't read the model") };
    let model = match FaceLandmarker::from_task(&bytes) {
        Ok(m) => m,
        Err(e) => return eprintln!("{e}"),
    };
    for f in files {
        let t = std::time::Instant::now();
        let load = || -> Result<image::DynamicImage, String> {
            let mut d = ImageReader::open(&f).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?.into_decoder().map_err(|e| e.to_string())?;
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
            img.resize(((img.width() as f32 * s).round() as u32).max(1), ((img.height() as f32 * s).round() as u32).max(1), image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgb8 = img.to_rgb8();
        let (w, h) = (rgb8.width() as usize, rgb8.height() as usize);
        let px: Vec<[f32; 3]> = rgb8.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
        let loaded = t.elapsed();
        let t = std::time::Instant::now();
        match analyze(&model, Frame { rgb: &px, w, h }) {
            Ok(faces) => {
                let faces: Vec<_> = faces
                    .iter()
                    .map(|f| {
                        serde_json::json!({
                            "bbox": f.bbox, "score": f.score, "turn": f.turn, "det": f.detection, "prop": f.proportion, "drift": f.drift, "agree": f.agreement, "votes": f.votes,
                            "earL": f.left.open, "earR": f.right.open, "sharpL": f.left.sharpness, "sharpR": f.right.sharpness, "detL": f.left.detail, "detR": f.right.detail,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({"file": f, "w": w, "h": h, "faces": faces, "ms": t.elapsed().as_secs_f64() * 1000.0, "loadMs": loaded.as_secs_f64() * 1000.0})
                );
            }
            Err(e) => println!("{}", serde_json::json!({"file": f, "error": e})),
        }
    }
}
