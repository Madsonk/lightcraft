//! Automatic masks from the optional SegFormer / matting models (see [`crate::autoseg`] and
//! `docs/autoseg.md`): `autoseg.model.*` and `mask.auto`.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, f64_or, has_active, str_param};
use crate::{Result, Session};

/// Long edge of the developed copy the networks look at.
pub const EDGE: usize = 1536;

#[cfg(feature = "autoseg")]
fn model_of(p: &Value, c: &str) -> Result<crate::autoseg::Model> {
    str_param(p, "model")
        .and_then(crate::autoseg::Model::parse)
        .ok_or_else(|| bad(c, "`model` must be \"segformer\" (sky, people, landscape) or \"subject\" (RMBG-1.4 matting)"))
}

/// The class group or `subject`/`background` a `mask.auto` kind names.
#[cfg(feature = "autoseg")]
fn kind_of(kind: &str) -> Option<(crate::autoseg::Kind, bool)> {
    use crate::autoseg::Kind;
    Some(match kind {
        "subject" => (Kind::Subject, false),
        "background" => (Kind::Subject, true),
        "people" | "person" => (Kind::Class("people".into()), false),
        "sky" | "vegetation" | "water" | "mountain" | "buildings" | "ground" => (Kind::Class(kind.into()), false),
        _ => return None,
    })
}

/// The job that renders the developed photo, uncropped and without its masks, as the networks see
/// it: detached from the session, so it can run on a worker.
pub(crate) fn developed_job(s: &mut Session, id: PhotoId, c: &str) -> Result<crate::media::RenderJob> {
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
    d.masks.clear();
    d.lens_blur.enabled = false;
    s.preview_job(id, EDGE, EDGE, false, &d).ok_or_else(|| bad(c, "no such photo"))
}

/// The developed photo, uncropped and without its masks, as the networks see it.
pub(crate) fn developed(s: &mut Session, id: PhotoId, c: &str) -> Result<lightcraft_raster::Rgba8> {
    let job = developed_job(s, id, c)?;
    Ok(job.run().rendered.map_err(|e| bad(c, e))?.image)
}

#[cfg(feature = "autoseg")]
fn auto(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "mask.auto";
    let kind = str_param(p, "kind").unwrap_or("sky");
    let (k, invert) =
        kind_of(kind).ok_or_else(|| bad(C, "`kind` must be sky, people, subject, background, vegetation, water, mountain, buildings or ground"))?;
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() || p.get("id").is_some() { s.targets(p).unwrap_or_default() } else { s.active().into_iter().collect() };
    if ids.is_empty() {
        return Err(bad(C, "no active photo"));
    }
    let refine = f64_or(p, "refine", 30.0).clamp(0.0, 100.0);
    let (mut made, mut secs) = (Vec::new(), 0.0);
    for id in ids {
        let img = developed(s, id, C)?;
        let (seg, t) = crate::autoseg::segment(&img, &k).map_err(crate::EngineError::Other)?;
        secs += t;
        let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
        let next = d.next_mask_id();
        let text = if kind == "background" { "subject" } else { kind };
        d.masks.push(Mask {
            id: next,
            name: str_param(p, "name").map_or_else(|| format!("{} {next}", capitalise(kind)), str::to_string),
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert,
                shape: MaskShape::Prompt { text: text.to_string(), seg: Some(seg), detail: Vec::new(), edge: 0.0 },
            }],
            refine,
            ..Default::default()
        });
        s.set_develop(id, d, "Automatic Mask")?;
        if s.active() == Some(id) {
            s.active_mask = Some(next);
        }
        made.push(json!({"id": id.0, "mask": next}));
    }
    Ok(json!({"kind": kind, "masks": made, "seconds": (secs * 100.0).round() / 100.0, "refine": refine, "inverted": invert}))
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "autoseg.model.status",
            "Automatic Mask Model Status",
            [],
            None,
            "{} — whether this build has automatic masks (sky, people, landscape, subject) and, for each model, whether it is installed, its licence and download progress → {available, dir, models: [{id: segformer|subject, title, files, sizeBytes, installed, licence, licenceUrl, sourceUrl, download: {running, done, total, finished, error}}]}",
            always,
            |_, _| {
                #[cfg(feature = "autoseg")]
                let models = crate::autoseg::models();
                #[cfg(not(feature = "autoseg"))]
                let models: Vec<Value> = Vec::new();
                Ok(json!({"available": crate::autoseg::AVAILABLE, "dir": crate::autoseg::models_dir().map(|d| d.display().to_string()), "models": models}))
            }
        ),
        cmd!(
            query "autoseg.model.download",
            "Download Automatic Mask Model",
            [],
            None,
            "{model: segformer|subject, acknowledged: true} — download a model in the background (segformer: SegFormer-B2 ADE20K, 110 MB, NVIDIA licence, research/non-commercial; subject: RMBG-1.4, 176 MB, BRIA licence, non-commercial), checked by size and SHA-256; only after the user agreed to its licence (see autoseg.model.status). Watch autoseg.model.status; autoseg.model.cancel stops it → {started, installed}",
            always,
            |_, p| {
                let c = "autoseg.model.download";
                #[cfg(feature = "autoseg")]
                {
                    let m = model_of(p, c)?;
                    let spec = m.spec();
                    if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                        let mb = spec.files.iter().filter_map(|f| f.size).sum::<u64>() as f64 / 1e6;
                        return Err(bad(c, format!("the {} model is a {mb:.0} MB download under its own licence ({}; {}): pass `acknowledged: true` once the user has agreed", spec.title, spec.licence, spec.licence_url)));
                    }
                    let started = crate::autoseg::start_download(m).map_err(crate::EngineError::Other)?;
                    Ok(json!({"started": started, "installed": crate::autoseg::status(m)["installed"]}))
                }
                #[cfg(not(feature = "autoseg"))]
                {
                    let _ = p;
                    Err(bad(c, "automatic masks are not available in this build"))
                }
            }
        ),
        cmd!(
            query "autoseg.model.cancel",
            "Cancel Automatic Mask Model Download",
            [],
            None,
            "{model: segformer|subject} — stop a model download (what has arrived is kept and resumed by a new download) → {cancelled}",
            always,
            |_, p| {
                #[cfg(feature = "autoseg")]
                {
                    let m = model_of(p, "autoseg.model.cancel")?;
                    Ok(json!({"cancelled": crate::autoseg::cancel_download(m)}))
                }
                #[cfg(not(feature = "autoseg"))]
                {
                    let _ = p;
                    Ok(json!({"cancelled": false}))
                }
            }
        ),
        cmd!(
            "mask.auto",
            "Automatic Mask (Sky, People, Subject…)",
            [],
            None,
            "{kind: sky|people|subject|background|vegetation|water|mountain|buildings|ground, ids? (default: the active photo), name?, refine?: 0..100 (30, Refine Edges)} — a new mask whose selection a network computed (sky, people and landscape classes: SegFormer; subject / background: RMBG-1.4 matting) and stored with the mask, so it renders without the model. Needs the model (autoseg.model.status / autoseg.model.download); one undo step per photo → {kind, masks: [{id, mask}], seconds}",
            has_active,
            |s, p| {
                #[cfg(feature = "autoseg")]
                return auto(s, p);
                #[cfg(not(feature = "autoseg"))]
                {
                    let _ = (s, p);
                    Err(bad("mask.auto", "automatic masks are not available in this build"))
                }
            }
        ),
    ]
}
