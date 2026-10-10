//! The recognizer: InsightFace's ArcFace network (the `w600k_r50` ResNet-50 of the `buffalo_l`
//! pack, or the lighter `w600k_mbf` MobileFaceNet of `buffalo_s`) run from its ONNX file by the
//! small pure-Rust ONNX interpreter of `lightcraft-inpaint` on candle's CPU backend. It maps a
//! 112 × 112 aligned face to a 512-number vector; two photos of one person give vectors with a
//! high cosine, photos of two people a low one.
//!
//! The weights are not part of LightCraft (InsightFace's models are for non-commercial research
//! use, see `docs/people.md`): the user downloads them when they ask for People.

use std::path::Path;

use candle_core::{Device, Tensor};
use lightcraft_inpaint::OnnxModel;

use crate::align::SIDE;
use crate::{DIM, Error, Result};

/// Faces run through the network together (the convolutions are matrix products: bigger batches
/// use the cores better).
const BATCH: usize = 8;

pub struct Recognizer {
    model: OnnxModel,
}

impl Recognizer {
    /// Read an ArcFace ONNX file.
    pub fn load(file: &Path) -> Result<Self> {
        let bytes = std::fs::read(file).map_err(|e| Error::Model(format!("{}: {e}", file.display())))?;
        let model = lightcraft_inpaint::load_onnx(&bytes, Device::Cpu).map_err(|e| Error::Model(e.to_string()))?;
        if model.inputs.len() != 1 {
            return Err(Error::Model("the recognizer takes one image".into()));
        }
        Ok(Self { model })
    }

    /// The unit embeddings of aligned crops (`3 × 112 × 112` floats each, see [`crate::align::crop`]).
    pub fn embed(&self, crops: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        let n = 3 * SIDE * SIDE;
        let mut out = Vec::with_capacity(crops.len());
        for chunk in crops.chunks(BATCH) {
            if chunk.iter().any(|c| c.len() != n) {
                return Err(Error::Model("a face crop has the wrong size".into()));
            }
            let flat: Vec<f32> = chunk.iter().flat_map(|c| c.iter().copied()).collect();
            let x = Tensor::from_vec(flat, (chunk.len(), 3, SIDE, SIDE), &Device::Cpu).map_err(|e| Error::Model(e.to_string()))?;
            let name = self.model.inputs[0].clone();
            let y = self.model.run(&[(name.as_str(), x)]).map_err(|e| Error::Model(e.to_string()))?;
            let v = y.flatten_all().and_then(|t| t.to_vec1::<f32>()).map_err(|e| Error::Model(e.to_string()))?;
            if v.len() != chunk.len() * DIM {
                return Err(Error::Model(format!("unexpected output of {} values", v.len())));
            }
            for e in v.as_chunks::<DIM>().0 {
                out.push(normalized(e));
            }
        }
        Ok(out)
    }
}

/// `v` scaled to unit length (a zero vector stays zero).
pub fn normalized(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n < 1e-12 { vec![0.0; v.len()] } else { v.iter().map(|x| x / n).collect() }
}

/// The crop mirrored left to right (the network is often run on both and the results averaged).
pub fn flipped(crop: &[f32]) -> Vec<f32> {
    let mut out = crop.to_vec();
    for row in out.as_chunks_mut::<SIDE>().0 {
        row.reverse();
    }
    out
}
