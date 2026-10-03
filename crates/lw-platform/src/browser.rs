//! Opening a documented external setup page in the user's normal browser.
//!
//! This never downloads or installs software. Vendor GPU stacks have their own licences,
//! hardware selection and often sign-in requirements, so OwlWhisp only takes a user to the
//! official instructions and lets them make that decision themselves.

use std::process::Command;

use crate::{Error, Result};

/// Open an HTTPS documentation or download page in the system browser.
pub fn open_https(url: &str) -> Result<()> {
    if !url.starts_with("https://") {
        return Err(Error::Unavailable("refusing to open a non-HTTPS URL".into()));
    }

    #[cfg(windows)]
    let result = Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let result: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("no browser launcher on this platform"));

    result
        .map(|_| ())
        .map_err(|e| Error::Unavailable(format!("could not open the browser: {e}")))
}

/// Ask Windows Package Manager to install a vendor package. This starts the normal package-manager
/// flow; Windows still owns elevation, licence prompts and any required reboot.
#[cfg(windows)]
pub fn install_with_winget(package_id: &str) -> Result<()> {
    if !package_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(Error::Unavailable("invalid package identifier".into()));
    }
    Command::new("winget.exe")
        .args([
            "install",
            "--exact",
            "--id",
            package_id,
            "--accept-source-agreements",
            "--accept-package-agreements",
        ])
        .spawn()
        .map(|_| ())
        .map_err(|e| Error::Unavailable(format!("could not start Windows Package Manager: {e}")))
}
