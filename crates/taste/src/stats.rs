//! Metrics and splits: Spearman rank correlation, ROC AUC, top-k precision, grouped k-fold.

/// Ranks with ties averaged (1-based). NaN sort last.
pub fn ranks(v: &[f32]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|a, b| v[*a].total_cmp(&v[*b]));
    let mut r = vec![0.0; v.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && v[idx[j + 1]] == v[idx[i]] {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for k in i..=j {
            r[idx[k]] = avg;
        }
        i = j + 1;
    }
    r
}

/// Pearson correlation (`None` when either side is constant).
fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != b.len() || a.len() < 2 {
        return None;
    }
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    let d = (saa * sbb).sqrt();
    (d > 0.0).then(|| sab / d)
}

/// Spearman rank correlation of two equally long series (`None` when one is constant).
pub fn spearman(a: &[f32], b: &[f32]) -> Option<f64> {
    pearson(&ranks(a), &ranks(b))
}

/// ROC AUC: the chance that a random positive scores higher than a random negative (ties count
/// half). `None` without both classes.
pub fn auc(scores: &[f32], positive: &[bool]) -> Option<f64> {
    if scores.len() != positive.len() {
        return None;
    }
    let r = ranks(scores);
    let np = positive.iter().filter(|p| **p).count() as f64;
    let nn = positive.len() as f64 - np;
    if np == 0.0 || nn == 0.0 {
        return None;
    }
    let sum: f64 = r.iter().zip(positive).filter(|(_, p)| **p).map(|(r, _)| *r).sum();
    Some((sum - np * (np + 1.0) / 2.0) / (np * nn))
}

/// Of the `k` best-scored items, how many are `relevant` (ties broken by position, so a model
/// that scores everything alike gets the luck of the order, not credit).
pub fn hits_at_k(scores: &[f32], relevant: &[bool], k: usize) -> usize {
    let mut idx: Vec<usize> = (0..scores.len().min(relevant.len())).collect();
    idx.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]).then(a.cmp(b)));
    idx.iter().take(k).filter(|i| relevant[**i]).count()
}

/// A small deterministic generator (splitmix64) so that splits are reproducible.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The fold (0..k) of every item when whole groups are kept together: groups are dealt out in a
/// shuffled order to the currently lightest fold, so the folds come out about the same size.
/// `group[i]` is any id (a shoot, a folder); items sharing it never straddle two folds, which is
/// what stops near-identical frames of one shoot from leaking between training and validation.
pub fn group_folds(group: &[u64], k: usize, seed: u64) -> Vec<usize> {
    let k = k.max(1);
    let mut ids: Vec<u64> = group.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut rng = Rng(seed);
    for i in (1..ids.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        ids.swap(i, j);
    }
    let size = |g: u64| group.iter().filter(|x| **x == g).count();
    let mut sized: Vec<(u64, usize)> = ids.into_iter().map(|g| (g, size(g))).collect();
    // big groups first (stable after the shuffle), so the small ones even the folds out
    sized.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut load = vec![0usize; k];
    let mut fold_of = std::collections::HashMap::new();
    for (g, n) in sized {
        let f = (0..k).min_by_key(|f| load[*f]).unwrap_or(0);
        load[f] += n;
        fold_of.insert(g, f);
    }
    group.iter().map(|g| fold_of.get(g).copied().unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spearman_known_values() {
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[10.0, 20.0, 30.0, 40.0]).unwrap() - 1.0).abs() < 1e-12);
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[4.0, 3.0, 2.0, 1.0]).unwrap() + 1.0).abs() < 1e-12);
        // monotone but not linear
        assert!((spearman(&[1.0, 2.0, 3.0], &[1.0, 10.0, 1000.0]).unwrap() - 1.0).abs() < 1e-12);
        // ties: [1,1,2] vs [1,2,3] → ranks [1.5,1.5,3] vs [1,2,3]
        let s = spearman(&[1.0, 1.0, 2.0], &[1.0, 2.0, 3.0]).unwrap();
        assert!((s - 0.8660254).abs() < 1e-6, "{s}");
        assert!(spearman(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]).is_none());
        assert!(spearman(&[1.0], &[1.0]).is_none());
    }

    #[test]
    fn auc_and_hits() {
        assert_eq!(auc(&[0.9, 0.8, 0.2, 0.1], &[true, true, false, false]), Some(1.0));
        assert_eq!(auc(&[0.1, 0.2, 0.8, 0.9], &[true, true, false, false]), Some(0.0));
        assert_eq!(auc(&[0.5, 0.5], &[true, false]), Some(0.5));
        assert_eq!(auc(&[0.5, 0.5], &[true, true]), None);
        assert_eq!(hits_at_k(&[0.9, 0.1, 0.8], &[true, false, true], 2), 2);
        assert_eq!(hits_at_k(&[0.9, 0.1, 0.8], &[false, true, false], 2), 0);
        assert_eq!(hits_at_k(&[1.0], &[true], 5), 1);
    }

    #[test]
    fn group_folds_keep_groups_whole_and_balanced() {
        let group: Vec<u64> = (0..100).map(|i| (i / 10) as u64).collect();
        let f = group_folds(&group, 5, 1);
        for g in 0..10u64 {
            let folds: std::collections::HashSet<usize> = group.iter().zip(&f).filter(|(x, _)| **x == g).map(|(_, f)| *f).collect();
            assert_eq!(folds.len(), 1);
        }
        for k in 0..5 {
            assert_eq!(f.iter().filter(|x| **x == k).count(), 20);
        }
        assert_eq!(f, group_folds(&group, 5, 1));
    }
}
