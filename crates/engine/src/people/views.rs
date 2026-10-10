//! What the People view lists: plain data, built by the engine, drawn by a frontend.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Thumb {
    pub photo: u64,
    pub face: String,
    pub rect: [f32; 4],
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
}
