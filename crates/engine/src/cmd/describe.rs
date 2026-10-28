//! Annotations: `describe.*` — what a photo or a moment of a video shows (scene, action,
//! description), written by a person or proposed by a program, and the search "who + what + where
//! + when" over photos and videos. See `crate::describe` and `docs/describe.md`.

use super::{CommandSpec, always, cmd};
use crate::describe::{ops, search, suggest};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "describe.list",
            "Descriptions",
            [],
            None,
            "{id?} (default: the active photo) — what a photo, or each moment of a video, shows: the value (what a person typed wins, field by field, over the programs' proposals, which are listed apart in `proposals`), `status` (candidate | confirmed), `from` (sources of the fields), who is there (`people`, from the faces under the same key; nothing copied), `frame` (the picture of the moment), Video Catcher's `hints` → {id, annotations: [{key, ms, scene, action, description, status, from, proposals, people, frame}], total}",
            always,
            ops::list
        ),
        cmd!(
            query "describe.set",
            "Describe",
            [],
            None,
            "{id?, ms? (a moment of a video, milliseconds), scene?, action?, description?} — what the person says the photo or moment shows (the fields given; \"\" clears one); kept apart from the programs' proposals and shown instead of them. Written to `<library>/describe/`, one step of describe.undo → {id, ms, annotation, cleared}",
            always,
            ops::set
        ),
        cmd!(
            query "describe.confirm",
            "Confirm Description",
            [],
            None,
            "{id?, ms?} — the person says the description shown is right: it becomes theirs, `confirmed`. The only way anything becomes confirmed (a program never confirms) → {id, ms, annotation}",
            always,
            ops::confirm
        ),
        cmd!(
            query "describe.delete",
            "Delete Description",
            [],
            None,
            "{id?, ms?, source?: manual (default: what the person typed) | siglip | videocatcher | all} — take that annotation back (a mark is appended; describe.undo restores it) → {id, ms, deleted, annotation}",
            always,
            ops::delete
        ),
        cmd!(
            query "describe.undo",
            "Undo Description",
            [],
            None,
            "{} — undo the last describe.set / confirm / delete of this session → {undone}",
            always,
            |s, _| ops::undo(s)
        ),
        cmd!(
            query "describe.suggest",
            "Propose Descriptions",
            [],
            None,
            "{ids? (default: the selection; never the whole library), ms? (one moment of each video), limit?: n pictures (500, at most 2000), threshold?: probability (0.02), scenes?: [..], actions?: [..], background?: bool} — proposals by the semantic search model (SigLIP 2, zero-shot): each picture (a photo, or the pictures of a video) is compared with French scenes and actions (built in, extended by `<config>/describe-vocabulary.json`); the best of each above the threshold is written as a `candidate`, `source: siglip`. Without the model: an error says so and nothing is written. Follow describe.status, stop with describe.cancel → {running, total, done, written, proposals, error}",
            always,
            suggest::suggest
        ),
        cmd!(
            query "describe.status",
            "Description Proposals Status",
            [],
            None,
            "{} — the describe.suggest run → {running, total, done, written, proposals, error}",
            always,
            |s, _| Ok(s.describe.job.report())
        ),
        cmd!(
            query "describe.cancel",
            "Stop Proposals",
            [],
            None,
            "{} — stop describe.suggest (what was written stays) → {cancelled}",
            always,
            |s, _| Ok(suggest::cancel(s))
        ),
        cmd!(
            query "describe.search",
            "Search Who and What",
            [],
            None,
            "{query (\"Madson conduit une voiture\"), people?: [names], kind?: photo | video | all, limit?: n (50), window?: seconds (6), semantic?: bool (true), centre?: none | text | gap (query vector minus the mean of neutral captions, gap: plus the mean of the pictures), floor?: false | margin (a meaning match must beat, by this cosine margin, the best cosine nonsense queries reach with that picture; true = 0), normalise?: bool (rank by that margin), minProbability?: (0.02), filter?: bool (show only the photos and videos found)} — photos and moments of videos where every person named (in the query or `people`) is there (a confirmed face in the photo, or in a picture of the video within `window` seconds) and the other words match what it shows (annotations, accents ignored; or the meaning of the picture when the semantic search model is installed and the pictures indexed). Never widened in silence: `diagnostics.personPresentWhatNotDetected` (the people are there, the words not found) and `whatFoundPeopleNotRecognized` come apart; not detected is not absent → {query, people, words, semantic, total, results: [{id, kind, ms, frame, scene, action, description, status, people, matched}], diagnostics}",
            always,
            search::search
        ),
    ]
}
