//! Quality and speed harness: embeds a list of images and runs queries against them.
//!
//! `eval MODEL_DIR IMAGES.txt QUERIES.txt OUT.json [f32|f16] [cpu]`
//! IMAGES.txt: one path per line. QUERIES.txt: one query per line. OUT.json gets, per query, the
//! 8 best paths with their cosine and probability, plus timings.
use std::time::Instant;

use candle_core::{DType, Device};
use lightcraft_search::{Embedder, VectorIndex, key_of, preprocess::preprocess};
use rayon::prelude::*;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let (dir, images, queries, out) = (&a[1], &a[2], &a[3], &a[4]);
    let dtype = if a.get(5).map(String::as_str) == Some("f16") { DType::F16 } else { DType::F32 };
    let device = if a.get(6).map(String::as_str) == Some("cpu") { Device::Cpu } else { lightcraft_search::best_device() };
    let t = Instant::now();
    let m = Embedder::load_on(std::path::Path::new(dir), device.clone(), dtype)?;
    let load_s = t.elapsed().as_secs_f64();
    let paths: Vec<String> = std::fs::read_to_string(images)?.lines().filter(|l| !l.is_empty()).map(String::from).collect();
    let limit: usize = std::env::var("LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let paths: Vec<String> = paths.into_iter().take(limit).collect();
    let res = m.resample();
    // decode + preprocess in parallel (as the engine does from its thumbnails)
    let t = Instant::now();
    let pre: Vec<Option<Vec<f32>>> = paths
        .par_iter()
        .map(|p| {
            let mut d = image::ImageReader::open(p).ok()?.with_guessed_format().ok()?.into_decoder().ok()?;
            let o = image::ImageDecoder::orientation(&mut d).ok()?;
            let mut im = image::DynamicImage::from_decoder(d).ok()?;
            im.apply_orientation(o);
            let im = im.to_rgb8();
            let (w, h) = im.dimensions();
            let s = 512.0 / w.max(h) as f32;
            let im = if s < 1.0 { image::imageops::thumbnail(&im, (w as f32 * s) as u32, (h as f32 * s) as u32) } else { im };
            preprocess(im.as_raw(), im.width() as usize, im.height() as usize, res)
        })
        .collect();
    let decode_s = t.elapsed().as_secs_f64();
    let ok: Vec<(usize, &Vec<f32>)> = pre.iter().enumerate().filter_map(|(i, v)| v.as_ref().map(|v| (i, v))).collect();
    let mut ix = VectorIndex::open(None, "eval", m.dim())?;
    let t = Instant::now();
    let bs: usize = std::env::var("BATCH").ok().and_then(|v| v.parse().ok()).unwrap_or(16);
    for chunk in ok.chunks(bs) {
        let batch: Vec<Vec<f32>> = chunk.iter().map(|(_, v)| (*v).clone()).collect();
        let e = m.embed_images(&batch)?;
        ix.add(&chunk.iter().zip(e).map(|((i, _), v)| (key_of(&paths[*i]), v)).collect::<Vec<_>>())?;
    }
    let embed_s = t.elapsed().as_secs_f64();
    let mut res_q = Vec::new();
    let qs: Vec<String> = std::fs::read_to_string(queries)?.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
    let by_key: std::collections::HashMap<u64, &String> = paths.iter().map(|p| (key_of(p), p)).collect();
    let t = Instant::now();
    for q in &qs {
        let tq = Instant::now();
        let v = m.embed_text(q)?;
        let text_ms = tq.elapsed().as_secs_f64() * 1e3;
        let tq = Instant::now();
        let top = ix.top(&v, 8, &|_| true);
        let search_ms = tq.elapsed().as_secs_f64() * 1e3;
        res_q.push(json!({"query": q, "text_ms": text_ms, "search_ms": search_ms,
            "top": top.iter().map(|(k, c)| json!({"path": by_key[k], "cos": c, "p": m.probability(*c)})).collect::<Vec<_>>()}));
    }
    let n = ok.len();
    let report = json!({"model": dir, "dtype": format!("{dtype:?}"), "device": format!("{:?}", device.location()), "images": n,
        "load_s": load_s, "decode_preprocess_s": decode_s, "embed_s": embed_s, "embed_ms_per_image": embed_s * 1e3 / n.max(1) as f64,
        "queries_s": t.elapsed().as_secs_f64(), "results": res_q});
    std::fs::write(out, serde_json::to_string_pretty(&report)?)?;
    println!("{} images, load {load_s:.1}s, decode+pre {decode_s:.1}s, embed {embed_s:.1}s = {:.1} ms/img", n, embed_s * 1e3 / n.max(1) as f64);
    Ok(())
}
