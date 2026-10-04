//! Measuring how fast and how accurately an engine runs **on this machine**.
//!
//! The catalog ([`crate::model::catalog`]) answers "how fast will this probably be?" with an
//! estimate. This module answers "how fast is it, here?" with a measurement. Keeping the two
//! apart is the point: a number produced here has actually been timed, and a number produced
//! there has not.
//!
//! Both the CLI (`lw bench --quick`) and the desktop app's benchmark panel call [`measure`], so
//! the two can never disagree about what a run means.
//!
//! Accuracy is only reported when the clips carry reference transcripts. Synthesized audio has
//! none, and [`measure`] returns `wer: None` rather than inventing one.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::audio::AudioBuffer;
use crate::engine::SpeechEngine;
use crate::error::Error;

/// Where a clip set came from, which decides whether WER is computable at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipSource {
    /// Real speech from a fixtures directory (reference transcripts available, so WER is real).
    Fixtures {
        /// The directory the clips were read from.
        dir: PathBuf,
    },
    /// Deterministically synthesized speech-like audio (no reference, so no WER).
    Synthetic,
}

impl ClipSource {
    /// One-line human description, including the honesty caveat for synthetic audio.
    pub fn describe(&self) -> String {
        match self {
            ClipSource::Fixtures { dir } => format!("{} (real speech)", dir.display()),
            ClipSource::Synthetic => "synthesized speech-like audio (no reference transcripts, so no WER; \
                 the decoder emits few tokens, making RTF a lower bound)"
                .to_string(),
        }
    }

    /// Whether clips from this source can produce a word error rate.
    pub fn has_references(&self) -> bool {
        matches!(self, ClipSource::Fixtures { .. })
    }
}

/// One clip to measure against.
#[derive(Clone, Debug)]
pub struct Clip {
    /// File name (or synthetic label) shown in output.
    pub name: String,
    /// The decoded audio.
    pub audio: AudioBuffer,
    /// Duration in seconds, used as the RTF denominator.
    pub duration_s: f32,
    /// Reference transcript, when one exists.
    pub reference: Option<String>,
    /// BCP-47-ish language tag of the speech, when known (`"en"`, `"ru"`, …).
    ///
    /// Used to decide whether scoring this clip against a given engine means anything: see
    /// [`measure`].
    pub language: Option<String>,
}

/// One clip's measured result.
#[derive(Clone, Debug, Serialize)]
pub struct ClipResult {
    /// Clip name.
    pub name: String,
    /// Audio duration in seconds.
    pub duration_s: f32,
    /// Wall time for the transcription in milliseconds.
    pub ms: f32,
    /// Real-time factor: wall seconds per second of audio. Lower is faster.
    pub rtf: f32,
    /// Word error rate against the reference, when the clip had one.
    ///
    /// Always computed, including for a clip whose language the engine does not claim — see
    /// [`ClipResult::scored`] for whether it reached [`Measurement::wer`]. The unscored figure is
    /// the evidence that excluding it was right, so it is reported rather than blanked.
    pub wer: Option<f32>,
    /// The clip's language, when known.
    pub language: Option<String>,
    /// Whether this clip's error rate contributed to [`Measurement::wer`].
    ///
    /// False when the engine does not claim the clip's language — the clip was still transcribed
    /// and timed, but scoring it would measure the wrong thing.
    pub scored: bool,
    /// What `wer` is measured in, chosen from the clip's language. A character rate must never be
    /// displayed as a word rate.
    pub unit: ErrorUnit,
}

/// One total, for the clips scored in one unit.
///
/// A run over Russian and Chinese has two of these: a word rate over the Russian clips and a
/// character rate over the Chinese ones. Both are real; only their average is not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitScore {
    /// What this total is measured in.
    pub unit: ErrorUnit,
    /// Token-weighted error rate over the scored clips that use this unit.
    pub rate: f32,
    /// How many clips contributed.
    pub clips: usize,
    /// How many reference tokens contributed -- words or characters, per `unit`.
    pub tokens: usize,
}

/// WER over the clips of one language.
#[derive(Clone, Debug, Serialize)]
pub struct LanguageScore {
    /// The language tag.
    pub language: String,
    /// Word-weighted WER over this language's clips.
    pub wer: f32,
    /// How many clips contributed.
    pub clips: usize,
    /// How many reference words contributed.
    pub words: usize,
    /// Whether the engine claimed this language.
    pub claimed: bool,
    /// What this language's rate is measured in.
    pub unit: ErrorUnit,
}

/// The result of one measured run. Every field was genuinely timed on the running machine.
#[derive(Clone, Debug, Serialize)]
pub struct Measurement {
    /// Mean RTF over the warm runs (the cold first run is reported separately).
    pub warm_rtf: f32,
    /// Cold (first) run RTF, which includes lazy allocations and cache warm-up.
    pub cold_rtf: f32,
    /// Number of warm runs contributing to `warm_rtf`.
    pub warm_count: usize,
    /// Per-clip warm wall time in ms.
    pub warm_ms: Vec<f32>,
    /// Token-weighted error rate over the **scored** clips, when any had reference transcripts
    /// **and** they all use the same unit. See [`Measurement::unit`] and
    /// [`Measurement::mixed_units`].
    pub wer: Option<f32>,
    /// What [`Measurement::wer`] is measured in, when there is one.
    pub unit: Option<ErrorUnit>,
    /// True when the scored clips spanned both words and characters, so no single total exists.
    ///
    /// Not an error: it means the run covered, say, Russian and Chinese at once. It is also not a
    /// reason to report nothing -- see [`Measurement::by_unit`], which holds a real total for each
    /// unit separately. Only their average is meaningless.
    pub mixed_units: bool,
    /// One total per unit the scored clips used, words first.
    ///
    /// This is what a caller shows when `wer` is `None`. Withholding the blend was right;
    /// withholding the two figures that make it up was not, and it left the model list printing a
    /// dash for every model that claims Chinese -- which reads as "the benchmark failed" rather
    /// than "there are two answers here".
    pub by_unit: Vec<UnitScore>,
    /// Languages the scored clips covered, sorted, for provenance.
    pub scored_languages: Vec<String>,
    /// Clips that were transcribed and timed but not scored, and the languages they were in.
    ///
    /// Empty unless [`MeasureOptions::include_unclaimed`] was set. By default such clips are not
    /// run at all, and their languages appear in [`Measurement::skipped_languages`] instead.
    pub unscored_languages: Vec<String>,
    /// Languages whose clips were **not run at all**, because the engine does not claim them.
    ///
    /// Different from `unscored_languages` in the way that matters: these never reached the
    /// engine, so they consumed no time and entered no average. An English-only model asked to
    /// transcribe Chinese still takes however long it takes to produce nonsense, and folding that
    /// into its RTF reports a speed the user will never see on audio the model is for.
    pub skipped_languages: Vec<String>,
    /// How many clips `skipped_languages` accounts for.
    pub skipped_clips: usize,
    /// WER broken down per language, over every clip that had a reference — claimed or not.
    ///
    /// This is what makes a misfit visible: a Russian-only model run against multilingual
    /// fixtures shows `ru` near zero and the rest near one, instead of one blended number that
    /// describes neither.
    pub per_language: Vec<LanguageScore>,
    /// Whether the engine declared any languages at all.
    ///
    /// False means it made no claim — not that it supports nothing. A single word-weighted total
    /// over several languages is not meaningful for such a model, and callers should say so
    /// rather than print one.
    pub engine_claimed_languages: bool,
    /// Every clip's individual result, in run order.
    pub results: Vec<ClipResult>,
    /// Total audio seconds run.
    pub audio_secs: f32,
    /// Where the audio came from.
    pub source: ClipSource,
}

/// How a measurement treats clips in languages the engine never claimed.
#[derive(Clone, Debug, Default)]
pub struct MeasureOptions {
    /// Languages to treat as the engine's claim when the engine itself declares none.
    ///
    /// Several real exports carry no language metadata at all -- a bare `encoder/decoder/joiner`
    /// transducer directory says nothing about what it speaks -- so `supported_languages()` comes
    /// back empty and every clip counts. For GigaAM v3 (Russian) and Parakeet TDT-CTC 110M
    /// (English) that meant being timed on Chinese, Ukrainian and Spanish audio they were never
    /// built for, which is the same corruption `include_unclaimed` exists to prevent, arriving by
    /// a different road.
    ///
    /// The catalog does know their languages, and it is what the UI already shows the user, so a
    /// caller holding a catalog entry passes them here. Empty means "no help available", and the
    /// old behaviour stands: an engine that claims nothing is scored on everything.
    pub assume_languages: Vec<String>,
    /// Run the engine on clips whose language it does not claim, instead of skipping them.
    ///
    /// Off by default, because including them corrupts every timing in the report: the clips are
    /// still transcribed at whatever speed the model manages on audio it was never built for, and
    /// that time lands in `cold_rtf`, `warm_rtf` and `audio_secs` alongside the real work. A
    /// caller comparing two models then compares them partly on languages neither is for.
    ///
    /// Worth switching on for exactly one purpose: seeing what a model actually does with a
    /// language it does not advertise. That is a legitimate question, and the answer describes
    /// the question rather than the model, so it has to be asked deliberately.
    pub include_unclaimed: bool,
}

/// Run every clip the engine claims once (the first is the cold run) and collect honest timings.
///
/// `per_clip` is called as each clip finishes so a caller can stream progress; it receives the
/// same values that end up in [`Measurement::results`].
///
/// Clips in languages the engine does not claim are skipped -- see [`measure_with`] to include
/// them.
pub fn measure(
    engine: &mut dyn SpeechEngine,
    clips: &[Clip],
    source: ClipSource,
    per_clip: impl FnMut(&ClipResult),
) -> Result<Measurement> {
    measure_with(engine, clips, source, MeasureOptions::default(), per_clip)
}

/// [`measure`], with control over the clips the engine does not claim.
pub fn measure_with(
    engine: &mut dyn SpeechEngine,
    clips: &[Clip],
    source: ClipSource,
    opts: MeasureOptions,
    mut per_clip: impl FnMut(&ClipResult),
) -> Result<Measurement> {
    if clips.is_empty() {
        return Err(Error::Other("benchmark needs at least one clip".into()));
    }

    // Which fixture languages this engine actually claims.
    //
    // Scoring a model on a language it never advertised does not measure the model, it measures
    // the question. Moonshine tiny en scores WER 0.092 on English clips and 0.850 once Russian,
    // Spanish and Ukrainian are added -- individual clips exceed 1.0, because it inserts more
    // words than the reference holds. The first number describes the model; the second describes
    // a mistake.
    //
    // Those clips are therefore not run at all, rather than run and left out of the WER. Running
    // them costs time that lands in cold_rtf, warm_rtf and audio_secs, where nothing separates it
    // from the real work afterwards -- so the speed figure would be part measurement of the model
    // and part measurement of the model failing at something it never offered.
    //
    // An engine that declares no languages has made no claim, so everything counts.
    let engine_claimed: Vec<String> = engine
        .supported_languages()
        .iter()
        .map(|l| l.0.to_ascii_lowercase())
        .collect();
    let engine_claimed_languages = !engine_claimed.is_empty();
    // Fall back to what the caller knows only when the engine itself knows nothing. An engine
    // that does declare its languages is the better authority: it was built from the files that
    // are actually on disk, while a catalog entry is a description of them.
    let claimed: Vec<String> = if engine_claimed.is_empty() {
        opts.assume_languages
            .iter()
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty())
            .collect()
    } else {
        engine_claimed
    };
    let counts = |clip: &Clip| -> bool {
        match (&clip.language, claimed.is_empty()) {
            (_, true) | (None, _) => true,
            (Some(lang), false) => {
                let lang = lang.to_ascii_lowercase();
                claimed.contains(&lang)
            }
        }
    };

    // Decide the run set before timing anything, so the cold run is a clip that counts.
    let (selected, skipped): (Vec<&Clip>, Vec<&Clip>) = if opts.include_unclaimed {
        (clips.iter().collect(), Vec::new())
    } else {
        clips.iter().partition(|c| counts(c))
    };
    let mut skipped_languages: Vec<String> = Vec::new();
    for clip in &skipped {
        if let Some(lang) = &clip.language
            && !skipped_languages.iter().any(|l| l == lang)
        {
            skipped_languages.push(lang.clone());
        }
    }
    skipped_languages.sort();
    if selected.is_empty() {
        // Better to say why than to report an empty run or, worse, a number from clips the model
        // never claimed.
        return Err(Error::Other(format!(
            "this model claims {}, and none of the {} clip(s) are in those languages -- they are in {}.              Nothing was measured: a figure from clips a model does not claim describes the fixtures, not the model.",
            if claimed.is_empty() {
                "nothing".to_string()
            } else {
                claimed.join("/")
            },
            clips.len(),
            if skipped_languages.is_empty() {
                "no declared language".to_string()
            } else {
                skipped_languages.join("/")
            },
        )));
    }

    let mut cold_rtf = 0.0f32;
    let mut warm_ms = Vec::new();
    let mut warm_rtf_sum = 0.0f64;
    // unit -> (weighted error, tokens, clips). One entry per unit that actually scored something.
    let mut totals: std::collections::BTreeMap<ErrorUnit, (f64, usize, usize)> =
        std::collections::BTreeMap::new();
    let mut audio_secs = 0.0f32;
    let mut results = Vec::with_capacity(selected.len());
    let mut scored_languages: Vec<String> = Vec::new();
    let mut unscored_languages: Vec<String> = Vec::new();
    // language -> (weighted error, tokens, clips, unit)
    let mut by_language: std::collections::BTreeMap<String, (f64, usize, usize, ErrorUnit)> =
        std::collections::BTreeMap::new();

    for (i, clip) in selected.iter().copied().enumerate() {
        let scored = counts(clip);
        let t0 = Instant::now();
        let transcript = engine
            .transcribe(&clip.audio)
            .map_err(|e| Error::Engine(e.to_string()))?;
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let rtf = (ms / 1000.0) / clip.duration_s.max(1e-6);
        audio_secs += clip.duration_s;

        // Chinese is not scored in words, Russian is not scored in characters, and the two rates
        // are not the same quantity — so the unit is chosen per clip from its language and carried
        // with the number everywhere it goes.
        let unit = clip.language.as_deref().map_or(ErrorUnit::Word, scoring_unit);

        // The per-clip rate is always computed and reported; only the weighted total is restricted,
        // so a curious caller can still see what an unclaimed language looked like.
        let wer = clip.reference.as_ref().map(|r| {
            let (w, n) = error_rate(r, &transcript.text, unit);
            if scored {
                let slot = totals.entry(unit).or_insert((0.0, 0, 0));
                slot.0 += w as f64 * n as f64;
                slot.1 += n;
                slot.2 += 1;
            }
            // The per-language tally counts every clip that has a reference, claimed or not:
            // seeing what an unclaimed language actually scored is the point of the breakdown.
            let key = clip.language.clone().unwrap_or_else(|| "unknown".to_string());
            let slot = by_language.entry(key).or_insert((0.0, 0, 0, unit));
            slot.0 += w as f64 * n as f64;
            slot.1 += n;
            slot.2 += 1;
            slot.3 = unit;
            w
        });
        if let Some(lang) = &clip.language {
            let bucket = if scored {
                &mut scored_languages
            } else {
                &mut unscored_languages
            };
            if !bucket.iter().any(|l| l == lang) {
                bucket.push(lang.clone());
            }
        }

        let result = ClipResult {
            name: clip.name.clone(),
            duration_s: clip.duration_s,
            ms,
            rtf,
            wer,
            unit,
            language: clip.language.clone(),
            scored,
        };
        per_clip(&result);
        results.push(result);
        if i == 0 {
            cold_rtf = rtf;
        } else {
            warm_ms.push(ms);
            warm_rtf_sum += rtf as f64;
        }
    }

    let warm_count = warm_ms.len();
    let warm_rtf = if warm_count > 0 {
        (warm_rtf_sum / warm_count as f64) as f32
    } else {
        cold_rtf
    };
    scored_languages.sort();
    unscored_languages.sort();
    let per_language = by_language
        .into_iter()
        .map(|(language, (err, words, clips, unit))| LanguageScore {
            wer: if words > 0 {
                (err / words as f64) as f32
            } else {
                0.0
            },
            claimed: claimed.is_empty() || claimed.contains(&language.to_ascii_lowercase()),
            language,
            clips,
            words,
            unit,
        })
        .collect();

    // A blended total exists only when everything in it is the same quantity. Mixing Chinese
    // characters with Russian words would produce a number with no unit and no meaning, so the
    // blend is withheld -- but each unit's own total is kept and reported, because those are
    // ordinary, meaningful numbers.
    let by_unit: Vec<UnitScore> = totals
        .iter()
        .filter(|(_, (_, tokens, _))| *tokens > 0)
        .map(|(unit, (err, tokens, clips))| UnitScore {
            unit: *unit,
            rate: (err / *tokens as f64) as f32,
            clips: *clips,
            tokens: *tokens,
        })
        .collect();
    let mixed_units = by_unit.len() > 1;
    let total_unit = if mixed_units {
        None
    } else {
        by_unit.first().map(|u| u.unit)
    };
    Ok(Measurement {
        warm_rtf,
        cold_rtf,
        warm_count,
        warm_ms,
        wer: (!mixed_units).then(|| by_unit.first().map(|u| u.rate)).flatten(),
        unit: total_unit,
        mixed_units,
        by_unit,
        scored_languages,
        unscored_languages,
        skipped_clips: skipped.len(),
        skipped_languages,
        per_language,
        engine_claimed_languages,
        results,
        audio_secs,
        source,
    })
}

/// What a transcript is compared in.
///
/// Splitting on whitespace is the right unit for languages that write it, and meaningless for the
/// ones that do not. Scoring Chinese by "words" makes every sentence one token, so any imperfect
/// transcript scores 1.0 and a perfect one scores 0.0 — a metric with two values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorUnit {
    /// Whitespace-separated words. Reported as **WER**.
    Word,
    /// Individual characters. Reported as **CER**, and never averaged with a WER.
    Character,
}

impl ErrorUnit {
    /// The abbreviation to print, so a character rate is never labelled as a word rate.
    pub fn label(self) -> &'static str {
        match self {
            ErrorUnit::Word => "WER",
            ErrorUnit::Character => "CER",
        }
    }
}

/// The unit a language should be scored in.
///
/// Character-scored: Chinese and Cantonese (no spaces at all), Japanese (kana and kanji run
/// together), Thai, Lao, Khmer, Burmese and Tibetan (no word delimiter). Korean is **not** in the
/// list: it writes spaces between eojeol, so words are a real unit there.
///
/// An unknown tag scores by word, which is the right default — it is what every European language
/// needs, and a wrong guess here is visible in the number rather than silent.
pub fn scoring_unit(language: &str) -> ErrorUnit {
    let base = language
        .split(['-', '_'])
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase();
    match base.as_str() {
        "zh" | "yue" | "cmn" | "wuu" | "nan" | "hak" | "ja" | "th" | "lo" | "my" | "km" | "bo" => {
            ErrorUnit::Character
        }
        _ => ErrorUnit::Word,
    }
}

/// Word error rate (Levenshtein over normalized words) and the reference word count.
///
/// Normalization lowercases and replaces every non-alphanumeric character with a space, so
/// punctuation and casing never count as errors. Number words still do: a reference reading
/// "2" against a transcript reading "two" scores as a substitution.
pub fn word_error_rate(reference: &str, hypothesis: &str) -> (f32, usize) {
    error_rate(reference, hypothesis, ErrorUnit::Word)
}

/// Levenshtein error rate in `unit`, and the size of the reference in that unit.
///
/// Both units normalize the same way — lowercase, punctuation to whitespace — so casing and
/// commas never count as errors in either. They differ only in what a token is.
pub fn error_rate(reference: &str, hypothesis: &str, unit: ErrorUnit) -> (f32, usize) {
    let norm = |s: &str| -> Vec<String> {
        let cleaned: String = s
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
            .collect();
        match unit {
            ErrorUnit::Word => cleaned.split_whitespace().map(str::to_string).collect(),
            // Whitespace is dropped rather than counted: how a transcript spaces CJK text is a
            // formatting choice, not a recognition error.
            ErrorUnit::Character => cleaned
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| c.to_string())
                .collect(),
        }
    };
    let r = norm(reference);
    let h = norm(hypothesis);
    if r.is_empty() {
        return (if h.is_empty() { 0.0 } else { 1.0 }, 0);
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut cur = vec![0usize; h.len() + 1];
    for (i, rw) in r.iter().enumerate() {
        cur[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            let cost = if rw == hw { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[h.len()] as f32 / r.len() as f32, r.len())
}

/// Load a WAV file (i16 or f32, any channel count) into a mono [`AudioBuffer`] at its own rate.
pub fn load_wav(path: &Path) -> Result<AudioBuffer> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| Error::Audio(format!("{}: {e}", path.display())))?;
    let spec = reader.spec();
    let channels = spec.channels;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Error::Audio(format!("{}: {e}", path.display())))?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| Error::Audio(format!("{}: {e}", path.display())))?
        }
    };
    let mono = crate::audio::downmix_to_mono(&interleaved, channels);
    Ok(AudioBuffer::new(mono, spec.sample_rate))
}

/// One entry of a fixtures directory's `fixtures.json`.
#[derive(Debug, Deserialize)]
struct FixtureItem {
    file: String,
    duration_s: f32,
    transcript: String,
    #[serde(default)]
    language: Option<String>,
}

/// Find reference audio: `$LW_FIXTURES`, the installed bundle, then the development checkout.
pub fn locate_fixtures() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LW_FIXTURES") {
        let p = PathBuf::from(p);
        if p.join("fixtures.json").exists() {
            return Some(p);
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| packaged_fixtures(&exe))
    {
        return Some(dir);
    }
    crate::model::locate_repo_path("tests/fixtures/audio").filter(|p| p.join("fixtures.json").exists())
}

fn packaged_fixtures(executable: &Path) -> Option<PathBuf> {
    let dir = executable.parent()?.join("benchmark/audio");
    dir.join("fixtures.json").is_file().then_some(dir)
}

/// Read up to `max_clips` clips from a fixtures directory holding a `fixtures.json`.
pub fn fixture_clips(dir: &Path, max_clips: usize) -> Result<Vec<Clip>> {
    let manifest = dir.join("fixtures.json");
    let text =
        std::fs::read_to_string(&manifest).map_err(|e| Error::io(manifest.display().to_string(), e))?;
    let items: Vec<FixtureItem> =
        serde_json::from_str(&text).map_err(|e| Error::Serde(format!("{}: {e}", manifest.display())))?;
    let mut clips = Vec::new();
    for it in items.into_iter().take(max_clips) {
        let audio = load_wav(&dir.join(&it.file))?;
        clips.push(Clip {
            name: it.file,
            duration_s: it.duration_s,
            audio,
            reference: Some(it.transcript),
            language: it.language,
        });
    }
    Ok(clips)
}

/// Build a clip set for a quick measurement: real fixtures when they can be found, otherwise
/// synthetic audio (which yields no WER, and an RTF that is only a lower bound).
pub fn quick_clips(fixtures: Option<PathBuf>, max_clips: usize) -> Result<(Vec<Clip>, ClipSource)> {
    if let Some(dir) = fixtures.or_else(locate_fixtures)
        && dir.join("fixtures.json").exists()
    {
        let clips = fixture_clips(&dir, max_clips)?;
        if !clips.is_empty() {
            return Ok((clips, ClipSource::Fixtures { dir }));
        }
    }
    let clips = (0..max_clips)
        .map(|i| {
            let audio = synth_clip(i, 4.0);
            Clip {
                name: format!("synth_{i}.wav"),
                duration_s: audio.duration_secs(),
                audio,
                reference: None,
                // Synthetic audio is not speech in any language, so it is scored by every engine.
                language: None,
            }
        })
        .collect();
    Ok((clips, ClipSource::Synthetic))
}

/// Deterministic speech-like audio: a syllable-rate-gated harmonic stack with an F0 glide plus a
/// little noise. It exercises mel + encoder + decode with the right spectral shape, but it is not
/// speech: the TDT decoder emits few tokens, so RTF measured on it is a **lower bound**.
pub fn synth_clip(index: usize, seconds: f32) -> AudioBuffer {
    let sr = 16_000u32;
    let n = (seconds * sr as f32) as usize;
    let mut out = Vec::with_capacity(n);
    // Small deterministic LCG so runs are repeatable without an rng dependency.
    let mut seed: u32 = 0x1234_5678u32.wrapping_add((index as u32).wrapping_mul(2_654_435_761));
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sr as f32;
        // 90 -> 190 Hz glide, repeating every 1.4 s, offset per clip.
        let cycle = (t + index as f32 * 0.37) % 1.4;
        let f0 = 90.0 + 100.0 * (cycle / 1.4);
        phase += std::f32::consts::TAU * f0 / sr as f32;
        if phase > std::f32::consts::TAU {
            phase -= std::f32::consts::TAU;
        }
        // Harmonic stack with a 1/k roll-off, roughly a voiced-speech spectrum.
        let mut s = 0.0f32;
        for k in 1..=12u32 {
            s += (phase * k as f32).sin() / k as f32;
        }
        // ~4.5 Hz syllable gate with a raised-cosine envelope.
        let syl = (std::f32::consts::TAU * 4.5 * t).sin();
        let gate = ((syl + 1.0) * 0.5).powf(1.5);
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        out.push((s * 0.16 + noise * 0.01) * gate);
    }
    AudioBuffer::new(out, sr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_benchmark_finds_and_decodes_bundled_reference_audio() {
        let bundle = tempfile::tempdir().unwrap();
        let executable = bundle.path().join("OwlWhisp.app/Contents/MacOS/OwlWhisp");
        let audio = executable.parent().unwrap().join("benchmark/audio");
        std::fs::create_dir_all(&audio).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/audio");
        for file in std::fs::read_dir(source).unwrap() {
            let file = file.unwrap();
            std::fs::copy(file.path(), audio.join(file.file_name())).unwrap();
        }
        let located = packaged_fixtures(&executable).expect("installed reference audio");
        assert_eq!(located, audio);
        let clips = fixture_clips(&located, 15).unwrap();
        assert_eq!(clips.len(), 15);
        assert!(
            clips
                .iter()
                .all(|c| c.reference.as_ref().is_some_and(|r| !r.trim().is_empty())
                    && c.audio.duration_secs() > 0.0)
        );
        assert!(clips.iter().any(|c| c.language.as_deref() == Some("zh")));
        assert!(packaged_fixtures(&bundle.path().join("elsewhere/owlwhisp")).is_none());
    }

    /// The reason `ErrorUnit` exists, demonstrated on one sentence.
    ///
    /// Chinese is written without spaces, so a word-level metric sees the whole sentence as one
    /// token: get one character wrong and the "WER" is 1.0 — the same score as transcribing
    /// nothing recognisable at all. Scored by character, the same transcript is 1 error in 9.
    #[test]
    fn a_run_spanning_two_units_reports_both_totals_instead_of_none() {
        // The model list showed a dash for every model that claims Chinese. The blend really is
        // meaningless -- a word rate and a character rate do not average -- but the two totals it
        // would have been made of are ordinary numbers, and withholding them said "the benchmark
        // failed" where the truth was "there are two answers".
        use crate::engine::Language;
        const BOTH: &[Language] = &[Language("ru"), Language("zh")];
        let clips = vec![
            clip_in("ru.wav", 1.0, Some("а б в г"), Some("ru")),
            clip_in("zh.wav", 1.0, Some("中文测试"), Some("zh")),
        ];
        let mut engine = ScriptedEngine {
            // Perfect Russian, one wrong character out of four in Chinese.
            replies: vec!["а б в г".into(), "中文测验".into()],
            next: 0,
            languages: BOTH,
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();

        assert!(m.mixed_units, "words and characters in one run");
        assert_eq!(m.wer, None, "and so no single blended figure");
        assert_eq!(m.by_unit.len(), 2, "but both totals must survive");

        let word = m
            .by_unit
            .iter()
            .find(|u| u.unit == ErrorUnit::Word)
            .expect("a word total");
        let ch = m
            .by_unit
            .iter()
            .find(|u| u.unit == ErrorUnit::Character)
            .expect("a character total");
        assert_eq!(word.rate, 0.0);
        assert_eq!(word.clips, 1);
        assert_eq!(word.tokens, 4);
        assert!(
            (ch.rate - 0.25).abs() < 1e-6,
            "one character in four: {}",
            ch.rate
        );
        assert_eq!(ch.clips, 1);
        assert_eq!(ch.tokens, 4);
        // Words first, so a caller that shows one figure shows the one the catalog column uses.
        assert_eq!(m.by_unit[0].unit, ErrorUnit::Word);
    }

    #[test]
    fn a_single_unit_run_still_reports_one_total_and_agrees_with_itself() {
        use crate::engine::Language;
        const EN: &[Language] = &[Language("en")];
        let clips = vec![clip_in("en.wav", 1.0, Some("a b c d"), Some("en"))];
        let mut engine = ScriptedEngine {
            replies: vec!["a b c x".into()],
            next: 0,
            languages: EN,
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        assert!(!m.mixed_units);
        assert_eq!(m.by_unit.len(), 1);
        assert_eq!(m.wer, Some(m.by_unit[0].rate), "the two must never disagree");
        assert_eq!(m.unit, Some(ErrorUnit::Word));
    }

    #[test]
    fn a_word_rate_is_meaningless_for_chinese_and_a_character_rate_is_not() {
        let reference = "今天天气非常好";
        let almost_right = "今天天气非常号"; // last character wrong
        let nonsense = "我不知道你在说什么";

        let (word_close, _) = error_rate(reference, almost_right, ErrorUnit::Word);
        let (word_wrong, _) = error_rate(reference, nonsense, ErrorUnit::Word);
        assert_eq!(word_close, 1.0, "one wrong character reads as a total miss");
        assert_eq!(
            word_close, word_wrong,
            "by word, a near-perfect transcript and nonsense score the same --              a metric with two values is not a metric"
        );

        let (char_close, n) = error_rate(reference, almost_right, ErrorUnit::Character);
        let (char_wrong, _) = error_rate(reference, nonsense, ErrorUnit::Character);
        assert_eq!(n, 7, "seven characters in the reference");
        assert!(
            (char_close - 1.0 / 7.0).abs() < 1e-6,
            "one error in seven: {char_close}"
        );
        assert!(char_wrong > char_close, "and nonsense still scores worse");
    }

    #[test]
    fn languages_without_word_boundaries_are_scored_by_character() {
        for lang in ["zh", "zh-CN", "cmn", "yue", "ja", "th", "lo", "km", "my", "bo"] {
            assert_eq!(scoring_unit(lang), ErrorUnit::Character, "{lang}");
        }
        // Korean writes spaces between eojeol, so words are a real unit there.
        for lang in ["en", "ru", "es", "uk", "ko", "vi", "tr", "unknown-tag"] {
            assert_eq!(scoring_unit(lang), ErrorUnit::Word, "{lang}");
        }
    }

    /// Whitespace in a CJK transcript is a formatting choice, not a recognition error.
    #[test]
    fn spacing_does_not_count_against_a_character_rate() {
        let (r, _) = error_rate("今天天气", "今 天 天 气", ErrorUnit::Character);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn a_character_rate_still_ignores_case_and_punctuation() {
        let (r, _) = error_rate("Hello, world!", "hello world", ErrorUnit::Character);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn wer_identical_is_zero() {
        let (w, n) = word_error_rate("hello world", "hello world");
        assert_eq!(w, 0.0);
        assert_eq!(n, 2);
    }

    #[test]
    fn wer_one_substitution_in_two_words() {
        let (w, _) = word_error_rate("hello world", "hello there");
        assert!((w - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wer_ignores_case_and_punctuation() {
        let (w, _) = word_error_rate("Hello, world!", "hello world");
        assert_eq!(w, 0.0);
    }

    #[test]
    fn wer_counts_a_deletion() {
        let (w, n) = word_error_rate("a b c d", "a b d");
        assert_eq!(n, 4);
        assert!((w - 0.25).abs() < 1e-6);
    }

    #[test]
    fn wer_empty_reference_against_text_is_total() {
        let (w, n) = word_error_rate("", "something");
        assert_eq!(w, 1.0);
        assert_eq!(n, 0);
    }

    #[test]
    fn synth_clip_is_deterministic_and_right_length() {
        let a = synth_clip(3, 1.0);
        let b = synth_clip(3, 1.0);
        assert_eq!(a.samples, b.samples);
        assert_eq!(a.sample_rate, 16_000);
        assert_eq!(a.samples.len(), 16_000);
        // Non-trivial signal, and not clipping.
        assert!(a.rms() > 0.01, "rms {}", a.rms());
        assert!(a.samples.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn synth_clips_differ_between_indices() {
        let a = synth_clip(0, 0.2);
        let b = synth_clip(1, 0.2);
        assert_ne!(a.samples, b.samples);
    }

    #[test]
    fn synthetic_source_reports_no_references() {
        assert!(!ClipSource::Synthetic.has_references());
        assert!(
            ClipSource::Fixtures {
                dir: PathBuf::from("x")
            }
            .has_references()
        );
    }

    #[test]
    fn quick_clips_falls_back_to_synthetic_for_a_bogus_dir() {
        let (clips, source) = quick_clips(Some(PathBuf::from("/definitely/not/here")), 2).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(source, ClipSource::Synthetic);
        assert!(clips.iter().all(|c| c.reference.is_none()));
    }

    /// Minimal engine that replays a fixed list of transcripts, so `measure`'s arithmetic can be
    /// tested without any model, runtime or hardware.
    struct ScriptedEngine {
        replies: Vec<String>,
        next: usize,
        languages: &'static [crate::engine::Language],
    }

    impl SpeechEngine for ScriptedEngine {
        fn backend_name(&self) -> &str {
            "scripted"
        }
        fn provider(&self) -> crate::engine::Provider {
            crate::engine::Provider::OnnxCpu
        }
        fn device(&self) -> crate::engine::DeviceInfo {
            crate::engine::DeviceInfo::new("test")
        }
        fn acceleration(&self) -> crate::engine::Acceleration {
            crate::engine::Acceleration::Cpu
        }
        fn supported_languages(&self) -> &[crate::engine::Language] {
            self.languages
        }
        fn supports_streaming(&self) -> bool {
            false
        }
        fn initialize(&mut self, _ctx: &crate::engine::EngineInitContext) -> Result<()> {
            Ok(())
        }
        fn health_check(&mut self) -> crate::engine::HealthReport {
            crate::engine::HealthReport {
                ok: true,
                provider: crate::engine::Provider::OnnxCpu,
                probe_latency_ms: None,
                message: String::new(),
            }
        }
        fn transcribe(&mut self, _audio: &AudioBuffer) -> Result<crate::engine::Transcript> {
            let text = self.replies.get(self.next).cloned().unwrap_or_default();
            self.next += 1;
            Ok(crate::engine::Transcript::from_text(text))
        }
    }

    fn clip(name: &str, seconds: f32, reference: Option<&str>) -> Clip {
        clip_in(name, seconds, reference, None)
    }

    fn clip_in(name: &str, seconds: f32, reference: Option<&str>, language: Option<&str>) -> Clip {
        Clip {
            name: name.to_string(),
            audio: synth_clip(0, seconds),
            duration_s: seconds,
            reference: reference.map(str::to_string),
            language: language.map(str::to_string),
        }
    }

    #[test]
    fn measure_rejects_an_empty_clip_set() {
        let mut engine = ScriptedEngine {
            replies: vec![],
            next: 0,
            languages: &[],
        };
        assert!(measure(&mut engine, &[], ClipSource::Synthetic, |_| {}).is_err());
    }

    #[test]
    fn measure_reports_every_clip_and_streams_them() {
        let clips = vec![
            clip("a.wav", 1.0, Some("one two")),
            clip("b.wav", 1.0, Some("three four")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["one two".into(), "three four".into()],
            next: 0,
            languages: &[],
        };
        let mut streamed = Vec::new();
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |r| {
            streamed.push(r.name.clone())
        })
        .unwrap();
        assert_eq!(streamed, vec!["a.wav", "b.wav"]);
        assert_eq!(m.results.len(), 2);
        assert_eq!(m.warm_count, 1, "first clip is the cold run");
        assert_eq!(m.wer, Some(0.0));
        assert!((m.audio_secs - 2.0).abs() < 1e-6);
    }

    #[test]
    fn measure_weights_wer_by_reference_length() {
        // 1 error in 4 words, then 0 errors in 1 word -> 1/5, not the unweighted mean of 0.25/0.
        let clips = vec![
            clip("long.wav", 1.0, Some("a b c d")),
            clip("short.wav", 1.0, Some("e")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["a b c X".into(), "e".into()],
            next: 0,
            languages: &[],
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        let wer = m.wer.unwrap();
        assert!((wer - 0.2).abs() < 1e-5, "expected 0.2, got {wer}");
    }

    #[test]
    fn measure_reports_no_wer_without_references() {
        let clips = vec![clip("a.wav", 1.0, None), clip("b.wav", 1.0, None)];
        let mut engine = ScriptedEngine {
            replies: vec!["anything".into(), "at all".into()],
            next: 0,
            languages: &[],
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        assert_eq!(m.wer, None);
        assert!(m.results.iter().all(|r| r.wer.is_none()));
    }

    #[test]
    fn a_clip_in_a_language_the_engine_does_not_claim_is_not_run_at_all() {
        // The real case this guards: Moonshine tiny en scores 0.092 on English clips and 0.850
        // once Russian is added. The second number measures the question, not the model -- and
        // the time spent producing it lands in the RTF, where nothing marks it as unreal.
        use crate::engine::Language;
        const EN: &[Language] = &[Language("en")];
        let clips = vec![
            clip_in("en.wav", 1.0, Some("a b c d"), Some("en")),
            clip_in("ru.wav", 1.0, Some("а б в г"), Some("ru")),
        ];
        let mut engine = ScriptedEngine {
            // Perfect on English, nonsense on Russian.
            replies: vec!["a b c d".into(), "totally wrong words here".into()],
            next: 0,
            languages: EN,
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();

        assert_eq!(m.wer, Some(0.0), "the Russian clip must not drag the total down");
        assert_eq!(m.scored_languages, vec!["en".to_string()]);
        assert_eq!(m.skipped_languages, vec!["ru".to_string()]);
        assert_eq!(m.skipped_clips, 1);
        // The engine never saw it: one result, and the timings cover English only.
        assert_eq!(m.results.len(), 1);
        assert!(m.results[0].scored);
        assert_eq!(m.audio_secs, 1.0, "skipped audio must not inflate the total");
        assert!(m.unscored_languages.is_empty());
    }

    #[test]
    fn including_unclaimed_clips_runs_them_but_still_keeps_them_out_of_the_total() {
        // The deliberate opposite: asking what a model does with a language it never offered.
        use crate::engine::Language;
        const EN: &[Language] = &[Language("en")];
        let clips = vec![
            clip_in("en.wav", 1.0, Some("a b c d"), Some("en")),
            clip_in("ru.wav", 1.0, Some("а б в г"), Some("ru")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["a b c d".into(), "totally wrong words here".into()],
            next: 0,
            languages: EN,
        };
        let opts = MeasureOptions {
            include_unclaimed: true,
            ..Default::default()
        };
        let m = measure_with(&mut engine, &clips, ClipSource::Synthetic, opts, |_| {}).unwrap();

        assert_eq!(m.results.len(), 2);
        assert!(m.skipped_languages.is_empty());
        assert_eq!(m.unscored_languages, vec!["ru".to_string()]);
        assert!(!m.results[1].scored);
        // Its own rate is still reported -- that is the point of asking -- just not counted.
        assert!(m.results[1].wer.is_some());
        assert_eq!(m.wer, Some(0.0));
    }

    #[test]
    fn a_catalog_language_stands_in_when_the_engine_declares_none() {
        // GigaAM v3 and Parakeet TDT-CTC 110M really do this: bare transducer exports with no
        // language metadata, so the engine claims nothing and every clip counts. Before this, both
        // were timed on Chinese and Ukrainian audio they were never built for.
        let clips = vec![
            clip_in("ru.wav", 1.0, Some("а б"), Some("ru")),
            clip_in("zh.wav", 1.0, Some("中文"), Some("zh")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["а б".into(), "nonsense".into()],
            next: 0,
            languages: &[],
        };
        let opts = MeasureOptions {
            assume_languages: vec!["ru".into()],
            ..Default::default()
        };
        let m = measure_with(&mut engine, &clips, ClipSource::Synthetic, opts, |_| {}).unwrap();

        assert_eq!(m.results.len(), 1, "only the Russian clip should have run");
        assert_eq!(m.skipped_languages, vec!["zh".to_string()]);
        assert_eq!(m.wer, Some(0.0));
        assert!(
            !m.engine_claimed_languages,
            "the claim came from the catalog, not the engine, and the report must not pretend otherwise"
        );
    }

    #[test]
    fn an_engine_that_declares_its_own_languages_ignores_the_catalog_hint() {
        use crate::engine::Language;
        const RU: &[Language] = &[Language("ru")];
        let clips = vec![
            clip_in("ru.wav", 1.0, Some("а б"), Some("ru")),
            clip_in("en.wav", 1.0, Some("a b"), Some("en")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["а б".into(), "a b".into()],
            next: 0,
            languages: RU,
        };
        // A stale or wrong catalog entry must not widen what the engine itself says it speaks.
        let opts = MeasureOptions {
            assume_languages: vec!["en".into(), "ru".into()],
            ..Default::default()
        };
        let m = measure_with(&mut engine, &clips, ClipSource::Synthetic, opts, |_| {}).unwrap();
        assert_eq!(m.results.len(), 1);
        assert_eq!(m.scored_languages, vec!["ru".to_string()]);
        assert!(m.engine_claimed_languages);
    }

    #[test]
    fn a_model_that_claims_none_of_the_fixture_languages_measures_nothing_and_says_so() {
        use crate::engine::Language;
        const JA: &[Language] = &[Language("ja")];
        let clips = vec![clip_in("ru.wav", 1.0, Some("а б"), Some("ru"))];
        let mut engine = ScriptedEngine {
            replies: vec!["whatever".into()],
            next: 0,
            languages: JA,
        };
        let err = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {})
            .expect_err("nothing is measurable, so this must not return a number");
        let msg = err.to_string();
        assert!(
            msg.contains("ja"),
            "the message must name what the model claims: {msg}"
        );
        assert!(msg.contains("ru"), "and what the clips are: {msg}");
    }

    #[test]
    fn an_engine_claiming_the_language_scores_it() {
        use crate::engine::Language;
        const BOTH: &[Language] = &[Language("en"), Language("ru")];
        let clips = vec![
            clip_in("en.wav", 1.0, Some("a b"), Some("en")),
            clip_in("ru.wav", 1.0, Some("а б"), Some("ru")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["a b".into(), "totally wrong".into()],
            next: 0,
            languages: BOTH,
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        assert!(
            m.wer.unwrap() > 0.0,
            "a claimed language must count against the model"
        );
        assert_eq!(m.scored_languages, vec!["en".to_string(), "ru".to_string()]);
        assert!(m.unscored_languages.is_empty());
    }

    #[test]
    fn the_per_language_breakdown_covers_claimed_and_unclaimed_alike() {
        // The breakdown is what stays honest when the claim is wrong or missing: a Russian-only
        // transducer run against these fixtures shows ru at 0 and the rest near 1, instead of one
        // blended figure that describes neither.
        use crate::engine::Language;
        const EN: &[Language] = &[Language("en")];
        let clips = vec![
            clip_in("en.wav", 1.0, Some("a b c d"), Some("en")),
            clip_in("ru1.wav", 1.0, Some("а б"), Some("ru")),
            clip_in("ru2.wav", 1.0, Some("в г"), Some("ru")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["a b c d".into(), "wrong".into(), "wrong".into()],
            next: 0,
            languages: EN,
        };
        // Deliberately includes the unclaimed clips: the breakdown is exactly what that mode is
        // for, and it is the only way to see a wrong or missing claim for what it is.
        let opts = MeasureOptions {
            include_unclaimed: true,
            ..Default::default()
        };
        let m = measure_with(&mut engine, &clips, ClipSource::Synthetic, opts, |_| {}).unwrap();

        assert!(m.engine_claimed_languages);
        let en = m.per_language.iter().find(|l| l.language == "en").unwrap();
        let ru = m.per_language.iter().find(|l| l.language == "ru").unwrap();
        assert_eq!(en.wer, 0.0);
        assert!(en.claimed);
        assert_eq!(en.clips, 1);
        assert!(ru.wer > 0.5, "the unclaimed language still gets a real number");
        assert!(!ru.claimed);
        assert_eq!(ru.clips, 2);
        assert_eq!(ru.words, 4);
        // ...but it must not touch the headline.
        assert_eq!(m.wer, Some(0.0));
    }

    #[test]
    fn an_engine_with_no_claim_is_flagged_so_callers_can_refuse_a_blended_total() {
        let clips = vec![
            clip_in("en.wav", 1.0, Some("a b"), Some("en")),
            clip_in("ru.wav", 1.0, Some("а б"), Some("ru")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["a b".into(), "wrong".into()],
            next: 0,
            languages: &[],
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        assert!(
            !m.engine_claimed_languages,
            "the CLI keys its 'no single figure' message off this"
        );
        assert_eq!(m.per_language.len(), 2);
        assert!(m.per_language.iter().all(|l| l.claimed));
    }

    #[test]
    fn an_engine_that_claims_nothing_is_scored_on_everything() {
        // No declaration means no claim to violate -- the old behaviour, preserved.
        let clips = vec![
            clip_in("en.wav", 1.0, Some("a b"), Some("en")),
            clip_in("ru.wav", 1.0, Some("а б"), Some("ru")),
        ];
        let mut engine = ScriptedEngine {
            replies: vec!["a b".into(), "а б".into()],
            next: 0,
            languages: &[],
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        assert_eq!(m.wer, Some(0.0));
        assert_eq!(m.scored_languages.len(), 2);
        assert!(m.unscored_languages.is_empty());
    }

    #[test]
    fn measure_uses_the_cold_run_as_warm_when_there_is_only_one_clip() {
        let clips = vec![clip("only.wav", 1.0, None)];
        let mut engine = ScriptedEngine {
            replies: vec!["x".into()],
            next: 0,
            languages: &[],
        };
        let m = measure(&mut engine, &clips, ClipSource::Synthetic, |_| {}).unwrap();
        assert_eq!(m.warm_count, 0);
        assert_eq!(m.warm_rtf, m.cold_rtf);
    }
}
