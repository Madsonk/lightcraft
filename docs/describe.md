# Annotations: what a photo or a moment of a video shows

One store, shared by photos and videos, for **what a picture shows**: the **scene** (where), the **action** (what is
happening, what people do) and a free **description**. Who is in it is not written here: the face index says it under
the same key ([`people.md`](people.md)), so "who + what + where + when" meet without copies — and `describe.search`
asks both at once.

## Where and how it is kept

`<library>/describe/annotations.jsonl`, next to the catalog, never in its journal: one JSON object per line, only ever
appended. A later line for the same `(item, ms, source)` replaces the earlier one; a line marked `deleted` takes it back;
a line cut short by a crash, or unreadable, is skipped (`describe.list` reports `skippedLines`).

| field | |
|---|---|
| `key` | 16 hex digits: `key_of(item)` for a whole photo, `key_of("<item>@<ms>")` for a moment of a video — the key of the face index |
| `item` | the catalog content key of the photo or video |
| `ms` | milliseconds into the video; absent for a photo (or a whole video) |
| `scene`, `action`, `description` | text (at most 4 000 characters each) |
| `status` | `candidate` or `confirmed`. **Only `describe.confirm` writes `confirmed`**: a program never does |
| `source` | who wrote it: `manual` (a person), `videocatcher`, `siglip` |
| `origin` | where in the source, e.g. `videocatcher:moment:250` |
| `score` | a program's confidence in its proposal (0 – 1) |
| `hints` | what a source said about who is there (`videocatcher:<name>`): a hint, **never** an identity |
| `deleted` | this line takes the slot back |
| `at` | when it was written |

### What a person writes wins

Each source has its own slot, as in Video Catcher (the value vs the `machine_*` texts): the programs' proposals are never
overwritten. What is shown (`describe.list`, `describe.search`) is the **value**: field by field, what the person typed
(`manual`), else the proposals (confirmed ones first, then Video Catcher's before a zero-shot label, then the latest). `from` says where each field comes from;
`proposals` lists the programs' texts as they wrote them.

## Commands

| Command | What it does |
|---|---|
| `describe.list {id?}` | the value of a photo, or of every moment of a video (time order), with `from`, `proposals`, `people` (the faces under its key and who they are), `frame` (the picture of the moment), `hints` |
| `describe.set {id?, ms?, scene?, action?, description?}` | what the person says (the fields given; `""` clears one). Writing is not confirming: the status stays what it was. One step of `describe.undo` |
| `describe.confirm {id?, ms?}` | "this is right": the value shown becomes the person's own copy, `confirmed` |
| `describe.delete {id?, ms?, source?}` | take back what the person wrote (`manual`, default), one program's proposal (`siglip`, `videocatcher`), or everything of the moment (`all`) |
| `describe.undo {}` | undo the last set / confirm / delete of the session |
| `describe.suggest {ids?, ms?, limit?, threshold?, scenes?, actions?, background?}` | proposals by SigLIP 2 zero-shot (below) |
| `describe.status {}`, `describe.cancel {}` | the proposal run: progress, what it wrote; stop it |
| `describe.search {query, people?, kind?, limit?, window?, semantic?, minProbability?, filter?}` | who + what over photos and videos (below) |
| `video.importVideoCatcher {db, createMissing?, dryRun?, maxBytes?}` | Video Catcher's pictures and described moments (below) |

`describe.*` commands do not change the catalog (they are not catalog undo steps): the keyword side of people stays with
`people.*`.

## Proposals: `describe.suggest`

Without an external service: the semantic search model (SigLIP 2, `search.model.download`, see
[`semantic-search.md`](semantic-search.md)) compares each picture — a photo, or the pictures cut out of a video
(`video.frames`; `ms` picks the one nearest a moment) — with French sentences: "une photo de {scene}" and "une photo
d'une personne en train de {action}". The best scene and the best action whose probability reaches `threshold` (0.02)
become a `candidate`, `source: siglip`, with its `score`. Nothing is confirmed, nothing replaces what a person wrote.

- **Without the model** the command fails with what to do; nothing is written, nothing is invented.
- **Only what is asked**: `ids`, else the selection; at most `limit` pictures (500, never more than 2 000). The whole
  library is never described at once.
- **Lists**: built in (32 scenes, 28 actions, French), extended by `<config>/describe-vocabulary.json`
  (`{"scenes": [...], "actions": [...]}`; `"replace": true` uses only yours), or given in the call (`scenes`, `actions`).
- `background: true` returns at once; `describe.status` follows it, `describe.cancel` stops it (what was written stays).
  Pictures without a search vector are embedded first (they then serve `search.semantic` and `describe.search` too).

## Search: `describe.search`

A query is split into the **people** it names — people named in People (a full name, or a first name only one of them
has), plus `people: [names]` — and the **words** left (French and English filler words dropped). A photo or a moment of
a video is a **result** when:

1. **every** person asked for is there: one of their *confirmed* faces is in the photo, or in a picture of the video
   within `window` seconds (6) of the moment;
2. the words match **what** it shows: the value of its annotation (scene, action, description; accents, case and word
   endings ignored: « conduisant » finds « conduit … voiture »), **or** its meaning, when the semantic model is installed
   and the picture is indexed (`search.index` indexes the pictures of videos too): probability ≥ `minProbability` (0.02).

Results: `{id, kind: photo|video, ms, frame, scene, action, description, status, from, people, matched: {words,
probability}}`, best first. `filter: true` shows only those photos and videos in the grid.

**Never widened in silence** (Video Catcher's rule): moments where the people are there but the words are not found are
listed apart (`diagnostics.personPresentWhatNotDetected`), moments that match the words without the people too
(`whatFoundPeopleNotRecognized`); a name nobody has is reported (`unknownPeople`). **"Not detected" is not "absent"**: a
face too small, turned away, or not confirmed yet (a suggestion is not counted), or a picture not indexed, is simply not
seen. `semantic.used` says whether the meaning was used, and why not.

The search bar's `semantic:` token is not wired to it yet (step 3: the Videos interface); `describe.search` is available
from the control channel, the CLI and MCP.

## In the app

**View ▸ Videos** (or the camera button of the bottom bar) opens the **Videos** view:

- **The list**: each video with its picture, name and length, the people named in it (click one: their photos and
  videos), how far its analysis went (*Pictures* cut · *Faces* looked for · *Described*) and how many moments are
  described and confirmed. Filters: a person, described or not, confirmed or to confirm. Buttons: **Import videos…** (a
  folder, with its videos), **Resume the Video Catcher import** (its videos the library lacks are imported in the
  background like any import, then its pictures and descriptions are read; the database is looked for at
  `~/Movies/VideoCatalog/libraries/…/video-catcher.sqlite3`, else a file dialog asks for it), **Cut pictures**, **Find
  faces**. Long runs (cutting pictures, finding faces, proposing descriptions) show a progress panel with **Cancel**.
- **A video's sheet** (click a video): its moments as a contact sheet — the picture with the faces outlined, the time,
  **Play at m:ss** (the system's player at that time: VLC with `--start-time` when installed, else the default player
  from the start; a double-click on the picture does the same), who is there (click a name), then **Scene**, **Action**
  and **Description**. Type and press **Enter** (or leave the field) to save; Shift+Enter makes a new line in a
  description. A proposal that differs from the value shows under its field (`+ …`): one click takes it. **Confirm**
  says the description is right (the only way to *Confirmed*); **Clear mine** takes back what you wrote. **Propose
  descriptions** proposes for this video only.
- **A photo**: the same editor is in the right panel (**Description**, Library space; Info elsewhere), with **Propose
  descriptions** for the selected photos.
- **Search**: type in the search field and press **Enter**: `Madson conduit une voiture` opens the results of
  `describe.search` in the Videos view — photos and moments of videos mixed, with their picture, time, people and
  status; a click opens the moment. "Person present, action not detected" is a separate list, closed by default, with
  the reminder that not detected is not absent. `semantic: …` searches photos by meaning (`search.semantic`) and shows
  them in the grid. Names of people complete as you type.
- **People ▸ Recognition**: a face seen in a video is cut out of its picture and marked *video · m:ss*; a click on the
  mark opens that moment. Naming, merging, moving and ignoring work as for any face.

There is no video player inside LightCraft yet: a moment opens in the system's player. For a future player, `video.clip {id, ms, seconds?}` makes a short clip around a moment (8 s by default, 540 lines, H.264 + AAC, MP4 `+faststart`) once and keeps it in `<library>/video/<key>/clips/`; it returns its path.

## Video Catcher

`video.importVideoCatcher {db, createMissing?, dryRun?}` reads a Video Catcher database (never writes it, nor the videos):

- its videos are matched to catalog videos by path; with `createMissing: true` (also `videos: true`), those the catalog
  lacks and whose file is there are first imported by file (ffprobe and three 1 MiB samples each: a video is never read
  whole), 25 at a time; already catalogued files are skipped, so an interrupted run resumes; `offline` counts files not
  found;
- its pictures are copied into `<library>/video/<key>/`; its described moments (`scene_text` / `action_text` /
  `description`, else the `machine_*` texts) become `candidate` annotations, `source: videocatcher` (a moment it marks
  `confirmed` stays confirmed: a person did it there); empty moments are skipped; importing again adds only what changed;
- its people on moments (`moment_people` → `persons`) become `hints` (`videocatcher:<name>`): never an identity, never a
  face of People. Who is there comes from LightCraft's own face index.
