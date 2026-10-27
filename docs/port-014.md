# Port of our LightCraft changes onto upstream v0.14.0

> Branch `lc-port-014` (from the upstream tag `v0.14.0`, 03c88c16). Written 2026-10-27.
> Source of the changes: branch `lc-ux-refonte` (107 non-merge commits on top of upstream `v0.5.0`).
> Principle (maintainer): independent of Adobe; take the idea and do better with our code; **when
> upstream 0.14.0 already provides a function, prefer upstream** and keep only what ours adds.

## 1. What our fork adds over upstream 0.5.0 (inventory)

`git log --no-merges v0.5.0..lc-ux-refonte` lists 107 commits. They fall in seven groups.

| Group | Commits (first … last) | What it is |
|---|---|---|
| A. Local base "v6 … v12" | `01a688be` … `ebc08b01` (HEIC/AVIF, Analyze Faces, AI denoise, depth + inpaint, semantic search, v6 snapshot, v9 menus / Sorting Help, v10 proposed retouch, v11 workspaces, v12 library steps) | A private line of work made before upstream shipped its own versions of most of it. |
| B. Video Catcher merged | `13e0a110` (base v16) … `6e443925` (≈ 35 commits) | Faces of videos in People, annotation store (`describe.*`), `video.*` commands, Videos view, silent preview, unreadable videos, `.LRV` skip, bundles and docs. |
| C. AI culling (lot A) | `58941595`, `5ce74ffc`, `0f742d37`, `64149e8a`, `860baf0d`, `b058bc95`, `079e574f`, `d7defe04` | `lightcraft-culler`, `cull.*`, `cull_job.rs`, verdicts in the catalogue, calibrated on real shoots. |
| D. Portrait retouch (lot C) | `3cd1c527`, `3186e441`, `9fc2857f`, `32c36595`, `e3a4ff26`, `10ed406a`, `8c3bac4d` | `retouch::portrait` (pure steps), `PortraitStack`, `portrait.apply / preview / reset / history / batch / auto / cancel / list / remove / openEyes`, off the UI thread. |
| E. Performance (lot E) | `ca036d1b` | `perf_bench` example, `tools/perf.sh`, `docs/performance.md`, preview read from the head of a raw. |
| F. Interface (lot D) and UX refonte | `13e54…`-series "Lot D" (14 commits), `828e9cc6`, `c19b6a9d`, `43205cc7`, `fbfd33b0` | "AI picks" section, Portrait tool, "cull and retouch in one click", guided tour, hints, `design.rs`, `journey.rs` (Import → Sort → Retouch → Export bar), `home.rs`, `quick_sort.rs`, `classify.rs`, `context_bar.rs`, `workspaces.rs`. |
| G. Fixes and French | `ba85ecd2` clippy, `a37020c2` library lock, `77124cdf` ui tests, `3f2f859a` / `f81d38c1` French | Mechanical fixes; French catalogue `fr.json` / `fr-formats.json`. |

## 2. Decision per function

Upstream 0.14.0 was read (`docs/roadmap.md`, `docs/whats-new.md`, crates list, command ids) before each
decision. *Port* = our code brought onto the 0.14.0 API; *Keep upstream* = nothing of ours is carried;
*Merge* = upstream's piece stays and ours is added on top.

| Function | Upstream 0.14.0 | Decision | Reason |
|---|---|---|---|
| French interface | `locales/fr.json` + `fr-formats.json` exist (2 392 messages) | **Keep upstream**, add only our new messages | Complete and maintained; ours differs in 677 shared messages, mostly wording. Our 589 extra keys belong to features not ported. |
| People / face recognition | `lightcraft-faces` (YuNet + ArcFace), People view, `faces.*` | **Keep upstream** | Same job. Our `people` crate is dropped. |
| Places (reverse geocoding) | `lightcraft-geocode` | **Keep upstream** | Same job. |
| Lens profiles | `lightcraft-lensdb` | **Keep upstream** | Same job. |
| AI denoise | `lightcraft-denoise` | **Keep upstream** | Same job. (Our SCUNet / SPAN `restore` crate not ported.) |
| Slideshow / web gallery | `lightcraft-slideshow` | **Keep upstream** | Another session handles galleries. |
| Raw preview read from the head of the file | `docs/raw-preview-reads.md`: partial preview reads are in | **Keep upstream** | Upstream made the same optimisation. |
| Assisted culling (`photo.analyze`, bursts) | present, simpler than ours | **Merge** | Upstream's command stays; it now keeps the faces and the verdict already stored on a photo. |
| **AI culling** (`lightcraft-culler`, `cull.*`) | nothing | **Port** | Verdicts, reasons, duplicate groups, scenes, genres. |
| **Analyze Faces** (MediaPipe mesh: eyes open / closed, eye sharpness) | YuNet only (5 points, no eye openness) | **Port** (`lightcraft-facemesh`, `photo.analyzeFaces`) | Needed by culling and by the portrait sliders. |
| **Portrait retouch by sliders** (`lightcraft-retouch`, `portrait.*`) | nothing | **Port** | Not in upstream. |
| Portrait masks (`portrait.retouch / clear`, dust) | nothing | **Port** | Provide the face geometry the sliders use. |
| Automatic masks (SegFormer, ISNet: `mask.auto`) | SAM 3 only (Object / Describe); Subject / Sky / People listed as open | **Port** (`lightcraft-autoseg`) | Needed for the Background slider. |
| Baked pixels carried by a spot (`SpotMode::Inpaint`, `Aux`, `pipeline::inpaint`) | PatchMatch fill only | **Port, reduced** | Patch part only; depth map (Lens Blur, Depth Range) and the MI-GAN model are left out. |
| "AI picks" panel, Portrait tool, one click, tour, grid verdict dot | nothing | **Port** into upstream's right panel and tool strip | Window ▸ AI Picks (Shift+K), Portrait Retouch (Shift+T); entries under Photo. |
| UX refonte: journey bar, home, quick sort, classify, workspaces | Lightroom-style modules | **Not ported** (§4) | It replaces the shell (spaces Library / Retouch / Create). Upstream's shell differs; porting means redesigning it. `design.rs` is ported because the panels use it. |
| Video Catcher (`video.*`, `describe.*`, Videos view) | M14 Video not started | **Not ported yet** (§4) | Depends on our `people`, `search`, `taste` crates, which upstream replaced; needs a rewrite on `lightcraft-faces`. |
| Semantic search (SigLIP), taste | not started | **Not ported yet** | Prerequisites of `describe.*`. |
| Depth map, Lens Blur, Depth Range, MI-GAN inpaint | depth not started | **Not ported** | Out of the portrait path. |
| Proposed retouch, skin harmonize, dodge & burn | nothing | **Not ported** | Not asked for in this port. |
| Library lock retry | not in upstream | **Not ported** | Only needed while a video player is launched. |
| `perf_bench` + `tools/perf.sh` | none | **Not ported yet** | Measurement tooling. |
| Extension points for lot B (profiles) and galleries | — | Kept free | `style.*` and `autocrop.*` ids are not used; the command registry is untouched. |

## 3. State of the port (2026-10-27)

Ports compile and run. Evidence (commands run on this machine, logs in `/Volumes/Data/work/lc-port-*.log`):

| Check | Result |
|---|---|
| `cargo check -p lightcraft-engine --features faces,autoseg`, `-p lightcraft-ui-egui -p lightcraft-cli` | OK |
| `cargo test --lib` culler / retouch / facemesh / autoseg / develop | 2 + 16 + 31 + 15 + 55 passed, 0 failed |
| `cargo test -p lightcraft-engine --features faces,autoseg --lib -- cull portrait facemesh autoseg dust` | 49 passed, 0 failed, 4 ignored |
| `cargo clippy … -- -D warnings` on culler, facemesh, retouch, autoseg, catalog, develop, pipeline, engine, ui-egui, cli | clean |
| `cargo build --release -p lightcraft --features heif` (CRAFT_FONTS_DIR set) | OK, 23 min, 121 MB |
| Bundle `/Volumes/Data/work/lightcraft-port-bundle/LightCraft.app` | id `ai.storyteller.lightcraft`, `0.14.0`, `LightCraftLocalBuild 0.14.0-madson1`, ad-hoc signed, `codesign --verify --strict --deep` valid |

Real run of the bundle (control channel, throw-away library, 30 CR2 of the 2024-12-08 food shoot copied to the internal disk,
copy and library deleted afterwards): import 30 photos; `cull.run` judged 30 in 3.5 s (2 highlight, 9 selected, 2 review,
17 rejected, 10 duplicate groups, 4 scenes); `cull.apply` rated 30, picked 2, rejected 3; `photo.analyzeFaces` loaded
the MediaPipe model and found 0 faces; `portrait.apply` / `portrait.auto` on a face-less photo answered
`skipped: no face found` and changed nothing; `portrait.openEyes` answered "no face with closed eyes is known…";
`app.export` wrote three 1600 px JPEGs. Screenshots (UI AI picks and Portrait panels, no faces):
`/Volumes/Data/work/lightcraft-port-bundle/verification/`.

Order of dependency, as committed: crates `culler`, `facemesh`, `retouch`, `autoseg` (copied unchanged) →
`catalog` (`FaceInfo`, `Analysis.faces`, `Analysis.verdict`, filter `faces`, rule fields) → `develop`
(`PortraitStack`, `InpaintPatch`, `Aux`, `SpotMode::Inpaint`, `Spot.patch`) → `pipeline` (`inpaint.rs`, `linear_stage`,
`cull::exposure`) → `engine` (`cull_job`, `facemesh`, `portrait*`, `autoseg`, `dust`, reduced `ai.rs`, patches
attached in `media.rs`, `photo.analyze` weighs open eyes and takes `rejectClosedEyes`) → `ui-egui` (`design`, `flow`,
`panels/cull`, `panels/portrait`, tool strip, menus, grid dot and "Eyes closed" pill) → apps (features `faces`, `autoseg`).

Known problems (honest list):
- The AI picks panel says "No verdicts yet" when the cull was started by command (`cull.run`) instead of its own button:
  the panel's list is loaded by its own button. The verdict dots and the active photo's reasons are right.
- `cull.run` rejected 17 of 30 on a food shoot (soft focus and duplicates, genre "general"): calibrated on people shoots,
  not on food; `cull.apply` only flags 3 as Reject.
- Portrait retouch with a real face was exercised by the unit tests (synthetic faces) only, not on a real portrait.
- `ui-egui` tests: the filtered run compiled (28 min) but its binary stayed frozen at launch (macOS blocks new executables again), so no UI test result exists; `cargo xtask ci`, the wasm check and the web build were not run.
- The engine without the `faces` / `autoseg` features (web build) compiles with dead-code warnings, untested for wasm.
- Models (face mesh, ISNet, SegFormer) are not in the bundle; they are downloaded on request as in the fork.

## 4. Left out on purpose
- **UX refonte shell** (journey bar, spaces, home, quick sort, classify): replaces the whole window; upstream's
  Lightroom-style modules stay. A thin journey bar mapped onto upstream's modules can come later.
- **Video Catcher** (`video.*`, `describe.*`, Videos view): rewrite on `lightcraft-faces` instead of our `people` crate.
- Semantic search, taste, depth / Lens Blur, MI-GAN inpainting, proposals, `perf_bench`.

## 5. Verification commands (run one build at a time, see the lock rule)
```bash
source /Volumes/Data/work/lc-port-env.sh          # RUSTUP_HOME, PATH, CARGO_BUILD_JOBS=3, CARGO_TARGET_DIR, lcbuild()
cd /Volumes/Data/work/lc-port-014
lcbuild cargo check -p lightcraft-engine --features faces,autoseg --lib
lcbuild cargo fmt --all -- --check
lcbuild cargo clippy -p lightcraft-culler -p lightcraft-facemesh -p lightcraft-retouch -p lightcraft-autoseg \
        -p lightcraft-catalog -p lightcraft-develop -p lightcraft-pipeline -p lightcraft-engine -p lightcraft-ui-egui \
        --all-targets --features lightcraft-engine/faces,lightcraft-engine/autoseg -- -D warnings
lcbuild cargo test -p lightcraft-culler -p lightcraft-retouch -p lightcraft-facemesh
lcbuild cargo test -p lightcraft-engine --features faces,autoseg --lib -- cull portrait facemesh autoseg
lcbuild cargo test -p lightcraft-ui-egui --lib -- panels tests_menubar tests_panels
lcbuild cargo xtask layers && lcbuild cargo xtask assets
# release build and bundle
lcbuild cargo build --release -p lightcraft --features lightcraft/heif
codesign --force -s - /Volumes/Data/work/lightcraft-port-target/release/lightcraft
/Volumes/Data/work/lc-port-bundle.sh /Volumes/Data/work/lightcraft-port-target/release/lightcraft \
        /Volumes/Data/work/lightcraft-port-bundle "$(git rev-parse HEAD)" madson1
```
The bundle copies the launcher's own `Info.plist` (id `ai.storyteller.lightcraft`, version `0.14.0`), adds
`LightCraftLocalBuild = 0.14.0-madson1`, and is signed ad hoc. Real-photo check: copy at most 40 face-free photos
(e.g. `2024/2024-12-08`) to the internal disk, import them into a throw-away library, run Import → Sort (AI picks) →
Portrait (no face: the message must be clear) → Export, take headless screenshots outside the repository, delete the copy.

## 6. Re-applying our changes when the launcher updates LightCraft (0.15.0 and later)

The launcher records `lightcraft` in `~/Library/Application Support/ArtCraft Launcher/installed.json`
(`version`, `path`) and replaces the app at that path when it installs a newer release. **It will overwrite our
build**, and we found no log line showing how it decides an app is up to date, so assume any update does.

1. `git fetch origin --tags`; `git worktree add -b lc-port-015 ../lc-port-015 v0.15.0`.
2. `git cherry-pick` the port commits of `lc-port-014` in order (`git log --oneline v0.14.0..lc-port-014`); each is
   small and one theme, so conflicts stay local. The risky files are the shared ones: `catalog/src/model.rs`
   (`Analysis`), `develop/src/settings.rs` (`DevelopSettings`, `Spot`), `engine/src/lib.rs` (modules, `Session`
   fields), `engine/src/cmd/mod.rs` (spec list), `ui-egui/src/{lib,state,menus,menubar}.rs`, `panels/{right,strip,grid}.rs`.
3. Check upstream's changelog (`docs/whats-new.md`, `docs/roadmap.md`) for anything that now replaces a ported piece
   (culling, portrait, eye analysis, automatic masks) and drop ours in favour of it.
4. Run §5 and rebuild the bundle with the new version number in the reference plist.
5. Keep a copy of the bundle outside `/Volumes/Madson/ArtCraft/` (here `/Volumes/Data/work/lightcraft-port-bundle/`)
   and do not edit `installed.json` without proof of how the launcher uses it.

## 7. Environment problem met during the port
From 06:33 on 2026-10-27, new executables on this Mac stayed in `_dyld_start` for 30 minutes and more (an ad-hoc
signed copy of `/bin/echo` too; cargo build scripts of every session). `syspolicyd` was working through a flood of
dylib assessments from another process. Rebooting or restarting `syspolicyd`, or adding the terminal app under
Privacy & Security ▸ Developer Tools, is the way out; building on the internal disk does not help.
