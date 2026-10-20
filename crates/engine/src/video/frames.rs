//! The pictures cut out of a video: which moments ([`plan`]), the list kept next to them
//! ([`Manifest`], `frames.json`) and the cutting itself ([`extract`]), which resumes where an
//! interrupted run stopped: a picture already listed is not cut again.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Pictures per video when nothing is asked (Video Catcher's choice: about twelve).
pub const DEFAULT_FRAMES: usize = 12;
/// The most pictures one video may get, whatever is asked.
pub const MAX_FRAMES: usize = 240;
/// Long side of a picture (Video Catcher's pictures are 1280 × 720).
pub const FRAME_EDGE: u32 = 1280;
/// The list of a video's pictures in its folder.
pub const MANIFEST: &str = "frames.json";
const VERSION: u32 = 1;

/// When to cut pictures, in milliseconds, for a video of `duration` seconds: `every` seconds
/// apart when asked (at most `max`), else [`DEFAULT_FRAMES`] spread over the video (one every
/// 2 s for a short one). An unknown or absurd duration (0, negative, NaN, infinite) gives one
/// picture, at the start.
pub fn plan(duration: Option<f64>, every: Option<f64>, max: usize) -> Vec<u64> {
    let max = max.clamp(1, MAX_FRAMES);
    #[cfg(not(target_arch = "wasm32"))]
    let duration = super::ffmpeg::sane_duration(duration);
    #[cfg(target_arch = "wasm32")]
    let duration = duration.filter(|d| d.is_finite() && *d > 0.0 && *d < 864_000.0);
    let Some(d) = duration else { return vec![0] };
    let every = every.filter(|e| e.is_finite() && *e >= 0.1);
    let n = match every {
        Some(e) => ((d / e).floor() as usize).clamp(1, max),
        None => (DEFAULT_FRAMES.min((d / 2.0).floor() as usize)).clamp(1, max),
    };
    let step = match every {
        Some(e) if (n as f64) * e <= d => e,
        _ => d / n as f64,
    };
    let mut out: Vec<u64> = (0..n).map(|i| ((i as f64 + 0.5) * step * 1000.0).floor() as u64).collect();
    out.dedup();
    out
}

/// One picture of the list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub ms: u64,
    /// File name in the video's folder.
    pub file: String,
    #[serde(default)]
    pub bytes: u64,
}

/// `frames.json`: what was cut out of one video, and from what.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    /// The catalog content key of the video (the folder's name is a hash of it).
    pub content_key: String,
    /// The video file the pictures came from.
    pub path: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Who cut them: `ffmpeg`, `videocatcher`.
    #[serde(default)]
    pub source: String,
    pub frames: Vec<Frame>,
    /// Moments that could not be cut, and why.
    #[serde(default)]
    pub failed: Vec<(u64, String)>,
    /// Every moment of the last plan is listed (or failed).
    #[serde(default)]
    pub complete: bool,
}

impl Manifest {
    /// The list in `dir`, if it is there and readable (an unreadable list is as good as none: the
    /// pictures are cut again).
    pub fn load(dir: &Path) -> Option<Manifest> {
        let bytes = std::fs::read(dir.join(MANIFEST)).ok()?;
        let m: Manifest = serde_json::from_slice(&bytes).ok()?;
        (m.version == VERSION).then_some(m)
    }

    pub fn new(content_key: &str, path: &str, duration_ms: Option<u64>, source: &str) -> Manifest {
        Manifest { version: VERSION, content_key: content_key.into(), path: path.into(), duration_ms, source: source.into(), ..Default::default() }
    }

    /// Write the list (a temporary file renamed over the old one: a crash leaves either).
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(format!("{MANIFEST}.part"));
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join(MANIFEST)).map_err(|e| format!("{}: {e}", dir.display()))
    }

    pub fn has(&self, ms: u64) -> bool {
        self.frames.iter().any(|f| f.ms == ms)
    }

    pub fn bytes(&self) -> u64 {
        self.frames.iter().map(|f| f.bytes).sum()
    }
}

/// The file name of the picture at `ms`.
pub fn file_name(ms: u64) -> String {
    format!("t{ms:010}.jpg")
}

/// What cutting one video did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cut {
    pub planned: usize,
    pub cut: usize,
    pub kept: usize,
    pub failed: usize,
    pub bytes: u64,
    /// Stopped because the pictures of the library reached their disk budget.
    pub over_budget: bool,
    pub error: Option<String>,
}

/// Cut the pictures `times` of `video` into `dir`, keeping those already there (unless `force`),
/// while `budget` (bytes still allowed) lasts. The list is written after every picture.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub fn extract(
    video: &Path,
    content_key: &str,
    duration_ms: Option<u64>,
    dir: &Path,
    times: &[u64],
    force: bool,
    budget: &std::sync::atomic::AtomicU64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Cut {
    use std::sync::atomic::Ordering;
    let mut cut = Cut { planned: times.len(), ..Default::default() };
    let mut m = match Manifest::load(dir) {
        Some(m) if !force && m.content_key == content_key => m,
        old => {
            // another video's pictures under this name (a hash collision), or `force`: start over
            if let Some(old) = old {
                for f in &old.frames {
                    let _ = std::fs::remove_file(dir.join(&f.file));
                }
            }
            Manifest::new(content_key, &video.to_string_lossy(), duration_ms, "ffmpeg")
        }
    };
    m.path = video.to_string_lossy().into();
    m.duration_ms = duration_ms.or(m.duration_ms);
    if let Err(e) = std::fs::create_dir_all(dir) {
        cut.error = Some(format!("{}: {e}", dir.display()));
        return cut;
    }
    m.failed.retain(|(ms, _)| !times.contains(ms));
    for &ms in times {
        if m.has(ms) {
            cut.kept += 1;
            continue;
        }
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if budget.load(Ordering::SeqCst) == 0 {
            cut.over_budget = true;
            break;
        }
        let name = file_name(ms);
        match super::ffmpeg::extract_frame(video, ms, FRAME_EDGE, &dir.join(&name)) {
            Ok(len) => {
                let mut left = budget.load(Ordering::SeqCst);
                while let Err(now) = budget.compare_exchange_weak(left, left.saturating_sub(len), Ordering::SeqCst, Ordering::SeqCst) {
                    left = now;
                }
                cut.cut += 1;
                cut.bytes += len;
                m.frames.push(Frame { ms, file: name, bytes: len });
                m.frames.sort_by_key(|f| f.ms);
            }
            Err(e) => {
                cut.failed += 1;
                m.failed.push((ms, e));
            }
        }
        if let Err(e) = m.save(dir) {
            cut.error = Some(e);
            return cut;
        }
    }
    m.complete = times.iter().all(|t| m.has(*t) || m.failed.iter().any(|(f, _)| f == t));
    if let Err(e) = m.save(dir) {
        cut.error = Some(e);
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_is_bounded_and_survives_hostile_durations() {
        assert_eq!(plan(Some(60.0), None, 120).len(), DEFAULT_FRAMES);
        assert_eq!(plan(Some(60.0), None, 120)[0], 2500);
        assert_eq!(plan(Some(5.0), None, 120), vec![1250, 3750]);
        assert_eq!(plan(Some(0.5), None, 120), vec![250]);
        assert_eq!(plan(Some(60.0), Some(10.0), 120), vec![5000, 15000, 25000, 35000, 45000, 55000]);
        assert_eq!(plan(Some(36_000.0), Some(0.1), 10_000).len(), MAX_FRAMES);
        for d in [None, Some(0.0), Some(-4.0), Some(f64::NAN), Some(f64::INFINITY), Some(1e300)] {
            assert_eq!(plan(d, None, 12), vec![0], "{d:?}");
        }
        for e in [Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let p = plan(Some(60.0), e, 12);
            assert!(!p.is_empty() && p.len() <= 12 && p.iter().all(|t| *t < 60_000), "{e:?} {p:?}");
        }
        assert!(plan(Some(60.0), None, 0).len() == 1);
    }

    #[test]
    fn a_damaged_list_is_ignored() {
        let d = std::env::temp_dir().join(format!("lc-frames-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(Manifest::load(&d).is_none());
        std::fs::write(d.join(MANIFEST), b"{\"version\":1,\"conte").unwrap();
        assert!(Manifest::load(&d).is_none());
        let mut m = Manifest::new("k", "/v.mp4", Some(1000), "ffmpeg");
        m.frames.push(Frame { ms: 5, file: file_name(5), bytes: 3 });
        m.save(&d).unwrap();
        assert_eq!(Manifest::load(&d).unwrap(), m);
        let _ = std::fs::remove_dir_all(&d);
    }
}
