//! The proposed retouch (LightCraft v10): skin harmonization, guided dodge & burn, palettes and
//! the "natural skin" recipe that proposes them in order. Docs: `docs/skin-harmonize.md`,
//! `docs/dodge-burn.md`, `docs/palette.md`.
//!
//! Rules this module keeps (tests enforce them): it only writes ordinary, editable settings —
//! masks named "Retouch: …" with local adjustments, the Colour Mixer, Color Grading — in one
//! undo step; it never touches pixels, geometry, heal spots or the crop; it generates nothing and
//! sends nothing anywhere; nothing happens until a command is run (proposing writes nothing).

mod analysis;
mod dnb;
mod outmask;
mod palette;
mod recipe;
mod skin;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::LocalAdjustments;
use serde_json::{Value, json};

use crate::cmd::CommandSpec;
use crate::{Result, Session};

/// Name prefix of every mask this module makes.
pub const PREFIX: &str = "Retouch: ";

/// Scale every slider of a local adjustment (all of them are centred on 0).
pub fn scale_adjust(a: &LocalAdjustments, k: f64) -> LocalAdjustments {
    LocalAdjustments {
        temp: a.temp * k,
        tint: a.tint * k,
        exposure: a.exposure * k,
        contrast: a.contrast * k,
        highlights: a.highlights * k,
        shadows: a.shadows * k,
        whites: a.whites * k,
        blacks: a.blacks * k,
        texture: a.texture * k,
        clarity: a.clarity * k,
        dehaze: a.dehaze * k,
        hue: a.hue * k,
        saturation: a.saturation * k,
        sharpness: a.sharpness * k,
        noise: a.noise * k,
        moire: a.moire * k,
        defringe: a.defringe * k,
        // Port 0.21: upstream's Lens Blur inside a mask (0..100) joins the scaled sliders; its
        // switches are copied as they are (a scale must not turn a blur on or off).
        lens_blur: a.lens_blur * k,
        lens_blur_off: a.lens_blur_off,
        lens_blur_outside: a.lens_blur_outside,
        lens_blur_boost: a.lens_blur_boost * k,
        lens_blur_transition: a.lens_blur_transition * k,
        color_hue: a.color_hue,
        color_sat: a.color_sat * k,
        amount: a.amount,
    }
}

/// Remove the masks whose name starts with `prefix` from `ids`, one undo step → `{removed}`.
pub(crate) fn clear_masks(s: &mut Session, ids: &[PhotoId], prefix: &str, label: &str) -> Result<Value> {
    let (mut ops, mut removed) = (Vec::new(), 0usize);
    for &id in ids {
        let Some(d) = s.develop_of(id) else { continue };
        let n = d.masks.iter().filter(|m| m.name.starts_with(prefix)).count();
        if n == 0 {
            continue;
        }
        let mut d = (*d).clone();
        d.masks.retain(|m| !m.name.starts_with(prefix));
        removed += n;
        ops.extend(s.develop_op(id, d, label));
    }
    if !ops.is_empty() {
        s.commit(label, lightcraft_catalog::Op::Batch { ops })?;
    }
    Ok(json!({"removed": removed}))
}

pub fn specs() -> Vec<CommandSpec> {
    let mut v = Vec::new();
    v.extend(skin::specs());
    v.extend(dnb::specs());
    v.extend(palette::specs());
    v.extend(recipe::specs());
    v
}
