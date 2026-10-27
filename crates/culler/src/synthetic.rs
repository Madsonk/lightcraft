//! A synthetic album with planted problems and known answers, for tests and benchmarks (no
//! images: the measurements the engine would take are generated directly, deterministically).

use crate::{Face, SIGNATURE_LEN, Shot};

/// What was planted in a shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plant {
    Good,
    ClosedEyes,
    Blurred,
    FaceBlurred,
    Dark,
    /// A frame of a burst that is not the best one.
    BurstWorse,
    /// The best frame of a burst.
    BurstBest,
}

impl Plant {
    /// Must the cull reject it (with the default genre)?
    pub fn must_reject(self) -> bool {
        matches!(self, Plant::ClosedEyes | Plant::Blurred | Plant::Dark | Plant::BurstWorse)
    }
}

pub struct Album {
    pub shots: Vec<Shot>,
    pub plants: Vec<Plant>,
    /// The scene each shot was planted in.
    pub scene_of: Vec<u32>,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn unit(&mut self) -> f32 {
        (self.next() % 1_000_000) as f32 / 1_000_000.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

fn unit_vec(rng: &mut Rng) -> Vec<f32> {
    let mut v: Vec<f32> = (0..SIGNATURE_LEN).map(|_| rng.range(-1.0, 1.0)).collect();
    normalize(&mut v);
    v
}

fn normalize(v: &mut [f32]) {
    let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
    v.iter_mut().for_each(|x| *x -= mean);
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter_mut().for_each(|x| *x /= norm);
}

fn faces(rng: &mut Rng, open: f32, sharp: f32, count: usize) -> Vec<Face> {
    (0..count)
        .map(|k| {
            let x = 0.2 + 0.35 * k as f32;
            Face {
                rect: [x, 0.2, x + 0.18, 0.5],
                open: [open + rng.range(-0.01, 0.01), open + rng.range(-0.01, 0.01)],
                sharp: [sharp + rng.range(-3.0, 3.0), sharp + rng.range(-3.0, 3.0)],
                judged: true,
                person: Some(if k == 0 { "Alice".into() } else { "Bob".into() }),
            }
        })
        .collect()
}

/// `scenes` scenes of `moments` moments each; roughly a quarter of the moments are bursts of
/// three frames. Same `seed`, same album.
pub fn album(scenes: usize, moments: usize, seed: u64) -> Album {
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let (mut shots, mut plants, mut scene_of) = (Vec::new(), Vec::new(), Vec::new());
    let mut id = 0u64;
    for scene in 0..scenes {
        // scenes are an hour apart; moments 40–90 s apart
        let mut t = 1_000_000 + scene as i64 * 3600;
        for _ in 0..moments {
            t += (40.0 + 50.0 * rng.unit()) as i64;
            let base = unit_vec(&mut rng);
            let roll = rng.unit();
            let mut push = |shots: &mut Vec<Shot>,
                            plant: Plant,
                            dt: i64,
                            sig: Vec<f32>,
                            sharp: f32,
                            face_open: f32,
                            face_sharp: f32,
                            mean: f32,
                            rng: &mut Rng| {
                id += 1;
                let count = 1 + (rng.next() % 2) as usize;
                shots.push(Shot {
                    id,
                    aspect: 1.5,
                    time: Some(t + dt),
                    sharpness: sharp,
                    clipped: rng.range(0.0, 0.04),
                    exposure: [mean, (mean - 0.35).max(0.01), if mean < 0.1 { mean * 4.0 } else { (mean + 0.4).min(0.99) }],
                    color: Vec::new(),
                    signature: sig,
                    faces: Some(faces(rng, face_open, face_sharp, count)),
                    taste: None,
                });
                plants.push(plant);
                scene_of.push(scene as u32);
            };
            if roll < 0.25 {
                // burst: three frames two seconds apart, the middle one the sharpest
                for (k, (sharp, plant)) in [(58.0, Plant::BurstWorse), (74.0, Plant::BurstBest), (62.0, Plant::BurstWorse)].into_iter().enumerate() {
                    let mut sig = base.clone();
                    for x in sig.iter_mut() {
                        *x += rng.range(-0.03, 0.03);
                    }
                    normalize(&mut sig);
                    let fs = sharp + 4.0;
                    push(&mut shots, plant, 2 * k as i64, sig, sharp, 0.30, fs, 0.45, &mut rng);
                }
                t += 6;
            } else {
                let (plant, sharp, fo, fs, mean) = if roll < 0.33 {
                    (Plant::ClosedEyes, rng.range(55.0, 80.0), 0.07, 70.0, 0.45)
                } else if roll < 0.40 {
                    (Plant::Blurred, rng.range(2.0, 9.0), 0.30, 8.0, 0.45)
                } else if roll < 0.45 {
                    (Plant::FaceBlurred, rng.range(55.0, 75.0), 0.30, 6.0, 0.45)
                } else if roll < 0.50 {
                    (Plant::Dark, rng.range(50.0, 70.0), 0.30, 60.0, 0.04)
                } else {
                    (Plant::Good, rng.range(55.0, 85.0), 0.30, rng.range(60.0, 90.0), rng.range(0.35, 0.55))
                };
                push(&mut shots, plant, 0, base, sharp, fo, fs, mean, &mut rng);
            }
        }
    }
    Album { shots, plants, scene_of }
}
