//! Duplicate groups: shots of one scene that look alike and were taken close together. Found by
//! comparing each shot with the next few of its scene (time order) and joining the pairs whose
//! signature similarity reaches the threshold (union-find), so a burst of five is one group even
//! when the first and last frames differ a little.

use crate::{Shot, similarity};

/// How many following shots each shot is compared with.
const WINDOW: usize = 24;
/// Extra similarity asked of shots without a capture time (nothing else says they are close).
const UNDATED_EXTRA: f32 = 0.04;

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, i: usize) -> usize {
        let mut root = i;
        while let Some(&p) = self.0.get(root) {
            if p == root {
                break;
            }
            root = p;
        }
        let mut cur = i;
        while let Some(&p) = self.0.get(cur) {
            if p == root {
                break;
            }
            if let Some(slot) = self.0.get_mut(cur) {
                *slot = root;
            }
            cur = p;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = (ra.min(rb), ra.max(rb));
            if let Some(slot) = self.0.get_mut(hi) {
                *slot = lo;
            }
        }
    }
}

/// The duplicate group of every shot (input order): `Some(n)` (numbered from 0 by the first shot
/// of each group in input order) when it has at least one look-alike, `None` otherwise.
pub fn group(shots: &[Shot], scenes: &[u32], threshold: f32, gap_secs: i64) -> Vec<Option<u32>> {
    let n = shots.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (scenes.get(i).copied().unwrap_or(0), shots.get(i).and_then(|s| s.time).unwrap_or(i64::MAX), i));
    let mut dsu = Dsu((0..n).collect());
    for (pos, &i) in order.iter().enumerate() {
        let (Some(a), Some(&sa)) = (shots.get(i), scenes.get(i)) else { continue };
        for &j in order.iter().skip(pos + 1).take(WINDOW) {
            let (Some(b), Some(&sb)) = (shots.get(j), scenes.get(j)) else { continue };
            if sa != sb {
                break;
            }
            let need = match (a.time, b.time) {
                (Some(x), Some(y)) => {
                    if y.saturating_sub(x).abs() > gap_secs {
                        // sorted by time: the following shots are further still
                        break;
                    }
                    threshold
                }
                _ => threshold + UNDATED_EXTRA,
            };
            if similarity(&a.signature, &b.signature) >= need {
                dsu.union(i, j);
            }
        }
    }
    let mut size = vec![0usize; n];
    for i in 0..n {
        let r = dsu.find(i);
        if let Some(c) = size.get_mut(r) {
            *c += 1;
        }
    }
    let mut number = vec![None; n];
    let mut next = 0u32;
    let mut out = vec![None; n];
    for i in 0..n {
        let r = dsu.find(i);
        if size.get(r).copied().unwrap_or(0) < 2 {
            continue;
        }
        let slot = number.get_mut(r);
        if let Some(slot) = slot {
            let g = *slot.get_or_insert_with(|| {
                next += 1;
                next - 1
            });
            if let Some(o) = out.get_mut(i) {
                *o = Some(g);
            }
        }
    }
    out
}
