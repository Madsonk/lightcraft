//! The "AI picks" section of the Library sidebar (Sort step): run AI Culling on the view, read the
//! four buckets with their counts and narrow the grid to one, see why the active photo got its
//! verdict, handle the duplicate groups, the key faces (people) and the shooting scenes, and write
//! the proposed stars and flags. All through `cull.*` (see `docs/culler.md`) via `crate::flow`.

use egui::{Color32, RichText, Sense, vec2};
use lightcraft_culler::Bucket;

use crate::LightcraftApp;
use crate::flow;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

pub const BUCKETS: [(&str, &str); 4] = [("highlight", "Best shots"), ("selected", "Selected"), ("review", "To review"), ("rejected", "Rejected")];

pub fn bucket_colour(bucket: Bucket) -> Color32 {
    match bucket {
        Bucket::Highlight => Color32::from_rgb(120, 170, 255),
        Bucket::Selected => Color32::from_rgb(110, 200, 130),
        Bucket::Review => Color32::from_rgb(235, 190, 80),
        Bucket::Rejected => Color32::from_rgb(225, 100, 95),
    }
}

fn colour_of(name: &str) -> Color32 {
    match name {
        "highlight" => bucket_colour(Bucket::Highlight),
        "selected" => bucket_colour(Bucket::Selected),
        "review" => bucket_colour(Bucket::Review),
        _ => bucket_colour(Bucket::Rejected),
    }
}

/// The reasons of a verdict, one translated sentence each (the hover text of the grid, the panel).
pub fn reasons_text(v: &lightcraft_culler::Verdict) -> Vec<String> {
    v.reasons.iter().map(|r| tr(r.text()).to_string()).collect()
}

pub fn bucket_label(b: Bucket) -> &'static str {
    match b {
        Bucket::Highlight => "Best shots",
        Bucket::Selected => "Selected",
        Bucket::Review => "To review",
        Bucket::Rejected => "Rejected",
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    register(ui, "region:aiPicks", ui.max_rect());
    super::right::header(ui, "AI picks");
    if app.flow.cull.genre.is_empty() {
        app.flow.cull.genre = "general".into();
        app.flow.cull.grouping = 0.5;
    }
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 8 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let running = app.flow.cull.running;
        let busy = running || app.flow.job.is_some();
        // ---- run
        ui.horizontal_wrapped(|ui| {
            if running {
                if text_button(ui, "cullCancel", "Stop", false).clicked() {
                    flow::cancel_cull(app);
                }
            } else if crate::design::button(ui, "cullRun", tr("Judge the photos"), crate::design::Kind::Secondary, !busy).clicked()
                && let Err(e) = flow::start_cull(app, &ctx)
            {
                app.toast_error(&ctx, e);
            }
            if crate::design::button(ui, "cullTour", tr("Guided tour"), crate::design::Kind::Quiet, true)
                .on_hover_text(tr("A short tour of the cull and retouch flow"))
                .clicked()
            {
                flow::start_tour(app);
            }
        });
        // ---- one click, first: the way most people will use it
        if crate::design::button(ui, "oneClick", tr("Cull and retouch in one click"), crate::design::Kind::Secondary, !busy).clicked() {
            flow::ask_one_click(app);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Kind of shoot")).color(t.text_label));
            let current = app.flow.cull.genre.clone();
            let combo = crate::widgets::combo_box("cullGenre").selected_text(tr(genre_label(&current))).show_ui(ui, |ui| {
                for g in ["general", "wedding", "portrait", "school", "sport"] {
                    if ui.selectable_label(current == g, tr(genre_label(g))).clicked() {
                        app.flow.cull.genre = g.into();
                    }
                }
            });
            crate::widgets::name_combo(&combo.response, tr("Kind of shoot"));
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Grouping")).color(t.text_label)).on_hover_text(tr("How alike two photos must be to count as duplicates"));
            ui.add(egui::Slider::new(&mut app.flow.cull.grouping, 0.0..=1.0).show_value(false));
        });
        if running {
            let st = &app.flow.cull.status;
            let (d, n) = (st["done"].as_u64().unwrap_or(0), st["total"].as_u64().unwrap_or(0));
            ui.add(egui::ProgressBar::new(if n > 0 { d as f32 / n as f32 } else { 0.0 }).desired_width(ui.available_width()));
            ui.label(RichText::new(tr_format!("{d} of {n} photos judged", d = d, n = n)).size(11.0).color(t.text_dim));
        }
        // ---- the four buckets
        let counts = flow::counts(app);
        match counts {
            None => {
                ui.label(RichText::new(tr("No verdicts yet. Press “Judge the photos”: nothing is written to your ratings.")).color(t.text_dim));
            }
            Some(c) => {
                let total: u64 = c.iter().sum();
                ui.label(RichText::new(tr_format!("{n} photos judged", n = total)).size(11.5).color(t.text_dim));
                let active = app.flow.cull.bucket.clone();
                for (i, (key, label)) in BUCKETS.iter().enumerate() {
                    let on = active.as_deref() == Some(*key);
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                    register(ui, format!("button:cullBucket-{key}"), r);
                    let fill = if on {
                        t.accent.gamma_multiply(0.45)
                    } else if resp.hovered() {
                        t.hover
                    } else {
                        t.button
                    };
                    ui.painter().rect_filled(r, 5.0, fill);
                    ui.painter().circle_filled(r.left_center() + vec2(14.0, 0.0), 5.0, colour_of(key));
                    ui.painter().text(r.left_center() + vec2(28.0, 0.0), egui::Align2::LEFT_CENTER, tr(label), t.semibold(12.0), t.text);
                    ui.painter().text(
                        r.right_center() - vec2(10.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        c[i].to_string(),
                        t.semibold(12.0),
                        t.text_label,
                    );
                    if resp.on_hover_text(tr("Show only these photos (click again to show all)")).clicked() {
                        flow::set_filter(app, if on { None } else { Some(key) }, None);
                    }
                }
                if (app.flow.cull.bucket.is_some() || app.flow.cull.scene.is_some() || app.flow.cull.person.is_some())
                    && text_button(ui, "cullShowAll", "Show all", false).clicked()
                {
                    flow::set_filter(app, None, None);
                }
                if text_button(ui, "cullApply", "Write the proposed stars and flags", false)
                    .on_hover_text(tr("Stars on unrated photos, Pick on the best, Reject on technical failures: one undo step"))
                    .clicked()
                {
                    flow::apply_cull(app, &ctx);
                }
            }
        }
        why(app, ui);
        if counts.is_some() {
            duplicates(app, ui);
            key_faces(app, ui);
            scenes(app, ui);
        }
    });
}

fn genre_label(g: &str) -> &'static str {
    match g {
        "wedding" => "Wedding",
        "portrait" => "Portrait session",
        "school" => "School photos",
        "sport" => "Sport",
        _ => "General",
    }
}

/// The verdict of the active photo and the reasons behind it.
fn why(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = app.session.active() else { return };
    let Some(v) = app.session.catalog.photo(id).and_then(|p| p.analysis.as_ref()).and_then(|a| a.verdict.clone()) else { return };
    ui.add_space(2.0);
    ui.separator();
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 5.0, bucket_colour(v.bucket));
        ui.label(RichText::new(tr(bucket_label(v.bucket))).font(t.semibold(12.5)).color(t.text));
        ui.label(RichText::new(tr_format!("{stars}★ proposed · score {score:.0}", stars = v.stars, score = v.score)).size(11.5).color(t.text_dim));
    });
    register(ui, "region:aiVerdict", ui.min_rect());
    let reasons = reasons_text(&v);
    if reasons.is_empty() {
        ui.label(RichText::new(tr("Nothing wrong found")).color(t.text_dim));
    }
    for r in reasons {
        ui.label(RichText::new(format!("• {r}")).size(11.5).color(t.text_label));
    }
}

fn duplicates(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let groups = flow::duplicate_groups(app);
    let ctx = ui.ctx().clone();
    let head = tr_format!("Duplicates · {n} groups", n = groups.len());
    let open =
        egui::CollapsingHeader::new(RichText::new(head).font(t.semibold(12.0))).id_salt("cullDup").default_open(!groups.is_empty()).show(ui, |ui| {
            if groups.is_empty() {
                ui.label(RichText::new(tr("No look-alike photos found")).color(t.text_dim));
            }
            let active = app.session.active().map(|i| i.0);
            // the active photo's group first, then the others
            let mut keys: Vec<_> = groups.keys().copied().collect();
            keys.sort_by_key(|k| !groups[k].iter().any(|m| Some(m.0) == active));
            for (n, key) in keys.into_iter().take(40).enumerate() {
                let members = &groups[&key];
                let best = members.first().map(|m| m.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tr_format!("Group {n}: {k} photos", n = n + 1, k = members.len())).font(t.semibold(11.5)).color(t.text));
                    if text_button(ui, &format!("dupSelect-{}-{}", key.0, key.1), "Select", false).clicked() {
                        let ids: Vec<u64> = members.iter().map(|m| m.0).collect();
                        let _ = app.run("library.select", serde_json::json!({"ids": ids}));
                    }
                });
                for (id, name, rank, score, _) in members.iter().take(8) {
                    ui.horizontal_wrapped(|ui| {
                        let mark = if *rank == 0 { format!(" · {}", tr("best")) } else { String::new() };
                        let short: String = name.chars().take(22).collect();
                        ui.label(RichText::new(format!("{short}  {score:.0}{mark}")).size(11.0).color(if *rank == 0 { t.text } else { t.text_dim }));
                        if text_button(ui, &format!("dupKeep-{id}"), "Keep", false)
                            .on_hover_text(tr("Pick this photo and reject the others of the group"))
                            .clicked()
                        {
                            let others: Vec<u64> = members.iter().map(|m| m.0).filter(|x| x != id).collect();
                            flow::keep_one(app, &ctx, *id, &others);
                        }
                    });
                }
                if members.len() > 8 {
                    ui.label(RichText::new(tr_format!("… and {n} more", n = members.len() - 8)).size(11.0).color(t.text_dim));
                }
                let _ = best;
            }
        });
    let _ = open;
}

fn key_faces(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::CollapsingHeader::new(RichText::new(tr("Key faces")).font(t.semibold(12.0))).id_salt("cullFaces").default_open(false).show(ui, |ui| {
        flow::load_people(app);
        let people = app.flow.cull.people.clone();
        if people.is_empty() {
            ui.label(RichText::new(tr("Name people in the People view to follow them here")).color(t.text_dim));
            return;
        }
        ui.label(RichText::new(tr("Key people: photos where none of them shows are flagged at the next run")).size(11.0).color(t.text_dim));
        for (pid, name) in people {
            ui.horizontal(|ui| {
                let mut key = app.flow.cull.key_people.contains(&name);
                if ui.checkbox(&mut key, "").changed() {
                    if key {
                        app.flow.cull.key_people.push(name.clone());
                    } else {
                        app.flow.cull.key_people.retain(|n| n != &name);
                    }
                }
                let on = app.flow.cull.person.as_deref() == Some(pid.as_str());
                if text_button(ui, &format!("cullPerson-{pid}"), &name, on).on_hover_text(tr("Show only the photos of this person")).clicked() {
                    flow::set_person_filter(app, if on { None } else { Some(&pid) });
                }
            });
        }
    });
}

fn scenes(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(list) = app.flow.cull.list.as_ref() else { return };
    let rows: Vec<(u64, u64, [u64; 4])> = list["scenes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    let n = |k: &str| s[k].as_u64().unwrap_or(0);
                    (n("scene"), n("count"), [n("highlight"), n("selected"), n("review"), n("rejected")])
                })
                .collect()
        })
        .unwrap_or_default();
    let head = tr_format!("Scenes · {n}", n = rows.len());
    egui::CollapsingHeader::new(RichText::new(head).font(t.semibold(12.0))).id_salt("cullScenes").default_open(false).show(ui, |ui| {
        for (scene, count, b) in rows.into_iter().take(60) {
            let on = app.flow.cull.scene == Some(scene);
            let label = tr_format!(
                "Scene {n} · {count} photos · {hi}/{sel}/{rev}/{rej}",
                n = scene + 1,
                count = count,
                hi = b[0],
                sel = b[1],
                rev = b[2],
                rej = b[3]
            );
            if text_button(ui, &format!("cullScene-{scene}"), &label, on).on_hover_text(tr("Best shots / selected / to review / rejected")).clicked()
            {
                flow::set_filter(app, None, if on { None } else { Some(scene) });
            }
        }
    });
}
