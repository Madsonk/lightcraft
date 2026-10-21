//! Shooting scenes ("smart shooting time"): shots are ordered by capture time and a new scene
//! starts at a gap that is long for *this* shoot (a multiple of its median gap, never under the
//! profile's minimum). A scene that is still very long is split by appearance with k-means on the
//! look-alike signatures, so a reception filmed in one go still separates dinner from dancing.

use crate::{Profile, Shot};

/// Scenes never get a larger gap threshold than this (4 h).
const MAX_GAP: i64 = 4 * 3600;
/// A gap this many times the median gap starts a scene.
const GAP_FACTOR: i64 = 8;
/// A scene longer than this is split by appearance.
const SPLIT_ABOVE: usize = 60;
/// The size k-means aims at.
const SPLIT_TARGET: usize = 30;

/// The scene of every shot (input order), numbered chronologically from 0; shots without a
/// capture time form the last scene.
pub fn segment(shots: &[Shot], profile: &Profile) -> Vec<u32> {
    let mut timed: Vec<usize> = (0..shots.len()).filter(|&i| shots.get(i).is_some_and(|s| s.time.is_some())).collect();
    timed.sort_by_key(|&i| (shots.get(i).and_then(|s| s.time).unwrap_or(0), i));
    let undated: Vec<usize> = (0..shots.len()).filter(|&i| shots.get(i).is_some_and(|s| s.time.is_none())).collect();
    let time = |i: usize| shots.get(i).and_then(|s| s.time).unwrap_or(0);

    let mut gaps: Vec<i64> = timed.windows(2).map(|w| time(w[1]).saturating_sub(time(w[0])).max(0)).collect();
    let threshold = if gaps.is_empty() {
        profile.scene_gap_secs
    } else {
        gaps.sort_unstable();
        let median = gaps.get(gaps.len() / 2).copied().unwrap_or(0);
        profile.scene_gap_secs.max(median.saturating_mul(GAP_FACTOR)).min(MAX_GAP.max(profile.scene_gap_secs))
    };

    let mut runs: Vec<Vec<usize>> = Vec::new();
    for &i in &timed {
        match runs.last_mut() {
            Some(run) if run.last().is_some_and(|&p| time(i).saturating_sub(time(p)) <= threshold) => run.push(i),
            _ => runs.push(vec![i]),
        }
    }
    let mut scenes: Vec<Vec<usize>> = Vec::new();
    for run in runs {
        if run.len() > SPLIT_ABOVE {
            scenes.extend(split_by_look(shots, &run));
        } else {
            scenes.push(run);
        }
    }
    if !undated.is_empty() {
        scenes.push(undated);
    }
    let mut out = vec![0u32; shots.len()];
    for (n, members) in scenes.iter().enumerate() {
        for &i in members {
            if let Some(slot) = out.get_mut(i) {
                *slot = n as u32;
            }
        }
    }
    out
}

/// Split a time-ordered run into appearance clusters, each still in time order, scenes ordered by
/// their first shot.
fn split_by_look(shots: &[Shot], run: &[usize]) -> Vec<Vec<usize>> {
    let k = run.len().div_ceil(SPLIT_TARGET).max(2);
    let vecs: Vec<&[f32]> = run.iter().map(|&i| shots.get(i).map_or(&[][..], |s| s.signature.as_slice())).collect();
    let labels = kmeans(&vecs, k, 12);
    let mut clusters: Vec<Vec<usize>> = vec![Vec::new(); k];
    for (pos, &i) in run.iter().enumerate() {
        if let Some(c) = labels.get(pos).and_then(|&l| clusters.get_mut(l)) {
            c.push(i);
        }
    }
    clusters.retain(|c| !c.is_empty());
    clusters.sort_by_key(|c| c.first().copied().unwrap_or(0));
    clusters
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Spherical k-means (cosine) with a deterministic farthest-point start; returns the cluster of
/// each vector. Vectors of different lengths just look unlike each other.
pub fn kmeans(vecs: &[&[f32]], k: usize, rounds: usize) -> Vec<usize> {
    let n = vecs.len();
    if n == 0 || k <= 1 {
        return vec![0; n];
    }
    let k = k.min(n);
    let get = |i: usize| vecs.get(i).copied().unwrap_or(&[]);
    // farthest-point initialisation, starting from the first vector
    let mut centers: Vec<Vec<f32>> = vec![get(0).to_vec()];
    while centers.len() < k {
        let mut best = (0usize, f32::INFINITY);
        for i in 0..n {
            let nearest = centers.iter().map(|c| dot(c, get(i))).fold(f32::NEG_INFINITY, f32::max);
            if nearest < best.1 {
                best = (i, nearest);
            }
        }
        centers.push(get(best.0).to_vec());
    }
    let mut labels = vec![0usize; n];
    for _ in 0..rounds {
        let mut moved = false;
        for i in 0..n {
            let mut best = (0usize, f32::NEG_INFINITY);
            for (c, center) in centers.iter().enumerate() {
                let d = dot(center, get(i));
                if d > best.1 {
                    best = (c, d);
                }
            }
            if labels.get(i) != Some(&best.0) {
                moved = true;
                if let Some(l) = labels.get_mut(i) {
                    *l = best.0;
                }
            }
        }
        if !moved {
            break;
        }
        for (c, center) in centers.iter_mut().enumerate() {
            let members: Vec<usize> = (0..n).filter(|&i| labels.get(i) == Some(&c)).collect();
            if members.is_empty() {
                continue;
            }
            let len = center.len();
            let mut sum = vec![0f32; len];
            for &i in &members {
                for (s, v) in sum.iter_mut().zip(get(i)) {
                    *s += v;
                }
            }
            let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
            *center = sum.into_iter().map(|v| v / norm).collect();
        }
    }
    labels
}
