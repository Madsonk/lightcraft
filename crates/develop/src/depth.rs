//! Data the render needs that the settings only *refer to*, never hold: the depth map behind
//! Lens Blur and the depth mask ([`DepthMap`]), and the pixels an inpainting produced
//! ([`InpaintPatch`]). Models make them once; they live in files next to the library (not in
//! the catalog's journal) and the engine attaches them to the settings it renders ([`Aux`]), so
//! rendering never needs a model.

use std::sync::Arc;

/// Largest side a stored depth map or patch may claim (hostile input caps the allocation).
pub const MAX_SIDE: usize = 8192;

/// A relative depth map of the oriented, uncropped photo: `u16` per cell, 65535 = nearest,
/// 0 = farthest, over the whole image (cell `(x, y)` covers normalized `((x + .5) / width,
/// (y + .5) / height)`). Depth is *relative*: a model predicts it up to an unknown scale and
/// shift, so values are normalized per photo (2nd..98th percentile → 0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u16>,
    /// A hash of the content (cache keys).
    pub id: u64,
}

fn hash_of(parts: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(parts)
}

impl DepthMap {
    pub fn new(width: usize, height: usize, data: Vec<u16>) -> Option<DepthMap> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || data.len() != width * height {
            return None;
        }
        let id = hash_of((width, height, &data));
        Some(DepthMap { width, height, data, id })
    }

    /// From a model's relative inverse depth (larger = nearer): normalized so the 2nd and 98th
    /// percentiles map to 0 and 1.
    pub fn from_relative(width: usize, height: usize, raw: &[f32]) -> Option<DepthMap> {
        if raw.len() != width * height || raw.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let mut s: Vec<f32> = raw.iter().step_by((raw.len() / 20_000).max(1)).copied().collect();
        s.sort_by(f32::total_cmp);
        let at = |q: f32| s[((s.len() - 1) as f32 * q) as usize];
        let (lo, hi) = (at(0.02), at(0.98));
        let span = (hi - lo).max(1e-6);
        let data = raw.iter().map(|v| (((v - lo) / span).clamp(0.0, 1.0) * 65535.0).round() as u16).collect();
        DepthMap::new(width, height, data)
    }

    /// The depth (0 far … 1 near) at normalized image point `(x, y)`, bilinearly interpolated.
    pub fn at(&self, x: f64, y: f64) -> f32 {
        let (w, h) = (self.width, self.height);
        let fx = ((x * w as f64 - 0.5) as f32).clamp(0.0, (w - 1) as f32);
        let fy = ((y * h as f64 - 0.5) as f32).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let g = |x: usize, y: usize| f32::from(self.data[y * w + x]) / 65535.0;
        let top = g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx;
        let bot = g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx;
        top * (1.0 - ty) + bot * ty
    }

    /// The file form: `LCD1`, width and height (u32 LE), then the cells' high bytes and low bytes
    /// as two planes, deflated.
    pub fn to_bytes(&self) -> Vec<u8> {
        let n = self.data.len();
        let mut planes = Vec::with_capacity(2 * n);
        planes.extend(self.data.iter().map(|v| (v >> 8) as u8));
        planes.extend(self.data.iter().map(|v| (*v & 0xff) as u8));
        let mut out = b"LCD1".to_vec();
        out.extend((self.width as u32).to_le_bytes());
        out.extend((self.height as u32).to_le_bytes());
        out.extend(miniz_oxide::deflate::compress_to_vec(&planes, 6));
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<DepthMap> {
        let rest = b.strip_prefix(b"LCD1")?;
        let (w, h) = (u32::from_le_bytes(rest.get(0..4)?.try_into().ok()?) as usize, u32::from_le_bytes(rest.get(4..8)?.try_into().ok()?) as usize);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return None;
        }
        let n = w * h;
        let planes = miniz_oxide::inflate::decompress_to_vec_with_limit(rest.get(8..)?, 2 * n).ok()?;
        if planes.len() != 2 * n {
            return None;
        }
        let data = (0..n).map(|i| (u16::from(planes[i]) << 8) | u16::from(planes[n + i])).collect();
        DepthMap::new(w, h, data)
    }
}

/// Pixels an inpainting made for part of the photo: the model's fill of the rectangle `rect`
/// (normalized, oriented uncropped image), `width × height` sRGB bytes with a coverage `alpha`
/// (the selection with feathered edges; 255 = use the fill).
#[derive(Clone, Debug, PartialEq)]
pub struct InpaintPatch {
    pub rect: [f64; 4],
    pub width: usize,
    pub height: usize,
    /// Interleaved sRGB, `width × height × 3`.
    pub rgb: Vec<u8>,
    pub alpha: Vec<u8>,
    pub id: u64,
}

impl InpaintPatch {
    pub fn new(rect: [f64; 4], width: usize, height: usize, rgb: Vec<u8>, alpha: Vec<u8>) -> Option<InpaintPatch> {
        let ok = width > 0
            && height > 0
            && width <= MAX_SIDE
            && height <= MAX_SIDE
            && rgb.len() == width * height * 3
            && alpha.len() == width * height
            && rect.iter().all(|v| v.is_finite())
            && rect[2] > rect[0]
            && rect[3] > rect[1];
        ok.then(|| {
            let id = hash_of((rect.map(f64::to_bits), width, height, &rgb, &alpha));
            InpaintPatch { rect, width, height, rgb, alpha, id }
        })
    }

    /// The id as [`Spot::patch`](crate::Spot) names it.
    pub fn hex_id(&self) -> String {
        format!("{:016x}", self.id)
    }

    /// The file form: `LCP1`, rect (4 × f64 LE), width, height (u32 LE), then alpha, red, green
    /// and blue planes, deflated.
    pub fn to_bytes(&self) -> Vec<u8> {
        let n = self.width * self.height;
        let mut planes = Vec::with_capacity(4 * n);
        planes.extend(&self.alpha);
        for c in 0..3 {
            planes.extend((0..n).map(|i| self.rgb[i * 3 + c]));
        }
        let mut out = b"LCP1".to_vec();
        for v in self.rect {
            out.extend(v.to_le_bytes());
        }
        out.extend((self.width as u32).to_le_bytes());
        out.extend((self.height as u32).to_le_bytes());
        out.extend(miniz_oxide::deflate::compress_to_vec(&planes, 6));
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<InpaintPatch> {
        let rest = b.strip_prefix(b"LCP1")?;
        let mut rect = [0f64; 4];
        for (i, r) in rect.iter_mut().enumerate() {
            *r = f64::from_le_bytes(rest.get(i * 8..i * 8 + 8)?.try_into().ok()?);
        }
        let w = u32::from_le_bytes(rest.get(32..36)?.try_into().ok()?) as usize;
        let h = u32::from_le_bytes(rest.get(36..40)?.try_into().ok()?) as usize;
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return None;
        }
        let n = w * h;
        let planes = miniz_oxide::inflate::decompress_to_vec_with_limit(rest.get(40..)?, 4 * n).ok()?;
        if planes.len() != 4 * n {
            return None;
        }
        let rgb = (0..n * 3).map(|i| planes[n + (i % 3) * n + i / 3]).collect();
        InpaintPatch::new(rect, w, h, rgb, planes[..n].to_vec())
    }
}

/// Render-time data attached to a settings value by the engine. Never serialized (it is not part
/// of the edit), and two values are equal when they hold the same objects.
#[derive(Clone, Default)]
pub struct Aux {
    pub depth: Option<Arc<DepthMap>>,
    pub patches: Vec<Arc<InpaintPatch>>,
}

impl Aux {
    pub fn is_empty(&self) -> bool {
        self.depth.is_none() && self.patches.is_empty()
    }

    /// A hash of what is attached (cache keys).
    pub fn key(&self) -> u64 {
        if self.is_empty() {
            return 0;
        }
        hash_of((self.depth.as_ref().map(|d| d.id), self.patches.iter().map(|p| p.id).collect::<Vec<_>>()))
    }

    /// The attached patch whose id is `hex` (as [`Spot::patch`](crate::Spot) names it).
    pub fn patch(&self, hex: &str) -> Option<&Arc<InpaintPatch>> {
        let id = u64::from_str_radix(hex, 16).ok()?;
        self.patches.iter().find(|p| p.id == id)
    }
}

impl PartialEq for Aux {
    fn eq(&self, o: &Aux) -> bool {
        let same = |a: &Option<Arc<DepthMap>>, b: &Option<Arc<DepthMap>>| match (a, b) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same(&self.depth, &o.depth) && self.patches.len() == o.patches.len() && self.patches.iter().zip(&o.patches).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl std::fmt::Debug for Aux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Aux {{ depth: {:?}, patches: {} }}", self.depth.as_ref().map(|d| (d.width, d.height)), self.patches.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_map_round_trips_and_samples() {
        let raw: Vec<f32> = (0..12 * 8).map(|i| (i % 12) as f32).collect();
        let d = DepthMap::from_relative(12, 8, &raw).unwrap();
        assert!(d.at(0.01, 0.5) < 0.1 && d.at(0.99, 0.5) > 0.9);
        let back = DepthMap::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(back, d);
        assert!(DepthMap::from_bytes(b"LCD1\xff\xff\xff\xff\xff\xff\xff\xff").is_none());
        assert!(DepthMap::from_bytes(&d.to_bytes()[..20]).is_none());
    }

    #[test]
    fn patch_round_trips() {
        let rgb: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
        let p = InpaintPatch::new([0.1, 0.2, 0.3, 0.4], 4, 3, rgb, (0..12).map(|i| i as u8 * 20).collect()).unwrap();
        assert_eq!(InpaintPatch::from_bytes(&p.to_bytes()).unwrap(), p);
        assert!(InpaintPatch::new([0.5, 0.5, 0.4, 0.6], 1, 1, vec![0; 3], vec![0]).is_none());
    }
}
