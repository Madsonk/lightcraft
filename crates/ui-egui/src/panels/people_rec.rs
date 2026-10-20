//! People ▸ Recognition: the faces LightCraft found and grouped by similarity. Name a group and
//! its photos get the keyword `People|<name>`; answer "Same person?" for the doubtful cases; a new
//! photo that looks like someone is only *suggested*, never tagged without a yes.
//!
//! The models are optional downloads (InsightFace weights, non-commercial research terms): the
//! set-up card at the top shows the terms and downloads only after the user agrees.

use std::collections::HashMap;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::people::{GroupView, Overview, Thumb};
use serde_json::{Value, json};

use super::dnd::{self, FaceDrag, Over};
use crate::LightcraftApp;
use crate::render::Slot;
use crate::theme::Tokens;
use crate::widgets::register;

const PAD: f32 = 20.0;
const THUMB: f32 = 72.0;

/// What the view shows, rebuilt when the engine says something changed.
#[derive(Default)]
pub struct RecCache {
    pub dirty: bool,
    /// The Recognition tab is showing (else the names read from XMP).
    pub recognition: bool,
    /// The People view has been drawn since it was last left (the tab is chosen on the first frame).
    pub visit: bool,
    ov: Option<Overview>,
    questions: Vec<Value>,
    status: Value,
    status_at: f64,
    /// Text being typed in each group's name field.
    names: HashMap<String, String>,
    agreed: bool,
    /// Faces picked with a click (a drag then carries all of them).
    pub selected: Vec<String>,
    /// The person whose faces are listed (`p:N`).
    pub open_person: Option<String>,
    members: Option<(String, Value)>,
    /// `people.faces` of the photos asked about since the last change (hover, selected photo).
    photo_faces: HashMap<u64, Value>,
    /// How many groups / suggestions / faces of the open person or group are listed (0: the first page); each
    /// "Load more" adds a page, so every group and every face can be reached.
    pub groups_shown: usize,
    pub suggestions_shown: usize,
    pub members_shown: usize,
    /// Questions waiting in all (the queue shows the first few and refills as they are answered).
    pub questions_total: usize,
    /// Questions answered this session (hidden while the engine still lists them).
    answered: std::collections::HashSet<String>,
    /// Which list the page shows.
    pub section: Section,
    /// Face tiles drawn this frame (the renderer keeps that many thumbnails).
    drawn: usize,
}

/// The lists of the Recognition page, one at a time or all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    All,
    ToName,
    Suggestions,
    Unknown,
    People,
}

/// Faces added by one "Load more".
const PAGE: usize = 60;
const FIRST_GROUPS: usize = 40;
const FIRST_SUGGESTIONS: usize = 24;
const FIRST_MEMBERS: usize = 48;
/// Questions shown at once.
const QUESTIONS_SHOWN: usize = 6;

impl RecCache {
    pub fn ov_is_none(&self) -> bool {
        self.ov.is_none()
    }

    /// How many photos' faces were looked up for tooltips and the selected photo since the last change (tests).
    pub fn cached_photos(&self) -> usize {
        self.photo_faces.len()
    }

    /// The overview the view was last drawn from (tests).
    pub fn overview(&self) -> Option<&Overview> {
        self.ov.as_ref()
    }
}

fn rect_of(t: &Thumb) -> lightcraft_geom::Rect {
    lightcraft_geom::Rect { x0: f64::from(t.rect[0]), y0: f64::from(t.rect[1]), x1: f64::from(t.rect[2]), y1: f64::from(t.rect[3]) }
}

/// Paint the close-up of a face in `r` (a square around the face). Never an empty dark square without a
/// reason: the crop of the photo's own preview when it renders; else the face cut out of the grid thumbnail
/// (blurrier, but the face); else a spinner while it loads, or a "!" with the reason when it cannot be read
/// (the original is offline and no preview was kept). Returns the failure, if any, for the tooltip.
pub(crate) fn paint_face(app: &mut LightcraftApp, ui: &egui::Ui, r: Rect, t: &Thumb) -> Option<String> {
    let tk = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 3.0, tk.canvas);
    // only what is on screen is rendered: a page of hundreds of faces would otherwise queue hundreds of
    // decodes of the originals (slow on a network drive) and none would ever show
    if !ui.is_rect_visible(r) {
        return None;
    }
    app.caches.people_rec.drawn += 1;
    let size = r.width();
    let px = (size * ui.ctx().pixels_per_point()).ceil() as usize;
    let id = PhotoId(t.photo);
    // 1. the sharp close-up, rendered from the photo's preview, when it is ready
    let mut failure = None;
    if let Some(job) = app.session.face_job(id, rect_of(t), px) {
        let key = job.key;
        if let Some(tex) = app.renderer.variant(job) {
            ui.painter().image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            return None;
        }
        failure = app.renderer.failure(Slot::Variant(key)).map(str::to_string);
    }
    // 2. meanwhile (or when that cannot be rendered) the face cut out of the grid thumbnail, which comes
    //    from the thumbnail cache and is quick: blurrier, but it is the face
    super::grid::request_thumb(app, id, 256, 8);
    let orient = app.session.catalog.photo(id).map(|p| p.develop.orientation);
    if let Some(tex) = app.renderer.thumb(id) {
        let [tw, th] = tex.size;
        let f = orient.map_or(rect_of(t), |o| o.map_norm_rect(rect_of(t)));
        let (w, h) = (tw as f64, th as f64);
        let side = (((f.x1 - f.x0) * w).max((f.y1 - f.y0) * h) * 1.7).clamp(1.0, w.min(h));
        let (cx, cy) = ((f.x0 + f.x1) / 2.0 * w, (f.y0 + f.y1) / 2.0 * h);
        let (x0, y0) = (cx.clamp(side / 2.0, w - side / 2.0) - side / 2.0, cy.clamp(side / 2.0, h - side / 2.0) - side / 2.0);
        let uv = Rect::from_min_max(pos2((x0 / w) as f32, (y0 / h) as f32), pos2(((x0 + side) / w) as f32, ((y0 + side) / h) as f32));
        ui.painter().image(tex.tex.id(), r, uv, Color32::WHITE);
        return None;
    }
    // 3. nothing yet: a loading mark, or a "!" when neither could be read
    let thumb_failed = app.renderer.failure(Slot::Thumb(id)).map(str::to_string);
    if failure.is_some() && thumb_failed.is_some() {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "!", tk.semibold(16.0), tk.text_dim);
        return failure.or(thumb_failed);
    }
    let a = ui.ctx().input(|i| i.time) as f32 * 4.0;
    let c = r.center();
    let k = (size * 0.12).max(3.0);
    ui.painter().circle_filled(c + vec2(a.cos(), a.sin()) * k, 2.0, tk.text_dim);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    None
}

/// A face close-up (see [`paint_face`]).
fn face(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Thumb, size: f32) {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if let Some(why) = paint_face(app, ui, r, t) {
        resp.on_hover_text(format!("This face cannot be shown: {why}"));
    }
}

fn faces_row(app: &mut LightcraftApp, ui: &mut egui::Ui, thumbs: &[Thumb], size: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for t in thumbs {
            face(app, ui, t, size);
        }
    });
}

fn thumbs_of_json(v: &Value) -> Vec<Thumb> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    let rect = t["rect"].as_array()?;
                    let r: Vec<f32> = rect.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
                    Some(Thumb {
                        photo: t["photo"].as_u64()?,
                        face: t["face"].as_str()?.to_string(),
                        rect: [*r.first()?, *r.get(1)?, *r.get(2)?, *r.get(3)?],
                        video: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn refresh(app: &mut LightcraftApp, now: f64) {
    let c = &mut app.caches.people_rec;
    let stale_status = now - c.status_at > 0.7;
    if !c.dirty && c.ov.is_some() && !stale_status {
        return;
    }
    if stale_status {
        let st = app.session.execute("people.status", &json!({})).unwrap_or(Value::Null);
        app.caches.people_rec.status = st;
        app.caches.people_rec.status_at = now;
    }
    if app.caches.people_rec.dirty || app.caches.people_rec.ov.is_none() {
        app.caches.people_rec.dirty = false;
        app.caches.people_rec.photo_faces.clear();
        app.caches.people_rec.members = None;
        let (g, sg) = {
            let c = &app.caches.people_rec;
            (c.groups_shown.max(FIRST_GROUPS), c.suggestions_shown.max(FIRST_SUGGESTIONS))
        };
        let ov = app.session.people_overview(g, sg, 6).ok();
        let qv = app.session.execute("people.questions", &json!({"limit": QUESTIONS_SHOWN})).ok();
        let mut q = qv.as_ref().and_then(|v| v["questions"].as_array().cloned()).unwrap_or_default();
        let before = q.len();
        q.retain(|x| x["id"].as_str().is_none_or(|id| !app.caches.people_rec.answered.contains(id)));
        let total = qv.as_ref().and_then(|v| v["total"].as_u64()).map_or(before, |t| t as usize).saturating_sub(before - q.len());
        let c = &mut app.caches.people_rec;
        c.ov = ov;
        c.questions = q;
        c.questions_total = total;
    }
}

/// Run a People command and refresh the view; failures go to the status line.
fn act(app: &mut LightcraftApp, id: &str, params: Value) -> Option<Value> {
    let r = app.run(id, params);
    app.caches.people_rec.dirty = true;
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let now = ui.ctx().input(|i| i.time);
    refresh(app, now);
    let t = Tokens::get(ui.ctx());
    let st = app.caches.people_rec.status.clone();
    if st["available"].as_bool() != Some(true) {
        super::empty_message(ui, ui.available_rect_before_wrap(), "Face recognition is not available in this build", "");
        return;
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
    drop_bar(app, ui);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.vertical(|ui| {
                setup(app, ui, &st);
                controls(app, ui, &st);
                this_photo(app, ui);
                section_bar(app, ui);
                let sec = app.caches.people_rec.section;
                let all = sec == Section::All;
                if all || sec == Section::ToName {
                    questions(app, ui);
                }
                if all || sec == Section::People {
                    persons(app, ui);
                }
                person_faces(app, ui);
                if all || sec == Section::ToName {
                    groups(app, ui);
                }
                if all || sec == Section::Unknown {
                    unknown_people(app, ui, &st);
                }
                if all || sec == Section::Suggestions {
                    suggestions(app, ui);
                }
            });
        });
        ui.add_space(30.0);
    });
    // the renderer keeps as many face thumbnails as were drawn (else a long list evicts and re-renders the same few)
    let drawn = std::mem::take(&mut app.caches.people_rec.drawn);
    app.renderer.want_variants(drawn);
    let _ = t;
}

/// Which list to look at, with how many each holds ("showing 40 of 1220" is under each list).
fn section_bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(ov) = app.caches.people_rec.ov.clone() else { return };
    let t = Tokens::get(ui.ctx());
    let items = [
        (Section::All, "All".to_string()),
        (Section::ToName, format!("To name {}", ov.groups_total)),
        (Section::Suggestions, format!("Suggestions {}", ov.suggestions_total)),
        (Section::Unknown, format!("Unknown {}", ov.unknown_total)),
        (Section::People, format!("People {}", ov.persons.len())),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("{} faces in {} photos.", ov.indexed_faces, ov.indexed_photos)).font(t.font(12.0)).color(t.text_dim));
        for (sec, label) in items {
            let on = app.caches.people_rec.section == sec;
            let r = crate::widgets::text_button(ui, &format!("recSection{sec:?}"), &label, on);
            if r.clicked() {
                app.caches.people_rec.section = sec;
            }
        }
    });
    ui.add_space(6.0);
}

/// "Showing N of TOTAL" and a button that lists one more page: every group and face can be reached.
fn more(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, shown: usize, total: usize, first: usize, field: fn(&mut RecCache) -> &mut usize) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("Showing {} of {total}", shown.min(total))).font(t.font(12.0)).color(t.text_dim));
        if shown < total {
            let b = ui.button(format!("Load {} more", PAGE.min(total - shown)));
            register(ui.ctx(), format!("recMore:{id}"), b.rect);
            if b.clicked() {
                let c = &mut app.caches.people_rec;
                let cur = field(c);
                *cur = (*cur).max(first) + PAGE;
                c.dirty = true;
            }
        }
    });
}

fn setup(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &Value) {
    let m = &st["model"];
    let faces_ok = st["faceModel"]["installed"].as_bool() == Some(true);
    if m["installed"].as_bool() == Some(true) && faces_ok {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let dl = &m["download"];
    let fdl = &st["faceModel"]["download"];
    egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(12.0).show(ui, |ui| {
        ui.set_max_width(640.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Recognition needs two models")).font(t.semibold(14.0)).color(t.text));
        ui.label(
            egui::RichText::new(format!(
                "{} ({:.0} MB) by {}. Terms: {}. It is not part of LightCraft; it is downloaded to your Mac only if you agree, and everything else works without it. Photos never leave your computer.",
                m["name"].as_str().unwrap_or(""),
                m["sizeBytes"].as_f64().unwrap_or(0.0) / 1e6,
                "InsightFace",
                m["licence"].as_str().unwrap_or("")
            ))
            .font(t.font(12.0))
            .color(t.text_label),
        );
        ui.hyperlink_to("Read the terms", m["licenceUrl"].as_str().unwrap_or("").to_string());
        let busy = dl["running"].as_bool() == Some(true) || fdl["running"].as_bool() == Some(true);
        if busy {
            let (done, total) = (dl["done"].as_f64().unwrap_or(0.0), dl["total"].as_f64().unwrap_or(1.0).max(1.0));
            ui.add(egui::ProgressBar::new((done / total) as f32).text(format!("{:.0} / {:.0} MB", done / 1e6, total / 1e6)));
            if ui.button(crate::i18n::tr("Cancel")).clicked() {
                act(app, "people.model.cancel", json!({}));
            }
        } else {
            let agreed = &mut app.caches.people_rec.agreed;
            ui.checkbox(agreed, "I have read the terms and I agree to download the model");
            let go = *agreed;
            if ui.add_enabled(go, egui::Button::new(crate::i18n::tr("Download"))).clicked() {
                act(app, "people.model.download", json!({"acknowledged": true}));
            }
            if let Some(e) = dl["error"].as_str().or(fdl["error"].as_str()) {
                ui.colored_label(t.caution, e);
            }
        }
    });
    ui.add_space(10.0);
}

fn controls(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &Value) {
    let t = Tokens::get(ui.ctx());
    let installed = st["model"]["installed"].as_bool() == Some(true) && st["faceModel"]["installed"].as_bool() == Some(true);
    let job = &st["job"];
    let running = job["running"].as_bool() == Some(true);
    let idx = &st["index"];
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("People")).font(t.semibold(15.0)).color(t.text));
        if let Some(o) = &app.caches.people_rec.ov {
            ui.label(
                egui::RichText::new(format!("{} photos looked at · {} faces", o.indexed_photos, o.indexed_faces))
                    .font(t.font(12.0))
                    .color(t.text_dim),
            );
        }
    });
    ui.horizontal(|ui| {
        if running {
            let (done, total) = (job["done"].as_f64().unwrap_or(0.0), job["total"].as_f64().unwrap_or(1.0).max(1.0));
            ui.add(
                egui::ProgressBar::new((done / total) as f32)
                    .desired_width(320.0)
                    .text(format!("{done:.0} / {total:.0} photos · {} faces", job["faces"].as_u64().unwrap_or(0))),
            );
            if ui.button(crate::i18n::tr("Stop")).clicked() {
                act(app, "people.cancel", json!({}));
            }
        } else {
            if ui
                .add_enabled(installed, egui::Button::new(crate::i18n::tr("Find people in the library")))
                .on_hover_text(crate::i18n::tr("Looks at each photo's preview (never the RAW), once. Interrupted runs resume."))
                .clicked()
            {
                act(app, "people.index", json!({"background": true}));
            }
            if ui.add_enabled(installed, egui::Button::new(crate::i18n::tr("Group again"))).clicked() {
                act(app, "people.refresh", json!({"background": true}));
            }
            if ui
                .add_enabled(app.session.can_undo_people(), egui::Button::new(crate::i18n::tr("Undo")))
                .on_hover_text(crate::i18n::tr("Undo the last People decision"))
                .clicked()
            {
                act(app, "people.undo", json!({}));
            }
        }
        if st["computing"].as_bool() == Some(true) {
            ui.spinner();
            ui.label(egui::RichText::new(crate::i18n::tr("grouping faces…")).font(t.font(12.0)).color(t.text_dim));
        }
    });
    if let Some(e) = job["error"].as_str() {
        ui.colored_label(t.caution, e);
    }
    let s = &idx["settings"];
    if idx.is_object() {
        ui.horizontal(|ui| {
            let mut auto = s["autoTag"].is_number();
            let mut v = s["autoTag"].as_f64().unwrap_or(0.65) as f32;
            let before = (auto, v);
            ui.checkbox(&mut auto, "Tag automatically above");
            ui.add_enabled(auto, egui::Slider::new(&mut v, 0.5..=0.95).fixed_decimals(2));
            ui.label(egui::RichText::new(crate::i18n::tr("(off: LightCraft only suggests)")).font(t.font(11.0)).color(t.text_dim));
            if (auto, v) != before {
                act(app, "people.settings", json!({"autoTag": if auto { json!(v) } else { Value::Null }}));
            }
        });
    }
    ui.add_space(6.0);
}

fn questions(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let qs = app.caches.people_rec.questions.clone();
    if qs.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let total = app.caches.people_rec.questions_total.max(qs.len());
    ui.label(
        egui::RichText::new(format!("Same person? ({total} questions, {} shown: answer to see the next ones)", qs.len()))
            .font(t.semibold(14.0))
            .color(t.text),
    );
    for q in qs {
        let id = q["id"].as_str().unwrap_or("").to_string();
        egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (a, b): (Vec<Thumb>, Vec<Thumb>) = match q["kind"].as_str() {
                    Some("face") => (thumbs_of_json(&json!([q["face"]])), thumbs_of_json(&q["personFaces"])),
                    Some("group-person") => (thumbs_of_json(&q["group"]), thumbs_of_json(&q["personFaces"])),
                    _ => (thumbs_of_json(&q["a"]), thumbs_of_json(&q["b"])),
                };
                faces_row(app, ui, &a, 64.0);
                ui.label(egui::RichText::new("≟").font(t.semibold(20.0)).color(t.text_dim));
                faces_row(app, ui, &b, 64.0);
                ui.vertical(|ui| {
                    if let Some(n) = q["name"].as_str() {
                        ui.label(egui::RichText::new(format!("Is this {n}?")).font(t.font(13.0)).color(t.text));
                    }
                    ui.horizontal(|ui| {
                        for (label, ans) in [("Yes", "yes"), ("No", "no"), ("Not sure", "unsure")] {
                            if ui.button(label).clicked() {
                                app.caches.people_rec.answered.insert(id.clone());
                                act(app, "people.confirm", json!({"question": id, "answer": ans}));
                            }
                        }
                    });
                });
            });
        });
    }
    ui.add_space(8.0);
}

// ------------------------------------------------------------------------ faces: tiles, drag, drop

/// The faces of a photo (cached until the next change): `people.faces`.
fn photo_faces(app: &mut LightcraftApp, photo: u64) -> Vec<Value> {
    if let Some(v) = app.caches.people_rec.photo_faces.get(&photo) {
        return v["faces"].as_array().cloned().unwrap_or_default();
    }
    let v = app.session.execute("people.faces", &json!({"id": photo})).unwrap_or(Value::Null);
    let faces = v["faces"].as_array().cloned().unwrap_or_default();
    app.caches.people_rec.photo_faces.insert(photo, v);
    faces
}

/// Who a face is, in words: the person's name, "unknown", "not a face", or "to name".
fn who(f: &Value) -> String {
    if let Some(n) = f["name"].as_str() {
        n.to_string()
    } else if f["unknown"].as_bool() == Some(true) {
        "unknown".into()
    } else if f["ignored"].as_bool() == Some(true) {
        "not a face".into()
    } else if f["person"].is_string() {
        "unnamed".into()
    } else {
        "to name".into()
    }
}

fn thumb_of(photo: u64, f: &Value) -> Option<Thumb> {
    let r: Vec<f32> = f["rect"].as_array()?.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
    Some(Thumb { photo, face: f["face"].as_str()?.to_string(), rect: [*r.first()?, *r.get(1)?, *r.get(2)?, *r.get(3)?], video: None })
}

/// A face you can pick (click) and drag (alone, or with the other picked ones) onto a person, a group,
/// "Unknown person" or "Not a real face". Hovering shows every face of its photo and who each is:
/// a photo can hold several people, and it is one face that moves, not the photo.
fn tile(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Thumb, size: f32, label: Option<&str>) -> egui::Response {
    let tk = Tokens::get(ui.ctx());
    let lh = if label.is_some() { 14.0 } else { 0.0 };
    let (full, resp) = ui.allocate_exact_size(vec2(size.max(52.0), size + lh), Sense::click_and_drag());
    let r = Rect::from_min_size(full.min, vec2(size, size));
    register(ui.ctx(), format!("recFace:{}", t.face), r);
    let why = paint_face(app, ui, r, t);
    let resp = match why {
        Some(w) => resp.on_hover_text(format!("This face cannot be shown: {w}")),
        None => resp,
    };
    let picked = app.caches.people_rec.selected.contains(&t.face);
    if picked {
        ui.painter().rect_stroke(r, 3.0, Stroke::new(2.5, tk.accent), StrokeKind::Outside);
    } else if resp.hovered() {
        ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, tk.text_dim), StrokeKind::Outside);
    }
    if let Some(l) = label {
        let l: String = if l.chars().count() > 11 { format!("{}…", l.chars().take(10).collect::<String>()) } else { l.to_string() };
        ui.painter().text(full.left_bottom() + vec2(1.0, -7.0), Align2::LEFT_CENTER, l, tk.font(10.5), tk.text_label);
    }
    if resp.clicked() {
        let sel = &mut app.caches.people_rec.selected;
        match sel.iter().position(|f| *f == t.face) {
            Some(i) => {
                sel.remove(i);
            }
            None => sel.push(t.face.clone()),
        }
    }
    if resp.drag_started() {
        let faces = if picked { app.caches.people_rec.selected.clone() } else { vec![t.face.clone()] };
        let count = faces.len();
        app.ui.dragging_faces = Some(FaceDrag { faces, count, photo: t.photo, rect: t.rect });
    }
    if app.ui.dragging_faces.is_some() {
        return resp;
    }
    resp.on_hover_ui(|ui| {
        let faces = photo_faces(app, t.photo);
        ui.label(egui::RichText::new(if faces.len() > 1 { "Faces in this photo" } else { "This photo" }).strong());
        ui.horizontal(|ui| {
            for f in &faces {
                let Some(th) = thumb_of(t.photo, f) else { continue };
                ui.vertical(|ui| {
                    let hit = th.face == t.face;
                    face(app, ui, &th, if hit { 56.0 } else { 44.0 });
                    ui.label(egui::RichText::new(who(f)).size(11.0).strong().color(if hit { Color32::WHITE } else { ui.visuals().text_color() }));
                });
            }
        });
        ui.label(egui::RichText::new(crate::i18n::tr("Click to pick · drag to move this face")).size(10.5).weak());
    })
}

/// Drop faces on `r`: highlighted while the pointer is over it, `people.move` on the release.
fn face_drop(app: &mut LightcraftApp, ui: &egui::Ui, r: Rect, to: &str, label: &str) {
    let Some(d) = app.ui.dragging_faces.clone() else { return };
    match dnd::over(ui, r) {
        Over::No => {}
        Over::Hover => {
            dnd::highlight(ui, r, true);
            dnd::hint(ui.ctx(), label.to_string());
        }
        Over::Drop => {
            app.ui.dragging_faces = None;
            app.caches.people_rec.selected.clear();
            let Some(v) = act(app, "people.move", json!({"faces": d.faces, "to": to})) else { return };
            let n = v["moved"].as_u64().or(v["unknown"].as_u64()).or(v["ignored"].as_u64()).unwrap_or(0);
            let faces = if n == 1 { "face" } else { "faces" };
            let mut msg = match to {
                "unknown" => format!("{n} {faces} marked as unknown person"),
                "ignored" => format!("{n} {faces} marked as not a real face"),
                _ if n == 0 => format!("Already {label}"),
                _ => format!("{n} {faces} moved to {label}"),
            };
            let (a, r) = (v["keywordsAdded"].as_u64().unwrap_or(0), v["keywordsRemoved"].as_u64().unwrap_or(0));
            if a + r > 0 {
                msg.push_str(&format!(" · keywords: {a} added, {r} removed"));
            }
            msg.push_str(" (People ▸ Undo)");
            app.toast_for(ui.ctx(), msg.clone(), 3.5);
            app.ui.status = msg;
        }
    }
}

/// The two zones faces can be dropped on from anywhere in the view, and what is picked.
fn drop_bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.label(egui::RichText::new(crate::i18n::tr("Drag faces onto a person or a group, or here:")).font(t.font(12.0)).color(t.text_dim));
        for (label, to, tip, id) in [
            ("Unknown person", "unknown", "You looked and don't know them. The faces leave the list to name (no keyword) and wait in “Unknown people”, where you can name them later.", "recDrop:unknown"),
            ("Not a real face", "ignored", "A statue, a poster, a wrong detection: these faces are left out for good.", "recDrop:ignored"),
        ] {
            let (r, resp) = ui.allocate_exact_size(vec2(132.0, 26.0), Sense::hover());
            register(ui.ctx(), id, r);
            let live = app.ui.dragging_faces.is_some();
            ui.painter().rect_filled(r, 4.0, t.inset);
            ui.painter().rect_stroke(r, 4.0, Stroke::new(1.0, if live { t.accent } else { t.divider }), StrokeKind::Inside);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, t.font(12.0), if live { t.text } else { t.text_label });
            face_drop(app, ui, r, to, label);
            resp.on_hover_text(tip);
        }
        let n = app.caches.people_rec.selected.len();
        if n > 0 {
            ui.label(egui::RichText::new(format!("{n} picked")).font(t.font(12.0)).color(t.text));
            if ui.small_button(crate::i18n::tr("Clear")).clicked() {
                app.caches.people_rec.selected.clear();
            }
        }
    });
    ui.add_space(2.0);
}

/// The faces of the photo on screen, each with who it is: drag the right one.
fn this_photo(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(id) = app.session.active() else { return };
    let faces = photo_faces(app, id.0);
    if faces.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let name = app.session.catalog.photo(id).map(|p| p.file_name.clone()).unwrap_or_default();
    ui.label(egui::RichText::new(format!("Faces in the selected photo ({name})")).font(t.semibold(14.0)).color(t.text));
    ui.horizontal_wrapped(|ui| {
        for f in &faces {
            if let Some(th) = thumb_of(id.0, f) {
                tile(app, ui, &th, 64.0, Some(&who(f)));
            }
        }
    });
    ui.add_space(8.0);
}

fn person_card(app: &mut LightcraftApp, ui: &mut egui::Ui, g: &GroupView) {
    let t = Tokens::get(ui.ctx());
    let card = ui
        .vertical(|ui| {
            ui.set_width(THUMB + 12.0);
            if let Some(th) = g.thumbs.first() {
                let (r, resp) = ui.allocate_exact_size(vec2(THUMB + 12.0, THUMB + 12.0), Sense::click());
                let px = ((THUMB + 12.0) * ui.ctx().pixels_per_point()).ceil() as usize;
                let _ = px;
                paint_face(app, ui, r, th);
                if resp.hovered() {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
                }
                if resp.on_hover_text(crate::i18n::tr("Show their photos")).clicked() {
                    act(app, "people.photos", json!({"person": g.id, "filter": true}));
                    let _ = app.run("view.photoGrid", json!({}));
                }
            }
            let label = g.name.clone().unwrap_or_else(|| "Unnamed".into());
            ui.painter().text(ui.cursor().min + vec2(2.0, 8.0), Align2::LEFT_CENTER, &label, t.semibold(12.0), t.text);
            ui.add_space(16.0);
            ui.label(egui::RichText::new(format!("{} photos", g.photos)).font(t.font(11.0)).color(t.text_dim));
            let open = app.caches.people_rec.open_person.as_deref() == Some(g.id.as_str());
            let b = ui
                .small_button(crate::i18n::tr(if open { "Hide faces" } else { "Faces" }))
                .on_hover_text(crate::i18n::tr("List every face of this person: drag one to move it to someone else"));
            register(ui.ctx(), format!("recPersonFaces:{}", g.id), b.rect);
            if b.clicked() {
                app.caches.people_rec.open_person = if open { None } else { Some(g.id.clone()) };
                app.caches.people_rec.members = None;
                app.caches.people_rec.members_shown = 0;
            }
        })
        .response;
    register(ui.ctx(), format!("recPerson:{}", g.id), card.rect);
    let label = g.name.clone().unwrap_or_else(|| "this person".into());
    face_drop(app, ui, card.rect.expand(3.0), &g.id, &label);
}

fn persons(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(ov) = app.caches.people_rec.ov.clone() else { return };
    if ov.persons.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(format!("Recognized people ({})", ov.persons.len())).font(t.semibold(14.0)).color(t.text));
    ui.horizontal_wrapped(|ui| {
        for g in &ov.persons {
            person_card(app, ui, g);
            ui.add_space(8.0);
        }
    });
    ui.add_space(10.0);
}

/// Every face of the opened person, each with the other faces of its photo (named), so the right
/// face of a group photo is the one that is dragged.
fn person_faces(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(pid) = app.caches.people_rec.open_person.clone() else { return };
    let t = Tokens::get(ui.ctx());
    let name = app.caches.people_rec.ov.as_ref().and_then(|o| {
        o.persons
            .iter()
            .find(|g| g.id == pid)
            .map(|g| g.name.clone().unwrap_or_else(|| "Unnamed".into()))
            .or_else(|| o.groups.iter().chain(o.unknown.iter()).find(|g| g.id == pid).map(|g| format!("group of {} faces", g.faces)))
    });
    let Some(name) = name else {
        app.caches.people_rec.open_person = None;
        return;
    };
    let want = app.caches.people_rec.members_shown.max(FIRST_MEMBERS);
    let stale = match app.caches.people_rec.members.as_ref() {
        Some((id, v)) if *id == pid => {
            let (n, total) = (v["faces"].as_array().map_or(0, Vec::len), v["total"].as_u64().unwrap_or(0) as usize);
            n < want.min(total)
        }
        _ => true,
    };
    if stale {
        let key = if pid.starts_with("p:") { "person" } else { "group" };
        let v = app.session.execute("people.members", &json!({key: pid, "limit": want})).unwrap_or(Value::Null);
        app.caches.people_rec.members = Some((pid.clone(), v));
    }
    let v = app.caches.people_rec.members.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("Faces of {name} ({})", v["total"].as_u64().unwrap_or(0))).font(t.semibold(14.0)).color(t.text));
        if ui.small_button(crate::i18n::tr("Close")).clicked() {
            app.caches.people_rec.open_person = None;
            app.caches.people_rec.members_shown = 0;
        }
    });
    // pick faces (click), then say what they are: several at once
    bulk_bar(app, ui);
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "A photo can hold several people: each face is shown with its name. Click faces to pick them, or drag one onto another person.",
        ))
        .font(t.font(11.0))
        .color(t.text_dim),
    );
    ui.horizontal_wrapped(|ui| {
        for m in v["faces"].as_array().cloned().unwrap_or_default() {
            let photo = m["photo"].as_u64().unwrap_or(0);
            let Some(th) = thumb_of(photo, &m) else { continue };
            egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(5.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    tile(app, ui, &th, 64.0, Some(&name));
                    for o in m["others"].as_array().cloned().unwrap_or_default() {
                        if let Some(ot) = thumb_of(photo, &o) {
                            tile(app, ui, &ot, 40.0, Some(&who(&o)));
                        }
                    }
                });
            });
        }
    });
    let shown = v["faces"].as_array().map_or(0, Vec::len);
    more(app, ui, "members", shown, v["total"].as_u64().unwrap_or(0) as usize, FIRST_MEMBERS, |c| &mut c.members_shown);
    ui.add_space(10.0);
}

/// Buttons for the faces that are picked: the same three outcomes as the drop zones, for many faces at once.
fn bulk_bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let picked = app.caches.people_rec.selected.clone();
    let persons: Vec<(String, String)> = app
        .caches
        .people_rec
        .ov
        .as_ref()
        .map(|o| o.persons.iter().filter_map(|p| Some((p.id.clone(), p.name.clone()?))).collect())
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} picked", picked.len()));
        let on = !picked.is_empty();
        for (id, label, to) in [("recBulkUnknown", "Unknown person", "unknown"), ("recBulkNotFace", "Not a real face", "ignored")] {
            let b = ui.add_enabled(on, egui::Button::new(label));
            register(ui.ctx(), format!("button:{id}"), b.rect);
            if b.clicked() && act(app, "people.move", json!({"faces": picked, "to": to})).is_some() {
                app.caches.people_rec.selected.clear();
            }
        }
        ui.add_enabled_ui(on && !persons.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("rec-bulk-person").selected_text("Same person as…").show_ui(ui, |ui| {
                for (pid, name) in &persons {
                    if ui.selectable_label(false, name).clicked() && act(app, "people.move", json!({"faces": picked, "to": pid})).is_some() {
                        app.caches.people_rec.selected.clear();
                    }
                }
            });
        });
        if on && ui.small_button(crate::i18n::tr("Clear")).clicked() {
            app.caches.people_rec.selected.clear();
        }
    });
}

/// The field to name a group, and its “Name” button; Enter names too.
fn name_row(app: &mut LightcraftApp, ui: &mut egui::Ui, gid: &str) {
    let text = app.caches.people_rec.names.entry(gid.to_string()).or_default();
    let r = ui.add(egui::TextEdit::singleline(text).hint_text(crate::i18n::tr("Name this person")).desired_width(180.0));
    let name = text.trim().to_string();
    let go = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        || ui.add_enabled(!name.is_empty(), egui::Button::new(crate::i18n::tr("Name"))).clicked();
    if go && !name.is_empty() {
        app.caches.people_rec.names.remove(gid);
        if let Some(v) = act(app, "people.name", json!({"group": gid, "name": name})) {
            let n = v["photos"].as_u64().unwrap_or(0);
            app.ui.status = format!("{} tagged on {n} photos", v["keyword"].as_str().unwrap_or(""));
        }
    }
}

fn group_button(app: &mut LightcraftApp, ui: &mut egui::Ui, kind: &str, gid: &str, label: &str, tip: &str, cmd: &str, params: Value) {
    let b = ui.button(label).on_hover_text(tip);
    register(ui.ctx(), format!("recBtn:{kind}:{gid}"), b.rect);
    if b.clicked() {
        act(app, cmd, params);
    }
}

fn groups(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(ov) = app.caches.people_rec.ov.clone() else { return };
    let t = Tokens::get(ui.ctx());
    if !ov.grouped {
        if ov.indexed_faces > 0 {
            ui.label(egui::RichText::new(crate::i18n::tr("Grouping the faces…")).font(t.font(12.0)).color(t.text_dim));
        } else {
            ui.label(egui::RichText::new(crate::i18n::tr("No faces yet: choose “Find people in the library”.")).font(t.font(12.0)).color(t.text_dim));
        }
        return;
    }
    ui.label(egui::RichText::new(format!("Groups to name ({})", ov.groups_total)).font(t.semibold(14.0)).color(t.text));
    if ov.stale {
        ui.label(egui::RichText::new(crate::i18n::tr("New faces were found: choose “Group again”.")).font(t.font(12.0)).color(t.caution));
    }
    if ov.skipped_groups > 0 {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} group(s) skipped for now.", ov.skipped_groups)).font(t.font(12.0)).color(t.text_dim));
            let b = ui.small_button(crate::i18n::tr("Show them"));
            register(ui.ctx(), "recBtn:unskip", b.rect);
            if b.clicked() {
                act(app, "people.skip", json!({"clear": true}));
            }
        });
    }
    for g in &ov.groups {
        group_card(app, ui, g, false);
    }
    let shown = ov.groups.len();
    more(app, ui, "groups", shown, ov.groups_total, FIRST_GROUPS, |c| &mut c.groups_shown);
    ui.add_space(10.0);
}

/// A group of faces: pick or drag its faces; name it, or say it is unknown, skip it, or not a face.
/// The whole card takes dropped faces (“these are the same person as this group”).
fn group_card(app: &mut LightcraftApp, ui: &mut egui::Ui, g: &GroupView, unknown: bool) {
    let t = Tokens::get(ui.ctx());
    let card = egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for th in &g.thumbs {
                    tile(app, ui, th, 60.0, None);
                }
            });
            ui.vertical(|ui| {
                // the whole group can be dragged by its caption
                let cap = ui
                    .add(egui::Label::new(egui::RichText::new(format!("{} faces · {} photos · drag", g.faces, g.photos)).font(t.font(12.0)).color(t.text_label)).sense(Sense::drag()))
                    .on_hover_text(crate::i18n::tr("Drag the whole group onto a person (or onto Unknown person / Not a real face)"));
                register(ui.ctx(), format!("recGroupGrip:{}", g.id), cap.rect);
                if cap.drag_started()
                    && let Some(th) = g.thumbs.first()
                {
                    app.ui.dragging_faces = Some(FaceDrag { faces: vec![g.id.clone()], count: g.faces, photo: th.photo, rect: th.rect });
                }
                ui.horizontal(|ui| {
                    name_row(app, ui, &g.id);
                    let open = app.caches.people_rec.open_person.as_deref() == Some(g.id.as_str());
                    let b = ui.small_button(crate::i18n::tr(if open { "Hide faces" } else { "All faces" })).on_hover_text(crate::i18n::tr("List every face of this group, to pick several and say what they are"));
                    register(ui.ctx(), format!("recGroupFaces:{}", g.id), b.rect);
                    if b.clicked() {
                        app.caches.people_rec.open_person = if open { None } else { Some(g.id.clone()) };
                        app.caches.people_rec.members = None;
                        app.caches.people_rec.members_shown = 0;
                    }
                });
                ui.horizontal(|ui| {
                    if unknown {
                        group_button(app, ui, "known", &g.id, "Not unknown", "Put these faces back among the groups to name", "people.unknown", json!({"group": g.id, "off": true}));
                    } else {
                        group_button(
                            app,
                            ui,
                            "unknown",
                            &g.id,
                            "Unknown person",
                            "You looked and don't know them. They leave this list (no keyword by default) and wait in “Unknown people”, where you can name them later.",
                            "people.unknown",
                            json!({"group": g.id}),
                        );
                        group_button(app, ui, "skip", &g.id, "Skip for now", "Hide this group for now. Nothing is decided; it comes back with “Group again”.", "people.skip", json!({"group": g.id}));
                    }
                    group_button(app, ui, "notface", &g.id, "Not a real face", "A statue, a poster, a wrong detection: leave these faces out for good.", "people.ignore", json!({"group": g.id}));
                });
                for l in &g.likely {
                    if let Some(n) = &l.name {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("Looks like {n} ({:.0} %)", l.score * 100.0)).font(t.font(12.0)).color(t.text_dim));
                            if ui.small_button(crate::i18n::tr("Yes, same")).clicked() {
                                act(app, "people.confirm", json!({"face": g.id, "person": format!("p:{}", l.person), "answer": "yes"}));
                            }
                            if ui.small_button(crate::i18n::tr("No")).clicked() {
                                act(app, "people.confirm", json!({"face": g.id, "person": format!("p:{}", l.person), "answer": "no"}));
                            }
                        });
                    }
                }
            });
        });
    });
    register(ui.ctx(), format!("recGroup:{}", g.id), card.response.rect);
    face_drop(app, ui, card.response.rect, &g.id, "the same person as this group");
}

/// Faces marked “Unknown person”: out of the list to name, kept here to be named later.
fn unknown_people(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &Value) {
    let Some(ov) = app.caches.people_rec.ov.clone() else { return };
    if ov.unknown_total == 0 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.label(
        egui::RichText::new(format!("Unknown people ({} groups · {} faces)", ov.unknown_total, ov.unknown_faces))
            .font(t.semibold(14.0))
            .color(t.text),
    );
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr("Seen and not known: no keyword unless you choose below. Name one when you find out who it is."))
                .font(t.font(11.0))
                .color(t.text_dim),
        );
    });
    let mut tag = st["index"]["settings"]["tagUnknown"].as_bool().unwrap_or(false);
    let before = tag;
    ui.checkbox(&mut tag, "Give their photos the keyword People|Unknown");
    if tag != before {
        act(app, "people.settings", json!({"tagUnknown": tag}));
    }
    for g in &ov.unknown {
        group_card(app, ui, g, true);
    }
    let shown = ov.unknown.len();
    more(app, ui, "unknown", shown, ov.unknown_total, FIRST_GROUPS, |c| &mut c.groups_shown);
    ui.add_space(10.0);
}

fn suggestions(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(ov) = app.caches.people_rec.ov.clone() else { return };
    if ov.suggestions.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(format!("Looks like someone you named ({})", ov.suggestions_total)).font(t.semibold(14.0)).color(t.text));
    ui.label(egui::RichText::new(crate::i18n::tr("Nothing is tagged until you say yes.")).font(t.font(11.0)).color(t.text_dim));
    ui.horizontal_wrapped(|ui| {
        for sg in &ov.suggestions {
            egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(6.0).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(THUMB + 20.0);
                    face(app, ui, &Thumb { photo: sg.photo, face: sg.face.clone(), rect: sg.rect, video: sg.video.clone() }, THUMB + 20.0);
                    let name = sg.name.clone().unwrap_or_else(|| "this person".into());
                    ui.label(egui::RichText::new(format!("{name}? {:.0} %", sg.score * 100.0)).font(t.font(11.0)).color(t.text_label));
                    ui.horizontal(|ui| {
                        if ui.small_button(crate::i18n::tr("Yes")).clicked() {
                            act(app, "people.confirm", json!({"face": sg.face, "person": format!("p:{}", sg.person), "answer": "yes"}));
                        }
                        if ui.small_button(crate::i18n::tr("No")).clicked() {
                            act(app, "people.confirm", json!({"face": sg.face, "person": format!("p:{}", sg.person), "answer": "no"}));
                        }
                    });
                });
            });
        }
    });
    let shown = ov.suggestions.len();
    more(app, ui, "suggestions", shown, ov.suggestions_total, FIRST_SUGGESTIONS, |c| &mut c.suggestions_shown);
}
