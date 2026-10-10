//! Build `lens-profiles.json.z` from the XML data files of the lensfun lens database (data only, CC BY-SA 3.0).
//!
//! `cargo run -p lightcraft-lens --example build_db -- <db-dir> <out.json.z> [revision] [retrieved-date]`
//!
//! `<db-dir>` holds the database's `*.xml` files (`mil-canon.xml`, `slr-canon.xml`, …). The output is a
//! derivative of those files: keep the attribution and the CC BY-SA 3.0 notice with it (see `data/README.md`).
//! This tool reads the XML data format only; it is not derived from any lensfun code.

use std::collections::BTreeMap;
use std::path::Path;

use lightcraft_lens::{Camera, Database, DistModel, Distortion, LensEntry, Mount, Provenance, Tca, TcaModel, Vignetting};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == name.as_bytes()).and_then(|a| a.unescape_value().ok().map(|v| v.to_string()))
}

fn num(e: &BytesStart, name: &str) -> Option<f64> {
    attr(e, name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

#[derive(Default)]
struct Raw {
    mounts: Vec<Mount>,
    cameras: Vec<Camera>,
    lenses: Vec<LensEntry>,
}

fn parse(xml: &str, out: &mut Raw) -> Result<(), String> {
    let mut r = Reader::from_str(xml);
    r.config_mut().trim_text(true);
    let mut path: Vec<String> = Vec::new();
    let mut mount = Mount::default();
    let mut cam = Camera::default();
    let mut lens = LensEntry::default();
    let mut lens_lang_names: Vec<(bool, String)> = Vec::new();
    let mut cam_lang_names: Vec<(bool, String)> = Vec::new();
    let mut lens_maker_seen = false;
    let mut cam_maker_seen = false;
    let mut lang: Option<String> = None;
    loop {
        let ev = r.read_event().map_err(|e| format!("xml: {e}"))?;
        let empty = matches!(ev, Event::Empty(_));
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "mount" if path.last().map(String::as_str) == Some("lensdatabase") => mount = Mount::default(),
                    "camera" => {
                        cam = Camera::default();
                        cam_lang_names.clear();
                        cam_maker_seen = false;
                    }
                    "lens" => {
                        lens = LensEntry::default();
                        lens_lang_names.clear();
                        lens_maker_seen = false;
                    }
                    "focal" if path.last().map(String::as_str) == Some("lens") => {
                        let v = num(e, "value");
                        let (lo, hi) = (num(e, "min").or(v), num(e, "max").or(v));
                        if let (Some(lo), Some(hi)) = (lo, hi) {
                            lens.focal = Some((lo, hi));
                        }
                    }
                    "aperture" if path.last().map(String::as_str) == Some("lens") => {
                        let v = num(e, "value");
                        let (lo, hi) = (num(e, "min").or(v), num(e, "max").or(v));
                        if let (Some(lo), Some(hi)) = (lo, hi) {
                            lens.aperture = Some((lo, hi));
                        }
                    }
                    "distortion" => {
                        let (Some(focal), Some(m)) = (num(e, "focal"), attr(e, "model")) else { continue };
                        let (model, k) = match m.as_str() {
                            "ptlens" => (DistModel::Ptlens, [num(e, "a").unwrap_or(0.0), num(e, "b").unwrap_or(0.0), num(e, "c").unwrap_or(0.0)]),
                            "poly3" => (DistModel::Poly3, [num(e, "k1").unwrap_or(0.0), 0.0, 0.0]),
                            "poly5" => (DistModel::Poly5, [num(e, "k1").unwrap_or(0.0), num(e, "k2").unwrap_or(0.0), 0.0]),
                            _ => continue,
                        };
                        lens.distortion.push(Distortion { focal, model, k });
                    }
                    "tca" => {
                        let (Some(focal), Some(m)) = (num(e, "focal"), attr(e, "model")) else { continue };
                        let (model, k) = match m.as_str() {
                            "linear" => (TcaModel::Linear, [num(e, "kr").unwrap_or(1.0), num(e, "kb").unwrap_or(1.0), 0.0, 0.0, 0.0, 0.0]),
                            "poly3" => (
                                TcaModel::Poly3,
                                [
                                    num(e, "vr").unwrap_or(1.0),
                                    num(e, "vb").unwrap_or(1.0),
                                    num(e, "cr").unwrap_or(0.0),
                                    num(e, "cb").unwrap_or(0.0),
                                    num(e, "br").unwrap_or(0.0),
                                    num(e, "bb").unwrap_or(0.0),
                                ],
                            ),
                            _ => continue,
                        };
                        lens.tca.push(Tca { focal, model, k });
                    }
                    "vignetting" => {
                        if attr(e, "model").as_deref() != Some("pa") {
                            continue;
                        }
                        let (Some(focal), Some(aperture), Some(distance)) = (num(e, "focal"), num(e, "aperture"), num(e, "distance")) else {
                            continue;
                        };
                        lens.vignetting.push(Vignetting {
                            focal,
                            aperture,
                            distance,
                            k: [num(e, "k1").unwrap_or(0.0), num(e, "k2").unwrap_or(0.0), num(e, "k3").unwrap_or(0.0)],
                        });
                    }
                    _ => {}
                }
                lang = attr(e, "lang");
                if !empty {
                    path.push(name);
                }
            }
            Event::End(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                path.pop();
                match name.as_str() {
                    "mount" if path.last().map(String::as_str) == Some("lensdatabase") => out.mounts.push(std::mem::take(&mut mount)),
                    "camera" => {
                        cam.names = ordered(&cam_lang_names);
                        if !cam.maker.is_empty() && !cam.names.is_empty() && cam.crop > 0.0 {
                            out.cameras.push(std::mem::take(&mut cam));
                        }
                    }
                    "lens" => {
                        lens.names = ordered(&lens_lang_names);
                        derive_ranges(&mut lens);
                        if !lens.names.is_empty() && lens.crop > 0.0 {
                            out.lenses.push(std::mem::take(&mut lens));
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                let text = t.decode().map_err(|e| format!("xml text: {e}"))?;
                let text = quick_xml::escape::unescape(&text).map_err(|e| format!("xml escape: {e}"))?.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let n = path.len();
                let (cur, parent) = (path.get(n.wrapping_sub(1)).map(String::as_str), path.get(n.wrapping_sub(2)).map(String::as_str));
                match (parent, cur) {
                    (Some("lensdatabase"), _) => {}
                    (Some("mount"), Some("name")) => mount.name = text,
                    (Some("mount"), Some("compat")) => mount.compat.push(text),
                    (Some("camera"), Some("maker")) => {
                        if !cam_maker_seen || lang.is_none() {
                            cam.maker = text;
                            cam_maker_seen = true;
                        }
                    }
                    (Some("camera"), Some("model")) => cam_lang_names.push((lang.is_none(), text)),
                    (Some("camera"), Some("mount")) => cam.mount = text,
                    (Some("camera"), Some("cropfactor")) => cam.crop = text.parse().unwrap_or(0.0),
                    (Some("lens"), Some("maker")) => {
                        if !lens_maker_seen || lang.is_none() {
                            lens.maker = text;
                            lens_maker_seen = true;
                        }
                    }
                    (Some("lens"), Some("model")) => lens_lang_names.push((lang.is_none(), text)),
                    (Some("lens"), Some("mount")) => lens.mounts.push(text),
                    (Some("lens"), Some("cropfactor")) => lens.crop = text.parse().unwrap_or(0.0),
                    (Some("lens"), Some("type")) => {
                        let t = text.to_lowercase();
                        if t != "rectilinear" {
                            lens.kind = Some(t);
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Many entries carry no `<focal>` / `<aperture>`; the model name (`70-210mm f/3.5-4.5`) or the focal lengths of
/// the calibration say it just as well.
fn derive_ranges(l: &mut LensEntry) {
    if l.focal.is_none() {
        for n in &l.names {
            if let Some(r) = focal_in_name(n) {
                l.focal = Some(r);
                break;
            }
        }
    }
    if l.focal.is_none() {
        let f: Vec<f64> =
            l.distortion.iter().map(|d| d.focal).chain(l.tca.iter().map(|t| t.focal)).chain(l.vignetting.iter().map(|v| v.focal)).collect();
        if !f.is_empty() {
            l.focal = Some((f.iter().copied().fold(f64::MAX, f64::min), f.iter().copied().fold(0.0, f64::max)));
        }
    }
    if l.aperture.is_none() {
        for n in &l.names {
            if let Some(r) = aperture_in_name(n) {
                l.aperture = Some(r);
                break;
            }
        }
    }
}

fn numbers_after(s: &str, at: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s[at..].chars() {
        if c.is_ascii_digit() || (c == '.' && !cur.is_empty()) {
            cur.push(c);
        } else {
            if !cur.is_empty() {
                out.extend(cur.parse::<f64>());
                cur.clear();
            }
            if c != '-' && c != '–' {
                break;
            }
        }
    }
    if !cur.is_empty() {
        out.extend(cur.parse::<f64>());
    }
    out
}

/// `24-105mm`, `50mm`, `EF-S 17-55mm` → focal range.
fn focal_in_name(n: &str) -> Option<(f64, f64)> {
    let b = n.as_bytes();
    let mm = n.find("mm")?;
    // walk back over digits, dots and dashes
    let mut start = mm;
    while start > 0 && (b[start - 1].is_ascii_digit() || b[start - 1] == b'.' || b[start - 1] == b'-') {
        start -= 1;
    }
    let v = numbers_after(n, start);
    match v.as_slice() {
        [a] => Some((*a, *a)),
        [a, b, ..] => Some((a.min(*b), a.max(*b))),
        _ => None,
    }
}

/// `f/1.8`, `f/3.5-4.5`, `1:2.8` → widest and narrowest-at-the-other-end aperture.
fn aperture_in_name(n: &str) -> Option<(f64, f64)> {
    let at = n.find("f/").map(|i| i + 2).or_else(|| n.find("1:").map(|i| i + 2))?;
    let v = numbers_after(n, at);
    match v.as_slice() {
        [a] => Some((*a, *a)),
        [a, b, ..] => Some((*a, *b)),
        _ => None,
    }
}

/// The names with the plain (language-less) one first, without duplicates.
fn ordered(names: &[(bool, String)]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for plain in [true, false] {
        for (p, n) in names {
            if *p == plain && !v.contains(n) {
                v.push(n.clone());
            }
        }
    }
    v
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(out)) = (args.first(), args.get(1)) else {
        return Err("usage: build_db <db-dir> <out.json.z> [revision] [retrieved-date]".into());
    };
    let mut raw = Raw::default();
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{dir}: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .collect();
    files.sort();
    for f in &files {
        let xml = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        parse(&xml, &mut raw).map_err(|e| format!("{}: {e}", f.display()))?;
    }
    // merge mounts of the same name, keep lens / camera order stable
    let mut mounts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in raw.mounts {
        let e = mounts.entry(m.name).or_default();
        for c in m.compat {
            if !e.contains(&c) {
                e.push(c);
            }
        }
    }
    let db = Database {
        format: 1,
        provenance: Provenance {
            name: "lensfun lens database (data files only)".into(),
            licence: "Creative Commons Attribution-ShareAlike 3.0 (CC BY-SA 3.0)".into(),
            url: "https://lensfun.github.io/".into(),
            revision: args.get(2).cloned().unwrap_or_default(),
            retrieved: args.get(3).cloned().unwrap_or_default(),
        },
        mounts: mounts.into_iter().map(|(name, compat)| Mount { name, compat }).collect(),
        cameras: raw.cameras,
        lenses: raw.lenses,
    };
    let bytes = db.to_compressed().map_err(|e| e.to_string())?;
    std::fs::write(Path::new(out), &bytes).map_err(|e| format!("{out}: {e}"))?;
    let calibrated = db.lenses.iter().filter(|l| l.has_calibration()).count();
    println!(
        "{} files → {} mounts, {} cameras, {} lens entries ({} with calibration); {} bytes",
        files.len(),
        db.mounts.len(),
        db.cameras.len(),
        db.lenses.len(),
        calibrated,
        bytes.len()
    );
    Ok(())
}
