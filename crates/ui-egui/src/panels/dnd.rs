//! Drag and drop of photos and faces.
//!
//! **Photos** are dragged from the grid and from the filmstrip (the selection moves together; a
//! photo that wasn't selected becomes the selection). They are dropped on a regular album (adds
//! them; a smart album or a folder refuses with a message) or on a keyword of the Keywords tree
//! of the left panel (adds the keyword: that tree also holds `People|<name>` and `Places|…`, so a
//! photo dropped on a person or a place is keyworded the same way; a person's keyword files the
//! photo under them as a whole, see `people.tagPhotos`). **Faces** are dragged from the People
//! view (one tile or the selected ones) onto a person, a group, "Unknown person" or "Not a real
//! face".
//!
//! Nothing is written before the button is released over a target: a drag only highlights. A
//! drop target calls [`over`] each frame; the pointer's release frame is its drop. [`feedback`]
//! (last in the frame) paints the badge at the pointer and ends the drag.

use egui::{Color32, Rect, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;

use crate::LightcraftApp;
use crate::theme::Tokens;

/// Faces being dragged in the People view.
#[derive(Clone, Debug)]
pub struct FaceDrag {
    /// What `people.move` takes: face ids (`KEY:N`), or a group (`g:N`).
    pub faces: Vec<String>,
    /// How many faces that is (a group counts all its faces).
    pub count: usize,
    /// The first face's photo and box, for the picture under the pointer.
    pub photo: u64,
    pub rect: [f32; 4],
}

/// Where the pointer is relative to a drop target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Over {
    No,
    Hover,
    /// The button was released here this frame.
    Drop,
}

/// Whether something is being dragged (photos or faces).
pub fn active(app: &LightcraftApp) -> bool {
    app.ui.dragging_photos.is_some() || app.ui.dragging_faces.is_some()
}

/// The pointer relative to `r` (the part of it that is visible in `ui`).
pub fn over(ui: &egui::Ui, r: Rect) -> Over {
    let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
    match pos {
        Some(p) if r.intersect(ui.clip_rect()).contains(p) => {
            if released {
                Over::Drop
            } else {
                Over::Hover
            }
        }
        _ => Over::No,
    }
}

/// Outline the target under the pointer (`ok: false`: it refuses).
pub fn highlight(ui: &egui::Ui, r: Rect, ok: bool) {
    let t = Tokens::get(ui.ctx());
    let c = if ok { t.accent } else { t.caution };
    ui.painter().rect_filled(r, 4.0, c.gamma_multiply(0.14));
    ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5, c), StrokeKind::Inside);
}

/// What the drag badge says next to the count while the pointer is over a target.
pub fn hint(ctx: &egui::Context, text: impl Into<String>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("dnd-hint"), text.into()));
}

fn take_hint(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new("dnd-hint")))
}

/// Start dragging a photo of the grid or of the filmstrip: the selection when it is in it, else
/// this photo alone (it becomes the selection).
pub fn begin_photos(app: &mut LightcraftApp, id: PhotoId) {
    if !app.session.selection.contains(id) {
        let _ = app.run("library.select", serde_json::json!({"ids": [id.0]}));
    }
    app.ui.dragging_photos = Some(app.session.selection.ids.iter().map(|p| p.0).collect());
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A drop on a keyword row of the Keywords tree (`People|<name>` and `Places|…` included). Hovering
/// a closed parent for a moment opens it. Returns whether the node should open.
pub fn keyword_drop(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, path: &str, has_children: bool, open: bool) -> bool {
    let Some(ids) = app.ui.dragging_photos.clone() else { return false };
    let mut open_now = false;
    match over(ui, resp.rect) {
        Over::No => {}
        Over::Hover => {
            highlight(ui, resp.rect.shrink2(vec2(8.0, 1.0)), true);
            hint(ui.ctx(), format!("add keyword “{path}”"));
            if has_children && !open {
                let id = egui::Id::new(("kw-hover-open", path.to_lowercase()));
                let now = ui.input(|i| i.time);
                let since: f64 = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || now));
                open_now = now - since > 0.7;
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        Over::Drop => {
            let n = ids.len();
            match app.run("keyword.apply", serde_json::json!({"keyword": path, "ids": ids})) {
                Ok(v) => {
                    let added = v["added"].as_u64().unwrap_or(0) as usize;
                    let msg = if added == 0 {
                        format!("“{path}” was already on {}", plural(n, "photo", "photos"))
                    } else if added < n {
                        format!("Keyword “{path}” added to {} ({} already had it)", plural(added, "photo", "photos"), n - added)
                    } else {
                        format!("Keyword “{path}” added to {}", plural(added, "photo", "photos"))
                    };
                    app.toast(ui.ctx(), msg.clone());
                    app.ui.status = msg;
                }
                Err(e) => app.toast_error(ui.ctx(), e),
            }
            app.ui.dragging_photos = None;
        }
    }
    if over(ui, resp.rect) == Over::No {
        let id = egui::Id::new(("kw-hover-open", path.to_lowercase()));
        ui.data_mut(|d| d.remove::<f64>(id));
    }
    open_now
}

/// A drop on an album row or an album folder.
pub fn album_drop(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, a: &lightcraft_catalog::Album) {
    let Some(ids) = app.ui.dragging_photos.clone() else { return };
    let refuse = if a.folder {
        Some(format!("“{}” is a folder: it holds albums, not photos. Drop on an album", a.name))
    } else if a.is_smart() {
        Some(format!("“{}” is a smart album: it fills itself from its rules. Drop on a regular album", a.name))
    } else {
        None
    };
    match over(ui, resp.rect) {
        Over::No => {}
        Over::Hover => {
            highlight(ui, resp.rect.shrink2(vec2(8.0, 1.0)), refuse.is_none());
            hint(ui.ctx(), refuse.clone().map_or_else(|| format!("add to “{}”", a.name), |_| "not here".to_string()));
        }
        Over::Drop => {
            let n = ids.len();
            if let Some(why) = refuse {
                app.toast_error(ui.ctx(), why.clone());
                app.ui.status = why;
            } else {
                match app.run("album.addPhotos", serde_json::json!({"id": a.id.0, "ids": ids})) {
                    Ok(v) => {
                        let added = v["added"].as_u64().unwrap_or(0) as usize;
                        let msg = if added == 0 {
                            format!("{} already in “{}”", plural(n, "photo", "photos"), a.name)
                        } else if added < n {
                            format!("{} added to “{}” ({} already there)", plural(added, "photo", "photos"), a.name, n - added)
                        } else {
                            format!("{} added to “{}”", plural(added, "photo", "photos"), a.name)
                        };
                        app.toast(ui.ctx(), msg.clone());
                        app.ui.status = msg;
                    }
                    Err(e) => app.toast_error(ui.ctx(), e),
                }
            }
            app.ui.dragging_photos = None;
        }
    }
}

/// While something is dragged: a badge at the pointer (a face picture and a count for faces, the
/// count and what a drop would do for photos); the drag ends when the button is up (drop targets
/// act on the release frame, before this runs) or on Escape.
pub fn feedback(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !(app.ui.dragging_photos.is_some() || app.ui.dragging_faces.is_some()) {
        return;
    }
    let (released, down, pos, esc) =
        ctx.input(|i| (i.pointer.any_released(), i.pointer.any_down(), i.pointer.latest_pos(), i.key_pressed(egui::Key::Escape)));
    let hint = take_hint(ctx);
    if released || !down || esc {
        app.ui.dragging_photos = None;
        app.ui.dragging_faces = None;
        return;
    }
    let Some(pos) = pos else { return };
    let t = Tokens::get(ctx);
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    ctx.request_repaint();
    if let Some(ids) = &app.ui.dragging_photos {
        let n = ids.len();
        let text = match &hint {
            Some(h) => format!("{} → {h}", plural(n, "photo", "photos")),
            None => plural(n, "photo", "photos"),
        };
        egui::Area::new(egui::Id::new("drag-photos")).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(14.0, 10.0)).show(
            ctx,
            |ui| {
                egui::Frame::NONE.fill(t.accent).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(Color32::WHITE).font(t.semibold(12.0)));
                });
            },
        );
    } else if let Some(d) = app.ui.dragging_faces.clone() {
        let n = d.count;
        let text = match &hint {
            Some(h) => format!("{} → {h}", plural(n, "face", "faces")),
            None => plural(n, "face", "faces"),
        };
        let size = 52.0;
        let px = (size * ctx.pixels_per_point()).ceil() as usize;
        let rect = lightcraft_geom::Rect { x0: f64::from(d.rect[0]), y0: f64::from(d.rect[1]), x1: f64::from(d.rect[2]), y1: f64::from(d.rect[3]) };
        let tex = app.session.face_job(PhotoId(d.photo), rect, px).and_then(|job| app.renderer.variant(job)).map(|v| v.tex.id());
        egui::Area::new(egui::Id::new("drag-faces")).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(10.0, 8.0)).show(
            ctx,
            |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
                // the ghost: the face being dragged, half see-through, with the count
                ui.painter().rect_filled(r, 6.0, t.canvas);
                if let Some(id) = tex {
                    ui.painter().image(id, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::from_white_alpha(190));
                }
                ui.painter().rect_stroke(r, 6.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                let b = Rect::from_min_size(r.right_top() + vec2(-8.0, -8.0), vec2(22.0, 18.0));
                ui.painter().rect_filled(b, 9.0, t.accent);
                ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, n.to_string(), t.semibold(11.0), Color32::WHITE);
                ui.painter().text(r.left_bottom() + vec2(0.0, 12.0), egui::Align2::LEFT_CENTER, text, t.semibold(11.0), t.text);
            },
        );
    }
}
