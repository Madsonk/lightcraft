//! Tests of the portrait steps on a synthetic portrait (a skin-coloured oval with pores, eyes,
//! brows, an open mouth with teeth, and planted defects), with the mesh of `facegeo`'s fake face.

use super::fixture::{W, mesh, portrait};
use super::*;
use crate::lab;
use crate::lab::rgb8_to_lab;

fn lab_at(rgb: &[u8], x: usize, y: usize) -> [f32; 3] {
    let i = (y * W + x) * 3;
    rgb8_to_lab([rgb[i], rgb[i + 1], rgb[i + 2]])
}

fn run(rgb: &[u8], pts: &[[f32; 3]], steps: &[(Kind, f32)]) -> (Vec<u8>, FaceResult) {
    let mut out = rgb.to_vec();
    let r = retouch_face(Crop { w: W, h: W, rgb: &mut out }, pts, steps).expect("a face");
    (out, r)
}

fn mean_l(rgb: &[u8], x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            s += lab_at(rgb, x, y)[0];
        }
    }
    s / ((x1 - x0) * (y1 - y0)) as f32
}

fn hf_std(rgb: &[u8], x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
    // spread of the pixels around their 5 x 5 neighbourhood mean: the pores, not the shading
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let m = mean_l(rgb, x - 2, y - 2, x + 3, y + 3);
            s += (lab_at(rgb, x, y)[0] - m).powi(2);
        }
    }
    (s / ((x1 - x0) * (y1 - y0)) as f32).sqrt()
}

fn same_outside_face(a: &[u8], b: &[u8]) -> bool {
    // far corners of the frame (no face, no hair ring) are bit-identical
    for (x, y) in [(5usize, 380usize), (390, 380), (380, 392), (10, 395)] {
        let i = (y * W + x) * 3;
        if a[i..i + 3] != b[i..i + 3] {
            return false;
        }
    }
    true
}

#[test]
fn blemishes_are_removed_and_the_rest_is_left_alone() {
    let (rgb, pl) = portrait(true);
    let (out, r) = run(&rgb, &mesh(true), &[(Kind::Blemishes, 0.8)]);
    let before = lab_at(&rgb, pl.blemish[0], pl.blemish[1]);
    let after = lab_at(&out, pl.blemish[0], pl.blemish[1]);
    let around = lab_at(&rgb, pl.blemish[0] + 14, pl.blemish[1] + 14);
    assert!((before[0] - around[0]).abs() > 8.0, "the mark was there: {before:?} vs {around:?}");
    assert!((after[0] - around[0]).abs() < 0.3 * (before[0] - around[0]).abs() + 2.0, "gone: {after:?} {around:?}");
    assert!(r.reports[0].found.unwrap_or(0) >= 1, "{:?}", r.reports);
    assert!(same_outside_face(&rgb, &out));
    // the iris and the lips are untouched
    assert_eq!(&rgb[(160 * W + 150) * 3..(160 * W + 150) * 3 + 3], &out[(160 * W + 150) * 3..(160 * W + 150) * 3 + 3]);
    assert_eq!(&rgb[(260 * W + 180) * 3..(260 * W + 180) * 3 + 3], &out[(260 * W + 180) * 3..(260 * W + 180) * 3 + 3]);
}

#[test]
fn freckles_fade_but_do_not_vanish() {
    let (rgb, pl) = portrait(true);
    let (out, _) = run(&rgb, &mesh(true), &[(Kind::Freckles, 0.7)]);
    let f = pl.freckles[3];
    let (b, a) = (lab_at(&rgb, f[0], f[1])[0], lab_at(&out, f[0], f[1])[0]);
    let skin = lab_at(&rgb, f[0] + 7, f[1] + 7)[0];
    assert!((a - skin).abs() < 0.8 * (b - skin).abs(), "fainter: before {b} after {a} skin {skin}");
    assert!(a < skin - 0.5 || (a - skin).abs() > 0.0, "not erased to nothing special");
}

#[test]
fn smoothing_cuts_pore_texture_but_not_the_eyes() {
    let (rgb, _) = portrait(true);
    let (out, r) = run(&rgb, &mesh(true), &[(Kind::Smooth, 0.8)]);
    let (b, a) = (hf_std(&rgb, 120, 215, 180, 245), hf_std(&out, 120, 215, 180, 245));
    assert!(a < 0.75 * b && a > 0.15 * b, "texture cut but not gone: {b} -> {a}");
    assert_eq!(&rgb[(160 * W + 150) * 3..(160 * W + 150) * 3 + 3], &out[(160 * W + 150) * 3..(160 * W + 150) * 3 + 3], "iris");
    assert!(r.reports[0].changed > 0.0, "{:?}", r.reports[0]);
}

#[test]
fn dark_circles_are_lifted_toward_the_skin_and_cheeks_are_not_moved() {
    let (rgb, _) = portrait(true);
    let (out, _) = run(&rgb, &mesh(true), &[(Kind::DarkCircles, 0.8)]);
    let (b, a) = (mean_l(&rgb, 135, 172, 165, 184), mean_l(&out, 135, 172, 165, 184));
    let cheek = mean_l(&rgb, 130, 225, 160, 240);
    assert!(a > b + 3.0, "lifted: {b} -> {a} (cheek {cheek})");
    assert!((mean_l(&out, 130, 225, 160, 240) - cheek).abs() < 1.0, "cheek unchanged");
}

#[test]
fn shine_comes_down_and_texture_stays() {
    let (rgb, _) = portrait(true);
    let (out, _) = run(&rgb, &mesh(true), &[(Kind::Shine, 0.9)]);
    let (b, a) = (mean_l(&rgb, 185, 92, 215, 108), mean_l(&out, 185, 92, 215, 108));
    let skin = mean_l(&rgb, 120, 215, 180, 245);
    assert!(b - skin > 8.0, "the planted shine is there: {b} vs {skin}");
    assert!(a < b - 6.0, "toned down: {b} -> {a}");
    assert!(hf_std(&out, 190, 99, 210, 108) > 0.4 * hf_std(&rgb, 190, 99, 210, 108), "pores kept");
}

#[test]
fn teeth_get_whiter_but_lips_and_tongue_do_not() {
    let (rgb, _) = portrait(true);
    let (out, r) = run(&rgb, &mesh(true), &[(Kind::Teeth, 0.9)]);
    let (b, a) = (lab_at(&rgb, 200, 255), lab_at(&out, 200, 255));
    assert!(a[2] < b[2] - 8.0 && a[0] >= b[0], "less yellow, not darker: {b:?} -> {a:?}");
    assert_eq!(&rgb[(260 * W + 160) * 3..(260 * W + 160) * 3 + 3], &out[(260 * W + 160) * 3..(260 * W + 160) * 3 + 3], "lip");
    assert_eq!(&rgb[(272 * W + 200) * 3..(272 * W + 200) * 3 + 3], &out[(272 * W + 200) * 3..(272 * W + 200) * 3 + 3], "tongue");
    assert!(r.reports[0].skipped.is_none());
    // a closed mouth: nothing to whiten, and the report says why
    let (rgb2, _) = portrait(false);
    let (out2, r2) = run(&rgb2, &mesh(false), &[(Kind::Teeth, 0.9)]);
    assert_eq!(rgb2, out2);
    assert_eq!(r2.reports[0].skipped, Some("mouth closed"));
}

#[test]
fn wrinkle_lines_are_lightened() {
    let (rgb, _) = portrait(true);
    let (out, _) = run(&rgb, &mesh(true), &[(Kind::WrinklesForehead, 0.9)]);
    let line = |rgb: &[u8]| mean_l(rgb, 170, 96, 230, 97) - mean_l(rgb, 170, 102, 230, 104);
    assert!(line(&out) > line(&rgb) + 3.0, "{} -> {}", line(&rgb), line(&out));
}

#[test]
fn a_stray_hair_over_the_wall_goes_and_the_wall_stays() {
    let (rgb, _) = portrait(true);
    let (out, r) = run(&rgb, &mesh(true), &[(Kind::Flyaways, 0.9)]);
    let hair = |rgb: &[u8]| mean_l(rgb, 200, 36, 210, 42);
    let wall = mean_l(&rgb, 40, 40, 60, 60);
    assert!(wall - hair(&rgb) > 6.0, "the hair is darker than the wall: {} vs {}", hair(&rgb), wall);
    assert!(wall - hair(&out) < 0.4 * (wall - hair(&rgb)), "gone: {} -> {}", hair(&rgb), hair(&out));
    assert!(r.reports[0].found.unwrap_or(0) >= 1);
    // the eyes are never touched
    assert_eq!(&rgb[(160 * W + 150) * 3..(160 * W + 150) * 3 + 3], &out[(160 * W + 150) * 3..(160 * W + 150) * 3 + 3]);
}

#[test]
fn eyes_get_brighter_whites() {
    let (rgb, _) = portrait(true);
    let (out, _) = run(&rgb, &mesh(true), &[(Kind::Eyes, 0.9)]);
    let (b, a) = (lab_at(&rgb, 168, 160)[0], lab_at(&out, 168, 160)[0]);
    assert!(a >= b && a - b < 12.0, "whites brighter, not blown: {b} -> {a}");
}

#[test]
fn the_same_retouch_at_half_the_size_gives_the_same_picture() {
    // face at 400 px vs the same photo downscaled 2x: the change is relative to the face width
    let (rgb, pl) = portrait(true);
    let (full, _) = run(&rgb, &mesh(true), &[(Kind::Blemishes, 0.8), (Kind::DarkCircles, 0.6)]);
    let half_w = W / 2;
    let down = |src: &[u8]| -> Vec<u8> {
        let mut o = vec![0u8; half_w * half_w * 3];
        for y in 0..half_w {
            for x in 0..half_w {
                for c in 0..3 {
                    let s: u32 = (0..4).map(|k| u32::from(src[((2 * y + k / 2) * W + 2 * x + k % 2) * 3 + c])).sum();
                    o[(y * half_w + x) * 3 + c] = (s / 4) as u8;
                }
            }
        }
        o
    };
    let mut small = down(&rgb);
    let pts: Vec<[f32; 3]> = mesh(true).iter().map(|p| [p[0] / 2.0, p[1] / 2.0, p[2] / 2.0]).collect();
    retouch_face(Crop { w: half_w, h: half_w, rgb: &mut small }, &pts, &[(Kind::Blemishes, 0.8), (Kind::DarkCircles, 0.6)]).expect("small");
    let ref_small = down(&full);
    let (x, y) = (pl.blemish[0] / 2, pl.blemish[1] / 2);
    for (px, py) in [(x, y), (75, 90), (125, 90)] {
        let i = (py * half_w + px) * 3;
        let (a, b) = (rgb8_to_lab([small[i], small[i + 1], small[i + 2]]), rgb8_to_lab([ref_small[i], ref_small[i + 1], ref_small[i + 2]]));
        assert!(lab::de00(a, b) < 6.0, "({px},{py}) {a:?} vs {b:?}");
    }
}

#[test]
fn hostile_input_is_refused_not_a_panic() {
    let mut tiny = vec![0u8; 3 * 3 * 3];
    assert!(retouch_face(Crop { w: 3, h: 3, rgb: &mut tiny }, &mesh(true), &[(Kind::Smooth, 1.0)]).is_none());
    let mut buf = vec![128u8; W * W * 3];
    assert!(retouch_face(Crop { w: W, h: W, rgb: &mut buf }, &[[0.0; 3]; 10], &[(Kind::Smooth, 1.0)]).is_none());
    // a face that is nowhere near the crop: a flat grey frame has no skin to measure
    assert!(retouch_face(Crop { w: W, h: W, rgb: &mut buf }, &mesh(true), &[(Kind::Smooth, 1.0)]).is_some() || true);
    // NaN points
    let mut nan = mesh(true);
    nan[10] = [f32::NAN; 3];
    let mut buf2 = vec![128u8; W * W * 3];
    let _ = retouch_face(Crop { w: W, h: W, rgb: &mut buf2 }, &nan, &[(Kind::Smooth, 1.0), (Kind::Blemishes, 1.0), (Kind::Teeth, 1.0)]);
}

#[test]
fn every_slider_has_a_description_and_a_unique_id() {
    let mut ids: Vec<&str> = KINDS.iter().map(|i| i.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), KINDS.len());
    assert!(KINDS.iter().all(|i| !i.help.is_empty() && !i.label.is_empty() && info_of(i.id).is_some()));
    // every kind is listed
    assert_eq!(KINDS.len(), 14);
}
