//! Offline reverse geocoding for LightCraft: from a GPS position to **country / region / city**, with no network.
//!
//! # Data and licence
//!
//! The places come from the **GeoNames** gazetteer (<https://www.geonames.org/>), licensed under **Creative Commons
//! Attribution 4.0** (CC BY 4.0): the `citiesN` dump (every populated place with at least N inhabitants), `countryInfo.txt` and
//! `admin1CodesASCII.txt` from <https://download.geonames.org/export/dump/>. They are converted by
//! `examples/build_db.rs` into one compact file, [`DB_FILE`] (`places-cities.bin`: a zlib-compressed table in k-d tree
//! order). The data file stays a **separate file** (it is never compiled into a binary) and keeps its attribution
//! with it (`data/README.md`, `NOTICE`, the provenance block printed by `places.status`). This crate's code is
//! MIT OR Apache-2.0 like the rest of LightCraft; no code from any GPL / LGPL / AGPL geocoder was read or copied.
//!
//! # How a position is resolved
//!
//! The nearest populated places are found with a k-d tree on the unit sphere (exact great-circle nearest neighbours; the
//! poles and the antimeridian need no special case). The city is the biggest of the 16 nearest towns whose built-up
//! area covers the position (a radius of 0.35 km per square root of a thousand inhabitants, at most `max_km`), else the
//! nearest town: a photo at the Eiffel Tower is "Paris", not the district GeoNames lists as a place of its own, while a
//! photo in a village is the village. Quarters flagged by GeoNames (`PPLX`) only count when no town is in range. How
//! precise the answer is depends on the distance to that place (`max_km`, 50 km by default):
//!
//! | distance to the nearest place | answer | [`Level`] |
//! |---|---|---|
//! | up to `max_km` | country, region (GeoNames admin level 1) and city | [`Level::City`] |
//! | up to `2 × max_km` | country and region | [`Level::Region`] |
//! | up to `6 × max_km` | country only | [`Level::Country`] |
//! | farther (open sea, poles, deserts) | nothing | — |
//!
//! There is no country outline in the data, so a country read from a place 100-300 km away can be wrong near a border
//! and for a boat in a strait; that is why it is reported as a coarser level, never as a city.
//!
//! Names are the GeoNames names (UTF-8, with accents; English for countries and regions, e.g. "New Aquitaine"). A
//! level that repeats the one before it is dropped ("Reunion" is both country and region; Dakar is both region and
//! city). [`Place::keyword`] gives the hierarchical keyword `Places|Country|Region|City` that LightCraft writes.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod format;
mod kd;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

pub use format::LoadError;

/// File name of the places database.
pub const DB_FILE: &str = "places-cities.bin";
/// Environment variable naming the database file (or a folder containing it).
pub const DB_ENV: &str = "LIGHTCRAFT_PLACES_DB";
/// The first level of every place keyword.
pub const ROOT: &str = "Places";
/// Default distance (km) up to which the nearest place names the city.
pub const DEFAULT_MAX_KM: f64 = 50.0;
/// Mean Earth radius (km), for great-circle distances.
const EARTH_KM: f64 = 6371.0088;

/// Where the data comes from, for attribution (shown in the app and kept in the file).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Provenance {
    pub name: String,
    pub licence: String,
    pub url: String,
    /// The GeoNames dump files used and the date they were downloaded.
    pub files: String,
    pub retrieved: String,
    /// What was changed.
    pub changes: String,
    /// The attribution line to keep with every copy.
    pub attribution: String,
}

/// A country of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct CountryRecord {
    /// ISO 3166-1 alpha-2 code (`FR`).
    pub code: String,
    pub name: String,
}

/// A first-level administrative division (region, state, island…).
#[derive(Clone, Debug, PartialEq)]
pub struct AdminRecord {
    pub country: String,
    /// GeoNames admin1 code (`75`).
    pub code: String,
    pub name: String,
}

/// A populated place as the builder hands it over.
#[derive(Clone, Debug, PartialEq)]
pub struct CityRecord {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub country: String,
    /// GeoNames admin1 code, when the place has one.
    pub admin1: Option<String>,
    pub population: u32,
    /// A quarter or section of a larger place (GeoNames `PPLX`).
    pub section: bool,
}

/// How precisely a position was placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Country,
    Region,
    City,
}

impl Level {
    pub fn id(self) -> &'static str {
        match self {
            Level::Country => "country",
            Level::Region => "region",
            Level::City => "city",
        }
    }
}

/// What a position resolves to.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub country_code: String,
    pub country: String,
    pub region: Option<String>,
    pub city: Option<String>,
    pub level: Level,
    /// Distance (km) from the position to the place that named it.
    pub distance_km: f64,
    /// Inhabitants of that place (GeoNames; 0 when unknown).
    pub population: u32,
}

/// A level's name as a keyword part: no `|` (the hierarchy separator) and no comma.
pub fn keyword_part(s: &str) -> String {
    s.chars().map(|c| if c == '|' || c == ',' { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Place {
    /// The names from country down, without a level that repeats the previous one.
    pub fn path(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for part in [Some(&self.country), self.region.as_ref(), self.city.as_ref()].into_iter().flatten() {
            let part = keyword_part(part);
            if !part.is_empty() && v.last().is_none_or(|l| !l.eq_ignore_ascii_case(&part)) {
                v.push(part);
            }
        }
        v
    }
    /// The hierarchical keyword, `Places|France|New Aquitaine|Poitiers`.
    pub fn keyword(&self) -> String {
        let mut s = String::from(ROOT);
        for p in self.path() {
            s.push('|');
            s.push_str(&p);
        }
        s
    }
}

/// Great-circle distance in km between two positions in degrees (haversine).
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_KM * a.sqrt().clamp(0.0, 1.0).asin()
}

/// A usable position: finite, within range, and not the (0, 0) that cameras write when they have no fix.
/// Longitudes just outside ±180 are wrapped.
pub fn valid_position(lat: f64, lon: f64) -> Option<(f64, f64)> {
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 360.0 || (lat == 0.0 && lon == 0.0) {
        return None;
    }
    let lon = if lon > 180.0 {
        lon - 360.0
    } else if lon < -180.0 {
        lon + 360.0
    } else {
        lon
    };
    Some((lat, lon))
}

/// A country of the loaded table.
#[derive(Clone, Debug, PartialEq)]
struct Country {
    code: String,
    name: String,
}

/// A region of the loaded table (`name` empty = the country's places without a division).
#[derive(Clone, Debug, PartialEq)]
struct Admin {
    country: u16,
    name: String,
}

/// The places database, in k-d tree order (see [`kd`]).
pub struct Database {
    provenance: Provenance,
    countries: Vec<Country>,
    admins: Vec<Admin>,
    names: String,
    name_start: Vec<u32>,
    lat_e5: Vec<i32>,
    lon_e5: Vec<i32>,
    admin: Vec<u16>,
    population: Vec<u32>,
    section: Vec<bool>,
    xyz: Vec<kd::Xyz>,
}

/// One nearby place, for diagnostics.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Nearby {
    pub name: String,
    pub country_code: String,
    pub region: Option<String>,
    pub distance_km: f64,
    pub population: u32,
    pub section: bool,
}

impl Database {
    /// Number of places.
    pub fn len(&self) -> usize {
        self.lat_e5.len()
    }
    pub fn is_empty(&self) -> bool {
        self.lat_e5.is_empty()
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    pub fn country_count(&self) -> usize {
        self.countries.len()
    }

    fn name_of(&self, i: usize) -> &str {
        let start = self.name_start.get(i).copied().unwrap_or(0) as usize;
        let end = self.name_start.get(i + 1).map_or(self.names.len(), |e| (*e as usize).saturating_sub(1));
        self.names.get(start..end).unwrap_or("")
    }

    fn position(&self, i: usize) -> (f64, f64) {
        (f64::from(self.lat_e5.get(i).copied().unwrap_or(0)) * 1e-5, f64::from(self.lon_e5.get(i).copied().unwrap_or(0)) * 1e-5)
    }

    fn admin_of(&self, i: usize) -> Option<&Admin> {
        self.admin.get(i).and_then(|a| self.admins.get(usize::from(*a)))
    }

    fn country_of(&self, i: usize) -> Option<&Country> {
        self.admin_of(i).and_then(|a| self.countries.get(usize::from(a.country)))
    }

    /// The `k` places nearest to the position, closest first (quarters included).
    pub fn nearby(&self, lat: f64, lon: f64, k: usize) -> Vec<Nearby> {
        let Some((lat, lon)) = valid_position(lat, lon) else { return Vec::new() };
        let tree = kd::Tree { points: &self.xyz };
        tree.nearest(kd::deg_to_xyz(lat, lon), k, &|_| true)
            .into_iter()
            .map(|(i, _)| {
                let (la, lo) = self.position(i);
                Nearby {
                    name: self.name_of(i).to_string(),
                    country_code: self.country_of(i).map(|c| c.code.clone()).unwrap_or_default(),
                    region: self.admin_of(i).map(|a| a.name.clone()).filter(|n| !n.is_empty()),
                    distance_km: distance_km(lat, lon, la, lo),
                    population: self.population.get(i).copied().unwrap_or(0),
                    section: self.section.get(i).copied().unwrap_or(false),
                }
            })
            .collect()
    }

    /// Resolve a position (degrees) to a [`Place`]; `max_km` is the distance up to which the nearest place names the
    /// city (see the crate docs for the coarser answers beyond). `None` for an invalid position or one far from any place.
    pub fn lookup(&self, lat: f64, lon: f64, max_km: f64) -> Option<Place> {
        let (lat, lon) = valid_position(lat, lon)?;
        let max_km = if max_km.is_finite() { max_km.clamp(1.0, 500.0) } else { DEFAULT_MAX_KM };
        let tree = kd::Tree { points: &self.xyz };
        let q = kd::deg_to_xyz(lat, lon);
        let km = |i: usize| {
            let (la, lo) = self.position(i);
            distance_km(lat, lon, la, lo)
        };
        let any = tree.nearest(q, 1, &|_| true).first().map(|x| x.0)?;
        let d_any = km(any);
        // a town (not a quarter of one) wins when it is in range. Among the nearest towns, the biggest one whose built-up
        // area (a radius of 0.35 km per sqrt(thousand inhabitants), at most `max_km`) covers the position names it: a photo
        // at the Eiffel Tower is "Paris", not the nearest district listed as a place of its own.
        let towns = tree.nearest(q, 16, &|i| !self.section.get(i).copied().unwrap_or(false));
        let nearest_town = towns.first().map(|x| x.0).filter(|i| km(*i) <= max_km);
        let covering = towns
            .iter()
            .map(|x| x.0)
            .filter(|i| {
                let pop = f64::from(self.population.get(*i).copied().unwrap_or(0));
                km(*i) <= (0.35 * (pop / 1000.0).sqrt()).min(max_km)
            })
            .max_by_key(|i| (self.population.get(*i).copied().unwrap_or(0), std::cmp::Reverse(*i)));
        let town = covering.or(nearest_town);
        let (named, level) = match town {
            Some(t) => (t, Level::City),
            None if d_any <= max_km => (any, Level::City),
            None if d_any <= 2.0 * max_km => (any, Level::Region),
            None if d_any <= 6.0 * max_km => (any, Level::Country),
            None => return None,
        };
        let country = self.country_of(named)?;
        let region = self.admin_of(named).map(|a| a.name.clone()).filter(|n| !n.is_empty());
        Some(Place {
            country_code: country.code.clone(),
            country: country.name.clone(),
            region: if level >= Level::Region { region } else { None },
            city: if level == Level::City { Some(self.name_of(named).to_string()) } else { None },
            level,
            distance_km: km(named),
            population: self.population.get(named).copied().unwrap_or(0),
        })
    }
}

// ---- finding and caching the installed database

/// The folders searched for [`DB_FILE`], in order: `$LIGHTCRAFT_PLACES_DB` (file or folder), next to the executable
/// (`places/`, then the macOS bundle's `Resources/places/`, then `../share/lightcraft/places/`), then the user's data folder.
pub fn search_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os(DB_ENV).map(PathBuf::from) {
        v.push(if p.is_dir() { p.join(DB_FILE) } else { p });
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        v.push(dir.join("places").join(DB_FILE));
        v.push(dir.join(DB_FILE));
        v.push(dir.join("..").join("Resources").join("places").join(DB_FILE));
        v.push(dir.join("..").join("Resources").join(DB_FILE));
        v.push(dir.join("..").join("share").join("lightcraft").join("places").join(DB_FILE));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        v.push(home.join("Library").join("Application Support").join("LightCraft").join("places").join(DB_FILE));
        v.push(home.join(".local").join("share").join("lightcraft").join("places").join(DB_FILE));
    }
    v
}

struct Loaded {
    db: Option<Database>,
    path: Option<PathBuf>,
    error: Option<String>,
}

static DB: OnceLock<Loaded> = OnceLock::new();
static INSTALLED: OnceLock<Loaded> = OnceLock::new();

/// Install `db` as the database of this process (tests, or a host that carries its own copy), replacing whatever
/// the search found. `false` when one was already installed.
pub fn install(db: Database, path: Option<PathBuf>) -> bool {
    INSTALLED.set(Loaded { db: Some(db), path, error: None }).is_ok()
}

/// Install the database file at `path` (see [`install`]).
pub fn install_file(path: &std::path::Path) -> Result<bool, LoadError> {
    Ok(install(Database::load(path)?, Some(path.to_path_buf())))
}

fn loaded() -> &'static Loaded {
    if let Some(l) = INSTALLED.get() {
        return l;
    }
    DB.get_or_init(|| {
        let mut error = None;
        for p in search_paths() {
            if !p.is_file() {
                continue;
            }
            match Database::load(&p) {
                Ok(db) => return Loaded { db: Some(db), path: Some(p), error: None },
                Err(e) => error = Some(e.to_string()),
            }
        }
        Loaded { db: None, path: None, error }
    })
}

/// The installed database, loaded once; `None` when no database file is installed.
pub fn database() -> Option<&'static Database> {
    loaded().db.as_ref()
}

/// Where the installed database was found, or why none was loaded.
pub fn database_status() -> Result<(PathBuf, &'static Provenance, usize), String> {
    let l = loaded();
    match (&l.db, &l.path) {
        (Some(db), p) => Ok((p.clone().unwrap_or_else(|| PathBuf::from("(installed by the host)")), &db.provenance, db.len())),
        _ => Err(l.error.clone().unwrap_or_else(|| format!("no {DB_FILE} found (searched {} places; set {DB_ENV})", search_paths().len()))),
    }
}

type CacheKey = (i32, i32, u32);
static CACHE: Mutex<Option<HashMap<CacheKey, Option<Arc<Place>>>>> = Mutex::new(None);
/// Entries kept before the cache starts over.
const CACHE_MAX: usize = 200_000;

/// [`Database::lookup`] on the installed database, memoised on a ~11 m grid (a burst of photos, or the same
/// library listed again and again, costs one search per spot). `None` when no database is installed, the position
/// is invalid or no place is in range.
pub fn place_at(lat: f64, lon: f64, max_km: f64) -> Option<Arc<Place>> {
    let db = database()?;
    let (lat, lon) = valid_position(lat, lon)?;
    let key = ((lat * 1e4).round() as i32, (lon * 1e4).round() as i32, (max_km.clamp(1.0, 500.0) * 10.0).round() as u32);
    {
        let g = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hit) = g.as_ref().and_then(|m| m.get(&key)) {
            return hit.clone();
        }
    }
    let r = db.lookup(lat, lon, max_km).map(Arc::new);
    let mut g = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() >= CACHE_MAX {
        m.clear();
    }
    m.insert(key, r.clone());
    r
}

#[cfg(test)]
mod tests;
