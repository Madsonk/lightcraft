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

_(filled in below: synthetic ground truth, then Madson's archive.)_

## Limits

_(see the end of this page.)_
