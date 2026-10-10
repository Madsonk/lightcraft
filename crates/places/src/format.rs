//! The file format of the places database and its builder.
//!
//! ```text
//! "LCPL" | version u8 (1) | zlib( payload )
//! payload: u32 n, provenance JSON (n bytes)
//!          u32 countries { u8 code len, code, u16 name len, name }*
//!          u32 admins    { u16 country, u16 name len, name }*     (empty name = the country's places without a division)
//!          u32 cities, u32 names len, names (each followed by '\n')
//!          lat i32[cities] (1e-5 deg), lon i32[cities], admin u16[cities], population u32[cities], flags u8[cities] (bit 0 = quarter)
//! ```
//! All integers are little-endian. The cities are stored in k-d tree order (see `kd`), which is what the search needs;
//! the loader checks every index it will later use.

use std::collections::HashMap;
use std::path::Path;

use crate::kd;
use crate::{Admin, AdminRecord, CityRecord, Country, CountryRecord, Database, Provenance};

const MAGIC: &[u8; 4] = b"LCPL";
const VERSION: u8 = 1;
/// Largest decompressed payload accepted (the real one is a few MB).
const MAX_PAYLOAD: usize = 256 << 20;

/// Why a database could not be loaded or built.
#[derive(Debug)]
pub enum LoadError {
    Io(String),
    Format(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "places database: {e}"),
            LoadError::Format(e) => write!(f, "places database: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

fn bad(msg: impl Into<String>) -> LoadError {
    LoadError::Format(msg.into())
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LoadError> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("truncated"))?;
        let s = self.buf.get(self.pos..end).ok_or_else(|| bad("truncated"))?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, LoadError> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }
    fn u16(&mut self) -> Result<u16, LoadError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]))
    }
    fn u32(&mut self) -> Result<u32, LoadError> {
        let b: [u8; 4] = self.take(4)?.try_into().map_err(|_| bad("truncated"))?;
        Ok(u32::from_le_bytes(b))
    }
    fn string(&mut self, len: usize) -> Result<String, LoadError> {
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| bad("not UTF-8"))
    }
    fn column<const N: usize>(&mut self, n: usize) -> Result<Vec<[u8; N]>, LoadError> {
        let bytes = self.take(n.checked_mul(N).ok_or_else(|| bad("truncated"))?)?;
        Ok(bytes.as_chunks::<N>().0.to_vec())
    }
}

fn put_str(out: &mut Vec<u8>, s: &str, wide: bool) {
    if wide {
        out.extend_from_slice(&(s.len().min(u16::MAX as usize) as u16).to_le_bytes());
    } else {
        out.push(s.len().min(255) as u8);
    }
    let cap = if wide { u16::MAX as usize } else { 255 };
    let mut end = s.len().min(cap);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out.extend_from_slice(s.get(..end).unwrap_or("").as_bytes());
}

/// A name without line breaks or other control characters, trimmed.
fn clean_name(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl Database {
    /// Assemble a database from records, putting the places in k-d order. Admin divisions that no place uses are kept.
    pub fn build(
        provenance: Provenance,
        countries: Vec<CountryRecord>,
        admins: Vec<AdminRecord>,
        cities: Vec<CityRecord>,
    ) -> Result<Database, LoadError> {
        let mut country_ix: HashMap<String, u16> = HashMap::new();
        let mut table_c: Vec<Country> = Vec::new();
        for c in countries {
            if table_c.len() >= usize::from(u16::MAX) {
                return Err(bad("too many countries"));
            }
            country_ix.insert(c.code.clone(), table_c.len() as u16);
            table_c.push(Country { code: clean_name(&c.code), name: clean_name(&c.name) });
        }
        let mut admin_ix: HashMap<(u16, String), u16> = HashMap::new();
        let mut table_a: Vec<Admin> = Vec::new();
        fn add_admin(ix: &mut HashMap<(u16, String), u16>, table: &mut Vec<Admin>, country: u16, code: &str, name: String) -> Result<u16, LoadError> {
            if let Some(i) = ix.get(&(country, code.to_string())) {
                return Ok(*i);
            }
            let i = u16::try_from(table.len()).map_err(|_| bad("too many divisions"))?;
            ix.insert((country, code.to_string()), i);
            table.push(Admin { country, name });
            Ok(i)
        }
        for a in admins {
            if let Some(c) = country_ix.get(&a.country) {
                add_admin(&mut admin_ix, &mut table_a, *c, &a.code, clean_name(&a.name))?;
            }
        }
        struct Row {
            name: String,
            lat: i32,
            lon: i32,
            admin: u16,
            pop: u32,
            section: bool,
        }
        let mut rows: Vec<Row> = Vec::with_capacity(cities.len());
        for c in cities {
            let Some(country) = country_ix.get(&c.country).copied() else { continue };
            if !c.lat.is_finite() || !c.lon.is_finite() || c.lat.abs() > 90.0 || c.lon.abs() > 180.0 {
                continue;
            }
            let name = clean_name(&c.name);
            if name.is_empty() {
                continue;
            }
            let code = c.admin1.clone().unwrap_or_default();
            // a division the table does not know falls back to "no division"
            let admin = match admin_ix.get(&(country, code)) {
                Some(i) => *i,
                None => add_admin(&mut admin_ix, &mut table_a, country, "", String::new())?,
            };
            rows.push(Row {
                name,
                lat: (c.lat * 1e5).round() as i32,
                lon: (c.lon * 1e5).round() as i32,
                admin,
                pop: c.population,
                section: c.section,
            });
        }
        // a stable starting order, so the same input always gives the same file
        rows.sort_by(|a, b| (a.lat, a.lon, &a.name).cmp(&(b.lat, b.lon, &b.name)));
        kd::arrange(&mut rows, &|r: &Row| kd::to_xyz(r.lat, r.lon));
        let mut names = String::new();
        let mut name_start = Vec::with_capacity(rows.len() + 1);
        for r in &rows {
            name_start.push(u32::try_from(names.len()).map_err(|_| bad("names too long"))?);
            names.push_str(&r.name);
            names.push('\n');
        }
        name_start.push(u32::try_from(names.len()).map_err(|_| bad("names too long"))?);
        let xyz = rows.iter().map(|r| kd::to_xyz(r.lat, r.lon)).collect();
        Ok(Database {
            provenance,
            countries: table_c,
            admins: table_a,
            names,
            name_start,
            lat_e5: rows.iter().map(|r| r.lat).collect(),
            lon_e5: rows.iter().map(|r| r.lon).collect(),
            admin: rows.iter().map(|r| r.admin).collect(),
            population: rows.iter().map(|r| r.pop).collect(),
            section: rows.iter().map(|r| r.section).collect(),
            xyz,
        })
    }

    /// The file's bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, LoadError> {
        let mut p: Vec<u8> = Vec::new();
        let json = serde_json::to_vec(&self.provenance).map_err(|e| bad(e.to_string()))?;
        p.extend_from_slice(&(json.len() as u32).to_le_bytes());
        p.extend_from_slice(&json);
        p.extend_from_slice(&(self.countries.len() as u32).to_le_bytes());
        for c in &self.countries {
            put_str(&mut p, &c.code, false);
            put_str(&mut p, &c.name, true);
        }
        p.extend_from_slice(&(self.admins.len() as u32).to_le_bytes());
        for a in &self.admins {
            p.extend_from_slice(&a.country.to_le_bytes());
            put_str(&mut p, &a.name, true);
        }
        p.extend_from_slice(&(self.len() as u32).to_le_bytes());
        p.extend_from_slice(&(self.names.len() as u32).to_le_bytes());
        p.extend_from_slice(self.names.as_bytes());
        for v in &self.lat_e5 {
            p.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.lon_e5 {
            p.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.admin {
            p.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.population {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.extend(self.section.iter().map(|s| u8::from(*s)));
        let mut out = Vec::with_capacity(p.len() / 3);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&p, 9));
        Ok(out)
    }

    /// Read a database from the file's bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Database, LoadError> {
        if bytes.get(..4) != Some(&MAGIC[..]) {
            return Err(bad("not a places database (bad signature)"));
        }
        if bytes.get(4).copied() != Some(VERSION) {
            return Err(bad("unsupported places database version"));
        }
        let payload = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(bytes.get(5..).unwrap_or(&[]), MAX_PAYLOAD)
            .map_err(|e| bad(format!("not a zlib stream ({e:?})")))?;
        let mut r = Reader { buf: &payload, pos: 0 };
        let n = r.u32()? as usize;
        let provenance: Provenance = serde_json::from_slice(r.take(n)?).map_err(|e| bad(e.to_string()))?;
        let n = r.u32()? as usize;
        let mut countries = Vec::new();
        for _ in 0..n {
            let l = usize::from(r.u8()?);
            let code = r.string(l)?;
            let l = usize::from(r.u16()?);
            countries.push(Country { code, name: r.string(l)? });
        }
        let n = r.u32()? as usize;
        let mut admins = Vec::new();
        for _ in 0..n {
            let country = r.u16()?;
            if usize::from(country) >= countries.len() {
                return Err(bad("a division names a country that does not exist"));
            }
            let l = usize::from(r.u16()?);
            admins.push(Admin { country, name: r.string(l)? });
        }
        let cities = r.u32()? as usize;
        let l = r.u32()? as usize;
        let names = r.string(l)?;
        let lat: Vec<i32> = r.column::<4>(cities)?.into_iter().map(i32::from_le_bytes).collect();
        let lon: Vec<i32> = r.column::<4>(cities)?.into_iter().map(i32::from_le_bytes).collect();
        let admin: Vec<u16> = r.column::<2>(cities)?.into_iter().map(u16::from_le_bytes).collect();
        let population: Vec<u32> = r.column::<4>(cities)?.into_iter().map(u32::from_le_bytes).collect();
        let section: Vec<bool> = r.column::<1>(cities)?.into_iter().map(|b| u8::from_le_bytes(b) & 1 == 1).collect();
        if admin.iter().any(|a| usize::from(*a) >= admins.len()) {
            return Err(bad("a place names a division that does not exist"));
        }
        if lat.iter().any(|v| v.abs() > 9_000_000) || lon.iter().any(|v| v.abs() > 18_000_000) {
            return Err(bad("a position is out of range"));
        }
        let mut name_start = Vec::with_capacity(cities + 1);
        let mut at = 0u32;
        for part in names.split_inclusive('\n') {
            name_start.push(at);
            at = at.saturating_add(part.len() as u32);
        }
        if name_start.len() != cities || (!names.is_empty() && !names.ends_with('\n')) {
            return Err(bad("the name table does not match the places"));
        }
        name_start.push(names.len() as u32);
        let xyz = lat.iter().zip(&lon).map(|(a, o)| kd::to_xyz(*a, *o)).collect();
        Ok(Database { provenance, countries, admins, names, name_start, lat_e5: lat, lon_e5: lon, admin, population, section, xyz })
    }

    /// Read the database file at `path`.
    pub fn load(path: &Path) -> Result<Database, LoadError> {
        let bytes = std::fs::read(path).map_err(|e| LoadError::Io(format!("{}: {e}", path.display())))?;
        Database::from_bytes(&bytes)
    }

    /// Write the database file to `path`.
    pub fn save(&self, path: &Path) -> Result<(), LoadError> {
        std::fs::write(path, self.to_bytes()?).map_err(|e| LoadError::Io(format!("{}: {e}", path.display())))
    }
}
