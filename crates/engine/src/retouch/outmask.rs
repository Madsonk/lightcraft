//! Soft planes to ordinary masks: a patch of stored logits over a face (or the whole frame)
//! carrying local adjustments, exactly like the portrait retouch's masks. Editable in the
//! Masking panel; the pixels of the photo are never touched.

use lightcraft_develop::{LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_retouch::facegeo::frame_to_grid;
use lightcraft_retouch::plane::Plane;

/// Side of a face patch and of a whole-frame plane.
pub const FACE_SIDE: usize = 256;
pub const FRAME_SIDE: usize = 512;

fn logit(p: f32) -> f32 {
    let p = p.clamp(0.002, 0.998);
    (p / (1.0 - p)).ln()
}

/// One face's share of a mask: an alpha plane over the frame and the face's rectangle (pixels).
pub struct FacePlane<'a> {
    pub rect: [f32; 4],
    pub plane: &'a Plane,
}

fn component(detail: Vec<SegMask>, seg: Option<SegMask>, text: &str) -> MaskComponent {
    MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Prompt { text: text.into(), seg, detail, edge: 0.0 } }
}

/// A mask made of per-face patches (at most the detail limit), `None` when nothing is covered.
pub fn face_mask(id: u32, name: &str, faces: &[FacePlane], w: usize, h: usize, adjust: LocalAdjustments) -> Option<Mask> {
    let mut detail = Vec::new();
    for f in faces.iter().take(crate::portrait_max_detail()) {
        let g = frame_to_grid(f.plane, f.rect, FACE_SIDE);
        if g.d.iter().cloned().fold(0.0, f32::max) < 0.01 {
            continue;
        }
        let rect =
            [f64::from(f.rect[0]) / w as f64, f64::from(f.rect[1]) / h as f64, f64::from(f.rect[2]) / w as f64, f64::from(f.rect[3]) / h as f64];
        let logits: Vec<f32> = g.d.iter().map(|v| logit(*v)).collect();
        detail.push(SegMask::from_logits_in(FACE_SIDE, &logits, rect));
    }
    if detail.is_empty() {
        return None;
    }
    Some(Mask { id, name: name.to_string(), components: vec![component(detail, None, "face")], adjust, ..Default::default() })
}

/// A mask over the whole frame from one alpha plane (stored at `FRAME_SIDE`), `None` when empty.
pub fn frame_mask(id: u32, name: &str, plane: &Plane, adjust: LocalAdjustments) -> Option<Mask> {
    if plane.d.iter().cloned().fold(0.0, f32::max) < 0.01 {
        return None;
    }
    let g = frame_to_grid(plane, [0.0, 0.0, plane.w as f32, plane.h as f32], FRAME_SIDE);
    let logits: Vec<f32> = g.d.iter().map(|v| logit(*v)).collect();
    Some(Mask {
        id,
        name: name.to_string(),
        components: vec![component(Vec::new(), Some(SegMask::from_logits(FRAME_SIDE, &logits)), "retouch")],
        adjust,
        ..Default::default()
    })
}

/// The alpha a stored mask gives over a `w x h` frame (union of its patches, max), for tests and
/// for "what changed outside" checks.
pub fn alpha_of(m: &Mask, w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h, 0.0);
    for c in &m.components {
        let MaskShape::Prompt { detail, seg, .. } = &c.shape else { continue };
        let segs: Vec<&SegMask> = if detail.is_empty() { seg.iter().collect() } else { detail.iter().collect() };
        for s in segs {
            let (Some(logits), Some(b)) = (s.logits(), s.bounds()) else { continue };
            let side = s.side as usize;
            let grid = Plane::from_vec(side, side, logits.iter().map(|l| 1.0 / (1.0 + (-l).exp())).collect());
            let rect = [b[0] * w as f64, b[1] * h as f64, b[2] * w as f64, b[3] * h as f64];
            let (x0, x1) = (rect[0].floor().max(0.0) as usize, (rect[2].ceil() as usize).min(w));
            let (y0, y1) = (rect[1].floor().max(0.0) as usize, (rect[3].ceil() as usize).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let (u, v) = ((x as f64 + 0.5 - rect[0]) / (rect[2] - rect[0]), (y as f64 + 0.5 - rect[1]) / (rect[3] - rect[1]));
                    let a = grid.sample((u * side as f64) as f32, (v * side as f64) as f32);
                    let i = y * w + x;
                    out.d[i] = out.d[i].max(a);
                }
            }
        }
    }
    out
}
