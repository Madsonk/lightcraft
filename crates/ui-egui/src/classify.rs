//! Library ▸ Classify: the bench for an archive. One page for the work that never ends on a library
//! of tens of thousands of photos: how healthy the archive is (photos with no keyword, no place, no
//! person, no rating, the same file twice: counters that open the photos they count), the people to
//! name, the places LightCraft can fill from the GPS, the keywords still missing and a preview of a
//! batch rename.
//!
//! It adds no analysis of its own. People are named in the People view (`faces.unnamed`,
//! `faces.nameFaces`: the faces nobody has named, look-alikes side by side, a suggested name when
//! the named ones recognise the face); Places are `metadata.reverseGeocode`; the rename is
//! `photo.renamePreview`. **Everything is a suggestion**: a place writes nothing until the user
//! presses the button that applies it.

use std::collections::{BTreeMap, HashMap};

use egui::{RichText, UiBuilder, pos2, vec2};
use lightcraft_catalog::Catalog;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};
use crate::workspaces::{self, Step};

const LEFT_W: f32 = 270.0;

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
    /// Face regions nobody has named.
    pub unnamed_faces: usize,
}

/// A photo has a person when a `People` keyword is on it or a face region carries a name.
fn has_person(p: &lightcraft_catalog::Photo) -> bool {
    p.meta.keywords.iter().any(|k| lightcraft_catalog::keywords::is_under(k, "People"))
        || p.meta.regions.iter().any(|r| r.kind == lightcraft_meta::RegionKind::Face && r.name.as_deref().is_some_and(|n| !n.trim().is_empty()))
}

/// A photo has a place when a place name is written on it (the GPS alone is what the Places tab turns into names).
fn has_place(p: &lightcraft_catalog::Photo) -> bool {
    let m = &p.meta;
    [&m.location, &m.city, &m.state, &m.country].iter().any(|s| !s.trim().is_empty())
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
        h.unnamed_faces += p
            .meta
            .regions
            .iter()
            .filter(|r| r.kind == lightcraft_meta::RegionKind::Face && r.name.as_deref().is_none_or(|n| n.trim().is_empty()))
            .count();
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
    /// The last places preview or result (`preview` / `done`).
    places: Option<(&'static str, Value)>,
    pub rename_template: String,
    rename_preview: Option<Value>,
    /// What the last batch operation did (`ui.inspect`).
    pub last: Option<Value>,
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

// ------------------------------------------------------------------------------------------ commands

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
            let ids = ids_for(&app.session.catalog, what).ok_or_else(|| format!("unknown counter `{what}`"))?;
            if ids.is_empty() {
                return Err(format!("no photo to show ({what})"));
            }
            app.run("library.source", json!({"kind": "all"}))?;
            app.run("library.clearFilter", json!({}))?;
            app.run("library.filter", json!({"only": ids}))?;
            workspaces::set_step(app, Step::Sort)?;
            Ok(json!({"count": app.session.visible().len()}))
        }
        "workspace.classify.refresh" => {
            app.workspaces.classify.health = None;
            app.workspaces.classify.places = None;
            Ok(Value::Null)
        }
        // places, in two steps: look (nothing written), then apply
        "workspace.classify.places" => {
            let action = sv("action").ok_or("missing `action` (preview | apply)")?;
            let (dry, kind) = match action {
                "preview" => (true, "preview"),
                "apply" => (false, "done"),
                other => return Err(format!("unknown places action `{other}`")),
            };
            let ids = ids_for(&app.session.catalog, "noPlace").unwrap_or_default();
            if ids.is_empty() {
                return Err("every photo has a place".into());
            }
            let r = app.run("metadata.reverseGeocode", json!({"ids": ids, "dryRun": dry}))?;
            app.workspaces.classify.places = Some((kind, r.clone()));
            app.workspaces.classify.last = Some(json!({"places": action, "result": r}));
            Ok(r)
        }
        // suggestions of keywords come with the semantic search (the model reads the photo)
        "workspace.classify.keywords" => Err("keyword suggestions need the semantic search model, which is not installed in this build".into()),
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

/// Once a frame: nothing to poll (the counters are counted when the catalogue changes).
pub fn tick(_app: &mut LightcraftApp, _now: f64) {}

// ------------------------------------------------------------------------------------------- drawing

fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(tr(text)).size(14.0).strong().color(t.text));
    ui.add_space(4.0);
}

fn counter(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, what: &str, label: &str, n: usize) {
    ui.horizontal(|ui| {
        let r = text_button(ui, &format!("classifyCounter:{what}"), &format!("{}  {n}", tr(label)), false);
        if r.clicked()
            && n > 0
            && let Err(e) = command(app, "workspace.classify.filter", &json!({"what": what}))
        {
            app.ui.status = e;
        }
        if n == 0 {
            ui.label(RichText::new("✓").color(t.text_dim));
        }
    });
}

fn left_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let h = health(app);
    heading(ui, &t, "Library health");
    ui.label(RichText::new(crate::i18n::tr_format!("{} of {} photos are classified", h.classified, h.total)).color(t.text_dim));
    ui.add_space(8.0);
    counter(app, ui, &t, "noKeyword", "No keyword", h.no_keyword);
    counter(app, ui, &t, "noPlace", "No place", h.no_place);
    counter(app, ui, &t, "noPerson", "No person", h.no_person);
    counter(app, ui, &t, "noRating", "No rating", h.no_rating);
    counter(app, ui, &t, "duplicates", "The same file twice", h.duplicates);
    ui.add_space(12.0);
    if text_button(ui, "classifyRefresh", "Count again", false).clicked() {
        let _ = command(app, "workspace.classify.refresh", &json!({}));
    }
    ui.add_space(12.0);
    if text_button(ui, "classifyBack", "Back to sorting", false).clicked() {
        let _ = workspaces::set_step(app, Step::Sort);
    }
}

fn people_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = health(app);
    heading(ui, t, "People");
    ui.label(RichText::new(tr("Name the faces once: the same person is then found in every photo.")).color(t.text_dim));
    ui.add_space(8.0);
    if h.unnamed_faces > 0 {
        ui.label(crate::i18n::tr_format!("{} faces nobody has named", h.unnamed_faces));
    } else {
        ui.label(tr("No unnamed face. Run View ▸ People to look for faces in new photos."));
    }
    ui.add_space(8.0);
    if text_button(ui, "classifyOpenPeople", "Name faces in People", false).clicked() {
        let _ = workspaces::switch(app, "Library");
        app.ui.view = crate::state::ViewMode::People;
    }
}

fn places_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = health(app);
    heading(ui, t, "Places");
    ui.label(crate::i18n::tr_format!("{} photos have no place written", h.no_place));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if text_button(ui, "classifyPlacesPreview", "Look (nothing is written)", false).clicked()
            && let Err(e) = command(app, "workspace.classify.places", &json!({"action": "preview"}))
        {
            app.ui.status = e;
        }
        if text_button(ui, "classifyPlacesApply", "Fill the places from the GPS", false).clicked()
            && let Err(e) = command(app, "workspace.classify.places", &json!({"action": "apply"}))
        {
            app.ui.status = e;
        }
    });
    if let Some((kind, r)) = &app.workspaces.classify.places {
        ui.add_space(8.0);
        let line = |k: &str| r.get(k).and_then(Value::as_u64);
        let (filled, skipped) = (line("filled").or_else(|| line("changed")).unwrap_or(0), line("skipped").unwrap_or(0));
        let text = if *kind == "preview" {
            crate::i18n::tr_format!("{} photos would get a place ({} left as they are)", filled, skipped)
        } else {
            crate::i18n::tr_format!("{} photos got a place ({} left as they are)", filled, skipped)
        };
        ui.label(text);
    }
}

fn keywords_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = health(app);
    heading(ui, t, "Keywords");
    ui.label(crate::i18n::tr_format!("{} photos have no keyword", h.no_keyword));
    ui.add_space(8.0);
    ui.label(RichText::new(tr("Keyword suggestions need the semantic search model.")).color(t.text_dim));
}

fn duplicates_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = health(app);
    heading(ui, t, "Same file");
    ui.label(crate::i18n::tr_format!("{} photos are the same file as another ({} sets)", h.duplicates, h.duplicate_sets));
    ui.add_space(8.0);
    if text_button(ui, "classifyShowDuplicates", "Show them", false).clicked()
        && let Err(e) = command(app, "workspace.classify.filter", &json!({"what": "duplicates"}))
    {
        app.ui.status = e;
    }
}

fn centre_column(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        for tab in Tab::ALL {
            let on = app.workspaces.classify.tab == tab;
            if text_button(ui, &format!("classifyTab:{}", tab.name()), tab.label(), on).clicked() {
                app.workspaces.classify.tab = tab;
            }
        }
    });
    ui.add_space(8.0);
    match app.workspaces.classify.tab {
        Tab::People => people_tab(app, ui, &t),
        Tab::Places => places_tab(app, ui, &t),
        Tab::Keywords => keywords_tab(app, ui, &t),
        Tab::Duplicates => duplicates_tab(app, ui, &t),
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    register(ui, "region:classify", full);
    let left = egui::Rect::from_min_max(full.min, pos2(full.min.x + LEFT_W, full.max.y));
    let centre = egui::Rect::from_min_max(pos2(left.max.x, full.min.y), full.max);
    ui.painter().rect_filled(left, 0.0, t.chrome);
    ui.painter().line_segment([left.right_top(), left.right_bottom()], egui::Stroke::new(1.0, t.divider));
    let pad = vec2(16.0, 14.0);
    ui.scope_builder(UiBuilder::new().max_rect(left.shrink2(pad)), |ui| {
        egui::ScrollArea::vertical().id_salt("classify-left").auto_shrink([false, false]).show(ui, |ui| left_column(app, ui));
    });
    ui.scope_builder(UiBuilder::new().max_rect(centre.shrink2(pad)), |ui| {
        egui::ScrollArea::vertical().id_salt("classify-centre").auto_shrink([false, false]).show(ui, |ui| centre_column(app, ui));
    });
}

pub fn inspect(app: &LightcraftApp) -> Value {
    let c = &app.workspaces.classify;
    json!({
        "visible": visible(app),
        "tab": c.tab.name(),
        "health": c.health.as_ref().map(|(_, h)| json!({
            "total": h.total, "noKeyword": h.no_keyword, "noPlace": h.no_place, "noPerson": h.no_person,
            "noRating": h.no_rating, "duplicates": h.duplicates, "duplicateSets": h.duplicate_sets, "classified": h.classified, "unnamedFaces": h.unnamed_faces,
        })),
        "places": c.places.as_ref().map(|(k, v)| json!({"kind": k, "result": v})),
        "renamePreview": c.rename_preview,
        "last": c.last,
    })
}
