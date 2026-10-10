//! Portrait retouch (`portrait.retouch`, `portrait.clear`): masks for the skin, eyes and teeth of
//! each face (from the face mesh of Analyze Faces' model) carrying ordinary local adjustments
//! (smoothing = texture / clarity down on the skin, brighter eyes, whiter teeth). Non-destructive:
//! the masks are normal masks, editable in the Masking panel; the retouch is just their start.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, f64_or, has_active};
use crate::portrait::{Patch, Region};
use crate::{Result, Session};

/// Name prefix of the masks this command owns (re-running replaces them).
pub const PREFIX: &str = "Portrait: ";

fn mask_name(r: Region) -> String {
    let n = r.name();
    format!("{PREFIX}{}{}", n[..1].to_uppercase(), &n[1..])
}

/// The faces' patches per region: `faces` = number of faces found, `by_region[i]` = patches.
struct Faces {
    found: usize,
    skipped: usize,
    by_region: Vec<(Region, Vec<Patch>)>,
}

#[cfg(feature = "faces")]
fn landmarker(s: &Session) -> Result<std::sync::Arc<lightcraft_facemesh::FaceLandmarker>> {
    use std::sync::{Arc, Mutex, PoisonError};
    static CACHE: Mutex<Option<(std::path::PathBuf, Arc<lightcraft_facemesh::FaceLandmarker>)>> = Mutex::new(None);
    let file = s.facemesh.model_file().ok_or_else(|| bad("portrait.retouch", "no folder is set for the face model"))?;
    if !s.facemesh.installed() {
        return Err(bad(
            "portrait.retouch",
            "the face model is not installed: download it with `facemesh.model.download {\"acknowledged\": true}` (MediaPipe Face Landmarker, 3.8 MB, Apache-2.0)",
        ));
    }
    let mut g = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((f, m)) = &*g
        && *f == file
    {
        return Ok(m.clone());
    }
    let bytes = std::fs::read(&file).map_err(|e| bad("portrait.retouch", format!("{}: {e}", file.display())))?;
    let m = Arc::new(lightcraft_facemesh::FaceLandmarker::from_task(&bytes).map_err(|e| bad("portrait.retouch", e))?);
    *g = Some((file, m.clone()));
    Ok(m)
}

#[cfg(feature = "faces")]
fn find(s: &mut Session, id: PhotoId, regions: &[Region], max_faces: usize) -> Result<Faces> {
    use lightcraft_facemesh::mediapipe::Frame;
    const C: &str = "portrait.retouch";
    let model = landmarker(s)?;
    let img = super::autoseg::developed(s, id, C)?;
    let (w, h) = (img.width, img.height);
    let rgb: Vec<[f32; 3]> = img.data.iter().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
    let mut meshes = model.find_all(Frame { rgb: &rgb, w, h }).map_err(|e| bad(C, e))?;
    // the biggest faces first (a crowd's background faces are not retouched)
    meshes.sort_by(|a, b| {
        let (ba, bb) = (a.bbox(), b.bbox());
        ((bb[2] - bb[0]) * (bb[3] - bb[1])).total_cmp(&((ba[2] - ba[0]) * (ba[3] - ba[1])))
    });
    let found = meshes.len();
    let skipped = found.saturating_sub(max_faces);
    meshes.truncate(max_faces);
    let by_region =
        regions.iter().map(|r| (*r, meshes.iter().filter_map(|m| crate::portrait::patch(&m.points, w, h, *r)).collect::<Vec<_>>())).collect();
    Ok(Faces { found, skipped, by_region })
}

#[cfg(not(feature = "faces"))]
fn find(_: &mut Session, _: PhotoId, _: &[Region], _: usize) -> Result<Faces> {
    Err(bad("portrait.retouch", "face analysis is not available in this build"))
}

/// A mask component made of the patches (the stored detail passes; no coarse pass).
fn component(patches: &[Patch]) -> MaskComponent {
    let detail = patches.iter().map(|p| SegMask::from_logits_in(p.side, &p.logits, p.rect)).collect();
    MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Prompt { text: "face".into(), seg: None, detail, edge: 0.0 } }
}

fn retouch(s: &mut Session, p: &Value) -> Result<Value> {
    retouch_with(s, p, &mut find)
}

type Finder<'a> = &'a mut dyn FnMut(&mut Session, PhotoId, &[Region], usize) -> Result<Faces>;

/// `portrait.retouch` with the face finder given (the tests supply meshes without the model).
fn retouch_with(s: &mut Session, p: &Value, find: Finder) -> Result<Value> {
    const C: &str = "portrait.retouch";
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p) } else { s.active().into_iter().collect() };
    if ids.is_empty() {
        return Err(bad(C, "no active photo"));
    }
    let amounts = [
        (Region::Skin, f64_or(p, "skin", 40.0)),
        (Region::Eyes, f64_or(p, "eyes", 30.0)),
        (Region::Teeth, f64_or(p, "teeth", 30.0)),
        (Region::Lips, f64_or(p, "lips", 0.0)),
    ];
    let regions: Vec<(Region, f64)> = amounts.into_iter().filter(|(_, a)| *a > 0.0).collect();
    if regions.is_empty() {
        return Err(bad(C, "all amounts are 0 (skin, eyes, teeth, lips: 0..100)"));
    }
    let max_faces = (f64_or(p, "maxFaces", 4.0) as usize).clamp(1, crate::portrait_max_detail());
    let refine = f64_or(p, "refine", 0.0).clamp(0.0, 100.0);
    let (mut out, mut total_faces) = (Vec::new(), 0usize);
    for id in ids {
        let rs: Vec<Region> = regions.iter().map(|(r, _)| *r).collect();
        let faces = find(s, id, &rs, max_faces)?;
        total_faces += faces.found;
        let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
        // the previous retouch goes (same names), what the user added stays
        d.masks.retain(|m| !m.name.starts_with(PREFIX));
        let mut made = Vec::new();
        for (region, patches) in &faces.by_region {
            if patches.is_empty() {
                continue;
            }
            let amount = regions.iter().find(|(r, _)| r == region).map_or(0.0, |(_, a)| *a);
            let next = d.next_mask_id();
            d.masks.push(Mask {
                id: next,
                name: mask_name(*region),
                components: vec![component(patches)],
                adjust: crate::portrait::adjustments(*region, amount),
                refine,
                ..Default::default()
            });
            made.push(json!({"mask": next, "region": region.name(), "faces": patches.len(), "amount": amount}));
        }
        if made.is_empty() {
            out.push(json!({"id": id.0, "faces": faces.found, "masks": []}));
            continue;
        }
        s.set_develop(id, d, "Portrait Retouch")?;
        out.push(json!({"id": id.0, "faces": faces.found, "skippedFaces": faces.skipped, "masks": made}));
    }
    Ok(json!({"photos": out, "faces": total_faces}))
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p) } else { s.active().into_iter().collect() };
    let mut removed = 0usize;
    for id in ids {
        let Some(d) = s.develop_of(id) else { continue };
        if !d.masks.iter().any(|m| m.name.starts_with(PREFIX)) {
            continue;
        }
        let mut d = (*d).clone();
        let n = d.masks.len();
        d.masks.retain(|m| !m.name.starts_with(PREFIX));
        removed += n - d.masks.len();
        s.set_develop(id, d, "Clear Portrait Retouch")?;
    }
    Ok(json!({"removed": removed}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "portrait.retouch",
            "Portrait Retouch",
            ["Photo"],
            Some("Cmd+Alt+P"),
            "{id?|ids?, skin?: 0..100 (40), eyes?: 0..100 (30), teeth?: 0..100 (30), lips?: 0..100 (0), maxFaces?: 1..4 (4, biggest first), refine?: 0..100 (0)} — find the faces (face model: facemesh.model.status) and add one mask per region — \"Portrait: Skin\" (texture and clarity down, noise reduction: smoothing that keeps eyes, brows and lips sharp), \"Portrait: Eyes\" (brighter, crisper), \"Portrait: Teeth\" (whiter; only on open mouths), \"Portrait: Lips\" — each carrying local adjustments scaled by its amount, so everything stays editable in the Masking panel. Running it again replaces its own masks; one undo step per photo → {photos: [{id, faces, skippedFaces, masks: [{mask, region, faces, amount}]}], faces}",
            has_active,
            retouch
        ),
        cmd!(
            "portrait.clear",
            "Remove Portrait Retouch",
            ["Photo"],
            None,
            "{id?|ids?} — remove the masks portrait.retouch added (named \"Portrait: …\"), keeping every other mask → {removed}",
            has_active,
            clear
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Meshes of a synthetic face, standing in for the model: skin, eyes and (open mouth) teeth.
    fn fake_faces(_: &mut Session, _: PhotoId, regions: &[Region], _: usize) -> Result<Faces> {
        let mut pts = vec![[0f32; 3]; 478];
        let ell = |cx: f32, cy: f32, rx: f32, ry: f32, n: usize, i: usize| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            [cx + rx * t.cos(), cy + ry * t.sin(), 0.0]
        };
        for (i, &k) in crate::portrait::OVAL.iter().enumerate() {
            pts[k] = ell(0.5, 0.5, 0.2, 0.3, 36, i);
        }
        for (i, &k) in crate::portrait::LEFT_EYE.iter().enumerate() {
            pts[k] = ell(0.6, 0.4, 0.05, 0.02, 16, i);
        }
        for (i, &k) in crate::portrait::RIGHT_EYE.iter().enumerate() {
            pts[k] = ell(0.4, 0.4, 0.05, 0.02, 16, i);
        }
        for (i, &k) in crate::portrait::LIPS_OUTER.iter().enumerate() {
            pts[k] = ell(0.5, 0.68, 0.09, 0.04, 20, i);
        }
        for (i, &k) in crate::portrait::LIPS_INNER.iter().enumerate() {
            pts[k] = ell(0.5, 0.68, 0.07, 0.02, 20, i);
        }
        for (i, &k) in crate::portrait::LEFT_BROW.iter().enumerate() {
            pts[k] = [0.55 + i as f32 * 0.01, 0.33, 0.0];
        }
        for (i, &k) in crate::portrait::RIGHT_BROW.iter().enumerate() {
            pts[k] = [0.35 + i as f32 * 0.01, 0.33, 0.0];
        }
        // mesh points are in frame pixels of a 1000 x 1000 frame
        let scaled: Vec<[f32; 3]> = pts.iter().map(|p| [p[0] * 1000.0, p[1] * 1000.0, 0.0]).collect();
        let by_region = regions.iter().map(|r| (*r, crate::portrait::patch(&scaled, 1000, 1000, *r).into_iter().collect::<Vec<_>>())).collect();
        Ok(Faces { found: 1, skipped: 0, by_region })
    }

    #[test]
    fn retouch_adds_owned_masks_replaces_them_on_rerun_and_clear_removes_only_them() {
        let mut s = Session::with_demo();
        let id = s.active().expect("a demo photo is active");
        // a mask of the user's own stays through everything
        s.execute("mask.add", &json!({"kind": "radial"})).expect("radial");
        let p = json!({"skin": 60, "eyes": 40, "teeth": 40});
        let out = retouch_with(&mut s, &p, &mut fake_faces).expect("retouch");
        let masks = s.develop_of(id).expect("develop").masks.clone();
        let names: Vec<&str> = masks.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names.iter().filter(|n| n.starts_with(PREFIX)).count(), 3, "{names:?} {out}");
        let skin = masks.iter().find(|m| m.name == "Portrait: Skin").expect("skin");
        assert_eq!(skin.adjust.texture, -48.0);
        let MaskShape::Prompt { detail, seg, .. } = &skin.components[0].shape else { panic!("a stored patch") };
        assert!(seg.is_none() && detail.len() == 1 && detail[0].logits().is_some());
        // again with other amounts: replaced, not stacked
        retouch_with(&mut s, &json!({"skin": 100, "eyes": 0, "teeth": 0}), &mut fake_faces).expect("rerun");
        let masks = s.develop_of(id).expect("develop").masks.clone();
        assert_eq!(masks.iter().filter(|m| m.name.starts_with(PREFIX)).count(), 1);
        assert_eq!(masks.iter().find(|m| m.name == "Portrait: Skin").expect("skin").adjust.texture, -80.0);
        // one undo step takes the retouch away, redo brings it back
        s.execute("edit.undo", &json!({})).expect("undo");
        assert_eq!(s.develop_of(id).expect("develop").masks.iter().filter(|m| m.name.starts_with(PREFIX)).count(), 3);
        s.execute("edit.redo", &json!({})).expect("redo");
        let r = clear(&mut s, &json!({})).expect("clear");
        assert_eq!(r["removed"], 1);
        let masks = s.develop_of(id).expect("develop").masks.clone();
        assert_eq!(masks.len(), 1, "the user's radial mask remains");
    }

    #[test]
    fn zero_amounts_are_refused_and_the_command_is_listed() {
        let mut s = Session::with_demo();
        let e = retouch_with(&mut s, &json!({"skin": 0, "eyes": 0, "teeth": 0, "lips": 0}), &mut fake_faces).unwrap_err();
        assert!(e.to_string().contains("all amounts are 0"), "{e}");
        assert!(super::super::find_command("portrait.retouch").is_some() && super::super::find_command("portrait.clear").is_some());
    }
}
