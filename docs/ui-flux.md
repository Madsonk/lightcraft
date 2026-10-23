# The flow in the interface: IMPORT → CULL → EDIT → RETOUCH (lot D)

AI culling (`cull.*`, [culler.md](culler.md)) and portrait retouch (`portrait.*`,
[portrait-retouch.md](portrait-retouch.md)) were commands only. This lot gives them a face **inside the
spaces LightCraft already has** (Library ▸ Import / Sort, Develop, [workspaces.md](workspaces.md)); it adds
no new space and no new engine logic. Everything goes through `app.run("cull.…")` / `app.run("portrait.…")`,
so the control channel, the CLI and MCP see exactly what the buttons do.

Code: `crates/ui-egui/src/flow.rs` (state, background work, overlays), `panels/cull.rs`, `panels/portrait.rs`,
hooks in `lib.rs`, `panels/right.rs`, `panels/strip.rs`, `panels/grid.rs`, `menus.rs`, `state.rs`.

## Where each function lives

| Aftershoot step | What the photographer does | Where in LightCraft | Command(s) |
|---|---|---|---|
| IMPORT | bring a card / a folder in | Library ▸ **Import** step (existing home) | `file.addPhotos`, `workspace.home.*` |
| CULL | judge the album | Library ▸ **Sort** step, right sidebar, top: **AI picks** (new section) | `cull.run`, `cull.status`, `cull.cancel` |
| | read the four buckets | the same section: Best shots / Selected / To review / Rejected, each with its count; a click narrows the grid | `cull.list {bucket, filter}` |
| | why this verdict | a coloured dot on every judged thumbnail, reasons on hover; the section shows the active photo's verdict and reasons | `Analysis.verdict`, `cull.explain` |
| | duplicates | section **Duplicates · N groups**: groups, best first, **Keep this one** (Pick it, Reject the others, one undo), **Select** | `cull.list`, `photo.flag`, `library.select` |
| | key faces | section **Key faces**: tick the people who matter (used by the next run), or show only one person | `people.list`, `people.photos {filter}`, `cull.run {keyPeople}` |
| | sort by scene | section **Scenes**: each scene with its four counts, a click narrows the grid to it | `cull.list {scene, filter}` |
| | stars, flags | **Write the proposed stars and flags** (one undo step); the grid, the keys 1–5, P, X stay as they were | `cull.apply` |
| EDIT | open the photo | the journey bar's next action (**Retouch the N picks →**) or step 3, Ctrl+2 | `workspace.retouchSelection` |
| RETOUCH | sliders by group | tool strip ▸ **Portrait** (Shift+T) in the Develop space: Skin, Wrinkles, Face, Mouth and eyes, Hair, Neck, Background | `portrait.list`, `portrait.apply` |
| | before / after | **Preview** (stores nothing) shows two pictures side by side in the panel | `portrait.preview` |
| | undo / redo | **Undo**, **Redo** buttons (Cmd+Z / Cmd+Shift+Z work as ever); **Remove the retouch** | `edit.undo`, `edit.redo`, `portrait.reset` |
| | auto retouch | **Auto retouch** (this photo), **Retouch the selection (N)** / **the album (N)** | `portrait.apply {auto}`, one photo per frame |
| | open eyes | group **Open eyes**: Preview the eyes → before/after + eye openness → **Accept** / **Discard** (level 1 only) | `portrait.openEyes` |
| ALL | one click | **Cull and retouch in one click** (bottom of AI picks, command palette ⌘K, Photo menu) | `flow.oneClick` |
| Help | guided tour, hints | **Guided tour** button, Window ▸ Guided Tour; toasts "next time, try the key…" | `flow.tour` |

New UI commands (`menus.rs`): `panel.portrait` (Shift+T), `flow.cullRun`, `flow.cullCancel`, `flow.oneClick`,
`flow.portraitAuto`, `flow.tour`.

## Wireframes

Library ▸ Sort, right sidebar (top):

```
┌ Sélections IA ───────────────────┐
│ [Juger les photos] [Visite guidée]│
│ Type de séance  [Général      ▾] │
│ Regroupement    ───●──────────── │
│ 24 photos jugées                  │
│ ● Points forts                  3 │  ← click: narrows the grid
│ ● Sélectionnées                11 │
│ ● À réviser                     6 │
│ ● Rejetées                      4 │
│ [Tout afficher]                   │
│ [Écrire les notes et drapeaux …]  │
│ ─────────────────────────────────│
│ ● À réviser   2★ proposée · 48    │  ← active photo
│ • un visage est un peu mou        │
│ ▸ Doublons · 3 groupes            │
│ ▸ Visages clés                    │
│ ▸ Scènes (temps de prise …) · 4   │
│ [Sélection et retouche en un clic]│
├ Traitement par lot ──────────────┤  (existing)
```

Grid cell: a coloured dot top right (blue highlight, green selected, amber review, red rejected); hovering the
thumbnail lists the reasons.

Develop ▸ Portrait (Shift+T):

```
┌ Retouche portrait ───────────────┐
│ [Retouche auto] [Retoucher l'album (24)]
│ ▾ Peau · 2                        │
│    Skin smoothing   ──●────  35   │
│    Blemishes        ───●───  55   │
│ ▸ Rides  ▸ Visage  ▸ Bouche et yeux  ▸ Cheveux  ▸ Cou  ▸ Arrière-plan
│ [Aperçu] [Appliquer] [Remettre à zéro]
│ [Annuler] [Rétablir] [Retirer la retouche]
│ ┌ Avant ──┐ ┌ Après ──┐           │
│ └─────────┘ └─────────┘           │
│ ▸ Ouvrir les yeux                 │
│     [Prévisualiser] → avant/après → [Valider] [Écarter]
```

One click: **confirmation** (what will happen, in numbers: judge N photos → write stars/flags → retouch the
highlights and selected with the default recipe; "originals are never touched, one undo") → progress
(AI Culling, then Portrait retouch, each with **Stop**) → **summary** (judged per bucket, written stars/flags,
retouched / without a face / failed, seconds) with **Undo everything** (one `edit.undo`: the stars, flags and
all the retouch steps were merged into one undo entry) and Close.

## Never freezes

* **Cull**: the engine runs it in its own thread; the UI polls `cull.status` ten times a second. A progress
  panel (bottom left, any space) and the section's bar show `done / total`; **Stop** calls `cull.cancel`.
* **Portrait retouch** (since the "off the UI thread" change): `crates/engine/src/portrait_job.rs` splits the
  work like the gallery and the dust search do. **Plan** on the UI thread (settings, render jobs, the face model:
  microseconds), **compute** on a worker (decode, face mesh, the steps, baking; checks a stop flag before each
  stage, each face and each step), **apply** on the UI thread (store the patch and the stack, one undo step per
  photo; a photo edited meanwhile is skipped, not overwritten). `PortraitRun` drives an album one photo at a time
  (album retouch, one click, Retouche auto, Appliquer), `PortraitPreview` makes Before / After in two stages
  (retouch, then the two renders). **Stop** drops the photo in flight at once (nothing is written for it); the
  worker notices within a step. `portrait.apply` / `.batch` / `.preview` keep their synchronous behaviour for the
  CLI, MCP and tests.
* Still on the UI thread: **Open eyes** (needs the burst's analysis and the reference photo through the
  session) and the phase switch of the one click (`cull.apply` + `cull.list`, about 100 ms).

## French

The French catalog (`locales/fr.json`) has every string of the three new files (the test
`french_covers_every_string_of_our_panels` includes them). Vocabulary: *Sélections IA, Points forts,
Sélectionnées, À réviser, Rejetées, Doublons, Visages clés, Scènes, Retouche portrait, Ouvrir les yeux*.
The verdict reasons come from the culler as English sentences and are translated by `tr`.

## Guided tour and hints

`flow.tour` (Window ▸ Guided Tour, or the **Guided tour** button of AI picks): seven bubbles at the top right
(Import, Cull, Read the verdicts, Duplicates, Edit, Retouch, One click), Back / Next / Skip. `flow_tour_done`
and the hints already shown are saved in `ui.json` (`flowTourDone`, `flowTips`). A hint is a toast, once per
action, only when the action was done with the mouse: "La prochaine fois, essayez la touche P" (keep one of a
duplicate group), the undo key (after writing stars/flags and after a portrait retouch).

## Measured (2026-10-22, 23 phone JPEGs of a family album, debug-optimised build, M4)

* **AI picks**, from the button: 23 photos judged in about 0.3 s (72 photos/s through the window); 2 best shots, 9
  selected, 0 to review, 12 rejected; 2 duplicate groups (one of 11 photos); 3 scenes. The window answers while it
  runs (the judging is the engine's own thread).
* **One click** on the same album: 25 s end to end (judge, write stars and flags, retouch the 11 highlights and
  selected photos: 9 retouched, 2 without a face, 0 failed). Before the change one photo with a face held the UI
  thread for about 2.7 s (worst frame 4.1 s). **After: worst frame 96–148 ms (the phase switch), every frame of the
  retouch itself 11 ms or less.** Stop in the middle of an album: the window answered in under 1 ms (harness) / 78 ms
  (running app, control round trip included) and nothing was written.
* **Running app**, album retouch of 23 photos (21 with a face, 57 s): 835 control round trips during the run,
  median 1.7 ms, p95 4.9 ms, 92 fps; the worst single frame of the app's life stayed at about 250 ms (220 ms
  before the retouch, at import). Evidence: `lot-d-gel-progression-retouche.png` in the session notes (cropped
  to the progress panel: the originals' faces are not kept).
* Portrait panel on a real face: Auto retouch gives 10 steps, the sliders then show the stored amounts
  (Skin 35 / 55, …), Preview shows Before / After in the panel (screenshot taken headlessly).
* Screenshots: the running app (French) for AI picks and the portrait panel; the headless harness for dialogs,
  the one-click summary and Before / After (the app's headless snapshot paints user textures white, a windowed
  one was not available while the display was off). Kept outside the repo (family photos).

## Tests

`tests_flow.rs` (5 + 1 ignored): judging in the background and narrowing the grid, Stop writes nothing, one click
(confirm → run → summary → Undo everything restores stars and flags), the portrait panel, the tour and the hint.
`real_album_flow` (ignored) runs the whole flow on `LIGHTCRAFT_FLOW_DIR` and saves screenshots. The French catalog
test covers the three new files. `cargo test -p lightcraft-ui-egui`: 466 passed. One older test
(`tests_v12` quick sort) had to scroll the sidebar to its button, now lower in the sidebar.

Other languages: the 21 new format messages are in every `*-formats.json` (the build needs them) with the English
text for de, es, ja, pt-br, ru, zh; only French is translated.

## Limits (honest list)

* **Open eyes still runs on the UI thread** (a second or two on a real photo), and the one-click phase switch
  holds one frame for about 100 ms. Single frames up to about 250 ms were seen in the running app.
* No separate IMPORT · CULL · EDIT · RETOUCH tab strip in the title bar: the existing spaces (Library with its
  Import / Sort / Classify steps, Develop, Create) are reused as asked. The context bar already has the
  Import and Sort chips.
* Duplicates: "keep this one" uses Pick / Reject flags; there is no side-by-side compare of the group yet
  (use Select, then Survey `N`).
* Key faces need named people (People view); photos where none of them appears are only flagged at the next
  run (`NoKeyFace` warning).
* The scene list filters the grid by scene; the grid itself is not re-sorted by scene (sort is the library's).
* Open eyes: level 1 only (needs a burst with an open-eyed frame of the same person); levels 2 and 3 are not in the UI.
* The background slider needs the subject model; without it the panel shows the engine's error.
