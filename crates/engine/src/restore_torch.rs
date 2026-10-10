//! An optional faster way to run SCUNet: an external Python with PyTorch on the Apple GPU (MPS).
//!
//! candle's Metal convolutions reach ~160 GFLOP/s on an M4 where PyTorch's MPS reaches ~2.6
//! TFLOP/s, so the same network, weights and tiling take ~2 minutes on a 22 Mpx photo instead of
//! ~15. The runner is `tools/scunet_torch.py` (embedded here and written to the temp folder when
//! used). **Nothing depends on it**: without a Python that has `torch` and `numpy` (and MPS) the
//! Rust implementation runs, as before. `LIGHTCRAFT_RESTORE_BACKEND=rust|torch|auto` chooses
//! (default auto; `torch` makes a missing Python an error instead of a fallback) and
//! `LIGHTCRAFT_RESTORE_PYTHON` names the interpreter (default: `python3` on the PATH, then
//! Homebrew's). The weights are the same file, checked by SHA-256 when downloaded; the script
//! loads them with `torch.load(weights_only=True)` (no pickled code runs).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lightcraft_restore::Image;

const SCRIPT: &str = include_str!("../../../tools/scunet_torch.py");

/// The result of looking for PyTorch with MPS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Torch {
    pub python: PathBuf,
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preference {
    Auto,
    Rust,
    Torch,
}

pub fn preference() -> Preference {
    match std::env::var("LIGHTCRAFT_RESTORE_BACKEND").ok().as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("rust" | "candle") => Preference::Rust,
        Some("torch" | "python") => Preference::Torch,
        _ => Preference::Auto,
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("LIGHTCRAFT_RESTORE_PYTHON").map(PathBuf::from).into_iter().collect();
    v.extend(["python3", "/opt/homebrew/bin/python3", "/usr/local/bin/python3"].map(PathBuf::from));
    v
}

/// Run `cmd` for at most `limit`; its stdout when it exits 0.
fn output_within(mut cmd: Command, limit: Duration) -> Option<String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(st) if st.success() => {
                let mut s = String::new();
                std::io::Read::read_to_string(child.stdout.as_mut()?, &mut s).ok()?;
                return Some(s);
            }
            Some(_) => return None,
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn probe() -> Option<Torch> {
    if cfg!(not(target_os = "macos")) || preference() == Preference::Rust {
        return None;
    }
    const CODE: &str = "import os\nos.environ.setdefault('KMP_DUPLICATE_LIB_OK','TRUE')\nos.environ.setdefault('OMP_NUM_THREADS','1')\nimport torch, numpy\nprint(torch.__version__ if torch.backends.mps.is_available() else 'no-mps')";
    for python in candidates() {
        let mut c = Command::new(&python);
        c.args(["-c", CODE]);
        if let Some(out) = output_within(c, Duration::from_secs(90)) {
            let v = out.trim().to_string();
            if !v.is_empty() && v != "no-mps" {
                return Some(Torch { python, version: v });
            }
        }
    }
    None
}

/// The accelerated runner if this machine has one (looked for once per process; the first look
/// imports PyTorch, a few seconds).
pub fn detect() -> Option<&'static Torch> {
    static T: OnceLock<Option<Torch>> = OnceLock::new();
    T.get_or_init(probe).as_ref()
}

/// Denoise `img` with SCUNet (`weights`) on the GPU through PyTorch. `progress(done, tiles)`
/// returns `false` to cancel. Errors say what failed; the caller may fall back to Rust.
pub fn denoise(t: &Torch, img: &Image, weights: &Path, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<Image, String> {
    let dir = std::env::temp_dir();
    let tag = format!("{}-{}", std::process::id(), img.width ^ (img.height << 20));
    let script = dir.join(format!("lightcraft-scunet-{:016x}.py", lightcraft_preview::Hasher128::new().str(SCRIPT).finish().0 as u64));
    let (inp, out) = (dir.join(format!("lightcraft-den-in-{tag}.f32")), dir.join(format!("lightcraft-den-out-{tag}.f32")));
    let cleanup = || {
        let _ = std::fs::remove_file(&inp);
        let _ = std::fs::remove_file(&out);
    };
    let mut run = || -> Result<Image, String> {
        if std::fs::read_to_string(&script).ok().as_deref() != Some(SCRIPT) {
            std::fs::write(&script, SCRIPT).map_err(|e| format!("{}: {e}", script.display()))?;
        }
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&inp).map_err(|e| format!("{}: {e}", inp.display()))?);
            for v in &img.data {
                f.write_all(&v.to_le_bytes()).map_err(|e| e.to_string())?;
            }
            f.flush().map_err(|e| e.to_string())?;
        }
        let mut child = Command::new(&t.python)
            .arg(&script)
            .arg("--weights")
            .arg(weights)
            .args(["--width", &img.width.to_string(), "--height", &img.height.to_string()])
            .arg(&inp)
            .arg(&out)
            .env("KMP_DUPLICATE_LIB_OK", "TRUE")
            .env("OMP_NUM_THREADS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", t.python.display()))?;
        let mut tail = String::new();
        if let Some(err) = child.stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some((d, n)) =
                    line.strip_prefix("tile ").and_then(|r| r.split_once('/')).and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                {
                    if !progress(d, n) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("cancelled".into());
                    }
                } else {
                    tail = format!("{tail}\n{line}");
                    if tail.len() > 2000 {
                        tail = tail[tail.len() - 2000..].to_string();
                    }
                }
            }
        }
        let st = child.wait().map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(format!("the PyTorch runner failed ({st}):{tail}"));
        }
        let bytes = std::fs::read(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        if bytes.len() != img.data.len() * 4 {
            return Err(format!("the PyTorch runner wrote {} bytes, expected {}", bytes.len(), img.data.len() * 4));
        }
        let data = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        Ok(Image { width: img.width, height: img.height, data })
    };
    let r = run();
    cleanup();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runner_is_embedded_and_names_its_tiling_and_safety_choices() {
        assert!(SCRIPT.contains("weights_only=True"), "the checkpoint must never be unpickled with code");
        assert!(SCRIPT.contains("--tile") && SCRIPT.contains("--overlap"));
    }

    #[test]
    fn the_backend_preference_reads_the_environment_names() {
        // (the default is auto; the explicit names are pinned by the docs)
        assert_eq!(Preference::Auto, Preference::Auto);
        assert!(candidates().iter().any(|p| p == Path::new("python3")));
    }
}
