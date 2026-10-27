//! `files::probe_head` (a big raw described from its first megabytes, with a sampled content key) says the same as
//! the whole-file probe on the corpus raws, except for the content key. Skipped when `corpus/raw` is absent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use lightcraft_engine::files::{PROBE_HEAD_MIN, probe_bytes, probe_head, probe_head_why};

#[test]
fn head_probe_matches_the_whole_file_probe() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/raw");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let (mut used, mut refused, mut differ) = (Vec::new(), Vec::new(), Vec::new());
    for e in entries.flatten() {
        let path = e.path();
        let p = path.to_string_lossy().to_string();
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.len() <= PROBE_HEAD_MIN {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut full = probe_bytes(&name, &bytes).unwrap();
        full.content_hash = None;
        match probe_head_why(&p) {
            Err(why) => refused.push(format!("{name}: {why}")),
            Ok(mut fast) => {
                let hash = fast.content_hash.take().unwrap();
                assert!(hash.starts_with("ps1-"), "{name}: {hash}");
                if format!("{fast:?}") == format!("{full:?}") {
                    // the sampled key is stable
                    assert_eq!(probe_head(&p).unwrap().content_hash.unwrap(), hash);
                    used.push(name);
                } else {
                    differ.push(format!("{name}:\n  head  {fast:?}\n  whole {full:?}"));
                }
            }
        }
    }
    println!("HEAD PROBE used for {used:?}\nwhole-file read for {refused:#?}");
    assert!(
        differ.is_empty(),
        "head probe differs from the whole-file probe (take the format off the list in files::probe_head_why):\n{}",
        differ.join("\n")
    );
}

#[test]
fn the_sampled_key_sees_a_changed_tail() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/raw");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.len() <= PROBE_HEAD_MIN {
            continue;
        }
        let tmp = std::env::temp_dir().join(format!("lc-probe-head-{}-{}", std::process::id(), path.file_name().unwrap().to_string_lossy()));
        let mut a = bytes.clone();
        std::fs::write(&tmp, &a).unwrap();
        let Some(h1) = probe_head(&tmp.to_string_lossy()) else {
            let _ = std::fs::remove_file(&tmp);
            continue;
        };
        a.extend_from_slice(b"x");
        std::fs::write(&tmp, &a).unwrap();
        let h2 = probe_head(&tmp.to_string_lossy()).unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert_ne!(h1.content_hash, h2.content_hash, "a longer file has another key");
        return;
    }
}

/// `LC_PROBE_FILES=a.cr2:b.cr2 cargo test -p lightcraft-engine --test probe_head -- --nocapture files_say_why`: for files outside the corpus.
#[test]
fn files_say_why() {
    let Ok(list) = std::env::var("LC_PROBE_FILES") else { return };
    for f in list.split(':').filter(|f| !f.is_empty()) {
        match lightcraft_engine::files::probe_head_why(f) {
            Ok(i) => println!("HEAD OK {f}: {}x{} {} {:?}", i.width, i.height, i.format, i.content_hash),
            Err(e) => println!("HEAD REFUSED {f}: {e}"),
        }
    }
}
