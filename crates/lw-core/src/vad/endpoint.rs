//! End-of-utterance state machine over a stream of per-frame speech probabilities.
//!
//! Design follows the VAD dossier's dictation-tuned recommendation:
//! - hysteresis: enter speech at `threshold`, leave at `neg_threshold = max(threshold - 0.15, 0.01)`
//! - `min_speech_ms` before a segment is accepted (rejects clicks)
//! - `hangover_ms` of sub-threshold audio before the utterance is declared finished
//! - `pre_roll_ms` re-attached before the first speech frame (avoid clipping the first phoneme)
//! - `max_segment_ms` forces a cut for very long speech (chunking)
//!
//! Times are configured in milliseconds and converted to frame counts with ceiling division so a
//! change of frame size never silently shortens the padding.

use serde::{Deserialize, Serialize};

/// Configuration for the endpoint detector. All durations are milliseconds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EndpointConfig {
    /// Speech-onset probability threshold.
    pub threshold: f32,
    /// Frame duration in ms (32 ms for Silero at 16 kHz / 512-sample hops).
    pub frame_ms: f32,
    /// Minimum speech duration to accept a segment.
    pub min_speech_ms: u32,
    /// Trailing sub-threshold duration that ends an utterance.
    pub hangover_ms: u32,
    /// Audio kept before the first speech frame.
    pub pre_roll_ms: u32,
    /// Trailing pad kept after the last speech frame.
    pub trailing_pad_ms: u32,
    /// Force a segment boundary after this much continuous speech (0 = never).
    pub max_segment_ms: u32,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        // Hands-free defaults; push-to-talk overrides hangover/pre-roll at call sites.
        Self {
            threshold: 0.5,
            frame_ms: 32.0,
            min_speech_ms: 200,
            hangover_ms: 1000,
            pre_roll_ms: 300,
            trailing_pad_ms: 250,
            max_segment_ms: 20_000,
        }
    }
}

impl EndpointConfig {
    /// The leave-speech threshold (Silero hysteresis rule).
    pub fn neg_threshold(&self) -> f32 {
        (self.threshold - 0.15).max(0.01)
    }
    fn frames(&self, ms: u32) -> u32 {
        if self.frame_ms <= 0.0 {
            return 0;
        }
        (ms as f32 / self.frame_ms).ceil() as u32
    }
}

/// A detected speech segment, in absolute frame indices (multiply by `frame_ms` for time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeechSegment {
    /// First frame of speech (pre-roll already applied), inclusive.
    pub start_frame: u64,
    /// One past the last frame (trailing pad already applied), exclusive.
    pub end_frame: u64,
}

impl SpeechSegment {
    /// Segment length in frames.
    pub fn len_frames(&self) -> u64 {
        self.end_frame.saturating_sub(self.start_frame)
    }
}

/// Events emitted as probabilities are pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointEvent {
    /// Speech onset confirmed (after `min_speech_ms`).
    SpeechStart {
        /// Frame where speech (with pre-roll) begins.
        start_frame: u64,
    },
    /// A complete segment (end of utterance or forced max-length cut).
    SegmentComplete(SpeechSegment),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Silence,
    /// In candidate speech but not yet past `min_speech_ms`.
    Rising {
        run_start: u64,
    },
    /// Confirmed speech.
    Speech {
        seg_start: u64,
        last_speech: u64,
    },
}

/// Streaming endpoint detector. Push one probability per frame; collect events.
pub struct EndpointDetector {
    cfg: EndpointConfig,
    state: State,
    frame_idx: u64,
    pre_roll_frames: u64,
    trailing_frames: u64,
    min_speech_frames: u64,
    hangover_frames: u64,
    max_segment_frames: u64,
    emitted_start: bool,
}

impl EndpointDetector {
    /// New detector from config.
    pub fn new(cfg: EndpointConfig) -> Self {
        Self {
            state: State::Silence,
            frame_idx: 0,
            pre_roll_frames: cfg.frames(cfg.pre_roll_ms) as u64,
            trailing_frames: cfg.frames(cfg.trailing_pad_ms) as u64,
            min_speech_frames: cfg.frames(cfg.min_speech_ms).max(1) as u64,
            hangover_frames: cfg.frames(cfg.hangover_ms).max(1) as u64,
            max_segment_frames: if cfg.max_segment_ms == 0 {
                u64::MAX
            } else {
                cfg.frames(cfg.max_segment_ms) as u64
            },
            emitted_start: false,
            cfg,
        }
    }

    /// The configuration in use.
    pub fn config(&self) -> &EndpointConfig {
        &self.cfg
    }

    /// Push one frame's speech probability. Returns any events triggered by this frame.
    pub fn push(&mut self, prob: f32) -> Vec<EndpointEvent> {
        let mut events = Vec::new();
        let i = self.frame_idx;
        let is_speech = prob >= self.cfg.threshold;
        let is_silence = prob < self.cfg.neg_threshold();

        match self.state {
            State::Silence => {
                if is_speech {
                    self.state = State::Rising { run_start: i };
                    self.emitted_start = false;
                }
            }
            State::Rising { run_start } => {
                if is_silence {
                    // false start
                    self.state = State::Silence;
                } else if i + 1 - run_start >= self.min_speech_frames {
                    let seg_start = run_start.saturating_sub(self.pre_roll_frames);
                    self.state = State::Speech {
                        seg_start,
                        last_speech: i,
                    };
                    self.emitted_start = true;
                    events.push(EndpointEvent::SpeechStart {
                        start_frame: seg_start,
                    });
                }
            }
            State::Speech {
                seg_start,
                last_speech,
            } => {
                let last = if is_speech { i } else { last_speech };
                self.state = State::Speech {
                    seg_start,
                    last_speech: last,
                };
                let silence_run = i.saturating_sub(last);
                let seg_len = i + 1 - seg_start;
                if silence_run >= self.hangover_frames {
                    let end = (last + 1 + self.trailing_frames).min(i + 1);
                    events.push(EndpointEvent::SegmentComplete(SpeechSegment {
                        start_frame: seg_start,
                        end_frame: end,
                    }));
                    self.state = State::Silence;
                } else if seg_len >= self.max_segment_frames {
                    let end = i + 1;
                    events.push(EndpointEvent::SegmentComplete(SpeechSegment {
                        start_frame: seg_start,
                        end_frame: end,
                    }));
                    // continue in speech from here if still speaking
                    self.state = if is_speech {
                        State::Speech {
                            seg_start: end,
                            last_speech: i,
                        }
                    } else {
                        State::Silence
                    };
                }
            }
        }
        self.frame_idx += 1;
        events
    }

    /// Flush at end of input (e.g. push-to-talk release). Emits a final segment if in speech.
    pub fn flush(&mut self) -> Option<SpeechSegment> {
        let i = self.frame_idx;
        let seg = match self.state {
            State::Speech {
                seg_start,
                last_speech,
            } => Some(SpeechSegment {
                start_frame: seg_start,
                end_frame: (last_speech + 1 + self.trailing_frames).min(i),
            }),
            State::Rising { run_start } if self.emitted_start => Some(SpeechSegment {
                start_frame: run_start.saturating_sub(self.pre_roll_frames),
                end_frame: i,
            }),
            _ => None,
        };
        self.state = State::Silence;
        seg
    }

    /// Whether any speech has been confirmed so far (for empty-recording detection).
    pub fn had_speech(&self) -> bool {
        self.emitted_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> EndpointConfig {
        EndpointConfig {
            threshold: 0.5,
            frame_ms: 32.0,
            min_speech_ms: 96,   // 3 frames
            hangover_ms: 160,    // 5 frames
            pre_roll_ms: 64,     // 2 frames
            trailing_pad_ms: 64, // 2 frames
            max_segment_ms: 0,
        }
    }

    #[test]
    fn detects_a_simple_utterance() {
        let mut d = EndpointDetector::new(cfg());
        let mut events = Vec::new();
        // 5 silence, 10 speech, 8 silence
        for _ in 0..5 {
            events.extend(d.push(0.0));
        }
        for _ in 0..10 {
            events.extend(d.push(0.9));
        }
        for _ in 0..8 {
            events.extend(d.push(0.0));
        }
        let starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, EndpointEvent::SpeechStart { .. }))
            .collect();
        let segs: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EndpointEvent::SegmentComplete(s) => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(segs.len(), 1);
        // speech started at frame 5, pre-roll 2 -> start 3
        assert_eq!(segs[0].start_frame, 3);
        assert!(d.had_speech());
    }

    #[test]
    fn rejects_short_click() {
        let mut d = EndpointDetector::new(cfg());
        let mut events = Vec::new();
        for _ in 0..3 {
            events.extend(d.push(0.0));
        }
        // 2 speech frames < min_speech (3)
        events.extend(d.push(0.9));
        events.extend(d.push(0.9));
        for _ in 0..6 {
            events.extend(d.push(0.0));
        }
        assert!(events.is_empty());
        assert!(!d.had_speech());
    }

    #[test]
    fn flush_emits_pending_speech() {
        let mut d = EndpointDetector::new(cfg());
        for _ in 0..5 {
            d.push(0.9);
        }
        let seg = d.flush().expect("segment on flush");
        assert_eq!(seg.start_frame, 0); // pre-roll clamps to 0
        assert!(seg.len_frames() >= 5);
    }

    #[test]
    fn max_segment_forces_cut() {
        let mut c = cfg();
        c.max_segment_ms = 160; // 5 frames
        let mut d = EndpointDetector::new(c);
        let mut segs = 0;
        for _ in 0..12 {
            for e in d.push(0.9) {
                if matches!(e, EndpointEvent::SegmentComplete(_)) {
                    segs += 1;
                }
            }
        }
        assert!(segs >= 2, "expected forced cuts, got {segs}");
    }

    #[test]
    fn hysteresis_neg_threshold() {
        let c = EndpointConfig {
            threshold: 0.5,
            ..cfg()
        };
        assert!((c.neg_threshold() - 0.35).abs() < 1e-6);
    }
}
