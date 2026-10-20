# Search by meaning and duplicates

LightCraft finds photos by what they show — "assiette de poisson avec bananes plantain", "child playing in a
swimming pool" — and finds duplicates and near-duplicates. The search runs **SigLIP 2** (Google) in pure Rust on
[candle](https://github.com/huggingface/candle) (`crates/search`): Metal on macOS, the CPU elsewhere. Everything stays
on your machine.

**The model is optional.** It is not part of LightCraft: without it, everything else works, exact duplicates work, and
near-duplicates work from perceptual hashes alone.

## Commands

(control channel, `lightcraft-cli run`, MCP; see `engine.commands`)

| Command | What it does |
|---|---|
| `search.model.status` / `.download {acknowledged: true}` / `.cancel` | the model, its licence, the download (resumable, each file checked by size and SHA-256) |
| `search.index {ids?, background?, force?, videos?}` | embeds the photos that have no vector yet (thumbnails, batches of 32) and hashes them, and (`videos: true`, the default) the pictures cut out of videos (`video.frames`), each under `key_of("<content key>@<ms>")`; interrupted runs resume; `background: true` returns at once |
| `search.status` / `search.cancel` | photos indexed / hashed / waiting, the run's progress and where its time went |
| `search.semantic {query, limit?, minScore?, ids?, filter?}` | photos ranked by similarity to a sentence → `{id, score (cosine), probability}` |
| `search.tag {labels, threshold?, template?, ids?, dryRun?}` | zero-shot: photos whose probability for a label reaches the threshold (default **0.02**) get it as a keyword |
| `library.findDuplicates {mode: exact\|near, maxDistance?, minCosine?, stack?, filter?}` | groups of duplicates; `stack` groups each into a stack (largest file on top) |

`semantic:` in the search bar is not wired yet; use `search.semantic` (`filter: true` shows the results).

## Getting the model

`search.model.download {"acknowledged": true}` (the app asks first): about 1.5 GB from
`huggingface.co/google/siglip2-base-patch16-224` at a pinned revision, into `<config>/models/search` or
`LIGHTCRAFT_SEARCH_DIR`. Extra mirrors: `LIGHTCRAFT_SEARCH_MIRRORS` or `<config>/models/search-mirrors.txt`.
Licence: **Apache License 2.0** per the model card (weights and `tokenizer.json`; the vocabulary is Gemma's, the repository
carries no separate LICENSE file). You can also put `model.safetensors`, `tokenizer.json` and
`preprocessor_config.json` there yourself; the SigLIP 1 base checkpoint (`google/siglip-base-patch16-224`, Apache-2.0)
loads too.

## What is stored

`<library>/search/vectors-<model>.bin` (768 × 16-bit floats + an 8-byte key per photo: **46 MB per 30 000 photos**) and
`hashes.bin` (24 bytes per photo): append-only, next to the catalog, never in its journal, keyed by the photo's content
key (a changed file is simply embedded again). A torn last record is cut off when the files open; delete the folder to
start over.

## Measurements

Apple Silicon, Metal, F32, 1 203 photos (1 079 JPEG phone photos, 106 Canon CR2 plates, 18 PNG/other; library on an
external HDD):

- Embedding: ~49 ms per photo (batch 32; F16 gave no gain: 46 vs 54 ms); CPU only: ~310–340 ms. Decoding the thumbnails
  took more (200 ms per photo wall-clock here, disk bound): a whole run was 355 s, of which 46 s loading the model from
  that disk. 30 000 photos ≈ 25 min of embedding plus decoding.
- Query: ~25 ms (text tower) + 1.6 ms for the search over 30 000 vectors (median, 8 cores).
- Precision@8, 17 queries per language judged by eye on 8-photo sheets (one rater; "pacifier", "pool", "pizza" have
  only ~3 matching photos in the corpus, which caps them): SigLIP 2 English 0.76, French 0.70; SigLIP 1 base English
  0.79, French 0.71. French works on both, SigLIP 2 keeps French results closer to English ones; SigLIP 1 drops
  apostrophes ("l'herbe" → "lherbe"). SigLIP 2 is the default.
- Near-duplicates (dHash, ≤ 10 of 128 bits, embeddings ≥ 0.90): 69 groups, all 24 inspected groups were true bursts
  (palm-tree series, plated-dish series); 67 of the 106 CR2 plates fall in a burst of the same framing; widening to 20
  bits groups 99 of 106 but starts chaining framings.
- Tagging at 0.02: the 12 lowest-scoring matches of each of 8 labels were 85–100 % correct; 0.01 degrades `selfie` and
  `chicken`. Recall is conservative (`sea` 45 photos at 0.02, 81 at 0.004; rare labels such as `pizza` mostly miss).

## Limits

`search.semantic` ranks photos only; the pictures of videos are indexed for `describe.search` ([`describe.md`](describe.md)), which searches photos and moments of videos with the people present; thumbnails decode at ≤ 512 px; no UI yet (no panel, no `semantic:` token, no download dialog:
the licence is shown by `search.model.status` and the refusal of `search.model.download` without consent); the model
stays in memory (about 1.5 GB) until `Searcher::unload`; no German/Spanish/etc. evaluation.
