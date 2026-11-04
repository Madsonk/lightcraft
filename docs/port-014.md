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
| **Video Catcher VID-1 → VID-5** (`video.*`, `describe.*`, Videos view) | M14 Video not started | **Port, rewritten on `lightcraft-faces`** (§3b) | Frames, clips, poster and probe, unreadable videos, `.LRV` skip, Video Catcher database import, annotation store and text search all ported; faces of videos rebuilt on YuNet + the chosen recognition model; the semantic (SigLIP) half is left out. |
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

## 3b. Video Catcher on 0.14.0 (VID-1 → VID-5)

| Piece | How it was ported |
|---|---|
| `video.frames / status / cancel / clip / list / moments / open / importVideoCatcher` | Copied (`crates/engine/src/video/`, `cmd/video.rs`). Hooks added to the 0.14.0 import (`ImportOptions.videos`, folders take videos only when asked, `ProbeInfo.duration`, poster and probe in `files.rs`, preview loader never reads a whole video) and `lightroom_catalog::read_sqlite_tables`. |
| `describe.set / confirm / delete / undo / list / suggest / search` | Copied (`crates/engine/src/describe/`). Text annotations and the who + what search work; `describe.suggest` and by-meaning ranking are unavailable (no SigLIP). `crate::search` is a 15-line module with the message and the key. |
| Faces of videos | **Rewritten** (`video/faces.rs`): `video.faces`, `video.facesStatus`, `video.facesCancel`. YuNet finds and aligns the faces of each picture, the chosen recognition model embeds them, results are kept in `<library>/video/faces-<model tag>.json`; moments show the people the **photos' named faces** suggest (same threshold and margin as `faces.suggest`). Nothing is ever written into a photo. Replaces the fork's `people.index` for videos. |
| Videos view | `panels/videos.rs` ported (view mode `Videos`, bottom bar button, View ▸ Videos, Import Videos…, Video Catcher import); the people parts now call `video.faces`. The fork's Recognition panel is not in 0.14.0: the faces of videos are on the person's page instead (VID-6, below). 117 French messages of ours already in `fr.json`, plus the Videos ones. |
| Asynchronous loading of the Videos view | Done in VID-6 (below). |

Real run of the bundle on a synthetic 8 s video (FFmpeg test pattern, throw-away library, deleted afterwards): `library.import {videos: true}` → 1 video, 8 s; `video.frames {every: 2}` → 4 pictures; `video.list` / `video.moments` fine; `describe.set` then `describe.search {query: "mire"}` finds it; `video.faces` loaded YuNet and the SFace model and processed 4 pictures (0 faces: a test pattern); screenshot `verification/videos.png`. Seen in the app: the poster of the list shows white (the test pattern's first picture is not necessarily white: unconfirmed), and the "Finding faces" panel stays up after the run until the next poll (3 s).

Evidence: `cargo test -p lightcraft-engine --features faces,autoseg --lib -- video describe`: 24 passed, 0 failed (real FFmpeg;
synthetic videos); clippy `-D warnings` clean on engine, ui-egui and cli. Not run: the `tests_videos` UI tests (the UI test binary does not start reliably on this machine), the real
`~/Movies/VideoCatalog` database (read-only data, never touched), `video.faces` on a real clip with named photos, and naming from the photos' people.

## 3c. VID-6 on 0.14.0 (branch `lc-vid6`, 28 October 2026)

Nothing of the fork was dropped; the fork had none of VID-6 except `examples/describe_bench.rs`, which is ported (and
extended). Everything below was measured on a **copy** of the test library (`/Volumes/Data/work/lc-vid6-lib`: 355 videos,
4 260 pictures, 1 400 Video Catcher moments; the real library and the originals were never touched; the originals
are only read by FFmpeg).

| Piece | What it is | Evidence |
|---|---|---|
| 1. Search floor | `describe/honest.rs`; `describe.search {floor, normalise}`: a meaning match must beat, by a margin (0.04 default), the best cosine fourteen nonsense queries reach with that picture | Bench (`docs/describe.md`): nonsense queries with a result 10/10 → **0/10** (200 → 0 results); real queries still answered by meaning 94/100; fusion r@10 60.7 → 58.7 % (short action) and 79.7 → 79.0 % (sentence) |
| 2. Text-mean centring FR+EN | `describe.search {centre: none\|text\|gap}`; mean of ~660 neutral captions in both languages (`Searcher::calibration`, computed once per model) | **Not proven**: +2 points r@10 on the meaning of a sentence, 66 → 52 median rank on a short action, ±1 point on the fusion. `gap` hurts (r@10 30.0 → 12.0 %). Default stays `none`, the option stays |
| 3. Shots + dense long shots | `video/scenes.rs`; `video.frames {plan: "scenes", sceneThreshold, maxGap}`; UI button "Cut pictures at shots" | Real test: a 3 s black shot + 12 s pattern → cut found at 3 s, pictures at 1.5 s and ≥ 2 in the long shot (`tests_video6`); FFmpeg cut detection of a 343 MB GoPro file: 8.4 s on the NAS, `pts_time` format checked on a real file; a 60 s single take gets 8 pictures at ≤ 8 s |
| 4. Transcription | `video/speech.rs`; `video.transcribe`, `speech.json`; UI button "Transcribe speech" and its progress window; `video.cancel` stops it | Real run: macOS `say` voice muxed into an MP4 → `mlx_whisper` large-v3-turbo wrote 1 sentence (« The man driver had went to the beach, then he goes home. » — the model mishears the synthetic voice, which is why the test searches « beach home »), search finds it at level ≤ 2 |
| 5. Literal search in levels | `speech::find`: one sentence / within 8 s / anywhere in the video, no stemming of English; wired into `describe.search` (`speech: {level, ms, text}`) | Unit test of the three levels + the real run above; a word that is not said finds nothing |
| 6. Retry of failures | `video.frames {retryFailed: true}` (+ UI "Retry failed pictures") | `tests_video6`: a picture removed and listed as failed comes back, the next retry cuts nothing |
| Videos view opens without freezing | **lc-port-014 did it first** (`video_frames_job` / `video_frames_apply`, `warmed`, commit 16269215) while this branch was waiting for the build lock; I wrote the same thing independently, compared, and kept theirs (it covers every section of the view and has its own test). What VID-6 adds: `warm()` is public so a person's page asks the same worker | Measured before either: first `video.list` **20.2 s** blocking on the window thread (cold disk, copy of 355 videos); the reading of the lists alone takes 12.1 s cold, now on a worker |
| `video.list` itself | the faces of videos are read once per command (`faces::reader`), names suggested for a face are remembered | Warm `video.list` **563 ms → 51 ms** (same copy; this copy has no named faces, so the gain on a library with 28 000 photos and 18 000 named faces is larger and was not measured); first call after opening 579 ms (loads the recognition model) |
| Faces of videos in People | `video.person {name}`; section « Dans les vidéos » (In videos) on a person's page (shows « Chargement… » while the worker reads): cut-out faces marked « vidéo · m:ss », a click opens that moment in the Videos view | `tests_video6::a_person_is_found_in_the_pictures_of_videos` (injected faces, no model). **Not seen on real faces**: the copy has no faces looked for with 0.14.0's models and the real library was off limits |

Not done / limits: the UI was compiled (`cargo check`, all targets) and the engine tested, but **no screenshot of the new
section or of « Chargement… »** (headless snapshot not run); the `tests_videos` UI tests of the fork are still not
ported; `video.person` runs on the window's thread once the lists are read (fast: names are remembered); the floor is
calibrated on one library with fourteen nonsense queries (`honest::NOISE`), so a library full of black screens would
raise it.

## 4. Left out on purpose
- **UX refonte shell** (journey bar, spaces, home, quick sort, classify): replaces the whole window; upstream's
  Lightroom-style modules stay. A thin journey bar mapped onto upstream's modules can come later.
- **VID-6**: done on branch `lc-vid6` (section 3c).
- Semantic proposals and by-meaning ranking of `describe.*` (they need the SigLIP model, `crates/search`): `describe.suggest` answers "Semantic search is not available in this build".
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

## 8. State of 28 October 2026 (rule: nothing of the fork is abandoned)

| Step | State |
|---|---|
| 1. People data | **Done on the real library** (people.migrate: 75 names / 0 missing, 18 078 + 4 878 existing regions, 302 + 66 added, 13 802 photos, originals identical, reopen OK; backups kept; one conflict, photo 4374 "Alba Madi" vs "Kurtis Madi", catalogue name kept). Unnamed groups stay in `people/legacy-people.json` (read with `people.legacy`). |
| 2. UX refonte | Workspaces (Library / Retouch / Create), journey bar, context bar, Import home, Sort helper, Create space, ported onto 0.14.0's docking shell and seen in headless snapshots. **Classify** rewritten on upstream commands (counters, places from GPS, same-file, link to People for naming). Left: Retouch left column (needs proposed retouch), module filmstrip under both sidebars, Sorting Help window (below). |
| 3. Semantic search + taste | Engine ported (crates `search`, `taste`, `search.*`, `taste.*`, `keywords.*`, near-duplicates, `describe.suggest` by meaning); real SigLIP model end-to-end test passes with the files already installed. UI (Sorting Help window with suggested picks / keywords, filter-bar entries) **not yet ported**. |
| 4. Lot E speed | `perf_bench` + `tools/perf.sh` ported (measure upstream's window read); import of big raws from their head with sampled keys (`ps1-`) merged onto upstream's scan (adoption of full hashes kept, find-missing knows both schemes). Not carried because upstream has the same: the 3 MB prefix read, grid thumbnails from the embedded preview, the GPU denoise race fix. |
| 5. Left out so far | Depth map / Lens Blur / Depth Range, MI-GAN, proposed retouch, skin harmonization, dodge & burn, SCUNet / SPAN restore, library lock retry, 589 French messages: **still to port**. |

## 0.15.0 port (branch `lc-port-015`, 2026-10-29)

The launcher installs LightCraft **0.15.0**, so the whole fork was re-merged onto upstream `v0.15.0` (new branch, `lc-port-014` is frozen).
Already inside: `lc-lot-b14` (My Style, autoframe), `lc-speed-sort` (sort view), `lc-vid6` (Videos, VID-6, video faces), lot E keys.

- Method: merge `v0.15.0`, `git checkout --theirs` on each of the 15 conflicting files, then re-apply the fork's diff by hand into the
  new submodule layout (engine `files/ import/ media/ cmd/library/`; ui-egui `dialogs/ state/ menus/ panels/right/ panels/detail/ app/`).
- New upstream rules met and followed, not weakened: 7 arguments max, no wildcard imports, cognitive complexity ≤ 30,
  `craft_ui` combo box instead of `egui::ComboBox`, `register(&Ui, …)`, renamed egui APIs.
- Painted text must not contain `▸` (no glyph in the bundled fonts, egui panics): the fork's strings and their French keys now use `→`
  (found by backtrace: `compare.rs:609`, `home.rs` action tiles, 10 `tr()` strings, 2 command descriptions).
- CI: `cargo xtask ci` — all 8 steps passed (fmt, clippy, heif, test, parity, layers, assets, wasm) on `d3aa4566`.
- Bundle: `lc-port-bundle.sh` (now 0.15.0): launcher `Info.plist`, `LightCraftLocalBuild = 0.15.0-madson1`, ad hoc signed, `codesign -v` valid.
- Checked on a copy of the test library (355 videos, 355 photos): thumbnails load, journey bar, Quick sort, suggested picks, keywords and
  People suggestions, Videos view lists the 355 videos, menus carry cull / portrait / Videos / People / Sorting Help / Style / tour.
- Videos "freeze" (looked at, 2026-10-29): the ~9 s `maxUpdateMs` seen on the copy was the **startup** worst frame (library open 5.3 s on a
  cold USB disk, before the window), not the Videos view. The view already reads its lists on a worker (`warm` / `ListsJob`, 32 threads,
  "Loading the videos…") and decodes posters off-thread; `sample` of the UI thread showed only egui text layout. One real cost was found and
  fixed at the cause (`47a08da8`): `video.list` re-read the described moments of the whole library for every video (quadratic) —
  one grouping now. Warm `video.list` 444 ms → ~24 ms; worst frame when opening the view 658 ms → 66–76 ms (warm disk cache).
  Headless screenshots show already-uploaded posters white; the windowed screenshot shows them (not a defect).
  Tool: `lc-port-015-measure.py <binary> <label> <library-copy> <port>` (same method for before and after).
- Measured with the script on a copy (355 videos), same method both sides, before = `d3aa4566` build, after = `47a08da8` build:
  `video.list` cold/warm 133/69 ms → 19/24 ms; first responsive frame after opening the view 23 ms → 16 ms; worst frame during the view
  155 ms (before) vs 1242 ms (after) — but the after figure was already reached before the view was opened (`maxBeforeView` 1242), i.e. startup
  noise on a busy USB disk, not the view. The worst-frame figures are therefore not a reliable before/after; `listsReadyMs` was not captured.
- CI: `cargo xtask ci` — all 8 steps passed on `47a08da8` (`lc-port-015-ci3.log`, "CI summary: all 8 steps passed", EXIT 0).
- Still open: the beginner walk (40 CR2); Retouch step; Retouch left column and module filmstrip under both sidebars; step 5 modules
  (depth / Lens Blur / Depth Range, MI-GAN, proposed retouch, skin harmonization, dodge & burn, SCUNet/SPAN, library-lock retry);
  ~97 missing French messages; the 5.3 s library open at startup on a cold disk (main thread, not Videos-specific).

## Step 5 — retouch modules (branch `lc-retouch-015`, from `lc-port-015`)

Ported from the fork (`lc-ux-refonte`); nothing dropped, compared function by function.

| Fork feature | Port location | Evidence |
|---|---|---|
| Depth (Depth Anything V2 Small) | `crates/depth`, `develop/src/depth_map.rs`, `engine/src/ai.rs` (`depth.*`) | crate verbatim; `tests_ai` |
| Lens Blur | `pipeline/src/lensblur.rs` (verbatim), `lens_active` + `lin_cpu` stage, `lensblur.set` | `examples/lensblur_eval.rs`, pipeline tests |
| Depth Range mask | `MaskShape::DepthRange`, `masks.rs` (`evaluate_aux`), `mask.add depthRange` | `tests_ai::depth_range_mask_selects_by_depth` (caught a missing parameter parse, fixed) |
| MI-GAN inpaint | `crates/migan` (fork `inpaint`, renamed `lightcraft-migan`), `spot.inpaint`, `inpaint.model.*` | crate tests; `examples/migan_check.rs` |
| Proposed retouch ("Natural skin"), skin harmonization, dodge & burn, palette | `engine/src/retouch/*` (fork verbatim, `targets(p)?`; faces read from `photo.meta.regions`), panel `ui-egui/src/panels/proposals.rs` | pure retouch crate diff 0 vs fork; `tests/proposals.rs` |
| SCUNet / SPAN restore | `crates/restore` (cherry-pick `05a0ee38`) | crate + engine tests |
| Library-lock retry | `catalog/src/lock.rs` (1.5 s retry on WouldBlock/Interrupted) | upstream's explicit unlock alone: 231/300 refusals on this macOS; with the retry, 7/7 lock tests pass |

Not ported: the Retouch left column (`retouch_left.rs`). The `lc-port-015-s5` worktree copy is superseded by this branch.
Not yet proven here: real-model runs of depth / MI-GAN / restore (only compile, crate and engine tests).

## Étape 5 portée (30/10)
lc-port-015 = 508cec44 (CI 8/8 prouvée sur cet arbre via lc-retouch-015, ci6). Restore SCUNet/SPAN, retouche proposée (Shift+N), depth/Lens Blur/MI-GAN présents ; modèles depth/face à télécharger par l’utilisateur (erreurs actionnables, pas de panic). Bundle signé ad hoc, codesign vérifié.

## Port on upstream v0.18.0 (2026-11-04)

Branch `lc-port-018` = `lc-port-015` + merge of v0.18.0 (18 conflict files resolved by hand, upstream taken then the fork re-applied).

| Point | Result |
|---|---|
| `thumb_job` returned `None` for videos (upstream has no video module) | the guard is removed; the fork's FFmpeg poster loader serves videos |
| `Shift+K` | upstream's keyword shortcut; the fork's AI Picks moved to `Cmd+Alt+J` (menu, tooltip, French, `docs/library-shortcuts.md`) |
| French | upstream's `fr.json` / `fr-formats.json` kept, 453 + 198 fork messages and 80 formats re-added (fork-only keys, original order kept) |
| Dropdowns | the fork's five unnamed combos now carry an accessible name (strict kittest check) |
| Tests that assumed the Library centre | `local_places` sets `workspaces = false` (an empty library opens on Import in the fork) |
| `Line::mid` | stays `pub` (autoframe uses it) |

CI read on this branch: fmt, clippy (`--workspace --all-targets`), heif, parity, layers, assets, wasm all EXIT 0. Tests: all pass except `dialog_role::settings_is_a_dialog_too`, which panics without a Georgian font (upstream added `ka`; the local craft-fonts has none): an environment gap, not the fork.
