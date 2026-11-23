//! File ▸ Online Galleries…: the manager of the galleries published from this library — what is online,
//! the photos of one gallery by chapter, its cover, its guests and access requests, adding the selection
//! or a whole album, and the local organiser ("Organise with AI": chapters, order, cover and the photos
//! to leave out, proposed from what LightCraft knows about the photos and applied only on a click).

use std::collections::HashMap;

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

mod layout;
use layout::Drop;

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
    ui.set_min_size(vec2(960.0, 640.0));
    if let Some(e) = st.error.clone() {
        ui.horizontal(|ui| {
            ui.colored_label(Color32::LIGHT_RED, e);
            if ui.small_button("✕").clicked() {
                st.error = None;
            }
        });
    }
    if st.setup {
        setup(app, ui, st);
        return;
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

/// First use: the gallery server's address and the owner token. Tried before it is saved.
fn setup(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &mut GalleriesState) {
    ui.add_space(8.0);
    ui.strong(tr("Connect to your gallery server"));
    ui.label(tr("Galleries are hosted on your own server. Enter its address and your owner token once: they stay on this computer."));
    ui.add_space(8.0);
    crate::text_field::TextField::singleline("field:galleryUrl", &mut st.url).hint(tr("https://…")).width(420.0).show(ui);
    crate::text_field::TextField::singleline("field:galleryOwnerToken", &mut st.token).hint(tr("Owner token")).width(420.0).password(true).show(ui);
    ui.add_space(6.0);
    let ready = !st.url.trim().is_empty() && !st.token.trim().is_empty();
    if ui.add_enabled(ready, egui::Button::new(tr("Connect"))).clicked() {
        let try_cfg = app.run("gallery.configure", json!({"url": st.url, "token": st.token, "persist": false}));
        let listed = try_cfg.and_then(|_| app.run("gallery.list", json!({})));
        match listed {
            Ok(v) => {
                // it works: keep it for next time
                match app.run("gallery.configure", json!({"url": st.url, "token": st.token, "persist": true})) {
                    Ok(_) => {
                        st.rows = v["galleries"].as_array().cloned().unwrap_or_default();
                        st.setup = false;
                        st.token.clear();
                        st.error = None;
                    }
                    Err(e) => st.error = Some(e),
                }
            }
            Err(e) => st.error = Some(e),
        }
    }
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
            if st.open.as_deref() != Some(slug) {
                // another gallery: nothing of the last one carries over
                (st.picked, st.anchor, st.drag, st.confirm_remove, st.undo, st.revoke) = Default::default();
                st.view = 0;
            }
            st.title = v["gallery"]["title"].as_str().unwrap_or_default().to_string();
            st.days = match v["gallery"]["expires"].as_f64().filter(|e| *e > 0.0) {
                Some(ts) => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
                    format!("{}", ((ts - now) / 86_400.0).ceil().max(0.0))
                }
                None => "0".into(),
            };
            let here: Vec<&str> = v["photos"].as_array().into_iter().flatten().filter_map(|p| p["pid"].as_str()).collect();
            st.picked.retain(|p| here.contains(&p.as_str()));
            st.detail = Some(v);
            st.open = Some(slug.to_string());
            st.guests = app.run("gallery.guests", json!({"slug": slug})).ok().and_then(|v| v["guests"].as_array().cloned()).unwrap_or_default();
            st.requests = app.run("gallery.requests", json!({"slug": slug})).ok().and_then(|v| v["requests"].as_array().cloned()).unwrap_or_default();
            st.likes = app.run("gallery.likes", json!({"slug": slug})).ok();
        }
        Err(e) => st.error = Some(e),
    }
}

/// Runs commands on the open gallery in turn, then reloads it and the list. `undo` is what takes the change back
/// (kept for the Undo button; empty when it cannot be taken back).
fn act_many(app: &mut LightcraftApp, st: &mut GalleriesState, commands: Vec<(String, Value)>, undo: Vec<(String, Value)>) {
    let Some(slug) = st.open.clone() else { return };
    let photos_only = commands.iter().all(|(c, _)| matches!(c.as_str(), "gallery.arrange" | "gallery.remove"));
    for (command, mut p) in commands {
        p["slug"] = json!(slug);
        if let Err(e) = app.run(&command, p) {
            st.error = Some(e);
            break;
        }
        st.undo = undo.clone();
    }
    reload_rows(app, st);
    // moving, hiding or removing photos changes nothing about the people or the hearts: the photos only
    if photos_only { load_photos(app, st, &slug) } else { load(app, st, &slug) }
}

/// Reloads the open gallery's photos and hearts only (what a move, a hide or a removal changes).
fn load_photos(app: &mut LightcraftApp, st: &mut GalleriesState, slug: &str) {
    match app.run("gallery.open", json!({"slug": slug})) {
        Ok(v) => {
            let here: Vec<&str> = v["photos"].as_array().into_iter().flatten().filter_map(|p| p["pid"].as_str()).collect();
            st.picked.retain(|p| here.contains(&p.as_str()));
            st.detail = Some(v);
            st.likes = app.run("gallery.likes", json!({"slug": slug})).ok(); // a removed photo's hearts stop counting
        }
        Err(e) => st.error = Some(e),
    }
}

fn act(app: &mut LightcraftApp, st: &mut GalleriesState, command: &str, p: Value) {
    act_many(app, st, vec![(command.to_string(), p)], Vec::new());
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
            if let Some(n) = g["likes"].as_u64().filter(|n| *n > 0) {
                text.push_str(&format!(" · ♥ {n}"));
            }
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
        let mut download = g["download"].as_bool().or_else(|| g["download"].as_i64().map(|n| n != 0)).unwrap_or(true);
        let dl = ui.checkbox(&mut download, tr("Downloads allowed")).on_hover_text(tr("Off: nobody can download, whatever their access level."));
        crate::widgets::register(ui, "checkbox:galleryDownload", dl.rect);
        if dl.changed() {
            act(app, st, "gallery.update", json!({"download": download}));
        }
        if ui.button(tr("Copy link")).clicked() {
            let base = app.run("gallery.settings", json!({})).ok().and_then(|v| v["url"].as_str().map(|u| u.trim_end_matches('/').to_string())).unwrap_or_default();
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
                        st.likes = None;
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
    hearts_row(app, ui, ctx, st, &slug);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.tab, 0, tr("Photos"));
        let waiting = st.requests.iter().filter(|r| r["status"].as_str() == Some("pending")).count();
        let label = if waiting > 0 { crate::i18n::tr_format!("Access ({g}) · ✉ {r}", g = st.guests.len(), r = waiting) } else { crate::i18n::tr_format!("Access ({g})", g = st.guests.len()) };
        let t = ui.selectable_value(&mut st.tab, 1, label);
        crate::widgets::register(ui, "tab:galleryAccess", t.rect);
    });
    ui.separator();
    if st.tab == 1 {
        access_panel(app, ui, ctx, st, &slug);
    } else {
        photos(app, ui, ctx, st, &detail, &slug);
    }
}

/// The hearts at a glance: how many, from how many people, the most loved photo, and the button that brings
/// them into the library (the keyword "Client favourite").
fn hearts_row(app: &mut LightcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, st: &mut GalleriesState, slug: &str) {
    let t = Tokens::get(ui.ctx());
    let total = st.likes.as_ref().and_then(|l| l["total"].as_u64()).unwrap_or(0);
    let people = st.likes.as_ref().and_then(|l| l["people"].as_array()).map_or(0, Vec::len);
    let top = st.likes.as_ref().map(|l| l["top"].clone()).filter(|v| !v.is_null());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr_format!("♥ {n} · {p} person(s)", n = total, p = people)).strong());
        if let Some(top) = &top
            && let Some(id) = top["id"].as_u64()
        {
            ui.label(egui::RichText::new(tr("Most loved:")).color(t.text_dim));
            thumb(app, ui, id, vec2(30.0, 30.0));
            ui.label(format!("♥ {}", top["count"].as_u64().unwrap_or(0)));
        }
        let pull = ui.button(tr("Bring the hearts into LightCraft")).on_hover_text(tr("Gives each loved photo the keyword “Client favourite” in your library. Nothing else is changed."));
        crate::widgets::register(ui, "button:galleryPull", pull.rect);
        if pull.clicked() {
            match app.run("gallery.pull", json!({"slug": slug})) {
                Ok(r) => {
                    let (n, gone) = (r["liked"].as_u64().unwrap_or(0), r["unliked"].as_u64().unwrap_or(0));
                    app.toast(ctx, crate::i18n::tr_format!("Client favourites: {n} liked · {gone} taken back", n = n, gone = gone));
                }
                Err(e) => st.error = Some(e),
            }
        }
        if ui.button(tr("Refresh")).clicked() {
            reload_rows(app, st);
            load(app, st, slug);
        }
    });
}

fn thumb(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64, size: egui::Vec2) -> (egui::Response, Rect) {
    thumb_sense(app, ui, id, size, Sense::click())
}

fn thumb_sense(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64, size: egui::Vec2, sense: Sense) -> (egui::Response, Rect) {
    let (rect, resp) = ui.allocate_exact_size(size, sense);
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

const LEVELS: [(&str, &str); 3] = [("view", "View only"), ("like", "View and heart"), ("download", "View, heart and download")];

fn level_label(level: &str) -> String {
    LEVELS.iter().find(|(k, _)| *k == level).map_or_else(|| level.to_string(), |(_, t)| tr(t))
}

/// "dd/mm/yyyy" of a Unix time (UTC), for the date an access was given.
fn day(ts: f64) -> String {
    // days → civil date (Howard Hinnant)
    let z = (ts / 86_400.0).floor() as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d:02}/{m:02}/{y}")
}

/// Everyone who can open the gallery — people let in by email and codes — what each may do, since when and how
/// many hearts they gave; the requests waiting for an answer; adding a code.
fn access_panel(app: &mut LightcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, st: &mut GalleriesState, slug: &str) {
    let t = Tokens::get(ui.ctx());
    let download_on = st.detail.as_ref().map_or(true, |d| d["gallery"]["download"].as_bool().or_else(|| d["gallery"]["download"].as_i64().map(|n| n != 0)).unwrap_or(true));
    let hearts: HashMap<String, u64> = st.likes.as_ref().and_then(|l| l["people"].as_array()).into_iter().flatten().filter_map(|p| Some((p["who"].as_str()?.to_lowercase(), p["count"].as_u64()?))).collect();
    let mut set_level: Option<(String, String)> = None;
    egui::ScrollArea::vertical().id_salt("galleryAccessList").auto_shrink([false, false]).show(ui, |ui| {
        let pending: Vec<Value> = st.requests.iter().filter(|r| r["status"].as_str() == Some("pending")).cloned().collect();
        if !pending.is_empty() {
            ui.strong(tr("Waiting for your answer"));
            for rq in pending {
                ui.horizontal_wrapped(|ui| {
                    let r = ui.label(format!("{} <{}>", rq["name"].as_str().unwrap_or_default(), rq["email"].as_str().unwrap_or_default()));
                    if let Some(m) = rq["message"].as_str().filter(|m| !m.trim().is_empty()) {
                        r.on_hover_text(m);
                    }
                    let Some(id) = rq["id"].as_i64() else { return };
                    for (label, decision, level) in [("Approve (like)", "approve", "like"), ("Approve + download", "approve", "download"), ("Deny", "deny", "like")] {
                        let b = ui.button(tr(label));
                        crate::widgets::register(ui, format!("button:galleryRequest:{id}:{decision}:{level}"), b.rect);
                        if b.clicked() {
                            act(app, st, "gallery.requests", json!({"decide": id, "decision": decision, "level": level}));
                        }
                    }
                });
            }
            ui.separator();
        }
        ui.strong(crate::i18n::tr_format!("People with access ({n})", n = st.guests.len()));
        if !download_on {
            ui.label(egui::RichText::new(tr("Downloads are off for the whole gallery: nobody can download, whatever their level.")).color(t.text_dim));
        }
        if st.guests.is_empty() {
            ui.label(tr("Nobody yet: add a code below, or approve a request."));
        } else {
            egui::Grid::new("galleryAccessGrid").striped(true).num_columns(6).spacing([14.0, 6.0]).show(ui, |ui| {
                for h in ["Name", "Email", "May", "Since", "Hearts", ""] {
                    ui.label(egui::RichText::new(tr(h)).color(t.text_dim));
                }
                ui.end_row();
                for gu in st.guests.clone() {
                    let Some(id) = gu["id"].as_str().map(String::from) else { continue };
                    let name = gu["name"].as_str().unwrap_or_default();
                    let level = gu["level"].as_str().unwrap_or("like").to_string();
                    ui.label(name);
                    match gu["email"].as_str() {
                        Some(e) => ui.label(e),
                        None => ui.label(egui::RichText::new(tr("Code")).color(t.text_dim)),
                    };
                    let combo = egui::ComboBox::from_id_salt(("galleryLevel", id.clone())).selected_text(level_label(&level)).show_ui(ui, |ui| {
                        for (k, label) in LEVELS {
                            let item = ui.selectable_label(level == k, tr(label));
                            crate::widgets::register(ui, format!("galleryLevel:{id}:{k}"), item.rect);
                            if item.clicked() && level != k {
                                set_level = Some((id.clone(), k.to_string()));
                            }
                        }
                    });
                    crate::widgets::register(ui, format!("combo:galleryLevel:{id}"), combo.response.rect);
                    ui.label(gu["created"].as_f64().map(day).unwrap_or_default());
                    ui.label(hearts.get(&name.to_lowercase()).map_or(String::new(), |n| format!("♥ {n}")));
                    if st.revoke.as_deref() == Some(id.as_str()) {
                        ui.horizontal(|ui| {
                            let go = ui.button(tr("Revoke now"));
                            crate::widgets::register(ui, format!("button:galleryRevokeNow:{id}"), go.rect);
                            if go.clicked() {
                                st.revoke = None;
                                act(app, st, "gallery.guests", json!({"revoke": id}));
                            }
                            if ui.button(tr("Cancel")).clicked() {
                                st.revoke = None;
                            }
                        });
                    } else {
                        let b = ui.button(tr("Revoke")).on_hover_text(tr("Cuts this access at once: the link or code stops working."));
                        crate::widgets::register(ui, format!("button:galleryRevoke:{id}"), b.rect);
                        if b.clicked() {
                            st.revoke = Some(id.clone());
                        }
                    }
                    ui.end_row();
                }
            });
        }
        ui.add_space(8.0);
        ui.strong(tr("New guest code"));
        ui.horizontal(|ui| {
            crate::text_field::TextField::singleline("field:galleryGuestName", &mut st.name).hint(tr("Guest name")).width(160.0).show(ui);
            let combo = egui::ComboBox::from_id_salt("galleryGuestLevel").selected_text(level_label(LEVELS[st.level.min(2)].0)).show_ui(ui, |ui| {
                for (i, (_, label)) in LEVELS.iter().enumerate() {
                    ui.selectable_value(&mut st.level, i, tr(label));
                }
            });
            crate::widgets::register(ui, "combo:galleryGuestLevel", combo.response.rect);
            let add = ui.button(tr("Add a code"));
            crate::widgets::register(ui, "button:galleryAddCode", add.rect);
            if add.clicked() && !st.name.trim().is_empty() {
                let (name, level) = (st.name.trim().to_string(), LEVELS[st.level.min(2)].0);
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
    if let Some((id, level)) = set_level {
        act(app, st, "gallery.setAccess", json!({"id": id, "level": level}));
    }
}

/// A photo of the gallery as the server lists it.
#[derive(Clone)]
struct Item {
    pid: String,
    id: Option<u64>,
    name: String,
    section: String,
    hidden: bool,
}

fn items(detail: &Value) -> Vec<Item> {
    detail["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Item {
            pid: p["pid"].as_str().unwrap_or_default().to_string(),
            id: p["id"].as_u64(),
            name: p["name"].as_str().unwrap_or_default().to_string(),
            section: p["section"].as_str().unwrap_or_default().to_string(),
            hidden: p["hidden"].as_bool().unwrap_or(false) || p["hidden"].as_i64() == Some(1),
        })
        .collect()
}

/// pid → (hearts, who gave them)
fn hearts_of(st: &GalleriesState) -> HashMap<String, (usize, Vec<String>)> {
    st.likes
        .as_ref()
        .and_then(|l| l["photos"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["pid"].as_str()?.to_string(), (p["count"].as_u64()? as usize, p["who"].as_array()?.iter().filter_map(|w| w.as_str().map(String::from)).collect()))))
        .collect()
}

/// `gallery.arrange` for a whole display order: every photo, its chapter, the chapters' order.
fn arrange_cmd(flat: &[(String, String)], chapters: &[String]) -> (String, Value) {
    let sections: serde_json::Map<String, Value> = flat.iter().map(|(p, s)| (p.clone(), json!(s))).collect();
    ("gallery.arrange".into(), json!({"pids": flat.iter().map(|(p, _)| p).collect::<Vec<_>>(), "sections": sections, "chapters": chapters}))
}

type Commands = Vec<(String, Value)>;

/// Moves `carried` to `to`: the command, and the one that puts it back. `None` when nothing would change.
fn move_action(flat: &[(String, String)], order: &[String], carried: &[String], to: &Drop) -> Option<(Commands, Commands)> {
    let new = layout::moved(flat, carried, to)?;
    Some((vec![arrange_cmd(&new, &layout::first_appearance(&new))], vec![arrange_cmd(flat, order)]))
}

/// Hides or shows `targets`; taking it back restores each photo's own state.
fn hide_action(list: &[Item], targets: &[String], hide: bool) -> (Commands, Commands) {
    let was = |hidden: bool| -> Vec<&String> { targets.iter().filter(|t| list.iter().any(|i| &i.pid == *t && i.hidden == hidden)).collect() };
    let cmd = |pids: Vec<&String>, hidden: bool| (!pids.is_empty()).then(|| ("gallery.remove".to_string(), json!({"pids": pids, "hide": true, "hidden": hidden})));
    (cmd(targets.iter().collect(), hide).into_iter().collect(), [cmd(was(true), true), cmd(was(false), false)].into_iter().flatten().collect())
}

/// What the grid decided this frame; run after it is drawn.
#[derive(Default)]
struct Out {
    drop: Option<Target>,
    run: Option<(Commands, Commands)>,
}

enum Target {
    Place(Drop),
    Trash,
}

/// The gallery's photos, by chapter. Click to pick (Cmd-click adds, Shift-click takes a range), drag to move
/// them — before or after another photo, onto a chapter's title, or onto the bin — and Backspace/Delete takes
/// the picked photos out of the gallery (after a confirmation: the library is never touched).
fn photos(app: &mut LightcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, st: &mut GalleriesState, detail: &Value, slug: &str) {
    let t = Tokens::get(ui.ctx());
    let list = items(detail);
    if list.is_empty() {
        ui.label(tr("No photo in this gallery yet: add the selection or an album."));
        return;
    }
    let cover = detail["gallery"]["cover"].as_str().unwrap_or_default().to_string();
    let order: Vec<String> = detail["gallery"]["sections_order"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    let flat = layout::display(&list.iter().map(|i| (i.pid.clone(), i.section.clone())).collect::<Vec<_>>(), &order);
    let by_pid: HashMap<&str, &Item> = list.iter().map(|i| (i.pid.as_str(), i)).collect();
    let hearts = hearts_of(st);
    let loved = |pid: &str| hearts.get(pid).map_or(0, |h| h.0);
    let reorder = st.view == 0;
    let mut shown: Vec<&Item> = flat.iter().filter_map(|(p, _)| by_pid.get(p.as_str()).copied()).collect();
    match st.view {
        1 => {
            shown.retain(|i| loved(&i.pid) > 0);
            shown.sort_by_key(|i| std::cmp::Reverse(loved(&i.pid)));
        }
        2 => shown.retain(|i| i.hidden),
        _ => {}
    }
    let visible: Vec<String> = shown.iter().map(|i| i.pid.clone()).collect();
    // what is not on screen (another view) is not picked: no action may reach a photo that cannot be seen
    st.picked.retain(|p| visible.contains(p));
    let hidden_count = list.iter().filter(|i| i.hidden).count();
    let mut out = Out::default();

    // keys: Cmd-A picks everything on screen, Backspace/Delete asks to take the picked photos out
    if !ctx.egui_wants_keyboard_input() && st.confirm_remove.is_empty() {
        let (all, del) = ctx.input(|i| (i.modifiers.command && i.key_pressed(egui::Key::A), i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)));
        if all {
            st.picked = visible.clone();
        } else if del && !st.picked.is_empty() {
            st.confirm_remove = st.picked.clone();
        }
    }

    // taking photos out asks first
    if !st.confirm_remove.is_empty() {
        let n = st.confirm_remove.len();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(crate::i18n::tr_format!("Take {n} out of the gallery?", n = n));
            ui.label(tr("They disappear from the page your clients see, and their hearts stop counting. Your originals and your LightCraft library are not touched. To put them back, add them again."));
            ui.horizontal(|ui| {
                let go = ui.button(tr("Take out of the gallery"));
                crate::widgets::register(ui, "button:galleryRemoveConfirm", go.rect);
                if go.clicked() {
                    let pids = std::mem::take(&mut st.confirm_remove);
                    st.picked.retain(|p| !pids.contains(p));
                    out.run = Some((vec![("gallery.remove".into(), json!({"pids": pids}))], Vec::new()));
                }
                if ui.button(tr("Keep them")).clicked() {
                    st.confirm_remove.clear();
                }
            });
        });
    }

    // toolbar
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let dragging = !st.drag.is_empty();
    let chapter_names: Vec<String> = layout::first_appearance(&flat);
    ui.horizontal_wrapped(|ui| {
        let views = [tr("All photos"), tr("Most loved"), crate::i18n::tr_format!("Hidden photos ({n})", n = hidden_count)];
        let combo = egui::ComboBox::from_id_salt("galleryView").selected_text(views[st.view.min(2)].clone()).show_ui(ui, |ui| {
            for (i, v) in views.iter().enumerate() {
                let item = ui.selectable_value(&mut st.view, i, v.clone());
                crate::widgets::register(ui, format!("galleryView:{i}"), item.rect);
            }
        });
        crate::widgets::register(ui, "combo:galleryView", combo.response.rect);
        let n = st.picked.len();
        if n == 0 {
            ui.label(egui::RichText::new(tr("Click to pick (⌘-click to add, Shift-click for a range), drag to move.")).color(t.text_dim));
        } else {
            ui.label(egui::RichText::new(crate::i18n::tr_format!("Picked: {n}", n = n)).strong());
            let all_hidden = st.picked.iter().all(|p| by_pid.get(p.as_str()).is_some_and(|i| i.hidden));
            let hide = ui.button(tr(if all_hidden { "Show again" } else { "Hide from the page" }));
            crate::widgets::register(ui, "button:galleryHidePicked", hide.rect);
            if hide.clicked() {
                out.run = Some(hide_action(&list, &st.picked, !all_hidden));
            }
            if n == 1 && let Some(id) = by_pid.get(st.picked[0].as_str()).and_then(|i| i.id) && ui.button(tr("Use as cover")).clicked() {
                out.run = Some((vec![("gallery.update".into(), json!({"cover": id}))], Vec::new()));
            }
            if reorder {
                ui.menu_button(tr("Move to chapter"), |ui| {
                    for name in chapter_names.iter().map(String::as_str).chain(std::iter::once("")) {
                        let label = if name.is_empty() { tr("Without chapter") } else { name.to_string() };
                        if ui.button(label).clicked() {
                            out.run = move_action(&flat, &order, &st.picked, &Drop::End(name.to_string()));
                            ui.close();
                        }
                    }
                });
                crate::text_field::TextField::singleline("field:galleryNewChapter", &mut st.new_chapter).hint(tr("New chapter")).width(120.0).show(ui);
                let name = st.new_chapter.trim().to_string();
                let make = ui.add_enabled(!name.is_empty(), egui::Button::new(tr("Create")));
                crate::widgets::register(ui, "button:galleryNewChapter", make.rect);
                if make.clicked() {
                    out.run = move_action(&flat, &order, &st.picked, &Drop::End(name));
                    st.new_chapter.clear();
                }
            }
            if ui.button(tr("Unpick")).clicked() {
                st.picked.clear();
            }
        }
        // the bin: click to take the picked photos out, or drop dragged ones on it
        let bin = ui.add_enabled(n > 0 || dragging, egui::Button::new(tr("🗑 Take out…")));
        crate::widgets::register(ui, "button:galleryTrash", bin.rect);
        if dragging && pointer.is_some_and(|p| bin.rect.contains(p)) {
            ui.painter().rect_stroke(bin.rect.expand(2.0), 4.0, Stroke::new(2.0, Color32::LIGHT_RED), StrokeKind::Outside);
            out.drop = Some(Target::Trash);
        }
        if bin.clicked() && n > 0 {
            st.confirm_remove = st.picked.clone();
        }
        let undo = (!st.undo.is_empty()).then(|| ui.button(tr("↶ Undo")));
        if let Some(u) = &undo {
            crate::widgets::register(ui, "button:galleryUndo", u.rect);
        }
        if undo.is_some_and(|u| u.clicked()) {
            let back = std::mem::take(&mut st.undo);
            act_many(app, st, back, Vec::new());
            st.undo.clear();
        }
    });

    let cx = Grid { cover: &cover, hearts: &hearts, visible: &visible, reorder, pointer, dragging };
    egui::ScrollArea::vertical().id_salt("galleryPhotos").auto_shrink([false, false]).max_height(ui.available_height().max(200.0)).show(ui, |ui| {
        if dragging && let Some(p) = pointer {
            // near the top or bottom edge of the grid: scroll with the drag
            let r = ui.clip_rect();
            if p.y < r.top() + 28.0 {
                ui.scroll_with_delta(vec2(0.0, 14.0));
            } else if p.y > r.bottom() - 28.0 {
                ui.scroll_with_delta(vec2(0.0, -14.0));
            }
        }
        if st.view == 0 {
            let chapters = layout::chapters(&flat);
            let named: Vec<&String> = chapters.iter().map(|(n, _)| n).filter(|n| !n.is_empty()).collect();
            for (name, pids) in &chapters {
                if chapters.len() > 1 || !name.is_empty() {
                    let head = ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{} · {}", if name.is_empty() { tr("Without chapter") } else { name.clone() }, pids.len())).strong());
                        if let Some(i) = named.iter().position(|n| *n == name) {
                            for (up, glyph, tip) in [(true, "▲", "Move chapter earlier"), (false, "▼", "Move chapter later")] {
                                let ok = if up { i > 0 } else { i + 1 < named.len() };
                                if ui.add_enabled(ok, egui::Button::new(glyph).small()).on_hover_text(tr(tip)).clicked() {
                                    let mut o: Vec<&String> = named.clone();
                                    o.swap(i, if up { i - 1 } else { i + 1 });
                                    out.run = Some((vec![("gallery.update".into(), json!({"sections_order": o}))], vec![("gallery.update".into(), json!({"sections_order": order}))]));
                                }
                            }
                        }
                    });
                    let zone = head.response.rect.expand2(vec2(ui.available_width() - head.response.rect.width(), 2.0));
                    if dragging && pointer.is_some_and(|p| zone.contains(p)) {
                        ui.painter().rect_stroke(zone, 4.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                        out.drop = Some(Target::Place(Drop::End(name.clone())));
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    for pid in pids {
                        if let Some(item) = by_pid.get(pid.as_str()) {
                            tile(app, ui, st, item, &cx, &flat, &order, &list, &chapter_names, &mut out);
                        }
                    }
                });
                ui.add_space(6.0);
            }
        } else if shown.is_empty() {
            ui.label(tr(if st.view == 1 { "No heart yet." } else { "No hidden photo." }));
        } else {
            ui.horizontal_wrapped(|ui| {
                for item in &shown {
                    tile(app, ui, st, item, &cx, &flat, &order, &list, &chapter_names, &mut out);
                }
            });
        }
    });

    // the drag follows the pointer and ends with the button
    if dragging {
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        ctx.request_repaint();
        if let Some(p) = pointer {
            let n = st.drag.len();
            egui::Area::new(egui::Id::new("galleryDragBadge")).order(egui::Order::Tooltip).interactable(false).fixed_pos(p + vec2(14.0, 10.0)).show(ctx, |ui| {
                egui::Frame::NONE.fill(t.accent).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
                    ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} photo(s)", n = n)).color(Color32::WHITE).font(t.semibold(12.0)));
                });
            });
        }
        if !ui.input(|i| i.pointer.any_down()) {
            let carried = std::mem::take(&mut st.drag);
            match out.drop.take() {
                Some(Target::Place(to)) => out.run = move_action(&flat, &order, &carried, &to),
                Some(Target::Trash) => st.confirm_remove = carried,
                None => {}
            }
        }
    }
    if let Some((commands, undo)) = out.run.take() {
        act_many(app, st, commands, undo);
    }
    let _ = slug;
}

/// What the tiles of a frame share.
struct Grid<'a> {
    cover: &'a str,
    hearts: &'a HashMap<String, (usize, Vec<String>)>,
    visible: &'a [String],
    reorder: bool,
    pointer: Option<egui::Pos2>,
    dragging: bool,
}

/// One photo: its picture, hearts, state, and what a click, a drag, a drop on it or a right click does.
#[allow(clippy::too_many_arguments)]
fn tile(app: &mut LightcraftApp, ui: &mut egui::Ui, st: &mut GalleriesState, item: &Item, cx: &Grid, flat: &[(String, String)], order: &[String], list: &[Item], chapters: &[String], out: &mut Out) {
    let t = Tokens::get(ui.ctx());
    let pid = item.pid.as_str();
    let sense = Sense::click_and_drag();
    let (resp, rect) = match item.id {
        Some(id) => thumb_sense(app, ui, id, vec2(TILE, TILE), sense),
        None => {
            let (r, resp) = ui.allocate_exact_size(vec2(TILE, TILE), sense);
            ui.painter().rect_filled(r, 3.0, t.skeleton);
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, tr("not in this library"), t.semibold(10.0), t.text_dim);
            (resp, r)
        }
    };
    crate::widgets::register(ui, format!("galleryPhoto:{pid}"), rect);
    let picked = st.picked.iter().any(|p| p == pid);
    let p = ui.painter();
    if item.hidden {
        p.rect_filled(rect, 3.0, Color32::from_black_alpha(150));
        p.text(rect.center(), egui::Align2::CENTER_CENTER, tr("hidden"), t.semibold(11.0), Color32::WHITE);
    }
    if picked {
        p.rect_filled(rect, 3.0, t.accent.gamma_multiply(0.25));
        p.rect_stroke(rect, 3.0, Stroke::new(2.5, t.accent), StrokeKind::Inside);
        p.text(rect.right_top() + vec2(-6.0, 3.0), egui::Align2::RIGHT_TOP, "✔", t.semibold(14.0), t.accent);
    }
    if pid == cx.cover {
        p.text(rect.left_top() + vec2(6.0, 4.0), egui::Align2::LEFT_TOP, "★", t.semibold(15.0), t.accent);
    }
    if let Some((n, _)) = cx.hearts.get(pid).filter(|(n, _)| *n > 0) {
        let g = p.layout_no_wrap(format!("♥ {n}"), t.semibold(11.0), Color32::WHITE);
        let r = Rect::from_min_size(rect.right_bottom() - g.size() - vec2(10.0, 8.0), g.size() + vec2(8.0, 4.0));
        p.rect_filled(r, 8.0, Color32::from_rgb(190, 50, 80));
        p.galley(r.min + vec2(4.0, 2.0), g, Color32::WHITE);
    }

    // pick
    if resp.clicked_by(egui::PointerButton::Primary) {
        let (command, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        layout::click(&mut st.picked, &mut st.anchor, cx.visible, pid, command, shift);
    }
    // drag
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if !picked {
            st.picked = vec![pid.to_string()];
            st.anchor = Some(pid.to_string());
        }
        st.drag = layout::carried(&st.picked, pid);
    }
    // drop: the left half of a tile is "before it", the right half "after it"
    if cx.dragging && cx.reorder && let Some(ptr) = cx.pointer && rect.expand2(vec2(4.0, 3.0)).contains(ptr) && ui.clip_rect().contains(ptr) && !st.drag.iter().any(|d| d == pid) {
        let before = ptr.x < rect.center().x;
        let x = if before { rect.left() - 3.0 } else { rect.right() + 3.0 };
        ui.painter().line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(3.0, t.accent));
        out.drop = Some(Target::Place(if before { Drop::Before(pid.to_string()) } else { Drop::After(pid.to_string()) }));
    }

    // right click acts on the picked photos when this one is among them, else on it alone
    if resp.secondary_clicked() && !picked {
        st.picked = vec![pid.to_string()];
        st.anchor = Some(pid.to_string());
    }
    let targets = layout::carried(&st.picked, pid);
    resp.context_menu(|ui| {
        if let Some(id) = item.id.filter(|_| targets.len() == 1) {
            if ui.button(tr("Use as cover")).clicked() {
                out.run = Some((vec![("gallery.update".into(), json!({"cover": id}))], Vec::new()));
                ui.close();
            }
        }
        if targets.len() == 1 && cx.reorder {
            if let Some(i) = flat.iter().position(|(p, _)| p == pid) {
                if i > 0 && ui.button(tr("Move earlier")).clicked() {
                    out.run = move_action(flat, order, &targets, &Drop::Before(flat[i - 1].0.clone()));
                    ui.close();
                }
                if i + 1 < flat.len() && ui.button(tr("Move later")).clicked() {
                    out.run = move_action(flat, order, &targets, &Drop::After(flat[i + 1].0.clone()));
                    ui.close();
                }
            }
        }
        if cx.reorder {
            ui.menu_button(tr("Move to chapter"), |ui| {
                for name in chapters.iter().map(String::as_str).chain(std::iter::once("")) {
                    let label = if name.is_empty() { tr("Without chapter") } else { name.to_string() };
                    if ui.button(label).clicked() {
                        out.run = move_action(flat, order, &targets, &Drop::End(name.to_string()));
                        ui.close();
                    }
                }
            });
        }
        let all_hidden = targets.iter().all(|p| list.iter().any(|i| &i.pid == p && i.hidden));
        if ui.button(tr(if all_hidden { "Show again" } else { "Hide from the page" })).clicked() {
            out.run = Some(hide_action(list, &targets, !all_hidden));
            ui.close();
        }
        if ui.button(tr("Remove from the gallery")).clicked() {
            st.confirm_remove = targets.clone();
            ui.close();
        }
        if item.id.is_none() {
            ui.label(tr("Published from another library: it can be moved, hidden or removed here."));
        }
    });
    let who = cx.hearts.get(pid).map(|(_, w)| w.clone()).unwrap_or_default();
    resp.on_hover_ui(|ui| {
        if !item.name.is_empty() {
            ui.label(&item.name);
        }
        if !who.is_empty() {
            ui.label(egui::RichText::new(tr("Loved by")).strong());
            for w in who.iter().take(20) {
                ui.label(format!("♥ {w}"));
            }
        }
    });
}
