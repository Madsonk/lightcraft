//! The portrait retouch stack: an ordered list of named steps (sliders such as *Blemishes* or
//! *Dark circles*), each with an amount, kept with the photo's settings. The steps are the
//! description of the retouch; the pixels they made live in an inpaint patch (see
//! [`crate::InpaintPatch`]) that a normal spot points at, so the original is never touched and
//! rendering never needs a model. Re-running the stack replaces the patch.

use serde::{Deserialize, Serialize};

/// One step of the stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortraitStep {
    /// Slider id (`blemishes`, `dark_circles`, `background`…; see `portrait.list`).
    pub kind: String,
    /// 0..100.
    pub amount: f64,
    /// A disabled step stays in the history but is not rendered.
    pub enabled: bool,
    /// Step options: `mode` (background: blur, color, dim), `color` (hex), `shadow`, `zoom`,
    /// `rotate`, `edge`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<(String, f64)>,
    /// A text option (background colour as `#rrggbb`, background mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// What the step changed when it was last rendered, as a share (0..1) of the face's pixels
    /// whose colour moved by a visible amount (a number to quote, not a quality score).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<f64>,
}

impl Default for PortraitStep {
    fn default() -> Self {
        Self { kind: String::new(), amount: 0.0, enabled: true, options: Vec::new(), text: None, changed: None }
    }
}

impl PortraitStep {
    pub fn option(&self, key: &str) -> Option<f64> {
        self.options.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortraitStack {
    pub steps: Vec<PortraitStep>,
    /// Hex id of the baked pixels (the same id as the spot that places them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Faces the stack was rendered for.
    pub faces: usize,
    /// Fingerprint of the settings the pixels were made from (white balance, geometry, lens
    /// correction…); when it no longer matches, the patch is out of date and the stack should be
    /// run again.
    #[serde(skip_serializing_if = "is_zero")]
    pub key: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl PortraitStack {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.patch.is_none()
    }

    pub fn step(&self, kind: &str) -> Option<&PortraitStep> {
        self.steps.iter().find(|s| s.kind == kind)
    }
}
