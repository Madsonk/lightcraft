//! Places: preview before writing, apply and undo, the Place filter (by keyword and by GPS), naming, suggestions
//! from neighbours in time, settings that survive reopening a library, and catalogs written before places existed.
//! Uses the real GeoNames table in `crates/places/data`.

use std::path::Path;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;

fn install_data() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("places").join("data").join("places-cities.bin");
    let _ = lightcraft_places::install_file(&path);
    assert!(lightcraft_places::database().is_some(), "the places data file is at {}", path.display());
}

fn ids(s: &Session, n: usize) -> Vec<u64> {
    s.catalog.photos().filter(|p| p.in_library()).map(|p| p.id.0).take(n).collect()
}

fn set_gps(s: &mut Session, id: u64, lat: f64, lon: f64) {
    s.execute("photo.setMeta", &json!({"ids": [id], "gps": [lat, lon]})).unwrap();
}

fn keywords(s: &Session, id: u64) -> Vec<String> {
    s.catalog.photo(PhotoId(id)).unwrap().meta.keywords.clone()
}

fn place_kw(s: &Session, id: u64) -> Vec<String> {
    keywords(s, id).into_iter().filter(|k| k.starts_with("Places")).collect()
}

/// Four photos with GPS (Poitiers, Paris, Moroni, mid-Atlantic) and one without.
fn four() -> (Session, Vec<u64>) {
    install_data();
    let mut s = Session::with_demo();
    let v = ids(&s, 5);
    set_gps(&mut s, v[0], 46.5802, 0.3404);
    set_gps(&mut s, v[1], 48.8584, 2.2945);
    set_gps(&mut s, v[2], -11.7022, 43.2551);
    set_gps(&mut s, v[3], 30.0, -40.0);
    (s, v)
}

#[test]
fn geotag_previews_without_writing_then_applies_in_one_undo_step() {
    let (mut s, v) = four();
    let rev = s.catalog.revision;
    let undo = s.undo.len();
    let r = s.execute("places.geotag", &json!({"ids": v})).unwrap();
    assert_eq!(
        (r["scanned"].as_u64(), r["withoutGps"].as_u64(), r["resolved"].as_u64(), r["unresolved"].as_u64()),
        (Some(5), Some(1), Some(3), Some(1)),
        "{r}"
    );
    assert_eq!(r["wouldWrite"], 3);
    assert_eq!(r["apply"], false);
    let found: Vec<&str> = r["photos"].as_array().unwrap().iter().filter_map(|p| p["place"].as_str()).collect();
    assert!(found.contains(&"Places|France|New Aquitaine|Poitiers") && found.contains(&"Places|Comoros|Grande Comore|Moroni"), "{found:?}");
    assert_eq!((s.catalog.revision, s.undo.len()), (rev, undo), "a preview changes nothing");
    assert!(v.iter().all(|i| place_kw(&s, *i).is_empty()));

    let r = s.execute("places.geotag", &json!({"ids": v, "apply": true})).unwrap();
    assert_eq!(r["written"], 3);
    assert_eq!(place_kw(&s, v[0]), vec!["Places|France|New Aquitaine|Poitiers"]);
    assert_eq!(place_kw(&s, v[2]), vec!["Places|Comoros|Grande Comore|Moroni"]);
    assert!(place_kw(&s, v[3]).is_empty() && place_kw(&s, v[4]).is_empty());
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    // a second run finds them placed
    let again = s.execute("places.geotag", &json!({"ids": v})).unwrap();
    assert_eq!((again["alreadyPlaced"].as_u64(), again["resolved"].as_u64()), (Some(3), Some(0)));
    // undo takes it all back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(v.iter().all(|i| place_kw(&s, *i).is_empty()));
    assert!(s.execute("places.geotag", &json!({"ids": v, "maxKm": 0})).is_err(), "maxKm is checked");
}

#[test]
fn place_filter_matches_keywords_and_gps_and_lists_counts() {
    let (mut s, v) = four();
    // GPS only: France is v0 and v1 before anything is written
    s.execute("library.filter", &json!({"place": "Places|France"})).unwrap();
    let mut got: Vec<u64> = s.visible().iter().map(|i| i.0).collect();
    got.sort();
    let mut want = vec![v[0], v[1]];
    want.sort();
    assert_eq!(got, want);
    s.execute("library.filter", &json!({"place": "Places|France|New Aquitaine"})).unwrap();
    assert_eq!(s.visible().iter().map(|i| i.0).collect::<Vec<_>>(), vec![v[0]]);
    // a keyword the user chose wins over the GPS
    s.execute("places.name", &json!({"ids": [v[1]], "place": "Italy|Lazio|Rome"})).unwrap();
    assert_eq!(place_kw(&s, v[1]), vec!["Places|Italy|Lazio|Rome"]);
    s.execute("library.filter", &json!({"place": "Places|France"})).unwrap();
    assert_eq!(s.visible().iter().map(|i| i.0).collect::<Vec<_>>(), vec![v[0]]);
    let list = s.execute("places.list", &json!({})).unwrap();
    let paths: Vec<&str> = list["places"].as_array().unwrap().iter().filter_map(|p| p["path"].as_str()).collect();
    assert!(paths.contains(&"Places|France|New Aquitaine|Poitiers") && paths.contains(&"Places|Italy|Lazio|Rome"), "{paths:?}");
    assert_eq!(list["fromGps"].as_u64(), Some(2), "Poitiers and Moroni are placed by their GPS only: {list}");
    assert_eq!(list["gpsUntagged"].as_u64(), Some(3));
    // "none" = no place at all
    s.execute("library.filter", &json!({"place": "none"})).unwrap();
    let none: Vec<u64> = s.visible().iter().map(|i| i.0).collect();
    assert!(none.contains(&v[3]) && none.contains(&v[4]) && !none.contains(&v[0]), "{none:?}");
    // the filter shows as a chip and clears
    let chips = crate::filter_chips(&s.filter, &s.catalog);
    assert!(chips.iter().any(|c| c.label == "No place"), "{:?}", chips.iter().map(|c| &c.label).collect::<Vec<_>>());
    s.execute("library.filter", &json!({"place": Value::Null})).unwrap();
    assert!(s.filter.place.is_none());
}

#[test]
fn name_clear_confirm_and_dismiss() {
    let (mut s, v) = four();
    s.execute("places.name", &json!({"ids": [v[4]], "place": "Places|Senegal|Dakar"})).unwrap();
    assert_eq!(place_kw(&s, v[4]), vec!["Places|Senegal|Dakar"]);
    s.execute("places.name", &json!({"ids": [v[4]], "place": "France|Poitiers"})).unwrap();
    assert_eq!(place_kw(&s, v[4]), vec!["Places|France|Poitiers"], "a photo has one place");
    assert!(s.execute("places.name", &json!({"ids": [v[4]], "place": "  | "})).is_err());
    s.execute("places.clear", &json!({"ids": [v[4]]})).unwrap();
    assert!(place_kw(&s, v[4]).is_empty());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(place_kw(&s, v[4]), vec!["Places|France|Poitiers"]);
    // other keywords are kept
    s.execute("photo.setMeta", &json!({"ids": [v[4]], "addKeywords": ["beach"]})).unwrap();
    s.execute("places.name", &json!({"ids": [v[4]], "place": "Comoros|Moroni"})).unwrap();
    assert!(keywords(&s, v[4]).contains(&"beach".to_string()));
}

#[test]
fn suggestions_from_neighbours_in_time_need_the_same_camera_and_a_confirmation() {
    let (mut s, v) = four();
    for (i, t) in [(0, "2026-05-01T10:00:00"), (4, "2026-05-01T10:12:00"), (1, "2026-05-01T18:00:00"), (2, "2026-05-02T10:00:00")] {
        s.execute("photo.setCaptureTime", &json!({"ids": [v[i]], "time": t, "each": true})).unwrap();
    }
    // the same camera for the Poitiers photo and the one without GPS; the others differ
    let cam = s.catalog.photo(PhotoId(v[0])).unwrap().meta.camera.clone();
    assert!(!cam.is_empty());
    {
        let mut m = s.catalog.photo(PhotoId(v[4])).unwrap().meta.clone();
        m.camera = cam.clone();
        s.commit("cam", lightcraft_catalog::Op::SetMeta { id: PhotoId(v[4]), meta: Box::new(m) }).unwrap();
    }
    let r = s.execute("places.suggest", &json!({"ids": [v[4]], "methods": ["time"]})).unwrap();
    let sg = &r["suggestions"].as_array().unwrap()[0];
    assert_eq!(sg["place"], "Places|France|New Aquitaine|Poitiers", "{r}");
    assert_eq!(sg["source"], "time");
    let c = sg["confidence"].as_f64().unwrap();
    assert!(c > 0.4 && c < 0.6, "12 minutes of a 30 minute window: {c}");
    assert!(place_kw(&s, v[4]).is_empty(), "a suggestion writes nothing");
    // outside the window: nothing
    let r = s.execute("places.suggest", &json!({"ids": [v[4]], "methods": ["time"], "windowMinutes": 5})).unwrap();
    assert_eq!(r["suggested"], 0, "{r}");
    // confirm writes the remembered suggestion
    s.execute("places.suggest", &json!({"ids": [v[4]], "methods": ["time"]})).unwrap();
    let c = s.execute("places.confirm", &json!({"ids": [v[4]]})).unwrap();
    assert_eq!(c["changed"], 1);
    assert_eq!(place_kw(&s, v[4]), vec!["Places|France|New Aquitaine|Poitiers"]);
    // dismissed photos are not proposed again this session
    let (mut s, v) = four();
    s.execute("photo.setCaptureTime", &json!({"ids": [v[0], v[4]], "time": "2026-05-01T10:00:00", "each": true})).unwrap();
    let cam = s.catalog.photo(PhotoId(v[0])).unwrap().meta.camera.clone();
    let mut m = s.catalog.photo(PhotoId(v[4])).unwrap().meta.clone();
    m.camera = cam;
    s.commit("cam", lightcraft_catalog::Op::SetMeta { id: PhotoId(v[4]), meta: Box::new(m) }).unwrap();
    s.execute("places.dismiss", &json!({"ids": [v[4]]})).unwrap();
    assert_eq!(s.execute("places.suggest", &json!({"ids": [v[4]], "methods": ["time"]})).unwrap()["suggested"], 0);
    // visual suggestions without the model say so instead of failing
    let r = s.execute("places.suggest", &json!({"ids": [v[4]], "methods": ["visual"]})).unwrap();
    assert!(r["notes"].as_array().is_some_and(|n| !n.is_empty()), "{r}");
}

#[test]
fn lookup_and_status() {
    let (mut s, _) = four();
    let r = s.execute("places.lookup", &json!({"lat": 14.6928, "lon": -17.4467})).unwrap();
    assert_eq!((r["found"].as_bool(), r["keyword"].as_str(), r["level"].as_str()), (Some(true), Some("Places|Senegal|Dakar"), Some("city")), "{r}");
    let r = s.execute("places.lookup", &json!({"lat": 30.0, "lon": -40.0})).unwrap();
    assert_eq!(r["found"], false);
    assert!(s.execute("places.lookup", &json!({"lat": 95.0, "lon": 0.0})).is_err());
    assert!(s.execute("places.lookup", &json!({"lat": 1.0})).is_err());
    let st = s.execute("places.status", &json!({})).unwrap();
    assert_eq!(st["data"]["installed"], true);
    assert_eq!(st["settings"]["autoGeotag"], false, "off by default");
    assert!(st["data"]["provenance"]["licence"].as_str().unwrap().contains("CC BY 4.0"));
}

#[test]
fn auto_geotag_is_off_by_default_persists_with_the_library_and_tags_new_photos() {
    install_data();
    let dir = std::env::temp_dir().join(format!("lc-places-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new();
    s.open_library(&dir, false).unwrap();
    assert!(!s.places_settings().auto_geotag);
    // off: a new photo gets nothing
    let mut m = lightcraft_catalog::Meta { gps: Some((46.5802, 0.3404)), ..Default::default() };
    crate::places::tag_new_photo(&mut s, &mut m);
    assert!(m.keywords.is_empty());
    s.execute("places.settings", &json!({"autoGeotag": true, "maxKm": 80})).unwrap();
    crate::places::tag_new_photo(&mut s, &mut m);
    assert_eq!(m.keywords, vec!["Places|France|New Aquitaine|Poitiers"]);
    // never twice, never without GPS
    crate::places::tag_new_photo(&mut s, &mut m);
    assert_eq!(m.keywords.len(), 1);
    let mut none = lightcraft_catalog::Meta::default();
    crate::places::tag_new_photo(&mut s, &mut none);
    assert!(none.keywords.is_empty());
    drop(s);
    let mut s2 = Session::new();
    s2.open_library(&dir, false).unwrap();
    let st = s2.places_settings();
    assert!(st.auto_geotag && (st.max_km - 80.0).abs() < 1e-9, "{st:?}");
    assert!(s2.execute("places.settings", &json!({"maxKm": 9999})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_catalog_and_filters_from_before_places_load_unchanged() {
    // an old saved filter (no `place`) and a smart album's rules
    let f: lightcraft_catalog::Filter = serde_json::from_value(json!({"rating": 3, "untagged": true, "keyword": "x"})).unwrap();
    assert_eq!(f.place, None);
    let v = serde_json::to_value(&f).unwrap();
    assert!(v.get("place").is_none(), "{v}");
    // a catalog written by this build without any place data reads back identically
    let mut s = Session::with_demo();
    let before = serde_json::to_string(&s.catalog.photos().map(|p| p.meta.clone()).collect::<Vec<_>>()).unwrap();
    s.execute("places.list", &json!({})).unwrap();
    let after = serde_json::to_string(&s.catalog.photos().map(|p| p.meta.clone()).collect::<Vec<_>>()).unwrap();
    assert_eq!(before, after);
}
