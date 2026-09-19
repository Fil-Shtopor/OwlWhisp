//! Where on the primary screen there is room for a window.
//!
//! One question only, asked by the dictation overlay: where is the bottom centre of the area the
//! user actually has free? The *work area* rather than the whole screen, because the taskbar lives
//! in the difference and a pill parked over it is a pill nobody can read.
//!
//! Returned in logical units, because that is what a window toolkit positions in. A caller that
//! gets `None` should let the toolkit place the window itself rather than guess: a wrong guess puts
//! the overlay off-screen, which looks exactly like the overlay being broken.

/// The free area of the primary display, in logical units, with the origin at its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkArea {
    /// Left edge. Can be negative when a second display sits to the left of the primary one.
    pub x: f32,
    /// Top edge. Non-zero when the taskbar is docked at the top.
    pub y: f32,
    /// Usable width.
    pub width: f32,
    /// Usable height.
    pub height: f32,
}

impl WorkArea {
    /// The top-left corner that centres a `width` x `height` window horizontally and parks it
    /// `margin` above the bottom edge.
    pub fn bottom_centre(&self, width: f32, height: f32, margin: f32) -> (f32, f32) {
        (
            self.x + (self.width - width) / 2.0,
            self.y + self.height - height - margin,
        )
    }
}

/// The free area of the primary display, or `None` if this OS cannot be asked yet.
#[cfg(windows)]
pub fn primary_work_area() -> Option<WorkArea> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    let mut rect = RECT::default();
    // SAFETY: SPI_GETWORKAREA writes a RECT through the void pointer; the size argument is ignored
    // for this action and the pointer is to a live, correctly sized local.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(std::ptr::from_mut(&mut rect).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.ok()?;

    // The rect is in physical pixels; a window is positioned in logical ones, and on this machine
    // those differ by 2x. Using the raw numbers puts the overlay off the bottom of the screen.
    // SAFETY: no arguments, no failure mode.
    let dpi = unsafe { GetDpiForSystem() } as f32;
    let scale = if dpi > 0.0 { dpi / 96.0 } else { 1.0 };

    Some(WorkArea {
        x: rect.left as f32 / scale,
        y: rect.top as f32 / scale,
        width: (rect.right - rect.left) as f32 / scale,
        height: (rect.bottom - rect.top) as f32 / scale,
    })
}

/// Not implemented away from Windows yet, so callers centre instead of guessing.
#[cfg(not(windows))]
pub fn primary_work_area() -> Option<WorkArea> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_centre_centres_horizontally_and_sits_above_the_bottom_edge() {
        let area = WorkArea {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
        };
        let (x, y) = area.bottom_centre(240.0, 56.0, 72.0);
        assert_eq!(x, 380.0);
        assert_eq!(y, 800.0 - 56.0 - 72.0);
    }

    #[test]
    fn a_work_area_that_does_not_start_at_the_origin_is_respected() {
        // A taskbar docked left, or a secondary display arranged to the left of the primary, both
        // give a work area with a non-zero origin. Ignoring it puts the pill on the wrong screen.
        let area = WorkArea {
            x: -1920.0,
            y: 40.0,
            width: 1000.0,
            height: 800.0,
        };
        let (x, y) = area.bottom_centre(240.0, 56.0, 72.0);
        assert_eq!(x, -1920.0 + 380.0);
        assert_eq!(y, 40.0 + 800.0 - 56.0 - 72.0);
    }

    #[cfg(windows)]
    #[test]
    fn the_primary_work_area_is_a_plausible_size() {
        let area = primary_work_area().expect("windows always answers this");
        assert!(area.width > 100.0 && area.height > 100.0, "{area:?}");
        // Logical, not physical: a 2x display would report thousands here if the scale were
        // dropped, which is the bug this guards.
        assert!(area.width < 20000.0 && area.height < 20000.0, "{area:?}");
    }
}
