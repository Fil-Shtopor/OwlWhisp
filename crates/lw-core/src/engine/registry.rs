//! Backend selection policy: given a preference and a set of candidate backends (each with a
//! `health_check`), pick the first that actually works and record why the others were skipped.

use serde::{Deserialize, Serialize};

use super::{Acceleration, HealthReport, Provider};

/// User preference for which backend to run.
///
/// Three coarse choices plus one entry per execution provider. The coarse ones say *what kind* of
/// hardware to use and let the machine decide the vendor; the exact ones pin a single provider,
/// which is what you want when comparing them, because a run that silently fell back would
/// attribute its numbers to the wrong thing.
///
/// Serialized as a plain string in `settings.json`; the first three names are unchanged from
/// earlier versions, so existing settings files keep working.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendPreference {
    /// Pick the best usable accelerator automatically, falling back to the CPU.
    #[default]
    Automatic,
    /// Any NPU on this machine. If none works, selection fails rather than falling back.
    ForceNpu,
    /// Any GPU on this machine. If none works, selection fails rather than falling back.
    ForceGpu,
    /// CPU only.
    ForceCpu,
    /// Qualcomm Hexagon NPU (QNN).
    Qnn,
    /// Any GPU through the portable WebGPU provider.
    WebGpu,
    /// NVIDIA GPU through CUDA.
    Cuda,
    /// NVIDIA GPU through TensorRT.
    TensorRt,
    /// Any Direct3D 12 GPU through DirectML.
    DirectMl,
    /// Apple Neural Engine / GPU through CoreML.
    CoreMl,
    /// Intel CPU / GPU / NPU through OpenVINO.
    OpenVino,
    /// AMD XDNA NPU through Vitis AI.
    VitisAi,
}

impl BackendPreference {
    /// The exact accelerator this pins, or `None` for the coarse preferences.
    pub fn accelerator(self) -> Option<crate::capabilities::Accelerator> {
        use crate::capabilities::Accelerator as A;
        Some(match self {
            BackendPreference::Automatic | BackendPreference::ForceNpu | BackendPreference::ForceGpu => {
                return None;
            }
            BackendPreference::ForceCpu => A::Cpu,
            BackendPreference::Qnn => A::QnnNpu,
            BackendPreference::WebGpu => A::WebGpu,
            BackendPreference::Cuda => A::Cuda,
            BackendPreference::TensorRt => A::TensorRt,
            BackendPreference::DirectMl => A::DirectMl,
            BackendPreference::CoreMl => A::CoreMl,
            BackendPreference::OpenVino => A::OpenVino,
            BackendPreference::VitisAi => A::VitisAi,
        })
    }

    /// The preference that pins `accel` exactly.
    pub fn for_accelerator(accel: crate::capabilities::Accelerator) -> Self {
        use crate::capabilities::Accelerator as A;
        match accel {
            A::Cpu => BackendPreference::ForceCpu,
            A::QnnNpu => BackendPreference::Qnn,
            A::WebGpu => BackendPreference::WebGpu,
            A::Cuda => BackendPreference::Cuda,
            A::TensorRt => BackendPreference::TensorRt,
            A::DirectMl => BackendPreference::DirectMl,
            A::CoreMl => BackendPreference::CoreMl,
            A::OpenVino => BackendPreference::OpenVino,
            A::VitisAi => BackendPreference::VitisAi,
        }
    }

    /// Whether a failure to honour this preference should be an error rather than a fallback.
    pub fn is_strict(self) -> bool {
        self != BackendPreference::Automatic
    }

    /// Every preference, in the order the settings UI should list them.
    pub fn all() -> Vec<Self> {
        let mut v = vec![
            BackendPreference::Automatic,
            BackendPreference::ForceNpu,
            BackendPreference::ForceGpu,
            BackendPreference::ForceCpu,
        ];
        v.extend(
            crate::capabilities::ALL_ACCELERATORS
                .iter()
                .filter(|a| **a != crate::capabilities::Accelerator::Cpu)
                .map(|a| BackendPreference::for_accelerator(*a)),
        );
        v
    }

    /// Human label for the settings UI.
    pub fn label(self) -> String {
        match self {
            BackendPreference::Automatic => "Automatic".to_string(),
            BackendPreference::ForceNpu => "Any NPU".to_string(),
            BackendPreference::ForceGpu => "Any GPU".to_string(),
            BackendPreference::ForceCpu => "CPU".to_string(),
            other => other
                .accelerator()
                .map(|a| a.label().to_string())
                .unwrap_or_else(|| "?".to_string()),
        }
    }
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
    /// Priority for `Automatic`: NPU first, then GPU/ANE, then CPU. Every other preference
    /// restricts the acceptable set to one class. A candidate is chosen only if its probe
    /// returns `ok`.
    pub fn select(pref: BackendPreference, mut candidates: Vec<BackendCandidate<'_>>) -> SelectionOutcome {
        let rank = |a: Acceleration| match a {
            Acceleration::Npu => 0,
            Acceleration::Ane => 1,
            Acceleration::Gpu => 2,
            Acceleration::Cpu => 3,
        };
        candidates.sort_by_key(|c| rank(c.acceleration));

        // A preference that pins an exact provider still only constrains the *class* here: this
        // registry sees accelerations, not providers. The engine enforces the exact provider.
        let acceptable = |a: Acceleration| match pref {
            BackendPreference::Automatic => true,
            BackendPreference::ForceNpu => a == Acceleration::Npu,
            BackendPreference::ForceGpu => matches!(a, Acceleration::Gpu | Acceleration::Ane),
            BackendPreference::ForceCpu => a == Acceleration::Cpu,
            other => match other.accelerator().map(|x| x.kind()) {
                Some(crate::capabilities::AcceleratorKind::Npu) => a == Acceleration::Npu,
                Some(crate::capabilities::AcceleratorKind::Gpu) => {
                    matches!(a, Acceleration::Gpu | Acceleration::Ane)
                }
                _ => a == Acceleration::Cpu,
            },
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
