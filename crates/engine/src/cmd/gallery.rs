//! `gallery.export`: a static web gallery of the selected photos (see [`crate::gallery`]).

use std::collections::HashSet;
use std::path::Path;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, f64_or, str_param};
use crate::export::{ExportFormat, ExportOptions, MetadataPolicy, PreparedFormats, Resize, SharpenFor, prepare_formats};
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

/// A web gallery set up from the session: [`Plan::run`] renders and writes it without the session,
/// so the app can run it on a worker thread (with progress and a Cancel) while the command
/// itself ([`export`]) just runs it at once.
pub struct Plan {
    dir: String,
    title: String,
    base_url: String,
    widths: Vec<u32>,
    formats: Vec<ExportFormat>,
    photos: Vec<PlanPhoto>,
}

struct PlanPhoto {
    id: PhotoId,
    name: String,
    stem: String,
    alt: String,
    alt_source: &'static str,
    caption: String,
    /// One render per width, in order; the first that could not be set up ends the list.
    rungs: Vec<std::result::Result<PreparedFormats, String>>,
}

impl Plan {
    /// How many photos the gallery has.
    pub fn photos(&self) -> usize {
        self.photos.len()
    }

    /// The folder it is written in.
    pub fn dir(&self) -> &str {
        &self.dir
    }
}

/// Read the parameters and the photos; nothing is rendered or written yet.
pub fn plan(s: &mut Session, p: &Value) -> Result<Plan> {
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
    let fm: Vec<(ExportFormat, u8)> = formats.iter().map(|f| (*f, if *f == ExportFormat::Avif { avif_q } else { jpeg_q })).collect();
    let (mut photos, mut used) = (Vec::new(), HashSet::new());
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
        let mut rungs = Vec::new();
        for w in &widths {
            let o = ExportOptions {
                resize: Some(Resize { value: *w as f32, ..Resize::long_edge(*w) }),
                sharpen: SharpenFor::Screen,
                metadata: MetadataPolicy::Copyright,
                remove_location: !keep_gps,
                ..Default::default()
            };
            let r = prepare_formats(s, *id, &o, &fm);
            let failed = r.is_err();
            rungs.push(r);
            if failed {
                break;
            }
        }
        photos.push(PlanPhoto { id: *id, name, stem, alt, alt_source, caption, rungs });
    }
    Ok(Plan { dir: dir.to_string(), title, base_url, widths, formats, photos })
}

impl Plan {
    /// Render and write the gallery. `progress(done, current file)` is called before each photo
    /// and once at the end; when it returns `false` the run stops after the photo in hand and the
    /// pages are not written (`{cancelled: true}`; the images already written stay in `img/`).
    pub fn run(self, progress: &mut dyn FnMut(usize, &str) -> bool) -> Result<Value> {
        const C: &str = "gallery.export";
        let Plan { dir, title, base_url, widths, formats, photos } = self;
        let root = Path::new(&dir);
        let total_photos = photos.len();
        let (mut entries, mut failed, mut total) = (Vec::new(), Vec::<Value>::new(), 0usize);
        let mut cancelled = false;
        let t0 = web_time::Instant::now();
        for (i, ph) in photos.into_iter().enumerate() {
            if !progress(i, &ph.stem) {
                cancelled = true;
                break;
            }
            let name = ph.name;
            let mut entry = Entry {
                id: ph.id.0,
                name: name.clone(),
                alt: ph.alt,
                alt_source: ph.alt_source,
                caption: ph.caption,
                width: 0,
                height: 0,
                variants: Vec::new(),
            };
            let mut err = None;
            for rung in ph.rungs {
                match rung.and_then(PreparedFormats::run) {
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
                Some(e) => failed.push(json!([ph.id.0, e])),
                None => entries.push(entry),
            }
        }
        progress(total_photos, "");
        if cancelled {
            return Ok(json!({"cancelled": true, "dir": dir, "photos": entries.len(), "total": total_photos, "failed": failed}));
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
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    plan(s, p)?.run(&mut |_, _| true)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "gallery.export",
        "Export Web Gallery…",
        ["File"],
        Some("Cmd+Alt+G"),
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

    #[test]
    fn a_planned_gallery_gives_the_same_files_and_reports_progress() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|i| i.0).collect();
        let (a, b) = (temp("plan-a"), temp("plan-b"));
        let params = |d: &std::path::Path| json!({"dir": d.to_str().unwrap(), "ids": ids, "widths": [200], "formats": []});
        let direct = s.execute("gallery.export", &params(&a)).unwrap();
        let plan = plan(&mut s, &params(&b)).unwrap();
        assert_eq!((plan.photos(), plan.dir()), (3, b.to_str().unwrap()));
        let mut seen = Vec::new();
        let via_plan = plan
            .run(&mut |done, name| {
                seen.push((done, name.to_string()));
                true
            })
            .unwrap();
        // before each photo, then once more at the end
        assert_eq!(seen.iter().map(|(d, _)| *d).collect::<Vec<_>>(), vec![0, 1, 2, 3], "{seen:?}");
        assert!(seen[..3].iter().all(|(_, n)| !n.is_empty()) && seen[3].1.is_empty());
        for k in ["photos", "files", "bytes"] {
            assert_eq!(direct[k], via_plan[k], "{k}");
        }
        for f in ["index.html", "snippet.html", "gallery.json"] {
            assert_eq!(!std::fs::read(a.join(f)).unwrap().is_empty(), b.join(f).is_file(), "{f}");
        }
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    #[test]
    fn cancelling_a_planned_gallery_stops_after_the_photo_in_hand_and_writes_no_page() {
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(4).map(|i| i.0).collect();
        let dir = temp("cancel");
        let plan = plan(&mut s, &json!({"dir": dir.to_str().unwrap(), "ids": ids, "widths": [150], "formats": []})).unwrap();
        // a Cancel pressed while the second photo is being made
        let r = plan.run(&mut |done, _| done < 2).unwrap();
        assert_eq!(r["cancelled"], true, "{r}");
        assert_eq!((r["photos"].as_u64(), r["total"].as_u64()), (Some(2), Some(4)), "{r}");
        assert!(!dir.join("index.html").exists() && dir.join("img").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
