//! The prefix read of an embedded preview (`files::preview_from_prefix`) gives what a whole-file read gives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use lightcraft_engine::files::{PREVIEW_PREFIX, embedded_preview_srgb, is_whole_jpeg, preview_from_prefix};

#[test]
fn a_jpeg_cut_off_after_its_exif_thumbnail_is_not_whole() {
    // SOI, APP1 holding a tiny JPEG thumbnail (with its own EOI), then the main scan
    let thumb = [0xff, 0xd8, 0xff, 0xd9];
    let mut app1 = vec![0xff, 0xe1];
    app1.extend(((thumb.len() + 2) as u16).to_be_bytes());
    app1.extend(thumb);
    let mut whole = vec![0xff, 0xd8];
    whole.extend(&app1);
    whole.extend([0xff, 0xda, 0x00, 0x02, 0x11, 0x22, 0x33, 0xff, 0xd9]);
    assert!(is_whole_jpeg(&whole));
    // cut inside the scan: the last FF D9 is the thumbnail's, but the file does not end with it
    assert!(!is_whole_jpeg(&whole[..whole.len() - 4]));
    // a file that stops right after the thumbnail ends with FF D9 and has no scan
    let mut cut = vec![0xff, 0xd8];
    cut.extend(&app1);
    assert!(!is_whole_jpeg(&cut), "no start of scan");
    assert!(!is_whole_jpeg(&[]) && !is_whole_jpeg(&[0xff, 0xd8]) && !is_whole_jpeg(&[0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff, 0xff, 0xd9]));
}

/// The prefix read gives the same picture as reading the whole raw (corpus files, skipped when absent).
#[test]
fn prefix_preview_matches_the_whole_file_read() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/raw");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut used = 0;
    for e in entries.flatten() {
        let path = e.path();
        let p = path.to_string_lossy().to_string();
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.len() <= PREVIEW_PREFIX * 2 {
            continue;
        }
        let Some(fast) = preview_from_prefix(&p, 256) else { continue };
        let slow = embedded_preview_srgb(&bytes, 256).expect("whole-file preview");
        assert_eq!((fast.width, fast.height), (slow.width, slow.height), "{p}");
        let mean = fast.data.iter().zip(&slow.data).map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c]) as u32).sum::<u32>()).sum::<u32>() as f64
            / (fast.data.len() * 3).max(1) as f64;
        assert!(mean < 3.0, "{p}: mean difference {mean}");
        used += 1;
    }
    eprintln!("prefix path used for {used} corpus files");
}
