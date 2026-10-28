//! Honest scores for the search by meaning (idea of the SCM study, written here for French and
//! English, measured with `examples/describe_bench.rs`).
//!
//! SigLIP's text vectors all lean the same way (anisotropy): some pictures are close to *every*
//! sentence. Two corrections, both optional on `describe.search`:
//! - **centring** — the query vector loses the mean of neutral captions (French and English, built
//!   from the glossary) before it is compared; with `gap` the mean of the library's pictures is
//!   added back, which moves the query into the pictures' cone;
//! - **a floor** — a handful of nonsense queries is asked to the same library; the best cosine any
//!   of them reaches is the *noise ceiling*, and a meaning match must beat it. A query that beats
//!   nothing finds nothing instead of the 20 least-bad pictures.

/// Queries that describe nothing a library of people and places shows.
pub const NOISE: &[&str] = &[
    "xkqzv wprtl mnbvc",
    "zzzzzzzz",
    "lorem ipsum dolor sit amet",
    "0 1 0 1 1 0 0 1",
    "a blank white page",
    "a black screen",
    "a screenshot of source code",
    "a technical drawing of a machine part",
    "un document texte en noir sur blanc",
    "un tableau de chiffres",
    "un écran noir",
    "un schéma électrique",
    "a microscope slide",
    "a map of the stars",
];

/// How the query vector is moved before the comparison.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Centre {
    /// As the model gives it.
    None,
    /// Minus the mean of neutral captions.
    Text,
    /// Minus the mean of neutral captions, plus the mean of the pictures.
    Gap,
}

impl Centre {
    pub fn parse(s: &str) -> Option<Centre> {
        match s {
            "none" => Some(Centre::None),
            "text" => Some(Centre::Text),
            "gap" => Some(Centre::Gap),
            _ => None,
        }
    }
}

/// Neutral captions in both languages: generic sentences and every word of the glossary.
pub fn neutral_captions() -> Vec<String> {
    let mut v: Vec<String> = ["une photo", "une image", "une vidéo", "un plan", "a photo", "a picture", "a video frame", "a shot"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for (fr, en) in super::glossary::pairs() {
        v.push(format!("une photo de {fr}"));
        v.push(format!("a photo of {en}"));
    }
    v
}

/// The mean of `vs`, as a vector (not normalized).
pub fn mean(vs: &[&[f32]]) -> Vec<f32> {
    let Some(first) = vs.first() else { return Vec::new() };
    let mut m = vec![0.0f32; first.len()];
    for v in vs {
        for (a, b) in m.iter_mut().zip(*v) {
            *a += b;
        }
    }
    let n = vs.len() as f32;
    m.iter_mut().for_each(|x| *x /= n);
    m
}

/// `q` moved as `centre` says, back to unit length.
pub fn moved(q: &[f32], centre: Centre, text_mean: &[f32], image_mean: &[f32]) -> Vec<f32> {
    let mut v = q.to_vec();
    if centre != Centre::None && text_mean.len() == v.len() {
        v.iter_mut().zip(text_mean).for_each(|(a, b)| *a -= b);
    }
    if centre == Centre::Gap && image_mean.len() == v.len() {
        v.iter_mut().zip(image_mean).for_each(|(a, b)| *a += b);
    }
    unit(&mut v);
    v
}

pub fn unit(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter_mut().for_each(|x| *x /= n);
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The noise ceiling × `factor`: the best cosine each noise query reaches, the highest of them.
pub fn floor(noise_tops: &[f32], factor: f32) -> Option<f32> {
    noise_tops.iter().copied().reduce(f32::max).map(|m| m * factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centring_removes_the_shared_direction() {
        let tm = [1.0, 0.0];
        let q = [1.0, 1.0];
        let v = moved(&q, Centre::Text, &tm, &[]);
        assert!((v[0]).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6, "{v:?}");
        let g = moved(&q, Centre::Gap, &tm, &[0.0, -1.0]);
        assert!((dot(&g, &g) - 1.0).abs() < 1e-5);
        assert_eq!(moved(&[3.0, 4.0], Centre::None, &tm, &[]), vec![0.6, 0.8]);
    }

    #[test]
    fn the_floor_is_the_highest_noise() {
        assert_eq!(floor(&[0.1, 0.3, 0.2], 1.0), Some(0.3));
        assert_eq!(floor(&[0.1, 0.3], 2.0), Some(0.6));
        assert_eq!(floor(&[], 1.0), None);
    }

    #[test]
    fn the_neutral_captions_cover_both_languages() {
        let n = neutral_captions();
        assert!(n.iter().any(|s| s.starts_with("une photo de ")) && n.iter().any(|s| s.starts_with("a photo of ")));
        assert!(n.len() > 100);
    }
}
