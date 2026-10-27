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

§3 to §6 (state of the port, bundle, verification, how to re-apply on 0.15.0) are filled in as the work lands.
