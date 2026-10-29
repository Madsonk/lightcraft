# Palettes (`palette.extract`, `palette.harmonize`, `palette.save`, `palette.list`)

LightCraft v10. The dominant colours of a photo or a batch, and a move of the **Colour Mixer**
toward a target palette. The result is the ordinary Colour Mixer (editable), never a LUT.

## Extract

`palette.extract {ids?, k?: 1..12 (6), lot?, each?}`: k-means in Lab on about 16 000 samples of the
developed photo (deterministic), close colours merged (CIEDE2000 under 7), sorted by share. Each
colour: `hex`, `lab`, `share`, an approximate `name` / `nameFr` (nearest of about 40 reference
colours) and `copy` (the hex list, to paste). With several photos and no `each`, one palette for
the whole batch.

## Harmonize

`palette.harmonize {ids?, target, strength?: 0..1 (0.6), protectSkin?: true, mode?: lot|each, apply?: true}`

`target` is one of: `{photo: id}` (the colours of a reference photo), `{scheme: complementary |
analogous | triadic | split}` (built on the accent colour of the photo), `{hexes: ["#rrggbb", …]}`,
`{palette: "name"}` (a saved palette).

* For each of the 8 mixer bands the plan takes the band's mean colour in the photo and the target
  colour nearest by hue; the hue shift is capped at 40° and scaled by `strength`; chroma and
  lightness follow (capped).
* The mixer values are sized from **probe renders of the real pipeline** (hue, saturation,
  luminance at +50 on all bands, measured per band), not from a model of it.
* **Skin protected**: with faces, the skin pixels are left out of the plan and the skin's median
  colour is checked after the move; red, orange and yellow bands are halved, then switched off,
  until the skin moves by less than ΔE 2.
* **Coherent batch**: with several photos the default `mode: "lot"` computes **one** set of mixer
  values from all the photos' samples and applies it to each (identical mixers: tested on 6
  photos); `each` plans per photo.
* **Preview before applying**: `apply: false` returns the figures (target colours, per-band
  shifts, and per photo the palette distance before and after, the skin shift) and writes nothing.
  The distance is the mean CIEDE2000 of the photo's coloured pixels to the nearest target colour,
  lightness left out (a hue move cannot fix lightness).
* One undo step. The move is added **on top of** the current mixer; running it again adds again
  (like any slider): undo to retry.

## Saved palettes

`palette.save {name, brand?, colors?: [hex…] | ids?+k?}` keeps a named palette per brand or
universe in LightCraft's config folder (`palettes.json`, or `$LIGHTCRAFT_PALETTES`);
`palette.list {brand?}`, `palette.delete {name}`.

## Measured (10 Oct 2026, 6 dishes of one shoot, 1 portrait)

* The plan is **tried on the real renders at four scales** (1, 0.7, 0.45, 0.25 of the plan) and kept
  only if it brings the palette closer by at least 2 %; else nothing is moved and the plan says so.
  The saved palette *Kerma test* gave no gain on these dark dishes (nothing moved); the other
  targets did: complementary scheme 10.7 … 12.7 → 9.7 … 11.3, triadic the same, hex list
  (teal / amber / terracotta) 16.5 … 19.2 → 15.1 … 17.8, i.e. 8 … 15 % closer, not more: a hue move of
  the mixer cannot change a dark dish's lightness or a grey plate's chroma.
* Lot coherence: the 6 photos got identical mixers (checked on the stored settings).
* Skin: portrait 370 with `protectSkin` 1.44 ΔE of skin movement (limit 2.0), palette distance
  13.4 → 11.9; without protection 12.4 → 11.0 and the skin moves freely.

## Limits

* Skin protection works from the faces of the photo: a hand or a body without a face is not
  protected (a dish photo with a hand warms the hand with the food).
* The move is limited to the 8 bands of the Colour Mixer (±40° of hue at most): a target far from
  the photo's hues is only approached.
