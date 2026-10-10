# Persistent dust across a series, and portrait retouch

## Persistent sensor dust (`dust.detect`, `dust.apply`)

Dust on the sensor sits at the same place on every photo of a shoot; everything the camera sees moves.
`spot.findDust` looks at one photo and has to guess; these commands look at the whole series, which
is what tells dust from scenery.

For each photo of the series (the selection, else the view; at least 3; an even sample of at most
`limit`, default 40) LightCraft renders a 2048 px copy without the photo's own edits (sensor geometry:
no crop, no lens correction) and marks the pixels that are a little darker than a 33 px neighbourhood
**in smooth areas** (sky, walls, blurred backgrounds: the only places where dust can be judged at
all). For each pixel it counts how often it was dark out of how often it could be judged; a place
that is dark in ≥ 60 % of the photos where it can be judged (≥ 3 of them) is dust. Neighbouring
pixels are merged into spots, and long thin runs (edges, wires) are dropped.

- `dust.detect {ids?, sensitivity?: 0..100 (50), limit?}` → `{photos, analysed, skipped, frame, spots:
  [{x, y, radius, seen, judged}]}`. Changes nothing.
- `dust.apply {ids?, sensitivity?, limit?, spots?, remove?}` (Photo ▸ Remove Dust From Series, and the *Dust From Series* button of the Remove panel):
  a heal spot (source picked automatically) on each dust spot on **every** photo of the series, skipping
  places already healed; one undo step for the whole series. With `background: true` (what the menu
  and the button send) the app renders and searches the sample on a worker thread, with a progress
  panel and a Cancel button (nothing is changed when it is cancelled), then heals the series by
  calling `dust.apply` with the spots it found.

Photos of another size or orientation than the first form another series (dust sits elsewhere in a
portrait frame): they are counted in `skipped`. The spots are placed in the uncropped frame; with a
lens-profile correction on, positions at the very edge can differ by up to a spot radius.

## Portrait retouch (`portrait.retouch`, `portrait.clear`)

Needs the face model of Analyze Faces (`facemesh.model.download`). For each face (the four biggest) the
478-point face mesh gives the regions; each becomes a normal mask with ordinary local adjustments:

| Mask | Region | Adjustments at 100 |
|---|---|---|
| Portrait: Skin | face oval minus eyes, brows and lips, soft edge | texture −80, clarity −25, noise reduction 25 |
| Portrait: Eyes | both eyelid contours | exposure +0.35, contrast +18, clarity +25, sharpness +30 |
| Portrait: Teeth | inside of the lips, only if the mouth is open | exposure +0.25, saturation −45 |
| Portrait: Lips | the lips minus the inside of the mouth | saturation +15 (off by default) |

`portrait.retouch {id?|ids?, skin: 40, eyes: 30, teeth: 30, lips: 0, maxFaces: 4, refine: 0}`: the
amounts scale the adjustments; running it again replaces its own masks (named "Portrait: …") and
leaves the user's. Everything stays editable in the Masking panel; `portrait.clear` removes the
retouch. The masks are stored with the photo (a high-resolution patch per face), so rendering never
needs the model.

Not done: GAN face restoration (GFPGAN / CodeFormer). Smoothing here is the texture slider through a
mask, which keeps the shapes and the eyes sharp but is not frequency separation.
