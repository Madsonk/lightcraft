//! `people.migrate`: the earlier people index (`people.json` + `faces-<model>.bin`) becomes named
//! face regions of the catalogue, nothing is lost, originals are never written, a second run adds
//! nothing, and one undo step takes it all back.

use std::path::Path;

use lightcraft_catalog::PhotoId;
use lightcraft_geom::Rect;
use lightcraft_meta::{Region, RegionKind};
use serde_json::{Value, json};

use crate::Session;
use crate::tests_video::{ids, import, library};

const REC: usize = 8 + 2 + 2 + 2 + 2 + 16 + 12 + 512 * 2;

fn write_png(path: &Path, seed: u8) {
    let data: Vec<[u8; 4]> = (0..64 * 48).map(|i| [(i % 64) as u8 * 3, (i / 64) as u8 * 4, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A legacy `faces-<model>.bin`: per photo key, its faces' boxes.
fn legacy_bin(faces: &[(u64, Vec<[f32; 4]>)]) -> Vec<u8> {
    let tag = b"test-model";
    let mut out = b"LCPF".to_vec();
    out.extend(1u32.to_le_bytes());
    out.extend((REC as u32).to_le_bytes());
    out.extend((tag.len() as u32).to_le_bytes());
    out.extend(tag);
    for (key, boxes) in faces {
        for (i, b) in boxes.iter().enumerate() {
            out.extend(key.to_le_bytes());
            out.extend((i as u16).to_le_bytes());
            out.extend((boxes.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(0u16.to_le_bytes());
            for x in b {
                out.extend(x.to_le_bytes());
            }
            for x in [0.9f32, 80.0, 0.0] {
                out.extend(x.to_le_bytes());
            }
            out.extend(vec![0u8; 512 * 2]);
        }
    }
    out
}

fn region(s: &Session, id: PhotoId) -> Vec<(Option<String>, [f64; 4])> {
    s.catalog.photo(id).unwrap().meta.regions.iter().map(|r| (r.name.clone(), [r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1])).collect()
}

#[test]
fn people_of_the_earlier_index_become_named_regions_without_loss() {
    let (mut s, dir) = library();
    let pics = dir.join("pics");
    std::fs::create_dir_all(&pics).unwrap();
    for (i, n) in ["a.png", "b.png", "c.png"].iter().enumerate() {
        write_png(&pics.join(n), 10 + i as u8 * 40);
    }
    let r = import(&mut s, &pics);
    let all = ids(&r);
    assert_eq!(all.len(), 3);
    let key = |id: PhotoId| crate::video::item_key(&crate::media::content_key(s.catalog.photo(id).unwrap()));
    let (a, b, c) = (all[0], all[1], all[2]);
    let (ka, kb, kc) = (key(a), key(b), key(c));
    let original_bytes: Vec<Vec<u8>> = ["a.png", "b.png", "c.png"].iter().map(|n| std::fs::read(pics.join(n)).unwrap()).collect();
    // catalogue before: b already has an unnamed region over Anna's face; c has "Zed" over Bob's
    let mut mb = s.catalog.photo(b).unwrap().meta.clone();
    mb.regions.push(Region { rect: Rect { x0: 0.11, y0: 0.11, x1: 0.41, y1: 0.41 }, kind: RegionKind::Face, name: None, description: None });
    s.commit("t", lightcraft_catalog::Op::SetMeta { id: b, meta: Box::new(mb) }).unwrap();
    let mut mc = s.catalog.photo(c).unwrap().meta.clone();
    mc.regions.push(Region { rect: Rect { x0: 0.5, y0: 0.5, x1: 0.8, y1: 0.8 }, kind: RegionKind::Face, name: Some("Zed".into()), description: None });
    s.commit("t", lightcraft_catalog::Op::SetMeta { id: c, meta: Box::new(mc) }).unwrap();
    // the earlier index
    let lib = dir.join("lib");
    let pdir = lib.join("people");
    std::fs::create_dir_all(&pdir).unwrap();
    let ghost = 0xdead_beef_u64;
    std::fs::write(
        pdir.join("faces-test.bin"),
        legacy_bin(&[
            (ka, vec![[0.1, 0.1, 0.4, 0.4], [0.5, 0.5, 0.8, 0.8]]),
            (kb, vec![[0.1, 0.1, 0.4, 0.4]]),
            (kc, vec![[0.5, 0.5, 0.8, 0.8]]),
            (ghost, vec![[0.2, 0.2, 0.3, 0.3]]),
        ]),
    )
    .unwrap();
    let people = json!({
        "format": 1, "next_id": 5,
        "persons": [
            {"id": 1, "name": "Anna", "faces": [[ka, 0], [kb, 0]]},
            {"id": 2, "name": "Bob", "faces": [[ka, 1], [kc, 0], [ghost, 0], [ka, 9]]},
            {"id": 3, "faces": []},
            {"id": 4, "faces": [[kc, 0]]}
        ],
        "rejected": [[[ka, 0], 2]], "different": [[[ka, 0], [kb, 0]]], "ignored": [[ghost, 0]], "unknown": [], "settings": {"link": 0.5}
    });
    std::fs::write(pdir.join("people.json"), serde_json::to_vec(&people).unwrap()).unwrap();
    let before_json = std::fs::read(pdir.join("people.json")).unwrap();

    // a dry run changes nothing and says everything
    let dry = s.execute("people.migrate", &json!({})).unwrap();
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(dry["legacy"], json!({"persons": 4, "named": 2, "faces": 7, "rejected": 1, "different": 1, "ignored": 1, "unknown": 0}), "{dry}");
    assert_eq!(dry["faces"]["noRecord"], 1, "{dry}");
    assert_eq!(dry["faces"]["photoNotInCatalogue"], 1, "{dry}");
    assert_eq!(dry["faces"]["conflicts"], 1, "Bob over Zed: {dry}");
    assert_eq!(region(&s, a).len(), 0);
    assert!(!pdir.join("legacy-people.json").exists());

    // applied
    let done = s.execute("people.migrate", &json!({"dryRun": false})).unwrap();
    let backup = done["backup"].as_str().expect("a backup path");
    assert!(Path::new(backup).join("people").join("people.json").is_file(), "{done}");
    assert_eq!(std::fs::read(Path::new(backup).join("people").join("people.json")).unwrap(), before_json);
    assert!(pdir.join("legacy-people.json").is_file());
    assert_eq!(std::fs::read(pdir.join("people.json")).unwrap(), before_json, "the earlier file is never changed");
    let ra = region(&s, a);
    assert!(ra.iter().any(|(n, _)| n.as_deref() == Some("Anna")) && ra.iter().any(|(n, _)| n.as_deref() == Some("Bob")), "{ra:?}");
    let rb = region(&s, b);
    assert_eq!(rb.len(), 1, "the region already there is named, not doubled: {rb:?}");
    assert_eq!(rb[0].0.as_deref(), Some("Anna"));
    let rc = region(&s, c);
    assert_eq!(rc.iter().filter(|(n, _)| n.as_deref() == Some("Zed")).count(), 1, "a name already there is never replaced: {rc:?}");
    assert!(!rc.iter().any(|(n, _)| n.as_deref() == Some("Bob")));
    // originals and sidecars untouched
    for (n, bytes) in ["a.png", "b.png", "c.png"].iter().zip(&original_bytes) {
        assert_eq!(&std::fs::read(pics.join(n)).unwrap(), bytes, "{n} was written");
    }
    assert!(!pics.join("a.xmp").exists() && !pics.join("a.png.xmp").exists(), "a sidecar was written");
    assert_eq!(done["verification"]["namesShort"], 0, "{done}");

    // a second run adds nothing
    let again = s.execute("people.migrate", &json!({"dryRun": false})).unwrap();
    assert_eq!(again["photosChanged"], 0, "{again}");
    assert_eq!(region(&s, a), ra);

    // the legacy decisions stay readable
    let l = s.execute("people.legacy", &json!({})).unwrap();
    assert_eq!(l["unnamed"], 2);
    assert_eq!(l["rejected"].as_array().map(Vec::len), Some(1));

    // one undo step takes the migration back (the second run changed nothing and made no step)
    s.undo_step().unwrap();
    assert!(region(&s, a).is_empty(), "undo restores the catalogue");
    assert!(region(&s, b).iter().all(|(n, _)| n.is_none()));
}

#[test]
fn a_library_without_the_earlier_index_is_left_alone() {
    let (mut s, _dir) = library();
    let e = s.execute("people.migrate", &json!({})).unwrap_err().to_string();
    assert!(e.contains("people.json"), "{e}");
    let _: Value = json!(null);
}

/// Not a test of its own: the report of `people.migrate` on a COPY of a real library
/// (`LC_MIGRATE_COPY` = the copy's folder, `LC_MIGRATE_REPORT` = where to write the JSON,
/// `LC_MIGRATE_APPLY=1` to apply on the copy). `cargo test … -- --ignored real_copy`.
#[test]
#[ignore]
fn real_copy_report() {
    let (Some(copy), Some(out)) = (std::env::var_os("LC_MIGRATE_COPY"), std::env::var_os("LC_MIGRATE_REPORT")) else { return };
    let apply = std::env::var("LC_MIGRATE_APPLY").is_ok_and(|v| v == "1");
    let mut s = Session::new().with_fs();
    s.open_library(std::path::PathBuf::from(copy), false).unwrap();
    let photos = s.catalog.photos().filter(|p| p.in_library()).count();
    let mut report = s.execute("people.migrate", &json!({"dryRun": !apply})).unwrap();
    report["photosInCatalogue"] = json!(photos);
    std::fs::write(out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
}
