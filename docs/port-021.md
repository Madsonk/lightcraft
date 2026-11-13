# Port of our LightCraft changes onto upstream v0.21.0

> Branch `port-0.21` (from `madson`, base upstream `v0.20.0` 192ff729), 2026-11-13.
> Merge commit: "Port 0.21.0: merge upstream v0.21.0 into madson (union fork features)".
> Principle (unchanged since port 0.14): when upstream already provides a function, prefer
> upstream and keep only what ours adds.

## 1. Shape of this port

| | Files |
|---|---|
| Fork delta vs v0.20.0 | 314 (208 new, 106 edits of upstream files) |
| Upstream v0.21.0 change | 112 |
| **Real conflicts** | **4** (`engine/Cargo.toml`, `pipeline/src/lib.rs`, `ui-egui/src/headless.rs`, `docs/parity-checklist.md`) |

Everything else merged automatically and was checked, not assumed: `locales/fr.json` has all
4 287 keys (650 fork + 28 upstream), `engine/src/segment.rs` carries both `MAX_CLICKS` and
`Tag::Subject`.

## 2. The four conflicts

| File | Resolution (union) |
|---|---|
| `crates/engine/Cargo.toml` | `siphasher` (fork) and `crc32fast` (upstream: CRC-32 of a catalog unpacked from a Lightroom backup zip) |
| `crates/ui-egui/src/headless.rs` | the fork's three "video busy" conditions and upstream's `survey_motion.moving(...)` |
| `crates/pipeline/src/lib.rs` | modules `inpaint` + `lensblur` (fork) and `lens_blur` (upstream), plus the interim gate below |
| `docs/parity-checklist.md` | both `LR-EDIT-LENSBLUR` rows (the fork's depth row and upstream's row) |

## 3. Two Lens Blurs on the same ground (the port's real decision)

- **fork**: `crates/pipeline/src/lensblur.rs`, a disc blur driven by the **depth map**
  (`lensblur.set`, `focal_distance`/`focal_range`), applied in `lin_cpu`.
- **upstream 0.21**: `crates/pipeline/src/lens_blur.rs` (1 109 lines), a blur **without depth**:
  subject estimate, `focus_mask`, bokeh, per-mask bokeh, Brush Refinement. Its `LensBlur` struct
  documents `focal_distance` / `focal_range` as `Reserved for a depth map (not rendered yet)`.

A plain union would have blurred twice (both paths run, no conflict reported). The fork's test
`tests_ai::lens_blur_uses_the_attached_depth_map_and_changes_the_render` caught it.

Resolution, documented in the code as `PORT 0.21 (interim gate)`:

- a photo **carrying a depth map** keeps the fork's depth-driven Lens Blur (in `lin_cpu` on the
  CPU renderer, and in `crates/gpu/src/render.rs` for the GPU one);
- **otherwise** upstream's subject/mask Lens Blur applies.

The fork's test now asserts the merged semantics: without a depth map the render **changes**
(upstream's subject blur runs) instead of staying untouched.

**Follow-up (not done here)**: fold the depth map into `lens_blur` as a circle-of-confusion
source and delete `crates/pipeline/src/lensblur.rs`. That removes three files from the delta for
good, and is a plausible upstream contribution.

## 4. Other fixes the merge needed

- `crates/engine/src/retouch/mod.rs`: `scale_adjust` now scales the `LocalAdjustments` Lens Blur
  fields and copies its switches (a scale must not turn a blur on or off).
- Five clippy lints of the fork surfaced under `-D warnings`: `honest` import gated on
  `feature = "search"`, `Honest` marked `cfg_attr(not(feature = "search"), allow(dead_code))`,
  `use super::*` in `restore.rs` made explicit, the `settle` tail's `unreachable!` allowed with a
  reason, and `panels/detail.rs::show` allowed for cognitive complexity (one draw pass).

## 5. Verification (this machine, 2026-11-13)

| Gate | Result |
|---|---|
| `cargo fmt --check` | clean (after `cargo fmt`) |
| `cargo check -p lightcraft-engine --features faces,autoseg -p lightcraft-ui-egui -p lightcraft-cli` | ok |
| `cargo clippy -p lightcraft-engine --features faces,autoseg -- -D warnings` | clean |
| `cargo clippy -p lightcraft-ui-egui -p lightcraft-cli -- -D warnings` | clean |
| `cargo test -p lightcraft-engine --features faces,autoseg --lib` | 1 782 passed, 0 failed, 15 ignored (200 s) |
| Fork crates tests (culler, retouch, facemesh, autoseg, style, taste, search) | 127 passed, 0 failed |
| `cargo build --release -p lightcraft --features heif` | ok (12 min 24 s, 142 MB) |
| Bundle | `LightCraft.app`, `ai.storyteller.lightcraft`, `0.21.0-madson1`, commit `64273c0d`, ad-hoc signed, `codesign --verify --strict --deep` valid |
| Control-channel smoke test (in-memory demo library) | 765 commands; fork and upstream cohabit (`lensblur.set` + `lensBlur.findSubject`); `cull.run` judged 24 photos in 2.46 s (10 selected, 2 highlight, 7 review, 5 rejected, 24 scenes); `app.export` wrote a 4000×6000 JPEG; `ui.screenshot` 1600×1000 |

## 6. Known gaps

- `crates/pipeline/src/lib.rs` edits upstream's render loop for the gate; the next port should
  replace it with the depth-aware `lens_blur` (section 3).
- The auto-updater offers v0.21.1 upstream: the fork will need the same ritual for it (expected to
  be nearly empty).
- CI (`port-check`) is unchanged here; the hardened version (rerere cache, dry merge, gates, PR
  instead of a direct push) is still to be proposed.
