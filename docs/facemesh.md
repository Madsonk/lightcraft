# Analyze Faces (eyes open / closed, eye sharpness)

LightCraft finds the faces of a photo and scores how **open** and how **sharp** each eye is, so that
Assisted Culling can reject the blinks and choose, among similar shots, the one where everybody's
eyes are open and crisp. (Lightroom's "Faces" panel in assisted culling does the same.)

It is built on **MediaPipe Face Landmarker** (Google): the BlazeFace short-range detector and Face
Mesh V2 (478 points), run by a small TensorFlow Lite interpreter written in plain Rust
(`crates/faces`, no ML framework, no C, no unsafe). The interpreter, its kernels and the MediaPipe
pipeline are adapted from EffectCraft's `effectcraft-segment` (same authors, MIT OR Apache-2.0),
where the interpreter's outputs were checked against TensorFlow Lite's own; the test
`matches_tensorflow_lite_on_the_official_weights` pins them here too.

**The model is optional.** It is not part of LightCraft: nothing requires it, and without it every
other feature works. See *Getting the model*.

## Using it

- **Photo ▸ Analyze Faces** (`photo.analyzeFaces`): analyzes the selected photos, else every photo in
  view. In the app it runs in the background (the grid stays usable, a toast says when it is done);
  photos already analyzed are skipped, and a run that is interrupted keeps what it did, so running it
  again resumes with the photos that have no faces yet. It is not an undo step: it is a measurement,
  like the focus score.
- **Info panel**: "N faces · eyes closed on K" and, per face from left to right, how open (0–100) and how
  sharp (0–100) each eye is. A grid badge "Eyes closed" marks photos with a closed eye.
- **Filtering**: `library.filter {faces: "any" | "none" | "closedEyes" | "eyesOpen" | "unanalyzed"}`,
  the search tokens `faces:any`, `eyes:closed`, `eyes:open`, and the smart-album rules *Faces*
  (`faceCount`), *Has Closed Eyes* (`eyesClosed`) and *All Eyes Open* (`eyesOpen`).
- **Assisted culling** (`photo.analyze`): `rejectClosedEyes: true` rejects photos with a closed eye
  (it analyzes the photos that have no faces yet, which needs the model); `faces: true` does the
  analysis without rejecting. When the shots of a burst have faces, the best of the burst is chosen on
  them (60 % eyes open and sharp, 40 % focus of the whole frame; a shot with a closed eye only wins
  when every shot has one); without faces nothing changes.
- **Control channel / CLI / MCP**: `photo.analyzeFaces {ids?, force?, level?, wait?}`, `photo.faces
  {id?}` (the faces of a photo), `facemesh.status`, `facemesh.cancel`, `facemesh.model.status`,
  `facemesh.model.download {acknowledged: true}`, `facemesh.model.cancel`. `lightcraft-cli` and the headless
  MCP server wait for the result; the app returns `{queued}`.

```sh
lightcraft-cli run --library ~/Pictures/Test.lcl --import ~/Pictures/party \
    facemesh.model.download acknowledged=true      # once, after the user agreed (3.8 MB)
lightcraft-cli run --library ~/Pictures/Test.lcl photo.analyzeFaces \
    photo.analyze rejectClosedEyes=true pickBest=true
```

## Getting the model

### Licence (what the authors' documents say)

The weights are the `face_landmarker.task` bundle of MediaPipe's Face Landmarker, version
`float16/1` (3 758 596 bytes, SHA-256
`64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff`), from
<https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task>.
The bundle holds the face detector, the landmark model and a blendshapes model (the last is not used).
Read at the source on 8 October 2026:

- **Model card, MediaPipe Face Mesh V2** (`Model Card MediaPipe Face Mesh V2.pdf`, authors Geng Yan and
  Ivan Grishchenko, Google; model date 15 September 2022): "LICENSED UNDER Apache License, Version
  2.0". Intended use: detecting 3D facial surface landmarks on monocular video from a front-facing
  smartphone camera, mainly AR entertainment. Out of scope: "not intended for human life-critical
  decisions", and the predicted landmarks "do not provide facial recognition or identification and do
  not store any unique face representation".
- **Model card, MediaPipe BlazeFace (Short Range)** (author Valentin Bazarevsky, Google; 9 June 2021):
  "LICENSED UNDER Apache License, Version 2.0". Out of scope: counting people in a crowd, faces looking
  away, people further than 2 m, and "any form of surveillance or identity recognition".

Both cards give Apache-2.0 and neither adds a condition on use or redistribution beyond the licence
itself. LightCraft uses the models to score eyes for culling, not to identify anyone: it stores boxes
and eye measurements only, never a face embedding. The licence is permissive (MIT/Apache-2.0
compatible), so the weights could be mirrored; **LightCraft still never bundles them**: the file is
downloaded when the user asks, from Google's own storage, checked against the size and SHA-256 above
(a file that doesn't match is deleted, never used), and the dialog names the licence first.

### In the app

Photo ▸ Analyze Faces without the model opens **Download the Face Model?**: what it is, its size
(3.8 MB), where it is saved, the licence and a link to the model card. Nothing is downloaded until
**Download** is clicked; the download runs in the background, can be cancelled, resumes where it
stopped, and goes through `lightcraft-fetch` (rustls with the RustCrypto provider; no OpenSSL).

### By hand, and where it lives

Put `face_landmarker.task` in `models/faces/` of the settings folder
(`~/Library/Application Support/LightCraft/` on macOS, `%APPDATA%\LightCraft\` on Windows,
`~/.config/lightcraft/` on Linux), or in the folder `LIGHTCRAFT_FACES_DIR` names. Extra download
locations (base URLs; the file is `<base>/face_landmarker.task`): `LIGHTCRAFT_FACES_MIRRORS`, or one URL per line in `models/faces-mirrors.txt` of the settings folder.

## How it works

1. **Source.** The photo is decoded at the *preview* level (2560 px long edge, `level` in the command
   changes it), upright (EXIF orientation applied), converted from the working space to display sRGB.
   A source that is dark overall (an unrendered raw) is brightened first so faces keep their
   contrast.
2. **Detection** (`crates/faces/src/mediapipe.rs`). BlazeFace sees a 128 × 128 window and finds faces
   that fill a good part of it, so it runs on windows of three sizes: the whole frame, then squares of
   0.6 and 0.3 of the short side sliding over it with a third of overlap (about 30 windows for a
   2560 × 1707 photo). Detections are merged by weighted non-maximum suppression.
3. **Mesh.** Each detection is cropped (1.5 × the box, rotated so the eyes are level) for Face Mesh
   V2, which returns 478 points and a presence score; the crop is recomputed once from the mesh's own
   points. Duplicates (one face seen through two detections) are merged.
4. **Filter.** The detector takes some food, ears and dark patches for faces. A face is kept only
   when its detector score is ≥ 0.70 and its mesh moved by at most 6 % of the face when recomputed
   from its own points (a mesh fitted to a non-face wanders), and when it is at least 40 px wide.
5. **Eye openness.** The *eye aspect ratio* (Soukupová & Čech, 2016): `EAR = (|p2 − p6| + |p3 − p5|) /
   (2 · |p1 − p4|)` over the eyelid points of the mesh (MediaPipe's canonical mesh numbering; "left" is
   the left of the image):

   | eye | p1 outer corner | p2 | p3 | p4 inner corner | p5 | p6 |
   |---|---|---|---|---|---|---|
   | left | 33 | 160 | 158 | 133 | 153 | 144 |
   | right | 263 | 387 | 385 | 362 | 380 | 373 |

   Open eyes give about 0.22–0.33, closed ones near 0; it does not depend on the head's roll. The
   score shown is 50 at the closed threshold and 100 from 0.24. Head turn is estimated from the nose
   tip (point 1) against the cheeks (234, 454); a face turned more than 55 % towards a cheek, or
   narrower than 2.5 % of the photo's long edge, is not judged.
6. **Eye sharpness.** The variance of the 4-neighbour Laplacian of the luminance on a crop of each eye
   (a square 1.8 × the eye's width, rotated with the head, averaged down to 48 × 48 px so that a
   small eye and a large one are measured on one scale), mapped to 0–100 with `100·(1 − e^(−v/0.004))`.

## Calibration (what the thresholds come from)

All numbers below were measured on real photos (the author's own portfolio, read only, nothing
uploaded), looked at by eye on contact sheets, with the scripts and raw data kept in
`/Volumes/Madson/ArtCraft/_tests/faces/work/` (`scan1d.jsonl`, `sheets.py`, `labels_*.json`).
`cargo run --release -p lightcraft-faces --example scan` prints the raw numbers per file.

### Closed eyes: the threshold

`EYE_CLOSED_BELOW = 0.15` (in `lightcraft-catalog`). On 340 raw detections of 171 photos (16 folders of
portraits, weddings and parties), the faces with the lowest eye aspect ratio were looked at as eye
crops, sorted by ratio. Below 0.15 nearly all are shut or lowered eyes (about 28 of 33 in the first
sheet; the others are sunglasses); from 0.16 up almost all are open, smiles squinting at 0.16–0.19
included (1 closed eye among the next 40, a profile). Open eyes sit at 0.22–0.33 (median of the
less open eye of a face: 0.26), hence 0.24 for "fully open". Seven closed-eye photos found this way
(ratio of the less open eye): `amelie/IMG_0110-Modifier.jpg` 0.04, `private party/2014_02_26_10153854659760156.jpg`
0.05, `Nabou/IMG_1362.jpg` 0.05, `cousinade/IMG_2577.jpg` 0.10, `allison photo/IMG_0055-Modifier.jpg` 0.10,
`private party/ed 8- birthday bash/10620267_….jpg` 0.11, `anniversaire ruth et ethan/IMG_1573.jpg` 0.15.
(Painting skin over the eyes of open-eyed portraits was tried to fabricate more: it does not look like
eyelids to the model, and was dropped.)

### Eye sharpness

30 large faces (≥ 250 px at 2560) were re-scored after a Gaussian blur of the whole photo. Median
sharpness 92 as shot, 35 after σ = 1.5 px, 8 after σ = 3 px, 1 after σ = 6 px (the variance falls to
24 %, 5 % and 1 % of the original), so `SHARP_SCALE = 0.004` separates a crisp eye from a soft one.

### Faces: recall and false detections

Counted by eye, face by face, on numbered contact sheets (a face counts when a person could be
recognized from it):

| set | photos with people | faces I counted | found | recall |
|---|---:|---:|---:|---:|
| portfolio JPEGs (47 photos drawn at random from the 171, 10 of them without people) | 37 | 87 | 82 | 94 % |
| phone HEICs, children (`heic_list.txt`, 19 files, through the engine) | 12 | 30 | 27 | 90 % |
| party photo with Santa (one of the CR2 folder's files) | 1 | 7 | 7 | 100 % |
| **total** | **50** | **124** | **116** | **93.5 %** |

Misses: small or turned faces in groups (a bridesmaid, a boy in profile on a swing, two guests of
a garden party). Detections that were not faces, on the 47 photos: 8 without the filter (a dessert
photo gave 4, a man seen from behind 1, two tiny boxes), 2 with it. The filter (detector score ≥ 0.70,
mesh drift ≤ 0.06) was chosen on 32 labelled non-faces and 108 labelled faces: 6 of the 32 pass,
4 of the 108 are lost. Over all 340 detections it removes 30 (9 %).

**Plates, no face expected**: the 106 CR2 plates of `2024-12-08` (+ 2 other files): 3 of the 105 plate
photos get a "face", all on the same fried bananas (their dark dots read as eyes and mouth): 2.9 %.

### Time

`photo.analyzeFaces`, model time per photo at the preview level (2560 px) on a heavily loaded
10-core Apple machine (load average 12–50 from other builds): median 250–350 ms (HEIC 175–320 ms, CR2
350 ms with a few raw files at 1–4 s), 90th percentile about 1 s on a photo with many windows hit.
Decoding is on top: the `photo.analyzeFaces` command took 22 s for the 19 HEICs (about 1.2 s a photo) and 178 s for the 108 CR2 and JPEG files (1.6 s a photo); importing them is separate. The model loads in under a second the first time.


## Storage and compatibility

The faces are stored in the photo's **analysis** (`Photo.analysis.faces`, optional), by the same
change (`SetAnalysis`) that already stores the focus score, so no new kind of change enters the
journal and the catalog format version is **not** bumped. Every new field is optional
(`#[serde(default, skip_serializing_if = "Option::is_none")]`): a photo that was never analyzed is
written exactly as before, and an older build that reads a library with faces ignores them.
Checked with LightCraft 0.2.1's CLI: it opens a library written by this build, lists and inspects its
photos and runs its own assisted culling on it. The one thing it cannot do is keep the faces when *it*
rewrites a photo's analysis (its own `photo.analyze`): they are dropped, and Analyze Faces recomputes
them.

## Limits

- Sunglasses and mirrored or opaque lenses hide the eyes, but the mesh still fits them: such faces
  read as "open" or as "closed" at random. No attempt is made to detect glasses.
- A downward gaze lowers the upper lid and reads as closed (it nearly is); a broad smile narrows the
  eyes to a slit that reads as 0.14–0.18, near the threshold.
- BlazeFace's short-range model is made for faces looking at the camera, and the mesh for faces that
  are upright within a few degrees: profiles, faces tilted past ~45° and faces under ~40 px are missed
  or not judged. Crowds (hundreds of tiny faces) are out of scope.
- The detector sometimes takes a dessert, an ear or a dark patch for a face; the filter above leaves
  a few.
- No expression, smile or gaze score, no recognition and no naming (those are the People view's XMP
  regions, unchanged).

## Developing

```sh
export RUSTUP_HOME=... CARGO_HOME=... PATH=...
cargo build --release -p lightcraft-cli
LIGHTCRAFT_FACE_LANDMARKER=/path/face_landmarker.task cargo test --release -p lightcraft-faces
cargo test --release -p lightcraft-catalog
cargo test --release -p lightcraft-engine --features faces
```

`crates/faces` is MIT OR Apache-2.0, adapted from EffectCraft's `effectcraft-segment`; the weights are
Google's (Apache-2.0) and are not in the repository.
