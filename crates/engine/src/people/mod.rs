//! People: recognize the faces of a library and tag the people in them (`lightcraft-people`,
//! cargo feature `people`; the desktop app and the CLI enable it).
//!
//! **Nothing requires the models.** The face finder is Google's MediaPipe Face Landmarker (the
//! `faces` feature's 3.8 MB file); the recognizer is InsightFace's ArcFace (174 MB for the
//! accurate ResNet-50, 14 MB for the fast MobileFaceNet). They are not part of LightCraft: the
//! user downloads them when they first ask and agree to their terms (`people.model.download`; the
//! InsightFace weights are for non-commercial research use), or puts the files in the model folder.
//! Without them `people.index` fails with a clear "not installed" error; every other feature never
//! touches them.
//!
//! What is stored, and where: next to the catalog, in `<library>/people/` — `faces-<model>.bin`
//! (one record per face: box, scores, 512-number embedding as 16-bit floats; append-only, keyed by
//! the photo's content key, so a changed file is analysed again and the catalog journal never
//! grows) and `people.json` (what the user decided: names, confirmations, "not this person").
//! Deleting the first loses nothing but work that can be redone; deleting the second loses the
//! user's decisions (the `People|<name>` keywords stay on the photos).
//!
//! Nothing is ever tagged without a yes: faces are *grouped* by similarity, the user names a group
//! (or says yes to "same person?") and only then the photos get the keyword `People|<name>`.

#[cfg(feature = "people")]
mod core;
#[cfg(feature = "people")]
pub(crate) mod ops;

mod views;

use std::path::PathBuf;

/// The keyword every person is filed under: `People|<name>`.
pub const KEYWORD_ROOT: &str = "People";
/// Environment variable naming the folder the models are in (`<folder>/<model id>/<file>`).
pub const DIR_ENV: &str = "LIGHTCRAFT_PEOPLE_DIR";
/// Environment variable with extra download locations (base URLs separated by commas or spaces).
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_PEOPLE_MIRRORS";
/// How errors about a missing model start (the UI offers the download on it).
pub const NOT_INSTALLED: &str = "The face recognition model is not installed";
/// What the build says when the feature is off.
pub const UNAVAILABLE: &str = "Face recognition is not available in this build";

/// The recognizer, the index and the indexing run of a session.
#[derive(Default)]
pub struct People {
    /// The folder of the models (`<folder>/<model id>/<file>`); else [`DIR_ENV`], else
    /// `<config>/models/people`.
    pub dir: Option<PathBuf>,
    /// The user's list of download mirrors (one base URL per line), if any.
    pub mirrors_file: Option<PathBuf>,
    /// Run in the background (the desktop app); otherwise commands wait for their results
    /// (CLI, MCP, tests).
    pub background: bool,
    /// Longest side of the picture faces are looked for in (default 1600 pixels).
    pub edge: Option<usize>,
    /// Recognize each face and its mirror image and average the two (a little more exact, twice
    /// the recognition time).
    pub flip: bool,
    /// An indexing run was started and its end has not been seen yet.
    pub(crate) was_running: bool,
    pub(crate) seen_rev: Option<u64>,
    #[cfg(feature = "people")]
    pub(crate) core: core::Core,
}

#[cfg(feature = "people")]
pub use ops::inject as inject_faces;
#[cfg(feature = "people")]
pub use ops::inject_keys as inject_faces_at;
pub use views::{GroupView, Likely, Overview, SuggestionView, Thumb, VideoAt};

impl People {
    /// Whether this build can recognize faces at all.
    pub const AVAILABLE: bool = cfg!(feature = "people");

    /// The folder of the models.
    pub fn model_folder(&self) -> Option<PathBuf> {
        self.dir
            .clone()
            .or_else(|| std::env::var_os(DIR_ENV).filter(|p| !p.is_empty()).map(PathBuf::from))
            .or_else(|| crate::camera_profiles::config_dir().map(|d| d.join("models").join("people")))
    }

    /// The model file (it may not exist).
    #[cfg(feature = "people")]
    pub fn model_file(&self, m: &lightcraft_people::ModelInfo) -> Option<PathBuf> {
        self.model_folder().map(|d| d.join(m.id).join(m.file_name))
    }

    /// Whether the model file is in place (right size; its SHA-256 is checked when it loads).
    #[cfg(feature = "people")]
    pub fn installed(&self, m: &lightcraft_people::ModelInfo) -> bool {
        Self::AVAILABLE && self.model_file(m).and_then(|f| std::fs::metadata(f).ok()).is_some_and(|x| x.is_file() && x.len() == m.size)
    }

    /// The download locations, in order: [`MIRRORS_ENV`], the mirrors file, then the model's
    /// published location.
    #[cfg(feature = "people")]
    pub fn mirrors(&self, m: &lightcraft_people::ModelInfo) -> Vec<String> {
        lightcraft_fetch::mirrors(std::env::var(MIRRORS_ENV).ok().as_deref(), self.mirrors_file.as_deref(), &[m.mirror])
    }

    /// The error for a missing model: what to do.
    #[cfg(feature = "people")]
    pub fn not_installed(&self, m: &lightcraft_people::ModelInfo) -> String {
        let d = self.core.download_status();
        if d.running {
            let pct = d.done.saturating_mul(100).checked_div(d.total).unwrap_or(0);
            return format!("{NOT_INSTALLED} yet: it is downloading ({pct} %).");
        }
        format!(
            "{NOT_INSTALLED}. Download it (about {:.0} MB; {}: {}) when LightCraft offers it, with `people.model.download {{\"acknowledged\": true}}`, or put {} in {}.",
            m.size as f64 / 1e6,
            m.authors,
            m.licence,
            m.file_name,
            self.model_folder().map(|f| f.join(m.id).display().to_string()).unwrap_or_default()
        )
    }
}

#[cfg(feature = "people")]
impl People {
    pub fn job_running(&self) -> bool {
        self.core.job_running()
    }

    pub fn computing(&self) -> bool {
        self.core.computing()
    }

    pub fn download_status(&self) -> crate::segment::DownloadStatus {
        self.core.download_status()
    }

    pub fn cancel_job(&self) -> bool {
        self.core.cancel_job()
    }

    pub fn cancel_download(&self) -> bool {
        self.core.cancel_download()
    }

    pub fn job_status(&self) -> core::JobStatus {
        self.core.job_status()
    }

    pub fn loaded(&self) -> bool {
        self.core.loaded()
    }

    pub fn unload(&self) -> bool {
        self.core.unload()
    }
}

impl crate::Session {
    /// Apply what background work finished (an indexing run, grouping, auto-tag). Cheap: call it
    /// every frame. True when the overview may have changed.
    pub fn people_poll(&mut self) -> bool {
        #[cfg(feature = "people")]
        return ops::poll(self);
        #[cfg(not(feature = "people"))]
        false
    }

    /// The People view's data: people, groups of faces to name, suggestions.
    pub fn people_overview(&mut self, groups: usize, suggestions: usize, thumbs: usize) -> Result<Overview, String> {
        #[cfg(feature = "people")]
        return ops::overview(self, groups, suggestions, thumbs).map_err(|e| e.to_string());
        #[cfg(not(feature = "people"))]
        {
            let _ = (groups, suggestions, thumbs);
            Err(UNAVAILABLE.into())
        }
    }
}

#[cfg(not(feature = "people"))]
impl People {
    pub fn job_running(&self) -> bool {
        false
    }

    pub fn computing(&self) -> bool {
        false
    }

    pub fn cancel_job(&self) -> bool {
        false
    }

    pub fn cancel_download(&self) -> bool {
        false
    }
}

impl crate::Session {
    /// Whether People has a decision to undo.
    pub fn can_undo_people(&self) -> bool {
        #[cfg(feature = "people")]
        return self.people.core.slot.try_lock().ok().and_then(|g| g.as_ref().map(|s| !s.history.is_empty())).unwrap_or(false);
        #[cfg(not(feature = "people"))]
        false
    }
}
