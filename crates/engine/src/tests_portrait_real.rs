//! Portrait retouch on a real catalogue (ignored tests: they need the face model and a folder of
//! photos, given by the environment). Nothing is written next to the photos; results go to
//! `LC_PORTRAIT_OUT`.
//!
//! `LC_PORTRAIT_DIR=<copy of a folder of JPEGs> LC_PORTRAIT_OUT=<folder> LIGHTCRAFT_FACES_DIR=<folder
//! with face_landmarker.task> [LC_MATTE_DIR=<folder of <file stem>.pgm mattes>]`

use std::path::PathBuf;

use lightcraft_catalog::PhotoId;
use lightcraft_retouch::plane::Plane;
use serde_json::{Value, json};

use crate::cmd::portrait_stack::{preview_with, real_faces};
use crate::portrait_stack::Sources;
use crate::{Result, Session};

fn env_dirs() -> Option<(PathBuf, PathBuf)> {
    Some((PathBuf::from(std::env::var_os("LC_PORTRAIT_DIR")?), PathBuf::from(std::env::var_os("LC_PORTRAIT_OUT")?)))
}

fn session(dir: &std::path::Path) -> (Session, Vec<(String, PhotoId)>) {
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut photos: Vec<(String, PhotoId)> = s.catalog.photos().map(|p| (p.file_name.clone(), p.id)).collect();
    photos.sort();
    (s, photos)
}

/// Analyze every photo's faces and bursts, then try `portrait.openEyes` on every closed-eyes face.
#[test]
#[ignore = "needs the face model and a folder of photos"]
fn real_blink_hunt() {
    let Some((dir, out)) = env_dirs() else { return };
    std::fs::create_dir_all(&out).unwrap();
    let (mut s, photos) = session(&dir);
    let t = std::time::Instant::now();
    let r = s.execute("photo.analyzeFaces", &json!({"wait": true})).unwrap();
    println!("analyzeFaces: {} photos in {:.1}s, analyzed {} failed {}", photos.len(), t.elapsed().as_secs_f64(), r["analyzed"], r["failed"]);
    let t = std::time::Instant::now();
    let r = s.execute("photo.analyze", &json!({"faces": false, "pickBest": false, "rejectClosedEyes": false})).unwrap();
    println!("analyze (bursts): groups={} in {:.1}s", r["groups"], t.elapsed().as_secs_f64());
    let (mut with_faces, mut closed) = (0, Vec::new());
    for (name, id) in &photos {
        let Some(a) = s.catalog.photo(*id).and_then(|p| p.analysis.clone()) else { continue };
        let faces = a.faces.clone().unwrap_or_default();
        if !faces.is_empty() {
            with_faces += 1;
        }
        let (judged, shut) = (faces.iter().filter(|f| f.eyes_judged()).count(), faces.iter().filter(|f| f.eyes_closed()).count());
        println!("{name}: faces={} judged={judged} closed={shut} group={:?}", faces.len(), a.group);
        if shut > 0 {
            closed.push((name.clone(), *id));
        }
    }
    println!("photos with faces: {with_faces}; photos with a closed-eyes face: {}", closed.len());
    for (name, id) in closed {
        let path = out.join(format!("{}-eyes-after.png", name.trim_end_matches(".jpg")));
        let t = std::time::Instant::now();
        match s.execute("portrait.openEyes", &json!({"id": id.0, "edge": 1600, "path": path.to_string_lossy()})) {
            Ok(v) => println!("OPENEYES {name}: source={} eyes={} t={:.1}s", v["source"], v["eyes"], t.elapsed().as_secs_f64()),
            Err(e) => println!("OPENEYES {name}: {e}"),
        }
    }
}

fn matte_from_pgm(path: &std::path::Path, w: usize, h: usize) -> Option<Plane> {
    let b = std::fs::read(path).ok()?;
    // binary PGM: `P5 <w> <h> 255` then the pixels; the pixels are the last w * h bytes
    let head = String::from_utf8_lossy(&b[..b.len().min(32)]).to_string();
    let mut tok = head.split_ascii_whitespace();
    tok.next()?;
    let mw: usize = tok.next()?.parse().ok()?;
    let mh: usize = tok.next()?.parse().ok()?;
    if mw == 0 || mh == 0 || b.len() < mw * mh {
        return None;
    }
    let rest = &b[b.len() - mw * mh..];
    let g = Plane::from_vec(mw, mh, rest[..mw * mh].iter().map(|v| f32::from(*v) / 255.0).collect());
    let mut out = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            out.d[y * w + x] = g.sample((x as f32 + 0.5) / w as f32 * mw as f32, (y as f32 + 0.5) / h as f32 * mh as f32);
        }
    }
    Some(out)
}

/// A variety of faces with every face slider (and the background when mattes are given).
#[test]
#[ignore = "needs the face model and a folder of photos"]
fn real_variety() {
    let Some((dir, out)) = env_dirs() else { return };
    std::fs::create_dir_all(&out).unwrap();
    let mattes = std::env::var_os("LC_MATTE_DIR").map(PathBuf::from);
    let (mut s, photos) = session(&dir);
    let names: std::collections::HashMap<u64, String> = photos.iter().map(|(n, i)| (i.0, n.clone())).collect();
    let mut faces = real_faces;
    let mut matte = |_: &mut Session, id: PhotoId, w: usize, h: usize| -> Result<Plane> {
        let stem = names.get(&id.0).map(|n| n.trim_end_matches(".jpg").to_string()).unwrap_or_default();
        mattes
            .as_ref()
            .and_then(|d| matte_from_pgm(&d.join(format!("{stem}.pgm")), w, h))
            .ok_or_else(|| crate::EngineError::Other("no matte for this photo".into()))
    };
    let mut src = Sources { faces: &mut faces, matte: &mut matte };
    for (name, id) in &photos {
        let stem = name.trim_end_matches(".jpg");
        let sliders = json!({"id": id.0, "edge": 1600, "smooth": 50, "blemishes": 60, "dark_circles": 50, "shine": 25, "teeth": 60, "eyes": 40, "flyaways": 60, "neck_smooth": 30,
            "path": out.join(format!("{stem}-all-after.png")).to_string_lossy()});
        let t = std::time::Instant::now();
        let r = preview_with(&mut s, &sliders, &mut src);
        let v: Value = match r {
            Ok(v) => v,
            Err(e) => {
                println!("{name}: ERROR {e}");
                continue;
            }
        };
        let nfaces = v["faces"].as_u64().unwrap_or(0);
        if nfaces == 0 {
            // no face: delete the pair, nothing to look at
            for suffix in ["all-after", "all-before"] {
                let _ = std::fs::remove_file(out.join(format!("{stem}-{suffix}.png")));
            }
            continue;
        }
        let steps: Vec<String> = v["steps"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| {
                        format!(
                            "{}:{:.1}%{}{}",
                            x["step"].as_str().unwrap_or("?"),
                            x["changed"].as_f64().unwrap_or(0.0) * 100.0,
                            x["found"].as_u64().map_or(String::new(), |n| format!("({n})")),
                            x["skipped"].as_str().map_or(String::new(), |w| format!("[{w}]"))
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        println!("{name}: faces={nfaces} {:.1}s {}", t.elapsed().as_secs_f64(), steps.join(" "));
        if mattes.is_some() {
            let bg = json!({"id": id.0, "edge": 1600, "background": {"mode": "blur", "amount": 70, "edge": 60}, "path": out.join(format!("{stem}-bg-after.png")).to_string_lossy()});
            match preview_with(&mut s, &bg, &mut src) {
                Ok(_) => println!("   background blur ok"),
                Err(e) => println!("   background: {e}"),
            }
        }
    }
}
