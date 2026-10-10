//! sRGB <-> CIE Lab (D65) and the CIEDE2000 difference.

use std::sync::OnceLock;

fn lin_table() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

const WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];

fn f_lab(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D * D * D { t.cbrt() } else { t / (3.0 * D * D) + 4.0 / 29.0 }
}

fn f_inv(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D { t * t * t } else { 3.0 * D * D * (t - 4.0 / 29.0) }
}

/// Lab of an 8-bit sRGB pixel (L 0..100).
pub fn rgb8_to_lab(p: [u8; 3]) -> [f32; 3] {
    let t = lin_table();
    let (r, g, b) = (t[p[0] as usize], t[p[1] as usize], t[p[2] as usize]);
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let (fx, fy, fz) = (f_lab(x / WHITE[0]), f_lab(y / WHITE[1]), f_lab(z / WHITE[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn enc(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The 8-bit sRGB colour of a Lab value (clipped to the gamut).
pub fn lab_to_rgb8(l: [f32; 3]) -> [u8; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let (fx, fz) = (fy + l[1] / 500.0, fy - l[2] / 200.0);
    let (x, y, z) = (f_inv(fx) * WHITE[0], f_inv(fy) * WHITE[1], f_inv(fz) * WHITE[2]);
    let r = 3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z;
    let g = -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z;
    let b = 0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z;
    [enc(r), enc(g), enc(b)]
}

pub fn hex(p: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn chroma(l: [f32; 3]) -> f32 {
    l[1].hypot(l[2])
}

/// Lab hue angle in degrees, 0..360 (0 = +a, red-magenta; 90 = +b, yellow).
pub fn hue_deg(l: [f32; 3]) -> f32 {
    let h = l[2].atan2(l[1]).to_degrees();
    if h < 0.0 { h + 360.0 } else { h }
}

/// Signed circular difference `b - a` in degrees, -180..180.
pub fn hue_diff(a: f32, b: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

/// CIEDE2000 (Sharma, Wu, Dalal 2005), kL = kC = kH = 1.
pub fn de00(x: [f32; 3], y: [f32; 3]) -> f32 {
    let (l1, a1, b1) = (f64::from(x[0]), f64::from(x[1]), f64::from(x[2]));
    let (l2, a2, b2) = (f64::from(y[0]), f64::from(y[1]), f64::from(y[2]));
    let (c1, c2) = (a1.hypot(b1), a2.hypot(b2));
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (ap1, ap2) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (cp1, cp2) = (ap1.hypot(b1), ap2.hypot(b2));
    let hp = |b: f64, ap: f64| {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let (h1, h2) = (hp(b1, ap1), hp(b2, ap2));
    let dl = l2 - l1;
    let dc = cp2 - cp1;
    let dh_deg = if cp1 * cp2 == 0.0 {
        0.0
    } else {
        let d = h2 - h1;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dh = 2.0 * (cp1 * cp2).sqrt() * (dh_deg.to_radians() / 2.0).sin();
    let lm = (l1 + l2) / 2.0;
    let cpm = (cp1 + cp2) / 2.0;
    let hm = if cp1 * cp2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos() + 0.24 * (2.0 * hm).to_radians().cos() + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cpm;
    let sh = 1.0 + 0.015 * cpm * t;
    let dt = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cpm.powi(7) / (cpm.powi(7) + 25f64.powi(7))).sqrt();
    let rt = -(2.0 * dt).to_radians().sin() * rc;
    let (a, b, c) = (dl / sl, dc / sc, dh / sh);
    (a * a + b * b + c * c + rt * b * c).max(0.0).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_of_white_black_and_a_mid_grey() {
        let w = rgb8_to_lab([255, 255, 255]);
        assert!((w[0] - 100.0).abs() < 0.05 && w[1].abs() < 0.05 && w[2].abs() < 0.05, "{w:?}");
        let k = rgb8_to_lab([0, 0, 0]);
        assert!(k[0].abs() < 0.01, "{k:?}");
        let g = rgb8_to_lab([119, 119, 119]);
        assert!((g[0] - 50.0).abs() < 0.6, "{g:?}");
    }

    #[test]
    fn lab_round_trips_through_srgb() {
        for p in [[200u8, 120, 90], [30, 20, 15], [90, 150, 210], [255, 0, 0]] {
            let back = lab_to_rgb8(rgb8_to_lab(p));
            assert!(p.iter().zip(back).all(|(a, b)| (i32::from(*a) - i32::from(b)).abs() <= 1), "{p:?} {back:?}");
        }
    }

    #[test]
    fn ciede2000_matches_the_published_pairs() {
        // Sharma et al. 2005, table 1 (pairs 1, 2, 7, 17)
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
            ([50.0, -1.0, 2.0], [50.0, 0.0, 0.0], 2.3669),
            ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        ];
        for (a, b, want) in cases {
            let got = de00(a, b);
            assert!((got - want).abs() < 0.002, "{a:?} {b:?}: {got} vs {want}");
        }
        assert_eq!(de00([50.0, 10.0, 10.0], [50.0, 10.0, 10.0]), 0.0);
    }

    #[test]
    fn hex_round_trips_and_hue_wraps() {
        assert_eq!(hex([255, 0, 128]), "#ff0080");
        assert_eq!(parse_hex("#FF0080"), Some([255, 0, 128]));
        assert_eq!(parse_hex("zzz"), None);
        assert!((hue_diff(350.0, 10.0) - 20.0).abs() < 1e-4 && (hue_diff(10.0, 350.0) + 20.0).abs() < 1e-4);
    }
}
