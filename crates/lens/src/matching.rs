//! Recognising the camera and the lens from photo metadata.
//!
//! The lens name written by cameras rarely equals the database's name (`EF24-105mm f/4L IS USM` against
//! `Canon EF 24-105mm f/4L IS USM`), and some bodies write only a focal length (`50mm`). So a lens is found by:
//! 1. the camera body (name → mount and crop factor), which restricts the lenses to that mount;
//! 2. a name similarity (Dice coefficient over lower-cased alphanumeric tokens, letters and digits split apart) when
//!    the name says something, otherwise the focal range and widest aperture the metadata carries;
//! 3. the calibration made on the largest sensor frame that is not smaller than the camera's.
//!
//! The answer says how sure it is and lists the other candidates; it never invents a profile.

use std::collections::HashSet;

use crate::profile::build_correction;
use crate::{Camera, Database, LensEntry, Resolved};

/// What the metadata of a photo says about its camera and lens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub camera_make: String,
    pub camera_model: String,
    pub lens_make: String,
    pub lens_model: String,
    /// Exif `LensSpecification`: min/max focal length, widest f-number at each end (0 = unknown).
    pub lens_spec: Option<[f64; 4]>,
    pub lens_max_aperture: Option<f64>,
    /// The shot: focal length (mm), 35 mm equivalent, f-number, subject distance (m).
    pub focal: Option<f64>,
    pub focal_35: Option<f64>,
    pub aperture: Option<f64>,
    pub distance: Option<f64>,
}

impl Query {
    /// A key identifying everything the lookup depends on (equal queries resolve alike).
    pub fn cache_key(&self) -> String {
        let q = |v: Option<f64>| v.map(|x| format!("{x:.3}")).unwrap_or_default();
        format!(
            "{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}",
            self.camera_make,
            self.camera_model,
            self.lens_make,
            self.lens_model,
            self.lens_spec,
            q(self.lens_max_aperture),
            q(self.focal),
            q(self.focal_35),
            q(self.aperture),
            q(self.distance)
        )
    }
}

/// How sure the lens recognition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// The name matches one database lens.
    Exact,
    /// One lens fits (by name, or by focal range and aperture when the name is only a focal length).
    Probable,
    /// Several database lenses fit equally (the first, best calibrated, is used).
    Ambiguous,
    /// No database lens fits.
    None,
}

/// The outcome of a recognition, for display and diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// The camera as the photo names it (maker + model).
    pub camera: String,
    /// The database knows this body (mount and crop factor are the database's).
    pub camera_known: bool,
    pub crop_factor: Option<f64>,
    /// The lens as the photo names it.
    pub lens_in_photo: String,
    /// The database lens used (display name), when one was found.
    pub lens: Option<String>,
    pub confidence: Confidence,
    /// The other lenses that fit as well as the chosen one.
    pub alternatives: Vec<String>,
    /// What the profile corrects at this focal length / aperture.
    pub distortion: bool,
    pub lateral_ca: bool,
    pub vignetting: bool,
    /// Why nothing could be applied, or a caveat.
    pub note: String,
}

impl Detection {
    /// Any correction available.
    pub fn has_corrections(&self) -> bool {
        self.distortion || self.lateral_ca || self.vignetting
    }
}

/// Normalised letters-and-digits form for exact comparisons.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Lower-case tokens, letters and digits kept apart (`24-105mm` → `24`, `105`, `mm`).
fn tokens(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = String::new();
    let mut digit = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        let d = c.is_ascii_digit();
        if c.is_alphanumeric() {
            if !cur.is_empty() && d != digit {
                out.insert(std::mem::take(&mut cur));
            }
            digit = d;
            cur.push(c);
        } else if !cur.is_empty() {
            out.insert(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.insert(cur);
    }
    const MAKERS: &[&str] = &[
        "canon",
        "nikon",
        "nikkor",
        "sony",
        "sigma",
        "tamron",
        "tokina",
        "panasonic",
        "lumix",
        "leica",
        "olympus",
        "pentax",
        "fujifilm",
        "fujinon",
        "zeiss",
        "samyang",
        "rokinon",
        "lens",
        "objectif",
    ];
    out.retain(|t| !MAKERS.contains(&t.as_str()));
    out
}

fn dice(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.intersection(b).count() as f64;
    2.0 * common / (a.len() + b.len()) as f64
}

/// A lens name that carries information beyond a focal length (`50mm`, `18-55mm`).
fn informative(name: &str) -> bool {
    let t = tokens(name);
    t.iter().any(|x| !x.chars().all(|c| c.is_ascii_digit()) && x != "mm") && t.len() >= 3
}

fn stops(a: f64, b: f64) -> f64 {
    if a > 0.0 && b > 0.0 { 2.0 * (a / b).log2().abs() } else { f64::INFINITY }
}

impl Database {
    /// The body `make model` names, matching either form of its name (`Canon EOS 5D Mark III`, `EOS 5D Mark III`).
    pub fn find_camera(&self, make: &str, model: &str) -> Option<&Camera> {
        let m = squash(model);
        if m.is_empty() {
            return None;
        }
        let mk = squash(make);
        let strip = |s: &str| s.strip_prefix(&mk).filter(|_| !mk.is_empty()).unwrap_or(s).to_string();
        let m_short = strip(&m);
        let maker_ok = |c: &Camera| mk.is_empty() || squash(&c.maker).contains(&mk) || mk.contains(&squash(&c.maker));
        self.cameras.iter().filter(|c| maker_ok(c)).find(|c| {
            c.names.iter().any(|n| {
                let n = squash(n);
                n == m || n == m_short || strip(&n) == m_short
            })
        })
    }

    fn allowed_mounts(&self, mount: &str) -> (Vec<String>, Vec<String>) {
        let native = vec![mount.to_string()];
        let adapted = self.mounts.iter().find(|m| m.name == mount).map(|m| m.compat.clone()).unwrap_or_default();
        (native, adapted)
    }

    /// Recognise the camera and lens of `q`, and build the corrections at its focal length, aperture and distance.
    pub fn resolve(&self, q: &Query) -> Resolved {
        let camera = [q.camera_make.trim(), q.camera_model.trim()].iter().filter(|s| !s.is_empty()).fold(String::new(), |mut a, s| {
            // "Canon" + "Canon EOS 5D" → "Canon EOS 5D"
            if !a.is_empty() && s.to_lowercase().starts_with(&a.to_lowercase()) {
                a.clear();
            }
            if !a.is_empty() {
                a.push(' ');
            }
            a.push_str(s);
            a
        });
        let cam = self.find_camera(&q.camera_make, &q.camera_model);
        let est_crop = match (q.focal, q.focal_35) {
            (Some(f), Some(f35)) if f > 0.0 && f35 > 0.0 && (0.5..8.0).contains(&(f35 / f)) => Some(f35 / f),
            _ => None,
        };
        let crop = cam.map(|c| c.crop).or(est_crop);
        let mut det = Detection {
            camera,
            camera_known: cam.is_some(),
            crop_factor: crop,
            lens_in_photo: q.lens_model.trim().to_string(),
            lens: None,
            confidence: Confidence::None,
            alternatives: Vec::new(),
            distortion: false,
            lateral_ca: false,
            vignetting: false,
            note: String::new(),
        };
        let none = |mut d: Detection, why: &str| {
            d.note = why.to_string();
            Resolved { detection: d, correction: None }
        };
        let Some(crop) = crop else { return none(det, "camera and sensor size unknown") };
        // focal range, widest aperture: the metadata's description of the lens
        let spec = q.lens_spec.filter(|s| s[0] > 0.0);
        let focal = q.focal.filter(|f| *f > 0.0);
        if det.lens_in_photo.is_empty() && spec.is_none() && focal.is_none() {
            return none(det, "the photo names no lens");
        }
        let widest = q.lens_max_aperture.or(spec.map(|s| s[2]).filter(|a| *a > 0.0));
        let by_name = informative(&det.lens_in_photo);
        let qt = tokens(&det.lens_in_photo);
        let (native, adapted) = cam.map(|c| self.allowed_mounts(&c.mount)).unwrap_or_default();
        let maker_hint = squash(&q.lens_make);
        let camera_maker = squash(&q.camera_make);

        // Score every database lens.
        struct Cand<'a> {
            e: &'a LensEntry,
            score: f64,
        }
        let score = |e: &LensEntry| -> Option<f64> {
            // focal range must contain the shot's focal length (and equal the described range when known)
            if let (Some((lo, hi)), Some(f)) = (e.focal, focal)
                && (f < lo * 0.97 || f > hi * 1.03)
            {
                return None;
            }
            let mut s = if by_name {
                let d = e.names.iter().map(|n| dice(&qt, &tokens(&format!("{} {}", e.maker, n)))).fold(0.0, f64::max);
                if d < 0.55 {
                    return None;
                }
                d
            } else {
                let (Some(sp), Some((lo, hi))) = (spec, e.focal) else { return None };
                let ok = |a: f64, b: f64| (a - b).abs() <= 0.02 * b.max(1.0);
                if !(ok(sp[0], lo) && ok(sp[1], hi)) {
                    return None;
                }
                0.6
            };
            // widest aperture, when both sides know it
            // (the metadata gives the widest aperture at the current zoom, so any value of the lens's range fits)
            if let (Some(w), Some((ap, ap_tele))) = (widest, e.aperture) {
                let (lo, hi) = (ap.min(ap_tele), ap.max(ap_tele));
                let d = if w >= lo && w <= hi { 0.0 } else { stops(w, lo).min(stops(w, hi)) };
                if d.is_finite() {
                    if !by_name && d > 0.4 {
                        return None;
                    }
                    s -= (d * 0.1).min(0.3);
                }
            }
            let em = squash(&e.maker);
            let same = |a: &str| !a.is_empty() && !em.is_empty() && (em.contains(a) || a.contains(&em));
            if !maker_hint.is_empty() && !same(&maker_hint) {
                s -= 0.1;
            }
            // a bare "50mm" is far more often the camera maker's own lens: a prior, not a rule
            if same(&camera_maker) {
                s += 0.05;
            }
            Some(s)
        };
        let collect = |mounts: &[String]| -> Vec<Cand<'_>> {
            self.lenses
                .iter()
                .filter(|e| mounts.is_empty() || e.mounts.iter().any(|m| mounts.contains(m)))
                .filter_map(|e| score(e).map(|s| Cand { e, score: s }))
                .collect()
        };
        let mut cands = collect(&native);
        if cands.is_empty() && cam.is_some() {
            cands = collect(&adapted);
        }
        if cands.is_empty() {
            let why = if !by_name && spec.is_none() {
                "the photo records no lens name or focal range, so the lens cannot be identified"
            } else if cam.is_none() {
                "camera not in the database and no lens of that name"
            } else if by_name {
                "no lens of that name in the database for this mount"
            } else {
                "no lens with that focal range in the database for this mount"
            };
            return none(det, why);
        }
        // one lens = one name; several calibrations of it differ by sensor crop
        let name_of = |e: &LensEntry| squash(&e.display_name());
        let best = cands.iter().map(|c| c.score).fold(0.0, f64::max);
        let top: Vec<&Cand<'_>> = cands.iter().filter(|c| c.score >= best - 0.02).collect();
        let mut names: Vec<String> = Vec::new();
        for c in &top {
            let n = name_of(c.e);
            if !names.contains(&n) {
                names.push(n);
            }
        }
        // among the best names, prefer the best calibrated at this focal length
        let fits_sensor = |e: &LensEntry| e.crop <= crop * 1.05 + 1e-6;
        let richness = |e: &LensEntry| -> usize {
            usize::from(!e.distortion.is_empty()) * 2 + usize::from(!e.vignetting.is_empty()) * 2 + usize::from(!e.tca.is_empty())
        };
        let mut pool: Vec<&Cand<'_>> = top.iter().copied().filter(|c| fits_sensor(c.e)).collect();
        if pool.is_empty() {
            let any = top.first().map(|c| c.e.display_name()).unwrap_or_default();
            det.lens = Some(any);
            det.confidence = Confidence::Probable;
            return none(det, "this lens is only calibrated for a larger sensor than the camera's: no correction applied");
        }
        pool.sort_by(|a, b| {
            richness(b.e)
                .cmp(&richness(a.e))
                .then(b.e.crop.partial_cmp(&a.e.crop).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.e.display_name().cmp(&b.e.display_name()))
        });
        let Some(chosen) = pool.first().map(|c| c.e) else { return none(det, "no lens") };
        let chosen_name = name_of(chosen);
        // the best calibration of the chosen lens for this sensor: the largest frame not above the camera's
        let entry = pool
            .iter()
            .map(|c| c.e)
            .filter(|e| name_of(e) == chosen_name)
            .max_by(|a, b| (a.crop, richness(a)).partial_cmp(&(b.crop, richness(b))).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(chosen);
        det.lens = Some(entry.display_name());
        det.alternatives = names
            .iter()
            .filter(|n| **n != chosen_name)
            .filter_map(|n| top.iter().find(|c| name_of(c.e) == *n).map(|c| c.e.display_name()))
            .take(5)
            .collect();
        det.confidence = if names.len() > 1 {
            Confidence::Ambiguous
        } else if by_name && best >= 0.85 {
            Confidence::Exact
        } else {
            Confidence::Probable
        };
        let (correction, flags) = build_correction(entry, crop, q);
        det.distortion = flags.0;
        det.lateral_ca = flags.1;
        det.vignetting = flags.2;
        if !det.has_corrections() {
            det.note = if entry.has_calibration() {
                "the profile has no data at this focal length / aperture".to_string()
            } else {
                "the lens is in the database but has no calibration yet".to_string()
            };
        } else if entry.kind.is_some() {
            det.note = "non-rectilinear lens: distortion not corrected".to_string();
        } else if det.confidence == Confidence::Ambiguous {
            det.note = format!("{} lenses fit equally; the best calibrated is used", names.len());
        }
        Resolved { detection: det, correction }
    }
}
