//! Data the render needs that the settings only *refer to*, never hold: the pixels a portrait
//! retouch baked ([`InpaintPatch`]). They live in files next to the library (not in the catalog's
//! journal) and the engine attaches them to the settings it renders ([`Aux`]), so rendering never
//! needs a model. (The fork's depth map for Lens Blur is not part of this port.)

use std::sync::Arc;

/// Largest side a stored patch may claim (hostile input caps the allocation).
pub const MAX_SIDE: usize = 8192;

fn hash_of(parts: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(parts)
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
    pub patches: Vec<Arc<InpaintPatch>>,
}

impl Aux {
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }

    /// A hash of what is attached (cache keys).
    pub fn key(&self) -> u64 {
        if self.is_empty() {
            return 0;
        }
        hash_of(self.patches.iter().map(|p| p.id).collect::<Vec<_>>())
    }

    /// The attached patch whose id is `hex` (as [`Spot::patch`](crate::Spot) names it).
    pub fn patch(&self, hex: &str) -> Option<&Arc<InpaintPatch>> {
        let id = u64::from_str_radix(hex, 16).ok()?;
        self.patches.iter().find(|p| p.id == id)
    }
}

impl PartialEq for Aux {
    fn eq(&self, o: &Aux) -> bool {
        self.patches.len() == o.patches.len() && self.patches.iter().zip(&o.patches).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl std::fmt::Debug for Aux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Aux {{ patches: {} }}", self.patches.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_round_trips() {
        let rgb: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
        let p = InpaintPatch::new([0.1, 0.2, 0.3, 0.4], 4, 3, rgb, (0..12).map(|i| i as u8 * 20).collect()).unwrap();
        assert_eq!(InpaintPatch::from_bytes(&p.to_bytes()).unwrap(), p);
        assert!(InpaintPatch::new([0.5, 0.5, 0.4, 0.6], 1, 1, vec![0; 3], vec![0]).is_none());
    }
}
