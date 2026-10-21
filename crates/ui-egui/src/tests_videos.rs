//! Videos view, headless: the list, a sheet and the search results draw whatever the data (empty,
//! missing or hostile fields) without a panic; the French texts are there; typing a description,
//! Enter, then Confirm writes it through `describe.*`; a face seen in a video is drawn and opens
//! its moment.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::i18n::{Locale, set_language, tr};
use crate::panels::videos::Section;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(60);

fn demo() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    Headless::new(app, [1300.0, 900.0], 1.0)
}

#[test]
fn the_videos_view_draws_any_data_without_a_panic() {
    let mut h = demo();
    let rev = h.app.session.catalog.revision;
    let hostile = vec![
        json!(null),
        json!({}),
        json!({"videos": "nope", "people": 3}),
        json!({"videos": [{"id": 1}, {"id": "x"}, {"id": 2, "name": 5, "duration": "NaN", "people": [null, 3, "Awa"], "frames": -1, "moments": 1e300, "poster": "/nowhere/x.jpg"}], "people": ["Awa", null]}),
        json!({"videos": [{"id": 3, "duration": -5.0, "facesLooked": 9, "frames": 2, "confirmed": 7, "moments": 2}]}),
    ];
    let sheets = vec![
        json!(null),
        json!({"moments": []}),
        json!({"video": true, "name": null, "duration": f64::MAX, "moments": [
            {},
            {"ms": u64::MAX, "frame": "/nowhere/t.jpg", "faces": [{"rect": [0.1]}, {"rect": [0.9, 0.9, 0.1, 0.1], "name": "Awa"}, {"rect": "x"}], "annotation": {"status": "confirmed", "scene": 3, "proposals": "x"}},
            {"ms": 0, "frame": "", "annotation": {"scene": "Plage", "proposals": [{"scene": "Mer"}, {"scene": null}, 7], "from": ["manual"]}},
        ]}),
        json!({"video": false, "moments": [{"ms": null, "annotation": null, "faces": null}]}),
    ];
    let searches = vec![
        json!({}),
        json!({"query": 5, "results": [{"id": 1, "ms": 1000}, {"id": null}, {"id": 2, "people": [1, "Awa"], "status": "confirmed"}], "diagnostics": {"personPresentWhatNotDetected": {"total": 3, "examples": [{"id": 1}]}, "whatFoundPeopleNotRecognized": {"total": "x"}}}),
    ];
    h.app.ui.view = ViewMode::Videos;
    for l in &hostile {
        h.app.caches.videos.section = Section::List;
        h.app.caches.videos.inject(rev, Some(l.clone()), None, None);
        h.step();
        h.step();
    }
    for sh in &sheets {
        h.app.caches.videos.section = Section::Sheet;
        h.app.caches.videos.inject(rev, None, Some((1, sh.clone())), None);
        h.app.caches.videos.focus_ms = Some(0);
        h.step();
        h.step();
    }
    for sr in &searches {
        h.app.caches.videos.section = Section::Search;
        h.app.caches.videos.show_diagnostics = true;
        h.app.caches.videos.inject(rev, None, None, Some(sr.clone()));
        h.step();
        h.step();
    }
    // the real engine answers too (a demo library has no video)
    h.app.caches.videos.section = Section::List;
    h.app.caches.videos.dirty = true;
    h.settle(SETTLE);
    let r = h.request("ui.set", json!({"view": "videos"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.view, ViewMode::Videos);
}

#[test]
fn french_texts_of_the_videos_view_are_there() {
    set_language(Locale::Fr);
    for (en, fr) in [
        ("Videos", "Vidéos"),
        ("Scene", "Scène"),
        ("Confirm", "Confirmer"),
        ("Proposed", "Proposé"),
        ("Confirmed", "Confirmé"),
        ("Propose descriptions", "Proposer des descriptions"),
        ("Resume the Video Catcher import", "Reprendre l'import Video Catcher"),
        ("To confirm", "À confirmer"),
    ] {
        assert_eq!(tr(en), fr, "{en}");
    }
    assert_eq!(crate::i18n::tr_format!("Person present, action not detected ({n})", n = 4), "Personne présente, action non détectée (4)");
    assert_eq!(crate::i18n::tr_format!("Play at {t}", t = "1:23"), "Lire à 1:23");
    assert_eq!(crate::panels::videos::described_line(4, 1), "4 moments décrits · 1 confirmé");
    assert_eq!(crate::panels::videos::described_line(1, 2), "1 moment décrit · 2 confirmés");
    assert_eq!(crate::panels::videos::described_line(0, 0), "0 moment décrit · 0 confirmé");
    set_language(Locale::En);
    assert_eq!(crate::panels::videos::described_line(1, 3), "1 moment described · 3 confirmed");
    assert_eq!(crate::panels::videos::described_line(2, 1), "2 moments described · 1 confirmed");
    set_language(Locale::Fr);
    set_language(Locale::En);
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("lc-ui-videos-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A photo's description: typed, Enter, a proposal taken in one click, then Confirm.
#[test]
fn typing_a_description_then_confirming_it_writes_it() {
    let dir = temp_dir("type");
    let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data: vec![[90, 120, 160, 255]; 64 * 48] };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(dir.join("p.png"), bytes).unwrap();
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [dir.join("p.png").to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap();
    let mut h = Headless::new(LightcraftApp::new(s, Services::default()), [1300.0, 900.0], 1.0);
    h.settle(SETTLE);
    let r = h.app.run("videos.open", json!({"id": id}));
    assert!(r.is_ok(), "{r:?}");
    h.settle(SETTLE);
    let field = format!("field:{id}:-:scene");
    let r = h.request("ui.clickWidget", json!({"id": field}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.request("ui.text", json!({"text": "Lac de Vassivière"}), T);
    h.request("ui.key", json!({"key": "Enter"}), T);
    h.settle(SETTLE);
    let l = h.app.session.execute("describe.list", &json!({"id": id})).unwrap();
    assert_eq!(l["annotations"][0]["scene"], "Lac de Vassivière", "{l}");
    assert_eq!(l["annotations"][0]["status"], "candidate", "typing is not confirming");
    let r = h.request("ui.clickWidget", json!({"id": format!("button:confirm:{id}:-")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let l = h.app.session.execute("describe.list", &json!({"id": id})).unwrap();
    assert_eq!(l["annotations"][0]["status"], "confirmed", "{l}");
    // the Info panel shows the same editor for the photo
    h.app.caches.videos.dirty = true;
    h.app.session.selection.ids = vec![lightcraft_catalog::PhotoId(id)];
    h.app.session.selection.active = Some(lightcraft_catalog::PhotoId(id));
    let r = h.request("ui.set", json!({"view": "detail", "right": "info"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let w: Value = h.request("ui.widgets", json!({}), T);
    assert!(w.to_string().contains(&format!("field:{id}:-:scene")), "the Info panel has the description: {w}");
}

/// The search field: "who + what" opens its results in the Videos view; `semantic:` does not
/// (it filters the grid by meaning, which needs the model).
#[test]
fn the_search_field_opens_who_and_what_results() {
    let dir = temp_dir("search");
    let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data: vec![[90, 120, 160, 255]; 64 * 48] };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(dir.join("p.png"), bytes).unwrap();
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [dir.join("p.png").to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap();
    s.execute("describe.set", &json!({"id": id, "action": "conduit une voiture"})).unwrap();
    let mut h = Headless::new(LightcraftApp::new(s, Services::default()), [1300.0, 900.0], 1.0);
    h.settle(SETTLE);
    h.app.ui.search = "conduite voiture".into();
    crate::panels::videos::search_from_field(&mut h.app, "conduite voiture");
    assert_eq!((h.app.ui.view, h.app.caches.videos.section), (ViewMode::Videos, Section::Search));
    h.settle(SETTLE);
    let total = h.app.caches.videos.search.as_ref().map(|r| r["total"].clone());
    assert_eq!(total, Some(json!(1)), "{:?}", h.app.caches.videos.search);
    let w: Value = h.request("ui.widgets", json!({}), T);
    assert!(w.to_string().contains(&format!("result:res:{id}:0")), "{w}");
    // a click on a result opens its sheet
    let r = h.request("ui.clickWidget", json!({"id": format!("result:res:{id}:0")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!((h.app.caches.videos.section, h.app.caches.videos.open), (Section::Sheet, Some(id)));
}

/// The sheet's silent preview: "Preview" on a moment makes the 8 s clip and decodes it on a
/// thread (the window keeps drawing meanwhile), it plays once ready, Pause / Play, -5 s makes a new one, Close stops
/// it. Skips when FFmpeg is not installed.
#[test]
fn the_preview_plays_a_moment_without_sound_and_closes() {
    let Some(ff) = lightcraft_engine::video::ffmpeg::ffmpeg() else {
        eprintln!("skipped: FFmpeg is not installed");
        return;
    };
    let dir = temp_dir("preview");
    let v = dir.join("v.mp4");
    let st = std::process::Command::new(ff)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=12:size=320x240:rate=10"])
        .args(["-pix_fmt", "yuv420p", "-c:v", "mpeg4"])
        .arg(&v)
        .status()
        .unwrap();
    assert!(st.success());
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [v.to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap();
    s.execute("video.frames", &json!({"ids": [id]})).unwrap();
    let ms = s.execute("video.moments", &json!({"id": id})).unwrap()["moments"]
        .as_array()
        .and_then(|m| m.iter().filter_map(|x| x["ms"].as_u64()).find(|&ms| ms >= 6000))
        .expect("a moment after 6 s");
    let mut h = Headless::new(LightcraftApp::new(s, Services::default()), [1300.0, 900.0], 1.0);
    h.settle(SETTLE);
    assert!(h.app.run("videos.open", json!({"id": id})).is_ok());
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": format!("button:preview:{id}:{ms}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.settle(SETTLE); // waits while the player is loading (headless::busy)
    let (frames, error, playing) =
        h.app.caches.videos.player.as_ref().map(|p| p.state()).map(|(n, e, p)| (n, e.map(str::to_string), p)).expect("a player");
    assert!(error.is_none(), "{error:?}");
    assert!(frames > 10, "the clip's pictures are decoded ({frames})");
    assert!(playing, "it plays once ready (the user asked for it)");
    let r = h.request("ui.clickWidget", json!({"id": "button:previewPlay"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.caches.videos.player.as_ref().is_some_and(|p| !p.state().2), "paused");
    let r = h.request("ui.clickWidget", json!({"id": "button:previewPlay"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.caches.videos.player.as_ref().is_some_and(|p| p.state().2), "playing again");
    for _ in 0..5 {
        h.step();
    }
    // -5 s: a new clip, earlier
    let r = h.request("ui.clickWidget", json!({"id": "button:previewBack"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.caches.videos.player.as_ref().is_some_and(|p| p.ms < ms), "a clip earlier in the video");
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "button:previewClose"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.caches.videos.player.is_none(), "closed");
}
