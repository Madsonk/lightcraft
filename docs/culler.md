# AI culling (`cull.*`)

LightCraft judges every photo of an album and gives it a **verdict**: *highlight*, *selected*,
*review* or *rejected*, with the reasons (closed eyes, blurred, face blurred, duplicate, bad
exposure…) and a **proposed** star rating. It does what a culling assistant does — pick the keepers,
drop the failures, group the look-alikes, order the shoot by scene — locally, in LightCraft's own
code, with no network and no model that is not already part of LightCraft.

Nothing is ever written to a rating or a flag by the cull itself. `cull.run` only stores verdicts in
the photos' analysis (a measurement, like focus and faces); `cull.apply` turns them into stars and
Pick / Reject flags, in one undo step, when you say so. Originals are only read.

## What exists, and what this builds on

| Piece | Where | Role here |
|---|---|---|
| Focus, clipping, exposure, look-alike signature | `lightcraft-pipeline::cull` | the per-photo measurements (original, thumbnail-level source, 512 px) |
| Faces, eye openness, eye sharpness | `lightcraft-facemesh` → `photo.analyzeFaces` ([facemesh.md](facemesh.md)) | closed eyes, blurred faces (read from the photo's analysis) |
| Face names | XMP/MWG face regions on the photo | key people |
| Assisted Culling | `photo.analyze` (`cmd/cull.rs`) | the older burst + focus suggestions; **kept**, `cull.*` is the full verdict |
| Taste | `lightcraft-taste` | not yet blended in (the `Shot.taste` hook exists; see limits) |
| Time/place groups | `series.groups` | different job (colour harmonisation); scenes here are separate and finer |

New: `crates/culler` (`lightcraft-culler`, L1, pure Rust, no I/O) holds the judging; the engine's
`cull_job.rs` runs it as a background job and exposes the commands. The verdict is stored in
`Analysis.verdict` (optional, ignored by older builds, like `faces`).

## The verdict

```
bucket   highlight | selected | review | rejected
stars    1..5  (proposal: highlight 5, selected 4 or 3, review 2, rejected 1)
score    0..100 overall quality
reasons  closedEyes · halfClosedEyes · blurred · soft · faceBlurred · faceSoft · duplicate ·
         underexposed · overexposed · clipped · noKeyFace · bestOfGroup · facesNotAnalyzed
flags    the Aftershoot-like names: selected, highlighted, closed_eyes, blurred, duplicated, warning
scene    shooting scene, chronological from 0
group    duplicate group (+ groupRank 0 = best, groupSize)
faces    per face: eyes open/half/closed/not judged, blur sharp/soft/blurred, quality, person
```

A reason has a severity: **reject** (**both** eyes closed in a strict genre, whole-photo blur, too dark / too
bright, a worse duplicate when the genre rejects them), **warning** (soft, a blurred or soft face, a
wink or half-closed eyes, clipped, no key person, duplicates in sport) or **info** (best of its
group, faces not analyzed). Any reject → *rejected*; else any warning → *review*; else *selected*.
Only the photo's **prominent** faces are judged: at least half the size of its largest face and at least
4 % of the long edge; the others are listed in `faces` (`counted: false`) but never judged. The face
measurements are evidence, not proof (see the trial below), so a blurred face only sends to review.
Among the selected, the top `highlightShare` (and at least score 55) become *highlight*.

### Score

`score = (1 − w)·focus + w·faceQuality − 50·clipped − 25·|meanLuminance − 0.45|`, minus 40 for a
closed eye (when the genre looks at eyes), then 0.85 · score + 0.15 · taste when a taste score is
supplied. `faceQuality` is the one `FaceInfo::quality` uses (70 % eye openness, 30 % eye
sharpness, the worse eye counts). `w` is the genre's face weight (0.2 sport … 0.7 portrait), applied
only when some counted face could be judged.

## Duplicates and scenes

* **Scenes** ("smart shooting time"): shots sorted by capture time; a gap longer than
  `max(genre minimum, 8 × the album's median gap)` (at most 4 h) starts a new scene — a slow
  shooter's long gaps do not split every other shot. A scene of more than 60 shots is split by
  appearance with **spherical k-means** on the signatures (deterministic farthest-point start,
  k ≈ n/30). Photos without a capture time form the last scene.
* **Duplicates**: inside a scene, each shot is compared with the next 24 (time order); a pair is
  joined when its signature similarity reaches the threshold and the capture gap is within the
  genre's limit (union-find, so a burst is one group). Undated shots need +0.04 similarity.
* **Grouping slider** (`grouping`, 0..1, default 0.5): moves the genre's similarity threshold by
  ±0.10 — 0 groups only near-identical frames, 1 groups loosely similar ones.
* Inside a group the best shot ranks first (a shot with a rejecting problem never beats one
  without); the others are *Duplicate* (rejected, or review in sport).

## Genre profiles

| | general | wedding | portrait | school | sport |
|---|---|---|---|---|---|
| closed eyes | warn | **reject** | **reject** | **reject** | ignored |
| blurred / soft (focus) | 15 / 30 | 15 / 30 | 15 / 30 | 15 / 30 | 10 / 22 |
| face blurred / soft (eye sharpness) | 20 / 40 | 20 / 40 | 28 / 50 | 20 / 40 | 20 / 40 |
| duplicate similarity · gap | 0.93 · 20 s | 0.90 · 30 s | 0.92 · 20 s | 0.88 · 60 s | 0.95 · 5 s |
| duplicates rejected | yes | yes | yes | yes | no (review) |
| scene gap (min) | 5 min | 10 min | 5 min | 5 min | 2 min |
| highlights | 15 % | 15 % | 20 % | 10 % | 10 % |
| face weight | 0.5 | 0.6 | 0.7 | 0.6 | 0.2 |

The whole profile of a genre is returned by `cull.setGenre` and by `cull.explain`. A photo is also
*blurred* when its focus is under 35 % (sport 25 %) of the album's median, once that median is
≥ 30. Exposure: mean luminance under 0.10 or over 0.90, or more than 45 % clipped, is rejected;
more than 18 % clipped warns.

## Key people

`keyPeople: ["Alice", "Bob"]` (case-insensitive) limits the eye and face judgements to those
people's faces and sends a photo where none of them appears to *review* (`noKeyFace`). A face takes
a name from the face region (XMP/MWG, as written by Lightroom, digiKam or the People view) whose
centre falls in it.

## Commands

All are ordinary engine commands: the same ids work in the app's control channel, `lightcraft-cli
run` and the MCP server (`lightcraft-cli mcp` lists them as tools).

| Command | What |
|---|---|
| `cull.run {ids?, genre?, grouping?, keyPeople?, faces?, force?, limit?, wait?}` | judge the album (default: the visible photos; refuses more than `limit` = 5000 photos). Background job; `wait: true` blocks until done. `faces: true` first runs Analyze Faces on photos that have none (needs the face model). |
| `cull.status` | `{state: idle/running/done/cancelled/failed, total, done, percent, elapsedMs, photosPerSecond, reused, measured, facesNotAnalyzed, failed, summary}` |
| `cull.cancel` | stops between two photos; a cancelled run writes nothing |
| `cull.list {bucket?, reason?, minStars?, scene?, group?, sort?, offset?, limit?, filter?}` | the results, sorted by scene (default), score or time, with a per-scene summary; `filter: true` narrows the view |
| `cull.explain {id}` | the reasons with severity and plain text, the faces, the duplicate-group mates, the measurements and the genre's thresholds |
| `cull.setGenre {genre?, grouping?, keyPeople?}` | the defaults of `cull.run`; no params reads them |
| `cull.apply {ids?, bucket?, stars?, flags?, overwrite?}` | write the proposed stars (unrated photos only) and Pick (highlights) / Reject flags (unflagged photos only); one undo step |

Smart-album rules: `cullBucket` (highlight / selected / review / rejected) and `cullStars`.

```bash
lightcraft-cli run --import ~/Pictures/shoot cull.setGenre genre=wedding cull.run wait=true cull.list bucket='["rejected"]'
```

Measurements are cached by file size and modification time in the library prefs (bounded, like Assisted
Culling's), so changing the genre or the grouping and running again is quick. Re-running on
unchanged files costs only the judging.

## Tests

* `crates/culler/src/tests.rs`: a synthetic album with planted faults and known answers
  (`synthetic.rs`): every planted fault rejected, no good shot rejected, bursts keep exactly their
  best frame, scenes follow the hours, k-means splits two looks, the grouping slider moves the
  result, key people, genre differences, hostile input (NaN, empty, extreme times) never panics.
* `crates/engine/src/tests_cull.rs`: a real album of PNG files through `cull.*` — verdicts, bursts,
  dark photo, scenes, originals byte-identical, cache reuse, list/explain/apply/undo, genre and
  faces, cancel, limit.

## Throughput

See the end of this page.

### Measured (21 Oct 2026)

`cargo test -p lightcraft-engine tests_cull::bench -- --ignored --nocapture` (generated album,
`LIGHTCRAFT_CULL_BENCH=48`, 3000 × 2000 JPEG, 6 Mpx; byte-identical files are merged at import, so 36 photos).
Apple Silicon, **test profile (opt-level 1, not release)**, machine shared with other builds:

| Run | Photos | Time | Throughput |
|---|---|---|---|
| cold (decode to the 512 px source + measure + judge) | 36 | 1.40 s | **25.8 photos/s** |
| warm (measurements cached, judge only) | 36 | < 1 ms | > 100 000 photos/s |

Judging alone (scenes, duplicates, verdicts) is negligible next to decoding. A release build should be
faster; **not measured**. Not measured either: RAW albums (the older notes put a RAW decode at ~0.8 s
per photo, which would dominate), and runs with `faces: true` (the face model is not installed on the
test machine).

## Limits

* The look-alike signature is an 8 × 8 luminance grid: it misses duplicates that differ in colour or
  framing and can group different photos of a similar tonal layout. A learned embedding would do better.
* Eye and face judgements need Analyze Faces (`faces: true` or `photo.analyzeFaces`) and its model;
  without it the verdict says `facesNotAnalyzed`. On the test machine this path was exercised with
  injected face data, not with the real model on real faces.
* Thresholds are reasoned defaults checked on synthetic and generated albums, **not calibrated on real
  shoots** by genre. The taste model is not blended in yet.
* Key people come from named face regions (XMP), not from the People view's recognition.
* Genre, grouping and key people are kept for the session, not saved to the library prefs.
* No panel in the app yet (`ui-egui` was out of scope); everything is reachable by command.

## Trial on a real album (21 Oct 2026)

A read-only copy of 12 JPEG + 1 PNG from a client wedding portfolio (kept on the internal disk, never the
originals, never the 736-photo album), imported in a throw-away catalogue, genre *wedding*, with the face model
(`LIGHTCRAFT_CULL_DIR=… LIGHTCRAFT_CULL_FACES=1 cargo test -p lightcraft-engine tests_cull::real_album -- --ignored --nocapture`).
13 photos in 0.2 s (test profile; faces were already found by the same run, 7 s in total).

* **Without faces** (focus, exposure, duplicates, scenes): 2 highlights, 8 selected, 1 review (soft), 2 rejected —
  both rejected are byte-identical re-exports of a better-ranked file in the same duplicate group, which is
  right. Four scenes from the capture times, two duplicate groups.
* **With faces**, the first rules rejected 7 of 13. Looking at the images, at least two rejections were wrong
  (a groom with open eyes rejected for "closed eyes" and "blurred face"; a ceremony photo rejected for out-of-focus
  guests in the background). Two fixes came out of it: only prominent faces are judged, and a blurred face no
  longer rejects. What remains: the eye measurements are noisy on small faces of this retouched portfolio (faces are
  2–10 % of the frame; many read "blurred"), so **closed-eyes rejections on a real shoot must be checked by a
  person** — they are reported with their reason and nothing is written without `cull.apply`.
* The dominant risk is therefore the face model's calibration on real faces (it was calibrated in `faces.md`
  on a few labelled photos), not the culling logic. Not yet compared with Aftershoot's verdicts on the same
  album: doing so means importing the client's photos into Aftershoot (its own database, possibly its cloud), which
  was not done without an explicit go.
