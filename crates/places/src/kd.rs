//! A static k-d tree over points on the unit sphere, stored implicitly: the points are laid out so that the node of
//! the slice `lo..hi` is the element at `lo + (hi - lo) / 2`, its left subtree is `lo..mid`, its right subtree
//! `mid + 1..hi`, and the splitting axis is `depth % 3`. [`arrange`] puts points in that order (the data builder does
//! it once); [`Tree::nearest`] searches without any allocation besides the result.

/// A point on the unit sphere (x, y, z).
pub type Xyz = [f32; 3];

/// Unit-sphere coordinates of a position given in 1e-5 degrees.
pub fn to_xyz(lat_e5: i32, lon_e5: i32) -> Xyz {
    let lat = (f64::from(lat_e5) * 1e-5).to_radians();
    let lon = (f64::from(lon_e5) * 1e-5).to_radians();
    let (sl, cl) = lat.sin_cos();
    let (so, co) = lon.sin_cos();
    [(cl * co) as f32, (cl * so) as f32, sl as f32]
}

/// Unit-sphere coordinates of a position in degrees.
pub fn deg_to_xyz(lat: f64, lon: f64) -> [f64; 3] {
    let (sl, cl) = lat.to_radians().sin_cos();
    let (so, co) = lon.to_radians().sin_cos();
    [cl * co, cl * so, sl]
}

/// Reorder `items` (with their `points`, kept in step) into the implicit k-d order.
pub fn arrange<T>(items: &mut [T], point: &impl Fn(&T) -> Xyz) {
    fn go<T>(items: &mut [T], depth: usize, point: &impl Fn(&T) -> Xyz) {
        if items.len() < 2 {
            return;
        }
        let mid = items.len() / 2;
        let axis = depth % 3;
        items.select_nth_unstable_by(mid, |a, b| {
            let (pa, pb) = (point(a), point(b));
            pa.get(axis).copied().unwrap_or(0.0).total_cmp(&pb.get(axis).copied().unwrap_or(0.0))
        });
        let (left, rest) = items.split_at_mut(mid);
        go(left, depth + 1, point);
        if let Some((_, right)) = rest.split_first_mut() {
            go(right, depth + 1, point);
        }
    }
    go(items, 0, point);
}

/// The k nearest accepted points found so far, closest first.
struct Best {
    k: usize,
    found: Vec<(f64, usize)>,
}

impl Best {
    fn worst(&self) -> f64 {
        if self.found.len() < self.k { f64::INFINITY } else { self.found.last().map_or(f64::INFINITY, |f| f.0) }
    }
    fn offer(&mut self, d2: f64, idx: usize) {
        if d2 >= self.worst() {
            return;
        }
        let at = self.found.partition_point(|f| f.0 <= d2);
        self.found.insert(at, (d2, idx));
        self.found.truncate(self.k);
    }
}

/// A borrowed tree in implicit order.
pub struct Tree<'a> {
    pub points: &'a [Xyz],
}

impl Tree<'_> {
    /// The indexes and squared chord distances of the `k` accepted points nearest to `q`, closest first.
    pub fn nearest(&self, q: [f64; 3], k: usize, accept: &impl Fn(usize) -> bool) -> Vec<(usize, f64)> {
        let mut best = Best { k: k.max(1), found: Vec::new() };
        self.visit(0, self.points.len(), 0, &q, &mut best, accept);
        best.found.into_iter().map(|(d2, i)| (i, d2)).collect()
    }

    fn visit(&self, lo: usize, hi: usize, depth: usize, q: &[f64; 3], best: &mut Best, accept: &impl Fn(usize) -> bool) {
        if lo >= hi {
            return;
        }
        let mid = lo + (hi - lo) / 2;
        let Some(p) = self.points.get(mid) else { return };
        let p = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        if accept(mid) {
            let d2 = (0..3).map(|i| (p[i] - q[i]) * (p[i] - q[i])).sum();
            best.offer(d2, mid);
        }
        let axis = depth % 3;
        let diff = q[axis] - p[axis];
        let (near, far) = if diff < 0.0 { ((lo, mid), (mid + 1, hi)) } else { ((mid + 1, hi), (lo, mid)) };
        self.visit(near.0, near.1, depth + 1, q, best, accept);
        if diff * diff < best.worst() {
            self.visit(far.0, far.1, depth + 1, q, best, accept);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 33) as f64) / ((1u64 << 31) as f64)
    }

    #[test]
    fn matches_brute_force_everywhere_including_poles_and_the_antimeridian() {
        let mut seed = 7;
        let mut pts: Vec<(i32, i32)> = (0..3000)
            .map(|_| (lcg(&mut seed) * 180.0 - 90.0, lcg(&mut seed) * 360.0 - 180.0))
            .map(|(la, lo)| ((la * 1e5) as i32, (lo * 1e5) as i32))
            .collect();
        pts.push((9_000_000, 0));
        pts.push((-9_000_000, 0));
        pts.push((0, 18_000_000));
        pts.push((0, -18_000_000));
        arrange(&mut pts, &|p: &(i32, i32)| to_xyz(p.0, p.1));
        let xyz: Vec<Xyz> = pts.iter().map(|p| to_xyz(p.0, p.1)).collect();
        let tree = Tree { points: &xyz };
        let mut queries: Vec<(f64, f64)> = (0..400).map(|_| (lcg(&mut seed) * 180.0 - 90.0, lcg(&mut seed) * 360.0 - 180.0)).collect();
        queries.extend([(90.0, 0.0), (-90.0, 10.0), (0.0, 180.0), (0.0, -180.0), (45.0, 179.999), (45.0, -179.999)]);
        let even = |i: usize| i.is_multiple_of(2);
        for (la, lo) in queries {
            let q = deg_to_xyz(la, lo);
            for (k, accept) in [(1usize, &(|_: usize| true) as &dyn Fn(usize) -> bool), (5, &even)] {
                let got = tree.nearest(q, k, &|i| accept(i));
                let mut brute: Vec<(usize, f64)> = xyz
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| accept(*i))
                    .map(|(i, p)| (i, (0..3).map(|a| (f64::from(p[a]) - q[a]).powi(2)).sum::<f64>()))
                    .collect();
                brute.sort_by(|a, b| a.1.total_cmp(&b.1));
                brute.truncate(k);
                let g: Vec<f64> = got.iter().map(|x| x.1).collect();
                let b: Vec<f64> = brute.iter().map(|x| x.1).collect();
                assert_eq!(g, b, "query ({la}, {lo}) k={k}");
            }
        }
    }
}
