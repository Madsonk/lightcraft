//! The Create space: what LightCraft already makes from a set of photos (the static web gallery,
//! the slideshow) in one panel beside the photos that will be used, and the "kit" — a folder of
//! rendered images plus a `manifest.json` (order, captions, keywords) — for the apps that lay pages
//! out (DesignCraft, PdfCraft, DeckCraft, FilmCraft). LightCraft only prepares the kit: nothing here
//! opens or drives another app.
//!
//! The photos used are the selection when several are selected, else everything in the current view
//! (the same rule as `gallery.export`), in grid order.

use std::path::Path;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Width of the Create panel.
pub const PANEL_W: f32 = 360.0;

/// The widths a gallery can be made at (`gallery.export`'s defaults).
pub const WIDTHS: [u32; 4] = [480, 960, 1600, 2400];

/// The long edges offered for a kit's images (0 = the photo's full size).
pub const EDGES: [u32; 4] = [1600, 2400, 3600, 0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Gallery,
    Kit,
    Slideshow,
}

/// Which app a kit is for. The kit is the same folder for every target; the target is written in
/// the manifest with the hint that goes with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Layout,
    Pdf,
    Deck,
    Film,
}

impl Target {
    pub const ALL: [Target; 4] = [Target::Layout, Target::Pdf, Target::Deck, Target::Film];

    pub fn id(self) -> &'static str {
        match self {
            Target::Layout => "designcraft",
            Target::Pdf => "pdfcraft",
            Target::Deck => "deckcraft",
            Target::Film => "filmcraft",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Layout => "DesignCraft",
            Target::Pdf => "PdfCraft",
            Target::Deck => "DeckCraft",
            Target::Film => "FilmCraft",
        }
    }

    pub fn parse(s: &str) -> Option<Target> {
        Target::ALL.into_iter().find(|t| t.id().eq_ignore_ascii_case(s) || t.label().eq_ignore_ascii_case(s))
    }

    /// What the receiving app does with the kit, written in the manifest and in the panel. Only
    /// DesignCraft's `file.place` was read in the app's own command list; the rest is said as it is.
    pub fn hint(self) -> &'static str {
        match self {
            Target::Layout => {
                "Book, album, flyer: in DesignCraft, file.new then one file.place {path, frame?, spread?, x?, y?, width?} per image, in manifest order; file.exportPdf makes the PDF."
            }
            Target::Pdf => {
                "PdfCraft opens and edits PDFs: make the pages in DesignCraft (file.exportPdf), then open that PDF in PdfCraft. It has no image import."
            }
            Target::Deck => {
                "Slides: one image per slide in manifest order (DeckCraft's slide commands were not checked: its command list did not answer)."
            }
            Target::Film => "Photo video: the images in manifest order are the clips; durations are yours to set.",
        }
    }
}

/// A kit whose images are still being written by the background export.
#[derive(Clone, Debug)]
pub struct KitJob {
    dir: String,
    name: String,
    target: Target,
    title: String,
    source: String,
    long_edge: u32,
    ids: Vec<PhotoId>,
}

/// The Create panel's state (nothing here is saved).
pub struct State {
    pub tab: Tab,
    pub title: String,
    pub widths: [bool; 4],
    pub avif: bool,
    pub kit_name: String,
    pub target: Target,
    pub long_edge: u32,
    pub interval: f32,
    pub pending: Option<KitJob>,
    /// What the last kit ended with (`ui.inspect` → create).
    pub last_kit: Option<Value>,
}

impl Default for State {
    fn default() -> Self {
        State {
            tab: Tab::Gallery,
            title: String::new(),
            widths: [true; 4],
            avif: true,
            kit_name: String::new(),
            target: Target::Layout,
            long_edge: 2400,
            interval: 4.0,
            pending: None,
            last_kit: None,
        }
    }
}

/// The photos the gallery and the kit use: the selection when several are selected, else the
/// whole view; always in grid order.
pub fn photos(app: &mut LightcraftApp) -> Vec<PhotoId> {
    let visible = app.session.visible_cloned();
    if app.session.selection.ids.len() > 1 {
        let sel: std::collections::HashSet<PhotoId> = app.session.selection.ids.iter().copied().collect();
        visible.into_iter().filter(|id| sel.contains(id)).collect()
    } else {
        visible
    }
}

/// How many of `ids` have a caption, a title, keywords, or only their file name to say what they show
/// (the gallery's alt text comes from the first of these).
pub fn readiness(app: &LightcraftApp, ids: &[PhotoId]) -> [usize; 4] {
    let mut n = [0usize; 4];
    for id in ids {
        let Some(p) = app.session.catalog.photo(*id) else { continue };
        let (_, source) = lightcraft_engine::gallery::alt_text(&p.meta.caption, &p.meta.title, &p.meta.keywords, &p.file_name);
        n[match source {
            "caption" => 0,
            "title" => 1,
            "keywords" => 2,
            _ => 3,
        }] += 1;
    }
    n
}

fn sanitize(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "kit".into() } else { s }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for now (days since 1970 turned into a civil date, so no date crate is needed).
fn utc_now() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `dir/name`, or `dir/name-2`… when that exists: a kit never lands in a folder that has things in it.
fn fresh_dir(dir: &str, name: &str) -> String {
    let base = Path::new(dir);
    let mut n = 1;
    loop {
        let candidate = if n == 1 { name.to_string() } else { format!("{name}-{n}") };
        let p = base.join(&candidate);
        if !p.exists() {
            return p.to_string_lossy().into_owned();
        }
        n += 1;
    }
}

/// `create.kit {dir?, name?, target?, longEdge?, ids?}`: render the photos into `<dir>/<name>/images/` (in the
/// background when the host can) and write `manifest.json` beside them once the images exist. Without `dir` the host asks
/// for a folder.
pub fn export_kit(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    if app.workspaces.create.pending.is_some() {
        return Err(crate::i18n::tr("A kit is already being made").to_string());
    }
    let ids: Vec<PhotoId> = match p.get("ids").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(PhotoId).collect(),
        None => photos(app),
    };
    if ids.is_empty() {
        return Err("no photos (select some, give `ids`, or filter the view)".into());
    }
    let st = &app.workspaces.create;
    let source = crate::i18n::source_title(&app.session);
    let title = p
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| if st.title.trim().is_empty() { source.clone() } else { st.title.trim().to_string() });
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| if st.kit_name.trim().is_empty() { title.clone() } else { st.kit_name.trim().to_string() });
    let target = match p.get("target").and_then(Value::as_str) {
        Some(t) => Target::parse(t).ok_or_else(|| format!("create.kit: unknown target `{t}` (designcraft|pdfcraft|deckcraft|filmcraft)"))?,
        None => st.target,
    };
    let long_edge = p.get("longEdge").and_then(Value::as_u64).map(|v| v.min(20_000) as u32).unwrap_or(st.long_edge);
    let parent = match p.get("dir").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => d.to_string(),
        None => match app.services.pick_folder.as_mut().and_then(|f| f()) {
            Some(d) => d,
            None => return Ok(Value::Null),
        },
    };
    let kit_dir = fresh_dir(&parent, &sanitize(&name));
    app.session.check_write_target(&format!("{kit_dir}/manifest.json"))?;
    let images = format!("{kit_dir}/images");
    std::fs::create_dir_all(&images).map_err(|e| format!("{images}: {e}"))?;
    // what `app.export` remembers for "Export with Previous" is the user's: a kit leaves it alone
    let previous = app.session.last_export.clone();
    let mut params = json!({
        "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(),
        "dir": images,
        "naming": "{seq:3}_{name}",
        "format": "jpeg",
        "quality": 92,
        "background": true,
    });
    if long_edge > 0 {
        params["longEdge"] = json!(long_edge);
        params["dontEnlarge"] = json!(true);
    }
    let out = app.run("app.export", params);
    app.session.last_export = previous;
    let out = out?;
    let job = KitJob { dir: kit_dir.clone(), name, target, title, source, long_edge, ids };
    let n = job.ids.len();
    match out.get("files").and_then(Value::as_array) {
        // the host has no background writer (the web build): the images are already there
        Some(files) => {
            let files = files.clone();
            finish(app, job, &files)?;
        }
        None => app.workspaces.create.pending = Some(job),
    }
    Ok(
        json!({"dir": kit_dir, "images": format!("{kit_dir}/images"), "manifest": format!("{kit_dir}/manifest.json"), "photos": n, "background": app.workspaces.create.pending.is_some()}),
    )
}

/// Once a frame: a kit whose export has ended gets its manifest.
pub fn tick(app: &mut LightcraftApp) {
    if app.workspaces.create.pending.is_none() || app.export.is_some() {
        return;
    }
    let Some(job) = app.workspaces.create.pending.take() else { return };
    let files: Vec<Value> = app.last_export_result.as_ref().and_then(|r| r["files"].as_array()).cloned().unwrap_or_default();
    if let Err(e) = finish(app, job, &files) {
        app.ui.status = e.clone();
        app.workspaces.create.last_kit = Some(json!({"error": e}));
    }
}

/// Write `manifest.json` and `README.txt` for the images `files` (one entry per photo, in order).
fn finish(app: &mut LightcraftApp, job: KitJob, files: &[Value]) -> Result<(), String> {
    let mut images = Vec::new();
    let mut failed = Vec::new();
    for (i, id) in job.ids.iter().enumerate() {
        let Some(p) = app.session.catalog.photo(*id) else { continue };
        let entry = files.get(i);
        let Some(path) = entry.and_then(|f| f["path"].as_str()) else {
            failed.push(json!({"source": p.file_name, "error": entry.and_then(|f| f["error"].as_str()).unwrap_or("not written")}));
            continue;
        };
        let file = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (alt, alt_source) = lightcraft_engine::gallery::alt_text(&p.meta.caption, &p.meta.title, &p.meta.keywords, &p.file_name);
        images.push(json!({
            "order": images.len() + 1,
            "file": format!("images/{file}"),
            "source": p.file_name,
            "title": p.meta.title,
            "caption": p.meta.caption,
            "altText": alt,
            "altSource": alt_source,
            "keywords": p.meta.keywords,
            "rating": p.rating,
            "captured": p.captured,
            "width": entry.and_then(|f| f["width"].as_u64()),
            "height": entry.and_then(|f| f["height"].as_u64()),
        }));
    }
    let manifest = json!({
        "kit": "lightcraft-kit",
        "version": 1,
        "createdBy": format!("LightCraft {}", env!("CARGO_PKG_VERSION")),
        "created": utc_now(),
        "title": job.title,
        "name": job.name,
        "source": job.source,
        "target": job.target.id(),
        "hint": job.target.hint(),
        "root": job.dir,
        "longEdge": job.long_edge,
        "format": "jpeg",
        "count": images.len(),
        "failed": failed,
        "images": images,
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(format!("{}/manifest.json", job.dir), text).map_err(|e| format!("{}/manifest.json: {e}", job.dir))?;
    let readme = format!(
        "LightCraft kit: {title}\n\n{n} image(s) in images/, in the order of manifest.json (order, file, caption, altText, keywords, rating).\nTarget: {target}.\n{hint}\n\nThe images are JPEG, long edge {edge}, with the edits of the photos at the time of the export. The originals were not touched.\n",
        title = job.title,
        n = manifest["count"],
        target = job.target.label(),
        hint = job.target.hint(),
        edge = if job.long_edge == 0 { "full size".to_string() } else { format!("{} px", job.long_edge) },
    );
    let _ = std::fs::write(format!("{}/README.txt", job.dir), readme);
    let ok = images.len();
    app.workspaces.create.last_kit = Some(json!({"dir": job.dir, "count": ok, "failed": failed.len()}));
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, crate::i18n::tr_format!("Kit ready: {n} image(s) and manifest.json in {dir}", n = ok, dir = job.dir));
    Ok(())
}

fn heading(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(8.0);
    ui.label(egui::RichText::new(crate::i18n::tr(text)).font(t.semibold(13.0)).color(t.text));
    ui.add_space(2.0);
}

fn dim(ui: &mut egui::Ui, text: String) {
    let t = Tokens::get(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(t.text_dim)).wrap());
}

/// The Create panel (right of the centre) and, in the rest of the centre, the photos that will be used.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let ids = photos(app);
    let selected = app.session.selection.ids.len();
    let ready = readiness(app, &ids);
    let mut st = std::mem::take(&mut app.workspaces.create);
    if st.title.is_empty() {
        st.title = crate::i18n::source_title(&app.session);
    }
    let mut run: Option<(&'static str, Value)> = None;
    egui::Panel::right("create_panel")
        .exact_size(PANEL_W)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::same(14)))
        .show(ui, |ui| {
            register(ui.ctx(), "panel:create", ui.max_rect());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, id, label) in [(Tab::Gallery, "createTabGallery", "Web gallery"), (Tab::Kit, "createTabKit", "Kit"), (Tab::Slideshow, "createTabSlideshow", "Slideshow")] {
                    if text_button(ui, id, label, st.tab == tab).clicked() {
                        st.tab = tab;
                    }
                }
            });
            let n = ids.len();
            let which = if selected > 1 {
                format!("{n} photos: the {selected} selected, in grid order", n = n, selected = selected)
            } else {
                format!("{n} photos: everything in this view (select several to use only those)", n = n)
            };
            ui.add_space(6.0);
            dim(ui, which);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match st.tab {
                Tab::Gallery => {
                    heading(ui, "Web gallery");
                    dim(ui, crate::i18n::tr("A static page with AVIF and JPEG at several widths, ready to upload or paste into a site.").to_string());
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(crate::i18n::tr("Title")).color(t.text_label).size(12.0));
                    let r = ui.add(egui::TextEdit::singleline(&mut st.title).desired_width(f32::INFINITY));
                    register(ui.ctx(), "field:createTitle", r.rect);
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(crate::i18n::tr("Widths")).color(t.text_label).size(12.0));
                    ui.horizontal(|ui| {
                        for (i, w) in WIDTHS.iter().enumerate() {
                            let r = ui.checkbox(&mut st.widths[i], w.to_string());
                            register(ui.ctx(), format!("check:createWidth-{w}"), r.rect);
                        }
                    });
                    let r = ui.checkbox(&mut st.avif, crate::i18n::tr("AVIF as well as JPEG (smaller files)"));
                    register(ui.ctx(), "check:createAvif", r.rect);
                    heading(ui, "Captions (alternative text)");
                    let [caption, title, keywords, file] = ready;
                    dim(
                        ui,
                        format!(
                            "{a} from a caption or title, {b} from keywords, {c} will say only the file name",
                            a = caption + title,
                            b = keywords,
                            c = file
                        ),
                    );
                    ui.add_space(10.0);
                    let widths: Vec<u32> = WIDTHS.iter().zip(st.widths).filter(|(_, on)| *on).map(|(w, _)| *w).collect();
                    let label = format!("Create the web gallery ({n} photos)", n = n);
                    let ok = n > 0 && !widths.is_empty() && app.services.pick_folder.is_some();
                    ui.add_enabled_ui(ok, |ui| {
                        if text_button(ui, "createGallery", &label, false).clicked() {
                            let mut p = json!({"background": true, "title": st.title.trim(), "widths": widths, "formats": if st.avif { json!(["avif"]) } else { json!([]) }});
                            if selected > 1 {
                                p["ids"] = json!(ids.iter().map(|i| i.0).collect::<Vec<_>>());
                            }
                            run = Some(("gallery.export", p));
                        }
                    });
                    dim(ui, crate::i18n::tr("It asks for a folder, then works in the background; location data is left out.").to_string());
                }
                Tab::Kit => {
                    heading(ui, "Kit for the layout apps");
                    dim(ui, crate::i18n::tr("A folder with the images, in order, and manifest.json (captions, keywords, rating). LightCraft opens nothing.").to_string());
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(crate::i18n::tr("For")).color(t.text_label).size(12.0));
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, tg) in Target::ALL.iter().enumerate() {
                            if text_button(ui, &format!("createTarget-{i}"), tg.label(), st.target == *tg).clicked() {
                                st.target = *tg;
                            }
                        }
                    });
                    dim(ui, st.target.hint().to_string());
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(crate::i18n::tr("Folder name")).color(t.text_label).size(12.0));
                    let r = ui.add(egui::TextEdit::singleline(&mut st.kit_name).hint_text(crate::i18n::tr("kit name (the title if empty)")).desired_width(f32::INFINITY));
                    register(ui.ctx(), "field:createKitName", r.rect);
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(crate::i18n::tr("Image size (long edge)")).color(t.text_label).size(12.0));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, e) in EDGES.iter().enumerate() {
                            let label = if *e == 0 { crate::i18n::tr("Full size").to_string() } else { format!("{e}") };
                            if text_button(ui, &format!("createEdge-{i}"), &label, st.long_edge == *e).clicked() {
                                st.long_edge = *e;
                            }
                        }
                    });
                    ui.add_space(10.0);
                    let label = format!("Export the kit ({n} photos)", n = n);
                    ui.add_enabled_ui(n > 0 && st.pending.is_none() && app.services.pick_folder.is_some(), |ui| {
                        if text_button(ui, "createKit", &label, false).clicked() {
                            run = Some(("create.kit", json!({"title": st.title.trim(), "name": st.kit_name.trim(), "target": st.target.id(), "longEdge": st.long_edge})));
                        }
                    });
                    if let Some(k) = &st.last_kit {
                        ui.add_space(6.0);
                        dim(ui, k["dir"].as_str().map(|d| format!("Last kit: {d}", d = d)).unwrap_or_else(|| k["error"].as_str().unwrap_or("").to_string()));
                    }
                }
                Tab::Slideshow => {
                    heading(ui, "Slideshow");
                    dim(ui, crate::i18n::tr("Full screen, one photo after another. A video of the photos is made in FilmCraft: export a kit for it.").to_string());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(crate::i18n::tr("Seconds per photo")).color(t.text_label).size(12.0));
                        let r = ui.add(egui::DragValue::new(&mut st.interval).range(0.5..=120.0).speed(0.1));
                        register(ui.ctx(), "field:createInterval", r.rect);
                    });
                    ui.add_space(10.0);
                    let label = format!("Start the slideshow ({n} photos)", n = n);
                    ui.add_enabled_ui(n > 0, |ui| {
                        if text_button(ui, "createSlideshow", &label, false).clicked() {
                            run = Some(("view.slideshow", json!({"interval": st.interval})));
                        }
                    });
                }
            });
        });
    app.workspaces.create = st;
    // the photos that will be used, as the usual grid
    crate::panels::grid::show(app, ui);
    if let Some((id, params)) = run {
        let _ = app.run(id, params);
    }
}
