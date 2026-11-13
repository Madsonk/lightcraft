//! The "natural skin" recipe: `retouch.propose` plans eight ordered steps for each photo and
//! writes nothing; `retouch.apply` writes the chosen steps as ordinary settings (masks named
//! "Retouch: …", the Colour Mixer, heal spots for sensor dust outside the faces) in one undo step,
//! and can leave three variants (Light, Medium, Strong) and the original in Versions.
//!
//! Red lines, all checked by tests: no mole, scar or tattoo is removed without an explicit
//! confirmation (`confirmSpots`); the shape of the face and the traits are never changed (no
//! geometry, crop or resampling: only tone and colour through masks); nothing is generated; no
//! photo leaves the machine; the skin's texture is kept (high-frequency variance measured before
//! and after, tolerance 5 %); natural look first: when a check goes over its threshold the dose is
//! reduced automatically.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::{Op, PhotoId, Version};
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_retouch::dnb::{Mode, Role};
use lightcraft_retouch::plane::Plane;
use lightcraft_retouch::{palette as pal, qc, skin as sk};
use serde_json::{Value, json};

use super::analysis::{self, Frame};
use super::dnb as dnbmod;
use super::outmask;
use super::palette as palmod;
use super::skin::{self as skinmod, Params, PhotoSkin, round1, round2, sigmas};
use crate::cmd::{CommandSpec, always, bad, bool_or, cmd, f64_or, str_param};
use crate::portrait::Region;
use crate::{Result, Session};

pub const RECIPE: &str = "natural-skin";

pub const STEPS: [(&str, &str); 8] = [
    ("camera", "Camera development"),
    ("clean", "Dust and defects"),
    ("skin_color", "Skin colour"),
    ("dnb_cleanup", "Dodge & burn: cleanup"),
    ("dnb_sculpt", "Dodge & burn: light sculpting"),
    ("eyes", "Eyes, teeth, lips"),
    ("color", "Overall colour"),
    ("finish", "Finishing"),
];

/// Most persistent dust spots a series may have for them to be believed.
const MAX_DUST: usize = 24;

/// Dose of the three variants.
pub const LIGHT: f32 = 0.4;
pub const MEDIUM: f32 = 0.7;
pub const STRONG: f32 = 1.0;

/// Nominal dodge & burn strength at dose 1.
const DNB_STRENGTH: f32 = 0.5;

struct Step {
    id: &'static str,
    label: &'static str,
    available: bool,
    reason: Option<String>,
    masks: Vec<Mask>,
    spots: Vec<Spot>,
    mixer_values: Option<[lightcraft_develop::Hsl; 8]>,
    target: Vec<[f32; 3]>,
    skin_candidates: Vec<[f32; 2]>,
    warnings: Vec<String>,
    info: String,
}

impl Step {
    fn new(id: &'static str, label: &'static str) -> Step {
        Step {
            id,
            label,
            available: true,
            reason: None,
            masks: Vec::new(),
            spots: Vec::new(),
            mixer_values: None,
            target: Vec::new(),
            skin_candidates: Vec::new(),
            warnings: Vec::new(),
            info: String::new(),
        }
    }
    fn off(mut self, why: impl Into<String>) -> Step {
        self.available = false;
        self.reason = Some(why.into());
        self
    }
}

struct PhotoPlan {
    id: PhotoId,
    ph: PhotoSkin,
    targets: Vec<Option<skinmod::Target>>,
    steps: Vec<Step>,
    subject: Plane,
    mode: Mode,
}

struct Opts {
    steps: HashMap<String, bool>,
    target: Option<Value>,
    person: Option<String>,
    reference: Option<PhotoId>,
    protect_skin: bool,
    confirm_spots: Vec<[f32; 2]>,
}

fn opts_of(p: &Value) -> Opts {
    let mut steps = HashMap::new();
    if let Some(m) = p.get("steps").and_then(Value::as_object) {
        for (k, v) in m {
            if let Some(b) = v.as_bool() {
                steps.insert(k.clone(), b);
            }
        }
    }
    let confirm_spots = p
        .get("confirmSpots")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|q| Some([q.get(0)?.as_f64()? as f32, q.get(1)?.as_f64()? as f32])).collect())
        .unwrap_or_default();
    Opts {
        steps,
        target: p.get("target").cloned().filter(|v| v.is_object()),
        person: str_param(p, "person").map(str::to_string),
        reference: p.get("reference").and_then(Value::as_u64).map(PhotoId),
        protect_skin: bool_or(p, "protectSkin", true),
        confirm_spots,
    }
}

pub fn dose_of(p: &Value) -> f32 {
    match p.get("impact") {
        Some(Value::String(s)) => match s.as_str() {
            "light" => LIGHT,
            "strong" => STRONG,
            _ => MEDIUM,
        },
        Some(v) => v.as_f64().map_or(MEDIUM, |x| x as f32).clamp(0.05, 1.0),
        None => MEDIUM,
    }
}

fn enabled(o: &Opts, st: &Step) -> bool {
    st.available && o.steps.get(st.id).copied().unwrap_or(st.id != "color" || o.target.is_some())
}

fn scaled_mask(m: &Mask, dose: f32, id: u32) -> Mask {
    let mut c = m.clone();
    c.id = id;
    c.adjust = super::scale_adjust(&m.adjust, f64::from(dose));
    c
}

/// The settings of a photo with the enabled steps at `dose` (spots and mixer included unless
/// `local_only`).
fn compose(pp: &PhotoPlan, o: &Opts, dose: f32, local_only: bool) -> DevelopSettings {
    let mut d = pp.ph.base.clone();
    let mut id = d.next_mask_id();
    for st in &pp.steps {
        if !enabled(o, st) {
            continue;
        }
        for m in &st.masks {
            d.masks.push(scaled_mask(m, dose, id));
            id += 1;
        }
        if local_only {
            continue;
        }
        if let Some(v) = &st.mixer_values {
            let mut scaled = *v;
            for h in &mut scaled {
                h.hue *= f64::from(dose);
                h.sat *= f64::from(dose);
                h.lum *= f64::from(dose);
            }
            d.mixer = add_mixer(&pp.ph.base.mixer, &scaled);
        }
        for sp in &st.spots {
            let near = |a: &Spot| a.points.first().zip(sp.points.first()).is_some_and(|(x, y)| (x.x - y.x).hypot(x.y - y.y) < sp.size);
            if !d.spots.iter().any(near) {
                d.spots.push(sp.clone());
            }
        }
    }
    d
}

fn add_mixer(base: &lightcraft_develop::Mixer, add: &[lightcraft_develop::Hsl; 8]) -> lightcraft_develop::Mixer {
    let mut m = *base;
    let bands = [&mut m.red, &mut m.orange, &mut m.yellow, &mut m.green, &mut m.aqua, &mut m.blue, &mut m.purple, &mut m.magenta];
    for (h, a) in bands.into_iter().zip(add) {
        h.hue = (h.hue + a.hue).clamp(-100.0, 100.0);
        h.sat = (h.sat + a.sat).clamp(-100.0, 100.0);
        h.lum = (h.lum + a.lum).clamp(-100.0, 100.0);
    }
    m
}

fn teeth_mask(ph: &PhotoSkin, id: u32) -> Option<Mask> {
    let (w, h) = (ph.frame.w, ph.frame.h);
    let mut detail = Vec::new();
    for f in ph.found.iter() {
        if let Some(p) = crate::portrait::patch(&f.face.p, w, h, Region::Teeth) {
            detail.push(SegMask::from_logits_in(p.side, &p.logits, p.rect));
        }
    }
    if detail.is_empty() {
        return None;
    }
    let adjust = LocalAdjustments { temp: -20.0, saturation: -20.0, exposure: 0.12, ..Default::default() };
    Some(Mask {
        id,
        name: format!("{}Eyes · Teeth", super::PREFIX),
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Prompt { text: "face".into(), seg: None, detail, edge: 0.0 },
        }],
        adjust,
        ..Default::default()
    })
}

/// Is the pixel on a face's skin region, or inside a face's box (a mole or a blemish is never
/// healed unasked, and a spot beside the skin is as good as on it)? Regions: coverage plane, box.
fn near_face(regions: &[(&Plane, [f32; 4])], x: usize, y: usize) -> bool {
    regions.iter().any(|(cov, b)| {
        let i = y.min(cov.h - 1) * cov.w + x.min(cov.w - 1);
        cov.d[i] > 0.02 || ((x as f32) > b[0] && (x as f32) < b[2] && (y as f32) > b[1] && (y as f32) < b[3])
    })
}

fn spot_of(s: &mut Session, id: PhotoId, d: &DevelopSettings, x: f64, y: f64, radius: f64) -> Spot {
    let mut spot = Spot {
        mode: SpotMode::Heal,
        points: vec![Point::new(x, y)],
        size: radius.clamp(crate::cmd::masks::SPOT_SIZE.0, crate::cmd::masks::SPOT_SIZE.1),
        feather: 50.0,
        opacity: 100.0,
        source_offset: None,
        patch: None,
    };
    spot.source_offset = crate::cmd::masks::pick_source(s, id, d, &spot, None);
    spot
}

#[allow(clippy::too_many_arguments)]
fn plan_photo(
    s: &mut Session,
    ph: PhotoSkin,
    targets: Vec<Option<skinmod::Target>>,
    o: &Opts,
    series_dust: &[(f64, f64, f64)],
    series_ran: bool,
    dust_overflow: usize,
    c: &str,
) -> Result<PhotoPlan> {
    let id = ph.id;
    let has_face = ph.skins.iter().any(Option::is_some);
    let mode = if has_face { Mode::Portrait } else { Mode::Flat };
    let no_face = "no face with usable skin was found";
    let mut steps = Vec::new();

    // 1. camera development: the user's own settings are the base, nothing is added
    let mut st = Step::new("camera", STEPS[0].1);
    st.info = format!("kept as it is: exposure {:+.1} EV, white balance {:.0} K", ph.base.light.exposure, ph.base.wb.temp);
    steps.push(st);

    // 2. sensor dust: only what a series proves (a spot that sits at the same place on most photos
    // of a shoot), and never on or beside a face; a single photo cannot tell dust from detail
    let mut st = Step::new("clean", STEPS[1].1);
    {
        let (w, h) = (ph.frame.w as f64, ph.frame.h as f64);
        let regions: Vec<(&Plane, [f32; 4])> = ph.found.iter().map(|f| (&f.cov, f.face.bbox)).collect();
        let (mut outside, mut on_skin) = (0usize, Vec::new());
        for &(x, y, r) in series_dust {
            let (px, py) = ((x * w) as usize, (y * h) as usize);
            if near_face(&regions, px, py) {
                on_skin.push([x as f32, y as f32]);
            } else {
                outside += 1;
                st.spots.push(spot_of(s, id, &ph.base, x, y, r * 2.2));
            }
        }
        // spots the user confirmed, wherever they are
        for q in &o.confirm_spots {
            st.spots.push(spot_of(s, id, &ph.base, f64::from(q[0]), f64::from(q[1]), 0.012));
        }
        st.skin_candidates = on_skin.clone();
        st.info = if dust_overflow > 0 {
            format!(
                "the series search returned {dust_overflow} candidates: far too many to be sensor dust (busy scenes, noise, photos of different shoots), nothing healed"
            )
        } else if series_dust.is_empty() && !series_ran {
            "no sensor dust healed: one photo cannot tell dust from detail (select 3 or more photos of a shoot, or use Photo > Remove Dust From Series); a mole or a blemish is never removed without your confirmation (`confirmSpots`)".to_string()
        } else {
            format!(
                "{outside} persistent sensor-dust spot(s) of the series healed outside the faces; {} on or beside a face left alone (a mole or a blemish is never removed without your confirmation: `confirmSpots`)",
                on_skin.len()
            )
        };
        if !o.confirm_spots.is_empty() {
            st.info.push_str(&format!("; {} confirmed by you", o.confirm_spots.len()));
        }
    }
    steps.push(st);

    // 3. skin colour
    let st = if !has_face {
        Step::new("skin_color", STEPS[2].1).off(no_face)
    } else {
        let mut st = Step::new("skin_color", STEPS[2].1);
        if let Some(plan) = skinmod::plan(s, &ph, &targets, &Params::default(), c)? {
            st.masks = plan.masks;
            st.warnings = plan.warnings;
        } else {
            st = st.off("nothing to even out");
        }
        st
    };
    steps.push(st);

    // 4/5/6/8: dodge & burn parts by role
    let roles: [(usize, &'static str, Vec<Role>); 4] = [
        (3, "dnb_cleanup", vec![Role::Cleanup]),
        (4, "dnb_sculpt", vec![Role::Sculpt]),
        (5, "eyes", vec![Role::Eyes, Role::Lips]),
        (7, "finish", vec![Role::Finish]),
    ];
    let mut subject = Plane::new(ph.frame.w, ph.frame.h, 0.0);
    let mut dnb_steps: HashMap<&'static str, Step> = HashMap::new();
    for (idx, key, rs) in &roles {
        let mut st = Step::new(key, STEPS[*idx].1);
        if (*key == "dnb_cleanup" || *key == "eyes") && !has_face {
            dnb_steps.insert(key, st.off(no_face));
            continue;
        }
        let pl = dnbmod::plan(s, &ph, mode, DNB_STRENGTH, rs, c)?;
        if *key == "finish" {
            let (sub, _) = dnbmod::subject_plane(s, &ph.frame, &ph.found);
            subject = sub;
        }
        st.masks = pl.masks;
        st.warnings = pl.warnings;
        if *key == "eyes"
            && let Some(m) = teeth_mask(&ph, 0)
        {
            st.masks.push(m);
        }
        if st.masks.is_empty() {
            st = st.off("nothing to do for this photo");
        }
        dnb_steps.insert(key, st);
    }
    let mut take = |k: &'static str| dnb_steps.remove(k).unwrap_or_else(|| Step::new(k, k).off("not planned"));
    steps.push(take("dnb_cleanup"));
    steps.push(take("dnb_sculpt"));
    steps.push(take("eyes"));

    // 7. overall colour: only with a target palette
    let st = match &o.target {
        None => Step::new("color", STEPS[6].1).off("no target palette: give `target` ({palette: name} | {photo: id} | {hexes: […]} | {scheme: …})"),
        Some(t) => {
            let mut st = Step::new("color", STEPS[6].1);
            let pl = palmod::plan(s, &[id], t, 0.6, o.protect_skin, false, 1.0, c)?;
            st.mixer_values = Some(pl.values);
            st.target = pl.target.clone();
            st.warnings = pl.warnings;
            st
        }
    };
    steps.push(st);
    steps.push(take("finish"));
    Ok(PhotoPlan { id, ph, targets, steps, subject, mode })
}

fn resample(p: &Plane, w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            out.d[y * w + x] = p.sample((x as f32 + 0.5) / w as f32 * p.w as f32, (y as f32 + 0.5) / h as f32 * p.h as f32);
        }
    }
    out
}

/// Edge of the frames the texture is measured on (pores are finer than the analysis frame).
const TEX_EDGE: usize = 2400;

#[derive(Default)]
struct Qc {
    texture_change: Option<f32>,
    texture_change_lstar: Option<f32>,
    halo: f32,
    skin_l: f32,
    outside_max: u8,
    outside_n: usize,
}

fn texture(s: &mut Session, pp: &PhotoPlan, d: &DevelopSettings, c: &str) -> Result<Option<(f32, f32)>> {
    if pp.ph.skins.iter().all(Option::is_none) {
        return Ok(None);
    }
    let before = analysis::render(s, pp.id, &pp.ph.base, TEX_EDGE, c)?;
    let after = analysis::render(s, pp.id, d, TEX_EDGE, c)?;
    let (mut vb, mut va, mut lb, mut la, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32);
    for (f, skn) in pp.ph.found.iter().zip(&pp.ph.skins) {
        let Some(skn) = skn else { continue };
        let inlier = resample(&skn.skin.inlier, before.w, before.h).map(|v| if v > 0.5 { 1.0 } else { 0.0 });
        let sigma = (0.004 * f.face.width * before.w as f32 / pp.ph.frame.w as f32).max(1.2);
        let mk = |fr: &Frame| {
            let skin = sk::Skin { weight: inlier.clone(), inlier: inlier.clone(), n: 1, median: [0.0; 3], mad: [0.0; 3] };
            (sk::hf_variance_log(&fr.lab, &skin, sigma), sk::hf_variance(&fr.lab, &skin, sigma))
        };
        let (b, a) = (mk(&before), mk(&after));
        vb += b.0;
        va += a.0;
        lb += b.1;
        la += a.1;
        n += 1.0;
    }
    Ok(Some((qc::texture_change(vb / n.max(1.0), va / n.max(1.0)), qc::texture_change(lb / n.max(1.0), la / n.max(1.0)))))
}

fn check(s: &mut Session, pp: &PhotoPlan, o: &Opts, dose: f32, c: &str) -> Result<(DevelopSettings, Arc<Frame>, Qc)> {
    let d = compose(pp, o, dose, false);
    let fin = analysis::render(s, pp.id, &d, analysis::EDGE, c)?;
    let tx = texture(s, pp, &d, c)?;
    let mut q = Qc { texture_change: tx.map(|t| t.0), texture_change_lstar: tx.map(|t| t.1), ..Default::default() };
    // lightness of the skin
    for skn in pp.ph.skins.iter().flatten() {
        let m = skn.skin.restat(&fin.lab).median;
        q.skin_l = q.skin_l.max((m[0] - skn.skin.median[0]).abs());
    }
    if enabled(o, pp.steps.iter().find(|s| s.id == "finish").unwrap_or(&pp.steps[0])) {
        q.halo = qc::halo(&pp.ph.frame.lab.l, &fin.lab.l, &pp.subject);
    }
    // outside the treated zones nothing may change (masks only: spots and mixer are not local)
    let local = compose(pp, o, dose, true);
    let lf = analysis::render(s, pp.id, &local, analysis::EDGE, c)?;
    let mut union = Plane::new(fin.w, fin.h, 0.0);
    for m in local.masks.iter().filter(|m| !pp.ph.base.masks.iter().any(|b| b.id == m.id && b.name == m.name)) {
        union = union.zip(&outmask::alpha_of(m, fin.w, fin.h), f32::max);
    }
    // the user's own masks were in the base: pixels they touch are the same before and after
    let (mx, n) = qc::outside_change(
        &analysis::render(s, pp.id, &pp.ph.base, analysis::EDGE, c)?.rgba,
        &lf.rgba,
        &union.map(|v| if v > 0.0 { v } else { 0.0 }),
    );
    q.outside_max = mx;
    q.outside_n = n;
    Ok((d, fin, q))
}

fn over(q: &Qc) -> Option<String> {
    if let Some(t) = q.texture_change
        && t.abs() > qc::TEXTURE_TOLERANCE
    {
        return Some(format!("skin texture changed by {:.1} % (tolerance {:.0} %)", t * 100.0, qc::TEXTURE_TOLERANCE * 100.0));
    }
    if q.halo > qc::HALO_TOLERANCE {
        return Some(format!("a halo of {:.1} L around the subject", q.halo));
    }
    if q.skin_l > qc::SKIN_L_TOLERANCE {
        return Some(format!("the skin's lightness moved by {:.1} L", q.skin_l));
    }
    None
}

/// Mean L of the image where `alpha` is above 0.5, before and after, and the area share.
fn region_l(alpha: &Plane, before: &Frame, after: &Frame) -> Option<(f32, f32, f32)> {
    let (mut b, mut a, mut n) = (0f64, 0f64, 0f64);
    for i in 0..alpha.d.len() {
        if alpha.d[i] > 0.5 {
            b += f64::from(before.lab.l.d[i]);
            a += f64::from(after.lab.l.d[i]);
            n += 1.0;
        }
    }
    (n > 30.0).then(|| ((b / n) as f32, (a / n) as f32, (n / alpha.d.len() as f64) as f32))
}

fn mask_alpha(pp: &PhotoPlan, d: &DevelopSettings, key: &str, fin: &Frame) -> Option<Plane> {
    let _ = pp;
    d.masks.iter().find(|m| m.name.contains(key)).map(|m| outmask::alpha_of(m, fin.w, fin.h))
}

fn explain(pp: &PhotoPlan, o: &Opts, d: &DevelopSettings, fin: &Frame, st: &Step) -> (String, Value) {
    let before = &pp.ph.frame;
    match st.id {
        "camera" => (st.info.clone(), json!({})),
        "clean" => (st.info.clone(), json!({"healed": st.spots.len(), "onSkinLeftAlone": st.skin_candidates})),
        "skin_color" => {
            let mut parts = Vec::new();
            let mut m = Vec::new();
            for (i, skn) in pp.ph.skins.iter().enumerate() {
                let Some(skn) = skn else { continue };
                let (lp, big, _) = sigmas(skn.width);
                let r = skn.skin.restat(&fin.lab);
                let a = sk::metrics(&fin.lab, &r, lp, big);
                let who = pp.ph.found[i].person.clone().map_or(String::new(), |n| format!(" ({n})"));
                parts.push(format!("face {i}{who}: skin colour made uniform from ΔE {:.1} to {:.1}", skn.before.de_color_mean, a.de_color_mean));
                let mut mj = json!({"face": i, "uniformityBefore": round1(skn.before.de_color_mean), "uniformityAfter": round1(a.de_color_mean), "medianBefore": skinmod::metrics_json(&skn.before)["medianLab"], "medianAfter": skinmod::metrics_json(&a)["medianLab"]});
                if let Some(Some(t)) = pp.targets.get(i) {
                    let f = |x: [f32; 3]| lightcraft_retouch::lab::de00(x, [x[0], t.ab[0], t.ab[1]]);
                    parts.push(format!("distance to {}'s colour over the series from ΔE {:.1} to {:.1}", t.person, f(skn.skin.median), f(r.median)));
                    mj["toSeriesBefore"] = json!(round1(f(skn.skin.median)));
                    mj["toSeriesAfter"] = json!(round1(f(r.median)));
                }
                m.push(mj);
            }
            (parts.join("; "), json!({"faces": m}))
        }
        "dnb_cleanup" => {
            let mut parts = Vec::new();
            for (i, skn) in pp.ph.skins.iter().enumerate() {
                let Some(skn) = skn else { continue };
                let (lp, big, _) = sigmas(skn.width);
                let r = skn.skin.restat(&fin.lab);
                let a = sk::metrics(&fin.lab, &r, lp, big);
                parts.push(format!("face {i}: lightness irregularities of the skin from {:.1} to {:.1} L (spread of the mid-frequency band)", skn.before.l_band_sd, a.l_band_sd));
            }
            (parts.join("; "), json!({}))
        }
        "dnb_sculpt" => {
            let lit = ["cheekbones", "nose ridge", "forehead", "Dodge: subject", "Dodge: foreground"].iter().filter_map(|k| mask_alpha(pp, d, k, fin)).fold(None::<Plane>, |a, p| Some(a.map_or(p.clone(), |x| x.zip(&p, f32::max))));
            let dark = ["sides of the face", "hot spots", "Burn: sky"].iter().filter_map(|k| mask_alpha(pp, d, k, fin)).fold(None::<Plane>, |a, p| Some(a.map_or(p.clone(), |x| x.zip(&p, f32::max))));
            let l = lit.and_then(|a| region_l(&a, before, fin));
            let k = dark.and_then(|a| region_l(&a, before, fin));
            let mut s = Vec::new();
            if let Some((b, a, _)) = l {
                s.push(format!("lit planes {:+.1} L (from {:.1} to {:.1})", a - b, b, a));
            }
            if let Some((b, a, _)) = k {
                s.push(format!("shaded sides {:+.1} L (from {:.1} to {:.1})", a - b, b, a));
            }
            (if s.is_empty() { "no relief added".into() } else { s.join("; ") }, json!({}))
        }
        "eyes" => {
            let mut s = Vec::new();
            for (key, label) in [("eye whites", "eye whites"), ("irises", "irises"), ("under the eyes", "under the eyes"), ("lips", "lips")] {
                if let Some(r) = mask_alpha(pp, d, key, fin).and_then(|a| region_l(&a, before, fin)) {
                    s.push(format!("{label} {:+.1} L", r.1 - r.0));
                }
            }
            if let Some(a) = mask_alpha(pp, d, "Teeth", fin) {
                let (mut b0, mut b1, mut n) = (0f64, 0f64, 0f64);
                for i in 0..a.d.len() {
                    if a.d[i] > 0.5 {
                        b0 += f64::from(before.lab.b.d[i]);
                        b1 += f64::from(fin.lab.b.d[i]);
                        n += 1.0;
                    }
                }
                if n > 20.0 {
                    s.push(format!("teeth: yellow (b*) from {:.1} to {:.1}", b0 / n, b1 / n));
                }
            }
            (if s.is_empty() { "no eye or mouth part found".into() } else { s.join("; ") }, json!({}))
        }
        "color" => {
            let sm = |f: &Frame| palmod::samples_of(f, None);
            let (db, da) = (pal::distance(&sm(before), &st.target), pal::distance(&sm(fin), &st.target));
            (format!("palette distance from ΔE {db:.1} to {da:.1} (colour only, lightness left out); skin protected"), json!({"distanceBefore": round1(db), "distanceAfter": round1(da)}))
        }
        "finish" => {
            let mut s = Vec::new();
            for (key, label) in [("background", "background"), ("Burn: edges", "edges"), ("distractions", "distractions")] {
                if let Some(r) = mask_alpha(pp, d, key, fin).and_then(|a| region_l(&a, before, fin)) {
                    s.push(format!("{label} {:+.1} L", r.1 - r.0));
                }
            }
            (if s.is_empty() { "nothing to darken".into() } else { s.join("; ") }, json!({}))
        }
        _ => (String::new(), json!({})),
    }
    .tap_enabled(o, st)
}

trait TapEnabled {
    fn tap_enabled(self, o: &Opts, st: &Step) -> Self;
}
impl TapEnabled for (String, Value) {
    fn tap_enabled(self, _: &Opts, _: &Step) -> Self {
        self
    }
}

struct PlanSet {
    photos: Vec<PhotoPlan>,
}

fn cache() -> &'static Mutex<Vec<(u64, Arc<PlanSet>)>> {
    static C: Mutex<Vec<(u64, Arc<PlanSet>)>> = Mutex::new(Vec::new());
    &C
}

fn key_of(s: &Session, ids: &[PhotoId], p: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for id in ids {
        id.0.hash(&mut h);
        s.develop_of(*id).map(|d| d.hash64()).hash(&mut h);
    }
    for k in ["steps", "target", "person", "reference", "protectSkin", "confirmSpots"] {
        p.get(k).map(Value::to_string).hash(&mut h);
    }
    h.finish()
}

fn ids_of(s: &Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p)? } else { s.active().into_iter().collect() };
    if ids.is_empty() { Err(bad(c, "no photo: give `ids`, or select one")) } else { Ok(ids) }
}

fn plans(s: &mut Session, ids: &[PhotoId], p: &Value, c: &str) -> Result<Arc<PlanSet>> {
    let key = key_of(s, ids, p);
    {
        let g = cache().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, v)) = g.iter().find(|(k, _)| *k == key) {
            return Ok(v.clone());
        }
    }
    let o = opts_of(p);
    let photos = skinmod::survey(s, ids, c)?;
    let targets = skinmod::series_targets(&photos, o.person.as_deref(), o.reference);
    // persistent dust of the series (3 photos or more of one size)
    let (mut dust, mut ran, mut overflow) = (Vec::new(), false, 0usize);
    if ids.len() >= 3
        && let Ok(v) = s.execute("dust.detect", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
    {
        ran = true;
        for sp in v["spots"].as_array().into_iter().flatten() {
            if let (Some(x), Some(y), Some(r)) = (sp["x"].as_f64(), sp["y"].as_f64(), sp["radius"].as_f64()) {
                dust.push((x, y, r));
            }
        }
    }
    // sensor dust is a handful of spots; hundreds are detail, not dust
    if dust.len() > MAX_DUST {
        overflow = dust.len();
        dust.clear();
    }
    let mut out = Vec::new();
    for (ph, t) in photos.into_iter().zip(targets) {
        out.push(plan_photo(s, ph, t, &o, &dust, ran, overflow, c)?);
    }
    let set = Arc::new(PlanSet { photos: out });
    let mut g = cache().lock().unwrap_or_else(PoisonError::into_inner);
    g.push((key, set.clone()));
    if g.len() > 4 {
        g.remove(0);
    }
    Ok(set)
}

/// Settle a photo at `dose`: reduce the dose while a check is over its threshold.
#[allow(clippy::unreachable)] // the loop returns on its last round; the tail is a proof, not a path
fn settle(s: &mut Session, pp: &PhotoPlan, o: &Opts, dose: f32, c: &str) -> Result<(f32, DevelopSettings, Arc<Frame>, Qc, Option<Value>)> {
    let mut dz = dose;
    let mut reduced = None;
    for round in 0..4 {
        let (d, fin, q) = check(s, pp, o, dz, c)?;
        match over(&q) {
            Some(why) if round < 3 => {
                reduced = Some(json!({"from": round2(dose), "to": round2(dz * 0.75), "reason": why}));
                dz *= 0.75;
            }
            _ => return Ok((dz, d, fin, q, reduced)),
        }
    }
    unreachable!("the loop returns on its last round")
}

/// The plan of a photo as JSON, with the settings it comes to and the dose it settled at.
fn photo_full(s: &mut Session, pp: &PhotoPlan, o: &Opts, dose: f32, c: &str) -> Result<(Value, DevelopSettings, f32)> {
    let (dz, d, fin, q, reduced) = settle(s, pp, o, dose, c)?;
    let mut steps = Vec::new();
    for (i, st) in pp.steps.iter().enumerate() {
        let on = enabled(o, st);
        let (text, metrics) = if on { explain(pp, o, &d, &fin, st) } else { (st.reason.clone().unwrap_or_else(|| "switched off".into()), json!({})) };
        steps.push(json!({"n": i + 1, "id": st.id, "label": st.label, "available": st.available, "enabled": on, "reason": st.reason, "explain": text, "metrics": metrics,
            "masks": st.masks.iter().map(|m| m.name.clone()).collect::<Vec<_>>(), "warnings": st.warnings}));
    }
    let v = json!({"id": pp.id.0, "mode": pp.mode.name(), "faces": pp.ph.skins.iter().flatten().count(), "faceNote": pp.ph.faces_note, "dose": round2(dz), "autoReduced": reduced, "steps": steps,
        "qc": {"textureChange": q.texture_change.map(round2), "textureChangeLstar": q.texture_change_lstar.map(round2), "textureTolerance": qc::TEXTURE_TOLERANCE, "halo": round2(q.halo), "skinLightnessShift": round2(q.skin_l),
               "outsideMaxLevelChange": q.outside_max, "outsidePixelsChanged": q.outside_n}});
    Ok((v, d, dz))
}

fn propose(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "retouch.propose";
    let ids = ids_of(s, p, C)?;
    if let Some(r) = str_param(p, "recipe")
        && r != RECIPE
    {
        return Err(bad(C, format!("unknown recipe `{r}` (retouch.recipes)")));
    }
    let o = opts_of(p);
    let dose = dose_of(p);
    let set = plans(s, &ids, p, C)?;
    let mut out = Vec::new();
    // `settings`: also the settings the plan comes to, so a frontend can show the effect without writing it
    let with_settings = bool_or(p, "settings", false);
    for pp in &set.photos {
        let (mut v, d, _) = photo_full(s, pp, &o, dose, C)?;
        if with_settings {
            v["settings"] = serde_json::to_value(&d).unwrap_or(Value::Null);
        }
        out.push(v);
    }
    Ok(json!({"recipe": RECIPE, "impact": round2(dose), "photos": out, "written": false}))
}

/// "Natural skin – Light": the name of a retouch copy (the dash is how `retouch.clear` finds them).
const COPY_PREFIX: &str = "Natural skin – ";

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "retouch.apply";
    let ids = ids_of(s, p, C)?;
    let o = opts_of(p);
    let dose = dose_of(p);
    let set = plans(s, &ids, p, C)?;
    let variants = bool_or(p, "variants", false);
    // `copy`: the retouch goes on virtual copies named "Natural skin – Light / Medium / Strong", stacked with
    // the original, which is not touched (the app's Apply does this); without it the settings are written on
    // the photo itself, as before
    if bool_or(p, "copy", false) {
        let undo0 = s.undo.len();
        let impact = str_param(p, "impact").unwrap_or("medium").to_lowercase();
        let doses: Vec<(&str, f32)> = if variants {
            vec![("Light", LIGHT), ("Medium", MEDIUM), ("Strong", STRONG)]
        } else {
            vec![(
                match impact.as_str() {
                    "light" => "Light",
                    "strong" => "Strong",
                    _ => "Medium",
                },
                dose,
            )]
        };
        let (mut out, mut made_all, mut shown) = (Vec::new(), Vec::new(), Vec::new());
        // every plan is settled first: making a copy changes the catalog the plans were read from
        let mut ready = Vec::new();
        for pp in &set.photos {
            for (label, dd) in &doses {
                let (summary, mut d, dz) = photo_full(s, pp, &o, *dd, C)?;
                d.masks.retain(|m| m.name.starts_with(super::PREFIX) || pp.ph.base.masks.iter().any(|b| b.id == m.id));
                ready.push((pp.id, *label, summary, d, dz));
            }
        }
        for (orig, label, summary, d, dz) in ready {
            let r = s.execute("photo.virtualCopy", &json!({"ids": [orig.0], "name": format!("{COPY_PREFIX}{label}")}))?;
            let Some(copy) = r["ids"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(PhotoId) else { continue };
            if let Some(op) = s.develop_op(copy, d, "Natural Skin Retouch") {
                s.commit("Natural Skin Retouch", op)?;
            }
            made_all.push(copy);
            if !variants || label == "Medium" {
                shown.push(copy);
            }
            out.push(json!({"id": orig.0, "copy": copy.0, "name": format!("{COPY_PREFIX}{label}"), "dose": round2(dz), "summary": summary}));
        }
        let steps = s.undo.len().saturating_sub(undo0);
        s.merge_undo(steps, "Natural Skin Retouch");
        if let Some(last) = shown.last().copied() {
            s.selection = crate::Selection { ids: shown.clone(), active: Some(last) };
        }
        return Ok(
            json!({"recipe": RECIPE, "photos": out, "written": true, "variants": variants, "copies": made_all.iter().map(|i| i.0).collect::<Vec<_>>()}),
        );
    }
    let mut ops = Vec::new();
    let mut out = Vec::new();
    for pp in &set.photos {
        let current = s.develop_of(pp.id).ok_or_else(|| bad(C, "no such photo"))?;
        // our own earlier result goes (same names); what the user added stays
        let (summary, mut d, dz) = photo_full(s, pp, &o, dose, C)?;
        // keep the user's masks that were added after: they are in `base` already (base = current without ours)
        d.masks.retain(|m| m.name.starts_with(super::PREFIX) || pp.ph.base.masks.iter().any(|b| b.id == m.id));
        ops.extend(s.develop_op(pp.id, d, "Natural Skin Retouch"));
        if variants {
            let mut versions = s.catalog.photo(pp.id).map(|ph| ph.versions.clone()).unwrap_or_default();
            let stamp = (s.clock)();
            let mut push = |name: &str, settings: DevelopSettings| {
                versions.push(Version { name: name.to_string(), created: stamp.clone(), settings: Arc::new(settings), auto: false })
            };
            push("Natural skin · Before", (*current).clone());
            for (label, dd) in [("Light", LIGHT), ("Medium", MEDIUM), ("Strong", STRONG)] {
                let (_, v, _, _, _) = settle(s, pp, &o, dd, C)?;
                push(&format!("Natural skin · {label}"), v);
            }
            ops.push(Op::SetVersions { id: pp.id, versions });
        }
        out.push(json!({"id": pp.id.0, "dose": round2(dz), "summary": summary}));
    }
    if !ops.is_empty() {
        s.commit("Natural Skin Retouch", Op::Batch { ops })?;
    }
    Ok(json!({"recipe": RECIPE, "photos": out, "written": true, "variants": variants}))
}

/// The retouch copies of `ids` (a copy given, or the copies of the photo given).
fn retouch_copies(s: &Session, ids: &[PhotoId]) -> Vec<PhotoId> {
    let mut v: Vec<PhotoId> = s
        .catalog
        .photos()
        .filter(|c| !c.deleted && c.copy_name.as_deref().is_some_and(|n| n.starts_with(COPY_PREFIX)))
        .filter(|c| ids.contains(&c.id) || c.copy_of.is_some_and(|m| ids.contains(&m)))
        .map(|c| c.id)
        .collect();
    v.sort();
    v
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = ids_of(s, p, "retouch.clear")?;
    // retouch copies go to Recently Deleted (the original was never touched); only a photo that has none
    // is cleared of the proposed-retouch masks, which is how v10/v11 wrote it
    let copies = retouch_copies(s, &ids);
    if !copies.is_empty() {
        s.execute("photo.delete", &json!({"ids": copies.iter().map(|i| i.0).collect::<Vec<_>>()}))?;
        return Ok(json!({"removed": 0, "copiesRemoved": copies.len()}));
    }
    super::clear_masks(s, &ids, super::PREFIX, "Clear Proposed Retouch")
}

fn recipes(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"recipes": [{"id": RECIPE, "label": "Natural skin", "impacts": {"light": LIGHT, "medium": MEDIUM, "strong": STRONG},
        "steps": STEPS.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>()}]}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "retouch.recipes",
            "Retouch Recipes",
            [],
            None,
            "→ {recipes: [{id, label, impacts: {light, medium, strong}, steps: [{id, label}]}]}",
            always,
            recipes
        ),
        cmd!(
            query "retouch.propose",
            "Propose Retouch",
            [],
            None,
            "{ids?, recipe?: \"natural-skin\", impact?: light|medium|strong|0.05..1 (medium 0.7), steps?: {camera|clean|skin_color|dnb_cleanup|dnb_sculpt|eyes|color|finish: bool}, target?: {palette|photo|hexes|scheme} (enables the colour step), person?: name, reference?: photo id, protectSkin?: true, confirmSpots?: [[x, y], …], settings?: bool (also return each photo's planned settings, to preview them without writing)} — plan the recipe's eight ordered steps for each photo and WRITE NOTHING: per step whether it applies, its masks, an explanation with measured figures (\"skin colour made uniform from ΔE 4.2 to 1.8\"), warnings; quality checks (skin texture change, halo, skin lightness, change outside the treated zones) and an automatic dose reduction when one is over its threshold → {recipe, impact, photos: [{id, mode, faces, dose, autoReduced, steps: [{n, id, label, enabled, explain, metrics, masks, warnings}], qc}], written: false}",
            always,
            propose
        ),
        cmd!(
            "retouch.apply",
            "Apply Proposed Retouch",
            [],
            None,
            "{…same as retouch.propose, variants?: bool, copy?: bool} — write what retouch.propose planned as ordinary editable settings (masks \"Retouch: …\", the Colour Mixer, heal spots for sensor dust outside faces) in ONE undo step; `variants` also leaves \"Natural skin · Before / Light / Medium / Strong\" in Versions for before/after; `copy` (what the app's Apply uses) writes the retouch on virtual copies named \"Natural skin – Light / Medium / Strong\" (one copy at the impact chosen, three with `variants`) stacked with the untouched original instead → {recipe, photos: [{id, dose, summary}], written, variants}. No mole, scar or tattoo is removed unless its position is in `confirmSpots`",
            always,
            apply
        ),
        cmd!(
            "retouch.clear",
            "Remove Proposed Retouch",
            [],
            None,
            "{ids?} — remove every \"Retouch: …\" mask (skin, dodge & burn, eyes, finishing) → {removed}",
            always,
            clear
        ),
    ]
}

#[allow(dead_code)]
fn _keep(_: Option<(&Opts, f64)>) {
    let _ = f64_or;
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::DevelopSettings;

    fn demo() -> (Session, PhotoId) {
        let s = Session::with_demo();
        let id = s.active().expect("a demo photo is active");
        (s, id)
    }

    fn same_untouchables(a: &DevelopSettings, b: &DevelopSettings) -> bool {
        a.crop == b.crop
            && a.geometry == b.geometry
            && a.orientation == b.orientation
            && a.lens_blur == b.lens_blur
            && a.enhance == b.enhance
            && a.red_eye == b.red_eye
            && a.optics == b.optics
            && a.detail == b.detail
            && a.light == b.light
            && a.wb == b.wb
            && a.color == b.color
            && a.curve == b.curve
    }

    #[test]
    fn proposing_writes_nothing() {
        let (mut s, id) = demo();
        let before = s.develop_of(id).expect("develop");
        let undo = s.undo.len();
        let r = s.execute("retouch.propose", &json!({"id": id.0, "impact": "light"})).expect("propose");
        assert_eq!(r["written"], false);
        assert_eq!(r["photos"][0]["steps"].as_array().map(Vec::len), Some(8), "{r}");
        assert!(*s.develop_of(id).expect("develop") == *before && s.undo.len() == undo, "nothing written, no undo step");
    }

    #[test]
    fn apply_is_one_undo_step_and_writes_only_ordinary_settings() {
        let (mut s, id) = demo();
        let before = (*s.develop_of(id).expect("develop")).clone();
        let undo = s.undo.len();
        let r = s.execute("retouch.apply", &json!({"id": id.0})).expect("apply");
        assert_eq!(r["written"], true, "{r}");
        let after = (*s.develop_of(id).expect("develop")).clone();
        assert_eq!(s.undo.len(), undo + 1, "one undo step");
        // red lines: no geometry, crop, tone of the whole photo, resampling or generated pixels
        assert!(same_untouchables(&before, &after), "only masks, mixer and heal spots may change");
        assert!(after.masks.iter().all(|m| m.name.starts_with(super::super::PREFIX)), "every mask is a named, editable Retouch mask");
        assert!(after.spots.starts_with(&before.spots), "heal spots are only added, never changed");
        assert!(
            after.spots.iter().all(|sp| sp.mode == SpotMode::Heal && sp.patch.is_none()),
            "no generated pixels: heal spots only, never an inpainted patch"
        );
        let q = &r["photos"][0]["summary"]["qc"];
        assert_eq!(q["outsidePixelsChanged"], 0, "nothing changes outside the treated zones: {q}");
        s.execute("edit.undo", &json!({})).expect("undo");
        assert!(*s.develop_of(id).expect("develop") == before, "one undo brings the photo back exactly");
    }

    #[test]
    fn variants_leave_before_light_medium_strong_in_versions() {
        let (mut s, id) = demo();
        let undo = s.undo.len();
        s.execute("retouch.apply", &json!({"id": id.0, "variants": true})).expect("apply");
        let names: Vec<String> = s.catalog.photo(id).expect("photo").versions.iter().map(|v| v.name.clone()).collect();
        for n in ["Before", "Light", "Medium", "Strong"] {
            assert!(names.iter().any(|x| x == &format!("Natural skin · {n}")), "{n} in {names:?}");
        }
        assert_eq!(s.undo.len(), undo + 1, "one undo step for the photo and its versions");
        s.execute("edit.undo", &json!({})).expect("undo");
        assert!(s.catalog.photo(id).expect("photo").versions.is_empty());
    }

    #[test]
    fn a_copy_carries_the_retouch_and_the_original_is_never_touched() {
        let (mut s, id) = demo();
        let before = s.develop_of(id).expect("develop");
        let (photos, undo0) = (s.catalog.photos().count(), s.undo.len());
        let r = s.execute("retouch.apply", &json!({"id": id.0, "impact": "light", "copy": true})).expect("apply as a copy");
        assert_eq!(r["copies"].as_array().map(Vec::len), Some(1), "{r}");
        assert_eq!(s.catalog.photos().count(), photos + 1);
        assert_eq!(s.undo.len(), undo0 + 1, "one undo step");
        assert!(*s.develop_of(id).expect("develop") == *before, "the original keeps its settings");
        assert!(s.catalog.photo(id).expect("photo").versions.is_empty(), "nothing written in Versions");
        let copy = s.catalog.photos().find(|p| p.copy_of == Some(id)).expect("a copy").clone();
        assert_eq!(copy.copy_name.as_deref(), Some("Natural skin – Light"));
        assert!(copy.develop.masks.iter().all(|m| m.name.starts_with(super::super::PREFIX) || before.masks.iter().any(|b| b.id == m.id)));
        assert!(s.catalog.stack_of(id).is_some_and(|st| st.photos.contains(&copy.id)), "stacked with the original");
        // three copies with variants
        let r = s.execute("retouch.apply", &json!({"id": id.0, "copy": true, "variants": true})).expect("apply 3 copies");
        assert_eq!(r["copies"].as_array().map(Vec::len), Some(3), "{r}");
        let mut names: Vec<String> = s.catalog.photos().filter(|p| p.copy_of == Some(id)).filter_map(|p| p.copy_name.clone()).collect();
        names.sort();
        assert_eq!(names.iter().filter(|n| n.ends_with("Light")).count(), 2, "{names:?}");
        assert!(*s.develop_of(id).expect("develop") == *before);
        // clear removes the copies and only them
        let c = s.execute("retouch.clear", &json!({"id": id.0})).expect("clear");
        assert_eq!(c["copiesRemoved"], 4, "{c}");
        assert!(s.catalog.photos().filter(|p| p.copy_of == Some(id)).all(|p| p.deleted));
        assert!(!s.catalog.photo(id).expect("photo").deleted && *s.develop_of(id).expect("develop") == *before);
    }

    #[test]
    fn a_proposal_can_return_its_settings_for_a_preview_and_still_writes_nothing() {
        let (mut s, id) = demo();
        let (undo0, photos) = (s.undo.len(), s.catalog.photos().count());
        let r = s.execute("retouch.propose", &json!({"id": id.0, "settings": true})).expect("propose");
        assert!(r["photos"][0]["settings"].is_object(), "the planned settings come back");
        assert!(serde_json::from_value::<DevelopSettings>(r["photos"][0]["settings"].clone()).is_ok(), "and they are settings");
        let plain = s.execute("retouch.propose", &json!({"id": id.0})).expect("propose");
        assert!(plain["photos"][0].get("settings").is_none());
        assert_eq!((s.undo.len(), s.catalog.photos().count()), (undo0, photos));
    }

    #[test]
    fn a_stronger_impact_is_a_bigger_dose_of_the_same_masks() {
        let (mut s, id) = demo();
        s.execute("retouch.apply", &json!({"id": id.0, "impact": "light"})).expect("light");
        let light = s.develop_of(id).expect("develop").masks.iter().map(|m| m.adjust.exposure.abs()).sum::<f64>();
        s.execute("edit.undo", &json!({})).expect("undo");
        s.execute("retouch.apply", &json!({"id": id.0, "impact": "strong"})).expect("strong");
        let strong = s.develop_of(id).expect("develop").masks.iter().map(|m| m.adjust.exposure.abs()).sum::<f64>();
        assert!(strong > light * 1.5, "{light} vs {strong}");
    }

    #[test]
    fn a_spot_is_healed_only_when_it_is_confirmed() {
        let (mut s, id) = demo();
        let at = |s: &Session, x: f64, y: f64| {
            s.develop_of(id).expect("develop").spots.iter().any(|sp| sp.points.first().is_some_and(|p| (p.x - x).hypot(p.y - y) < 0.02))
        };
        s.execute("retouch.apply", &json!({"id": id.0, "steps": {"clean": true}})).expect("apply");
        let spots_without = s.develop_of(id).expect("develop").spots.len();
        s.execute("edit.undo", &json!({})).expect("undo");
        s.execute("retouch.apply", &json!({"id": id.0, "confirmSpots": [[0.5, 0.5]]})).expect("apply");
        assert!(at(&s, 0.5, 0.5), "the confirmed spot is there");
        assert_eq!(s.develop_of(id).expect("develop").spots.len(), spots_without + 1);
    }

    #[test]
    fn a_candidate_on_a_face_is_left_alone() {
        let cov = Plane::new(100, 100, 0.0).zip(&Plane::new(100, 100, 0.0), |a, _| a);
        let mut face = cov.clone();
        for y in 30..70 {
            for x in 30..70 {
                face.d[y * 100 + x] = 1.0;
            }
        }
        let regions = [(&face, [25.0f32, 25.0, 75.0, 75.0])];
        assert!(near_face(&regions, 50, 50), "on the skin");
        assert!(near_face(&regions, 27, 27), "inside the face box, beside the skin");
        assert!(!near_face(&regions, 5, 5), "far from any face");
        assert!(!near_face(&[], 50, 50));
    }

    #[test]
    fn a_check_over_its_threshold_reduces_the_dose() {
        let ok = Qc::default();
        assert!(over(&ok).is_none());
        assert!(over(&Qc { texture_change: Some(-0.08), ..Default::default() }).is_some_and(|w| w.contains("texture")));
        assert!(over(&Qc { texture_change: Some(0.04), ..Default::default() }).is_none());
        assert!(over(&Qc { halo: 1.6, ..Default::default() }).is_some_and(|w| w.contains("halo")));
        assert!(over(&Qc { skin_l: 2.0, ..Default::default() }).is_some_and(|w| w.contains("lightness")));
        // the scaled masks carry smaller adjustments
        let m = Mask { adjust: LocalAdjustments { exposure: 0.4, saturation: -30.0, ..Default::default() }, ..Default::default() };
        let half = scaled_mask(&m, 0.5, 7);
        assert_eq!((half.id, half.adjust.exposure, half.adjust.saturation), (7, 0.2, -15.0));
    }

    #[test]
    fn the_retouch_never_reaches_the_network_or_a_generator() {
        let forbidden: Vec<String> = [
            ["std::", "net"].concat(),
            ["Tcp", "Stream"].concat(),
            ["req", "west"].concat(),
            ["u", "req"].concat(),
            ["lightcraft_", "fetch"].concat(),
            ["lightcraft_", "inpaint"].concat(),
            ["SpotMode::", "Inpaint"].concat(),
            ["Command::", "new"].concat(),
        ]
        .into();
        for (name, src) in [
            ("mod", include_str!("mod.rs")),
            ("analysis", include_str!("analysis.rs")),
            ("outmask", include_str!("outmask.rs")),
            ("skin", include_str!("skin.rs")),
            ("dnb", include_str!("dnb.rs")),
            ("palette", include_str!("palette.rs")),
            ("recipe", include_str!("recipe.rs")),
        ] {
            // the test module of this file names the tokens: look above it
            let code = src.split("#[cfg(test)]").next().unwrap_or(src);
            for f in &forbidden {
                assert!(!code.contains(f.as_str()), "{name}.rs mentions `{f}`");
            }
        }
        for (name, src) in [
            ("crate lab", include_str!("../../../retouch/src/lab.rs")),
            ("crate skin", include_str!("../../../retouch/src/skin.rs")),
            ("crate dnb", include_str!("../../../retouch/src/dnb.rs")),
            ("crate palette", include_str!("../../../retouch/src/palette.rs")),
        ] {
            for f in &forbidden {
                assert!(!src.contains(f.as_str()), "{name} mentions `{f}`");
            }
        }
    }
}
