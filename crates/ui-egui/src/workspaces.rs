//! Workspaces (LightCraft v11, phase 2 of the UX study): three spaces — Library, Retouch, Create — each a
//! saved arrangement of the panels that already exist, plus the one context (source, filter, selection,
//! active photo) that travels between them.
//!
//! A space is only a *layout*: which view the centre shows, which panels are open on the left, right
//! and bottom, how wide they are. Changing space applies that layout to [`crate::UiState`] and nothing
//! else, so the library filter and the selection (which live in the session, not in the UI state) are
//! never touched. The last layout of every space is remembered and written to `workspaces.json` beside
//! `ui.json`, the way FilmCraft keeps its own `workspaces.json` (same top-level shape: `current` plus a
//! list of `{name, layout}` entries, each layout kept as JSON so one unreadable entry loses nothing).
//!
//! Everything is switchable: with Settings ▸ Workspaces off the selector, the context bar and the
//! commands are gone and the window is the official LightCraft's. The code lives in this file,
//! `context_bar.rs` and `create_space.rs`; the shared files only carry one-line hooks.
//!
//! Docking later (dragging tabs, floating panels): a [`Layout`] is a flat description of "what is
//! where", and the commands go through [`apply`]/[`capture`] only. A dock tree can replace those two
//! functions (and add a `dock` field to [`Layout`]) without touching the selector, the context bar, the
//! commands, the menu or the file format, which already tolerates unknown fields.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::{LEFT_WIDTH, RIGHT_WIDTH, RightPanel, ViewMode};

/// `<config>/workspaces.json`, beside `ui.json`.
pub const FILE: &str = "workspaces.json";

/// The kind of a space: what its centre shows and what its context bar offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Space {
    #[default]
    Library,
    Retouch,
    Create,
}

impl Space {
    pub const ALL: [Space; 3] = [Space::Library, Space::Retouch, Space::Create];

    /// The built-in space's name (also its entry in `workspaces.json`).
    pub fn name(self) -> &'static str {
        match self {
            Space::Library => "Library",
            Space::Retouch => "Retouch",
            Space::Create => "Create",
        }
    }

    /// `workspace.library` / `workspace.retouch` / `workspace.create`.
    pub fn command(self) -> &'static str {
        match self {
            Space::Library => "workspace.library",
            Space::Retouch => "workspace.retouch",
            Space::Create => "workspace.create",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Space::Library => "Ctrl+1",
            Space::Retouch => "Ctrl+2",
            Space::Create => "Ctrl+3",
        }
    }

    /// One line for the selector's tooltip.
    pub fn tip(self) -> &'static str {
        match self {
            Space::Library => "Library: import, sort and organize",
            Space::Retouch => "Retouch: correct and style the selected photos",
            Space::Create => "Create: web gallery, slideshow and kits for the layout apps",
        }
    }

    pub fn from_name(name: &str) -> Option<Space> {
        Space::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// The three steps of the Library space (v12, phases 3 and 4 of the UX study): **Import** is the home
/// screen (resume, drop zone, essential import settings, background tasks), **Sort** is the grid with
/// the Quick sort panel, **Classify** is the bench for an archive (health counters, questions, groups).
/// A step only decides what the centre of the window shows (and which side panels give way); the
/// layout, the selection and the filter are never touched by changing step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Import,
    #[default]
    Sort,
    Classify,
}

impl Step {
    pub const ALL: [Step; 3] = [Step::Import, Step::Sort, Step::Classify];

    pub fn name(self) -> &'static str {
        match self {
            Step::Import => "import",
            Step::Sort => "sort",
            Step::Classify => "classify",
        }
    }

    pub fn from_name(name: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// What a space arranges: the shared panel state of [`crate::UiState`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Layout {
    pub space: Space,
    pub view: ViewMode,
    pub left_panel: bool,
    pub right: RightPanel,
    pub presets: bool,
    pub filmstrip: bool,
    pub filter_bar: bool,
    pub left_width: f32,
    pub right_width: f32,
}

impl Default for Layout {
    fn default() -> Self {
        factory(Space::Library)
    }
}

/// The factory layout of a space.
pub fn factory(space: Space) -> Layout {
    let base = Layout {
        space,
        view: ViewMode::PhotoGrid,
        left_panel: true,
        right: RightPanel::None,
        presets: false,
        filmstrip: false,
        filter_bar: false,
        left_width: LEFT_WIDTH.default,
        right_width: RIGHT_WIDTH.default,
    };
    match space {
        // decide fast: the sources on the left, the filter bar over the grid, keywords on the right
        Space::Library => Layout { filter_bar: true, filmstrip: true, right: RightPanel::Keywords, ..base },
        // the photo and its tools: no sources column, Edit on the right, the strip of photos below
        Space::Retouch => Layout { view: ViewMode::Detail, left_panel: true, right: RightPanel::Edit, filmstrip: true, ..base },
        // the photos that will go out on the grid, the sources (albums) on the left; the Create panel
        // takes the right-hand side of the centre
        Space::Create => base,
    }
}

/// A saved arrangement: `layout` is the last one used (remembered as you work), `origin` the one to
/// go back to with Reset (none for the built-in spaces: their origin is [`factory`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Saved {
    pub name: String,
    pub layout: Value,
    pub origin: Option<Value>,
}

/// The contents of `workspaces.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    pub version: u32,
    /// One-time migration of built-in layouts; custom spaces are preserved.
    pub module_layout_revision: u32,
    pub current: String,
    pub saved: Vec<Saved>,
    /// The import settings the home screen remembers for the next import (v12).
    pub home: Option<Value>,
}

impl Prefs {
    pub fn load(path: &std::path::Path) -> Option<Prefs> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// Written atomically (`.tmp`, then rename), like FilmCraft's.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }
}

/// App-level state of the workspaces.
pub struct Workspaces {
    pub prefs: Prefs,
    /// Where `workspaces.json` lives (`None`: not written — tests, `--memory`, `LIGHTCRAFT_NO_PREFS`).
    pub path: Option<PathBuf>,
    /// The space in use (a built-in name or a custom one).
    pub current: String,
    /// What the current space is: it decides what the centre and the context bar show.
    pub kind: Space,
    /// The layout seen at the last look; a difference with the next one is the user's change.
    seen: Option<Layout>,
    dirty: bool,
    written_at: f64,
    failing: bool,
    /// The Create panel's state.
    pub create: crate::create_space::State,
    /// What the context bar said at the last frame (`ui.inspect`).
    pub context: Option<crate::context_bar::Summary>,
    /// The top bar had room for the space selector at the last frame (else the context bar carries it).
    pub selector_in_top_bar: bool,
    /// The step of the Library space (v12).
    pub step: Step,
    /// The view seen at the last frame: a view changed by anything but a step leaves Import / Classify.
    last_view: Option<ViewMode>,
    /// The first frame has chosen the step (an empty library opens on Import).
    step_chosen: bool,
    /// The home screen (Import step).
    pub home: crate::home::State,
    /// The Quick sort panel (Sort step).
    pub quick: crate::quick_sort::State,
    /// The Classify bench.
    pub classify: crate::classify::State,
}

impl Default for Workspaces {
    fn default() -> Self {
        Workspaces {
            prefs: Prefs::default(),
            path: None,
            current: Space::Library.name().into(),
            kind: Space::Library,
            seen: None,
            dirty: false,
            written_at: f64::NEG_INFINITY,
            failing: false,
            create: Default::default(),
            context: None,
            selector_in_top_bar: true,
            step: Step::Sort,
            last_view: None,
            step_chosen: false,
            home: Default::default(),
            quick: Default::default(),
            classify: Default::default(),
        }
    }
}

impl Workspaces {
    fn entry(&self, name: &str) -> Option<&Saved> {
        self.prefs.saved.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Every space, built-in first, then the user's own in the order they were made.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = Space::ALL.iter().map(|s| s.name().to_string()).collect();
        for s in &self.prefs.saved {
            if !v.iter().any(|n| n.eq_ignore_ascii_case(&s.name)) {
                v.push(s.name.clone());
            }
        }
        v
    }

    pub fn is_builtin(name: &str) -> bool {
        Space::from_name(name).is_some()
    }

    /// The canonical spelling of an existing space's name.
    pub fn find(&self, name: &str) -> Option<String> {
        self.names().into_iter().find(|n| n.eq_ignore_ascii_case(name.trim()))
    }

    /// What `name` is (a custom space remembers the kind it was made from).
    pub fn kind_of(&self, name: &str) -> Space {
        Space::from_name(name).or_else(|| self.layout_of(name).map(|l| l.space)).unwrap_or_default()
    }

    /// The layout to open `name` with: the remembered one, else the factory's (or, for a space of
    /// the user's own with an unreadable entry, its kind's factory layout).
    pub fn layout_of(&self, name: &str) -> Option<Layout> {
        let from_saved = self.entry(name).and_then(|s| serde_json::from_value::<Layout>(s.layout.clone()).ok());
        from_saved.or_else(|| Space::from_name(name).map(factory))
    }

    /// The layout Reset goes back to.
    fn origin_of(&self, name: &str) -> Layout {
        if let Some(space) = Space::from_name(name) {
            return factory(space);
        }
        self.entry(name).and_then(|s| s.origin.clone()).and_then(|o| serde_json::from_value(o).ok()).unwrap_or_default()
    }

    /// Remember `layout` as the last one of `name` (built-in spaces equal to their factory layout
    /// are not stored at all).
    fn store(&mut self, name: &str, layout: &Layout) {
        let value = serde_json::to_value(layout).unwrap_or_default();
        if Space::from_name(name).is_some_and(|s| *layout == factory(s)) {
            self.prefs.saved.retain(|s| !s.name.eq_ignore_ascii_case(name));
        } else if let Some(s) = self.prefs.saved.iter_mut().find(|s| s.name.eq_ignore_ascii_case(name)) {
            s.layout = value;
        } else {
            self.prefs.saved.push(Saved { name: name.to_string(), layout: value, origin: None });
        }
        self.dirty = true;
    }
}

/// Whether the feature is on (Settings ▸ Workspaces).
pub fn enabled(app: &LightcraftApp) -> bool {
    app.ui.settings.workspaces
}

/// The current layout of the shared panel state, as a layout of `kind`.
pub fn capture(app: &LightcraftApp, kind: Space) -> Layout {
    let u = &app.ui;
    Layout {
        space: kind,
        // Compare, Survey and Reference need a pair of photos picked at the moment: a space opens on Detail instead
        view: match u.view {
            ViewMode::Compare | ViewMode::Survey | ViewMode::Reference => ViewMode::Detail,
            v => v,
        },
        left_panel: u.left_panel,
        right: u.right,
        presets: u.presets,
        filmstrip: u.filmstrip,
        filter_bar: u.filter_bar,
        left_width: u.left_width,
        right_width: u.right_width,
    }
}

/// Put `layout` on the shared panel state. Touches nothing of the session: not the selection, not the
/// filter, not the source.
pub fn apply(app: &mut LightcraftApp, layout: &Layout) {
    let has_photo = app.session.active().is_some();
    let u = &mut app.ui;
    u.view = match layout.view {
        ViewMode::Compare | ViewMode::Survey | ViewMode::Reference => ViewMode::Detail,
        v => v,
    };
    // the loupe of nothing is an empty screen: with no photo to open, the grid shows what there is to pick from
    if u.view == ViewMode::Detail && !has_photo {
        u.view = ViewMode::PhotoGrid;
    }
    u.left_panel = layout.left_panel;
    u.right = layout.right;
    u.presets = layout.presets;
    u.filmstrip = layout.filmstrip;
    u.filter_bar = layout.filter_bar;
    u.left_width = LEFT_WIDTH.clamp(layout.left_width);
    u.right_width = RIGHT_WIDTH.clamp(layout.right_width);
    // leaving a tool behind: a crop or brush from another space must not stay armed
    u.tool.clear();
    u.keyword_painter = None;
    u.fullscreen = false;
    // a view applied by a space is not "a view the user changed" (see `tick`)
    let view = app.ui.view;
    app.workspaces.last_view = Some(view);
}

/// Open the space `name` (built-in or the user's own): remember what the space being left looks like,
/// then apply the target's layout. The session is not touched.
pub fn switch(app: &mut LightcraftApp, name: &str) -> Result<Value, String> {
    let name = app.workspaces.find(name).ok_or_else(|| format!("unknown workspace `{}`", name.trim()))?;
    if name == app.workspaces.current && app.workspaces.seen.is_some() {
        return Ok(json!({"workspace": name, "changed": false}));
    }
    // what the space being left looks like now (the per-frame sync already has it, a script may not have waited a frame)
    let (leaving, kind) = (app.workspaces.current.clone(), app.workspaces.kind);
    let now = capture(app, kind);
    if app.workspaces.seen.as_ref().is_none_or(|s| *s != now) {
        app.workspaces.store(&leaving, &now);
    }
    let layout = app.workspaces.layout_of(&name).unwrap_or_default();
    app.workspaces.kind = layout.space;
    app.workspaces.current = name.clone();
    app.workspaces.prefs.current = name.clone();
    app.workspaces.dirty = true;
    apply(app, &layout);
    // what was really applied (Detail without a photo became the grid): that is not a user change, so it is not remembered
    app.workspaces.seen = Some(capture(app, layout.space));
    Ok(json!({"workspace": name, "changed": true, "view": app.ui.view}))
}

/// Reset: the current space goes back to its factory (or original) layout.
fn reset(app: &mut LightcraftApp) -> Result<Value, String> {
    let name = app.workspaces.current.clone();
    let origin = app.workspaces.origin_of(&name);
    app.workspaces.kind = origin.space;
    if Workspaces::is_builtin(&name) {
        app.workspaces.prefs.saved.retain(|s| !s.name.eq_ignore_ascii_case(&name));
    } else if let Some(s) = app.workspaces.prefs.saved.iter_mut().find(|s| s.name.eq_ignore_ascii_case(&name)) {
        s.layout = serde_json::to_value(&origin).unwrap_or_default();
    }
    app.workspaces.dirty = true;
    apply(app, &origin);
    app.workspaces.seen = Some(capture(app, origin.space));
    Ok(json!({"workspace": name, "reset": true}))
}

/// A trimmed name for a new space, or why it can't be used.
fn new_name(app: &LightcraftApp, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the workspace needs a name".into());
    }
    if let Some(n) = app.workspaces.find(name) {
        return Err(format!("a workspace named `{n}` already exists"));
    }
    Ok(name.to_string())
}

/// Save the current arrangement as a space of the user's own, and move to it.
fn save_as(app: &mut LightcraftApp, name: &str) -> Result<Value, String> {
    let name = new_name(app, name)?;
    let kind = app.workspaces.kind;
    let layout = capture(app, kind);
    let value = serde_json::to_value(&layout).unwrap_or_default();
    // the space being left keeps what it had
    let leaving = app.workspaces.current.clone();
    app.workspaces.store(&leaving, &layout);
    app.workspaces.prefs.saved.push(Saved { name: name.clone(), layout: value.clone(), origin: Some(value) });
    app.workspaces.current = name.clone();
    app.workspaces.prefs.current = name.clone();
    app.workspaces.dirty = true;
    app.workspaces.seen = Some(layout);
    Ok(json!({"saved": name, "workspace": name}))
}

/// Delete a space of the user's own (the built-in ones can't be deleted: Reset them).
fn delete(app: &mut LightcraftApp, name: &str) -> Result<Value, String> {
    let name = app.workspaces.find(name).ok_or_else(|| format!("unknown workspace `{name}`"))?;
    if Workspaces::is_builtin(&name) {
        return Err(format!("`{name}` is a built-in workspace and can't be deleted (Reset Workspace puts it back as it was)"));
    }
    let was_current = name == app.workspaces.current;
    app.workspaces.prefs.saved.retain(|s| !s.name.eq_ignore_ascii_case(&name));
    app.workspaces.dirty = true;
    if was_current {
        // back to Library without remembering the deleted space's arrangement as Library's
        let layout = app.workspaces.layout_of(Space::Library.name()).unwrap_or_default();
        app.workspaces.current = Space::Library.name().into();
        app.workspaces.prefs.current = Space::Library.name().into();
        app.workspaces.kind = Space::Library;
        apply(app, &layout);
        app.workspaces.seen = Some(capture(app, Space::Library));
    }
    Ok(json!({"deleted": name}))
}

/// Retouch the selection: the Retouch space with the selection and the filter exactly as they are.
fn retouch_selection(app: &mut LightcraftApp) -> Result<Value, String> {
    if app.session.selection.ids.is_empty() && app.session.active().is_none() {
        return Err("select the photos to retouch first".into());
    }
    // a selection without an active photo (a rubber band) opens its first photo; the selection stays
    if app.session.active().is_none()
        && let Some(first) = app.session.selection.ids.first().copied()
    {
        app.session.selection.active = Some(first);
    }
    let r = switch(app, Space::Retouch.name())?;
    // Retouch opens on the photo even when this space was last left on the grid
    if app.ui.view != ViewMode::Detail && app.session.active().is_some() {
        app.ui.view = ViewMode::Detail;
    }
    Ok(json!({"workspace": Space::Retouch.name(), "selected": app.session.selection.ids.len(), "switched": r}))
}

/// The step the centre of the Library space shows (`None`: workspaces off, or another space).
pub fn step_of(app: &LightcraftApp) -> Option<Step> {
    (enabled(app) && app.workspaces.kind == Space::Library).then_some(app.workspaces.step)
}

/// Import and Classify take the whole window (their own left, centre and right columns): the usual
/// side panels give way while they are shown.
pub fn owns_window(app: &LightcraftApp) -> bool {
    matches!(step_of(app), Some(Step::Import | Step::Classify))
}

/// Go to a step of the Library space (opening the space first when another one is shown). Sort shows
/// the grid again; nothing of the session (selection, filter, source) is touched.
pub fn set_step(app: &mut LightcraftApp, step: Step) -> Result<Value, String> {
    if app.workspaces.kind != Space::Library || app.workspaces.current != Space::Library.name() {
        switch(app, Space::Library.name())?;
    }
    if step == Step::Sort && !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid | ViewMode::Compare | ViewMode::Survey) {
        app.ui.view = ViewMode::PhotoGrid;
    }
    app.workspaces.step = step;
    app.workspaces.step_chosen = true;
    app.workspaces.last_view = Some(app.ui.view);
    Ok(json!({"workspace": "Library", "step": step.name()}))
}

/// `workspace.*` commands (UI commands: they act on the window, not the library).
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    let arg = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if !enabled(app) {
        return Err("Workspaces are off (Settings ▸ General ▸ Workspaces)".into());
    }
    match id {
        "workspace.library" => switch(app, "Library"),
        "workspace.retouch" => switch(app, "Retouch"),
        "workspace.create" => switch(app, "Create"),
        "workspace.switch" => switch(app, &arg("name").ok_or("workspace.switch: missing `name`")?),
        "workspace.reset" => reset(app),
        "workspace.saveAs" => match p.get("name") {
            Some(n) => save_as(app, n.as_str().unwrap_or("")),
            None => {
                app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
                    title: crate::i18n::tr("Save Workspace As…").to_string(),
                    hint: crate::i18n::tr("Name of the workspace").to_string(),
                    value: String::new(),
                    command: "workspace.saveAs".into(),
                    params: json!({}),
                    key: "name".into(),
                });
                Ok(json!({"dialog": "saveWorkspace"}))
            }
        },
        "workspace.delete" => delete(app, &arg("name").unwrap_or_else(|| app.workspaces.current.clone())),
        "workspace.retouchSelection" => retouch_selection(app),
        "workspace.step" => {
            let name = arg("step").ok_or("workspace.step: missing `step` (import | sort | classify)")?;
            set_step(app, Step::from_name(&name).ok_or_else(|| format!("unknown step `{name}` (import | sort | classify)"))?)
        }
        other if other.starts_with("workspace.home.") => crate::home::command(app, other, p),
        other if other.starts_with("workspace.sort.") => crate::quick_sort::command(app, other, p),
        other if other.starts_with("workspace.classify.") => crate::classify::command(app, other, p),
        "workspace.backToLibrary" => switch(app, "Library"),
        "workspace.clearContext" => {
            // the one explicit way to empty the context: the filter, then the selection
            app.run("library.clearFilter", json!({}))?;
            app.run("library.selectNone", json!({}))?;
            Ok(Value::Null)
        }
        other => Err(format!("unknown workspace command `{other}`")),
    }
}

/// Point the workspaces at `workspaces.json` and, when it exists, reopen the space last used with the
/// layout it had (the first run opens the Library space's factory arrangement).
pub fn attach(app: &mut LightcraftApp, path: Option<PathBuf>) {
    app.workspaces.path = path.clone();
    let Some(mut prefs) = path.as_deref().and_then(Prefs::load) else {
        app.workspaces.prefs.module_layout_revision = 15;
        // first run (no workspaces.json yet): the window opens as the Library space is meant to look, not as
        // `ui.json` left it, or the arrangement of the last session would become the "Library" the user finds later
        if enabled(app) {
            let layout = factory(Space::Library);
            apply(app, &layout);
            app.workspaces.seen = Some(capture(app, Space::Library));
        }
        return;
    };
    if prefs.module_layout_revision < 15 {
        prefs.saved.retain(|s| Space::from_name(&s.name).is_none());
        prefs.module_layout_revision = 15;
        app.workspaces.dirty = true;
    }
    let current = prefs.current.clone();
    app.workspaces.prefs = prefs;
    if !enabled(app) {
        return;
    }
    if let Some(name) = app.workspaces.find(&current) {
        let layout = app.workspaces.layout_of(&name).unwrap_or_default();
        app.workspaces.kind = layout.space;
        app.workspaces.current = name;
        apply(app, &layout);
        app.workspaces.seen = Some(capture(app, layout.space));
    }
}

/// Once a frame: remember the layout the user is working with, and write `workspaces.json` (at most
/// once a second) when something changed.
pub fn tick(app: &mut LightcraftApp, now: f64) {
    // a kit started from the Create panel gets its manifest even if the workspaces were switched off meanwhile
    crate::create_space::tick(app);
    // the sliced analysis (Sorting Help's too) goes on whatever the workspaces switch says
    crate::quick_sort::tick(app, now);
    if !enabled(app) {
        // off: the window is the official one; forget what was seen so turning it on starts clean
        app.workspaces.seen = None;
        return;
    }
    step_tick(app);
    crate::home::tick(app, now);
    crate::classify::tick(app, now);
    let kind = app.workspaces.kind;
    let layout = capture(app, kind);
    match &app.workspaces.seen {
        None => app.workspaces.seen = Some(layout),
        Some(seen) if *seen != layout => {
            let name = app.workspaces.current.clone();
            app.workspaces.store(&name, &layout);
            app.workspaces.seen = Some(layout);
        }
        Some(_) => {}
    }
    flush(app, now, false);
}

/// Once a frame: an empty library opens on Import; a view changed by anything but a step (View ▸ People,
/// the loupe…) leaves Import and Classify, because the centre would not show it.
fn step_tick(app: &mut LightcraftApp) {
    if !app.workspaces.step_chosen {
        app.workspaces.step_chosen = true;
        if app.workspaces.kind == Space::Library && !app.session.catalog.photos().any(|p| p.in_library()) {
            app.workspaces.step = Step::Import;
        }
    }
    let view = app.ui.view;
    if let Some(prev) = app.workspaces.last_view
        && prev != view
        && app.workspaces.step != Step::Sort
    {
        app.workspaces.step = Step::Sort;
    }
    app.workspaces.last_view = Some(view);
}

/// Write `workspaces.json` when it is out of date (and `force`, or a second has passed).
pub fn flush(app: &mut LightcraftApp, now: f64, force: bool) {
    let w = &mut app.workspaces;
    if !w.dirty || !(force || now - w.written_at >= 1.0) {
        return;
    }
    w.written_at = now;
    let Some(path) = w.path.clone() else {
        w.dirty = false;
        return;
    };
    w.prefs.version = 1;
    match w.prefs.save(&path) {
        Ok(()) => {
            w.dirty = false;
            w.failing = false;
        }
        Err(e) => {
            if !w.failing {
                log::warn!("saving workspaces: {e}");
            }
            w.failing = true;
        }
    }
}

/// Whether the centre is the Create space's: the Create panel beside the photos to be used.
pub fn create_center(app: &LightcraftApp) -> bool {
    enabled(app) && app.workspaces.kind == Space::Create && matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid)
}

/// The tool strip (the rail of panel icons) is for the spaces that edit; Create has none.
pub fn hides_strip(app: &LightcraftApp) -> bool {
    enabled(app) && (app.workspaces.kind == Space::Create || owns_window(app))
}

/// `workspaces.json` has something new to write (the home screen's remembered settings).
pub fn mark_dirty(app: &mut LightcraftApp) {
    app.workspaces.dirty = true;
}

/// `ui.inspect` → `workspaces`.
pub fn inspect(app: &LightcraftApp) -> Value {
    let w = &app.workspaces;
    json!({
        "enabled": enabled(app),
        "current": w.current,
        "kind": w.kind,
        "names": w.names(),
        "file": w.path.as_ref().map(|p| p.display().to_string()),
        "context": w.context.as_ref().map(crate::context_bar::Summary::to_json),
        "create": w.create.last_kit,
        "step": w.step,
        "home": crate::home::inspect(app),
        "quick": crate::quick_sort::inspect(app),
        "classify": crate::classify::inspect(app),
    })
}
