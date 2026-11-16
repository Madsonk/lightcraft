//! Videos view, headless: the list, a sheet and the search results draw whatever the data (empty,
//! missing or hostile fields) without a panic; the French texts are there; typing a description,
//! Enter, then Confirm writes it through `describe.*`; a face seen in a video is drawn and opens
//! its moment.

use std::time::Duration;

use egui::{Pos2, Rect};
use lightcraft_catalog::{AlbumId, PhotoId};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::i18n::{Locale, set_language, tr};
use crate::panels::grid;
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
    assert_eq!(crate::i18n::tr_format!("{f} face{fs} · {p} photo{ps} · drag", f = 1, fs = "", p = 1, ps = ""), "1 visage · 1 photo · glisser");
    assert_eq!(crate::i18n::tr_format!("{f} face{fs} · {p} photo{ps} · drag", f = 4, fs = "s", p = 2, ps = "s"), "4 visages · 2 photos · glisser");
    assert_eq!(crate::i18n::tr_format!("{faces} faces found: name them…", faces = 943), "943 visages trouvés : nommez-les…");
    assert_eq!(tr("All people and groups…"), "Toutes les personnes et groupes…");
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

// ------------------------------------------------------------------ dragging a video out of the list

/// `path` as a video file: the bytes are never read by a drop (an album, a keyword and a folder drop
/// only change the catalog row, or move the file), so any content does.
fn write_clip(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"a video file, not decoded by these tests").unwrap();
}

/// `path` in the catalog as a library video (as an import would leave it).
fn add_video(s: &mut lightcraft_engine::Session, path: &std::path::Path) -> u64 {
    use lightcraft_catalog::{MediaKind, Op, Photo, Source};
    let id = s.catalog.alloc_photo_id();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let mut p = Photo::new(id, Source::File { path: path.to_string_lossy().into() }, &name, "MP4", 320, 240, "2026-01-01T10:00:00");
    p.kind = MediaKind::Video;
    p.duration = Some(12.0);
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id.0
}

/// `path` in the catalog as a library photo (as an import would leave it).
fn add_photo(s: &mut lightcraft_engine::Session, path: &std::path::Path) -> u64 {
    use lightcraft_catalog::{Op, Photo, Source};
    let id = s.catalog.alloc_photo_id();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let p = Photo::new(id, Source::File { path: path.to_string_lossy().into() }, &name, "PNG", 8, 8, "2026-01-01T10:00:00");
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id.0
}

/// The Videos view on `s` with one video per `paths`, `set` applied to the window, settled.
fn videos_view(mut s: lightcraft_engine::Session, paths: &[&std::path::Path], set: Value) -> (Headless, Vec<u64>) {
    let ids = paths.iter().map(|p| add_video(&mut s, p)).collect();
    let mut h = Headless::new(LightcraftApp::new(s, Services { png: None, ..Default::default() }), [1300.0, 1100.0], 1.0);
    let r = h.request("ui.set", set, T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    (h, ids)
}

fn widget(h: &Headless, id: &str) -> Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

/// Press the primary button at `at`.
fn press(h: &mut Headless, at: Pos2) {
    h.app.synthetic.push(egui::Event::PointerMoved(at));
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
}

/// Move the pointer, pressed, from `from` to `to` over a few frames.
fn glide(h: &mut Headless, from: Pos2, to: Pos2) {
    for i in 1..=8 {
        h.app.synthetic.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
}

fn release(h: &mut Headless, at: Pos2) {
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    h.step();
}

/// Click a widget the way a person does (press and release on it).
fn click(h: &mut Headless, id: &str) {
    let at = widget(h, id).center();
    press(h, at);
    release(h, at);
}

/// What the dragged label says this frame instead of the count, if a drop target set it.
fn hint(h: &Headless) -> Option<String> {
    h.view.ctx.data(|d| d.get_temp::<(u64, String)>(egui::Id::unique("drag-photos-hint"))).map(|(_, text)| text)
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.as_ref().map(|(t, _, _)| t.clone()).unwrap_or_default()
}

fn keywords_of(h: &Headless, id: u64) -> Vec<String> {
    h.app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().meta.keywords.clone()
}

/// A video dragged from the list joins an album, and the notice says "video".
#[test]
fn a_video_dragged_from_the_videos_view_joins_an_album() {
    let dir = temp_dir("album-drop");
    let clip = dir.join("clip.mp4");
    write_clip(&clip);
    let (mut h, ids) =
        videos_view(lightcraft_engine::Session::with_demo(), &[&clip], json!({"view": "videos", "leftPanel": true, "sidebar": ["albums"]}));
    let id = ids[0];
    let album = h.app.run("album.create", json!({"name": "Plage", "addSelected": false})).unwrap()["id"].as_u64().unwrap();
    h.settle(SETTLE);
    // Given its poster pressed and dragged onto the album's row
    let from = widget(&h, &format!("videoCard:{id}")).center();
    let row = widget(&h, &format!("source:album:{album}")).center();
    press(&mut h, from);
    glide(&mut h, from, row);
    // Then the drag carries that video alone: the Videos view has no selection
    assert_eq!(h.app.ui.dragging_photos, Some(vec![id]));
    assert!(!h.app.session.selection.contains(PhotoId(id)), "the Videos view selects nothing");
    release(&mut h, row);
    // and the drop put it in the album, saying "video"
    assert_eq!(h.app.session.catalog.album(AlbumId(album)).unwrap().photos, vec![PhotoId(id)]);
    assert_eq!(toast(&h), "Added 1 video to “Plage”");
    assert!(h.app.ui.dragging_photos.is_none(), "the drag ended");
}

/// A video dragged from the list takes a keyword from the Keyword List, and the notice says "video".
#[test]
fn a_video_dragged_from_the_videos_view_is_tagged_in_the_keyword_list() {
    let dir = temp_dir("keyword-drop");
    let clip = dir.join("clip.mp4");
    write_clip(&clip);
    let (mut h, ids) = videos_view(lightcraft_engine::Session::with_demo(), &[&clip], json!({"view": "videos", "right": "keywords"}));
    let id = ids[0];
    assert!(keywords_of(&h, id).is_empty());
    let from = widget(&h, &format!("videoCard:{id}")).center();
    let row = widget(&h, "keywordRow:mountains").center();
    press(&mut h, from);
    glide(&mut h, from, row);
    release(&mut h, row);
    assert_eq!(keywords_of(&h, id), vec!["mountains".to_string()]);
    assert_eq!(toast(&h), "Added “mountains” to 1 video");
}

/// A video dragged from the list onto a folder asks first (titled for videos), then moves there on
/// disk, and the label and the notice say "video".
#[test]
fn a_video_dragged_from_the_videos_view_moves_to_a_folder() {
    let dir = temp_dir("folder-drop");
    let (trip, home) = (dir.join("trip"), dir.join("home"));
    let (clip, other) = (trip.join("clip.mp4"), home.join("other.mp4"));
    write_clip(&clip);
    write_clip(&other);
    let (mut h, ids) = videos_view(
        lightcraft_engine::Session::new().with_fs(),
        &[&clip, &other],
        json!({"view": "videos", "leftPanel": true, "sidebar": ["folders"]}),
    );
    let id = ids[0];
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let from = widget(&h, &format!("videoCard:{id}")).center();
    let row = widget(&h, &format!("source:libfolder:{}", dir.to_string_lossy().replace('\\', "/"))).center();
    press(&mut h, from);
    glide(&mut h, from, row);
    // (the label is read from the pointer as the frame before saw it)
    h.step();
    // Then the label says where it would go, and "video"
    assert_eq!(hint(&h).as_deref(), Some(format!("Move 1 video to “{name}”").as_str()));
    // and the release asks first: nothing moved yet
    release(&mut h, row);
    assert!(matches!(h.app.ui.dialog, Some(crate::state::Dialog::MovePhotos { .. })), "{:?}", h.app.ui.dialog);
    assert!(clip.is_file(), "not before Move");
    // the question is titled for videos, not photos
    let dlg = h.app.ui.dialog.clone().expect("the question");
    assert_eq!(crate::dialogs::dialog_title(&h.app, &dlg), "Move Videos");
    h.step();
    click(&mut h, "button:dialogOk");
    // Then the file moved, the video follows it, and the notice says "video"
    assert!(dir.join("clip.mp4").is_file() && !clip.exists(), "moved on disk");
    assert_eq!(toast(&h), format!("Moved 1 video to “{name}”. Edit → Undo moves them back."));
    assert!(h.app.ui.dragging_photos.is_none(), "the drag ended");
    // and one Undo moves it back
    h.app.run("edit.undo", json!({})).unwrap();
    assert!(clip.is_file() && !dir.join("clip.mp4").exists(), "back where it was");
}

/// A drag of photos and videos together keeps the photo words, as it kept them before videos could be
/// dragged.
#[test]
fn only_a_drag_of_nothing_but_videos_says_video() {
    let dir = temp_dir("all-videos");
    let clip = dir.join("clip.mp4");
    write_clip(&clip);
    let (mut h, ids) = videos_view(lightcraft_engine::Session::new().with_fs(), &[&clip], json!({"view": "videos"}));
    let video = ids[0];
    let image = add_photo(&mut h.app.session, &dir.join("p.png"));
    assert!(grid::all_videos(&h.app, &[video]), "a video");
    assert!(!grid::all_videos(&h.app, &[video, image]), "a photo among them: photo words");
    assert!(!grid::all_videos(&h.app, &[image]), "a photo");
    assert!(!grid::all_videos(&h.app, &[]), "nothing");
}

/// The texts a drop of videos shows, in both languages.
#[test]
fn the_words_of_a_video_drop_say_video() {
    assert_eq!(crate::panels::left::moved_text(1, "home", true), "Moved 1 video to “home”. Edit → Undo moves them back.");
    assert_eq!(crate::panels::left::moved_text(3, "home", true), "Moved 3 videos to “home”. Edit → Undo moves them back.");
    assert_eq!(crate::panels::left::moved_text(1, "home", false), "Moved 1 photo to “home”. Edit → Undo moves them back.");
    assert_eq!(crate::panels::left::moved_text(0, "home", true), "Already in “home”");
    set_language(Locale::Fr);
    assert_eq!(crate::i18n::tr_format!("{n} video{}", "", n = 1), "1 vidéo");
    assert_eq!(crate::i18n::tr_format!("{n} video{}", "s", n = 4), "4 vidéos");
    assert_eq!(crate::i18n::tr_format!("Added {n} video{} to “{}”", "", "Plage", n = 1), "1 vidéo ajoutée(s) à « Plage »");
    assert_eq!(crate::i18n::tr_format!("Added “{keyword}” to {n} video{}", "", keyword = "montagnes", n = 1), "« montagnes » ajouté à 1 vidéo");
    assert_eq!(
        crate::i18n::tr_format!("Moved {n} video{} to “{}”. Edit → Undo moves them back.", "", "home", n = 1),
        "1 vidéo déplacée vers « home ». Modifier → Annuler les remet en place."
    );
    assert_eq!(crate::i18n::tr_format!("Move {n} video{} to “{}”?", "", "home", n = 1), "Déplacer 1 vidéo vers « home » ?");
    assert_eq!(crate::panels::left::moved_text(1, "home", true), "1 vidéo déplacée vers « home ». Modifier → Annuler les remet en place.");
    assert_eq!(crate::panels::left::moved_text(2, "home", true), "2 vidéos déplacées vers « home ». Modifier → Annuler les remet en place.");
    assert_eq!(tr("Move Videos"), "Déplacer les vidéos");
    assert_eq!(tr("Move Photos"), "Déplacer les photos");
    set_language(Locale::En);
}
