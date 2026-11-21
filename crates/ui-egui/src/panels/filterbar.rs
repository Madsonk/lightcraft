//! The library filter bar above the grid: rating (≥ / = / ≤), flags, colour labels, kind,
//! edited state and camera / lens / keyword pickers — every control drives `library.filter`.
//! "Save as Smart Album…" keeps the current view as a live album; "Clear" resets the filter.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, vec2};
use lightcraft_catalog::{ColorLabel, Flag, MediaKind, RatingOp};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

pub const HEIGHT: f32 = 40.0;
/// Bar width (points) below which the controls take two rows.
const WIDE: f32 = 1240.0;

/// Colour-label swatches (UI colours, our own choice).
pub use crate::theme::label_color;

fn filter(app: &mut LightcraftApp, patch: Value) {
    let _ = app.run("library.filter", patch);
}

/// A small label before a group of controls.
fn caption(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(crate::i18n::tr(text)).font(t.font(12.0)).color(t.text_dim));
}

/// A square toggle with an icon or a colour swatch.
fn toggle(ui: &mut egui::Ui, id: &str, on: bool, tip: &str, draw: impl FnOnce(&egui::Painter, Rect, Color32)) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    crate::widgets::announce_selected(&resp, crate::i18n::tr(tip), on);
    register(ui, format!("filter:{id}"), r);
    let p = ui.painter();
    if on {
        p.rect_filled(r, 5.0, t.tool_active);
        p.rect_stroke(r, 5.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 5.0, t.hover);
    }
    draw(
        p,
        r.shrink(5.0),
        if on {
            t.text
        } else if resp.hovered() {
            t.text_label
        } else {
            t.icon
        },
    );
    resp.on_hover_text(crate::i18n::tr(tip))
}

/// A dropdown showing `current`; `items` are (label, filter patch, selected).
fn picker(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, current: &str, active: bool, items: Vec<(String, Value, bool)>) {
    let t = Tokens::get(ui.ctx());
    // camera/lens/keyword and kind labels are passed pre-translated/composed (or are raw metadata values)
    let pretranslated = matches!(id, "camera" | "lens" | "keyword" | "kind");
    let r = crate::widgets::dropdown(
        ui,
        &format!("filter-{id}"),
        if pretranslated { current } else { crate::i18n::tr(current) },
        t.font(12.5),
        if active { t.text } else { t.text_label },
    );
    if active {
        ui.painter().rect_stroke(r.rect.expand(2.0), 4.0, Stroke::new(1.0, t.accent.gamma_multiply(0.7)), StrokeKind::Outside);
    }
    let mut chosen = None;
    egui::Popup::menu(&r).show(|ui| {
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (label, patch, sel) in &items {
                if ui.selectable_label(*sel, if pretranslated { label.as_str() } else { crate::i18n::tr(label) }).clicked() {
                    chosen = Some(patch.clone());
                }
            }
        });
    });
    if let Some(p) = chosen {
        filter(app, p);
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // one row when there is room, else the metadata pickers and actions go to a second row
    let two_rows = ui.available_width() < WIDE;
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), if two_rows { 2.0 * HEIGHT - 8.0 } else { HEIGHT }), Sense::hover());
    ui.painter().rect_filled(bar, 0.0, t.chrome);
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, t.divider));
    register(ui, "filterbar", bar);
    let f = app.session.filter.clone();
    let row = |ui: &mut egui::Ui, i: f32| {
        let r = Rect::from_min_size(bar.min + vec2(16.0, 4.0 + i * (HEIGHT - 8.0)), vec2(bar.width() - 32.0, HEIGHT - 8.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
        c.spacing_mut().item_spacing.x = 4.0;
        c
    };
    let mut row1 = row(ui, 0.0);
    let mut row2 = row(ui, 1.0);
    let ui = &mut row1;

    // rating: operator + stars
    caption(ui, "Rating");
    let op_label = match f.rating_op {
        RatingOp::AtLeast => "≥",
        RatingOp::Exactly => "=",
        RatingOp::AtMost => "≤",
    };
    picker(
        app,
        ui,
        "ratingOp",
        op_label,
        false,
        [("≥  at least", "atLeast", RatingOp::AtLeast), ("=  exactly", "exactly", RatingOp::Exactly), ("≤  at most", "atMost", RatingOp::AtMost)]
            .into_iter()
            .map(|(l, k, op)| (l.to_string(), json!({"ratingOp": k}), f.rating_op == op))
            .collect(),
    );
    if let Some(r) = crate::rating::Rating::new("filter", f.rating).automation_ids("filter:star").show(ui) {
        filter(app, json!({"rating": r}));
    }
    ui.add_space(14.0);

    // flags: several can be on at once (any of them matches), as in Lightroom Classic
    caption(ui, "Flag");
    let chosen: Vec<Flag> = f.flags_shown().unwrap_or_default();
    for (id, icon, flag, tip) in [
        ("pick", Icon::FlagPick, Flag::Pick, "Picked"),
        ("reject", Icon::FlagReject, Flag::Reject, "Rejected"),
        ("none", Icon::Circle, Flag::None, "Unflagged"),
    ] {
        let on = chosen.contains(&flag);
        let color = match flag {
            Flag::Pick if on => t.pick,
            Flag::Reject if on => t.reject,
            _ => Color32::TRANSPARENT,
        };
        let icon = if on && flag == Flag::Pick { Icon::FlagPickFilled } else { icon };
        let resp = toggle(ui, id, on, tip, |p, r, c| paint(p, r, icon, if color == Color32::TRANSPARENT { c } else { color }));
        if resp.clicked() {
            // in the bar's order, so the chip reads the same whichever was clicked first
            let names: Vec<String> = [Flag::Pick, Flag::Reject, Flag::None]
                .into_iter()
                .filter(|x| if *x == flag { !on } else { chosen.contains(x) })
                .map(|x| format!("{x:?}").to_lowercase())
                .collect();
            filter(app, json!({"flag": Value::Null, "flags": names}));
        }
    }
    ui.add_space(14.0);

    // colour labels
    caption(ui, "Label");
    // several labels can be on at once (any of them matches); lit are those `label` and `labels` both let through
    let chosen: Vec<ColorLabel> = f.labels_shown().unwrap_or_default();
    for l in ColorLabel::ALL {
        let on = chosen.contains(&l);
        let name = format!("{l:?}").to_lowercase();
        let tip = crate::i18n::tr_format!("{} label (click more labels to show any of them)", crate::i18n::color_label(&app.session.catalog, l));
        let resp = toggle(ui, &format!("label-{name}"), on, &tip, |p, r, _| {
            p.circle_filled(r.center(), 5.5, label_color(l));
        });
        if resp.clicked() {
            let mut next = chosen.clone();
            if on {
                next.retain(|x| *x != l);
            } else {
                next.push(l);
            }
            next.sort();
            let names: Vec<String> = next.iter().map(|x| format!("{x:?}").to_lowercase()).collect();
            filter(app, json!({"label": Value::Null, "labels": names}));
        }
    }
    ui.add_space(14.0);

    // kind + edited
    let kind_excl = f.not_kind;
    let kind_name = |k: MediaKind| match k {
        MediaKind::Image => "Photos",
        MediaKind::Raw => "Raw",
        MediaKind::Video => "Videos",
    };
    let (kind_label, kind_active) = if let Some(k) = kind_excl {
        (format!("{} {}", crate::i18n::tr("All except"), crate::i18n::tr(kind_name(k))), true)
    } else {
        let l = match f.kind {
            None => "All types",
            Some(MediaKind::Image) => "Photos",
            Some(MediaKind::Raw) => "Raw",
            Some(MediaKind::Video) => "Videos",
        };
        let l = match f.merged.as_deref() {
            Some("hdr") => "HDR",
            Some("panorama") => "Panoramas",
            Some("hdrPanorama") => "HDR Panoramas",
            Some(_) => "Merged",
            None => l,
        };
        (crate::i18n::tr(l).to_string(), f.kind.is_some() || f.merged.is_some())
    };
    let kind_items = {
        let mut items: Vec<(String, Value, bool)> = vec![(
            crate::i18n::tr("All types").to_string(),
            json!({"kind": Value::Null, "notKind": Value::Null, "merged": Value::Null}),
            f.kind.is_none() && kind_excl.is_none() && f.merged.is_none(),
        )];
        for (l, v, k) in [("Photos", "image", MediaKind::Image), ("Raw", "raw", MediaKind::Raw), ("Videos", "video", MediaKind::Video)] {
            items.push((
                crate::i18n::tr(l).to_string(),
                json!({"kind": v, "notKind": Value::Null, "merged": Value::Null}),
                f.kind == Some(k) && kind_excl.is_none() && f.merged.is_none(),
            ));
            items.push((
                format!("{} {}", crate::i18n::tr("All except"), crate::i18n::tr(l)),
                json!({"kind": Value::Null, "notKind": v, "merged": Value::Null}),
                kind_excl == Some(k) && f.merged.is_none(),
            ));
        }
        for (l, m) in [("HDR", "hdr"), ("Panoramas", "panorama"), ("HDR Panoramas", "hdrPanorama")] {
            items.push((
                crate::i18n::tr(l).to_string(),
                json!({"kind": Value::Null, "notKind": Value::Null, "merged": m}),
                f.merged.as_deref() == Some(m),
            ));
        }
        items
    };
    picker(app, ui, "kind", &kind_label, kind_active, kind_items);
    let edited_label = match f.edited {
        None => "Edited or not",
        Some(true) => "Edited",
        Some(false) => "Unedited",
    };
    picker(
        app,
        ui,
        "edited",
        edited_label,
        f.edited.is_some(),
        [("Edited or not", Value::Null, None), ("Edited", json!(true), Some(true)), ("Unedited", json!(false), Some(false))]
            .into_iter()
            .map(|(l, v, e)| (l.to_string(), json!({"edited": v}), f.edited == e))
            .collect(),
    );

    // metadata pickers
    let ui: &mut egui::Ui = if two_rows { &mut row2 } else { &mut row1 };
    let values = app.caches.filter_values(&app.session.catalog);
    let (cameras, lenses, keywords) = (values.cameras.clone(), values.lenses.clone(), values.keywords.clone());
    // (id, include key, exclude key, all-label key, current include, current exclude, values, extra no-keyword)
    let meta_specs: Vec<(&str, &str, &str, &str, Option<String>, Option<String>, Vec<String>, bool)> = vec![
        ("camera", "camera", "notCamera", "Camera", f.camera.clone(), f.not_camera.clone(), cameras, false),
        ("lens", "lens", "notLens", "Lens", f.lens.clone(), f.not_lens.clone(), lenses, false),
        ("keyword", "keyword", "notKeyword", "Keyword", f.keyword.clone(), f.not_keyword.clone(), keywords, true),
    ];
    for (id, key, not_key, all_label, cur_in, cur_ex, vals, has_no_keyword) in meta_specs {
        let no_kw = id == "keyword" && f.no_keyword;
        let any_key = format!("Any {}", all_label.to_lowercase());
        let any_label = crate::i18n::tr(&any_key);
        let mut items: Vec<(String, Value, bool)> = vec![(
            any_label.to_string(),
            if has_no_keyword {
                json!({key: Value::Null, not_key: Value::Null, "noKeyword": false})
            } else {
                json!({key: Value::Null, not_key: Value::Null})
            },
            cur_in.is_none() && cur_ex.is_none() && !no_kw,
        )];
        if has_no_keyword {
            items.push((crate::i18n::tr("No keyword").to_string(), json!({key: Value::Null, not_key: Value::Null, "noKeyword": true}), no_kw));
        }
        for v in vals {
            let sel = cur_in.as_deref() == Some(v.as_str());
            items.push((
                v.clone(),
                if has_no_keyword {
                    json!({key: v.clone(), not_key: Value::Null, "noKeyword": false})
                } else {
                    json!({key: v.clone(), not_key: Value::Null})
                },
                sel,
            ));
        }
        let (shown, active, neg_value) = if no_kw {
            (crate::i18n::tr("No keyword").to_string(), true, None)
        } else if let Some(ref v) = cur_ex {
            (format!("{} {v}", crate::i18n::tr("not")), true, Some(v.clone()))
        } else if let Some(ref v) = cur_in {
            (v.clone(), true, Some(v.clone()))
        } else {
            (crate::i18n::tr(all_label).to_string(), false, None)
        };
        let shown = if shown.chars().count() > 22 { format!("{}…", shown.chars().take(21).collect::<String>()) } else { shown };
        picker(app, ui, id, &shown, active, items);
        // "−" toggle next to a selected value: include ↔ exclude ("tous sauf …")
        if let Some(v) = neg_value.filter(|_| !no_kw) {
            let is_ex = cur_ex.is_some();
            let tip = if is_ex { crate::i18n::tr("Include this value") } else { crate::i18n::tr("Exclude this value") };
            let resp = ui.add(egui::Button::new(egui::RichText::new(if is_ex { "−" } else { "≠" }).font(t.font(13.0))).small());
            register(ui, format!("filter:{id}-not"), resp.rect);
            if resp.on_hover_text(tip).clicked() {
                if is_ex {
                    filter(
                        app,
                        if has_no_keyword {
                            json!({key: v, not_key: Value::Null, "noKeyword": false})
                        } else {
                            json!({key: v, not_key: Value::Null})
                        },
                    );
                } else {
                    filter(
                        app,
                        if has_no_keyword {
                            json!({key: Value::Null, not_key: v, "noKeyword": false})
                        } else {
                            json!({key: Value::Null, not_key: v})
                        },
                    );
                }
            }
        }
    }

    // the photos still in no album, to sort them out
    let na = ui.selectable_label(f.no_album, egui::RichText::new(crate::i18n::tr("In no album")).font(t.font(12.5)));
    register(ui, "button:noAlbum", na.rect);
    if na.clicked() {
        filter(app, json!({"noAlbum": !f.no_album}));
    }

    // faces marked as passers-by, to check them or take the mark off
    let strangers = f.faces.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("strangers"));
    let st = ui.selectable_label(strangers, egui::RichText::new(crate::i18n::tr("Has strangers")).font(t.font(12.5)));
    register(ui, "button:strangers", st.rect);
    if st.clicked() {
        filter(app, json!({"faces": if strangers { Value::Null } else { json!("strangers") }}));
    }

    // save / clear
    ui.add_space(14.0);
    let filtering = app.session.filter != Default::default();
    let save = ui.add_enabled(filtering, egui::Button::new(egui::RichText::new(crate::i18n::tr("Save as Smart Album…")).font(t.font(12.5))));
    register(ui, "button:filterSave", save.rect);
    if save.clicked() {
        let _ = app.run("dialog.newSmartAlbum", json!({}));
    }
    let clear = ui.add_enabled(filtering, egui::Button::new(egui::RichText::new(crate::i18n::tr("Clear")).font(t.font(12.5))));
    register(ui, "button:filterClear", clear.rect);
    if clear.clicked() {
        app.ui.search.clear();
        let _ = app.run("library.clearFilter", json!({}));
    }
    // picks the taste model proposes among the photos in view (suggestions only: nothing is rated).
    // It opens Sorting Help at that helper: what it has been measured to be worth comes first.
    let sp = ui.button(egui::RichText::new(crate::i18n::tr("Suggested picks")).font(t.font(12.5)));
    register(ui, "button:suggestedPicks", sp.rect);
    let sp = sp.on_hover_text(crate::i18n::tr(
        "Shows the unrated photos your taste model likes best (learned from your stars: taste.train). Nothing is rated.",
    ));
    if sp.clicked() {
        let _ = app.run("dialog.sortingHelp", json!({"section": "picks"}));
    }
    // filter presets: apply one, save the current filter, delete
    let presets = crate::widgets::dropdown(ui, "filterPresets", crate::i18n::tr("Presets"), t.font(12.5), t.text);
    register(ui, "button:filterPresets", presets.rect);
    egui::Popup::menu(&presets).show(|ui| {
        let names: Vec<String> = app.session.filter_presets.iter().map(|f| f.name.clone()).collect();
        for n in &names {
            let r = ui.button(n);
            if r.clicked() {
                let _ = app.run("filter.applyPreset", json!({"name": n}));
            }
            crate::widgets::context_menu(&r, |ui| {
                if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
                    let _ = app.run("filter.deletePreset", json!({"name": n}));
                }
            });
        }
        if names.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No filter presets yet")).color(t.text_dim));
        }
        ui.separator();
        if ui.add_enabled(filtering, egui::Button::new(crate::i18n::tr("Save Current Filter as Preset…"))).clicked() {
            crate::panels::dialogs::prompt(app, "Save Filter Preset", "Preset name", "", "filter.savePreset", json!({}), "name");
        }
    });
}
