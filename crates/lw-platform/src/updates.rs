//! Launch update installers through the operating system's normal security checks.

use std::path::Path;

/// Open a Windows installer, retaining ShellExecute/SmartScreen checks and prompts.
pub fn open_installer(path: &Path, parameters: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{PCWSTR, w};

        let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let arguments: Vec<u16> = parameters.encode_utf16().chain(Some(0)).collect();
        if file[..file.len() - 1].contains(&0) || arguments[..arguments.len() - 1].contains(&0) {
            return Err("Installer command contains a NUL character".into());
        }
        // All strings remain alive for the call. Windows owns the launched process.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(file.as_ptr()),
                PCWSTR(arguments.as_ptr()),
                None,
                SW_SHOWNORMAL,
            )
        };
        let code = result.0 as isize;
        if code <= 32 {
            return Err(format!("Windows could not open the installer (code {code})"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (path, parameters);
        Err("In-app installation is currently supported on Windows x64/ARM64".into())
    }
}
