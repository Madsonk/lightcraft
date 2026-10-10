//! Lens recognition: which camera and lens took a photo, and which profile corrects it (`lens.info`,
//! `lens.detect`). The profile is found in the lens database (`lightcraft-lens`, data from the lensfun lens
//! database, CC BY-SA 3.0) unless the file itself embeds corrections (DNG opcodes), which win.

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_lens::Confidence;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd};
use crate::{Result, Session};

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Exact => "exact",
        Confidence::Probable => "probable",
        Confidence::Ambiguous => "ambiguous",
        Confidence::None => "none",
    }
}

/// What is known about the lens of `p`, as JSON: the camera and lens the file names, where the profile comes from
/// (`embedded` in the file, the `database`, or `none`) and what it corrects. Never claims a profile that was not found.
pub fn describe(p: &Photo) -> Value {
    let db_state = match lightcraft_lens::database_status() {
        Ok((path, prov, n)) => {
            json!({"installed": true, "path": path.to_string_lossy(), "lenses": n, "source": prov.name, "licence": prov.licence, "revision": prov.revision})
        }
        Err(why) => json!({"installed": false, "reason": why}),
    };
    let q = p.lens_query();
    // the model usually repeats the make ("Canon" + "Canon EOS 5D Mark III")
    let camera = if q.camera_model.to_lowercase().starts_with(&q.camera_make.to_lowercase()) {
        q.camera_model.clone()
    } else {
        [q.camera_make.as_str(), q.camera_model.as_str()].join(" ").trim().to_string()
    };
    let mut out = json!({
        "camera": if p.meta.camera.is_empty() { Value::Null } else { json!(camera) },
        "lensInPhoto": if p.meta.lens.is_empty() { Value::Null } else { json!(p.meta.lens) },
        "focalMm": p.meta.focal_mm, "aperture": p.meta.aperture,
        "lensMaxAperture": p.meta.lens_max_aperture, "subjectDistance": p.meta.subject_distance,
        "database": db_state,
        "enabled": p.develop.optics.lens_profile,
    });
    let det = p.database_lens();
    let (source, mut profile) = if p.embedded_lens.is_some() {
        let l = p.embedded_lens.unwrap_or_default();
        (
            "embedded",
            json!({"distortion": l.warp.is_some(), "vignetting": l.vignette.is_some(), "lateralCa": false, "note": "corrections written by the camera in the file (DNG)"}),
        )
    } else if let Some(r) = det.as_ref().filter(|r| r.correction.is_some()) {
        let d = &r.detection;
        (
            "database",
            json!({
                "lens": d.lens, "confidence": confidence_name(d.confidence), "alternatives": d.alternatives,
                "distortion": d.distortion, "lateralCa": d.lateral_ca, "vignetting": d.vignetting,
                "cropFactor": d.crop_factor, "cameraInDatabase": d.camera_known, "note": d.note,
            }),
        )
    } else {
        let note = match &det {
            Some(r) => r.detection.note.clone(),
            None => match lightcraft_lens::database_status() {
                Err(why) => why,
                Ok(_) => "the photo names no camera or lens".into(),
            },
        };
        let lens = det.as_ref().and_then(|r| r.detection.lens.clone());
        ("none", json!({"lens": lens, "distortion": false, "vignetting": false, "lateralCa": false, "note": note}))
    };
    if let Some(o) = profile.as_object_mut() {
        o.insert("source".into(), json!(source));
    }
    if let Some(o) = out.as_object_mut() {
        o.insert("profile".into(), profile);
    }
    out
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad("lens.info", "no photo (give `id`)"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad("lens.info", "no such photo"))?;
    let mut v = describe(ph);
    if let Some(o) = v.as_object_mut() {
        o.insert("id".into(), json!(id.0));
    }
    Ok(v)
}

/// Re-read the lens facts of the photos' files (older catalogs lack the lens range, widest aperture and subject
/// distance that tell similar lenses apart), report what each photo's lens resolves to and, with `enable`, switch
/// "Use Profile Corrections" on for those that have a profile (only where it is off; edits and history are kept).
fn detect(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "lens.detect";
    let all = bool_or(p, "all", false);
    let enable = bool_or(p, "enable", false);
    let ids: Vec<PhotoId> = if all { s.catalog.photos().filter(|p| p.in_library() || p.local).map(|p| p.id).collect() } else { s.targets(p) };
    if ids.is_empty() {
        return Err(bad(C, "no photos (give `ids`, select some, or pass `all: true`)"));
    }
    // re-read the file headers of the photos that lack lens facts (or all with `reread`)
    let reread = bool_or(p, "reread", true);
    let files: Vec<(PhotoId, String)> = ids
        .iter()
        .filter_map(|id| s.catalog.photo(*id))
        .filter(|ph| reread && ph.meta.lens_spec.is_none() && ph.meta.lens_max_aperture.is_none() && ph.meta.focal_35mm.is_none())
        .filter_map(|ph| match &ph.source {
            Source::File { path } => Some((ph.id, path.clone())),
            _ => None,
        })
        .collect();
    let probed = crate::import::probe_paths(s, &files.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>());
    let mut ops = Vec::new();
    let mut reread_ids = 0usize;
    let mut unreadable = 0usize;
    for ((id, _), info) in files.iter().zip(probed) {
        match (info, s.catalog.photo(*id)) {
            (Ok(info), Some(ph)) => {
                let o = super::convert::fill_missing_meta(*id, ph, &info);
                if !o.is_empty() {
                    reread_ids += 1;
                    ops.extend(o);
                }
            }
            _ => unreadable += 1,
        }
    }
    if !ops.is_empty() {
        s.commit("Read Lens Information", Op::Batch { ops })?;
    }
    let (mut embedded, mut database, mut none) = (0usize, 0usize, 0usize);
    let mut lenses: std::collections::BTreeMap<String, (usize, &'static str, String)> = std::collections::BTreeMap::new();
    let mut enable_ops = Vec::new();
    for id in &ids {
        let Some(ph) = s.catalog.photo(*id) else { continue };
        let d = describe(ph);
        let src = d["profile"]["source"].as_str().unwrap_or("none");
        match src {
            "embedded" => embedded += 1,
            "database" => database += 1,
            _ => none += 1,
        }
        let key = format!("{} | {}", d["camera"].as_str().unwrap_or("?"), d["lensInPhoto"].as_str().unwrap_or("(no lens name)"));
        let e = lenses.entry(key).or_insert((0, "none", String::new()));
        e.0 += 1;
        if src != "none" {
            e.1 = if src == "embedded" { "embedded" } else { "database" };
            e.2 = d["profile"]["lens"].as_str().unwrap_or("").to_string();
        }
        if enable && src != "none" && !ph.develop.optics.lens_profile {
            let mut dev = (*ph.develop).clone();
            dev.optics.lens_profile = true;
            // like the import default: no history step, the edited stamp untouched
            enable_ops.push(Op::SetDevelop {
                id: *id,
                settings: std::sync::Arc::new(dev),
                label: "Use Profile Corrections".into(),
                edited: ph.edited.clone(),
            });
        }
    }
    let enabled = enable_ops.len();
    if !enable_ops.is_empty() {
        s.commit("Use Profile Corrections", Op::Batch { ops: enable_ops })?;
    }
    Ok(json!({
        "photos": ids.len(), "reread": reread_ids, "unreadable": unreadable,
        "embedded": embedded, "database": database, "none": none, "enabled": enabled,
        "lenses": lenses.into_iter().map(|(k, (n, src, lens))| json!({"photoLens": k, "photos": n, "profile": src, "databaseLens": lens})).collect::<Vec<_>>(),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "lens.info", "Lens Profile Information", [], None, "{id?} → {camera, lensInPhoto, profile: {source: embedded|database|none, lens, confidence, distortion, vignetting, lateralCa, note}, database}", always, info),
        cmd!(
            "lens.detect",
            "Detect Lenses",
            [],
            None,
            "{ids?|all?: bool, enable?: bool (switch Use Profile Corrections on where a profile exists), reread?: bool (default true: read lens facts from the files of photos imported before they were kept)} → counts and the lenses found",
            always,
            detect
        ),
    ]
}
