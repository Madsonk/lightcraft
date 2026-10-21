//! `portrait.*` — retouch by named sliders on pixels, with a per-photo stack of steps: `list`,
//! `apply`, `preview`, `reset`, `history`, `batch`, `auto`, `cancel`, `remove`. The work is in
//! [`crate::portrait_stack`]; this file reads parameters, finds the faces and the subject with
//! the optional models, and writes the settings (one undo step per photo).

use lightcraft_catalog::PhotoId;
use lightcraft_develop::PortraitStep;
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::portrait::KINDS;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, f64_or, has_active, str_param};
use crate::portrait_stack::{self as ps, Meshes, Sources};
use crate::{Result, Session};

// ---- the real face finder and subject matte --------------------------------------------------

#[cfg(feature = "faces")]
fn real_faces(s: &mut Session, id: PhotoId) -> Result<Meshes> {
    use lightcraft_facemesh::mediapipe::Frame;
    const C: &str = "portrait.apply";
    let model = super::portrait::landmarker(s)?;
    let img = super::autoseg::developed(s, id, C)?;
    let (w, h) = (img.width, img.height);
    let rgb: Vec<[f32; 3]> = img.data.iter().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
    let mut meshes = model.find_all(Frame { rgb: &rgb, w, h }).map_err(|e| bad(C, e))?;
    meshes.sort_by(|a, b| {
        let (ba, bb) = (a.bbox(), b.bbox());
        ((bb[2] - bb[0]) * (bb[3] - bb[1])).total_cmp(&((ba[2] - ba[0]) * (ba[3] - ba[1])))
    });
    Ok(meshes.iter().map(|m| m.points.iter().map(|p| [p[0] / w as f32, p[1] / h as f32, p[2] / w as f32]).collect()).collect())
}

#[cfg(not(feature = "faces"))]
fn real_faces(_: &mut Session, _: PhotoId) -> Result<Meshes> {
    Err(bad("portrait.apply", "face analysis is not available in this build"))
}

#[cfg(feature = "autoseg")]
fn real_matte(s: &mut Session, id: PhotoId, w: usize, h: usize) -> Result<Plane> {
    const C: &str = "portrait.apply";
    let img = super::autoseg::developed(s, id, C)?;
    let (seg, _) = crate::autoseg::segment(&img, &crate::autoseg::Kind::Subject).map_err(crate::EngineError::Other)?;
    matte_from(&seg, w, h).ok_or_else(|| bad(C, "the subject matte is damaged"))
}

#[cfg(not(feature = "autoseg"))]
fn real_matte(_: &mut Session, _: PhotoId, _: usize, _: usize) -> Result<Plane> {
    Err(bad("portrait.apply", "the subject model is not available in this build (background steps need it)"))
}

/// A stored segmentation sampled at `w x h` as a 0..1 plane.
pub(crate) fn matte_from(seg: &lightcraft_develop::SegMask, w: usize, h: usize) -> Option<Plane> {
    let logits = seg.logits()?;
    let side = seg.side as usize;
    let g = Plane::from_vec(side, side, logits.iter().map(|l| 1.0 / (1.0 + (-l).exp())).collect());
    let mut out = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            out.d[y * w + x] = g.sample((x as f32 + 0.5) / w as f32 * side as f32, (y as f32 + 0.5) / h as f32 * side as f32);
        }
    }
    Some(out)
}

// ---- shared steps ----------------------------------------------------------------------------

fn ids_of(s: &mut Session, p: &Value) -> Vec<PhotoId> {
    if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p) } else { s.active().into_iter().collect() }
}

fn label_of(steps: &[PortraitStep]) -> String {
    let names: Vec<&str> =
        steps.iter().filter(|st| st.enabled).map(|st| KINDS.iter().find(|i| i.id == st.kind).map_or("Background", |i| i.label)).collect();
    format!("Portrait: {}", if names.len() > 3 { format!("{} steps", names.len()) } else { names.join(", ") })
}

/// Apply (or re-run) the stack on one photo. Returns the report, or `{skipped}` when there was
/// nothing to do for this photo.
pub(crate) fn apply_one(s: &mut Session, id: PhotoId, new: Vec<PortraitStep>, replace: bool, max_faces: usize, src: &mut Sources) -> Result<Value> {
    const C: &str = "portrait.apply";
    let d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
    let mut steps = if replace { Vec::new() } else { d.portrait.steps.clone() };
    ps::merge(&mut steps, new);
    // a step with amount 0 is a step removed
    steps.retain(|st| st.amount > 0.0 || !st.enabled);
    let base = ps::base_settings(&d);
    if steps.is_empty() {
        if d.portrait.is_empty() {
            return Ok(json!({"id": id.0, "skipped": "no steps"}));
        }
        let mut nd = base;
        nd.portrait = Default::default();
        s.set_develop(id, nd, "Portrait: reset")?;
        return Ok(json!({"id": id.0, "reset": true}));
    }
    let o = ps::render(s, id, &base, &steps, ps::EDGE, max_faces, src)?;
    if o.faces == 0 && o.patch.is_none() {
        return Ok(json!({"id": id.0, "skipped": "no face found", "seconds": o.seconds}));
    }
    ps::annotate(&mut steps, &o);
    let stored = match &o.patch {
        Some(p) => Some(s.store_patch(p.clone())?),
        None => None,
    };
    let nd = ps::with_stack(&base, steps.clone(), &o, stored.as_deref());
    s.set_develop(id, nd, &label_of(&steps))?;
    Ok(json!({
        "id": id.0,
        "faces": o.faces,
        "skippedFaces": o.skipped_faces,
        "steps": ps::report_json(&o),
        "changedShare": (o.changed_share * 1000.0).round() / 1000.0,
        "patch": o.patch.as_ref().map(|p| json!({"id": p.hex_id(), "width": p.width, "height": p.height})),
        "seconds": (o.seconds * 1000.0).round() / 1000.0,
    }))
}

fn max_faces_of(p: &Value) -> usize {
    (f64_or(p, "maxFaces", 4.0) as usize).clamp(1, 8)
}

fn real_sources<'a>(
    f: &'a mut dyn FnMut(&mut Session, PhotoId) -> Result<Meshes>,
    m: &'a mut dyn FnMut(&mut Session, PhotoId, usize, usize) -> Result<Plane>,
) -> Sources<'a> {
    Sources { faces: f, matte: m }
}

// ---- commands --------------------------------------------------------------------------------

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let d = ids_of(s, p).first().and_then(|id| s.develop_of(*id));
    let current = |kind: &str| {
        d.as_ref().and_then(|d| d.portrait.step(kind)).map(|st| json!({"amount": st.amount, "enabled": st.enabled, "changed": st.changed}))
    };
    let sliders: Vec<Value> = KINDS
        .iter()
        .map(|i| json!({"id": i.id, "label": i.label, "group": i.group, "help": i.help, "auto": (i.auto * 100.0).round(), "current": current(i.id)}))
        .chain([json!({
            "id": ps::BACKGROUND, "label": "Background", "group": "Background",
            "help": "replaces the background behind the subject: mode blur | color | dim, plus shadow 0..100, zoom 0..60 (%), rotate -45..45 (°), edge 0..100 (needs the subject model: mask.auto / autoseg.model.download)",
            "auto": 0, "current": current(ps::BACKGROUND)
        })])
        .collect();
    Ok(json!({"sliders": sliders, "order": "marks, shapes, colour, mouth and eyes, hair, then background (fixed)"}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "portrait.apply";
    let ids = ids_of(s, p);
    if ids.is_empty() {
        return Err(bad(C, "no active photo"));
    }
    let new = if bool_or(p, "auto", false) { ps::auto_steps() } else { ps::steps_from_params(p, C)? };
    let replace = bool_or(p, "replace", false);
    let max_faces = max_faces_of(p);
    ps::clear_cancel();
    let mut photos = Vec::new();
    let (mut f, mut m) = (real_faces, real_matte);
    for id in ids {
        if ps::cancel_requested() {
            photos.push(json!({"id": id.0, "skipped": "cancelled"}));
            continue;
        }
        let mut src = real_sources(&mut f, &mut m);
        photos.push(apply_one(s, id, new.clone(), replace, max_faces, &mut src)?);
    }
    Ok(json!({"photos": photos}))
}

fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut f, mut m) = (real_faces, real_matte);
    let mut src = real_sources(&mut f, &mut m);
    preview_with(s, p, &mut src)
}

fn write_png(img: &lightcraft_raster::Rgba8, path: &std::path::Path, c: &str) -> Result<()> {
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &Default::default()).map_err(|e| bad(c, e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| bad(c, format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(path, png).map_err(|e| bad(c, format!("{}: {e}", path.display())))
}

/// `portrait.preview`: the photo with and without the retouch, small, written as PNG files.
pub(crate) fn preview_with(s: &mut Session, p: &Value, src: &mut Sources) -> Result<Value> {
    const C: &str = "portrait.preview";
    let id = ids_of(s, p).first().copied().ok_or_else(|| bad(C, "no active photo"))?;
    let edge = (f64_or(p, "edge", ps::PREVIEW_EDGE as f64) as usize).clamp(256, 2048);
    let new = if bool_or(p, "auto", false) { ps::auto_steps() } else { ps::steps_from_params(p, C)? };
    let d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
    let mut steps = if bool_or(p, "replace", false) { Vec::new() } else { d.portrait.steps.clone() };
    ps::merge(&mut steps, new);
    steps.retain(|st| st.amount > 0.0);
    if steps.is_empty() {
        return Err(bad(C, "no steps: give slider amounts (see portrait.list), `steps`, or `auto: true`"));
    }
    let base = ps::base_settings(&d);
    let o = ps::render(s, id, &base, &steps, edge, max_faces_of(p), src)?;
    let t = web_time::Instant::now();
    let cached = o.patch.clone().map(|pt| s.cache_patch(pt));
    let after_settings = ps::with_stack(&base, steps.clone(), &o, cached.as_deref());
    let render = |s: &mut Session, settings: &lightcraft_develop::DevelopSettings| -> Result<lightcraft_raster::Rgba8> {
        let job = s.preview_job(id, edge, edge, true, settings).ok_or_else(|| bad(C, "no such photo"))?;
        Ok(job.run().rendered.map_err(|e| bad(C, e))?.image)
    };
    let (before, after) = (render(s, &base)?, render(s, &after_settings)?);
    let dir = std::env::temp_dir().join("lightcraft-portrait");
    let path = str_param(p, "path").map_or_else(|| dir.join(format!("{}-after.png", id.0)), std::path::PathBuf::from);
    let before_path =
        path.with_file_name(format!("{}-before.png", path.file_stem().and_then(|x| x.to_str()).unwrap_or("preview").trim_end_matches("-after")));
    write_png(&after, &path, C)?;
    write_png(&before, &before_path, C)?;
    Ok(json!({
        "id": id.0,
        "after": path.display().to_string(),
        "before": before_path.display().to_string(),
        "width": after.width, "height": after.height,
        "faces": o.faces,
        "steps": ps::report_json(&o),
        "eyes": ps::eyes_json(&o),
        "seconds": {"retouch": (o.seconds * 1000.0).round() / 1000.0, "render": (t.elapsed().as_secs_f64() * 1000.0).round() / 1000.0},
        "stored": false,
    }))
}

fn reset(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "portrait.reset";
    let ids = ids_of(s, p);
    let only = str_param(p, "step").map(str::to_string);
    let mut done = 0usize;
    let (mut f, mut m) = (real_faces, real_matte);
    for id in ids {
        let Some(d) = s.develop_of(id) else { continue };
        if d.portrait.is_empty() {
            continue;
        }
        match &only {
            None => {
                let mut nd = ps::base_settings(&d);
                nd.portrait = Default::default();
                s.set_develop(id, nd, "Portrait: reset")?;
            }
            Some(kind) => {
                if d.portrait.step(kind).is_none() {
                    return Err(bad(C, format!("no step `{kind}` on photo {}", id.0)));
                }
                // the other steps run again without it
                let mut src = real_sources(&mut f, &mut m);
                apply_one(s, id, vec![PortraitStep { kind: kind.clone(), amount: 0.0, ..Default::default() }], false, max_faces_of(p), &mut src)?;
            }
        }
        done += 1;
    }
    Ok(json!({"reset": done}))
}

fn history(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "portrait.history";
    let id = ids_of(s, p).first().copied().ok_or_else(|| bad(C, "no active photo"))?;
    let d = s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?;
    let stale = d.portrait.patch.is_some() && d.portrait.key != ps::linear_key(&d);
    let steps: Vec<Value> = d
        .portrait
        .steps
        .iter()
        .map(|st| json!({"step": st.kind, "amount": st.amount, "enabled": st.enabled, "changed": st.changed, "options": st.options, "text": st.text}))
        .collect();
    Ok(json!({"id": id.0, "steps": steps, "faces": d.portrait.faces, "patch": d.portrait.patch, "stale": stale,
        "note": if stale { "white balance, lens correction or geometry changed since the pixels were made: run portrait.apply again (no arguments) to refresh" } else { "" }}))
}

fn batch(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut f, mut m) = (real_faces, real_matte);
    let mut src = real_sources(&mut f, &mut m);
    batch_with(s, p, &mut src)
}

/// `portrait.batch` / `portrait.auto`: a chunk of photos, one after the other, with the count
/// done and where to continue, and a cancel flag checked between photos.
pub(crate) fn batch_with(s: &mut Session, p: &Value, src: &mut Sources) -> Result<Value> {
    const C: &str = "portrait.batch";
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { s.visible_cloned() };
    if ids.is_empty() {
        return Err(bad(C, "no photos: give `ids`, or select some, or filter the view"));
    }
    let new =
        if bool_or(p, "auto", false) || p.get("steps").is_none() && KINDS.iter().all(|i| p.get(i.id).is_none()) && p.get(ps::BACKGROUND).is_none() {
            ps::auto_steps()
        } else {
            ps::steps_from_params(p, C)?
        };
    let replace = bool_or(p, "replace", false);
    let (offset, chunk) = (f64_or(p, "offset", 0.0).max(0.0) as usize, (f64_or(p, "chunk", 8.0) as usize).clamp(1, 500));
    let end = (offset + chunk).min(ids.len());
    if offset == 0 {
        ps::clear_cancel();
    }
    let t = web_time::Instant::now();
    let (mut photos, mut cancelled, mut next) = (Vec::new(), false, offset);
    for id in ids.iter().take(end).skip(offset) {
        if ps::cancel_requested() {
            cancelled = true;
            break;
        }
        // one photo failing (unreadable file…) does not stop the album
        match apply_one(s, *id, new.clone(), replace, max_faces_of(p), src) {
            Ok(v) => photos.push(v),
            Err(e) => photos.push(json!({"id": id.0, "error": e.to_string()})),
        }
        next += 1;
    }
    let treated = photos.iter().filter(|v| v.get("faces").is_some()).count();
    let secs = t.elapsed().as_secs_f64();
    Ok(json!({
        "total": ids.len(), "done": next, "next": if next < ids.len() && !cancelled { json!(next) } else { Value::Null },
        "cancelled": cancelled, "retouched": treated, "photos": photos,
        "seconds": (secs * 1000.0).round() / 1000.0,
        "secondsPerPhoto": if photos.is_empty() { Value::Null } else { json!((secs / photos.len() as f64 * 1000.0).round() / 1000.0) },
    }))
}

fn cancel(_: &mut Session, _: &Value) -> Result<Value> {
    ps::request_cancel();
    Ok(json!({"cancelRequested": true}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    s.execute("spot.inpaint", p)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "portrait.openEyes",
            "Open Eyes",
            ["Photo"],
            None,
            "{id?, level?: 1|2|3 (1), source?: photo id of the reference (else the best shot of the same burst where the same person has open eyes), face?: index among the photo's faces (else the first with closed eyes), eyes?: both|left|right, apply?: false, confirmed?: false, edge?: 256..2048 (1024), path?} — put the eyes of the same person from another shot of the series into this photo (level 1: real pixels from a real frame, aligned on the eye corners, light and colour matched, feathered; nothing is invented). Always a preview first (before/after PNGs and the numbers: openBefore/openAfter, seamDeltaE, scale, rotationDeg); `apply: true` + `confirmed: true` stores it as the `open_eyes` step of the portrait stack, only for the pair previewed (one undo; the original is never touched). No usable reference → an error saying so. Level 2 (local generative) is not available: no local model with a permissive licence is good enough; level 3 (online) only prepares the eyes crop and asks for consent: nothing is sent → {id, source, face, before, after, eyes, ...}",
            has_active,
            open_eyes_cmd
        ),
        cmd!(
            query "portrait.list",
            "Portrait Sliders",
            [],
            None,
            "{id?} — the portrait sliders (id, label, group, help, auto default, and the current step of the photo) → {sliders: [{id, label, group, help, auto, current}]}",
            always,
            list
        ),
        cmd!(
            "portrait.apply",
            "Portrait Sliders",
            ["Photo"],
            None,
            "{id?|ids?, <slider>: 0..100 …, background?: {mode: blur|color|dim, amount, color?, shadow?, zoom?, rotate?, edge?}, steps?: [{kind, amount, enabled?}], auto?: true, replace?: false, maxFaces?: 1..8 (4)} — retouch the faces by named sliders (smooth, blemishes, freckles, wrinkles_eyes, wrinkles_forehead, wrinkles_lines, dark_circles, shine, double_chin, teeth, eyes, flyaways, neck_smooth, neck_light, background): the steps are kept in the photo's stack (given steps replace the same ones, `replace` starts the stack over, `auto` uses the default recipe, none re-runs the stack) and the pixels they make are a patch placed by a spot: the original is never touched, rendering needs no model; one undo step per photo → {photos: [{id, faces, steps: [{step, changed, found, skipped}], seconds}]}",
            has_active,
            apply
        ),
        cmd!(
            query "portrait.preview",
            "Portrait Preview",
            [],
            None,
            "{id?, <same sliders as portrait.apply>, edge?: 256..2048 (1024), path?} — what portrait.apply would give, small, without storing anything: writes the retouched and the original photo as PNG files → {before, after, width, height, faces, steps, seconds: {retouch, render}}",
            has_active,
            preview
        ),
        cmd!(
            "portrait.reset",
            "Reset Portrait Retouch",
            ["Photo"],
            None,
            "{id?|ids?, step?} — remove the whole portrait stack (the original look comes back), or one step (the others run again) → {reset}",
            has_active,
            reset
        ),
        cmd!(
            query "portrait.history",
            "Portrait History",
            [],
            None,
            "{id?} — the steps of the photo's stack with their amounts, what each changed, and `stale` when the pixels no longer match the white balance / lens / geometry → {steps, faces, patch, stale}",
            has_active,
            history
        ),
        cmd!(
            "portrait.batch",
            "Portrait Batch",
            [],
            None,
            "{ids? (else the selection, else the view), <sliders as portrait.apply> | auto, offset?: 0, chunk?: 1..500 (8), maxFaces?} — apply the same steps to a chunk of photos; no sliders = the auto recipe. One failing photo does not stop the others; photos without a face are skipped and reported. Call again with `offset` = the returned `next` until it is null; portrait.cancel stops it between two photos → {total, done, next, cancelled, retouched, photos, seconds, secondsPerPhoto}",
            always,
            batch
        ),
        cmd!(
            "portrait.auto",
            "Auto Portrait Retouch",
            ["Photo"],
            None,
            "{ids?, offset?, chunk?, maxFaces?} — portrait.batch with the default recipe (smoothing 35, blemishes 55, dark circles 40, shine 40, teeth 30, eyes 30, light wrinkles, neck 25) over the selection or the view",
            always,
            auto_batch
        ),
        cmd!(
            "portrait.cancel",
            "Cancel Portrait Batch",
            [],
            None,
            "{} — stop a running portrait.batch / portrait.apply after the photo it is on → {cancelRequested}",
            always,
            cancel
        ),
        cmd!(
            "portrait.remove",
            "Portrait Remove Object",
            [],
            None,
            "{rect|ellipse|points, size?} — remove an object (a stray person, a pimple-sized mark, a cable) with the MI-GAN inpainting model: same as spot.inpaint (see its parameters and inpaint.model.download)",
            has_active,
            remove
        ),
    ]
}

fn auto_batch(s: &mut Session, p: &Value) -> Result<Value> {
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        o.insert("auto".into(), json!(true));
    }
    batch(s, &q)
}

// ---- open eyes -------------------------------------------------------------------------------

/// The pair (photo, reference, face) of the last preview: `apply` only accepts what was seen.
static LAST_EYES_PREVIEW: std::sync::Mutex<Option<(u64, u64, usize)>> = std::sync::Mutex::new(None);

/// Names of the people the People index recognized in a photo, by face box (empty without it).
#[cfg(feature = "people")]
fn names_in(s: &mut Session, id: PhotoId) -> Vec<([f32; 2], Option<String>)> {
    let Ok(v) = crate::people::ops::faces(s, &json!({"id": id.0})) else { return Vec::new() };
    let Some(list) = v.get("faces").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .filter_map(|f| {
            let r = f.get("rect")?.as_array()?;
            let (x0, y0, x1, y1) = (r.first()?.as_f64()?, r.get(1)?.as_f64()?, r.get(2)?.as_f64()?, r.get(3)?.as_f64()?);
            Some(([((x0 + x1) / 2.0) as f32, ((y0 + y1) / 2.0) as f32], f.get("name").and_then(Value::as_str).map(str::to_string)))
        })
        .collect()
}

#[cfg(not(feature = "people"))]
fn names_in(_: &mut Session, _: PhotoId) -> Vec<([f32; 2], Option<String>)> {
    Vec::new()
}

fn centre(r: [f32; 4]) -> [f32; 2] {
    [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
}

fn name_at(names: &[([f32; 2], Option<String>)], c: [f32; 2]) -> Option<String> {
    names
        .iter()
        .filter(|(k, _)| (k[0] - c[0]).hypot(k[1] - c[1]) < 0.15)
        .min_by(|a, b| (a.0[0] - c[0]).hypot(a.0[1] - c[1]).total_cmp(&(b.0[0] - c[0]).hypot(b.0[1] - c[1])))
        .and_then(|x| x.1.clone())
}

/// The face of `id` whose eyes are closed (Analyze Faces' numbers), as (index in the analysis, its centre).
fn closed_face(s: &Session, id: PhotoId) -> Option<(usize, [f32; 2])> {
    let a = s.catalog.photo(id)?.analysis.as_ref()?;
    let faces = a.faces.as_ref()?;
    faces.iter().enumerate().find(|(_, f)| f.eyes_closed()).map(|(i, f)| (i, centre(f.rect)))
}

/// The best reference for the face at `c` of `id`: a photo of the same burst where a face at about
/// the same place (and, when People knows them, the same person) has both eyes open.
fn find_reference(s: &mut Session, id: PhotoId, c: [f32; 2]) -> Option<PhotoId> {
    use lightcraft_catalog::{EYE_OPEN_FROM, FaceInfo};
    let group = s.catalog.photo(id)?.analysis.as_ref()?.group?;
    let mine = names_in(s, id);
    let who = name_at(&mine, c);
    let cands: Vec<PhotoId> =
        s.catalog.photos().filter(|p| p.id != id && p.analysis.as_ref().and_then(|a| a.group) == Some(group)).map(|p| p.id).collect();
    let mut best: Option<(f32, PhotoId)> = None;
    for cid in cands {
        let Some(faces) = s.catalog.photo(cid).and_then(|p| p.analysis.as_ref()).and_then(|a| a.faces.clone()) else { continue };
        let theirs = names_in(s, cid);
        for f in faces.iter() {
            let k = centre(f.rect);
            if (k[0] - c[0]).hypot(k[1] - c[1]) > 0.2 || !FaceInfo::eyes_judged(f) || f.open[0].min(f.open[1]) < EYE_OPEN_FROM * 0.9 {
                continue;
            }
            // two named people must be the same one
            if let (Some(a), Some(b)) = (&who, name_at(&theirs, k))
                && *a != b
            {
                continue;
            }
            let q = f.quality();
            if best.is_none_or(|(bq, _)| q > bq) {
                best = Some((q, cid));
            }
        }
    }
    best.map(|b| b.1)
}

fn open_eyes_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut f, mut m) = (real_faces, real_matte);
    let mut src = real_sources(&mut f, &mut m);
    open_eyes_with(s, p, &mut src)
}

pub(crate) fn open_eyes_with(s: &mut Session, p: &Value, src: &mut Sources) -> Result<Value> {
    const C: &str = "portrait.openEyes";
    let id = ids_of(s, p).first().copied().ok_or_else(|| bad(C, "no active photo"))?;
    let level = f64_or(p, "level", 1.0) as u32;
    match level {
        1 => {}
        2 => {
            return Err(bad(
                C,
                "level 2 (local generative repair of the eyes) is not available: no local model with a licence LightCraft can ship draws a given person's open eyes well enough (see docs/portrait-retouch.md, Open eyes). Use level 1 with another shot of the series.",
            ));
        }
        3 => return prepare_online(s, id, p, src),
        _ => return Err(bad(C, "`level` must be 1, 2 or 3")),
    }
    // which face
    let meshes = (src.faces)(s, id)?;
    let (face_idx, c) = match p.get("face").and_then(Value::as_u64) {
        Some(i) => {
            let m = meshes.get(i as usize).ok_or_else(|| bad(C, format!("no face number {i} (the photo has {})", meshes.len())))?;
            (i as usize, ps::centre_of(m).unwrap_or([0.5, 0.5]))
        }
        None => {
            let (_, c) = closed_face(s, id)
                .ok_or_else(|| bad(C, "no face with closed eyes is known for this photo: run photo.analyzeFaces first, or give `face`"))?;
            let (i, d) = ps::nearest_face(&meshes, c).ok_or_else(|| bad(C, "no face found in this photo"))?;
            if d > 0.25 {
                return Err(bad(C, "the closed face of the analysis is not among the faces found now: give `face`"));
            }
            (i, c)
        }
    };
    // which reference
    let source = match p.get("source").and_then(Value::as_u64) {
        Some(x) => PhotoId(x),
        None => find_reference(s, id, c).ok_or_else(|| {
            bad(C, "no reference photo (pas de photo de référence): no other shot of this burst shows this person with both eyes open")
        })?,
    };
    let eyes = str_param(p, "eyes").unwrap_or("both");
    let step = ps::step_from_json(&json!({"kind": ps::OPEN_EYES, "source": source.0, "face": face_idx, "eyes": eyes}), C)?;
    let apply = bool_or(p, "apply", false);
    if apply {
        let seen = *LAST_EYES_PREVIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !bool_or(p, "confirmed", false) || seen != Some((id.0, source.0, face_idx)) {
            return Err(bad(
                C,
                "open eyes is only applied after a preview of this very photo, reference and face has been looked at: call it without `apply`, look at `before`/`after`, then call again with `apply: true, confirmed: true`",
            ));
        }
        let r = apply_one(s, id, vec![step], false, max_faces_of(p), src)?;
        return Ok(json!({"id": id.0, "source": source.0, "face": face_idx, "applied": true, "result": r}));
    }
    let mut q = json!({"id": id.0, "steps": [{"kind": ps::OPEN_EYES, "source": source.0, "face": face_idx, "eyes": eyes}], "edge": f64_or(p, "edge", 1024.0)});
    if let Some(path) = str_param(p, "path") {
        q["path"] = json!(path);
    }
    let mut r = preview_with(s, &q, src)?;
    *LAST_EYES_PREVIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((id.0, source.0, face_idx));
    if let Some(o) = r.as_object_mut() {
        o.insert("source".into(), json!(source.0));
        o.insert("face".into(), json!(face_idx));
        o.insert("needsConfirmation".into(), json!(true));
    }
    Ok(r)
}

/// Level 3 (online generative repair) is only prepared here: the crop that would leave the Mac is
/// written for the user to look at, and nothing is sent.
fn prepare_online(s: &mut Session, id: PhotoId, p: &Value, src: &mut Sources) -> Result<Value> {
    const C: &str = "portrait.openEyes";
    let meshes = (src.faces)(s, id)?;
    let face = p.get("face").and_then(Value::as_u64).unwrap_or(0) as usize;
    let m = meshes.get(face).ok_or_else(|| bad(C, "no such face"))?;
    let d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
    let job = s.preview_job(id, 2048, 2048, false, &d).ok_or_else(|| bad(C, "no such photo"))?;
    let img = job.run().rendered.map_err(|e| bad(C, e))?.image;
    let (w, h) = (img.width, img.height);
    let eye_pts: Vec<[f32; 2]> = lightcraft_retouch::facegeo::LEFT_EYE
        .iter()
        .chain(lightcraft_retouch::facegeo::RIGHT_EYE.iter())
        .filter_map(|&i| m.get(i))
        .map(|q| [q[0] * w as f32, q[1] * h as f32])
        .collect();
    if eye_pts.is_empty() {
        return Err(bad(C, "the face mesh has no eyes"));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for q in &eye_pts {
        x0 = x0.min(q[0]);
        y0 = y0.min(q[1]);
        x1 = x1.max(q[0]);
        y1 = y1.max(q[1]);
    }
    let pad = 0.35 * (x1 - x0).max(1.0);
    let (cx0, cy0) = (((x0 - pad).max(0.0)) as usize, ((y0 - pad).max(0.0)) as usize);
    let (cx1, cy1) = (((x1 + pad) as usize).min(w), ((y1 + pad) as usize).min(h));
    if cx1 <= cx0 + 4 || cy1 <= cy0 + 4 {
        return Err(bad(C, "the eyes are outside the frame"));
    }
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    let data: Vec<[u8; 4]> = (0..ch).flat_map(|y| (0..cw).map(move |x| (x, y))).map(|(x, y)| img.data[(cy0 + y) * w + cx0 + x]).collect();
    let crop = lightcraft_raster::Rgba8 { width: cw, height: ch, data };
    let path = std::env::temp_dir().join("lightcraft-portrait").join(format!("{}-eyes-to-send.png", id.0));
    write_png(&crop, &path, C)?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "level": 3,
        "sent": false,
        "needsConsent": true,
        "willLeaveTheMac": {"what": "a crop of the eyes region only, no metadata (no EXIF, no GPS, no file name)", "pixels": [cw, ch], "bytes": bytes, "file": path.display().to_string()},
        "note": "Nothing was sent. Sending a face crop to an online image model needs Madson's explicit yes for this photo, after looking at the file above. This build does not call any online service."
    }))
}
