# Lens recognition and profile corrections

LightCraft reads the camera and lens of a photo from its metadata, looks the lens up in a profile database and applies
its **distortion**, **lateral chromatic aberration** and **vignetting** corrections (Optics section: *Use Profile
Corrections*, on by default for raws that have a profile). It never invents a profile: when none is found the panel says
so and why.

## Where the data comes from

1. **Corrections embedded in the file** win: DNG `WarpRectilinear` / `FixVignetteRadial` opcodes
   (`crates/engine/src/files.rs`, unchanged). Other formats' embedded correction tables (Sony ARW, Fuji, Panasonic RW2,
   Olympus) are **not** read yet: their layouts are not published by the makers and were not reverse engineered here.
2. **The lens database** (`crates/lens`): lens calibrations from the lensfun project's lens database, **data only**
   (CC BY-SA 3.0, see `crates/lens/data/README.md` for the obligations; no LGPL code is used), shipped as a separate file
   `lens-profiles.json.z`, found next to the application (`Resources/lens/` in the macOS bundle, `lens/` beside the
   executable), in `~/Library/Application Support/LightCraft/lens/`, or at `$LIGHTCRAFT_LENS_DB`.

## Recognition

Metadata read (`crates/meta`): Exif `Make`, `Model`, `LensModel`, `LensSpecification`, focal length (and 35 mm
equivalent), f-number, subject distance, plus the maker note where Exif lacks the lens: Canon (`LensModel`,
`CameraSettings` focal range and widest aperture, lens type id), Panasonic (`LensType`), Nikon (`LensInfo`), Fujifilm
(focal range and apertures). Facts kept per photo: `Meta.lens_make / lens_spec / lens_max_aperture / focal_35mm /
subject_distance` (optional, absent in older catalogs).

Matching (`crates/lens/src/matching.rs`): the body gives the mount and sensor crop factor; lenses of that mount are
scored by name similarity (token Dice coefficient) when the name says something, otherwise by the focal range and widest
aperture (a bare `50mm` is common). The result says how sure it is: `exact`, `probable`, `ambiguous` (several lenses fit;
the best calibrated is used and the others are listed) or `none`. Check any photo with `lens.info`.

## Corrections

Calibrations are evaluated at the shot's focal length, aperture and focus distance (interpolated), re-expressed in the
image's own radius (`r = 1` at the image corner; a smaller sensor than the calibration frame sees `r ≤ crop_lens/crop_camera`)
and fitted by least squares onto the pipeline's existing lens stage (per-plane radial polynomial + vignetting polynomial;
fit error is bounded in tests). The geometry keeps the image corner in place. Pipeline order is unchanged: lens
distortion and lateral CA are part of the single geometry resample (before perspective, crop and everything tonal);
the vignetting gain is applied at the source position before tone. Previews, thumbnails and exports use the same
`SourceInfo.lens`.

## Commands

* `lens.info {id?}`: the camera, the lens as named in the file, where the profile comes from (`embedded`, `database`,
  `none`), the database lens, confidence, what is corrected, and the note when nothing is.
* `lens.detect {ids? | all, enable?, reread?}`: report (and, with `enable`, switch *Use Profile Corrections* on for
  photos that have a profile; the edit stamp and history are left alone, like an import default). `reread` (default)
  reads the lens facts from the files of photos imported before they were kept.

## Limits

* Profiles exist only for lenses the database lists; the database is community made: coverage and accuracy vary.
* The conventions of the database (radius normalised to the half-diagonal of the calibration frame) were checked on the
  data's internal consistency and on a synthetic round trip, not against an independent measurement of a strongly
  distorting lens.
* Non-rectilinear (fisheye) lenses: chromatic aberration and vignetting only.
* Photos imported before this feature keep *Use Profile Corrections* off until `lens.detect enable=true` (or the box) is used.
* JPEG / HEIC are recognised too, but their box is not ticked for you (cameras often correct their own JPEGs).
