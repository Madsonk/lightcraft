# Performance: protocol, budgets, measurements

Goal (maintainer, 21 Oct 2026): LightCraft should feel as fast as Aftershoot on a photo library. This page defines how
speed is measured, the budgets, what was measured on 21 Oct 2026 (LightCraft and, passively, Aftershoot), the one
optimisation that landed with a before/after, and what is **not** measured.

> **Read this first: noisy numbers.** The machine (Apple Silicon, 10 cores, 16 GB) was shared with other builds the whole
> time and `/Volumes/Data` was saturated for part of the day (a `rustc` hello-world took 3 min 18 s; fresh executables
> sat for minutes in `dyld_start` before running). Earlier runs of that session are unusable and are not reported.
> Everything below ran from a **local internal disk** (`~/lc-perf`), not the USB volume, and the figures are medians or
> ranges over 3 passes with about ±15 % spread. Do not read a 10 % difference as a result.

## Protocol (reproducible)

Test set: `cargo xtask corpus --download` fetches CC0 raw samples (raw.pixls.us, sha256-pinned). `tools/perf.sh prepare`
copies them to a local folder and builds **N = 300 unique files**: 70 % raw (Sony ARW, Canon CR2/CR3, Nikon NEF, Fuji RAF,
Olympus ORF, Panasonic RW2/RAW, Samsung SRW, DNG; 12 to 50 MB) and 30 % camera-size JPEGs (4 to 14 MB). Files are APFS
clones (`cp -c`: no extra disk) with a few trailing bytes each, so every file has its own content hash (the engine keys
everything by content hash; exact copies would hit one cache entry). Never a personal library.

Run: `tools/perf.sh run [DIR] [VISIBLE=48] [RUNS=3]` (builds and runs `crates/engine/examples/perf_bench.rs`, which calls
the same engine entry points as the app: `import`, the grid's `JobPool`, `thumb_job`, `quick_thumb_job`, `loupe_job`,
`quick_view_job`, the preview loader used by face/analysis scans). `THREADS=n` sets the pool size.
Memory is the process peak RSS. The disk cache starts empty for the cold pass.

## Budgets and measurements (LightCraft, 300 photos, release build, 6 workers)

| Metric | Budget | Measured | Note |
|---|---|---|---|
| Import of 300 photos (probe + content hash + catalogue) | ≤ 3 s | 1.5 to 1.9 s (5 to 6 ms/photo) | warm page cache; a cold card/NAS reads every byte once for the hash |
| Time to first thumbnail on the grid | ≤ 50 ms | 2 to 80 ms | embedded raw preview |
| First paint of 48 visible cells | ≤ 600 ms | 374 to 509 ms | previews first, final renders after |
| Final thumbnails of the 48 visible cells | ≤ 5 s | 2.7 to 5.3 s | |
| Final thumbnails of all 300 (cold disk cache) | ≤ 30 s | 18 to 24 s (13 to 17 photos/s) | not helped by more threads (3, 6 and 9 workers: same total) |
| Visible grid after restart (disk cache) | ≤ 20 ms | 5 ms | |
| Next photo in the loupe, stand-in image | ≤ 60 ms | 50 to 57 ms | |
| Next photo, sharp 2560 px, cold | ≤ 200 ms | 154 to 166 ms | |
| Next photo, sharp, neighbour prefetched | ≤ 50 ms | 36 to 40 ms | |
| Analysis input per photo (preview decode for face/culling scans) | ≤ 25 ms | 17 to 20 ms | analysis models themselves not measured here (lot A) |
| Memory peak, 300 photos | ≤ 1.5 GB | 1.25 to 1.30 GB | |

Where the thumbnail time goes (`LIGHTCRAFT_PROFILE=1`, 313 CPU thumbnail renders): the pipeline stages sum to about 25 s of
work (`finish` 13 s, `sample` 6 s, `wb/spots/nr` 4 s, `nr colour` 3 s). The raw decode (read, decode, binned demosaic) is **not
instrumented** in that profile, so its share is unknown. Worker count (3, 6, 9) does not change the total, so the limit is
not the pool size; the likely suspects (rayon oversubscription between pool workers and the loops inside each render, or
the memory gate) are untested. Moving thumbnail renders to the GPU (the loupe already uses it) is **not done**.

## Aftershoot 2.24.1, measured passively (21 Oct 2026)

App started with `--remote-debugging-port=9222` (Chrome DevTools Protocol, Network domain only), the **13-photo test
album** ("adrienne wedding": 12 JPEG of about 8.8 MB and 1 PNG) opened by a double-click, one navigation test with the
arrow keys, then the app closed. No processing, no export, the 736-photo album never opened, nothing written to photo folders.
Backups of `rs.Aftershoot/` and the album were made first.

| Observation | Value |
|---|---|
| `GET /extract` calls on the 3 extractor ports (43941/43943/43945) after opening the album | 54 calls, from 0.98 s to 1.96 s after the double-click |
| Duration of one `/extract` call | median 8 ms, max 102 ms (responses about 84 KB: small previews) |
| Arrow key to first image request | 59 to 81 ms |
| Arrow key to all related image loads finished (about 20 requests: loupe + filmstrip + prefetch) | 0.59 to 1.26 s |

LightCraft on 13 camera JPEGs (same size class, `perf_bench run` on a 13-file folder): 13 grid thumbnails in **218 ms**.
**Not equivalent**: Aftershoot's figure spans opening the album screen and its UI, ours is the engine alone with no
painting. It shows the engine is not the slower side for JPEG thumbnails; it does not prove LightCraft's UI is as fast.

## Optimisation landed: read only the head of a raw for its embedded preview

Before, `fs_preview_loader` (grid first paint, neighbour stand-ins, face/analysis scans) read the **whole** raw (12 to 50 MB)
to extract a 0.3 to 2 MB JPEG. Now it first reads the first 3 MB and takes the preview from there when it is a complete JPEG
(segments walked to the start of scan, ends with the end-of-image marker: a cut-off file is never accepted, even when it still
holds the EXIF thumbnail's own marker) of at least the requested size. Otherwise, and for Nikon files (their colour space is
read from a maker note that may lie further in), it reads the whole file as before. Code: `crates/engine/src/files.rs`
(`preview_from_prefix`, `is_whole_jpeg`); a bytes counter `files::PREVIEW_BYTES_READ` backs the numbers.

A/B in one process, 60 raw files of the set, same files read both ways:

| Requested edge | Whole file: ms / MB per photo | Prefix: ms / MB per photo | Prefix used |
|---|---|---|---|
| 256 px (grid) | 20.4 ms / 20.5 MB | 15.4 ms / 7.0 MB | 68 % of the files |
| 1024 px (analysis) | 21.1 ms / 20.5 MB | 19.8 ms / 7.0 MB | 68 % |
| 2560 px (loupe stand-in) | 31.1 ms / 20.5 MB | 32.0 ms / 13.8 MB | 40 % |

Bytes read per photo drop by 66 % (grid, analysis) and 33 % (loupe); the time gain is only −25 % at 256 px here because every
file was in the page cache. The real gain is expected on a cold card, USB disk or NAS, where reading 20 MB costs far more than
decoding; that is **not measured** (no `purge` without sudo). End to end the first grid paint went from 479 and 509 ms to
374 and 397 ms and analysis input from 19.8 to 16.7 ms, within the noise for the first. Tests:
`crates/engine/tests/preview_prefix.rs` (cut-off JPEG refused; on the corpus, the prefix picture has the same size and a mean
difference under 3/255 against the whole-file picture). The picture can differ slightly from before when a larger preview
sits at the end of the file and a smaller one (still at least the requested size) sits in the head.

Not changed (deliberately): the content hash read at import still reads the whole file, and changing it would alter the
catalogue's `content_hash` (a format change that needs a tested migration).

## Slow volume (USB disk; a NAS is slower still): 27 Oct 2026

The library of originals is on `/Volumes/Qnap bureau`, which is today a **USB disk** (`diskutil`: Protocol USB), not a network
share. No SMB mount was available, so nothing here is a network figure. Test: 40 Canon CR2 (26.5 MB, 5760×3840) of an existing
folder, read in place through symbolic links (originals never written), `perf_bench run usb40 24`.

Raw read speed (Python, `F_NOCACHE`, 20 distinct files each): **whole file 22.5 MB/s = 1.16 s per file**; first 3 MiB
30.6 MB/s = 103 ms per file. Reading a raw whole is what made everything slow:

| Step, 40 CR2 on USB | Before | After |
|---|---|---|
| Import (probe + content key + catalogue) | 31.0 s and 32.2 s in two runs (774–805 ms/photo) | **7.9 s** (198 ms/photo), one run |
| First paint, 24 visible cells | 2.6 s | 1.1 s |
| 40 final thumbnails | 28.7 s (decode reads each raw whole) | 26.0 s (not improved) |
| Embedded preview read, per photo (A/B) | 859 ms whole file, 26.5 MB | 140 ms, 3.1 MB |

What changed (`files::probe_head`, `lightcraft_raw::data_windows`): a raw over 6 MiB is described from its first 2 MiB plus
64 KiB at the start of each image strip (a lossless-JPEG raw keeps its sample header there), and its content key is a
**sampled hash** `ps1-…` (size, then 1 MiB from the start, the middle and the end: the scheme videos already used, `vs1-…`)
instead of the hash of every byte. About 4 MiB are read instead of 26.5. A file the head does not settle is read whole as
before. Formats handled: Sony ARW, Canon CR2, Samsung SRW, Panasonic RW2/RAW, Fuji RAF (GFX 50S layout); on the corpus each
gives the same probe as the whole file (`tests/probe_head.rs`). **Still read whole**: Canon CR3, Nikon NEF, Olympus ORF, older
Fuji RAF, DNG, JPEG and every other format (their header reader needs data past the head).

Catalogue compatibility (no schema change: `content_hash` was already an opaque optional string, videos already use a
sampled one): photos imported earlier keep their full hash; a file imported again from its own path is a duplicate by path;
a copy of an old photo elsewhere is found as a duplicate too (when an older photo has the same size, the file is read once
and, if it is that photo's content, takes its key); Find Missing Photos compares with the scheme of the stored key; probing an
old photo again keeps its key. Tested end to end in `tests/probe_head_import.rs` (old catalogue, new probe, same folder,
copy in another folder, new file, old keys unchanged). Rolling back: an older build reads `ps1-` keys as plain strings; its only
loss is that relinking a moved file cannot verify it by content.

Limits, measured or not:
- The "after" import is one run and the "before" two; the page cache was probably warm for part of the after run (the A/B
  loop shows whole-file reads at 29 ms, i.e. cached, after the thumbnail pass read every file). Treat 7.9 s as an upper
  bound on the gain of the *same order*, not a precise ratio. A cold, repeated measure needs `purge` (sudo).
- A sampled key cannot tell apart two different files that share size and the three sampled MiB, for example a raw whose
  middle was edited in place. An exact comparison (as Import ▸ Copy does for videos) stays available for copies.
- **Thumbnails still decode each raw whole** (26 s for 40 on this disk, 1.5 photos/s): the next step is to build browse
  thumbnails of unedited raws from the embedded preview (head read) and keep full decode for editing and export, which
  changes what a grid cell shows (the camera's rendering, not LightCraft's) and needs a decision.
- Whole-library SQLite on the slow disk: not tested (the catalogue of this run lived in memory).
- `cargo xtask ci` and clippy have not been run on this branch.

## What is not measured, and why

- Aftershoot on **raw** files: the only album allowed for tests holds JPEGs. Its per-photo analysis time and pixels on screen
  were not captured (starting a cull on the album was out of scope; DevTools Network shows requests, not paint times).
- LightCraft on a **cold** disk cache of the originals (see above) and on a NAS.
- LightCraft's UI paint times (the benchmark is headless); use `ui.inspect` → `perf` in the running app for those.
- Culling/analysis models (lot A) and GPU thumbnail rendering.
- `cargo clippy -D warnings` and the full `cargo xtask ci` were **not run** on this branch (the shared machine could not
  afford another workspace build); `cargo fmt` and the two new tests are green. Run `cargo xtask ci` before merging.
