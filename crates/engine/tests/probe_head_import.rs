//! Importing with the sampled key of big raws next to a catalogue made with full hashes: nothing is lost or doubled,
//! old photos keep their key (so their cached thumbnails stay valid). Skipped when `corpus/raw` is absent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::files::{PROBE_HEAD_MIN, fs_hooks, probe_bytes, probe_head};
use lightcraft_engine::import::{ImportMode, import};

fn big_raws(n: usize) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/raw");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.metadata().is_ok_and(|m| m.len() as usize > PROBE_HEAD_MIN))
        // the files the head probe settles (the others are read whole whichever way they are imported)
        .filter(|p| probe_head(&p.to_string_lossy()).is_some())
        .collect();
    v.sort();
    v.truncate(n);
    v
}

fn put(dir: &Path, src: &Path, tag: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let mut b = std::fs::read(src).unwrap();
    b.extend_from_slice(tag.as_bytes());
    let to = dir.join(src.file_name().unwrap());
    std::fs::write(&to, b).unwrap();
    to
}

#[test]
fn old_catalogue_and_sampled_keys_live_together() {
    let raws = big_raws(3);
    if raws.len() < 3 {
        return;
    }
    let root = std::env::temp_dir().join(format!("lc-probe-head-import-{}", std::process::id()));
    let (a, b) = (root.join("a"), root.join("b"));
    let _ = std::fs::remove_dir_all(&root);
    for r in &raws[..2] {
        put(&a, r, "tag-a");
        let to = b.join(r.file_name().unwrap());
        std::fs::create_dir_all(&b).unwrap();
        std::fs::copy(a.join(r.file_name().unwrap()), to).unwrap(); // the same bytes in another folder
    }
    let mut s = lightcraft_engine::Session::new().with_fs();
    // 1. a catalogue made the old way: the hash of every byte
    s.media.file_probe = Some(Arc::new(|p: &str| probe_bytes(p.rsplit('/').next().unwrap(), &std::fs::read(p).map_err(|e| e.to_string())?)));
    let old = import(&mut s, &[a.to_string_lossy().to_string()], ImportMode::Add).unwrap();
    assert_eq!(old.imported.len(), 2, "{:?}", old.failed);
    let old_hashes: Vec<_> = old.imported.iter().map(|i| s.catalog.photo(PhotoId(*i)).unwrap().content_hash.clone().unwrap()).collect();
    assert!(old_hashes.iter().all(|h| !h.contains('-')), "{old_hashes:?}");
    // 2. the new probe: the same folder is the same photos, the same bytes elsewhere too, a new file is new
    s.media.file_probe = Some(fs_hooks().1);
    let again = import(&mut s, &[a.to_string_lossy().to_string()], ImportMode::Add).unwrap();
    assert_eq!((again.imported.len(), again.duplicates.len()), (0, 2));
    let copy = import(&mut s, &[b.to_string_lossy().to_string()], ImportMode::Add).unwrap();
    assert_eq!((copy.imported.len(), copy.duplicates.len()), (0, 2), "a copy of an old photo is a duplicate, not a new photo");
    let fresh = put(&root.join("c"), &raws[2], "tag-c");
    let new = import(&mut s, &[fresh.to_string_lossy().to_string()], ImportMode::Add).unwrap();
    assert_eq!(new.imported.len(), 1, "{:?}", new.failed);
    let key = s.catalog.photo(PhotoId(new.imported[0])).unwrap().content_hash.clone().unwrap();
    assert!(key.starts_with("ps1-"), "{key}");
    // 3. the old photos kept their key
    let kept: Vec<_> = old.imported.iter().map(|i| s.catalog.photo(PhotoId(*i)).unwrap().content_hash.clone().unwrap()).collect();
    assert_eq!(kept, old_hashes);
    // 4. and the new photo, imported again from another folder, is a duplicate of itself
    let twin = put(&root.join("d"), &raws[2], "tag-c");
    let dup = import(&mut s, &[twin.parent().unwrap().to_string_lossy().to_string()], ImportMode::Add).unwrap();
    assert_eq!((dup.imported.len(), dup.duplicates.len()), (0, 1));
    let _ = std::fs::remove_dir_all(&root);
}
