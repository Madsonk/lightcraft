//! What is said in a video. `mlx_whisper` (Whisper large-v3-turbo on Apple silicon, one process at a
//! time) listens to the soundtrack that FFmpeg extracted; its sentences are cleaned of the loops and
//! stock phrases Whisper invents on music and silence ([`clean`]) and kept next to the video's
//! pictures as `speech.json`. [`find`] searches them by *words*, in three levels and with no stemming
//! of English (the library is French and English): the sentence that holds every word, the words
//! within a few seconds of each other, the words anywhere in the video.

use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The file of a video's sentences, in its folder.
pub const FILE: &str = "speech.json";
const VERSION: u32 = 1;
/// The model asked of `mlx_whisper` (a Hugging Face repository or a folder).
pub const DEFAULT_MODEL: &str = "mlx-community/whisper-large-v3-turbo";
/// Environment variable naming `mlx_whisper` (full path).
pub const WHISPER_ENV: &str = "LIGHTCRAFT_MLX_WHISPER";
/// Seconds within which the words of a query make level 2.
pub const WINDOW_MS: u64 = 8_000;

/// One sentence, with its start and end in milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Utterance {
    pub t0: u64,
    pub t1: u64,
    pub text: String,
}

/// `speech.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Speech {
    pub version: u32,
    pub content_key: String,
    pub model: String,
    /// The language asked (`auto` = Whisper's guess) and the one it found.
    pub language: String,
    pub utterances: Vec<Utterance>,
    /// Sentences dropped by the filter (loops, stock phrases, silence).
    #[serde(default)]
    pub dropped: usize,
}

impl Speech {
    pub fn load(dir: &Path) -> Option<Speech> {
        let s: Speech = serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()?;
        (s.version == VERSION).then_some(s)
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(format!("{FILE}.part"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| format!("{}: {e}", dir.display()))
    }
}

/// A sentence as `mlx_whisper` writes it (`--output-format json`).
#[derive(Deserialize)]
pub struct RawSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub no_speech_prob: Option<f64>,
    #[serde(default)]
    pub avg_logprob: Option<f64>,
    #[serde(default)]
    pub compression_ratio: Option<f64>,
}

#[derive(Deserialize)]
struct RawOutput {
    #[serde(default)]
    segments: Vec<RawSegment>,
    #[serde(default)]
    language: String,
}

/// Phrases Whisper writes over silence and music (subtitle credits, "thanks for watching").
const STOCK: &[&str] = &[
    "sous titres realises par la communaute d amara org",
    "sous titrage st 501",
    "sous titrage societe radio canada",
    "merci d avoir regarde cette video",
    "merci d avoir regarde",
    "thanks for watching",
    "thank you for watching",
    "subtitles by the amara org community",
    "please subscribe",
];

fn norm(s: &str) -> String {
    crate::describe::fold(s)
}

/// The sentences worth keeping: no stock phrase, no loop (the same sentence more than twice in a
/// row, or more than four times in the video), nothing Whisper itself flags as silence or noise.
/// Returns the kept sentences and how many were dropped.
pub fn clean(raw: Vec<RawSegment>) -> (Vec<Utterance>, usize) {
    let mut out: Vec<Utterance> = Vec::new();
    let mut dropped = 0usize;
    let mut run = 0usize;
    let mut last = String::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for seg in raw {
        let (t0, t1) = ((seg.start * 1000.0).round() as u64, (seg.end * 1000.0).round() as u64);
        let (no_speech, logprob, ratio) = (seg.no_speech_prob, seg.avg_logprob, seg.compression_ratio);
        let text = seg.text.trim().to_string();
        let n = norm(&text);
        let letters = n.chars().filter(|c| c.is_alphanumeric()).count();
        let silent = no_speech.is_some_and(|p| p > 0.6) && logprob.is_some_and(|l| l < -1.0);
        let looping = ratio.is_some_and(|r| r > 2.4);
        let stock = STOCK.iter().any(|s| n == *s || n.starts_with(s));
        if letters < 2 || silent || looping || stock {
            dropped += 1;
            continue;
        }
        run = if n == last { run + 1 } else { 1 };
        last = n.clone();
        let times = seen.entry(n).or_default();
        *times += 1;
        if run > 2 || *times > 4 {
            dropped += 1;
            continue;
        }
        out.push(Utterance { t0, t1: t1.max(t0), text });
    }
    (out, dropped)
}

/// Sentences of `mlx_whisper`'s JSON output: `(sentences, dropped, language)`.
pub fn parse(json: &[u8]) -> Result<(Vec<Utterance>, usize, String), String> {
    let o: RawOutput = serde_json::from_slice(json).map_err(|e| format!("unreadable transcript: {e}"))?;
    let raw = o.segments.into_iter().filter(|s| s.start.is_finite() && s.end.is_finite() && s.start >= 0.0).collect();
    let (u, dropped) = clean(raw);
    Ok((u, dropped, o.language))
}

/// `mlx_whisper`, if installed.
#[cfg(not(target_arch = "wasm32"))]
pub fn whisper() -> Option<PathBuf> {
    super::ffmpeg::find("mlx_whisper", WHISPER_ENV)
}

/// Longest the soundtrack of one video may take to extract, and the transcription of it.
#[cfg(not(target_arch = "wasm32"))]
const AUDIO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20 * 60);
#[cfg(not(target_arch = "wasm32"))]
const WHISPER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90 * 60);

/// Listen to `video` and write its `speech.json` into `dir`. `language`: `auto` or a code (`fr`,
/// `en`). The soundtrack (16 kHz mono WAV) is a temporary file in `dir`, removed afterwards.
#[cfg(not(target_arch = "wasm32"))]
pub fn transcribe(video: &Path, content_key: &str, dir: &Path, model: &str, language: &str) -> Result<Speech, String> {
    use std::process::Command;

    use super::ffmpeg;
    let ff = ffmpeg::ffmpeg().ok_or_else(|| format!("{} (set {})", ffmpeg::NOT_FOUND, ffmpeg::FFMPEG_ENV))?;
    let mw = whisper().ok_or_else(|| format!("mlx_whisper was not found (pip install mlx-whisper, or set {WHISPER_ENV})"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let wav = dir.join("speech.wav.part");
    let out_dir = dir.join("speech.tmp");
    let _ = std::fs::remove_dir_all(&out_dir);
    let cleanup = || {
        let _ = std::fs::remove_file(&wav);
        let _ = std::fs::remove_dir_all(&out_dir);
    };
    let mut cmd = Command::new(ff);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(format!("file:{}", video.display()))
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ar", "16000", "-ac", "1", "-f", "wav", "-y"])
        .arg(format!("file:{}", wav.display()));
    let r = ffmpeg::run(cmd, AUDIO_TIMEOUT);
    let size = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
    match r {
        Ok(o) if o.ok && size > 44 => {}
        Ok(o) => {
            cleanup();
            // no soundtrack is not a failure: the video says nothing
            if o.stderr.contains("matches no streams") || o.stderr.contains("does not contain any stream") {
                let s = Speech {
                    version: VERSION,
                    content_key: content_key.into(),
                    model: model.into(),
                    language: language.into(),
                    ..Default::default()
                };
                s.save(dir)?;
                return Ok(s);
            }
            return Err(format!("no soundtrack could be read: {}", o.stderr.lines().next().unwrap_or("")));
        }
        Err(e) => {
            cleanup();
            return Err(e);
        }
    }
    let mut cmd = Command::new(mw);
    cmd.arg(&wav)
        .args(["--model", model, "--output-format", "json", "--verbose", "False", "--condition-on-previous-text", "False", "--output-dir"])
        .arg(&out_dir);
    if language != "auto" {
        cmd.args(["--language", language]);
    }
    let r = ffmpeg::run(cmd, WHISPER_TIMEOUT);
    let json = std::fs::read_dir(&out_dir)
        .ok()
        .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "json")))
        .and_then(|p| std::fs::read(p).ok());
    cleanup();
    let json = match (r, json) {
        (Ok(o), Some(j)) if o.ok => j,
        (Ok(o), _) => return Err(format!("mlx_whisper failed: {}", o.stderr.lines().last().unwrap_or(""))),
        (Err(e), _) => return Err(e),
    };
    let (utterances, dropped, found) = parse(&json)?;
    let s = Speech {
        version: VERSION,
        content_key: content_key.into(),
        model: model.into(),
        language: if language == "auto" { found } else { language.into() },
        utterances,
        dropped,
    };
    s.save(dir)?;
    Ok(s)
}

/// How a query's words were found in what is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Every word in one sentence.
    Line = 1,
    /// Every word within [`WINDOW_MS`] of each other.
    Window = 2,
    /// Every word somewhere in the video.
    Video = 3,
}

/// A place where the words of a query are said.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub level: Level,
    pub t0: u64,
    pub t1: u64,
    pub text: String,
}

/// Whether the folded `term` is a form of the folded `word` (a short term must begin the word, a
/// long one shares its first five letters: « conduit » ~ « conduisant »). No stemming of English.
pub fn same_word(term: &str, word: &str) -> bool {
    if term.chars().count() <= 5 {
        return word.starts_with(term);
    }
    term.chars().zip(word.chars()).take_while(|(a, b)| a == b).count() >= 5
}

fn has(terms: &[String], text: &str) -> usize {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    terms.iter().filter(|t| words.iter().any(|w| same_word(t, w))).count()
}

/// Where `terms` (folded, all required) are said, best level first and in time order within a level.
/// A sentence that is a level-1 hit is not repeated at level 2, nor either in level 3.
pub fn find(utts: &[Utterance], terms: &[String], window_ms: u64) -> Vec<Hit> {
    if terms.is_empty() {
        return Vec::new();
    }
    let folded: Vec<String> = utts.iter().map(|u| norm(&u.text)).collect();
    let mut hits = Vec::new();
    let mut line_idx = Vec::new();
    for (i, u) in utts.iter().enumerate() {
        if has(terms, &folded[i]) == terms.len() {
            hits.push(Hit { level: Level::Line, t0: u.t0, t1: u.t1, text: u.text.clone() });
            line_idx.push(i);
        }
    }
    let mut covered_until = 0u64;
    for (i, u) in utts.iter().enumerate() {
        if line_idx.contains(&i) || u.t0 < covered_until {
            continue;
        }
        let upto = u.t0 + window_ms;
        let span: Vec<usize> = (i..utts.len()).take_while(|j| utts[*j].t0 <= upto).collect();
        let joined = span.iter().map(|j| folded[*j].as_str()).collect::<Vec<_>>().join(" ");
        // a window that starts on a sentence with one of the words, so it is not found from every sentence before it
        if has(terms, &folded[i]) > 0 && has(terms, &joined) == terms.len() {
            let last = span.last().copied().unwrap_or(i);
            hits.push(Hit {
                level: Level::Window,
                t0: u.t0,
                t1: utts[last].t1,
                text: span.iter().map(|j| utts[*j].text.as_str()).collect::<Vec<_>>().join(" "),
            });
            covered_until = utts[last].t1;
        }
    }
    if hits.is_empty() {
        let all = folded.join(" ");
        if has(terms, &all) == terms.len() {
            if let Some(u) = utts.iter().zip(&folded).find(|(_, f)| has(terms, f) > 0).map(|(u, _)| u) {
                hits.push(Hit { level: Level::Video, t0: u.t0, t1: u.t1, text: u.text.clone() });
            }
        }
    }
    hits.sort_by_key(|h| (h.level, h.t0));
    hits
}

/// Seconds between `ms` and the hit's span (0 inside it).
pub fn distance_ms(h: &Hit, ms: u64) -> u64 {
    if ms < h.t0 { h.t0 - ms } else { ms.saturating_sub(h.t1) }
}

/// A transcription run: progress and what each video gave (sentences kept, or why not). One video
/// at a time: the model fills the machine's memory and its graphics.
#[derive(Default)]
pub struct Job {
    pub running: std::sync::atomic::AtomicBool,
    pub cancel: std::sync::atomic::AtomicBool,
    pub total: std::sync::atomic::AtomicU64,
    pub done: std::sync::atomic::AtomicU64,
    pub runs: std::sync::atomic::AtomicU64,
    state: std::sync::Mutex<(Vec<(u64, Result<usize, String>)>, Option<String>)>,
}

impl Job {
    fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<(u64, Result<usize, String>)>, Option<String>)> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn begin(&self, total: usize) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        if self.running.swap(true, SeqCst) {
            return false;
        }
        self.cancel.store(false, SeqCst);
        self.total.store(total as u64, SeqCst);
        self.done.store(0, SeqCst);
        *self.lock() = (Vec::new(), None);
        true
    }

    pub fn record(&self, id: u64, r: Result<usize, String>) {
        self.done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lock().0.push((id, r));
    }

    pub fn finish(&self, error: Option<String>) {
        use std::sync::atomic::Ordering::SeqCst;
        self.lock().1 = error;
        self.runs.fetch_add(1, SeqCst);
        self.running.store(false, SeqCst);
    }

    pub fn cancel_run(&self) -> bool {
        let running = self.running.load(std::sync::atomic::Ordering::SeqCst);
        if running {
            self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        running
    }

    pub fn report(&self) -> serde_json::Value {
        use std::sync::atomic::Ordering::SeqCst;
        let g = self.lock();
        serde_json::json!({
            "running": self.running.load(SeqCst), "cancelled": self.cancel.load(SeqCst),
            "total": self.total.load(SeqCst), "done": self.done.load(SeqCst),
            "sentences": g.0.iter().filter_map(|(_, r)| r.as_ref().ok()).sum::<usize>(),
            "failed": g.0.iter().filter(|(_, r)| r.is_err()).map(|(i, r)| serde_json::json!({"id": i, "error": r.as_ref().err()})).collect::<Vec<_>>(),
            "error": g.1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(t0: u64, t1: u64, text: &str, no_speech: Option<f64>, logprob: Option<f64>, ratio: Option<f64>) -> RawSegment {
        RawSegment {
            start: t0 as f64 / 1000.0,
            end: t1 as f64 / 1000.0,
            text: text.into(),
            no_speech_prob: no_speech,
            avg_logprob: logprob,
            compression_ratio: ratio,
        }
    }

    fn u(t0: u64, text: &str) -> Utterance {
        Utterance { t0, t1: t0 + 2000, text: text.into() }
    }

    #[test]
    fn whisper_inventions_are_dropped() {
        let raw = vec![
            seg(0, 2000, "  Bonjour tout le monde ", Some(0.1), Some(-0.3), Some(1.2)),
            seg(2000, 4000, "Sous-titres réalisés par la communauté d'Amara.org", None, None, None),
            seg(4000, 5000, "Wow.", None, None, None),
            seg(5000, 6000, "Wow.", None, None, None),
            seg(6000, 7000, "Wow.", None, None, None),
            seg(7000, 8000, "Wow.", None, None, None),
            seg(8000, 9000, "ok", Some(0.9), Some(-1.5), None),
            seg(9000, 9500, ".", None, None, None),
            seg(9500, 12000, "la la la la la la la", None, None, Some(3.1)),
            seg(12000, 14000, "On part demain matin", None, None, None),
        ];
        let (u, dropped) = clean(raw);
        let texts: Vec<&str> = u.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["Bonjour tout le monde", "Wow.", "Wow.", "On part demain matin"], "{texts:?}");
        assert_eq!(dropped, 6);
    }

    #[test]
    fn a_phrase_said_again_and_again_far_apart_is_a_loop_too() {
        let raw: Vec<_> = (0..8u64)
            .flat_map(|i| {
                [
                    seg(i * 4000, i * 4000 + 1000, "Merci.", None, None, None),
                    seg(i * 4000 + 1000, i * 4000 + 2000, &format!("phrase {i} différente"), None, None, None),
                ]
            })
            .collect();
        let (u, _) = clean(raw);
        assert_eq!(u.iter().filter(|x| x.text == "Merci.").count(), 4);
    }

    #[test]
    fn mlx_whisper_json_is_read() {
        let j = r#"{"text":"x","language":"fr","segments":[{"start":0.0,"end":2.5,"text":" Il conduit une voiture.","no_speech_prob":0.01,"avg_logprob":-0.2,"compression_ratio":1.1},{"start":3.0,"end":2.0,"text":"inversé"}]}"#.as_bytes();
        let (u, dropped, lang) = parse(j).unwrap();
        assert_eq!((lang.as_str(), dropped), ("fr", 0));
        assert_eq!(u[0], Utterance { t0: 0, t1: 2500, text: "Il conduit une voiture.".into() });
        assert_eq!(u[1].t1, 3000, "an end before the start is the start");
        assert!(parse(b"nope").is_err());
        assert!(parse(r#"{"segments":[{"start":-1,"end":2,"text":"avant zéro"}]}"#.as_bytes()).unwrap().0.is_empty());
    }

    #[test]
    fn words_are_found_in_three_levels() {
        let us = vec![
            u(0, "Bonjour à tous"),
            u(10_000, "Il conduit la camionnette"),
            u(12_500, "vers la plage"),
            u(40_000, "Une voiture rouge"),
            u(60_000, "Ensuite il conduit"),
            u(200_000, "La voiture est garée"),
        ];
        let t = |s: &[&str]| -> Vec<String> { s.iter().map(|x| x.to_string()).collect() };
        // level 1: both words in one sentence
        let h = find(&us, &t(&["conduit", "camionnette"]), WINDOW_MS);
        assert_eq!((h.len(), h[0].level, h[0].t0), (1, Level::Line, 10_000));
        // level 2: the words in neighbouring sentences
        let h = find(&us, &t(&["camionnette", "plage"]), WINDOW_MS);
        assert_eq!((h[0].level, h[0].t0), (Level::Window, 10_000), "{h:?}");
        // level 3: far apart, still in the video — and only then
        let h = find(&us, &t(&["plage", "garee"]), WINDOW_MS);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].level, Level::Video);
        // the stem is not trimmed in English: « voiture » finds « voiture », not « voitures » by chopping
        assert_eq!(find(&us, &t(&["voiture"]), WINDOW_MS).len(), 2);
        assert!(find(&us, &t(&["moto"]), WINDOW_MS).is_empty());
        assert!(find(&us, &[], WINDOW_MS).is_empty());
        // a French form still matches by its beginning
        assert!(!find(&us, &t(&["conduisant"]), WINDOW_MS).is_empty());
        // « danse » is not « dans »
        assert!(!same_word("danse", "dans") && same_word("danse", "dansent"));
    }

    #[test]
    fn the_hit_is_near_a_picture_when_the_picture_is_within_the_span() {
        let h = Hit { level: Level::Line, t0: 10_000, t1: 12_000, text: String::new() };
        assert_eq!((distance_ms(&h, 11_000), distance_ms(&h, 8_000), distance_ms(&h, 15_000)), (0, 2_000, 3_000));
    }
}
