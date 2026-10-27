//! What the describe store needs from semantic search while the SigLIP index is not part of this
//! build (the 0.14.0 port): the "not available" message and the key a picture would have in it.
//! Semantic proposals and by-meaning ranking answer with [`UNAVAILABLE`]; the text of annotations
//! stays the search engine (see `docs/port-014.md`).

pub const UNAVAILABLE: &str = "Semantic search is not available in this build";

/// The key of an item (`ms` = None) or of one of its pictures, as the face index keys them.
pub(crate) fn search_key(content_key: &str, ms: Option<u64>) -> u64 {
    match ms {
        Some(ms) => crate::video::frame_key(content_key, ms),
        None => crate::video::item_key(content_key),
    }
}
