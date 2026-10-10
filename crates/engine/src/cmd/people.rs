//! People: `people.*` — recognize the faces of the library, group those of one person, name a
//! group (tagging its photos `People|<name>`), and ask "same person?" about the doubtful ones.
//! The models are optional and downloaded on request (see [`crate::people`] and `docs/people.md`).

use serde_json::json;

use super::{CommandSpec, always, cmd};
#[cfg(feature = "people")]
use crate::people::ops;

#[cfg(feature = "people")]
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "people.status",
            "People Status",
            [],
            None,
            "{} — whether this build can recognize faces, the recognition model (installed, where, licence, download progress), the face finder, the index (photos and faces analysed, people, groups), the indexing run and the settings → {available, model, faceModel, index, job, computing}",
            always,
            |s, _| Ok(ops::status(s))
        ),
        cmd!(
            query "people.model.status",
            "People Model Status",
            [],
            None,
            "{} — same as people.status",
            always,
            |s, _| Ok(ops::status(s))
        ),
        cmd!(
            query "people.model.download",
            "Download People Model",
            [],
            None,
            "{acknowledged: true} — download the recognition model (InsightFace ArcFace, 174 MB for the accurate model or 14 MB for the fast one, `people.settings {model}`; published for non-commercial research use, not part of LightCraft) and, if needed, the 3.8 MB face finder, in the background; only after the user agreed to their terms. Each file is checked against its size and SHA-256. Watch people.status; people.model.cancel stops it → {started, installed, downloading}",
            always,
            ops::model_download
        ),
        cmd!(
            query "people.model.cancel",
            "Cancel People Model Download",
            [],
            None,
            "{} — stop the download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.people.cancel_download()}))
        ),
        cmd!(
            query "people.index",
            "Find People in Photos",
            [],
            None,
            "{ids? (default: the whole library), force?: bool, background?: bool (false: wait), edge?: 320..4096 (1600: longest side the faces are looked for in), flip?: bool, limit?: n photos} — find the faces of the photos that were not looked at yet (from their previews, never the RAW), embed each face and append it to `<library>/people/`; interrupted runs resume where they stopped. Then groups the faces (people.list). Needs the face finder and the recognition model → {queued, alreadyIndexed, failed, failures, faces, seconds, secondsPerPhoto, phaseSeconds, indexedPhotos, indexedFaces}",
            always,
            ops::index
        ),
        cmd!(
            query "people.cancel",
            "Cancel Finding People",
            [],
            None,
            "{} — stop the indexing run after its current batch (what it did is kept) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.people.cancel_job()}))
        ),
        cmd!(
            query "people.refresh",
            "Group Faces Again",
            [],
            None,
            "{background?: bool} — group the faces nobody has confirmed again (after new photos, or a change of threshold)",
            always,
            ops::refresh
        ),
        cmd!(
            query "people.list",
            "People",
            [],
            None,
            "{groups?: n (30), suggestions?: n (30), thumbs?: n faces per entry (4)} — the people the user confirmed (`p:N`, name, faces, photos), the groups of similar faces nobody confirmed yet (`g:N`, biggest first, with `likely` known people) and the suggestions (faces that look like a known person), each with face thumbnails {photo, face, rect} → {indexedPhotos, indexedFaces, persons, groups, groupsTotal, suggestions, suggestionsTotal, leftOut, computing, stale}",
            always,
            ops::list
        ),
        cmd!(
            query "people.photos",
            "Photos of a Person",
            [],
            None,
            "{person|group: p:N | g:N, filter?: bool (show only them)} → {ids, count}",
            always,
            ops::photos
        ),
        cmd!(
            query "people.faces",
            "Faces of a Photo",
            [],
            None,
            "{id?} — the faces found in a photo and who they are → {id, analysed, faces: [{face, rect, size, quality, person, name, ignored}]}",
            always,
            ops::faces
        ),
        cmd!(
            "people.name",
            "Name a Person",
            [],
            None,
            "{group|person: g:N | p:N, name, dryRun?: bool} — say that a group of faces is this person: the faces become theirs (a new person, or the existing one of that name) and every photo of those faces gets the keyword `People|<name>`; naming a person with another person's name merges them. One undo step (people.undo) → {person, id, name, keyword, faces, photos}",
            always,
            ops::name
        ),
        cmd!(
            "people.rename",
            "Rename a Person",
            [],
            None,
            "{person: p:N, name} — the person's keyword is renamed on every photo",
            always,
            ops::rename
        ),
        cmd!(
            "people.confirm",
            "Same Person?",
            [],
            None,
            "{question: id from people.questions | face + person|group, answer: yes | no | unsure} — `yes`: the face (or group) is that person (or two groups are one person); `no`: it is not (never suggested for them again); `unsure`: ask later. Confirmed people get the keyword `People|<name>` on their photos → {answer, persons}",
            always,
            ops::confirm
        ),
        cmd!(
            "people.merge",
            "Merge People",
            [],
            None,
            "{from, into: p:N | g:N} — `from` is the same person as `into`; `from`'s faces and photos join `into` → {person}",
            always,
            ops::merge
        ),
        cmd!(
            "people.ignore",
            "Ignore Faces",
            [],
            None,
            "{face|group} — leave these faces out (not a face, or not wanted); a face of a person is taken from them",
            always,
            ops::ignore
        ),
        cmd!(
            "people.delete",
            "Delete a Person",
            [],
            None,
            "{person: p:N} — forget the person: their faces are free again and their keyword is removed from the photos",
            always,
            ops::delete
        ),
        cmd!(
            query "people.suggest",
            "Suggest People",
            [],
            None,
            "{ids?, limit?: n (50), minScore?: cosine} — faces that look like a person the user confirmed, most alike first; nothing is tagged until `people.confirm` (or the auto-tag threshold, `people.settings`) → {suggestions: [{face, photo, rect, person, name, score, runnerUp, ask}], total}",
            always,
            ops::suggest
        ),
        cmd!(
            query "people.questions",
            "People Questions",
            [],
            None,
            "{limit?: n (10)} — \"is it the same person?\" questions, the doubtful ones first: a face and a known person, a group and a known person, two groups; answer each with `people.confirm {question: id, answer}` → {questions: [{id, kind, score, ...thumbnails}], total}",
            always,
            ops::questions
        ),
        cmd!(
            "people.undo",
            "Undo People Decision",
            [],
            None,
            "{} — undo the last people decision (name, confirmation, merge…) and the keywords it put on photos → {undone}",
            always,
            |s, _| ops::undo(s)
        ),
        cmd!(
            "people.settings",
            "People Settings",
            [],
            None,
            "{link?: cosine (0.50: faces this alike are grouped), suggest?: cosine (0.45: this alike to a known person is suggested), autoTag?: cosine | null (null: always ask; e.g. 0.65 tags without asking above that), minFacePx?: n (40), model?: fast | accurate} → {settings, model}",
            always,
            ops::settings
        ),
        cmd!(
            "people.sync",
            "Tag People Again",
            [],
            None,
            "{dryRun?: bool} — give every named person's photos their keyword again (after an import, or an undo of keywords) → {added, removed}",
            always,
            ops::sync
        ),
    ]
}

#[cfg(not(feature = "people"))]
pub fn specs() -> Vec<CommandSpec> {
    fn off(_: &mut crate::Session, _: &serde_json::Value) -> crate::Result<serde_json::Value> {
        Err(crate::EngineError::Other(crate::people::UNAVAILABLE.into()))
    }
    vec![
        cmd!(query "people.status", "People Status", [], None, "{} → {available: false}", always, |_, _| Ok(json!({"available": false}))),
        cmd!("people.index", "Find People in Photos", [], None, "{}", always, off),
    ]
}
