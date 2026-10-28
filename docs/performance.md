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

## Reading the head of a raw for its embedded preview (upstream 0.14.0's way)

The fork's own version of this (a 3 MB prefix read, `preview_from_prefix`) is **not carried**: upstream 0.14.0 reads the
header and exactly the preview window it points to (`docs/raw-preview-reads.md`, `files::preview_window`), which is
better (5.9 MB of a 56 MB raw, no 3 MB guess, planned reads for containers the header cannot point into). Both were
measured; the fork's numbers (bytes per photo down 66 % at 256 px against reading the whole file) are in the git
history (`ca036d1b`). `perf_bench` now measures upstream's loader: `files::PREVIEW_BYTES_READ` counts the bytes
`fs_preview_loader` reads, and section 7 of the benchmark compares it with reading the whole file.

Not changed (deliberately): the content hash read at import still reads the whole file, and changing it would alter the
catalogue's `content_hash` (a format change that needs a tested migration).

## What is not measured, and why

- Aftershoot on **raw** files: the only album allowed for tests holds JPEGs. Its per-photo analysis time and pixels on screen
  were not captured (starting a cull on the album was out of scope; DevTools Network shows requests, not paint times).
- LightCraft on a **cold** disk cache of the originals (see above) and on a NAS.
- LightCraft's UI paint times (the benchmark is headless); use `ui.inspect` → `perf` in the running app for those.
- Culling/analysis models (lot A) and GPU thumbnail rendering.
- `cargo clippy -D warnings` and the full `cargo xtask ci` were **not run** on this branch (the shared machine could not
  afford another workspace build); `cargo fmt` and the two new tests are green. Run `cargo xtask ci` before merging.
