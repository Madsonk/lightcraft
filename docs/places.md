# Places

Where were these photos taken? LightCraft turns a photo's GPS position into a place keyword **offline**, lets you give a
place by hand, proposes places for photos that have no GPS, and filters the library by place.

* Data: `crates/places` (reverse geocoding), a separate file `places-cities.bin` made from **GeoNames** (CC BY 4.0).
* Library side: `crates/catalog/src/places.rs` (`Filter.place`, the place tree), `crates/engine/src/places/` and
  `crates/engine/src/cmd/places.rs` (the commands), `crates/ui-egui/src/panels/places.rs` (the view) and the **Place**
  picker of the filter bar.
* Cargo feature `places` of `lightcraft-engine` (the desktop app and the CLI enable it; off for the web build, where
  `Filter.place` still works on keywords).

## A photo's place

A place is a hierarchical keyword, `Places|Country|Region|City`, e.g. `Places|France|New Aquitaine|Poitiers`
(names as GeoNames gives them: cities in their own spelling with accents, countries and regions in English; a level that
repeats the previous one is dropped, so Dakar is `Places|Senegal|Dakar`). It is an ordinary keyword: undoable, written to the
XMP sidecar like the others, visible in the Keywords panel and list.

A photo's place for the **Place filter, the place list and the suggestions** is:
1. its `Places|…` keyword(s) when it has any (what you chose always wins over the GPS), else
2. the place its GPS position resolves to (default radius), else none.

So the filter works on a library that was never geotagged. `places.geotag` writes step 2 into step 1.

## How a position is resolved

The data file holds 171 174 populated places of at least 1 000 inhabitants (GeoNames `cities1000`), their country and
first-level division (region). The city is the biggest of the 16 nearest towns whose built-up area covers the position
(radius 0.35 km per √(thousand inhabitants), at most `maxKm`), else the nearest town (so a photo in central Paris is Paris
and not a district listed on its own; GeoNames' quarter flag is used as a fallback only). The precision then follows the
distance `d` to that place, with `maxKm` = 50 by default:

| `d` | answer | `level` |
|---|---|---|
| ≤ `maxKm` | `Places|Country|Region|City` | `city` |
| ≤ 2 × `maxKm` | `Places|Country|Region` | `region` |
| ≤ 6 × `maxKm` | `Places|Country` | `country` |
| farther (open sea, poles, deserts) | no place | — |

(0, 0) (what cameras write without a fix), NaN, out-of-range positions and the poles give no place. Longitudes of ±180 and
beyond are wrapped, so both sides of the antimeridian find Fiji.

## Commands

| id | what |
|---|---|
| `places.status` | is the data installed (path, provenance, licence), the settings, whether SigLIP can suggest |
| `places.settings {autoGeotag?, maxKm?}` | `autoGeotag`: give new photos their place keyword when imported or added, **off by default**; `maxKm` (1-500, default 50). Saved in `<library>/places/settings.json` |
| `places.lookup {lat, lon, maxKm?}` | reverse geocoding of one position, with the 3 nearest places |
| `places.geotag {ids?, apply?, maxKm?, overwrite?, limit?}` | **preview** (default, writes nothing): photos scanned, without GPS, already placed, resolved, unresolved, counts by place and by level, the first `limit` photos with place and distance. With `apply: true` the same report and the keywords written in **one undo step** (`Geotag N Photos`). Photos that already have a `Places` keyword are skipped unless `overwrite` |
| `places.name {ids?, place}` | give the photos a place by hand (`"France|Poitiers"` or `"Places|France|Poitiers"`), replacing their other place; one undo step |
| `places.clear {ids?}` | remove the place keywords |
| `places.list {inView?}` | the places with photo counts (tree and flat list), photos placed only by GPS, photos without a place |
| `places.suggest {ids?, methods?, windowMinutes?, sameCamera?, k?, minConfidence?}` | proposals for photos with neither GPS nor place (below); nothing is written |
| `places.confirm {ids?, place?}` | write the place (default: the one last suggested for each photo) |
| `places.dismiss {ids?}` | drop the suggestions and stop proposing them this session |
| `library.filter {place}` | filter by place: a keyword path (`Places|France` finds everything below it) or `none` |
| `view.places` (View menu) | the Places view |

`ids` omitted = the whole library for `geotag`, `suggest` and `list`, the selection for `name`, `clear`, `confirm`.
All of it is available through the CLI (`lightcraft-cli run places.geotag apply=true`), the control channel and the MCP server.

## Suggestions for photos without GPS

Only photos with **no GPS and no place** are candidates. Nothing is written until `places.confirm` (or the Confirm
button).

* **By time** (`time`): the nearest photo of the **same camera** (`sameCamera: false` to drop that) taken within
  `windowMinutes` (default 30) that has a place. `confidence = 0.9 × (1 − gap / window)`; if the photo before and the one
  after agree it gets +0.08; if they disagree the nearer one wins, the confidence is halved and the other place is
  reported as `conflict`. Camera clocks differ, so this is only meaningful within one camera.
* **By look** (`visual`): the `k` (7) most similar photos among those that have a place, by SigLIP 2 image embeddings
  (the search index of the library: run `search.index` first; photos not indexed are counted in `unindexed`). Their
  weights are `exp((cos − best)/0.05)`; the answer is the **deepest place holding at least 60 %** of the weight (so
  neighbours from two cities of one region give the region), and `confidence = share × clamp((best cosine − 0.78) / 0.14)`.
  Needs the semantic search model; without it the command says so in `notes` and the time suggestions still come.
* A photo keeps its best suggestion; defaults drop confidences under 0.2 (time) and 0.5 (look).

## Interface

* **View ▸ Places**: the places with photo counts (a click shows that place's photos in the grid; *GPS* marks places known
  only from the GPS), **Preview Geotag** then **Apply to N**, a checkbox *Geotag new photos on import* (off), *Name a place*
  for the selection, and **Find Suggestions** with Confirm / Dismiss per photo and *Confirm all ≥ 80 %*. The GeoNames
  attribution is shown at the bottom.
* **Filter bar ▸ Place** next to Keywords: All places / No place (n) / every place with its count, indented by level; a
  chip *Place: France › New Aquitaine* shows in the grid header and clears.

## Where things are stored

Place keywords: the catalog (like any keyword; no catalog format change). Settings: `<library>/places/settings.json`.
`Filter.place` is a new field of the saved filter / smart-album rules, left out of the JSON when unset, so older filters,
presets and catalogs load unchanged. The data file is **not** compiled in: it is
found at `$LIGHTCRAFT_PLACES_DB` (file or folder), next to the executable (`places/`), in the macOS bundle
(`Contents/Resources/places/`), `../share/lightcraft/places/`, or `~/Library/Application Support/LightCraft/places/`.
`lightcraft-cli run places.status` says which one was used.

## Measurements (2026-10-10, Apple Silicon, GeoNames dump of 2026-10-09)

* Data file: 2 404 676 bytes (171 174 places, 252 countries), loads in 24 ms; 75 000 uncached lookups per second on random
  world points (single thread); a photo already seen at the same ~11 m spot costs a hash lookup.
* **Real archive** (`Adobe/2020…2026`, 2 205 JPEGs with GPS found of about 5 900; Samsung SM-N975F, SM-G965F, SM-N970F,
  Redmi 23106RN0DA, Galaxy S25+): 930 imported by reference (9 failed to decode: damaged JPEGs, a codec limit unrelated to
  places). All **921 resolved at city level**; distance to the named place: median 1.1 km, 90 % under 3.9 km, max 9.9 km.
  Checked by eye against known places: Poitiers (202 photos), Navajas (Spain, the Salto de la Novia, 117), Peníscola, Porto,
  La Flotte and Rivedoux-Plage (île de Ré), Cognac, Orléans, Lyon, Barcelona, Salou, Morella, Saumur, Bordeaux… all
  plausible. Where the nearest town of 1 000 inhabitants is a few km away the *neighbouring* town is named (7 photos at
  (46.330, 0.665) are "Lussac-les-Châteaux" at 9.3 km: no listed town is closer; 28 of 921 photos are named after a town more
  than 5 km away). Two photos with GPS (0, 0) are treated as having none.
* Preview of 921 photos: 28 ms; apply (write + save): 136 ms; undo 150 ms, redo 127 ms. (Importing the 930 files from the NAS
  took 161 s, 5.7 files/s: reading the files dominates.)
* **GPS in files**, read by the existing `crates/meta` and checked against macOS Spotlight (`mdls`): JPEG (Samsung, Xiaomi) ✓;
  HEIC (Samsung): 4 of 19 have GPS, the same 4 and the same coordinates (5 decimals) as Spotlight; DNG: the one file
  with GPS is read (Navajas); 38 iPhone HEIC from an iCloud export, 20 CR2 and 8 NEF: no GPS, agreeing with Spotlight. No
  CR2/CR3 with GPS was available, so raw GPS is only covered by the synthetic tests of `crates/meta`.
* **Suggestions by time**, hold-out of a random 20 % of the geotagged photos (184; their GPS and place removed, the others
  are the anchors): 127 got a suggestion, **126 right (99 %)**, 1 wrong (a neighbouring suburb). Hiding whole days
  (104 photos) leaves no neighbour within 30 minutes, so nothing is suggested, as designed.
* **Suggestions by look** (SigLIP 2, 923 photos indexed in 245 s; suggestions for 184 photos take 6 ms):
  * random 20 % hidden (a burst's neighbours stay): confidence ≥ 0.8: **74 of 75 exactly right**, 1 right at a coarser level;
    0.5-0.8: 23 of 31 exact, 8 coarser, 0 wrong; below 0.5 (dropped by default): 28 % exact, 19 % wrong.
  * whole days hidden (nothing similar from the same day): nothing reaches 0.8; 0.5-0.8: 3 of 8 exact, 3 coarser, 2 wrong;
    below 0.5: 6-9 % exact, 62-76 % wrong. **In other words: it recognises the same place on the same outing, not a place
    seen in other days' very different photos.** Keep the confirmation click.
* Tests: places crate 8 + 1 bench (ignored), engine 8 new, ui-egui 4 new, catalog 2 new (see the CI log of the commit).

## Limits

* Only places of 1 000 inhabitants or more (GeoNames `cities1000`; `cities500`/`cities5000` can be built the same way). A
  hamlet is named after the nearest town; Lightroom-style street or landmark names do not exist here.
* No country outlines: the country and region of a position far from any town (the 50-300 km zone) is that of the nearest town,
  which can be wrong near a border or for a boat in a strait. Hence the coarser `level` and distances reported.
* Names are GeoNames' (English countries and regions: "New Aquitaine", "Brittany", "Reunion"); no translation. Overseas
  territories are GeoNames "countries" (Reunion, Mayotte, Guadeloupe…): `Places|Reunion|Saint-Denis`, not under France.
* The IPTC `Location / City / State / Country` fields are not filled; only the keyword is. No map.
* Time suggestions assume one camera's clock; GPS from a phone cannot place a camera's photos unless the clocks agree.
* Look-alike suggestions need the SigLIP model (about 1.5 GB, downloaded on request: `docs/semantic-search.md`).

## Licence

Place data from **GeoNames** (<https://www.geonames.org/>), licensed under **Creative Commons Attribution 4.0**
(<https://creativecommons.org/licenses/by/4.0/>). Required mention: *"Place data from GeoNames (https://www.geonames.org/),
CC BY 4.0, converted by LightCraft."* — in `NOTICE`, `assets/ATTRIBUTION.md`, the Places view and
`places.status`. The file is a derivative (changes listed in `crates/places/data/README.md`) and stays separate from code.
No GPL / LGPL / AGPL code was read or copied.
