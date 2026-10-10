//! Build `places-cities.bin` from GeoNames dump files (CC BY 4.0, <https://download.geonames.org/export/dump/>).
//!
//! `cargo run --release -p lightcraft-places --example build_db -- <citiesN.txt> <countryInfo.txt> <admin1CodesASCII.txt> <out.bin> [retrieved-date] [min-population]`
//!
//! `citiesN.txt` is the text file inside `citiesN.zip` (unzip it first). The output is a derivative of those files: keep
//! the attribution and the CC BY 4.0 notice with it (see `data/README.md`). This tool reads GeoNames' tab-separated data
//! format only.

use std::path::Path;

use lightcraft_places::{AdminRecord, CityRecord, CountryRecord, Database, Provenance};

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(Path::new(path)).map_err(|e| format!("{path}: {e}"))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [cities_path, country_path, admin_path, out] = [0, 1, 2, 3].map(|i| args.get(i).cloned().unwrap_or_default());
    if out.is_empty() {
        return Err("usage: build_db <citiesN.txt> <countryInfo.txt> <admin1CodesASCII.txt> <out.bin> [retrieved-date] [min-population]".into());
    }
    let retrieved = args.get(4).cloned().unwrap_or_else(|| "unknown".into());
    let min_pop: u32 = args.get(5).and_then(|v| v.parse().ok()).unwrap_or(1000);

    // countryInfo.txt: ISO, ISO3, ISO-Numeric, fips, Country, ...
    let mut countries = Vec::new();
    for line in read(&country_path)?.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if let (Some(code), Some(name)) = (f.first(), f.get(4)) {
            countries.push(CountryRecord { code: code.to_string(), name: name.to_string() });
        }
    }
    // admin1CodesASCII.txt: <country>.<admin1>, name, ascii name, geonameid (the UTF-8 name is used)
    let mut admins = Vec::new();
    for line in read(&admin_path)?.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let Some((country, code)) = f.first().and_then(|k| k.split_once('.')) else { continue };
        let Some(name) = f.get(1).filter(|n| !n.trim().is_empty()) else { continue };
        admins.push(AdminRecord { country: country.to_string(), code: code.to_string(), name: name.to_string() });
    }
    // cities: 0 id, 1 name, 2 ascii, 3 alternates, 4 lat, 5 lon, 6 class, 7 feature code, 8 country, 9 cc2, 10 admin1, ..., 14 population
    let mut cities = Vec::new();
    for line in read(&cities_path)?.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(name), Some(lat), Some(lon), Some(code), Some(country)) =
            (f.get(1), f.get(4).and_then(|v| v.parse::<f64>().ok()), f.get(5).and_then(|v| v.parse::<f64>().ok()), f.get(7), f.get(8))
        else {
            continue;
        };
        let pop = f.get(14).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        cities.push(CityRecord {
            name: name.to_string(),
            lat,
            lon,
            country: country.to_string(),
            admin1: f.get(10).map(|a| a.to_string()).filter(|a| !a.is_empty()),
            population: pop,
            section: *code == "PPLX",
        });
    }
    let n_in = cities.len();
    let provenance = Provenance {
        name: "GeoNames gazetteer".into(),
        licence: "Creative Commons Attribution 4.0 International (CC BY 4.0)".into(),
        url: "https://www.geonames.org/".into(),
        files: format!(
            "cities{min_pop}.txt, countryInfo.txt, admin1CodesASCII.txt from https://download.geonames.org/export/dump/ ({n_in} populated places with at least {min_pop} inhabitants)"
        ),
        retrieved,
        changes: "tab-separated text converted to a compact binary table in k-d tree order; kept per place: name (UTF-8), latitude and longitude (1e-5 deg), country, first-level division, population, quarter flag; alternate names, elevation, time zones and all other columns dropped".into(),
        attribution: "Place data from GeoNames (https://www.geonames.org/), CC BY 4.0, converted by LightCraft (see changes in the file).".into(),
    };
    let db = Database::build(provenance, countries, admins, cities).map_err(|e| e.to_string())?;
    db.save(Path::new(&out)).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("{out}: {} places from {n_in} rows, {} countries, {size} bytes", db.len(), db.country_count());
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
