//! Reproducible grouped cross-validation folds (whole shoots never straddle two folds).

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
