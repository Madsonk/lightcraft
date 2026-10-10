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

use crate::LightcraftApp;
use crate::theme::Tokens;

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
}

impl RecCache {
    pub fn ov_is_none(&self) -> bool {
        self.ov.is_none()
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
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.vertical(|ui| {
                setup(app, ui, &st);
                controls(app, ui, &st);
                questions(app, ui);
                persons(app, ui);
                groups(app, ui);
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

fn person_card(app: &mut LightcraftApp, ui: &mut egui::Ui, g: &GroupView) {
    let t = Tokens::get(ui.ctx());
    ui.vertical(|ui| {
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
        ui.painter().text(ui.cursor().min + vec2(2.0, 8.0), Align2::LEFT_CENTER, label, t.semibold(12.0), t.text);
        ui.add_space(16.0);
        ui.label(egui::RichText::new(format!("{} photos", g.photos)).font(t.font(11.0)).color(t.text_dim));
    });
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
    for g in ov.groups.iter().filter(|g| g.faces >= 2).chain(ov.groups.iter().filter(|g| g.faces < 2).take(6)) {
        egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                faces_row(app, ui, &g.thumbs, 60.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(format!("{} faces · {} photos", g.faces, g.photos)).font(t.font(12.0)).color(t.text_label));
                    ui.horizontal(|ui| {
                        let text = app.caches.people_rec.names.entry(g.id.clone()).or_default();
                        let r = ui.add(egui::TextEdit::singleline(text).hint_text("Name this person").desired_width(180.0));
                        let name = text.trim().to_string();
                        let go = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            || ui.add_enabled(!name.is_empty(), egui::Button::new("Name")).clicked();
                        if go && !name.is_empty() {
                            app.caches.people_rec.names.remove(&g.id);
                            if let Some(v) = act(app, "people.name", json!({"group": g.id, "name": name})) {
                                let n = v["photos"].as_u64().unwrap_or(0);
                                app.ui.status = format!("{} tagged on {n} photos", v["keyword"].as_str().unwrap_or(""));
                            }
                        }
                        if ui.button("Not a person").on_hover_text("Leave these faces out").clicked() {
                            act(app, "people.ignore", json!({"group": g.id}));
                        }
                    });
                    for l in &g.likely {
                        if let Some(n) = &l.name {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Looks like {n} ({:.0} %)", l.score * 100.0)).font(t.font(12.0)).color(t.text_dim),
                                );
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
