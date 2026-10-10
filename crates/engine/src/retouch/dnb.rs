//! Guided dodge & burn (`retouch.dodgeBurn`): editable masks that lighten and darken, drawn from
//! the face mesh (478 points), the depth of the face and the subject, never from guessing at the
//! pixels. Two roles that are not mixed up: *cleanup* evens out the skin's luminosity
//! irregularities (dark circles, patches, folds), *sculpt* adds a little relief; eyes, lips, the
//! background and the edges are finishing. Manual Dodge and Burn brushes (`retouch.dodge`,
//! `retouch.burn`) can be limited to a depth range when the photo has a depth map.

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_retouch::dnb::{self, Adj, Mode, Part, Role};
use lightcraft_retouch::facegeo;
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::skin as sk;
use serde_json::{Value, json};

use super::analysis::{self, Found, Frame};
use super::outmask::{self, FacePlane};
use super::skin::{PhotoSkin, SkinFace, round2, sigmas};
use crate::cmd::{CommandSpec, always, bad, bool_or, cmd, f64_or, has_active, str_param};
use crate::{Result, Session};

/// Prefix of the masks of this step.
pub const DNB: &str = "Retouch: D&B";

pub fn adj_of(a: &Adj, k: f32) -> LocalAdjustments {
    LocalAdjustments {
        exposure: f64::from(a.exposure * k),
        contrast: f64::from(a.contrast * k),
        saturation: f64::from(a.saturation * k),
        highlights: f64::from(a.highlights * k),
        shadows: f64::from(a.shadows * k),
        ..Default::default()
    }
}

/// Parse `role`: a name or a list; `all` (default) is every role.
pub fn roles_of(p: &Value) -> Vec<Role> {
    let all = vec![Role::Cleanup, Role::Sculpt, Role::Eyes, Role::Lips, Role::Finish];
    let one = |s: &str| match s {
        "cleanup" => Some(Role::Cleanup),
        "sculpt" => Some(Role::Sculpt),
        "eyes" => Some(Role::Eyes),
        "lips" => Some(Role::Lips),
        "finish" => Some(Role::Finish),
        _ => None,
    };
    match p.get("role") {
        Some(Value::String(s)) if s != "all" && s != "both" => one(s).into_iter().collect(),
        Some(Value::String(s)) if s == "both" => vec![Role::Cleanup, Role::Sculpt],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).filter_map(one).collect(),
        _ => all,
    }
}

/// The subject of a frame, soft (0..1) and where it came from: the matting model when it is
/// installed, else a guess (the faces' bodies for a portrait, the colourful bright middle else).
pub fn subject_plane(s: &mut Session, frame: &Frame, found: &[Found]) -> (Plane, &'static str) {
    let _ = &s;
    #[cfg(feature = "autoseg")]
    {
        let img = lightcraft_raster::Rgba8 { width: frame.w, height: frame.h, data: frame.rgba.clone() };
        if let Ok((seg, _)) = crate::autoseg::segment(&img, &crate::autoseg::Kind::Subject)
            && let Some(p) = seg_plane(&seg, frame.w, frame.h)
        {
            return (p, "subject model");
        }
    }
    if !found.is_empty() {
        let mut out = Plane::new(frame.w, frame.h, 0.0);
        for f in found {
            let fw = f.face.width;
            let c = [(f.face.bbox[0] + f.face.bbox[2]) / 2.0, f.face.bbox[1] + 0.5 * (f.face.bbox[3] - f.face.bbox[1])];
            // the head and the shoulders below it
            let head = ellipse(frame.w, frame.h, c, 0.75 * fw, 0.9 * fw, 0.12 * fw);
            let body = ellipse(frame.w, frame.h, [c[0], c[1] + 1.4 * fw], 1.7 * fw, 1.5 * fw, 0.35 * fw);
            out = out.zip(&head, f32::max).zip(&body, f32::max);
        }
        return (out, "faces");
    }
    (dnb::saliency(&frame.lab), "saliency")
}

fn ellipse(w: usize, h: usize, c: [f32; 2], rx: f32, ry: f32, soft: f32) -> Plane {
    let mut p = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 + 0.5 - c[0]) / rx, (y as f32 + 0.5 - c[1]) / ry);
            let d = (dx * dx + dy * dy).sqrt();
            let k = soft / rx.min(ry).max(1.0);
            p.d[y * w + x] = 1.0 - lightcraft_retouch::plane::smoothstep(1.0 - k, 1.0 + k, d);
        }
    }
    p
}

#[cfg(feature = "autoseg")]
fn seg_plane(seg: &lightcraft_develop::SegMask, w: usize, h: usize) -> Option<Plane> {
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

/// The sky of a frame (model only), soft.
pub fn sky_plane(s: &mut Session, frame: &Frame) -> Option<Plane> {
    let _ = (&s, frame);
    #[cfg(feature = "autoseg")]
    {
        let img = lightcraft_raster::Rgba8 { width: frame.w, height: frame.h, data: frame.rgba.clone() };
        crate::autoseg::segment(&img, &crate::autoseg::Kind::Class("sky".into())).ok().and_then(|(seg, _)| seg_plane(&seg, frame.w, frame.h))
    }
    #[cfg(not(feature = "autoseg"))]
    None
}

pub struct PartReport {
    pub name: String,
    pub role: &'static str,
    pub coverage: f32,
    pub adjust: LocalAdjustments,
}

pub struct DnbPlan {
    pub masks: Vec<Mask>,
    pub parts: Vec<PartReport>,
    pub subject_source: &'static str,
    pub halo: f32,
    pub warnings: Vec<String>,
}

/// How the cleanup exposure maps to lightness on this skin: L per EV, from a +0.5 EV probe.
fn kappa(s: &mut Session, ph: &PhotoSkin, c: &str) -> Result<f32> {
    let adj = LocalAdjustments { exposure: 0.5, ..Default::default() };
    let mut d = ph.base.clone();
    if let Some(m) = super::skin::skin_union_masks(&ph.base, &ph.frame, &ph.found, &ph.skins, adj, "Retouch: probe") {
        d.masks.push(m);
    }
    let f = analysis::render(s, ph.id, &d, analysis::EDGE, c)?;
    let (mut sum, mut n) = (0f64, 0f64);
    for sk in ph.skins.iter().flatten() {
        for i in 0..f.w * f.h {
            if sk.skin.weight.d[i] > 0.9 {
                sum += f64::from(f.lab.l.d[i] - ph.frame.lab.l.d[i]);
                n += 1.0;
            }
        }
    }
    Ok(if n == 0.0 { 10.0 } else { ((sum / n) as f32 / 0.5).max(1.0) })
}

fn coverage(p: &Plane) -> f32 {
    p.d.iter().sum::<f32>() / p.d.len().max(1) as f32
}

/// Plan the dodge & burn of one photo. `ph` holds the faces and their skin.
pub fn plan(s: &mut Session, ph: &PhotoSkin, mode: Mode, strength: f32, roles: &[Role], c: &str) -> Result<DnbPlan> {
    let (w, h) = (ph.frame.w, ph.frame.h);
    let k = strength.clamp(0.0, 1.5);
    let mut warnings = Vec::new();
    let mut named: Vec<(String, Role, Vec<(Plane, [f32; 4])>, Plane, LocalAdjustments)> = Vec::new();
    let mut subject_source = "none";
    let mut subject = Plane::new(w, h, 0.0);
    let mut face_parts: Vec<(usize, Vec<Part>)> = Vec::new();

    if mode == Mode::Portrait {
        for (i, (f, sk)) in ph.found.iter().zip(&ph.skins).enumerate() {
            let Some(sk) = sk else { continue };
            face_parts.push((i, dnb::portrait(&f.face, &sk.skin.weight, w, h, roles)));
        }
        // group the faces' parts by key: one mask per key
        let mut keys: Vec<&'static str> = Vec::new();
        for (_, ps) in &face_parts {
            for p in ps {
                if !keys.contains(&p.key) {
                    keys.push(p.key);
                }
            }
        }
        for key in keys {
            let mut per_face = Vec::new();
            let mut tmpl: Option<&Part> = None;
            for (i, ps) in &face_parts {
                if let Some(p) = ps.iter().find(|p| p.key == key) {
                    per_face.push((p.alpha.clone(), ph.skins[*i].as_ref().map_or([0.0, 0.0, w as f32, h as f32], |s| s.rect)));
                    tmpl = Some(p);
                }
            }
            if let Some(t) = tmpl {
                named.push((format!("{DNB} · {}", t.label), t.role, per_face, Plane::new(1, 1, 0.0), adj_of(&t.adj, k)));
            }
        }
    }
    // the cleanup of the skin's luminosity (portrait only): its own masks from the field
    if mode == Mode::Portrait && roles.contains(&Role::Cleanup) && ph.skins.iter().any(Option::is_some) {
        let kap = kappa(s, ph, c)?;
        let k_clean = (k * 1.25).clamp(0.0, 1.0);
        let mut fields: Vec<Option<Plane>> = Vec::new();
        let mut dref = 1.0f32;
        for sk in &ph.skins {
            match sk {
                Some(sk) => {
                    let (lp, big, _) = sigmas(sk.width);
                    let band = sk::l_band(&ph.frame.lab, &sk.skin, lp * 0.75, big);
                    let f = sk::cleanup_field(&band, k_clean, 6.0);
                    dref = dref.max(sk::dref_of(&[&f], &sk.skin.inlier, 1.0));
                    fields.push(Some(f));
                }
                None => fields.push(None),
            }
        }
        let (mut dodge, mut burn) = (Vec::new(), Vec::new());
        for (sk, f) in ph.skins.iter().zip(&fields) {
            if let (Some(sk), Some(f)) = (sk, f) {
                let m = sk::signed_masks(f, &sk.skin.weight, dref);
                dodge.push((m.pos, sk.rect));
                burn.push((m.neg, sk.rect));
            }
        }
        let ev = dref / kap;
        named.push((
            format!("{DNB} · Dodge: cleanup (skin)"),
            Role::Cleanup,
            dodge,
            Plane::new(1, 1, 0.0),
            LocalAdjustments { exposure: f64::from(ev), ..Default::default() },
        ));
        named.push((
            format!("{DNB} · Burn: cleanup (skin)"),
            Role::Cleanup,
            burn,
            Plane::new(1, 1, 0.0),
            LocalAdjustments { exposure: f64::from(-ev), ..Default::default() },
        ));
    }
    let want_finish = roles.contains(&Role::Finish) || (mode != Mode::Portrait && !roles.is_empty());
    let mut frame_parts: Vec<Part> = Vec::new();
    if want_finish || mode != Mode::Portrait {
        let (subj, src) = subject_plane(s, &ph.frame, &ph.found);
        subject = subj;
        subject_source = src;
        let sky = if mode == Mode::Landscape { sky_plane(s, &ph.frame) } else { None };
        let parts = dnb::scene(mode, &ph.frame.lab, &subject, sky.as_ref());
        frame_parts = if mode == Mode::Portrait {
            parts.into_iter().filter(|p| p.role == Role::Finish).collect()
        } else {
            parts.into_iter().filter(|p| roles.contains(&p.role)).collect()
        };
    }
    // build the masks
    let mut masks = Vec::new();
    let mut reports = Vec::new();
    let mut id = ph.base.next_mask_id();
    for (name, role, planes, _, adj) in &named {
        let fps: Vec<FacePlane> = planes.iter().map(|(p, r)| FacePlane { rect: *r, plane: p }).collect();
        if let Some(m) = outmask::face_mask(id, name, &fps, w, h, *adj) {
            id += 1;
            reports.push(PartReport { name: name.clone(), role: role.name(), coverage: planes.iter().map(|(p, _)| coverage(p)).sum(), adjust: *adj });
            masks.push(m);
        }
    }
    for p in &frame_parts {
        let adj = adj_of(&p.adj, k);
        let name = format!("{DNB} · {}", p.label);
        if let Some(m) = outmask::frame_mask(id, &name, &p.alpha, adj) {
            id += 1;
            reports.push(PartReport { name, role: p.role.name(), coverage: coverage(&p.alpha), adjust: adj });
            masks.push(m);
        }
    }
    let mut d = ph.base.clone();
    d.masks.extend(masks.iter().cloned());
    let after = analysis::render(s, ph.id, &d, analysis::EDGE, c)?;
    let halo = if subject_source != "none" { lightcraft_retouch::qc::halo(&ph.frame.lab.l, &after.lab.l, &subject) } else { 0.0 };
    if halo > lightcraft_retouch::qc::HALO_TOLERANCE {
        warnings.push(format!("a halo of {halo:.1} L around the subject: lower the strength"));
    }
    Ok(DnbPlan { masks, parts: reports, subject_source, halo, warnings })
}

fn ids_of(s: &Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p) } else { s.active().into_iter().collect() };
    if ids.is_empty() { Err(bad(c, "no photo: give `ids`, or select one")) } else { Ok(ids) }
}

/// Look at a photo for a mode: with faces for a portrait, without otherwise.
pub fn survey_for(s: &mut Session, id: PhotoId, mode: Option<Mode>, c: &str) -> Result<(PhotoSkin, Mode)> {
    let base = analysis::base_develop(s, id, c)?;
    let frame = analysis::render(s, id, &base, analysis::EDGE, c)?;
    let wants_faces = mode.is_none_or(|m| m == Mode::Portrait);
    let (found, faces_note) = if wants_faces {
        match analysis::faces(s, id, &base, &frame, 4, c) {
            Ok(f) => (f, None),
            Err(e) if mode.is_none() => (Arc::new(Vec::new()), Some(e.to_string())),
            Err(e) => return Err(e),
        }
    } else {
        (Arc::new(Vec::new()), None)
    };
    let skins = super::skin::analyse(&frame, &found);
    let has_face = skins.iter().any(Option::is_some);
    let m = match mode {
        Some(m) => m,
        None => {
            let sky = None::<f32>;
            dnb::auto_mode(has_face, sky)
        }
    };
    Ok((PhotoSkin { id, base, frame, found, skins, faces_note }, m))
}

fn dodge_burn(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "retouch.dodgeBurn";
    let ids = ids_of(s, p, C)?;
    let mode_in = match str_param(p, "mode") {
        None | Some("auto") => None,
        Some(m) => Some(Mode::parse(m).ok_or_else(|| bad(C, "`mode` must be portrait, flat (food), product, landscape or auto"))?),
    };
    let strength = f64_or(p, "strength", 0.4).clamp(0.0, 1.5) as f32;
    let roles = roles_of(p);
    if roles.is_empty() {
        return Err(bad(C, "`role` must be cleanup, sculpt, eyes, lips, finish, both or all"));
    }
    let apply = bool_or(p, "apply", true);
    let (mut ops, mut out) = (Vec::new(), Vec::new());
    for id in ids {
        let (ph, mode) = survey_for(s, id, mode_in, C)?;
        let plan = plan(s, &ph, mode, strength, &roles, C)?;
        out.push(json!({"id": id.0, "mode": mode.name(), "subject": plan.subject_source, "faces": ph.skins.iter().flatten().count(),
            "masks": plan.parts.iter().map(|r| json!({"name": r.name, "role": r.role, "coverage": round2(r.coverage), "exposure": round2(r.adjust.exposure as f32), "contrast": r.adjust.contrast, "saturation": r.adjust.saturation, "highlights": r.adjust.highlights, "shadows": r.adjust.shadows})).collect::<Vec<_>>(),
            "halo": round2(plan.halo), "warnings": plan.warnings}));
        if apply && !plan.masks.is_empty() {
            let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
            d.masks.retain(|m| !m.name.starts_with(DNB));
            let first = d.next_mask_id();
            for (i, mut m) in plan.masks.into_iter().enumerate() {
                m.id = first + i as u32;
                d.masks.push(m);
            }
            ops.extend(s.develop_op(id, d, "Dodge & Burn"));
        }
    }
    if !ops.is_empty() {
        s.commit("Dodge & Burn", lightcraft_catalog::Op::Batch { ops })?;
    }
    Ok(json!({"photos": out, "applied": apply}))
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = ids_of(s, p, "retouch.dodgeBurnClear")?;
    super::clear_masks(s, &ids, DNB, "Clear Dodge & Burn")
}

fn brush(s: &mut Session, p: &Value, sign: f64, c: &'static str) -> Result<Value> {
    let id = if p.get("id").is_some() { s.targets(p).first().copied() } else { s.active() }.ok_or_else(|| bad(c, "no active photo"))?;
    let pts: Vec<Point> = p
        .get("points")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|q| Some(Point::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))).collect())
        .unwrap_or_default();
    if pts.is_empty() {
        return Err(bad(c, "`points` [[x, y], …] (normalized to the photo) is required"));
    }
    let amount = f64_or(p, "amount", 0.25).clamp(0.0, 2.0) * sign;
    let stroke = BrushStroke {
        points: pts,
        size: f64_or(p, "size", 0.04).clamp(0.002, 0.5),
        feather: f64_or(p, "feather", 70.0).clamp(0.0, 100.0),
        flow: f64_or(p, "flow", 40.0).clamp(1.0, 100.0),
        ..Default::default()
    };
    let mut comps = vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes: vec![stroke] } }];
    let mut depth_note = Value::Null;
    if let Some(r) = p.get("depthRange").and_then(Value::as_array) {
        let (lo, hi) = (r.first().and_then(Value::as_f64).unwrap_or(0.0), r.get(1).and_then(Value::as_f64).unwrap_or(1.0));
        let ph = s.catalog.photo(id).cloned().ok_or_else(|| bad(c, "no such photo"))?;
        if s.depth_map(&ph).is_none() {
            return Err(bad(c, "no depth map for this photo: run `depth.estimate` first (Depth Anything model), or give no `depthRange`"));
        }
        comps.push(MaskComponent {
            name: None,
            op: MaskOp::Intersect,
            invert: false,
            shape: MaskShape::DepthRange { lo: lo.min(hi), hi: hi.max(lo), feather: f64_or(p, "depthFeather", 0.08) },
        });
        depth_note = json!([lo, hi]);
    }
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
    let name = format!("{DNB} · {} (brush)", if sign > 0.0 { "Dodge" } else { "Burn" });
    let mid = d.next_mask_id();
    d.masks.push(Mask {
        id: mid,
        name,
        components: comps,
        adjust: LocalAdjustments { exposure: amount, ..Default::default() },
        ..Default::default()
    });
    s.set_develop(id, d, if sign > 0.0 { "Dodge" } else { "Burn" })?;
    Ok(json!({"id": id.0, "mask": mid, "exposure": amount, "depthRange": depth_note}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "retouch.dodgeBurn",
            "Guided Dodge & Burn",
            [],
            None,
            "{ids?, mode?: auto|portrait|flat|product|landscape (auto: portrait when a face is found, else flat), strength?: 0..1.5 (0.4, subtle), role?: all|cleanup|sculpt|eyes|lips|finish|both|[…] (all), apply?: bool (true; false = report only)} — editable masks that dodge and burn: portrait = under the eyes, eye whites, irises, lips, cheekbones / nose ridge / forehead lit, the sides of the face and hollows shaded (mesh + depth of the face), the skin's luminosity irregularities evened out (cleanup, calibrated on the real render), background and edges lightly darkened, distracting spots attenuated; flat/product/landscape = subject lifted, background, edges and distractions burnt (subject model when installed, else a stated heuristic). Each part is one \"Retouch: D&B · …\" mask with ordinary local exposure (edit it in the Masking panel); running it again replaces them; one undo step → {photos: [{id, mode, subject, masks: [{name, role, coverage, exposure…}], halo, warnings}], applied}",
            always,
            dodge_burn
        ),
        cmd!(
            "retouch.dodgeBurnClear",
            "Remove Guided Dodge & Burn",
            [],
            None,
            "{ids?} — remove the \"Retouch: D&B …\" masks (guided and brushed) → {removed}",
            always,
            clear
        ),
        cmd!(
            "retouch.dodge",
            "Dodge Brush",
            [],
            None,
            "{id?, points: [[x, y], …] normalized to the photo, size?: radius as a share of the long edge (0.04), amount?: EV (0.25), feather?: 0..100 (70), flow?: 1..100 (40), depthRange?: [lo, hi] 0 far..1 near (needs a depth map: depth.estimate)} — a manual Dodge (lighten) brush stroke as an ordinary mask; with `depthRange` it only acts on that depth range of the photo → {id, mask, exposure}",
            has_active,
            |s, p| brush(s, p, 1.0, "retouch.dodge")
        ),
        cmd!(
            "retouch.burn",
            "Burn Brush",
            [],
            None,
            "{id?, points, size?, amount?: EV (0.25), feather?, flow?, depthRange?} — the Burn (darken) brush: see retouch.dodge → {id, mask, exposure}",
            has_active,
            |s, p| brush(s, p, -1.0, "retouch.burn")
        ),
    ]
}

#[allow(dead_code)]
fn _unused(_: &DevelopSettings, _: &SkinFace, _: &facegeo::Face) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brushes_make_ordinary_masks_and_a_depth_range_needs_a_depth_map() {
        let mut s = Session::with_demo();
        let id = s.active().expect("photo");
        s.execute("retouch.dodge", &json!({"points": [[0.4, 0.4], [0.5, 0.5]], "amount": 0.3})).expect("dodge");
        s.execute("retouch.burn", &json!({"points": [[0.6, 0.6]]})).expect("burn");
        let d = s.develop_of(id).expect("develop");
        let (dodge, burn) =
            (d.masks.iter().find(|m| m.name.contains("Dodge")).expect("dodge"), d.masks.iter().find(|m| m.name.contains("Burn")).expect("burn"));
        assert!(dodge.adjust.exposure > 0.29 && burn.adjust.exposure < -0.2);
        assert!(matches!(dodge.components[0].shape, MaskShape::Brush { .. }));
        let e = s.execute("retouch.dodge", &json!({"points": [[0.5, 0.5]], "depthRange": [0.0, 0.4]})).unwrap_err().to_string();
        assert!(e.contains("depth map"), "{e}");
        assert!(s.execute("retouch.dodge", &json!({})).is_err(), "points are required");
        assert_eq!(s.execute("retouch.dodgeBurnClear", &json!({})).expect("clear")["removed"], 2);
        assert!(s.develop_of(id).expect("develop").masks.is_empty());
    }

    #[test]
    fn a_flat_scene_burns_its_background_and_edges_in_one_undo_step() {
        let mut s = Session::with_demo();
        let id = s.active().expect("photo");
        let undo = s.undo.len();
        let r = s.execute("retouch.dodgeBurn", &json!({"mode": "flat", "strength": 0.4})).expect("dodgeBurn");
        assert_eq!(r["photos"][0]["mode"], "flat", "{r}");
        let names: Vec<String> = s.develop_of(id).expect("develop").masks.iter().map(|m| m.name.clone()).collect();
        assert!(names.iter().all(|n| n.starts_with(DNB)), "{names:?}");
        assert!(names.iter().any(|n| n.contains("background")) && names.iter().any(|n| n.contains("edges")), "{names:?}");
        assert!(s.develop_of(id).expect("develop").masks.iter().filter(|m| m.name.contains("Burn")).all(|m| m.adjust.exposure < 0.0));
        assert_eq!(s.undo.len(), undo + 1);
        // running it again replaces, never stacks
        s.execute("retouch.dodgeBurn", &json!({"mode": "flat"})).expect("again");
        let again: Vec<String> = s.develop_of(id).expect("develop").masks.iter().map(|m| m.name.clone()).collect();
        let mut unique = again.clone();
        unique.sort();
        unique.dedup();
        assert!(again.iter().all(|n| n.starts_with(DNB)) && unique.len() == again.len(), "replaced, never stacked: {again:?}");
        // a preview writes nothing; a bad mode is refused
        let steps = s.undo.len();
        s.execute("retouch.dodgeBurn", &json!({"mode": "flat", "apply": false})).expect("preview");
        assert_eq!(s.undo.len(), steps);
        assert!(s.execute("retouch.dodgeBurn", &json!({"mode": "weird"})).is_err());
    }

    #[test]
    fn roles_parse_names_lists_and_all() {
        assert_eq!(roles_of(&json!({})).len(), 5);
        assert_eq!(roles_of(&json!({"role": "cleanup"})), vec![Role::Cleanup]);
        assert_eq!(roles_of(&json!({"role": "both"})), vec![Role::Cleanup, Role::Sculpt]);
        assert_eq!(roles_of(&json!({"role": ["eyes", "lips", "nope"]})), vec![Role::Eyes, Role::Lips]);
    }
}
