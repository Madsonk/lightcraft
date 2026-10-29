# Depth (Lens Blur, Depth Range) and inpainting (object removal)

LightCraft estimates the depth of a photo with **Depth Anything V2 Small** and fills a selected
area with **MI-GAN**. Both models run in pure Rust on [candle](https://github.com/huggingface/candle)
(`crates/depth`: Metal on macOS, CPU elsewhere; `crates/inpaint`: CPU, a small interpreter of the model's ONNX
graph), and **neither is part of LightCraft**: like SAM 3 (see [ai-masks.md](ai-masks.md)) they are optional, downloaded
only when the user asks, after being shown their licence, and verified by SHA-256. Without them nothing else
changes; a photo that already has its depth map or its fill renders the same without the model.

## What they do

| Command | Params | Result |
|---|---|---|
| `depth.estimate` | `{ids?}` (the selection by default) | depth map of each photo *as developed* (uncropped, no masks, no Lens Blur) → `{photos: [{id, width, height, seconds, device}]}` |
| `depth.export` | `{path}` | the active photo's depth map as a 16-bit grayscale PNG (white = near) |
| `lensblur.set` | `{enabled?, amount?, focus?, point?, range?}` | Lens Blur on the active photo (one undo step); `amount` 0..100 (radius: 2 % of the long edge at 100), `focus` 0..100 (the depth kept sharp, 0 far … 100 near), `point` `[x, y]` normalized (focus on what is there), `range` 0..100 (the depth band kept sharp). On by default when `amount > 0`. |
| `mask.add` | `{kind: "depthRange", lo, hi, feather}` | a mask by depth, `lo`..`hi` from the nearest (0) to the farthest (1) of the photo with a feathered falloff (combines like any mask) |
| `spot.inpaint` | `{rect?, ellipse?, points?, size?}` (normalized photo coordinates) | fills the selection (a spot in `inpaint` mode, one undo step) → `{index, patch, seconds}` |
| `depth.model.status` / `.download {acknowledged: true}` / `.cancel` | | the depth model: installed?, licence, progress; `download` is refused without `acknowledged` |
| `inpaint.model.status` / `.download {acknowledged: true}` / `.cancel` | | the same for the inpainting model |

There is no panel for them in the app yet: they are control-protocol commands (CLI `lightcraft-cli run`,
MCP, the app's control channel), like `spot.findDust` was before its button.

```sh
lightcraft-cli run --import photo.CR2 \
  depth.estimate lensblur.set '{"point":[0.4,0.45],"amount":70,"range":10}' \
  spot.inpaint '{"ellipse":{"center":[0.67,0.44],"rx":0.04,"ry":0.03}}' \
  app.export path=out.jpg longEdge=2000
```

## How it works

**Depth.** The photo as developed is shown to the network at a short side of 518 px (multiple of 14, at
most 6000 patches). The relative inverse depth it returns is normalized per photo (2nd..98th percentile →
0..1) and kept as 16-bit cells in `<library>/depth/<hash of the content>.lcd` (in memory without a library).
It is **not** in the catalog's journal or in the settings: the engine attaches it to the settings of every
render that needs it (`DevelopSettings::aux`, never serialized, its identity goes into the cache keys).

**Lens Blur** (`crates/pipeline/src/lensblur.rs`, in the scene-linear stage, so bokeh keeps its highlights).
The depth map is resampled to the output and snapped to the photo's own edges with a guided filter. Each
pixel gets a signed circle of confusion from its distance to the focal band (negative behind the focus,
positive in front) and belongs to the two nearest of up to 21 layers; each layer is blurred with a disc of
its own radius (premultiplied by its membership, on a reduced copy for large radii), and the layers are
composited back to front, then divided by the accumulated coverage. That keeps the sharp subject sharp,
lets a blurred foreground bleed over the sharp area behind it, and avoids the halo of a plain "blur by
depth". Lens Blur and inpainting run on the CPU stage (also in the GPU renderer, which hands the linear
stage to the CPU, like spot removal).

**Inpainting** (`crates/inpaint`). The ONNX file is decoded by a small protobuf reader and run by a small
interpreter of the dozen operators it uses (convolutions, resizes, pads, shape arithmetic on the host);
depthwise convolutions are sums of shifted copies, 1 × 1 convolutions are matrix products, and the network's
`LeakyRelu × √2` is one fused step. Against onnxruntime on the same 512 × 512 input it differs by at most 3
levels of 255 (mean 0.03). A square of context (2.2 × the selection's box, at least 96 px, inside the
photo) is cut from the photo at the scene-linear stage (a 2560 px working copy, as 8-bit sRGB), stretched to
the model's 512 × 512 input and filled; the fill is brought back to the crop's resolution, its tone is
matched to the photo around the area, and it is kept with a feathered alpha as `<library>/inpaint/<id>.lcp`.
The edit is a `Spot` with `mode: "inpaint"` naming that file, so the settings stay small, undo works, and
renders composite the stored pixels without the model.

## Getting the models

In the commands above, `*.model.download {"acknowledged": true}` downloads them in the background (resume,
timeouts, SHA-256, mirrors: environment variables `LIGHTCRAFT_DEPTH_MIRRORS` / `LIGHTCRAFT_INPAINT_MIRRORS`,
then `models/depth-mirrors.txt` / `inpaint-mirrors.txt` in the settings folder, then the built-in official
repositories below). Only after the user has agreed to the download **and its licence**.

By hand, put the file in the folder named by `LIGHTCRAFT_DEPTH_DIR` / `LIGHTCRAFT_INPAINT_DIR`, or
`models/depth/` / `models/inpaint/` in the settings folder (`~/Library/Application Support/LightCraft/` on
macOS):

| Model | File | Size | SHA-256 | Source |
|---|---|---:|---|---|
| Depth Anything V2 Small | `model.safetensors` | 99 173 660 | `3152477ce0d8d6978d76b995120de97cb5b928701fd0f817769f59e249a16b70` | `depth-anything/Depth-Anything-V2-Small-hf` (Hugging Face, public) |
| MI-GAN 512 (Places2) | `migan.onnx` | 29 546 882 | `593eba0b7e04730f1b61c0a3cbca68d97d8d6a7ff5c6a44a7b9d7fcd880fc5ae` | `andraniksargsyan/migan` (Hugging Face, public; the ONNX export of MI-GAN's first author) |

## Licences (read at the source, 8 Oct 2026)

LightCraft's code is MIT OR Apache-2.0. `crates/depth` is a port of Apache-2.0 code and is Apache-2.0
only (see NOTICE); `crates/inpaint` is original code under the project's licence.

- **Depth Anything V2 Small — Apache-2.0.** The model card of `depth-anything/Depth-Anything-V2-Small-hf`
  declares `license: apache-2.0`, and the upstream README says: "Depth-Anything-V2-Small model is under the
  Apache-2.0 license. Depth-Anything-V2-Base/Large/Giant models are under the CC-BY-NC-4.0 license." **Only the
  Small model is supported; the others are non-commercial and must never be added.** The files used by the
  `huggingface/candle` example (`lmz/candle-dino-v2`: cc-by-nc-4.0; `jeroenvlek/depth-anything-v2-safetensors`:
  no licence) are **not** used: `crates/depth` reads the official checkpoint's own tensor names.
  *Not checked:* the licences of the data the Depth Anything authors trained on (synthetic sets, and 62 M
  unlabeled real images); the authors release the Small weights under Apache-2.0 and LightCraft relies on that.
- **MI-GAN — MIT, code and weights.** `LICENSE` and `LICENSE-WEIGHTS` of Picsart-AI-Research/MI-GAN are the MIT
  licence (Copyright (c) 2024 Picsart AI Research), and so is the `LICENSE` of the ONNX export repository
  (`license: mit` on its card). **Open question for the maintainers:** the model was trained on Places2 (scenes;
  its terms of use say "You will use the data only for non-commercial research and educational purposes") and
  FFHQ (the dataset is CC BY-NC-SA 4.0; its images are CC BY / BY-NC / public domain). The weights' own licence
  is MIT, and LightCraft ships no weights, but whether weights trained on non-commercial data are themselves
  free for commercial use is not settled by those texts. LaMa (Apache-2.0 weights) has the same Places2 origin.
  The download dialog should show the MIT licence **and** this training-data note.
- **No C, C++ or assembly.** `cargo tree -p lightcraft-inpaint -e normal,build` lists no `cc`, `cmake`,
  `pkg-config` or `*-sys` crate (same for `lightcraft-depth`). The ONNX runtime [tract](https://github.com/sonos/tract)
  (MIT OR Apache-2.0) was tried first and ran the model, but its `tract-linalg` build compiles hand-written
  arm64 / x86-64 assembly kernels through the `cc` crate, which the project avoids (like `nasm` for AVIF); that
  is why the model is run by `crates/inpaint/src/exec.rs` instead.

## Limits

- The depth is *relative*: a monocular model cannot know distances, only order and rough shape. Fine
  structures (hair, branches, wire fences), glass and mirrors get a wrong or smoothed depth: hair strands
  that the model puts on the background are blurred with it (a visible smear near flyaway hair).
- The focus band is a range of that relative depth: a subject that spans a lot of it (a full-length
  portrait) needs a wider `range` than a face.
- Lens Blur rebuilds nothing behind the subject: what a blurred foreground uncovers is extended from its
  neighbours.
- A fill takes seconds on the CPU (see the report; Metal was slower than the CPU for this network in candle).
- MI-GAN fills 512 × 512 at a time and the fill is resampled to the photo: it is convincing on out-of-focus
  backgrounds and mixed textures, weak on smooth surfaces with sparse specks (a pale smudge), on repeating
  structures (windows of a facade) and on large areas (a hand with a phone). The fill's detail is that of a
  2560 px working copy.
- The depth map and the fill belong to the photo's file in the library: syncing the settings to another photo
  copies the spot but not its pixels (it then renders nothing); undo and presets behave as for other edits.

## Measures

See the report of the change that introduced this document (photos, contact sheets, timings); the test
photos are not in the repository.
