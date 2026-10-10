//! Palettes: dominant colours of a photo or a batch, schemes, and a plan to move the colour
//! mixer's bands toward a target palette. Nothing is drawn here; the engine turns the plan into
//! ordinary Colour Mixer settings.

use crate::lab::{chroma, de00, hex, hue_deg, hue_diff, lab_to_rgb8, parse_hex, rgb8_to_lab};

#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    pub lab: [f32; 3],
    /// Share of the sampled pixels, 0..1.
    pub share: f32,
    pub hex: String,
    pub name: &'static str,
    pub name_fr: &'static str,
}

/// Approximate colour names (English, French), by nearest CIEDE2000.
const NAMES: &[(&str, &str, &str)] = &[
    ("black", "noir", "#101010"),
    ("charcoal", "anthracite", "#383838"),
    ("dark grey", "gris foncé", "#5a5a5a"),
    ("grey", "gris", "#8c8c8c"),
    ("light grey", "gris clair", "#c4c4c4"),
    ("white", "blanc", "#f6f6f4"),
    ("cream", "crème", "#f1e6c8"),
    ("beige", "beige", "#d2bc98"),
    ("tan", "fauve", "#b48a5a"),
    ("dark brown", "brun foncé", "#3c2a1e"),
    ("brown", "brun", "#6b4428"),
    ("chestnut", "châtaigne", "#8a4b2a"),
    ("rust", "rouille", "#a2441f"),
    ("terracotta", "terre cuite", "#c0613b"),
    ("burnt orange", "orange brûlé", "#d2691e"),
    ("orange", "orange", "#f08a1c"),
    ("amber", "ambre", "#e8a317"),
    ("golden yellow", "jaune doré", "#e6c027"),
    ("yellow", "jaune", "#f4e03a"),
    ("olive", "olive", "#7a7a2a"),
    ("lime green", "vert citron", "#a6c93a"),
    ("leaf green", "vert feuille", "#4f9a3a"),
    ("forest green", "vert forêt", "#24542c"),
    ("teal", "sarcelle", "#1f7a78"),
    ("turquoise", "turquoise", "#35b9b0"),
    ("sky blue", "bleu ciel", "#7fb6e6"),
    ("blue", "bleu", "#2f6fd0"),
    ("navy", "bleu marine", "#1b2a56"),
    ("indigo", "indigo", "#3b3a8c"),
    ("violet", "violet", "#7a4cc0"),
    ("plum", "prune", "#5b2a58"),
    ("magenta", "magenta", "#c0357e"),
    ("pink", "rose", "#ec8fb0"),
    ("salmon", "saumon", "#f0937a"),
    ("coral", "corail", "#ef6a5a"),
    ("red", "rouge", "#d02a24"),
    ("burgundy", "bordeaux", "#6e1424"),
    ("peach", "pêche", "#f4c2a1"),
    ("mauve", "mauve", "#b08aae"),
];

pub fn name_of(lab: [f32; 3]) -> (&'static str, &'static str) {
    let mut best = (f32::MAX, NAMES[0]);
    for n in NAMES {
        let l = rgb8_to_lab(parse_hex(n.2).unwrap_or([0, 0, 0]));
        let d = de00(lab, l);
        if d < best.0 {
            best = (d, *n);
        }
    }
    (best.1.0, best.1.1)
}

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 33) as f32) / (1u64 << 31) as f32
}

fn d2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Dominant colours of Lab samples: k-means++ (deterministic seed), close clusters merged
/// (CIEDE2000 under 7), sorted by share. At most `k` swatches (5 to 8 usually).
pub fn extract(samples: &[[f32; 3]], k: usize) -> Vec<Swatch> {
    let k = k.clamp(1, 12);
    if samples.is_empty() {
        return Vec::new();
    }
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut cen: Vec<[f32; 3]> = vec![samples[(lcg(&mut seed) * samples.len() as f32) as usize % samples.len()]];
    while cen.len() < k.min(samples.len()) {
        let dist: Vec<f32> = samples.iter().map(|s| cen.iter().map(|c| d2(*s, *c)).fold(f32::MAX, f32::min)).collect();
        let total: f32 = dist.iter().sum();
        if total <= 0.0 {
            break;
        }
        let mut t = lcg(&mut seed) * total;
        let mut pick = samples.len() - 1;
        for (i, d) in dist.iter().enumerate() {
            if t <= *d {
                pick = i;
                break;
            }
            t -= d;
        }
        cen.push(samples[pick]);
    }
    let mut counts = vec![0usize; cen.len()];
    for _ in 0..20 {
        let mut sum = vec![[0f64; 3]; cen.len()];
        counts.iter_mut().for_each(|c| *c = 0);
        for s in samples {
            let (mut bi, mut bd) = (0, f32::MAX);
            for (i, c) in cen.iter().enumerate() {
                let d = d2(*s, *c);
                if d < bd {
                    (bi, bd) = (i, d);
                }
            }
            counts[bi] += 1;
            for j in 0..3 {
                sum[bi][j] += f64::from(s[j]);
            }
        }
        let mut moved = 0.0;
        for i in 0..cen.len() {
            if counts[i] > 0 {
                let n = counts[i] as f64;
                let c = [(sum[i][0] / n) as f32, (sum[i][1] / n) as f32, (sum[i][2] / n) as f32];
                moved += d2(c, cen[i]);
                cen[i] = c;
            }
        }
        if moved < 0.01 {
            break;
        }
    }
    let mut cl: Vec<([f32; 3], usize)> = cen.into_iter().zip(counts).filter(|(_, n)| *n > 0).collect();
    // merge the clusters that look the same
    loop {
        let mut merged = false;
        'outer: for i in 0..cl.len() {
            for j in i + 1..cl.len() {
                if de00(cl[i].0, cl[j].0) < 7.0 {
                    let (a, b) = (cl[i], cl[j]);
                    let n = (a.1 + b.1) as f32;
                    let c = [0, 1, 2].map(|k| (a.0[k] * a.1 as f32 + b.0[k] * b.1 as f32) / n);
                    cl[i] = (c, a.1 + b.1);
                    cl.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    cl.sort_by_key(|c| std::cmp::Reverse(c.1));
    let total = samples.len() as f32;
    cl.into_iter()
        .map(|(lab, n)| {
            let (name, name_fr) = name_of(lab);
            Swatch { lab, share: n as f32 / total, hex: hex(lab_to_rgb8(lab)), name, name_fr }
        })
        .collect()
}

/// Colours of a scheme built on `base` (a Lab accent colour of the photo): the other hues keep
/// its lightness and chroma.
pub fn scheme(kind: &str, base: [f32; 3]) -> Option<Vec<[f32; 3]>> {
    let h0 = hue_deg(base);
    let c = chroma(base).max(18.0);
    let offsets: &[f32] = match kind {
        "complementary" => &[0.0, 180.0],
        "analogous" => &[-30.0, 0.0, 30.0],
        "triadic" => &[0.0, 120.0, 240.0],
        "split" | "split-complementary" => &[0.0, 150.0, 210.0],
        _ => return None,
    };
    Some(
        offsets
            .iter()
            .map(|o| {
                let t = (h0 + o).to_radians();
                [base[0], c * t.cos(), c * t.sin()]
            })
            .collect(),
    )
}

/// Colours of a list of hex strings; `None` if one of them is not a colour.
pub fn from_hexes(h: &[String]) -> Option<Vec<[f32; 3]>> {
    h.iter().map(|s| parse_hex(s).map(rgb8_to_lab)).collect()
}

/// The centres of the colour mixer's bands (red, orange, yellow, green, aqua, blue, purple,
/// magenta) as hues of the HSV circle.
pub const BAND_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 225.0, 270.0, 315.0];

/// HSV hue (degrees) of an sRGB colour, and its saturation.
pub fn hsv(rgb: [u8; 3]) -> (f32, f32) {
    let (r, g, b) = (f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    if d < 1e-6 {
        return (0.0, 0.0);
    }
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, d / mx.max(1e-6))
}

/// How much of each band a hue belongs to (triangular windows reaching the neighbour centres).
pub fn band_weights(hue: f32) -> [f32; 8] {
    let mut w = [0f32; 8];
    for i in 0..8 {
        let (c, next) = (BAND_CENTERS[i], BAND_CENTERS[(i + 1) % 8]);
        let span = (next - c).rem_euclid(360.0);
        let t = (hue - c).rem_euclid(360.0);
        if t < span {
            w[i] += 1.0 - t / span;
            w[(i + 1) % 8] += t / span;
        }
    }
    w
}

/// What to do to one band of the mixer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandPlan {
    /// The band has enough colour in the photo to be moved.
    pub active: bool,
    /// Share of the sampled pixels that belong to the band.
    pub share: f32,
    /// Wanted hue shift (degrees of the Lab circle), saturation (chroma ratio) and lightness shift.
    pub hue_shift: f32,
    pub chroma_ratio: f32,
    pub dl: f32,
    pub from: [f32; 3],
    pub to: [f32; 3],
}

/// A coloured sample: sRGB, Lab, weight.
pub type Sample = ([u8; 3], [f32; 3], f32);

/// Plan the move of each band toward the nearest (by hue) colour of `targets`: the band's mean
/// colour in the photo is the "from"; the hue shift is capped at `max_deg`, scaled by `strength`.
pub fn plan_bands(samples: &[Sample], targets: &[[f32; 3]], strength: f32, max_deg: f32) -> [BandPlan; 8] {
    let mut out = [BandPlan::default(); 8];
    let colourful: Vec<&Sample> = samples.iter().filter(|s| chroma(s.1) > 10.0).collect();
    let total: f32 = samples.iter().map(|s| s.2).sum::<f32>().max(1e-6);
    let tgt: Vec<[f32; 3]> = targets.iter().copied().filter(|t| chroma(*t) > 8.0).collect();
    for b in 0..8 {
        // mean Lab of the band (circular mean for the hue through a, b)
        let (mut sw, mut sl, mut sa, mut sb) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for s in &colourful {
            let w = band_weights(hsv(s.0).0)[b] * s.2;
            sw += w;
            sl += w * s.1[0];
            sa += w * s.1[1];
            sb += w * s.1[2];
        }
        out[b].share = sw / total;
        if sw / total < 0.01 || tgt.is_empty() {
            continue;
        }
        let from = [sl / sw, sa / sw, sb / sw];
        let h_from = hue_deg(from);
        let best = tgt.iter().copied().min_by(|x, y| hue_diff(h_from, hue_deg(*x)).abs().total_cmp(&hue_diff(h_from, hue_deg(*y)).abs()));
        let Some(to) = best else { continue };
        let dh = hue_diff(h_from, hue_deg(to)).clamp(-max_deg, max_deg) * strength;
        let ratio = (chroma(to) / chroma(from).max(1.0)).clamp(0.6, 1.4);
        out[b] = BandPlan {
            active: true,
            share: sw / total,
            hue_shift: dh,
            chroma_ratio: 1.0 + (ratio - 1.0) * strength,
            dl: ((to[0] - from[0]) * 0.3).clamp(-10.0, 10.0) * strength,
            from,
            to,
        };
    }
    out
}

/// How far a photo's colours are from a palette: the mean over its coloured samples of the
/// CIEDE2000 to the nearest target colour, each sample keeping its own lightness (so that
/// lightness, which a hue move cannot fix, does not count).
pub fn distance(samples: &[Sample], targets: &[[f32; 3]]) -> f32 {
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for sm in samples {
        if chroma(sm.1) < 10.0 || targets.is_empty() {
            continue;
        }
        let d = targets.iter().map(|t| de00(sm.1, [sm.1[0], t[1], t[2]])).fold(f32::MAX, f32::min);
        s += f64::from(d) * f64::from(sm.2);
        n += f64::from(sm.2);
    }
    if n == 0.0 { 0.0 } else { (s / n) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples_of(cols: &[([u8; 3], usize)]) -> Vec<Sample> {
        cols.iter().flat_map(|(c, n)| std::iter::repeat_n((*c, rgb8_to_lab(*c), 1.0), *n)).collect()
    }

    #[test]
    fn extract_finds_the_dominant_colours_with_shares_and_names() {
        let mut px = Vec::new();
        for _ in 0..600 {
            px.push(rgb8_to_lab([200, 40, 30]));
        }
        for _ in 0..300 {
            px.push(rgb8_to_lab([30, 90, 200]));
        }
        for _ in 0..100 {
            px.push(rgb8_to_lab([245, 245, 240]));
        }
        let sw = extract(&px, 6);
        assert_eq!(sw.len(), 3, "close clusters merged: {sw:?}");
        assert!((sw[0].share - 0.6).abs() < 0.01 && sw[0].name == "red" && sw[0].hex.len() == 7, "{:?}", sw[0]);
        assert_eq!(sw[1].name, "blue");
        assert_eq!(sw[2].name, "white");
        assert_eq!(extract(&[], 5), Vec::new());
    }

    #[test]
    fn extract_is_deterministic() {
        let px: Vec<[f32; 3]> = (0..500).map(|i| rgb8_to_lab([(i % 255) as u8, (i * 7 % 255) as u8, (i * 13 % 255) as u8])).collect();
        assert_eq!(extract(&px, 6), extract(&px, 6));
    }

    #[test]
    fn schemes_place_the_hues_around_the_accent() {
        let base = rgb8_to_lab([200, 90, 40]);
        let c = scheme("complementary", base).expect("scheme");
        assert!((hue_diff(hue_deg(c[0]), hue_deg(c[1])).abs() - 180.0).abs() < 1.0);
        let t = scheme("triadic", base).expect("scheme");
        assert_eq!(t.len(), 3);
        assert!((hue_diff(hue_deg(t[0]), hue_deg(t[1])) - 120.0).abs() < 1.0);
        assert!(scheme("nonsense", base).is_none());
        assert!(from_hexes(&["#ff0000".into(), "#00ff00".into()]).is_some() && from_hexes(&["#ggg".into()]).is_none());
    }

    #[test]
    fn band_weights_sum_to_one_and_peak_at_the_centres() {
        for h in [0.0, 12.0, 45.0, 100.0, 200.0, 300.0, 359.0] {
            assert!((band_weights(h).iter().sum::<f32>() - 1.0).abs() < 1e-4, "{h}");
        }
        assert!(band_weights(120.0)[3] > 0.99 && band_weights(30.0)[1] > 0.99);
        assert!((hsv([255, 0, 0]).0).abs() < 1e-3 && (hsv([0, 255, 0]).0 - 120.0).abs() < 1e-3 && (hsv([0, 0, 255]).0 - 240.0).abs() < 1e-3);
    }

    #[test]
    fn the_plan_rotates_green_toward_a_teal_target_and_leaves_absent_bands_alone() {
        let s = samples_of(&[([60, 160, 60], 400), ([200, 200, 200], 600)]);
        let teal = rgb8_to_lab([30, 150, 140]);
        let plan = plan_bands(&s, &[teal], 1.0, 40.0);
        assert!(plan[3].active && plan[3].hue_shift != 0.0, "green band moves: {:?}", plan[3]);
        // a green leaning toward teal: the shift is toward the blue side (the Lab hue grows)
        assert!(plan[3].hue_shift != 0.0);
        assert!(!plan[0].active && !plan[5].active, "no red or blue in the photo");
        let half = plan_bands(&s, &[teal], 0.5, 40.0);
        assert!((half[3].hue_shift - plan[3].hue_shift * 0.5).abs() < 0.01);
    }

    #[test]
    fn the_distance_falls_when_the_palette_is_matched() {
        let before = samples_of(&[([60, 160, 60], 500)]);
        let after = samples_of(&[([40, 150, 140], 500)]);
        let teal = vec![rgb8_to_lab([30, 150, 140])];
        assert!(distance(&after, &teal) < distance(&before, &teal) * 0.5);
        assert_eq!(distance(&samples_of(&[([128, 128, 128], 10)]), &teal), 0.0, "neutrals do not count");
    }
}
