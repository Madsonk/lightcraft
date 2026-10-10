use super::*;

fn city(name: &str, lat: f64, lon: f64, country: &str, admin: Option<&str>, pop: u32, section: bool) -> CityRecord {
    CityRecord { name: name.into(), lat, lon, country: country.into(), admin1: admin.map(str::to_string), population: pop, section }
}

fn tiny() -> Database {
    let countries = vec![
        CountryRecord { code: "FR".into(), name: "France".into() },
        CountryRecord { code: "KM".into(), name: "Comoros".into() },
        CountryRecord { code: "RE".into(), name: "Reunion".into() },
        CountryRecord { code: "FJ".into(), name: "Fiji".into() },
        CountryRecord { code: "SN".into(), name: "Senegal".into() },
    ];
    let admins = vec![
        AdminRecord { country: "FR".into(), code: "75".into(), name: "New Aquitaine".into() },
        AdminRecord { country: "FR".into(), code: "11".into(), name: "Ile-de-France".into() },
        AdminRecord { country: "KM".into(), code: "02".into(), name: "Grande Comore".into() },
        AdminRecord { country: "RE".into(), code: "RE".into(), name: "Reunion".into() },
        AdminRecord { country: "SN".into(), code: "01".into(), name: "Dakar".into() },
    ];
    let cities = vec![
        city("Poitiers", 46.58261, 0.34348, "FR", Some("75"), 88291, false),
        city("Paris", 48.85341, 2.3488, "FR", Some("11"), 2138551, false),
        city("Quartier des Halles", 48.8625, 2.3469, "FR", Some("11"), 0, true),
        city("Moroni", -11.70216, 43.25506, "KM", Some("02"), 74749, false),
        city("Saint-Denis", -20.88231, 55.4504, "RE", Some("RE"), 154765, false),
        city("Dakar", 14.6937, -17.44406, "SN", Some("01"), 2646503, false),
        city("Somosomo", -16.7683, -179.9500, "FJ", None, 1200, false),
        city("Labasa", -16.4333, 179.3667, "FJ", None, 27949, false),
        city("Nowhere Special", 10.0, 10.0, "FR", Some("zz"), 10, false),
    ];
    Database::build(Provenance { name: "test".into(), ..Default::default() }, countries, admins, cities).unwrap()
}

#[test]
fn resolves_known_places_and_builds_the_keyword() {
    let db = tiny();
    let p = db.lookup(46.5802, 0.3404, DEFAULT_MAX_KM).unwrap();
    assert_eq!(
        (p.level, p.city.as_deref(), p.region.as_deref(), p.country.as_str()),
        (Level::City, Some("Poitiers"), Some("New Aquitaine"), "France")
    );
    assert_eq!(p.keyword(), "Places|France|New Aquitaine|Poitiers");
    assert!(p.distance_km < 1.0, "{}", p.distance_km);
    // a district of a larger city does not name a photo taken in it when the city itself is in range
    let paris = db.lookup(48.8626, 2.3468, DEFAULT_MAX_KM).unwrap();
    assert_eq!(paris.city.as_deref(), Some("Paris"), "{paris:?}");
    // the Comoros, La Reunion: a level that repeats the one before is dropped
    assert_eq!(db.lookup(-11.7, 43.25, 50.0).unwrap().keyword(), "Places|Comoros|Grande Comore|Moroni");
    assert_eq!(db.lookup(-20.88, 55.45, 50.0).unwrap().keyword(), "Places|Reunion|Saint-Denis");
    assert_eq!(db.lookup(14.7, -17.45, 50.0).unwrap().keyword(), "Places|Senegal|Dakar");
    // a division the table lacks leaves it out
    assert_eq!(db.lookup(10.0, 10.0, 50.0).unwrap().keyword(), "Places|France|Nowhere Special");
}

#[test]
fn precision_falls_back_with_distance_and_gives_up_at_sea() {
    let db = tiny();
    // ~80 km east of Poitiers: region; ~200 km: country only; open sea: nothing
    let region = db.lookup(46.58, 0.34 + 80.0 / 77.0, 50.0).unwrap();
    assert_eq!((region.level, region.city.clone()), (Level::Region, None), "{region:?}");
    assert_eq!(region.keyword(), "Places|France|New Aquitaine");
    let country = db.lookup(46.58, 0.34 + 200.0 / 77.0, 50.0).unwrap();
    assert_eq!((country.level, country.keyword()), (Level::Country, "Places|France".to_string()));
    assert!(db.lookup(30.0, -40.0, 50.0).is_none(), "mid-Atlantic");
    // a wider radius reaches further, a narrower one gives a coarser answer
    assert_eq!(db.lookup(46.58, 0.34 + 80.0 / 77.0, 100.0).unwrap().level, Level::City);
    assert_eq!(db.lookup(46.58, 0.34 + 20.0 / 77.0, 10.0).unwrap().level, Level::Region);
}

#[test]
fn poles_antimeridian_and_bad_input() {
    let db = tiny();
    assert!(db.lookup(90.0, 0.0, 50.0).is_none() && db.lookup(-90.0, 123.0, 50.0).is_none());
    // either side of the 180th meridian finds Fiji
    for lon in [180.0, -180.0, 179.9, -179.9, 540.0 - 360.0] {
        let p = db.lookup(-16.6, lon, 100.0).unwrap_or_else(|| panic!("no place at {lon}"));
        assert_eq!(p.country_code, "FJ", "{lon}: {p:?}");
    }
    assert_eq!(db.lookup(-16.77, 180.0 + 0.05, 50.0).unwrap().city.as_deref(), Some("Somosomo"));
    for (la, lo) in [(f64::NAN, 0.0), (0.0, f64::INFINITY), (91.0, 0.0), (0.0, 721.0), (0.0, 0.0)] {
        assert!(db.lookup(la, lo, 50.0).is_none(), "{la} {lo}");
    }
    assert!(db.lookup(46.58, 0.34, f64::NAN).is_some());
}

#[test]
fn file_round_trip_and_hostile_files() {
    let db = tiny();
    let bytes = db.to_bytes().unwrap();
    let back = Database::from_bytes(&bytes).unwrap();
    assert_eq!(back.len(), db.len());
    assert_eq!(back.lookup(14.7, -17.45, 50.0), db.lookup(14.7, -17.45, 50.0));
    assert_eq!(back.to_bytes().unwrap(), bytes, "the file is reproducible");
    // every truncation, and flipped bytes, are errors or a different database, never a panic
    for cut in (0..bytes.len()).step_by(7) {
        let _ = Database::from_bytes(&bytes[..cut]);
    }
    for at in (0..bytes.len()).step_by(5) {
        let mut b = bytes.clone();
        b[at] ^= 0x5a;
        if let Ok(d) = Database::from_bytes(&b) {
            let _ = d.lookup(46.58, 0.34, 50.0);
        }
    }
    assert!(Database::from_bytes(b"nope").is_err() && Database::from_bytes(&[]).is_err());
}

#[test]
fn keyword_parts_never_contain_the_separator() {
    assert_eq!(keyword_part("  A|B,  C "), "A B C");
    let p = Place {
        country_code: "XX".into(),
        country: "Land|of, Oz".into(),
        region: Some("land of oz".into()),
        city: Some("Emerald City".into()),
        level: Level::City,
        distance_km: 1.0,
        population: 0,
    };
    // the region repeats the country once cleaned (case-insensitive), so it is dropped
    assert_eq!(p.keyword(), "Places|Land of Oz|Emerald City");
    assert!((distance_km(0.0, 0.0, 0.0, 1.0) - 111.19).abs() < 0.05);
}

/// The shipped data file, when present (it is delivered next to the code, not inside the patch).
fn real() -> Option<Database> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join(DB_FILE);
    if !path.is_file() {
        eprintln!("(skipped: {} is not there)", path.display());
        return None;
    }
    Some(Database::load(&path).unwrap())
}

#[test]
fn the_real_database_places_known_cities() {
    let Some(db) = real() else { return };
    assert!(db.len() > 100_000, "{} places", db.len());
    let kw = |lat: f64, lon: f64| db.lookup(lat, lon, DEFAULT_MAX_KM).map(|p| p.keyword());
    assert_eq!(kw(48.8584, 2.2945).as_deref(), Some("Places|France|Île-de-France|Paris"), "Eiffel Tower");
    assert_eq!(kw(46.5802, 0.3404).as_deref(), Some("Places|France|New Aquitaine|Poitiers"));
    assert_eq!(kw(-11.7022, 43.2551).as_deref(), Some("Places|Comoros|Grande Comore|Moroni"));
    assert_eq!(kw(14.6928, -17.4467).as_deref(), Some("Places|Senegal|Dakar"));
    assert_eq!(kw(-20.8789, 55.4481).as_deref(), Some("Places|Reunion|Saint-Denis"));
    assert!(kw(-17.7134, 178.065).is_some_and(|k| k.starts_with("Places|Fiji")), "Fiji at 178 E");
    assert!(kw(-16.8, 179.99).is_some_and(|k| k.starts_with("Places|Fiji")), "Taveuni side of the antimeridian");
    assert!(kw(-16.8, -179.99).is_some_and(|k| k.starts_with("Places|Fiji")), "and the other side");
    assert_eq!(kw(30.0, -40.0), None, "mid-Atlantic");
    assert_eq!(kw(90.0, 0.0), None, "North Pole");
    assert_eq!(kw(-90.0, 0.0), None, "South Pole");
    // the sea off the coast of Brittany: far enough from any place for country only or nothing
    let sea = db.lookup(47.5, -6.0, DEFAULT_MAX_KM);
    assert!(sea.as_ref().is_none_or(|p| p.level != Level::City), "{sea:?}");
}

#[test]
fn the_real_database_matches_a_brute_force_search() {
    let Some(db) = real() else { return };
    let mut seed = 99u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as f64) / ((1u64 << 31) as f64)
    };
    for _ in 0..300 {
        let (la, lo) = (next() * 180.0 - 90.0, next() * 360.0 - 180.0);
        let (la, lo) = if la == 0.0 && lo == 0.0 { (1.0, 1.0) } else { (la, lo) };
        let got = db.nearby(la, lo, 1);
        let brute = (0..db.len())
            .map(|i| {
                let (a, o) = db.position(i);
                distance_km(la, lo, a, o)
            })
            .fold(f64::INFINITY, f64::min);
        assert!((got[0].distance_km - brute).abs() < 0.02, "({la}, {lo}): {} vs {brute}", got[0].distance_km);
    }
}

/// `cargo test --release -p lightcraft-places -- --ignored --nocapture`: how fast uncached lookups are.
#[test]
#[ignore]
fn bench_lookups() {
    let Some(db) = real() else { return };
    let t = std::time::Instant::now();
    load_time(&t);
    let mut seed = 5u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as f64) / ((1u64 << 31) as f64)
    };
    let pts: Vec<(f64, f64)> = (0..200_000).map(|_| (next() * 150.0 - 60.0, next() * 360.0 - 180.0)).collect();
    let t = std::time::Instant::now();
    let found = pts.iter().filter(|(a, o)| db.lookup(*a, *o, 50.0).is_some()).count();
    let s = t.elapsed().as_secs_f64();
    println!("{} uncached lookups in {s:.3} s = {:.0} per second ({found} found)", pts.len(), pts.len() as f64 / s);
}

fn load_time(_: &std::time::Instant) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join(DB_FILE);
    let t = std::time::Instant::now();
    let _ = Database::load(&path).unwrap();
    println!("load: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
}
