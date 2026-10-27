//! Guided dodge & burn: the soft planes (alpha 0..1 over the frame) and the nominal adjustments
//! of each part. A part becomes one ordinary local mask in the engine; nothing here moves a pixel.
//!
//! Two roles that are not to be mixed up (the retouchers' distinction): *cleanup* evens the
//! irregularities of the skin's luminosity (dark circles, patches, folds) without changing the
//! face; *sculpt* adds a little relief (cheekbones, nose ridge, forehead lit; the sides and
//! hollows shaded). Eyes, lips and the frame's edges are *finish*.

use crate::facegeo::{self, Face};
use crate::plane::{Plane, quantile, smoothstep};
use crate::skin::LabImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Cleanup,
    Sculpt,
    Eyes,
    Lips,
    Finish,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Cleanup => "cleanup",
            Role::Sculpt => "sculpt",
            Role::Eyes => "eyes",
            Role::Lips => "lips",
            Role::Finish => "finish",
        }
    }
}

/// Local adjustments of a part at strength 1 (the engine scales them by the asked strength;
/// the default 0.4 is a subtle one).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Adj {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub highlights: f32,
    pub shadows: f32,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub key: &'static str,
    pub label: &'static str,
    pub role: Role,
    pub alpha: Plane,
    pub adj: Adj,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Portrait,
    /// Flat lay / food: a plate or a dish on a surface.
    Flat,
    Product,
    Landscape,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "portrait" => Mode::Portrait,
            "flat" | "food" => Mode::Flat,
            "product" => Mode::Product,
            "landscape" => Mode::Landscape,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Portrait => "portrait",
            Mode::Flat => "flat",
            Mode::Product => "product",
            Mode::Landscape => "landscape",
        }
    }
}

fn mul(a: &Plane, b: &Plane) -> Plane {
    a.zip(b, |x, y| x * y)
}

fn finalize(parts: Vec<Part>) -> Vec<Part> {
    parts
        .into_iter()
        .map(|mut p| {
            p.alpha = p.alpha.map(|v| v.clamp(0.0, 1.0));
            p
        })
        .collect()
}

fn scaled(p: &Plane, k: f32) -> Plane {
    p.map(|v| (v * k).clamp(0.0, 1.0))
}

fn nonempty(p: &Plane, min_area: f32) -> bool {
    p.d.iter().sum::<f32>() > min_area
}

/// The parts of a portrait for one face. `skin` is the face's soft skin weight (eyes, lips, brows
/// excluded; beard, hair and highlights out), `w x h` the frame. `roles` limits what is drawn.
pub fn portrait(face: &Face, skin: &Plane, w: usize, h: usize, roles: &[Role]) -> Vec<Part> {
    let want = |r: Role| roles.contains(&r);
    let mut out = Vec::new();
    let fw = face.width;
    let blur = |p: &Plane, frac: f32| p.gauss((frac * fw).max(0.7));
    let (le, re) = (face.left_eye(), face.right_eye());
    let eyes_cov = facegeo::fill(&[&le, &re], w, h);

    if want(Role::Eyes) {
        // the whites and the irises of both eyes
        let mut iris = Plane::new(w, h, 0.0);
        let mut found = false;
        for left in [true, false] {
            if let Some((c, r)) = face.iris(left) {
                found = true;
                let d = facegeo::disc(w, h, c, r * 1.02, r * 0.18);
                iris = iris.zip(&d, f32::max);
            }
        }
        let iris = mul(&iris, &eyes_cov);
        let whites = eyes_cov.zip(&iris, |e, i| (e - i).max(0.0));
        let whites = blur(&whites, 0.004);
        if found && nonempty(&iris, 20.0) {
            out.push(Part {
                key: "iris",
                label: "Dodge: irises",
                role: Role::Eyes,
                alpha: iris.gauss((0.003 * fw).max(0.6)),
                adj: Adj { exposure: 0.55, contrast: 14.0, saturation: 8.0, ..Default::default() },
            });
        }
        if nonempty(&whites, 30.0) {
            out.push(Part {
                key: "whites",
                label: "Dodge: eye whites",
                role: Role::Eyes,
                alpha: whites,
                adj: Adj { exposure: 0.40, ..Default::default() },
            });
        }
    }
    if want(Role::Eyes) {
        // the skin under the eyes (dark circles): dodged
        let mut band = Plane::new(w, h, 0.0);
        for left in [true, false] {
            if let Some(p) = face.under_eye(left, 0.075) {
                band = band.zip(&facegeo::fill(&[&p], w, h), f32::max);
            }
        }
        let band = mul(&blur(&band, 0.012), skin);
        let band = band.zip(&eyes_cov, |b, e| b * (1.0 - e));
        if nonempty(&band, 50.0) {
            out.push(Part {
                key: "under_eyes",
                label: "Dodge: under the eyes",
                role: Role::Eyes,
                alpha: scaled(&band, 1.6),
                adj: Adj { exposure: 0.45, ..Default::default() },
            });
        }
    }
    if want(Role::Lips) {
        let (lo, li) = (face.lips_outer(), face.lips_inner());
        let outer = facegeo::fill(&[&lo], w, h);
        let inner = facegeo::fill(&[&li], w, h);
        let lips = blur(&outer.zip(&inner, |o, i| (o - i).max(0.0)), 0.005);
        if nonempty(&lips, 40.0) {
            out.push(Part {
                key: "lips",
                label: "Dodge: lips",
                role: Role::Lips,
                alpha: lips,
                adj: Adj { exposure: 0.22, saturation: 6.0, ..Default::default() },
            });
        }
    }
    if want(Role::Sculpt) {
        let rect = face.rect(0.12, w, h);
        let near = facegeo::nearness(face, rect);
        let cg = facegeo::convexity(&near, face, rect);
        let cvals: Vec<f32> = cg.d.iter().map(|v| v.abs()).collect();
        let cref = quantile(&cvals, 0.95).max(1e-4);
        let c = facegeo::grid_to_frame(&cg, rect, w, h);
        let convex = mul(&c.map(|v| (v / cref).clamp(0.0, 1.0)), skin);
        let concave = mul(&c.map(|v| (-v / cref).clamp(0.0, 1.0)), skin);
        // cheekbones: the convexity of the middle of the face, away from the nose
        let cheeks = mul(&convex, &mid_face(face, w, h, true));
        let cheeks = blur(&cheeks, 0.02);
        if nonempty(&cheeks, 80.0) {
            out.push(Part {
                key: "cheekbones",
                label: "Dodge: cheekbones",
                role: Role::Sculpt,
                alpha: scaled(&cheeks, 1.3),
                adj: Adj { exposure: 0.50, ..Default::default() },
            });
        }
        // the nose ridge and the middle of the forehead
        let line = face.nose_line();
        let mut ridge = Plane::new(w, h, 0.0);
        for pair in line.windows(2).take(4) {
            ridge = ridge.zip(&facegeo::capsule(w, h, pair[0], pair[1], 0.012 * fw, 0.025 * fw), f32::max);
        }
        let ridge = mul(&ridge, skin);
        if nonempty(&ridge, 40.0) {
            out.push(Part {
                key: "nose_ridge",
                label: "Dodge: nose ridge",
                role: Role::Sculpt,
                alpha: ridge,
                adj: Adj { exposure: 0.40, ..Default::default() },
            });
        }
        if let (Some(a), Some(b)) = (face.point(151), face.point(9)) {
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let blob = facegeo::disc(w, h, mid, 0.07 * fw, 0.10 * fw);
            let blob = mul(&blob, skin);
            if nonempty(&blob, 80.0) {
                out.push(Part {
                    key: "forehead",
                    label: "Dodge: forehead",
                    role: Role::Sculpt,
                    alpha: scaled(&blob, 1.0),
                    adj: Adj { exposure: 0.30, ..Default::default() },
                });
            }
        }
        // the sides of the face and the hollows: shaded
        let ring = mul(&oval_ring(face, w, h), skin);
        let hollows = blur(&concave, 0.02);
        let sides = ring.zip(&hollows, |r, c| (0.7 * r + 0.5 * c).clamp(0.0, 1.0));
        if nonempty(&sides, 120.0) {
            out.push(Part {
                key: "sides",
                label: "Burn: sides of the face",
                role: Role::Sculpt,
                alpha: sides,
                adj: Adj { exposure: -0.40, ..Default::default() },
            });
        }
    }
    finalize(out)
}

/// The middle of the face between the lower lids and the mouth, away from the nose axis
/// (`lateral` true), or the nose axis band itself (false). Soft.
fn mid_face(face: &Face, w: usize, h: usize, lateral: bool) -> Plane {
    let fw = face.width;
    let nose =
        face.point(168).zip(face.point(4)).map_or([face.bbox[0] + fw / 2.0, face.bbox[1] + fw], |(a, b)| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let eye_y = face.point(168).unwrap_or(nose);
    let mouth = face.point(0).unwrap_or(nose);
    let d = face.down;
    let perp = [-d[1], d[0]];
    let (v_top, v_bot) = ((eye_y[0] - nose[0]) * d[0] + (eye_y[1] - nose[1]) * d[1], (mouth[0] - nose[0]) * d[0] + (mouth[1] - nose[1]) * d[1]);
    let mut out = Plane::new(w, h, 0.0);
    let r = face.rect(0.1, w, h);
    for y in r[1] as usize..(r[3] as usize).min(h) {
        for x in r[0] as usize..(r[2] as usize).min(w) {
            let (px, py) = (x as f32 + 0.5 - nose[0], y as f32 + 0.5 - nose[1]);
            let (u, v) = (px * perp[0] + py * perp[1], px * d[0] + py * d[1]);
            let vert = smoothstep(v_top - 0.02 * fw, v_top + 0.05 * fw, v) * (1.0 - smoothstep(v_bot - 0.12 * fw, v_bot + 0.02 * fw, v));
            let lat = smoothstep(0.06 * fw, 0.16 * fw, u.abs());
            out.d[y * w + x] = vert * if lateral { lat } else { 1.0 - lat };
        }
    }
    out
}

/// A band just inside the face oval (1 at the contour, 0 deeper than a tenth of the face width).
fn oval_ring(face: &Face, w: usize, h: usize) -> Plane {
    let oval = facegeo::fill(&[&face.oval], w, h);
    let inner = oval.gauss((0.07 * face.width).max(1.0));
    oval.zip(&inner, |o, i| o * (1.0 - smoothstep(0.55, 0.97, i)))
}

/// Subject plane guess when no matting model is there: the colourful, bright, contrasty part of
/// the middle of the frame (soft, 0..1). Honest about being a heuristic: models do better.
pub fn saliency(lab: &LabImage) -> Plane {
    let (w, h) = (lab.w, lab.h);
    let edge = w.max(h) as f32;
    let g = |p: &Plane, f: f32| p.gauss((f * edge).max(1.0));
    let chroma = lab.a.zip(&lab.b, |a, b| a.hypot(b));
    let c = g(&chroma, 0.02);
    let l = g(&lab.l, 0.02);
    let contrast = lab.l.zip(&g(&lab.l, 0.03), |a, b| (a - b).abs());
    let contrast = g(&contrast, 0.02);
    let norm = |p: &Plane| {
        let hi = quantile(&p.d, 0.98).max(1e-3);
        let lo = quantile(&p.d, 0.05);
        p.map(|v| ((v - lo) / (hi - lo)).clamp(0.0, 1.0))
    };
    let (c, l, ct) = (norm(&c), norm(&l), norm(&contrast));
    let mut s = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 / w as f32 - 0.5) / 0.38, (y as f32 / h as f32 - 0.5) / 0.38);
            let centre = (-(dx * dx + dy * dy) / 2.0).exp();
            let i = y * w + x;
            s.d[i] = centre * (0.35 * c.d[i] + 0.35 * l.d[i] + 0.30 * ct.d[i]);
        }
    }
    let thr = quantile(&s.d, 0.62);
    let hi = quantile(&s.d, 0.90).max(thr + 1e-3);
    g(&s.map(|v| smoothstep(thr, hi, v)), 0.012)
}

/// 0 in the middle of the frame, 1 at its borders (soft).
pub fn edges(w: usize, h: usize) -> Plane {
    let mut p = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, (y as f32 + 0.5) / h as f32 * 2.0 - 1.0);
            let r = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
            p.d[y * w + x] = smoothstep(0.55, 1.0, r);
        }
    }
    p
}

/// Bright or loud spots outside the subject, which pull the eye away: soft, 0..1.
pub fn distractions(lab: &LabImage, subject: &Plane) -> Plane {
    let edge = lab.w.max(lab.h) as f32;
    let l = lab.l.gauss((0.012 * edge).max(1.0));
    let chroma = lab.a.zip(&lab.b, |a, b| a.hypot(b)).gauss((0.012 * edge).max(1.0));
    let wide = lab.l.gauss((0.08 * edge).max(2.0));
    let (l_hi, c_hi) = (quantile(&l.d, 0.90), quantile(&chroma.d, 0.93));
    let mut out = Plane::new(lab.w, lab.h, 0.0);
    for i in 0..lab.w * lab.h {
        let bright = smoothstep(l_hi, l_hi + 12.0, l.d[i]) * smoothstep(2.0, 10.0, l.d[i] - wide.d[i]);
        let loud = smoothstep(c_hi, c_hi + 15.0, chroma.d[i]) * smoothstep(2.0, 8.0, l.d[i] - wide.d[i]);
        out.d[i] = bright.max(0.8 * loud) * (1.0 - subject.d[i]).powi(2);
    }
    out.gauss((0.01 * edge).max(1.0))
}

/// Parts for the scenes without a face: a dish on a surface, a product, a landscape. `subject`
/// is the soft subject plane (a matting model's, else [`saliency`]); `sky` a sky plane when known.
pub fn scene(mode: Mode, lab: &LabImage, subject: &Plane, sky: Option<&Plane>) -> Vec<Part> {
    let (w, h) = (lab.w, lab.h);
    let edge = w.max(h) as f32;
    let soft = |p: &Plane| p.gauss((0.012 * edge).max(1.0));
    let bg = soft(&subject.map(|v| 1.0 - v));
    let mut out = Vec::new();
    let ring = edges(w, h);
    let dis = distractions(lab, subject);
    match mode {
        Mode::Flat | Mode::Product | Mode::Portrait => {
            let strong = mode == Mode::Product;
            // the subject is lifted a little where the eye should land
            let centre = soft(subject).zip(&centre_bias(w, h), |s, c| s * (0.5 + 0.5 * c));
            out.push(Part {
                key: "subject",
                label: "Dodge: subject",
                role: Role::Sculpt,
                alpha: centre,
                adj: Adj { exposure: 0.25, shadows: 10.0, ..Default::default() },
            });
            if strong {
                let hot = soft(subject).zip(&lab.l.map(|l| smoothstep(78.0, 96.0, l)), |s, l| s * l).gauss((0.01 * edge).max(1.0));
                out.push(Part {
                    key: "hot_spots",
                    label: "Burn: hot spots",
                    role: Role::Sculpt,
                    alpha: hot,
                    adj: Adj { exposure: -0.20, highlights: -35.0, ..Default::default() },
                });
            }
            out.push(Part {
                key: "background",
                label: "Burn: background",
                role: Role::Finish,
                alpha: bg,
                adj: Adj { exposure: if strong { -0.45 } else { -0.30 }, ..Default::default() },
            });
            out.push(Part {
                key: "edges",
                label: "Burn: edges",
                role: Role::Finish,
                alpha: ring,
                adj: Adj { exposure: -0.22, ..Default::default() },
            });
            out.push(Part {
                key: "distractions",
                label: "Burn: distractions",
                role: Role::Finish,
                alpha: dis,
                adj: Adj { exposure: -0.32, ..Default::default() },
            });
        }
        Mode::Landscape => {
            let sky_plane = match sky {
                Some(s) => soft(s),
                None => top_gradient(w, h),
            };
            out.push(Part {
                key: "sky",
                label: "Burn: sky",
                role: Role::Sculpt,
                alpha: sky_plane.clone(),
                adj: Adj { exposure: -0.35, highlights: -20.0, ..Default::default() },
            });
            let ground = soft(&sky_plane.map(|v| 1.0 - v)).zip(&centre_bias(w, h), |g, c| g * (0.4 + 0.6 * c));
            out.push(Part {
                key: "foreground",
                label: "Dodge: foreground",
                role: Role::Sculpt,
                alpha: ground,
                adj: Adj { exposure: 0.22, shadows: 12.0, ..Default::default() },
            });
            out.push(Part {
                key: "edges",
                label: "Burn: edges",
                role: Role::Finish,
                alpha: ring,
                adj: Adj { exposure: -0.20, ..Default::default() },
            });
            out.push(Part {
                key: "distractions",
                label: "Burn: distractions",
                role: Role::Finish,
                alpha: dis,
                adj: Adj { exposure: -0.30, ..Default::default() },
            });
        }
    }
    finalize(out.into_iter().filter(|p| nonempty(&p.alpha, 0.0004 * (w * h) as f32)).collect())
}

fn centre_bias(w: usize, h: usize) -> Plane {
    let mut p = Plane::new(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 / w as f32 - 0.5) / 0.45, (y as f32 / h as f32 - 0.5) / 0.45);
            p.d[y * w + x] = (-(dx * dx + dy * dy) / 2.0).exp();
        }
    }
    p
}

fn top_gradient(w: usize, h: usize) -> Plane {
    let mut p = Plane::new(w, h, 0.0);
    for y in 0..h {
        let v = 1.0 - smoothstep(0.15, 0.55, y as f32 / h as f32);
        for x in 0..w {
            p.d[y * w + x] = v;
        }
    }
    p
}

/// Choose a mode for a photo: portrait when a face was found, landscape when the sky covers
/// much of the frame, else flat.
pub fn auto_mode(has_face: bool, sky_share: Option<f32>) -> Mode {
    if has_face {
        Mode::Portrait
    } else if sky_share.is_some_and(|s| s > 0.2) {
        Mode::Landscape
    } else {
        Mode::Flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facegeo::tests::fake_mesh;

    fn skin_plane(face: &Face, w: usize, h: usize) -> Plane {
        let oval = facegeo::fill(&[&face.oval], w, h);
        let (le, re) = (face.left_eye(), face.right_eye());
        let holes = facegeo::fill(&[&le, &re, &face.lips_outer()], w, h).gauss(3.0).map(|v| (v * 3.0).min(1.0));
        oval.zip(&holes, |o, k| o * (1.0 - k))
    }

    #[test]
    fn a_portrait_gets_eyes_under_eyes_lips_and_sculpt_parts_in_their_places() {
        let f = Face::new(&fake_mesh()).expect("face");
        let skin = skin_plane(&f, 400, 400);
        let parts = portrait(&f, &skin, 400, 400, &[Role::Cleanup, Role::Sculpt, Role::Eyes, Role::Lips]);
        let keys: Vec<&str> = parts.iter().map(|p| p.key).collect();
        for k in ["iris", "whites", "under_eyes", "lips", "nose_ridge", "sides"] {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }
        let get = |k: &str| parts.iter().find(|p| p.key == k).expect(k);
        assert!(get("iris").alpha.at(250, 160) > 0.8 && get("iris").alpha.at(250, 172) < 0.3, "iris only on the iris");
        assert!(get("whites").alpha.at(268, 160) > 0.5 && get("whites").alpha.at(250, 160) < 0.2, "whites not on the iris");
        assert!(get("under_eyes").alpha.at(250, 175) > 0.3 && get("under_eyes").alpha.at(250, 160) < 0.1);
        assert!(get("lips").alpha.at(200, 272) > 0.3 && get("lips").alpha.at(200, 260) < 0.2, "lips minus the mouth opening");
        // nothing is drawn outside the face, whatever the part
        for p in &parts {
            assert!(p.alpha.at(5, 5) == 0.0 && p.alpha.d.iter().all(|v| (0.0..=1.0).contains(v)), "{}", p.key);
        }
    }

    #[test]
    fn roles_limit_what_is_drawn() {
        let f = Face::new(&fake_mesh()).expect("face");
        let skin = skin_plane(&f, 400, 400);
        let only = portrait(&f, &skin, 400, 400, &[Role::Eyes]);
        assert!(only.iter().all(|p| p.role == Role::Eyes), "{:?}", only.iter().map(|p| p.key).collect::<Vec<_>>());
        assert!(portrait(&f, &skin, 400, 400, &[]).is_empty());
    }

    fn plate() -> LabImage {
        // a colourful bright disc on a dark frame
        let (w, h) = (160usize, 120usize);
        let mut px = vec![[18u8, 18, 20, 255]; w * h];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = ((x as f32 - 80.0) / 40.0, (y as f32 - 60.0) / 32.0);
                if dx * dx + dy * dy < 1.0 {
                    px[y * w + x] = [200, 150, 60, 255];
                }
            }
        }
        // a bright spot in a corner
        for y in 8..16 {
            for x in 8..16 {
                px[y * w + x] = [250, 250, 245, 255];
            }
        }
        LabImage::from_rgba(w, h, &px)
    }

    #[test]
    fn the_saliency_fallback_finds_the_dish_not_the_corner() {
        let lab = plate();
        let s = saliency(&lab);
        assert!(s.at(80, 60) > 0.8 && s.at(12, 12) < 0.3 && s.at(150, 110) < 0.2, "{} {} {}", s.at(80, 60), s.at(12, 12), s.at(150, 110));
    }

    #[test]
    fn a_dish_scene_lifts_the_dish_and_burns_the_background_edges_and_the_bright_corner() {
        let lab = plate();
        let subject = saliency(&lab);
        let parts = scene(Mode::Flat, &lab, &subject, None);
        let get = |k: &str| parts.iter().find(|p| p.key == k);
        assert!(get("subject").is_some_and(|p| p.alpha.at(80, 60) > 0.4 && p.adj.exposure > 0.0));
        assert!(get("background").is_some_and(|p| p.alpha.at(150, 110) > 0.7 && p.alpha.at(80, 60) < 0.2 && p.adj.exposure < 0.0));
        assert!(get("edges").is_some_and(|p| p.alpha.at(2, 2) > 0.5 && p.alpha.at(80, 60) == 0.0));
        assert!(get("distractions").is_some_and(|p| p.alpha.at(12, 12) > 0.3 && p.alpha.at(80, 60) < 0.05), "the bright corner is attenuated");
    }

    #[test]
    fn auto_mode_prefers_faces_then_sky() {
        assert_eq!(auto_mode(true, Some(0.9)), Mode::Portrait);
        assert_eq!(auto_mode(false, Some(0.4)), Mode::Landscape);
        assert_eq!(auto_mode(false, None), Mode::Flat);
        assert_eq!(Mode::parse("food"), Some(Mode::Flat));
    }
}
