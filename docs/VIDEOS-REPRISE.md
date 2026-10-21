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
| 1b | clippy `-D warnings` on the workspace | done (mechanical) | `ba85ecd2` |
| 1c | wasm target | installed; checked by `cargo xtask ci` | CI log `/Volumes/Data/work/lv-ci.log` (running, worktree `/Volumes/Data/work/lv-ci`) |
| 1d | the 14 red ui-egui tests | **14 → 0** (`cargo test -p lightcraft-ui-egui`: 459 passed, then the last one — 21 French messages with `▸` — fixed, i18n 28/28). 2 real bugs: unpaintable `▸` in the French catalog; the context bar's Before/After ran side by side (`view.beforeAfter`) while it says "show the original" → `view.showOriginal`. Sorting Help: a closed section shown even outside the Sort step → only in the Sort step, open. 5 model-setup tests: `-p lightcraft-ui-egui` built the engine without `denoise` → dev-dependency feature. Stale tests (code changed on purpose in v14–v16): language list, tool height 44 → 48, Retouch left column, proposal steps under Details | `77124cdf`, `3f2f859a` |
| 2 | French search bench | done: 1 312 questions; fusion (text + meaning, both languages) is the default; moment median rank 14 → 8 (short action) and 14 → 3 (sentence); r@10 45 % → 53.5 % and 45 % → 74.4 %. The glossary alone does not help (table in `describe.md`) | `11b58a42`, docs |
| 3 | real scale (lv-e2e3) | `people.index` 3 897 pictures in 1 635 s, 943 faces, peak 1.15 GB; `search.index` 246 s, 2.2 GB; on disk people 5 MB, search 6 MB, video 377 MB, describe 0.4 MB; search reads the frame lists once per query. Faces: 146 groups; biggest (407 faces / 67 videos) = one man, no clear stranger in 80; one group of 33 non-faces (wheels, objects); defaults unchanged (`people.md`). UI with 386 videos: to do with the bundle | docs |
| 4 | silent preview in the sheet | done; test `the_preview_plays_a_moment_without_sound_and_closes` (real FFmpeg clip: decoded on a thread, plays when ready, Pause/Play, -5 s makes an earlier clip, Close) passes; tests_videos 5/5 | `88dbd522` + test |
| 5 | unreadable videos | done (`video.unreadable`, list section); 21 NAS mp4 unknown to Video Catcher: 16 readable by ffprobe, 5 not (4 empty, 1 without `moov`: DJI_0533, 256 MB); `.LRV` proxies no longer imported | `b2d90667`, `88dbd522` |
| 6 | translations (Recognition, context bar) | done | `3f2f859a`, `77124cdf` |
| 7 | this file | written | |
| 8 | bundle + 3 window captures | to do | |
