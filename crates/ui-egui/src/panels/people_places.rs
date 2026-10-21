//! "People" and "Places" in the left panel: the keywords the people and places features write
//! (`People|Name`, `Places|Country|City`), listed with their photo counts as sections of their own
//! instead of being buried in the Keywords tree. A click filters the grid by that person or place
//! (the same keyword filter), a photo dropped on a row gets the keyword (the drop target the
//! Keywords tree already was: `dnd::keyword_drop`), and a last row opens the whole view.
//!
//! Nothing here is new data: the rows are the catalog's keyword tree, so a person named in
//! Recognition appears as soon as `people.sync` has tagged their photos.

use serde_json::{Value, json};

use super::left::{keyword_rows_with, row, sidebar_section_header};
use crate::LightcraftApp;
use crate::icons::Icon;

const PEOPLE: &str = "People";
const PLACES: &str = "Places";

/// Whether the keyword at the top of `path` is one of the two roots these sections show.
pub fn owns(path: &str) -> bool {
    let root = path.split('|').next().unwrap_or(path);
    root.eq_ignore_ascii_case(PEOPLE) || root.eq_ignore_ascii_case(PLACES)
}

/// `(faces found, people named)` when the library has a people index on disk (read at most every
/// few seconds, and never opening an index that is not there).
fn people_index(app: &mut LightcraftApp, ui: &egui::Ui) -> Option<(u64, u64)> {
    // (only a library on disk that has an index folder: asking any other would only create one)
    let indexed = app.session.library.as_ref().is_some_and(|l| l.dir.join("people").is_dir());
    let id = egui::Id::new("left-people-index");
    let now = ui.input(|i| i.time);
    let rev = app.session.catalog.revision;
    if let Some((at, r, v)) = ui.data(|d| d.get_temp::<(f64, u64, Option<(u64, u64)>)>(id))
        && r == rev
        && now - at < 5.0
    {
        return v;
    }
    let v = indexed.then(|| app.session.execute("people.status", &json!({})).ok()).flatten().and_then(|s: Value| {
        let i = &s["index"];
        Some((i["faces"].as_u64()?, i["named"].as_u64()?))
    });
    ui.data_mut(|d| d.insert_temp(id, (now, rev, v)));
    v
}

/// The sections appear once there is something to list: a person or a place already keyworded, or
/// faces found and waiting for names (a library that has neither shows neither: People and Places
/// are always one click away in the view bar and the View menu).
pub fn sections(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if app.session.catalog.is_empty() {
        return;
    }
    let tree = app.caches.keyword_tree(&app.session.catalog);
    let root = |name: &str| tree.iter().find(|n| n.path.eq_ignore_ascii_case(name)).cloned();
    let (people, places) = (root(PEOPLE), root(PLACES));
    let index = people_index(app, ui);
    let faces = index.map_or(0, |(f, _)| f);

    if people.is_some() || faces > 0 {
        ui.add_space(10.0);
        if sidebar_section_header(app, ui, "people", PEOPLE).1 {
            if let Some(p) = &people {
                keyword_rows_with(app, ui, &p.children, 0.0, Icon::Subject);
            }
            let named = people.as_ref().map_or(0, |p| p.children.len());
            let label = match index {
                Some((faces, _)) if faces > 0 && named == 0 => crate::i18n::tr_format!("{faces} faces found: name them…", faces = faces),
                Some((faces, _)) if faces > 0 => crate::i18n::tr("All people and groups…").to_string(),
                _ => crate::i18n::tr("All people…").to_string(),
            };
            if row(app, ui, "peopleOpen", Icon::Subject, &label, None, false, 0.0)
                .on_hover_text(crate::i18n::tr("Opens the People view: the faces found in your photos, grouped, to name"))
                .clicked()
            {
                let _ = app.run("view.people", json!({}));
            }
        }
    }

    if let Some(p) = &places {
        ui.add_space(10.0);
        if sidebar_section_header(app, ui, "places", PLACES).1 {
            keyword_rows_with(app, ui, &p.children, 0.0, Icon::Folder);
            if row(app, ui, "placesOpen", Icon::Folder, "All places…", None, false, 0.0)
                .on_hover_text(crate::i18n::tr("Opens the Places view: where your photos were taken, from their GPS"))
                .clicked()
            {
                let _ = app.run("view.places", json!({}));
            }
        }
    }
}
