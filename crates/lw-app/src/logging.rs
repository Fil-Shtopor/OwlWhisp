//! File logging under `<app-data>/logs/`.
//!
//! Without this every `tracing` call in the application is discarded: a release build has no
//! console, so a user hitting a problem has nothing to send and no way to see why, for instance, a
//! hotkey failed to register.
//!
//! **Nothing private is written.** No transcript text, audio, clipboard contents or keys are ever
//! passed to `tracing`, and the default filter is `info`, which carries device and backend facts
//! only. `LW_LOG` raises it for debugging (e.g. `LW_LOG=debug`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing_appender::non_blocking::WorkerGuard;

/// The writer's flush guard.
///
/// Kept here rather than handed to the caller because [`flush`] must be able to *take* it: the
/// guard flushes when it is dropped, and the way this application quits -- `exit_without_teardown`,
/// which is a `TerminateProcess` on itself -- runs no destructors at all. A guard living in a local
/// in `main` would never be dropped on that path, and the last lines before a quit are exactly the
/// ones worth having.
static GUARD: OnceLock<Mutex<Option<WorkerGuard>>> = OnceLock::new();

/// Where the log files are written, so a diagnostics screen can point at the folder.
pub fn log_dir() -> PathBuf {
    crate::paths::app_data_dir().join("logs")
}

/// Start logging to `<app-data>/logs/localwisper.log.<date>`. Answers whether it worked.
pub fn init() -> bool {
    let guard = init_in(&log_dir());
    let started = guard.is_some();
    let _ = GUARD.set(Mutex::new(guard));
    started
}

/// Write out anything the appender is still holding.
///
/// Called on the way out, before the process is ended the hard way. Idempotent: a second call has
/// nothing left to flush.
pub fn flush() {
    if let Some(slot) = GUARD.get()
        && let Ok(mut guard) = slot.lock()
    {
        drop(guard.take());
    }
}

/// As [`init`], but into a directory of the caller's choosing. Separated for tests, which must not
/// write into the real application data directory.
pub fn init_in(logs: &Path) -> Option<WorkerGuard> {
    use tracing_subscriber::EnvFilter;

    std::fs::create_dir_all(logs).ok()?;
    let appender = tracing_appender::rolling::daily(logs, "localwisper.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("LW_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        // No ANSI: these are files, and escape codes in a log a user is asked to send make it
        // unreadable in whatever they open it with.
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok()?;
    tracing::info!(
        "LocalWisper {} starting; logs in {}",
        env!("CARGO_PKG_VERSION"),
        logs.display()
    );
    Some(guard)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_log_directory_sits_under_the_application_data_directory() {
        let dir = super::log_dir();
        assert!(dir.ends_with("logs"), "{}", dir.display());
        assert_eq!(dir.parent(), Some(crate::paths::app_data_dir().as_path()));
    }
}
