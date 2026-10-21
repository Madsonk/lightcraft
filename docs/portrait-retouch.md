# Portrait retouch by sliders (`portrait.*`)

A portrait is retouched with **named sliders** — *Blemishes*, *Dark circles*, *Teeth whitening*,
*Background*… — each with an amount from 0 to 100. The sliders are kept as an ordered **stack** with the
photo, the pixels they make are an inpaint patch placed by an ordinary spot, so **the original is never
touched**, every step can be turned off, changed or removed, and rendering never needs a model.

This is lot C of the "LightCraft × Aftershoot" plan: the idea (retouch by sliders, one model or method per
zone, a step history) comes from what Aftershoot shows in its UI; the code and the method are ours.
Nothing here uses an Adobe asset, a network call, or an Aftershoot file.

## What exists, what is new

| Piece | State before | State now |
|---|---|---|
| `portrait.retouch` / `portrait.clear` | masks "Portrait: Skin / Eyes / Teeth / Lips" with *local adjustments* (texture, clarity, exposure). No pixel is generated; no blemish removal | unchanged: the light, fully-editable route. Described in [dust-portrait.md](dust-portrait.md) |
| `facemesh` (MediaPipe Face Landmarker, Apache-2.0) | 478 points per face | unchanged, the geometry of every slider |
| `retouch` (L2, pure maths) | Lab, skin measurement, harmonization fields, dodge & burn planes | + module `portrait`: the slider steps and the background compositing (`crates/retouch/src/portrait/`) |
| `autoseg` (ISNet subject matte) | masks | used for the *Background* step (see licences below) |
| `inpaint` (MI-GAN) | `spot.inpaint` | the same patch format carries the portrait pixels; `portrait.remove` = `spot.inpaint` |
| `restore`, `depth` | AI denoise / super-resolution, depth maps | not used by the sliders (no GAN face restoration here) |
| **new** `portrait.apply / preview / reset / history / batch / auto / cancel / list / remove` | — | the stack, previews, album batches |

## The sliders

Fixed processing order (whatever order they are given in): marks first, so smoothing never smears them,
then shapes, colour, mouth and eyes, hair, background.

| Id | Group | What it does | Method (all classical, ours) |
|---|---|---|---|
| `blemishes` | Skin | removes acne, pimples, isolated marks | difference-of-Gaussians on L and a\* on the skin; compact, small, round blobs only (hairs and wrinkles are elongated and are rejected); filled from the surrounding skin (normalized convolution that leaves the marks out) plus matched grain |
| `freckles` | Skin | fades freckles and small dark spots without erasing them | the same detector at a finer scale; adds back a share of the missing darkness and colour; deep marks and edges left to `blemishes` |
| `smooth` | Skin | even skin tone, softer texture | frequency separation: low pass from a weighted blur inside the skin; the detail is attenuated but 30 % is kept (pores); strong detail (edges, hairs) is gated out; colour blotches blurred on a, b |
| `wrinkles_eyes` / `_forehead` / `_lines` | Wrinkles | crow's feet and lower-lid lines / forehead lines / nose-to-mouth lines | the dark thin lines of the band-pass of L inside a region drawn from the mesh (outer eye corners, above the brows, nose wing to mouth corner) are lightened; very deep lines (nostril, lip crease) are left alone |
| `dark_circles` | Face | lightens and neutralizes the skin under the eyes | band under each lower lid; its low-pass L is lifted toward the skin median, a and b pulled toward the skin's hue (bluish shade → skin); texture untouched |
| `shine` | Face | oily highlights | smooth highlights above the skin's median are pulled down and re-coloured (specular is desaturated); computed on a low-pass so pores stay |
| `double_chin` | Face | lifts the shadow under the chin | **tonal only, no reshaping** (see limits) |
| `teeth` | Mouth and eyes | whitens teeth of an open mouth | inside the inner lips, only light, non-red pixels (so lips, gums, tongue stay); yellow (b\*) pulled toward neutral, a little lighter. Skipped, and reported, when the mouth is closed |
| `eyes` | Mouth and eyes | brighter whites, less redness, contrast in the iris | inside the eyelid contours; whites = light and neutral; iris from the mesh's iris circle |
| `flyaways` | Hair | removes thin hairs that cross a *smooth* background | white and black top-hat of L (a thin line is removed by an opening/closing of radius ≈ 1 % of the face width), only in a ring around the head and only where the neighbourhood is smooth (so the texture of the hair itself is not erased) |
| `neck_smooth` / `neck_light` | Neck | smooths the neck / brings its brightness toward the face's | neck region = ellipse below the chin, restricted to pixels whose colour looks like the face's skin (so collars and clothes are left out) |
| `background` | Background | replaces the background behind the subject | needs a subject matte: `mode` `blur` (matte-aware, the subject's colours are not pulled in), `color` (+`color`), `dim` (darker/brighter); `shadow`, `zoom` (%), `rotate` (°), `edge` (hardness of the matte, with colour decontamination at the edge) |

Not done yet (honest list): **body** smoothing/blemishes (needs a body-skin segmentation), **clothing
wrinkles** (needs a clothes segmentation), **lens flare** removal, **double chin reshaping**, **hair
re-colouring**, **eye colour**, **GAN face restoration**. *Object removal* is `portrait.remove` =
`spot.inpaint` (MI-GAN) and works on any selection.

## How it works

```
portrait.apply {blemishes: 60, dark_circles: 40, background: {mode: "blur", amount: 60}}
   │
   ├─ render the photo at the scene-linear stage, uncropped (white balance, lens correction, geometry
   │  applied; tone, colour and masks not yet), as 8-bit sRGB, at most 4096 px long
   ├─ find the faces (MediaPipe) on the developed picture, biggest first (maxFaces, default 4)
   ├─ per face: crop with room for hair and neck; run the steps on a copy whose face is ≤ 720 px wide;
   │  the *difference* is scaled back up and added to the full-resolution crop (so pores the steps did
   │  not touch keep their full-resolution detail)
   ├─ background: subject matte → refine edge → blur/colour/dim, zoom/rotate, shadow, composite
   └─ bake what changed into one InpaintPatch (a rectangle of RGB + alpha); a spot places it;
      the steps and their numbers are stored in DevelopSettings.portrait
```

* **Non-destructive.** The file is never read for writing. The patch is a separate file next to the
  library (`inpaint/<hash>.lcp`); the spot and the stack are in the settings. `edit.undo` removes a whole
  `portrait.apply` in one step. `portrait.reset` removes the stack; `portrait.reset {step}` removes one step
  and runs the others again.
* **Preview vs final.** `portrait.preview {…, edge: 1024}` runs the same code on a 1024 px picture and
  writes `before`/`after` PNGs without storing anything; sizes are fractions of the face width, so the
  preview and the final render do the same thing (tested).
* **Stale pixels.** The patch is made at the scene-linear stage. If the white balance, lens correction or
  geometry change afterwards, `portrait.history` says `stale: true`; `portrait.apply` with no arguments
  re-runs the stack. Tone, colour, crop and masks can change freely.
* **History per step.** Each step keeps its amount, whether it is enabled, and what it changed last time
  (`changed` = share of the face's pixels that moved by ΔE > 2). That is the number to quote in a UI
  ("changes 12 % of the face"), not a quality score.

## Commands

| Command | Parameters | Result |
|---|---|---|
| `portrait.list` | `{id?}` | the sliders (id, label, group, help, auto default, the photo's current step) |
| `portrait.apply` | `{id?\|ids?, <slider>: 0..100 …, background?: {mode, amount, color?, shadow?, zoom?, rotate?, edge?}, steps?: [{kind, amount, enabled?}], auto?, replace?, maxFaces?}` | `{photos: [{id, faces, steps: [{step, changed, found, skipped}], seconds, patch}]}`; given steps replace the same ones in the stack, `replace` starts over, `auto` = the default recipe, nothing = re-run the stack; an amount of 0 removes a step |
| `portrait.preview` | same sliders `+ {edge?, path?}` | `{before, after, width, height, faces, steps, seconds: {retouch, render}}` — two PNG files, nothing stored |
| `portrait.reset` | `{id?\|ids?, step?}` | `{reset}` |
| `portrait.history` | `{id?}` | `{steps, faces, patch, stale}` |
| `portrait.batch` | `{ids? (else selection, else view), sliders \| auto, offset?, chunk? (8), maxFaces?}` | `{total, done, next, cancelled, retouched, photos, seconds, secondsPerPhoto}` — call again with `offset = next` until `next` is null; a photo without a face is skipped and reported; a failing photo does not stop the album |
| `portrait.auto` | `{ids?, offset?, chunk?}` | `portrait.batch` with the default recipe |
| `portrait.cancel` | `{}` | stops a running batch between two photos (`portrait::request_cancel()` from an app thread) |
| `portrait.remove` | `{rect \| ellipse \| points, size?}` | = `spot.inpaint` |

Same commands over the CLI (`lightcraft-cli run`) and MCP (`lightcraft-cli mcp`), like every command.

```sh
lightcraft-cli run --import portrait.jpg \
  portrait.preview '{"blemishes":60,"dark_circles":40,"teeth":30,"path":"/tmp/p-after.png"}' \
  portrait.apply '{"blemishes":60,"dark_circles":40,"teeth":30}'
lightcraft-cli run --import-folder shoot/ portrait.auto '{"chunk":50}'
```

### Auto retouch

The default recipe (`auto: true`, `portrait.auto`): smoothing 35, blemishes 55, dark circles 40, shine 40,
teeth 30, eyes 30, eye wrinkles 20, forehead wrinkles 20, smile lines 15, neck smoothing 25. It is a
*prudent* default — visible on a close look, never plastic — chosen to be safe over a whole album, not to
be the best look for each face. Freckles, double chin, stray hairs, neck brightness and the background are
never in it (they change identity or composition).

## Models, and their licences

Only the **face mesh** is needed for the face sliders (MediaPipe Face Landmarker, Apache-2.0, 3.8 MB,
already in LightCraft, downloaded on request). The sliders themselves are classical image processing in
Rust — no weights, nothing to license.

The **background** step needs a subject matte. What LightCraft has today (`autoseg`):

| Model | Licence | Fit |
|---|---|---|
| ISNet / BRIA RMBG-1.4 (current `autoseg` subject) | **non-commercial** (BRIA licence) | works, but personal use only |
| SegFormer ADE20K (people, sky…) | NVIDIA Source Code License, **non-commercial** | personal use only |

Permissive candidates to replace them (from the projects' published licences — **verify at the source and
record the exact file hash before embedding**, this session had no network access):

| Candidate | Licence (published) | Use |
|---|---|---|
| IS-Net `isnet-general-use` (DIS project) | Apache-2.0 | same architecture as the current ISNet loader; a drop-in subject matte |
| U²-Net / U²-Netp (human-seg weights) | Apache-2.0 | smaller, faster matte |
| MODNet (portrait matting) | Apache-2.0 | portrait-specific matte with soft hair edges |
| MediaPipe Selfie *multiclass* segmenter | Apache-2.0 | background / hair / body skin / face skin / clothes in one model: would give **body smoothing, clothing and hair** sliders with a permissive licence; `lightcraft-facemesh` already contains a TFLite reader |
| MI-GAN (already used by `inpaint`) | MIT (code and weights, as stated in `crates/inpaint`) | object removal |
| Face-parsing nets trained on CelebAMask-HQ | the dataset is research/non-commercial | **avoid** for shipped weights |

Own weights: not trained yet. The sliders above are the part that does not need any.

## Open eyes (`portrait.openEyes`)

A good photo where one person blinked. Three levels, tried in this order; the user (not the program) moves
from one to the next.

| Level | What | State |
|---|---|---|
| **1** local transfer | the eyes of **the same person** from another shot of the **same burst** (`Analysis.group`), real pixels | **done**, default |
| **2** local generative | a local model repaints the eyes, guided by the person's identity | **not feasible today** (below) |
| **3** online generative | an online image model (kie.ai, gpt-image-2, Banana/Gemini) | **consent contract only**: the crop is prepared and shown; **nothing is sent**, this build calls no online service |

**Level 1, how.** The closed face is the first one whose eye aspect ratio is below the "closed" limit of
Analyze Faces (`photo.analyzeFaces`: the detection is lot A's, it is not duplicated here). The reference is the
best shot of the burst (`FaceInfo::quality`) where a face at about the same place has both eyes open; when the
People index knows both faces by name, the names must be equal. Then, per eye
(`lightcraft_retouch::portrait::eyes`): the reference eye is aligned on the photo's by a similarity fixed by
the two eye corners; the **light and colour** are matched by a smooth shift measured on a ring of skin around
the eye; the two are joined by a feathered mask. Guards that fail say why and do nothing (never a bad eye):
reference eye not open (< 0.2), head angle differs (> 0.12 face widths of nose offset), face size differs
(×0.4..×2.5), mesh incomplete.

**Always a preview first.** `portrait.openEyes {}` writes `before`/`after` PNGs and the numbers (`openBefore`,
`openAfter`, `seamDeltaE` = mean colour difference on the join band, `scale`, `rotationDeg`) and changes nothing.
`{apply: true, confirmed: true}` stores it, **only for the very photo / reference / face that was previewed**
in this session, as the `open_eyes` step of the portrait stack (one undo, the original untouched, the
reference photo is read each time the stack is re-run). No usable reference → the error *"no reference photo
(pas de photo de référence)"*: no eye is ever invented.

**Level 2, why not.** Required: a model that draws *this person's* open eyes, running locally, with a licence
LightCraft can ship. Looked at (licences as published; verify before relying on them):

| Candidate | Licence | Why it does not do the job |
|---|---|---|
| LaMa | Apache-2.0 | generic inpainting: fills an eye region with plausible texture, no identity: not "the same person" |
| MI-GAN (already used by `inpaint`) | MIT | same: scene inpainting, not identity-guided |
| Stable Diffusion inpainting family | CreativeML OpenRAIL-M (use restrictions) | several GB, no pure-Rust runtime tuned for it here, and needs an identity adapter on top |
| IP-Adapter-FaceID / InstantID | built on InsightFace weights: non-commercial | cannot be shipped |
| GFPGAN / CodeFormer / RestoreFormer | code Apache-2.0 or S-Lab non-commercial; trained on FFHQ | restore a degraded face, do not open closed eyes |

Verdict: **not feasible today** with a model we can ship; `level: 2` returns that message. Revisit when a
permissive, identity-conditioned inpainting model exists.

**Level 3, the contract.** `portrait.openEyes {level: 3, face}` writes the crop that *would* leave the Mac —
the eyes region only, no EXIF, no GPS, no file name — and returns `{sent: false, needsConsent: true,
willLeaveTheMac: {what, pixels, bytes, file}}`. Sending needs Madson's explicit yes **for that photo**, after
looking at the file, with a clear warning that the face region leaves the Mac. The online call itself is not
implemented: wiring a key and a script from `infra-capabilities-index` is a deliberate, separate step.
Preview before/after and a single undo apply to levels 2 and 3 as to level 1.

**Measured (level 1, synthetic pair).** Reference = the same synthetic face shifted by (9, −6) px and 6 % darker
(the target has both eyes painted closed, eye aspect ratio ≈ 0.04): both eyes are aligned and transferred
(`openAfter` ≈ 0.36), the sclera goes back to L > 85 and the iris to L < 40, the skin right under the eye keeps
the photo's brightness within 4 L, nothing changes away from the eyes, a closed reference or a head turned by
more than the guard are refused with a reason; end to end through `portrait.openEyes`: preview writes two PNGs
and stores nothing, `apply` without a preview or without `confirmed` is refused, a confirmed apply is one undo
and the file is byte-identical (`cargo test -p lightcraft-retouch portrait`: 20; `-p lightcraft-engine
tests_portrait_stack`: 11). **Not measured: resemblance on real faces** (no real portrait available to this
session, and gaze / angle differences cannot be simulated honestly); the first real burst must be looked at
by eye before trusting the seam number.

**Limits of level 1** (consigned, to be measured on real faces): a different **gaze** between the two frames
(the iris looks elsewhere), a different **angle** (the guard stops at ~0.12 face widths; below that the eye
can still look slightly wrong), **eyelid shadow** and lash shadow on the cheek that belong to the closed pose,
**glasses** (reflections do not match), makeup or tears that changed, motion blur between frames, and
the burst having no frame with both eyes open (then: no reference).

## Measured

On the synthetic portrait (`lightcraft_retouch::portrait::fixture`), release build, Apple Silicon, machine
under load from other jobs (so these are upper bounds):

| Face size | One step | Auto recipe (10 steps) |
|---|---|---|
| 400 px frame (face 220 px) | 9–33 ms | 0.05 s |
| 1600 px frame (face 880 px, worked at 720 px) | 0.17–0.53 s (flyaways and blemishes the slowest) | **1.1 s** |

End to end through `portrait.apply` (engine test, a 400 px PNG, rendering the linear stage, patch baking and
the store) the whole stack takes well under a second; the 3 s per photo seen in an early run came from
upscaling a small file to the 4096 px edge, now capped to the file's size. The model-bound part (face mesh,
subject matte) is not in these numbers: it is the face landmarker's cost per photo, measured elsewhere
([facemesh.md](facemesh.md)).

Checks that pass (`cargo test -p lightcraft-retouch portrait`: 18, `-p lightcraft-engine tests_portrait_stack`
with `--features people,faces,autoseg`: 9):
a planted blemish loses > 50 % of its contrast and nothing outside the face changes; freckles fade without
vanishing; smoothing cuts pore texture to 15–75 % while the iris stays bit-identical; dark circles gain
> 3 L while the cheek moves < 1; shine drops > 6 L and keeps > 40 % of the pores; teeth lose yellow
(Δb\* < −8) while lips and tongue stay bit-identical, and a closed mouth is reported as skipped;
forehead lines are lightened; a stray hair over the wall disappears; the same retouch at half the size
matches within ΔE00 < 6; the file on disk is byte-identical after `portrait.apply`; one undo removes a whole
application; a photo without a face is skipped in a batch; a cancelled batch stops.

## Limits (read before trusting a ✅)

* **Verified on synthetic portraits only** (`lightcraft_retouch::portrait::fixture`: a procedural face with
  planted defects). No real face photo was used — none is in the repository (the corpus is CC0 scenes), and
  the face model was not installed in this session. Real skin, hair and lighting will expose cases the
  synthetic face cannot: the thresholds are relative to measured skin statistics, not tuned on real faces.
* The mesh is MediaPipe's; faces turned > ~45°, covered by hair or glasses, or very small (< 40 px wide)
  are skipped or poorly covered.
* `smooth` is frequency separation, not a learned retoucher: it does not invent skin and cannot fix
  shape; at 100 it looks processed.
* `blemishes` rejects elongated and large marks on purpose (scars, moles, beard stubble stay); a pimple
  on a hair-covered or shadowed edge can be missed.
* `flyaways` handles thin hairs over a smooth background only; over busy backgrounds nothing is touched.
* `double_chin` lifts the shadow; it does not change the shape.
* Pixels are made at the scene-linear stage as 8 bit sRGB: very dark under-exposed raws are posterized
  in the patch's shadows. Faces are limited to the 4096 px render (patch resolution).
* Several faces: each face is retouched in turn on the shared picture; overlapping crops compose.
* The patch is a stored result: copying a photo's settings to another photo copies the steps but not the
  face; run `portrait.apply` on the target.
