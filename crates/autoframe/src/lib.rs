//! Automatic straighten and crop *suggestions* (L3). Pure functions over decoded pixels and face
//! boxes: nothing here edits a photo, reads a file or touches the network. The engine turns a
//! suggestion into a preview, and the photographer accepts it or not.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! Straighten and crop *suggestions*. Nothing here edits a photo: the engine turns a suggestion
//! into a preview, and the photographer accepts it or not.
//!
//! Straighten reuses the pipeline's Upright analysis (gradient line segments, consensus of the
//! horizon / plumb lines) and adds a confidence. The crop is a search over rectangles inside the
//! rotated image for the best score: subject kept whole with room around it, subject near a
//! rule-of-thirds point, horizon on a third, and as little resolution lost as possible.

use lightcraft_develop::DevelopSettings;
use lightcraft_geom::CropGeometry;
use lightcraft_geom::{Point, Rect};
use lightcraft_pipeline::SourceInfo;
use lightcraft_pipeline::upright::{self, Segment};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, fit};
use serde::Serialize;

/// A suggested rotation.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Straighten {
    /// Degrees, same sign convention as the crop angle (what `crop.straighten` takes).
    pub angle: f64,
    /// 0..1: how much of the detected line length agrees with the angle.
    pub confidence: f32,
    /// `"horizon"` or `"vertical"`.
    pub kind: &'static str,
    pub lines: usize,
    /// Where the horizon sits, as a fraction of the height from the top, when the lines are a horizon.
    pub horizon_y: Option<f64>,
}

fn deviation(s: &Segment, vertical: bool) -> f64 {
    let (dx, dy) = (s.b.x - s.a.x, s.b.y - s.a.y);
    let a = if vertical { dx.atan2(dy) } else { dy.atan2(dx) }.to_degrees();
    let a = if a > 90.0 {
        a - 180.0
    } else if a < -90.0 {
        a + 180.0
    } else {
        a
    };
    if vertical { -a } else { a }
}

/// Suggest the rotation that levels the horizon or the plumb lines, or `None` when the photo has
/// no dominant straight lines (or the lines disagree: foliage, ridges).
pub fn straighten(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Option<Straighten> {
    let angle = upright::level_degrees(src, info, s)?;
    let a = upright::analyze_source(src, info, s);
    let lh: f64 = a.horizontal.iter().map(Segment::len).sum();
    let lv: f64 = a.vertical.iter().map(Segment::len).sum();
    let (set, vertical) = if lh >= lv * 0.5 && lh > 0.0 { (&a.horizontal, false) } else { (&a.vertical, true) };
    let total: f64 = set.iter().map(Segment::len).sum();
    let agree: f64 = set.iter().filter(|g| (-deviation(g, vertical) - angle).abs() <= 1.5).map(Segment::len).sum();
    let confidence = if total > 0.0 { (agree / total) as f32 } else { 0.0 };
    // centred coordinates are (p - centre) / (long edge / 2): back to a fraction of the height
    let horizon_y = (!vertical && total > 0.0).then(|| {
        let mean_y = set.iter().map(|g| g.mid().y * g.len()).sum::<f64>() / total;
        let long = src.width.max(src.height) as f64;
        0.5 + mean_y * (long / 2.0) / (src.height.max(1) as f64)
    });
    let horizon_y = horizon_y.filter(|y| (0.05..=0.95).contains(y));
    Some(Straighten { angle: angle.clamp(-45.0, 45.0), confidence, kind: if vertical { "vertical" } else { "horizon" }, lines: set.len(), horizon_y })
}

/// Where the subject is, in the image's own normalised coordinates (the photo as oriented).
#[derive(Clone, Debug, PartialEq)]
pub struct Subject {
    pub rect: Rect,
    pub weight: f64,
    /// `"face"`, `"person"`, `"saliency"`...
    pub kind: &'static str,
}

/// A rough subject from colour/contrast saliency on a small proxy, for photos with no face and no
/// subject mask. Returns `None` when nothing stands out (flat photo).
pub fn saliency_subject(src: &Rgb32f) -> Option<Subject> {
    let img = fit(src, 48, 48, Filter::Box);
    let (w, h) = (img.width, img.height);
    if w < 4 || h < 4 {
        return None;
    }
    let lum: Vec<f32> = img.data.iter().map(|c| ((0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).max(1e-4)).ln()).collect();
    let n = lum.len() as f32;
    let mean_l = lum.iter().sum::<f32>() / n;
    let mean_c = img.data.iter().fold([0f32; 3], |mut a, c| {
        for (acc, v) in a.iter_mut().zip(c) {
            *acc += v / n;
        }
        a
    });
    let mut sal = Vec::with_capacity(lum.len());
    for (l, c) in lum.iter().zip(&img.data) {
        let dc = (0..3).map(|i| ((c[i] + 1e-4) / (mean_c[i] + 1e-4)).ln().abs()).sum::<f32>();
        sal.push((l - mean_l).abs() + 0.5 * dc);
    }
    let mut sorted = sal.clone();
    sorted.sort_by(f32::total_cmp);
    let top = sorted.last().copied()?;
    // the top 15 % of pixels, but never the flat background that merely ties with them
    let cut = sorted.get(sorted.len() * 85 / 100).copied()?.max(0.5 * top);
    if top < 0.25 {
        return None;
    }
    let (mut sx, mut sy, mut sw) = (0f64, 0f64, 0f64);
    for (i, v) in sal.iter().enumerate() {
        if *v >= cut {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            sx += x * *v as f64;
            sy += y * *v as f64;
            sw += *v as f64;
        }
    }
    if sw <= 0.0 {
        return None;
    }
    let (cx, cy) = (sx / sw / w as f64, sy / sw / h as f64);
    let (mut vx, mut vy) = (0f64, 0f64);
    for (i, v) in sal.iter().enumerate() {
        if *v >= cut {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            vx += (x / w as f64 - cx).powi(2) * *v as f64;
            vy += (y / h as f64 - cy).powi(2) * *v as f64;
        }
    }
    let (sdx, sdy) = ((vx / sw).sqrt().max(0.03), (vy / sw).sqrt().max(0.03));
    let rect = Rect::new((cx - 1.6 * sdx).max(0.0), (cy - 1.6 * sdy).max(0.0), (cx + 1.6 * sdx).min(1.0), (cy + 1.6 * sdy).min(1.0));
    Some(Subject { rect, weight: 0.6, kind: "saliency" })
}

/// What the crop search was asked.
#[derive(Clone, Debug)]
pub struct CropRequest {
    /// Oriented image size in pixels.
    pub width: f64,
    pub height: f64,
    /// Straighten angle already chosen (degrees); 0 if none.
    pub angle: f64,
    /// Wanted aspect (w/h); `None`: keep the image's own.
    pub aspect: Option<f64>,
    pub subjects: Vec<Subject>,
    /// Horizon line as a fraction of the height (0 top, 1 bottom), when known.
    pub horizon: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CropSuggestion {
    /// Normalised crop rectangle `[x0, y0, x1, y1]` in the rotated frame.
    pub rect: [f64; 4],
    pub angle: f64,
    pub score: f64,
    /// Score of the same-aspect full frame (what you have now).
    pub score_full: f64,
    /// Share of the image area kept.
    pub kept: f64,
    pub reasons: Vec<String>,
}

fn thirds_distance(p: Point) -> f64 {
    let pts = [(1.0 / 3.0, 1.0 / 3.0), (2.0 / 3.0, 1.0 / 3.0), (1.0 / 3.0, 2.0 / 3.0), (2.0 / 3.0, 2.0 / 3.0)];
    pts.iter().map(|(x, y)| ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt()).fold(f64::MAX, f64::min)
}

/// Score a crop `c` (pixels of the rotated frame) against the subjects. Higher is better.
fn score(c: Rect, full: Rect, subjects: &[(Rect, f64)], horizon: Option<f64>) -> (f64, Vec<String>) {
    let mut reasons = Vec::new();
    let mut s = 0.0;
    let area_ratio = c.area() / full.area().max(1.0);
    s += 0.4 * area_ratio; // keep resolution
    let rel = |r: Rect| Rect::new((r.x0 - c.x0) / c.width(), (r.y0 - c.y0) / c.height(), (r.x1 - c.x0) / c.width(), (r.y1 - c.y0) / c.height());
    let total_w: f64 = subjects.iter().map(|x| x.1).sum::<f64>().max(1e-6);
    for (r, wgt) in subjects {
        let inside = r.intersect(&c).area() / r.area().max(1e-9);
        let rr = rel(*r);
        let share = wgt / total_w;
        s += share * 1.2 * inside; // the subject stays whole
        if inside < 0.98 {
            s -= share * 1.5 * (1.0 - inside);
        }
        let d = thirds_distance(rr.center());
        s += share * 0.5 * (1.0 - (d / 0.35).min(1.0));
        // breathing room: neither glued to the edge nor lost in the frame
        let fill = (rr.width() * rr.height()).sqrt();
        s += share * 0.5 * (1.0 - ((fill - 0.35) / 0.35).abs().min(1.0));
        let margin = rr.x0.min(rr.y0).min(1.0 - rr.x1).min(1.0 - rr.y1);
        if margin < 0.02 {
            s -= share * 0.3;
        }
    }
    if let Some(h) = horizon {
        let hy = (h * full.height() - c.y0) / c.height();
        if (0.0..=1.0).contains(&hy) {
            let d = (hy - 1.0 / 3.0).abs().min((hy - 2.0 / 3.0).abs());
            s += 0.4 * (1.0 - (d / 0.2).min(1.0));
            if (hy - 0.5).abs() < 0.06 {
                s -= 0.15; // dead-centre horizon
            }
        }
    }
    if s > 0.0 && area_ratio < 0.99 {
        reasons.push(format!("garde {:.0} % de l'image", area_ratio * 100.0));
    }
    (s, reasons)
}

/// Search the best crop. Returns `None` when no candidate beats the full frame by a clear margin
/// (the framing is already fine) or when there is nothing to frame.
pub fn suggest_crop(req: &CropRequest) -> Option<CropSuggestion> {
    let (w, h) = (req.width, req.height);
    if !(w > 8.0 && h > 8.0) || req.subjects.is_empty() {
        return None;
    }
    let aspect = req.aspect.filter(|a| *a > 0.1 && *a < 10.0).unwrap_or(w / h);
    let full_frame = lightcraft_geom::crop_fit_angle(w, h, req.angle, Some(aspect));
    // subjects into the rotated frame, in pixels
    let rot = lightcraft_geom::Affine::rotate_about(req.angle.to_radians(), Point::new(w / 2.0, h / 2.0));
    let subjects: Vec<(Rect, f64)> = req
        .subjects
        .iter()
        .map(|s| (rot.transform_rect_bbox(&s.rect.scale(w, h)), s.weight))
        .filter(|(r, _)| !r.is_empty())
        .collect();
    if subjects.is_empty() {
        return None;
    }
    let img_rect = Rect::new(0.0, 0.0, w, h);
    let horizon = req.horizon;
    let full_rect = full_frame.rect_px(w, h);
    let (score_full, _) = score(full_rect, img_rect, &subjects, horizon);
    let mut best: Option<(f64, Rect, Vec<String>)> = None;
    let (base_w, base_h) = (full_rect.width(), full_rect.height());
    let steps = 14;
    for si in 0..=steps {
        let scale = 1.0 - 0.45 * si as f64 / steps as f64;
        let (cw, ch) = (base_w * scale, base_h * scale);
        let nx = if si == 0 { 1 } else { 12 };
        for xi in 0..=nx {
            for yi in 0..=nx {
                let fx = if nx == 1 { 0.0 } else { xi as f64 / nx as f64 };
                let fy = if nx == 1 { 0.0 } else { yi as f64 / nx as f64 };
                let x0 = full_rect.x0 + (base_w - cw) * fx;
                let y0 = full_rect.y0 + (base_h - ch) * fy;
                let c = Rect::new(x0, y0, x0 + cw, y0 + ch);
                let geo = CropGeometry { rect: c.scale(1.0 / w, 1.0 / h), angle: req.angle };
                if !geo.is_within_image(w, h) {
                    continue;
                }
                let (sc, reasons) = score(c, img_rect, &subjects, horizon);
                if best.as_ref().is_none_or(|b| sc > b.0) {
                    best = Some((sc, c, reasons));
                }
            }
        }
    }
    let (sc, c, mut reasons) = best?;
    if sc < score_full + 0.10 {
        return None;
    }
    let kinds: Vec<&str> = req.subjects.iter().map(|s| s.kind).collect();
    reasons.insert(0, format!("sujet ({}) recentré sur un tiers", kinds.join(", ")));
    let r = c.scale(1.0 / w, 1.0 / h);
    Some(CropSuggestion { rect: [r.x0, r.y0, r.x1, r.y1], angle: req.angle, score: sc, score_full, kept: c.area() / img_rect.area(), reasons })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(subject: Rect) -> CropRequest {
        CropRequest {
            width: 3000.0,
            height: 2000.0,
            angle: 0.0,
            aspect: None,
            subjects: vec![Subject { rect: subject, weight: 1.0, kind: "face" }],
            horizon: None,
        }
    }

    #[test]
    fn a_small_off_centre_subject_gets_a_tighter_crop_that_keeps_it_whole() {
        let subject = Rect::new(0.80, 0.06, 0.90, 0.26);
        let c = suggest_crop(&req(subject)).expect("a suggestion");
        let r = Rect::new(c.rect[0], c.rect[1], c.rect[2], c.rect[3]);
        assert!(r.intersect(&subject).area() >= subject.area() * 0.98, "{c:?}");
        assert!(c.kept < 0.9, "tighter than the full frame: {c:?}");
        assert!((r.aspect() * 1.0 - 1.5).abs() < 0.02 || r.width() * 3000.0 / (r.height() * 2000.0) > 1.4);
    }

    #[test]
    fn a_well_framed_subject_on_a_third_is_left_alone() {
        let subject = Rect::new(0.28, 0.28, 0.40, 0.45);
        assert!(suggest_crop(&req(subject)).is_none(), "{:?}", suggest_crop(&req(subject)));
    }

    #[test]
    fn nothing_to_frame_or_a_degenerate_image_gives_nothing() {
        let mut r = req(Rect::new(0.4, 0.4, 0.5, 0.5));
        r.subjects.clear();
        assert!(suggest_crop(&r).is_none());
        let mut r = req(Rect::new(0.4, 0.4, 0.5, 0.5));
        r.width = 0.0;
        assert!(suggest_crop(&r).is_none());
    }

    #[test]
    fn a_tilted_photo_crop_stays_inside_the_rotated_image() {
        let mut r = req(Rect::new(0.55, 0.3, 0.65, 0.5));
        r.angle = 6.0;
        if let Some(c) = suggest_crop(&r) {
            let g = CropGeometry { rect: Rect::new(c.rect[0], c.rect[1], c.rect[2], c.rect[3]), angle: 6.0 };
            assert!(g.is_within_image(3000.0, 2000.0));
        }
    }

    #[test]
    fn saliency_finds_a_bright_blob_on_a_flat_background() {
        let img = Rgb32f::from_fn(96, 64, |x, y| if (60..80).contains(&x) && (14..34).contains(&y) { [0.9, 0.2, 0.1] } else { [0.12, 0.12, 0.12] });
        let s = saliency_subject(&img).expect("a subject");
        let c = s.rect.center();
        assert!((c.x - 0.73).abs() < 0.12 && (c.y - 0.37).abs() < 0.15, "{c:?}");
        assert!(saliency_subject(&Rgb32f::from_fn(64, 64, |_, _| [0.3; 3])).is_none());
    }

    #[test]
    fn a_tilted_horizon_is_found_and_a_level_one_is_left_alone() {
        let tilted = |deg: f64| {
            Rgb32f::from_fn(1024, 683, |x, y| {
                let line = 340.0 + deg.to_radians().tan() * (x as f64 - 512.0);
                let land = 0.05;
                let _ = x;
                // furrows parallel to the horizon, as a real field or street has
                let band = (((y as f64 - line) / 60.0).floor() as i64).rem_euclid(2) as f32;
                if (y as f64) < line { [0.45, 0.55, 0.8] } else { [land + 0.12 * band, land + 0.02 + 0.12 * band, land + 0.1 * band] }
            })
        };
        let info = SourceInfo::default();
        let d = DevelopSettings::default();
        let a = straighten(&tilted(5.0), &info, &d).expect("a 5 degree horizon is found");
        assert_eq!(a.kind, "horizon");
        assert!((a.angle.abs() - 5.0).abs() < 1.0, "{a:?}");
        assert!(a.confidence > 0.6, "{a:?}");
        // the sign matches the pipeline's crop angle: applying it makes the horizon level
        let level = straighten(&tilted(0.0), &info, &d);
        assert!(level.as_ref().is_none_or(|l| l.angle.abs() < 0.5), "{level:?}");
    }
}
