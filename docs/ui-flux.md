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
| | read the four buckets | the same section: Highlights / Selected / To review / Rejected, each with its count; a click narrows the grid | `cull.list {bucket, filter}` |
| | why this verdict | a coloured dot on every judged thumbnail, reasons on hover; the section shows the active photo's verdict and reasons | `Analysis.verdict`, `cull.explain` |
| | duplicates | section **Duplicates · N groups**: groups, best first, **Keep this one** (Pick it, Reject the others, one undo), **Select** | `cull.list`, `photo.flag`, `library.select` |
| | key faces | section **Key faces**: tick the people who matter (used by the next run), or show only one person | `people.list`, `people.photos {filter}`, `cull.run {keyPeople}` |
| | sort by scene | section **Scenes**: each scene with its four counts, a click narrows the grid to it | `cull.list {scene, filter}` |
| | stars, flags | **Write the proposed stars and flags** (one undo step); the grid, the keys 1–5, P, X stay as they were | `cull.apply` |
| EDIT | open the photo | context bar **Develop active photo →** (existing), Ctrl+2 | `workspace.retouchSelection` |
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
* **Album retouch and one click**: one photo per frame through `portrait.apply`, with a progress panel and
  **Stop** between two photos. Stopping keeps what is done (still one undo step).
* **Preview / apply of one photo**: runs on the UI thread (the engine session is not shared with workers);
  the window shows "Calcul en cours…" for a frame before it starts. See limits.

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

## Limits (honest list)

* No separate IMPORT · CULL · EDIT · RETOUCH tab strip in the title bar: the existing spaces (Library with its
  Import / Sort / Classify steps, Develop, Create) are reused as asked. The context bar already has the
  Import and Sort chips.
* One photo's preview / apply runs on the UI thread: a large raw with several faces can hold the window for a
  couple of seconds (measured numbers in `portrait-retouch.md`); an album is chunked so Stop answers.
* Duplicates: "keep this one" uses Pick / Reject flags; there is no side-by-side compare of the group yet
  (use Select, then Survey `N`).
* Key faces need named people (People view); photos where none of them appears are only flagged at the next
  run (`NoKeyFace` warning).
* The scene list filters the grid by scene; the grid itself is not re-sorted by scene (sort is the library's).
* Open eyes: level 1 only (needs a burst with an open-eyed frame of the same person); levels 2 and 3 are not in the UI.
* The background slider needs the subject model; without it the panel shows the engine's error.
