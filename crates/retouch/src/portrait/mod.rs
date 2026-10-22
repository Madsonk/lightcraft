//! Portrait retouch by named sliders, on pixels: the pure maths of LightCraft's portrait stack.
//!
//! Every slider is a [`Kind`] with an amount 0..1. [`retouch_face`] takes an sRGB crop around a
//! face and its 478-point mesh and returns the crop with the steps applied, in order. Nothing
//! here reads a model, a file or the network; the engine finds the faces and stores the result
//! as an inpaint patch, so the original is never touched.
//!
//! All sizes are fractions of the face width, so a preview at 1/8 of the resolution and the
//! full-resolution render do the same thing. The work is done on a copy of the crop whose face is
//! at most [`WORK_FACE`] pixels wide; what the steps changed (a difference of Lab planes) is
//! scaled back up and added to the full-resolution crop, so pores and hairs the steps did not
//! touch keep their full-resolution detail.

pub mod background;
pub mod eyes;
pub mod fixture;
pub mod ops;
pub mod raster;
#[cfg(test)]
mod tests;

use crate::facegeo::Face;
use crate::lab::{lab_to_rgb8, rgb8_to_lab};
use crate::plane::Plane;
use crate::skin::LabImage;

/// Widest face (pixels) the steps are computed at.
pub const WORK_FACE: f32 = 720.0;

/// A portrait slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Acne, pimples and other small isolated marks: removed.
    Blemishes,
    /// Freckles and small dark spots: faded, not erased.
    Freckles,
    /// Smooth skin that keeps pores (frequency separation).
    Smooth,
    WrinklesEyes,
    WrinklesForehead,
    WrinklesLines,
    DarkCircles,
    /// Oily shine on the skin.
    Shine,
    DoubleChin,
    Teeth,
    /// Eye whites and irises.
    Eyes,
    Flyaways,
    NeckSmooth,
    NeckLight,
}

/// Static description of a slider.
pub struct Info {
    pub kind: Kind,
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub help: &'static str,
    /// Amount used by "auto retouch" (0 = not part of it).
    pub auto: f32,
}

pub const KINDS: &[Info] = &[
    Info {
        kind: Kind::Smooth,
        id: "smooth",
        label: "Skin smoothing",
        group: "Skin",
        help: "evens skin tone and fades small texture, keeps pores, eyes and edges",
        auto: 0.35,
    },
    Info { kind: Kind::Blemishes, id: "blemishes", label: "Blemishes", group: "Skin", help: "removes acne, pimples and isolated marks", auto: 0.55 },
    Info { kind: Kind::Freckles, id: "freckles", label: "Freckles", group: "Skin", help: "fades freckles and small dark spots", auto: 0.0 },
    Info {
        kind: Kind::WrinklesEyes,
        id: "wrinkles_eyes",
        label: "Eye wrinkles",
        group: "Wrinkles",
        help: "softens crow's feet and fine lines around the eyes",
        auto: 0.2,
    },
    Info {
        kind: Kind::WrinklesForehead,
        id: "wrinkles_forehead",
        label: "Forehead wrinkles",
        group: "Wrinkles",
        help: "softens forehead lines",
        auto: 0.2,
    },
    Info {
        kind: Kind::WrinklesLines,
        id: "wrinkles_lines",
        label: "Smile lines",
        group: "Wrinkles",
        help: "softens the lines from nose to mouth",
        auto: 0.15,
    },
    Info {
        kind: Kind::DarkCircles,
        id: "dark_circles",
        label: "Dark circles",
        group: "Face",
        help: "lightens and neutralizes the skin under the eyes",
        auto: 0.4,
    },
    Info {
        kind: Kind::Shine,
        id: "shine",
        label: "Face shine",
        group: "Face",
        help: "tones down oily highlights and gives their colour back",
        auto: 0.25,
    },
    Info {
        kind: Kind::DoubleChin,
        id: "double_chin",
        label: "Double chin",
        group: "Face",
        help: "lifts the shadow under the chin (no reshaping)",
        auto: 0.0,
    },
    Info {
        kind: Kind::Teeth,
        id: "teeth",
        label: "Teeth whitening",
        group: "Mouth and eyes",
        help: "whitens teeth of an open mouth, never the lips or tongue",
        auto: 0.3,
    },
    Info {
        kind: Kind::Eyes,
        id: "eyes",
        label: "Eye brightness",
        group: "Mouth and eyes",
        help: "brightens the whites, removes redness, adds contrast to the irises",
        auto: 0.3,
    },
    Info {
        kind: Kind::Flyaways,
        id: "flyaways",
        label: "Stray hairs",
        group: "Hair",
        help: "removes thin hairs that cross a smooth background",
        auto: 0.0,
    },
    Info { kind: Kind::NeckSmooth, id: "neck_smooth", label: "Neck smoothing", group: "Neck", help: "smooths the skin of the neck", auto: 0.25 },
    Info {
        kind: Kind::NeckLight,
        id: "neck_light",
        label: "Neck brightness",
        group: "Neck",
        help: "brings the neck's brightness closer to the face's",
        auto: 0.0,
    },
];

pub fn info_of(id: &str) -> Option<&'static Info> {
    KINDS.iter().find(|i| i.id == id)
}

/// What one step did to one face.
#[derive(Clone, Debug, PartialEq)]
pub struct StepReport {
    pub kind: Kind,
    /// Share (0..1) of the face's pixels whose colour moved by a visible amount (ΔE > 2).
    pub changed: f32,
    /// Things the step found and treated (blemish count…); `None` when not applicable.
    pub found: Option<usize>,
    /// Why nothing was done, when that is the case.
    pub skipped: Option<&'static str>,
}

pub struct FaceResult {
    pub reports: Vec<StepReport>,
}

/// An 8-bit sRGB crop, interleaved.
pub struct Crop<'a> {
    pub w: usize,
    pub h: usize,
    pub rgb: &'a mut [u8],
}

/// Apply `steps` (kind, amount 0..1) to `crop` for a face whose mesh `points` are in the crop's
/// pixel coordinates. Pixels whose Lab colour moved by more than `0.7` are rewritten.
pub fn retouch_face(crop: Crop, points: &[[f32; 3]], steps: &[(Kind, f32)]) -> Option<FaceResult> {
    let (w, h) = (crop.w, crop.h);
    if w < 8 || h < 8 || crop.rgb.len() < w * h * 3 {
        return None;
    }
    let face = Face::new(points)?;
    // working copy: the face at most WORK_FACE wide
    let scale = (WORK_FACE / face.width).min(1.0);
    let (ww, wh) = (((w as f32 * scale).round() as usize).max(8), ((h as f32 * scale).round() as usize).max(8));
    let full = lab_of(crop.rgb, w, h);
    let (sx, sy) = (ww as f32 / w as f32, wh as f32 / h as f32);
    let small = if (ww, wh) == (w, h) {
        LabImage { w, h, l: full.l.clone(), a: full.a.clone(), b: full.b.clone() }
    } else {
        LabImage { w: ww, h: wh, l: raster::resize(&full.l, ww, wh), a: raster::resize(&full.a, ww, wh), b: raster::resize(&full.b, ww, wh) }
    };
    let pts: Vec<[f32; 3]> = points.iter().map(|p| [p[0] * sx, p[1] * sy, p[2] * sx]).collect();
    let small_face = Face::new(&pts)?;
    let mut work = ops::Work::new(small, small_face)?;
    let reports: Vec<StepReport> = steps.iter().filter(|(_, a)| *a > 0.0).map(|(k, a)| work.apply(*k, a.clamp(0.0, 1.0))).collect();
    // the change, scaled back up
    let (dl, da, db) = work.delta();
    let (dl, da, db) =
        if (ww, wh) == (w, h) { (dl, da, db) } else { (raster::resize(&dl, w, h), raster::resize(&da, w, h), raster::resize(&db, w, h)) };
    for i in 0..w * h {
        let (l, a, b) = (dl.d[i], da.d[i], db.d[i]);
        if l.abs() + a.abs() + b.abs() < 0.7 {
            continue;
        }
        let v = [(full.l.d[i] + l).clamp(0.0, 100.0), full.a.d[i] + a, full.b.d[i] + b];
        let px = lab_to_rgb8(v);
        crop.rgb[i * 3..i * 3 + 3].copy_from_slice(&px);
    }
    Some(FaceResult { reports })
}

/// Lab planes of interleaved 8-bit sRGB.
pub fn lab_of(rgb: &[u8], w: usize, h: usize) -> LabImage {
    let n = w * h;
    let (mut l, mut a, mut b) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for px in rgb.as_chunks::<3>().0.iter().take(n) {
        let v = rgb8_to_lab([px[0], px[1], px[2]]);
        l.push(v[0]);
        a.push(v[1]);
        b.push(v[2]);
    }
    LabImage { w, h, l: Plane::from_vec(w, h, l), a: Plane::from_vec(w, h, a), b: Plane::from_vec(w, h, b) }
}
