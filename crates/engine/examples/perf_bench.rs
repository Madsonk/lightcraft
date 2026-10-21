//! Library-speed benchmark (lot E): import, first grid paint, thumbnails, next photo in the loupe,
//! analysis scan, memory — measured on a folder of test photos, through the same engine calls the
//! app makes (the grid's job pool, `thumb_job`, `quick_thumb_job`, `loupe_job`, `quick_view_job`).
//!
//! ```text
//! cargo run --release -p lightcraft-engine --example perf_bench -- make  <src raw dir> <dest> <N>
//! cargo run --release -p lightcraft-engine --example perf_bench -- run   <dest> [VISIBLE=48]
//! ```
//!
//! `make` builds the test set from CC0 raw samples (`cargo xtask corpus --download`): N files, ~70 %
//! raw and ~30 % camera JPEGs (the raws' embedded previews), each made unique by a few trailing
//! bytes (the engine keys everything by content hash, so exact copies would all hit one cache entry).
//! Originals are only read. `run` prints `metric<TAB>value<TAB>unit` lines (`scripts/perf.sh` and
//! `docs/performance.md` use them). Env: `THREADS` (job-pool workers, default as the app),
//! `COLD=1` (drop nothing — only tags the run; the disk cache is always fresh for a cold pass).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::import::{ImportMode, import};
use lightcraft_engine::media::RenderResult;
use lightcraft_preview::JobPool;

/// Peak resident set (the only `unsafe` is this libc call, in a dev-only example, as in `render_bench`).
#[allow(unsafe_code)]
fn rss_mb() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: getrusage only writes into the struct we pass.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    // macOS reports bytes, Linux kilobytes
    let k = if cfg!(target_os = "macos") { 1024.0 * 1024.0 } else { 1024.0 };
    ru.ru_maxrss as f64 / k
}

fn out(metric: &str, v: f64, unit: &str) {
    println!("{metric}\t{v:.1}\t{unit}");
}

fn make(src: &Path, dest: &Path, n: usize) {
    std::fs::create_dir_all(dest).unwrap();
    let mut raws: Vec<PathBuf> = std::fs::read_dir(src)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| !matches!(e.to_string_lossy().to_lowercase().as_str(), "txt" | "sha256" | "md")))
        .collect();
    raws.sort();
    assert!(!raws.is_empty(), "no raw samples in {}", src.display());
    let mut jpegs: Vec<Vec<u8>> = Vec::new();
    for r in &raws {
        if let Some(j) = std::fs::read(r).ok().and_then(|b| lightcraft_raw::embedded_preview(&b)) {
            jpegs.push(j);
        }
    }
    for i in 0..n {
        let tail = format!("lightcraft-perf-{i:06}").into_bytes();
        if i % 10 >= 7 && !jpegs.is_empty() {
            let mut b = jpegs[i % jpegs.len()].clone();
            b.extend_from_slice(&tail);
            std::fs::write(dest.join(format!("IMG_{i:05}.jpg")), b).unwrap();
        } else {
            let r = &raws[i % raws.len()];
            // APFS clone (`cp -c`: no extra disk), then a few bytes appended: only the last block is copied
            let ext = r.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let to = dest.join(format!("IMG_{i:05}.{ext}"));
            let ok = std::process::Command::new("/bin/cp").arg("-c").arg(r).arg(&to).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                std::fs::copy(r, &to).unwrap();
            }
            use std::io::Write;
            std::fs::OpenOptions::new().append(true).open(&to).unwrap().write_all(&tail).unwrap();
        }
    }
    println!("made {n} files in {}", dest.display());
}

type Pool = JobPool<u64, RenderResult>;

/// Wait until `want` results with a slot in `slots` arrived; returns (ms to first, ms to all).
fn drain(pool: &Pool, t0: Instant, mut left: std::collections::HashSet<u64>) -> (f64, f64) {
    let mut first = None;
    while !left.is_empty() {
        match pool.try_recv() {
            Some(d) => {
                if left.remove(&d.slot) && first.is_none() {
                    first = Some(t0.elapsed().as_secs_f64() * 1e3);
                }
            }
            None => std::thread::sleep(std::time::Duration::from_micros(200)),
        }
    }
    (first.unwrap_or(0.0), t0.elapsed().as_secs_f64() * 1e3)
}

fn run(dir: &Path, visible: usize) {
    let tmp = std::env::temp_dir().join(format!("lc-perf-{}", std::process::id()));
    let cache = tmp.join("thumbs");
    let threads: usize = std::env::var("THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| Pool::default_threads().min(6));
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.media.attach_disk_cache(&cache, lightcraft_engine::media::DISK_CACHE_BYTES);

    // 1. import
    let t = Instant::now();
    let rep = import(&mut s, &[dir.to_string_lossy().to_string()], ImportMode::Add).expect("import");
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let n = rep.imported.len();
    println!("# photos {n} (failed {}, duplicates {}), pool threads {threads}", rep.failed.len(), rep.duplicates.len());
    out("import_total", ms, "ms");
    out("import_per_photo", ms / n.max(1) as f64, "ms");
    out("rss_after_import", rss_mb(), "MB");
    let ids: Vec<PhotoId> = rep.imported.iter().map(|i| PhotoId(*i)).collect();

    // 2. first grid paint: quick (embedded preview) thumbnails of the visible cells
    let mut pool: Pool = JobPool::new(threads);
    let t0 = Instant::now();
    let vis: Vec<PhotoId> = ids.iter().copied().take(visible).collect();
    let mut left = std::collections::HashSet::new();
    for id in &vis {
        let Some(job) = s.thumb_job(*id, 256) else { continue };
        let slot = id.0;
        left.insert(slot);
        match s.quick_thumb_job(&job) {
            Some(mut q) => {
                q.key = job.key;
                pool.submit(slot, job.key, 10, Box::new(move || lightcraft_engine::memory::in_background(|| q.run())));
            }
            None => pool.submit(slot, job.key, 10, Box::new(move || lightcraft_engine::memory::in_background(|| job.run()))),
        }
    }
    let (first, all) = drain(&pool, t0, left);
    out("grid_first_thumb", first, "ms");
    out("grid_visible_first_paint", all, "ms");
    out("grid_preview_mb_read", lightcraft_engine::files::PREVIEW_BYTES_READ.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6, "MB");

    // 3. final thumbnails: visible first (priority 10), then the rest (priority 1) — cold disk cache
    let t0 = Instant::now();
    let mut vis_left = std::collections::HashSet::new();
    let mut all_left = std::collections::HashSet::new();
    for (i, id) in ids.iter().enumerate() {
        let Some(job) = s.thumb_job(*id, 256) else { continue };
        let slot = id.0 + (1 << 40); // distinct from the quick slots
        all_left.insert(slot);
        if i < visible {
            vis_left.insert(slot);
        }
        let prio = if i < visible { 10 } else { 1 };
        pool.submit(slot, job.key, prio, Box::new(move || lightcraft_engine::memory::in_background(|| job.run())));
    }
    let mut vis_done = None;
    while !all_left.is_empty() {
        match pool.try_recv() {
            Some(d) => {
                all_left.remove(&d.slot);
                if vis_left.remove(&d.slot) && vis_left.is_empty() {
                    vis_done = Some(t0.elapsed().as_secs_f64() * 1e3);
                }
            }
            None => std::thread::sleep(std::time::Duration::from_micros(200)),
        }
    }
    let total = t0.elapsed().as_secs_f64() * 1e3;
    out("thumbs_visible_final_cold", vis_done.unwrap_or(0.0), "ms");
    out("thumbs_all_cold_total", total, "ms");
    out("thumbs_all_cold_per_photo", total / n.max(1) as f64, "ms");
    out("thumbs_all_cold_rate", n as f64 / (total / 1e3), "photos/s");
    out("rss_after_thumbs", rss_mb(), "MB");
    // let background cache writers finish before the warm pass
    std::thread::sleep(std::time::Duration::from_millis(1500));

    // 4. warm pass: a fresh session (restart) with the disk cache already filled
    let mut s2 = lightcraft_engine::Session::new().with_fs();
    s2.media.attach_disk_cache(&cache, lightcraft_engine::media::DISK_CACHE_BYTES);
    let rep2 = import(&mut s2, &[dir.to_string_lossy().to_string()], ImportMode::Add).expect("import");
    let ids2: Vec<PhotoId> = rep2.imported.iter().map(|i| PhotoId(*i)).collect();
    let t0 = Instant::now();
    let mut left = std::collections::HashSet::new();
    let mut pool2: Pool = JobPool::new(threads);
    for id in ids2.iter().take(visible) {
        let Some(job) = s2.thumb_job(*id, 256) else { continue };
        left.insert(id.0);
        pool2.submit(id.0, job.key, 10, Box::new(move || lightcraft_engine::memory::in_background(|| job.run())));
    }
    let (_, all) = drain(&pool2, t0, left);
    out("grid_visible_warm_restart", all, "ms");

    // 5. next photo in the loupe: stand-in (embedded preview / cached) then the sharp 2560 px render
    let loupe = 2560usize;
    let mut stand = Vec::new();
    let mut sharp = Vec::new();
    let mut sharp_prefetched = Vec::new();
    let sample: Vec<PhotoId> = ids.iter().copied().skip(visible).step_by((n / 20).max(1)).take(20).collect();
    for id in &sample {
        if let Some(q) = s.quick_view_job(*id, loupe, true) {
            let t = Instant::now();
            let _ = q.run();
            stand.push(t.elapsed().as_secs_f64() * 1e3);
        }
        if let Some(job) = s.loupe_job(*id, loupe, loupe, true) {
            let t = Instant::now();
            let r = job.run();
            sharp.push(t.elapsed().as_secs_f64() * 1e3);
            s.accept(&r);
        }
    }
    // with the neighbour prefetched in the background while the previous photo is viewed
    for w in sample.windows(2) {
        let (cur, next) = (w[0], w[1]);
        if let Some(job) = s.loupe_job(next, loupe, loupe, true) {
            // prefetch = the source decode of the neighbour (the app's Slot::Prefetch job)
            let r = lightcraft_engine::memory::in_background(|| job.run());
            s.accept(&r);
        }
        let _ = cur;
        if let Some(job) = s.loupe_job(next, loupe, loupe, true) {
            let t = Instant::now();
            let _ = job.run();
            sharp_prefetched.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    out("loupe_standin_median", med(&mut stand), "ms");
    out("loupe_sharp_cold_median", med(&mut sharp), "ms");
    out("loupe_sharp_prefetched_median", med(&mut sharp_prefetched), "ms");

    // 6. analysis per photo: what a scan reads (embedded preview for raws, else the decoded thumbnail)
    let t = Instant::now();
    let mut count = 0usize;
    for id in ids.iter().take(60) {
        if let Some(p) = s.catalog.photo(*id)
            && let lightcraft_catalog::Source::File { path } = &p.source
        {
            let _ = lightcraft_engine::memory::in_background(|| (lightcraft_engine::files::fs_preview_loader())(path, 1024));
            count += 1;
        }
    }
    out("preview_mb_read_total", lightcraft_engine::files::PREVIEW_BYTES_READ.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6, "MB");
    if count > 0 {
        out("analysis_input_per_photo", t.elapsed().as_secs_f64() * 1e3 / count as f64, "ms");
    }
    // 7. A/B of the embedded-preview read: the whole file vs the first megabytes (same files, same process)
    for edge in [256usize, 1024, 2560] {
        let paths: Vec<String> = ids
            .iter()
            .filter_map(|id| match &s.catalog.photo(*id)?.source {
                lightcraft_catalog::Source::File { path } if lightcraft_engine::files::PREVIEW_PREFIX > 0 => Some(path.clone()),
                _ => None,
            })
            .filter(|p| !p.ends_with(".jpg"))
            .take(60)
            .collect();
        let (mut full_bytes, mut hits, mut pre_bytes) = (0u64, 0usize, 0u64);
        let t = Instant::now();
        for p in &paths {
            let b = std::fs::read(p).unwrap();
            full_bytes += b.len() as u64;
            let _ = lightcraft_engine::files::embedded_preview_srgb(&b, edge);
        }
        let full_ms = t.elapsed().as_secs_f64() * 1e3 / paths.len().max(1) as f64;
        let t = Instant::now();
        for p in &paths {
            let before = lightcraft_engine::files::PREVIEW_BYTES_READ.load(std::sync::atomic::Ordering::Relaxed);
            let got = lightcraft_engine::files::preview_from_prefix(p, edge);
            let after = lightcraft_engine::files::PREVIEW_BYTES_READ.load(std::sync::atomic::Ordering::Relaxed);
            pre_bytes += after - before;
            if got.is_some() {
                hits += 1;
            } else {
                // the loader then reads the whole file: count it and its time
                full_bytes += 0;
                pre_bytes += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                let b = std::fs::read(p).unwrap();
                let _ = lightcraft_engine::files::embedded_preview_srgb(&b, edge);
            }
        }
        let pre_ms = t.elapsed().as_secs_f64() * 1e3 / paths.len().max(1) as f64;
        let n = paths.len().max(1) as f64;
        out(&format!("ab{edge}_whole_file_ms_per_photo"), full_ms, "ms");
        out(&format!("ab{edge}_prefix_ms_per_photo"), pre_ms, "ms");
        out(&format!("ab{edge}_whole_file_mb_per_photo"), full_bytes as f64 / n / 1e6, "MB");
        out(&format!("ab{edge}_prefix_mb_per_photo"), pre_bytes as f64 / n / 1e6, "MB");
        out(&format!("ab{edge}_prefix_hit_rate"), 100.0 * hits as f64 / n, "%");
    }
    out("rss_peak", rss_mb(), "MB");
    let _ = std::fs::remove_dir_all(&tmp);
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("make") => make(Path::new(&a[1]), Path::new(&a[2]), a[3].parse().expect("N")),
        Some("run") => run(Path::new(&a[1]), a.get(2).and_then(|v| v.parse().ok()).unwrap_or(48)),
        _ => eprintln!("usage: perf_bench make <raw dir> <dest> <N> | run <dir> [visible]"),
    }
}
