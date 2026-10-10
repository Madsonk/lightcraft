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
| `people.rename {person, name}`, `people.merge {from, into}`, `people.delete {person}`, `people.ignore {face\|group}` | edit people |
| `people.confirm {question\|face+person, answer: yes\|no\|unsure}` | "same person?" |
| `people.questions {limit?}` | the doubtful cases, nearest to the thresholds first |
| `people.suggest {ids?, limit?, minScore?}` | faces that look like a known person |
| `people.photos {person\|group, filter?}`, `people.faces {id?}` | a person's photos; the faces of a photo and who they are |
| `people.undo` | undo the last decision and the keywords it set (one step) |
| `people.settings {link?, suggest?, autoTag?, minFacePx?, model?: fast\|accurate}` | thresholds, auto-tag, model |
| `people.sync` | give every named person's photos their keyword again |

In the app: View ▸ People ▸ **Recognition**.

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
