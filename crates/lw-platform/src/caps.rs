//! Hardware/OS capability detection, filling `lw_core::capabilities::Capabilities`.
//!
//! Cross-platform basics (OS, version, arch, CPU brand, cores, RAM) come from `sysinfo`.
//! On Windows the CPU brand is refined from the registry (`ProcessorNameString`) and the
//! Qualcomm NPU is detected from the driver store (filesystem only, no admin):
//! `C:\Windows\System32\DriverStore\FileRepository\qcnspmcdm*\HTP\libQnnHtpV*Skel*.so`
//! (or `QnnHtpV*StubDrv.dll`) names carry the HTP architecture number.
//!
//! `providers.*` (which ONNX Runtime execution providers actually load) is left at its
//! default — `lw-ort` fills that in from real EP enumeration.
//!
//! The string parsers are pure functions, unit-tested below.

use lw_core::capabilities::{Architecture, Capabilities, HtpArch, NpuInfo, OperatingSystem, Platform};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

mod driver_store;
pub use driver_store::detect_npu_in;

/// Raw observations from the OS. Supplying these explicitly does not load any driver.
#[derive(Clone, Debug)]
pub struct CapabilityObservations {
    /// Platform of the running executable.
    pub platform: Platform,
    /// OS version reported by sysinfo.
    pub os_version: String,
    /// CPU name reported by sysinfo.
    pub cpu_brand: String,
    /// Optional Windows registry refinement; ignored on other operating systems.
    pub registry_cpu_brand: Option<String>,
    /// Logical CPU count; zero means the OS could not report it.
    pub cpu_cores: usize,
    /// Physical memory in bytes.
    pub ram_bytes: u64,
    /// Qualcomm driver-store observation, independent of provider availability.
    pub qnn_driver: NpuInfo,
}

/// Interpret OS observations using the same rules as [`detect`].
pub fn capabilities_from_observations(o: CapabilityObservations) -> Capabilities {
    let mut caps = Capabilities::unknown();
    caps.os = o.platform.os_name().into();
    caps.arch = o.platform.arch_name().into();
    caps.os_version = o.os_version;
    caps.cpu_brand = o.cpu_brand.trim().into();
    if o.platform.os == OperatingSystem::Windows
        && let Some(brand) = o.registry_cpu_brand.filter(|b| !b.trim().is_empty())
    {
        caps.cpu_brand = brand.trim().into();
    }
    caps.cpu_cores = o.cpu_cores.max(1);
    caps.ram_mib = o.ram_bytes / (1024 * 1024);
    caps.is_qualcomm = is_qualcomm_brand(&caps.cpu_brand);
    caps.snapdragon_generation = snapdragon_generation(&caps.cpu_brand);
    // DriverStore can retain packages for hardware no longer installed. A package alone
    // must not turn an Intel/AMD PC or an x64 emulated process into a QNN-capable machine.
    if o.platform.os == OperatingSystem::Windows && o.platform.arch == Architecture::Arm64 && caps.is_qualcomm
    {
        caps.npu = o.qnn_driver;
    }
    caps
}

/// Marker type for capability detection (see [`detect`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformCapabilities;

impl PlatformCapabilities {
    /// Detect capabilities on the current machine. Same as the free [`detect`].
    pub fn detect() -> Capabilities {
        detect()
    }
}

/// Detect hardware/OS capabilities on the current machine.
///
/// Never fails: anything undetectable stays at the `Capabilities::unknown()` value.
pub fn detect() -> Capabilities {
    let sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );

    let os_version = System::long_os_version()
        .or_else(System::os_version)
        .unwrap_or_default();
    let cpu_cores = if sys.cpus().is_empty() {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    } else {
        sys.cpus().len()
    };
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    #[cfg(windows)]
    let registry_cpu_brand = crate::windows::processor_name_from_registry();
    #[cfg(not(windows))]
    let registry_cpu_brand = None;
    #[cfg(windows)]
    let qnn_driver = crate::windows::detect_npu();
    #[cfg(not(windows))]
    let qnn_driver = NpuInfo::default();
    capabilities_from_observations(CapabilityObservations {
        platform: Platform::current(),
        os_version,
        cpu_brand,
        registry_cpu_brand,
        cpu_cores,
        ram_bytes: sys.total_memory(),
        qnn_driver,
    })
}

/// Physical GPU adapters, independent of whether an ONNX provider is installed.
/// On other platforms the runtime's device list remains the available GPU probe.
pub fn physical_gpu_descriptions() -> Vec<String> {
    #[cfg(windows)]
    {
        crate::windows::display_adapters()
            .into_iter()
            .map(|a| format!("Windows display: {} GPU ({})", a.name, a.hardware_id))
            .collect()
    }
    #[cfg(target_os = "linux")]
    {
        linux_gpu_descriptions(std::path::Path::new("/sys/class/drm"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    Vec::new()
}

/// Read a Linux DRM tree at an explicit root; connectors and render nodes are not adapters.
pub fn linux_gpu_descriptions(root: &std::path::Path) -> Vec<String> {
    let Ok(cards) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    cards
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("card") && name.len() > 4 && name[4..].chars().all(|c| c.is_ascii_digit())
        })
        .filter_map(|entry| {
            let vendor = std::fs::read_to_string(entry.path().join("device/vendor")).ok()?;
            let name = match vendor.trim().to_ascii_lowercase().as_str() {
                "0x10de" => "NVIDIA",
                "0x8086" => "Intel",
                "0x1002" => "AMD",
                _ => "Other",
            };
            Some(format!(
                "Linux display: {name} GPU ({})",
                entry.file_name().to_string_lossy()
            ))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pure parsers (unit-tested)
// ---------------------------------------------------------------------------

/// Whether a CPU brand string identifies a Qualcomm Snapdragon part.
pub fn is_qualcomm_brand(brand: &str) -> bool {
    let b = brand.to_lowercase();
    b.contains("qualcomm") || b.contains("snapdragon") || b.contains("oryon")
}

/// Parse the Snapdragon generation from a CPU brand string
/// (e.g. `"Snapdragon(R) X2 Elite X2E-96-100"` → `"X2 Elite"`).
///
/// Longest-match-first so `"X2 Elite Extreme"` isn't reported as `"X2 Elite"`.
pub fn snapdragon_generation(brand: &str) -> Option<String> {
    const GENERATIONS: &[&str] = &[
        "X2 Elite Extreme",
        "X2 Elite",
        "X2 Plus",
        "X Elite",
        "X Plus",
        "8cx Gen 3",
        "8cx",
    ];
    let b = brand.to_lowercase();
    GENERATIONS
        .iter()
        .find(|g| b.contains(&g.to_lowercase()))
        .map(|g| g.to_string())
}

/// Parse the HTP architecture number out of a QNN driver file name:
/// `libQnnHtpV81Skel.so`, `libQnnHtpV81SkelDrv.so`, and `QnnHtpV81StubDrv.dll` all → 81.
/// Requires a Skel/Stub file (rejects e.g. `QnnHtpV81.dll` alone).
pub fn htp_arch_from_filename(file_name: &str) -> Option<u32> {
    let idx = file_name.find("QnnHtpV")?;
    let digits: String = file_name[idx + "QnnHtpV".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &file_name[idx + "QnnHtpV".len() + digits.len()..];
    if !(rest.starts_with("Skel") || rest.starts_with("Stub")) {
        return None;
    }
    digits.parse().ok()
}

/// Best-effort map from an HTP architecture number to the QNN `soc_model` id.
/// V81 → 88 (SC8480XP / X2 Elite), V73 → 60 (X Elite / X Plus). Others unknown.
pub fn soc_model_for_htp_arch(arch: u32) -> Option<u32> {
    match arch {
        81 => Some(88),
        73 => Some(60),
        _ => None,
    }
}

/// Parse the driver version out of an INF `DriverVer` line:
/// `"DriverVer = 07/18/2025,1.0.1.1"` → `"1.0.1.1"`.
pub fn driver_version_from_inf_line(line: &str) -> Option<String> {
    let (key, value) = line.split_once('=')?;
    if !key.trim().eq_ignore_ascii_case("DriverVer") {
        return None;
    }
    let version = value.split(',').nth(1)?.trim();
    // Strip a trailing comment.
    let version = version.split(';').next()?.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// Assemble an [`NpuInfo`] from a detected HTP arch number and optional driver version.
pub fn npu_info_from_arch(arch_num: u32, driver_version: Option<String>) -> NpuInfo {
    NpuInfo {
        present: true,
        htp_arch: Some(HtpArch::from_num(arch_num)),
        soc_model: soc_model_for_htp_arch(arch_num),
        driver_version,
        qnn_runtime_version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualcomm_brand_detection() {
        assert!(is_qualcomm_brand(
            "Snapdragon(R) X Elite - X1E80100 - Qualcomm(R) Oryon(TM) CPU"
        ));
        assert!(is_qualcomm_brand("Qualcomm Oryon"));
        assert!(!is_qualcomm_brand("Intel(R) Core(TM) i7-1365U"));
        assert!(!is_qualcomm_brand("Apple M3 Pro"));
        assert!(!is_qualcomm_brand(""));
    }

    #[test]
    fn generation_parsing() {
        assert_eq!(
            snapdragon_generation("Snapdragon(R) X Elite - X1E80100 - Qualcomm(R) Oryon(TM) CPU"),
            Some("X Elite".to_string())
        );
        assert_eq!(
            snapdragon_generation("Snapdragon(R) X2 Elite X2E-96-100"),
            Some("X2 Elite".to_string())
        );
        assert_eq!(
            snapdragon_generation("Snapdragon(R) X2 Elite Extreme X2E-96-100"),
            Some("X2 Elite Extreme".to_string())
        );
        assert_eq!(
            snapdragon_generation("Snapdragon(R) X Plus - X1P64100"),
            Some("X Plus".to_string())
        );
        assert_eq!(snapdragon_generation("Intel(R) Core(TM) i7"), None);
        // Case-insensitive.
        assert_eq!(
            snapdragon_generation("SNAPDRAGON X2 ELITE"),
            Some("X2 Elite".to_string())
        );
    }

    #[test]
    fn htp_arch_filename_parsing() {
        assert_eq!(htp_arch_from_filename("libQnnHtpV81SkelDrv.so"), Some(81));
        assert_eq!(htp_arch_from_filename("libQnnHtpV73Skel.so"), Some(73));
        assert_eq!(htp_arch_from_filename("QnnHtpV81StubDrv.dll"), Some(81));
        assert_eq!(htp_arch_from_filename("QnnHtpV68Stub.dll"), Some(68));
        assert_eq!(htp_arch_from_filename("QnnHtpPrepareDrv.dll"), None);
        assert_eq!(htp_arch_from_filename("QnnHtpV81.dll"), None); // not a Skel/Stub file
        assert_eq!(htp_arch_from_filename("QnnSystemInfo.dll"), None);
        assert_eq!(htp_arch_from_filename(""), None);
    }

    #[test]
    fn soc_model_mapping() {
        assert_eq!(soc_model_for_htp_arch(81), Some(88));
        assert_eq!(soc_model_for_htp_arch(73), Some(60));
        assert_eq!(soc_model_for_htp_arch(68), None);
    }

    #[test]
    fn inf_driver_version_parsing() {
        assert_eq!(
            driver_version_from_inf_line("DriverVer = 07/18/2025,1.0.1.1"),
            Some("1.0.1.1".into())
        );
        assert_eq!(
            driver_version_from_inf_line("DriverVer=01/01/2024,2.3.4.5 ; comment"),
            Some("2.3.4.5".into())
        );
        assert_eq!(
            driver_version_from_inf_line("driverver = 07/18/2025, 1.0.1.1"),
            Some("1.0.1.1".into())
        );
        assert_eq!(
            driver_version_from_inf_line("CatalogFile = qcnspmcdm8480.cat"),
            None
        );
        assert_eq!(driver_version_from_inf_line("DriverVer = 07/18/2025"), None);
    }

    #[test]
    fn npu_info_assembly() {
        let npu = npu_info_from_arch(81, Some("1.0.1.1".into()));
        assert!(npu.present);
        assert_eq!(npu.htp_arch, Some(HtpArch::V81));
        assert_eq!(npu.soc_model, Some(88));
        assert_eq!(npu.driver_version.as_deref(), Some("1.0.1.1"));
        let npu = npu_info_from_arch(75, None);
        assert_eq!(npu.htp_arch, Some(HtpArch::V75));
        assert_eq!(npu.soc_model, None);
    }

    #[test]
    fn detect_reports_sane_basics() {
        let caps = detect();
        assert!(!caps.os.is_empty());
        assert!(caps.cpu_cores >= 1);
        // providers are left for lw-ort; only the CPU default from unknown() is set.
        assert!(caps.providers.cpu);
        assert!(!caps.providers.qnn);
    }
}
