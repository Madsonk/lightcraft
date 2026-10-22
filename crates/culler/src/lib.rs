//! AI culling for LightCraft: one **verdict** per photo of an album — highlight / selected /
//! to review / rejected — with the reasons (closed eyes, blurred, face blurred, duplicate, bad
//! exposure…) and a proposed star rating, in pure Rust.
//!
//! The crate does no I/O and decodes nothing: it judges [`Shot`]s, the measurements the engine
//! took (focus, clipping, exposure, a look-alike signature, the faces and their eyes), so it is
//! fast, deterministic and testable on synthetic albums ([`synthetic`]). It only ever *proposes*:
//! writing a rating or a flag is the engine's `cull.apply`, behind the user's yes.
//!
//! - [`profile`]: genre profiles (wedding, portrait, school, sport, general) and the options.
//! - [`scene`]: shooting scenes by capture-time gaps, split by appearance with k-means.
//! - [`dupes`]: duplicate groups (signature similarity + time, adjustable grouping).
//! - [`judge`]: everything together.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod dupes;
pub mod profile;
pub mod scene;
pub mod synthetic;
mod verdict;

pub use profile::{EyePolicy, Genre, Options, Profile};
pub use verdict::{BlurClass, Bucket, EyeClass, FaceVerdict, Reason, Severity, Verdict, judge};

/// Version of the judging rules; stored in every verdict so old ones can be told from new ones.
pub const VERSION: u32 = 1;

/// Length of a look-alike signature (8 × 8 luminance, zero mean, unit norm).
pub const SIGNATURE_LEN: usize = 64;

/// One face of a photo, as Analyze Faces measured it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Face {
    /// `[x0, y0, x1, y1]`, 0..1 of the upright photo.
    pub rect: [f32; 4],
    /// Eye aspect ratio of the image-left and image-right eye (≈ 0.3 open, ≈ 0 closed).
    pub open: [f32; 2],
    /// Sharpness 0..100 of the image-left and image-right eye.
    pub sharp: [f32; 2],
    /// Whether the eyes can be judged (a face of some size, not turned away).
    pub judged: bool,
    /// The person's name when the face is named (a face region or a recognized person).
    pub person: Option<String>,
}

/// What the engine measured on one photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shot {
    /// The caller's id (a photo id); returned nowhere, only for the caller's bookkeeping.
    pub id: u64,
    /// Width over height of the photo; 0 = unknown (taken as 1). Face boxes are fractions of each axis.
    pub aspect: f32,
    /// Capture time in seconds on any monotonic scale; `None` when unknown.
    pub time: Option<i64>,
    /// Focus 0..100 (detail of the sharpest areas).
    pub sharpness: f32,
    /// Share of clipped pixels 0..1.
    pub clipped: f32,
    /// `[mean, 5th percentile, 95th percentile]` of the display luminance, 0..1.
    pub exposure: [f32; 3],
    /// Look-alike signature ([`SIGNATURE_LEN`] floats); a shorter one just matches nothing.
    pub signature: Vec<f32>,
    /// Colour layout (chroma on a 4 × 4 grid, centred, unit norm); empty = unknown. Duplicates need both the
    /// luminance layout and the colour layout to agree: dark low-key shots differ only by colour.
    pub color: Vec<f32>,
    /// `None`: faces not analyzed yet. `Some(vec![])`: no face.
    pub faces: Option<Vec<Face>>,
    /// The taste model's score 0..100 when there is a trained one, blended in lightly.
    pub taste: Option<f32>,
}

/// Similarity of two signatures: 1 = the same picture, ≤ 0 = unrelated.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    if d.is_finite() { d.clamp(-1.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests;
