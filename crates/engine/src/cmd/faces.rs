//! Analyze Faces: find faces and score each eye's openness and sharpness (the model is
//! optional and downloaded on request; see [`crate::faces`] and `docs/faces.md`).

use lightcraft_catalog::{FaceInfo, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::faces::{Faces, LICENSE_NAME, LICENSE_URL, MODEL_BYTES};
use crate::media::SourceLevel;
use crate::{Result, Session};

fn round(v: f32, digits: i32) -> f64 {
    let k = 10f64.powi(digits);
    (f64::from(v) * k).round() / k
}

/// A face as commands report it.
fn face_json(f: &FaceInfo) -> Value {
    json!({
        "rect": f.rect.map(|v| round(v, 4)),
        "score": round(f.score, 3),
        "eyeOpen": f.open.map(|v| round(v, 3)),
        "eyeOpenScore": f.open.map(|v| round(FaceInfo::open_score(v), 1)),
        "eyeSharpness": f.sharp.map(|v| round(v, 1)),
        "turn": round(f.turn, 2),
        "eyesJudged": f.eyes_judged(),
        "eyesClosed": f.eyes_closed(),
    })
}

fn status_json(s: &Session) -> Value {
    let g = &s.faces;
    let p = g.progress();
    json!({
        "available": Faces::AVAILABLE,
        "installed": g.installed(),
        "file": g.model_file().map(|d| d.display().to_string()),
        "loaded": g.loaded(),
        "busy": g.busy(),
        "sizeBytes": MODEL_BYTES,
        "license": LICENSE_NAME,
        "licenseUrl": LICENSE_URL,
        "mirrors": g.mirrors().len(),
        "download": g.download_status(),
        "run": {"total": p.total, "done": p.done, "failed": p.failed},
    })
}

fn analyze_faces(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.analyzeFaces";
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { s.visible_cloned() };
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let wait = p.get("wait").and_then(Value::as_bool);
    if let Some(l) = p.get("level").and_then(Value::as_str) {
        s.faces.level = match l {
            "thumb" => SourceLevel::Thumb,
            "preview" => SourceLevel::Preview,
            "full" => SourceLevel::Full,
            _ => return Err(bad(C, "level must be thumb|preview|full")),
        };
    }
    let (queued, skipped) = s.faces_enqueue(&ids, force).map_err(|e| bad(C, e))?;
    if s.faces.background && wait != Some(true) {
        return Ok(json!({"queued": queued, "alreadyAnalyzed": skipped, "background": true}));
    }
    let rows = s.faces_wait(queued);
    let (mut photos, mut failed) = (Vec::new(), Vec::new());
    for r in &rows {
        match &r.result {
            Ok(ok) => photos.push(json!({"id": r.id.0, "faces": ok.faces, "eyesClosed": ok.eyes_closed, "ms": round(ok.ms as f32, 0)})),
            Err(e) => failed.push(json!([r.id.0, e])),
        }
    }
    Ok(json!({"analyzed": photos.len(), "alreadyAnalyzed": skipped, "photos": photos, "failed": failed, "background": false}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.analyzeFaces",
            "Analyze Faces",
            ["Photo"],
            None,
            "{ids?, force?: bool, level?: thumb|preview|full (preview), wait?: bool} — find the faces of the selected photos (else all in view) and score each eye's openness and sharpness; skips photos already analyzed unless `force`. Needs the face model (faces.model.status / faces.model.download). In the app it runs in the background and returns {queued} (watch faces.status; running it again resumes with the photos not analyzed yet); the CLI and MCP wait (or pass wait: true) → {analyzed, alreadyAnalyzed, photos: [{id, faces, eyesClosed, ms}], failed}. Not an undo step",
            always,
            analyze_faces
        ),
        cmd!(
            query "photo.faces",
            "Photo Faces",
            [],
            None,
            "{id?} — the faces found in a photo: [{rect, score, eyeOpen: [left, right] (eye aspect ratio), eyeOpenScore, eyeSharpness, turn, eyesJudged, eyesClosed}], or null before Analyze Faces ran → {faces}",
            super::has_active,
            |s, p| {
                let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad("photo.faces", "no photo"))?;
                let ph = s.catalog.photo(id).ok_or_else(|| bad("photo.faces", "no such photo"))?;
                Ok(json!({"id": id.0, "faces": ph.faces().map(|f| f.iter().map(face_json).collect::<Vec<_>>())}))
            }
        ),
        cmd!(
            query "faces.status",
            "Analyze Faces Progress",
            [],
            None,
            "{} — the model (installed, loaded, download) and the current run → {available, installed, file, loaded, busy, sizeBytes, license, licenseUrl, mirrors, download: {running, done, total, file, error, finished}, run: {total, done, failed}}",
            always,
            |s, _| Ok(status_json(s))
        ),
        cmd!(
            query "faces.cancel",
            "Cancel Analyze Faces",
            [],
            None,
            "{} — drop the photos still queued (what was analyzed is kept)",
            always,
            |s, _| {
                s.faces.cancel();
                Ok(json!({"cancelled": true}))
            }
        ),
        cmd!(
            query "faces.model.status",
            "Face Model Status",
            [],
            None,
            "{} — same as faces.status",
            always,
            |s, _| Ok(status_json(s))
        ),
        cmd!(
            query "faces.model.download",
            "Download Face Model",
            [],
            None,
            "{acknowledged: true} — download the face model (MediaPipe Face Landmarker, 3.8 MB, Apache-2.0 by Google; not part of LightCraft) in the background from Google's model storage (or LIGHTCRAFT_FACES_MIRRORS); only after the user agreed to it. The file is checked against its published size and SHA-256. Watch faces.status; faces.model.cancel stops it → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "faces.model.download";
                if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        c,
                        format!(
                            "the face model is a {:.1} MB download from Google under the {} licence ({}): pass `acknowledged: true` once the user has agreed to download it",
                            MODEL_BYTES as f64 / 1e6,
                            LICENSE_NAME,
                            LICENSE_URL
                        ),
                    ));
                }
                let started = s.faces.start_download().map_err(|e| bad(c, e))?;
                Ok(json!({"started": started, "installed": s.faces.installed(), "downloading": s.faces.download_status().running}))
            }
        ),
        cmd!(
            query "faces.model.cancel",
            "Cancel Face Model Download",
            [],
            None,
            "{} — stop the face model download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.faces.cancel_download()}))
        ),
    ]
}
