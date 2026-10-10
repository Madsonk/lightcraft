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
}

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

/// A face close-up (a square around the face); a gap while it renders.
fn face(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Thumb, size: f32) {
    let tk = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().rect_filled(r, 3.0, tk.canvas);
    let px = (size * ui.ctx().pixels_per_point()).ceil() as usize;
    if let Some(job) = app.session.face_job(PhotoId(t.photo), rect_of(t), px)
        && let Some(tex) = app.renderer.variant(job)
    {
        ui.painter().image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
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
        let ov = app.session.people_overview(40, 24, 6).ok();
        let q =
            app.session.execute("people.questions", &json!({"limit": 3})).ok().and_then(|v| v["questions"].as_array().cloned()).unwrap_or_default();
        let c = &mut app.caches.people_rec;
        c.ov = ov;
        c.questions = q;
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
                questions(app, ui);
                persons(app, ui);
                person_faces(app, ui);
                groups(app, ui);
                unknown_people(app, ui, &st);
                suggestions(app, ui);
            });
        });
        ui.add_space(30.0);
    });
    let _ = t;
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
        ui.label(egui::RichText::new("Recognition needs two models").font(t.semibold(14.0)).color(t.text));
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
            if ui.button("Cancel").clicked() {
                act(app, "people.model.cancel", json!({}));
            }
        } else {
            let agreed = &mut app.caches.people_rec.agreed;
            ui.checkbox(agreed, "I have read the terms and I agree to download the model");
            let go = *agreed;
            if ui.add_enabled(go, egui::Button::new("Download")).clicked() {
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
        ui.label(egui::RichText::new("People").font(t.semibold(15.0)).color(t.text));
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
            if ui.button("Stop").clicked() {
                act(app, "people.cancel", json!({}));
            }
        } else {
            if ui
                .add_enabled(installed, egui::Button::new("Find people in the library"))
                .on_hover_text("Looks at each photo's preview (never the RAW), once. Interrupted runs resume.")
                .clicked()
            {
                act(app, "people.index", json!({"background": true}));
            }
            if ui.add_enabled(installed, egui::Button::new("Group again")).clicked() {
                act(app, "people.refresh", json!({"background": true}));
            }
            if ui.add_enabled(app.session.can_undo_people(), egui::Button::new("Undo")).on_hover_text("Undo the last People decision").clicked() {
                act(app, "people.undo", json!({}));
            }
        }
        if st["computing"].as_bool() == Some(true) {
            ui.spinner();
            ui.label(egui::RichText::new("grouping faces…").font(t.font(12.0)).color(t.text_dim));
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
            ui.label(egui::RichText::new("(off: LightCraft only suggests)").font(t.font(11.0)).color(t.text_dim));
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
    ui.label(egui::RichText::new("Same person?").font(t.semibold(14.0)).color(t.text));
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
    Some(Thumb { photo, face: f["face"].as_str()?.to_string(), rect: [*r.first()?, *r.get(1)?, *r.get(2)?, *r.get(3)?] })
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
    ui.painter().rect_filled(r, 3.0, tk.canvas);
    let px = (size * ui.ctx().pixels_per_point()).ceil() as usize;
    if let Some(job) = app.session.face_job(PhotoId(t.photo), rect_of(t), px)
        && let Some(tex) = app.renderer.variant(job)
    {
        ui.painter().image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
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
        ui.label(egui::RichText::new("Click to pick · drag to move this face").size(10.5).weak());
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
        ui.label(egui::RichText::new("Drag faces onto a person or a group, or here:").font(t.font(12.0)).color(t.text_dim));
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
            if ui.small_button("Clear").clicked() {
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
                ui.painter().rect_filled(r, 3.0, t.canvas);
                if let Some(job) = app.session.face_job(PhotoId(th.photo), rect_of(th), px)
                    && let Some(tex) = app.renderer.variant(job)
                {
                    ui.painter().image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                if resp.hovered() {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
                }
                if resp.on_hover_text("Show their photos").clicked() {
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
                .small_button(if open { "Hide faces" } else { "Faces" })
                .on_hover_text("List every face of this person: drag one to move it to someone else");
            register(ui.ctx(), format!("recPersonFaces:{}", g.id), b.rect);
            if b.clicked() {
                app.caches.people_rec.open_person = if open { None } else { Some(g.id.clone()) };
                app.caches.people_rec.members = None;
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
    let name = app
        .caches
        .people_rec
        .ov
        .as_ref()
        .and_then(|o| o.persons.iter().find(|g| g.id == pid))
        .map(|g| g.name.clone().unwrap_or_else(|| "Unnamed".into()));
    let Some(name) = name else {
        app.caches.people_rec.open_person = None;
        return;
    };
    if app.caches.people_rec.members.as_ref().is_none_or(|(id, _)| *id != pid) {
        let v = app.session.execute("people.members", &json!({"person": pid, "limit": 48})).unwrap_or(Value::Null);
        app.caches.people_rec.members = Some((pid.clone(), v));
    }
    let v = app.caches.people_rec.members.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("Faces of {name} ({})", v["total"].as_u64().unwrap_or(0))).font(t.semibold(14.0)).color(t.text));
        if ui.small_button("Close").clicked() {
            app.caches.people_rec.open_person = None;
        }
    });
    ui.label(
        egui::RichText::new("A photo can hold several people: each face is shown with its name. Drag one onto another person.")
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
    ui.add_space(10.0);
}

/// The field to name a group, and its “Name” button; Enter names too.
fn name_row(app: &mut LightcraftApp, ui: &mut egui::Ui, gid: &str) {
    let text = app.caches.people_rec.names.entry(gid.to_string()).or_default();
    let r = ui.add(egui::TextEdit::singleline(text).hint_text("Name this person").desired_width(180.0));
    let name = text.trim().to_string();
    let go =
        (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.add_enabled(!name.is_empty(), egui::Button::new("Name")).clicked();
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
            ui.label(egui::RichText::new("Grouping the faces…").font(t.font(12.0)).color(t.text_dim));
        } else {
            ui.label(egui::RichText::new("No faces yet: choose “Find people in the library”.").font(t.font(12.0)).color(t.text_dim));
        }
        return;
    }
    ui.label(egui::RichText::new(format!("Groups to name ({})", ov.groups_total)).font(t.semibold(14.0)).color(t.text));
    if ov.stale {
        ui.label(egui::RichText::new("New faces were found: choose “Group again”.").font(t.font(12.0)).color(t.caution));
    }
    if ov.skipped_groups > 0 {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} group(s) skipped for now.", ov.skipped_groups)).font(t.font(12.0)).color(t.text_dim));
            let b = ui.small_button("Show them");
            register(ui.ctx(), "recBtn:unskip", b.rect);
            if b.clicked() {
                act(app, "people.skip", json!({"clear": true}));
            }
        });
    }
    for g in ov.groups.iter().filter(|g| g.faces >= 2).chain(ov.groups.iter().filter(|g| g.faces < 2).take(6)) {
        group_card(app, ui, g, false);
    }
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
                    .on_hover_text("Drag the whole group onto a person (or onto Unknown person / Not a real face)");
                register(ui.ctx(), format!("recGroupGrip:{}", g.id), cap.rect);
                if cap.drag_started()
                    && let Some(th) = g.thumbs.first()
                {
                    app.ui.dragging_faces = Some(FaceDrag { faces: vec![g.id.clone()], count: g.faces, photo: th.photo, rect: th.rect });
                }
                ui.horizontal(|ui| {
                    name_row(app, ui, &g.id);
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
                            if ui.small_button("Yes, same").clicked() {
                                act(app, "people.confirm", json!({"face": g.id, "person": format!("p:{}", l.person), "answer": "yes"}));
                            }
                            if ui.small_button("No").clicked() {
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
            egui::RichText::new("Seen and not known: no keyword unless you choose below. Name one when you find out who it is.")
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
    ui.add_space(10.0);
}

fn suggestions(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(ov) = app.caches.people_rec.ov.clone() else { return };
    if ov.suggestions.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(format!("Looks like someone you named ({})", ov.suggestions_total)).font(t.semibold(14.0)).color(t.text));
    ui.label(egui::RichText::new("Nothing is tagged until you say yes.").font(t.font(11.0)).color(t.text_dim));
    ui.horizontal_wrapped(|ui| {
        for sg in &ov.suggestions {
            egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(6.0).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(THUMB + 20.0);
                    face(app, ui, &Thumb { photo: sg.photo, face: sg.face.clone(), rect: sg.rect }, THUMB + 20.0);
                    let name = sg.name.clone().unwrap_or_else(|| "this person".into());
                    ui.label(egui::RichText::new(format!("{name}? {:.0} %", sg.score * 100.0)).font(t.font(11.0)).color(t.text_label));
                    ui.horizontal(|ui| {
                        if ui.small_button("Yes").clicked() {
                            act(app, "people.confirm", json!({"face": sg.face, "person": format!("p:{}", sg.person), "answer": "yes"}));
                        }
                        if ui.small_button("No").clicked() {
                            act(app, "people.confirm", json!({"face": sg.face, "person": format!("p:{}", sg.person), "answer": "no"}));
                        }
                    });
                });
            });
        }
    });
}
