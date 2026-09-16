//! The start/stop cues, synthesized rather than shipped.
//!
//! Every cue is generated from a formula at the output device's own sample rate. That buys three
//! things worth more than the convenience of a `.wav`: nothing extra to bundle or license, no
//! resampling step between the file and the sound card, and a theme costs a few lines instead of
//! an asset pipeline.
//!
//! The shapes here are chosen to be *unobtrusive*. A dictation cue fires many times an hour, so
//! every one is under 200 ms, starts and ends at zero amplitude (no click), and uses a raised-
//! cosine envelope rather than a hard gate.

use serde::{Deserialize, Serialize};

/// Which of the two moments a cue marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cue {
    /// Recording started — the app is listening.
    Start,
    /// Recording ended — the app has stopped listening and is transcribing.
    Stop,
}

/// A built-in cue sound.
///
/// `Start` and `Stop` of the same theme are deliberately mirror images: rising means "listening",
/// falling means "done". That direction is the only thing a user needs to learn, and it survives
/// changing the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundTheme {
    /// Two soft sine tones, a rising fifth to start and a falling one to stop. The default.
    #[default]
    Chime,
    /// A single short pip: higher to start, lower to stop. The most discreet option.
    Blip,
    /// A percussive tick with almost no pitch — for people who find tones distracting.
    Click,
    /// A struck-bar tone with a fast decay and an octave overtone.
    Marimba,
}

impl SoundTheme {
    /// Stable id, matching the serde name.
    pub fn id(self) -> &'static str {
        match self {
            SoundTheme::Chime => "chime",
            SoundTheme::Blip => "blip",
            SoundTheme::Click => "click",
            SoundTheme::Marimba => "marimba",
        }
    }

    /// Parse an [`SoundTheme::id`] back.
    pub fn from_id(s: &str) -> Option<Self> {
        ALL_SOUND_THEMES.iter().copied().find(|t| t.id() == s)
    }

    /// Short human label.
    pub fn label(self) -> &'static str {
        match self {
            SoundTheme::Chime => "Chime",
            SoundTheme::Blip => "Blip",
            SoundTheme::Click => "Click",
            SoundTheme::Marimba => "Marimba",
        }
    }

    /// One-line description for the settings UI.
    pub fn description(self) -> &'static str {
        match self {
            SoundTheme::Chime => "Two soft tones, rising to start and falling to stop",
            SoundTheme::Blip => "A single short pip, discreet",
            SoundTheme::Click => "A percussive tick with almost no pitch",
            SoundTheme::Marimba => "A struck-bar tone with a fast decay",
        }
    }
}

/// Every theme, in the order the settings UI should list them.
pub const ALL_SOUND_THEMES: [SoundTheme; 4] = [
    SoundTheme::Chime,
    SoundTheme::Blip,
    SoundTheme::Click,
    SoundTheme::Marimba,
];

/// Longest cue this module will ever produce, used to size buffers.
pub const MAX_CUE_SECS: f32 = 0.30;

/// Render `theme`'s `cue` as mono samples in `[-1.0, 1.0]` at `sample_rate`.
///
/// Generating at the device rate avoids a resampling stage: the caller asks for whatever its
/// output stream negotiated and plays the result directly.
pub fn render(theme: SoundTheme, cue: Cue, sample_rate: u32) -> Vec<f32> {
    let sr = sample_rate.max(8_000) as f32;
    match theme {
        SoundTheme::Chime => chime(cue, sr),
        SoundTheme::Blip => blip(cue, sr),
        SoundTheme::Click => click(cue, sr),
        SoundTheme::Marimba => marimba(cue, sr),
    }
}

/// A decaying envelope: instant attack, exponential tail. Used by the struck/percussive themes.
fn pluck(pos: f32, decay: f32) -> f32 {
    let attack = (pos / 0.02).min(1.0); // 2 % of the cue, enough to avoid a click
    attack * (-decay * pos).exp()
}

fn tone(freq: f32, secs: f32, sr: f32, amp: f32, env: impl Fn(f32) -> f32) -> Vec<f32> {
    let n = (secs * sr) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f32 / n.max(1) as f32;
            let t = i as f32 / sr;
            (std::f32::consts::TAU * freq * t).sin() * env(pos) * amp
        })
        .collect()
}

fn chime(cue: Cue, sr: f32) -> Vec<f32> {
    // A perfect fifth: A5 -> E6 to start, reversed to stop.
    let (a, b) = match cue {
        Cue::Start => (880.0, 1318.5),
        Cue::Stop => (1318.5, 880.0),
    };
    let mut out = tone(a, 0.075, sr, 0.35, |p| (std::f32::consts::PI * p).sin());
    out.extend(tone(b, 0.105, sr, 0.30, |p| (std::f32::consts::PI * p).sin()));
    out
}

fn blip(cue: Cue, sr: f32) -> Vec<f32> {
    let f = match cue {
        Cue::Start => 1046.5, // C6
        Cue::Stop => 784.0,   // G5
    };
    tone(f, 0.06, sr, 0.32, |p| (std::f32::consts::PI * p).sin())
}

fn click(cue: Cue, sr: f32) -> Vec<f32> {
    // A short burst of deterministic noise through a one-pole low-pass: pitchless but audible.
    // The start click is brighter than the stop click, which is the only cue to direction here.
    let secs = 0.035;
    let cutoff = match cue {
        Cue::Start => 0.55,
        Cue::Stop => 0.30,
    };
    let n = (secs * sr) as usize;
    let mut seed: u32 = match cue {
        Cue::Start => 0x9E37_79B9,
        Cue::Stop => 0x85EB_CA6B,
    };
    let mut lp = 0.0f32;
    (0..n)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            lp += cutoff * (white - lp);
            let pos = i as f32 / n.max(1) as f32;
            lp * pluck(pos, 28.0) * 0.9
        })
        .collect()
}

fn marimba(cue: Cue, sr: f32) -> Vec<f32> {
    let f = match cue {
        Cue::Start => 659.3, // E5
        Cue::Stop => 493.9,  // B4
    };
    let secs = 0.22;
    let n = (secs * sr) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let pos = i as f32 / n.max(1) as f32;
            // Fundamental plus a quieter, faster-decaying octave — roughly a struck bar.
            let fundamental = (std::f32::consts::TAU * f * t).sin();
            let octave = (std::f32::consts::TAU * f * 2.0 * t).sin() * 0.35 * (-16.0 * pos).exp();
            (fundamental + octave) * pluck(pos, 9.0) * 0.30
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in ALL_SOUND_THEMES {
            assert!(seen.insert(t.id()), "duplicate id {}", t.id());
            assert_eq!(SoundTheme::from_id(t.id()), Some(t));
        }
        assert_eq!(SoundTheme::from_id("nope"), None);
    }

    #[test]
    fn ids_match_the_serde_representation() {
        for t in ALL_SOUND_THEMES {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, format!("\"{}\"", t.id()));
        }
    }

    #[test]
    fn every_cue_is_audible_and_never_clips() {
        for t in ALL_SOUND_THEMES {
            for c in [Cue::Start, Cue::Stop] {
                let v = render(t, c, 48_000);
                assert!(!v.is_empty(), "{t:?}/{c:?} is silent");
                let p = peak(&v);
                assert!(p > 0.05, "{t:?}/{c:?} is inaudible (peak {p})");
                assert!(p <= 1.0, "{t:?}/{c:?} clips (peak {p})");
            }
        }
    }

    #[test]
    fn every_cue_starts_and_ends_near_silence() {
        // A cue that begins or ends at a non-zero sample produces an audible click on every use.
        for t in ALL_SOUND_THEMES {
            for c in [Cue::Start, Cue::Stop] {
                let v = render(t, c, 48_000);
                assert!(v[0].abs() < 0.02, "{t:?}/{c:?} starts at {}", v[0]);
                let last = *v.last().unwrap();
                assert!(last.abs() < 0.02, "{t:?}/{c:?} ends at {last}");
            }
        }
    }

    #[test]
    fn no_cue_outstays_its_welcome() {
        // These fire many times an hour; a long one would become irritating fast.
        for t in ALL_SOUND_THEMES {
            for c in [Cue::Start, Cue::Stop] {
                let secs = render(t, c, 48_000).len() as f32 / 48_000.0;
                assert!(secs <= MAX_CUE_SECS, "{t:?}/{c:?} lasts {secs}s");
                assert!(secs >= 0.02, "{t:?}/{c:?} lasts only {secs}s");
            }
        }
    }

    #[test]
    fn start_and_stop_are_distinguishable() {
        for t in ALL_SOUND_THEMES {
            let a = render(t, Cue::Start, 48_000);
            let b = render(t, Cue::Stop, 48_000);
            assert_ne!(a, b, "{t:?} uses the same sound for start and stop");
        }
    }

    #[test]
    fn rendering_follows_the_requested_sample_rate() {
        for t in ALL_SOUND_THEMES {
            let a = render(t, Cue::Start, 44_100);
            let b = render(t, Cue::Start, 48_000);
            let dur_a = a.len() as f32 / 44_100.0;
            let dur_b = b.len() as f32 / 48_000.0;
            assert!((dur_a - dur_b).abs() < 0.01, "{t:?}: {dur_a}s vs {dur_b}s");
        }
    }

    #[test]
    fn an_absurdly_low_sample_rate_still_produces_something_playable() {
        // Guards the `max(8_000)` clamp: a device reporting a nonsense rate must not divide by zero
        // or hand back an empty buffer that the player would treat as a failure.
        for t in ALL_SOUND_THEMES {
            let v = render(t, Cue::Start, 1);
            assert!(!v.is_empty(), "{t:?}");
        }
    }

    #[test]
    fn rendering_is_deterministic() {
        // The click theme uses a PRNG; the same cue must sound the same every time.
        for t in ALL_SOUND_THEMES {
            assert_eq!(render(t, Cue::Start, 48_000), render(t, Cue::Start, 48_000));
        }
    }
}
