//! Places (feature `places`): where photos were taken. A photo's place is the hierarchical keyword
//! `Places|Country|Region|City` (set by `places.geotag` from the GPS position, or by `places.name` / `places.confirm`
//! by hand), and a photo with a GPS position but no such keyword is placed by its position anyway (see
//! `lightcraft_catalog::places`). Nothing is written to a photo without the user asking: `places.geotag` shows a preview
//! first, suggestions for photos without GPS (neighbours in time, visually similar photos) are only ever proposed.
//!
//! What is stored, and where: the place keyword is an ordinary keyword (undoable, written to XMP like the others). The
//! two settings (`autoGeotag`, `maxKm`) live in `<library>/places/settings.json` (memory only for a library that is not on
//! disk). The reverse-geocoding data is a separate file (`lightcraft-places`, GeoNames, CC BY 4.0).

mod suggest;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use lightcraft_catalog::{Meta, Op, Photo, PhotoId, places as cat};
use serde::{Deserialize, Serialize};

pub use suggest::{SuggestOptions, Suggestion, suggest as suggest_for};

use crate::{EngineError, Result, Session};

/// Whether this build can resolve a position to a place at all.
pub const AVAILABLE: bool = cfg!(feature = "places");
/// What commands say when the feature is off.
pub const UNAVAILABLE: &str = "Places are not available in this build";

/// The settings, saved with the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Give new photos their place keyword when they are imported or added. Off by default.
    pub auto_geotag: bool,
    /// Distance (km) up to which the nearest place names the city.
    pub max_km: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_geotag: false, max_km: DEFAULT_MAX_KM }
    }
}

/// The default radius (km) of a city answer.
pub const DEFAULT_MAX_KM: f64 = 50.0;

/// The session's places state.
#[derive(Default)]
pub struct Places {
    settings: Settings,
    /// The library the settings were read for (`Some(None)` = not on disk).
    loaded_for: Option<Option<PathBuf>>,
    /// The last suggestions (`places.suggest`), for `places.confirm` without a place.
    pub suggestions: HashMap<PhotoId, Suggestion>,
    /// Photos whose suggestion the user turned down, this session.
    pub dismissed: HashSet<PhotoId>,
}

fn other(e: impl Into<String>) -> EngineError {
    EngineError::Other(e.into())
}

impl Session {
    fn places_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("places"))
    }

    /// The settings of the open library (read from its folder the first time, and again when the library changes).
    pub fn places_settings(&mut self) -> Settings {
        let dir = self.places_dir();
        if self.places.loaded_for.as_ref() != Some(&dir) {
            if self.places.loaded_for.is_some() {
                // another library: its suggestions are not these
                self.places.suggestions.clear();
                self.places.dismissed.clear();
            }
            self.places.settings = dir
                .as_ref()
                .and_then(|d| std::fs::read(d.join("settings.json")).ok())
                .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
                .map(|mut s| {
                    s.max_km = clamp_km(s.max_km);
                    s
                })
                .unwrap_or_default();
            self.places.loaded_for = Some(dir);
        }
        self.places.settings.clone()
    }

    /// Change the settings and save them with the library.
    pub fn set_places_settings(&mut self, new: Settings) -> Result<()> {
        self.places_settings();
        self.places.settings = Settings { max_km: clamp_km(new.max_km), ..new };
        let Some(dir) = self.places_dir() else { return Ok(()) };
        std::fs::create_dir_all(&dir).map_err(|e| other(format!("places: {}: {e}", dir.display())))?;
        let bytes = serde_json::to_vec_pretty(&self.places.settings).map_err(|e| other(e.to_string()))?;
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, dir.join("settings.json"))).map_err(|e| other(format!("places: {e}")))
    }
}

fn clamp_km(v: f64) -> f64 {
    if v.is_finite() { v.clamp(1.0, 500.0) } else { DEFAULT_MAX_KM }
}

// ---- resolving positions

/// A resolved place with what the commands report about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub keyword: String,
    pub level: &'static str,
    pub distance_km: f64,
    pub country: String,
    pub region: Option<String>,
    pub city: Option<String>,
    pub population: u32,
}

/// Resolve a position with the installed data. `Err` when the build or the install has no data.
pub fn resolve(lat: f64, lon: f64, max_km: f64) -> std::result::Result<Option<Resolved>, String> {
    #[cfg(feature = "places")]
    {
        lightcraft_places::database().ok_or_else(|| match lightcraft_places::database_status() {
            Err(e) => e,
            Ok(_) => "the places database could not be used".to_string(),
        })?;
        Ok(lightcraft_places::place_at(lat, lon, max_km).map(|p| Resolved {
            keyword: p.keyword(),
            level: p.level.id(),
            distance_km: p.distance_km,
            country: p.country.clone(),
            region: p.region.clone(),
            city: p.city.clone(),
            population: p.population,
        }))
    }
    #[cfg(not(feature = "places"))]
    {
        let _ = (lat, lon, max_km);
        Err(UNAVAILABLE.into())
    }
}

/// Use the places data file at `path` for this process (tests and hosts that carry their own copy; the normal search
/// finds the installed file by itself). `Ok(false)` when a database was already in use.
pub fn install_database(path: &std::path::Path) -> std::result::Result<bool, String> {
    #[cfg(feature = "places")]
    {
        lightcraft_places::install_file(path).map_err(|e| e.to_string())
    }
    #[cfg(not(feature = "places"))]
    {
        let _ = path;
        Err(UNAVAILABLE.into())
    }
}

/// The names of the `Places` keywords of a photo.
pub fn place_keywords(p: &Photo) -> Vec<&String> {
    p.meta.keywords.iter().filter(|k| cat::is_place_keyword(k)).collect()
}

/// Normalise a place given by the user (`France|Poitiers` or `Places|France|Poitiers`) into a keyword under `Places`.
pub fn normalize_place(input: &str) -> Option<String> {
    let parts: Vec<String> = input.split(lightcraft_catalog::keywords::SEP).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    let rest: &[String] = match parts.first() {
        Some(f) if f.eq_ignore_ascii_case(cat::ROOT) => parts.get(1..)?,
        _ => &parts,
    };
    if rest.is_empty() {
        return None;
    }
    let rest: Vec<String> = rest.iter().map(|p| p.replace(',', " ")).collect();
    Some(format!("{}|{}", cat::ROOT, rest.join("|")))
}

/// Give a photo being imported its place keyword when the setting asks for it and its GPS resolves. Never fails.
pub fn tag_new_photo(s: &mut Session, meta: &mut Meta) {
    let st = s.places_settings();
    if !st.auto_geotag || meta.keywords.iter().any(|k| cat::is_place_keyword(k)) {
        return;
    }
    let Some((lat, lon)) = meta.gps else { return };
    if let Ok(Some(r)) = resolve(lat, lon, st.max_km) {
        meta.keywords.push(r.keyword);
    }
}

// ---- geotag

/// One photo of a geotag plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub id: PhotoId,
    pub lat: f64,
    pub lon: f64,
    pub place: Resolved,
}

/// What `places.geotag` would do.
#[derive(Debug, Default)]
pub struct Plan {
    pub scanned: usize,
    pub no_gps: usize,
    pub already_placed: usize,
    pub unresolved: Vec<(PhotoId, f64, f64)>,
    pub planned: Vec<Planned>,
}

/// The photos in scope (`ids`, else the whole library) that are worth a look.
pub fn geotag_plan(s: &mut Session, ids: &[PhotoId], max_km: f64, overwrite: bool) -> Result<Plan> {
    let mut plan = Plan::default();
    let mut data_error: Option<String> = None;
    for id in ids {
        let Some(p) = s.catalog.photo(*id) else { continue };
        plan.scanned += 1;
        let Some((lat, lon)) = p.meta.gps.and_then(|(a, o)| valid_gps(a, o)) else {
            plan.no_gps += 1;
            continue;
        };
        if !overwrite && !place_keywords(p).is_empty() {
            plan.already_placed += 1;
            continue;
        }
        match resolve(lat, lon, max_km) {
            Ok(Some(place)) => plan.planned.push(Planned { id: *id, lat, lon, place }),
            Ok(None) => plan.unresolved.push((*id, lat, lon)),
            Err(e) => {
                data_error = Some(e);
                break;
            }
        }
    }
    match data_error {
        Some(e) => Err(other(e)),
        None => Ok(plan),
    }
}

/// A usable position (the (0, 0) of a camera without a fix is not one).
fn valid_gps(lat: f64, lon: f64) -> Option<(f64, f64)> {
    if lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 360.0 && !(lat == 0.0 && lon == 0.0) { Some((lat, lon)) } else { None }
}

/// Set `keyword` as the only place of each photo of `assign` (keeping other keywords) in one undo step. Returns how many
/// photos changed.
pub fn apply_places(s: &mut Session, label: &str, assign: &[(PhotoId, Option<String>)]) -> Result<usize> {
    let mut ops = Vec::new();
    for (id, kw) in assign {
        let Some(p) = s.catalog.photo(*id) else { continue };
        let mut m = p.meta.clone();
        m.keywords.retain(|k| !cat::is_place_keyword(k));
        if let Some(kw) = kw {
            m.keywords.push(kw.clone());
        }
        if m.keywords != p.meta.keywords {
            ops.push(Op::SetMeta { id: *id, meta: Box::new(m) });
        }
    }
    let n = ops.len();
    if n > 0 {
        s.commit(label, Op::Batch { ops })?;
    }
    Ok(n)
}

/// Count by keyword, most photos first.
pub fn count_by_place(planned: &[Planned]) -> Vec<(String, usize)> {
    let mut m: BTreeMap<&str, usize> = BTreeMap::new();
    for p in planned {
        *m.entry(&p.place.keyword).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}
