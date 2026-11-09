//! Portrait retouch in the Develop space (tool strip ▸ Portrait, Shift+T): named sliders in
//! collapsible groups (skin, wrinkles, face, mouth and eyes, hair, neck, background), a Before /
//! After preview that stores nothing, Apply (one undo step), Auto retouch for the photo or the
//! selection / album, and "Open eyes" (level 1: the eyes of the same person from another shot of the
//! burst, previewed and validated). All through `portrait.*` (see `docs/portrait-retouch.md`).

use egui::{RichText, Sense, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::flow::{self, Pair};
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// The group captions in the order a retoucher works down the face.
const GROUPS: [&str; 7] = ["Skin", "Wrinkles", "Face", "Mouth and eyes", "Hair", "Neck", "Background"];

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    super::right::header(ui, "Portrait retouch");
    register(ui, "region:portraitPanel", ui.max_rect());
    flow::load_portrait(app);
    if app.flow.portrait.want_eyes {
        if app.flow.portrait.busy_frames == 0 {
            app.flow.portrait.busy_frames = 1;
            ctx.request_repaint();
        } else {
            app.flow.portrait.want_eyes = false;
            app.flow.portrait.busy_frames = 0;
            flow::run_eyes_now(app, &ctx);
        }
    }
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 8 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let busy = app.flow.job.is_some();
        let working = app.flow.portrait.preview.is_some() || app.flow.portrait.apply.is_some() || app.flow.portrait.want_eyes;
        // ---- auto retouch
        let batch_running = app.flow.job.as_ref().is_some_and(|j| matches!(j.phase, flow::Phase::Retouching));
        ui.horizontal_wrapped(|ui| {
            if crate::design::button(ui, "portraitAuto", tr("Auto retouch"), crate::design::Kind::Secondary, !busy && !working && !batch_running)
                .on_hover_text(tr("The prudent default recipe on this photo: one undo step"))
                .clicked()
            {
                flow::apply_portrait(app, true);
            }
            let (ids, with_selection) = flow::targets(app);
            let n = ids.len();
            let auto_label =
                if with_selection { tr_format!("Auto retouch selection ({n})", n = n) } else { tr_format!("Auto retouch album ({n})", n = n) };
            if text_button(ui, "portraitAlbum", &auto_label, false)
                .on_hover_text(tr("Auto retouch every photo with the default recipe, one by one; Stop keeps what is done"))
                .clicked()
                && !busy
                && !batch_running
            {
                flow::start_retouch(app, ids.clone());
            }
            let apply_label = if with_selection {
                tr_format!("Apply settings to selection ({n})", n = n)
            } else {
                tr_format!("Apply settings to album ({n})", n = n)
            };
            if text_button(ui, "portraitBatchSettings", &apply_label, false)
                .on_hover_text(tr("Apply the current sliders to every photo, one by one (AI retouch in batch); Stop keeps what is done"))
                .clicked()
                && !busy
                && !batch_running
            {
                match flow::current_portrait_steps(app) {
                    Ok(steps) => flow::start_retouch_with_steps(app, ids, steps),
                    Err(e) => app.flow.portrait.error = e,
                }
            }
        });
        if working || batch_running {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Working…")).color(t.accent));
                if text_button(ui, "portraitStop", "Stop", false).clicked() {
                    if let Some(pv) = &mut app.flow.portrait.preview {
                        pv.cancel();
                    }
                    app.flow.portrait.preview = None;
                    if let Some((run, _)) = &mut app.flow.portrait.apply {
                        run.cancel();
                    }
                    app.flow.portrait.apply = None;
                    if let Some(job) = &mut app.flow.job {
                        job.stop = true;
                        if let Some(run) = &mut job.run {
                            run.cancel();
                        }
                    }
                }
            });
        }
        if batch_running && let Some(job) = &app.flow.job {
            let (retouched, no_face, failed) = job.run.as_ref().map_or((0, 0, 0), |r| r.counts());
            let done = job.run.as_ref().map_or(job.done, |r| r.done);
            let total = job.total.max(1);
            ui.add(
                egui::ProgressBar::new((done as f32 / total as f32).clamp(0.0, 1.0))
                    .text(format!("{done}/{total}"))
                    .desired_width(ui.available_width()),
            );
            let elapsed = job.started.elapsed().as_secs_f64();
            let eta = if done > 0 && done < total {
                let per = elapsed / done as f64;
                let rem = per * (total.saturating_sub(done)) as f64;
                tr_format!(" · ~{rem}s left", rem = rem.round().max(0.0) as u64)
            } else {
                String::new()
            };
            ui.label(
                RichText::new(tr_format!(
                    "Retouched: {r} · No face: {nf} · Failed: {f}{eta}",
                    r = retouched,
                    nf = no_face,
                    f = failed,
                    eta = eta
                ))
                .size(11.5)
                .color(t.text_dim),
            );
            ui.label(
                RichText::new(tr("AI batch retouch runs one photo at a time; originals are never touched. Stop keeps completed photos (one undo step)."))
                    .size(11.0)
                    .color(t.text_dim),
            );
        }
        if !app.flow.portrait.error.is_empty() {
            ui.label(RichText::new(app.flow.portrait.error.clone()).color(egui::Color32::from_rgb(225, 120, 110)));
        }
        if let Some(pair) = &app.flow.portrait.pair {
            ui.label(RichText::new(app.flow.portrait.note.clone()).size(11.0).color(t.text_dim));
            before_after(ui, pair, "portraitPair");
        }
        if crate::design::button(ui, "portraitRecipe", tr("Natural skin recipe…"), crate::design::Kind::Quiet, true)
            .on_hover_text(tr("The ready-made retouch with Light / Medium / Strong, copies and Before / After"))
            .clicked()
        {
            let _ = app.run("panel.proposals", json!({}));
        }
        // ---- the groups
        let sliders = app.flow.portrait.sliders.clone();
        for group in GROUPS {
            let members: Vec<_> = sliders.iter().filter(|s| s.group == group).collect();
            if members.is_empty() {
                continue;
            }
            let touched = members.iter().filter(|s| app.flow.portrait.amounts.get(&s.id).copied().unwrap_or(0.0) > 0.0).count();
            let head = if touched > 0 { format!("{} · {touched}", tr(group)) } else { tr(group).to_string() };
            egui::CollapsingHeader::new(RichText::new(head).font(t.semibold(12.0)))
                .id_salt(("portraitGroup", group))
                .default_open(group == "Skin")
                .show(ui, |ui| {
                    ui.spacing_mut().slider_width = (ui.available_width() - 215.0).max(50.0);
                    for s in members {
                        let mut v = app.flow.portrait.amounts.get(&s.id).copied().unwrap_or(0.0);
                        let resp = ui.add(egui::Slider::new(&mut v, 0.0..=100.0).text(tr(&s.label)).integer());
                        register(ui, format!("slider:portrait.{}", s.id), resp.rect);
                        if resp.changed() {
                            app.flow.portrait.amounts.insert(s.id.clone(), v);
                        }
                        if s.id == "background" && v > 0.0 {
                            ui.horizontal(|ui| {
                                for (mode, label) in [("blur", "Blur"), ("dim", "Dim")] {
                                    if text_button(ui, &format!("portraitBg-{mode}"), label, app.flow.portrait.background_mode == mode).clicked() {
                                        app.flow.portrait.background_mode = mode.into();
                                    }
                                }
                            });
                        }
                    }
                });
        }
        // ---- preview / apply / undo
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if text_button(ui, "portraitPreview", "Preview", false).on_hover_text(tr("Before / After without storing anything")).clicked() {
                flow::start_preview(app);
            }
            if text_button(ui, "portraitApply", "Apply", false).on_hover_text(tr("Store this retouch on the photo (original untouched)")).clicked() {
                flow::apply_portrait(app, false);
            }
            if text_button(ui, "portraitZero", "Zero the sliders", false).clicked() {
                for v in app.flow.portrait.amounts.values_mut() {
                    *v = 0.0;
                }
                app.flow.portrait.pair = None;
            }
        });
        ui.horizontal_wrapped(|ui| {
            let can_undo = !app.session.undo.is_empty();
            if ui.add_enabled(can_undo, egui::Button::new(tr("Undo"))).clicked() {
                let _ = app.run("edit.undo", serde_json::json!({}));
                app.flow.portrait.loaded_for = None;
            }
            if ui.add_enabled(!app.session.redo.is_empty(), egui::Button::new(tr("Redo"))).clicked() {
                let _ = app.run("edit.redo", serde_json::json!({}));
                app.flow.portrait.loaded_for = None;
            }
            if text_button(ui, "portraitReset", "Remove the retouch", false).on_hover_text(tr("Back to the photo without portrait retouch")).clicked()
            {
                if let Some(id) = app.session.active() {
                    let _ = app.run("portrait.reset", serde_json::json!({"id": id.0}));
                }
                app.flow.portrait.loaded_for = None;
            }
        });
        // ---- open eyes
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new(tr("Open eyes")).font(t.semibold(12.0))).id_salt("portraitEyes").default_open(false).show(
            ui,
            |ui| {
                ui.label(
                    RichText::new(tr(
                        "Takes the eyes of the same person from another shot of the burst. Always a preview first; nothing is invented.",
                    ))
                    .size(11.0)
                    .color(t.text_dim),
                );
                if text_button(ui, "eyesPreview", "Preview the eyes", false).clicked() {
                    app.flow.portrait.want_eyes = true;
                    app.flow.portrait.error.clear();
                }
                if let Some(pair) = &app.flow.portrait.eyes {
                    before_after(ui, pair, "eyesPair");
                    let mut accept = false;
                    let mut reject = false;
                    ui.horizontal(|ui| {
                        accept = crate::design::button(ui, "eyesAccept", tr("Accept"), crate::design::Kind::Secondary, true).clicked();
                        reject = text_button(ui, "eyesReject", "Discard", false).clicked();
                    });
                    if accept {
                        flow::confirm_eyes(app, &ctx);
                    }
                    if reject {
                        app.flow.portrait.eyes = None;
                    }
                }
            },
        );
    });
}

/// Before and After side by side, each as wide as half the panel.
fn before_after(ui: &mut egui::Ui, pair: &Pair, id: &str) {
    let t = Tokens::get(ui.ctx());
    let w = (ui.available_width() - 8.0) / 2.0;
    let ratio = pair.size[1] as f32 / pair.size[0].max(1) as f32;
    let h = (w * ratio).min(w * 1.6);
    if !pair.info.is_empty() {
        ui.label(RichText::new(pair.info.clone()).size(11.0).color(t.text_dim));
    }
    ui.horizontal(|ui| {
        for (label, tex, key) in [("Before", &pair.before, "before"), ("After", &pair.after, "after")] {
            ui.vertical(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                register(ui, format!("image:{id}-{key}"), r);
                ui.label(RichText::new(tr(label)).size(11.0).color(t.text_label));
            });
        }
    });
}
