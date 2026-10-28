//! `bench_index [N]` — query time and file size of an index of N random unit vectors (default 30 000).
use lightcraft_search::VectorIndex;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let n: usize = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(30_000);
    let path = std::env::temp_dir().join("lc-bench-index.bin");
    let _ = std::fs::remove_file(&path);
    let mut ix = VectorIndex::open(Some(&path), "bench", 768)?;
    let mut x = 88172645463325252u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % 2000) as f32 / 1000.0 - 1.0
    };
    let entries: Vec<(u64, Vec<f32>)> = (0..n).map(|i| (i as u64, (0..768).map(|_| next()).collect())).collect();
    ix.add(&entries)?;
    println!("{n} vectors: file {:.1} MB", std::fs::metadata(&path)?.len() as f64 / 1e6);
    let t = std::time::Instant::now();
    let reopened = VectorIndex::open(Some(&path), "bench", 768)?;
    println!("reopen (read + f16→f32): {:.0} ms ({})", t.elapsed().as_secs_f64() * 1e3, reopened.len());
    let q = &entries[7].1;
    let mut times = Vec::new();
    for _ in 0..20 {
        let t = std::time::Instant::now();
        let top = reopened.top(q, 50, &|_| true);
        times.push(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(top[0].0, 7);
    }
    times.sort_by(f64::total_cmp);
    println!("top-50 query: median {:.2} ms, max {:.2} ms", times[10], times[19]);
    let _ = std::fs::remove_file(&path);
    Ok(())
}
