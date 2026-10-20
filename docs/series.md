# Private series workbench — local v16

In Library, select 2–1000 photos and make the reference photo active. Open **Photo series**.
Choose exposure, white balance and/or reference colour look, then **Preview series corrections**.
The analysis runs on a worker; preview and pause/resume never change the catalogue. Pause
finishes the current photo. Resume retains measured photos for this app session; the plan is
not persisted across quitting or a crash.

Review the numeric proposals and click a photo row to see its existing and proposed render
side by side. Reviewing a row preserves the active reference and selection. **Apply reviewed
series** validates the plan, changes the other photos and creates one undo step. The reference
is unchanged. Changed settings, metadata, sources, source size/mtime or a reopened library
invalidate the plan before any edit. Failed decodes are reported and skipped; reference failure
blocks application. File size/mtime is a freshness check, not a cryptographic content guarantee.

**Find time and place groups** groups selected photos by capture-time gaps of at most 15 minutes,
location/city and available GPS proximity. Missing capture times remain individual. These are
suggestions: lighting changes are not inferred from metadata alone. Groups use stored local
timestamps; cameras with incompatible timezones must have capture times corrected first.

The native method measures thumbnail-level source pixels per photo with the existing tone/WB
estimators. Exposure changes relative to the reference are capped at 2 stops. WB requires enough
low-chroma midtones; otherwise it stays unchanged and the row requests review. Creative WB
is transferred in reciprocal Kelvin, with capped temperature/tint differences. The common look
copies profile/LUT, curve, colour, mixer and grading. Local masks, crop and skin edits stay per photo.
This is a statistical estimate, not calibrated AI or a promise of colour fidelity across scenes.

Shared engine commands: `series.groups`, `series.plan`, `series.status`, `series.pause`,
`series.resume`, `series.apply`. Every command accepts the existing `expectedSelection` guard.
The CLI/connector and native UI use the same implementation.

## Private AI experiment

Madson confirmed private, noncommercial use on 2026-10-12. DeepWB (Afifi & Brown, CVPR2020),
<https://github.com/mahmoudnafifi/Deep_White_Balance>, is evaluated in a separate local clone:
`/Volumes/Data/work/lightcraft-private-deepwb`. Keep its CC BY-NC-SA4.0 licence and attribution.
It works on rendered RGB; its output is not a RAW Kelvin calibration. Pretrained weights and
upstream code are not included in the LightCraft app or cumulative patch. No publication is implied.

The private runner loads weights with `weights_only=True`, writes a distinct output file and a
JSON report, and leaves the input untouched. Use the isolated `.venv/bin/python` interpreter;
the general Python environment had conflicting OpenMP runtimes and is not a reliable evaluator.

## Selective upstream integration

The current `origin/main` at `76b5377868d912db69a777bda253a5b1b96b22bd` contains substantially
more changes than v0.5.0. v16 selectively imports the date-shift overflow/year-boundary fix in
`crates/catalog/src/dates.rs`, with the upstream regression tests. It does not claim a complete
v0.6.0 rebase. Detectron2 remains deferred by Madson's earlier instruction.
