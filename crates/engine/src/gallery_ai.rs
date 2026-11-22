//! Local "AI" that lays out a gallery before it is published: it drops weak photos, groups the rest
//! into chapters, orders them, and picks a cover. It only reads numbers LightCraft has already
//! computed (sharpness, cull verdict, retouch score, bursts, faces, capture time, scene text): no
//! photo leaves the machine, no network call, no model. Same input, same output: every tie is
//! broken by the input order or the photo id.
//!
//! `captured` is "seconds since an epoch" parsed from the naive local capture time; the time of day
//! is read as `captured mod 86400`, so the caller must parse the ISO string as if it were UTC.

use std::collections::{BTreeMap, HashSet};

use lightcraft_catalog::PhotoId;

/// Photos below this sharpness (0..100) are "very blurry".
const BLURRY: f32 = 15.0;
/// A face counts as sharp when the photo's sharpness reaches this (or is unknown).
const SHARP_FACE: f32 = 40.0;
/// At most this share of the gallery (percent) is ever dropped.
const MAX_DROP_PCT: usize = 40;
/// A time chapter with fewer photos is merged into its closest neighbour.
const MIN_CHAPTER: usize = 6;
/// A chapter opens with its best photo only when it has more photos than this.
const OPEN_ABOVE: usize = 12;
/// Share (percent) of a chapter that must carry the same scene for the scene to name it.
const SCENE_SHARE_PCT: usize = 60;
const SCENE_MAX_CHARS: usize = 40;
/// Landscape bonus on the 0..1 quality scale when `target_cover_landscape` is set.
const LANDSCAPE_BONUS: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Fr,
    En,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: PhotoId,
    /// Seconds since an epoch (naive capture time), `None` when unknown.
    pub captured: Option<i64>,
    /// 0..100.
    pub sharpness: Option<f32>,
    /// Overall cull-verdict score, 0..100 (`lightcraft_culler::Verdict::score`).
    pub verdict_score: Option<f32>,
    /// 0..100, 100 = nothing to fix.
    pub retouch_score: Option<f32>,
    /// Burst group (`Analysis.group`).
    pub burst: Option<u32>,
    /// `Analysis.best`.
    pub best_of_burst: bool,
    /// Detected faces; `None` = not analysed.
    pub faces: Option<u32>,
    /// Named people. Never copied into the plan's notes.
    pub people: Vec<String>,
    /// Scene description, when one exists.
    pub scene: Option<String>,
    pub width: u32,
    pub height: u32,
    pub rating: u8,
    pub flag_pick: bool,
    pub flag_reject: bool,
    /// Chapter the user already chose (album name).
    pub album: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub lang: Lang,
    /// Silence between two consecutive photos that starts a new chapter.
    pub gap_minutes: i64,
    /// Drop weak photos (blurry, burst duplicates). Rejected photos are dropped either way.
    pub drop_weak: bool,
    pub target_cover_landscape: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { lang: Lang::Fr, gap_minutes: 45, drop_weak: false, target_cover_landscape: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub name: String,
    /// In display order.
    pub ids: Vec<PhotoId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub id: PhotoId,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub chapters: Vec<Chapter>,
    pub cover: Option<PhotoId>,
    pub dropped: Vec<Dropped>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    Rejected,
    Blurry,
    Burst,
}

/// Chapter under construction; `name` is `None` until derived from scene / time of day.
struct Group {
    name: Option<String>,
    ids: Vec<usize>,
}

fn unit(v: Option<f32>) -> Option<f32> {
    v.filter(|x| x.is_finite()).map(|x| (x / 100.0).clamp(0.0, 1.0))
}

/// A pick or a 4+ star photo is the user's own choice: never dropped as blurry or as a duplicate.
fn protected(c: &Candidate) -> bool {
    c.flag_pick || c.rating >= 4
}

fn sharp_face(c: &Candidate) -> bool {
    c.faces.unwrap_or(0) > 0 && c.sharpness.is_none_or(|s| s.is_finite() && s >= SHARP_FACE)
}

/// Quality 0..1: weighted mean of the components that exist (missing data is skipped, never
/// penalised). Weights: verdict 3, sharpness 2, retouch 1, face sharpness 1.5 (only when a face is
/// detected and sharpness is known), star rating 1.5, pick 1 (as 1.0), reject 3 (as 0.0).
/// Nothing known at all gives a neutral 0.5.
pub fn quality(c: &Candidate) -> f32 {
    let sharp = unit(c.sharpness);
    let face = if c.faces.unwrap_or(0) > 0 { sharp } else { None };
    let stars = (c.rating > 0).then(|| f32::from(c.rating.min(5)) / 5.0);
    let parts = [
        (unit(c.verdict_score), 3.0),
        (sharp, 2.0),
        (unit(c.retouch_score), 1.0),
        (face, 1.5),
        (stars, 1.5),
        (c.flag_pick.then_some(1.0), 1.0),
        (c.flag_reject.then_some(0.0), 3.0),
    ];
    let (sum, weight) = parts.iter().filter_map(|&(v, w)| v.map(|v| (v * w, w))).fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    if weight > 0.0 { sum / weight } else { 0.5 }
}

fn opt_last(a: Option<i64>, b: Option<i64>) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Less,
        (None, Some(_)) => Greater,
        (None, None) => Equal,
    }
}

/// Burst duplicates: in each burst keep one photo (the flagged best, else the sharpest), drop the
/// others unless the user protected them. Photos already dropped do not take part.
fn mark_burst_duplicates(cs: &[Candidate], q: &[f32], why: &mut [Option<Why>]) {
    let mut groups: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, c) in cs.iter().enumerate() {
        if let (Some(g), None) = (c.burst, why[i]) {
            groups.entry(g).or_default().push(i);
        }
    }
    let sharp = |i: usize| cs[i].sharpness.filter(|s| s.is_finite()).unwrap_or(-1.0);
    for members in groups.values().filter(|m| m.len() > 1) {
        let keeper = members.iter().copied().max_by(|&a, &b| {
            cs[a].best_of_burst.cmp(&cs[b].best_of_burst).then(sharp(a).total_cmp(&sharp(b))).then(q[a].total_cmp(&q[b])).then(cs[b].id.cmp(&cs[a].id))
        });
        for &i in members {
            if Some(i) != keeper && !protected(&cs[i]) {
                why[i] = Some(Why::Burst);
            }
        }
    }
}

/// Decides what is dropped. Returns the reason per photo and whether the 40 % ceiling kicked in.
fn find_dropped(cs: &[Candidate], q: &[f32], drop_weak: bool) -> (Vec<Option<Why>>, bool) {
    let mut why: Vec<Option<Why>> = cs.iter().map(|c| c.flag_reject.then_some(Why::Rejected)).collect();
    if drop_weak {
        for (i, c) in cs.iter().enumerate() {
            if why[i].is_none() && !protected(c) && c.sharpness.is_some_and(|s| s.is_finite() && s < BLURRY) {
                why[i] = Some(Why::Blurry);
            }
        }
        mark_burst_duplicates(cs, q, &mut why);
    }
    // ceiling: past it, only the worst candidates for dropping stay dropped
    let cap = cs.len() * MAX_DROP_PCT / 100;
    let mut idx: Vec<usize> = (0..cs.len()).filter(|&i| why[i].is_some()).collect();
    let capped = idx.len() > cap;
    if capped {
        idx.sort_by(|&a, &b| q[a].total_cmp(&q[b]).then(cs[a].id.cmp(&cs[b].id)));
        for &i in &idx[cap..] {
            why[i] = None;
        }
    }
    (why, capped)
}

fn reason(why: Why, lang: Lang) -> String {
    match (why, lang) {
        (Why::Rejected, Lang::Fr) => "rejetée",
        (Why::Rejected, Lang::En) => "rejected",
        (Why::Blurry, Lang::Fr) => "floue",
        (Why::Blurry, Lang::En) => "blurry",
        (Why::Burst, Lang::Fr) => "rafale : une meilleure existe",
        (Why::Burst, Lang::En) => "burst: a better one exists",
    }
    .to_string()
}

fn others(lang: Lang) -> String {
    if lang == Lang::Fr { "Autres" } else { "Others" }.to_string()
}

/// One chapter per album, ordered by the album's first photo in time. A photo without album goes
/// to the album nearest in time (distance to the album's time span), or to "Others" without time.
fn album_groups(cs: &[Candidate], kept: &[usize], lang: Lang) -> Vec<Group> {
    let album = |i: usize| cs[i].album.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let mut groups: Vec<Group> = Vec::new();
    let mut loose = Vec::new();
    for &i in kept {
        let Some(a) = album(i) else {
            loose.push(i);
            continue;
        };
        match groups.iter_mut().find(|g| g.name.as_deref() == Some(a)) {
            Some(g) => g.ids.push(i),
            None => groups.push(Group { name: Some(a.to_string()), ids: vec![i] }),
        }
    }
    let span = |g: &Group| {
        let times = g.ids.iter().filter_map(|&i| cs[i].captured);
        (times.clone().min(), times.max())
    };
    groups.sort_by(|a, b| opt_last(span(a).0, span(b).0).then(a.name.cmp(&b.name)));
    let spans: Vec<(Option<i64>, Option<i64>)> = groups.iter().map(span).collect();
    let mut rest = Vec::new();
    for i in loose {
        let near = cs[i].captured.and_then(|t| {
            let dist = |s: &(Option<i64>, Option<i64>)| s.0.zip(s.1).map(|(lo, hi)| (lo - t).max(t - hi).max(0));
            spans.iter().enumerate().filter_map(|(k, s)| dist(s).map(|d| (d, k))).min().map(|(_, k)| k)
        });
        match near {
            Some(k) => groups[k].ids.push(i),
            None => rest.push(i),
        }
    }
    if !rest.is_empty() {
        groups.push(Group { name: Some(others(lang)), ids: rest });
    }
    groups
}

/// Merges chapters under `MIN_CHAPTER` photos (smallest first) into the neighbour closest in time.
fn merge_small(runs: &mut Vec<Vec<usize>>, cs: &[Candidate]) {
    let start = |r: &Vec<usize>| r.first().and_then(|&i| cs[i].captured).unwrap_or_default();
    let end = |r: &Vec<usize>| r.last().and_then(|&i| cs[i].captured).unwrap_or_default();
    while runs.len() > 1 {
        let Some(s) = (0..runs.len()).filter(|&k| runs[k].len() < MIN_CHAPTER).min_by_key(|&k| (runs[k].len(), k)) else {
            break;
        };
        let before = (s > 0).then(|| start(&runs[s]).saturating_sub(end(&runs[s - 1])));
        let after = (s + 1 < runs.len()).then(|| start(&runs[s + 1]).saturating_sub(end(&runs[s])));
        let small = runs.remove(s);
        // tie: the earlier neighbour
        if before.is_some_and(|b| after.is_none_or(|a| b <= a)) {
            runs[s - 1].extend(small);
        } else {
            runs[s].splice(0..0, small);
        }
    }
}

/// Chapters cut at every silence longer than `gap_s`; photos without a time go last.
fn time_groups(cs: &[Candidate], kept: &[usize], gap_s: i64, lang: Lang) -> Vec<Group> {
    let mut timed: Vec<usize> = kept.iter().copied().filter(|&i| cs[i].captured.is_some()).collect();
    let untimed: Vec<usize> = kept.iter().copied().filter(|&i| cs[i].captured.is_none()).collect();
    timed.sort_by_key(|&i| cs[i].captured);
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for i in timed {
        let t = cs[i].captured.unwrap_or_default();
        let last = runs.last().and_then(|r| r.last()).and_then(|&j| cs[j].captured).unwrap_or_default();
        match runs.last_mut() {
            Some(r) if t.saturating_sub(last) <= gap_s => r.push(i),
            _ => runs.push(vec![i]),
        }
    }
    merge_small(&mut runs, cs);
    let mut groups: Vec<Group> = runs.into_iter().map(|ids| Group { name: None, ids }).collect();
    if !untimed.is_empty() {
        let name = if groups.is_empty() { "Photos".to_string() } else { others(lang) };
        groups.push(Group { name: Some(name), ids: untimed });
    }
    groups
}

fn shorten(s: &str) -> String {
    if s.chars().count() <= SCENE_MAX_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(SCENE_MAX_CHARS - 1).collect();
    format!("{}…", head.trim_end())
}

/// The scene shared by at least 60 % of the chapter (case-insensitive), if any.
fn dominant_scene(cs: &[Candidate], ids: &[usize]) -> Option<String> {
    let mut count: BTreeMap<String, (usize, &str)> = BTreeMap::new();
    for &i in ids {
        if let Some(s) = cs[i].scene.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            count.entry(s.to_lowercase()).or_insert((0, s)).0 += 1;
        }
    }
    // BTreeMap order + strict ">" = the alphabetically first of equal counts
    let best = count.values().fold(None::<(usize, &str)>, |b, &v| if b.is_none_or(|b| v.0 > b.0) { Some(v) } else { b })?;
    (best.0 * 100 >= SCENE_SHARE_PCT * ids.len()).then(|| shorten(best.1))
}

/// Time of day of the chapter's middle photo: 05-12 morning, 12-14 noon, 14-18 afternoon,
/// 18-22 evening, else night.
fn moment(cs: &[Candidate], ids: &[usize], lang: Lang) -> String {
    let mut times: Vec<i64> = ids.iter().filter_map(|&i| cs[i].captured).collect();
    times.sort_unstable();
    let Some(&mid) = times.get(times.len() / 2) else {
        return "Photos".to_string();
    };
    let names = match mid.rem_euclid(86_400) / 3600 {
        5..=11 => ("Matin", "Morning"),
        12..=13 => ("Midi", "Noon"),
        14..=17 => ("Après-midi", "Afternoon"),
        18..=21 => ("Soirée", "Evening"),
        _ => ("Nuit", "Night"),
    };
    if lang == Lang::Fr { names.0 } else { names.1 }.to_string()
}

/// Final names, unique: a repeated name becomes "Evening 2", "Evening 3"...
fn name_groups(cs: &[Candidate], groups: Vec<Group>, lang: Lang) -> Vec<(String, Vec<usize>)> {
    let mut used: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for g in groups {
        let base = g.name.unwrap_or_else(|| dominant_scene(cs, &g.ids).unwrap_or_else(|| moment(cs, &g.ids, lang)));
        let mut name = base.clone();
        let mut n = 2;
        while !used.insert(name.clone()) {
            name = format!("{base} {n}");
            n += 1;
        }
        out.push((name, g.ids));
    }
    out
}

fn best_of(cs: &[Candidate], q: &[f32], ids: &[usize]) -> Option<usize> {
    ids.iter().copied().max_by(|&a, &b| q[a].total_cmp(&q[b]).then(cs[b].id.cmp(&cs[a].id)))
}

/// Chronological story; a chapter of more than 12 photos opens with its best photo when that photo
/// is among the best 25 % of the whole gallery (a weak chapter does not open on a mediocre frame).
fn order_chapter(cs: &[Candidate], q: &[f32], ids: &mut Vec<usize>, kept: &[usize]) {
    ids.sort_by(|&a, &b| opt_last(cs[a].captured, cs[b].captured));
    if ids.len() <= OPEN_ABOVE {
        return;
    }
    let Some(best) = best_of(cs, q, ids) else { return };
    let better = kept.iter().filter(|&&j| q[j] > q[best]).count();
    if better < kept.len().div_ceil(4) {
        ids.retain(|&i| i != best);
        ids.insert(0, best);
    }
}

/// Best quality among the kept photos with a sharp face (when the gallery has any), else among all
/// kept; a landscape photo gets a small bonus when asked. Returns the photo and whether a face drove it.
fn pick_cover(cs: &[Candidate], q: &[f32], kept: &[usize], landscape: bool) -> Option<(usize, bool)> {
    let faces: Vec<usize> = kept.iter().copied().filter(|&i| sharp_face(&cs[i])).collect();
    let by_face = !faces.is_empty();
    let pool = if by_face { &faces } else { kept };
    let score = |i: usize| q[i] + if landscape && cs[i].width > cs[i].height { LANDSCAPE_BONUS } else { 0.0 };
    let best = pool.iter().copied().max_by(|&a, &b| score(a).total_cmp(&score(b)).then(cs[b].id.cmp(&cs[a].id)))?;
    Some((best, by_face))
}

fn count_word(n: usize, lang: Lang, fr: (&str, &str), en: (&str, &str)) -> String {
    let (one, many) = if lang == Lang::Fr { fr } else { en };
    format!("{n} {}", if n == 1 { one } else { many })
}

struct Summary {
    chapters: usize,
    kept: usize,
    dropped: usize,
    total: usize,
    by_album: bool,
    capped: bool,
    cover: Option<bool>,
    landscape: bool,
    no_sharpness: usize,
    no_faces: usize,
}

/// 2 to 4 short sentences; counts only, never names.
fn notes(s: &Summary, lang: Lang) -> Vec<String> {
    let fr = lang == Lang::Fr;
    let chapters = count_word(s.chapters, lang, ("chapitre", "chapitres"), ("chapter", "chapters"));
    let origin = match (s.by_album, fr) {
        (true, true) => "albums existants",
        (true, false) => "existing albums",
        (false, true) => "découpage par heure",
        (false, false) => "split by time",
    };
    let mut out = vec![if fr {
        format!("{chapters} ({origin}), {} sur {} photos gardées.", s.kept, s.total)
    } else {
        format!("{chapters} ({origin}), {} of {} photos kept.", s.kept, s.total)
    }];
    let mut dropped = if s.dropped == 0 {
        if fr { "Aucune photo écartée.".to_string() } else { "No photo dropped.".to_string() }
    } else {
        let n = count_word(s.dropped, lang, ("photo écartée", "photos écartées"), ("photo dropped", "photos dropped"));
        format!("{n}.")
    };
    if s.capped {
        dropped.push_str(if fr { " Plafond de 40 % atteint : les moins mauvaises sont gardées." } else { " 40 % ceiling reached: the least bad are kept." });
    }
    out.push(dropped);
    if let Some(face) = s.cover {
        let what = match (face, fr) {
            (true, true) => "la meilleure photo avec un visage net",
            (true, false) => "the best photo with a sharp face",
            (false, true) => "la meilleure photo de la galerie",
            (false, false) => "the best photo of the gallery",
        };
        let land = match (s.landscape, fr) {
            (true, true) => ", paysage de préférence",
            (true, false) => ", landscape preferred",
            _ => "",
        };
        out.push(if fr { format!("Couverture : {what}{land}.") } else { format!("Cover: {what}{land}.") });
    }
    let (missing, what_fr, what_en) = if s.no_sharpness > 0 { (s.no_sharpness, "netteté", "sharpness") } else { (s.no_faces, "visages", "faces") };
    if missing > 0 {
        out.push(if fr {
            format!("{missing} photos sans analyse de {what_fr} : lance Analyser.")
        } else {
            format!("{missing} photos without {what_en} analysis: run Analyse.")
        });
    }
    out
}

/// Organises the gallery: what to drop, the chapters, their order, the cover.
pub fn organise(candidates: &[Candidate], opts: &Options) -> Plan {
    let cs = candidates;
    if cs.is_empty() {
        return Plan::default();
    }
    let lang = opts.lang;
    let q: Vec<f32> = cs.iter().map(quality).collect();
    let (why, capped) = find_dropped(cs, &q, opts.drop_weak);
    let kept: Vec<usize> = (0..cs.len()).filter(|&i| why[i].is_none()).collect();
    let dropped: Vec<Dropped> = cs.iter().zip(&why).filter_map(|(c, w)| w.map(|w| Dropped { id: c.id, reason: reason(w, lang) })).collect();

    let by_album = kept.iter().any(|&i| cs[i].album.as_deref().is_some_and(|a| !a.trim().is_empty()));
    let groups = if by_album { album_groups(cs, &kept, lang) } else { time_groups(cs, &kept, opts.gap_minutes.max(1).saturating_mul(60), lang) };
    let mut named = name_groups(cs, groups, lang);
    for (_, ids) in &mut named {
        order_chapter(cs, &q, ids, &kept);
    }
    let chapters: Vec<Chapter> = named.into_iter().map(|(name, ids)| Chapter { name, ids: ids.into_iter().map(|i| cs[i].id).collect() }).collect();

    let cover = pick_cover(cs, &q, &kept, opts.target_cover_landscape);
    let summary = Summary {
        chapters: chapters.len(),
        kept: kept.len(),
        dropped: dropped.len(),
        total: cs.len(),
        by_album,
        capped,
        cover: cover.map(|c| c.1),
        landscape: opts.target_cover_landscape,
        no_sharpness: cs.iter().filter(|c| c.sharpness.is_none()).count(),
        no_faces: cs.iter().filter(|c| c.faces.is_none()).count(),
    };
    Plan { notes: notes(&summary, lang), cover: cover.map(|c| cs[c.0].id), chapters, dropped }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Day 20000, midnight: hour `h` is `at(h)`.
    const DAY: i64 = 20_000 * 86_400;
    fn at(h: f64) -> i64 {
        DAY + (h * 3600.0) as i64
    }

    fn cand(id: u64) -> Candidate {
        Candidate {
            id: PhotoId(id),
            captured: None,
            sharpness: None,
            verdict_score: None,
            retouch_score: None,
            burst: None,
            best_of_burst: false,
            faces: None,
            people: vec![],
            scene: None,
            width: 2000,
            height: 3000,
            rating: 0,
            flag_pick: false,
            flag_reject: false,
            album: None,
        }
    }

    /// `n` photos from id `first`, a minute apart starting at hour `h`.
    fn run(first: u64, n: u64, h: f64) -> Vec<Candidate> {
        (0..n).map(|k| Candidate { captured: Some(at(h) + 60 * k as i64), ..cand(first + k) }).collect()
    }

    fn names(p: &Plan) -> Vec<&str> {
        p.chapters.iter().map(|c| c.name.as_str()).collect()
    }

    fn opts(lang: Lang) -> Options {
        Options { lang, ..Options::default() }
    }

    #[test]
    fn splits_on_gap_and_merges_small_chapters() {
        let mut v = run(1, 8, 9.0);
        v.extend(run(100, 8, 15.0));
        v.extend(run(200, 2, 16.5)); // 2 photos, 80 min after the second run, far from the first
        let p = organise(&v, &opts(Lang::Fr));
        assert_eq!(p.chapters.len(), 2);
        assert_eq!(p.chapters[0].ids.len(), 8);
        assert_eq!(p.chapters[1].ids.len(), 10);
        assert!(p.chapters[1].ids.contains(&PhotoId(200)));
    }

    #[test]
    fn tiny_gallery_stays_one_chapter() {
        let mut v = run(1, 3, 9.0);
        v.extend(run(10, 3, 20.0));
        assert_eq!(organise(&v, &opts(Lang::En)).chapters.len(), 1);
    }

    #[test]
    fn day_moments_in_both_languages_and_numbered_duplicates() {
        let mut v = Vec::new();
        for (k, h) in [9.0, 12.5, 15.0, 19.0, 21.0, 23.5].into_iter().enumerate() {
            v.extend(run(k as u64 * 100, 6, h));
        }
        assert_eq!(names(&organise(&v, &opts(Lang::Fr))), ["Matin", "Midi", "Après-midi", "Soirée", "Soirée 2", "Nuit"]);
        assert_eq!(names(&organise(&v, &opts(Lang::En))), ["Morning", "Noon", "Afternoon", "Evening", "Evening 2", "Night"]);
    }

    #[test]
    fn dominant_scene_names_the_chapter() {
        let mut v = run(1, 10, 10.0);
        for c in v.iter_mut().take(6) {
            c.scene = Some(" Plage ".into());
        }
        assert_eq!(names(&organise(&v, &opts(Lang::Fr))), ["Plage"]);
        // 5 of 10 is under 60 %: the moment names it
        v[5].scene = Some("Ville".into());
        assert_eq!(names(&organise(&v, &opts(Lang::Fr))), ["Matin"]);
    }

    #[test]
    fn existing_albums_win() {
        let mut v = run(1, 3, 15.0);
        v.extend(run(10, 3, 9.0));
        for c in v.iter_mut().take(3) {
            c.album = Some("Mariage".into());
        }
        for c in v.iter_mut().skip(3) {
            c.album = Some("Préparatifs".into());
        }
        let mut near = cand(50); // no album, close to "Préparatifs"
        near.captured = Some(at(9.5));
        let lost = cand(51); // no album, no time
        v.push(near);
        v.push(lost);
        let p = organise(&v, &opts(Lang::Fr));
        assert_eq!(names(&p), ["Préparatifs", "Mariage", "Autres"]);
        assert!(p.chapters[0].ids.contains(&PhotoId(50)));
        assert_eq!(p.chapters[2].ids, [PhotoId(51)]);
        assert!(p.notes[0].contains("albums existants"));
    }

    #[test]
    fn bursts_keep_the_best_never_a_pick_or_four_stars() {
        let mut v = run(1, 10, 9.0);
        for c in v.iter_mut().take(4) {
            c.burst = Some(7);
            c.sharpness = Some(50.0);
        }
        v[1].best_of_burst = true;
        v[2].flag_pick = true;
        v[3].rating = 4;
        let o = Options { drop_weak: true, ..opts(Lang::Fr) };
        let p = organise(&v, &o);
        assert_eq!(p.dropped, [Dropped { id: PhotoId(1), reason: "rafale : une meilleure existe".into() }]);
        // no flagged best: the sharpest stays
        let mut w = run(1, 10, 9.0);
        for (k, c) in w.iter_mut().take(3).enumerate() {
            c.burst = Some(1);
            c.sharpness = Some(40.0 + 10.0 * k as f32);
        }
        let p = organise(&w, &Options { drop_weak: true, ..opts(Lang::En) });
        let ids: Vec<u64> = p.dropped.iter().map(|d| d.id.0).collect();
        assert_eq!(ids, [1, 2]);
        assert_eq!(p.dropped[0].reason, "burst: a better one exists");
    }

    #[test]
    fn rejected_always_dropped_blurry_only_when_asked() {
        let mut v = run(1, 10, 9.0);
        v[0].flag_reject = true;
        v[1].sharpness = Some(5.0);
        let p = organise(&v, &opts(Lang::Fr));
        assert_eq!(p.dropped, [Dropped { id: PhotoId(1), reason: "rejetée".into() }]);
        let p = organise(&v, &Options { drop_weak: true, ..opts(Lang::Fr) });
        assert_eq!(p.dropped.iter().map(|d| d.reason.as_str()).collect::<Vec<_>>(), ["rejetée", "floue"]);
        assert_eq!(p.chapters[0].ids.len(), 8);
    }

    #[test]
    fn never_drops_more_than_forty_percent() {
        let mut v = run(1, 10, 9.0);
        for c in v.iter_mut() {
            c.flag_reject = true;
            c.verdict_score = Some(c.id.0 as f32 * 10.0);
        }
        let p = organise(&v, &opts(Lang::Fr));
        let ids: Vec<u64> = p.dropped.iter().map(|d| d.id.0).collect();
        assert_eq!(ids, [1, 2, 3, 4], "the worst four go, the least bad stay");
        assert_eq!(p.chapters[0].ids.len(), 6);
        assert!(p.notes.iter().any(|n| n.contains("40 %")));
    }

    #[test]
    fn opens_with_best_photo_only_above_twelve() {
        let build = |n: u64| {
            let mut v = run(1, n, 9.0);
            v[6].verdict_score = Some(95.0);
            v
        };
        let p = organise(&build(13), &opts(Lang::Fr));
        assert_eq!(p.chapters[0].ids[0], PhotoId(7));
        assert_eq!(p.chapters[0].ids[1], PhotoId(1));
        let p = organise(&build(12), &opts(Lang::Fr));
        let ids: Vec<u64> = p.chapters[0].ids.iter().map(|i| i.0).collect();
        assert_eq!(ids, (1..=12).collect::<Vec<_>>());
    }

    #[test]
    fn chapter_is_chronological_whatever_the_input_order() {
        let mut v = run(1, 6, 9.0);
        v.reverse();
        let ids: Vec<u64> = organise(&v, &opts(Lang::Fr)).chapters[0].ids.iter().map(|i| i.0).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn cover_prefers_sharp_faces_landscape_and_is_deterministic() {
        let mut v = run(1, 8, 9.0);
        v[0].verdict_score = Some(99.0); // best overall, no face
        v[1].faces = Some(1);
        v[1].sharpness = Some(80.0);
        v[2].faces = Some(2);
        v[2].sharpness = Some(80.0);
        v[3].faces = Some(1);
        v[3].sharpness = Some(10.0); // face but not sharp
        v[3].verdict_score = Some(100.0);
        // equal quality for photos 2 and 3: the lower id wins
        assert_eq!(organise(&v, &opts(Lang::Fr)).cover, Some(PhotoId(2)));
        let mut r = v.clone();
        r.reverse();
        assert_eq!(organise(&r, &opts(Lang::Fr)).cover, Some(PhotoId(2)));
        // landscape bonus breaks the tie the other way
        v[2].width = 3000;
        v[2].height = 2000;
        let o = Options { target_cover_landscape: true, ..opts(Lang::Fr) };
        assert_eq!(organise(&v, &o).cover, Some(PhotoId(3)));
        // no face anywhere: best quality overall
        let mut w = run(1, 8, 9.0);
        w[4].verdict_score = Some(90.0);
        assert_eq!(organise(&w, &opts(Lang::Fr)).cover, Some(PhotoId(5)));
    }

    #[test]
    fn cover_is_never_a_dropped_photo() {
        let mut v = run(1, 10, 9.0);
        v[0].flag_reject = true;
        v[0].verdict_score = Some(100.0);
        v[0].faces = Some(3);
        v[0].sharpness = Some(100.0);
        let p = organise(&v, &opts(Lang::Fr));
        assert!(p.dropped.iter().any(|d| d.id == PhotoId(1)));
        assert_ne!(p.cover, Some(PhotoId(1)));
        assert!(p.cover.is_some());
    }

    #[test]
    fn empty_input_gives_an_empty_plan() {
        assert_eq!(organise(&[], &opts(Lang::Fr)), Plan::default());
        assert_eq!(organise(&[], &Options { drop_weak: true, ..opts(Lang::En) }), Plan::default());
    }

    #[test]
    fn missing_data_everywhere_gives_one_photos_chapter() {
        let v: Vec<Candidate> = (1..=5).map(cand).collect();
        let p = organise(&v, &Options { drop_weak: true, ..opts(Lang::Fr) });
        assert_eq!(names(&p), ["Photos"]);
        assert_eq!(p.chapters[0].ids.len(), 5);
        assert!(p.dropped.is_empty());
        assert_eq!(p.cover, Some(PhotoId(1)));
        assert!(p.notes.iter().any(|n| n.contains("5 photos sans analyse de netteté : lance Analyser")));
        assert!((2..=4).contains(&p.notes.len()));
    }

    #[test]
    fn notes_carry_no_names_and_follow_the_language() {
        let mut v = run(1, 8, 9.0);
        v[0].people = vec!["Awa Diallo".into()];
        v[0].faces = Some(1);
        v[0].sharpness = Some(70.0);
        let en = organise(&v, &opts(Lang::En));
        assert!(en.notes.iter().all(|n| !n.contains("Awa")));
        assert!(en.notes.iter().any(|n| n.starts_with("Cover:")));
        let fr = organise(&v, &opts(Lang::Fr));
        assert!(fr.notes.iter().any(|n| n.starts_with("Couverture :")));
    }

    #[test]
    fn quality_ignores_missing_data_and_rewards_stars() {
        assert_eq!(quality(&cand(1)), 0.5);
        let a = Candidate { sharpness: Some(80.0), ..cand(1) };
        assert!((quality(&a) - 0.8).abs() < 1e-6);
        let b = Candidate { rating: 5, ..a.clone() };
        assert!(quality(&b) > quality(&a));
        let r = Candidate { flag_reject: true, ..a.clone() };
        assert!(quality(&r) < quality(&a));
    }

    #[test]
    fn same_input_same_output() {
        let mut v = run(1, 14, 9.0);
        v.extend(run(100, 7, 17.0));
        let a = organise(&v, &Options { drop_weak: true, ..opts(Lang::Fr) });
        let b = organise(&v, &Options { drop_weak: true, ..opts(Lang::Fr) });
        assert_eq!(a, b);
    }
}
