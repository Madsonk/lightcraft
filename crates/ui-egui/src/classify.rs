//! The Classify step (people, places, keywords, rename): to be rebuilt on the faces, geocode and
//! keyword commands of 0.14.0. Until then the step shows where it will be and nothing else.

use serde_json::{Value, json};

use crate::LightcraftApp;

#[derive(Clone, Debug, Default)]
pub struct State;

pub fn tick(_app: &mut LightcraftApp, _now: f64) {}

pub fn command(_app: &mut LightcraftApp, id: &str, _p: &Value) -> Result<Value, String> {
    Err(format!("{id}: the Classify step is not rebuilt on 0.14.0 yet"))
}

pub fn inspect(_app: &LightcraftApp) -> Value {
    json!({"ready": false})
}

pub fn show(_app: &mut LightcraftApp, ui: &mut egui::Ui) {
    ui.label(crate::i18n::tr("Classify"));
}
