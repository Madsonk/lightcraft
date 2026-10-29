//! Palettes (`palette.extract`, `palette.harmonize`, `palette.save`, `palette.list`): the dominant
//! colours of a photo or a batch, named and as hex; a move of the Colour Mixer toward a target
//! palette (a reference photo, a scheme, a list of hex colours, a saved palette), the skin
//! protected, one parameter set for a whole batch so that it stays coherent. The result is the
//! ordinary Colour Mixer (editable), never a LUT.

use std::path::PathBuf;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Hsl, Mixer};
use lightcraft_retouch::lab::{chroma, de00, hex, hue_deg, hue_diff, lab_to_rgb8, rgb8_to_lab};
use lightcraft_retouch::palette::{self, BAND_CENTERS, BandPlan, Sample, Swatch};
use serde_json::{Value, json};

use super::analysis::{self, Frame};
use super::skin::{PhotoSkin, round1, round2};
use crate::cmd::{CommandSpec, always, bad, bool_or, cmd, f64_or, str_param};
use crate::{Result, Session};

/// About this many samples are taken from a frame.
const SAMPLES: usize = 16_000;

/// A palettes file chosen by the host or a test (before the environment and the config folder).
static OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Keep the named palettes in `path` from now on (`None` = back to the defaults).
#[cfg(test)]
pub fn set_store(path: Option<PathBuf>) {
    *OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = path;
}

/// Where the named palettes live (`$LIGHTCRAFT_PALETTES` for tests, else `<config>/palettes.json`).
pub fn store_path() -> Option<PathBuf> {
    if let Some(p) = OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
        return Some(p);
    }
    std::env::var_os("LIGHTCRAFT_PALETTES")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::config::config_dir().map(|d| d.join("palettes.json")))
}

fn load_store() -> Vec<Value> {
    store_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("palettes").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn save_store(list: &[Value]) -> std::result::Result<(), String> {
    let path = store_path().ok_or("no folder for the palettes")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("part");
    let body = serde_json::to_vec_pretty(&json!({"palettes": list})).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Samples of a frame at a fixed stride, skipping the pixels `skip` marks.
pub fn samples_of(frame: &Frame, skip: Option<&dyn Fn(usize) -> bool>) -> Vec<Sample> {
    let stride = (((frame.w * frame.h) as f32 / SAMPLES as f32).sqrt().ceil() as usize).max(1);
    let mut out = Vec::new();
    let mut y = stride / 2;
    while y < frame.h {
        let mut x = stride / 2;
        while x < frame.w {
            let i = y * frame.w + x;
            if skip.is_none_or(|f| !f(i)) {
                let p = frame.rgba[i];
                out.push(([p[0], p[1], p[2]], frame.lab.lab_at(i), 1.0));
            }
            x += stride;
        }
        y += stride;
    }
    out
}

fn swatch_json(s: &Swatch) -> Value {
    json!({"hex": s.hex, "lab": [round1(s.lab[0]), round1(s.lab[1]), round1(s.lab[2])], "share": round2(s.share), "name": s.name, "nameFr": s.name_fr})
}

fn ids_of(s: &Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p) } else { s.active().into_iter().collect() };
    if ids.is_empty() { Err(bad(c, "no photo: give `ids`, or select one")) } else { Ok(ids) }
}

fn frame_of(s: &mut Session, id: PhotoId, c: &str) -> Result<std::sync::Arc<Frame>> {
    let d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
    analysis::render(s, id, &d, analysis::EDGE, c)
}

fn extract(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "palette.extract";
    let ids = ids_of(s, p, C)?;
    let k = f64_or(p, "k", 6.0).clamp(1.0, 12.0) as usize;
    let lot = bool_or(p, "lot", false) || p.get("ids").is_some_and(|a| a.as_array().is_some_and(|a| a.len() > 1)) && !bool_or(p, "each", false);
    let mut per = Vec::new();
    let mut pooled: Vec<[f32; 3]> = Vec::new();
    for id in &ids {
        let f = frame_of(s, *id, C)?;
        let sm: Vec<[f32; 3]> = samples_of(&f, None).into_iter().map(|x| x.1).collect();
        if lot {
            pooled.extend(sm);
        } else {
            per.push(json!({"id": id.0, "palette": palette::extract(&sm, k).iter().map(swatch_json).collect::<Vec<_>>()}));
        }
    }
    if lot {
        let sw = palette::extract(&pooled, k);
        let hexes: Vec<String> = sw.iter().map(|x| x.hex.clone()).collect();
        return Ok(json!({"lot": true, "photos": ids.len(), "palette": sw.iter().map(swatch_json).collect::<Vec<_>>(), "copy": hexes.join(" ")}));
    }
    let copy: Vec<String> =
        per.iter().flat_map(|v| v["palette"].as_array().into_iter().flatten().filter_map(|c| c["hex"].as_str().map(str::to_string))).collect();
    Ok(json!({"lot": false, "photos": per, "copy": copy.join(" ")}))
}

fn palette_save(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "palette.save";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "`name` is required"))?;
    let mut colors: Vec<(String, Option<String>)> = Vec::new();
    if let Some(a) = p.get("colors").and_then(Value::as_array) {
        for c in a {
            let h = c.as_str().or_else(|| c.get("hex").and_then(Value::as_str)).ok_or_else(|| bad(C, "`colors` is a list of hex strings"))?;
            let rgb = lightcraft_retouch::lab::parse_hex(h).ok_or_else(|| bad(C, format!("`{h}` is not a hex colour")))?;
            let nm = palette::name_of(rgb8_to_lab(rgb)).0.to_string();
            colors.push((hex(rgb), Some(nm)));
        }
    } else if p.get("ids").is_some() || p.get("id").is_some() || s.active().is_some() {
        let r = extract(s, &json!({"ids": ids_of(s, p, C)?.iter().map(|i| i.0).collect::<Vec<_>>(), "k": f64_or(p, "k", 6.0), "lot": true}))?;
        for c in r["palette"].as_array().into_iter().flatten() {
            colors.push((c["hex"].as_str().unwrap_or("#000000").to_string(), c["name"].as_str().map(str::to_string)));
        }
    }
    if colors.is_empty() {
        return Err(bad(C, "give `colors` (hex list) or photos (`ids`)"));
    }
    let mut list = load_store();
    list.retain(|v| v["name"].as_str() != Some(name));
    list.push(json!({"name": name, "brand": str_param(p, "brand").unwrap_or(""), "colors": colors.iter().map(|(h, n)| json!({"hex": h, "name": n})).collect::<Vec<_>>()}));
    save_store(&list).map_err(|e| bad(C, e))?;
    Ok(json!({"saved": name, "colors": colors.len(), "palettes": list.len()}))
}

fn palette_list(_: &mut Session, p: &Value) -> Result<Value> {
    let brand = str_param(p, "brand");
    let list: Vec<Value> = load_store().into_iter().filter(|v| brand.is_none_or(|b| v["brand"].as_str() == Some(b))).collect();
    Ok(json!({"palettes": list}))
}

fn palette_delete(_: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "palette.delete";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "`name` is required"))?;
    let mut list = load_store();
    let n = list.len();
    list.retain(|v| v["name"].as_str() != Some(name));
    if list.len() != n {
        save_store(&list).map_err(|e| bad(C, e))?;
    }
    Ok(json!({"removed": n - list.len()}))
}

/// The target colours a `target` parameter names, with a label.
fn targets_of(s: &mut Session, t: &Value, source: &[Swatch], c: &str) -> Result<(Vec<[f32; 3]>, String)> {
    if let Some(id) = t.get("photo").and_then(Value::as_u64) {
        let f = frame_of(s, PhotoId(id), c)?;
        let sm: Vec<[f32; 3]> = samples_of(&f, None).into_iter().map(|x| x.1).collect();
        let sw = palette::extract(&sm, 6);
        return Ok((sw.iter().map(|x| x.lab).collect(), format!("photo {id}")));
    }
    if let Some(kind) = t.get("scheme").and_then(Value::as_str) {
        // the accent of the source: the most colourful of its dominant colours
        let accent = source
            .iter()
            .filter(|x| chroma(x.lab) > 12.0)
            .max_by(|a, b| (chroma(a.lab) * a.share.sqrt()).total_cmp(&(chroma(b.lab) * b.share.sqrt())))
            .ok_or_else(|| bad(c, "the photo has no colour to build a scheme on"))?;
        let cols = palette::scheme(kind, accent.lab).ok_or_else(|| bad(c, "`scheme` must be complementary, analogous, triadic or split"))?;
        return Ok((cols, format!("{kind} scheme on {}", accent.hex)));
    }
    if let Some(a) = t.get("hexes").and_then(Value::as_array) {
        let hs: Vec<String> = a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let cols = palette::from_hexes(&hs).ok_or_else(|| bad(c, "`hexes` must be hex colours (#rrggbb)"))?;
        return Ok((cols, "hex list".into()));
    }
    if let Some(name) = t.get("palette").and_then(Value::as_str) {
        let list = load_store();
        let v = list
            .iter()
            .find(|v| v["name"].as_str() == Some(name))
            .ok_or_else(|| bad(c, format!("no saved palette named `{name}` (palette.list)")))?;
        let hs: Vec<String> = v["colors"].as_array().into_iter().flatten().filter_map(|c| c["hex"].as_str().map(str::to_string)).collect();
        let cols = palette::from_hexes(&hs).ok_or_else(|| bad(c, "the saved palette is damaged"))?;
        return Ok((cols, format!("palette {name}")));
    }
    Err(bad(c, "`target` is {photo: id} | {scheme: complementary|analogous|triadic|split} | {hexes: [..]} | {palette: name}"))
}

/// How the Colour Mixer's sliders move colours on this photo, from three probe renders.
#[derive(Clone)]
struct Probe {
    /// Degrees of Lab hue per +50 hue slider, chroma ratio per +50 saturation, L per +50 luminance.
    hue50: [f32; 8],
    sat50: [f32; 8],
    lum50: [f32; 8],
}

fn mixer_with(base: &Mixer, f: impl Fn(&mut Hsl)) -> Mixer {
    let mut m = *base;
    for h in [&mut m.red, &mut m.orange, &mut m.yellow, &mut m.green, &mut m.aqua, &mut m.blue, &mut m.purple, &mut m.magenta] {
        f(h);
    }
    m
}

fn probe(s: &mut Session, id: PhotoId, d: &DevelopSettings, before: &Frame, c: &str) -> Result<Probe> {
    let mut out = Probe { hue50: [0.0; 8], sat50: [1.0; 8], lum50: [0.0; 8] };
    let stride = (((before.w * before.h) as f32 / SAMPLES as f32).sqrt().ceil() as usize).max(1);
    for kind in 0..3 {
        let mut dd = d.clone();
        dd.mixer = mixer_with(&d.mixer, |h| match kind {
            0 => h.hue += 50.0,
            1 => h.sat += 50.0,
            _ => h.lum += 50.0,
        });
        let after = analysis::render(s, id, &dd, analysis::EDGE, c)?;
        let (mut sw, mut sv) = ([0f64; 8], [0f64; 8]);
        let mut y = stride / 2;
        while y < before.h {
            let mut x = stride / 2;
            while x < before.w {
                let i = y * before.w + x;
                let (lb, la) = (before.lab.lab_at(i), after.lab.lab_at(i));
                if chroma(lb) > 12.0 {
                    let p = before.rgba[i];
                    let w = palette::band_weights(palette::hsv([p[0], p[1], p[2]]).0);
                    let v = match kind {
                        0 => f64::from(hue_diff(hue_deg(lb), hue_deg(la))),
                        1 => f64::from(chroma(la) / chroma(lb).max(1.0)),
                        _ => f64::from(la[0] - lb[0]),
                    };
                    for b in 0..8 {
                        sw[b] += f64::from(w[b]);
                        sv[b] += f64::from(w[b]) * v;
                    }
                }
                x += stride;
            }
            y += stride;
        }
        for b in 0..8 {
            if sw[b] > 20.0 {
                let v = (sv[b] / sw[b]) as f32;
                match kind {
                    0 => out.hue50[b] = v,
                    1 => out.sat50[b] = v,
                    _ => out.lum50[b] = v,
                }
            }
        }
    }
    Ok(out)
}

/// The Mixer values a band plan needs, from the probe; a band the probe could not move is left.
fn mixer_values(plan: &[BandPlan; 8], pr: &Probe, scale: [f32; 8]) -> [Hsl; 8] {
    let mut out = [Hsl::default(); 8];
    for b in 0..8 {
        if !plan[b].active {
            continue;
        }
        let pick = |want: f32, per50: f32| if per50.abs() < 0.2 { 0.0 } else { (want / per50 * 50.0).clamp(-100.0, 100.0) };
        out[b] = Hsl {
            hue: f64::from(pick(plan[b].hue_shift, pr.hue50[b]) * scale[b]),
            sat: f64::from(if (pr.sat50[b] - 1.0).abs() < 0.02 {
                0.0
            } else {
                ((plan[b].chroma_ratio - 1.0) / (pr.sat50[b] - 1.0) * 50.0).clamp(-100.0, 100.0) * scale[b]
            }),
            lum: f64::from(pick(plan[b].dl, pr.lum50[b]) * scale[b]),
        };
    }
    out
}

fn add_mixer(base: &Mixer, add: &[Hsl; 8]) -> Mixer {
    let mut m = *base;
    let bands = [&mut m.red, &mut m.orange, &mut m.yellow, &mut m.green, &mut m.aqua, &mut m.blue, &mut m.purple, &mut m.magenta];
    for (h, a) in bands.into_iter().zip(add) {
        h.hue = (h.hue + a.hue).clamp(-100.0, 100.0);
        h.sat = (h.sat + a.sat).clamp(-100.0, 100.0);
        h.lum = (h.lum + a.lum).clamp(-100.0, 100.0);
    }
    m
}

const BAND_NAMES: [&str; 8] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];

/// What a palette move did to one photo.
pub struct PhotoResult {
    pub id: PhotoId,
    pub distance: (f32, f32),
    pub skin_de: Option<f32>,
    pub mixer: Mixer,
}

pub struct PalettePlan {
    pub label: String,
    pub target: Vec<[f32; 3]>,
    pub bands: [BandPlan; 8],
    pub values: [Hsl; 8],
    pub photos: Vec<PhotoResult>,
    pub warnings: Vec<String>,
}

/// Skin pixels of a photo (mask of the faces' skin), to protect.
fn skin_mask(ph: &PhotoSkin) -> Option<Vec<bool>> {
    let any = ph.skins.iter().any(Option::is_some);
    any.then(|| (0..ph.frame.w * ph.frame.h).map(|i| ph.found.iter().any(|f| f.cov.d[i] > 0.05)).collect())
}

/// Plan a palette harmonization over `ids` (`lot`: one parameter set for all of them).
pub fn plan(
    s: &mut Session,
    ids: &[PhotoId],
    target: &Value,
    strength: f32,
    protect_skin: bool,
    lot: bool,
    dose: f32,
    c: &str,
) -> Result<PalettePlan> {
    // look at every photo: its current frame and, when asked, its faces
    let mut looks: Vec<(PhotoId, DevelopSettings, std::sync::Arc<Frame>, Option<Vec<bool>>, Option<PhotoSkin>)> = Vec::new();
    for &id in ids {
        let d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
        let f = analysis::render(s, id, &d, analysis::EDGE, c)?;
        let (mask, ph) = if protect_skin {
            match analysis::faces(s, id, &d, &f, 4, c) {
                Ok(found) => {
                    let skins = super::skin::analyse(&f, &found);
                    let ph = PhotoSkin { id, base: d.clone(), frame: f.clone(), found, skins, faces_note: None };
                    (skin_mask(&ph), Some(ph))
                }
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };
        looks.push((id, d, f, mask, ph));
    }
    let sample_of = |k: usize| -> Vec<Sample> {
        let (_, _, f, m, _) = &looks[k];
        match m {
            Some(m) => samples_of(f, Some(&|i| m[i])),
            None => samples_of(f, None),
        }
    };
    let all: Vec<Vec<Sample>> = (0..looks.len()).map(sample_of).collect();
    let pooled: Vec<Sample> = all.iter().flatten().cloned().collect();
    let source = palette::extract(&pooled.iter().map(|x| x.1).collect::<Vec<_>>(), 6);
    let (tcols, label) = targets_of(s, target, &source, c)?;
    let strength = (strength * dose).clamp(0.0, 1.0);
    let mut warnings = Vec::new();
    let sm = |f: &Frame, m: Option<&Vec<bool>>| match m {
        Some(m) => samples_of(f, Some(&|i| m[i])),
        None => samples_of(f, None),
    };
    // phase A: the plan and the probe of every photo
    struct Prep {
        bands: [BandPlan; 8],
        pr: Probe,
        before: f32,
    }
    let mut preps: Vec<Prep> = Vec::new();
    for (k, (id, d, f, mask, _)) in looks.iter().enumerate() {
        let bands = if lot { palette::plan_bands(&pooled, &tcols, strength, 40.0) } else { palette::plan_bands(&all[k], &tcols, strength, 40.0) };
        // one parameter set for a batch: the first photo's probe serves them all
        let pr = if lot && k > 0 { preps[0].pr.clone() } else { probe(s, *id, d, f, c)? };
        preps.push(Prep { bands, pr, before: palette::distance(&sm(f, mask.as_ref()), &tcols) });
    }
    let render_with = |s: &mut Session, k: usize, values: &[Hsl; 8]| -> Result<std::sync::Arc<Frame>> {
        let (id, d, _, _, _) = &looks[k];
        let mut dd = d.clone();
        dd.mixer = add_mixer(&d.mixer, values);
        analysis::render(s, *id, &dd, analysis::EDGE, c)
    };
    // phase B: how much of the plan helps? Measured on the real renders, one scale for the whole
    // batch (the batch stays coherent); a move that does not bring the palette closer is not made
    let (mut best_scale, mut best_mean) = (0.0f32, preps.iter().map(|p| p.before).sum::<f32>() / preps.len().max(1) as f32);
    let start_mean = best_mean;
    for scale in [1.0f32, 0.7, 0.45, 0.25] {
        let mut total = 0.0;
        for k in 0..looks.len() {
            let values = mixer_values(&preps[k].bands, &preps[k].pr, [scale; 8]);
            let after = render_with(s, k, &values)?;
            total += palette::distance(&sm(&after, looks[k].3.as_ref()), &tcols);
        }
        let mean = total / looks.len().max(1) as f32;
        if mean < best_mean * 0.98 {
            (best_scale, best_mean) = (scale, mean);
        }
    }
    if best_scale == 0.0 {
        warnings.push(format!("no scale of this plan brings the palette closer (mean distance {start_mean:.1}): nothing is moved"));
    }
    let mut photos = Vec::new();
    let mut shown_plan = preps[0].bands;
    let mut shown_values = [Hsl::default(); 8];
    for (k, (id, d, f, mask, ph)) in looks.iter().enumerate() {
        let mut scale = [best_scale; 8];
        let mut values = mixer_values(&preps[k].bands, &preps[k].pr, scale);
        let mut after = render_with(s, k, &values)?;
        // the skin must not move: shrink the bands that reach it until it moves by under 2 (CIEDE2000)
        let mut skin_de = None;
        if let Some(ph) = ph {
            let mut de = skin_shift(ph, &after);
            for round in 0..4 {
                if de <= 2.0 {
                    break;
                }
                for b in [0usize, 1, 2] {
                    scale[b] *= if round < 2 { 0.5 } else { 0.0 };
                }
                values = mixer_values(&preps[k].bands, &preps[k].pr, scale);
                after = render_with(s, k, &values)?;
                de = skin_shift(ph, &after);
            }
            skin_de = Some(de);
            if de > 2.0 {
                warnings.push(format!("photo {}: the skin still moves by ΔE {de:.1}", id.0));
            }
        }
        let dist = (preps[k].before, palette::distance(&sm(&after, mask.as_ref()), &tcols));
        if k == 0 || !lot {
            shown_plan = preps[k].bands;
            shown_values = values;
        }
        photos.push(PhotoResult { id: *id, distance: dist, skin_de, mixer: add_mixer(&d.mixer, &values) });
        let _ = f;
    }
    Ok(PalettePlan { label, target: tcols, bands: shown_plan, values: shown_values, photos, warnings })
}

/// Median CIEDE2000 (colour only) between the skin of the faces before and after.
fn skin_shift(ph: &PhotoSkin, after: &Frame) -> f32 {
    let mut worst = 0.0f32;
    for sk in ph.skins.iter().flatten() {
        let m = sk.skin.restat(&after.lab).median;
        let b = sk.skin.median;
        worst = worst.max(de00([b[0], b[1], b[2]], [b[0], m[1], m[2]]));
    }
    worst
}

fn plan_json(p: &PalettePlan) -> Value {
    json!({
        "target": p.label,
        "targetColors": p.target.iter().map(|l| { let rgb = lab_to_rgb8(*l); json!({"hex": hex(rgb), "name": palette::name_of(*l).0}) }).collect::<Vec<_>>(),
        "bands": (0..8).filter(|b| p.bands[*b].active).map(|b| json!({"band": BAND_NAMES[b], "centerHue": BAND_CENTERS[b], "share": round2(p.bands[b].share), "hueShiftDeg": round1(p.bands[b].hue_shift), "chromaRatio": round2(p.bands[b].chroma_ratio),
                "mixer": {"hue": round1(p.values[b].hue as f32), "sat": round1(p.values[b].sat as f32), "lum": round1(p.values[b].lum as f32)}})).collect::<Vec<_>>(),
        "photos": p.photos.iter().map(|r| json!({"id": r.id.0, "distanceBefore": round2(r.distance.0), "distanceAfter": round2(r.distance.1), "skinDeltaE": r.skin_de.map(round2)})).collect::<Vec<_>>(),
        "warnings": p.warnings,
    })
}

fn harmonize(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "palette.harmonize";
    let ids = ids_of(s, p, C)?;
    let target = p.get("target").ok_or_else(|| bad(C, "`target` is required: {photo} | {scheme} | {hexes} | {palette}"))?;
    let strength = f64_or(p, "strength", 0.6).clamp(0.0, 1.0) as f32;
    let protect = bool_or(p, "protectSkin", true);
    let lot = match str_param(p, "mode") {
        Some("each") => false,
        Some("lot") => true,
        _ => ids.len() > 1,
    };
    let apply = bool_or(p, "apply", true);
    let plan = plan(s, &ids, target, strength, protect, lot, 1.0, C)?;
    let mut ops = Vec::new();
    if apply {
        for r in &plan.photos {
            let mut d = (*s.develop_of(r.id).ok_or_else(|| bad(C, "no such photo"))?).clone();
            d.mixer = r.mixer;
            ops.extend(s.develop_op(r.id, d, "Harmonize Palette"));
        }
        if !ops.is_empty() {
            s.commit("Harmonize Palette", lightcraft_catalog::Op::Batch { ops })?;
        }
    }
    let mut v = plan_json(&plan);
    v["mode"] = json!(if lot { "lot" } else { "each" });
    v["applied"] = json!(apply);
    Ok(v)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "palette.extract",
            "Extract Palette",
            [],
            None,
            "{ids?, k?: 1..12 (6, usually 5 to 8), lot?: true (one palette for all the ids), each?: true} — the dominant colours of a photo (or of a batch pooled): k-means in Lab, close colours merged → [{hex, lab, share, name, nameFr}] and `copy` (the hex list, to paste). Names are approximate (nearest of ~40 reference colours). Changes nothing",
            always,
            extract
        ),
        cmd!(
            "palette.harmonize",
            "Harmonize Palette",
            [],
            None,
            "{ids?, target: {photo: id} | {scheme: complementary|analogous|triadic|split} | {hexes: [\"#rrggbb\", …]} | {palette: saved name}, strength?: 0..1 (0.6), protectSkin?: true (faces' skin is held within ΔE 2, bands shrunk if needed), mode?: lot|each (lot with several photos: ONE set of mixer values from the whole batch, so the batch stays coherent), apply?: bool (true; false = preview figures only)} — move the Colour Mixer (hue / saturation / luminance of the 8 bands, sized from probe renders of the real pipeline) toward the target palette, on top of the current mixer; one undo step → {target, targetColors, bands: [{band, hueShiftDeg, mixer}], photos: [{id, distanceBefore, distanceAfter, skinDeltaE}], mode, applied}. The distance is the mean CIEDE2000 of the photo's colours to the nearest target colour, lightness left out",
            always,
            harmonize
        ),
        cmd!(
            "palette.save",
            "Save Palette",
            [],
            None,
            "{name, brand?, colors?: [hex…] | ids?+k? (extracted from photos)} — keep a named palette per brand or universe (in LightCraft's config folder, `palettes.json`) → {saved, colors, palettes}",
            always,
            palette_save
        ),
        cmd!(query "palette.list", "List Palettes", [], None, "{brand?} → {palettes: [{name, brand, colors: [{hex, name}]}]}", always, palette_list),
        cmd!("palette.delete", "Delete Palette", [], None, "{name} → {removed}", always, palette_delete),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-palette-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d.join("palettes.json")
    }

    #[test]
    fn palettes_are_extracted_saved_listed_and_deleted() {
        set_store(Some(temp_file("store")));
        let mut s = Session::with_demo();
        let r = s.execute("palette.extract", &json!({"k": 5})).expect("extract");
        let sw = r["photos"][0]["palette"].as_array().expect("palette");
        assert!(!sw.is_empty() && sw.len() <= 5, "{r}");
        assert!(sw.iter().all(|c| c["hex"].as_str().is_some_and(|h| h.len() == 7) && c["name"].is_string()));
        let total: f64 = sw.iter().filter_map(|c| c["share"].as_f64()).sum();
        assert!((total - 1.0).abs() < 0.08, "shares add up to about 1: {total}");
        assert!(r["copy"].as_str().is_some_and(|c| c.starts_with('#')));
        s.execute("palette.save", &json!({"name": "Kerma", "brand": "Kerma", "colors": ["#c4572b", "#1f7a78"]})).expect("save");
        s.execute("palette.save", &json!({"name": "From photo", "k": 4})).expect("save from the active photo");
        let l = s.execute("palette.list", &json!({})).expect("list");
        assert_eq!(l["palettes"].as_array().map(Vec::len), Some(2), "{l}");
        let k = s.execute("palette.list", &json!({"brand": "Kerma"})).expect("list");
        assert_eq!(k["palettes"][0]["colors"][0]["hex"], "#c4572b");
        assert!(s.execute("palette.save", &json!({"name": "x", "colors": ["nope"]})).is_err());
        assert_eq!(s.execute("palette.delete", &json!({"name": "Kerma"})).expect("delete")["removed"], 1);
        assert_eq!(s.execute("palette.list", &json!({})).expect("list")["palettes"].as_array().map(Vec::len), Some(1));
        set_store(None);
    }

    #[test]
    fn a_batch_gets_one_mixer_and_a_preview_writes_nothing() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|i| i.0).collect();
        assert!(ids.len() == 3, "the demo library has photos");
        let target = json!({"hexes": ["#1f7a78", "#e8a317", "#c4572b"]});
        let undo = s.undo.len();
        let before: Vec<_> = ids.iter().map(|i| s.develop_of(lightcraft_catalog::PhotoId(*i)).expect("develop").mixer).collect();
        let prev = s.execute("palette.harmonize", &json!({"ids": ids, "target": target, "apply": false})).expect("preview");
        assert_eq!(prev["mode"], "lot");
        assert!(
            prev["photos"].as_array().is_some_and(|a| a.iter().all(|p| p["distanceBefore"].is_number() && p["distanceAfter"].is_number())),
            "{prev}"
        );
        assert_eq!(s.undo.len(), undo, "a preview writes nothing");
        assert!(ids.iter().zip(&before).all(|(i, m)| s.develop_of(lightcraft_catalog::PhotoId(*i)).expect("develop").mixer == *m));
        s.execute("palette.harmonize", &json!({"ids": ids, "target": target, "apply": true})).expect("apply");
        assert_eq!(s.undo.len(), undo + 1, "one undo step for the batch");
        let mixers: Vec<_> = ids.iter().map(|i| s.develop_of(lightcraft_catalog::PhotoId(*i)).expect("develop").mixer).collect();
        assert!(
            mixers.windows(2).all(|w| w[0] == w[1]) || before.windows(2).any(|w| w[0] != w[1]),
            "the same plan for every photo of the batch (on top of their own mixers)"
        );
        assert!(mixers != before, "the mixer moved");
        s.execute("edit.undo", &json!({})).expect("undo");
        assert!(ids.iter().zip(&before).all(|(i, m)| s.develop_of(lightcraft_catalog::PhotoId(*i)).expect("develop").mixer == *m));
    }
}
