//! Backend selection policy: given a preference and a set of candidate backends (each with a
//! `health_check`), pick the first that actually works and record why the others were skipped.

use serde::{Deserialize, Serialize};

use super::{Acceleration, HealthReport, Provider};

/// User preference for which backend to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendPreference {
    /// Pick the best working backend automatically (NPU → GPU/ANE → CPU).
    #[default]
    Automatic,
    /// Force the NPU; if unavailable, selection fails (no silent CPU fallback).
    ForceNpu,
    /// Force CPU.
    ForceCpu,
}

/// A candidate backend the registry can probe: an id, its intended acceleration, and a probe.
pub struct BackendCandidate<'a> {
    /// Stable id, e.g. `"qnn-htp"`, `"ort-cpu"`.
    pub id: &'a str,
    /// The acceleration this candidate intends to provide.
    pub acceleration: Acceleration,
    /// The provider this candidate intends to use.
    pub provider: Provider,
    /// Runs a real probe (tiny inference). `Box<dyn FnMut>` so it can hold session state.
    pub probe: Box<dyn FnMut() -> HealthReport + 'a>,
}

/// The outcome of a selection pass.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SelectionOutcome {
    /// The id of the chosen backend, if any.
    pub chosen: Option<String>,
    /// Per-candidate notes (id → reason/health), in evaluation order.
    pub notes: Vec<(String, String)>,
}

/// Selects a backend by policy. Stateless; operates on the candidate list.
pub struct EngineRegistry;

impl EngineRegistry {
    /// Evaluate candidates in priority order for `pref` and return the outcome.
    ///
    /// Priority for `Automatic`: NPU first, then GPU/ANE, then CPU. `ForceNpu`/`ForceCpu` restrict
    /// the acceptable set. A candidate is chosen only if its probe returns `ok`.
    pub fn select(pref: BackendPreference, mut candidates: Vec<BackendCandidate<'_>>) -> SelectionOutcome {
        let rank = |a: Acceleration| match a {
            Acceleration::Npu => 0,
            Acceleration::Ane => 1,
            Acceleration::Gpu => 2,
            Acceleration::Cpu => 3,
        };
        candidates.sort_by_key(|c| rank(c.acceleration));

        let acceptable = |a: Acceleration| match pref {
            BackendPreference::Automatic => true,
            BackendPreference::ForceNpu => a == Acceleration::Npu,
            BackendPreference::ForceCpu => a == Acceleration::Cpu,
        };

        let mut notes = Vec::new();
        for cand in candidates.iter_mut() {
            if !acceptable(cand.acceleration) {
                notes.push((
                    cand.id.to_string(),
                    format!("skipped ({} not requested)", cand.acceleration),
                ));
                continue;
            }
            let report = (cand.probe)();
            if report.ok {
                notes.push((
                    cand.id.to_string(),
                    format!("selected: {} on {}", report.provider, cand.acceleration),
                ));
                return SelectionOutcome {
                    chosen: Some(cand.id.to_string()),
                    notes,
                };
            } else {
                notes.push((cand.id.to_string(), format!("unavailable: {}", report.message)));
            }
        }
        SelectionOutcome { chosen: None, notes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_report(p: Provider) -> HealthReport {
        HealthReport {
            ok: true,
            provider: p,
            probe_latency_ms: Some(1.0),
            message: "ok".into(),
        }
    }
    fn fail_report(p: Provider, msg: &str) -> HealthReport {
        HealthReport {
            ok: false,
            provider: p,
            probe_latency_ms: None,
            message: msg.into(),
        }
    }

    #[test]
    fn automatic_prefers_npu_when_healthy() {
        let candidates = vec![
            BackendCandidate {
                id: "cpu",
                acceleration: Acceleration::Cpu,
                provider: Provider::OnnxCpu,
                probe: Box::new(|| ok_report(Provider::OnnxCpu)),
            },
            BackendCandidate {
                id: "npu",
                acceleration: Acceleration::Npu,
                provider: Provider::QnnHtp,
                probe: Box::new(|| ok_report(Provider::QnnHtp)),
            },
        ];
        let out = EngineRegistry::select(BackendPreference::Automatic, candidates);
        assert_eq!(out.chosen.as_deref(), Some("npu"));
    }

    #[test]
    fn automatic_falls_back_to_cpu_when_npu_fails() {
        let candidates = vec![
            BackendCandidate {
                id: "npu",
                acceleration: Acceleration::Npu,
                provider: Provider::QnnHtp,
                probe: Box::new(|| fail_report(Provider::QnnHtp, "no device")),
            },
            BackendCandidate {
                id: "cpu",
                acceleration: Acceleration::Cpu,
                provider: Provider::OnnxCpu,
                probe: Box::new(|| ok_report(Provider::OnnxCpu)),
            },
        ];
        let out = EngineRegistry::select(BackendPreference::Automatic, candidates);
        assert_eq!(out.chosen.as_deref(), Some("cpu"));
        assert!(
            out.notes
                .iter()
                .any(|(id, note)| id == "npu" && note.contains("unavailable"))
        );
    }

    #[test]
    fn force_npu_does_not_fall_back() {
        let candidates = vec![
            BackendCandidate {
                id: "npu",
                acceleration: Acceleration::Npu,
                provider: Provider::QnnHtp,
                probe: Box::new(|| fail_report(Provider::QnnHtp, "no device")),
            },
            BackendCandidate {
                id: "cpu",
                acceleration: Acceleration::Cpu,
                provider: Provider::OnnxCpu,
                probe: Box::new(|| ok_report(Provider::OnnxCpu)),
            },
        ];
        let out = EngineRegistry::select(BackendPreference::ForceNpu, candidates);
        assert_eq!(out.chosen, None);
    }
}
