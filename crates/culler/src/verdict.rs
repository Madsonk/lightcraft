//! The verdict of a photo: bucket, reasons, score, proposed stars.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::profile::{EyePolicy, Options};
use crate::{Face, Shot, VERSION, dupes, scene};

/// Eye aspect ratio below which an eye is closed / half closed (calibrated in `docs/faces.md`).
const CLOSED_BELOW: f32 = 0.15;
/// Eye classes (eye aspect ratio). Calibrated on real shoots: smiling and squinting in the sun put one in
/// four faces under 0.16, so only *both* eyes under 0.12 are closed and both under 0.17 narrowed;
/// a single eye under 0.10 is a wink.
const BOTH_CLOSED: f32 = 0.12;
const BOTH_NARROW: f32 = 0.17;
const WINK: f32 = 0.10;
const OPEN_FROM: f32 = 0.24;
/// A trustworthy album median focus.
const MEDIAN_MIN: f32 = 30.0;
/// Lowest score a photo needs to become a highlight.
const HIGHLIGHT_MIN: f32 = 55.0;
/// A face is judged when its size is at least this share of the photo's largest face.
const PROMINENT: f32 = 0.5;
/// Faces narrower than this share of the long edge are never judged: on a real wedding portfolio
/// the eye measurements of smaller faces were noise (see docs/culler.md, trial).
const MIN_JUDGED: f32 = 0.04;

/// Where the photo ends up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Bucket {
    /// The best of the shoot.
    Highlight,
    /// Good to keep.
    Selected,
    /// Something to look at before deciding.
    Review,
    /// Technically failed or a worse duplicate.
    Rejected,
}

impl Bucket {
    pub fn name(self) -> &'static str {
        match self {
            Bucket::Highlight => "highlight",
            Bucket::Selected => "selected",
            Bucket::Review => "review",
            Bucket::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Does not count against the photo.
    Info,
    /// Sends the photo to review.
    Warning,
    /// Rejects the photo.
    Reject,
}

/// Why a photo is where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    ClosedEyes,
    HalfClosedEyes,
    Blurred,
    Soft,
    FaceBlurred,
    FaceSoft,
    Duplicate,
    Underexposed,
    Overexposed,
    Clipped,
    NoKeyFace,
    BestOfGroup,
    /// No face analysis for this photo: eyes and face focus were not judged.
    FacesNotAnalyzed,
}

impl Reason {
    /// The Aftershoot-like flag name (`closed_eyes`, `blurred`, `duplicated`, `warning`…).
    pub fn flag(self) -> &'static str {
        match self {
            Reason::ClosedEyes | Reason::HalfClosedEyes => "closed_eyes",
            Reason::Blurred | Reason::Soft | Reason::FaceBlurred | Reason::FaceSoft => "blurred",
            Reason::Duplicate | Reason::BestOfGroup => "duplicated",
            Reason::Underexposed | Reason::Overexposed | Reason::Clipped | Reason::NoKeyFace | Reason::FacesNotAnalyzed => "warning",
        }
    }

    /// One plain sentence for `cull.explain`.
    pub fn text(self) -> &'static str {
        match self {
            Reason::ClosedEyes => "a person has closed eyes",
            Reason::HalfClosedEyes => "a person is blinking or squinting",
            Reason::Blurred => "the whole photo is out of focus",
            Reason::Soft => "the photo is a little soft",
            Reason::FaceBlurred => "a face is out of focus",
            Reason::FaceSoft => "a face is a little soft",
            Reason::Duplicate => "a better shot of the same moment exists",
            Reason::Underexposed => "the photo is too dark",
            Reason::Overexposed => "the photo is too bright",
            Reason::Clipped => "a large part of the photo is clipped to black or white",
            Reason::NoKeyFace => "none of the key people is in the photo",
            Reason::BestOfGroup => "the best shot of its group of look-alikes",
            Reason::FacesNotAnalyzed => "faces were not analyzed: eyes and face focus were not judged",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EyeClass {
    Open,
    Half,
    Closed,
    /// Too small or turned away.
    NotJudged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlurClass {
    Sharp,
    Soft,
    Blurred,
    NotJudged,
}

/// The verdict on one face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceVerdict {
    pub rect: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    pub eyes: EyeClass,
    pub blur: BlurClass,
    /// Whether the face counts for the cull (all faces, or the key people's when some are set).
    pub counted: bool,
    /// 0..100: both eyes open and sharp.
    pub quality: f32,
}

/// The verdict of one photo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    /// Version of the rules ([`VERSION`]).
    pub version: u32,
    pub genre: crate::Genre,
    pub bucket: Bucket,
    /// Proposed rating 1..5 (never written without `cull.apply`).
    pub stars: u8,
    /// Overall quality 0..100.
    pub score: f32,
    pub reasons: Vec<Reason>,
    /// Shooting scene (chronological, from 0).
    pub scene: u32,
    /// Duplicate group, when the photo has look-alikes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
    #[serde(default)]
    pub group_size: u32,
    /// 0 = the best of its group.
    #[serde(default)]
    pub group_rank: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<FaceVerdict>,
    /// The measurements the verdict rests on.
    pub sharpness: f32,
    pub clipped: f32,
    pub exposure: [f32; 3],
    /// What the measurements were taken from (the engine's freshness key); empty when unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub basis: String,
}

impl Verdict {
    pub fn severity_of(reason: Reason, profile_dup_reject: bool) -> Severity {
        match reason {
            Reason::ClosedEyes | Reason::Blurred | Reason::Underexposed | Reason::Overexposed => Severity::Reject,
            Reason::Duplicate if profile_dup_reject => Severity::Reject,
            Reason::HalfClosedEyes
            | Reason::Soft
            | Reason::FaceSoft
            | Reason::FaceBlurred
            | Reason::Clipped
            | Reason::NoKeyFace
            | Reason::Duplicate => Severity::Warning,
            Reason::BestOfGroup | Reason::FacesNotAnalyzed => Severity::Info,
        }
    }

    /// The Aftershoot-like flags of the photo (`selected`, `highlighted`, `closed_eyes`,
    /// `blurred`, `duplicated`, `warning`), in that order, without repeats.
    pub fn flags(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        match self.bucket {
            Bucket::Highlight => out.extend(["selected", "highlighted"]),
            Bucket::Selected => out.push("selected"),
            Bucket::Review => out.push("warning"),
            Bucket::Rejected => {}
        }
        for r in &self.reasons {
            let f = r.flag();
            if !out.contains(&f) && !matches!(r, Reason::BestOfGroup | Reason::FacesNotAnalyzed) {
                out.push(f);
            }
        }
        out
    }
}

fn eye_class(face: &Face) -> EyeClass {
    if !face.judged {
        return EyeClass::NotJudged;
    }
    let (lo, hi) = (face.open[0].min(face.open[1]), face.open[0].max(face.open[1]));
    if !lo.is_finite() || !hi.is_finite() {
        EyeClass::NotJudged
    } else if hi < BOTH_CLOSED {
        EyeClass::Closed
    } else if hi < BOTH_NARROW || lo < WINK {
        EyeClass::Half
    } else {
        EyeClass::Open
    }
}

fn open_score(ear: f32) -> f32 {
    if ear < CLOSED_BELOW {
        (50.0 * ear / CLOSED_BELOW).max(0.0)
    } else {
        (50.0 + 50.0 * (ear - CLOSED_BELOW) / (OPEN_FROM - CLOSED_BELOW)).min(100.0)
    }
}

fn face_quality(face: &Face) -> f32 {
    let sharp = face.sharp[0].min(face.sharp[1]);
    let open = open_score(face.open[0].min(face.open[1]));
    let q = 0.7 * open + 0.3 * sharp;
    if q.is_finite() { q.clamp(0.0, 100.0) } else { 0.0 }
}

fn is_key(face: &Face, key: &[String]) -> bool {
    key.is_empty() || face.person.as_deref().is_some_and(|p| key.iter().any(|k| k.eq_ignore_ascii_case(p)))
}

fn median(v: &mut Vec<f32>) -> f32 {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

struct Draft {
    reasons: Vec<Reason>,
    faces: Vec<FaceVerdict>,
    score: f32,
}

fn draft(shot: &Shot, opts: &Options, album_median: f32) -> Draft {
    // face boxes are fractions of each axis: scale to a share of the long edge
    let aspect = if shot.aspect.is_finite() && shot.aspect > 0.0 { shot.aspect } else { 1.0 };
    let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
    let p = &opts.profile;
    let mut reasons: Vec<Reason> = Vec::new();
    let sharp = if shot.sharpness.is_finite() { shot.sharpness.clamp(0.0, 100.0) } else { 0.0 };
    // global focus
    let relative = album_median >= MEDIAN_MIN && sharp < p.relative_blur * album_median;
    // a whole shoot that is soft (low light, shallow depth of field on a small subject) must not be rejected
    // by absolute numbers made for crisp ones: the thresholds never exceed a share of the album's median
    let (reject_at, warn_at) = if album_median > 0.0 { (p.blur_reject.min(0.4 * album_median), p.blur_warn.min(0.8 * album_median)) } else { (p.blur_reject, p.blur_warn) };
    if sharp < reject_at || relative {
        reasons.push(Reason::Blurred);
    } else if sharp < warn_at {
        reasons.push(Reason::Soft);
    }
    // exposure
    let mean = shot.exposure[0];
    let clipped = if shot.clipped.is_finite() { shot.clipped.clamp(0.0, 1.0) } else { 0.0 };
    if (mean < p.dark_below && shot.exposure[2] < 0.35) || (clipped > p.clipped_reject && mean < 0.3) {
        reasons.push(Reason::Underexposed);
    } else if (mean > p.bright_above && shot.exposure[1] > 0.5) || (clipped > p.clipped_reject && mean > 0.7) {
        reasons.push(Reason::Overexposed);
    } else if clipped > p.clipped_warn {
        reasons.push(Reason::Clipped);
    }
    // faces
    let mut faces = Vec::new();
    let mut counted_quality = Vec::new();
    let mut counted_any = false;
    match &shot.faces {
        None => reasons.push(Reason::FacesNotAnalyzed),
        Some(list) => {
            // the people the photo is about: the largest faces. Smaller ones (guests, background,
            // out of the focal plane) are shown in `faces` but never judged.
            let side = |f: &Face| ((f.rect[2] - f.rect[0]) * kx).max((f.rect[3] - f.rect[1]) * ky);
            let largest = list.iter().filter(|f| is_key(f, &opts.key_people)).map(side).filter(|v| v.is_finite()).fold(0.0f32, f32::max);
            for f in list {
                let counted = is_key(f, &opts.key_people) && side(f) >= PROMINENT * largest && side(f) >= MIN_JUDGED;
                let eyes = eye_class(f);
                let size = side(f);
                let eye_sharp = f.sharp[0].min(f.sharp[1]);
                let blur = if eyes == EyeClass::NotJudged || !eye_sharp.is_finite() || size < p.face_blur_min_size {
                    BlurClass::NotJudged
                } else if eye_sharp < p.face_blur_reject {
                    BlurClass::Blurred
                } else if eye_sharp < p.face_blur_warn {
                    BlurClass::Soft
                } else {
                    BlurClass::Sharp
                };
                let quality = face_quality(f);
                if counted {
                    counted_any = true;
                    if p.eyes != EyePolicy::Ignore {
                        match eyes {
                            // a wink, or one eye lost in shadow, is for the photographer to look at;
                            // only both eyes closed rejects
                            EyeClass::Closed => reasons.push(if p.eyes == EyePolicy::Reject { Reason::ClosedEyes } else { Reason::HalfClosedEyes }),
                            EyeClass::Half => reasons.push(Reason::HalfClosedEyes),
                            _ => {}
                        }
                    }
                    match blur {
                        BlurClass::Blurred => reasons.push(Reason::FaceBlurred),
                        BlurClass::Soft => reasons.push(Reason::FaceSoft),
                        _ => {}
                    }
                    if eyes != EyeClass::NotJudged {
                        counted_quality.push(quality);
                    }
                }
                faces.push(FaceVerdict { rect: f.rect, person: f.person.clone(), eyes, blur, counted, quality });
            }
            if !opts.key_people.is_empty() && !counted_any {
                reasons.push(Reason::NoKeyFace);
            }
        }
    }
    let mut seen = Vec::new();
    reasons.retain(|r| {
        let new = !seen.contains(r);
        seen.push(*r);
        new
    });
    // score
    let face_q = (!counted_quality.is_empty()).then(|| counted_quality.iter().sum::<f32>() / counted_quality.len() as f32);
    let mut score = match face_q {
        Some(q) => (1.0 - p.face_weight) * sharp + p.face_weight * q,
        None => sharp,
    };
    score -= 50.0 * clipped;
    if mean.is_finite() {
        score -= 25.0 * (mean - 0.45).abs();
    }
    if reasons.contains(&Reason::ClosedEyes) {
        score -= 40.0;
    }
    if let Some(t) = shot.taste.filter(|t| t.is_finite()) {
        score = 0.85 * score + 0.15 * t.clamp(0.0, 100.0);
    }
    Draft { reasons, faces, score: score.clamp(0.0, 100.0) }
}

/// Judge a whole album. `shots` in any order; the verdicts come back in the same order.
pub fn judge(shots: &[Shot], opts: &Options) -> Vec<Verdict> {
    let n = shots.len();
    let scenes = scene::segment(shots, &opts.profile);
    let groups = dupes::group(shots, &scenes, opts.dup_threshold(), opts.dup_color_threshold(), opts.profile.dup_gap_secs);
    let album_median = median(&mut shots.iter().map(|s| s.sharpness).collect());
    let mut drafts: Vec<Draft> = shots.iter().map(|s| draft(s, opts, album_median)).collect();

    // rank inside each duplicate group: best score first, then the earliest, then input order
    let mut members: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, g) in groups.iter().enumerate() {
        if let Some(g) = g {
            members.entry(*g).or_default().push(i);
        }
    }
    let mut rank = vec![0u32; n];
    let mut size = vec![0u32; n];
    for list in members.values_mut() {
        let score = |i: usize| drafts.get(i).map_or(0.0, |d| d.score);
        let time = |i: usize| shots.get(i).and_then(|s| s.time).unwrap_or(i64::MAX);
        // a shot with a rejecting problem never beats one without
        let bad = |i: usize| drafts.get(i).is_some_and(|d| d.reasons.iter().any(|r| Verdict::severity_of(*r, false) == Severity::Reject));
        list.sort_by(|&a, &b| bad(a).cmp(&bad(b)).then(score(b).total_cmp(&score(a))).then(time(a).cmp(&time(b))).then(a.cmp(&b)));
        let len = list.len() as u32;
        for (r, &i) in list.iter().enumerate() {
            if let (Some(rk), Some(sz)) = (rank.get_mut(i), size.get_mut(i)) {
                *rk = r as u32;
                *sz = len;
            }
        }
    }
    for (i, d) in drafts.iter_mut().enumerate() {
        if groups.get(i).copied().flatten().is_some() {
            d.reasons.push(if rank.get(i) == Some(&0) { Reason::BestOfGroup } else { Reason::Duplicate });
        }
    }

    // buckets
    let dup_reject = opts.profile.dup_reject;
    let mut buckets: Vec<Bucket> = drafts
        .iter()
        .map(|d| {
            let worst = d.reasons.iter().map(|r| Verdict::severity_of(*r, dup_reject)).max_by_key(|s| *s as u8);
            match worst {
                Some(Severity::Reject) => Bucket::Rejected,
                Some(Severity::Warning) => Bucket::Review,
                _ => Bucket::Selected,
            }
        })
        .collect();
    // highlights: the top share of the selected photos, above an absolute floor
    // a shoot that is soft or dark as a whole still has its best frames: the floor never exceeds the album's median score
    let highlight_floor = HIGHLIGHT_MIN.min(median(&mut drafts.iter().map(|d| d.score).collect()));
    let mut selected: Vec<f32> = buckets.iter().zip(&drafts).filter(|(b, _)| **b == Bucket::Selected).map(|(_, d)| d.score).collect();
    selected.sort_by(|a, b| b.total_cmp(a));
    let keep = ((selected.len() as f32 * opts.profile.highlight_share).ceil() as usize).min(selected.len());
    if let Some(&cut) = keep.checked_sub(1).and_then(|k| selected.get(k)) {
        let cut = cut.max(highlight_floor);
        for (b, d) in buckets.iter_mut().zip(&drafts) {
            if *b == Bucket::Selected && d.score >= cut {
                *b = Bucket::Highlight;
            }
        }
    }

    drafts
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            let bucket = buckets.get(i).copied().unwrap_or(Bucket::Review);
            let stars = match bucket {
                Bucket::Highlight => 5,
                Bucket::Selected if d.score >= 65.0 => 4,
                Bucket::Selected => 3,
                Bucket::Review => 2,
                Bucket::Rejected => 1,
            };
            let shot = shots.get(i);
            Verdict {
                version: VERSION,
                genre: opts.profile.genre,
                bucket,
                stars,
                score: (d.score * 10.0).round() / 10.0,
                reasons: d.reasons,
                scene: scenes.get(i).copied().unwrap_or(0),
                group: groups.get(i).copied().flatten(),
                group_size: size.get(i).copied().unwrap_or(0),
                group_rank: rank.get(i).copied().unwrap_or(0),
                faces: d.faces,
                sharpness: shot.map_or(0.0, |s| s.sharpness),
                clipped: shot.map_or(0.0, |s| s.clipped),
                exposure: shot.map_or([0.0; 3], |s| s.exposure),
                basis: String::new(),
            }
        })
        .collect()
}
