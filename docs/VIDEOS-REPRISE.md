# Videos in LightCraft — resuming the work

Branch `videos-v1` of `/Volumes/Data/work/lightcraft-videos` (base `13e0a110` = local v16). Short notes for whoever
picks the work up; the details are in [`describe.md`](describe.md) and [`people.md`](people.md) (Faces in videos).

## Architecture

Everything is an engine command (`crates/engine/src/cmd/video.rs`, `cmd/describe.rs`); the Videos view
(`crates/ui-egui/src/panels/videos.rs`) only draws their answers. Three stores sit next to the catalog, never in its
journal, all rebuildable except the annotations a person wrote:

| Folder | What | Written by |
|---|---|---|
| `<library>/video/<16 hex of key_of(content key)>/` | `frames.json` + `t<ms>.jpg` pictures (origin `ffmpeg` or `videocatcher`), `clips/` (8 s H.264 previews) | `video.frames`, `video.importVideoCatcher`, `video.clip` |
| `<library>/video/unreadable.json` | videos an import could not read, with their cause | imports |
| `<library>/people/` | `faces-<model>.bin` (one record per face; a picture of a video under `key_of("<ck>@<ms>")`), `people.json` (decisions) | `people.*` |
| `<library>/search/` | SigLIP vectors (pictures of videos under the same `<ck>@<ms>` strings) | `search.index` |
| `<library>/describe/annotations.jsonl` | scene / action / description per photo or moment, one slot per source (`manual`, `videocatcher`, `siglip`) | `describe.*`, Video Catcher import |

Commands: `video.status / frames / cancel / list / moments / open / clip / unreadable / importVideoCatcher`,
`describe.list / set / confirm / delete / undo / suggest / status / cancel / search`, `people.*` (unchanged, they see
the faces of videos), `library.import {videos: true}`. FFmpeg is a helper program (`/opt/homebrew/bin`), never linked.

## Redo the full import (a test library)

```sh
CLI=/Volumes/Data/work/lightcraft-videos-target/debug/lightcraft-cli
LIB=/Volumes/Data/work/lv-e2e3/lib   # never Madson's catalog
export LIGHTCRAFT_SEARCH_DIR="$HOME/Library/Application Support/LightCraft/models/search"
$CLI run --library $LIB video.importVideoCatcher \
  db=$HOME/Movies/VideoCatalog/libraries/e7f6a598ad826a8d/runtime/video-catcher.sqlite3 createMissing=true
$CLI run --library $LIB people.index      # 3 897 pictures: ~27 min on the M4 under load
$CLI run --library $LIB search.index      # ~4 min
```
Every step resumes where it stopped.

## Rebuild the test bundle

```sh
cd /Volumes/Data/work/lightcraft-videos
CARGO_HOME=/Volumes/Data/rust/cargo RUSTUP_HOME=/Volumes/Data/rust/rustup PATH=/Volumes/Data/rust/cargo/bin:$PATH \
CARGO_TARGET_DIR=/Volumes/Data/work/lightcraft-videos-release-target cargo build --release -p lightcraft --features heif
B=/Volumes/Data/work/lv-bundle; rm -rf $B/LightCraft.app
ditto /Volumes/Madson/ArtCraft/lightcraft/LightCraft-v16.app $B/LightCraft.app
cp /Volumes/Data/work/lightcraft-videos-release-target/release/lightcraft $B/LightCraft.app/Contents/MacOS/LightCraft
/usr/libexec/PlistBuddy -c "Set :CFBundleName LightCraft v17 vidéos" \
  -c "Set :CFBundleShortVersionString 0.5.0-local-ia-v17-videos" -c "Set :CFBundleVersion 0.5.0-local-ia-v17-videos" \
  $B/LightCraft.app/Contents/Info.plist
codesign --force --deep --sign - $B/LightCraft.app && codesign --verify --strict --verbose=2 $B/LightCraft.app
# try it on a test library only:
LIGHTCRAFT_NO_PREFS=1 $B/LightCraft.app/Contents/MacOS/LightCraft --library /Volumes/Data/work/lv-e2e2/lib --control 18733
```

## Git: this folder is its own worktree (fixed 21 Oct 2026)

`lightcraft-videos` was made as a copy of `lightcraft-v14`, `.git` file included, so both folders shared one worktree
entry (`.git/worktrees/lightcraft-v14` of `/Volumes/studio madson/ArtCraft-build/lightcraft-050`): the
`checkout videos-v1` of 20 Oct moved **v14's** HEAD too, and v14's `git status` showed the video work. No file of
`lightcraft-v14` was ever written. Fixed: `lightcraft-videos` has its own entry (`worktrees/lightcraft-videos`), v14's
HEAD is back on branch `v14` (`ebc08b01`) with its index re-read from it (its 64 uncommitted files are as they were,
now all unstaged — a staging state from before 20 Oct, if any, could not be recovered). Backup of the admin files:
`/Volumes/Data/work/lv-gitadmin-backup-202610210608/`. Lesson: copy a worktree with `git worktree add`, never `cp -R`.

## Final bundle (21 Oct 2026)

`/Volumes/Data/work/lv-bundle-final/LightCraft.app`, built from a clean worktree (`/Volumes/Data/work/lv-final`, commit
`889bee37`: the tree `lightcraft-videos` also holds another session's unfinished culling work, not included).
Binary `Contents/MacOS/LightCraft` after ad-hoc signing: 68 372 960 bytes, SHA-256
`ff5180709efe15e512f33edddeec5b41dd591dee6218c1350c4306a54f73c0a7`; bundle 69 MB; `codesign --verify --strict` valid.
`LightCraft-f81d38c1-gel-NE-PAS-INSTALLER.app` next to it is the build before the fix of the window freeze
(`video.status` read every video's list of pictures every 3 s on the window's thread): never install it.
The first launch of a newly signed binary takes about 6 min on this Mac (system check), not a freeze.

Smoke tests of this bundle (21 Oct, `LIGHTCRAFT_NO_PREFS=1`, French):
- **Copy of the test library** (`/Volumes/Data/work/lv-smoke/lib2`, APFS clone of lv-e2e3, 355 videos): first launch
  369 s to the control port (system check of the new binary), first frame 41 s later; the Videos view is in French
  (`lv-ui-proofs/20-final-videos-fr.png`); `app.quit` closed it in about 28 s. **Its first opening of the Videos view
  froze the window for about 10 min**: `video.list` read the `frames.json` of each of the 355 videos on the window's
  thread, on a disk then answering a one-line write in 15 s. Fixed after this bundle: `video.list` takes the remembered
  lists (one read of each, then nothing until they change) — commit after `889bee37`, not in this bundle.
- **Blank photo catalog** (`/Volumes/Data/work/lv-smoke/photolib`, 2 JPEG, no `video/` nor `describe/`): port in 13 s,
  `libraryProblem: null`, no ERROR in the log; the Videos view says « Aucune vidéo dans la bibliothèque » and the grid
  shows the 2 photos (`lv-ui-proofs/21-…`, `22-…`); opening it created no `video/` nor `describe/` folder.

## Install / go back (Madson's decision)

```sh
mv /Applications/Artcraft/LightCraft.app "/Volumes/Madson/ArtCraft/lightcraft/LightCraft-installee-avant-v17-$(date +%Y%m%d).app"
ditto /Volumes/Data/work/lv-bundle/LightCraft.app /Applications/Artcraft/LightCraft.app
# back: remove it and ditto the saved copy back to /Applications/Artcraft/LightCraft.app
```
v16 ignores the new folders (`video/`, `describe/`).

## Known limits

- No player with sound inside LightCraft: a silent 8 s preview, and the system's player (VLC at the time).
- Search by meaning (SigLIP) is weak on free French sentences (see the bench in `describe.md`); the annotations' text is
  the reliable part. A face only *suggested* is not "there" for the search.
- A video whose index (`moov`) is missing cannot be repaired here.
- Undo of annotations lasts for the session.

## Avancement VID-5

| # | Subject | State | Commit / proof |
|---|---|---|---|
| 1a | "Library already open" test flakes | done: a child process started by another thread holds the lock's descriptor until its program starts; the lock is asked again for 1.5 s | `a37020c2` (refused 63/300 before, 0 after) |
| 1b | clippy `-D warnings` + full `cargo xtask ci` | clippy clean (`ba85ecd2`, `a9354617`), fmt ok (`afb1ac36`). CI at `a9354617`: fmt ok, clippy ok, heif ok, **test: 2 304 passed, 2 failed** — `lightcraft-gpu` `nn::tests` (GPU denoise on Metal differs from the CPU reference by ~0.84 of the output size for tiles ≥ 3; deterministic, 3 runs; `crates/gpu`, `crates/denoise*` and the lock are identical to base v16 → **pre-existing, not from this work**, but real: `runOn: auto` can pick the GPU). parity ok. **layers: 6 violations** (people, search, autoseg, restore, retouch, taste not registered in `xtask/src/layers.rs` — all from v12–v16). **assets: 27** codecs test files without attribution (pre-existing). **wasm: ok** (all crates + lightcraft-web) | logs `/Volumes/Data/work/lv-ci3*.log` |
| 1c | wasm target | installed; **`cargo xtask wasm` ok** (21 Oct). First run (worktree `lv-ci`, cold target) killed after 97 min stuck on the saturated disk; rerun in the main tree at `0a8a48f4` | log `/Volumes/Data/work/lv-ci2.log` (ends with `CI EXIT n`); rerun: `bash /Volumes/Data/work/lv-ci2.sh` |
| 1d | the 14 red ui-egui tests | **14 → 0** (`cargo test -p lightcraft-ui-egui`: 459 passed, then the last one — 21 French messages with `▸` — fixed, i18n 28/28). 2 real bugs: unpaintable `▸` in the French catalog; the context bar's Before/After ran side by side (`view.beforeAfter`) while it says "show the original" → `view.showOriginal`. Sorting Help: a closed section shown even outside the Sort step → only in the Sort step, open. 5 model-setup tests: `-p lightcraft-ui-egui` built the engine without `denoise` → dev-dependency feature. Stale tests (code changed on purpose in v14–v16): language list, tool height 44 → 48, Retouch left column, proposal steps under Details | `77124cdf`, `3f2f859a` |
| 2 | French search bench | done: 1 312 questions; fusion (text + meaning, both languages) is the default; moment median rank 14 → 8 (short action) and 14 → 3 (sentence); r@10 45 % → 53.5 % and 45 % → 74.4 %. The glossary alone does not help (table in `describe.md`) | `11b58a42`, docs |
| 3 | real scale (lv-e2e3) | `people.index` 3 897 pictures in 1 635 s, 943 faces, peak 1.15 GB; `search.index` 246 s, 2.2 GB; on disk people 5 MB, search 6 MB, video 377 MB, describe 0.4 MB; search reads the frame lists once per query. Faces: 146 groups; biggest (407 faces / 67 videos) = one man, no clear stranger in 80; one group of 33 non-faces (wheels, objects); defaults unchanged (`people.md`). UI with 386 videos: to do with the bundle | docs |
| 4 | silent preview in the sheet | done; test `the_preview_plays_a_moment_without_sound_and_closes` (real FFmpeg clip: decoded on a thread, plays when ready, Pause/Play, -5 s makes an earlier clip, Close) passes; tests_videos 5/5 | `88dbd522` + test |
| 5 | unreadable videos | done (`video.unreadable`, list section); 21 NAS mp4 unknown to Video Catcher: 16 readable by ffprobe, 5 not (4 empty, 1 without `moov`: DJI_0533, 256 MB); `.LRV` proxies no longer imported | `b2d90667`, `88dbd522` |
| 6 | translations | done: Recognition panel, context bar, People sidebar ("943 faces found: name them…" was English in the window), singular/plural of "1 visage · 1 photo" in Recognition | `3f2f859a`, `77124cdf`, + this commit |
| 7 | this file | written | |
| 8 | bundle + 3 window captures | bundle rebuilt at `8e5578a8` (release 41 min), signed, `codesign --verify --strict` valid; launched on lv-e2e3 (355 videos in the catalog): list capture `10-v17-liste-videos.png`, list scrolls at ~17 ms a step. Sheet and search captures **not taken**: Madson was using the window (naming people in Recognition), so it was not driven further. The People-sidebar and plural fixes are after this bundle | `/Volumes/Data/work/lv-bundle/LightCraft.app` |

## Avancement bundle final2

Decision 21 Oct: one installation only, with `b021248f` (first opening of the Videos view). Criterion: the first
opening of the Videos view on a fresh copy of the 355-video test library under 5 s.

| Step | State | Proof |
|---|---|---|
| build release from `/Volumes/Data/work/lv-final` at `ef8fc47b` (same change as `b021248f`, without other sessions' work) | done 18:45 (binary built from lv-final sources only) | log `/Volumes/Data/work/lv-release-final2.log` (ends `RELEASE EXIT n`); rerun `bash /Volumes/Data/work/lv-release-final2.sh` |
| bundle `/Volumes/Data/work/lv-bundle-final2/LightCraft.app`, ad-hoc signed, `codesign --verify --strict` | **rebuilt from lv-final `48af008d`** (= videos-v1 `3a4ba43e`: parallel read): valid on disk, satisfies its Designated Requirement; binary 68 248 400 bytes, SHA-256 `7ed29a368d5386dfed7fb0d9673a1e8255de90aff708e5348512494be2d54b1c`; bundle 69 MB. The `ef8fc47b` build is kept next to it as `LightCraft-ef8fc47b-premiere-ouverture-30s-NE-PAS-INSTALLER.app` | |
| smoke: fresh copy of lv-e2e3, time of the first Videos view | **failed the criterion with `ef8fc47b`: 29.8 s** (control port 67 s after launch, first frame 72 s). Sample: all the time in `video_frames` → `Manifest::load`, 355 cold opens one after the other (~80 ms each on this disk). Measured outside the app on fresh clones: 355 `frames.json` read one by one 16.6 s, 32 at a time 3.7 s. Fix in progress in lv-final (uncommitted then committed): the lists are read 32 at a time (sequential on wasm). New build: log `/Volumes/Data/work/lv-final3.log` (clippy, video tests, release); bundle final2 will be rebuilt from it | |
| smoke `48af008d` bundle, fresh copy lv-smoke/lib9 | first launch: control port 334 s (system check of the new binary), first frame 10 s later; **first Videos view 14.0 s** (355 cards; was 29.8 s with `ef8fc47b`). A second fresh copy (lib10), with the disk answering a one-line write in 6.4 s and 16 rustc/cargo of other sessions running: **134 s**, the window then waiting in `people::ensure_open` (the faces file) as much as in the lists. **The < 5 s criterion is not met on this machine in this state**; the remaining time is disk waits, not computation. `app.quit` closes it in 5–23 s. Screenshot `lv-ui-proofs/30-final2-videos-fr.png` | |
| smoke: blank photo catalog | done: `/Volumes/Data/work/lv-smoke/photolib2` (2 JPEG, no `video/` nor `describe/`): port 33 s, `libraryProblem: null`, 0 ERROR in the log, Videos view « Aucune vidéo dans la bibliothèque », grid with the 2 photos (`31-…`, `32-…`), no `video/` nor `describe/` created; quit clean | |
