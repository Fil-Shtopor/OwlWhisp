//! Windows-specific capability probes: the registry CPU brand string and Qualcomm NPU
//! detection from the driver store (filesystem + registry only, no admin rights).

use std::path::Path;

use lw_core::capabilities::NpuInfo;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_VALUE_TYPE, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::w;

use crate::caps::detect_npu_in;

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

/// Whether Windows reports an NVIDIA display adapter, independently of any ONNX provider.
pub fn has_nvidia_gpu() -> bool {
    display_adapters()
        .iter()
        .any(|adapter| adapter.hardware_id.to_ascii_lowercase().contains("ven_10de"))
}

/// Physical display adapters, even when no OwlWhisp execution provider has loaded yet.
#[derive(Clone, Debug)]
pub struct DisplayAdapter {
    /// The adapter's Windows display name.
    pub name: String,
    /// The PCI hardware identifier used to match driver updates.
    pub hardware_id: String,
}

/// Enumerate display adapters without requiring an ONNX provider or administrator rights.
pub fn display_adapters() -> Vec<DisplayAdapter> {
    let mut adapters = Vec::new();
    for index in 0..32 {
        let mut device = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: device is a valid output buffer and `cb` carries its size.
        if !unsafe { EnumDisplayDevicesW(None, index, &mut device, 0) }.as_bool() {
            break;
        }
        let name = String::from_utf16_lossy(&device.DeviceString)
            .trim_end_matches('\0')
            .to_string();
        let hardware_id = String::from_utf16_lossy(&device.DeviceID)
            .trim_end_matches('\0')
            .to_string();
        if !name.is_empty()
            && !hardware_id.is_empty()
            && !adapters
                .iter()
                .any(|a: &DisplayAdapter| a.hardware_id == hardware_id)
        {
            adapters.push(DisplayAdapter { name, hardware_id });
        }
    }
    adapters
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

#[cfg(test)]
mod tests {
    use super::*;
    use lw_core::capabilities::HtpArch;

    #[test]
    #[ignore = "reports the actual display adapters of the machine running the test"]
    fn report_display_adapters() {
        for adapter in display_adapters() {
            println!("{}: {}", adapter.name, adapter.hardware_id);
        }
    }

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
}
