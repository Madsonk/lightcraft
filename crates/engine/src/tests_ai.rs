//! Depth and inpainting without the models: what the engine does with a depth map or a fill that
//! already exists (a synthetic one here), and what it says when the models are missing.

use lightcraft_develop::{DepthMap, InpaintPatch};
use serde_json::json;

use crate::Session;

/// A depth map that is near on the left half and far on the right half.
fn split_depth() -> DepthMap {
    let (w, h) = (32, 20);
    DepthMap::new(w, h, (0..w * h).map(|i| if i % w < w / 2 { 60000 } else { 5000 }).collect()).unwrap()
}

/// The active photo rendered to fit 240 × 160 (its size follows the photo's aspect ratio).
fn render_img(s: &mut Session) -> lightcraft_raster::Rgba8 {
    let id = s.active().unwrap();
    s.render_job(id, 240, 160, false, true).unwrap().run().rendered.unwrap().image
}

fn render(s: &mut Session) -> Vec<[u8; 4]> {
    render_img(s).data
}

#[test]
fn lens_blur_uses_the_attached_depth_map_and_changes_the_render() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let ph = s.catalog.photo(id).unwrap().clone();
    let plain = render(&mut s);
    // Port 0.21: without a depth map, upstream's subject-based Lens Blur applies (the fork's
    // depth path needs the map), so the render now changes instead of staying untouched.
    s.execute("lensblur.set", &json!({"amount": 100, "focus": 100, "range": 10})).unwrap();
    assert_ne!(render(&mut s), plain, "upstream Lens Blur blurs by its subject estimate");
    // With the depth map attached, the fork's depth-driven path takes over (the gate in
    // `pipeline::lib` keeps upstream's module out when a depth map is present).
    assert!(s.execute("lensblur.set", &json!({"point": [0.3, 0.5]})).is_err(), "a focus point needs the depth map");
    // with a depth map the far half is blurred and the focused half is not
    s.store_depth_map(&ph, split_depth()).unwrap();
    // focus on the near half (the depth under a point on it)
    let r = s.execute("lensblur.set", &json!({"point": [0.1, 0.5]})).unwrap();
    assert!(r["lensBlur"]["focal_distance"].as_f64().unwrap() > 85.0, "{r}");
    let blurred = render(&mut s);
    assert_ne!(blurred, plain);
    let (w, h) = (render_img(&mut s).width, render_img(&mut s).height);
    let diff = |a: &[[u8; 4]], b: &[[u8; 4]], x0: usize, x1: usize| -> usize {
        (0..h).flat_map(|y| (x0..x1).map(move |x| (y, x))).filter(|(y, x)| a[y * w + x] != b[y * w + x]).count()
    };
    assert_eq!(diff(&plain, &blurred, 0, w * 3 / 8), 0, "the focused (near) half stays as it was, away from the border");
    assert!(diff(&plain, &blurred, w * 5 / 8, w) > h * w / 8, "the far half is blurred");
    // the depth never enters the settings (and so the journal / XMP)
    let d = s.develop_of(id).unwrap();
    assert!(d.aux.is_empty());
    assert!(!serde_json::to_string(&*d).unwrap().contains("depth"));
    // switching it off restores the plain render
    s.execute("lensblur.set", &json!({"enabled": false})).unwrap();
    assert_eq!(render(&mut s), plain);
}

#[test]
fn depth_range_mask_selects_by_depth() {
    let mut s = Session::with_demo();
    let ph = s.catalog.photo(s.active().unwrap()).unwrap().clone();
    s.store_depth_map(&ph, split_depth()).unwrap();
    // the near half (far = 0..0.3), darkened
    s.execute("mask.add", &json!({"kind": "depthRange", "lo": 0.0, "hi": 0.3, "feather": 0.05})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": -3.0}})).unwrap();
    let dark = render(&mut s);
    s.execute("mask.delete", &json!({})).unwrap();
    let plain = render(&mut s);
    let (w, h) = (render_img(&mut s).width, render_img(&mut s).height);
    let lum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    let mean = |img: &[[u8; 4]], x0: usize, x1: usize| -> f64 {
        (0..h).flat_map(|y| (x0..x1).map(move |x| (y, x))).map(|(y, x)| f64::from(lum(img[y * w + x]))).sum::<f64>() / (h as f64 * (x1 - x0) as f64)
    };
    let (a, b) = (w / 20, w * 3 / 8);
    assert!(mean(&dark, a, b) < 0.5 * mean(&plain, a, b), "the near side is darker");
    assert!((mean(&dark, w * 2 / 3, w * 19 / 20) - mean(&plain, w * 2 / 3, w * 19 / 20)).abs() < 1.0, "the far side is untouched");
}

#[test]
fn an_inpaint_spot_shows_its_fill_and_survives_without_the_model() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let plain = render(&mut s);
    // a pure green 40 × 40 fill over the middle, fully opaque inside
    let (pw, ph) = (40usize, 40usize);
    let rgb: Vec<u8> = (0..pw * ph).flat_map(|_| [0u8, 255, 0]).collect();
    let alpha = vec![255u8; pw * ph];
    let patch = s.store_patch_for_test(InpaintPatch::new([0.4, 0.4, 0.6, 0.6], pw, ph, rgb, alpha).unwrap());
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.spots.push(lightcraft_develop::Spot {
        mode: lightcraft_develop::SpotMode::Inpaint,
        points: vec![lightcraft_geom::Point::new(0.5, 0.5)],
        patch: Some(patch.hex_id()),
        ..Default::default()
    });
    s.set_develop(id, d, "Remove (inpaint)").unwrap();
    let filled = render(&mut s);
    let (w, h) = (render_img(&mut s).width, render_img(&mut s).height);
    let c = filled[h / 2 * w + w / 2];
    assert!(c[1] > 150 && c[0] < 80, "the fill is green at the centre: {c:?}");
    assert_eq!(filled[5 * w + 5], plain[5 * w + 5], "elsewhere unchanged");
    // the spot is in the settings, the pixels are not
    let json = serde_json::to_string(&*s.develop_of(id).unwrap()).unwrap();
    assert!(json.contains(&patch.hex_id()) && json.len() < 20_000);
    // undo removes it
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(render(&mut s), plain);
}

#[test]
fn models_are_optional_and_downloads_need_consent() {
    let mut s = Session::with_demo();
    s.ai.depth_dir = Some(std::env::temp_dir().join("lc-test-no-depth-model"));
    s.ai.inpaint_dir = Some(std::env::temp_dir().join("lc-test-no-inpaint-model"));
    for kind in ["depth", "inpaint"] {
        let st = s.execute(&format!("{kind}.model.status"), &json!({})).unwrap();
        assert_eq!(st["installed"], false, "{st}");
        let e = s.execute(&format!("{kind}.model.download"), &json!({})).unwrap_err().to_string();
        assert!(e.contains("acknowledged"), "{e}");
    }
    // without the model the commands explain what to do, and nothing else is affected
    let e = s.execute("depth.estimate", &json!({})).unwrap_err().to_string();
    assert!(e.contains("not installed") || e.contains("not available"), "{e}");
    let e = s.execute("spot.inpaint", &json!({"rect": [0.4, 0.4, 0.5, 0.5]})).unwrap_err().to_string();
    assert!(e.contains("not installed") || e.contains("not available"), "{e}");
    assert!(s.develop_of(s.active().unwrap()).unwrap().spots.is_empty());
}
