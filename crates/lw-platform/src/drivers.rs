//! Install a driver update offered by Windows Update for a detected display adapter.
//! The hardware ID is matched before download, so this never installs an unrelated update.

use crate::{Error, Result};

/// Select physical PCI devices for a display-driver update, without starting Windows Update.
/// Vendor IDs remain readable when Windows uses the generic Basic Display Adapter driver.
pub fn display_driver_targets(accelerator_id: &str, hardware_ids: &[String]) -> Result<Vec<String>> {
    let vendor = match accelerator_id {
        "cuda" | "tensor_rt" => Some("10DE"),
        "open_vino" => Some("8086"),
        "web_gpu" | "direct_ml" => None,
        _ => {
            return Err(Error::Unavailable(
                "no display driver for this accelerator".into(),
            ));
        }
    };
    let mut ids = Vec::new();
    for id in hardware_ids {
        if let Some(key) = hardware_id_key(id)
            && vendor.is_none_or(|vendor| &key[8..12] == vendor)
            && !ids.contains(&key)
        {
            ids.push(key);
        }
    }
    if ids.is_empty() {
        return Err(Error::Unavailable(
            "no matching physical display adapter was found".into(),
        ));
    }
    Ok(ids)
}

/// Windows Update can provide display drivers. Provider libraries are app components and are
/// deliberately outside this operation.
#[cfg(windows)]
pub fn install_display_driver(accelerator_id: &str) -> Result<String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let hardware_ids: Vec<_> = crate::windows::display_adapters()
        .into_iter()
        .map(|adapter| adapter.hardware_id)
        .collect();
    let ids = display_driver_targets(accelerator_id, &hardware_ids)?;

    // PowerShell is only a COM bridge to the OS update service. The script is a constant and
    // adapter IDs are passed as data through the environment, never interpolated into code.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
function DeviceKey([string]$id) {
    if ($id -match '(?i)PCI\\VEN_[0-9A-F]{4}&DEV_[0-9A-F]{4}') { return $Matches[0].ToUpperInvariant() }
    return ''
}
$targets = @($env:OWLWHISP_DRIVER_IDS.Split(';') | Where-Object { $_ })
$session = New-Object -ComObject Microsoft.Update.Session
$session.ClientApplicationID = 'OwlWhisp accelerator setup'
$searcher = $session.CreateUpdateSearcher()
$found = $searcher.Search("IsInstalled=0 and Type='Driver' and IsHidden=0")
$updates = New-Object -ComObject Microsoft.Update.UpdateColl
for ($i = 0; $i -lt $found.Updates.Count; $i++) {
    $update = $found.Updates.Item($i)
    $key = try { DeviceKey([string]$update.DriverHardwareID) } catch { '' }
    if ($key -and $targets -contains $key) {
        if (-not $update.EulaAccepted) { $update.AcceptEula() }
        [void]$updates.Add($update)
    }
}
if ($updates.Count -eq 0) { throw 'Windows Update has no matching driver update for this adapter.' }
$downloader = $session.CreateUpdateDownloader()
$downloader.Updates = $updates
$download = $downloader.Download()
if ($download.ResultCode -ne 2) { throw "Driver download failed (Windows Update code $($download.ResultCode))." }
$installer = $session.CreateUpdateInstaller()
$installer.Updates = $updates
$installed = $installer.Install()
if ($installed.ResultCode -ne 2) { throw "Driver installation failed (Windows Update code $($installed.ResultCode))." }
if ($installed.RebootRequired) { Write-Output 'Driver installed. Restart Windows, then refresh accelerator status.' }
else { Write-Output 'Driver installed. Refreshing accelerator status.' }
"#;
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("OWLWHISP_DRIVER_IDS", ids.join(";"))
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| Error::Unavailable(format!("could not start Windows Update: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::Unavailable(format!("Windows Update: {stderr}")))
    }
}

#[cfg(not(windows))]
pub fn install_display_driver(_accelerator_id: &str) -> Result<String> {
    Err(Error::Unavailable(
        "automatic display driver installation is available on Windows only".into(),
    ))
}

fn hardware_id_key(id: &str) -> Option<String> {
    let upper = id.to_ascii_uppercase();
    let start = upper.find("PCI\\VEN_")?;
    let remaining = &upper[start..];
    let key = remaining.get(..21)?; // PCI\VEN_10DE&DEV_2801
    let (vendor, device) = key.split_once("&DEV_")?;
    if vendor.len() == 12
        && vendor[8..].chars().all(|c| c.is_ascii_hexdigit())
        && device.len() == 4
        && device.chars().all(|c| c.is_ascii_hexdigit())
    {
        Some(key.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::hardware_id_key;

    #[test]
    fn only_pci_vendor_and_device_identify_a_driver_target() {
        assert_eq!(
            hardware_id_key(r"PCI\VEN_10DE&DEV_2801&SUBSYS_12345678"),
            Some(r"PCI\VEN_10DE&DEV_2801".into())
        );
        assert_eq!(hardware_id_key("untrusted input"), None);
    }
}
