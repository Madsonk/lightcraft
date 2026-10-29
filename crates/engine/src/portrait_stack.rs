//! The portrait retouch stack (`portrait.apply`, `portrait.preview`, `portrait.batch`…): named
//! sliders (blemishes, dark circles, teeth, background…) kept as an ordered list of steps in the
//! photo's develop settings. Rendering the stack finds the faces, runs the steps of
//! `lightcraft_retouch::portrait` on the photo at the scene-linear stage, and bakes the result
//! into an inpaint patch that an ordinary spot places, so the original file and its other edits
//! are never touched and rendering needs no model.
//!
//! The face finder and the subject matte are passed in (the commands give the real models; the
//! tests give synthetic ones), so everything else is testable without a download.

use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, InpaintPatch, PortraitStep, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_retouch::facegeo::OVAL;
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::portrait::{self, Crop, Kind, background};
use serde_json::{Value, json};

use crate::cmd::bad;
use crate::{Result, Session};

/// Longest edge of the scene-linear picture the steps run on (and so of the baked patch).
pub const EDGE: usize = 4096;
/// Longest edge of a preview.
pub const PREVIEW_EDGE: usize = 1024;

/// The order the steps run in, whatever order they were given in: marks first (so smoothing does
/// not smear them), then the shapes, the colour, the mouth and eyes, the hair, the background.
const ORDER: [&str; 14] = [
    "blemishes",
    "freckles",
    "wrinkles_eyes",
    "wrinkles_forehead",
    "wrinkles_lines",
    "dark_circles",
    "shine",
    "double_chin",
    "smooth",
    "neck_smooth",
    "neck_light",
    "teeth",
    "eyes",
    "flyaways",
];
/// [`ORDER`] for the other modules.
pub const ORDER_IDS: [&str; 14] = ORDER;
pub const BACKGROUND: &str = "background";
/// The step that takes the eyes of the same person from another shot (see `portrait.openEyes`).
pub const OPEN_EYES: &str = "open_eyes";

static CANCEL: AtomicBool = AtomicBool::new(false);

/// Ask the running batch to stop after the photo it is on (also `portrait.cancel`).
pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub(crate) fn clear_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

pub(crate) fn cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

/// Face meshes of a photo, normalized to the uncropped photo (x, y in 0..1; z in widths), the
/// biggest first.
pub type Meshes = Vec<Vec<[f32; 3]>>;

pub struct Sources<'a> {
    pub faces: &'a mut dyn FnMut(&mut Session, PhotoId) -> Result<Meshes>,
    /// The subject matte of the photo at `w x h` (1 = subject).
    pub matte: &'a mut dyn FnMut(&mut Session, PhotoId, usize, usize) -> Result<Plane>,
}

/// What one render of the stack found out.
pub struct Outcome {
    pub w: usize,
    pub h: usize,
    pub faces: usize,
    pub skipped_faces: usize,
    pub patch: Option<InpaintPatch>,
    /// Per step id: share of the pixels changed (0..1), things found, why nothing happened.
    pub reports: Vec<(String, f64, Option<usize>, Option<String>)>,
    pub seconds: f64,
    pub changed_share: f64,
    /// What the open-eyes step did to each eye (empty without that step).
    pub eyes: Vec<lightcraft_retouch::portrait::eyes::EyeReport>,
}

/// A step from JSON: `{kind, amount, enabled?, mode?, color?, shadow?, zoom?, rotate?, edge?, dim?}`.
pub fn step_from_json(v: &Value, c: &str) -> Result<PortraitStep> {
    let kind = v.get("kind").and_then(Value::as_str).ok_or_else(|| bad(c, "a step needs a `kind`"))?;
    if kind != BACKGROUND && kind != OPEN_EYES && portrait::info_of(kind).is_none() {
        let all: Vec<&str> = portrait::KINDS.iter().map(|i| i.id).chain([BACKGROUND, OPEN_EYES]).collect();
        return Err(bad(c, format!("unknown step `{kind}` (one of {})", all.join(", "))));
    }
    let amount = v.get("amount").and_then(Value::as_f64).unwrap_or(40.0);
    if !amount.is_finite() {
        return Err(bad(c, "`amount` must be a number"));
    }
    let mut step = PortraitStep { kind: kind.to_string(), amount: amount.clamp(0.0, 100.0), ..Default::default() };
    if let Some(e) = v.get("enabled").and_then(Value::as_bool) {
        step.enabled = e;
    }
    if kind == OPEN_EYES {
        // options: source (photo id of the reference), face (index among the photo's faces), eyes (0 both, 1 left, 2 right)
        let source =
            v.get("source").and_then(Value::as_u64).ok_or_else(|| bad(c, "`open_eyes` needs `source`: the id of the photo with the open eyes"))?;
        step.options.push(("source".into(), source as f64));
        step.options.push(("face".into(), v.get("face").and_then(Value::as_u64).unwrap_or(0).min(15) as f64));
        let eyes = match v.get("eyes").and_then(Value::as_str) {
            None | Some("both") => 0.0,
            Some("left") => 1.0,
            Some("right") => 2.0,
            Some(_) => return Err(bad(c, "`eyes` must be both, left or right")),
        };
        step.options.push(("eyes".into(), eyes));
        step.amount = 100.0;
    }
    if kind == BACKGROUND {
        let mode = v.get("mode").and_then(Value::as_str).unwrap_or("blur");
        step.text = Some(match mode {
            "blur" | "dim" => mode.to_string(),
            "color" => {
                let hex = v.get("color").and_then(Value::as_str).unwrap_or("#808080");
                lightcraft_retouch::lab::parse_hex(hex).ok_or_else(|| bad(c, "`color` must be #rrggbb"))?;
                format!("color:{hex}")
            }
            _ => return Err(bad(c, "background `mode` must be blur, color or dim")),
        });
        for k in ["shadow", "zoom", "rotate", "edge", "dim"] {
            if let Some(x) = v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()) {
                let (lo, hi) = match k {
                    "rotate" => (-45.0, 45.0),
                    "dim" => (-100.0, 100.0),
                    "zoom" => (0.0, 60.0),
                    _ => (0.0, 100.0),
                };
                step.options.push((k.to_string(), x.clamp(lo, hi)));
            }
        }
    }
    Ok(step)
}

/// Steps from the convenient form `{"blemishes": 60, "dark_circles": 40, "background": {...}}`
/// or the list form `[{kind, amount…}]`.
pub fn steps_from_params(p: &Value, c: &str) -> Result<Vec<PortraitStep>> {
    if let Some(list) = p.get("steps").and_then(Value::as_array) {
        return list.iter().map(|v| step_from_json(v, c)).collect();
    }
    let mut out = Vec::new();
    for info in portrait::KINDS {
        if let Some(a) = p.get(info.id).and_then(Value::as_f64) {
            out.push(step_from_json(&json!({"kind": info.id, "amount": a}), c)?);
        }
    }
    if let Some(b) = p.get(BACKGROUND) {
        let mut b = b.clone();
        if let Some(o) = b.as_object_mut() {
            o.insert("kind".into(), json!(BACKGROUND));
            o.entry("amount").or_insert(json!(60));
        } else if let Some(a) = b.as_f64() {
            b = json!({"kind": BACKGROUND, "amount": a});
        }
        out.push(step_from_json(&b, c)?);
    }
    Ok(out)
}

/// The steps of "auto retouch": the sliders that have a sensible default, at that default.
pub fn auto_steps() -> Vec<PortraitStep> {
    portrait::KINDS
        .iter()
        .filter(|i| i.auto > 0.0)
        .map(|i| PortraitStep { kind: i.id.to_string(), amount: f64::from(i.auto * 100.0).round(), ..Default::default() })
        .collect()
}

/// `stack` updated with `new`: a step of the same kind is replaced in place, others are appended.
pub fn merge(stack: &mut Vec<PortraitStep>, new: Vec<PortraitStep>) {
    for n in new {
        match stack.iter_mut().find(|s| s.kind == n.kind) {
            Some(s) => *s = n,
            None => stack.push(n),
        }
    }
}

fn background_params(step: &PortraitStep) -> background::Params {
    let text = step.text.as_deref().unwrap_or("blur");
    let mode = match text {
        "dim" => background::Mode::Dim,
        t if t.starts_with("color:") => background::Mode::Color(lightcraft_retouch::lab::parse_hex(&t[6..]).unwrap_or([128, 128, 128])),
        _ => background::Mode::Blur,
    };
    let o = |k: &str, d: f64| step.option(k).unwrap_or(d) as f32;
    background::Params {
        mode,
        amount: (step.amount / 100.0) as f32,
        dim: if mode == background::Mode::Dim { -(step.amount as f32) / 100.0 } else { o("dim", 0.0) / 100.0 },
        shadow: o("shadow", 0.0) / 100.0,
        zoom: 1.0 + o("zoom", 0.0) / 100.0,
        rotate_deg: o("rotate", 0.0),
        edge: o("edge", 50.0) / 100.0,
    }
}

fn kind_of(id: &str) -> Option<Kind> {
    portrait::info_of(id).map(|i| i.kind)
}

/// The photo's scene-linear picture as 8-bit sRGB, without any portrait patch, at most `edge` long.
fn linear_rgb(s: &mut Session, id: PhotoId, d: &DevelopSettings, edge: usize, c: &str) -> Result<(Vec<u8>, usize, usize)> {
    let job = s.render_job(id, edge, edge, false, false).ok_or_else(|| bad(c, "no such photo"))?;
    let source = job.source.load_source().map_err(|e| bad(c, e))?;
    let info = source.info_or(job.info);
    // never bigger than the file itself (the pipeline would scale a small photo up to the edge)
    let edge = edge.min(source.image.width.max(source.image.height)).max(16);
    let lin = lightcraft_pipeline::linear_stage(&source.image, &info, d, edge);
    drop(source);
    let (w, h) = (lin.width, lin.height);
    Ok((lightcraft_pipeline::inpaint::to_srgb8(&lin), w, h))
}

/// The develop settings the stack renders from: the photo's own, without the previous portrait
/// pixels.
pub fn base_settings(d: &DevelopSettings) -> DevelopSettings {
    let mut b = d.clone();
    if let Some(old) = b.portrait.patch.clone() {
        b.spots.retain(|sp| sp.patch.as_deref() != Some(old.as_str()));
    }
    b.portrait.patch = None;
    b
}

/// Run `steps` on photo `id` (its settings without the previous portrait pixels) and bake the
/// result. Changes nothing in the session.
pub fn render(
    s: &mut Session,
    id: PhotoId,
    d: &DevelopSettings,
    steps: &[PortraitStep],
    edge: usize,
    max_faces: usize,
    src: &mut Sources,
) -> Result<Outcome> {
    const C: &str = "portrait.apply";
    let t = web_time::Instant::now();
    let on: Vec<&PortraitStep> = steps.iter().filter(|st| st.enabled && st.amount > 0.0).collect();
    let has_face_steps = ORDER.iter().any(|k| on.iter().any(|st| st.kind == *k));
    let eyes_step = on.iter().find(|st| st.kind == OPEN_EYES).copied();
    let (base, w, h) = linear_rgb(s, id, d, edge, C)?;
    let mut out = base.clone();
    let mut eye_reports = Vec::new();
    let mut eyes_changed = None;
    let mut meshes: Meshes = Vec::new();
    if has_face_steps || eyes_step.is_some() {
        meshes = (src.faces)(s, id)?;
        if let Some(st) = eyes_step {
            let before = out.clone();
            let (reports, why) = open_eyes(s, id, &mut out, w, h, &meshes, st, edge, src)?;
            eye_reports = reports;
            let moved = (0..w * h).filter(|&i| before[i * 3..i * 3 + 3] != out[i * 3..i * 3 + 3]).count();
            eyes_changed = Some((moved as f64 / (w * h) as f64, why));
        }
    }
    let input = CoreInput { base, out, w, h, steps, meshes: &meshes, max_faces, eye_reports, eyes_changed, started: t };
    let mut matte = |mw: usize, mh: usize| (src.matte)(s, id, mw, mh);
    render_core(input, &mut matte, &|| false)
}

/// Everything [`render_core`] needs once the photo's picture and faces are in hand.
pub struct CoreInput<'a> {
    /// The scene-linear picture as 8-bit sRGB, and the same after the steps that came before the
    /// face steps (open eyes).
    pub base: Vec<u8>,
    pub out: Vec<u8>,
    pub w: usize,
    pub h: usize,
    pub steps: &'a [PortraitStep],
    pub meshes: &'a Meshes,
    pub max_faces: usize,
    pub eye_reports: Vec<lightcraft_retouch::portrait::eyes::EyeReport>,
    pub eyes_changed: Option<(f64, Option<String>)>,
    pub started: web_time::Instant,
}

/// The cancelled outcome of the worker steps: an error nobody shows (the run was stopped).
pub fn cancelled() -> crate::EngineError {
    crate::EngineError::Other("cancelled".to_string())
}

pub fn is_cancelled(e: &crate::EngineError) -> bool {
    e.to_string().contains("cancelled")
}

/// The steps on the faces, the background, and the baking of what moved into one patch. Needs no
/// session: `matte_of` gives the subject matte at `w × h` when the background step is on, and
/// `cancelled` is looked at before each face and between two steps (the result is then an error).
pub fn render_core(input: CoreInput, matte_of: &mut dyn FnMut(usize, usize) -> Result<Plane>, cancelled_now: &dyn Fn() -> bool) -> Result<Outcome> {
    const C: &str = "portrait.apply";
    let CoreInput { base, mut out, w, h, steps, meshes, max_faces, eye_reports, eyes_changed, started: t } = input;
    let on: Vec<&PortraitStep> = steps.iter().filter(|st| st.enabled && st.amount > 0.0).collect();
    let face_steps: Vec<(Kind, f32)> = ORDER
        .iter()
        .filter_map(|k| on.iter().find(|st| st.kind == *k).and_then(|st| kind_of(&st.kind).map(|kk| (kk, (st.amount / 100.0) as f32))))
        .collect();
    let bg_step = on.iter().find(|st| st.kind == BACKGROUND).copied();
    let mut sums: Vec<(Kind, f32, usize, usize, Option<&'static str>)> = face_steps.iter().map(|(k, _)| (*k, 0.0, 0usize, 0usize, None)).collect();
    let (mut faces_done, mut skipped) = (0usize, 0usize);
    if !face_steps.is_empty() || eyes_changed.is_some() {
        skipped = meshes.len().saturating_sub(max_faces);
        for mesh in meshes.iter().take(if face_steps.is_empty() { 0 } else { max_faces }) {
            if cancelled_now() {
                return Err(cancelled());
            }
            let pts: Vec<[f32; 3]> = mesh.iter().map(|p| [p[0] * w as f32, p[1] * h as f32, p[2] * w as f32]).collect();
            if pts.len() < 468 {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for &i in &OVAL {
                let p = pts[i];
                if !(p[0].is_finite() && p[1].is_finite()) {
                    continue;
                }
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            let fw = x1 - x0;
            if !(fw.is_finite() && fw >= 40.0) {
                skipped += 1;
                continue;
            }
            // the face with room around it: hair above, neck below
            let cx0 = ((x0 - 0.55 * fw).floor().max(0.0)) as usize;
            let cy0 = ((y0 - 0.55 * fw).floor().max(0.0)) as usize;
            let cx1 = ((x1 + 0.55 * fw).ceil() as usize).min(w);
            let cy1 = ((y1 + 0.9 * fw).ceil() as usize).min(h);
            if cx1 <= cx0 + 8 || cy1 <= cy0 + 8 {
                continue;
            }
            let (cw, ch) = (cx1 - cx0, cy1 - cy0);
            let mut crop = vec![0u8; cw * ch * 3];
            for row in 0..ch {
                let src_row = ((cy0 + row) * w + cx0) * 3;
                crop[row * cw * 3..(row + 1) * cw * 3].copy_from_slice(&out[src_row..src_row + cw * 3]);
            }
            let local: Vec<[f32; 3]> = pts.iter().map(|p| [p[0] - cx0 as f32, p[1] - cy0 as f32, p[2]]).collect();
            let Some(res) = portrait::retouch_face_until(Crop { w: cw, h: ch, rgb: &mut crop }, &local, &face_steps, cancelled_now) else {
                if cancelled_now() {
                    return Err(cancelled());
                }
                skipped += 1;
                continue;
            };
            faces_done += 1;
            for row in 0..ch {
                let dst = ((cy0 + row) * w + cx0) * 3;
                out[dst..dst + cw * 3].copy_from_slice(&crop[row * cw * 3..(row + 1) * cw * 3]);
            }
            for (acc, r) in sums.iter_mut().zip(&res.reports) {
                acc.1 += r.changed;
                acc.2 += r.found.unwrap_or(0);
                acc.3 += 1;
                if r.skipped.is_some() {
                    acc.4 = r.skipped;
                }
            }
        }
    }
    let mut reports: Vec<(String, f64, Option<usize>, Option<String>)> = sums
        .iter()
        .map(|(k, ch, found, n, why)| {
            let id = portrait::KINDS.iter().find(|i| i.kind == *k).map_or("", |i| i.id).to_string();
            let found = (matches!(k, Kind::Blemishes | Kind::Flyaways)).then_some(*found);
            (
                id,
                if *n > 0 { f64::from(*ch) / *n as f64 } else { 0.0 },
                found,
                if faces_done == 0 { Some("no face found".to_string()) } else { why.map(str::to_string) },
            )
        })
        .collect();
    if let Some((share, why)) = eyes_changed {
        let n = eye_reports.iter().filter(|e| e.skipped.is_none()).count();
        reports.insert(0, (OPEN_EYES.to_string(), share, Some(n), why));
        faces_done = faces_done.max(1);
    }
    if let Some(step) = bg_step {
        if cancelled_now() {
            return Err(cancelled());
        }
        let matte = matte_of(w, h)?;
        let params = background_params(step);
        let new = background::replace(&out, w, h, &matte, &params).ok_or_else(|| bad(C, "the subject matte does not fit the photo"))?;
        let moved = (0..w * h).filter(|&i| new[i * 3..i * 3 + 3] != out[i * 3..i * 3 + 3]).count();
        out = new;
        reports.push((BACKGROUND.to_string(), moved as f64 / (w * h) as f64, None, None));
    }
    // what moved, and its box
    let (mut bx0, mut by0, mut bx1, mut by1, mut n) = (usize::MAX, usize::MAX, 0usize, 0usize, 0usize);
    for y in 0..h {
        let (a, b) = (&base[y * w * 3..(y + 1) * w * 3], &out[y * w * 3..(y + 1) * w * 3]);
        if a == b {
            continue;
        }
        for x in 0..w {
            if a[x * 3..x * 3 + 3] != b[x * 3..x * 3 + 3] {
                bx0 = bx0.min(x);
                bx1 = bx1.max(x);
                by0 = by0.min(y);
                by1 = by1.max(y);
                n += 1;
            }
        }
    }
    let patch = if n == 0 {
        None
    } else {
        // a one pixel border keeps the interpolation at the edge on unchanged pixels
        let (x0, y0) = (bx0.saturating_sub(1), by0.saturating_sub(1));
        let (x1, y1) = ((bx1 + 2).min(w), (by1 + 2).min(h));
        let (pw, ph) = (x1 - x0, y1 - y0);
        let mut rgb = vec![0u8; pw * ph * 3];
        let mut alpha = vec![0u8; pw * ph];
        for y in 0..ph {
            for x in 0..pw {
                let i = (y0 + y) * w + x0 + x;
                rgb[(y * pw + x) * 3..(y * pw + x) * 3 + 3].copy_from_slice(&out[i * 3..i * 3 + 3]);
                alpha[y * pw + x] = if base[i * 3..i * 3 + 3] != out[i * 3..i * 3 + 3] { 255 } else { 0 };
            }
        }
        let rect = [x0 as f64 / w as f64, y0 as f64 / h as f64, x1 as f64 / w as f64, y1 as f64 / h as f64];
        InpaintPatch::new(rect, pw, ph, rgb, alpha)
    };
    Ok(Outcome {
        eyes: eye_reports,
        w,
        h,
        faces: faces_done,
        skipped_faces: skipped,
        patch,
        reports,
        seconds: t.elapsed().as_secs_f64(),
        changed_share: n as f64 / (w * h) as f64,
    })
}

/// A spot that places `patch` (the rectangle it covers gives the pin).
fn spot_for(patch: &InpaintPatch) -> Spot {
    let r = patch.rect;
    let centre = Point::new((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    Spot {
        mode: SpotMode::Inpaint,
        points: vec![centre],
        size: ((r[2] - r[0]).max(r[3] - r[1]) / 2.0).clamp(0.002, 0.5),
        feather: 0.0,
        opacity: 100.0,
        source_offset: None,
        patch: Some(patch.hex_id()),
    }
}

/// Fingerprint of what the baked pixels depend on (the scene-linear stage: white balance,
/// profile, lens correction, geometry, orientation). Tone, colour and masks come after it.
pub fn linear_key(d: &DevelopSettings) -> u64 {
    let parts = json!([d.profile, d.wb, d.optics, d.geometry, d.orientation, d.treatment]);
    let text = serde_json::to_string(&parts).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h | 1
}

/// The settings with `steps` as the stack and `patch` placed by a spot.
pub fn with_stack(base: &DevelopSettings, steps: Vec<PortraitStep>, outcome: &Outcome, patch: Option<&InpaintPatch>) -> DevelopSettings {
    let mut d = base.clone();
    d.portrait.steps = steps;
    d.portrait.faces = outcome.faces;
    d.portrait.key = linear_key(base);
    match patch {
        Some(p) => {
            d.spots.push(spot_for(p));
            d.portrait.patch = Some(p.hex_id());
        }
        None => d.portrait.patch = None,
    }
    d
}

/// Record on each step what it changed last time.
pub fn annotate(steps: &mut [PortraitStep], o: &Outcome) {
    for st in steps.iter_mut() {
        st.changed = o.reports.iter().find(|r| r.0 == st.kind).map(|r| (r.1 * 1000.0).round() / 1000.0);
    }
}

pub fn report_json(o: &Outcome) -> Value {
    json!(
        o.reports
            .iter()
            .map(|(k, ch, found, why)| json!({"step": k, "changed": (ch * 1000.0).round() / 1000.0, "found": found, "skipped": why}))
            .collect::<Vec<_>>()
    )
}

pub fn centre_of(mesh: &[[f32; 3]]) -> Option<[f32; 2]> {
    let pts: Vec<[f32; 3]> = OVAL.iter().filter_map(|&i| mesh.get(i).copied()).filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f32;
    Some([pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[1]).sum::<f32>() / n])
}

/// The index of the mesh whose centre is nearest `c` (normalized coordinates) and its distance.
pub fn nearest_face(meshes: &Meshes, c: [f32; 2]) -> Option<(usize, f32)> {
    meshes.iter().enumerate().filter_map(|(i, m)| centre_of(m).map(|k| (i, (k[0] - c[0]).hypot(k[1] - c[1])))).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The open-eyes step: the eyes of the same person from the reference photo `source` go into the
/// face `face` of this photo. `(reports, why nothing happened)`.
#[allow(clippy::too_many_arguments)]
fn open_eyes(
    s: &mut Session,
    id: PhotoId,
    out: &mut [u8],
    w: usize,
    h: usize,
    meshes: &Meshes,
    st: &PortraitStep,
    edge: usize,
    src: &mut Sources,
) -> Result<(Vec<lightcraft_retouch::portrait::eyes::EyeReport>, Option<String>)> {
    const C: &str = "portrait.openEyes";
    let sid = PhotoId(st.option("source").unwrap_or(0.0) as u64);
    if sid == id {
        return Err(bad(C, "the reference must be another photo"));
    }
    let face = st.option("face").unwrap_or(0.0) as usize;
    let sides = match st.option("eyes").unwrap_or(0.0) as u8 {
        1 => portrait::eyes::Sides::Left,
        2 => portrait::eyes::Sides::Right,
        _ => portrait::eyes::Sides::Both,
    };
    let Some(target) = meshes.get(face) else { return Ok((Vec::new(), Some(format!("no face number {face} in this photo")))) };
    let tc = centre_of(target).unwrap_or([0.5, 0.5]);
    let sd = base_settings(&*s.develop_of(sid).ok_or_else(|| bad(C, "the reference photo does not exist"))?);
    let (srgb, sw, sh) = linear_rgb(s, sid, &sd, edge, C)?;
    let smeshes = (src.faces)(s, sid)?;
    let Some((si, dist)) = nearest_face(&smeshes, tc) else { return Ok((Vec::new(), Some("no face found in the reference photo".into()))) };
    if dist > 0.25 {
        return Ok((Vec::new(), Some("the reference's face is not where this one is".into())));
    }
    let px =
        |m: &[[f32; 3]], w: usize, h: usize| -> Vec<[f32; 3]> { m.iter().map(|p| [p[0] * w as f32, p[1] * h as f32, p[2] * w as f32]).collect() };
    let (dpts, spts) = (px(target, w, h), px(&smeshes[si], sw, sh));
    let Some(res) = portrait::eyes::transfer(
        portrait::eyes::Target { pix: out, w, h, pts: &dpts },
        portrait::eyes::Reference { pix: &srgb, w: sw, h: sh, pts: &spts },
        sides,
    ) else {
        return Ok((Vec::new(), Some("the faces could not be aligned".into())));
    };
    let why =
        res.eyes.iter().all(|e| e.skipped.is_some()).then(|| res.eyes.iter().filter_map(|e| e.skipped).next().unwrap_or("nothing done").to_string());
    Ok((res.eyes, why))
}

pub fn eyes_json(o: &Outcome) -> Value {
    json!(o
        .eyes
        .iter()
        .map(|e| json!({
            "side": e.side, "skipped": e.skipped, "scale": (e.scale * 1000.0).round() / 1000.0, "rotationDeg": (e.rotation_deg * 100.0).round() / 100.0,
            "openBefore": (e.open_before * 1000.0).round() / 1000.0, "openAfter": (e.open_after * 1000.0).round() / 1000.0,
            "seamDeltaE": (e.seam_de * 100.0).round() / 100.0
        }))
        .collect::<Vec<_>>())
}
