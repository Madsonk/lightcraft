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
   and the picture is indexed (`search.index` indexes the pictures of videos too): among the `semanticTop` (20) pictures
   closest to the query. A rank, not an absolute probability: SigLIP's probabilities on a French sentence are all tiny
   and their level changes from one query to the next, so a fixed threshold (0.02 before) let almost nothing or almost
   everything through. `minProbability` is still honoured when it is given.

The meaning is asked with the query as typed **and** in English (`lang`: `fr`, `en`, `both` — the default): a local
French → English glossary of about 300 words of the library's annotations (`describe/glossary.rs`, nothing sent
anywhere; a word it does not know stays as it is) — the model reads English best. The text and the meaning are merged by
**rank fusion** (reciprocal rank, k = 60): a moment found by both comes first. `fields` limits the words to some of
`scene` / `action` / `description`; `text: false` or `semantic: false` turn one side off.

**Bench** (`examples/describe_bench.rs`, test library of 386 videos, 21 Oct 2026). The 1 312 moments Video Catcher
described with both an action and a description are the questions: the query is one field, the search may only use
the other fields and the pictures (limit 200). Recall@k = the moment is in the first k; "video" = the right video.

| query | method | r@1 | r@5 | r@10 | median rank | video r@1 | video r@5 |
|---|---|---|---|---|---|---|---|
| action (short) | text only | 19.5 % | 36.4 % | 45.0 % | 14 | 22.7 % | 41.8 % |
| | meaning, French as typed | 6.2 % | 19.1 % | 26.4 % | 69 | 15.7 % | 36.7 % |
| | meaning, glossary English | 6.7 % | 16.4 % | 23.8 % | 90 | 15.3 % | 33.9 % |
| | meaning, both | 8.1 % | 20.4 % | 27.2 % | 63 | 18.4 % | 38.2 % |
| | **fusion, both (default)** | **20.9 %** | **42.3 %** | **53.5 %** | **8** | **25.6 %** | **52.0 %** |
| | fusion, English | 19.2 % | 40.6 % | 51.7 % | 10 | 24.5 % | 49.7 % |
| description (long) | text only | 20.5 % | 36.9 % | 45.3 % | 14 | 23.9 % | 44.5 % |
| | meaning, French as typed | 22.7 % | 49.7 % | 61.6 % | 6 | 46.4 % | 77.8 % |
| | meaning, glossary English | 18.9 % | 43.8 % | 55.4 % | 8 | 40.2 % | 70.4 % |
| | meaning, both | 22.2 % | 48.9 % | 61.7 % | 6 | 46.2 % | 77.1 % |
| | **fusion, both (default)** | **35.4 %** | **64.0 %** | **74.4 %** | **3** | 43.4 % | 73.0 % |
| | fusion, English | 33.5 % | 61.6 % | 72.0 % | 3 | 41.0 % | 71.1 % |

Read with care: the fusion is the best way to find *the moment* for both kinds of query (median rank 14 → 8 for a
short action, 14 → 3 for a sentence), so it is the default. The glossary alone does **not** help (English is below
French as typed on both); asking both only helps a little on short queries — it is kept because it costs nothing and
never hurt the fusion. To find *the video* from a long sentence, the meaning alone does a little better than the fusion
(77.8 % vs 73.0 % in the first 5). Video Catcher wrote its descriptions from these very pictures with a vision model,
so the meaning's numbers here are optimistic for a person's own words. The old default (meaning kept above an absolute
probability of 0.02) was not measured in this bench. 0.03–0.07 s per query; the run took 28 min for 12 × 1 312
searches (peak memory 3.3 GB).

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

- **Preview** (on a moment of the sheet): plays 8 s around the moment in the sheet itself — the clip `video.clip` makes,
  shown as 8 pictures a second (480 px), **without sound**; a click on the picture or **Play / Pause** stops and goes on,
  the bar moves inside the clip, **-5 s / +5 s** make the clip around the new time, **Play in VLC** opens the system's
  player there. The clip is made and decoded on a worker thread; closing the preview stops it.
- **Unreadable** (in the list): the videos an import could not read, with their cause in plain words — an empty file
  (0 bytes), a file cut short whose index (`moov`) is missing (it cannot be rebuilt without a sound file from the same
  camera; LightCraft does not try), no picture track, a file gone — and **Open the folder**. A file that imports later
  leaves the list (`video.unreadable`, `<library>/video/unreadable.json`).

There is no full player inside LightCraft (no sound, no long playback): the system's player does that. For a future player, `video.clip {id, ms, seconds?}` makes a short clip around a moment (8 s by default, 540 lines, H.264 + AAC, MP4 `+faststart`) once and keeps it in `<library>/video/<key>/clips/`; it returns its path.

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
