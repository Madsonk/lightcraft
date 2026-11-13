# Manifeste des features du fork LightCraft (à versionner en `docs/fork-features.md`)

> Rôle : la liste qui rend les ports vérifiables. Une ligne = une feature + la preuve qui doit rester verte.
> Établi le 2026-11-13 à partir de `git log v0.20.0..HEAD`, des crates et des tests existants.

| # | Feature | Surface (commandes / API) | Code | Tests existants | Upstream 0.21 | Décision | Preuve à conserver |
|---|---|---|---|---|---|---|---|
| 1 | Culling IA | `cull.run / apply / list`, verdicts, doublons, scènes | `crates/culler` (7 fichiers), `engine/src/cmd/cull.rs`, `cull_job.rs` | **16** | absent | garder fork | `cargo test -p lightcraft-culler` + un `cull.run` sur 30 photos ≥ 1 verdict |
| 2 | Portrait retouch | `portrait.apply / preview / auto / batch / openEyes / reset`, `PortraitStack` | `crates/retouch` (9), `develop/src/portrait.rs` | **55** | absent | garder fork | `cargo test -p lightcraft-retouch` + `portrait.auto` répond `skipped: no face` sur photo sans visage |
| 3 | Masques auto (SegFormer/ISNet) | `mask.auto` (subject/background) | `crates/autoseg` (4) + `engine` | 2 crate / 15 engine | SAM 3 (objet) seulement | garder fork | tests engine `autoseg` + `mask.auto` sur 1 photo |
| 4 | Facemesh (yeux ouverts/fermés, netteté) | `photo.analyzeFaces` | `crates/facemesh` (6) | **15** | YuNet 5 points | garder fork | `cargo test -p lightcraft-facemesh` |
| 5 | Depth / `depthRange` sur masque | `mask.add {depthRange}`, `develop/depth_map.rs` | `crates/depth` (3) | **0** ⚠ | **Lens Blur natif + `focus_mask`, bokeh par masque** | **arbitrer** : garder le Lens Blur upstream, ne conserver que `depthRange` | test à créer (depthrange → masque non vide) + un `mask.add depthRange` accepté |
| 6 | Inpaint MI-GAN | `SpotMode::Inpaint`, `Aux`, `Spot.patch` | `crates/migan` (4), `pipeline/inpaint.rs` | 3 | PatchMatch seul | garder fork | `cargo test -p lightcraft-migan` |
| 7 | Style / Autoframe / Taste | `style.*`, `autocrop.*` | `crates/style` (10), `taste` (6), `autoframe` (2) | 18 + 14 | absent | garder fork | tests style + taste |
| 8 | Video + Describe + Recherche | `video.*`, `describe.*`, `search.index` (SigLIP) | `engine/src/cmd/video.rs`, `describe.rs`, `crates/search` (8) | 8 (search) | absent (M14 non commencé) | garder fork | `video.status` + `search.index` sur une petite bibliothèque |
| 9 | Perf tri/triage | sort view, prefetch, grid buffer, drop de photos sur Keywords | `ui-egui` (sort, grid, keywords) | 0 (UI) | Survey (autre vue) | garder fork | mesure (`tools/perf.sh`) : temps d'ouverture du tri + requêtes par flèche |
| 10 | Catalogue FR | `locales/fr.json` (+651 clés) | `ui-egui/locales` | — | `fr.json` upstream (+28) | **sortir du fork** : overlay | toutes les clés utilisées présentes (test xtask i18n) |
| 11 | Infra de port | `port-check.yml`, lock reconcile | `.github/workflows` | — | n/a | à durcir | le workflow tourne et publie son rapport |

## Règles

- Une feature **sans preuve** n'est pas protégée : un merge propre peut l'effacer sans que rien ne le signale.
- Toute ligne dont la colonne « Upstream » devient « fourni » doit être **réévaluée** : on préfère l'upstream et on supprime notre code (règle du port 0.14 : « quand upstream fournit déjà, préférer upstream »).
- `depth` (#5) est le cas urgent : 0 test, et upstream vient d'arriver sur le même terrain.
