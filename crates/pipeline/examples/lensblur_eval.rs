//! Measures Lens Blur on synthetic scenes where the right answer is known: a sharp subject over
//! a textured background, once with the layered compositing (`lensblur::apply`) and once with a
//! plain "blur each pixel by its own depth" gather (what causes halos).
//!
//! `cargo run --release -p lightcraft-pipeline --example lensblur_eval`
use lightcraft_develop::LensBlur;
use lightcraft_pipeline::lensblur;
use lightcraft_raster::{Plane, Rgb32f};

const N: usize = 256;

fn scene(foreground_near: bool) -> (Rgb32f, Plane, Vec<bool>) {
    let mut img = Rgb32f::new(N, N);
    let mut depth = Plane::new(N, N);
    let mut fg = vec![false; N * N];
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 128.0, y as f32 - 128.0);
            let inside = (dx * dx / 3600.0 + dy * dy / 2500.0) <= 1.0;
            // textured background: a fine checker with a gradient
            let tex = if (x / 3 + y / 3) % 2 == 0 { 0.12 } else { 0.55 };
            let bg = [tex, tex * 0.9, tex * 0.8 + x as f32 / 2000.0];
            // the subject: saturated red, with a little texture of its own
            let s = 0.7 + 0.2 * (((x + y) % 5) as f32 / 4.0);
            img.data[y * N + x] = if inside { [s, 0.08, 0.05] } else { bg };
            depth.data[y * N + x] = if inside == foreground_near { 0.85 } else { 0.15 };
            fg[y * N + x] = inside;
        }
    }
    (img, depth, fg)
}

/// Gather with each pixel's own radius, no occlusion handling.
fn naive(img: &Rgb32f, depth: &Plane, lb: &LensBlur, radius: f32) -> Rgb32f {
    let mut out = img.clone();
    for y in 0..N {
        for x in 0..N {
            let r = lensblur::signed_coc(depth.data[y * N + x], lb, radius).abs();
            if r < 0.5 {
                continue;
            }
            let ri = r.ceil() as i32;
            let (mut s, mut n) = ([0f32; 3], 0f32);
            for dy in -ri..=ri {
                for dx in -ri..=ri {
                    if ((dx * dx + dy * dy) as f32).sqrt() <= r {
                        let (xx, yy) = ((x as i32 + dx).clamp(0, N as i32 - 1) as usize, (y as i32 + dy).clamp(0, N as i32 - 1) as usize);
                        let p = img.data[yy * N + xx];
                        s[0] += p[0];
                        s[1] += p[1];
                        s[2] += p[2];
                        n += 1.0;
                    }
                }
            }
            out.data[y * N + x] = s.map(|v| v / n);
        }
    }
    out
}

/// Mean "redness" (R − G) of the background pixels within 1..8 px outside the subject: the subject's
/// colour leaking into the blurred background.
fn leak(img: &Rgb32f, fg: &[bool]) -> f32 {
    let (mut s, mut n) = (0.0f32, 0.0f32);
    for y in 8..N - 8 {
        for x in 8..N - 8 {
            if fg[y * N + x] {
                continue;
            }
            let near = (-8i32..=8).any(|d| fg[(y as i32 + d) as usize * N + x] || fg[y * N + (x as i32 + d) as usize]);
            if near {
                let p = img.data[y * N + x];
                s += p[0] - p[1];
                n += 1.0;
            }
        }
    }
    s / n.max(1.0)
}

/// Mean gradient energy inside the subject (3 px in from its edge).
fn sharpness(img: &Rgb32f, fg: &[bool]) -> f32 {
    let (mut s, mut n) = (0.0f32, 0.0f32);
    for y in 4..N - 4 {
        for x in 4..N - 4 {
            if (-3i32..=3).all(|d| fg[(y as i32 + d) as usize * N + x] && fg[y * N + (x as i32 + d) as usize]) {
                let (a, b, c) = (img.data[y * N + x][0], img.data[y * N + x + 1][0], img.data[(y + 1) * N + x][0]);
                s += (a - b).powi(2) + (a - c).powi(2);
                n += 1.0;
            }
        }
    }
    s / n.max(1.0)
}

fn main() {
    let radius = 10.0f32;
    println!("scene: sharp red ellipse over a fine checker; blur radius {radius} px");
    for (name, near_fg, focus) in [("subject near, focus on it", true, 85.0), ("subject far, focus on it", false, 15.0)] {
        let (img, depth, fg) = scene(near_fg);
        let lb = LensBlur { enabled: true, amount: 100.0, focal_distance: focus, focal_range: 20.0 };
        // amount 100 → radius 0.02 × px_per_long: choose px_per_long for `radius` px
        let ppl = f64::from(radius) / lensblur::MAX_RADIUS;
        let mut layered = img.clone();
        let t = std::time::Instant::now();
        lensblur::apply(&mut layered, &depth, &lb, ppl);
        let layered_ms = t.elapsed().as_secs_f64() * 1e3;
        let plain = naive(&img, &depth, &lb, radius);
        println!("\n{name}");
        println!(
            "  background leak (R-G just outside the subject):  source {:+.4}   layered {:+.4}   naive {:+.4}",
            leak(&img, &fg),
            leak(&layered, &fg),
            leak(&plain, &fg)
        );
        println!(
            "  subject sharpness (gradient energy):             source {:.5}   layered {:.5}   naive {:.5}",
            sharpness(&img, &fg),
            sharpness(&layered, &fg),
            sharpness(&plain, &fg)
        );
        println!("  layered blur: {layered_ms:.1} ms for {N}x{N}");
    }
}
