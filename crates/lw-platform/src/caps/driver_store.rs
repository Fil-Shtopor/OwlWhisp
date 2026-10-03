//! Filesystem-only Qualcomm driver-store probe, usable in cross-platform tests.
use super::{driver_version_from_inf_line, htp_arch_from_filename, npu_info_from_arch};
use lw_core::capabilities::NpuInfo;
use std::path::{Path, PathBuf};

/// Inspect a Qualcomm driver-store root without requiring Windows or loading a driver.
pub fn detect_npu_in(repository: &Path) -> NpuInfo {
    let Ok(entries) = std::fs::read_dir(repository) else {
        return NpuInfo::default();
    };
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !name.starts_with("qcnspmcdm") {
            continue;
        }
        let package = entry.path();
        if !package.is_dir() {
            continue;
        }
        if let Some(arch) = scan_htp_arch(&package.join("HTP"))
            && best.as_ref().is_none_or(|(a, _)| arch > *a)
        {
            best = Some((arch, package));
        }
    }
    match best {
        Some((arch, package)) => npu_info_from_arch(arch, read_driver_version(&package)),
        None => NpuInfo::default(),
    }
}

/// Highest HTP arch number named by the Skel/Stub files in an `HTP` directory.
fn scan_htp_arch(htp_dir: &Path) -> Option<u32> {
    let entries = std::fs::read_dir(htp_dir).ok()?;
    entries
        .flatten()
        .filter_map(|e| htp_arch_from_filename(&e.file_name().to_string_lossy()))
        .max()
}

/// The `DriverVer` version from the package's `.inf` file (UTF-16 or ANSI/UTF-8).
fn read_driver_version(package: &Path) -> Option<String> {
    let entries = std::fs::read_dir(package).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("inf")) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let text = decode_inf(&bytes);
        for line in text.lines() {
            if let Some(v) = driver_version_from_inf_line(line) {
                return Some(v);
            }
        }
    }
    None
}

/// INF files ship as UTF-16LE (BOM FF FE) or ANSI/UTF-8; decode accordingly.
fn decode_inf(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}
