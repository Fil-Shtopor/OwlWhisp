//! One running copy, not several.
//!
//! This matters more here than in an ordinary application. Two copies would both install a global
//! keyboard hook on the same combination, both open the microphone when it fires, and both type
//! their transcript into the focused window -- so the symptom of launching twice is not "two
//! windows" but every dictated sentence appearing twice.
//!
//! A second launch is not an error to report, either: the user double-clicked the icon because
//! they wanted the window, and the right answer is to give them the one that already exists.

/// Held by the first instance for as long as it runs. Dropping it lets the next launch win.
pub struct Guard {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    _private: (),
}

// SAFETY: a mutex HANDLE is a process-wide kernel object, valid from any thread; the guard only
// ever closes it, once, on drop.
#[cfg(windows)]
unsafe impl Send for Guard {}
#[cfg(windows)]
unsafe impl Sync for Guard {}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: the handle came from CreateMutexW and is closed exactly once, here.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// What happened when this process tried to be the only one.
pub enum Claim {
    /// It is. Keep the guard alive for the life of the process.
    First(Guard),
    /// Another copy already had the claim. It has been asked to show its window; this process
    /// should exit quietly, without a message -- from the user's point of view their double-click
    /// simply worked.
    Already,
}

/// Try to be the only running copy. `app_id` names the claim; `window_title` is how the existing
/// copy's window is found if the claim is already taken.
#[cfg(windows)]
pub fn claim(app_id: &str, window_title: &str) -> Claim {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

    // "Local\" scope: per user session, so this does not stop two people using the machine at
    // once through fast user switching.
    let name = HSTRING::from(format!("Local\\{app_id}"));
    // SAFETY: a named mutex with a valid wide string; the handle is owned by the guard below.
    let handle = match unsafe { CreateMutexW(None, true, &name) } {
        Ok(h) => h,
        Err(e) => {
            // Cannot tell: let the launch through rather than refuse to start over a doubt.
            tracing::warn!("single-instance mutex could not be created ({e}); starting anyway");
            return Claim::First(Guard {
                handle: windows::Win32::Foundation::HANDLE::default(),
            });
        }
    };

    // SAFETY: reads the calling thread's last error, set by the call above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // The handle is still ours to close, and closing it must not release the *other* process's
        // ownership -- it does not; ownership belongs to whoever acquired it.
        let guard = Guard { handle };
        raise_existing_window(window_title);
        drop(guard);
        return Claim::Already;
    }

    Claim::First(Guard { handle })
}

/// Bring the already-running copy's window to the front.
///
/// Matched by title *and* by executable path: a window called "OwlWhisp" that belongs to
/// something else is somebody else's window, and showing it would be an application reaching into
/// an unrelated program.
#[cfg(windows)]
fn raise_existing_window(title: &str) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowThreadProcessId, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };
    use windows::core::HSTRING;

    // SAFETY: a top-level window lookup by title; null class is allowed.
    let hwnd = match unsafe { FindWindowW(None, &HSTRING::from(title)) } {
        Ok(h) if !h.is_invalid() => h,
        _ => return,
    };

    if !same_executable(hwnd) {
        tracing::warn!("a window titled {title:?} belongs to another program; leaving it alone");
        return;
    }

    // SAFETY: a valid HWND; both calls are best-effort and their results are advisory.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }

    fn same_executable(hwnd: HWND) -> bool {
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        };
        let Ok(ours) = std::env::current_exe() else {
            return false;
        };
        let mut pid = 0u32;
        // SAFETY: writes the owning process id through a pointer to a live local.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return false;
        }
        // SAFETY: opening for a query only; the handle is closed below.
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
            return false;
        };
        let mut buf = [0u16; 32768];
        let mut len = buf.len() as u32;
        // SAFETY: buffer and length are consistent and live for the call.
        let ok = unsafe {
            QueryFullProcessImageNameW(
                process,
                Default::default(),
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        // SAFETY: the handle came from OpenProcess and is closed exactly once.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(process);
        }
        if ok.is_err() {
            return false;
        }
        let theirs = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        theirs == ours
    }
}

/// Not implemented away from Windows yet: every launch believes it is the first.
#[cfg(not(windows))]
pub fn claim(_app_id: &str, _window_title: &str) -> Claim {
    Claim::First(Guard { _private: () })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_first_claim_wins_and_the_second_does_not() {
        // A name of this test's own, so a real OwlWhisp running beside the test suite is not
        // what decides the result.
        let id = format!("lw-test-{}-{:?}", std::process::id(), std::thread::current().id());

        let first = claim(&id, "a window title nothing has");
        assert!(matches!(first, Claim::First(_)), "the first claim lost");

        let second = claim(&id, "a window title nothing has");
        assert!(matches!(second, Claim::Already), "the second claim won too");

        // ...and once the first lets go, the next launch is allowed to start.
        drop(first);
        assert!(
            matches!(claim(&id, "a window title nothing has"), Claim::First(_)),
            "the claim was not released"
        );
    }
}
