# Library steps: Import, Sort, Classify (LightCraft v12)

The Library space (see `workspaces.md`) has three steps, chosen in the context bar: **Import** (the home
screen), **Sort** (the grid and the Quick sort panel) and **Classify** (the archive bench). They are only
ways to lay out the centre of the window: changing step never touches the selection, the filter, the
source or the panel layout, and the commands that change the view (View ▸ People, the loupe…) leave
Import and Classify for Sort. With **Settings ▸ General ▸ Workspaces** off none of this exists and File ▸
Import opens the classic review window.

An empty library opens on Import; otherwise the window opens on Sort.

Code: `home.rs`, `quick_sort.rs`, `classify.rs` (new), `tests_v12.rs`; thin hooks in `workspaces.rs`,
`context_bar.rs`, `lib.rs`, `import.rs`, `shortcuts.rs`, `panels/grid.rs`, `menus.rs`.

## Import (home screen)

Three columns inside the centre: **Resume** (where you left off, the last import days with how many photos
are sorted) and the **background tasks**; the **drop zone** (a folder, a card, photos: dropped on the window
or chosen with the buttons) and the **preview** of what the scan found, with the checkboxes of the classic
review; the **settings** (transfer: add in place / copy / move, destination, folders by day or month, album,
preset, keywords) and what to start in the background once the photos are in.

* A scan only lists. The review that used to be a modal window (it dimmed the app until answered) is this
  page: `import::open` scans on a worker thread as before and `import::poll_scan` hands the result to the
  page (`home::capture`) instead of opening `Dialog::Import`. **More options…** opens the classic modal for
  what the page does not show (folder template, DNG, metadata presets, photos in Recently Deleted).
* The blue button runs the existing `import::start`; the import runs on its worker thread and its batches
  join the catalog between frames, exactly as before. The page then opens **Sort on Recently Added**, so the
  first photos are on the grid as soon as their batch is in. The window never blocks: no modal, the task list
  shows the import with a progress bar and **Cancel** (files already added stay).
* The settings are remembered for the next import (`workspaces.json` → `home`): transfer, destination,
  folders, rename template, preset, metadata preset, DNG and the follow-ups. The keywords and the album belong
  to the shoot and are not remembered.
* Follow-ups, each an existing command run on the photos **this import added** (and only those): previews
  (`library.buildPreviews`), bursts and soft photos (`photo.analyze`, in slices, see Quick sort), faces
  (`people.index`, only when the People model is installed), places from the GPS (`places.geotag {apply}`),
  suggested keywords (`keywords.suggest`, slow the first time, nothing is added until accepted). Default:
  previews, bursts and soft photos, places.
* Commands: `workspace.step {step}`, `workspace.home.scan {paths}`, `workspace.home.device {path?}`,
  `workspace.home.settings {mode?, destination?, organize?, rename?, keywords?, album?, preset?, after?}`,
  `workspace.home.import`, `workspace.home.moreOptions`, `workspace.home.clear`,
  `workspace.home.resume {day?}`. `ui.inspect` → `workspaces.home` (candidates, settings, tasks, the last
  import with `firstPhotoMs` and `doneMs`).

## Sort (grid + Quick sort)

The Quick sort panel, to the right of the grid, gathers what LightCraft already finds, as lines with a
count and a **Select** button:

| Line | Where it comes from |
|---|---|
| Rated or flagged: N of M, **Next unsorted**, **Select unsorted** | rating and flag of the photos in view |
| Soft photos | `photo.analyze` focus score below a share of the **median** score of the photos scored (Low 0.35, Medium 0.5, High 0.7): a fixed score would not mean the same on a portrait and a landscape. The threshold is shown. |
| Bursts (groups) · extras, **Stack the bursts** | the burst groups of `photo.analyze` (look-alike shots within 10 s); the best one of each is marked. Stack = `stack.group` per burst, best on top, one undo step. |
| Closed eyes | the faces analysis of `photo.analyzeFaces` (button offered when the face model is installed) |
| Same file | photos of the view with the same content hash |

**Everything only selects.** Nothing is rated, flagged, rejected, stacked or deleted until you press `1`-`5`,
`P`, `X`, `U` (or the three buttons under *This selection*, which act on the selection when clicked), or the
*Stack the bursts* button. The grid cells get the badges *Soft*, *Burst ×n · best* and *Same file* (the
*Eyes closed* badge was already there).

The analysis of a view is cut into slices (one burst, or one photo, per slice: it is cut only at a gap of more
than 10 s in capture time, so no burst is ever cut), one slice per frame; a frame costs the decode of one photo
(about 0.8 s for a Canon CR2 over the thumbnail path, measured on 714 portraits: the window still hitches, once
per photo, instead of freezing for the whole run); it shows in the task list with a Cancel and the
slices are one undo step. The burst of a photo is recomputed from the stored culling groups and the capture
times (the group number alone is per slice).

Commands: `workspace.sort.analyze {ids?}`, `workspace.sort.cancel`, `workspace.sort.sensitivity {value}`,
`workspace.sort.select {what: soft|closedEyes|burstExtras|duplicates|unsorted}`,
`workspace.sort.stackBursts`, `workspace.sort.next`.

## Classify (archive bench)

Left: **Archive health**, counters that open the photos they count (All Photos with that filter, in Sort):
no keyword (`untagged`), no place (`place: none`), no person (no `People` keyword and no named face region),
no rating, **same file** (identical content hash; the count is the photos, the sets are listed). *Classified*
= a person or a place, and a keyword. The numbers are counted straight on the photos of the library (not
deleted, not only browsed) and recomputed when the catalog changes. **Queue** (questions, groups) and the
background tasks.

Centre, four tabs:

* **People**: the **“same person?” queue** (`people.questions`), answered on the keyboard: **Y** yes, **N**
  no, **U** not sure (`people.confirm`; U is also “unflag” elsewhere: on this page it is the queue's). Below,
  the **groups to name**, biggest first: a field and Enter name the group (`people.name`), **Unknown**
  (`people.unknown`), **Skip** (`people.skip`), **Merge into…** (`people.merge`). On the right the faces of the
  open group; click faces to pick them, then **Not them** (`people.move` to unknown) or **Not a real face**
  (`people.ignore`). Without faces read yet the tab offers to read them (background) or to open the classic
  People view for the model's terms.
* **Places**: **Preview places from the GPS** (`places.geotag`, nothing written) then **Apply to N photos**;
  **Suggest places for photos without GPS** (`places.suggest`, time method) then **Confirm the N suggestions
  without conflict** (`places.confirm`).
* **Keywords**: **Suggest keywords** for 200 photos without keyword at a time (`keywords.suggest`, background);
  accept or ignore each keyword, or all (`keywords.accept`).
* **Same file**: the sets of identical files, with *Select the extra copies* (all but the first of each set):
  LightCraft lists and selects, it deletes nothing.

Right, below the faces: **Rename in batch**: a template (`photo.renameTokens`: `{name} {num} {seq} {date}
{folder} {camera} {lens} {iso} {rating} {title} {creator} {ext}`; there is **no place or person tag**), a
preview of the first 12 new names (`photo.renamePreview`) and *Rename…*, which opens the existing Rename
Photos window: files are renamed only there.

Commands: `workspace.classify.tab {tab}`, `.filter {what}`, `.answer {answer}`, `.name {group, name}`,
`.places {action: preview|apply|suggest|confirm}`, `.keywords {action: suggest|acceptAll|ignoreAll}`,
`.rename {template}`, `.refresh`.

## Recognition page (People ▸ Recognition)

Fixes of v12 on the classic page: every group, suggestion and face can now be reached. The lists show a first
page (40 groups, 24 suggestions, 48 faces of a person or group) with **Showing N of TOTAL** and **Load N more**
under each; the **To name / Suggestions / Unknown / People** buttons at the top show one list at a time with
its count; **All faces** on a group lists every face of the group (as **Faces** does for a person); click
faces to pick them, then **Unknown person**, **Not a real face** or **Same person as…** in the bar above the
list (`people.move`) for many faces at once. The “same person?” header says how many questions wait; answered
ones are hidden at once (the engine lists them until the faces are grouped again) and the next ones come in.
**Face thumbnails** are never left as empty dark squares: the crop of the photo's preview when it renders, else
the face cut out of the grid thumbnail, else a loading mark, or a “!” with the reason in its tooltip. Ignored
faces have no list in the engine (only the count of faces left out).

## Limits

* Labels are English only (the app has no French locale); strings go through `tr()` and fall back to English.
* A scan of a very large folder is listed in full but only the first 32 candidates are drawn; the rest is
  imported unless unchecked in *More options…*.
* `places.geotag` and the health counters run on the UI thread (a pass over the photos: milliseconds per
  thousand photos); the heavy runs (analysis, faces, keywords, previews, import) are background tasks.
* The soft-photo threshold is a heuristic (a share of the median), not a calibrated blur detector.
