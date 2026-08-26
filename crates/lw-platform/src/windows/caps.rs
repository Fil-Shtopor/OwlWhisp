//! Windows-specific capability probes: the registry CPU brand string and Qualcomm NPU
//! detection from the driver store (filesystem + registry only, no admin rights).

use std::path::{Path, PathBuf};

use lw_core::capabilities::NpuInfo;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_VALUE_TYPE, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::w;

use crate::caps::{driver_version_from_inf_line, htp_arch_from_filename, npu_info_from_arch};

/// The CPU marketing name from
/// `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0\ProcessorNameString`.
pub fn processor_name_from_registry() -> Option<String> {
    let mut buf = [0u16; 512];
    let mut byte_len = (buf.len() * 2) as u32;
    let mut value_type = REG_VALUE_TYPE(0);
    // SAFETY: all pointers reference valid, owned locals; RegGetValueW bounds its writes
    // by byte_len and NUL-terminates REG_SZ data.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            Some(&mut value_type),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let units = &buf[..(byte_len as usize / 2)];
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let name = String::from_utf16_lossy(&units[..end]).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Detect the Qualcomm Hexagon NPU from the driver store.
///
/// Looks for `C:\Windows\System32\DriverStore\FileRepository\qcnspmcdm*\HTP\` and parses
/// the HTP architecture number out of the `libQnnHtpV*Skel*.so` / `QnnHtpV*StubDrv.dll`
/// file names. The driver version comes from the package's `.inf` (`DriverVer=` line).
/// Returns `NpuInfo::default()` (not present) when nothing matches.
pub fn detect_npu() -> NpuInfo {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let repository = Path::new(&windir).join(r"System32\DriverStore\FileRepository");
    detect_npu_in(&repository)
}

/// [`detect_npu`] with an explicit driver-store root (separated for testability).
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

#[cfg(test)]
mod tests {
    use super::*;
    use lw_core::capabilities::HtpArch;

    #[test]
    fn processor_name_is_readable() {
        // HKLM\HARDWARE\DESCRIPTION exists on every Windows install and needs no admin.
        let name = processor_name_from_registry().expect("ProcessorNameString");
        assert!(!name.is_empty());
    }

    #[test]
    fn missing_repository_reports_no_npu() {
        let npu = detect_npu_in(Path::new(r"C:\definitely\not\a\driver\store"));
        assert!(!npu.present);
        assert_eq!(npu.htp_arch, None);
    }

    #[test]
    fn synthetic_driver_store_is_parsed() {
        let root = std::env::temp_dir().join(format!("lw-npu-test-{}", std::process::id()));
        let package = root.join("qcnspmcdm8480.inf_arm64_cafebabe");
        let htp = package.join("HTP");
        std::fs::create_dir_all(&htp).unwrap();
        std::fs::write(htp.join("libQnnHtpV81SkelDrv.so"), b"").unwrap();
        std::fs::write(htp.join("QnnHtpV81StubDrv.dll"), b"").unwrap();
        std::fs::write(htp.join("QnnHtpPrepareDrv.dll"), b"").unwrap();
        std::fs::write(
            package.join("qcnspmcdm8480.inf"),
            b"[Version]\r\nSignature=\"$WINDOWS NT$\"\r\nDriverVer = 07/18/2025,1.0.1.1\r\n",
        )
        .unwrap();

        let npu = detect_npu_in(&root);
        std::fs::remove_dir_all(&root).unwrap();

        assert!(npu.present);
        assert_eq!(npu.htp_arch, Some(HtpArch::V81));
        assert_eq!(npu.soc_model, Some(88));
        assert_eq!(npu.driver_version.as_deref(), Some("1.0.1.1"));
    }

    #[test]
    fn utf16_inf_is_decoded() {
        let text = "DriverVer = 01/01/2024,9.9.9.9\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let decoded = decode_inf(&bytes);
        assert_eq!(
            decoded.lines().find_map(driver_version_from_inf_line).as_deref(),
            Some("9.9.9.9")
        );
    }
}
