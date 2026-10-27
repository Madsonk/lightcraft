//! FFmpeg as a helper program (never linked: LightCraft stays pure Rust). `ffprobe` describes a
//! video, `ffmpeg` cuts one picture out of it. Every call is a child process started without a
//! shell, with its arguments passed one by one, its output bounded and a deadline after which it
//! is killed: a broken, huge or hostile file costs at most that deadline, never a hang or a crash.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Environment variables naming the programs (full paths), when they are not found by themselves.
pub const FFMPEG_ENV: &str = "LIGHTCRAFT_FFMPEG";
pub const FFPROBE_ENV: &str = "LIGHTCRAFT_FFPROBE";
/// What errors about the missing programs start with.
pub const NOT_FOUND: &str = "FFmpeg is not installed";

/// Longest a probe may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest cutting one picture may take (a slow network share, a long seek).
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(90);
/// Most bytes read from a program's output (a picture; a probe's JSON).
const MAX_OUTPUT: usize = 64 << 20;
const MAX_ERRORS: usize = 16 << 10;

/// Folders looked in after `PATH` (an app started from the Finder has a short `PATH`).
const FALLBACK_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/opt/local/bin"];

fn find(name: &str, env: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).chain(FALLBACK_DIRS.iter().map(PathBuf::from)).map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// `ffmpeg`, if installed.
pub fn ffmpeg() -> Option<PathBuf> {
    find("ffmpeg", FFMPEG_ENV)
}

/// `ffprobe`, if installed.
pub fn ffprobe() -> Option<PathBuf> {
    find("ffprobe", FFPROBE_ENV)
}

fn missing(which: &str, env: &str) -> String {
    format!("{NOT_FOUND}: `{which}` was not found (install FFmpeg, e.g. `brew install ffmpeg`, or set {env} to its full path)")
}

/// A path as an FFmpeg input/output that is never read as an option or a protocol (`-x`, `http:`).
fn file_arg(p: &Path) -> String {
    format!("file:{}", p.display())
}

/// Output of a finished program.
pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

fn read_bounded(mut r: impl Read, max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64 << 10];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // keep draining (a child blocked on a full pipe would never exit) but stop keeping
                let room = max.saturating_sub(out.len());
                out.extend_from_slice(buf.get(..n.min(room)).unwrap_or(&[]));
            }
        }
    }
    out
}

/// Run `cmd`, killing it after `timeout`.
pub fn run(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not start {:?}: {e}", cmd.get_program()))?;
    let out = child.stdout.take().map(|o| std::thread::spawn(move || read_bounded(o, MAX_OUTPUT)));
    let err = child.stderr.take().map(|e| std::thread::spawn(move || read_bounded(e, MAX_ERRORS)));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
    match status {
        None => Err(format!("{:?} took longer than {} s and was stopped", cmd.get_program(), timeout.as_secs())),
        Some(s) => Ok(Output { ok: s.success(), stdout, stderr }),
    }
}

/// What `ffprobe` says about a video.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoInfo {
    /// As displayed (rotation applied).
    pub width: u32,
    pub height: u32,
    /// Seconds; `None` when unknown or not a usable number.
    pub duration: Option<f64>,
    pub codec: String,
    /// Container (`mov,mp4,m4a,3gp,3g2,mj2`, `matroska,webm`…).
    pub container: String,
    /// `creation_time` of the container, as written (ISO 8601).
    pub created: Option<String>,
}

/// A duration that can be used: finite, positive, below ten days.
pub fn sane_duration(d: Option<f64>) -> Option<f64> {
    d.filter(|d| d.is_finite() && *d > 0.0 && *d < 864_000.0)
}

fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Read `ffprobe -of json` output (pure, for tests).
pub fn parse_probe(json: &[u8]) -> Result<VideoInfo, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("unreadable ffprobe output: {e}"))?;
    let stream = v.get("streams").and_then(|s| s.as_array()).and_then(|s| s.first()).ok_or("the file has no video track")?;
    let dim = |k: &str| stream.get(k).and_then(num).filter(|x| x.is_finite() && *x >= 1.0 && *x <= 65_536.0).map(|x| x as u32);
    let (Some(mut w), Some(mut h)) = (dim("width"), dim("height")) else { return Err("the video track has no picture size".into()) };
    // a phone video filmed upright is stored sideways with a rotation to apply
    let rotation = stream
        .get("side_data_list")
        .and_then(|l| l.as_array())
        .into_iter()
        .flatten()
        .filter_map(|d| d.get("rotation").and_then(num))
        .chain(stream.get("tags").and_then(|t| t.get("rotate")).and_then(num))
        .find(|r| r.is_finite())
        .unwrap_or(0.0);
    if ((rotation.abs() / 90.0).round() as i64) % 2 == 1 {
        std::mem::swap(&mut w, &mut h);
    }
    let format = v.get("format");
    let duration =
        sane_duration(format.and_then(|f| f.get("duration")).and_then(num)).or_else(|| sane_duration(stream.get("duration").and_then(num)));
    Ok(VideoInfo {
        width: w,
        height: h,
        duration,
        codec: stream.get("codec_name").and_then(|c| c.as_str()).unwrap_or_default().to_string(),
        container: format.and_then(|f| f.get("format_name")).and_then(|c| c.as_str()).unwrap_or_default().to_string(),
        created: format.and_then(|f| f.get("tags")).and_then(|t| t.get("creation_time")).and_then(|c| c.as_str()).map(str::to_string),
    })
}

/// Describe the video at `path`.
pub fn probe(path: &Path) -> Result<VideoInfo, String> {
    let bin = ffprobe().ok_or_else(|| missing("ffprobe", FFPROBE_ENV))?;
    let mut cmd = Command::new(bin);
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,codec_name,duration:stream_tags=rotate:stream_side_data=rotation:format=duration,format_name:format_tags=creation_time",
        "-of",
        "json",
    ])
    .arg(file_arg(path));
    let out = run(cmd, PROBE_TIMEOUT)?;
    if !out.ok {
        return Err(if out.stderr.is_empty() {
            "not a readable video".to_string()
        } else {
            format!("not a readable video: {}", first_line(&out.stderr))
        });
    }
    parse_probe(&out.stdout)
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

/// `-vf` that fits a picture inside `edge` × `edge` (never enlarged, even sizes for the encoder).
fn fit_filter(edge: u32) -> String {
    let e = edge.clamp(64, 8192);
    format!("scale=w='if(gte(iw,ih),min({e},iw),-2)':h='if(gte(iw,ih),-2,min({e},ih))'")
}

fn frame_command(bin: &Path, video: &Path, at_ms: u64, edge: u32) -> Command {
    let mut cmd = Command::new(bin);
    // `-ss` before `-i`: a fast seek to the nearest key picture, then decoded up to the time asked
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{:.3}", at_ms as f64 / 1000.0))
        .arg("-i")
        .arg(file_arg(video))
        .args(["-map", "0:v:0", "-frames:v", "1", "-an", "-sn", "-dn", "-vf"])
        .arg(fit_filter(edge))
        .args(["-c:v", "mjpeg", "-q:v", "3"]);
    cmd
}

/// Cut the picture shown at `at_ms` into a JPEG file at `out` (at most `edge` pixels on its long
/// side). Written to a temporary file first, so `out` is either whole or absent.
pub fn extract_frame(video: &Path, at_ms: u64, edge: u32, out: &Path) -> Result<u64, String> {
    let bin = ffmpeg().ok_or_else(|| missing("ffmpeg", FFMPEG_ENV))?;
    let tmp = out.with_extension("part");
    let _ = std::fs::remove_file(&tmp);
    let mut cmd = frame_command(&bin, video, at_ms, edge);
    cmd.args(["-f", "image2", "-update", "1", "-y"]).arg(file_arg(&tmp));
    let r = run(cmd, FRAME_TIMEOUT);
    let len = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    match r {
        Ok(o) if o.ok && len > 0 => {
            std::fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))?;
            Ok(len)
        }
        Ok(o) => {
            let _ = std::fs::remove_file(&tmp);
            Err(if o.stderr.is_empty() {
                format!("no picture at {at_ms} ms")
            } else {
                format!("no picture at {at_ms} ms: {}", first_line(&o.stderr))
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Longest a short clip may take to make.
pub const CLIP_TIMEOUT: Duration = Duration::from_secs(180);

/// A short clip of `video` from `start_ms`, `len_ms` long, 540 lines high, H.264 + AAC in MP4
/// (`+faststart`, so a player starts at once), at `out`, written to a temporary file first.
pub fn extract_clip(video: &Path, start_ms: u64, len_ms: u64, out: &Path) -> Result<u64, String> {
    let bin = ffmpeg().ok_or_else(|| missing("ffmpeg", FFMPEG_ENV))?;
    let tmp = out.with_extension("part");
    let _ = std::fs::remove_file(&tmp);
    let mut last = String::new();
    // the software encoder first; macOS's own encoder when that one is not in this FFmpeg
    for codec in [["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"], ["-c:v", "h264_videotoolbox", "-b:v", "3M", "-realtime", "1"]] {
        let mut cmd = Command::new(&bin);
        cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-ss"])
            .arg(format!("{:.3}", start_ms as f64 / 1000.0))
            .arg("-i")
            .arg(file_arg(video))
            .arg("-t")
            .arg(format!("{:.3}", len_ms as f64 / 1000.0))
            .args(["-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn", "-vf", "scale=-2:'min(540,ih)'", "-pix_fmt", "yuv420p"])
            .args(codec)
            .args(["-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart", "-f", "mp4", "-y"])
            .arg(file_arg(&tmp));
        let r = run(cmd, CLIP_TIMEOUT);
        let len = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        match r {
            Ok(o) if o.ok && len > 0 => {
                std::fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))?;
                return Ok(len);
            }
            Ok(o) => last = first_line(&o.stderr).to_string(),
            Err(e) => last = e,
        }
        let _ = std::fs::remove_file(&tmp);
    }
    Err(if last.is_empty() { "the clip could not be made".into() } else { format!("the clip could not be made: {last}") })
}

/// The pictures of a short clip, `fps` a second, at most `edge` pixels on their long side, as JPEG
/// files' bytes (a preview without sound). At most `max` pictures.
pub fn clip_pictures(clip: &Path, fps: u32, edge: u32, max: usize) -> Result<Vec<Vec<u8>>, String> {
    let bin = ffmpeg().ok_or_else(|| missing("ffmpeg", FFMPEG_ENV))?;
    let mut cmd = Command::new(bin);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(file_arg(clip))
        .args(["-map", "0:v:0", "-an", "-vf"])
        .arg(format!("fps={},{}", fps.clamp(1, 30), fit_filter(edge)))
        .args(["-c:v", "mjpeg", "-q:v", "5", "-f", "image2pipe", "-"]);
    let o = run(cmd, FRAME_TIMEOUT)?;
    if !o.ok {
        return Err(if o.stderr.is_empty() { "the clip cannot be read".into() } else { first_line(&o.stderr).to_string() });
    }
    Ok(split_jpegs(&o.stdout, max))
}

/// JPEG files laid end to end (start `FF D8`, end `FF D9`), split.
pub fn split_jpegs(bytes: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = None;
    let mut i = 0;
    while i + 1 < bytes.len() && out.len() < max {
        match (bytes.get(i), bytes.get(i + 1)) {
            (Some(0xFF), Some(0xD8)) if start.is_none() => start = Some(i),
            (Some(0xFF), Some(0xD9)) => {
                if let Some(s) = start.take() {
                    out.push(bytes.get(s..i + 2).unwrap_or(&[]).to_vec());
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The JPEG bytes of the picture at `at_ms` (for a poster), without touching the disk.
pub fn frame_bytes(video: &Path, at_ms: u64, edge: u32) -> Result<Vec<u8>, String> {
    let bin = ffmpeg().ok_or_else(|| missing("ffmpeg", FFMPEG_ENV))?;
    let mut cmd = frame_command(&bin, video, at_ms, edge);
    cmd.args(["-f", "image2pipe", "-"]);
    let o = run(cmd, FRAME_TIMEOUT)?;
    if o.ok && !o.stdout.is_empty() {
        Ok(o.stdout)
    } else {
        Err(if o.stderr.is_empty() { format!("no picture at {at_ms} ms") } else { format!("no picture at {at_ms} ms: {}", first_line(&o.stderr)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_output_hostile_values_never_panic() {
        let ok = br#"{"streams":[{"codec_name":"h264","width":1920,"height":1080,"side_data_list":[{"rotation":-90}]}],"format":{"duration":"12.5","format_name":"mov,mp4"}}"#;
        let v = parse_probe(ok).unwrap();
        assert_eq!((v.width, v.height, v.duration, v.codec.as_str()), (1080, 1920, Some(12.5), "h264"));
        for bad in [
            &br#"{"streams":[]}"#[..],
            br#"{}"#,
            br#"not json"#,
            br#"{"streams":[{"width":-5,"height":1e30}]}"#,
            br#"{"streams":[{"width":"NaN","height":10}]}"#,
            b"",
        ] {
            assert!(parse_probe(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        for d in ["\"NaN\"", "\"inf\"", "-3", "0", "\"\"", "1e300"] {
            let j = format!(r#"{{"streams":[{{"width":64,"height":48}}],"format":{{"duration":{d}}}}}"#);
            assert_eq!(parse_probe(j.as_bytes()).unwrap().duration, None, "{d}");
        }
    }

    #[test]
    fn jpeg_streams_split_and_hostile_bytes_never_panic() {
        let one = [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
        let two: Vec<u8> = one.iter().chain(one.iter()).copied().collect();
        assert_eq!(split_jpegs(&two, 10).len(), 2);
        assert_eq!(split_jpegs(&two, 1).len(), 1);
        for bad in [&[][..], &[0xFF], &[0xFF, 0xD8, 0xFF], &[0xFF, 0xD9, 0xFF, 0xD9]] {
            assert!(split_jpegs(bad, 10).is_empty());
        }
    }

    #[test]
    fn a_program_that_hangs_is_stopped() {
        if !Path::new("/bin/sleep").exists() {
            return;
        }
        let mut c = Command::new("/bin/sleep");
        c.arg("30");
        let t = Instant::now();
        assert!(run(c, Duration::from_millis(300)).is_err());
        assert!(t.elapsed() < Duration::from_secs(10));
    }
}
