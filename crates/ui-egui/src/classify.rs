//! Library ▸ Classify: the bench for an archive (LightCraft v12, phase 4 of the UX study). One page for
//! the work that never ends on a library of tens of thousands of photos: how healthy the archive is
//! (photos with no keyword, no place, no person, no rating, the same file twice — counters that open
//! the photos they count), the queue of "same person?" questions answered on the keyboard, the groups
//! of faces sorted by size and named in a field (type, Enter), the places and the keywords LightCraft
//! suggests, confirmed in batches, and a preview of a batch rename.
//!
//! It adds no analysis of its own: People (`people.questions`, `people.confirm`, `people.name`,
//! `people.unknown`, `people.skip`, `people.merge`, `people.move`, `people.ignore`), Places
//! (`places.geotag`, `places.suggest`, `places.confirm`), the keyword suggestions (`keywords.suggest`,
//! `keywords.accept`) and `photo.renamePreview` are the existing commands. **Everything is a
//! suggestion**: a question, a place or a keyword writes nothing until the user answers it, confirms it
//! or presses the button that applies it; the heavy runs go through the existing background tasks.

use std::collections::{BTreeMap, HashMap};

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use lightcraft_catalog::{Catalog, PhotoId};
use lightcraft_engine::people::Thumb;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};
use crate::workspaces::{self, Step};

const LEFT_W: f32 = 270.0;
const RIGHT_W: f32 = 330.0;
/// Questions fetched at a time (the queue refills below [`REFILL`]).
const FETCH: usize = 40;
const REFILL: usize = 4;
/// Groups shown (the biggest first; naming one brings the next).
const GROUPS_SHOWN: usize = 12;
/// Photos asked for keyword suggestions at a time (the model is slow).
pub const KEYWORD_BATCH: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    People,
    Places,
    Keywords,
    Duplicates,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::People, Tab::Places, Tab::Keywords, Tab::Duplicates];

    pub fn name(self) -> &'static str {
        match self {
            Tab::People => "people",
            Tab::Places => "places",
            Tab::Keywords => "keywords",
            Tab::Duplicates => "duplicates",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::People => "People",
            Tab::Places => "Places",
            Tab::Keywords => "Keywords",
            Tab::Duplicates => "Same file",
        }
    }

    pub fn parse(s: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(s.trim()))
    }
}

// ---------------------------------------------------------------------------------------------- health

/// What is missing from the library, counted directly from the photos (the photos of the library: not
/// deleted, not only browsed). Each count is exactly what its counter opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub total: usize,
    pub no_keyword: usize,
    pub no_place: usize,
    pub no_person: usize,
    pub no_rating: usize,
    /// Photos whose file content is the same as another photo's.
    pub duplicates: usize,
    /// Of those, how many sets of identical files.
    pub duplicate_sets: usize,
    /// Photos with a person or a place, and a keyword.
    pub classified: usize,
}

/// A photo has a person when a `People` keyword is on it or a face region carries a name.
fn has_person(p: &lightcraft_catalog::Photo) -> bool {
    p.meta.keywords.iter().any(|k| lightcraft_catalog::keywords::is_under(k, "People"))
        || p.meta.regions.iter().any(|r| r.kind == lightcraft_meta::RegionKind::Face && r.name.as_deref().is_some_and(|n| !n.trim().is_empty()))
}

fn has_place(p: &lightcraft_catalog::Photo) -> bool {
    !lightcraft_catalog::places::matches(p, lightcraft_catalog::places::NONE)
}

/// Count the health of `cat` (one pass over the photos).
pub fn count_health(cat: &Catalog) -> Health {
    let mut h = Health::default();
    let mut by_hash: HashMap<&str, usize> = HashMap::new();
    for p in cat.photos().filter(|p| p.in_library()) {
        h.total += 1;
        let (kw, place, person) = (!p.meta.keywords.is_empty(), has_place(p), has_person(p));
        h.no_keyword += usize::from(!kw);
        h.no_place += usize::from(!place);
        h.no_person += usize::from(!person);
        h.no_rating += usize::from(p.rating == 0);
        h.classified += usize::from(kw && (place || person));
        if let Some(c) = p.content_hash.as_deref() {
            *by_hash.entry(c).or_default() += 1;
        }
    }
    for n in by_hash.values().filter(|n| **n > 1) {
        h.duplicates += n;
        h.duplicate_sets += 1;
    }
    h
}

/// The photos a counter opens, by name (`noKeyword`, `noPlace`, `noPerson`, `noRating`, `duplicates`).
pub fn ids_for(cat: &Catalog, what: &str) -> Option<Vec<u64>> {
    let all = cat.photos().filter(|p| p.in_library());
    Some(match what {
        "noKeyword" => all.filter(|p| p.meta.keywords.is_empty()).map(|p| p.id.0).collect(),
        "noPlace" => all.filter(|p| !has_place(p)).map(|p| p.id.0).collect(),
        "noPerson" => all.filter(|p| !has_person(p)).map(|p| p.id.0).collect(),
        "noRating" => all.filter(|p| p.rating == 0).map(|p| p.id.0).collect(),
        "duplicates" => {
            let mut by_hash: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
            for p in all {
                if let Some(c) = p.content_hash.as_deref() {
                    by_hash.entry(c).or_default().push(p.id.0);
                }
            }
            by_hash.into_values().filter(|v| v.len() > 1).flatten().collect()
        }
        _ => return None,
    })
}

// ----------------------------------------------------------------------------------------------- state

#[derive(Default)]
pub struct State {
    pub tab: Tab,
    health: Option<((u64, u64), Health)>,
    /// The "same person?" questions at the head of the queue.
    pub queue: Vec<Value>,
    pub queue_total: usize,
    queue_dirty: bool,
    queue_checked: bool,
    retry_at: f64,
    retry_ov_at: f64,
    /// Questions answered this session: the engine lists them until the faces are grouped again.
    answered_ids: std::collections::HashSet<String>,
    /// Answers given this session: (yes, no, unsure), and when each was given (ms).
    pub answers: [usize; 3],
    pub answer_times: Vec<f64>,
    pub last_error: Option<String>,
    ov: Option<(u64, lightcraft_engine::people::Overview)>,
    people_status: Value,
    people_status_at: f64,
    /// Text typed in each group's name field, and the field to focus next.
    names: HashMap<String, String>,
    focus_first: bool,
    /// The group shown on the right (`g:N`) and its faces.
    pub open_group: Option<String>,
    members: Option<(String, Value)>,
    picked_faces: Vec<String>,
    places: Option<(&'static str, Value)>,
    pub rename_template: String,
    rename_preview: Option<Value>,
    /// What the last batch operation did (`ui.inspect`).
    pub last: Option<Value>,
}

impl State {
    /// Groups in the overview (tests).
    pub fn ov_len(&self) -> usize {
        self.ov.as_ref().map_or(0, |(_, o)| o.groups.len())
    }

    /// The biggest group to name (tests).
    pub fn first_group_id(&self) -> Option<String> {
        self.ov.as_ref().and_then(|(_, o)| o.groups.first().map(|g| g.id.clone()))
    }

    fn dirty(&mut self) {
        self.queue_dirty = true;
        self.ov = None;
        self.members = None;
    }
}

fn health(app: &mut LightcraftApp) -> Health {
    let key = (app.session.catalog.revision, app.session.library_generation());
    if let Some((k, h)) = &app.workspaces.classify.health
        && *k == key
    {
        return h.clone();
    }
    let h = count_health(&app.session.catalog);
    app.workspaces.classify.health = Some((key, h.clone()));
    h
}

pub fn visible(app: &LightcraftApp) -> bool {
    workspaces::step_of(app) == Some(Step::Classify)
}

// ------------------------------------------------------------------------------------------ questions

fn refill(app: &mut LightcraftApp, now: f64) {
    let c = &mut app.workspaces.classify;
    // (the queue is read once, then again only when an answer changed what is doubtful or it runs low)
    if (c.queue_checked && !c.queue_dirty) || now < c.retry_at {
        return;
    }
    match app.session.execute("people.questions", &json!({"limit": FETCH})) {
        // the faces are grouped in the background: ask again in a moment
        Ok(v) if v.get("grouped").and_then(Value::as_bool) == Some(false) => {
            app.workspaces.classify.retry_at = now + 0.4;
        }
        Ok(v) => {
            let c = &mut app.workspaces.classify;
            c.queue_checked = true;
            c.queue_dirty = false;
            let mut q: Vec<Value> = v["questions"].as_array().cloned().unwrap_or_default();
            let before = q.len();
            q.retain(|x| x["id"].as_str().is_none_or(|id| !c.answered_ids.contains(id)));
            let skipped = before - q.len();
            c.queue = q;
            c.queue_total = v["total"].as_u64().map(|t| t as usize).unwrap_or(before).saturating_sub(skipped);
            c.last_error = None;
        }
        Err(e) => {
            let c = &mut app.workspaces.classify;
            c.queue_checked = true;
            c.queue_dirty = false;
            c.queue.clear();
            c.queue_total = 0;
            c.last_error = Some(e.to_string());
        }
    }
}

/// Answer the question at the head of the queue: `yes`, `no` or `unsure` (`people.confirm`).
pub fn answer(app: &mut LightcraftApp, ans: &str) -> Result<Value, String> {
    let q = app.workspaces.classify.queue.first().cloned().ok_or("no question to answer")?;
    let id = q["id"].as_str().ok_or("the question has no id")?.to_string();
    let r = app.run("people.confirm", json!({"question": id, "answer": ans}));
    let c = &mut app.workspaces.classify;
    // a question the engine refuses (a group it has merged since) is dropped, so the queue goes on
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            c.queue.remove(0);
            c.queue_total = c.queue_total.saturating_sub(1);
            c.answered_ids.insert(id);
            return Err(e);
        }
    };
    c.queue.remove(0);
    c.answered_ids.insert(id);
    c.queue_total = c.queue_total.saturating_sub(1);
    c.answers[match ans {
        "yes" => 0,
        "no" => 1,
        _ => 2,
    }] += 1;
    c.answer_times.push(crate::now_ms());
    // the queue is read again only when it runs low: the engine regroups the faces in the background after a
    // decision and answers nothing until it is done, so a long buffer keeps the keys going meanwhile
    if c.queue.len() < REFILL {
        c.dirty();
    }
    Ok(r)
}

/// The keys of the queue, while the People tab is in view and no text field has the keyboard: `Y` yes,
/// `N` no, `U` not sure. Returns whether a key was used (the global shortcuts then skip the frame:
/// `U` is also "unflag").
pub fn handle_keys(app: &mut LightcraftApp, ctx: &egui::Context) -> bool {
    if !visible(app) || app.workspaces.classify.tab != Tab::People || app.workspaces.classify.queue.is_empty() {
        return false;
    }
    let pressed = |k: egui::Key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    let ans = if pressed(egui::Key::Y) {
        "yes"
    } else if pressed(egui::Key::N) {
        "no"
    } else if pressed(egui::Key::U) {
        "unsure"
    } else {
        return false;
    };
    if let Err(e) = answer(app, ans) {
        app.ui.status = e;
    }
    true
}

// ------------------------------------------------------------------------------------------ commands

fn go_sort(app: &mut LightcraftApp) -> Result<Value, String> {
    workspaces::set_step(app, Step::Sort)
}

/// `workspace.classify.*`.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    let sv = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    match id {
        "workspace.classify.tab" => {
            let t = Tab::parse(sv("tab").ok_or("missing `tab` (people | places | keywords | duplicates)")?).ok_or("unknown tab")?;
            app.workspaces.classify.tab = t;
            workspaces::set_step(app, Step::Classify)
        }
        // open the photos a counter counts, on the grid (the source is All Photos, the filter is the counter's)
        "workspace.classify.filter" => {
            let what = sv("what").ok_or("missing `what` (noKeyword | noPlace | noPerson | noRating | duplicates)")?;
            let patch = match what {
                "noKeyword" => json!({"untagged": true}),
                "noPlace" => json!({"place": lightcraft_catalog::places::NONE}),
                "noPerson" | "noRating" | "duplicates" => {
                    let ids = ids_for(&app.session.catalog, what).unwrap_or_default();
                    if ids.is_empty() {
                        return Err(format!("no photo to show ({what})"));
                    }
                    json!({"only": ids})
                }
                other => return Err(format!("unknown counter `{other}`")),
            };
            app.run("library.source", json!({"kind": "all"}))?;
            app.run("library.clearFilter", json!({}))?;
            app.run("library.filter", patch)?;
            go_sort(app)?;
            Ok(json!({"count": app.session.visible().len()}))
        }
        "workspace.classify.answer" => answer(app, sv("answer").ok_or("missing `answer` (yes | no | unsure)")?),
        // name a group (people.name) — typed in the group's field in the page
        "workspace.classify.name" => {
            let g = sv("group").ok_or("missing `group` (g:N)")?;
            let name = sv("name").ok_or("missing `name`")?;
            let r = app.run("people.name", json!({"group": g, "name": name}))?;
            app.workspaces.classify.names.remove(g);
            app.workspaces.classify.dirty();
            Ok(r)
        }
        "workspace.classify.refresh" => {
            app.workspaces.classify.dirty();
            app.workspaces.classify.queue_checked = false;
            Ok(Value::Null)
        }
        // places, in two steps: look (nothing written), then apply
        "workspace.classify.places" => {
            let action = sv("action").ok_or("missing `action` (preview | apply | suggest | confirm)")?;
            let r = match action {
                "preview" => app.run("places.geotag", json!({"limit": 8}))?,
                "apply" => app.run("places.geotag", json!({"apply": true, "limit": 0}))?,
                "suggest" => app.run("places.suggest", json!({"methods": ["time"]}))?,
                "confirm" => {
                    let ids: Vec<u64> = app
                        .workspaces
                        .classify
                        .places
                        .as_ref()
                        .filter(|(k, _)| *k == "suggest")
                        .and_then(|(_, v)| v["suggestions"].as_array().cloned())
                        .unwrap_or_default()
                        .iter()
                        .filter(|s| s["conflict"].as_bool() != Some(true))
                        .filter_map(|s| s["id"].as_u64())
                        .collect();
                    if ids.is_empty() {
                        return Err("no suggestion to confirm (ask for suggestions first)".into());
                    }
                    app.run("places.confirm", json!({"ids": ids}))?
                }
                other => return Err(format!("unknown places action `{other}`")),
            };
            let kind = match action {
                "preview" => "preview",
                "suggest" => "suggest",
                _ => "done",
            };
            app.workspaces.classify.places = Some((kind, r.clone()));
            app.workspaces.classify.last = Some(json!({"places": action, "result": r}));
            Ok(r)
        }
        // keyword suggestions for the photos with none (a batch at a time, in the background)
        "workspace.classify.keywords" => {
            let action = sv("action").ok_or("missing `action` (suggest | acceptAll | ignoreAll)")?;
            match action {
                "suggest" => {
                    let ids: Vec<u64> = ids_for(&app.session.catalog, "noKeyword").unwrap_or_default().into_iter().take(KEYWORD_BATCH).collect();
                    if ids.is_empty() {
                        return Err("every photo has a keyword".into());
                    }
                    let n = ids.len();
                    crate::heavy::suggest(app, &json!({"ids": ids, "includeExisting": false, "background": true}))?;
                    Ok(json!({"photos": n}))
                }
                "acceptAll" | "ignoreAll" => {
                    let Some(kw) = app.sorting.kw.as_ref() else { return Err("no suggestion to review".into()) };
                    let items: Vec<Value> =
                        kw.photos.iter().flat_map(|(id, list)| list.iter().map(move |s| json!({"id": id, "keyword": s.keyword}))).collect();
                    if action == "ignoreAll" {
                        app.sorting.kw = None;
                        return Ok(json!({"ignored": items.len()}));
                    }
                    let r = app.run("keywords.accept", json!({"items": items}))?;
                    app.sorting.kw = None;
                    Ok(r)
                }
                other => Err(format!("unknown keywords action `{other}`")),
            }
        }
        // the batch rename, previewed (the rename itself stays the Rename Photos… window's)
        "workspace.classify.rename" => {
            let template = sv("template").map(str::to_string).unwrap_or_else(|| app.workspaces.classify.rename_template.clone());
            if template.trim().is_empty() {
                return Err("missing `template`".into());
            }
            let sel: Vec<u64> = if app.session.selection.ids.len() > 1 {
                app.session.selection.ids.iter().map(|i| i.0).collect()
            } else {
                app.session.visible_cloned().iter().map(|i| i.0).collect()
            };
            let ids: Vec<u64> = sel.into_iter().take(12).collect();
            let r = app.session.execute("photo.renamePreview", &json!({"template": template, "ids": ids})).map_err(|e| e.to_string())?;
            app.workspaces.classify.rename_preview = Some(r.clone());
            Ok(r)
        }
        other => Err(format!("unknown classify command `{other}`")),
    }
}

/// Once a frame: read the people overview and the status when the page is in view.
pub fn tick(app: &mut LightcraftApp, now: f64) {
    if !visible(app) {
        return;
    }
    refill(app, now);
    let c = &mut app.workspaces.classify;
    if c.people_status.is_null() || now - c.people_status_at >= 0.8 {
        c.people_status_at = now;
        let st = app.session.execute("people.status", &json!({})).unwrap_or(Value::Null);
        app.workspaces.classify.people_status = st;
    }
    if app.workspaces.classify.tab == Tab::People && app.workspaces.classify.ov.is_none() && now >= app.workspaces.classify.retry_ov_at {
        match app.session.people_overview(60, 0, 6) {
            Ok(ov) if ov.grouped => app.workspaces.classify.ov = Some((0, ov)),
            // not grouped yet (a background run): look again in a moment
            _ => app.workspaces.classify.retry_ov_at = now + 0.4,
        }
    }
}

// ------------------------------------------------------------------------------------------- drawing

fn face_tile(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Thumb, size: f32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    match crate::panels::people_rec::paint_face(app, ui, r, t) {
        Some(why) => resp.on_hover_text(format!("This face cannot be shown: {why}")),
        None => resp,
    }
}

fn thumbs_of(v: &Value) -> Vec<Thumb> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    let r: Vec<f32> = t["rect"].as_array()?.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
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

fn faces_row(app: &mut LightcraftApp, ui: &mut egui::Ui, thumbs: &[Thumb], size: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for t in thumbs.iter().take(4) {
            face_tile(app, ui, t, size);
        }
    });
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    register(ui.ctx(), "region:classify", full);
    let left = Rect::from_min_max(full.min, pos2(full.min.x + LEFT_W, full.max.y));
    let right_w = if full.width() > 1100.0 { RIGHT_W } else { 280.0 };
    let right = Rect::from_min_max(pos2(full.max.x - right_w, full.min.y), full.max);
    let centre = Rect::from_min_max(pos2(left.max.x, full.min.y), pos2(right.min.x, full.max.y));
    ui.painter().rect_filled(left, 0.0, t.chrome);
    ui.painter().rect_filled(right, 0.0, t.chrome);
    ui.painter().line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0, t.divider));
    ui.painter().line_segment([right.left_top(), right.left_bottom()], Stroke::new(1.0, t.divider));
    let pad = vec2(16.0, 14.0);
    ui.scope_builder(UiBuilder::new().max_rect(left.shrink2(pad)), |ui| {
        egui::ScrollArea::vertical().id_salt("classify-left").auto_shrink([false, false]).show(ui, |ui| left_column(app, ui));
    });
    ui.scope_builder(UiBuilder::new().max_rect(centre.shrink2(pad)), |ui| {
        egui::ScrollArea::vertical().id_salt("classify-centre").auto_shrink([false, false]).show(ui, |ui| centre_column(app, ui));
    });
    ui.scope_builder(UiBuilder::new().max_rect(right.shrink2(pad)), |ui| {
        egui::ScrollArea::vertical().id_salt("classify-right").auto_shrink([false, false]).show(ui, |ui| right_column(app, ui));
    });
}

/// One health counter: label, count, a bar of the share, clickable.
fn counter(ui: &mut egui::Ui, id: &str, label: &str, n: usize, total: usize, tip: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
    register(ui.ctx(), format!("counter:{id}"), r);
    let resp = resp.on_hover_text(tip);
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, t.hover);
    }
    ui.painter().text(r.min + vec2(4.0, 10.0), egui::Align2::LEFT_CENTER, label, t.font(12.5), t.text);
    ui.painter().text(pos2(r.right() - 4.0, r.top() + 10.0), egui::Align2::RIGHT_CENTER, format!("{n} of {total}"), t.font(12.0), t.text_label);
    let bar = Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 24.0), vec2(w - 8.0, 6.0));
    ui.painter().rect_filled(bar, 3.0, t.track);
    let frac = if total > 0 { n as f32 / total as f32 } else { 0.0 };
    ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, 6.0)), 3.0, if n > 0 { t.accent } else { t.pick });
    resp.clicked()
}

fn left_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let h = health(app);
    crate::home::heading(ui, &t, "Archive health");
    let pct = if h.total > 0 { 100.0 * h.classified as f32 / h.total as f32 } else { 0.0 };
    ui.label(
        egui::RichText::new(format!("Classified: {pct:.1} % ({} of {} photos: a person or a place, and a keyword)", h.classified, h.total))
            .color(t.text_label)
            .size(11.5),
    );
    ui.add_space(6.0);
    let mut open: Option<&str> = None;
    for (what, id, label, n, tip) in [
        ("noKeyword", "noKeyword", "No keyword", h.no_keyword, "Show the photos that have no keyword at all"),
        ("noPlace", "noPlace", "No place", h.no_place, "Show the photos with no Places keyword and no GPS that resolves to a place"),
        ("noPerson", "noPerson", "No person", h.no_person, "Show the photos with no People keyword and no named face"),
        ("noRating", "noRating", "No rating", h.no_rating, "Show the photos with no star"),
    ] {
        if counter(ui, id, label, n, h.total, tip) {
            open = Some(what);
        }
    }
    if counter(
        ui,
        "duplicates",
        &format!("Same file ({} sets)", h.duplicate_sets),
        h.duplicates,
        h.total,
        "Show the photos whose file content is identical to another photo's",
    ) {
        open = Some("duplicates");
    }
    if let Some(what) = open
        && let Err(e) = app.run("workspace.classify.filter", json!({"what": what}))
    {
        app.ui.status = e;
    }
    ui.add_space(12.0);
    crate::home::heading(ui, &t, "Queue");
    let st = app.workspaces.classify.people_status.clone();
    let (read, total) = (st["index"]["photos"].as_u64().unwrap_or(0), h.total);
    ui.label(egui::RichText::new(format!("Faces read: {read} of {total} photos")).color(t.text_label).size(12.0));
    let c = &app.workspaces.classify;
    let groups = c.ov.as_ref().map_or(0, |(_, o)| o.groups_total);
    let named = c.ov.as_ref().map_or(0, |(_, o)| o.persons.len());
    let mut tab: Option<Tab> = None;
    for (id, label, n, which) in
        [("queueQuestions", "Questions “same person?”", c.queue_total, Tab::People), ("queueGroups", "Groups to name", groups, Tab::People)]
    {
        let r = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(t.text).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| text_button(ui, id, &n.to_string(), false).clicked())
        });
        if r.inner.inner {
            tab = Some(which);
        }
    }
    // what the other tabs have waiting (known once they were asked)
    let places_waiting = c.places.as_ref().and_then(|(k, v)| match *k {
        "preview" => v["wouldWrite"].as_u64(),
        "suggest" => v["suggestions"].as_array().map(|a| a.len() as u64),
        _ => None,
    });
    let kw_waiting = app.sorting.kw.as_ref().map(|kw| {
        let mut all: std::collections::BTreeSet<&str> = Default::default();
        kw.photos.iter().flat_map(|(_, l)| l.iter()).for_each(|s| {
            all.insert(s.keyword.as_str());
        });
        all.len() as u64
    });
    for (id, label, n, which) in
        [("queuePlaces", "Places to confirm", places_waiting, Tab::Places), ("queueKeywords", "Keywords to review", kw_waiting, Tab::Keywords)]
    {
        let r = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(t.text).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                text_button(ui, id, &n.map_or("—".to_string(), |n| n.to_string()), false).clicked()
            })
        });
        if r.inner.inner {
            tab = Some(which);
        }
    }
    ui.label(egui::RichText::new(format!("People named: {named}")).color(t.text_dim).size(11.5));
    if let Some(tab) = tab {
        let _ = app.run("workspace.classify.tab", json!({"tab": tab.name()}));
    }
    ui.add_space(12.0);
    crate::home::tasks_panel(app, ui);
}

fn centre_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for tab in Tab::ALL {
            if text_button(ui, &format!("classifyTab:{}", tab.name()), crate::i18n::tr(tab.label()), app.workspaces.classify.tab == tab).clicked() {
                app.workspaces.classify.tab = tab;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if text_button(ui, "classifyClassic", crate::i18n::tr("Classic People view"), false)
                .on_hover_text(crate::i18n::tr("The People ▸ Recognition view of LightCraft (drag and drop, persons, suggestions)"))
                .clicked()
            {
                let _ = app.run("view.people", json!({}));
            }
        });
    });
    ui.add_space(10.0);
    match app.workspaces.classify.tab {
        Tab::People => people_tab(app, ui, &t),
        Tab::Places => places_tab(app, ui, &t),
        Tab::Keywords => keywords_tab(app, ui, &t),
        Tab::Duplicates => duplicates_tab(app, ui, &t),
    }
}

fn people_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let st = app.workspaces.classify.people_status.clone();
    if st["available"].as_bool() != Some(true) {
        ui.label(egui::RichText::new(crate::i18n::tr("This build cannot recognize faces.")).color(t.text_dim));
        return;
    }
    let indexed = st["index"]["faces"].as_u64().unwrap_or(0);
    if indexed == 0 {
        crate::home::heading(ui, t, "Faces have not been read yet");
        let installed = st["model"]["installed"].as_bool() == Some(true);
        let running = st["job"]["running"].as_bool() == Some(true);
        if installed {
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    "Reading the faces of a whole library takes hours (about 0.45 s per photo). It runs in the background and can be stopped.",
                ))
                .color(t.text_label),
            );
            if ui.add_enabled(!running, egui::Button::new(crate::i18n::tr("Read the faces of the library"))).clicked() {
                let r = app.run("people.index", json!({"background": true}));
                if let Err(e) = r {
                    app.ui.status = e;
                }
            }
        } else {
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    "The People model is a download that needs your agreement: open the classic People view to read its terms and install it.",
                ))
                .color(t.text_label),
            );
        }
        return;
    }
    // ---- the question
    let total = app.workspaces.classify.queue_total;
    crate::home::heading(ui, t, &format!("Same person? ({total} questions)"));
    ui.label(
        egui::RichText::new(crate::i18n::tr("Y yes · N no · U not sure — nothing is decided until you answer; “not sure” asks again later."))
            .color(t.text_dim)
            .size(12.0),
    );
    ui.add_space(4.0);
    match app.workspaces.classify.queue.first().cloned() {
        Some(q) => question_card(app, ui, t, &q),
        None => {
            let e = app.workspaces.classify.last_error.clone();
            ui.label(egui::RichText::new(e.unwrap_or_else(|| crate::i18n::tr("No doubtful pair right now.").into())).color(t.text_dim));
        }
    }
    ui.add_space(14.0);
    // ---- the groups
    let Some((_, ov)) = app.workspaces.classify.ov.clone() else {
        ui.label(egui::RichText::new(crate::i18n::tr("Grouping the faces…")).color(t.text_dim));
        return;
    };
    crate::home::heading(ui, t, &format!("Groups to name ({}) — biggest first", ov.groups_total));
    if ov.groups.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("Every group is named, unknown or skipped.")).color(t.text_dim));
    }
    let persons: Vec<(String, String)> = ov.persons.iter().filter_map(|p| Some((p.id.clone(), p.name.clone()?))).collect();
    let mut first = true;
    for g in ov.groups.iter().take(GROUPS_SHOWN) {
        group_card(app, ui, t, g, &persons, first);
        first = false;
    }
    app.workspaces.classify.focus_first = false;
}

fn question_card(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, q: &Value) {
    let (a, b): (Vec<Thumb>, Vec<Thumb>) = match q["kind"].as_str() {
        Some("face") => (thumbs_of(&json!([q["face"]])), thumbs_of(&q["personFaces"])),
        Some("group-person") => (thumbs_of(&q["group"]), thumbs_of(&q["personFaces"])),
        _ => (thumbs_of(&q["a"]), thumbs_of(&q["b"])),
    };
    egui::Frame::new().fill(t.inset).corner_radius(6.0).inner_margin(12.0).show(ui, |ui| {
        register(ui.ctx(), "classify:question", ui.max_rect());
        ui.horizontal(|ui| {
            faces_row(app, ui, &a, 76.0);
            ui.label(egui::RichText::new("≟").font(t.semibold(24.0)).color(t.text_dim));
            faces_row(app, ui, &b, 76.0);
        });
        if let Some(n) = q["name"].as_str() {
            ui.label(egui::RichText::new(format!("Is this {n}?")).color(t.text).size(13.0));
        }
        ui.horizontal(|ui| {
            for (id, label, ans, primary) in
                [("classifyYes", "Yes  Y", "yes", true), ("classifyNo", "No  N", "no", false), ("classifyUnsure", "Not sure  U", "unsure", false)]
            {
                let b = if primary {
                    ui.add(egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(t.accent))
                } else {
                    ui.button(label)
                };
                register(ui.ctx(), format!("button:{id}"), b.rect);
                if b.clicked()
                    && let Err(e) = answer(app, ans)
                {
                    app.ui.status = e;
                }
            }
        });
    });
}

fn group_card(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    g: &lightcraft_engine::people::GroupView,
    persons: &[(String, String)],
    first: bool,
) {
    let selected = app.workspaces.classify.open_group.as_deref() == Some(g.id.as_str());
    let mut act: Option<(&'static str, Value)> = None;
    egui::Frame::new()
        .fill(t.inset)
        .corner_radius(6.0)
        .stroke(if selected { Stroke::new(1.5, t.accent) } else { Stroke::NONE })
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    faces_row(app, ui, &g.thumbs, 56.0);
                    let r = ui.add(
                        egui::Label::new(egui::RichText::new(format!("{} faces · {} photos", g.faces, g.photos)).color(t.text_dim).size(11.5))
                            .sense(Sense::click()),
                    );
                    register(ui.ctx(), format!("classify:group:{}", g.id), r.rect);
                    if r.clicked() {
                        act = Some(("open", json!(g.id)));
                    }
                });
                ui.vertical(|ui| {
                    let name = app.workspaces.classify.names.entry(g.id.clone()).or_default();
                    let eid = egui::Id::new(("classify-name", &g.id));
                    let r = ui.add(
                        egui::TextEdit::singleline(name)
                            .id(eid)
                            .hint_text(crate::i18n::tr("Name this person…"))
                            .desired_width(ui.available_width().min(250.0)),
                    );
                    register(ui.ctx(), format!("field:classifyName:{}", g.id), r.rect);
                    if first && app.workspaces.classify.focus_first {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let typed = name.trim().to_string();
                    ui.horizontal(|ui| {
                        if (text_button(ui, &format!("classifyNameGo:{}", g.id), crate::i18n::tr("Name"), false).clicked() || enter)
                            && !typed.is_empty()
                        {
                            act = Some(("name", json!({"group": g.id, "name": typed})));
                        }
                        if text_button(ui, &format!("classifyUnknown:{}", g.id), crate::i18n::tr("Unknown"), false)
                            .on_hover_text(crate::i18n::tr("You looked and do not know them: they leave this list (nameable later)"))
                            .clicked()
                        {
                            act = Some(("people.unknown", json!({"group": g.id})));
                        }
                        if text_button(ui, &format!("classifySkip:{}", g.id), crate::i18n::tr("Skip"), false)
                            .on_hover_text(crate::i18n::tr("Hide this group for now; it decides nothing"))
                            .clicked()
                        {
                            act = Some(("people.skip", json!({"group": g.id})));
                        }
                    });
                    if !persons.is_empty() {
                        egui::ComboBox::from_id_salt(("classify-merge", &g.id)).selected_text(crate::i18n::tr("Merge into…")).show_ui(ui, |ui| {
                            for (pid, pname) in persons {
                                if ui.selectable_label(false, pname).clicked() {
                                    act = Some(("people.merge", json!({"from": g.id, "into": pid})));
                                }
                            }
                        });
                    }
                });
            });
        });
    ui.add_space(6.0);
    let Some((what, params)) = act else { return };
    match what {
        "open" => {
            app.workspaces.classify.open_group = params.as_str().map(str::to_string);
            app.workspaces.classify.members = None;
        }
        "name" => match app.run("workspace.classify.name", params) {
            Ok(_) => app.workspaces.classify.focus_first = true,
            Err(e) => app.ui.status = e,
        },
        cmd => {
            if let Err(e) = app.run(cmd, params) {
                app.ui.status = e;
            } else {
                app.workspaces.classify.dirty();
            }
        }
    }
}

fn places_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let st = app.session.execute("places.status", &json!({})).unwrap_or(Value::Null);
    if st["data"]["installed"].as_bool() != Some(true) {
        ui.label(egui::RichText::new(crate::i18n::tr("The offline places data (GeoNames) is not installed in this build.")).color(t.text_dim));
        return;
    }
    let h = health(app);
    crate::home::heading(ui, t, &format!("Places: {} photos have none", h.no_place));
    ui.label(
        egui::RichText::new(crate::i18n::tr("Both steps only look first: nothing is written until you apply or confirm."))
            .color(t.text_dim)
            .size(12.0),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if text_button(ui, "classifyPlacesPreview", crate::i18n::tr("Preview places from the GPS"), false).clicked()
            && let Err(e) = app.run("workspace.classify.places", json!({"action": "preview"}))
        {
            app.ui.status = e;
        }
        if text_button(ui, "classifyPlacesSuggest", crate::i18n::tr("Suggest places for photos without GPS"), false)
            .on_hover_text(crate::i18n::tr("From the place of a photo taken by the same camera within 30 minutes"))
            .clicked()
            && let Err(e) = app.run("workspace.classify.places", json!({"action": "suggest"}))
        {
            app.ui.status = e;
        }
    });
    ui.add_space(8.0);
    let Some((kind, v)) = app.workspaces.classify.places.clone() else { return };
    match kind {
        "preview" => {
            let n = v["wouldWrite"].as_u64().unwrap_or(0);
            ui.label(
                egui::RichText::new(format!(
                    "{n} photos would get a place · {} already have one · {} have no GPS",
                    v["alreadyPlaced"], v["withoutGps"]
                ))
                .color(t.text)
                .size(13.0),
            );
            for p in v["places"].as_array().into_iter().flatten().take(8) {
                ui.label(egui::RichText::new(format!("   {} · {}", p["place"].as_str().unwrap_or(""), p["count"])).color(t.text_label).size(12.0));
            }
            if n > 0 {
                let b = ui
                    .add(egui::Button::new(egui::RichText::new(format!("Apply to {n} photos (one undo step)")).color(Color32::WHITE)).fill(t.accent));
                register(ui.ctx(), "button:classifyPlacesApply", b.rect);
                if b.clicked() {
                    match app.run("workspace.classify.places", json!({"action": "apply"})) {
                        Ok(r) => app.toast(ui.ctx(), crate::i18n::tr_format!("{} photos got a place", r["written"].as_u64().unwrap_or(0))),
                        Err(e) => app.ui.status = e,
                    }
                }
            }
        }
        "suggest" => {
            let ok = v["suggestions"].as_array().map_or(0, |a| a.iter().filter(|s| s["conflict"].as_bool() != Some(true)).count());
            ui.label(
                egui::RichText::new(format!("{} suggestions ({} without conflict) for {} photos looked at", v["suggested"], ok, v["candidates"]))
                    .color(t.text)
                    .size(13.0),
            );
            let mut by: BTreeMap<String, usize> = BTreeMap::new();
            for s in v["suggestions"].as_array().into_iter().flatten() {
                *by.entry(s["place"].as_str().unwrap_or("").to_string()).or_default() += 1;
            }
            let mut by: Vec<_> = by.into_iter().collect();
            by.sort_by_key(|b| std::cmp::Reverse(b.1));
            for (p, n) in by.iter().take(8) {
                ui.label(egui::RichText::new(format!("   {p} · {n}")).color(t.text_label).size(12.0));
            }
            if ok > 0 {
                let b = ui.add(
                    egui::Button::new(egui::RichText::new(format!("Confirm the {ok} suggestions without conflict")).color(Color32::WHITE))
                        .fill(t.accent),
                );
                register(ui.ctx(), "button:classifyPlacesConfirm", b.rect);
                if b.clicked() {
                    match app.run("workspace.classify.places", json!({"action": "confirm"})) {
                        Ok(r) => app.toast(ui.ctx(), crate::i18n::tr_format!("{} places confirmed", r["changed"].as_u64().unwrap_or(0))),
                        Err(e) => app.ui.status = e,
                    }
                }
            }
        }
        _ => {
            ui.label(egui::RichText::new(format!("Done: {v}")).color(t.text_label).size(12.0));
        }
    }
}

fn keywords_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = health(app);
    crate::home::heading(ui, t, &format!("Keywords: {} photos have none", h.no_keyword));
    ui.label(egui::RichText::new(crate::i18n::tr(&format!("The search model looks at {KEYWORD_BATCH} photos without a keyword at a time (the first use loads it: about a minute). Nothing is added until you accept."))).color(t.text_dim).size(12.0));
    ui.add_space(6.0);
    let running = app.tasks.is_running(crate::heavy::SUGGEST);
    ui.horizontal(|ui| {
        if ui.add_enabled(!running && h.no_keyword > 0, egui::Button::new(crate::i18n::tr("Suggest keywords"))).clicked()
            && let Err(e) = app.run("workspace.classify.keywords", json!({"action": "suggest"}))
        {
            app.ui.status = e;
        }
        if running {
            ui.spinner();
        }
    });
    let Some(kw) = app.sorting.kw.as_ref() else { return };
    if let Some(e) = &kw.error {
        ui.label(egui::RichText::new(e).color(t.caution));
        return;
    }
    let mut agg: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for (_, list) in &kw.photos {
        for s in list {
            let e = agg.entry(s.keyword.clone()).or_insert((0, 0.0));
            e.0 += 1;
            e.1 = e.1.max(s.score);
        }
    }
    if agg.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("No keyword reached the threshold.")).color(t.text_dim));
        return;
    }
    let mut rows: Vec<(String, usize, f64)> = agg.into_iter().map(|(k, (n, s))| (k, n, s)).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)));
    ui.add_space(8.0);
    let mut accept: Vec<String> = Vec::new();
    let mut ignore: Vec<String> = Vec::new();
    for (k, n, s) in rows.iter().take(40) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{}  ·  {n} photos  ·  {:.0} %", k.replace('|', " › "), s * 100.0)).color(t.text).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_button(ui, &format!("classifyKwIgnore:{k}"), crate::i18n::tr("Ignore"), false).clicked() {
                    ignore.push(k.clone());
                }
                if text_button(ui, &format!("classifyKwAccept:{k}"), crate::i18n::tr("Accept"), false).clicked() {
                    accept.push(k.clone());
                }
            });
        });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if text_button(ui, "classifyKwAcceptAll", &format!("Accept all {}", rows.len()), false).clicked() {
            accept = rows.iter().map(|r| r.0.clone()).collect();
        }
        if text_button(ui, "classifyKwIgnoreAll", crate::i18n::tr("Ignore all"), false).clicked() {
            ignore = rows.iter().map(|r| r.0.clone()).collect();
        }
    });
    if !accept.is_empty() {
        let items: Vec<Value> = app
            .sorting
            .kw
            .as_ref()
            .map(|kw| {
                kw.photos
                    .iter()
                    .flat_map(|(id, list)| list.iter().filter(|s| accept.contains(&s.keyword)).map(move |s| json!({"id": id, "keyword": s.keyword})))
                    .collect()
            })
            .unwrap_or_default();
        match app.run("keywords.accept", json!({"items": items})) {
            Ok(v) => app.ui.status = format!("{} keywords added", v["changed"].as_u64().unwrap_or(0)),
            Err(e) => app.ui.status = e,
        }
        ignore.extend(accept);
    }
    if !ignore.is_empty()
        && let Some(kw) = &mut app.sorting.kw
    {
        for (_, list) in &mut kw.photos {
            list.retain(|s| !ignore.contains(&s.keyword));
        }
    }
}

fn duplicates_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = health(app);
    crate::home::heading(ui, t, &format!("Same file: {} photos in {} sets", h.duplicates, h.duplicate_sets));
    ui.label(
        egui::RichText::new(crate::i18n::tr("These photos have exactly the same bytes. LightCraft only lists them: it deletes nothing."))
            .color(t.text_dim)
            .size(12.0),
    );
    ui.add_space(6.0);
    let mut sets: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for p in app.session.catalog.photos().filter(|p| p.in_library()) {
        if let Some(c) = p.content_hash.as_deref() {
            sets.entry(c.to_string()).or_default().push(p.id.0);
        }
    }
    let sets: Vec<Vec<u64>> = sets.into_values().filter(|v| v.len() > 1).collect();
    if sets.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("No two photos share a file.")).color(t.text_dim));
        return;
    }
    let extras: Vec<u64> = sets.iter().flat_map(|v| v.iter().skip(1).copied()).collect();
    if text_button(ui, "classifyDupExtras", &format!("Select the {} extra copies", extras.len()), false)
        .on_hover_text(crate::i18n::tr("Selects all but the first of each set, so you can look at them and delete them yourself"))
        .clicked()
    {
        let _ = app.run("library.select", json!({"ids": extras}));
    }
    ui.add_space(6.0);
    for (i, v) in sets.iter().take(30).enumerate() {
        let names: Vec<String> = v.iter().filter_map(|id| app.session.catalog.photo(PhotoId(*id)).map(|p| p.file_name.clone())).collect();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(names.join("  =  ")).color(t.text).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_button(ui, &format!("classifyDupSet{i}"), crate::i18n::tr("Select"), false).clicked() {
                    let _ = app.run("library.select", json!({"ids": v}));
                }
            });
        });
    }
}

fn right_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // the faces of the group in view
    crate::home::heading(ui, &t, "Photos of this group");
    let gid = app
        .workspaces
        .classify
        .open_group
        .clone()
        .or_else(|| app.workspaces.classify.ov.as_ref().and_then(|(_, o)| o.groups.first().map(|g| g.id.clone())));
    match gid {
        Some(gid) if app.workspaces.classify.tab == Tab::People => {
            if app.workspaces.classify.members.as_ref().is_none_or(|(g, _)| *g != gid) {
                let v = app.session.execute("people.members", &json!({"group": gid, "limit": 12})).unwrap_or(Value::Null);
                app.workspaces.classify.members = Some((gid.clone(), v));
                app.workspaces.classify.picked_faces.clear();
            }
            let faces = app.workspaces.classify.members.as_ref().map(|(_, v)| v["faces"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
            let per_row = ((ui.available_width() + 4.0) / 76.0).floor().max(1.0) as usize;
            for chunk in faces.chunks(per_row) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for f in chunk {
                        let Some(th) = thumbs_of(&json!([{"photo": f["photo"], "face": f["face"], "rect": f["rect"]}])).into_iter().next() else {
                            continue;
                        };
                        let on = app.workspaces.classify.picked_faces.contains(&th.face);
                        let r = face_tile(app, ui, &th, 72.0);
                        if on {
                            ui.painter().rect_stroke(r.rect, 3.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                        }
                        register(ui.ctx(), format!("classify:face:{}", th.face), r.rect);
                        if r.clicked() {
                            let p = &mut app.workspaces.classify.picked_faces;
                            if let Some(i) = p.iter().position(|x| *x == th.face) {
                                p.remove(i);
                            } else {
                                p.push(th.face.clone());
                            }
                        }
                    }
                });
            }
            let picked = app.workspaces.classify.picked_faces.clone();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let enabled = !picked.is_empty();
                if ui
                    .add_enabled_ui(enabled, |ui| text_button(ui, "classifyMoveUnknown", crate::i18n::tr("Not them"), false))
                    .inner
                    .on_hover_text(crate::i18n::tr("Click faces to pick them, then move them out of this group (people.move)"))
                    .clicked()
                {
                    match app.run("people.move", json!({"faces": picked, "to": "unknown"})) {
                        Ok(_) => app.workspaces.classify.dirty(),
                        Err(e) => app.ui.status = e,
                    }
                }
                if ui.add_enabled_ui(enabled, |ui| text_button(ui, "classifyNotFace", crate::i18n::tr("Not a real face"), false)).inner.clicked() {
                    match app.run("people.ignore", json!({"faces": picked})) {
                        Ok(_) => app.workspaces.classify.dirty(),
                        Err(e) => app.ui.status = e,
                    }
                }
            });
        }
        _ => {
            ui.label(egui::RichText::new(crate::i18n::tr("Pick a group on the People tab to see its faces here.")).color(t.text_dim).size(12.0));
        }
    }
    ui.add_space(14.0);
    // batch rename, previewed
    crate::home::heading(ui, &t, "Rename in batch");
    if app.workspaces.classify.rename_template.is_empty() {
        app.workspaces.classify.rename_template = "{date}_{name}".into();
    }
    let r = ui.add(egui::TextEdit::singleline(&mut app.workspaces.classify.rename_template).desired_width(f32::INFINITY));
    register(ui.ctx(), "field:classifyRename", r.rect);
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "Tags: {name} {num} {seq} {date} {folder} {camera} {lens} {iso} {rating} {title} {creator} {ext}. There is no place or person tag.",
        ))
        .color(t.text_dim)
        .size(11.0),
    );
    ui.horizontal(|ui| {
        if text_button(ui, "classifyRenamePreview", crate::i18n::tr("Preview the first 12"), false).clicked()
            && let Err(e) = app.run("workspace.classify.rename", json!({}))
        {
            app.ui.status = e;
        }
        if text_button(ui, "classifyRenameOpen", crate::i18n::tr("Rename…"), false)
            .on_hover_text(crate::i18n::tr("Opens Rename Photos… for the selection: the files are renamed only there"))
            .clicked()
        {
            let _ = app.run("dialog.rename", json!({}));
        }
    });
    if let Some(v) = &app.workspaces.classify.rename_preview {
        for p in v.as_array().into_iter().flatten().take(12) {
            ui.label(
                egui::RichText::new(format!("{}  →  {}", p["from"].as_str().unwrap_or(""), p["to"].as_str().unwrap_or("")))
                    .color(t.text_label)
                    .size(11.5),
            );
        }
    }
}

/// `ui.inspect` → `workspaces.classify`.
pub fn inspect(app: &LightcraftApp) -> Value {
    let c = &app.workspaces.classify;
    json!({
        "visible": visible(app),
        "tab": c.tab.name(),
        "queue": c.queue.len(),
        "answeredIds": c.answered_ids.len(),
        "queueTotal": c.queue_total,
        "answers": {"yes": c.answers[0], "no": c.answers[1], "unsure": c.answers[2]},
        "health": c.health.as_ref().map(|(_, h)| json!({
            "total": h.total, "noKeyword": h.no_keyword, "noPlace": h.no_place, "noPerson": h.no_person,
            "noRating": h.no_rating, "duplicates": h.duplicates, "duplicateSets": h.duplicate_sets, "classified": h.classified,
        })),
        "last": c.last,
    })
}
