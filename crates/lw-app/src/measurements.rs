//! Benchmark results measured on *this* machine, kept across restarts.
//!
//! The catalog ships measurements taken on the developer's hardware, which is the honest thing to
//! ship but answers a question the user did not ask: they want to know what *their* machine does.
//! A benchmark already answers that, and until now the answer lived only in the Benchmark tab
//! until the app was closed. This records it beside the model it measured.
//!
//! One record per (model, accelerator). A second run on the same pair replaces the first, because
//! the newer measurement is the better estimate of the same thing — keeping a history would invite
//! the question of which one to believe.
//!
//! **Nothing private is stored.** Model id, accelerator id, three numbers, a clip count and a
//! timestamp. No audio, no transcripts, no paths outside the app's own data directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What one benchmark on this machine found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalMeasurement {
    /// Stable accelerator id (`"qnn-npu"`, `"cpu"`, `"webgpu"`), so runs can be compared without
    /// matching display prose.
    pub accelerator: String,
    /// The label the engine reported, for showing.
    pub accelerator_label: String,
    /// Real-time factor of the first clip, which carries the one-time warm-up.
    pub cold_rtf: f32,
    /// Mean real-time factor after the first clip, when there was more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_rtf: Option<f32>,
    /// Token-weighted error rate, when every scored clip used the same unit.
    ///
    /// `None` for a run spanning both words and characters -- a model that claims Chinese as well
    /// as Russian, say. That is not a failure, and `by_unit` carries the real totals; a reader
    /// must show those rather than a blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wer: Option<f32>,
    /// One total per unit, words first. Empty in records written before this was stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_unit: Vec<lw_core::bench::UnitScore>,
    /// How many clips the WER was computed over — a three-clip figure is not a twelve-clip one.
    pub scored_clips: usize,
    /// How many clips ran in total.
    pub clips: usize,
    /// How many clips were skipped because this model does not claim their language.
    ///
    /// `#[serde(default)]`: records written before skipping existed have no such field, and they
    /// were taken over every clip, so zero is the honest reading of their absence.
    #[serde(default)]
    pub skipped_clips: usize,
    /// The languages the figures above cover. Empty in records written before this was stored.
    #[serde(default)]
    pub scored_languages: Vec<String>,
    /// RFC 3339 UTC, so a stale measurement can be recognised as stale.
    pub measured_at: String,
}

/// The whole file: model id to its measurements, one per accelerator.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LocalMeasurements {
    #[serde(default)]
    pub models: BTreeMap<String, Vec<LocalMeasurement>>,
}

impl LocalMeasurements {
    /// Read the file, or an empty set if it is missing or unreadable.
    ///
    /// A corrupt file is not an error worth stopping for: these are convenience numbers, and the
    /// alternative is an app that will not list models because a cache did not parse. It is logged
    /// and replaced on the next write.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(
                    "local measurements at {} did not parse ({e}); starting empty",
                    path.display()
                );
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!("could not read local measurements: {e}");
                Self::default()
            }
        }
    }

    /// Write the file, replacing it atomically so an interrupted write cannot truncate it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }

    /// Record a measurement, replacing any previous one for the same model and accelerator.
    pub fn record(&mut self, model_id: &str, measurement: LocalMeasurement) {
        let list = self.models.entry(model_id.to_string()).or_default();
        list.retain(|m| m.accelerator != measurement.accelerator);
        list.push(measurement);
        list.sort_by(|a, b| a.accelerator.cmp(&b.accelerator));
    }

    /// The measurements for one model, if any.
    pub fn for_model(&self, model_id: &str) -> &[LocalMeasurement] {
        self.models.get(model_id).map_or(&[], Vec::as_slice)
    }
}

/// Where the file lives, beside `settings.json`.
pub fn path_for(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("measurements.json")
}

/// Turn a finished benchmark into a record, or `None` when it measured nothing usable.
pub fn from_report(report: &crate::bench::BenchReport) -> Option<LocalMeasurement> {
    if report.clips.is_empty() {
        return None;
    }
    Some(LocalMeasurement {
        accelerator: report
            .accelerator
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        accelerator_label: report.backend.clone(),
        cold_rtf: report.cold_rtf,
        warm_rtf: report.warm_rtf,
        wer: report.wer,
        by_unit: report.by_unit.clone(),
        scored_clips: report.clips.iter().filter(|c| c.scored).count(),
        clips: report.clips.len(),
        skipped_clips: report.skipped_clips,
        scored_languages: report.scored_languages.clone(),
        measured_at: now_rfc3339(),
    })
}

/// UTC timestamp without pulling in a date library for one string.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Civil date from a Unix timestamp (Howard Hinnant's algorithm), which is exact and short.
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(accel: &str, wer: f32) -> LocalMeasurement {
        LocalMeasurement {
            accelerator: accel.into(),
            accelerator_label: accel.into(),
            cold_rtf: 0.1,
            warm_rtf: Some(0.05),
            wer: Some(wer),
            by_unit: vec![lw_core::bench::UnitScore {
                unit: lw_core::bench::ErrorUnit::Word,
                rate: wer,
                clips: 12,
                tokens: 240,
            }],
            scored_clips: 12,
            clips: 12,
            skipped_clips: 3,
            scored_languages: vec!["en".into(), "ru".into()],
            measured_at: "2026-09-18T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_record_written_before_skipping_existed_still_loads() {
        // Users have these on disk already. Dropping them on upgrade would silently empty the
        // "on your machine" column, which reads exactly like "never benchmarked".
        let older = r#"{"accelerator":"cpu","accelerator_label":"CPU","cold_rtf":0.1,
            "warm_rtf":0.05,"wer":0.06,"scored_clips":3,"clips":12,
            "measured_at":"2026-09-18T00:00:00Z"}"#;
        let m: LocalMeasurement = serde_json::from_str(older).expect("older record must still load");
        assert_eq!(m.clips, 12);
        assert_eq!(m.skipped_clips, 0, "absence means it ran everything, which it did");
        assert!(m.scored_languages.is_empty());
        assert!(
            m.by_unit.is_empty(),
            "no breakdown was stored then; the reader must fall back to `wer`, not invent a unit"
        );
    }

    #[test]
    fn a_second_run_on_the_same_accelerator_replaces_the_first() {
        let mut set = LocalMeasurements::default();
        set.record("a", m("cpu", 0.10));
        set.record("a", m("cpu", 0.08));
        assert_eq!(set.for_model("a").len(), 1, "one record per accelerator");
        assert_eq!(set.for_model("a")[0].wer, Some(0.08), "the newer run wins");
    }

    #[test]
    fn different_accelerators_are_kept_side_by_side() {
        let mut set = LocalMeasurements::default();
        set.record("a", m("cpu", 0.10));
        set.record("a", m("qnn-npu", 0.09));
        let ids: Vec<&str> = set
            .for_model("a")
            .iter()
            .map(|x| x.accelerator.as_str())
            .collect();
        assert_eq!(ids, ["cpu", "qnn-npu"], "both, sorted by accelerator");
    }

    #[test]
    fn an_unknown_model_has_no_measurements() {
        assert!(LocalMeasurements::default().for_model("nope").is_empty());
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_rather_than_failing() {
        let dir = std::env::temp_dir().join("lw-measurements-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("measurements.json");
        std::fs::write(&p, b"{ not json").unwrap();
        assert!(LocalMeasurements::load(&p).models.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn it_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join("lw-measurements-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("measurements.json");
        let mut set = LocalMeasurements::default();
        set.record("parakeet", m("qnn-npu", 0.048));
        set.save(&p).unwrap();
        let back = LocalMeasurements::load(&p);
        assert_eq!(back.for_model("parakeet"), set.for_model("parakeet"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_timestamp_is_a_plausible_rfc3339_instant() {
        let s = now_rfc3339();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z'), "{s}");
        let year: i32 = s[..4].parse().unwrap();
        assert!((2020..2100).contains(&year), "{s}");
    }
}
