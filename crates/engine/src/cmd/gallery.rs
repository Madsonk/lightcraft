//! `gallery.export`: a static web gallery of the selected photos (see [`crate::gallery`]).

use std::collections::HashSet;
use std::path::Path;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, f64_or, str_param};
use crate::export::{ExportFormat, ExportOptions, MetadataPolicy, Resize, SharpenFor, export_formats};
use crate::gallery::{Entry, Variant, alt_text};
use crate::presets::slug;
use crate::{Result, Session};

const WIDTHS: [u32; 4] = [480, 960, 1600, 2400];

fn mime(f: ExportFormat) -> &'static str {
    match f {
        ExportFormat::Avif => "image/avif",
        ExportFormat::Webp => "image/webp",
        ExportFormat::Png => "image/png",
        _ => "image/jpeg",
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| crate::EngineError::Other(format!("{}: {e}", d.display())))?;
    }
    std::fs::write(path, bytes).map_err(|e| crate::EngineError::Other(format!("{}: {e}", path.display())))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.export";
    let dir = str_param(p, "dir").filter(|d| !d.trim().is_empty()).ok_or_else(|| bad(C, "missing `dir` (the folder to write the gallery in)"))?;
    s.check_write_target(&format!("{}/index.html", dir.trim_end_matches('/'))).map_err(|e| bad(C, e))?;
    let ids: Vec<PhotoId> =
        if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { s.visible_cloned() };
    if ids.is_empty() {
        return Err(bad(C, "no photos (select some, give `ids`, or filter the view)"));
    }
    let mut widths: Vec<u32> = match p.get("widths").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(|w| w.clamp(64, 8192) as u32).collect(),
        None => WIDTHS.to_vec(),
    };
    widths.sort_unstable();
    widths.dedup();
    if widths.is_empty() {
        return Err(bad(C, "`widths` is empty"));
    }
    let mut formats: Vec<ExportFormat> = match p.get("formats").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).filter_map(ExportFormat::parse).collect(),
        None => vec![ExportFormat::Avif, ExportFormat::Jpeg],
    };
    // a JPEG fallback is always there (every browser reads it) and comes last
    formats.retain(|f| f.is_rendered() && *f != ExportFormat::Png && *f != ExportFormat::Tiff && *f != ExportFormat::Jpeg);
    formats.dedup();
    formats.push(ExportFormat::Jpeg);
    let jpeg_q = f64_or(p, "quality", 82.0).clamp(30.0, 100.0) as u8;
    let avif_q = f64_or(p, "avifQuality", 55.0).clamp(10.0, 100.0) as u8;
    let title = str_param(p, "title").map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Gallery").to_string();
    let base_url =
        str_param(p, "baseUrl").map(|u| if u.is_empty() || u.ends_with('/') { u.to_string() } else { format!("{u}/") }).unwrap_or_default();
    let keep_gps = bool_or(p, "keepLocation", false);
    let root = Path::new(dir);
    let (mut entries, mut failed, mut used, mut total) = (Vec::new(), Vec::<Value>::new(), HashSet::new(), 0usize);
    let t0 = web_time::Instant::now();
    for id in &ids {
        let Some(ph) = s.catalog.photo(*id) else { continue };
        let stem = ph.file_name.rsplit_once('.').map_or(ph.file_name.as_str(), |(a, _)| a).to_string();
        let (alt, alt_source) = alt_text(&ph.meta.caption, &ph.meta.title, &ph.meta.keywords, &ph.file_name);
        let caption = if ph.meta.caption.trim().is_empty() { ph.meta.title.clone() } else { ph.meta.caption.clone() };
        let mut name = slug(&stem);
        if !used.insert(name.clone()) {
            name = format!("{name}-{}", id.0);
            used.insert(name.clone());
        }
        let mut entry = Entry { id: id.0, name: name.clone(), alt, alt_source, caption, width: 0, height: 0, variants: Vec::new() };
        let mut err = None;
        for w in &widths {
            let o = ExportOptions {
                resize: Some(Resize { value: *w as f32, ..Resize::long_edge(*w) }),
                sharpen: SharpenFor::Screen,
                metadata: MetadataPolicy::Copyright,
                remove_location: !keep_gps,
                ..Default::default()
            };
            let fm: Vec<(ExportFormat, u8)> = formats.iter().map(|f| (*f, if *f == ExportFormat::Avif { avif_q } else { jpeg_q })).collect();
            match export_formats(s, *id, &o, &fm) {
                Ok((files, fw, fh)) => {
                    // (a photo smaller than a rung is not enlarged: that rung repeats the previous size)
                    if entry.variants.iter().any(|v| v.width == fw && v.mime == mime(files[0].0)) {
                        continue;
                    }
                    entry.width = entry.width.max(fw);
                    entry.height = entry.height.max(fh);
                    for (f, bytes) in files {
                        let file = format!("img/{name}-{fw}.{}", f.extension());
                        write(&root.join(&file), &bytes)?;
                        total += bytes.len();
                        entry.variants.push(Variant { file, mime: mime(f), width: fw, height: fh, bytes: bytes.len() });
                    }
                }
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        match err {
            Some(e) => failed.push(json!([id.0, e])),
            None => entries.push(entry),
        }
    }
    if entries.is_empty() {
        return Err(bad(C, format!("no photo could be exported: {failed:?}")));
    }
    write(&root.join("index.html"), crate::gallery::page(&title, &entries, &base_url).as_bytes())?;
    write(&root.join("snippet.html"), crate::gallery::snippet(&entries).as_bytes())?;
    let manifest = serde_json::to_vec_pretty(&crate::gallery::manifest(&title, &entries)).unwrap_or_default();
    write(&root.join("gallery.json"), &manifest)?;
    let files: usize = entries.iter().map(|e| e.variants.len()).sum();
    Ok(json!({
        "dir": dir, "photos": entries.len(), "files": files, "bytes": total, "seconds": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        "widths": widths, "formats": formats.iter().map(|f| f.extension()).collect::<Vec<_>>(),
        "index": root.join("index.html").display().to_string(), "failed": failed,
        "altFromFilename": entries.iter().filter(|e| e.alt_source == "filename").count(),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "gallery.export",
        "Export Web Gallery…",
        ["File", "Export Web Gallery…"],
        None,
        "{dir, ids?, title?, widths?: [480,960,1600,2400], formats?: [avif, webp, jpeg] (avif+jpeg; a JPEG fallback is always written), quality? (JPEG 82), avifQuality? (55), baseUrl? (for the JSON-LD), keepLocation?: bool (GPS is removed by default)} — a static web gallery of the selection (else the view): each photo rendered once per width and encoded in each format (never enlarged), `index.html` (responsive <picture>/srcset, lazy loading, lightbox, JSON-LD ImageGallery, no external request), `snippet.html` (the grid alone, for an Odoo page), `gallery.json`, `img/`. Alt text from caption, title, keywords, else the file name (reported). Writes nothing outside `dir` → {dir, photos, files, bytes, seconds, widths, formats, index, failed, altFromFilename}",
        super::always,
        export
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-gallery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_gallery_is_written_with_every_width_format_and_page() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|i| i.0).collect();
        let dir = temp("a");
        let r = s
            .execute(
                "gallery.export",
                &json!({"dir": dir.to_str().unwrap(), "ids": ids, "title": "Demo <1>", "widths": [200, 400], "formats": ["avif"], "avifQuality": 40}),
            )
            .unwrap();
        assert_eq!(r["photos"], 3, "{r}");
        // 3 photos x 2 widths x (avif + jpeg), unless a photo is smaller than a rung
        assert_eq!(r["files"], 12, "{r}");
        for f in ["index.html", "snippet.html", "gallery.json"] {
            assert!(dir.join(f).is_file(), "{f}");
        }
        let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
        assert!(html.contains("<title>Demo &lt;1&gt;</title>") && html.contains("image/avif"), "{html}");
        let m: Value = serde_json::from_slice(&std::fs::read(dir.join("gallery.json")).unwrap()).unwrap();
        let files = m["photos"][0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 4);
        for f in files {
            let path = dir.join(f["file"].as_str().unwrap());
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(bytes.len() as u64, f["bytes"].as_u64().unwrap());
            match f["type"].as_str().unwrap() {
                "image/jpeg" => assert_eq!(&bytes[..2], &[0xff, 0xd8]),
                _ => assert_eq!(&bytes[4..8], b"ftyp", "avif is an ISO-BMFF file"),
            }
            assert!(f["width"].as_u64().unwrap() <= 400);
        }
        // GPS never leaves by default
        assert!(!html.contains("GPS"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_is_required_and_unknown_formats_still_get_a_jpeg_fallback() {
        let mut s = Session::with_demo();
        assert!(s.execute("gallery.export", &json!({})).unwrap_err().to_string().contains("missing `dir`"));
        let dir = temp("b");
        let first = s.visible_cloned()[0].0;
        let r = s.execute("gallery.export", &json!({"dir": dir.to_str().unwrap(), "ids": [first], "widths": [150], "formats": ["nope"]})).unwrap();
        assert_eq!(r["formats"], json!(["jpg"]));
        assert_eq!(r["files"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
