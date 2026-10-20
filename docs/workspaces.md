# Workspaces, the context bar and the Create kit (`workspace.*`, `create.kit`)

LightCraft v11. Three **spaces** (Library, Retouch, Create) in the top bar, one **context bar** under
it, and a **Create** space that makes a web gallery, a slideshow, or a **kit** for the layout apps.
All of it can be switched off (Settings ▸ General ▸ Workspaces): off, the window is the classic one
(tool strip, no selector, no context bar, no `workspace.*` commands, no Window ▸ Workspaces).

## Product scope agreed with Madson — 2026-10-12

This is the requested target behaviour, not a claim that every capability below is implemented.

- **Library = organise and process a batch.** Classification, face/person recognition across the explicit selection, metadata, and batch development belong here. A series from the same place and moment can share a preset or LUT; harmonisation must also compensate individual exposure/white-balance differences rather than merely copy identical slider values. Show the target photo count, preview the result, and keep application undoable.
- **Retouch = work on the active photo.** Keep all tools for that photo reachable together: detected people and editable detection regions, crop, colour controls, AI skin retouch, AI colour harmonisation, and location metadata. Correcting a detection region is separate from cropping the photograph. Browsing the filmstrip changes the active photo; editing defaults to that photo alone. Applying or syncing to others requires an explicit batch action with visible targets.
- Preserve the selection and filter when switching spaces; use the existing shared command engine and tools. Workspace changes must clarify the scope rather than duplicate processing engines.

Implementation backlog and the deferred Detectron2 experiment are tracked in [ROADMAP.md](../ROADMAP.md).

## v15: Bibliothèque / Développement (2026-10-12)

The built-in saved ids and command ids remain `Library` and `Retouch` for compatibility; the French UI shows **Bibliothèque** and **Développement**. Both factory layouts have a filmstrip spanning the window beneath both sidebars. The grid, navigator and filmstrip reuse the largest thumbnail bucket of the current photo snapshot, avoiding repeated renders when their sizes differ. The built-in layouts migrate once to the requested module arrangement; custom workspaces remain intact.

Bibliothèque groups the histogram, sorting assistance, Quick Develop, keywords and metadata in one right sidebar. Quick Develop applies relative exposure/temperature/tint changes to the explicit selection, presets and imported LUT profiles to the selected photos, and offers explicit colour sync and EXIF-based exposure matching. Palette harmonisation previews each selected photo against the active photo before application; a selection guard rejects a stale preview. Missing EXIF limits exposure matching; palette harmonisation adjusts the colour mixer, not white balance. No claim of automatic lighting/scene grouping or Lightroom rendering parity.

Développement has the active-photo navigator, presets, versions and history on the left, the photograph in the centre, and its settings on the right. People, detection-region handles, crop, masks, location metadata, portrait skin retouch and colour harmony stay reachable from the right sidebar. Preset application, reset, paste, metadata, detection and retouch defaults are restricted to the active photo. Proposals also target that photo. Sync Settings explicitly copies settings to the rest of the selection. Face-based AI actions require their existing optional models; this delivery does not install Detectron2 or additional weights.

## The three spaces

A space is a **saved arrangement of the panels that already exist**: the centre view, the left
sidebar, the right panel, the Presets column, the filmstrip, the filter bar and the panel widths.
Nothing else: the library filter, the source, the selection and the active photo live in the session
and are **never touched by a change of space**.

| Space | Shortcut | Factory arrangement |
|---|---|---|
| **Library** | `Ctrl+1` | photo grid, sources on the left, filter bar, Keywords on the right. Two steps: **Sort** (grid) and **Classify** (People / Places), plus **Import…** and **Sorting Help** in the context bar |
| **Retouch** | `Ctrl+2` | the photo (Detail), Edit on the right, filmstrip below, no sources column; Versions (with Proposals) one click away |
| **Create** | `Ctrl+3` | the photos on the grid, sources on the left, the **Create panel** on the right of the centre; no tool strip |

(`Ctrl` is the Control key on macOS: `⌘1…5` are still the Edit sections and `1…5` the ratings.)

The last arrangement of each space is **remembered as you work** and written to
`<config>/workspaces.json` (beside `ui.json`; not written with `--memory` or `LIGHTCRAFT_NO_PREFS`).
The space in use reopens at launch with its arrangement; the first run keeps the window as `ui.json`
restored it. The file has FilmCraft's shape (`current` plus a list of `{name, layout}`, each layout kept as
JSON so an entry written by a newer version does not lose the others, plus `origin` for a space of your own).

Window ▸ **Workspaces**: the three spaces and your own (the open one checked), **Reset Workspace**
(the factory arrangement, or the one a space of your own was saved with), **Save Workspace As…** (a
space of your own, from the current arrangement, listed after the three), **Delete Workspace** (spaces
of your own only). The same commands exist for scripts: `workspace.library|retouch|create`,
`workspace.switch {name}`, `workspace.reset`, `workspace.saveAs {name}`, `workspace.delete {name?}`,
`workspace.retouchSelection`, `workspace.backToLibrary`, `workspace.clearContext`.

## The context bar

One line under the top bar: **Source · Filter · N selected of M · Photo**, the same in every space.
In Library it carries **Import…**, **Sort** / **Classify**, **Sorting Help**, and the primary button
**Retouch selection (N) →**, which opens the Retouch space with the selection and the filter as they
are. In Retouch and Create it carries **← Library** (same selection, same filter) and, from
Retouch, **Create →**. **Clear** is the only way to empty the filter and the selection (the active photo stays selected: LightCraft always has one).
`ui.inspect` → `workspaces` reports the current space, its kind and what the bar says.

## Create

The Create panel has three tabs. The photos used are the selection when several photos are selected,
else everything in the current view, in grid order (the rule of `gallery.export`).

* **Web gallery**: title, widths, AVIF; says how many photos have a caption or title, keywords, or
  only a file name for their alternative text; runs `gallery.export` in the background (it asks
  for a folder; location data is left out).
* **Kit**: `create.kit {dir?, name?, title?, target?, longEdge?, ids?}` (File ▸ Export Kit for
  Layout Apps…). Renders the photos to JPEG (quality 92, long edge 1600 / 2400 / 3600 / full size) into
  `<dir>/<name>/images/NNN_<original name>.jpg` in the background, then writes `manifest.json` and
  `README.txt`. A kit always goes in a **new folder** (`name`, `name-2`…). The "Export with Previous"
  memory is left as it was. Nothing is opened in another app.
* **Slideshow**: the existing `view.slideshow`, with the seconds per photo.

`manifest.json` (`kit: "lightcraft-kit"`, `version: 1`): `title`, `name`, `source`, `target`
(`designcraft`, `pdfcraft`, `deckcraft`, `filmcraft`), `hint`, `root`, `longEdge`, `count`, `failed`
and `images`, one entry per image in order: `order`, `file` (relative to the kit), `source` (the
original file name), `title`, `caption`, `altText`, `altSource` (`caption`, `title`, `keywords` or
`filename`), `keywords`, `rating`, `captured`, `width`, `height`.

### What the receiving apps take (read-only check, 10 Oct 2026)

* **DesignCraft 0.5.0**: `designcraft-cli commands` lists `file.new {preset?, width?, height?, pages?,
  facingPages?, columns?, gutter?, margins?, bleed?, title?}`, `file.place {path?, frame?, spread?, x?,
  y?, width?, pdfPage?, …}` (an image into the selected empty frame, else at x/y) and
  `file.exportPdf {path?, pages?, …}`. A script can therefore make a book or a flyer from the manifest
  (one `file.place` per image, in `order`, with `caption` as the text).
* **PdfCraft 0.5.0**: its README and tool list describe PDFs only (open, combine, extract, split,
  edit, comments, forms); no image import is documented. The route for a print PDF is DesignCraft's
  `file.exportPdf`, then PdfCraft.
* **DeckCraft 0.4.0** and **FilmCraft**: not checked. DeckCraft's CLI did not answer within 30 s
  when asked for its command list from a script (`deckcraft-cli commands`, same for `pdfcraft-cli tools`),
  so the slide commands for images are unverified; the README shows `slide.new={"layout","title"}`.

## Rebasing on a newer official LightCraft

All of it is in `workspaces.rs`, `context_bar.rs`, `create_space.rs` and `tests_v11.rs`. The shared
files only carry one-line hooks: `lib.rs` (modules, one field, one call in `logic`, the context bar
after the top bar, the strip and the centre), `state.rs` (one setting), `menus.rs` (commands and two
routes), `menubar.rs` (the Window submenu), `panels/topbar.rs` (the selector) and `panels/settings.rs`
(one check box), `control.rs` (`ui.inspect`), `apps/lightcraft/src/main.rs` (the file path).

## Docking later (not done)

Dragging panel tabs and floating panels are **not** in v11. A space is a flat `Layout` that goes
through two functions only, `workspaces::capture` and `workspaces::apply`. A dock tree (FilmCraft's
`DockNode` copied and made generic) can replace those two and add a `dock` field to `Layout`;
the selector, the context bar, the commands, the menu and `workspaces.json` (which ignores unknown
fields) do not change.

## v14: the Retouch column, Develop by task, Proposals, ⌘K, Window ▸ Panels

Everything below is behind the same Workspaces switch (off = the classic window), except the French
locale (`docs/localization-fr.md`), the command palette and the typed toasts, which are harmless in both.

- **Retouch space, left column** (`retouch_left.rs`): the selection being retouched (count, position,
  previous / next, a strip of its thumbnails, the active one framed), the photo's versions with their
  look as a thumbnail (click restores), its history (click goes back to that step). It replaces the
  Library sources column in the Retouch space; the factory layout of that space now opens it.
- **Develop by task** (`edit_tasks.rs`): the Develop panel header offers *By task | By type*
  (`edit_by_task` in `ui.json`). By task: Base (profile, white balance, exposure, contrast), Light
  (tones, curve, dehaze), Color (vibrance, saturation, mixer, grading, calibration), Detail (texture,
  clarity, sharpening, noise, grain), Optics (lens, geometry, vignette) and Local (the tools that touch
  a part of the photo). Sections fold like the others (`open_sections`, ids `task.*`). The field
  *Search a control…* lists the sliders whose name contains the text, accents optional, in any language.
- **Before / After in one gesture**: a button in the context bar of the Retouch space (`view.beforeAfter`,
  the same as holding `\`).
- **Proposals panel** (`panels/proposals.rs`, `panel.proposals`, Shift+N, a tool of the strip): Before /
  After of the active photo, the impact, one primary button per state (Propose, then Create retouched
  copy), the steps folded under Details. The classic window keeps the section at the top of Versions.
- **Command palette** (`palette.rs`, `app.commandPalette`, ⌘K): every menu command, filtered by name,
  menu path or shortcut; Enter runs, Esc closes. Addressable: `field:commandPalette`, `palette:<id>`.
- **Window ▸ Panels**: `window.panel.left`, `.right`, `.filmstrip`, `.filterBar`, `.histogram`, checked
  when open.
- **Typed toasts**: `toast` (information), `toast_success` (2 s, green), `toast_error` (6 s, red): a colour
  bar and a glyph on the left.
- **Readable strip**: labels at 10 pt in the label colour, group captions semi-bold.

Rebasing: new files `edit_tasks.rs`, `retouch_left.rs`, `palette.rs`, `tests_v14.rs`,
`locales/fr*.json`, `docs/localization-fr.md`. Hooks in `lib.rs` (modules, one field, the left column
and the palette calls, the toast kind), `state.rs` (`ToastKind`, `edit_by_task`, `edit_search`,
`RightPanel::Proposals`), `menus.rs` (commands and routes), `menubar.rs` (`@Panels`, checks),
`panels/right.rs` (two match arms, the scroll bar), `panels/edit.rs` (`profile_row` shared, the
title "Develop", the view toggle, three `pub(crate)`), `panels/strip.rs` (one tool, fonts),
`context_bar.rs` (tooltips with shortcuts, Before/After), `workspaces.rs` (one factory flag),
`panels/mod.rs` (the toast), `i18n.rs` (the `Fr` entry, the system language), `Cargo.toml` (`sys-locale`).
Not done: docking (dragging tabs, floating panels), see above.

## v16: coherent keywords, people and places

The Library sidebar includes **Classification consistency**. **Audit keywords, people and places**
is read-only and separates the vocabulary into three cases:

- a flat keyword that exactly matches a named person becomes `People|Name` and files those photos
  under that existing person;
- a flat place name becomes a `Places|Country|Region|City` path only when the photo metadata, GPS,
  or one unique existing place path proves the mapping;
- descriptive terms such as `agriculture`, `carousel`, `night` or `portrait` remain ordinary
  keywords. LightCraft does not guess that a capitalized word is a person or a place.

The preview lists safe changes and ambiguous names separately. **Apply safe harmonization** accepts
the preview only while the catalog revision is unchanged; otherwise a fresh audit is required.
Ambiguous names always require a manual choice. Commands: `classification.audit` and
`classification.harmonize {expectedRevision?}`.
