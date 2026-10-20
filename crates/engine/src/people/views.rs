//! What the People view lists: plain data, built by the engine, drawn by a frontend.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Thumb {
    /// The catalog item: a photo, or the video the face was seen in.
    pub photo: u64,
    pub face: String,
    /// `[x0, y0, x1, y1]`, fractions of the photo — or of the picture of the video (`video.frame`).
    pub rect: [f32; 4],
    /// The face was seen in a video: when, and the picture it is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoAt>,
}

/// A moment of a video: the video's catalog id, milliseconds into it, the picture cut there
/// (`<library>/video/<key>/t<ms>.jpg`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VideoAt {
    pub video: u64,
    pub ms: u64,
    pub frame: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Likely {
    pub person: u32,
    pub name: Option<String>,
    pub score: f32,
}

/// A person or a group of faces nobody confirmed, as the overview lists them.
#[derive(Clone, Debug, Serialize)]
pub struct GroupView {
    /// `p:<id>` for a person, `g:<slot>` for a group.
    pub id: String,
    pub person: Option<u32>,
    pub name: Option<String>,
    pub keyword: Option<String>,
    pub faces: usize,
    pub photos: usize,
    pub thumbs: Vec<Thumb>,
    /// For a group: known people it may be (nearest first).
    pub likely: Vec<Likely>,
    /// Mean similarity of the faces to the group's centre (how tight it is).
    pub tightness: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SuggestionView {
    pub face: String,
    pub photo: u64,
    pub rect: [f32; 4],
    pub person: u32,
    pub name: Option<String>,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoAt>,
    pub runner_up: Option<Likely>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub indexed_photos: usize,
    pub indexed_faces: usize,
    pub persons: Vec<GroupView>,
    pub groups: Vec<GroupView>,
    pub groups_total: usize,
    pub suggestions: Vec<SuggestionView>,
    pub suggestions_total: usize,
    /// Faces left out of the groups (too small, or ignored).
    pub left_out: usize,
    pub computing: bool,
    /// New faces arrived since the groups were made.
    pub stale: bool,
    pub grouped: bool,
    /// Changes whenever the overview may have.
    pub revision: u64,
    /// Groups the user marked "unknown person" (nameable later), biggest first.
    pub unknown: Vec<GroupView>,
    pub unknown_total: usize,
    /// Faces of those groups.
    pub unknown_faces: usize,
    /// Groups hidden by "Skip for now".
    pub skipped_groups: usize,
}
