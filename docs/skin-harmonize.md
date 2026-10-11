# Skin harmonization and the proposed retouch (`skin.*`, `retouch.propose`, `retouch.apply`)

LightCraft v10. Everything here writes **ordinary, editable settings** (local masks named
`Retouch: …`, the Colour Mixer, heal spots for sensor dust) in **one undo step**. Nothing is a LUT,
nothing is baked into pixels, nothing is generated, no photo leaves the machine. Proposing writes
nothing.

## What it does

Even out the **colour** of skin, never its texture and not its lightness:

1. **Inside a photo** — blotches (redness on the nose and cheeks, yellow or grey zones, the hue of
   dark circles) are brought toward the face's own median colour.
2. **Across a series** — the skin of the **same person** (People) is brought toward their colour
   over the series, or toward a reference photo you choose. A real change of light is only
   corrected in part: `series` (0..1, default 0.6) and `maxShift` (Lab units, default 6).

## How

* **Skin pixels** of a face = the Portrait skin region (face mesh, eyes, brows, lips left out),
  then a robust trimming in Lab: no highlights (bright and less colourful), hair, beard or deep
  shadow. Median and spread are measured on what is left.
* **Colour only, low frequency only.** The corrected quantity is the skin's `a`/`b` blurred to
  about 2 % of the face width (3 box blurs, weighted by the skin pixels). The fine detail of the
  photo is never read back, so pores and hairs are untouched by construction; the high-frequency
  variance of the luminance is measured before and after (tolerance 5 %).
* **Polar, relative units.** The wanted shift is expressed as a relative change of chroma and a
  rotation of the hue around the face's own colour. That makes it hold for a dark skin as well as a
  light one (a "+5 Lab units" cast is not the same thing on a skin of chroma 8 and one of chroma
  30; "10 % less saturated" is).
* **Four soft masks per face** (`Skin · less saturated`, `more saturated`, `hue toward red`,
  `hue toward yellow`). Each mask's alpha is the size of the wanted shift divided by a reference
  size, so one ordinary local Saturation or Hue slider rebuilds the field. Edit or delete them in
  the Masking panel.
* **Sized on the real pipeline, not on a model of it.** Probe renders (local saturation and hue,
  up and down, on the face) give the pipeline's actual response; a closed loop then renders the
  result, compares the achieved improvement with the wanted one and corrects the dose twice at
  most. A median-shift feedback keeps the skin's median where the field says (zero inside a
  photo): fixing a blotch must not move the skin's colour.
* **Dark skins.** The series target is stretched to keep at least 97 % of the face's own chroma
  (never ashen, waxy or washed out to match a duller photo); the field's median is never allowed to
  lower chroma by more than 3 %; the lightness is not touched (measured: the median L moves by less
  than 1.2).

## Measurements and the vectorscope

`skin.measure {ids, person?, reference?}`: Lab median, chroma, hue, **colour uniformity**
(mean CIEDE2000 between each skin pixel's low-frequency colour and the median, each pixel keeping
the median lightness so that lightness is left out), the spread, the **vectorscope angle** of the
skin against the **skin line (123°)**, the cast of the neutral parts (greys, whites, walls), and per
person over the series how far each photo is from the series colour.

Reading the vectorscope: when the neutral parts moved the same way as the skin between two photos
(`is_cast`), the difference is a **cast over the whole picture**, not a real difference of skin: fix
it with the white balance rather than by painting the skin.

## Commands

* `skin.measure {ids?, person?, reference?}` — query, changes nothing.
* `skin.harmonize {ids?, strength?: 0..1 (1), uniform?: 0..1 (0.8), series?: 0..1 (0.6), maxShift?: 6, person?, reference?, apply?: true}`
* `skin.clear {ids?}` — remove the `Retouch: Skin …` masks.
* `retouch.propose`, `retouch.apply`, `retouch.clear`, `retouch.recipes` — the **Natural skin**
  recipe (below).

## The Natural skin recipe

Eight ordered steps, each with a switch, an impact and a measured explanation
(`"skin colour made uniform from ΔE 4.2 to 1.8"`):

| # | Step | What it writes |
|---|---|---|
| 1 | Camera development | nothing: your own settings are the base |
| 2 | Dust and defects | heal spots for **persistent sensor dust of a series** (3 or more photos), never on or beside a face; a single photo cannot tell dust from detail |
| 3 | Skin colour | `Retouch: Skin · …` masks (this page) |
| 4 | Dodge & burn: cleanup | even out the skin's luminosity irregularities ([dodge-burn.md](dodge-burn.md)) |
| 5 | Dodge & burn: light sculpting | cheekbones, nose ridge, forehead lit; sides and hollows shaded |
| 6 | Eyes, teeth, lips | whites, irises, under the eyes, lips; teeth: less yellow rather than just whiter |
| 7 | Overall colour | only with a target palette: Colour Mixer ([palette.md](palette.md)) |
| 8 | Finishing | background and edges lightly darkened, distractions attenuated |

`impact`: `light` (0.4), `medium` (0.7, default), `strong` (1.0) or a number. `retouch.apply
{variants: true}` also leaves **Natural skin · Before / Light / Medium / Strong** in Versions for
before/after. In the app: the **Proposals** section at the top of the Versions panel.

**Copies (v12).** `retouch.apply {copy: true}` (what the app's buttons use) leaves the original **untouched**
and writes the retouch on virtual copies named **Natural skin – Light / Medium / Strong** (the impact asked
for, or the three with `variants`), stacked with the original, in one undo step. `retouch.clear` removes
those copies (to Recently Deleted) and only falls back to clearing the `Retouch: …` masks of a photo that
has no copy (what v10 and v11 wrote). `retouch.propose {settings: true}` also returns the settings each plan
comes to: the app shows them in the loupe as a temporary look (**Preview on the photo**), written nowhere.
The app plans one photo per frame with a progress bar (a single photo still takes its ~8 s in one go:
the planning needs the session and cannot run on a worker).

### Red lines (each one has an automated test)

* **No mole, scar or tattoo is removed without explicit confirmation.** Heal spots are only added,
  never on or beside a face; positions you confirm yourself go through `confirmSpots: [[x, y]]`.
* **No change of the shape of the face or of the traits.** No geometry, crop, resampling or liquify:
  only tone and colour through masks.
* **No generated pixels.** No inpainting, no generative model; heal spots only.
* **No photo sent anywhere.** The module has no network code (a test scans the sources).
* **Texture kept.** High-frequency variance of the luminosity (log scale, so that lightening a
  pore does not count as smoothing it) measured on the skin of a 2400 px render before and after:
  tolerance 5 %.
* **Natural first.** When a check goes over its threshold (texture, a halo around the subject, the
  skin's lightness) the dose is reduced automatically (×0.75, three times at most) and the plan
  says so (`autoReduced`).
* **Nothing changes outside the treated zones.** Measured: the pixels where no mask acts are
  identical before and after (`outsidePixelsChanged: 0`).

## Measured (10 Oct 2026, 10 portraits and 6 dishes, copies of test libraries)

The portraits are phone photos of a dark-skinned man (and his partner) under very different light
(a dim restaurant, strong sun, shade): 10 photos, 16 faces. They are **not** portraits of Tonton
Youmba: the Tonton Youmba shoot found on disk (106 RAW + the edited JPEGs) is dishes and hands.

| measure | result |
|---|---|
| colour uniformity inside a photo (mean CIEDE2000 of the low-frequency colour to the median, lightness left out), measured again on the real result of the recipe | man 4.41 → 3.21 (10 photos), woman 2.81 → 1.39 (5 photos) |
| same, `skin.harmonize` alone over 16 photos of the man | 4.38 → 3.39; uniformity only (`series: 0`): → 2.96 |
| distance of each photo's skin to the person's series colour (recipe, default `series` 0.6, cap 6) | man mean 8.91 → 8.45 (max 17.0 → 16.9), woman 2.66 → 2.01 (max 7.3 → 5.7); `skin.harmonize` alone over 16 photos of the man: 6.10 → 5.27 |
| chroma of the skin after (ratio to before) | at least 0.97 with the defaults |
| texture: high-frequency variance of log luminance on a 2400 px render, 16 faces | −4.0 % … +2.0 % (raw L\* variance −8 % … +4 %), tolerance 5 %; no dose reduction needed |
| pixels changed outside the treated zones | 0 of every photo (10/10 portraits, 6/6 dishes) |
| lightness of the skin | median L moved by at most 0.58 |
| time (M4, other work running) | propose 10 portraits 80 s (≈ 8 s a photo), apply with 3 variants 96 s; recipe on 6 dishes 70 s |

The series distance only closes **partly, by design**: the man's photos differ by real light
(blue shade: b\* = −14 against +13 in the restaurant, a 20 ΔE gap that the neutral parts show too:
a cast, not a difference of skin). `series` and `maxShift` raise what is corrected; the chroma floor
and the 20 % relative cap keep it natural.

## Limits

* Faces come from the MediaPipe face model (see `docs/facemesh.md`); a photo without a detected
  face gets no skin step (food, objects) and the plan says why.
* The People match uses the existing index; faces nobody named only get the inside-photo
  uniformization. Name them (or pass `person`) to harmonize a series.
* Proposing takes about 8 s a photo (renders and probes); it runs on the interface thread, so the
  window waits (the engine commands are synchronous: CLI and MCP are unaffected).
* The face filter is the one of Analyze Faces and People (detection score, drift, size): a plate is
  not taken for a face any more (a first version found "faces" in dishes).
* Sensor dust: a single photo is never healed. A series search returning more than 24 spots is not
  believed (hundreds of "persistent" spots were found on a set of unrelated photos, and healing
  them would have smeared a shirt and a facade): nothing is healed and the plan says why.
