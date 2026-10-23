# UX audit of LightCraft before the refonte (23 October 2026)

Method: the running app (build of 81a666f0), a throwaway library of **30 Canon CR2** from a food shoot (no faces),
French UI, window 1600 × 950, driven through the control channel (`ui.widgets`, `ui.menu.tree`,
`engine.commands`, `ui.screenshot`). "Visible controls" = widgets the app registered on screen for that
screen (buttons, icons, sliders, stars, flags, filters, thumbnails…). Screenshots are kept outside the repository.

## 1. The numbers

| What | Count |
|---|---:|
| Engine + UI commands (`engine.commands`) | **632** |
| Top-level menus | 6 (File, Edit, View, Photo, Window, Help) |
| Menu items / submenus | **263 / 20** (the View menu alone carries 40+) |
| Spaces (title bar) | 3 (Library, Develop, Create) + the Library's 3 steps (Import, Sort, Classify) = **two navigation levels, six places** |
| Tool-strip tools (right edge) | 12: Presets, Develop, Crop, Remove, Masking, Red Eye, Proposals, Portrait, Versions, History, Keywords, Info |

Visible controls per screen (all kinds, from the registry):

| Screen | All widgets | Buttons + icons | Remark |
|---|---:|---:|---|
| Library ▸ Import | 63 | 39 | 18 icons are the chrome, 21 buttons |
| Library ▸ Sort | 188 | **61** | 20 thumbnails, 13 filters, 7 dropdowns |
| Library ▸ Classify | 58 | 38 | |
| Develop (no panel) | 137 | **83** | 48 buttons |
| Develop ▸ Crop | 155 | **99** | the busiest screen of the application |
| Develop ▸ Remove / Masking / Proposals / Portrait | 124–135 | 78–83 | |
| Develop ▸ Versions / Keywords / Info | 117–131 | 77–80 | |
| Create | 136 | 29 | 17 album/source rows |

Rule of thumb from the study of 10 October: a screen that offers more than ~7 competing actions needs one of them
to say "start here". None of these screens does.

## 2. The same function in several places

| Function | Where it lives today | Places |
|---|---|---:|
| "Help me sort" | Photo ▸ Assisted Culling (dialog), the context bar's *Sorting Help*, the Quick sort panel (Library sidebar), the *Suggested photos* filter, **AI picks** (lot D) | **5** |
| Open the Develop tool | tool-strip icon, five buttons repeated at the top of the Develop sidebar (Develop, Crop, Masking, Location, Proposals), Window menu, a key | 4–5 |
| Portrait retouch | strip *Portrait* (sliders), strip *Proposals* (recipe "Natural skin"), the sidebar button *Portrait Retouch* (masks engine), Photo ▸ Portrait Retouch, one-click | **5, on two different engines** |
| Presets | left column (Develop), strip *Presets*, Quick Develop combo (Library) | 3 |
| History / Versions | left column *Historique* + *Versions*, and again as strip tools | 2 × 2 |
| Export | File ▸ Export…, the Create space, the share icon of the title bar | 3 |
| Import | File menu (3 items), the Import step, the "resume" banner's *Import photos…* | 3 |
| "Develop" | the *space* is called Développement, so is the tool and its panel | 3 meanings |

## 3. The journey, as it was ("I have a shoot, I sort, I retouch, I export")

Measured on the 30-photo library, UI clicks only (keys and OS dialogs counted apart):

| # | Step | Clicks |
|---|---|---:|
| 1 | Choose a folder (button) — OS dialog: 2 actions | 1 (+2) |
| 2 | "Import N photos and open Sort" | 1 |
| 3 | AI picks: "Judge the photos" | 1 |
| 4 | Narrow to the kept: *Selected* bucket | 1 |
| 5 | select all (⌘A), "Develop active photo →" | 1 (+1 key) |
| 6 | pick a preset in the left column | 1 |
| 7 | "Create →" | 1 |
| 8 | pick the kind (gallery / kit), folder (OS dialog), "Create" | 2 (+2) |
| | **Total** | **9 clicks + 5 dialog/key actions, 4 screens of 29–99 controls** |

At no point does the application say which button comes next: three blue buttons compete on the Sort screen
("Judge the photos", "Cull and retouch in one click", "Develop active photo →") and two on Import.

## 4. Visual and wording inconsistencies (from the screenshots)

1. **Three button looks**: filled blue, dark bordered, and borderless chip-like rows; heights 24 and 26; radii 4 and 5.
2. **Mixed languages on one screen** (French UI): Import shows *Resume*, *Where you left off*, *Open in Sort*, *Background
   tasks*, *Import settings*, *While you start sorting (in the background)*, and the banner *Welcome back…, Resume editing, Hide*;
   Sort shows *Quick sort*, *Rated or flagged: 0 of 30*; Classify *Archive health*, *Queue*, *Rename in batch*.
3. **The tool strip cuts its labels**: "Développement" shows as "eloppemen", "Préréglages" as "réréglage".
4. **Margins**: panel titles start at 24 pt, the content under "Batch processing" at 0 pt ("Photos sélectionnées : 1" is
   flush against the edge while the AI picks section above it is indented).
5. **Title sizes** 14 / 15 / 16 pt for the same level (`home::heading`, `right::header`, section headers).
6. **Blank right panel** in Develop when no tool is open: a 300 pt empty column, no hint.
7. **Verdict marks vs older badges**: the *Soft* label (text on the thumbnail) and the new verdict dot say related things in
   two styles.
8. **Two navigations in two bars**: the space selector (title bar) and the step chips (context bar), plus the "Develop active
   photo →" button that is a third way to change space.

## 5. What this means for the refonte

* The structure is already right (Library → Develop → Create, three Library steps); what is missing is **a thread**
  (where am I, what is done, what comes next) and **one main place per function**.
* Most of the weight is repetition (sections 2 and 4), so most of the work is removing and demoting, not adding.
* See [ux-flow.md](ux-flow.md) for the journey and [design.rs](../crates/ui-egui/src/design.rs) for the tokens.
