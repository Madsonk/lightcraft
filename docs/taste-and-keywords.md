# Taste model, suggested picks and automatic keywords

Everything here **suggests**. Nothing writes a star, a flag or a keyword without a confirmation
(`culling.accept {confirm: true}`, `keywords.accept`, or the `autoAccept` setting, off by default).
It builds on the SigLIP 2 vectors of [semantic search](semantic-search.md): `search.index` must
have embedded the photos first. Nothing in the `People|…` / `Places|…` keyword spaces is ever
suggested, learned or written by these commands (those belong to the people and places features).

## 1. Taste: `taste.train`, `culling.suggest`, `culling.accept`

**Model.** A ridge regression (closed form, no randomness) on the photo's unit embedding
(768 numbers), optionally plus five thumbnail measurements (`extras: true`: focus, clipping, mean /
dark / bright luminance). Labels come from the catalog: stars 1–5, rejected photos = 0, and (option
`implicitNegatives`, on) the unrated photos of *shoots the user clearly went through* (a folder
where at least 5 photos and 10 % have stars) = "not chosen", weight 0.4. The regularisation is
chosen by 5-fold cross-validation; the model is stored in `<library>/search/taste.json` with the
numbers it was judged on (`taste.status`). Crate `lightcraft-taste` (pure Rust, tests included).

**How it is measured (`split`).** Folds hold out whole units, and the metrics are computed inside
each held-out fold then averaged (pooling folds rewards differences in a fold's average stars, not
the ranking of photos):

| `split` | held out | what it answers |
|---|---|---|
| `shoot` (default) | whole folders | does it rate a shoot it has never seen? |
| `burst` | whole 5-minute windows of a folder | can it finish a shoot you rated in part? |
| `photo` | random photos | optimistic: near-identical frames leak between train and test |

**Measured on a copy of the author's archive** (27 786 photos, 1 055 rated: 877 × 1 star, 171 × 2,
4 × 3, 3 × 4; 15 rejected; 13 reviewed folders giving 1 673 "not chosen"; 278 shoots; 5 860 photos
embedded, among them every labelled one):

| split | Spearman (stars, rated photos) | AUC chosen vs not | top-k hit rate (chance) |
|---|---|---|---|
| `shoot` | 0.04 | 0.56 | 0.33 (0.31) |
| `burst` | 0.50 | 0.77 | 0.53 (0.31) |
| `photo` | 0.48 | 0.79 | 0.55 (0.31) |

*top-k hit rate*: in each held-out shoot with R chosen photos, the share of the R best-scored that
were really chosen. Reading: **on a shoot it has never seen the model has no usable skill on this
archive**; it has real skill (AUC 0.77) *inside* a shoot where the user already rated some frames,
but even there the `burst` figure is still flattered by look-alike frames a few minutes apart.
The technical features did not help (`extras`: same numbers; sharpness alone correlates -0.10
with the stars, i.e. the starred photos are not the sharper ones), so they are off by default and
cost ~10 minutes of thumbnail decoding the first time. Why it is hard here: 83 % of the stars are
1 star, and in the biggest shoot the starred photos I looked at (12 drawn at random) were mostly strongly
graded ones, so the model may largely be learning "this looks like that batch" (a guess, not measured). `culling.suggest` shows the
cross-validated numbers next to every answer so a poor model cannot pass for a good one.

`culling.suggest {ids?, unratedOnly, fraction | count, minScore, filter}` → predicted stars,
percentile and the proposed picks among unrated photos (`filter: true` shows only them: the
filter bar's **Suggested picks** button). `culling.accept {ids, mode: pick | rating, confirm: true}`
is the only command that writes.

## 2. Keywords: `keywords.suggest`, `keywords.accept`, `keywords.learn`

**Controlled vocabulary.** 158 built-in entries in five roots: `Food|…` (dishes, drinks), `Subject|…`
(people *without names*: portrait, child, couple, wedding…), `Scene|…` (generic places),
`Object|…`, `Mood|…`; each has English prompts and a French name (`keywords.vocabulary
{language: "fr"}`; English by default). Extend or switch entries off with `keywords.vocabulary
{add, remove}` (kept in `<library>/search/vocabulary.json`).

**Zero-shot.** An entry's prompts are embedded by the SigLIP text tower (cached, so the model is
loaded only for new prompts) and averaged. A photo is suggested the keyword when (a) its cosine is
unusually high compared with the library's photos (z-score, which removes SigLIP's per-prompt
bias) and (b) SigLIP's own probability for the pair is at least `minProbability` (0.001). The
`score` is 0.5 at z = 1 and rises with z; `threshold` (default 0.5) cuts on it.

**Learned.** `keywords.learn` fits one probe per keyword the library already uses (≥ 20 indexed
photos, not on more than 40 % of the library), cross-validates it over whole folders and keeps the
score above which the held-out precision reached 0.8. Those suggestions are marked `learned`.

**Measured.** Zero-shot: 144 JPEGs drawn uniformly at random from the library, labelled by hand
(from thumbnails) with the vocabulary (39 keywords have at least 3 positives), precision / recall
of the suggestions:

| minProbability | threshold 0.3 | 0.5 | 0.8 |
|---|---|---|---|
| 0.001 (default) | P 0.53 R 0.40 | **P 0.58 R 0.39** (2.5 / photo) | P 0.63 R 0.23 |
| 0.003 | P 0.64 R 0.27 | P 0.67 R 0.27 | P 0.78 R 0.17 (0.8 / photo) |
| 0.006 | P 0.70 R 0.20 | P 0.72 R 0.20 | P 0.79 R 0.14 |
| 0.012 | P 0.72 R 0.13 | P 0.72 R 0.13 | P 0.79 R 0.11 |

Raising the threshold alone does not help precision; the probability gate does, at the price of
recall. The `autoAccept` setting therefore also requires a probability of 0.006 (precision about
0.8, recall about 0.14). Weak spots seen in the sample: very generic keywords (`Scene|Interior`,
`Scene|Outdoor`, `Subject|Portrait`) are missed (a z-score cannot flag something half the photos
have), and `Mood|…` entries are unreliable.
Learned probes (5 860 indexed photos, folders held out, target precision 0.8): `ducasse` recall
0.70, `fee sucree` 0.81, `Gl event` 0.49, `ademi` 0.41, `Mayagri` 0.28, `karim` 0.10, `JS Blervaque`
0.08; 21 other keywords were not kept (too few photos, too frequent, or never reaching 0.8). Note
that several are people's names: a probe on a name is a similarity guess, not face recognition;
that is the people feature's job.
The index used for these numbers is a sample (stratified by keyword, 5 860 of 27 786 photos); the
precision on the whole library is lower than on the sample for the learned probes.

**Alt text.** `alt.suggest` writes nothing unless `apply: true` (and only into an empty alt text):
a *template* over the photo's suggested keywords ("Photo of a child and a cake, a garden."). **No
caption model is used**: candle's caption models (BLIP, Moondream) live in `candle-transformers`,
which this workspace does not depend on (and the web / pure-Rust constraints keep it out); adding
BLIP (BSD-3, ~1 GB) is the next step if the template proves too poor. Its quality was not scored
beyond the keyword precision above (130 of 144 sample photos got a sentence; a wrong keyword gives
a wrong sentence): review before publishing.

## 3. UI
**Photo ▸ Sorting Help** (⌥⌘A) gathers the three helpers in one window: *Assisted Culling*, *Suggested
picks* and *Suggested keywords*, each with how well it was measured to work (the numbers above) and
the same commands underneath; nothing is rated, flagged or keyworded until a button says so.
- *Suggested keywords* → *Suggest* runs `keywords.suggest` in the background for the selection (loads
  the search model on first use, about a minute; the window stays usable, Cancel drops the answer) →
  click a chip to add it (one undo step per keyword), *Add all*. The Keywords panel keeps the off-by-
  default box "Add suggestions above 90 % automatically", and its *Suggest* button opens Sorting Help
  at this section.
- *Suggested picks* → *Show Suggested Picks* filters the grid to the unrated photos the taste model
  likes best, after the warning about its measured skill. The filter bar's **Suggested picks** button
  opens Sorting Help at this section.
- Control channel: `dialog.cull {section?: "culling" | "picks" | "keywords"}` opens it;
  `keywords.suggest {background: true, wait?: true}` runs the same in the background (`ui.inspect` →
  `taskProgress`, `lastTask`).

## 4. Commands
`taste.train`, `taste.status`, `culling.suggest`, `culling.accept`, `keywords.suggest`,
`keywords.accept`, `keywords.vocabulary`, `keywords.learn`, `keywords.settings`, `keywords.evaluate`,
`alt.suggest`. Files next to the vectors in `<library>/search/`: `taste.json`, `taste-extras.bin`,
`vocabulary.json`, `keyword-probes.json`, `keywords-settings.json`, `vocab-text-<model>.json`.
Libraries that don't have them behave exactly as before.
