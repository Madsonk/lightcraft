//! Recognition against the real profile database shipped in `data/` (Canon bodies and lenses the author owns).

use lightcraft_lens::{Confidence, Database, Query};

fn db() -> Database {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join(lightcraft_lens::DB_FILE);
    Database::load(&path).expect("database loads")
}

fn canon(lens: &str, spec: Option<[f64; 4]>, max_ap: Option<f64>, focal: f64, ap: f64) -> Query {
    Query {
        camera_make: "Canon".into(),
        camera_model: "Canon EOS 5D Mark III".into(),
        lens_model: lens.into(),
        lens_spec: spec,
        lens_max_aperture: max_ap,
        focal: Some(focal),
        aperture: Some(ap),
        ..Default::default()
    }
}

#[test]
fn provenance_and_size() {
    let d = db();
    assert_eq!(d.format, 1);
    assert!(d.provenance.licence.contains("CC BY-SA 3.0"));
    assert!(d.lenses.len() > 1000 && d.cameras.len() > 500);
}

#[test]
fn canon_24_105_by_name() {
    let r = db().resolve(&canon("EF24-105mm f/4L IS USM", Some([24.0, 105.0, 4.0, 4.0]), Some(4.0), 24.0, 4.0));
    let d = &r.detection;
    assert!(d.camera_known);
    assert_eq!(d.crop_factor, Some(1.0));
    assert_eq!(d.lens.as_deref(), Some("Canon EF 24-105mm f/4L IS USM"), "{d:?}");
    assert_eq!(d.confidence, Confidence::Exact);
    assert!(d.distortion && d.vignetting, "{d:?}");
    let c = r.correction.expect("correction");
    // at 24 mm the lens is barrel distorted: the centre is magnified relative to the corner (ratio above 1)
    assert!(c.ratio(1, 0.0) > 1.0, "{}", c.ratio(1, 0.0));
    assert!(c.gain(1.0) > 1.3, "corner gain at f/4 24 mm: {}", c.gain(1.0));
}

#[test]
fn bare_focal_name_uses_range_and_widest_aperture() {
    let r = db().resolve(&canon("50mm", Some([50.0, 50.0, 1.8, 1.8]), Some(1.8), 50.0, 4.5));
    let d = &r.detection;
    println!("{d:?}");
    assert!(d.lens.as_deref().is_some_and(|l| l.contains("50mm f/1.8")), "{d:?}");
    assert!(matches!(d.confidence, Confidence::Probable | Confidence::Ambiguous), "{d:?}");
    assert!(r.correction.is_some(), "{d:?}");
}

#[test]
fn old_zoom() {
    let r = db().resolve(&canon("EF70-210mm f/3.5-4.5 USM", Some([70.0, 210.0, 3.5, 4.5]), Some(3.5), 120.0, 4.0));
    println!("{:?}", r.detection);
    assert!(r.detection.lens.as_deref().is_some_and(|l| l.contains("70-210")), "{:?}", r.detection);
}

#[test]
fn unknown_lens_gets_no_invented_profile() {
    let mut q = canon("Zorg 77mm f/0.7 Imaginary", None, None, 77.0, 1.0);
    q.lens_spec = None;
    let r = db().resolve(&q);
    assert!(r.correction.is_none());
    assert_eq!(r.detection.confidence, Confidence::None);
    assert!(!r.detection.note.is_empty());
}

#[test]
fn nothing_to_go_on() {
    let r = db().resolve(&Query { camera_make: "Canon".into(), camera_model: "Canon EOS 5D Mark III".into(), ..Default::default() });
    assert!(r.correction.is_none());
    assert_eq!(r.detection.note, "the photo names no lens");
}
