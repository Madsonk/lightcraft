//! Semantic photo search and near-duplicates (`lightcraft-search`, cargo feature `search`; the
//! desktop app and the CLI enable it).
//!
//! **Nothing requires the model.** SigLIP 2 (Google, Apache-2.0) is not part of LightCraft: the
//! user downloads it when they first ask for it and agree to its terms (`search.model.download`),
//! or puts the files in the model folder themselves. Without it, `search.index`, `search.semantic`
//! and `search.tag` fail with a clear "not installed" error; exact duplicates need no model at
//! all, and near-duplicates only need the perceptual hashes (`library.findDuplicates`).
//!
//! What is stored, and where: next to the catalog, in `<library>/search/` — the 16-bit vectors in
//! `vectors-<model>.bin` and the perceptual hashes in `hashes.bin`, both append-only files keyed by
//! the photo's content key (so a changed file is simply not found and is embedded again, and the
//! catalog journal never grows). A library that isn't on disk keeps them in memory.
//!
//! The indexing run is one worker thread: it decodes thumbnails in parallel, runs the image tower
//! in batches and appends each batch to the files, so it can be stopped (`search.cancel`) and
//! simply resumed by running `search.index` again.

#[cfg(feature = "search")]
mod core;
#[cfg(feature = "search")]
mod ops;
#[cfg(not(feature = "search"))]
mod ops {
    //! Without the feature the commands that need the model say so.
    use serde_json::Value;

    use crate::{EngineError, Result, Session};

    fn off() -> EngineError {
        EngineError::Other(super::UNAVAILABLE.into())
    }
    pub fn index(_: &mut Session, _: &Value) -> Result<Value> {
        Err(off())
    }
    pub fn semantic(_: &mut Session, _: &Value) -> Result<Value> {
        Err(off())
    }
    pub fn tag(_: &mut Session, _: &Value) -> Result<Value> {
        Err(off())
    }
    pub fn near(_: &mut Session, _: &[lightcraft_catalog::PhotoId], _: u32, _: f32) -> Result<Vec<Group>> {
        Err(off())
    }
    pub fn status(s: &Session) -> Value {
        serde_json::json!({"available": false, "installed": false, "dir": s.searcher.model_folder().map(|d| d.display().to_string())})
    }

    /// A set of near-identical photos.
    pub struct Group {
        pub ids: Vec<lightcraft_catalog::PhotoId>,
        pub distance: u32,
        pub cosine: Option<f32>,
        pub confirmed: bool,
    }
}

use std::path::PathBuf;

pub(crate) use ops::{index, near, semantic, status, tag};

/// The model shown to the user and downloaded.
pub const MODEL_NAME: &str = "SigLIP 2 base, patch 16, 224 px (Google)";
pub const MODEL_URL: &str = "https://huggingface.co/google/siglip2-base-patch16-224";
/// The weights' licence (not LightCraft's, though it permits the same uses): shown before downloading.
pub const LICENSE_NAME: &str = "Apache License 2.0";
pub const LICENSE_URL: &str = "https://www.apache.org/licenses/LICENSE-2.0";
/// Download size: `model.safetensors` (1.50 GB) + `tokenizer.json` (34 MB) + `preprocessor_config.json`.
pub const MODEL_BYTES: u64 = 1_500_800_904 + 34_363_039 + 394;
/// How errors about a missing model start (the UI offers the download on it).
pub const NOT_INSTALLED: &str = "The semantic search model is not installed";
/// Environment variable with the model folder (the apps' default is `<config>/models/search`).
pub const DIR_ENV: &str = "LIGHTCRAFT_SEARCH_DIR";
/// Environment variable with extra download mirrors (base URLs, comma or space separated).
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_SEARCH_MIRRORS";
/// What the build says when the feature is off.
pub const UNAVAILABLE: &str = "Semantic search is not available in this build";

/// The model, the index and the indexing run of a session.
#[derive(Default)]
pub struct Searcher {
    /// Where the SigLIP files live (set by the app; else `LIGHTCRAFT_SEARCH_DIR`).
    pub dir: Option<PathBuf>,
    /// The user's list of download mirrors (one base URL per line), if any.
    pub mirrors_file: Option<PathBuf>,
    #[cfg(feature = "search")]
    core: core::Core,
}

impl Searcher {
    /// Whether this build can search by meaning at all.
    pub const AVAILABLE: bool = cfg!(feature = "search");

    /// The model folder: the one set by the app, else `LIGHTCRAFT_SEARCH_DIR`.
    pub fn model_folder(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(|| std::env::var_os(DIR_ENV).filter(|p| !p.is_empty()).map(PathBuf::from))
    }

    /// Whether the model's files are in place.
    pub fn installed(&self) -> bool {
        #[cfg(feature = "search")]
        if let Some(dir) = self.model_folder() {
            return lightcraft_search::is_model_dir(&dir);
        }
        false
    }

    /// The model folder, if the model is there; otherwise an error that says what to do.
    pub fn model_dir(&self) -> Result<PathBuf, String> {
        if !Self::AVAILABLE {
            return Err(UNAVAILABLE.into());
        }
        let dir = self.model_folder().ok_or("no folder is set for the semantic search model")?;
        if self.installed() {
            return Ok(dir);
        }
        let d = self.download_status();
        if d.running {
            let pct = d.done.saturating_mul(100).checked_div(d.total).unwrap_or(0);
            return Err(format!("{NOT_INSTALLED} yet: it is downloading ({pct} %)."));
        }
        Err(format!(
            "{NOT_INSTALLED}. Download it (about {:.1} GB, {LICENSE_NAME}) when LightCraft offers it, with `search.model.download {{\"acknowledged\": true}}`, or put model.safetensors, tokenizer.json and preprocessor_config.json of {MODEL_URL} in {}.",
            MODEL_BYTES as f64 / 1e9,
            dir.display()
        ))
    }
}

#[cfg(not(feature = "search"))]
impl Searcher {
    pub fn mirrors(&self) -> Vec<String> {
        Vec::new()
    }
    pub fn download_status(&self) -> crate::segment::DownloadStatus {
        Default::default()
    }
    pub fn start_download(&self) -> Result<bool, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn cancel_download(&self) -> bool {
        false
    }
    pub fn loaded(&self) -> bool {
        false
    }
    pub fn unload(&self) -> bool {
        false
    }
    pub fn cancel_job(&self) -> bool {
        false
    }
}
