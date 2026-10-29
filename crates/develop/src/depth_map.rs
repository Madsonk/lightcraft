//! The relative depth map behind Lens Blur and the Depth Range mask ([`DepthMap`]). A model makes it
//! once; it lives in a file next to the library and the engine attaches it to the settings it renders
//! ([`crate::Aux`]), so rendering never needs the model.

use crate::inpaint_patch::{MAX_SIDE, hash_of};

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
}
