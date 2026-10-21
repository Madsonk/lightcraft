//! AI culling (`cull.*`) end to end on a small real album of PNG files.

use crate::Session;
use lightcraft_catalog::{Analysis, FaceInfo, Op, PhotoId};
use lightcraft_raster::Rgb32f;
use serde_json::{Value, json};
use std::sync::Arc;

fn scratch(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("lc-cull-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn blur(img: &Rgb32f, r: usize) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let (mut acc, mut n) = ([0f32; 3], 0.0);
            for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                    let c = img.data[yy * w + xx];
                    acc = [acc[0] + c[0], acc[1] + c[1], acc[2] + c[2]];
                    n += 1.0;
                }
            }
            out.data[y * w + x] = acc.map(|v| v / n);
        }
    }
    out
}

fn save(img: &Rgb32f, path: &std::path::Path) {
    let data: Vec<[u8; 4]> = img
        .data
        .iter()
        .map(|c| {
            let e = |v: f32| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
            [e(c[0]), e(c[1]), e(c[2]), 255]
        })
        .collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: img.width, height: img.height, data }),
        &Default::default(),
    )
    .unwrap();
    std::fs::write(path, png).unwrap();
}

/// The album: scene 1 at 10:00 (a burst of three, three distinct pictures, one dark one), scene 2
/// an hour later (two pictures).
fn album(tag: &str) -> (Session, std::path::PathBuf, Vec<(String, PhotoId)>) {
    let dir = scratch(tag);
    let lib = lightcraft_scenes::demo_library();
    let render = |k: usize| lib[k].render(480, 320);
    let dark = |img: &Rgb32f| Rgb32f { width: img.width, height: img.height, data: img.data.iter().map(|c| c.map(|v| v * 0.04)).collect() };
    let files: Vec<(&str, Rgb32f, &str)> = vec![
        ("a1_burst_soft.png", blur(&render(0), 3), "2026-05-01T10:00:00"),
        ("a2_burst_sharp.png", render(0), "2026-05-01T10:00:02"),
        ("a3_burst_softer.png", blur(&render(0), 6), "2026-05-01T10:00:04"),
        ("b1_other.png", render(3), "2026-05-01T10:01:30"),
        ("b2_other.png", render(5), "2026-05-01T10:03:00"),
        ("b3_dark.png", dark(&render(7)), "2026-05-01T10:04:10"),
        ("c1_later.png", render(9), "2026-05-01T11:30:00"),
        ("c2_later.png", render(11), "2026-05-01T11:31:00"),
    ];
    for (name, img, _) in &files {
        save(img, &dir.join(name));
    }
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut ids = Vec::new();
    for (name, _, when) in &files {
        let id = s.catalog.photos().find(|p| p.file_name == *name).unwrap().id;
        s.commit("t", Op::SetCaptured { id, captured: Some((*when).into()) }).unwrap();
        ids.push((name.to_string(), id));
    }
    (s, dir, ids)
}

fn id_of(ids: &[(String, PhotoId)], name: &str) -> PhotoId {
    ids.iter().find(|(n, _)| n == name).unwrap().1
}

fn verdict(s: &Session, id: PhotoId) -> Arc<lightcraft_culler::Verdict> {
    s.catalog.photo(id).unwrap().analysis.as_ref().unwrap().verdict.clone().unwrap()
}

#[test]
fn run_judges_an_album_and_never_touches_the_originals() {
    let (mut s, dir, ids) = album("run");
    let before: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| std::fs::read(e.unwrap().path()).unwrap()).collect();
    let all: Vec<u64> = ids.iter().map(|i| i.1.0).collect();
    let st = s.execute("cull.run", &json!({"ids": all, "wait": true})).unwrap();
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["total"], 8);
    assert_eq!(st["percent"], 100.0);
    assert_eq!(st["summary"]["scenes"], 2, "{st}");
    // the burst keeps its sharpest frame, the dark one is rejected
    let (soft, sharp, softer) = (
        verdict(&s, id_of(&ids, "a1_burst_soft.png")),
        verdict(&s, id_of(&ids, "a2_burst_sharp.png")),
        verdict(&s, id_of(&ids, "a3_burst_softer.png")),
    );
    assert!(sharp.group.is_some() && sharp.group == soft.group && sharp.group == softer.group, "{sharp:?}");
    assert_eq!(sharp.group_rank, 0);
    assert_ne!(sharp.bucket, lightcraft_culler::Bucket::Rejected, "{sharp:?}");
    assert!(soft.group_rank > 0 && softer.group_rank > 0);
    assert_eq!(softer.bucket, lightcraft_culler::Bucket::Rejected);
    let dark = verdict(&s, id_of(&ids, "b3_dark.png"));
    assert!(dark.reasons.contains(&lightcraft_culler::Reason::Underexposed), "{dark:?}");
    assert_eq!(dark.bucket, lightcraft_culler::Bucket::Rejected);
    // scenes: the first six are one scene, the last two another
    assert_eq!(verdict(&s, id_of(&ids, "a1_burst_soft.png")).scene, 0);
    assert_eq!(verdict(&s, id_of(&ids, "c2_later.png")).scene, 1);
    // originals untouched
    let after: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| std::fs::read(e.unwrap().path()).unwrap()).collect();
    assert_eq!(before, after);
    // a second run reuses the measurements
    let again = s.execute("cull.run", &json!({"ids": ids.iter().map(|i| i.1.0).collect::<Vec<_>>(), "wait": true})).unwrap();
    assert_eq!(again["reused"], 8, "{again}");
    assert_eq!(again["measured"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_explain_apply_and_undo() {
    let (mut s, dir, ids) = album("list");
    s.execute("cull.run", &json!({"ids": ids.iter().map(|i| i.1.0).collect::<Vec<_>>(), "wait": true})).unwrap();
    let all: Vec<u64> = ids.iter().map(|i| i.1.0).collect();
    let l = s.execute("cull.list", &json!({"ids": all})).unwrap();
    assert_eq!(l["judged"], 8);
    let scenes = l["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), 2);
    assert_eq!(scenes[0]["count"], 6);
    let photos = l["photos"].as_array().unwrap();
    assert!(photos.windows(2).all(|w| w[0]["scene"].as_u64() <= w[1]["scene"].as_u64()), "scene order");
    let rejected = s.execute("cull.list", &json!({"ids": all, "bucket": ["rejected"]})).unwrap();
    assert!(rejected["matched"].as_u64().unwrap() >= 3, "{rejected}");
    assert!(s.execute("cull.list", &json!({"ids": all, "bucket": ["nope"]})).is_err());
    let dup = s.execute("cull.list", &json!({"ids": all, "reason": "duplicate"})).unwrap();
    assert_eq!(dup["matched"], 2, "{dup}");
    let e = s.execute("cull.explain", &json!({"id": id_of(&ids, "b3_dark.png").0})).unwrap();
    assert_eq!(e["bucket"], "rejected");
    assert!(e["reasons"].as_array().unwrap().iter().any(|r| r["reason"] == "underexposed" && r["severity"] == "reject"), "{e}");
    // nothing was written to ratings / flags by the run
    assert!(s.catalog.photos().all(|p| p.rating == 0 && p.flag == lightcraft_catalog::Flag::None));
    let a = s.execute("cull.apply", &json!({"ids": all})).unwrap();
    assert!(a["rated"].as_u64().unwrap() >= 6 && a["rejected"].as_u64().unwrap() >= 3, "{a}");
    assert_eq!(s.catalog.photo(id_of(&ids, "b3_dark.png")).unwrap().flag, lightcraft_catalog::Flag::Reject);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photos().all(|p| p.rating == 0 && p.flag == lightcraft_catalog::Flag::None), "one undo step");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn genre_and_known_faces_change_the_verdict() {
    let (mut s, dir, ids) = album("genre");
    let all: Vec<u64> = ids.iter().map(|i| i.1.0).collect();
    // a face with a closed eye on a good picture
    let target = id_of(&ids, "b1_other.png");
    let prev = s.catalog.photo(target).and_then(|p| p.analysis.clone()).unwrap_or_default();
    let face = FaceInfo { rect: [0.3, 0.2, 0.5, 0.5], score: 1.0, open: [0.05, 0.06], sharp: [80.0, 80.0], turn: 0.0 };
    s.catalog.apply(Op::SetAnalysis { id: target, analysis: Some(Analysis { faces: Some(Arc::new(vec![face])), ..prev }) }).unwrap();
    let r = s.execute("cull.setGenre", &json!({"genre": "wedding"})).unwrap();
    assert_eq!(r["genre"], "wedding");
    s.execute("cull.run", &json!({"ids": all, "wait": true})).unwrap();
    let v = verdict(&s, target);
    assert!(v.reasons.contains(&lightcraft_culler::Reason::ClosedEyes), "{v:?}");
    assert_eq!(v.bucket, lightcraft_culler::Bucket::Rejected);
    assert_eq!(v.genre, lightcraft_culler::Genre::Wedding);
    // sport does not care
    let r = s.execute("cull.run", &json!({"ids": all, "genre": "sport", "wait": true})).unwrap();
    assert_eq!(r["genre"], "sport");
    assert!(!verdict(&s, target).reasons.contains(&lightcraft_culler::Reason::ClosedEyes));
    assert!(s.execute("cull.setGenre", &json!({"genre": "opera"})).is_err());
    assert!(s.execute("cull.run", &json!({"ids": all, "grouping": 3})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_keeps_the_catalogue_clean_and_the_limit_protects_big_libraries() {
    let (mut s, dir, ids) = album("cancel");
    let all: Vec<u64> = ids.iter().map(|i| i.1.0).collect();
    let st = s.execute("cull.run", &json!({"ids": all, "force": true})).unwrap();
    assert!(matches!(st["state"].as_str(), Some("running" | "done")), "{st}");
    if st["state"] == "running" {
        assert!(s.execute("cull.run", &json!({"ids": all})).is_err(), "one run at a time");
    }
    let c = s.execute("cull.cancel", &json!({})).unwrap();
    let mut state = c["state"].as_str().unwrap().to_string();
    for _ in 0..500 {
        if state != "running" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        state = s.execute("cull.status", &json!({})).unwrap()["state"].as_str().unwrap().to_string();
    }
    assert!(matches!(state.as_str(), "cancelled" | "done"), "{state}");
    if state == "cancelled" {
        assert!(s.catalog.photos().all(|p| p.analysis.as_ref().and_then(|a| a.verdict.as_ref()).is_none()), "a cancelled run writes nothing");
    }
    let e = s.execute("cull.run", &json!({"ids": all, "limit": 3, "force": true})).unwrap_err().to_string();
    assert!(e.contains("limit"), "{e}");
    assert!(matches!(s.execute("cull.status", &json!({})).unwrap()["state"].as_str(), Some(_)));
    assert!(s.execute("cull.explain", &json!({"id": 999_999})).is_err());
    let _: Value = s.execute("cull.status", &json!({})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Throughput of `cull.run` on a generated album of JPEG files (`LIGHTCRAFT_CULL_BENCH=<photos>`,
/// default 48; long edge `LIGHTCRAFT_CULL_EDGE`, default 3000). Run in release:
/// `cargo test --release -p lightcraft-engine tests_cull::bench -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark: prints photos per second"]
fn bench() {
    let n: usize = std::env::var("LIGHTCRAFT_CULL_BENCH").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
    let edge: usize = std::env::var("LIGHTCRAFT_CULL_EDGE").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let dir = scratch("bench");
    let lib = lightcraft_scenes::demo_library();
    let (w, h) = (edge, edge * 2 / 3);
    let t0 = std::time::Instant::now();
    for k in 0..n.div_ceil(4) {
        let base = lib[k % lib.len()].render(w, h);
        let variants =
            [base.clone(), blur(&base, 2), Rgb32f { width: w, height: h, data: base.data.iter().map(|c| c.map(|v| v * 0.04)).collect() }, base];
        for (v, img) in variants.iter().enumerate() {
            let i = k * 4 + v;
            if i >= n {
                break;
            }
            let data: Vec<[u8; 4]> = img
                .data
                .iter()
                .map(|c| {
                    let e = |x: f32| (lightcraft_color::transfer::linear_to_srgb(x.clamp(0.0, 1.0)) * 255.0).round() as u8;
                    [e(c[0]), e(c[1]), e(c[2]), 255]
                })
                .collect();
            let jpg = lightcraft_codecs::encode_jpeg(
                &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
                90,
                lightcraft_codecs::ChromaSubsampling::default(),
                &Default::default(),
            )
            .unwrap();
            std::fs::write(dir.join(format!("p{i:04}.jpg")), jpg).unwrap();
        }
    }
    eprintln!("generated {n} JPEGs of {w}x{h} in {:.1}s", t0.elapsed().as_secs_f64());
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let ids: Vec<(PhotoId, usize)> = s.catalog.photos().map(|p| (p.id, p.file_name[1..5].parse().unwrap_or(0))).collect();
    for (id, i) in &ids {
        let t = format!("2026-05-01T10:{:02}:{:02}", (i / 4 * 20) / 60 % 60, (i / 4 * 20) % 60 + i % 4);
        s.commit("t", Op::SetCaptured { id: *id, captured: Some(t) }).unwrap();
    }
    for pass in ["cold", "warm"] {
        let st = s.execute("cull.run", &json!({"wait": true, "force": pass == "cold", "genre": "wedding"})).unwrap();
        eprintln!(
            "cull.run {pass}: {} photos in {} ms = {} photos/s (measured {}, reused {}); {}",
            st["total"], st["elapsedMs"], st["photosPerSecond"], st["measured"], st["reused"], st["summary"]
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
