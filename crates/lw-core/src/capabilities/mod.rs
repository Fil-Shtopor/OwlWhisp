//! Hardware/OS capability model. The *data* lives here; the *detection* lives in `lw-platform`
//! (which fills these structs from the registry, driver store, ORT device enumeration, etc.).
//!
//! Detection is capability-based, never CPU-marketing-string-based: an NPU is "present" only if a
//! QNN/NPU execution-provider device enumerates, and the HTP arch is read from the driver package.

use serde::{Deserialize, Serialize};

/// Hexagon HTP architecture generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HtpArch {
    /// Hexagon V68 (SM8350-class).
    V68,
    /// Hexagon V73 (Snapdragon X Elite / X Plus).
    V73,
    /// Hexagon V75.
    V75,
    /// Hexagon V79.
    V79,
    /// Hexagon V81 (Snapdragon X2 Elite).
    V81,
    /// A generation we recognized numerically but don't have a variant for.
    Other(u32),
}

impl HtpArch {
    /// Parse from the numeric arch in a skel filename (e.g. `81` from `libQnnHtpV81Skel.so`).
    pub fn from_num(n: u32) -> Self {
        match n {
            68 => HtpArch::V68,
            73 => HtpArch::V73,
            75 => HtpArch::V75,
            79 => HtpArch::V79,
            81 => HtpArch::V81,
            other => HtpArch::Other(other),
        }
    }
    /// The numeric arch value.
    pub fn num(&self) -> u32 {
        match self {
            HtpArch::V68 => 68,
            HtpArch::V73 => 73,
            HtpArch::V75 => 75,
            HtpArch::V79 => 79,
            HtpArch::V81 => 81,
            HtpArch::Other(n) => *n,
        }
    }
}

/// Details of a detected Qualcomm NPU.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct NpuInfo {
    /// True if an NPU execution-provider device enumerated.
    pub present: bool,
    /// HTP architecture, if determined from the driver package.
    pub htp_arch: Option<HtpArch>,
    /// QNN `soc_model` numeric id, if known (e.g. 88 for SC8480XP).
    pub soc_model: Option<u32>,
    /// NPU driver version string, if read.
    pub driver_version: Option<String>,
    /// The QNN/QAIRT runtime version the app will use, if known.
    pub qnn_runtime_version: Option<String>,
}

/// The set of ML execution providers available in the loaded ONNX Runtime.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AvailableProviders {
    /// CPU EP is essentially always present.
    pub cpu: bool,
    /// QNN EP registered and an NPU device enumerated.
    pub qnn: bool,
    /// CoreML EP available (macOS).
    pub coreml: bool,
    /// DirectML EP available.
    pub directml: bool,
}

/// Full platform capability report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capabilities {
    /// `"windows"`, `"macos"`, `"linux"`.
    pub os: String,
    /// OS version string.
    pub os_version: String,
    /// `"aarch64"`, `"x86_64"`.
    pub arch: String,
    /// CPU brand string.
    pub cpu_brand: String,
    /// Logical CPU count.
    pub cpu_cores: usize,
    /// Total RAM in MiB.
    pub ram_mib: u64,
    /// Whether the CPU is a Qualcomm Snapdragon.
    pub is_qualcomm: bool,
    /// Detected Snapdragon generation, if any (e.g. "X2 Elite").
    pub snapdragon_generation: Option<String>,
    /// NPU details.
    pub npu: NpuInfo,
    /// Available ML providers.
    pub providers: AvailableProviders,
}

impl Capabilities {
    /// One-line human summary of this machine, e.g.
    /// `"Snapdragon(R) X2 Elite Extreme ... - 18 cores - NPU Hexagon V81 via QNN EP"`.
    ///
    /// The NPU clause distinguishes *present* from *usable*: a driver package can be installed
    /// while the QNN execution provider still fails to load, and saying "NPU" in that case would
    /// promise acceleration the machine cannot deliver.
    pub fn summary(&self) -> String {
        let brand = if self.cpu_brand.trim().is_empty() {
            "unidentified CPU".to_string()
        } else {
            self.cpu_brand.clone()
        };
        let ep = if self.providers.qnn {
            " via QNN EP"
        } else {
            " (QNN EP unavailable)"
        };
        let npu = match (self.npu.present, self.npu.htp_arch) {
            (true, Some(a)) => format!("NPU Hexagon V{}{ep}", a.num()),
            (true, None) => format!("NPU present{ep}"),
            _ => "no NPU".to_string(),
        };
        format!("{brand} · {} cores · {npu}", self.cpu_cores)
    }

    /// A minimal, honest report for an unknown machine.
    pub fn unknown() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            os_version: String::new(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_brand: String::new(),
            cpu_cores: 1,
            ram_mib: 0,
            is_qualcomm: false,
            snapdragon_generation: None,
            npu: NpuInfo::default(),
            providers: AvailableProviders {
                cpu: true,
                ..Default::default()
            },
        }
    }

    /// The best acceleration class this machine can offer for STT.
    pub fn best_acceleration(&self) -> crate::engine::Acceleration {
        use crate::engine::Acceleration;
        if self.npu.present && self.providers.qnn {
            Acceleration::Npu
        } else if self.providers.coreml {
            Acceleration::Ane
        } else {
            Acceleration::Cpu
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_names_an_unknown_cpu_honestly() {
        let line = Capabilities::unknown().summary();
        assert!(line.contains("unidentified CPU"), "{line}");
        assert!(line.contains("no NPU"), "{line}");
    }

    #[test]
    fn summary_distinguishes_npu_present_from_npu_usable() {
        let mut caps = Capabilities::unknown();
        caps.cpu_brand = "Test CPU".into();
        caps.npu.present = true;
        caps.npu.htp_arch = Some(HtpArch::V81);

        caps.providers.qnn = false;
        let unusable = caps.summary();
        assert!(unusable.contains("QNN EP unavailable"), "{unusable}");

        caps.providers.qnn = true;
        let usable = caps.summary();
        assert!(usable.contains("NPU Hexagon V81 via QNN EP"), "{usable}");
    }

    #[test]
    fn htp_arch_roundtrip() {
        assert_eq!(HtpArch::from_num(81), HtpArch::V81);
        assert_eq!(HtpArch::V81.num(), 81);
        assert_eq!(HtpArch::from_num(99), HtpArch::Other(99));
    }

    #[test]
    fn best_acceleration_prefers_npu() {
        let mut c = Capabilities::unknown();
        c.npu.present = true;
        c.providers.qnn = true;
        assert_eq!(c.best_acceleration(), crate::engine::Acceleration::Npu);
    }

    #[test]
    fn unknown_is_cpu() {
        assert_eq!(
            Capabilities::unknown().best_acceleration(),
            crate::engine::Acceleration::Cpu
        );
    }
}
