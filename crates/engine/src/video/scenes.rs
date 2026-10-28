//! Cuts between shots (idea of the SCM study, written here): FFmpeg compares neighbouring pictures
//! of the video at 360 lines and reports the times where the picture changes more than a threshold.
//! One picture is cut in the middle of each shot, and a shot longer than `max_gap` gets more, so a
//! long take is not described by a single picture (the weak point of the study's plan).

use std::time::Duration;

/// How much two neighbouring pictures must differ (0 – 1) to be a cut.
pub const THRESHOLD: f64 = 0.3;
/// Longest a shot may go without a picture, in milliseconds.
pub const MAX_GAP_MS: u64 = 8_000;
/// Shots shorter than this are not worth a picture of their own (the cut is ignored).
pub const MIN_SHOT_MS: u64 = 700;
/// Longest the cut detection may take on one video (it reads the whole file).
pub const TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// The times (ms) where `video` changes shot, in order.
#[cfg(not(target_arch = "wasm32"))]
pub fn detect(video: &std::path::Path, threshold: f64) -> Result<Vec<u64>, String> {
    use std::process::Command;

    use super::ffmpeg;
    let bin = ffmpeg::ffmpeg().ok_or_else(|| format!("{} (set {})", ffmpeg::NOT_FOUND, ffmpeg::FFMPEG_ENV))?;
    let t = threshold.clamp(0.05, 0.95);
    let mut cmd = Command::new(bin);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(format!("file:{}", video.display()))
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf"])
        .arg(format!("scale=-2:360,select='gt(scene,{t})',metadata=print:file=-"))
        .args(["-f", "null", "-"]);
    let o = ffmpeg::run(cmd, TIMEOUT)?;
    if !o.ok {
        return Err(if o.stderr.is_empty() { "ffmpeg could not read the video".into() } else { o.stderr.lines().next().unwrap_or("").to_string() });
    }
    Ok(parse_cuts(&String::from_utf8_lossy(&o.stdout)))
}

/// The cut times (ms, sorted, once each) in what `metadata=print` wrote
/// (`frame:3 pts:4004 pts_time:4.004` lines).
pub fn parse_cuts(out: &str) -> Vec<u64> {
    let mut v: Vec<u64> = out
        .lines()
        .filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("pts_time:")))
        .filter_map(|t| t.parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t >= 0.0 && *t < 864_000.0)
        .map(|t| (t * 1000.0).round() as u64)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The moments (ms) to cut for a video of `duration_ms` cut at `cuts`: the middle of each shot, plus
/// enough pictures in a long shot that no stretch longer than `max_gap_ms` has none; at most `max`
/// pictures (the middle of each shot first, then the extra ones of the longest shots).
pub fn plan(duration_ms: u64, cuts: &[u64], max_gap_ms: u64, max: usize) -> Vec<u64> {
    let max = max.max(1);
    let max_gap = max_gap_ms.max(500);
    let mut bounds = vec![0u64];
    for &c in cuts {
        if c < duration_ms && c.saturating_sub(*bounds.last().unwrap_or(&0)) >= MIN_SHOT_MS {
            bounds.push(c);
        }
    }
    if duration_ms.saturating_sub(*bounds.last().unwrap_or(&0)) < MIN_SHOT_MS && bounds.len() > 1 {
        bounds.pop();
    }
    bounds.push(duration_ms);
    let shots: Vec<(u64, u64)> = bounds.windows(2).map(|w| (w[0], w[1])).collect();
    // one picture per shot first
    let mut mids: Vec<u64> = shots.iter().map(|(a, b)| a + (b - a) / 2).collect();
    if mids.len() > max {
        // too many shots: spread the pictures over them evenly
        let step = mids.len() as f64 / max as f64;
        mids = (0..max).map(|i| mids[((i as f64 + 0.5) * step) as usize % mids.len()]).collect();
    } else {
        // the extra pictures of long shots, longest first, while room is left
        let mut extra: Vec<(u64, u64, u64)> = shots
            .iter()
            .filter(|(a, b)| b - a > max_gap)
            .map(|(a, b)| (b - a, *a, *b))
            .collect();
        extra.sort_by_key(|x| std::cmp::Reverse(x.0));
        let mut room = max - mids.len();
        for (len, a, _) in extra {
            let n = len.div_ceil(max_gap) as usize;
            if n <= 1 {
                continue;
            }
            let take = n.min(room + 1);
            // the shot's pictures: `take` evenly spread (the middle one is already there when `take` is odd)
            mids.retain(|m| !(*m > a && *m < a + len && *m == a + len / 2));
            for i in 0..take {
                mids.push(a + (len as f64 * (i as f64 + 0.5) / take as f64) as u64);
            }
            room = room.saturating_sub(take - 1);
        }
    }
    mids.sort_unstable();
    mids.dedup();
    mids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_are_read_from_the_metadata_lines() {
        let out = "frame:0    pts:30030   pts_time:30.03\nlavfi.scene_score=0.41\nframe:1    pts:90090   pts_time:90.09\nlavfi.scene_score=0.5\nnoise\npts_time:nan\n";
        assert_eq!(parse_cuts(out), vec![30_030, 90_090]);
        assert!(parse_cuts("").is_empty() && parse_cuts("pts_time:-3 pts_time:abc").is_empty());
    }

    #[test]
    fn one_picture_per_shot_and_more_in_a_long_one() {
        // 0–10 s, 10–12 s, 12–60 s
        let p = plan(60_000, &[10_000, 12_000], MAX_GAP_MS, 100);
        assert!(p.contains(&11_000), "the short shot has its middle: {p:?}");
        assert!(p.iter().all(|t| *t < 60_000));
        // no stretch of the long shot (12–60 s) left without a picture for more than 8 s + a margin
        let long: Vec<u64> = p.iter().copied().filter(|t| *t >= 12_000).collect();
        assert!(long.len() >= 6, "{long:?}");
        for w in long.windows(2) {
            assert!(w[1] - w[0] <= MAX_GAP_MS + 1, "{long:?}");
        }
        // a video without cuts is one long shot
        assert!(plan(30_000, &[], MAX_GAP_MS, 100).len() >= 4);
        // a short video without cuts: its middle
        assert_eq!(plan(4_000, &[], MAX_GAP_MS, 100), vec![2_000]);
    }

    #[test]
    fn the_budget_and_hostile_cuts_hold() {
        let cuts: Vec<u64> = (1..500).map(|i| i * 1000).collect();
        let p = plan(500_000, &cuts, MAX_GAP_MS, 40);
        assert!(p.len() <= 40 && !p.is_empty(), "{}", p.len());
        // cuts out of range, unsorted-looking duplicates, a zero duration
        assert_eq!(plan(0, &[5, 9_999_999], MAX_GAP_MS, 10).len(), 1);
        assert!(plan(10_000, &[20_000, 3, 4], MAX_GAP_MS, 1).len() == 1);
        // cuts that make slivers are ignored
        let p = plan(10_000, &[100, 200, 300], MAX_GAP_MS, 50);
        assert!(p.len() <= 3, "{p:?}");
    }
}
