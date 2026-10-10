//! Grouping the faces of one person.
//!
//! Faces whose embeddings have a cosine above a threshold are the same person's. Done in three
//! steps, on the faces nobody has confirmed yet (confirmed faces belong to their [`Person`]):
//!
//! 1. **Known people.** A face close to a person the user confirmed (mean similarity to that
//!    person's three nearest confirmed faces) is a *suggestion* for them, not a new group.
//! 2. **Groups.** The others go through a one-pass leader clustering, best pictures first (a
//!    face joins the nearest group whose centre is above the link threshold, or starts one), two
//!    refinement rounds (every face goes to its nearest centre; faces far from any centre start
//!    their own), then groups whose centres are alike are merged, nearest pair first. Pairs the
//!    user said are different are never put together.
//! 3. **Questions** are the cases the thresholds decide by a hair (see [`crate::ask`]).
//!
//! All similarities are dot products of unit vectors; the work is a few hundred million
//! multiply-adds for a library of 40 000 faces, spread over the cores.

use std::collections::{BinaryHeap, HashMap};

use rayon::prelude::*;

use crate::DIM;
use crate::state::{Settings, State};
use crate::store::{FaceRef, FaceStore};

/// Dot product of two vectors (eight lanes, so the compiler vectorizes it).
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    for (x, y) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        for l in 0..8 {
            acc[l] += x[l] * y[l];
        }
    }
    acc.iter().sum()
}

fn add_to(sum: &mut [f32], v: &[f32]) {
    for (s, x) in sum.iter_mut().zip(v) {
        *s += x;
    }
}

fn unit(sum: &[f32]) -> Vec<f32> {
    let n = dot(sum, sum).sqrt();
    if n < 1e-9 { vec![0.0; sum.len()] } else { sum.iter().map(|x| x / n).collect() }
}

/// A group of faces nobody confirmed yet.
#[derive(Clone, Debug)]
pub struct Group {
    /// Slots in the [`FaceStore`], best picture first.
    pub members: Vec<usize>,
    /// Unit mean of the members' embeddings.
    pub centroid: Vec<f32>,
}

/// A face that looks like a person the user already confirmed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Suggestion {
    pub slot: usize,
    pub person: u32,
    pub score: f32,
    /// The next best person, if any came close.
    pub runner_up: Option<(u32, f32)>,
}

#[derive(Debug, Default)]
pub struct Grouping {
    /// Groups, biggest first.
    pub groups: Vec<Group>,
    pub suggestions: Vec<Suggestion>,
    /// Faces left out: too small, or ignored.
    pub left_out: usize,
}

/// The unit mean embedding of a person's confirmed faces, with the vectors it is made of.
pub struct Known {
    pub id: u32,
    pub centroid: Vec<f32>,
    pub slots: Vec<usize>,
}

/// The known people of `state` with their faces found in `store`.
pub fn known_people(store: &FaceStore, state: &State) -> Vec<Known> {
    state
        .persons
        .iter()
        .filter_map(|p| {
            let slots: Vec<usize> = p.faces.iter().filter_map(|f| store.slot(*f)).collect();
            if slots.is_empty() {
                return None;
            }
            let mut sum = vec![0.0f32; DIM];
            slots.iter().for_each(|s| add_to(&mut sum, store.vector(*s)));
            Some(Known { id: p.id, centroid: unit(&sum), slots })
        })
        .collect()
}

/// How alike a face is to a known person: the mean similarity to their three nearest faces.
pub fn match_score(store: &FaceStore, v: &[f32], k: &Known) -> f32 {
    let mut sims: Vec<f32> = k.slots.iter().map(|s| dot(v, store.vector(*s))).collect();
    sims.sort_by(|a, b| b.total_cmp(a));
    let n = sims.len().clamp(1, 3);
    sims.iter().take(n).sum::<f32>() / n as f32
}

/// Best and second best known person for a face (rejected pairs skipped), by `match_score`.
pub fn best_people(
    store: &FaceStore,
    state: &State,
    known: &[Known],
    face: FaceRef,
    v: &[f32],
    floor: f32,
) -> Option<(u32, f32, Option<(u32, f32)>)> {
    let mut scored: Vec<(u32, f32)> = known
        .iter()
        .filter(|k| !state.is_rejected(face, k.id) && dot(v, &k.centroid) >= floor - 0.2)
        .map(|k| (k.id, match_score(store, v, k)))
        .filter(|(_, s)| *s >= floor - 0.1)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let first = *scored.first()?;
    Some((first.0, first.1, scored.get(1).copied()))
}

/// Group every face of `store` nobody has confirmed.
pub fn group(store: &FaceStore, state: &State) -> Grouping {
    let s: &Settings = &state.settings;
    let confirmed: std::collections::HashSet<FaceRef> = state.persons.iter().flat_map(|p| p.faces.iter().copied()).collect();
    let ignored: std::collections::HashSet<FaceRef> = state.ignored.iter().copied().collect();
    let unknown: std::collections::HashSet<FaceRef> = state.unknown.iter().copied().collect();
    let mut out = Grouping::default();
    let mut eligible: Vec<usize> = Vec::new();
    for slot in store.slots() {
        let Some(m) = store.meta(slot) else { continue };
        if confirmed.contains(&m.face()) {
            continue;
        }
        if ignored.contains(&m.face()) || m.size < s.min_face_px || store.vector(slot).len() != DIM {
            out.left_out += 1;
            continue;
        }
        eligible.push(slot);
    }
    // 1. known people
    let known = known_people(store, state);
    let mut free: Vec<usize> = Vec::new();
    if known.is_empty() {
        free = eligible;
    } else {
        let found: Vec<Option<Suggestion>> = eligible
            .par_iter()
            .map(|&slot| {
                let m = store.meta(slot)?;
                // a face the user marked "unknown" is never suggested as someone: it stays in its group
                if unknown.contains(&m.face()) {
                    return None;
                }
                let (person, score, runner_up) = best_people(store, state, &known, m.face(), store.vector(slot), s.suggest)?;
                (score >= s.suggest).then_some(Suggestion { slot, person, score, runner_up })
            })
            .collect();
        for (slot, f) in eligible.iter().zip(found) {
            match f {
                Some(sg) => out.suggestions.push(sg),
                None => free.push(*slot),
            }
        }
        out.suggestions.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
    out.groups = cluster(store, state, &free, s.link);
    out
}

/// Leader clustering with refinement and merging of `slots` (see the module docs).
pub fn cluster(store: &FaceStore, state: &State, slots: &[usize], link: f32) -> Vec<Group> {
    let n = slots.len();
    if n == 0 {
        return Vec::new();
    }
    let mut x: Vec<f32> = Vec::with_capacity(n * DIM);
    for s in slots {
        x.extend_from_slice(store.vector(*s));
    }
    let row = |i: usize| &x[i * DIM..(i + 1) * DIM];
    let quality: Vec<f32> = slots.iter().map(|s| store.meta(*s).map_or(0.0, |m| m.quality())).collect();
    // faces the user said differ, as local indices
    let local: HashMap<FaceRef, usize> = slots.iter().enumerate().filter_map(|(i, s)| store.meta(*s).map(|m| (m.face(), i))).collect();
    let mut cannot: HashMap<usize, Vec<usize>> = HashMap::new();
    for (a, b) in &state.different {
        if let (Some(&i), Some(&j)) = (local.get(a), local.get(b)) {
            cannot.entry(i).or_default().push(j);
            cannot.entry(j).or_default().push(i);
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| quality[*b].total_cmp(&quality[*a]));

    // leader pass, in blocks: the nearest old centre is found in parallel, new centres made inside
    // the block are checked one by one
    let mut label = vec![usize::MAX; n];
    let mut cent: Vec<f32> = Vec::new();
    let mut sums: Vec<f32> = Vec::new();
    let violates = |label: &[usize], i: usize, c: usize| cannot.get(&i).is_some_and(|js| js.iter().any(|j| label[*j] == c));
    for block in order.chunks(256) {
        let old = cent.len() / DIM;
        let best: Vec<(usize, f32)> = block
            .par_iter()
            .map(|&i| {
                let v = row(i);
                cent.as_chunks::<DIM>()
                    .0
                    .iter()
                    .enumerate()
                    .map(|(c, ce)| (c, dot(v, ce)))
                    .fold((usize::MAX, f32::MIN), |b, e| if e.1 > b.1 { e } else { b })
            })
            .collect();
        let mut touched = std::collections::BTreeSet::new();
        for (b, &i) in block.iter().enumerate() {
            let v = row(i);
            let (mut bc, mut bs) = best[b];
            if bc != usize::MAX && violates(&label, i, bc) {
                // the nearest centre holds a face the user said differs: the next one that does not
                (bc, bs) = cent
                    .as_chunks::<DIM>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(c, _)| !violates(&label, i, *c))
                    .map(|(c, ce)| (c, dot(v, ce)))
                    .fold((usize::MAX, f32::MIN), |b, e| if e.1 > b.1 { e } else { b });
            }
            for c in old..cent.len() / DIM {
                let sc = dot(v, &cent[c * DIM..(c + 1) * DIM]);
                if sc > bs && !violates(&label, i, c) {
                    (bc, bs) = (c, sc);
                }
            }
            if bc != usize::MAX && bs >= link {
                label[i] = bc;
                add_to(&mut sums[bc * DIM..(bc + 1) * DIM], v);
                touched.insert(bc);
                if bc >= old {
                    let u = unit(&sums[bc * DIM..(bc + 1) * DIM]);
                    cent[bc * DIM..(bc + 1) * DIM].copy_from_slice(&u);
                }
            } else {
                label[i] = cent.len() / DIM;
                cent.extend_from_slice(v);
                sums.extend_from_slice(v);
            }
        }
        for c in touched.into_iter().filter(|c| *c < old) {
            let u = unit(&sums[c * DIM..(c + 1) * DIM]);
            cent[c * DIM..(c + 1) * DIM].copy_from_slice(&u);
        }
    }

    // refinement: every face to its nearest centre, then centres from the members
    for _ in 0..2 {
        let nc = cent.len() / DIM;
        let moves: Vec<(usize, f32)> = (0..n)
            .into_par_iter()
            .map(|i| {
                cent.as_chunks::<DIM>()
                    .0
                    .iter()
                    .enumerate()
                    .map(|(c, ce)| (c, dot(row(i), ce)))
                    .fold((usize::MAX, f32::MIN), |b, e| if e.1 > b.1 { e } else { b })
            })
            .collect();
        let mut next = nc;
        let mut new_label = label.clone();
        for i in 0..n {
            let (c, sc) = moves[i];
            let own = dot(row(i), &cent[label[i] * DIM..(label[i] + 1) * DIM]);
            if c != usize::MAX && sc >= link - 0.02 && !violates(&new_label, i, c) {
                new_label[i] = c;
            } else if own < link - 0.08 {
                // far from every centre: a group of its own
                new_label[i] = next;
                next += 1;
            }
        }
        label = new_label;
        // compact the labels and recompute the centres
        let mut remap: HashMap<usize, usize> = HashMap::new();
        for l in label.iter_mut() {
            let k = remap.len();
            *l = *remap.entry(*l).or_insert(k);
        }
        let nc = remap.len();
        sums = vec![0.0; nc * DIM];
        for i in 0..n {
            add_to(&mut sums[label[i] * DIM..(label[i] + 1) * DIM], row(i));
        }
        cent = sums.as_chunks::<DIM>().0.iter().flat_map(|c| unit(c)).collect();
    }

    // merge groups whose centres are alike, nearest pair first
    let nc = cent.len() / DIM;
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); nc];
    for i in 0..n {
        members[label[i]].push(i);
    }
    let mut alive = vec![true; nc];
    let mut version = vec![0u32; nc];
    #[derive(PartialEq)]
    struct Pair(f32, usize, usize, u32, u32);
    impl Eq for Pair {}
    impl PartialOrd for Pair {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Pair {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&o.0)
        }
    }
    let merge_at = link;
    let mut heap: BinaryHeap<Pair> = (0..nc)
        .into_par_iter()
        .flat_map_iter(|a| {
            let (cent, link) = (&cent, merge_at);
            (a + 1..nc).filter_map(move |b| {
                let sc = dot(&cent[a * DIM..(a + 1) * DIM], &cent[b * DIM..(b + 1) * DIM]);
                (sc >= link).then_some(Pair(sc, a, b, 0, 0))
            })
        })
        .collect::<Vec<_>>()
        .into();
    let separated = |members: &[Vec<usize>], label: &[usize], a: usize, b: usize| -> bool {
        members[a].iter().any(|i| cannot.get(i).is_some_and(|js| js.iter().any(|j| label[*j] == b)))
    };
    while let Some(Pair(_, a, b, va, vb)) = heap.pop() {
        if !alive[a] || !alive[b] || version[a] != va || version[b] != vb {
            continue;
        }
        if !cannot.is_empty() && separated(&members, &label, a, b) {
            continue;
        }
        // b joins a
        let moved = std::mem::take(&mut members[b]);
        for &i in &moved {
            label[i] = a;
            add_to(&mut sums[a * DIM..(a + 1) * DIM], row(i));
        }
        members[a].extend(moved);
        alive[b] = false;
        version[a] += 1;
        let u = unit(&sums[a * DIM..(a + 1) * DIM]);
        cent[a * DIM..(a + 1) * DIM].copy_from_slice(&u);
        for c in 0..nc {
            if c != a && alive[c] {
                let sc = dot(&u, &cent[c * DIM..(c + 1) * DIM]);
                if sc >= merge_at {
                    let (lo, hi) = if a < c { (a, c) } else { (c, a) };
                    heap.push(Pair(sc, lo, hi, version[lo], version[hi]));
                }
            }
        }
    }

    let mut groups: Vec<Group> = (0..nc)
        .filter(|c| alive[*c])
        .map(|c| {
            let mut m = std::mem::take(&mut members[c]);
            m.sort_by(|a, b| quality[*b].total_cmp(&quality[*a]));
            Group { centroid: cent[c * DIM..(c + 1) * DIM].to_vec(), members: m.into_iter().map(|i| slots[i]).collect() }
        })
        .filter(|g| !g.members.is_empty())
        .collect();
    groups.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then(a.members[0].cmp(&b.members[0])));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FaceMeta;

    /// Deterministic pseudo-random unit vector.
    fn rnd(seed: u64) -> Vec<f32> {
        let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let v: Vec<f32> = (0..DIM)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect();
        unit(&v)
    }

    /// `people` identities, `each` noisy faces per identity (noise `amp`).
    fn world(people: usize, each: usize, amp: f32) -> (FaceStore, Vec<usize>) {
        let mut store = FaceStore::open(None, "t").unwrap();
        let mut truth = Vec::new();
        let mut photos = Vec::new();
        for p in 0..people {
            let id = rnd(1000 + p as u64);
            for f in 0..each {
                let noise = rnd(50_000 + (p * 100 + f) as u64);
                let v: Vec<f32> = id.iter().zip(&noise).map(|(a, b)| a + amp * b).collect();
                let key = (p * each + f) as u64 + 1;
                photos.push((key, vec![(FaceMeta { key, idx: 0, rect: [0.0; 4], det: 0.95, size: 100.0, turn: 0.0 }, unit(&v))]));
                truth.push(p);
            }
        }
        store.add(photos).unwrap();
        (store, truth)
    }

    #[test]
    fn faces_of_one_identity_end_up_together_and_identities_apart() {
        let (store, truth) = world(12, 15, 0.6);
        let state = State::default();
        let g = group(&store, &state);
        assert_eq!(g.groups.len(), 12, "{:?}", g.groups.iter().map(|x| x.members.len()).collect::<Vec<_>>());
        for grp in &g.groups {
            let ids: std::collections::HashSet<usize> = grp.members.iter().map(|s| truth[store.meta(*s).unwrap().key as usize - 1]).collect();
            assert_eq!(ids.len(), 1);
            assert_eq!(grp.members.len(), 15);
        }
    }

    #[test]
    fn a_pair_the_user_separated_is_never_grouped_and_known_people_are_suggested() {
        let (store, truth) = world(3, 10, 0.5);
        let mut state = State::default();
        // two faces of identity 0 declared different
        let a = FaceRef(1, 0);
        let b = FaceRef(2, 0);
        state.different.push((a, b));
        let g = group(&store, &state);
        for grp in &g.groups {
            let has = |f: FaceRef| grp.members.iter().any(|s| store.meta(*s).unwrap().face() == f);
            assert!(!(has(a) && has(b)));
        }
        // confirming five faces of identity 1 as a person: the other five become suggestions
        let mut state = State::default();
        let id = state.new_person(Some("Bao".into()));
        let faces: Vec<FaceRef> = (0..10).filter(|i| truth[*i] == 0).take(5).map(|i| FaceRef(i as u64 + 1, 0)).collect();
        state.assign(id, &faces);
        let g = group(&store, &state);
        assert_eq!(g.suggestions.len(), 5);
        assert!(g.suggestions.iter().all(|sg| sg.person == id && truth[store.meta(sg.slot).unwrap().key as usize - 1] == 0));
        assert_eq!(g.groups.len(), 2);
    }
}
