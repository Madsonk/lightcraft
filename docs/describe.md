# Annotations: what a photo or a moment of a video shows

One store, shared by photos and videos, for **what a picture shows**: the **scene** (where), the **action** (what is
happening, what people do) and a free **description**. Who is in it is not written here: the face index says it under
the same key ([`people.md`](people.md)), so "who + what + where + when" meet without copies.

## Where and how it is kept

`<library>/describe/annotations.jsonl`, next to the catalog, never in its journal: one JSON object per line, only ever
appended. A later line for the same `(item, ms, source)` replaces the earlier one; a line cut short by a crash, or
unreadable, is skipped (`describe.list` reports `skippedLines`).

| field | |
|---|---|
| `key` | 16 hex digits: `key_of(item)` for a whole photo, `key_of("<item>@<ms>")` for a moment of a video — the key of the face index |
| `item` | the catalog content key of the photo or video |
| `ms` | milliseconds into the video; absent for a photo |
| `scene`, `action`, `description` | text (at most 4 000 characters each) |
| `status` | `candidate` (written by a program) or `confirmed` (a person said yes). **A program never writes `confirmed`** |
| `source` | who wrote it: `videocatcher`, `manual`, a model's id |
| `origin` | where in the source, e.g. `videocatcher:moment:250` |
| `at` | when it was written |

## Commands

| Command | What it does |
|---|---|
| `describe.list {id?}` | the annotations of a photo, or of every moment of a video (time order), each with `people` (the faces found under its key and who they are) and `frame` (the picture of the moment) |
| `video.importVideoCatcher {db, dryRun?, maxBytes?}` | reads a Video Catcher database (never writes it): for the catalog's videos it knows by path, its pictures go to `<library>/video/<key>/` and its described moments (`moments`: `scene_text`/`action_text`/`description`, else the `machine_*` texts) become `candidate` annotations, `source: videocatcher` (a moment Video Catcher marks `confirmed` stays confirmed: a person did it there). Empty moments are skipped; importing again adds only what changed |

Not yet (next step): writing annotations by hand or by a model for selected photos, confirming them, and searching them
by meaning with semantic search. Photos get annotations on demand only; nothing is filled in for the whole library.
