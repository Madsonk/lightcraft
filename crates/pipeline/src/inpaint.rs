//! Inpainted spots: the pixels the inpainting model made (stored apart from the settings, see
//! `lightcraft_develop::InpaintPatch`) composited into the scene-linear image. Nothing here
//! needs the model; a spot whose pixels are not attached is skipped.

use lightcraft_color::transfer::{linear_to_srgb, srgb_to_linear};
use lightcraft_color::{RgbSpace, SRGB, WORKING};
use lightcraft_develop::{Aux, InpaintPatch, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;

fn matrix(from: &RgbSpace, to: &RgbSpace) -> [[f32; 3]; 3] {
    from.to_space(to).to_f32()
}

fn mul(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2])
}

/// 8-bit sRGB (interleaved) of a working-space linear image: what the inpainting model sees.
pub fn to_srgb8(img: &Rgb32f) -> Vec<u8> {
    let m = matrix(&WORKING, &SRGB);
    img.data.iter().flat_map(|c| mul(&m, *c).map(|v| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8)).collect()
}

/// Composite the patches of `spots` (inpaint mode) into `img` (output resolution, working space).
pub fn apply(img: &mut Rgb32f, spots: &[Spot], aux: &Aux, frame: &Frame) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let (to_norm, to_out) = (frame.out_to_norm(w, h), frame.norm_to_out(w, h));
    let m = matrix(&SRGB, &WORKING);
    let lut: Vec<f32> = (0..256).map(|v| srgb_to_linear(v as f32 / 255.0)).collect();
    for spot in spots.iter().filter(|s| s.mode == SpotMode::Inpaint) {
        let Some(p) = spot.patch.as_deref().and_then(|id| aux.patch(id)) else { continue };
        let opacity = (spot.opacity / 100.0).clamp(0.0, 1.0) as f32;
        // the part of the output the patch's rectangle can touch
        let corners = [(p.rect[0], p.rect[1]), (p.rect[2], p.rect[1]), (p.rect[0], p.rect[3]), (p.rect[2], p.rect[3])]
            .map(|(x, y)| to_out.apply(Point::new(x, y)));
        let (x0, x1) = (corners.iter().map(|c| c.x).fold(f64::MAX, f64::min), corners.iter().map(|c| c.x).fold(f64::MIN, f64::max));
        let (y0, y1) = (corners.iter().map(|c| c.y).fold(f64::MAX, f64::min), corners.iter().map(|c| c.y).fold(f64::MIN, f64::max));
        let (bx0, by0) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
        let (bx1, by1) = ((x1.ceil().max(0.0) as usize).min(w), (y1.ceil().max(0.0) as usize).min(h));
        for y in by0..by1 {
            for x in bx0..bx1 {
                let n = to_norm.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
                let (u, v) = ((n.x - p.rect[0]) / (p.rect[2] - p.rect[0]), (n.y - p.rect[1]) / (p.rect[3] - p.rect[1]));
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let (a, rgb) = sample(p, u as f32, v as f32);
                let a = a * opacity;
                if a <= 0.0 {
                    continue;
                }
                let lin = mul(&m, rgb.map(|b| lut[b as usize]));
                let px = &mut img.data[y * w + x];
                for c in 0..3 {
                    px[c] += (lin[c] - px[c]) * a;
                }
            }
        }
    }
}

/// Coverage (0..1) and bilinearly interpolated colour (8-bit values as floats) of the patch at
/// `(u, v)` in 0..1. Colour is interpolated in 8-bit sRGB (sufficient for a smooth fill).
fn sample(p: &InpaintPatch, u: f32, v: f32) -> (f32, [u8; 3]) {
    let (w, h) = (p.width, p.height);
    let fx = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let wt = [(x0, y0, (1.0 - tx) * (1.0 - ty)), (x1, y0, tx * (1.0 - ty)), (x0, y1, (1.0 - tx) * ty), (x1, y1, tx * ty)];
    let mut a = 0.0;
    let mut c = [0f32; 3];
    for (x, y, k) in wt {
        let i = y * w + x;
        a += f32::from(p.alpha[i]) * k;
        for ch in 0..3 {
            c[ch] += f32::from(p.rgb[i * 3 + ch]) * k;
        }
    }
    (a / 255.0, c.map(|v| v.round().clamp(0.0, 255.0) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::DevelopSettings;
    use std::sync::Arc;

    #[test]
    fn a_patch_replaces_its_area_and_only_that() {
        let (w, h) = (40, 40);
        let mut img = Rgb32f::filled(w, h, [0.2, 0.2, 0.2]);
        // a 16 × 16 red patch over the middle quarter, fully opaque in its centre
        let (pw, ph) = (16, 16);
        let rgb: Vec<u8> = (0..pw * ph).flat_map(|_| [255u8, 0, 0]).collect();
        let alpha: Vec<u8> = (0..pw * ph).map(|i| if (4..12).contains(&(i % pw)) && (4..12).contains(&(i / pw)) { 255 } else { 0 }).collect();
        let patch = Arc::new(InpaintPatch::new([0.25, 0.25, 0.75, 0.75], pw, ph, rgb, alpha).unwrap());
        let spot = Spot { mode: SpotMode::Inpaint, points: vec![Point::new(0.5, 0.5)], patch: Some(patch.hex_id()), ..Default::default() };
        let s = DevelopSettings { spots: vec![spot], aux: Aux { patches: vec![patch], ..Default::default() }, ..Default::default() };
        let frame = crate::geometry::Frame::new(w, h, &s, true);
        apply(&mut img, &s.spots, &s.aux, &frame);
        assert!(img.data[20 * w + 20][0] > 0.5 && img.data[20 * w + 20][1] < 0.1, "centre is filled: {:?}", img.data[20 * w + 20]);
        assert_eq!(img.data[2 * w + 2], [0.2, 0.2, 0.2]);
        assert_eq!(img.data[20 * w + 11], [0.2, 0.2, 0.2], "transparent part of the patch");
        // a spot whose pixels are not attached does nothing
        let mut none = Rgb32f::filled(w, h, [0.2, 0.2, 0.2]);
        apply(&mut none, &s.spots, &Aux::default(), &frame);
        assert_eq!(none.data[20 * w + 20], [0.2, 0.2, 0.2]);
    }
}
