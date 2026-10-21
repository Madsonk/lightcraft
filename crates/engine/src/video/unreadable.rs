//! Videos an import could not read (an empty file, a transfer cut short whose index — the `moov`
//! box — never got written, a file with no picture track): remembered in
//! `<library>/video/unreadable.json` so the Videos view can list them with their cause and the
//! folder they are in, instead of an error that scrolls away. A file that imports later leaves the
//! list. Nothing is repaired: a missing `moov` can only be rebuilt from a sound recording of the same
//! camera (tools like untrunc), never invented, so LightCraft only says so.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The list, in `<library>/video/`.
pub const FILE: &str = "unreadable.json";
/// Most files remembered.
const MAX: usize = 5000;

/// A video that could not be read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    /// `empty` (0 bytes), `noIndex` (no `moov`: cut short), `noPicture` (no video track),
    /// `missing` (the file is gone), `other`.
    pub cause: String,
    /// What the reader said.
    pub error: String,
    #[serde(default)]
    pub bytes: u64,
}

/// The cause of `error` for a file of `bytes`.
pub fn cause(error: &str, bytes: Option<u64>) -> &'static str {
    let e = error.to_ascii_lowercase();
    if bytes == Some(0) || e.contains("0 bytes") {
        "empty"
    } else if e.contains("moov atom not found") || e.contains("moov") {
        "noIndex"
    } else if e.contains("no video track") {
        "noPicture"
    } else if e.contains("no such file") || e.contains("not found") {
        "missing"
    } else {
        "other"
    }
}

pub fn load(root: &Path) -> Vec<Entry> {
    std::fs::read(root.join(FILE)).ok().and_then(|b| serde_json::from_slice::<Vec<Entry>>(&b).ok()).unwrap_or_default()
}

fn save(root: &Path, list: &[Entry]) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let tmp = root.join(format!("{FILE}.part"));
    let bytes = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, root.join(FILE)).map_err(|e| format!("{}: {e}", root.display()))
}

/// After an import: its videos that failed join the list, the files it catalogued leave it.
pub fn note_import(s: &crate::Session, failed: &[(String, String)], imported: &[u64]) {
    let Some(root) = super::cache_root(s) else { return };
    let videos: Vec<&(String, String)> = failed.iter().filter(|(p, _)| super::is_video_path(p)).collect();
    let done: Vec<String> = imported
        .iter()
        .filter_map(|id| s.catalog.photo(lightcraft_catalog::PhotoId(*id)))
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    if videos.is_empty() && (done.is_empty() || !root.join(FILE).is_file()) {
        return;
    }
    let mut list = load(&root);
    let before = list.clone();
    list.retain(|e| !done.contains(&e.path) && !videos.iter().any(|(p, _)| *p == e.path));
    for (path, error) in videos {
        let bytes = std::fs::metadata(path).ok().map(|m| m.len());
        list.push(Entry { path: path.clone(), cause: cause(error, bytes).into(), error: error.clone(), bytes: bytes.unwrap_or(0) });
    }
    list.truncate(MAX);
    if list != before
        && let Err(e) = save(&root, &list)
    {
        log::warn!("unreadable videos: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::cause;

    #[test]
    fn causes() {
        assert_eq!(cause("empty file (0 bytes)", Some(0)), "empty");
        assert_eq!(cause("not a readable video: [mov,mp4 @ 0x1] moov atom not found", Some(256_901_120)), "noIndex");
        assert_eq!(cause("the file has no video track", Some(10)), "noPicture");
        assert_eq!(cause("weird", None), "other");
    }
}
