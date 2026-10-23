# The journey: Import → Sort → Retouch → Export

Madson's brief (23 October 2026): *"il y a déjà beaucoup dans LightCraft mais faut tout améliorer, penser
utilisateur, UI design, un design cohérent, car ça commence à être une usine à gaz avec tout partout ; faire un
processus logique."* Rule: simple at first sight, deep on demand. The audit that motivates this is
[ux-audit.md](ux-audit.md); the tokens are in `crates/ui-egui/src/design.rs`; the bar is `journey.rs`.

## The thread

One bar under the title bar says where you are, how far you got, and what to do next:

```
 ① Import ✓ 30   ② Sort 12/30   ③ Retouch photos 4   ④ Export     Source · filter · selection        [ Retouch the 11 picks → ]
```

* The four steps are the existing spaces and steps, renamed in a photographer's words. Nothing was removed:
  *Import* and *Sort* are the Library's steps, *Retouch photos* is the Retouch space (labelled Développement in the
  old UI), *Export* is the Create space. `Ctrl+1/2/3` and every `workspace.*` command still work.
* The three-space selector of the title bar is gone (a second navigation to the same places). A workspace of
  your own still shows there.
* Each step shows a small figure: photos in the library, photos rated or flagged out of the view, photos edited.
  A finished step turns into a ✓.
* **The next action** is at the right end of the bar and is the one primary (accent) button of the screen.
  Everything else is secondary or quiet. It changes with the state of the work:

| Where you are | What the bar proposes | Why |
|---|---|---|
| any step, library empty | **Import →** | nothing else makes sense |
| Import, a folder scanned | nothing (the import button of the screen is the primary) | never two primaries |
| Import, photos present | **Sort the photos →** | the usual next step |
| Sort, not judged | **Judge the photos** (AI picks, background, Stop) | decide fast; nothing is written |
| Sort, judging | *Judging 12 / 30* (disabled, a progress) | say what is happening |
| Sort, judged, picks exist | **Retouch the N picks →** (selects them, opens Retouch) | the point of sorting |
| Sort, nothing kept | **Retouch photos →** (the active photo) | do not dead-end |
| Retouch | **Export →** | the end of the line |
| Export | nothing (its panel has its own *Create*) | |

## One main place per function

Rule: a function has one main place; other entries are shortcuts to it. Nothing was deleted.

| Function | Main place | Shortcuts (kept) |
|---|---|---|
| Find what to keep | Sort ▸ **AI picks** (right sidebar, first section) | Photo ▸ Assisted Culling, *More tools ▸ Sorting help*, the *Suggested photos* filter, the bar's *Judge the photos* |
| Sort by hand | the grid, keys `1–5`, `P/X/U`, the bottom bar's stars and flags | *More tools ▸ This selection* |
| Batch actions, series, keywords, metadata | Sort ▸ **More tools** (closed until asked: `ui.more_tools`) | Window menu, Info/Keywords strip tools in Retouch |
| Develop (light, colour, detail, optics) | Retouch ▸ strip **Develop** | `E`, Window menu |
| Crop, Remove, Masking, Red eye | Retouch ▸ their strip tools | keys `C H M` |
| Portrait retouch | Retouch ▸ strip **Portrait** (sliders by group, Before / After, auto, open eyes) | the *Natural skin recipe…* button of its panel (Light / Medium / Strong, copies), `Shift+N`, Photo menu |
| Versions, history | Retouch ▸ **left column** | `Shift+V`, `Y`; the strip shows them only in the classic window |
| Presets | Retouch ▸ left column | strip *Presets* (classic window), Quick Develop (More tools) |
| Archive health, people, places | Library ▸ **Archive** (one quiet button of the bar) | View ▸ People, Places |
| Export, gallery, slideshow, kit | **Export** step | File ▸ Export… |

## What you see by default, per step

| Step | Left | Centre | Right (one main panel) | Behind "More / Advanced" |
|---|---|---|---|---|
| **Import** | resume, background tasks | drop zone, the scanned files | import settings; **the import button** | *More options…* |
| **Sort** | sources (albums, folders, dates) | the grid, filter bar | **AI picks**: judge, four buckets, why, duplicates, key faces, scenes, write stars and flags | *More tools* |
| **Retouch photos** | navigator, presets, versions, history | the photo | the tool you picked in the strip (Develop by default) | Develop by task / by type, section folding |
| **Export** | sources | the photos | the Create panel (gallery, kit, slideshow) | kit target, edge size |

## Wording and states

* French first, in a photographer's words: *Importer, Trier, Retoucher, Exporter*; *Juger les photos*;
  *Points forts / Sélectionnées / À réviser / Rejetées*; *Plus d'outils*; *Archive*.
* Empty states say what to do: an empty library makes the bar offer **Import →**; AI picks says "No verdicts yet.
  Press Judge the photos: nothing is written to your ratings"; `design::empty_state` gives the pattern
  (title, one line of help, at most one action).
* Errors name the way out ("No closed eyes found in this photo", "Move a slider first, or press Auto retouch").
* Loading is always cancellable and never freezes the window (cull and portrait retouch run on workers).

## Design system (summary; `design.rs`)

One accent (blue); three button kinds (primary: once per screen, secondary, quiet); spacing 4 / 8 / 12 / 16 / 24;
radii 4 / 6 / 10; type 11 / 12.5 / 14 / 16; five states (ok, warning, error, info, busy) with colour and glyph;
buttons 26 pt high, registered as `button:{id}`. Screens touched by the refonte use it; older screens keep
`widgets::text_button`, which draws the same secondary look.

## Before / after (same 30-photo CR2 library, same window, measured on the running app)

| | Before | After |
|---|---:|---:|
| Navigation levels / places (title bar spaces + Library chips + "Develop active photo") | 2 levels, 6 places, 3 ways to change space | **1 bar, 4 steps** (+ "Archive", one quiet button) |
| Menus / items / submenus (unchanged: nothing removed) | 6 / 263 / 20 | 6 / 263 / 20 |
| Tool-strip tools | 12 (Retouch space: 12) | 11 (Retouch space: **9**: History, Versions, Proposals are in the left column / a button of Portrait) |
| Primary (blue) buttons on the Sort screen | 3 | **1** (the bar's next action) |
| Visible buttons + icons: Import / Sort / Classify | 39 / 61 / 38 | 34 / **49** / 36 (+6 when "More tools" is open) |
| Visible buttons + icons: Develop / Develop ▸ Crop / Create | 83 / 99 / 29 | 74 / 90 / 29 |
| Right sidebar of Sort, closed by default | AI picks + Batch processing + Sorting help + Quick Develop + People recognition + Keywords + Description + Metadata | AI picks; everything else under **More tools** |
| "Help me sort" places | 5 | 1 main (AI picks), 4 shortcuts |
| Portrait places / engines | 5 / 2 | 1 main (Portrait) + 1 button to the recipe / 2 |
| UI clicks, shoot → sorted → retouched → exported | **9** (+5 dialog/key actions) | **8**: import 2 (+2 dialog), then **4 on the same button at the same place** (*Sort the photos →*, *Judge the photos*, *Retouch the 10 picks →*, *Export →*), then 1 (*Create the web gallery*) (+1 folder dialog) |
| Screens that say what to do next | 0 | every step |
| English strings seen on the Import / Sort / Export screens (French UI) | ~25 | 0 on those three screens |
| Button looks on the touched screens | 3 | 3 kinds, one 26 pt height |

The click count falls by one and the dialog/key actions by four, but the real gain is that every one of the four
journey clicks is "the blue button at the right end of the bar". A beginner never has to look for the next step.

## Beginner's walk (what still gets in the way)

Run on the app, French UI, from an empty library to a gallery. What worked: the bar always named the next step;
the Sort sidebar showed one panel; the Export screen had one blue button. What is left, honestly:

1. **Import still needs the OS folder dialog** (2 actions) and there is no drag-and-drop proof from the walk.
2. **The Retouch step does not say what to retouch.** The bar says *Export →* even if nothing was edited; the Develop
   panel (by task, by type) is as dense as before, and the strip still has 9 tools. A "start with a preset" hint
   belongs here.
3. **AI picks is long** (judge, four buckets, why, duplicates, key faces, scenes, apply, one click): three of its sub-
   sections should fold until there is something in them.
4. **The two-row filter bar** above the grid is on by default in Sort.
5. **Archive (Classify) was not reworked**, and the legacy *Quick sort* is hidden, not merged, in More tools.
6. **Design tokens are used on the screens touched**: the other panels (Develop sliders, Masking, Info…) still
   draw with `text_button`, and some English strings remain there (not audited).
7. **A 30-photo library hides the cost of the long sidebar**: with 27 000 photos the Sort figure counts only the view.
