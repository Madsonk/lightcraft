# Videos in LightCraft — resuming the work

Branch `videos-v1` of `/Volumes/Data/work/lightcraft-videos` (base `13e0a110` = local v16). Short notes for whoever
picks the work up; the full picture is in [`describe.md`](describe.md) and [`people.md`](people.md) (Faces in videos).

## Avancement VID-5

| # | Subject | State | Commit / proof |
|---|---|---|---|
| 1a | "Library already open" test flakes | done: cause = a child process started by another thread holds the lock's descriptor until its program starts; the lock is asked again for 1.5 s | `a37020c2` (regression test: refused 63/300 before, 0 after) |
| 1b | clippy `-D warnings` on the workspace | done (mechanical) | `ba85ecd2`; `cargo clippy --workspace --all-targets -- -D warnings` exit 0 |
| 1c | wasm target | installed (`rustup target add wasm32-unknown-unknown`); checked by `cargo xtask ci` | see CI below |
| 1d | the 14 red ui-egui tests | to do | |
| 2 | French search bench | to do | |
| 3 | real scale (lv-e2e3) | indexing done: `people.index` 3 897 pictures in 1 635 s, 943 faces, peak 1.15 GB; `search.index` 3 897 in 246 s, peak 2.2 GB; on disk people 5 MB, search 6 MB, video 377 MB, describe 0.4 MB | |
| 4–8 | player, unreadable videos, finishings, this file, bundle | to do | |
