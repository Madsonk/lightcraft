//! Photo ▸ Sorting Help: the three helpers that suggest which photos to keep and how to label
//! them, in one window instead of three places — **Assisted Culling** (focus, exposure, bursts),
//! **Suggested picks** (the taste model's shortlist) and **Suggested keywords** (the search
//! model). Each keeps its own name and its own command; this window only gathers them, says how
//! well each one has been measured to work, and never writes a rating, a flag or a keyword unless
//! the user clicks the button that does.
//!
//! The filter bar's "Suggested picks" and the Keywords panel's "Suggest" open it at their section
//! (`dialog.cull {section}`), so no helper has two implementations.

use std::collections::BTreeMap;

use egui::RichText;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// The window's title, the menu entry's label and the name every entry point uses.
pub const TITLE: &str = "Sorting Help";

/// Where the window opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Culling,
    Picks,
    Keywords,
}

impl Section {
    pub fn parse(s: &str) -> Option<Section> {
        match s {
            "culling" => Some(Section::Culling),
            "picks" => Some(Section::Picks),
            "keywords" => Some(Section::Keywords),
            _ => None,
        }
    }
}

/// The last answers, kept while the window is closed so a background run can finish in peace.
#[derive(Default)]
pub struct State {
    /// The section to scroll to, once, when the window shows.
    pub goto: Option<Section>,
    /// `taste.status` when the window opened.
    pub taste: Option<Value>,
    /// `culling.suggest`'s answer for the last "Show Suggested Picks" (or its error).
    pub picks: Option<Result<Value, String>>,
    pub culling: String,
    pub kw: Option<Kw>,
}

/// Suggested keywords for a set of photos.
pub struct Kw {
    pub ids: Vec<u64>,
    /// Per photo, what is still on offer.
    pub photos: Vec<(u64, Vec<Sugg>)>,
    pub unindexed: u64,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Sugg {
    pub keyword: String,
    pub score: f64,
}

impl State {
    /// What `keywords.suggest` answered for `ids`.
    pub fn set_keywords(&mut self, ids: &[u64], v: &Value) {
        let photos = v["photos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let list = p["suggestions"]
                    .as_array()?
                    .iter()
                    .filter_map(|x| Some(Sugg { keyword: x["keyword"].as_str()?.to_string(), score: x["score"].as_f64()? }))
                    .collect();
                Some((p["id"].as_u64()?, list))
            })
            .collect();
        self.kw = Some(Kw { ids: ids.to_vec(), photos, unindexed: v["unindexed"].as_u64().unwrap_or(0), error: None });
        // the answer is below the Suggest button: bring that section into view when it is drawn
        self.goto = Some(Section::Keywords);
    }

    pub fn set_keyword_error(&mut self, ids: &[u64], e: &str) {
        self.kw = Some(Kw { ids: ids.to_vec(), photos: Vec::new(), unindexed: 0, error: Some(e.to_string()) });
        self.goto = Some(Section::Keywords);
    }
}

/// Open the window (Photo ▸ Sorting Help, or an entry point that goes straight to a section).
pub fn open(app: &mut LightcraftApp, section: Section) {
    app.sorting.goto = Some(section);
    app.sorting.taste = app.session.execute("taste.status", &json!({})).ok();
    if !matches!(app.ui.dialog, Some(crate::state::Dialog::SortingHelp { .. })) {
        app.ui.dialog = Some(crate::state::Dialog::SortingHelp { reject_below: 0.0, pick_best: true });
    }
}

/// What was measured on the author's archive (infra-artcraft § 5quaterdecies), said before the
/// picks are offered.
pub const PICKS_WARNING: &str = "Read this first. Measured on a real archive of 1,055 rated photos (5-fold cross-validation): on a shoot the model has never seen it has no usable skill: AUC 0.56, and its top picks are right 33 % of the time against 31 % by pure chance. Inside a shoot you have already partly rated it does better (AUC 0.77), but that figure is inflated by near-identical photos. Use the picks as a shortlist to look at, never as a decision.";

pub const KEYWORDS_WARNING: &str = "Measured on 144 random photos by one labeller: at the default setting about 6 suggestions in 10 are right (precision 0.58) and 4 in 10 of the right keywords are found (recall 0.39). Generic keywords (Scene|Interior…) are often missed and Mood|… is unreliable. People are never named here: that is People › Recognition.";

fn heading(ui: &mut egui::Ui, t: &Tokens, id: &str, text: &str, goto: &mut Option<Section>, which: Section) {
    ui.add_space(6.0);
    let r = ui.label(RichText::new(crate::i18n::tr(text)).font(t.semibold(14.0)).color(t.text));
    register(ui.ctx(), format!("sorting:{id}"), r.rect);
    if *goto == Some(which) {
        ui.scroll_to_rect(r.rect.expand2(egui::vec2(0.0, 6.0)), Some(egui::Align::TOP));
        *goto = None;
    }
}

fn warning(ui: &mut egui::Ui, t: &Tokens, id: &str, text: &str) {
    let r = egui::Frame::new().fill(t.inset).stroke(egui::Stroke::new(1.0, t.caution)).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(crate::i18n::tr(text)).size(12.0).color(t.text_label)).wrap());
    });
    register(ui.ctx(), format!("sorting:{id}"), r.response.rect);
}

fn note(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(t.text_dim)).wrap());
}

/// The window's body. Returns `true` to close it (after the picks are shown in the grid).
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, reject_below: &mut f32, pick_best: &mut bool) -> bool {
    let mut close = false;
    ui.set_min_width(580.0);
    ui.add(
        egui::Label::new(
            RichText::new(crate::i18n::tr(
                "Three helpers that suggest which photos to keep and how to label them. They only suggest: nothing is rated, flagged or keyworded until you say so.",
            ))
            .color(t.text_label),
        )
        .wrap(),
    );
    let mut goto = app.sorting.goto.take();
    egui::ScrollArea::vertical().id_salt("sorting-help-scroll").max_height(520.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        culling(app, ui, t, reject_below, pick_best, &mut goto);
        ui.separator();
        close |= picks(app, ui, t, &mut goto);
        ui.separator();
        keywords(app, ui, t, &mut goto);
    });
    // (a section asked for that was not drawn yet is kept for the next frame)
    if goto.is_some() {
        app.sorting.goto = goto;
    }
    close
}

fn culling(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, reject_below: &mut f32, pick_best: &mut bool, goto: &mut Option<Section>) {
    heading(ui, t, "culling", "Assisted Culling", goto, Section::Culling);
    let n = app.session.selection.ids.len();
    let scope = if n > 1 {
        crate::i18n::tr_format!("the {n} selected photos", n = n)
    } else {
        crate::i18n::tr_format!("the {} photos in view", app.session.visible_cloned().len())
    };
    ui.add(
        egui::Label::new(
            RichText::new(crate::i18n::tr_format!(
                "Scores {scope} for focus and exposure and finds similar shots taken within seconds of each other (bursts).",
                scope = scope
            ))
            .color(t.text_label),
        )
        .wrap(),
    );
    let r = ui.add(egui::Slider::new(reject_below, 0.0..=80.0).text(crate::i18n::tr("Reject below focus")).step_by(1.0));
    register(ui.ctx(), "field:cullReject", r.rect);
    note(
        ui,
        t,
        crate::i18n::tr(if *reject_below > 0.0 { "Blurry photos below the score are flagged as rejects." } else { "0: nothing is rejected." }),
    );
    let r = ui.checkbox(pick_best, crate::i18n::tr("Pick the sharpest photo of each burst"));
    register(ui.ctx(), "check:cullPick", r.rect);
    note(ui, t, crate::i18n::tr("Scores stay on the photos: filter or make smart albums with Focus and Best of Similar Shots."));
    note(
        ui,
        t,
        "It measures technique only (focus, clipping, bursts): it knows nothing about your taste, and flags and rejects it sets are one undo step.",
    );
    // sliced and cancellable (the Quick sort analysis): a big view must not freeze the window
    let running = app.workspaces.quick.job.is_some();
    ui.horizontal_wrapped(|ui| {
        let preview = ui.add_enabled(!running, egui::Button::new(crate::i18n::tr("Preview Without Applying")));
        register(ui.ctx(), "button:sortingCullPreview", preview.rect);
        if preview.clicked() {
            let mut p = json!({"pickBest": *pick_best, "dryRun": true});
            if *reject_below > 0.0 {
                p["rejectBelow"] = json!(*reject_below);
            }
            let ids = if app.session.selection.ids.len() > 1 {
                app.session.selection.ids.iter().map(|i| i.0).collect()
            } else {
                app.session.visible_cloned().iter().map(|i| i.0).collect()
            };
            app.sorting.culling = match crate::quick_sort::analyze_with(app, ids, p) {
                Ok(()) => String::new(),
                Err(e) => e,
            };
        }
        let b = ui.add_enabled(!running, egui::Button::new(crate::i18n::tr("Analyze Photos")));
        register(ui.ctx(), "button:sortingCull", b.rect);
        if b.clicked() {
            let mut p = json!({"pickBest": *pick_best});
            if *reject_below > 0.0 {
                p["rejectBelow"] = json!(*reject_below);
            }
            let ids: Vec<u64> = if app.session.selection.ids.len() > 1 {
                app.session.selection.ids.iter().map(|i| i.0).collect()
            } else {
                app.session.visible_cloned().iter().map(|i| i.0).collect()
            };
            app.sorting.culling = match crate::quick_sort::analyze_with(app, ids, p) {
                Ok(()) => String::new(),
                Err(e) => e,
            };
        }
        if let Some(r) = crate::quick_sort::progress(app) {
            ui.add(egui::ProgressBar::new(r.done as f32 / r.total.max(1) as f32).desired_width(140.0));
            ui.label(RichText::new(format!("{} of {} photos", r.done, r.total)).size(12.0).color(t.text_label));
            if text_button(ui, "sortingCullCancel", "Cancel", false).clicked() {
                crate::quick_sort::cancel(app);
            }
            ui.ctx().request_repaint();
        } else if let Some(l) = app.workspaces.quick.last.clone().filter(|_| app.sorting.culling.is_empty()) {
            if l["dryRun"] == true {
                ui.label(
                    RichText::new(crate::i18n::tr_format!(
                        "Preview: {reject} proposed rejects · {pick} proposed picks · no changes",
                        reject = l["wouldReject"],
                        pick = l["wouldPick"]
                    ))
                    .size(12.0)
                    .color(t.text_label),
                );
                if let Some(error) = l["error"].as_str() {
                    ui.label(RichText::new(error).size(12.0).color(t.caution));
                }
                if l["cancelled"] == true {
                    ui.label(crate::i18n::tr("Preview stopped: results are partial."));
                }
            } else {
                ui.label(
                    RichText::new(format!(
                        "{} photos scored · {} rejected · {} picked{}",
                        l["photos"],
                        l["rejected"],
                        l["picked"],
                        if l["cancelled"] == true { " · stopped" } else { "" }
                    ))
                    .size(12.0)
                    .color(t.text_label),
                );
            }
        } else if !app.sorting.culling.is_empty() {
            ui.label(RichText::new(&app.sorting.culling).size(12.0).color(t.text_label));
        }
    });
    if let Some(last) = &app.workspaces.quick.last
        && last["dryRun"] == true
        && let Some(rows) = last["proposals"].as_array()
    {
        ui.collapsing(crate::i18n::tr("Review suggestions (up to 200 photos)"), |ui| {
            egui::ScrollArea::vertical().id_salt("culling-proposals").max_height(180.0).show(ui, |ui| {
                for row in rows {
                    let id = row["id"].as_u64().unwrap_or(0);
                    let name = app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).map(|p| p.file_name.as_str()).unwrap_or("?");
                    let reasons: Vec<&str> = row["reasons"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|reason| {
                            crate::i18n::tr(match reason {
                                "focus_below_threshold" => "Focus below threshold",
                                "closed_eyes" => "Closed eyes",
                                "best_in_burst" => "Best in burst",
                                "eyes_not_evaluated" => "Eyes not evaluated",
                                _ => "Review",
                            })
                        })
                        .collect();
                    let flag = crate::i18n::tr(match row["suggestedFlag"].as_str() {
                        Some("reject") => "Reject",
                        Some("pick") => "Pick",
                        _ => "Review",
                    });
                    ui.label(format!("{name} · {flag} · {}", reasons.join(" · ")));
                }
            });
        });
    }
}

fn picks(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, goto: &mut Option<Section>) -> bool {
    let mut close = false;
    heading(ui, t, "picks", "Suggested picks", goto, Section::Picks);
    note(ui, t, crate::i18n::tr("Shows the unrated photos your taste model likes best (learned from your stars: taste.train). Nothing is rated."));
    warning(ui, t, "picksWarning", PICKS_WARNING);
    if let Some(taste) = &app.sorting.taste {
        let (trained, rated) = (taste["trained"].as_bool().unwrap_or(false), taste["ratedPhotos"].as_u64().unwrap_or(0));
        let line = if trained {
            format!("Your model is trained ({rated} rated photos).")
        } else {
            format!("No model is trained yet ({rated} rated photos): train it before asking for picks.")
        };
        note(ui, t, &line);
    }
    ui.horizontal(|ui| {
        if text_button(ui, "sortingPicksShow", "Show Suggested Picks", false).clicked() {
            let r = app.run("culling.suggest", json!({"filter": true}));
            close = r.is_ok();
            if let Ok(v) = &r {
                let n = v["picks"].as_array().map_or(0, Vec::len);
                app.ui.status = format!("{n} suggested picks");
            }
            app.sorting.picks = Some(r);
        }
    });
    match &app.sorting.picks {
        Some(Ok(v)) => {
            let m = &v["model"];
            let num = |x: &Value| x.as_f64().map_or("?".to_string(), |x| format!("{x:.2}"));
            note(
                ui,
                t,
                &format!(
                    "{} picks among {} candidates. Top-pick hit rate of your model on folders it had not seen: {} (chance {}).",
                    v["picks"].as_array().map_or(0, Vec::len),
                    v["candidates"].as_u64().unwrap_or(0),
                    num(&m["topKHitRate"]),
                    num(&m["topKChance"])
                ),
            );
        }
        Some(Err(e)) => note(ui, t, e),
        None => {}
    }
    close
}

fn keywords(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, goto: &mut Option<Section>) {
    heading(ui, t, "keywords", "Suggested keywords", goto, Section::Keywords);
    note(
        ui,
        t,
        crate::i18n::tr(
            "Looks at the photos with the search model (the first use loads it: about a minute). Nothing is added until you click a keyword.",
        ),
    );
    warning(ui, t, "keywordsWarning", KEYWORDS_WARNING);
    let auto_id = egui::Id::new("kw-auto-accept");
    let mut auto: bool = match ui.data(|d| d.get_temp::<bool>(auto_id)) {
        Some(b) => b,
        None => {
            let b = app.run("keywords.settings", json!({})).ok().and_then(|v| v["autoAccept"].as_bool()).unwrap_or(false);
            ui.data_mut(|d| d.insert_temp(auto_id, b));
            b
        }
    };
    let changed = ui
        .checkbox(&mut auto, crate::i18n::tr("Add suggestions above 90 % automatically"))
        .on_hover_text(crate::i18n::tr("Off by default. When on, Suggest also adds the keywords at or above the threshold."))
        .changed();
    if changed && app.run("keywords.settings", json!({"autoAccept": auto})).is_ok() {
        ui.data_mut(|d| d.insert_temp(auto_id, auto));
    }
    let n = if app.session.selection.ids.len() > 1 { app.session.selection.ids.len() } else { usize::from(app.session.active().is_some()) };
    let running = app.tasks.is_running(crate::heavy::SUGGEST);
    ui.horizontal(|ui| {
        let scope = match n {
            0 => "no photo selected".to_string(),
            1 => "for this photo".to_string(),
            n => format!("for the {n} selected photos"),
        };
        let go = ui.add_enabled(n > 0 && !running, egui::Button::new(RichText::new(crate::i18n::tr("Suggest")).font(t.semibold(11.5))));
        register(ui.ctx(), "button:sortingKwSuggest", go.rect);
        if go.clicked() {
            let r = crate::heavy::suggest(app, &json!({"includeExisting": false, "background": true}));
            if let Err(e) = r {
                let ids: Vec<u64> = app.session.targets(&json!({})).unwrap_or_default().iter().map(|i| i.0).collect();
                app.sorting.set_keyword_error(&ids, &e);
            }
        }
        ui.label(RichText::new(scope).size(12.0).color(t.text_dim));
        if running {
            ui.spinner();
            if text_button(ui, "sortingKwCancel", "Cancel", false)
                .on_hover_text(crate::i18n::tr("The model cannot be interrupted while it loads: Cancel drops the answer when it comes."))
                .clicked()
            {
                app.tasks.cancel(crate::heavy::SUGGEST);
            }
        }
    });
    let Some(kw) = &app.sorting.kw else { return };
    if let Some(e) = &kw.error {
        note(ui, t, e);
        return;
    }
    // one row per keyword over the photos it was suggested for
    let mut agg: BTreeMap<&str, (usize, f64)> = BTreeMap::new();
    for (_, list) in &kw.photos {
        for s in list {
            let e = agg.entry(s.keyword.as_str()).or_insert((0, 0.0));
            e.0 += 1;
            e.1 = e.1.max(s.score);
        }
    }
    if agg.is_empty() {
        let msg = if kw.unindexed > 0 {
            crate::i18n::tr("This photo is not indexed yet: run Search ▸ Index Photos.")
        } else {
            crate::i18n::tr("No keyword reached the threshold.")
        };
        note(ui, t, msg);
        return;
    }
    let mut rows: Vec<(&str, usize, f64)> = agg.into_iter().map(|(k, (n, s))| (k, n, s)).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)));
    let single = kw.photos.len() == 1;
    let mut accept: Vec<String> = Vec::new();
    ui.horizontal_wrapped(|ui| {
        for (k, count, score) in rows.iter().take(60) {
            let label = if single {
                format!("+ {}  {:.0}%", k.replace('|', " › "), score * 100.0)
            } else {
                format!("+ {}  ·  {count}  ·  {:.0}%", k.replace('|', " › "), score * 100.0)
            };
            let r = ui.add(egui::Button::new(RichText::new(label).color(t.text_label)).corner_radius(10.0));
            register(ui.ctx(), format!("kwSuggested:{k}"), r.rect);
            let r = if single { r } else { r.on_hover_text(format!("{count} photos: the best score shown")) };
            if r.clicked() {
                accept.push((*k).to_string());
            }
        }
    });
    let all = text_button(ui, "sortingKwAll", "Add all", false).clicked();
    if all {
        accept = rows.iter().map(|r| r.0.to_string()).collect();
    }
    if accept.is_empty() {
        return;
    }
    let items: Vec<Value> = kw
        .photos
        .iter()
        .flat_map(|(id, list)| list.iter().filter(|s| accept.contains(&s.keyword)).map(move |s| json!({"id": id, "keyword": s.keyword})))
        .collect();
    match app.run("keywords.accept", json!({"items": items})) {
        Ok(v) => {
            let n = v["changed"].as_u64().unwrap_or(0);
            app.ui.status = format!("{n} keyword{} added", if n == 1 { "" } else { "s" });
            if let Some(kw) = &mut app.sorting.kw {
                for (_, list) in &mut kw.photos {
                    list.retain(|s| !accept.contains(&s.keyword));
                }
            }
        }
        Err(e) => app.ui.status = e,
    }
}
