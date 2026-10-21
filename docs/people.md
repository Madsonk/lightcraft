# People: recognizing the faces of a library

LightCraft finds the faces of your photos, groups those of one person, asks you "same person?" where it is not
sure, and files the photos under the keyword `People|<name>` once you say so. Everything runs on your machine, in
pure Rust (`crates/people`).

**Nothing is tagged without a yes.** A new photo that looks like someone you named is only *suggested*
(`people.suggest`, People ▸ Recognition ▸ "Looks like someone you named"). An optional auto-tag threshold
(`people.settings {autoTag}`, off by default) tags above a very high similarity; the run is one undo step.

## How it works

1. **Faces**: the MediaPipe Face Landmarker of Analyze Faces (`docs/faces.md`) finds each face and its 478-point
   mesh, from the photo's preview (at most 2560 px; RAW files use their embedded JPEG), shrunk to 1600 px by default.
   Faces narrower than 40 px, or whose mesh does not hold (`MIN_DETECTION`, `MAX_DRIFT`), are left out.
2. **Alignment**: five landmarks (iris centres, nose tip, mouth corners) are mapped by the least-squares similarity
   onto the ArcFace template; a 112 × 112 crop is sampled.
3. **Embedding**: InsightFace's ArcFace network turns the crop into 512 numbers (unit length). It is run from its ONNX
   file by the small ONNX interpreter of `lightcraft-inpaint` on candle (operators added for it: BatchNormalization,
   PRelu, Flatten, Gemm; Conv + BatchNorm pairs are folded into the convolution at load).
4. **Index**: `<library>/people/faces-<model>.bin`, append-only, next to the catalog (never in the RAW files, never
   in the catalog journal). One record per face: box, detector score, size, head turn, the embedding as 16-bit floats
   (1 068 bytes). A photo is written whole; a record set cut short by a crash is ignored; a photo with no face gets a
   marker so it is not looked at again. Keyed by the photo's content key: only new or changed photos are analysed, and
   an interrupted run resumes.
5. **Groups**: faces whose cosine similarity is at least `link` (default 0.50) end up together (leader clustering,
   refinement, merge of alike groups); faces close to a person you confirmed (`suggest`, default 0.45) are suggestions
   for them, not groups. Pairs you said are different are never put together.
6. **Your decisions** are in `<library>/people/people.json` (small, readable): persons and their confirmed faces,
   "not this person", "these two differ", ignored faces, questions answered "not sure", settings.

## Commands

(control channel, `lightcraft-cli run`, MCP; see `engine.commands`)

| Command | What it does |
|---|---|
| `people.status` / `people.model.status` | models (installed, licence, download), index, groups, run |
| `people.model.download {acknowledged: true}` / `.cancel` | the recognition model and, if needed, the 3.8 MB face finder; resumable, checked by size and SHA-256 |
| `people.index {ids?, force?, background?, edge?, flip?, limit?}` | find, align and embed the faces of photos not looked at yet; then groups them |
| `people.cancel`, `people.refresh` | stop the run (what was done is kept); group the faces again |
| `people.list {groups?, suggestions?, thumbs?}` | persons (`p:N`), groups to name (`g:N`), suggestions, with face thumbnails |
| `people.name {group\|person, name, dryRun?}` | the group is this person: faces become theirs, photos get `People|<name>` |
| `people.rename {person, name}`, `people.merge {from, into}`, `people.delete {person}` | edit people |
| `people.ignore {face\|group\|faces}` | **Not a real face**: a statue, a poster, a wrong detection; left out for good |
| `people.unknown {face\|group\|faces, off?}` | **Unknown person**: seen, not known (see below) |
| `people.skip {face\|group\|faces, clear?}` | **Skip for now**: hide a group until “Group again”; decides nothing |
| `people.move {faces, to: p:N\|g:N\|unknown\|ignored}` | this face is actually that person (see below) |
| `people.tagPhotos {person, ids?}` / `people.untagPhotos` | file whole photos under a person (a photo dropped on them) |
| `people.members {person\|group, limit?}` | the faces of a person or group, each with the other faces of its photo and who they are |
| `keyword.apply {keyword, ids?}` | add a keyword to the photos (only those that lack it; one undo step); `People|<name>` files them under the person |
| `people.confirm {question\|face+person, answer: yes\|no\|unsure}` | "same person?" |
| `people.questions {limit?}` | the doubtful cases, nearest to the thresholds first |
| `people.suggest {ids?, limit?, minScore?}` | faces that look like a known person |
| `people.photos {person\|group, filter?}`, `people.faces {id?}` | a person's photos; the faces of a photo and who they are |
| `people.undo` | undo the last decision and the keywords it set (one step) |
| `people.settings {link?, suggest?, autoTag?, minFacePx?, model?: fast\|accurate, tagUnknown?}` | thresholds, auto-tag, model, keyword for unknown faces |
| `people.sync` | give every named person's photos their keyword again |

In the app: View ▸ People ▸ **Recognition**.

## Faces in videos

A video of the catalog is recognized through a few **pictures cut out of it**, which go through the same face finder,
alignment and ArcFace network as photos, into the **same index**:

1. `video.frames {ids?, every?, max?, force?, maxBytes?}` cuts the pictures with **FFmpeg**, run as a helper program
   (never linked: found on `PATH`, in `/opt/homebrew/bin`, `/usr/local/bin`, or named by `LIGHTCRAFT_FFMPEG` /
   `LIGHTCRAFT_FFPROBE`; each call has a deadline and is killed after it). By default 12 pictures spread over the video
   (one every 2 s for a short one; 120 at most per video with `every`, never more than 240), JPEG, 1280 px. They are kept
   in `<library>/video/<16 hex digits of key_of(content key)>/t<ms>.jpg` with a `frames.json` that lists them — never
   next to the video, never in the catalog journal; all of them together stay under `maxBytes` (4 GiB by default). The
   list is written after every picture: an interrupted run resumes, and a picture already there is not cut again
   (`force` cuts LightCraft's own pictures again, each replaced in one rename; pictures brought in from Video Catcher are never removed nor replaced; a stopped run leaves a whole list). The folder is a cache, deleting it loses nothing but work. `background: true` returns at
   once: `video.status` → `job` shows the progress, `video.cancel` stops it (what was cut is kept).
2. `people.index` (`videos: true` by default) also looks at the pictures of the videos in scope; videos without
   pictures are counted in `videosWithoutFrames`. The faces of a picture are stored under the key of its moment,
   **`key_of("<content key of the video>@<ms>")`**, where a photo's are under `key_of("<content key>")`.
3. From there a face of a video is a face like any other: it is grouped with the faces of the photos, suggested for a
   person named on photos, named, merged, moved, ignored, marked unknown, said "not this person"; **`People|<name>`
   goes on the video** (the catalog item) as long as one of the person's faces is confirmed in one of its pictures (or
   the video was filed under them by hand).
4. What the lists say of a face of a video: `people.list` (thumbnails of persons, groups and suggestions),
   `people.members`, `people.faces {id: video}` add `video: {video, ms, frame}` (catalog id, milliseconds into the
   video, path of the picture; the face's `rect` is a fraction of that picture, not of the video's poster);
   `people.photos` adds `videos` (how many of `ids` are videos) and `moments: [{video, ms, frame}]`.

`video.importVideoCatcher {db, createMissing?, dryRun?}` takes the pictures Video Catcher already cut (and its described
moments, see [`describe.md`](describe.md)) for the catalog's videos it knows by path, instead of cutting them again;
`createMissing: true` first imports its videos the catalog lacks. Its people are only hints, never faces of People.
Who is where and doing what: `describe.search` ([`describe.md`](describe.md)).

**At scale (test library of 386 videos, 3 897 pictures, M4 under load, 21 Oct 2026).** `people.index` took 1 635 s
(943 faces, peak memory 1.15 GB), `search.index` 246 s (2.2 GB); on disk `people/` 5 MB, `search/` 6 MB, `video/`
377 MB (pictures and previews), `describe/` 0.4 MB. Grouping with the photo settings (`link` 0.5, `suggest` 0.45,
`minFacePx` 40) gave 146 groups. Checked by eye on face sheets: the biggest group (407 faces in 67 videos) is one man,
the one who films himself, with no clear stranger in an 80-face sample; the next 14 groups are each one person, except
**one group of 33 "faces" that are not faces** (car wheels, a steering wheel, objects — the finder's false detections,
which group together) and two or three groups that may hold two look-alike men under caps or at a sharp angle. No
default was changed: nothing measured says another `link` would do better on photos. What to do in a video library:
mark the false-detection group *Not a real face* (`people.ignore`), and check the big group's pictures at an angle
before confirming it whole.

Importing a video: `library.import {paths: [file]}` with the file named, or a folder with `videos: true` (by default a
folder brings its photos only, so a photo library is not filled with videos unless asked; files already catalogued are
skipped, so an interrupted import of a big folder resumes).
Its size and duration come from `ffprobe`; its content key is a hash of its size and three 1 MiB samples (`vs1-…`; a
video is not read whole); the grid shows a picture FFmpeg cuts at 1 s.

## A photo can hold several people

The model is **face → person**; the keyword is at the level of the **photo**: `People|<name>` is on a photo as long as
at least one face of that person is confirmed in it, or the photo was filed under them by hand. So:

- moving **one** face from A to B adds `People|B` to the photo, and removes `People|A` **only if no other face of A is
  confirmed in that photo** (two faces of A in one photo: A stays) and the photo was not filed under A by hand;
- “Not a real face” or “Unknown person” on one face never removes the keyword of the **other** people of the photo;
- a photo with a known face and an unknown one keeps the known person's keyword;
- a photo dropped on a person is filed under them **as a whole**: it gets `People|<name>` (not twice), whether or not a
  face of theirs was found, and it is remembered by hand in `people.json` (`photo_tags: [[photo key, person]]`), so
  `people.sync`, regrouping, or moving a face of that photo to someone else never removes it. Other people's keywords are
  untouched;
- the person a face leaves is told “not them” (`rejected`): it is not suggested for them again.

## Unknown person, Skip for now, Not a real face

For a group you do not know, in the card of the group (People ▸ Recognition):

- **Unknown person**: you looked, you do not know them. The group leaves the list to name, **without any keyword**, and
  waits in **Unknown people** (nameable later: type a name, or drag them onto a person). Recorded in `people.json`
  (`unknown`), one undo step (`people.undo`). Optional setting `people.settings {tagUnknown: true}` (off by default): the
  photos of unknown faces get `People|Unknown` (switching it off removes it; a person cannot be named “Unknown”).
- **Skip for now**: hides the group for the session or until “Group again”; nothing is decided or written.
- **Not a real face**: the faces are left out for good (`ignored`).

## Drag and drop

In People ▸ Recognition, **click** a face to pick it (click again to unpick), **drag** it (or the picked ones) onto:

- a **person** (their card): “this face is actually that person” (`people.move`); a **group** (its card): the same person
  as that group (an unnamed person is made, name it later); **Unknown person** / **Not a real face** (the two zones at the
  top). The target is highlighted; a ghost of the face with the count follows the pointer; a message says what was
  done; **Undo** (or `people.undo`) takes the whole gesture back. Escape cancels a drag. The caption “N faces · M photos · drag” of a
  group drags the whole group.
- A photo may hold several faces: hover a face to see every face of its photo and who it is; a person's **Faces** list
  shows each of their faces with the other faces of the same photo, named (“unknown”, “not a face”, “to name”); the
  selected photo's faces are listed at the top. It is always **one face** that is dragged, never “the photo”.

**Photos** are dragged from the grid or the filmstrip (the selection moves together; a photo that was not selected becomes
the selection) onto the left panel: a **regular album** (adds them; a smart album or a folder refuses with a message), or
a **keyword** of the Keywords tree (`keyword.apply`): that tree also holds `People|<name>` (files the photos under the
person, see above) and `Places|…` (the place keyword of that node, nothing above it). A closed parent opens when you
hover it. Nothing is written before the button is released over a target; releasing anywhere else, or Escape, writes
nothing. Dragging in the loupe of the detail view still pans the photo: use the filmstrip to drag from there.

## The models (not part of LightCraft)

| | file | size | SHA-256 |
|---|---|---|---|
| accurate (default) | `w600k_r50.onnx`, ArcFace ResNet-50, WebFace600K (`buffalo_l`) | 174 383 860 | `4c06341c…619e43` |
| fast | `model.onnx`, ArcFace MobileFaceNet, WebFace600K (`buffalo_s`) | 13 616 099 | `9cc6e4a7…19eb4f` |

Downloaded on request (`people.model.download`, the app shows the terms first) into
`<config>/models/people/<model id>/` or `LIGHTCRAFT_PEOPLE_DIR`; mirrors: `LIGHTCRAFT_PEOPLE_MIRRORS`. Source:
`huggingface.co/public-data/insightface` and `immich-app/buffalo_s`, pinned to a commit.

**Licence**: InsightFace's models are released **for non-commercial research purposes only**
(https://github.com/deepinsight/insightface/tree/master/python-package#license). They are fine for personal use of
your own photos; they must not be bundled with a product or used commercially. LightCraft's own code stays MIT OR
Apache-2.0; no InsightFace code is used (the alignment template is a list of ten numbers, the network is run by our
own interpreter).
