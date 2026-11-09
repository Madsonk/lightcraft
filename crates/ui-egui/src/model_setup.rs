//! Resume a denoise request after model setup (stubbed during 0.19.0 port merge; re-integrate against v0.19 APIs).

use crate::LightcraftApp;
use serde_json::Value;

#[derive(Default)]
pub(crate) struct Pending {
    #[allow(dead_code)]
    next_check: f64,
}

#[allow(dead_code)]
pub(crate) fn denoise_requested(_app: &LightcraftApp, _id: lightcraft_catalog::PhotoId) -> bool {
    false
}

#[allow(dead_code)]
pub(crate) fn intercept(_app: &mut LightcraftApp, _command: &str, _params: &Value) -> Option<Result<Value, String>> {
    None
}

#[allow(dead_code)]
pub(crate) fn apply_if_ready(_app: &mut LightcraftApp) {}

#[allow(dead_code)]
pub(crate) fn open(_app: &mut LightcraftApp) {}
