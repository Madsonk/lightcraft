//! Bench of `describe.search` in French: every moment Video Catcher described is a question whose
//! answer is known. The query is one field of its description (its action, or its long
//! description); the search may only use the *other* fields and the pictures, so a hit means the
//! words or the meaning led back to the moment, not that the query found itself.
//!
//! ```text
//! LIGHTCRAFT_SEARCH_DIR=… cargo run --release -p lightcraft-engine --features people,faces,search \
//!     --example describe_bench -- <library> [max queries]
//! ```
//!
//! Prints recall@1/5/10 and the median rank of the moment (and of its video) per method.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use lightcraft_catalog::MediaKind;
use serde_json::{Value, json};

fn median(mut v: Vec<usize>) -> usize {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let lib = std::path::PathBuf::from(args.get(1).expect("library folder"));
    let max: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.searcher.dir = std::env::var_os("LIGHTCRAFT_SEARCH_DIR").map(Into::into);
    s.open_library(&lib, false).expect("open the library");
    let ids: HashMap<String, u64> = s
        .catalog
        .photos()
        .filter(|p| p.in_library() && p.kind == MediaKind::Video)
        .map(|p| (lightcraft_engine::media::content_key(p), p.id.0))
        .collect();
    let store = lightcraft_engine::describe::Store::open(Some(&lib));
    let mut cases: Vec<(u64, u64, String, String)> = store
        .all()
        .into_iter()
        .filter(|a| a.source == "videocatcher")
        .filter_map(|a| Some((*ids.get(&a.item)?, a.ms?, a.action.clone(), a.description.clone())))
        .filter(|(_, _, act, desc)| !act.trim().is_empty() && !desc.trim().is_empty())
        .collect();
    cases.sort();
    cases.truncate(max);
    println!("{} questions ({} videos)", cases.len(), ids.len());
    const LIMIT: usize = 200;
    let methods: Vec<(&str, Value)> = vec![
        ("text", json!({"semantic": false, "textAll": false})),
        ("meaning-fr", json!({"text": false, "lang": "fr", "semanticTop": 1_000_000})),
        ("meaning-en", json!({"text": false, "lang": "en", "semanticTop": 1_000_000})),
        ("meaning-both", json!({"text": false, "lang": "both", "semanticTop": 1_000_000})),
        ("fusion-both", json!({"lang": "both", "textAll": false, "semanticTop": 1_000_000})),
        ("fusion-en", json!({"lang": "en", "textAll": false, "semanticTop": 1_000_000})),
    ];
    for (field, other) in [("action", vec!["scene", "description"]), ("description", vec!["scene", "action"])] {
        println!("\n## query = {field}, searched in {other:?} + pictures");
        println!("| method | r@1 | r@5 | r@10 | median rank | video r@1 | video r@5 | median video rank | s/query |");
        println!("|---|---|---|---|---|---|---|---|---|");
        for (name, extra) in &methods {
            let t0 = std::time::Instant::now();
            let (mut ranks, mut vranks) = (Vec::new(), Vec::new());
            for (id, ms, act, desc) in &cases {
                let q = if field == "action" { act } else { desc };
                let mut p = json!({"query": q, "kind": "video", "limit": LIMIT, "fields": other});
                for (k, v) in extra.as_object().unwrap() {
                    p[k] = v.clone();
                }
                let r = match s.execute("describe.search", &p) {
                    Ok(r) => r,
                    Err(_) => {
                        ranks.push(LIMIT + 1);
                        vranks.push(LIMIT + 1);
                        continue;
                    }
                };
                let rows = r["results"].as_array().cloned().unwrap_or_default();
                let at = rows.iter().position(|x| x["id"].as_u64() == Some(*id) && x["ms"].as_u64() == Some(*ms));
                ranks.push(at.map_or(LIMIT + 1, |i| i + 1));
                // the video: its first moment in the list (distinct videos counted)
                let mut seen = Vec::new();
                let mut vr = LIMIT + 1;
                for x in &rows {
                    let v = x["id"].as_u64().unwrap_or(0);
                    if !seen.contains(&v) {
                        seen.push(v);
                    }
                    if v == *id {
                        vr = seen.len();
                        break;
                    }
                }
                vranks.push(vr);
            }
            let n = ranks.len().max(1) as f64;
            let at = |v: &[usize], k: usize| v.iter().filter(|r| **r <= k).count() as f64 / n * 100.0;
            let med = |v: Vec<usize>| {
                let m = median(v);
                if m > LIMIT { format!(">{LIMIT}") } else { m.to_string() }
            };
            println!(
                "| {name} | {:.1} % | {:.1} % | {:.1} % | {} | {:.1} % | {:.1} % | {} | {:.2} |",
                at(&ranks, 1),
                at(&ranks, 5),
                at(&ranks, 10),
                med(ranks.clone()),
                at(&vranks, 1),
                at(&vranks, 5),
                med(vranks.clone()),
                t0.elapsed().as_secs_f64() / n
            );
        }
    }
}
