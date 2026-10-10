//! The Places view and the Place picker of the filter bar, driven headless like a user: open the view, click a place,
//! preview and apply a geotag, name a place, confirm a suggestion. They need the engine's `places` feature and the
//! data file; without the feature they pass trivially. `LIGHTCRAFT_TEST_SHOTS=<folder>` also saves screenshots.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn setup() -> Option<(Headless, Vec<u64>)> {
    if !lightcraft_engine::places::AVAILABLE {
        eprintln!("(skipped: built without the engine's `places` feature)");
        return None;
    }
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("places").join("data").join("places-cities.bin");
    let _ = lightcraft_engine::places::install_database(&data);
    let mut s = lightcraft_engine::Session::with_demo();
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let ids: Vec<u64> = s.catalog.photos().filter(|p| p.in_library()).map(|p| p.id.0).take(8).collect();
    // Poitiers x3, Paris x2, Moroni x1 by GPS; one tagged by hand; one without any place
    for (i, (la, lo)) in [(46.5802, 0.3404), (46.60, 0.30), (46.55, 0.36), (48.8584, 2.2945), (48.86, 2.34), (-11.7022, 43.2551)].iter().enumerate() {
        s.execute("photo.setMeta", &json!({"ids": [ids[i]], "gps": [la, lo]})).unwrap();
    }
    s.execute("places.name", &json!({"ids": [ids[6]], "place": "Senegal|Dakar"})).unwrap();
    let app = LightcraftApp::new(
        s,
        Services {
            png: Some(Box::new(png)),
            write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
            ..Default::default()
        },
    );
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "places"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(60));
    Some((h, ids))
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn shot(h: &mut Headless, name: &str) {
    if let Some(dir) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        let r = h.request("ui.screenshot", json!({"path": Path::new(&dir).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

#[test]
fn the_view_lists_places_with_counts_and_a_click_filters_the_grid() {
    let Some((mut h, ids)) = setup() else { return };
    for w in ["place:Places|France", "place:Places|France|New Aquitaine|Poitiers", "place:Places|Comoros", "place:Places|Senegal|Dakar", "place:none"]
    {
        assert!(has(&h, w), "{w} in {:?}", h.app.widgets.iter().map(|w| &w.0).filter(|w| w.starts_with("place")).collect::<Vec<_>>());
    }
    shot(&mut h, "places_view.png");
    // three photos around Poitiers: the region holds them all, the city only the one in its centre
    click(&mut h, "place:Places|France|New Aquitaine");
    assert_eq!(h.app.session.filter.place.as_deref(), Some("Places|France|New Aquitaine"));
    assert_eq!(h.app.ui.view, crate::state::ViewMode::PhotoGrid, "the grid shows the place's photos");
    let mut vis: Vec<u64> = h.app.session.visible().iter().map(|i| i.0).collect();
    vis.sort();
    assert_eq!(vis, ids[..3].to_vec());
}

#[test]
fn the_filter_bar_has_a_place_picker_next_to_keywords() {
    let Some((mut h, _)) = setup() else { return };
    let r = h.request("ui.set", json!({"view": "photoGrid", "filterBar": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "library.filter", "params": {"place": "Places|France"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(60));
    assert_eq!(h.app.session.visible().len(), 5, "Poitiers x3 and Paris x2");
    // the bar shows the choice
    h.step();
    if has(&h, "dropdown:filter-place") {
        click(&mut h, "dropdown:filter-place");
        h.step();
        assert!(has(&h, "filteritem:place:Places|France|Île-de-France|Paris") || has(&h, "filteritem:place:all"), "the popup lists the places");
        shot(&mut h, "places_filterbar.png");
        if has(&h, "filteritem:place:none") {
            click(&mut h, "filteritem:place:none");
            assert_eq!(h.app.session.filter.place.as_deref(), Some("none"));
        }
    } else {
        panic!("no Place picker in the filter bar: {:?}", h.app.widgets.iter().map(|w| &w.0).filter(|w| w.contains("filter")).collect::<Vec<_>>());
    }
}

#[test]
fn geotag_preview_then_apply_and_naming_from_the_view() {
    let Some((mut h, ids)) = setup() else { return };
    click(&mut h, "button:placesPreview");
    assert!(h.app.caches.places.preview.as_ref().is_some_and(|p| p["wouldWrite"] == 6), "{:?}", h.app.caches.places.preview);
    assert!(
        h.app.session.catalog.photo(lightcraft_catalog::PhotoId(ids[0])).unwrap().meta.keywords.iter().all(|k| !k.starts_with("Places")),
        "preview writes nothing"
    );
    h.step();
    shot(&mut h, "places_preview.png");
    click(&mut h, "button:placesApply");
    let kw = &h.app.session.catalog.photo(lightcraft_catalog::PhotoId(ids[5])).unwrap().meta.keywords;
    assert!(kw.contains(&"Places|Comoros|Grande Comore|Moroni".to_string()), "{kw:?}");
    // naming for the selection
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[7]]}}), T);
    let _ = r;
    h.app.session.selection.ids = vec![lightcraft_catalog::PhotoId(ids[7])];
    h.app.session.selection.active = Some(lightcraft_catalog::PhotoId(ids[7]));
    h.app.caches.places.name = "Spain|Madrid".into();
    h.step();
    h.step();
    click(&mut h, "button:placesName");
    let kw = &h.app.session.catalog.photo(lightcraft_catalog::PhotoId(ids[7])).unwrap().meta.keywords;
    assert!(kw.contains(&"Places|Spain|Madrid".to_string()), "{kw:?}");
    // undo takes the naming back
    h.request("engine.execute", json!({"command": "edit.undo", "params": {}}), T);
    let kw = &h.app.session.catalog.photo(lightcraft_catalog::PhotoId(ids[7])).unwrap().meta.keywords;
    assert!(!kw.contains(&"Places|Spain|Madrid".to_string()));
}

#[test]
fn the_menu_has_places_under_view() {
    let Some((mut h, _)) = setup() else { return };
    let r = h.request("ui.menu.list", json!({}), T);
    let text = r.to_string();
    assert!(text.contains("view.places"), "{text}");
    let _: Value = r;
}
