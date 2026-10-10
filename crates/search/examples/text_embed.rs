//! `text_embed MODEL_DIR PROMPTS.txt OUT.json` — embeds one prompt per line (unit vectors) and
//! writes `{"scale", "bias", "embeddings": {prompt: [..]}}`. Used to evaluate the keyword
//! vocabulary offline.
use lightcraft_search::Embedder;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let m = Embedder::load(std::path::Path::new(&a[1]))?;
    let prompts: Vec<String> = std::fs::read_to_string(&a[2])?.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    let mut map = serde_json::Map::new();
    for chunk in prompts.chunks(32) {
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        for (p, v) in chunk.iter().zip(m.embed_texts(&refs)?) {
            map.insert(p.clone(), json!(v));
        }
    }
    let p0 = m.probability(0.0);
    let p1 = m.probability(1.0);
    std::fs::write(&a[3], serde_json::to_string(&json!({"p_at_0": p0, "p_at_1": p1, "embeddings": map}))?)?;
    println!("{} prompts", prompts.len());
    Ok(())
}
