//! File ▸ Online Galleries…: the manager of the galleries published from this library — what is online,
//! the photos of one gallery by chapter, its cover, its guests and access requests, adding the selection
//! or a whole album, and the local organiser ("Organise with AI": chapters, order, cover and the photos
//! to leave out, proposed from what LightCraft knows about the photos and applied only on a click).

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::{Dialog, GalleriesState};
use crate::theme::Tokens;

const TILE: f32 = 104.0;
const LEFT: f32 = 250.0;

fn tr(s: &str) -> String {
    crate::i18n::tr(s).to_string()
}

pub(super) fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, dlg: &mut Dialog) {
    let Dialog::Galleries(st) = dlg else { return };
    let ctx = ui.ctx().clone();
    ui.set_min_size(vec2(960.0, 580.0));
    if let Some(e) = st.error.clone() {
        ui.horizontal(|ui| {
            ui.colored_label(Color32::LIGHT_RED, e);
            if ui.small_button("✕").clicked() {
                st.error = None;
            }
        });
    }
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(LEFT);
            left(app, ui, st);
        });
        ui.separator();
        ui.vertical(|ui| right(app, ui, &ctx, st));
    });
}

/// Every album a gallery can be made from (folders hold albums, not photos).
fn albums(app: &LightcraftApp) -> Vec<(u64, String)> {
    let mut v: Vec<(u64, String)> = app.session.catalog.albums().filter(|a| !a.folder).map(|a| (a.id.0, a.name.clone())).collect();
    v.sort_by_key(|(_, n)| n.to_lowercase());
    v
}

fn reload_rows(app: &mut LightcraftApp, st: &mut GalleriesState) {
    match app.run("gallery.list", json!({})) {
        Ok(v) => st.rows = v["galleries"].as_array().cloned().unwrap_or_default(),
        Err(e) => st.error = Some(e),
    }
}

fn load(app: &mut LightcraftApp, st: &mut GalleriesState, slug: &str) {
    st.plan = None;
    st.code = None;
    match app.run("gallery.open", json!({"slug": slug})) {
        Ok(v) => {
            st.title = v["gallery"]["title"].as_str().unwrap_or_default().to_string();
            st.days = match v["gallery"]["expires"].as_f64().filter(|e| *e > 0.0) {
                Some(ts) => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
                    format!("{}", ((ts - now) / 86_400.0).ceil().max(0.0))
                }
                None => "0".into(),
            };
            st.detail = Some(v);
            st.open = Some(slug.to_string());
            st.guests = app.run("gallery.guests", json!({"slug": slug})).ok().and_then(|v| v["guests"].as_array().cloned()).unwrap_or_default();
            st.requests = app.run("gallery.requests", json!({"slug": slug})).ok().and_then(|v| v["requests"].as_array().cloned()).unwrap_or_default();
        }
        Err(e) => st.error = Some(e),
    }
}

/// Runs a command on the open gallery, then reloads it and the list.
fn act(app: &mut LightcraftApp, st: &mut GalleriesState, command: &str, mut p: Value) {
    let Some(slug) = st.open.clone() else { return };
    p["slug"] = json!(slug);
    match app.run(command, p) {
        Ok(_) => {
            reload_rows(app, st);
            load(app, st, &slug);
        }
        Err(e) => st.error = Some(e),
    }
}

/// Opens the Publish dialog for this library's photos (the selection, or `album`), as a new gallery or
/// added to `add_to`.
fn open_publish(app: &mut LightcraftApp, album: Option<(u64, String)>, add_to: Option<String>) {
    if app.run("dialog.gallery", json!({})).is_ok()
        && let Some(Dialog::Gallery { album: a, add_to: t, .. }) = app.ui.dialog.as_mut()
    {
        *a = album;
        *t = add_to;
    }
}

fn album_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, label: &str, add_to: Option<String>) {
    let list = albums(app);
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id.to_string()).selected_text(label.to_string()).width(210.0).show_ui(ui, |ui| {
        if list.is_empty() {
            ui.label(tr("No album yet."));
        }
        for (aid, name) in &list {
            if ui.selectable_label(false, name).clicked() {
                chosen = Some((*aid, name.clone()));
            }
        }
    });
    if chosen.is_some() {
        open_publish(app, chosen, add_to);
    }
}

fn left(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &mut GalleriesState) {
    let n = app.session.selected().len();
    ui.strong(tr("Online galleries"));
    ui.add_space(4.0);
    let publish = ui.add_enabled(n > 0, egui::Button::new(crate::i18n::tr_format!("New gallery from the selection ({n})…", n = n)));
    crate::widgets::register(ui, "button:galleryNew", publish.rect);
    if publish.clicked() {
        open_publish(app, None, None);
    }
    album_menu(app, ui, "galleryNewAlbum", &tr("New gallery from an album…"), None);
    ui.separator();
    if st.rows.is_empty() {
        ui.label(tr("No gallery online."));
    }
    egui::ScrollArea::vertical().id_salt("galleryList").max_height(470.0).show(ui, |ui| {
        for g in st.rows.clone() {
            let slug = g["slug"].as_str().unwrap_or_default().to_string();
            let title = g["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(&slug).to_string();
            let count = g["photos"].as_u64().unwrap_or(0);
            let req = g["requests"].as_u64().unwrap_or(0);
            let mut text = format!("{title} · {count}");
            if req > 0 {
                text.push_str(&format!(" · ✉ {req}"));
            }
            let r = ui.selectable_label(st.open.as_deref() == Some(slug.as_str()), text);
            crate::widgets::register(ui, format!("gallery:{slug}"), r.rect);
            if r.clicked() {
                load(app, st, &slug);
            }
        }
    });
}

fn right(app: &mut LightcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, st: &mut GalleriesState) {
    let Some(slug) = st.open.clone() else {
        ui.label(tr("Choose a gallery on the left, or make a new one."));
        return;
    };
    let Some(detail) = st.detail.clone() else { return };
    let g = &detail["gallery"];
    let base = app.run("gallery.settings", json!({})).ok().and_then(|v| v["url"].as_str().map(|u| u.trim_end_matches('/').to_string())).unwrap_or_default();

    // settings
    ui.horizontal(|ui| {
        crate::text_field::TextField::singleline("field:galleryEditTitle", &mut st.title).hint(tr("Gallery title")).width(260.0).show(ui);
        if ui.button(tr("Save title")).clicked() && !st.title.trim().is_empty() {
            let t = st.title.clone();
            act(app, st, "gallery.update", json!({"title": t}));
        }
        ui.label(tr("Closes in (days, 0 = never)"));
        crate::text_field::TextField::singleline("field:galleryEditDays", &mut st.days).width(50.0).show(ui);
        if ui.button(tr("Save")).clicked() {
            if let Ok(d) = st.days.trim().parse::<f64>() {
                act(app, st, "gallery.update", json!({"expiresInDays": d.max(0.0)}));
            }
        }
    });
    ui.horizontal(|ui| {
        let mut listed = g["listed"].as_bool().unwrap_or(false);
        if ui.checkbox(&mut listed, tr("Shown on the website")).changed() {
            act(app, st, "gallery.update", json!({"listed": listed}));
        }
        if ui.button(tr("Copy link")).clicked() {
            ctx.copy_text(format!("{base}/g/{slug}"));
            app.toast(ctx, tr("Link copied"));
        }
        let n = app.session.selected().len();
        let add = ui.add_enabled(n > 0, egui::Button::new(crate::i18n::tr_format!("Add the selection ({n})…", n = n)));
        crate::widgets::register(ui, "button:galleryAddSelection", add.rect);
        if add.clicked() {
            open_publish(app, None, Some(slug.clone()));
        }
        album_menu(app, ui, "galleryAddAlbum", &tr("Add an album…"), Some(slug.clone()));
    });
    ui.horizontal(|ui| {
        let ai = ui.button(tr("✨ Organise with AI…")).on_hover_text(tr("Proposes chapters, an order, a cover and the weak photos to leave out, from what LightCraft knows (sharpness, bursts, faces, time). It runs on this machine; nothing changes until you apply it."));
        crate::widgets::register(ui, "button:galleryOrganise", ai.rect);
        if ai.clicked() {
            let lang = if crate::i18n::language().code().starts_with("fr") { "fr" } else { "en" };
            match app.run("gallery.organise", json!({"slug": slug, "lang": lang})) {
                Ok(plan) => st.plan = Some(plan),
                Err(e) => st.error = Some(e),
            }
        }
        if st.confirm.as_deref() == Some(slug.as_str()) {
            if ui.button(tr("Delete for good")).clicked() {
                match app.run("gallery.delete", json!({"slug": slug})) {
                    Ok(_) => {
                        st.confirm = None;
                        st.open = None;
                        st.detail = None;
                        reload_rows(app, st);
                    }
                    Err(e) => st.error = Some(e),
                }
            }
            if ui.button(tr("Cancel")).clicked() {
                st.confirm = None;
            }
        } else if ui.button(tr("Delete the gallery…")).clicked() {
            st.confirm = Some(slug.clone());
        }
    });

    if st.plan.is_some() {
        plan_panel(app, ui, st);
    }
    access_panel(app, ui, ctx, st, &slug);
    ui.separator();
    photos(app, ui, ctx, st, &detail);
}

fn thumb(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64, size: egui::Vec2) -> (egui::Response, Rect) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(rect, 3.0, t.skeleton);
    let pid = PhotoId(id);
    crate::panels::grid::request_thumb(app, pid, 256, 8);
    if let Some(tex) = app.renderer.thumb(pid) {
        let a = tex.size[0].max(1) as f32 / tex.size[1].max(1) as f32;
        let fit = if a >= size.x / size.y { vec2(size.x, size.x / a) } else { vec2(size.y * a, size.y) };
        let r = Rect::from_center_size(rect.center(), fit);
        ui.painter().image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    (resp, rect)
}

/// The proposal of the organiser: what it would do, and the button that does it.
fn plan_panel(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &mut GalleriesState) {
    let Some(plan) = st.plan.clone() else { return };
    let t = Tokens::get(ui.ctx());
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong(tr("✨ Proposal"));
        for note in plan["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            ui.label(egui::RichText::new(note).color(t.text_dim));
        }
        ui.horizontal(|ui| {
            if let Some(c) = plan["cover"].as_u64() {
                ui.label(tr("Cover:"));
                let (_, rect) = thumb(app, ui, c, vec2(54.0, 54.0));
                ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Outside);
            }
            ui.vertical(|ui| {
                for ch in plan["chapters"].as_array().into_iter().flatten() {
                    let n = ch["ids"].as_array().map_or(0, Vec::len);
                    ui.label(format!("{} · {}", ch["name"].as_str().unwrap_or_default(), n));
                }
                let dropped = plan["dropped"].as_array().map_or(0, Vec::len);
                if dropped > 0 {
                    egui::CollapsingHeader::new(crate::i18n::tr_format!("{n} left out (hidden, not deleted)", n = dropped)).id_salt("planDropped").show(ui, |ui| {
                        for d in plan["dropped"].as_array().into_iter().flatten().take(40) {
                            ui.label(format!("{} — {}", d["id"].as_u64().unwrap_or(0), d["reason"].as_str().unwrap_or_default()));
                        }
                    });
                }
            });
        });
        ui.horizontal(|ui| {
            let apply = ui.button(tr("Apply the proposal"));
            crate::widgets::register(ui, "button:galleryApplyPlan", apply.rect);
            if apply.clicked() {
                if let Some(slug) = st.open.clone() {
                    match app.run("gallery.applyPlan", json!({"slug": slug, "plan": plan})) {
                        Ok(_) => {
                            reload_rows(app, st);
                            load(app, st, &slug);
                        }
                        Err(e) => st.error = Some(e),
                    }
                }
            }
            if ui.button(tr("Dismiss")).clicked() {
                st.plan = None;
            }
        });
    });
}

fn access_panel(app: &mut LightcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, st: &mut GalleriesState, slug: &str) {
    let waiting = st.requests.iter().filter(|r| r["status"].as_str() == Some("pending")).count();
    let head = crate::i18n::tr_format!("Guests ({g}) and requests ({r})", g = st.guests.len(), r = waiting);
    egui::CollapsingHeader::new(head).id_salt(("galleryAccess", slug.to_string())).default_open(waiting > 0).show(ui, |ui| {
        for rq in st.requests.clone().into_iter().filter(|r| r["status"].as_str() == Some("pending")) {
            ui.horizontal(|ui| {
                ui.label(format!("{} <{}>", rq["name"].as_str().unwrap_or_default(), rq["email"].as_str().unwrap_or_default()));
                let Some(id) = rq["id"].as_i64() else { return };
                for (label, decision, level) in [("Approve (like)", "approve", "like"), ("Approve + download", "approve", "download"), ("Deny", "deny", "like")] {
                    if ui.button(tr(label)).clicked() {
                        act(app, st, "gallery.requests", json!({"decide": id, "decision": decision, "level": level}));
                    }
                }
            });
        }
        for gu in st.guests.clone() {
            ui.horizontal(|ui| {
                ui.label(format!("{} · {}", gu["name"].as_str().unwrap_or_default(), gu["level"].as_str().unwrap_or_default()));
                if let Some(id) = gu["id"].as_i64()
                    && ui.button(tr("Revoke")).clicked()
                {
                    act(app, st, "gallery.guests", json!({"revoke": id}));
                }
            });
        }
        ui.horizontal(|ui| {
            crate::text_field::TextField::singleline("field:galleryGuestName", &mut st.name).hint(tr("Guest name")).width(160.0).show(ui);
            let labels = ["view", "like", "download"];
            egui::ComboBox::from_id_salt("galleryGuestLevel").selected_text(labels[st.level.min(2)]).show_ui(ui, |ui| {
                for (i, l) in labels.iter().enumerate() {
                    ui.selectable_value(&mut st.level, i, *l);
                }
            });
            if ui.button(tr("Add a code")).clicked() && !st.name.trim().is_empty() {
                let (name, level) = (st.name.trim().to_string(), labels[st.level.min(2)]);
                match app.run("gallery.guests", json!({"slug": slug, "add": name, "level": level})) {
                    Ok(r) => {
                        let code = r["code"].as_str().map(String::from);
                        st.name = String::new();
                        reload_rows(app, st);
                        load(app, st, slug);
                        st.code = code;
                    }
                    Err(e) => st.error = Some(e),
                }
            }
        });
        if let Some(c) = st.code.clone() {
            ui.horizontal(|ui| {
                ui.label(tr("Code (shown once):"));
                ui.monospace(&c);
                if ui.button(tr("Copy")).clicked() {
                    ctx.copy_text(c);
                }
            });
        }
    });
}

/// The gallery's photos, by chapter, each with a menu: cover, remove, hide, move.
fn photos(app: &mut LightcraftApp, ui: &mut egui::Ui, _ctx: &egui::Context, st: &mut GalleriesState, detail: &Value) {
    let t = Tokens::get(ui.ctx());
    let cover = detail["gallery"]["cover"].as_str().unwrap_or_default().to_string();
    let list: Vec<Value> = detail["photos"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        ui.label(tr("No photo in this gallery yet: add the selection or an album."));
        return;
    }
    // local photos in display order: what a move swaps
    let order: Vec<u64> = list.iter().filter_map(|p| p["id"].as_u64()).collect();
    let mut chapters: Vec<(String, Vec<&Value>)> = Vec::new();
    for p in &list {
        let name = p["section"].as_str().unwrap_or_default().to_string();
        match chapters.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => v.push(p),
            None => chapters.push((name, vec![p])),
        }
    }
    let mut action: Option<(String, Value)> = None;
    egui::ScrollArea::vertical().id_salt("galleryPhotos").auto_shrink([false, false]).max_height(330.0).show(ui, |ui| {
        for (name, items) in &chapters {
            if chapters.len() > 1 || !name.is_empty() {
                ui.label(egui::RichText::new(format!("{} · {}", if name.is_empty() { tr("Without chapter") } else { name.clone() }, items.len())).strong());
            }
            ui.horizontal_wrapped(|ui| {
                for p in items {
                    let pid = p["pid"].as_str().unwrap_or_default();
                    let hidden = p["hidden"].as_bool().unwrap_or(false) || p["hidden"].as_i64() == Some(1);
                    let local = p["id"].as_u64();
                    let (resp, rect) = match local {
                        Some(id) => thumb(app, ui, id, vec2(TILE, TILE)),
                        None => {
                            let (r, resp) = ui.allocate_exact_size(vec2(TILE, TILE), Sense::click());
                            ui.painter().rect_filled(r, 3.0, t.skeleton);
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, tr("not in this library"), t.semibold(10.0), t.text_dim);
                            (resp, r)
                        }
                    };
                    crate::widgets::register(ui, format!("galleryPhoto:{pid}"), rect);
                    if hidden {
                        ui.painter().rect_filled(rect, 3.0, Color32::from_black_alpha(150));
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, tr("hidden"), t.semibold(11.0), Color32::WHITE);
                    }
                    if pid == cover {
                        ui.painter().rect_stroke(rect, 3.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                        ui.painter().text(rect.left_top() + vec2(6.0, 4.0), egui::Align2::LEFT_TOP, "★", t.semibold(15.0), t.accent);
                    }
                    resp.context_menu(|ui| {
                        if let Some(id) = local {
                            if ui.button(tr("Use as cover")).clicked() {
                                action = Some(("gallery.update".into(), json!({"cover": id})));
                                ui.close();
                            }
                            if let Some(i) = order.iter().position(|x| *x == id) {
                                if i > 0 && ui.button(tr("Move earlier")).clicked() {
                                    let mut o = order.clone();
                                    o.swap(i, i - 1);
                                    action = Some(("gallery.arrange".into(), json!({"ids": o})));
                                    ui.close();
                                }
                                if i + 1 < order.len() && ui.button(tr("Move later")).clicked() {
                                    let mut o = order.clone();
                                    o.swap(i, i + 1);
                                    action = Some(("gallery.arrange".into(), json!({"ids": o})));
                                    ui.close();
                                }
                            }
                            let label = if hidden { "Show again" } else { "Hide from the page" };
                            if ui.button(tr(label)).clicked() {
                                action = Some(("gallery.remove".into(), json!({"ids": [id], "hide": true, "hidden": !hidden})));
                                ui.close();
                            }
                            if ui.button(tr("Remove from the gallery")).clicked() {
                                action = Some(("gallery.remove".into(), json!({"ids": [id]})));
                                ui.close();
                            }
                        } else {
                            ui.label(tr("Published from another library: manage it there."));
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
    });
    if let Some((command, p)) = action {
        act(app, st, &command, p);
    }
}
