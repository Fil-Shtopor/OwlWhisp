//! Foreground application inspection: executable name + window title, feeding
//! `lw_core::profiles` matching.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

use crate::inject::{ForegroundApp, exe_name};
use crate::{Error, Result};

/// The executable file name and window title of the current foreground window.
pub fn foreground_app() -> Result<ForegroundApp> {
    // SAFETY: no preconditions; may return a null HWND when no window has focus.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return Err(Error::Unavailable("no foreground window".into()));
    }

    // Window title (empty is fine — some windows have none).
    let mut title_buf = [0u16; 512];
    // SAFETY: hwnd was non-null just above; the buffer is a valid, owned slice.
    let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
    let title = String::from_utf16_lossy(&title_buf[..title_len.max(0) as usize]);

    // Owning process id.
    let mut pid = 0u32;
    // SAFETY: hwnd is valid; pid is a valid out pointer.
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if thread_id == 0 || pid == 0 {
        return Err(Error::Unavailable(
            "could not resolve foreground process id".into(),
        ));
    }

    // Full image path -> bare exe name.
    // SAFETY: standard OpenProcess with limited query rights; handle closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| Error::Unavailable(format!("OpenProcess({pid}): {e}")))?;
    let mut path_buf = [0u16; 1024];
    let mut len = path_buf.len() as u32;
    // SAFETY: process handle is valid; buffer/len describe a valid wide buffer.
    let query = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path_buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: closing the handle we opened.
    let _ = unsafe { CloseHandle(process) };
    query.map_err(|e| Error::Unavailable(format!("QueryFullProcessImageNameW: {e}")))?;

    let image_path = String::from_utf16_lossy(&path_buf[..len as usize]);
    Ok(ForegroundApp {
        exe: exe_name(&image_path),
        title,
    })
}

#[cfg(test)]
mod tests {
    /// Runtime smoke test: on an interactive desktop there is normally a foreground
    /// window. Ignored by default because CI/service sessions may not have one.
    #[test]
    #[ignore = "needs an interactive desktop session"]
    fn foreground_app_smoke() {
        let app = super::foreground_app().unwrap();
        assert!(!app.exe.is_empty());
        assert!(app.exe.contains('.')); // e.g. something.exe
    }
}
