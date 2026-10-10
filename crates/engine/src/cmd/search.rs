//! Search by meaning and duplicates: `search.*` (the SigLIP model, its index, search, zero-shot
//! labels) and `library.findDuplicates` (exact by content, near by perceptual hash + embeddings).

use std::collections::HashMap;

use lightcraft_catalog::{PhotoId, Source};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, f64_or, str_param};
use crate::search::{self, Searcher};
use crate::{EngineError, Result, Session};

/// Searcher failures are shown as they are (not as "invalid parameters").
fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

fn model_status(s: &mut Session, _: &Value) -> Result<Value> {
    let g = &s.searcher;
    Ok(json!({
        "available": Searcher::AVAILABLE,
        "model": search::MODEL_NAME,
        "installed": g.installed(),
        "dir": g.model_folder().map(|d| d.display().to_string()),
        "loaded": g.loaded(),
        "sizeBytes": search::MODEL_BYTES,
        "license": search::LICENSE_NAME,
        "licenseUrl": search::LICENSE_URL,
        "modelUrl": search::MODEL_URL,
        "mirrors": g.mirrors().len(),
        "download": g.download_status(),
    }))
}

fn model_download(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "search.model.download";
    if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
        return Err(bad(
            C,
            format!(
                "{} is a {:.1} GB download from {}, published by Google under the {} ({}): pass `acknowledged: true` once the user has agreed to download it. It is not part of LightCraft; everything else works without it.",
                search::MODEL_NAME,
                search::MODEL_BYTES as f64 / 1e9,
                search::MODEL_URL,
                search::LICENSE_NAME,
                search::LICENSE_URL
            ),
        ));
    }
    let started = s.searcher.start_download().map_err(other)?;
    Ok(json!({"started": started, "installed": s.searcher.installed(), "downloading": s.searcher.download_status().running}))
}

/// Groups of photos with the same content (one file's virtual copies aren't duplicates).
fn exact_groups(s: &Session, ids: &[PhotoId]) -> Vec<Vec<PhotoId>> {
    let mut by_hash: HashMap<&str, Vec<(PhotoId, &str)>> = HashMap::new();
    for id in ids {
        let Some(p) = s.catalog.photo(*id) else { continue };
        let (Some(h), Source::File { path }) = (p.content_hash.as_deref(), &p.source) else { continue };
        // (Photo ▸ Duplicate marks its copy's hash `<hash>:dup<id>` to keep their cache keys apart: the content is the same)
        by_hash.entry(h.split(":dup").next().unwrap_or(h)).or_default().push((*id, path));
    }
    let mut out: Vec<Vec<PhotoId>> = Vec::new();
    for (_, mut v) in by_hash {
        v.sort_by_key(|(i, _)| i.0);
        // one photo per file
        let mut seen = std::collections::HashSet::new();
        v.retain(|(_, path)| seen.insert(*path));
        if v.len() >= 2 {
            out.push(v.into_iter().map(|(i, _)| i).collect());
        }
    }
    out.sort_by_key(|g| (std::cmp::Reverse(g.len()), g[0].0));
    out
}

fn find_duplicates(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.findDuplicates";
    let mode = str_param(p, "mode").unwrap_or("exact");
    let ids: Vec<PhotoId> = if p.get("ids").is_some() { s.targets(p) } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() };
    let groups: Vec<Value> = match mode {
        "exact" => exact_groups(s, &ids).into_iter().map(|g| json!({"ids": g.iter().map(|i| i.0).collect::<Vec<_>>(), "kind": "exact"})).collect(),
        "near" => {
            let max = f64_or(p, "maxDistance", 10.0).clamp(0.0, 128.0) as u32;
            let min_cos = f64_or(p, "minCosine", 0.90).clamp(-1.0, 1.0) as f32;
            search::near(s, &ids, max, min_cos)?
                .into_iter()
                .map(|g| {
                    json!({
                        "ids": g.ids.iter().map(|i| i.0).collect::<Vec<_>>(),
                        "kind": "near",
                        "distance": g.distance,
                        "cosine": g.cosine.map(|c| (c * 1000.0).round() / 1000.0),
                        "confirmed": g.confirmed,
                    })
                })
                .collect()
        }
        other => return Err(bad(C, format!("`mode` is `exact` or `near`, not `{other}`"))),
    };
    let all: Vec<PhotoId> = groups.iter().flat_map(|g| g["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(PhotoId)).collect();
    let mut stacked = 0;
    if bool_or(p, "stack", false) {
        for g in &groups {
            let gids: Vec<u64> = g["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
            // the largest file on top
            let top = gids.iter().max_by_key(|i| s.catalog.photo(PhotoId(**i)).map_or(0, |q| q.file_size)).copied();
            if let Some(top) = top
                && s.execute("stack.group", &json!({"ids": gids, "top": top, "collapsed": true})).is_ok()
            {
                stacked += 1;
            }
        }
    }
    if bool_or(p, "filter", false) {
        s.filter.only = all.clone();
    }
    Ok(json!({"mode": mode, "groups": groups, "photos": all.len(), "stacked": stacked}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "search.model.status",
            "Search Model Status",
            [],
            None,
            "{} — whether this build can search by meaning, whether the SigLIP 2 model is installed (and where), loaded in memory, its download size and licence, and the download's progress → {available, model, installed, dir, loaded, sizeBytes, license, licenseUrl, modelUrl, mirrors, download: {running, done, total, file, error, finished}}",
            always,
            model_status
        ),
        cmd!(
            query "search.model.download",
            "Download Search Model",
            [],
            None,
            "{acknowledged: true} — download SigLIP 2 (about 1.5 GB, Google, Apache License 2.0, not part of LightCraft) in the background, only after the user agreed to it; each file is verified by size and SHA-256. Watch search.model.status; search.model.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            model_download
        ),
        cmd!(
            query "search.model.cancel",
            "Cancel Search Model Download",
            [],
            None,
            "{} — stop the download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.searcher.cancel_download()}))
        ),
        cmd!(
            query "search.status",
            "Search Index Status",
            [],
            None,
            "{} — the index (photos indexed / not yet / hashed), the model, and the indexing run → {available, installed, loaded, index: {photos, indexed, hashed, unindexed, vectorsStored, file}, job: {running, total, done, failed, seconds, error, failures}}",
            always,
            |s, _| Ok(search::status(s))
        ),
        cmd!(
            query "search.index",
            "Index Photos for Search",
            [],
            None,
            "{ids? (default: the whole library), background?: bool (false: wait for the run), force?: bool} — embed the photos that have no vector yet (and compute their perceptual hashes) from their thumbnails, in batches, appending to `<library>/search/`; interrupted runs resume where they stopped; with `background: true` it returns at once (watch search.status, stop with search.cancel) → {queued, alreadyIndexed, failed, failures, seconds, secondsPerPhoto, vectors}",
            always,
            search::index
        ),
        cmd!(
            query "search.cancel",
            "Cancel Indexing",
            [],
            None,
            "{} — stop the indexing run after its current batch (what it did is kept) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.searcher.cancel_job()}))
        ),
        cmd!(
            query "search.semantic",
            "Search by Meaning",
            [],
            None,
            "{query, limit?: 1..5000 (50), minScore?: cosine, ids?, filter?: bool (false: filter the view to the results)} — the photos that best match a sentence, in English or French (\"enfant qui joue dans une piscine\"), most similar first → {query, photos: [{id, score: cosine, probability}], indexed, unindexed}",
            always,
            search::semantic
        ),
        cmd!(
            "search.tag",
            "Label Photos by Meaning",
            [],
            None,
            "{labels: [string], threshold?: probability 0..1 (0.02), template?: \"a photo of {}\", ids?, dryRun?: bool} — zero-shot: each photo whose probability for a label reaches the threshold gets that label as a keyword (one undo step per label) → {labels: [{label, matched, tagged, ids, best: [{id, probability}]}], threshold, scored, unindexed}",
            always,
            search::tag
        ),
        cmd!(
            "library.findDuplicates",
            "Find Duplicates",
            ["Photo"],
            None,
            "{mode?: \"exact\" | \"near\" (exact), ids?, maxDistance?: 0..128 hash bits (10), minCosine?: 0..1 (0.90), stack?: bool, filter?: bool} — `exact`: files with the same content hash; `near`: look-alikes (bursts, re-exports) by perceptual hash, confirmed by the embeddings when indexed (hashes are computed on the spot, no model needed); `stack` groups each set into a stack (largest file on top), `filter` shows only them → {mode, groups: [{ids, kind, distance?, cosine?, confirmed?}], photos, stacked}",
            always,
            find_duplicates
        ),
    ]
}
