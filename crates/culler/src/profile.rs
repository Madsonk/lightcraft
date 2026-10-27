//! Genre profiles: how strict the cull is, per kind of shoot. A profile is plain data, so the
//! thresholds are visible in `docs/culler.md` and in `cull.explain`.

use serde::{Deserialize, Serialize};

/// The kind of shoot being culled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Genre {
    #[default]
    General,
    Wedding,
    Portrait,
    School,
    Sport,
}

impl Genre {
    pub const ALL: [Genre; 5] = [Genre::General, Genre::Wedding, Genre::Portrait, Genre::School, Genre::Sport];

    pub fn name(self) -> &'static str {
        match self {
            Genre::General => "general",
            Genre::Wedding => "wedding",
            Genre::Portrait => "portrait",
            Genre::School => "school",
            Genre::Sport => "sport",
        }
    }

    /// Case-insensitive; French names accepted (`mariage`, `école`).
    pub fn parse(s: &str) -> Option<Genre> {
        match s.trim().to_lowercase().as_str() {
            "general" | "général" | "generic" | "default" => Some(Genre::General),
            "wedding" | "mariage" => Some(Genre::Wedding),
            "portrait" => Some(Genre::Portrait),
            "school" | "ecole" | "école" => Some(Genre::School),
            "sport" | "sports" => Some(Genre::Sport),
            _ => None,
        }
    }

    pub fn profile(self) -> Profile {
        Profile::for_genre(self)
    }
}

/// What to do about closed eyes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EyePolicy {
    /// Not looked at (sport: masks, helmets, profile views).
    Ignore,
    /// A warning: the photo goes to review.
    Warn,
    /// The photo is rejected.
    Reject,
}

/// The thresholds of one genre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub genre: Genre,
    /// Global focus (0..100) below which a photo is blurred (rejected).
    pub blur_reject: f32,
    /// Global focus below which a photo is soft (review).
    pub blur_warn: f32,
    /// A photo whose focus is below this share of the album's median focus is blurred, once the
    /// median itself is trustworthy (≥ 30).
    pub relative_blur: f32,
    /// Eye sharpness (0..100) below which a face is blurred / soft.
    pub face_blur_reject: f32,
    pub face_blur_warn: f32,
    /// Faces narrower than this share of the long edge only warn when blurred (the eyes are a few pixels).
    pub face_blur_min_size: f32,
    pub eyes: EyePolicy,
    /// Share of clipped pixels from which a photo is rejected / warned.
    pub clipped_reject: f32,
    pub clipped_warn: f32,
    /// Mean luminance (display-referred, 0..1) under / over which the exposure is rejected.
    pub dark_below: f32,
    pub bright_above: f32,
    /// Signature similarity from which two shots of one scene are duplicates (before `grouping`).
    pub dup_similarity: f32,
    /// Colour-layout similarity two duplicates also need (when both shots have one).
    pub dup_color: f32,
    /// Largest gap (seconds) between two shots of one duplicate group.
    pub dup_gap_secs: i64,
    /// Duplicates that are not the best of their group are rejected (otherwise: review).
    pub dup_reject: bool,
    /// Capture-time gap (seconds) that starts a new scene, at least (it grows with sparse shooting).
    pub scene_gap_secs: i64,
    /// Share of the selected photos promoted to highlights.
    pub highlight_share: f32,
    /// Share of the quality score that comes from the faces when there are some.
    pub face_weight: f32,
}

impl Profile {
    pub fn for_genre(genre: Genre) -> Profile {
        let base = Profile {
            genre,
            blur_reject: 15.0,
            blur_warn: 30.0,
            relative_blur: 0.35,
            face_blur_reject: 15.0,
            face_blur_warn: 30.0,
            face_blur_min_size: 0.10,
            eyes: EyePolicy::Warn,
            clipped_reject: 0.45,
            clipped_warn: 0.18,
            dark_below: 0.10,
            bright_above: 0.90,
            dup_similarity: 0.93,
            dup_color: 0.85,
            dup_gap_secs: 20,
            dup_reject: true,
            scene_gap_secs: 300,
            highlight_share: 0.15,
            face_weight: 0.5,
        };
        match genre {
            Genre::General => base,
            Genre::Wedding => {
                Profile { eyes: EyePolicy::Reject, dup_similarity: 0.90, dup_gap_secs: 30, scene_gap_secs: 600, face_weight: 0.6, ..base }
            }
            Genre::Portrait => Profile {
                eyes: EyePolicy::Reject,
                face_blur_reject: 22.0,
                face_blur_warn: 40.0,
                dup_similarity: 0.92,
                highlight_share: 0.20,
                face_weight: 0.7,
                ..base
            },
            // many near-identical frames of one pose: group generously, keep one
            Genre::School => {
                Profile { eyes: EyePolicy::Reject, dup_similarity: 0.88, dup_gap_secs: 60, highlight_share: 0.10, face_weight: 0.6, ..base }
            }
            // motion blur is part of the picture; sequences are kept for review, not thrown away
            Genre::Sport => Profile {
                eyes: EyePolicy::Ignore,
                blur_reject: 10.0,
                blur_warn: 22.0,
                relative_blur: 0.25,
                dup_similarity: 0.95,
                dup_gap_secs: 5,
                dup_reject: false,
                scene_gap_secs: 120,
                highlight_share: 0.10,
                face_weight: 0.2,
                ..base
            },
        }
    }
}

/// What the user can change on top of a genre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub profile: Profile,
    /// 0 .. 1, default 0.5: how eagerly look-alike shots are grouped (0 = only near-identical
    /// frames, 1 = loosely similar ones too). Moves the profile's similarity by ±0.10.
    pub grouping: f32,
    /// Names of the people the cull is about (case-insensitive). When set, only their faces are
    /// judged and a photo without any of them goes to review.
    pub key_people: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options::for_genre(Genre::General)
    }
}

impl Options {
    pub fn for_genre(genre: Genre) -> Options {
        Options { profile: Profile::for_genre(genre), grouping: 0.5, key_people: Vec::new() }
    }

    /// The colour similarity two duplicates need, with `grouping` applied.
    pub fn dup_color_threshold(&self) -> f32 {
        let g = if self.grouping.is_finite() { self.grouping.clamp(0.0, 1.0) } else { 0.5 };
        (self.profile.dup_color + (0.5 - g) * 0.2).clamp(0.3, 0.99)
    }

    /// The similarity two shots need to be duplicates, with `grouping` applied.
    pub fn dup_threshold(&self) -> f32 {
        let g = if self.grouping.is_finite() { self.grouping.clamp(0.0, 1.0) } else { 0.5 };
        (self.profile.dup_similarity + (0.5 - g) * 0.2).clamp(0.5, 0.995)
    }
}
