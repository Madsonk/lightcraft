# Workspaces, the context bar and the Create kit (`workspace.*`, `create.kit`)

LightCraft v11. Three **spaces** (Library, Retouch, Create) in the top bar, one **context bar** under
it, and a **Create** space that makes a web gallery, a slideshow, or a **kit** for the layout apps.
All of it can be switched off (Settings ▸ General ▸ Workspaces): off, the window is the classic one
(tool strip, no selector, no context bar, no `workspace.*` commands, no Window ▸ Workspaces).

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
