//! Skin harmonization: measure the skin of each face, bring its colour toward a target (its own
//! median inside the photo, a person's median across a series), as editable local masks.
//!
//! Colour only, low frequency only: the field that is corrected is the skin's blurred `a`/`b`
//! (a few percent of the face width), never the pores; luminosity is not touched (its volumes
//! belong to the dodge & burn). The field is cut into four soft masks (less red, more red, less
//! yellow, more yellow) whose alpha is the size of the wanted shift, each carrying one ordinary
//! local temp/tint adjustment sized from a probe render (the real pipeline, not a model of it).

use std::collections::BTreeMap;
use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask};
use lightcraft_retouch::lab::{de00, hue_deg, lab_to_rgb8};
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::skin::{self, LabImage, Metrics, Skin};
use serde_json::{Value, json};

use super::analysis::{self, Found, Frame};
use super::outmask::{self, FacePlane};
use crate::cmd::{CommandSpec, always, bad, bool_or, cmd, f64_or, str_param};
use crate::{Result, Session};

/// The skin of one face, measured.
pub struct SkinFace {
    pub skin: Skin,
    pub before: Metrics,
    pub lp: (Plane, Plane),
    pub rect: [f32; 4],
    pub width: f32,
}

pub fn sigmas(width: f32) -> (f32, f32, f32) {
    ((0.02 * width).max(2.0), (0.08 * width).max(6.0), (0.004 * width).max(1.0))
}

pub fn analyse(frame: &Frame, found: &[Found]) -> Vec<Option<SkinFace>> {
    found
        .iter()
        .map(|f| {
            let (lp, big, soft) = sigmas(f.face.width);
            let sk = skin::select(&frame.lab, &f.cov, soft)?;
            let before = skin::metrics(&frame.lab, &sk, lp, big);
            let lpab = skin::lowpass_ab(&frame.lab, &sk, lp);
            Some(SkinFace { skin: sk, before, lp: lpab, rect: f.face.rect(0.08, frame.w, frame.h), width: f.face.width })
        })
        .collect()
}

/// One photo, looked at: settings without our masks, the frame, the faces, their skin.
pub struct PhotoSkin {
    pub id: PhotoId,
    pub base: DevelopSettings,
    pub frame: Arc<Frame>,
    pub found: Arc<Vec<Found>>,
    pub skins: Vec<Option<SkinFace>>,
    /// Why no face was looked for or found (the face model is not installed, …), if known.
    pub faces_note: Option<String>,
}

pub fn survey(s: &mut Session, ids: &[PhotoId], c: &str) -> Result<Vec<PhotoSkin>> {
    survey_as(s, ids, true, c)
}

/// Look at the photos; `strip`: without the masks this module made (the plan's "before"), else
/// as they are now (what a measurement after the retouch must see).
pub fn survey_as(s: &mut Session, ids: &[PhotoId], strip: bool, c: &str) -> Result<Vec<PhotoSkin>> {
    let mut out = Vec::new();
    for &id in ids {
        let base = if strip { analysis::base_develop(s, id, c)? } else { (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone() };
        let frame = analysis::render(s, id, &base, analysis::EDGE, c)?;
        // a photo without faces (a dish) is not an error; a missing face model is said, not hidden
        let (found, faces_note) = match analysis::faces(s, id, &base, &frame, 4, c) {
            Ok(f) => (f, None),
            Err(e) => (Arc::new(Vec::new()), Some(e.to_string())),
        };
        let skins = analyse(&frame, &found);
        out.push(PhotoSkin { id, base, frame, found, skins, faces_note });
    }
    Ok(out)
}

/// Where a face's colour should go.
#[derive(Clone, Debug)]
pub struct Target {
    pub ab: [f32; 2],
    pub person: String,
    /// How many photos the target was taken over (1 = a reference photo).
    pub photos: usize,
    pub reference: bool,
}

fn median(v: &mut [f32]) -> f32 {
    lightcraft_retouch::plane::quantile(v, 0.5)
}

/// Targets per photo and face: a person's median skin colour over the series (or in a reference
/// photo). `person` names everybody in the photos; otherwise the People names are used, and
/// faces nobody named get no series target (they are only made uniform inside their photo).
pub fn series_targets(photos: &[PhotoSkin], person: Option<&str>, reference: Option<PhotoId>) -> Vec<Vec<Option<Target>>> {
    let name_of =
        |p: &PhotoSkin, i: usize| -> Option<String> { person.map(str::to_string).or_else(|| p.found.get(i).and_then(|f| f.person.clone())) };
    let mut by: BTreeMap<String, (Vec<f32>, Vec<f32>, usize)> = BTreeMap::new();
    for p in photos {
        for (i, sk) in p.skins.iter().enumerate() {
            let (Some(sk), Some(n)) = (sk, name_of(p, i)) else { continue };
            // one entry per photo and person (the biggest face of that person)
            let e = by.entry(n).or_default();
            e.0.push(sk.skin.median[1]);
            e.1.push(sk.skin.median[2]);
            e.2 += 1;
        }
    }
    let mut refs: BTreeMap<String, [f32; 2]> = BTreeMap::new();
    if let Some(rid) = reference
        && let Some(rp) = photos.iter().find(|p| p.id == rid)
    {
        for (i, sk) in rp.skins.iter().enumerate() {
            if let (Some(sk), Some(n)) = (sk, name_of(rp, i)) {
                refs.entry(n).or_insert([sk.skin.median[1], sk.skin.median[2]]);
            }
        }
    }
    photos
        .iter()
        .map(|p| {
            (0..p.skins.len())
                .map(|i| {
                    let n = name_of(p, i)?;
                    p.skins[i].as_ref()?;
                    if let Some(ab) = refs.get(&n) {
                        return Some(Target { ab: *ab, person: n, photos: 1, reference: true });
                    }
                    let (a, b, k) = by.get(&n)?;
                    if *k < 2 {
                        return None;
                    }
                    Some(Target { ab: [median(&mut a.clone()), median(&mut b.clone())], person: n, photos: *k, reference: false })
                })
                .collect()
        })
        .collect()
}

/// What the local saturation and hue sliders do to a face's skin, measured on the real pipeline
/// in relative terms (so that it holds for a dark skin as for a light one), in both directions
/// (they are not symmetric): relative chroma change per saturation unit, rotation (radians) of
/// the Lab hue per hue unit.
#[derive(Clone, Copy, Debug)]
pub struct Jac {
    pub r_pos: f32,
    pub r_neg: f32,
    pub t_pos: f32,
    pub t_neg: f32,
}

impl Jac {
    /// The (saturation, hue) sliders that give a relative chroma change `r` and a rotation `t`;
    /// `None` when the probe moved nothing.
    pub fn solve(&self, r: f32, t: f32) -> Option<(f32, f32)> {
        let per = |v: f32, pos: f32, neg: f32| {
            let k = if v >= 0.0 { pos } else { neg };
            (k.abs() > 1e-6).then(|| v / k)
        };
        Some((per(r, self.r_pos, self.r_neg)?, per(t, self.t_pos, self.t_neg)?))
    }
}

pub fn skin_union_masks(
    base: &DevelopSettings,
    frame: &Frame,
    found: &[Found],
    skins: &[Option<SkinFace>],
    adj: LocalAdjustments,
    name: &str,
) -> Option<Mask> {
    let planes: Vec<FacePlane> =
        skins.iter().zip(found).filter_map(|(s, _)| s.as_ref()).map(|s| FacePlane { rect: s.rect, plane: &s.skin.weight }).collect();
    outmask::face_mask(base.next_mask_id(), name, &planes, frame.w, frame.h, adj)
}

/// Mean relative chroma change, hue rotation (radians) and lightness change of the skin pixels
/// (weight above 0.9) between two renders.
fn mean_polar(before: &LabImage, after: &LabImage, skins: &[Option<SkinFace>]) -> (f32, f32, f32) {
    let (mut sr, mut st, mut sl, mut n) = (0f64, 0f64, 0f64, 0f64);
    for sk in skins.iter().flatten() {
        for i in 0..before.w * before.h {
            if sk.skin.weight.d[i] > 0.9 {
                let (b, a) = (before.lab_at(i), after.lab_at(i));
                let (cb, ca) = (b[1].hypot(b[2]).max(1.0), a[1].hypot(a[2]));
                sr += f64::from(ca / cb - 1.0);
                st += f64::from(lightcraft_retouch::lab::hue_diff(hue_deg(b), hue_deg(a)).to_radians());
                sl += f64::from(a[0] - b[0]);
                n += 1.0;
            }
        }
    }
    if n == 0.0 { (0.0, 0.0, 0.0) } else { ((sr / n) as f32, (st / n) as f32, (sl / n) as f32) }
}

/// Probe the local saturation and hue (up and down) on one face's skin: four renders.
pub fn calibrate(
    s: &mut Session,
    id: PhotoId,
    base: &DevelopSettings,
    frame: &Frame,
    found: &[Found],
    skins: &[Option<SkinFace>],
    c: &str,
) -> Result<Jac> {
    const PROBE: f64 = 30.0;
    let mut got = Vec::new();
    for adj in [
        LocalAdjustments { saturation: PROBE, ..Default::default() },
        LocalAdjustments { saturation: -PROBE, ..Default::default() },
        LocalAdjustments { hue: PROBE, ..Default::default() },
        LocalAdjustments { hue: -PROBE, ..Default::default() },
    ] {
        let mut d = base.clone();
        if let Some(m) = skin_union_masks(base, frame, found, skins, adj, "Retouch: probe") {
            d.masks.push(m);
        }
        let f = analysis::render(s, id, &d, analysis::EDGE, c)?;
        got.push(mean_polar(&frame.lab, &f.lab, skins));
    }
    let k = PROBE as f32;
    Ok(Jac { r_pos: got[0].0 / k, r_neg: got[1].0 / -k, t_pos: got[2].1 / k, t_neg: got[3].1 / -k })
}

/// Parameters of a harmonization.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// How far each pixel's low-frequency colour goes toward its face's median (0..1).
    pub uniform: f32,
    /// How far the face's median goes toward the series target (0..1).
    pub series: f32,
    /// Largest series shift, Lab units.
    pub max_shift: f32,
    /// Scale of everything (the recipe's impact), 0..1.
    pub dose: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params { uniform: 0.8, series: 0.6, max_shift: 6.0, dose: 1.0 }
    }
}

pub struct FaceReport {
    pub face: usize,
    pub person: Option<String>,
    pub before: Metrics,
    pub after: Metrics,
    pub target: Option<Target>,
    /// ΔE00 (colour only) from the face's median to its target, before and after.
    pub target_de: Option<(f32, f32)>,
}

pub struct SkinPlan {
    pub masks: Vec<Mask>,
    pub faces: Vec<FaceReport>,
    pub gain: f32,
    pub warnings: Vec<String>,
}

const AXES: [(&str, usize, f32); 4] =
    [("less saturated", 0, -1.0), ("more saturated", 0, 1.0), ("hue toward red", 1, -1.0), ("hue toward yellow", 1, 1.0)];

/// A face's wanted shift (Lab units) as relative chroma change and hue rotation (radians), taking
/// the direction of the face's own median colour as "radial".
fn polar(da: &Plane, db: &Plane, own: [f32; 2]) -> (Plane, Plane) {
    let c0 = own[0].hypot(own[1]).max(6.0);
    let n = own[0].hypot(own[1]).max(1e-3);
    let (ux, uy) = (own[0] / n, own[1] / n);
    let r = da.zip(db, |a, b| (a * ux + b * uy) / c0);
    let t = da.zip(db, |a, b| (-a * uy + b * ux) / c0);
    (r, t)
}

/// The four masks of one face (names carry the face's number when the photo has several).
fn build(ph: &PhotoSkin, face: usize, field: &(Plane, Plane), refs: (f32, f32), jac: &Jac, gain: f32, start_id: u32) -> Vec<Mask> {
    let (w, h) = (ph.frame.w, ph.frame.h);
    let Some(Some(sk)) = ph.skins.get(face) else { return Vec::new() };
    let multi = ph.skins.iter().flatten().count() > 1;
    let a = skin::signed_masks(&field.0, &sk.skin.weight, refs.0);
    let b = skin::signed_masks(&field.1, &sk.skin.weight, refs.1);
    let planes = [a.neg, a.pos, b.neg, b.pos];
    let mut masks = Vec::new();
    let mut id = start_id;
    for (k, (label, axis, sign)) in AXES.iter().enumerate() {
        let (r, t) = if *axis == 0 { (sign * refs.0 * gain, 0.0) } else { (0.0, sign * refs.1 * gain) };
        let Some((sat, hue)) = jac.solve(r, t) else { continue };
        let adj =
            LocalAdjustments { saturation: f64::from(sat).clamp(-100.0, 100.0), hue: f64::from(hue).clamp(-100.0, 100.0), ..Default::default() };
        let fps = [FacePlane { rect: sk.rect, plane: &planes[k] }];
        let name = if multi { format!("{}Skin · face {} · {label}", super::PREFIX, face + 1) } else { format!("{}Skin · {label}", super::PREFIX) };
        if let Some(m) = outmask::face_mask(id, &name, &fps, w, h, adj) {
            masks.push(m);
            id += 1;
        }
    }
    masks
}

fn with_masks(base: &DevelopSettings, masks: &[Mask]) -> DevelopSettings {
    let mut d = base.clone();
    d.masks.extend(masks.iter().cloned());
    d
}

/// The series target of a face with its chroma kept: a target greyer than the face by more than
/// 3 % is stretched to 97 % of the face's own chroma (same hue as the target), so that a rich or
/// dark skin is never made ashen, waxy or washed out to match a duller photo.
pub fn keep_chroma(own: [f32; 2], tgt: [f32; 2]) -> [f32; 2] {
    let (co, ct) = (own[0].hypot(own[1]), tgt[0].hypot(tgt[1]));
    if ct < 0.97 * co && ct > 1e-3 {
        let k = 0.97 * co / ct;
        [tgt[0] * k, tgt[1] * k]
    } else {
        tgt
    }
}

/// Plan one face: its field, its probe, its masks, sized by a closed loop on the real render.
fn plan_face(
    s: &mut Session,
    ph: &PhotoSkin,
    i: usize,
    target: Option<&Target>,
    p: &Params,
    start_id: u32,
    c: &str,
) -> Result<Option<(Vec<Mask>, f32)>> {
    let Some(Some(sk)) = ph.skins.get(i) else { return Ok(None) };
    let (ku, ks) = (p.uniform * p.dose, p.series * p.dose);
    let own = [sk.skin.median[1], sk.skin.median[2]];
    let tgt = target.map_or(own, |t| keep_chroma(own, t.ab));
    let f = skin::harmonize_field(&sk.lp, own, tgt, ku, ks, p.max_shift);
    let mut field = polar(&f.da, &f.db, own);
    // the median shift the field asks for (zero inside a photo, the series part with a target)
    let med_of = |p: &Plane| {
        let v: Vec<f32> = (0..p.d.len()).filter(|&k| sk.skin.inlier.d[k] > 0.5).map(|k| p.d[k]).collect();
        lightcraft_retouch::plane::quantile(&v, 0.5)
    };
    let c0 = own[0].hypot(own[1]).max(6.0);
    let nrm = own[0].hypot(own[1]).max(1e-3);
    let (ux, uy) = (own[0] / nrm, own[1] / nrm);
    // a skin is never made greyer by more than 3 % by this step (no ashen, waxy or washed-out skin)
    let dc_r = med_of(&field.0);
    if dc_r < -0.03 {
        field.0 = field.0.map(|v| v + (-0.03 - dc_r));
    }
    let (mr, mt) = (med_of(&field.0), med_of(&field.1));
    let want = [c0 * (mr * ux - mt * uy), c0 * (mr * uy + mt * ux)];
    let refs_of =
        |f: &(Plane, Plane)| (skin::dref_of(&[&f.0], &sk.skin.inlier, 0.03).min(0.20), skin::dref_of(&[&f.1], &sk.skin.inlier, 0.03).min(0.20));
    let jac = calibrate(s, ph.id, &ph.base, &ph.frame, &ph.found[i..=i], &ph.skins[i..=i], c)?;
    let mut gain = 1.0f32;
    let mut refs = refs_of(&field);
    let mut masks = build(ph, i, &field, refs, &jac, gain, start_id);
    let (lp, big, _) = sigmas(sk.width);
    for _ in 0..3 {
        let after = analysis::render(s, ph.id, &with_masks(&ph.base, &masks), analysis::EDGE, c)?;
        let m = sk.skin.restat(&after.lab);
        // 1. the median must go where the field says (a blotch fix must not move the skin's colour)
        let e = [m.median[1] - sk.skin.median[1] - want[0], m.median[2] - sk.skin.median[2] - want[1]];
        let (er, et) = ((e[0] * ux + e[1] * uy) / c0, (-e[0] * uy + e[1] * ux) / c0);
        if er.abs() > 0.02 || et.abs() > 0.02 {
            field = (field.0.map(|v| v - er), field.1.map(|v| v - et));
            refs = refs_of(&field);
            masks = build(ph, i, &field, refs, &jac, gain, start_id);
            continue;
        }
        // 2. the improvement must be what was asked for
        let a = skin::metrics(&after.lab, &m, lp, big);
        let mut wanted = ku * sk.before.de_color_mean;
        let mut got = sk.before.de_color_mean - a.de_color_mean;
        if let Some(t) = target {
            let ab = keep_chroma(own, t.ab);
            let b0 = de00(sk.skin.median, [sk.skin.median[0], ab[0], ab[1]]);
            let a0 = de00(m.median, [m.median[0], ab[0], ab[1]]);
            wanted += ks.min(1.0) * b0.min(p.max_shift.max(0.5) * 1.5);
            got += b0 - a0;
        }
        if wanted < 0.4 {
            break;
        }
        let rho = got / wanted;
        if (0.85..=1.15).contains(&rho) {
            break;
        }
        gain = (gain / rho.clamp(0.4, 2.5)).clamp(0.3, 3.0);
        masks = build(ph, i, &field, refs, &jac, gain, start_id);
    }
    Ok(Some((masks, gain)))
}

/// Plan the harmonization of one photo. `targets[i]` is face `i`'s series target. `None` when
/// there is no usable skin or nothing to do.
pub fn plan(s: &mut Session, ph: &PhotoSkin, targets: &[Option<Target>], p: &Params, c: &str) -> Result<Option<SkinPlan>> {
    if ph.skins.iter().all(Option::is_none) || (p.uniform * p.dose < 0.01 && p.series * p.dose < 0.01) {
        return Ok(None);
    }
    let mut masks: Vec<Mask> = Vec::new();
    let mut gain = 1.0f32;
    let mut jac_note = None;
    for i in 0..ph.skins.len() {
        let start = ph.base.next_mask_id() + masks.len() as u32;
        if let Some((m, g)) = plan_face(s, ph, i, targets.get(i).and_then(|t| t.as_ref()), p, start, c)? {
            masks.extend(m);
            gain = gain.max(g);
            jac_note = Some(g);
        }
    }
    let _ = jac_note;
    let after = analysis::render(s, ph.id, &with_masks(&ph.base, &masks), analysis::EDGE, c)?;
    let mut warnings = Vec::new();
    let mut faces = Vec::new();
    for (i, sk) in ph.skins.iter().enumerate() {
        let Some(sk) = sk else { continue };
        let m = sk.skin.restat(&after.lab);
        let (lp, big, _) = sigmas(sk.width);
        let a = skin::metrics(&after.lab, &m, lp, big);
        let tgt = targets.get(i).and_then(|t| t.clone());
        let target_de = tgt.as_ref().map(|t| {
            let f = |m: [f32; 3]| de00([m[0], m[1], m[2]], [m[0], t.ab[0], t.ab[1]]);
            (f(sk.skin.median), f(m.median))
        });
        if (a.median[0] - sk.skin.median[0]).abs() > lightcraft_retouch::qc::SKIN_L_TOLERANCE {
            warnings.push(format!("face {i}: the skin's lightness moved by {:.1} L", a.median[0] - sk.skin.median[0]));
        }
        if a.chroma < 0.85 * sk.before.chroma {
            warnings.push(format!("face {i}: the skin lost {:.0} % of its chroma", 100.0 * (1.0 - a.chroma / sk.before.chroma)));
        }
        faces.push(FaceReport {
            face: i,
            person: ph.found.get(i).and_then(|f| f.person.clone()),
            before: sk.before.clone(),
            after: a,
            target: tgt,
            target_de,
        });
    }
    Ok(Some(SkinPlan { masks, faces, gain, warnings }))
}

pub fn metrics_json(m: &Metrics) -> Value {
    let rgb = lab_to_rgb8(m.median);
    json!({"n": m.n, "medianLab": [round1(m.median[0]), round1(m.median[1]), round1(m.median[2])], "chroma": round1(m.chroma), "hueDeg": round1(m.hue_deg),
           "deColorMean": round2(m.de_color_mean), "deColorP95": round2(m.de_color_p95), "lBandSd": round2(m.l_band_sd), "hex": lightcraft_retouch::lab::hex(rgb)})
}

pub fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}
pub fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// A vectorscope reading of a skin colour: the angle and how far it is from the skin line.
pub fn vectorscope_json(median: [f32; 3]) -> Value {
    let rgb = lab_to_rgb8(median);
    let a = skin::vectorscope_deg([f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0]);
    json!({"angleDeg": round1(a), "skinLineDeg": skin::SKIN_LINE_DEG, "offsetDeg": round1(lightcraft_retouch::lab::hue_diff(skin::SKIN_LINE_DEG, a))})
}

/// Mean (a, b) of the neutral-ish pixels (grey, white, walls) outside the faces' skin: the cast.
pub fn neutral_mean(frame: &Frame, skins: &[Option<SkinFace>]) -> Option<[f32; 2]> {
    let (mut sa, mut sb, mut n) = (0f64, 0f64, 0f64);
    for i in 0..frame.w * frame.h {
        let l = frame.lab.lab_at(i);
        let c = l[1].hypot(l[2]);
        if c < 9.0 && l[0] > 25.0 && l[0] < 92.0 && skins.iter().flatten().all(|s| s.skin.weight.d[i] < 0.01) {
            sa += f64::from(l[1]);
            sb += f64::from(l[2]);
            n += 1.0;
        }
    }
    (n > 0.01 * (frame.w * frame.h) as f64).then(|| [(sa / n) as f32, (sb / n) as f32])
}

fn ids_of(s: &Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p) } else { s.active().into_iter().collect() };
    if ids.is_empty() { Err(bad(c, "no photo: give `ids`, or select one")) } else { Ok(ids) }
}

fn measure(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "skin.measure";
    let ids = ids_of(s, p, C)?;
    let photos = survey_as(s, &ids, false, C)?;
    let mut out = Vec::new();
    let mut neutral: Vec<Option<[f32; 2]>> = Vec::new();
    for ph in &photos {
        neutral.push(neutral_mean(&ph.frame, &ph.skins));
    }
    for (k, ph) in photos.iter().enumerate() {
        let faces: Vec<Value> = ph
            .skins
            .iter()
            .enumerate()
            .map(|(i, sk)| match sk {
                Some(sk) => json!({"face": i, "person": ph.found[i].person, "skin": metrics_json(&sk.before), "vectorscope": vectorscope_json(sk.skin.median)}),
                None => json!({"face": i, "skin": null, "note": "no usable skin on this face"}),
            })
            .collect();
        out.push(json!({"id": ph.id.0, "faces": faces, "neutralMeanAb": neutral[k].map(|n| [round1(n[0]), round1(n[1])])}));
    }
    // series: spread of the medians per person
    let tg = series_targets(&photos, str_param(p, "person"), p.get("reference").and_then(Value::as_u64).map(PhotoId));
    let mut spread: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    for (ph, ts) in photos.iter().zip(&tg) {
        for (sk, t) in ph.skins.iter().zip(ts) {
            if let (Some(sk), Some(t)) = (sk, t) {
                let m = sk.skin.median;
                spread.entry(t.person.clone()).or_default().push(de00([m[0], m[1], m[2]], [m[0], t.ab[0], t.ab[1]]));
            }
        }
    }
    let series: Vec<Value> = spread
        .into_iter()
        .map(|(n, v)| json!({"person": n, "photos": v.len(), "meanDeToTarget": round2(v.iter().sum::<f32>() / v.len() as f32), "maxDeToTarget": round2(v.iter().cloned().fold(0.0, f32::max))}))
        .collect();
    Ok(json!({"photos": out, "series": series}))
}

fn harmonize(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "skin.harmonize";
    let ids = ids_of(s, p, C)?;
    let strength = f64_or(p, "strength", 1.0).clamp(0.0, 1.0) as f32;
    let params = Params {
        uniform: f64_or(p, "uniform", 0.8).clamp(0.0, 1.0) as f32,
        series: f64_or(p, "series", 0.6).clamp(0.0, 1.0) as f32,
        max_shift: f64_or(p, "maxShift", 6.0).clamp(0.5, 20.0) as f32,
        dose: strength,
    };
    let photos = survey(s, &ids, C)?;
    let tg = series_targets(&photos, str_param(p, "person"), p.get("reference").and_then(Value::as_u64).map(PhotoId));
    let apply = bool_or(p, "apply", true);
    let (mut ops, mut out) = (Vec::new(), Vec::new());
    for (ph, ts) in photos.iter().zip(&tg) {
        let Some(plan) = plan(s, ph, ts, &params, C)? else {
            out.push(json!({"id": ph.id.0, "faces": ph.found.len(), "masks": [], "note": "no usable skin"}));
            continue;
        };
        let faces: Vec<Value> = plan
            .faces
            .iter()
            .map(|f| {
                json!({"face": f.face, "person": f.person, "before": metrics_json(&f.before), "after": metrics_json(&f.after),
                       "target": f.target.as_ref().map(|t| json!({"person": t.person, "ab": [round1(t.ab[0]), round1(t.ab[1])], "photos": t.photos, "reference": t.reference})),
                       "deToTarget": f.target_de.map(|(a, b)| json!({"before": round2(a), "after": round2(b)}))})
            })
            .collect();
        out.push(json!({"id": ph.id.0, "masks": plan.masks.iter().map(|m| json!({"name": m.name, "saturation": round2(m.adjust.saturation as f32), "hue": round2(m.adjust.hue as f32)})).collect::<Vec<_>>(),
                        "faces": faces, "gain": round2(plan.gain), "warnings": plan.warnings}));
        if apply && !plan.masks.is_empty() {
            let mut d = (*s.develop_of(ph.id).ok_or_else(|| bad(C, "no such photo"))?).clone();
            d.masks.retain(|m| !m.name.starts_with(&format!("{}Skin", super::PREFIX)));
            let first = d.next_mask_id();
            for (i, mut m) in plan.masks.into_iter().enumerate() {
                m.id = first + i as u32;
                d.masks.push(m);
            }
            if let Some(op) = s.develop_op(ph.id, d, "Harmonize Skin") {
                ops.push(op);
            }
        }
    }
    if !ops.is_empty() {
        s.commit("Harmonize Skin", lightcraft_catalog::Op::Batch { ops })?;
    }
    Ok(json!({"photos": out, "applied": apply}))
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "skin.clear";
    let ids = ids_of(s, p, C)?;
    super::clear_masks(s, &ids, &format!("{}Skin", super::PREFIX), "Clear Skin Harmonization")
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "skin.measure",
            "Measure Skin Colour",
            [],
            None,
            "{ids?, person?: name (everybody in the photos), reference?: photo id} — find the faces and measure their skin on the developed photo (no highlights, hair, beard, lips or eyes): Lab median, chroma, hue, colour uniformity (mean CIEDE2000 of the low-frequency colour to the median, lightness left out), vectorscope angle against the skin line (123°), the neutral parts' cast, and per person over the series how far each photo's median is from the series median → {photos: [{id, faces: [{face, person, skin: {medianLab, chroma, hueDeg, deColorMean, …}, vectorscope: {angleDeg, offsetDeg}}], neutralMeanAb}], series: [{person, photos, meanDeToTarget, maxDeToTarget}]}. Changes nothing",
            always,
            measure
        ),
        cmd!(
            "skin.harmonize",
            "Harmonize Skin Colour",
            [],
            None,
            "{ids?, strength?: 0..1 (1), uniform?: 0..1 (0.8, inside a photo), series?: 0..1 (0.6, toward the person's colour over the series), maxShift?: Lab units (6), person?: name, reference?: photo id (the person's colour there is the target), apply?: bool (true; false = report only)} — even out the colour of the skin, texture untouched and lightness untouched: four soft masks \"Retouch: Skin · less red / more red / less yellow / more yellow\" with ordinary local temp/tint (edit them in the Masking panel), one undo step; with several photos each person's skin goes toward their series colour (a real change of light is only partly corrected: `series` and `maxShift`) → {photos: [{id, masks, faces: [{before, after, target, deToTarget}], gain, warnings}], applied}",
            always,
            harmonize
        ),
        cmd!("skin.clear", "Remove Skin Harmonization", [], None, "{ids?} — remove the \"Retouch: Skin …\" masks → {removed}", always, clear),
    ]
}
