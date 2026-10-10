//! AI restore models: status, download, cancel (see `crate::restore` and `docs/ai-restore.md`).

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd};

#[cfg(feature = "restore")]
fn model_of(p: &Value, c: &str) -> super::Result<crate::restore::Model> {
    super::str_param(p, "model")
        .and_then(crate::restore::Model::parse)
        .ok_or_else(|| bad(c, "`model` must be \"denoise\" (SCUNet) or \"upscale\" (SPAN 2x)"))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "restore.model.status",
            "AI Restore Model Status",
            [],
            None,
            "{} — whether this build has AI denoise / super-resolution and, for each model, whether it is installed, its licence and download progress → {available, dir, models: [{id: denoise|upscale, title, file, sizeBytes, installed, licence, licenceUrl, sourceUrl, download: {running, done, total, finished, error}}]}",
            super::always,
            |_, _| {
                #[cfg(feature = "restore")]
                let models = crate::restore::models();
                #[cfg(not(feature = "restore"))]
                let models: Vec<Value> = Vec::new();
                Ok(json!({
                    "available": crate::restore::AVAILABLE,
                    "dir": crate::restore::models_dir().map(|d| d.display().to_string()),
                    "models": models,
                }))
            }
        ),
        cmd!(
            query "restore.model.download",
            "Download AI Restore Model",
            [],
            None,
            "{model: denoise|upscale, acknowledged: true} — download a model in the background (denoise: SCUNet, 72 MB, authors' release; upscale: SPAN 2x, 4.5 MB, CC BY 4.0), checked by size and SHA-256; only after the user agreed to its licence (see restore.model.status). Watch restore.model.status; restore.model.cancel stops it → {started, installed}",
            super::always,
            |_, p| {
                let c = "restore.model.download";
                #[cfg(feature = "restore")]
                {
                    let m = model_of(p, c)?;
                    let spec = m.spec();
                    if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                        return Err(bad(
                            c,
                            format!(
                                "the {} model is a {:.0} MB download under its own licence ({}; {}): pass `acknowledged: true` once the user has agreed",
                                spec.title,
                                spec.file.size.unwrap_or(0) as f64 / 1e6,
                                spec.licence,
                                spec.licence_url
                            ),
                        ));
                    }
                    let started = crate::restore::start_download(m).map_err(crate::EngineError::Other)?;
                    Ok(json!({"started": started, "installed": crate::restore::status(m)["installed"]}))
                }
                #[cfg(not(feature = "restore"))]
                {
                    let _ = p;
                    Err(bad(c, "AI denoise and super-resolution are not available in this build"))
                }
            }
        ),
        cmd!(
            query "restore.backend",
            "AI Denoise Backend",
            [],
            None,
            "{probe?: bool (true)} — what runs AI denoise on this machine: `torch` (an external Python with PyTorch on the GPU, ~9x faster on a 22 Mpx photo) or `rust` (candle). Chosen by LIGHTCRAFT_RESTORE_BACKEND=auto|rust|torch and LIGHTCRAFT_RESTORE_PYTHON; probing imports PyTorch once (a few seconds), `probe: false` only reports what is already known → {preference, denoiseBackend: torch|rust|unknown|none, python, torch}",
            super::always,
            |_, p| {
                #[cfg(feature = "restore")]
                {
                    Ok(crate::restore::denoise_backend(super::bool_or(p, "probe", true)))
                }
                #[cfg(not(feature = "restore"))]
                {
                    let _ = p;
                    Ok(json!({"preference": "auto", "denoiseBackend": "none", "python": null, "torch": null}))
                }
            }
        ),
        cmd!(
            query "restore.model.cancel",
            "Cancel AI Restore Model Download",
            [],
            None,
            "{model: denoise|upscale} — stop a model download (what has arrived is kept and resumed by a new download) → {cancelled}",
            super::always,
            |_, p| {
                #[cfg(feature = "restore")]
                {
                    let m = model_of(p, "restore.model.cancel")?;
                    Ok(json!({"cancelled": crate::restore::cancel_download(m)}))
                }
                #[cfg(not(feature = "restore"))]
                {
                    let _ = p;
                    Ok(json!({"cancelled": false}))
                }
            }
        ),
    ]
}
