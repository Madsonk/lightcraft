//! Export presets: built-in ones ([`crate::export::builtin_presets`]) and the user's, saved with the
//! library. `app.export {preset}` (desktop app, MCP, `lightcraft-cli run`) starts from a preset's
//! params; the call's own params override them (see [`Session::export_params`]).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::export::{ExportPreset, builtin_presets};
use crate::{Result, Session};

impl Session {
    /// Built-in presets followed by the user's.
    pub fn all_export_presets(&self) -> Vec<(ExportPreset, bool)> {
        builtin_presets().into_iter().map(|p| (p, true)).chain(self.export_presets.iter().cloned().map(|p| (p, false))).collect()
    }

    /// `app.export` params with a named `preset` expanded: the preset's params, overridden by the
    /// call's own (`preset` itself removed). Unknown preset names are an error.
    pub fn export_params(&self, p: &Value) -> std::result::Result<Value, String> {
        let guarded = self.guarded_params("app.export", p).map_err(|e| e.to_string())?;
        let p = &guarded;
        if p.get("id").is_some() || p.get("ids").is_some() {
            self.checked_targets("app.export", p).map_err(|e| e.to_string())?;
        }
        let Some(name) = p.get("preset").and_then(Value::as_str) else { return Ok(p.clone()) };
        let (preset, _) = self
            .all_export_presets()
            .into_iter()
            .find(|(x, _)| x.name.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| format!("unknown export preset `{name}` (see export.presets)"))?;
        // a preset saved by another version may hold keys this one doesn't know: not the caller's typo
        let mut out = crate::export::ExportOptions::known_keys_only(&preset.params);
        // Older presets could accidentally remember a single photo id. Targets belong to the
        // current call, never to reusable settings, even when loaded from an older library.
        if let Some(settings) = out.as_object_mut() {
            settings.retain(|key, _| !NOT_SETTINGS.contains(&key.as_str()));
        }
        if let (Some(o), Some(own)) = (out.as_object_mut(), p.as_object()) {
            for (k, v) in own.iter().filter(|(k, _)| *k != "preset") {
                o.insert(k.clone(), v.clone());
            }
        }
        Ok(out)
    }
}

fn list(s: &Session) -> Value {
    Value::Array(s.all_export_presets().into_iter().map(|(p, builtin)| json!({"name": p.name, "builtin": builtin, "params": p.params})).collect())
}

/// Params that describe a destination or a selection, not the export settings.
const NOT_SETTINGS: &[&str] = crate::export::TARGET_PARAMS;

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "export.savePreset";
    let name = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(ID, "missing `name`"))?;
    if builtin_presets().iter().any(|b| b.name.eq_ignore_ascii_case(name)) {
        return Err(bad(ID, format!("`{name}` is a built-in preset; choose another name")));
    }
    let mut params = match p.get("params") {
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(bad(ID, "`params` must be an object of app.export params")),
        None => s.last_export.clone().ok_or_else(|| bad(ID, "no `params` and no previous export to save"))?,
    };
    if let Some(o) = params.as_object_mut() {
        o.retain(|k, _| !NOT_SETTINGS.contains(&k.as_str()));
    }
    // a preset that app.export would refuse is refused here, where the typo is on screen
    crate::export::ExportOptions::validate(ID, &params)?;
    let preset = ExportPreset { name: name.to_string(), params };
    match s.export_presets.iter_mut().find(|x| x.name.eq_ignore_ascii_case(name)) {
        Some(x) => *x = preset,
        None => s.export_presets.push(preset),
    }
    s.save_prefs()?;
    Ok(list(s))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "export.deletePreset";
    let name = p.get("name").and_then(Value::as_str).ok_or_else(|| bad(ID, "missing `name`"))?;
    let before = s.export_presets.len();
    s.export_presets.retain(|x| !x.name.eq_ignore_ascii_case(name.trim()));
    if s.export_presets.len() == before {
        return Err(bad(ID, format!("no user export preset `{name}`")));
    }
    s.save_prefs()?;
    Ok(list(s))
}

/// Verify a saved export against the encoder's receipt. Never writes or repairs a file.
fn verify(_: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "export.verify";
    let path = p.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).ok_or_else(|| bad(C, "missing path"))?;
    let mut file = std::fs::File::open(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
    // Bounded allocation even if the file grows between metadata and read.
    const MAX: u64 = 128 * 1024 * 1024;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.by_ref().take(MAX + 1).read_to_end(&mut bytes).map_err(|e| bad(C, e.to_string()))?;
    if bytes.len() as u64 > MAX {
        return Err(bad(C, "verification supports files up to 128 MiB"));
    }
    let hash = lightcraft_preview::hash_bytes(&bytes).to_string();
    if let Some(n) = p.get("bytes") {
        if n.as_u64() != Some(bytes.len() as u64) {
            return Err(bad(C, "output byte count differs from receipt"));
        }
    }
    if let Some(expected) = p.get("contentHash") {
        if expected.as_str() != Some(hash.as_str()) {
            return Err(bad(C, "output content hash differs from receipt"));
        }
    }
    let header = lightcraft_codecs::read_header(&bytes).map_err(|e| bad(C, format!("invalid exported image: {e}")))?;
    for (key, actual) in [("width", header.width), ("height", header.height)] {
        if let Some(expected) = p.get(key) {
            if expected.as_u64() != Some(u64::from(actual)) {
                return Err(bad(C, format!("output {key} differs from receipt")));
            }
        }
    }
    // Validate compressed pixels too, at a bounded preview size (not just a valid header).
    lightcraft_codecs::decode(&bytes, lightcraft_codecs::DecodeOptions { max_size: Some((64, 64)), max_pixels: 100_000_000 })
        .map_err(|e| bad(C, format!("export cannot be decoded: {e}")))?;
    Ok(json!({"path": path, "verified": true, "bytes": bytes.len(), "contentHash": hash,
        "width": header.width, "height": header.height, "format": format!("{:?}", header.format), "decodeChecked": true}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "export.verify", "Verify Exported Image", [], None,
            "{path, bytes?, contentHash?, width?, height?} — read back a rendered image, check receipt and decode a bounded preview. No writes; max 128 MiB / 100 MP → {verified, path, bytes, contentHash, width, height, format, decodeChecked}; raw originals/DNG are not supported", always, verify),
        cmd!(
            "export.presets",
            "Export Presets",
            [],
            None,
            "{} → [{name, builtin, params}] (use with app.export {preset: name, …overrides})",
            always,
            |s, _| Ok(list(s))
        ),
        cmd!(
            "export.savePreset",
            "Save Export Preset",
            [],
            None,
            "{name, params?: app.export params (default: the last export's)} — adds or replaces a user preset, saved with the library → presets",
            always,
            save
        ),
        cmd!("export.deletePreset", "Delete Export Preset", [], None, "{name} — removes a user preset → presets", always, delete),
        cmd!(
            query "export.checkTarget",
            "Check Output Path",
            [],
            None,
            "{path} — fails when writing `path` would replace a photo's original (or its XMP sidecar) in the library; front ends call it before saving a render or screenshot to a user-given path → {path}",
            always,
            |s, p| {
                let path = p.get("path").and_then(Value::as_str).ok_or_else(|| bad("export.checkTarget", "missing `path`"))?;
                s.check_write_target(path).map_err(|e| bad("export.checkTarget", e))?;
                Ok(json!({"path": path}))
            }
        ),
    ]
}
