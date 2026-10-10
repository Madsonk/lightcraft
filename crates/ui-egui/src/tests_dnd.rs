//! Drag and drop, headless, like a user: photos from the grid and the filmstrip onto albums, keywords,
//! people and places; faces onto people, groups, "Unknown person" and "Not a real face" in
//! People ▸ Recognition (several people in one photo included). Drops are simulated with the control
//! channel's pointer input (`ui.drag` between widgets), so the whole path runs: the press starts the
//! drag, the release over a target writes, anything else writes nothing. `LIGHTCRAFT_TEST_SHOTS=<folder>`
//! also saves screenshots.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);
const DIM: usize = 512;

fn unit(v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / n).collect()
}

fn rnd(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    unit(
        (0..DIM)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect(),
    )
}

fn face(who: u64, n: u64) -> Vec<f32> {
    let (a, b) = (rnd(100 + who), rnd(9000 + who * 31 + n));
    unit(a.iter().zip(&b).map(|(x, y)| x + 0.5 * y).collect())
}

fn services() -> Services {
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    }
}

fn shot(h: &mut Headless, name: &str) {
    if let Ok(dir) = std::env::var("LIGHTCRAFT_TEST_SHOTS") {
        let _ = std::fs::create_dir_all(&dir);
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&dir).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget `{id}` on screen"))
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn drag(h: &mut Headless, from: &str, to: &str) {
    let (a, b) = (rect(h, from).center(), rect(h, to).center());
    let r = h.request("ui.drag", json!({"x": a.x, "y": a.y, "toX": b.x, "toY": b.y, "steps": 14}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

fn kws(h: &Headless, id: u64) -> Vec<String> {
    h.app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().meta.keywords.clone()
}

fn open_keyword(h: &Headless, path: &str) {
    h.view.ctx.data_mut(|d| d.insert_temp(egui::Id::new(("kw-open", path.to_lowercase())), true));
}

// ------------------------------------------------------------------------------------ photos

fn demo_with_albums() -> (Headless, Vec<u64>, u64, u64, u64) {
    let mut s = lightcraft_engine::Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().filter(|p| p.in_library()).map(|p| p.id.0).take(14).collect();
    assert!(ids.len() >= 12, "the demo library has photos");
    let regular = s.execute("album.create", &json!({"name": "Regular"})).unwrap()["id"].as_u64().unwrap();
    let smart = s.execute("album.createSmart", &json!({"name": "Smart", "rules": {"keyword": "nothing-has-this"}})).unwrap()["id"].as_u64().unwrap();
    let folder = s.execute("album.create", &json!({"name": "Folder", "folder": true})).unwrap()["id"].as_u64().unwrap();
    // keywords in the tree: a plain one, a place (with its parents), a person
    s.execute("photo.setMeta", &json!({"ids": [ids[10]], "addKeywords": ["Vacation", "Places|France|Poitiers"]})).unwrap();
    // people: three photos of Awa (faces injected: no model needed), named
    let photos = ids[..6].iter().enumerate().map(|(i, id)| (*id, vec![(face(1, i as u64), [0.3, 0.2, 0.6, 0.6])])).collect::<Vec<_>>();
    lightcraft_engine::people::inject_faces(&mut s, photos.into_iter().map(|(i, f)| (lightcraft_catalog::PhotoId(i), f)).collect());
    let g = s.execute("people.list", &json!({})).unwrap()["groups"][0]["id"].as_str().unwrap().to_string();
    s.execute("people.name", &json!({"group": g, "name": "Awa"})).unwrap();
    let mut app = LightcraftApp::new(s, services());
    app.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(app, [1500.0, 2200.0], 1.0);
    // the sidebar: only what the test needs, so the keyword rows are in view
    if !h.app.ui.sidebar_section_collapsed("byDate") {
        h.app.ui.toggle_sidebar_section("byDate");
    }
    h.request("engine.execute", json!({"command": "view.leftPanel"}), T);
    for k in ["Places", "Places|France", "People"] {
        open_keyword(&h, k);
    }
    h.settle(SETTLE);
    (h, ids, regular, smart, folder)
}

#[test]
fn photos_dragged_from_the_grid_land_on_albums_keywords_people_and_places_and_nothing_else_writes() {
    let (mut h, ids, regular, smart, folder) = demo_with_albums();
    assert!(h.app.ui.left_panel, "the left panel is shown");
    let vis: Vec<u64> = h.app.session.visible_cloned().iter().map(|p| p.0).collect();
    // photos without a face of Awa (her six are ids[..6]) and without keywords of the test
    let free: Vec<u64> = vis.iter().copied().filter(|p| !ids[..6].contains(p) && *p != ids[10]).collect();
    let pics = [free[0], free[1], free[2]];
    // three photos selected
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": pics}}), T);
    h.settle(SETTLE);
    let undo0 = h.app.session.undo.len();
    let before: Vec<Vec<String>> = pics.iter().map(|p| kws(&h, *p)).collect();

    // releasing over nothing writes nothing (the badge follows the pointer, the drag ends)
    let (a, b) = (rect(&h, &format!("thumb:{}", pics[0])).center(), rect(&h, &format!("thumb:{}", free[3])).center());
    h.request("ui.drag", json!({"x": a.x, "y": a.y, "toX": b.x, "toY": b.y, "steps": 12}), T);
    h.settle(SETTLE);
    assert!(h.app.ui.dragging_photos.is_none(), "the drag ended");
    assert_eq!(h.app.session.undo.len(), undo0, "nothing was written");
    assert_eq!(pics.iter().map(|p| kws(&h, *p)).collect::<Vec<_>>(), before, "no keyword changed");

    // a smart album refuses, with a message; a folder too
    drag(&mut h, &format!("thumb:{}", pics[0]), &format!("source:album:{smart}"));
    assert_eq!(h.app.session.undo.len(), undo0, "a smart album takes nothing");
    assert!(h.app.ui.status.contains("smart album"), "{}", h.app.ui.status);
    assert!(h.app.session.catalog.album(lightcraft_catalog::AlbumId(smart)).unwrap().photos.is_empty());
    drag(&mut h, &format!("thumb:{}", pics[0]), &format!("source:folder:{folder}"));
    assert_eq!(h.app.session.undo.len(), undo0);
    assert!(h.app.ui.status.contains("folder"), "{}", h.app.ui.status);

    // a regular album takes the whole selection, once
    drag(&mut h, &format!("thumb:{}", pics[0]), &format!("source:album:{regular}"));
    let members = h.app.session.catalog.album(lightcraft_catalog::AlbumId(regular)).unwrap().photos.clone();
    assert_eq!(members.len(), 3, "{members:?}");
    assert!(h.app.ui.status.contains("3 photos added"), "{}", h.app.ui.status);
    assert_eq!(h.app.session.undo.len(), undo0 + 1);
    drag(&mut h, &format!("thumb:{}", pics[1]), &format!("source:album:{regular}"));
    assert_eq!(h.app.session.catalog.album(lightcraft_catalog::AlbumId(regular)).unwrap().photos.len(), 3, "no doubles");
    assert!(h.app.ui.status.contains("already in"), "{}", h.app.ui.status);

    // a keyword of the tree: added to the three, one undo step, counts follow, no doubles
    shot(&mut h, "dnd_photos_before_keyword.png");
    drag(&mut h, &format!("thumb:{}", pics[0]), "source:keyword:Vacation");
    for p in pics {
        assert!(kws(&h, p).contains(&"Vacation".to_string()), "{p}: {:?}", kws(&h, p));
    }
    let labels: Vec<&str> = h.app.session.undo.iter().skip(undo0).map(|u| u.label.as_str()).collect();
    assert_eq!(h.app.session.undo.len(), undo0 + 2, "one undo step for the whole drop: {labels:?}");
    assert!(h.app.ui.status.contains("added to 3 photos"), "{}", h.app.ui.status);
    let tree = h.app.session.catalog.keyword_tree();
    assert_eq!(tree.iter().find(|n| n.path == "Vacation").map(|n| n.count), Some(4), "3 dropped + the one that had it");
    drag(&mut h, &format!("thumb:{}", pics[2]), "source:keyword:Vacation");
    assert!(h.app.ui.status.contains("already on"), "{}", h.app.ui.status);
    assert_eq!(kws(&h, pics[2]).iter().filter(|k| *k == "Vacation").count(), 1);
    assert_eq!(h.app.session.undo.len(), undo0 + 2, "an already-present keyword writes nothing");

    // a place (a node of the Places tree): the keyword of that node, nothing above it
    drag(&mut h, &format!("thumb:{}", pics[0]), "source:keyword:Places|France|Poitiers");
    assert!(pics.iter().all(|p| kws(&h, *p).contains(&"Places|France|Poitiers".to_string())));
    assert!(pics.iter().all(|p| !kws(&h, *p).contains(&"Places".to_string())));

    // a person (a node of the People tree): the photos are filed under Awa, a face of hers found or not
    assert!(has(&h, "source:keyword:People|Awa"), "the People tree shows Awa");
    drag(&mut h, &format!("thumb:{}", pics[0]), "source:keyword:People|Awa");
    for p in pics {
        assert!(kws(&h, p).contains(&"People|Awa".to_string()), "{p}");
    }
    let faces = h.app.session.execute("people.faces", &json!({"id": pics[2]})).unwrap();
    assert_eq!(faces["filedUnder"][0]["name"], "Awa", "{faces}");
    // a sync keeps them: they are hand-filed, not derived from a face
    let sy = h.app.session.execute("people.sync", &json!({})).unwrap();
    assert_eq!(sy["removed"], 0, "{sy}");
    assert!(pics.iter().all(|p| kws(&h, *p).contains(&"People|Awa".to_string())));
    shot(&mut h, "dnd_photos_after.png");

    // People ▸ Undo takes the drop on a person back in one step: the keyword and the hand-filing
    // (the sync above changed nothing, so it left no step of its own to undo first)
    h.app.run("people.undo", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(pics.iter().all(|p| !kws(&h, *p).contains(&"People|Awa".to_string())), "undone");
    let faces = h.app.session.execute("people.faces", &json!({"id": pics[2]})).unwrap();
    assert!(faces["filedUnder"].as_array().unwrap().is_empty(), "{faces}");
    // and the keyword drops before it are ordinary undo steps
    h.request("engine.execute", json!({"command": "edit.undo"}), T);
    h.settle(SETTLE);
    assert!(pics.iter().all(|p| !kws(&h, *p).contains(&"Places|France|Poitiers".to_string())), "the place keyword undone");
    assert!(pics.iter().all(|p| kws(&h, *p).contains(&"Vacation".to_string())), "the earlier one stays");
}

#[test]
fn photos_dragged_from_the_filmstrip_land_on_an_album_too() {
    let (mut h, _ids, regular, _smart, _folder) = demo_with_albums();
    let vis: Vec<u64> = h.app.session.visible_cloned().iter().map(|p| p.0).collect();
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [vis[1]]}}), T);
    let r = h.request("ui.set", json!({"view": "detail", "filmstrip": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let film = format!("film:{}", vis[2]);
    assert!(has(&h, &film), "the filmstrip is shown");
    shot(&mut h, "dnd_filmstrip.png");
    drag(&mut h, &film, &format!("source:album:{regular}"));
    let members = h.app.session.catalog.album(lightcraft_catalog::AlbumId(regular)).unwrap().photos.clone();
    assert_eq!(members, vec![lightcraft_catalog::PhotoId(vis[2])], "the dragged photo became the selection and joined the album");
    // and a keyword
    drag(&mut h, &format!("film:{}", vis[3]), "source:keyword:Vacation");
    assert!(kws(&h, vis[3]).contains(&"Vacation".to_string()));
}

// ------------------------------------------------------------------------------------ faces

/// The demo library with faces injected: p0 A, p1 A, p2 A+B, p3 B, p4 B, p5 A+C, p6 C, p7 C.
fn faces_app() -> (Headless, Vec<u64>) {
    let mut s = lightcraft_engine::Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().filter(|p| p.in_library()).map(|p| p.id.0).take(8).collect();
    assert!(ids.len() >= 8);
    let big = [0.3, 0.2, 0.6, 0.6];
    let right = [0.65, 0.2, 0.9, 0.6];
    let layout: Vec<Vec<(u64, [f32; 4])>> = vec![
        vec![(1, big)],
        vec![(1, big)],
        vec![(1, big), (2, right)],
        vec![(2, big)],
        vec![(2, big)],
        vec![(1, big), (3, right)],
        vec![(3, big)],
        vec![(3, big)],
    ];
    let photos = ids
        .iter()
        .zip(layout)
        .enumerate()
        .map(|(i, (id, fs))| {
            (lightcraft_catalog::PhotoId(*id), fs.into_iter().enumerate().map(|(k, (w, r))| (face(w, (i * 3 + k) as u64), r)).collect())
        })
        .collect();
    lightcraft_engine::people::inject_faces(&mut s, photos);
    let mut app = LightcraftApp::new(s, services());
    app.ui.view = ViewMode::People;
    app.caches.people_rec.recognition = true;
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    h.settle(SETTLE);
    let t0 = std::time::Instant::now();
    while !h.app.caches.people_rec.overview().is_some_and(|o| o.grouped) && t0.elapsed() < SETTLE {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    h.settle(SETTLE);
    (h, ids)
}

fn faces_of(h: &mut Headless, photo: u64) -> Vec<Value> {
    h.app.session.execute("people.faces", &json!({"id": photo})).unwrap()["faces"].as_array().unwrap().clone()
}

fn group_of(h: &mut Headless, face: &str) -> String {
    let l = h.app.session.execute("people.list", &json!({"groups": 50, "thumbs": 20})).unwrap();
    for key in ["groups", "unknown"] {
        for g in l[key].as_array().unwrap() {
            if g["thumbs"].as_array().unwrap().iter().any(|t| t["face"] == face) {
                return g["id"].as_str().unwrap().to_string();
            }
        }
    }
    panic!("no group with {face}: {l}");
}

fn name_group(h: &mut Headless, photo: u64, idx: usize, name: &str) -> String {
    let f = faces_of(h, photo)[idx]["face"].as_str().unwrap().to_string();
    let g = group_of(h, &f);
    let r = h.app.session.execute("people.name", &json!({"group": g, "name": name})).unwrap();
    h.app.caches.people_rec.dirty = true;
    h.settle(SETTLE);
    r["id"].as_str().unwrap().to_string()
}

fn face_id(h: &mut Headless, photo: u64, idx: usize) -> String {
    faces_of(h, photo)[idx]["face"].as_str().unwrap().to_string()
}

fn who(h: &mut Headless, photo: u64, idx: usize) -> (Option<String>, bool, bool) {
    let f = &faces_of(h, photo)[idx];
    (f["name"].as_str().map(String::from), f["unknown"].as_bool().unwrap_or(false), f["ignored"].as_bool().unwrap_or(false))
}

#[test]
fn faces_move_between_people_one_face_at_a_time_in_photos_with_several_people() {
    let (mut h, ids) = faces_app();
    let ov = h.app.caches.people_rec.overview().cloned().unwrap();
    assert_eq!((ov.indexed_faces, ov.groups.len()), (10, 3), "{ov:?}");
    // the group cards, with their buttons
    let gid = ov.groups[0].id.clone();
    h.app.caches.people_rec.dirty = true;
    h.step_until(SETTLE, |h| has(h, &format!("recGroup:{gid}")));
    for id in [format!("recGroup:{gid}"), format!("recBtn:unknown:{gid}"), format!("recBtn:skip:{gid}"), format!("recBtn:notface:{gid}")] {
        assert!(has(&h, &id), "{id}");
    }
    assert!(has(&h, "recDrop:unknown") && has(&h, "recDrop:ignored"), "the two zones are always there");
    shot(&mut h, "dnd_faces_groups.png");

    let awa = name_group(&mut h, ids[0], 0, "Awa");
    let bao = name_group(&mut h, ids[3], 0, "Bao");
    let kw = |h: &Headless, i: usize| {
        let mut k = kws(h, ids[i]);
        k.retain(|x| x.starts_with("People|"));
        k.sort();
        k
    };
    assert_eq!(kw(&h, 2), vec!["People|Awa".to_string(), "People|Bao".to_string()], "photo 2 holds two people");
    assert_eq!(kw(&h, 5), vec!["People|Awa".to_string()]);
    let undo0 = h.app.session.undo.len();

    // open Awa's faces: each of her faces is a tile, the photo with two people shows both faces named
    click(&mut h, &format!("recPersonFaces:{awa}"));
    let (a2, b2) = (face_id(&mut h, ids[2], 0), face_id(&mut h, ids[2], 1));
    assert!(has(&h, &format!("recFace:{a2}")) && has(&h, &format!("recFace:{b2}")), "both faces of the shared photo are on screen");
    shot(&mut h, "dnd_faces_person_open.png");

    // hovering a face lists the faces of its photo (and who they are)
    h.request("ui.hoverWidget", json!({"id": format!("recFace:{a2}")}), T);
    h.settle(SETTLE);
    assert!(h.app.caches.people_rec.cached_photos() >= 1, "the photo's faces were looked up for the tooltip");
    shot(&mut h, "dnd_faces_hover.png");
    h.request("ui.move", json!({"x": 5.0, "y": 5.0}), T);

    // drag Awa's face of photo 2 onto Bao: the face changes person, photo 2 keeps only Bao (no other Awa face in it)
    drag(&mut h, &format!("recFace:{a2}"), &format!("recPerson:{bao}"));
    assert_eq!(who(&mut h, ids[2], 0).0.as_deref(), Some("Bao"));
    assert_eq!(who(&mut h, ids[2], 1).0.as_deref(), Some("Bao"));
    assert_eq!(kw(&h, 2), vec!["People|Bao".to_string()]);
    assert_eq!(kw(&h, 0), vec!["People|Awa".to_string()], "her other photos are untouched");
    assert_eq!(kw(&h, 5), vec!["People|Awa".to_string()]);
    assert_eq!(h.app.session.undo.len(), undo0 + 1, "one undo step");
    assert!(h.app.ui.status.contains("1 face moved to Bao"), "{}", h.app.ui.status);
    assert!(h.app.ui.dragging_faces.is_none());
    // undo in one gesture
    h.app.run("people.undo", json!({})).unwrap();
    h.app.caches.people_rec.dirty = true;
    h.settle(SETTLE);
    assert_eq!(kw(&h, 2), vec!["People|Awa".to_string(), "People|Bao".to_string()]);
    assert_eq!(who(&mut h, ids[2], 0).0.as_deref(), Some("Awa"));

    // Awa's face of photo 2 is "Unknown person": photo 2 keeps Bao; the face is marked
    drag(&mut h, &format!("recFace:{a2}"), "recDrop:unknown");
    assert_eq!(who(&mut h, ids[2], 0), (None, true, false));
    assert_eq!(kw(&h, 2), vec!["People|Bao".to_string()]);
    assert!(h.app.ui.status.contains("unknown"), "{}", h.app.ui.status);
    h.app.run("people.undo", json!({})).unwrap();
    h.app.caches.people_rec.dirty = true;
    h.settle(SETTLE);

    // pick two of Awa's faces (photos 0 and 1), drag one: both go to "Not a real face"
    let (f0, f1) = (face_id(&mut h, ids[0], 0), face_id(&mut h, ids[1], 0));
    click(&mut h, &format!("recFace:{f0}"));
    click(&mut h, &format!("recFace:{f1}"));
    assert_eq!(h.app.caches.people_rec.selected.len(), 2);
    shot(&mut h, "dnd_faces_picked.png");
    drag(&mut h, &format!("recFace:{f0}"), "recDrop:ignored");
    assert_eq!(who(&mut h, ids[0], 0), (None, false, true));
    assert_eq!(who(&mut h, ids[1], 0), (None, false, true));
    assert!(h.app.ui.status.contains("2 faces marked as not a real face"), "{}", h.app.ui.status);
    assert!(h.app.caches.people_rec.selected.is_empty(), "the pick is cleared after the drop");
    // photo 2 still has both of its people
    assert_eq!(kw(&h, 2), vec!["People|Awa".to_string(), "People|Bao".to_string()]);
    h.app.run("people.undo", json!({})).unwrap();
    h.app.caches.people_rec.dirty = true;
    h.settle(SETTLE);
    assert_eq!(who(&mut h, ids[0], 0).0.as_deref(), Some("Awa"));

    // releasing a face on empty space changes nothing
    let n = h.app.session.undo.len();
    let (a, b) = (rect(&h, &format!("recFace:{f0}")).center(), egui::pos2(700.0, 40.0));
    h.request("ui.drag", json!({"x": a.x, "y": a.y, "toX": b.x, "toY": b.y, "steps": 10}), T);
    h.settle(SETTLE);
    assert_eq!(h.app.session.undo.len(), n);
    assert_eq!(who(&mut h, ids[0], 0).0.as_deref(), Some("Awa"));
    assert!(h.app.ui.dragging_faces.is_none());
}

#[test]
fn the_group_buttons_unknown_skip_and_not_a_real_face_and_the_unknown_section() {
    let (mut h, ids) = faces_app();
    let _awa = name_group(&mut h, ids[0], 0, "Awa");
    let _bao = name_group(&mut h, ids[3], 0, "Bao");
    // what is left: Cleo's group (photos 5's right face, 6, 7)
    let f6 = face_id(&mut h, ids[6], 0);
    let g = group_of(&mut h, &f6);
    assert!(has(&h, &format!("recBtn:unknown:{g}")));
    let undo0 = h.app.session.undo.len();

    // Skip for now: hidden, nothing decided (not an undo step), a note says so, "Show them" brings it back
    click(&mut h, &format!("recBtn:skip:{g}"));
    assert!(!has(&h, &format!("recGroup:{g}")), "the group is hidden");
    assert!(has(&h, "recBtn:unskip"));
    assert_eq!(h.app.session.undo.len(), undo0);
    let hist = h.app.session.execute("people.status", &json!({})).unwrap();
    assert_eq!(hist["index"]["skippedFaces"], 3, "{hist}");
    click(&mut h, "recBtn:unskip");
    let g = group_of(&mut h, &f6);
    assert!(has(&h, &format!("recGroup:{g}")), "back");

    // Unknown person: it leaves the groups to name, waits in the Unknown section, no keyword
    click(&mut h, &format!("recBtn:unknown:{g}"));
    let ov = h.app.caches.people_rec.overview().cloned().unwrap();
    assert_eq!((ov.groups_total, ov.unknown_total), (0, 1), "{ov:?}");
    let ug = ov.unknown[0].id.clone();
    assert!(has(&h, &format!("recBtn:known:{ug}")), "the unknown section is shown");
    assert!(ids.iter().all(|i| !kws(&h, *i).iter().any(|k| k == "People|Unknown")));
    shot(&mut h, "dnd_faces_unknown_section.png");
    // later: drag the whole unknown group onto Awa by its grip
    drag(&mut h, &format!("recGroupGrip:{ug}"), &format!("recPerson:{_awa}"));
    let ov = h.app.caches.people_rec.overview().cloned().unwrap();
    assert_eq!(ov.unknown_total, 0, "{ov:?}");
    assert_eq!(who(&mut h, ids[6], 0).0.as_deref(), Some("Awa"));
    assert!(kws(&h, ids[6]).contains(&"People|Awa".to_string()));
    h.app.run("people.undo", json!({})).unwrap();
    h.app.caches.people_rec.dirty = true;
    h.step_until(SETTLE, |h| h.app.caches.people_rec.overview().is_some_and(|o| o.unknown_total == 1));
    h.settle(SETTLE);
    let ug = h.app.caches.people_rec.overview().cloned().unwrap().unknown[0].id.clone();

    // name an unknown group from its card
    h.app.run("people.name", json!({"group": ug, "name": "Cleo"})).unwrap();
    h.app.caches.people_rec.dirty = true;
    h.settle(SETTLE);
    assert!(kws(&h, ids[7]).contains(&"People|Cleo".to_string()));
    assert_eq!(h.app.caches.people_rec.overview().cloned().unwrap().unknown_total, 0);
}
