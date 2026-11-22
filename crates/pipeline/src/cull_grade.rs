//! Retouch score of a photo: how much work its colour and exposure need. Classical statistics on
//! the 512 px measuring copy, like the rest of `cull` — no learned model.

use lightcraft_raster::Rgb32f;

/// What a photo's colour says about its need for a retouch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    /// How far the near-neutral pixels lean off grey, 0 (neutral) .. 1: the white balance error.
    pub cast: f32,
    /// Which way they lean: `[0]` red − blue (positive = warm), `[1]` green − mean of red and blue
    /// (positive = green, negative = magenta), both relative to the grey level.
    pub lean: [f32; 2],
    /// Mean chroma of the mid-tones, 0 (grey) .. 1.
    pub saturation: f32,
}

/// Colour cast and saturation of `img`: the cast is read on its least coloured pixels (what should
/// be grey), the saturation on everything that is neither crushed nor blown.
pub fn look(img: &Rgb32f) -> Look {
    let step = (img.data.len() / 4096).max(1);
    let (mut grey, mut g_n, mut sat, mut s_n) = ([0f64; 3], 0f64, 0f64, 0f64);
    for c in img.data.iter().step_by(step) {
        let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
        if !(0.02..=0.9).contains(&mx) {
            continue;
        }
        let chroma = (mx - mn) / mx;
        sat += chroma as f64;
        s_n += 1.0;
        if chroma < 0.25 {
            for (a, v) in grey.iter_mut().zip(c) {
                *a += *v as f64;
            }
            g_n += 1.0;
        }
    }
    let saturation = if s_n > 0.0 { (sat / s_n) as f32 } else { 0.0 };
    if g_n < 8.0 {
        return Look { saturation, ..Default::default() };
    }
    let m = ((grey[0] + grey[1] + grey[2]) / 3.0).max(1e-6);
    let lean = [((grey[0] - grey[2]) / m) as f32, ((grey[1] - (grey[0] + grey[2]) / 2.0) / m) as f32];
    Look { cast: (lean[0].hypot(lean[1]) * 2.0).min(1.0), lean, saturation }
}

/// Retouch score 0..100 (100 = nothing to fix) and the ways it falls short, worst first. Every
/// penalty is a plain distance from a comfortable range, so the score reads as "how much work".
/// `exposure` is `cull::exposure`'s `[mean, p5, p95]`, `clipped` the share of clipped pixels.
pub fn grade(look: &Look, exposure: [f32; 3], clipped: f32) -> (f32, Vec<&'static str>) {
    let [mean, p5, p95] = exposure;
    let cast = if look.lean[0].abs() >= look.lean[1].abs() {
        if look.lean[0] > 0.0 { "too warm" } else { "too cool" }
    } else if look.lean[1] > 0.0 {
        "green tint"
    } else {
        "magenta tint"
    };
    // warmth is often the intent (sun, skin, tungsten): half weight; on 250 of the maker's photos 52 % read "too warm" at full weight
    let weight = if cast == "too warm" { 20.0 } else { 40.0 };
    let mut parts: Vec<(f32, &'static str)> = vec![
        (look.cast * weight, cast),
        ((0.12 - look.saturation).max(0.0) * 200.0, "flat colour"),
        ((look.saturation - 0.45).max(0.0) * 150.0, "oversaturated"),
        ((0.30 - mean).max(0.0) * 150.0, "too dark"),
        ((mean - 0.65).max(0.0) * 150.0, "too bright"),
        ((p5 - 0.12).max(0.0) * 150.0, "no deep blacks"),
        ((0.80 - p95).max(0.0) * 100.0, "no bright whites"),
        (clipped * 100.0, "clipped"),
    ];
    parts.retain(|p| p.0 >= 3.0);
    parts.sort_by(|a, b| b.0.total_cmp(&a.0));
    let loss: f32 = parts.iter().map(|p| p.0).sum();
    ((100.0 - loss).clamp(0.0, 100.0), parts.into_iter().map(|p| p.1).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cull::{clipped, exposure};

    #[test]
    fn a_warm_dark_photo_scores_lower_and_says_why() {
        let mut ok = Rgb32f::filled(32, 32, [0.2, 0.2, 0.2]);
        for (i, c) in ok.data.iter_mut().enumerate() {
            if i % 7 == 0 {
                *c = [0.5, 0.2, 0.1];
            }
        }
        let (good, why) = grade(&look(&ok), exposure(&ok), clipped(&ok));
        let warm = Rgb32f::filled(32, 32, [0.03, 0.027, 0.024]);
        let (bad, reasons) = grade(&look(&warm), exposure(&warm), clipped(&warm));
        assert!(good > bad + 15.0, "{good} {why:?} vs {bad} {reasons:?}");
        assert!(reasons.contains(&"too warm") && reasons.contains(&"too dark"), "{reasons:?}");
        assert!(look(&Rgb32f::filled(32, 32, [0.3, 0.3, 0.3])).cast < 0.01);
    }
}
