//! Catalog records.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PhotoId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AlbumId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StackId(pub u64);

/// A stack: photos grouped under a top photo (bursts, brackets, merge results with their sources).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub id: StackId,
    /// Members in stack order; `photos[0]` is the top of the stack. At least two.
    pub photos: Vec<PhotoId>,
    /// Collapsed stacks show only their top photo in the grid.
    #[serde(default)]
    pub collapsed: bool,
}

impl Stack {
    pub fn top(&self) -> PhotoId {
        self.photos[0]
    }
    pub fn position(&self, id: PhotoId) -> Option<usize> {
        self.photos.iter().position(|p| *p == id)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    #[default]
    Image,
    Raw,
    Video,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

impl Flag {
    pub fn parse(s: &str) -> Option<Flag> {
        match s.to_ascii_lowercase().as_str() {
            "pick" | "picked" | "flagged" => Some(Flag::Pick),
            "reject" | "rejected" => Some(Flag::Reject),
            "none" | "unflagged" | "" => Some(Flag::None),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorLabel {
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl ColorLabel {
    pub const ALL: [ColorLabel; 5] = [ColorLabel::Red, ColorLabel::Yellow, ColorLabel::Green, ColorLabel::Blue, ColorLabel::Purple];
    pub fn parse(s: &str) -> Option<ColorLabel> {
        ColorLabel::ALL.into_iter().find(|c| format!("{c:?}").eq_ignore_ascii_case(s))
    }
}

/// Where the pixels come from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// A file on disk (native) or in the browser's storage (web).
    File { path: String },
    /// A procedurally generated demo scene (by `lightcraft-scenes` id).
    Demo { scene: u32 },
}

/// Copyright status (IPTC / XMP Rights Management `xmpRights:Marked`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CopyrightStatus {
    /// Not stated (no `xmpRights:Marked`).
    #[default]
    Unknown,
    /// `xmpRights:Marked` = True.
    Copyrighted,
    /// `xmpRights:Marked` = False.
    PublicDomain,
}

impl CopyrightStatus {
    pub const ALL: [CopyrightStatus; 3] = [CopyrightStatus::Unknown, CopyrightStatus::Copyrighted, CopyrightStatus::PublicDomain];
    /// `unknown` / `copyrighted` / `publicDomain` (also `public domain`, `public-domain`).
    pub fn parse(s: &str) -> Option<CopyrightStatus> {
        match s.trim().to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "unknown" | "" => Some(CopyrightStatus::Unknown),
            "copyrighted" => Some(CopyrightStatus::Copyrighted),
            "publicdomain" => Some(CopyrightStatus::PublicDomain),
            _ => None,
        }
    }
    /// The id used by commands (`photo.setMeta`'s `copyrightStatus`).
    pub fn id(self) -> &'static str {
        match self {
            CopyrightStatus::Unknown => "unknown",
            CopyrightStatus::Copyrighted => "copyrighted",
            CopyrightStatus::PublicDomain => "publicDomain",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CopyrightStatus::Unknown => "Unknown",
            CopyrightStatus::Copyrighted => "Copyrighted",
            CopyrightStatus::PublicDomain => "Public Domain",
        }
    }
    /// As `xmpRights:Marked`: `Some(true)` copyrighted, `Some(false)` public domain.
    pub fn marked(self) -> Option<bool> {
        match self {
            CopyrightStatus::Unknown => None,
            CopyrightStatus::Copyrighted => Some(true),
            CopyrightStatus::PublicDomain => Some(false),
        }
    }
    pub fn from_marked(m: Option<bool>) -> CopyrightStatus {
        match m {
            None => CopyrightStatus::Unknown,
            Some(true) => CopyrightStatus::Copyrighted,
            Some(false) => CopyrightStatus::PublicDomain,
        }
    }
    pub fn is_unknown(&self) -> bool {
        *self == CopyrightStatus::Unknown
    }
}

/// Descriptive + capture metadata.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub camera: String,
    pub lens: String,
    pub focal_mm: Option<f32>,
    pub aperture: Option<f32>,
    /// What lens-profile recognition needs beyond the names above (all optional, absent in older catalogs):
    /// the lens maker, its focal range and widest aperture (`[min mm, max mm, f at min, f at max]`), the
    /// 35 mm equivalent focal length and the subject distance in metres.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lens_make: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_spec: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_max_aperture: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_35mm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_distance: Option<f32>,
    pub shutter: String,
    pub iso: Option<u32>,
    /// Place (sublocation, e.g. a landmark).
    pub location: String,
    pub city: String,
    pub state: String,
    pub country: String,
    pub gps: Option<(f64, f64)>,
    pub title: String,
    pub caption: String,
    /// Accessibility text and long description.
    pub alt_text: String,
    pub extended_description: String,
    pub copyright: String,
    /// Copyright status, rights usage terms and copyright info URL (IPTC Core rights fields).
    #[serde(skip_serializing_if = "CopyrightStatus::is_unknown")]
    pub copyright_status: CopyrightStatus,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub usage_terms: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub copyright_url: String,
    pub creator: String,
    pub keywords: Vec<String>,
    /// Face/pet/focus regions read from XMP (MWG-RS), on the upright (EXIF-oriented) photo.
    /// Removing or resizing one edits the catalog only; LightCraft never writes regions to XMP.
    /// Left out of the catalog JSON when empty (most photos), so older catalogs read unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<lightcraft_meta::Region>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub name: String,
    pub created: String,
    pub settings: Arc<DevelopSettings>,
    #[serde(default)]
    pub auto: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryStep {
    pub label: String,
    pub settings: Arc<DevelopSettings>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Photo {
    pub id: PhotoId,
    pub source: Source,
    pub file_name: String,
    #[serde(default)]
    pub kind: MediaKind,
    pub format: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub file_size: u64,
    /// ISO 8601 local capture time.
    #[serde(default)]
    pub captured: Option<String>,
    pub imported: String,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub rating: u8,
    #[serde(default)]
    pub flag: Flag,
    #[serde(default)]
    pub label: Option<ColorLabel>,
    pub develop: Arc<DevelopSettings>,
    #[serde(default)]
    pub edited: Option<String>,
    #[serde(default)]
    pub versions: Vec<Version>,
    #[serde(default)]
    pub history: Vec<HistoryStep>,
    /// In "Recently Deleted".
    #[serde(default)]
    pub deleted: bool,
    /// Seen while browsing a folder on disk (Local), not added to the library: only folder views
    /// show it until it is added.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// Video duration in seconds.
    #[serde(default)]
    pub duration: Option<f64>,
    /// Raw files: the camera's as-shot white balance (Kelvin, tint).
    #[serde(default)]
    pub as_shot_wb: Option<(f64, f64)>,
    /// Hash of the file's bytes (hex), for duplicate detection and preview-cache keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Lens corrections embedded in the file (DNG `WarpRectilinear` / `FixVignetteRadial`), applied when
    /// "Enable Profile Corrections" is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_lens: Option<lightcraft_develop::EmbeddedLens>,
    /// A virtual copy: the photo it was copied from (it shares that photo's file but has its own
    /// settings, metadata and history).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_of: Option<PhotoId>,
    /// Virtual copies' name ("Copy 1").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_name: Option<String>,
    /// The settings import gave the photo when a user default (a raw/JPEG default preset, see
    /// the engine's import defaults) changed them from [`Photo::camera_defaults`]. They count as
    /// unedited, and Reset returns to them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_look: Option<Arc<DevelopSettings>>,
    /// Assisted culling scores (`None` until analysed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<Analysis>,
    /// A raw file whose sensor data can't be decoded yet (an unsupported raw variant): why (the
    /// raw decoder's reason, e.g. "Nikon Huffman-compressed NEF …"). The photo is shown and
    /// edited from the camera's embedded JPEG preview — a rendered image with the camera's
    /// picture style baked in — so it is treated as a rendered (non-raw) source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_only: Option<String>,
    /// A Local record: the fingerprint of its state when the browse catalogued it, to tell
    /// whether the user changed anything since (see [`crate::local`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_baseline: Option<u64>,
}

/// A face found in a photo by Analyze Faces (the face model in `lightcraft-faces`). Coordinates are
/// fractions of the upright photo (EXIF orientation applied, before the user's Rotate Left/Right
/// and crop), like the XMP regions in [`Photo::regions`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FaceInfo {
    /// The face's box `[x0, y0, x1, y1]`, 0..1.
    pub rect: [f32; 4],
    /// The model's confidence that this is a face, 0..1.
    pub score: f32,
    /// Eye aspect ratio (lid gap over eye width) of the image-left and image-right eye: ≈ 0.3 open,
    /// near 0 closed.
    pub open: [f32; 2],
    /// Sharpness of the image-left and image-right eye, 0..100 (variance of the Laplacian on a
    /// normalized crop of the eye).
    pub sharp: [f32; 2],
    /// Head turn, −1 .. 1 (0 = facing the camera).
    pub turn: f32,
}

/// Eye aspect ratio below which an eye counts as closed (calibrated on real photos: see
/// `docs/faces.md`).
pub const EYE_CLOSED_BELOW: f32 = 0.15;
/// Eye aspect ratio from which an eye counts as fully open (openness score 100).
pub const EYE_OPEN_FROM: f32 = 0.24;
/// Faces turned further than this (|turn|) are not judged: the far eye is hidden.
pub const MAX_TURN: f32 = 0.55;
/// Faces narrower than this share of the photo's long edge are not judged (too small to see the eyes).
pub const MIN_FACE_SHARE: f32 = 0.025;

impl FaceInfo {
    /// Whether the eyes can be judged: a face of some size, not turned away.
    pub fn eyes_judged(&self) -> bool {
        self.turn.abs() <= MAX_TURN && (self.rect[2] - self.rect[0]).max(self.rect[3] - self.rect[1]) >= MIN_FACE_SHARE
    }

    /// Openness 0 (closed) .. 100 (wide open) of an eye, from its eye aspect ratio: 50 at the
    /// closed threshold ([`EYE_CLOSED_BELOW`]), 100 from [`EYE_OPEN_FROM`].
    pub fn open_score(ear: f32) -> f32 {
        if ear < EYE_CLOSED_BELOW {
            (50.0 * ear / EYE_CLOSED_BELOW).max(0.0)
        } else {
            (50.0 + 50.0 * (ear - EYE_CLOSED_BELOW) / (EYE_OPEN_FROM - EYE_CLOSED_BELOW)).min(100.0)
        }
    }

    /// Whether at least one eye is closed (only for faces whose eyes can be judged).
    pub fn eyes_closed(&self) -> bool {
        self.eyes_judged() && self.open[0].min(self.open[1]) < EYE_CLOSED_BELOW
    }

    /// How good the face is for choosing between similar shots, 0..100: both eyes open and sharp.
    /// The less open and the less sharp of the two eyes count (a wink or a blurred eye spoils it).
    pub fn quality(&self) -> f32 {
        let open = Self::open_score(self.open[0].min(self.open[1]));
        let sharp = self.sharp[0].min(self.sharp[1]);
        0.7 * open + 0.3 * sharp
    }
}

/// What assisted culling measured on a photo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    /// The faces found in the photo: `None` until Analyze Faces ran on it, `Some` of an empty list
    /// when it has none. Kept in the analysis (not as a new kind of change) so that older
    /// LightCraft builds, which ignore the field, still read the library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Arc<Vec<FaceInfo>>>,
    /// Focus: 0 (blurred) .. 100 (crisp), from the detail in the photo's sharpest area.
    pub sharpness: f32,
    /// Exposure: share of clipped pixels, 0..1 (shadows + highlights).
    pub clipped: f32,
    /// Similar-shot group (burst), when the photo has neighbours that look alike.
    pub group: Option<u32>,
    /// The best photo of its group.
    pub best: bool,
}

impl Photo {
    /// The faces Analyze Faces found (`None` until it ran on this photo).
    pub fn faces(&self) -> Option<&[FaceInfo]> {
        self.analysis.as_ref().and_then(|a| a.faces.as_deref()).map(Vec::as_slice)
    }

    /// Whether some face whose eyes can be judged has a closed eye.
    pub fn has_closed_eyes(&self) -> bool {
        self.faces().is_some_and(|f| f.iter().any(FaceInfo::eyes_closed))
    }

    /// Whether the photo has faces whose eyes can be judged and all of them are open.
    pub fn has_open_eyes(&self) -> bool {
        self.faces().is_some_and(|f| f.iter().any(FaceInfo::eyes_judged) && !f.iter().any(FaceInfo::eyes_closed))
    }

    pub fn new(id: PhotoId, source: Source, file_name: &str, format: &str, width: u32, height: u32, imported: &str) -> Photo {
        Photo {
            id,
            source,
            file_name: file_name.to_string(),
            kind: MediaKind::Image,
            format: format.to_string(),
            width,
            height,
            file_size: 0,
            captured: None,
            imported: imported.to_string(),
            meta: Meta::default(),
            rating: 0,
            flag: Flag::None,
            label: None,
            develop: Arc::new(DevelopSettings::default()),
            edited: None,
            versions: Vec::new(),
            history: Vec::new(),
            deleted: false,
            local: false,
            duration: None,
            as_shot_wb: None,
            content_hash: None,
            embedded_lens: None,
            copy_of: None,
            copy_name: None,
            import_look: None,
            analysis: None,
            preview_only: None,
            local_baseline: None,
        }
    }
    /// A raw file developed from its sensor data: not a rendered image, and not a raw shown from
    /// its embedded preview ([`Photo::preview_only`]).
    pub fn develops_raw(&self) -> bool {
        self.kind == MediaKind::Raw && self.preview_only.is_none()
    }
    /// The current ARW, NEF, RW2 and CR2 readers have vendor WB multipliers but no measured camera
    /// illuminant. Use adjustments relative to the camera's as-shot look, as for rendered
    /// photographs (the engine's `camera_preview::file_local_look` covers the same formats; RWL and
    /// RAW are Leica's and the oldest Panasonic bodies' names for RW2 files).
    pub fn relative_wb(&self) -> bool {
        self.develops_raw() && ["ARW", "NEF", "NRW", "RW2", "RWL", "RAW", "CR2"].iter().any(|f| self.format.eq_ignore_ascii_case(f))
    }
    /// The develop settings import gave this photo: [`Photo::camera_defaults`], or the user's
    /// default preset applied on top of them ([`Photo::import_look`]).
    pub fn import_defaults(&self) -> DevelopSettings {
        match &self.import_look {
            Some(l) => (**l).clone(),
            None => self.camera_defaults(),
        }
    }
    /// The built-in defaults for this photo, before any user default preset: raws start from
    /// their as-shot white balance; embedded lens corrections on when the file has them.
    pub fn camera_defaults(&self) -> DevelopSettings {
        let wb = if self.relative_wb() { Some((6500.0, 0.0)) } else { self.as_shot_wb };
        let mut d = match wb {
            Some((t, tint)) => DevelopSettings::for_raw(t, tint),
            None => DevelopSettings::default(),
        };
        if self.embedded_lens.is_some() || (self.kind == MediaKind::Raw && self.database_lens().is_some_and(|r| r.detection.has_corrections())) {
            d.optics.lens_profile = true;
        }
        d
    }

    /// The camera and lens this photo's metadata names, as a lens-profile lookup.
    pub fn lens_query(&self) -> lightcraft_lens::Query {
        let m = &self.meta;
        // `camera` is "<make> <model>" (the model usually repeats the make: "Canon Canon EOS 5D Mark III")
        let (make, model) = m.camera.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((String::new(), m.camera.clone()));
        let f = |v: Option<f32>| v.map(f64::from).filter(|x| *x > 0.0);
        lightcraft_lens::Query {
            camera_make: make,
            camera_model: model,
            lens_make: m.lens_make.clone(),
            lens_model: m.lens.clone(),
            lens_spec: m.lens_spec.map(|s| s.map(f64::from)),
            lens_max_aperture: f(m.lens_max_aperture),
            focal: f(m.focal_mm),
            focal_35: f(m.focal_35mm),
            aperture: f(m.aperture),
            distance: f(m.subject_distance),
        }
    }

    /// The lens-profile database's answer for this photo (camera + lens recognised, corrections found), `None` when
    /// no database is installed or the photo carries no camera/lens information.
    pub fn database_lens(&self) -> Option<std::sync::Arc<lightcraft_lens::Resolved>> {
        if self.meta.camera.is_empty() && self.meta.lens.is_empty() {
            return None;
        }
        lightcraft_lens::resolve(&self.lens_query())
    }

    /// The lens corrections to apply: the ones embedded in the file (DNG opcodes) when it has them, else the
    /// database profile of the recognised lens, else none.
    pub fn effective_lens(&self) -> Option<lightcraft_develop::EmbeddedLens> {
        if self.embedded_lens.is_some() {
            return self.embedded_lens;
        }
        let r = self.database_lens()?;
        embedded_from_correction(r.correction.as_ref()?, f64::from(self.width.max(1)), f64::from(self.height.max(1)))
    }
    /// In the library: not deleted and not only browsed (Local).
    pub fn in_library(&self) -> bool {
        !self.deleted && !self.local
    }
    /// Edited by the user: settings differ from what import gave the photo.
    pub fn is_edited(&self) -> bool {
        *self.develop != self.import_defaults() && !self.develop.is_unedited()
    }
    /// Capture time if known, else import time (sort key).
    pub fn date(&self) -> &str {
        self.captured.as_deref().unwrap_or(&self.imported)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: AlbumId,
    pub name: String,
    /// Containing folder (an album with `folder = true`).
    #[serde(default)]
    pub parent: Option<AlbumId>,
    /// Folders hold albums, not photos.
    #[serde(default)]
    pub folder: bool,
    #[serde(default)]
    pub photos: Vec<PhotoId>,
    #[serde(default)]
    pub cover: Option<PhotoId>,
    /// A smart album: its photos are every (non-deleted) photo matching these rules, evaluated
    /// live; `photos` stays empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<Box<crate::Filter>>,
    /// The Quick Collection: a regular album that B adds to (one per library).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quick: bool,
}

impl Album {
    /// A regular (manual) album.
    pub fn new(id: AlbumId, name: impl Into<String>) -> Album {
        Album { id, name: name.into(), parent: None, folder: false, photos: Vec::new(), cover: None, smart: None, quick: false }
    }
    pub fn is_smart(&self) -> bool {
        self.smart.is_some()
    }
}

/// The pipeline's lens data for a database correction on a `w x h` (oriented) image: the radius is the image's
/// half-diagonal, the centre the image centre.
pub fn embedded_from_correction(c: &lightcraft_lens::LensCorrection, w: f64, h: f64) -> Option<lightcraft_develop::EmbeddedLens> {
    use lightcraft_develop::{EmbeddedLens, EmbeddedVignette, EmbeddedWarp};
    let long = w.max(h);
    if !(long.is_finite() && long > 1.0) {
        return None;
    }
    let radius = (w.hypot(h) / 2.0) / long;
    let center = lightcraft_geom::Point::new(0.5, 0.5);
    let warp = c.planes.map(|p| EmbeddedWarp { planes: p.map(|k| [k[0], k[1], k[2], k[3], 0.0, 0.0]), center, radius });
    let vignette = c.vignette.map(|k| EmbeddedVignette { k, center, radius });
    (warp.is_some() || vignette.is_some()).then_some(EmbeddedLens { warp, vignette })
}

#[cfg(test)]
mod edited_tests {
    use super::*;

    #[test]
    fn raw_with_import_settings_is_not_edited() {
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.arw", "ARW", 10, 10, "2026-10-01T00:00:00");
        p.as_shot_wb = Some((5200.0, 4.0));
        p.develop = Arc::new(p.import_defaults());
        assert!(!p.is_edited(), "import look counts as unedited");
        let mut d = (*p.develop).clone();
        d.light.exposure = 0.5;
        p.develop = Arc::new(d);
        assert!(p.is_edited());
        p.develop = Arc::new(DevelopSettings::default());
        assert!(!p.is_edited(), "a full reset is unedited too");
    }

    #[test]
    fn sony_and_nikon_raws_use_relative_white_balance() {
        for (name, format, relative) in [
            ("a.arw", "ARW", true),
            ("a.nef", "NEF", true),
            ("a.nrw", "nrw", true),
            ("a.rw2", "RW2", true),
            ("a.rwl", "RWL", true),
            ("a.raw", "RAW", true),
            ("a.dng", "DNG", false),
        ] {
            let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, name, format, 10, 10, "2026-10-01T00:00:00");
            p.kind = MediaKind::Raw;
            p.as_shot_wb = Some((5200.0, 4.0));
            assert_eq!(p.relative_wb(), relative, "{format}");
            let wb = p.camera_defaults().wb;
            assert_eq!((wb.temp, wb.tint), if relative { (6500.0, 0.0) } else { (5200.0, 4.0) }, "{format}");
            // shown from the embedded preview: a rendered image, not a relative-WB raw
            p.preview_only = Some("unsupported".into());
            assert!(!p.relative_wb());
        }
    }
}
