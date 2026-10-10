//! Taste and automatic keywords: `taste.*`, `culling.suggest` / `culling.accept`, `keywords.*`
//! (the model and the zero-shot work live in `crate::search::taste` / `crate::search::keywords`,
//! cargo feature `search`). Everything here *suggests*; only the `*.accept` commands write.

use serde_json::Value;

use super::{CommandSpec, always, cmd};
#[cfg(not(feature = "search"))]
use crate::EngineError;
use crate::{Result, Session};

#[cfg(not(feature = "search"))]
fn off(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other(crate::search::UNAVAILABLE.into()))
}

macro_rules! gated {
    ($name:ident, $path:path) => {
        #[cfg(feature = "search")]
        fn $name(s: &mut Session, p: &Value) -> Result<Value> {
            $path(s, p)
        }
        #[cfg(not(feature = "search"))]
        fn $name(s: &mut Session, p: &Value) -> Result<Value> {
            off(s, p)
        }
    };
}

gated!(taste_train, crate::search::taste::train);
gated!(taste_status, crate::search::taste::status);
gated!(culling_suggest, crate::search::taste::suggest);
gated!(culling_accept, crate::search::taste::accept);
gated!(kw_suggest, crate::search::keywords::suggest);
gated!(kw_accept, crate::search::keywords::accept);
gated!(kw_vocab, crate::search::keywords::vocabulary);
gated!(kw_learn, crate::search::keywords::learn);
gated!(kw_settings, crate::search::keywords::settings);
gated!(alt_suggest, crate::search::keywords::alt_suggest);
gated!(kw_eval, crate::search::keywords::evaluate);

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "taste.train",
            "Learn My Taste",
            [],
            None,
            "{ids? (default: the library), implicitNegatives?: bool (true: unrated photos of shoots you clearly went through count as \"not chosen\", weight 0.4), extras?: bool (false: add focus / clipping / exposure features: measured useless on the author's archive and slow to compute the first time), index?: bool (false: embed the rated photos that have no vector yet first), folds?: 2..10 (5), minRated?: 5.. (30: refuse to learn from fewer rated photos), split?: \"shoot\" (default: whole folders held out = a shoot the model never saw) | \"burst\" (whole 5-minute windows of a folder: other moments of a shoot you partly rated) | \"photo\" (at random: near-identical frames leak, an optimistic upper bound), lambdas?: [..], save?: bool (true)} — learn from your stars and rejects a ridge model on the SigLIP 2 embeddings (+ technical features); the regularisation is chosen by k-fold cross-validation over whole shoots and the numbers measured on photos it did not see are returned and stored with the model (taste.json in <library>/search). Writes no rating → {trained, counts: {rated, rejected, implicit, groups, stars}, metrics: {spearmanRated, spearmanAll, aucChosen, topKHitRate, topKChance, spearmanSharpnessOnly, lambda, tried}, unindexedLabelled, saved, seconds}",
            always,
            taste_train
        ),
        cmd!(
            query "taste.status",
            "Taste Model Status",
            [],
            None,
            "{} → {trained, model: {trained, embedding, counts, metrics}, ratedPhotos, reviewedFolders}",
            always,
            taste_status
        ),
        cmd!(
            query "culling.suggest",
            "Suggested Picks",
            [],
            None,
            "{ids? (default: the current view), unratedOnly?: bool (true: only photos without stars or flag), fraction?: 0..1 (0.10: the best share is proposed as picks), count?: n (overrides fraction), minScore?: stars, limit?: 1..5000 (100), filter?: bool (false: show only the picks in the grid)} — predict the stars you would give each photo (needs taste.train and the photos indexed by search.index) and propose the best ones; SUGGESTIONS ONLY, nothing is written → {scored, withoutEmbedding, candidates, picks: [ids], photos: [{id, score: predicted stars 0..5, percentile, rating}], model: {trained, rated, spearmanRated, topKHitRate, topKChance}}",
            always,
            culling_suggest
        ),
        cmd!(
            "culling.accept",
            "Apply Suggested Picks",
            [],
            None,
            "{ids, mode?: \"pick\" (flag them as picks) | \"rating\" (stars = the rounded prediction, or `rating`), rating?: 1..5, confirm: true} — the only command that writes what culling.suggest proposed; refuses without `confirm: true` (one undo step per command) → {mode, changed}",
            always,
            culling_accept
        ),
        cmd!(
            "keywords.suggest",
            "Suggested Keywords",
            [],
            None,
            "{ids? (default: the selection), threshold?: score 0..1 (per-keyword calibrated default), limit?: per photo (8), categories?: [..], includeExisting?: bool (false), minProbability?: SigLIP probability a zero-shot match must reach (0.001), autoAccept?: ignored unless the `autoAccept` setting is on} — keywords for photos from the controlled vocabulary (built-in list + yours + keywords learned from your archive) scored on the SigLIP 2 embeddings: zero-shot against text prompts, plus supervised probes for keywords you already use; nothing is written unless the `autoAccept` setting (off by default) is on and the score reaches its threshold → {photos: [{id, suggestions: [{keyword, score, source: \"zero-shot\"|\"learned\", category}]}], applied, indexed, unindexed}",
            always,
            kw_suggest
        ),
        cmd!(
            "keywords.accept",
            "Accept Suggested Keywords",
            [],
            None,
            "{items: [{id, keyword}] | ids + keywords: [..]} — add the chosen keywords (one undo step per call) → {changed, added}",
            always,
            kw_accept
        ),
        cmd!(
            "keywords.vocabulary",
            "Keyword Vocabulary",
            [],
            None,
            "{add?: [{keyword, prompts: [..], category?}], remove?: [keyword], language?: \"en\"|\"fr\"} — read or extend the controlled vocabulary (yours is kept in <library>/search/vocabulary.json) → {language, categories, entries, learned}",
            always,
            kw_vocab
        ),
        cmd!(
            "keywords.learn",
            "Learn Keywords from Archive",
            [],
            None,
            "{keywords?: [..] (default: every keyword used on at least `minPhotos` indexed photos), minPhotos?: 20, folds?: 5, precision?: 0.8 target} — fit one probe per keyword already in the library on the embeddings, cross-validate it and keep the score above which it reached the target precision; reports precision / recall per keyword → {keywords: [{keyword, photos, precision, recall, threshold, kept}]}",
            always,
            kw_learn
        ),
        cmd!(
            query "keywords.settings",
            "Keyword Suggestion Settings",
            [],
            None,
            "{autoAccept?: bool (default false), autoThreshold?: 0..1 (0.9), language?: \"en\"|\"fr\"} — read / change the settings kept in <library>/search/keywords-settings.json → {autoAccept, autoThreshold, language}",
            always,
            kw_settings
        ),
        cmd!(
            "alt.suggest",
            "Suggested Alt Text",
            [],
            None,
            "{ids? (default: the selection), apply?: bool (false: write it as the photo's alt text, only if empty)} — a short alt text built from the photo's keywords, suggested keywords and capture facts (a template, no caption model: documented in docs/taste-and-keywords.md) → {photos: [{id, altText}], applied}",
            always,
            alt_suggest
        ),
        cmd!(
            query "keywords.evaluate",
            "Evaluate Keyword Suggestions",
            [],
            None,
            "{keywords?: [..], threshold?} — precision and recall of the suggestions on the photos of the library that already carry those keywords → {keywords: [{keyword, tp, fp, fn, precision, recall}]}",
            always,
            kw_eval
        ),
    ]
}
