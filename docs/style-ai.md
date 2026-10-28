# My style, auto straighten and auto crop (lot B)

Madson's brief (23 October 2026): take the *idea* of Aftershoot's AI editing — a profile learned from the
photographer's own edited photos, automatic straighten and crop — and do it better, in our own code, locally,
with no Adobe SDK, no network and no model of anybody else's. This page is the design, the numbers that were
measured and what is still missing. Where a number says "synthetic" it was measured against a photographer
made up for the test (known ground truth); where it says "real" it was measured on Madson's own archive.

## What it does

* **Learn my style** (`style.train`). From the photos you already edited — their develop settings, for instance
  imported from a Lightroom catalogue (`docs/lightroom-catalog-import.md`) — it learns how you set 14 sliders:
  exposure, temperature, tint, contrast, highlights, shadows, whites, blacks, vibrance, saturation and the four
  parametric tone-curve zones. Everything runs on this Mac, on a worker thread, with progress and Stop.
* **Apply my style** (`style.apply`). `preview` computes the suggestion for a selection on a worker (nothing
  changes; the interface renders Before / After from the proposal), `apply` commits it as **one undo step**
  (it needs `confirm: true`), `undo` reverts it. A photo edited since the preview is left alone; sliders you
  already moved on a photo are kept unless `overwrite: true`.
* **Series consistency.** Photos of one time-and-place series (the series v16 grouping, `docs/series.md`) get a
  shared style: every slider is pulled towards the series' median (`consistency`, 0.7 by default) — except
  exposure, where the *offset from each photo's own auto exposure* is shared, so a darker frame is still
  brightened more than a brighter one.
* **Auto straighten** (`autostraighten.suggest` / `.apply`). The pipeline's Upright analysis (gradient line
  segments, consensus of horizon or plumb lines) gives an angle; we add a confidence (share of the line length
  that agrees with the angle) and refuse to suggest below 0.6.
* **Auto crop** (`autocrop.suggest` / `.apply`). A search over rectangles inside the (straightened) image, scored
  for: the subject kept whole with room around it, the subject near a rule-of-thirds point, the horizon on a
  third, and as little resolution lost as possible. The subject is the faces Analyze Faces already found, else a
  colour/contrast saliency blob. It answers "the framing is already good" rather than crop for the sake of it.
  A requested aspect (`1x1`, `4x5`, `3x2`, `16x9`, `[w, h]`) is respected.
* Suggestions are **never applied without `confirm: true`**, are previewed first and are one undo step.

Commands: `style.train`, `style.status`, `style.list`, `style.apply`, `style.cancel`, `style.delete`,
`autostraighten.suggest`, `autostraighten.apply`, `autocrop.suggest`, `autocrop.apply` (CLI, MCP and the app all
dispatch the same ids). The interface is the **Style** tool of the Retouch step (`panel.style`, "Mon style" and
"Recadrage auto" in French).

## How it works

```text
decoded thumbnail (unedited) ──► 33 numbers (features.rs) ──► ridge regression per slider ──► 14 sliders
photographer's develop settings ──► 14 sliders (targets.rs) ───────────────┘
```

* **Features** (`crates/style/src/features.rs`): the pipeline's own auto tone and auto white balance outputs
  (so the model learns *your offset from auto* instead of rediscovering exposure), the as-shot white,
  log-luminance percentiles and spread, clipped and crushed shares, mean saturation, red/green and blue/green
  log ratios, a 3 × 3 luminance layout. Measured on the *unedited* pixels, never on the edit.
* **Targets** (`targets.rs`): sliders as they are, except white balance, which is learned as a *shift in mired
  from the as-shot white* plus a tint delta, because 5200 K means nothing without the camera's own white.
* **Model** (`model.rs`, `train.rs`): one closed-form ridge regression per slider (`lightcraft-taste`'s solver:
  deterministic, no random start, no iteration). The regularisation is chosen by an *inner* grouped
  cross-validation inside each training fold, so the reported error is not tuned on the photos it is measured on.
  Folds hold out **whole shoots** (folders) when there are enough, so near-identical frames never leak.
* **Honesty gate.** A slider whose cross-validated error does not beat the photographer's own **median** by 3 %
  is marked *not learned*: the profile suggests only the median and `style.apply` leaves it untouched. The
  panel shows which sliders were learned and the measured error of each next to "leave it at 0".
* **Profile file.** `<library>/style/<name>.json` (a few KB), versioned; a corrupt or foreign file is skipped,
  never an error. A library without a folder (tests, the demo) keeps profiles in memory.

## Measured

_Not measured yet: the first build of this lot could not run (the machine refused to start freshly built
executables on 27 October). The method is fixed so the numbers are comparable when they exist:_

* **Synthetic ground truth** (`lightcraft_style::synth`): a made-up photographer whose edit is a known function of
  the photo plus noise, one slider pure noise. Expected: the learned sliders beat "median" by well over the 3 %
  gate, the noise slider is *not learned*. Reported: held-out MAE per slider against *no adjustment* and
  *median adjustment*, training time (`summary.trainSeconds`), inference time per photo (features + predict), profile
  size (JSON bytes).
* **Madson's archive (real)**: the Lightroom catalogue v13 (read on a copy) has 14 702 photos of which 314 are
  edited and 222 still have their original. Import them into a scratch library, run `style.train`, report the
  leave-shoots-out error per slider against the same two baselines.
* **Straighten / crop**: run `autostraighten.suggest` and `autocrop.suggest` on ~100 real JPEG + CR2 photos, judge each
  suggestion image by image (good / acceptable / wrong), report the rate and list the failures.

## Limits

* **222 edited photos is a small sample.** Aftershoot asks for thousands. With about 33 numbers per photo and 14
  sliders, expect only the sliders that follow the photo's own brightness or white balance (exposure, temperature,
  shadows, highlights) to clear the honesty gate; the rest will honestly say "median only".
* The model is linear (ridge regression) on global statistics. It cannot see content: a portrait and a landscape
  with the same histogram get the same edit. No local adjustments, masks, grain or presets are predicted.
* A learned style is only as consistent as its photographer. Edits made for different clients or looks in one
  catalogue average out; train a profile per look.
* Crop suggestions use detected faces when Analyze Faces has run, otherwise a colour / contrast saliency blob, which
  is a heuristic and will be wrong on busy scenes. Straighten needs dominant straight lines; it stays silent
  otherwise rather than guess.
* Not validated yet on a second photographer, nor by Madson's eye on the rendered result.
* The pipeline's `taste` crate is missing from `xtask/src/layers.rs` TABLE on this base; `cargo xtask layers` may
  flag it (not introduced by this lot).


## Porting to the 0.14.0 base

The lot is built so the port is mostly a copy. Two **self-contained crates** (pure functions, no I/O, no network,
no UI, `#![forbid(unsafe_code)]`, panic-free lints) and a thin integration layer.

Copy as they are:

* `crates/style/` (`lightcraft-style`): features, targets, ridge training with grouped cross-validation, profile
  JSON, series harmonisation, synthetic photographer. Depends on `lightcraft-taste` (ridge solver, group folds),
  `develop`, `pipeline` (`auto::{auto_tone, auto_wb, current_wb}`, `SourceInfo`), `raster`, `color`.
* `crates/autoframe/` (`lightcraft-autoframe`): straighten (reuses `pipeline::upright`) and crop search. Depends on
  `geom`, `raster`, `develop`, `pipeline`.

Integration, in the order to port it (every file listed is new unless marked *edit*):

| File | What | Note |
|---|---|---|
| `Cargo.toml` (*edit*) | workspace members' path deps `lightcraft-style`, `lightcraft-autoframe` | 2 lines |
| `xtask/src/layers.rs` (*edit*) | `("style", Layer(3))`, `("autoframe", Layer(3))` | 2 lines |
| `crates/pipeline/src/upright.rs` (*edit*) | `Segment::mid` made `pub` | 1 word |
| `crates/engine/Cargo.toml` (*edit*) | the two dependencies | 2 lines |
| `crates/engine/src/style_job.rs` | the whole engine layer: `style.train/apply/status/list/cancel/delete`, `autostraighten.suggest/apply`, `autocrop.suggest/apply`, worker threads, profile files | one file, ~900 lines |
| `crates/engine/src/lib.rs` (*edit*) | `pub mod style_job;`, field `pub style: style_job::StyleLab` + its `Default` in `Session::new`, `#[cfg(test)] mod tests_style;` | 4 lines |
| `crates/engine/src/cmd/mod.rs` (*edit*) | `v.extend(crate::style_job::specs());` | 1 line |
| `crates/engine/src/series.rs` (*edit*) | the grouping body of `groups()` extracted as `pub(crate) fn group_photos`; `groups()` calls it | pure refactor |
| `crates/engine/src/tests_style.rs` | engine tests on the demo library | |
| `crates/ui-egui/src/panels/style.rs` | the "My style" / "Auto crop" panel | |
| `crates/ui-egui/src/state.rs` (*edit*) | `RightPanel::Style` | 1 variant |
| `crates/ui-egui/src/panels/mod.rs`, `panels/right.rs` (*edit*) | `pub mod style;`, one `match` arm | 2 lines |
| `crates/ui-egui/src/menus.rs` (*edit*) | `("panel.style", "My Style", None, "Window")` and its `"panel.style"` arm | 2 spots |
| `crates/ui-egui/src/panels/strip.rs` (*edit*) | `const STYLE: Tool` + one `tool(...)` call next to Portrait | |
| `crates/ui-egui/locales/fr.json` (*edit*) | 48 French strings (append) | |

Things the port must re-check on 0.14.0: the names `Session::source_ref_uncached`, `source_info`, `develop_op`,
`commit`, `checked_targets`, `library_generation`, `guard::catch`, `describe::ops::library_dir`;
`Photo::faces()` (rects normalised to the upright photo); `proposals::pair` (Before / After tile) in the UI;
`design::{button, status_line, section_title}`. Nothing else is shared with the rest of the application.
