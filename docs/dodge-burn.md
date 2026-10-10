# Guided dodge & burn (`retouch.dodgeBurn`, `retouch.dodge`, `retouch.burn`)

LightCraft v10. Editable **local masks** that lighten and darken, built from the 478-point face
mesh, the depth of the face and the subject. Nothing is baked: each part is one ordinary mask named
`Retouch: D&B · …` carrying a local exposure (and sometimes contrast, saturation or highlights).
Running the command again replaces its own masks; one undo step; `retouch.dodgeBurnClear` removes
them.

## Two roles that are not to be mixed up

* **Cleanup** evens out the irregularities of the skin's **luminosity** (dark circles, patches,
  folds) *without* changing the face: the mid-frequency band of L on the skin (blur at 1.5 % minus
  blur at 8 % of the face width) is brought toward zero. The exposure of the two masks
  (`Dodge: cleanup (skin)`, `Burn: cleanup (skin)`) is sized from a probe render (L per EV on this
  very skin), so it is the same on a dark skin as on a light one.
* **Sculpt** adds a little **relief**: cheekbones, nose ridge and forehead lit; the sides of the face
  and the hollows shaded. The planes come from the **convexity of the face's depth** (the mesh's z
  interpolated, small blur minus large blur): where the surface bulges toward the camera it is
  dodged, where it falls away it is burnt.
* Eyes (whites, irises, under the eyes), lips and the frame (background, edges, distractions) are
  separate parts.

## Modes

| mode | what it draws |
|---|---|
| `portrait` | under the eyes, eye whites, irises, lips, cheekbones, nose ridge, forehead, sides of the face; the skin cleanup; background, edges, distractions |
| `flat` (food) | subject lifted, background, edges, distractions burnt |
| `product` | like flat, background stronger, hot spots on the subject attenuated |
| `landscape` | sky burnt, foreground lifted, edges, distractions |
| `auto` | `portrait` when a face is found, `landscape` when the sky covers more than 20 % (model only), else `flat` |

The **subject** comes from the matting model when it is installed (`autoseg`, see
`docs/autoseg.md`); otherwise from a stated heuristic (the head and shoulders under each face for a
portrait, the colourful, bright, contrasty middle of the frame else). The result says which one
(`subject`).

`strength` (0..1.5, default **0.4**, subtle) scales every adjustment; `role` limits the parts
(`all`, `cleanup`, `sculpt`, `eyes`, `lips`, `finish`, `both` = cleanup + sculpt, or a list).

## Quality checks

* **Halo** around the subject: the lightness change just outside its edge must lie between the
  change inside and the change far away; a dark or bright rim sticks out of that interval and is
  measured in L (tolerance 1.0). The planes are feathered (about 1.2 % of the long edge) rather
  than snapped to the edge.
* **Nothing outside the treated zones**: the pixels where no mask acts are identical before and
  after (checked on the full recipe, `outsidePixelsChanged`).

## Manual brushes

`retouch.dodge` and `retouch.burn` add a brush stroke as an ordinary mask (`points`, `size`,
`amount` in EV, `feather`, `flow`). With `depthRange: [lo, hi]` (0 far … 1 near) the stroke is
**intersected with a depth range**: it only acts on that distance range. That needs a depth map for
the photo (`depth.estimate`, Depth Anything V2 Small); without one the command says so.

The depth map **embedded in iPhone / Android HEIC files is not read** (see Limits).

## Commands

* `retouch.dodgeBurn {ids?, mode?, strength?, role?, apply?: true}`
* `retouch.dodgeBurnClear {ids?}`
* `retouch.dodge {id?, points, size?, amount?, feather?, flow?, depthRange?}` /
  `retouch.burn {…}`

## Limits

* The mesh's z is the face model's own estimate (smooth, not a scan): the sculpt is gentle by
  design.
* Embedded HEIC depth: not implemented (no HEIC with a depth auxiliary image was available to
  verify a decoder; the container parser of the HEIF crate exposes the auxiliary items, a decoder
  for the monochrome HEVC depth item is the missing part).
* The fallback subject is a guess: with a plain background and a centred subject it works; with a
  busy scene install the matting model.
