# Reliable library, culling and export workflow

The local workflow improvements use LightCraft's own implementation. No Adobe code,
models, profiles or assets are included.

## Target the intended photos

Read `library.state` before editing. Every engine and desktop command accepts an optional
`expectedSelection: {active, ids}` matching the returned selection. If the human changes
the selection before the command runs, it fails before making changes. The guard checks
both the active photo and the selected set, ignoring order. It is a selection check, not
a lock or a check of develop settings.

For exports and assisted culling, use explicit `id` or `ids`. Malformed, empty or unknown
targets fail; they never fall back to another selection. Desktop export now honours a
single `id` even when other photos are selected. Export receipts identify each photo.
Saved export presets and previous-export settings do not retain photo targets.

## Diagnose and preview reconnection

`library.diagnoseFiles {}` checks in-library original files and returns per-photo states
and counts: `available`, `missing`, `volume_offline` (macOS `/Volumes` root is absent),
`access_denied`, `not_a_file`, `io_error`. A missing path is not proof of deletion.
It performs filesystem I/O and can take time on remote volumes; it changes no catalog data.

`library.findMissing {folder, dryRun: true}` returns a plan without relinking. Review
`found` and `ambiguous`, then pass the plan to `library.findMissing` to apply the existing
matching logic in one undo step. Ambiguous candidates are skipped. The plan is rechecked
against current catalog paths before application; original-file identity should be reviewed
before relinking. `edit.undo` restores catalog links without moving files on disk.

## Explain and preview assisted culling

`photo.analyze {ids, dryRun: true, rejectBelow: 20, pickBest: true}` returns suggestions,
reasons and `wouldPick`/`wouldReject`; it changes no stored scores, flags or undo history.
Dry runs use existing face results only and never start face detection. `eyes_not_evaluated`
must not be interpreted as eyes open. Measurement basis is the original thumbnail, not the
current edited render. Suggestions remain separate from applied flags.

Regular analysis stores measurements in a bounded 1,024-entry `cullingCache` in the
library's existing prefs.json. Cache keys contain algorithm version, source identity,
current file size and modification time. Burst membership and face weighting are always
recomputed. `force: true` bypasses cached measurements. Cache reuse is reported via
`cached` and `measured`; invalid entries are discarded on load. Changing the scoring or
thumbnail algorithm requires changing `MEASUREMENT_VERSION`. This is a heuristic focus
and clipping measurement, not a learned aesthetic-quality model. A file replaced with
the same size and modification time is not detected by this stamp; use force after such
external replacements. No cache is reused when the original cannot be stat'ed.

Existing versions, history, selective sync, undo and redo remain the retouch workflow.

## Verify exports

The native shared file writer reads written bytes back through a bounded buffer before
reporting success. This catches a truncated or different output; it does not promise
power-loss durability or a transaction across a main file and its sidecars.

Each successful batch item includes `photo`, `path`, `width`, `height`, `bytes`,
`contentHash` and sidecars. The hash is LightCraft's existing content identity hash,
not a cryptographic authenticity proof.

Pass a receipt to `export.verify`. It compares expected byte count, content hash and
dimensions, then decodes a small preview. It fails on mismatches or broken images and
never changes the file. Limits: 128 MiB input, 100 MP source; rendered formats supported
by the codecs only. RAW-original and DNG verification is outside this command's scope.
Custom or in-memory writers are not automatically verified by the native writer;
`export.verify` is the explicit on-disk check. Verification covers the main rendered file,
not XMP sidecars.

The Sorting Help preview displays a bounded review list (200 photos maximum), with translated
reasons; cancelled previews are marked partial.

Regression tests: `cargo test --release --lib -p lightcraft-engine --features people workflow_` and
`cargo test --release --lib -p lightcraft-ui-egui workflow_export_explicit_id_wins_over_selection`.

For package reference checks, set `LIGHTCRAFT_PARITY_CLI` to the freshly built CLI
and run `cargo xtask parity`. This verifies the delivered command registry without
building another copy in the debug profile. Use a CLI built from this checkout.
