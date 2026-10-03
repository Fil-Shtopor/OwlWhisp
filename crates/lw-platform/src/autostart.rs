//! Whether the application is registered to start when the user logs in.
//!
//! Always read back from the operating system, never remembered in `settings.json`. The
//! registration can be removed in Task Manager, in `launchctl`, or in a desktop environment's own
//! startup list, entirely outside this application -- and a checkbox that disagreed with the OS
//! would be worse than no checkbox at all. `set` therefore writes and then re-reads, and a caller
//! shows what came back rather than what it asked for.
//!
//! No elevation and nothing machine-wide: this writes only under the current user, so it cannot
//! affect anyone else and cannot fail for want of administrator rights.

use std::path::PathBuf;

/// The value name under the Run key. Stable, because changing it orphans the old registration.
const RUN_VALUE: &str = "OwlWhisp";

/// The current executable, quoted, as the command to run at login.
fn command() -> std::io::Result<String> {
    let exe: PathBuf = std::env::current_exe()?;
    Ok(format!("\"{}\"", exe.display()))
}

#[cfg(windows)]
mod imp {
    use super::{RUN_VALUE, command};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn is_enabled() -> std::io::Result<bool> {
        // `reg query` rather than a registry crate: this is two calls in the whole application,
        // and a dependency for two calls is a dependency to audit, pin and ship.
        let out = std::process::Command::new("reg")
            .args(["query", &format!(r"HKCU\{RUN_KEY}"), "/v", RUN_VALUE])
            .output()?;
        Ok(out.status.success())
    }

    pub fn set(enabled: bool) -> std::io::Result<()> {
        let status = if enabled {
            std::process::Command::new("reg")
                .args([
                    "add",
                    &format!(r"HKCU\{RUN_KEY}"),
                    "/v",
                    RUN_VALUE,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &command()?,
                    "/f",
                ])
                .status()?
        } else {
            let status = std::process::Command::new("reg")
                .args(["delete", &format!(r"HKCU\{RUN_KEY}"), "/v", RUN_VALUE, "/f"])
                .status()?;
            // Deleting something that is not there is the state the caller asked for, not a
            // failure, and `reg delete` reports it as one.
            if !status.success() && !is_enabled()? {
                return Ok(());
            }
            status
        };
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "reg exited with {status}"
            )))
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{RUN_VALUE, command};
    use std::io::Write;

    fn plist_path() -> std::io::Result<std::path::PathBuf> {
        let home = std::env::var("HOME")
            .map_err(|_| std::io::Error::other("HOME is not set"))?;
        Ok(std::path::PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("ai.owlwhisp.{RUN_VALUE}.plist")))
    }

    pub fn is_enabled() -> std::io::Result<bool> {
        Ok(plist_path()?.exists())
    }

    pub fn set(enabled: bool) -> std::io::Result<()> {
        let path = plist_path()?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            };
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&path)?;
        write!(
            f,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>ai.owlwhisp.{RUN_VALUE}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#,
            command()?.trim_matches('"')
        )?;
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{RUN_VALUE, command};
    use std::io::Write;

    fn desktop_path() -> std::io::Result<std::path::PathBuf> {
        let base = std::env::var("XDG_CONFIG_HOME").ok().unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/.config")
        });
        Ok(std::path::PathBuf::from(base)
            .join("autostart")
            .join(format!("{RUN_VALUE}.desktop")))
    }

    pub fn is_enabled() -> std::io::Result<bool> {
        Ok(desktop_path()?.exists())
    }

    pub fn set(enabled: bool) -> std::io::Result<()> {
        let path = desktop_path()?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            };
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&path)?;
        write!(
            f,
            "[Desktop Entry]\nType=Application\nName={RUN_VALUE}\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            command()?.trim_matches('"')
        )?;
        Ok(())
    }
}

/// Is the application registered to start at login, according to the operating system?
pub fn is_enabled() -> std::io::Result<bool> {
    imp::is_enabled()
}

/// Register or unregister. Read [`is_enabled`] afterwards: a managed machine can refuse.
pub fn set(enabled: bool) -> std::io::Result<()> {
    imp::set(enabled)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_command_is_quoted_so_a_path_with_spaces_survives() {
        // `C:\Program Files\...` is the ordinary case on Windows, and an unquoted Run value
        // truncates at the first space -- producing a registration that looks right and launches
        // nothing.
        let c = super::command().expect("current exe");
        assert!(c.starts_with('"') && c.ends_with('"'), "{c}");
    }
}
