//! Lens recognition and the profile corrections end to end: a photo whose metadata names a Canon body and lens gets
//! the lens database's profile, switched on by default for raws, rendered like any other lens correction (a
//! synthetic grid, bent by the lens model, comes out straight), and never invented when the lens is unknown.

use lightcraft_catalog::{MediaKind, Photo, PhotoId, Source};
use lightcraft_develop::{DevelopSettings, EmbeddedLens};
use lightcraft_pipeline::{RenderRequest, SourceInfo};
use lightcraft_raster::{Rgb32f, Rgba8};

fn install_db() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("lens").join("data").join(lightcraft_lens::DB_FILE);
    let _ = lightcraft_lens::install_file(&p);
    assert!(lightcraft_lens::database().is_some(), "the lens database in crates/lens/data loads");
}

fn canon(lens: &str, focal: f32, aperture: f32, w: u32, h: u32) -> Photo {
    let mut p = Photo::new(PhotoId(1), Source::File { path: "IMG_0001.CR2".into() }, "IMG_0001.CR2", "CR2", w, h, "");
    p.kind = MediaKind::Raw;
    p.meta.camera = "Canon Canon EOS 5D Mark III".into();
    p.meta.lens = lens.into();
    p.meta.focal_mm = Some(focal);
    p.meta.aperture = Some(aperture);
    p
}

#[test]
fn a_recognised_lens_gets_its_profile_and_it_is_on_by_default_for_raws() {
    install_db();
    let p = canon("EF24-105mm f/4L IS USM", 24.0, 4.0, 5760, 3840);
    let r = p.database_lens().expect("lookup");
    assert_eq!(r.detection.lens.as_deref(), Some("Canon EF 24-105mm f/4L IS USM"));
    assert!(r.detection.distortion && r.detection.vignetting && r.detection.lateral_ca, "{:?}", r.detection);
    assert!(p.effective_lens().is_some_and(|l| l.warp.is_some() && l.vignette.is_some()));
    assert!(p.camera_defaults().optics.lens_profile, "profile corrections are on by default when a raw has a profile");
    // a JPEG of the same lens is recognised too, but nothing is switched on behind the user's back
    let mut j = p.clone();
    j.kind = MediaKind::Image;
    assert!(j.effective_lens().is_some());
    assert!(!j.camera_defaults().optics.lens_profile);
    // corrections the file itself embeds (DNG opcodes) win over the database
    let mut d = p.clone();
    let own = EmbeddedLens {
        warp: None,
        vignette: Some(lightcraft_develop::EmbeddedVignette {
            k: [0.5, 0.0, 0.0, 0.0, 0.0],
            center: lightcraft_geom::Point::new(0.5, 0.5),
            radius: 1.0,
        }),
    };
    d.embedded_lens = Some(own);
    assert_eq!(d.effective_lens(), Some(own));
}

#[test]
fn an_unknown_lens_gets_no_profile_and_the_reason_is_given() {
    install_db();
    let p = canon("Zorg 77mm f/0.7 Imaginary", 77.0, 1.0, 5760, 3840);
    assert!(p.effective_lens().is_none());
    assert!(!p.camera_defaults().optics.lens_profile);
    let v = crate::cmd::lens::describe(&p);
    assert_eq!(v["profile"]["source"], "none");
    assert!(!v["profile"]["note"].as_str().unwrap_or("").is_empty(), "{v}");
    // no camera, no lens: nothing to look up
    let mut q = canon("", 50.0, 4.0, 100, 100);
    q.meta.camera.clear();
    assert!(q.database_lens().is_none());
}

#[test]
fn old_catalog_metadata_still_reads() {
    // a photo record written before the lens facts existed: they default to absent
    let m: lightcraft_catalog::Meta =
        serde_json::from_str(r#"{"camera":"Canon Canon EOS 5D Mark III","lens":"50mm","focal_mm":50.0,"aperture":4.5}"#).unwrap();
    assert!(m.lens_spec.is_none() && m.lens_max_aperture.is_none() && m.lens_make.is_empty());
    // and the new facts are not written when absent
    let out = serde_json::to_string(&m).unwrap();
    assert!(!out.contains("lens_spec") && !out.contains("subject_distance"), "{out}");
}

/// A grid of dark lines (spacing 100 px) at continuous position `q`: 1 on the paper, 0 on a line.
fn grid(x: f64, y: f64) -> f32 {
    let d = |v: f64| {
        let m = v.rem_euclid(100.0);
        m.min(100.0 - m)
    };
    let dist = d(x).min(d(y));
    let t = ((dist - 1.5) / 2.0).clamp(0.0, 1.0);
    (0.02 + 0.88 * t * t * (3.0 - 2.0 * t)) as f32
}

/// The photo of the grid as the lens would have bent it: source pixel `p` shows the grid at the corrected position
/// `q` with `p = c + (q - c)·f(|q − c| / R)` (the pipeline's mapping, inverted by bisection).
fn bent_grid(w: usize, h: usize, lens: &EmbeddedLens) -> Rgb32f {
    let warp = lens.warp.expect("warp");
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let big_r = (w as f64).hypot(h as f64) / 2.0;
    let f = |r: f64| {
        let r2 = r * r;
        let k = warp.planes[1];
        k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]))
    };
    Rgb32f::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
        let dist = dx.hypot(dy);
        if dist < 1e-9 {
            let v = grid(cx, cy);
            return [v, v, v];
        }
        let (mut lo, mut hi) = (0.0f64, 1.6f64);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if mid * big_r * f(mid) < dist { lo = mid } else { hi = mid }
        }
        let s = lo * big_r / dist;
        let v = grid(cx + dx * s, cy + dy * s);
        [v, v, v]
    })
}

/// Largest distance (px) of the centre of a near-vertical grid line from the straight line fitted through it.
fn line_bow(img: &Rgba8, x_hint: f64) -> f64 {
    let (w, h) = (img.width, img.height);
    let lum = |x: usize, y: usize| f64::from(img.data[y * w + x][1]);
    let mut pts = Vec::new();
    let y_mid = h / 2;
    // find the line near the hint on the middle row, then follow it up and down
    let centre_at = |y: usize, around: f64, half: i32| -> Option<f64> {
        let (mut sw, mut sx) = (0.0, 0.0);
        for x in ((around as i32 - half).max(0))..=((around as i32 + half).min(w as i32 - 1)) {
            let wgt = (230.0 - lum(x as usize, y)).max(0.0);
            sw += wgt;
            sx += wgt * f64::from(x);
        }
        (sw > 200.0).then(|| sx / sw)
    };
    // the darkest column near the hint on the middle row (rows through a horizontal line are skipped below)
    let mut y0 = y_mid;
    while (((x_hint as usize).saturating_sub(15))..(x_hint as usize + 15).min(w)).filter(|x| lum(*x, y0) < 120.0).count() > 10 {
        y0 += 1;
    }
    let darkest = (((x_hint as i32 - 60).max(0))..=((x_hint as i32 + 60).min(w as i32 - 1)))
        .min_by(|a, b| lum(*a as usize, y0).total_cmp(&lum(*b as usize, y0)))
        .unwrap_or(0);
    let Some(c0) = centre_at(y0, f64::from(darkest), 6) else { return f64::NAN };
    let y_mid = y0;
    pts.push((y_mid as f64, c0));
    for dir in [-1i32, 1] {
        let mut c = c0;
        let mut y = y_mid as i32;
        let mut misses = 0;
        loop {
            y += dir * 3;
            if y < (h as f64 * 0.12) as i32 || y > (h as f64 * 0.88) as i32 {
                break;
            }
            // rows crossing a horizontal grid line are wide and dark: skip them
            let dark = ((c as i32 - 9).max(0)..=(c as i32 + 9).min(w as i32 - 1)).filter(|x| lum(*x as usize, y as usize) < 120.0).count();
            if dark > 8 {
                continue;
            }
            match centre_at(y as usize, c, 8) {
                Some(n) => {
                    c = n;
                    misses = 0;
                    pts.push((f64::from(y), n));
                }
                None => {
                    misses += 1;
                    if misses > 5 {
                        break;
                    }
                }
            }
        }
    }
    eprintln!(
        "  tracked {} rows, x from {:.1} to {:.1}",
        pts.len(),
        pts.iter().map(|p| p.1).fold(f64::MAX, f64::min),
        pts.iter().map(|p| p.1).fold(0.0, f64::max)
    );
    let n = pts.len() as f64;
    let (sy, sx) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    let sxx: f64 = pts.iter().map(|p| (p.0 - sy).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - sy) * (p.1 - sx)).sum();
    let slope = sxy / sxx.max(1e-9);
    pts.iter().map(|p| (p.1 - (sx + slope * (p.0 - sy))).abs()).fold(0.0, f64::max)
}

#[test]
fn the_profile_straightens_the_lines_the_lens_bent() {
    install_db();
    let (w, h) = (1500usize, 1000usize);
    // a wide-angle zoom at its widest end: clearly barrel distorted
    let p = canon("EF24-105mm f/4L IS USM", 24.0, 4.0, w as u32, h as u32);
    let lens = p.effective_lens().expect("profile");
    eprintln!("warp G plane: {:?}", lens.warp.map(|w| w.planes[1]));
    let src = bent_grid(w, h, &lens);
    let mut s = DevelopSettings::default();
    let req = RenderRequest::fit(w, h);
    let info = SourceInfo { lens: Some(lens), ..Default::default() };
    s.optics.lens_profile = false;
    let off = lightcraft_pipeline::render(&src, &info, &s, &req).image;
    s.optics.lens_profile = true;
    let on = lightcraft_pipeline::render(&src, &info, &s, &req).image;
    // a vertical grid line near the left edge of the frame bows most
    let hint_off = off.width as f64 * 0.07;
    let (bow_off, bow_on) = (line_bow(&off, hint_off), line_bow(&on, on.width as f64 * 0.07));
    eprintln!("line bow: {bow_off:.2} px without the profile, {bow_on:.2} px with it");
    assert!(bow_off > 1.5, "the synthetic lens should bend the line by several pixels, got {bow_off}");
    assert!(bow_on < 0.5 * bow_off && bow_on < 1.5, "profile on: {bow_on} px vs off: {bow_off} px");
}

#[test]
fn lens_detect_reports_enables_and_keeps_the_edit_state() {
    use serde_json::json;
    install_db();
    let mut s = crate::Session::with_demo();
    for (id, lens, focal) in [(100u64, "EF24-105mm f/4L IS USM", 24.0f32), (101, "Zorg 77mm f/0.7 Imaginary", 77.0)] {
        let mut p = canon(lens, focal, 4.0, 400, 300);
        p.id = PhotoId(id);
        // imported before profile corrections existed: off
        p.develop = std::sync::Arc::new(DevelopSettings::for_raw(6500.0, 0.0));
        s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let before = s.catalog.photo(PhotoId(100)).unwrap().edited.clone();
    let info = s.execute("lens.info", &json!({"id": 100})).unwrap();
    assert_eq!(info["profile"]["source"], "database");
    assert_eq!(info["profile"]["lens"], "Canon EF 24-105mm f/4L IS USM");
    assert_eq!(info["profile"]["confidence"], "exact");
    assert_eq!(info["camera"], "Canon EOS 5D Mark III");
    assert_eq!(info["enabled"], false);
    // report only
    let r = s.execute("lens.detect", &json!({"ids": [100, 101], "reread": false})).unwrap();
    assert_eq!((r["database"].as_u64(), r["none"].as_u64(), r["enabled"].as_u64()), (Some(1), Some(1), Some(0)));
    assert!(!s.catalog.photo(PhotoId(100)).unwrap().develop.optics.lens_profile);
    // enable: only the photo that has a profile, edit stamp and history untouched
    let r = s.execute("lens.detect", &json!({"ids": [100, 101], "reread": false, "enable": true})).unwrap();
    assert_eq!(r["enabled"].as_u64(), Some(1));
    let p100 = s.catalog.photo(PhotoId(100)).unwrap();
    assert!(p100.develop.optics.lens_profile);
    assert_eq!(p100.edited, before);
    assert!(p100.history.is_empty());
    assert!(!s.catalog.photo(PhotoId(101)).unwrap().develop.optics.lens_profile);
    // it is the photo's import default now, so it does not count as edited
    assert!(!s.catalog.photo(PhotoId(100)).unwrap().is_edited());
    // undo restores
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!s.catalog.photo(PhotoId(100)).unwrap().develop.optics.lens_profile);
}

/// The two features on one view: the "No keyword" filter is on while the lens profile is switched on for the
/// photos it shows. The filter, its counts and the profile state must not disturb each other, and tagging a
/// photo takes it out of the view without touching its profile.
#[test]
fn no_keyword_filter_and_an_active_lens_profile_share_a_view() {
    use serde_json::json;
    install_db();
    let mut s = crate::Session::with_demo();
    let demo = s.catalog.photos().filter(|p| p.in_library()).count();
    // six Canon raws with a known lens: ids 200..206, the even ones tagged, the odd ones not
    for id in 200u64..206 {
        let mut p = canon("EF24-105mm f/4L IS USM", 24.0, 4.0, 400, 300);
        p.id = PhotoId(id);
        p.develop = std::sync::Arc::new(DevelopSettings::for_raw(6500.0, 0.0));
        if id % 2 == 0 {
            p.meta.keywords = vec!["airshow".into()];
        }
        s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let untagged_total = s.catalog.photos().filter(|p| p.in_library() && p.meta.keywords.is_empty()).count();
    let tagged_total = s.catalog.photos().filter(|p| p.in_library() && !p.meta.keywords.is_empty()).count();
    assert_eq!(untagged_total + tagged_total, demo + 6);

    s.execute("library.filter", &json!({"untagged": true})).unwrap();
    let mut mine: Vec<u64> = s.visible_cloned().iter().map(|id| id.0).filter(|id| (200..206).contains(id)).collect();
    mine.sort_unstable();
    assert_eq!(mine, vec![201, 203, 205], "the three untagged Canon raws are in the filtered view");
    assert_eq!(s.visible().len(), untagged_total);

    // switch the profile on for the filtered view's own photos
    let r = s.execute("lens.detect", &json!({"ids": mine, "reread": false, "enable": true})).unwrap();
    assert_eq!((r["database"].as_u64(), r["enabled"].as_u64()), (Some(3), Some(3)));
    for id in &mine {
        assert!(s.catalog.photo(PhotoId(*id)).unwrap().develop.optics.lens_profile);
        let info = s.execute("lens.info", &json!({"id": id})).unwrap();
        assert_eq!((info["enabled"].as_bool(), info["profile"]["source"].as_str()), (Some(true), Some("database")));
    }
    // the filter and its counts did not move
    assert_eq!(s.visible().len(), untagged_total);
    let c = s.execute("keyword.untagged", &json!({})).unwrap();
    assert_eq!((c["untagged"].as_u64(), c["tagged"].as_u64()), (Some(untagged_total as u64), Some(tagged_total as u64)));
    assert_eq!(s.keyword_presence(), (untagged_total, tagged_total));

    // tag one: it leaves the view, its profile stays on
    s.execute("photo.setMeta", &json!({"ids": [201], "keywords": ["f16"]})).unwrap();
    assert_eq!(s.visible().len(), untagged_total - 1);
    assert!(!s.visible_cloned().iter().any(|id| id.0 == 201));
    assert!(s.catalog.photo(PhotoId(201)).unwrap().develop.optics.lens_profile);

    // undo the tagging: back in the view with its profile; undo the detection: every profile off again, view unchanged
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.visible().len(), untagged_total);
    assert!(s.catalog.photo(PhotoId(201)).unwrap().develop.optics.lens_profile);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(mine.iter().all(|id| !s.catalog.photo(PhotoId(*id)).unwrap().develop.optics.lens_profile));
    assert_eq!(s.visible().len(), untagged_total);
    s.execute("library.clearFilter", &json!({})).unwrap();
    assert_eq!(s.visible().len(), demo + 6);
}
