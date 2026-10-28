//! `check MODEL_DIR IMAGE TEXT…` — prints the image embedding's first values and each text's
//! token ids, cosine and probability (compared against `tools/ref_siglip.py` in the tests).
use lightcraft_search::{Embedder, preprocess::preprocess, tokenizer::Tokenizer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let m = Embedder::load(dir)?;
    let tok = Tokenizer::load(dir)?;
    let im = image::open(&a[2])?.to_rgb8();
    // (the reference resizes the full image; the engine resizes a 512 px thumbnail)
    let px = preprocess(im.as_raw(), im.width() as usize, im.height() as usize, m.resample()).ok_or("bad image")?;
    let ie = m.embed_images(&[px])?.remove(0);
    let texts: Vec<&str> = a[3..].iter().map(String::as_str).collect();
    let te = m.embed_texts(&texts)?;
    let out: Vec<_> = texts
        .iter()
        .zip(&te)
        .map(|(t, e)| {
            let cos: f32 = ie.iter().zip(e).map(|(x, y)| x * y).sum();
            serde_json::json!({"text": t.to_lowercase(), "ids": &tok.encode(t)[..12], "cos": cos, "p": m.probability(cos)})
        })
        .collect();
    println!("{}", serde_json::json!({"image_emb_head": &ie[..6], "texts": out}));
    Ok(())
}
