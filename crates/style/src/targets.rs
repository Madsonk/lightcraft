//! The develop sliders the model predicts, and how they map to and from [`DevelopSettings`].
//!
//! White balance is learned as a *shift from the as-shot white* (mired and tint), because a
//! Kelvin value means nothing without the camera's own white: the same warm look is 5200 K on one
//! raw and 6500 K on another. Everything else is the slider value itself.

use lightcraft_develop::{DevelopSettings, WbMode};
use lightcraft_pipeline::SourceInfo;
use lightcraft_pipeline::auto::current_wb;

pub struct TargetSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    /// Smaller than this is "no adjustment" (used to decide what is worth touching).
    pub dead_zone: f32,
}

pub const TARGETS: [TargetSpec; 14] = [
    TargetSpec { id: "exposure", label: "Exposition", min: -5.0, max: 5.0, dead_zone: 0.03 },
    TargetSpec { id: "temp", label: "Température (mired)", min: -120.0, max: 120.0, dead_zone: 1.0 },
    TargetSpec { id: "tint", label: "Teinte", min: -60.0, max: 60.0, dead_zone: 1.0 },
    TargetSpec { id: "contrast", label: "Contraste", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "highlights", label: "Hautes lumières", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "shadows", label: "Ombres", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "whites", label: "Blancs", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "blacks", label: "Noirs", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "vibrance", label: "Vibrance", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "saturation", label: "Saturation", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "curve_highlights", label: "Courbe : hautes lumières", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "curve_lights", label: "Courbe : tons clairs", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "curve_darks", label: "Courbe : tons sombres", min: -100.0, max: 100.0, dead_zone: 1.0 },
    TargetSpec { id: "curve_shadows", label: "Courbe : ombres", min: -100.0, max: 100.0, dead_zone: 1.0 },
];

pub const N_TARGETS: usize = TARGETS.len();

/// One value per target, in [`TARGETS`] order.
pub type Targets = [f32; N_TARGETS];

pub const EXPOSURE: usize = 0;

fn mired(kelvin: f64) -> f64 {
    1.0e6 / kelvin.clamp(1500.0, 60000.0)
}

/// The sliders a photo's edit amounts to, relative to the as-shot state of `info`.
pub fn from_settings(s: &DevelopSettings, info: &SourceInfo) -> Targets {
    let base = DevelopSettings::default();
    let (t0, tint0) = current_wb(info, &base);
    let (t, tint) = current_wb(info, s);
    let c = &s.curve;
    [
        s.light.exposure as f32,
        (mired(t) - mired(t0)) as f32,
        (tint - tint0) as f32,
        s.light.contrast as f32,
        s.light.highlights as f32,
        s.light.shadows as f32,
        s.light.whites as f32,
        s.light.blacks as f32,
        s.color.vibrance as f32,
        s.color.saturation as f32,
        c.highlights as f32,
        c.lights as f32,
        c.darks as f32,
        c.shadows as f32,
    ]
}

pub fn clamp(t: &mut Targets) {
    for (v, spec) in t.iter_mut().zip(TARGETS.iter()) {
        *v = if v.is_finite() { v.clamp(spec.min, spec.max) } else { 0.0 };
    }
}

/// Write the predicted sliders over `s` (everything else in `s` is kept). Values inside a
/// target's dead zone leave the slider alone, so a tiny prediction does not turn a photo "edited".
pub fn apply(s: &mut DevelopSettings, t: &Targets, info: &SourceInfo) {
    let mut t = *t;
    clamp(&mut t);
    let on = |i: usize| TARGETS.get(i).is_some_and(|sp| t.get(i).copied().unwrap_or(0.0).abs() >= sp.dead_zone);
    let v = |i: usize| t.get(i).copied().unwrap_or(0.0) as f64;
    if on(0) {
        s.light.exposure = (v(0) * 100.0).round() / 100.0;
    }
    if on(1) || on(2) {
        let base = DevelopSettings::default();
        let (t0, tint0) = current_wb(info, &base);
        let m = (mired(t0) + v(1)).clamp(1.0e6 / 50000.0, 1.0e6 / 2000.0);
        s.wb.temp = (1.0e6 / m).round();
        s.wb.tint = (tint0 + v(2)).round().clamp(-150.0, 150.0);
        s.wb.mode = WbMode::Custom;
    }
    let set = |slot: &mut f64, i: usize| {
        if on(i) {
            *slot = v(i).round();
        }
    };
    set(&mut s.light.contrast, 3);
    set(&mut s.light.highlights, 4);
    set(&mut s.light.shadows, 5);
    set(&mut s.light.whites, 6);
    set(&mut s.light.blacks, 7);
    set(&mut s.color.vibrance, 8);
    set(&mut s.color.saturation, 9);
    set(&mut s.curve.highlights, 10);
    set(&mut s.curve.lights, 11);
    set(&mut s.curve.darks, 12);
    set(&mut s.curve.shadows, 13);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_settings() {
        let info = SourceInfo::default();
        let mut s = DevelopSettings::default();
        s.light.exposure = 0.7;
        s.light.contrast = 12.0;
        s.light.shadows = 30.0;
        s.color.vibrance = 15.0;
        s.curve.lights = 8.0;
        s.wb.temp = 5600.0;
        s.wb.tint = 4.0;
        s.wb.mode = WbMode::Custom;
        let t = from_settings(&s, &info);
        assert!(t[1] > 10.0, "warmer light means a positive mired shift: {t:?}");
        let mut out = DevelopSettings::default();
        apply(&mut out, &t, &info);
        assert!((out.light.exposure - 0.7).abs() < 0.01);
        assert_eq!(out.light.contrast, 12.0);
        assert_eq!(out.light.shadows, 30.0);
        assert!((out.wb.temp - 5600.0).abs() < 40.0, "{}", out.wb.temp);
        assert_eq!(out.wb.tint, 4.0);
    }

    #[test]
    fn tiny_values_leave_the_photo_unedited_and_nan_is_harmless() {
        let info = SourceInfo::default();
        let mut s = DevelopSettings::default();
        let mut t = [0.0; N_TARGETS];
        t[0] = 0.01;
        t[3] = 0.4;
        t[4] = f32::NAN;
        apply(&mut s, &t, &info);
        assert_eq!(s, DevelopSettings::default());
    }
}
