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
* **Model** (`model.rs`, `train.rs`): one closed-form ridge regression per slider (a copy of `lightcraft-taste`'s solver, vendored as
  `ridge.rs` / `linalg.rs` / `folds.rs`: deterministic, no random start, no iteration). The regularisation is chosen by an *inner* grouped
  cross-validation inside each training fold, so the reported error is not tuned on the photos it is measured on.
  Folds hold out **whole shoots** (folders) when there are enough, so near-identical frames never leak.
* **Honesty gate.** A slider whose cross-validated error does not beat **both** the photographer's median and
  "no adjustment" by 3 % is marked *not learned*: the profile suggests the better of those two (the median, or
  nothing) and `style.apply` leaves it untouched. The panel shows which sliders were learned and the measured
  error of each next to the baselines.
* **Profile file.** `<library>/style/<name>.json` (about 35 KB), versioned; a corrupt or foreign file is skipped,
  never an error. A library without a folder (tests, the demo) keeps profiles in memory.

## Measured

All numbers below come from runs on 28 October 2026 (release/optimised test builds, Apple silicon).

### 1. Synthetic photographer (known truth)

`cargo test -p lightcraft-style --release report -- --ignored --nocapture`. A made-up photographer whose edit
is a known function of the photo plus noise; `curve_shadows` is pure noise; whites, blacks, saturation and two
curve zones do not depend on anything measured. Held-out photos: 300 never seen, 30 other shoots.

| training photos (shoots) | train | predict | profile | sliders learned |
|---|---|---|---|---|
| 60 (6) | 0.01 s | 0.1 µs / photo | 34 KB | 8 / 14 |
| 222 (22) | 0.02 s | 0.1 µs / photo | 34 KB | 8 / 14 |
| 1000 (60) | 0.06 s | 0.1 µs / photo | 34 KB | 8 / 14 |

(Feature extraction is separate: it reads the photo thumbnail, ~ms; the 0.64 s / photo seen on real raws below
is almost all decoding from a USB disk.) Held-out error (slider units) with 222 training photos:

| slider | model | median | no adjustment |
|---|---|---|---|
| exposure (EV) | 0.05 | 0.26 | 0.39 |
| temperature (mired) | 12.4 | 17.9 | 22.6 |
| tint | 14.0 | 21.1 | 21.2 |
| highlights | 2.4 | 6.4 | 6.3 |
| shadows | 2.1 | 5.9 | 13.8 |
| vibrance | 3.2 | 4.2 | 4.2 |
| curve_shadows (pure noise) | 4.3 *not learned* | 4.3 | 4.3 |

The learner finds the rule when there is one (error divided by 2 to 5 against the median), refuses the noise
slider at every sample size, and its results do not move between 60 and 1000 photos on this easy problem.
This validates the machinery, **not** the quality on a real photographer.

### 2. Madson's catalogue (real)

Lightroom catalogue `toutes les photos-v13-3.lrcat` (read on a copy, `library.importLightroom`): 14 192 photos
imported, **314 carry an edit that moves at least one learnable slider, 222 of them with their original on disk**
(CR2, from a USB disk). `style.train`: 5-fold cross-validation holding out whole shoots (25 shoots), 222 photos.
Wall time 136 s (reading 222 raws from the USB disk, parallel); the learning itself 0.23 s; profile 36 KB.

Held-out mean absolute error (slider units, Lightroom scales), as first measured:

| slider | model | median | no adjustment | verdict |
|---|---|---|---|---|
| exposure | 0.52 | 0.49 | 0.68 | not learned |
| temperature (mired) | 19.4 | 17.5 | 16.2 | not learned |
| tint | **8.2** | 15.6 | 12.0 | learned |
| contrast | 9.6 | 9.6 | 8.3 | not learned |
| highlights | **21.2** | 24.8 | 81.7 | learned |
| shadows | **34.9** | 42.0 | 46.0 | learned |
| whites | 13.7 | 16.0 | **11.2** | was "learned" but worse than leaving it at 0 |
| blacks | 11.1 | 11.2 | 35.3 | not learned |
| vibrance | **16.7** | 20.5 | 31.0 | learned |
| saturation | 6.4 | 6.4 | 7.8 | not learned |
| curve zones | ≈ no better than 0 | | | not learned (used in ~1 % of photos) |

Two findings: (1) the honesty gate compared with the median only, so *whites* passed although doing nothing was
better; the gate now requires beating **both** the median and "no adjustment", and a slider that is not learned
suggests whichever of the two did better held-out (0 for whites, temperature, contrast). With that rule 4 sliders
are learned (tint, highlights, shadows, vibrance); the table above was not re-run with the new rule (the run takes
over an hour on this disk, almost all of it importing the 14 k-photo catalogue), but the held-out errors do not
depend on the gate, only on which column is used, so the new verdicts follow from the same numbers.
(2) Overall: skill vs "no adjustment" 0.23, vs the median 0.10: **a modest gain on four sliders, nothing
on exposure and temperature** — the two sliders a photographer would most want predicted. With 222 photos in 25
shoots this is a small sample (see Limits).

### 3. Straighten and crop on real photos

`real_framing_report`: 139 photos read in place (nothing written next to them) from four shoots of the same
disk: a hall shoot (27 RW2 + 1 JPG, camera on a tripod, rolled), a basketball game (37 CR2, indoor), cookie product
shots (≈ 37 CR2, plain background) and street portraits (≈ 38 CR2, faces not analysed). Time: 0.64 s / photo for
`autostraighten.suggest`, 0.80 s / photo for `autocrop.suggest` (dominated by decoding raws from the USB disk).
Judged by eye on contact sheets (before | straightened | straightened + cropped, 210 px tiles: sub-degree effects
are not visible, said so below):

| | photos | suggestions | good | neutral / imperceptible | poor |
|---|---|---|---|---|---|
| Straighten, hall series (true roll 2.5–7.7°) | 27 | 20 | 20 | 0 | 0 — **7 missed** (same roll, no suggestion) |
| Straighten, basketball | 37 | 14 | 8 (0.8–2.3°) | 6 (< 0.5°) | 0 seen |
| Straighten, cookies | ≈ 37 | 0 | – | – | correct silence |
| Straighten, street portraits | ≈ 38 | 0 | – | – | **≥ 3 likely missed** (river-bank horizon tilted) |
| Crop | 139 | 8 real crops (31 more are only the straighten angle) | 3 | 2 acceptable | **3 poor** |

Failures, by photo name in the printed report: crop `IMG_0191` (hoop cut at the top), `IMG_0053` and `IMG_0043`
of the cookie shoot (cookie pushed to a corner for the sake of a third, big empty area); straighten misses
`PANA2244/2243/2242/2234/2233/2231/2226` (same scene and same roll as their neighbours, but too few or
disagreeing lines) and the riverside portraits.
Straighten is the reliable half (no wrong correction seen; it follows verticals of door frames and walls, with a
consistent angle across a series); **crop is weak** — 3 good of 8, and it never suggests a crop on portraits
because faces were not analysed.

## Limits

* **222 edited photos is a small sample.** Aftershoot asks for thousands. On Madson's real edits only 4 sliders
  beat the baselines held out; exposure and temperature, the ones most wanted, did not. More data (and
  per-look profiles) is the first thing to do before claiming a style is learned.
* The model is linear (ridge regression) on 33 global statistics. It cannot see content: a portrait and a
  landscape with the same histogram get the same edit. No local adjustments, masks, grain or presets.
* A learned style is only as consistent as its photographer. Edits made for different looks or clients in one
  catalogue average out; train a profile per look.
* Crop: faces (Analyze Faces) or a colour / contrast saliency blob. The saliency subject is a heuristic and was
  wrong on 3 of 8 real crops; the thirds-and-resolution score is hand-weighted, not learned. Straighten needs
  dominant straight lines and stays silent otherwise (7 misses in one series; river-bank portraits).
* The judging of straighten / crop is one person's eye on 210 px contact sheets, not a measured angle error; real
  horizon ground truth (or Madson's verdict) is still to be collected. Sub-degree suggestions are not judged.
* The catalogue run was not repeated after the gate fix (see §2); numbers in §2 are as first measured.
* Not validated on a second photographer, nor by Madson's eye on the rendered style result.

## Porting to the 0.14.0 base

Done on 28 October 2026 on branch `lc-lot-b14` (from `lc-port-014`). The lot is two **self-contained crates**
(pure functions, no I/O, no network, no UI, `#![forbid(unsafe_code)]`, panic-free lints) and a thin integration
layer. The 0.14.0 base has neither the `taste` crate nor the series module, so the crates carry what they need.

Copy as they are:

* `crates/style/` (`lightcraft-style`): features, targets, ridge training with grouped cross-validation (vendored
  solver and folds), profile JSON, series harmonisation, synthetic photographer. Depends on `color`, `raster`,
  `develop`, `pipeline` (`auto::{auto_tone, auto_wb, current_wb}`, `SourceInfo`).
* `crates/autoframe/` (`lightcraft-autoframe`): straighten (reuses `pipeline::upright`) and crop search. Depends on
  `geom`, `raster`, `develop`, `pipeline`.

Integration (new files unless marked *edit*):

| File | What |
|---|---|
| `Cargo.toml`, `crates/engine/Cargo.toml` (*edit*) | the two path dependencies |
| `xtask/src/layers.rs` (*edit*) | `("style", Layer(3))`, `("autoframe", Layer(3))` |
| `crates/pipeline/src/upright.rs` (*edit*) | `Segment::mid` made `pub` |
| `crates/engine/src/style_job.rs` | the whole engine layer (`style.*`, `autostraighten.*`, `autocrop.*`), its own `checked_targets` and time-and-place `group_photos` |
| `crates/engine/src/lib.rs` (*edit*) | `pub mod style_job;`, field `pub style: style_job::StyleLab` (+ its `Default`), `mod tests_style;` |
| `crates/engine/src/cmd/mod.rs` (*edit*) | `v.extend(crate::style_job::specs());` |
| `crates/engine/src/tests_style.rs` | 7 engine tests on the demo library + two ignored real-data reports |
| `crates/ui-egui/src/panels/style.rs` | the "My style" / "Auto crop" panel (own Before / After tile) |
| `crates/ui-egui/src/state.rs`, `panels/{mod,right,strip}.rs`, `menus.rs` (*edit*) | `RightPanel::Style`, one match arm, a tool-strip entry, the `panel.style` menu entry and command |
| `crates/ui-egui/locales/fr.json` (*edit*) | 49 French strings |

The only registry ids are `style.*`, `autostraighten.*`, `autocrop.*`, `panel.style`. The 0.14.0 UI has no journey
bar, so the panel is a tool-strip entry (Sliders icon, French label « Mon style »); moving it into a journey step
is a one-line change in `strip.rs` once that bar exists on this base.
