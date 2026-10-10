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

## Limits

* Faces come from the MediaPipe face model (see `docs/facemesh.md`); a photo without a detected
  face gets no skin step (food, objects) and the plan says why.
* The People match uses the existing index; faces nobody named only get the inside-photo
  uniformization. Name them (or pass `person`) to harmonize a series.
* Proposing takes seconds per photo (renders and probes); it runs on the interface thread.
