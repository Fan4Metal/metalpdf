//! Keeps the restored window on a screen. The saved position can lie off
//! every screen (the monitor it was on is gone, or the resolution changed),
//! and Windows would then show only a taskbar button. Checked once, while
//! the window is still hidden, in physical pixels, so it works on any mix
//! of monitors and scaling. A window closed maximized comes back so
//! through `show_maximized`.

/// A Win32 `RECT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// Moves window `hwnd` into the work area of the nearest monitor, and
/// shrinks it to fit, if its caption cannot be reached there.
#[cfg(windows)]
pub fn keep_on_screen(hwnd: isize) {
    #[repr(C)]
    #[derive(Default)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }
    // user32 is declared by hand, to spare more `windows-sys` features.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        fn IsZoomed(hwnd: isize) -> i32;
        fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;

    let mut window = Rect::default();
    let mut info = MonitorInfo {
        size: size_of::<MonitorInfo>() as u32,
        ..Default::default()
    };
    unsafe {
        // A maximized window fills its monitor whatever its saved place.
        if IsZoomed(hwnd) != 0 || GetWindowRect(hwnd, &mut window) == 0 {
            return;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor == 0 || GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
    }
    let Some(to) = placed(window, info.work) else {
        return;
    };
    let resize = to.right - to.left != window.right - window.left
        || to.bottom - to.top != window.bottom - window.top;
    let flags = SWP_NOZORDER | SWP_NOACTIVATE | if resize { 0 } else { SWP_NOSIZE };
    unsafe {
        SetWindowPos(
            hwnd,
            0,
            to.left,
            to.top,
            to.right - to.left,
            to.bottom - to.top,
            flags,
        );
    }
}

#[cfg(not(windows))]
pub fn keep_on_screen(_hwnd: isize) {}

/// Moves window `hwnd`'s outer corner to `x`, `y`, physical pixels of the
/// screen, its size kept (Windows rescales it if the monitor's DPI is
/// another, as for a window dragged there).
#[cfg(windows)]
pub fn move_to(hwnd: isize, x: i32, y: i32) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, width: i32, height: i32, flags: u32) -> i32;
    }
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    unsafe { SetWindowPos(hwnd, 0, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
}

#[cfg(not(windows))]
pub fn move_to(_hwnd: isize, _x: i32, _y: i32) {}

/// Maximizes window `hwnd`, made at its restored place, which it keeps for
/// the restore button. winit shows a window made maximized at once, before
/// its first frame (`SW_MAXIMIZE` before it hides it again), white, so
/// `main` makes it unmaximized and `App::new` calls this while the window
/// is still cloaked.
#[cfg(windows)]
pub fn show_maximized(hwnd: isize) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    }
    const SW_SHOWMAXIMIZED: i32 = 3;
    unsafe { ShowWindow(hwnd, SW_SHOWMAXIMIZED) };
}

#[cfg(not(windows))]
pub fn show_maximized(_hwnd: isize) {}

/// Restores window `hwnd` if it is minimized and puts it in front, for the
/// files another launch hands over. Done with Win32 rather than viewport
/// commands: eframe runs no frame for a minimized window, only
/// `App::logic`, and winit does not focus a minimized window. Taking the
/// focus is allowed because the launch that hands over calls
/// `AllowSetForegroundWindow` (`single_instance::hand_off`).
#[cfg(windows)]
pub fn bring_to_front(hwnd: isize) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn IsIconic(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }
    const SW_RESTORE: i32 = 9;
    unsafe {
        // A window minimized from maximized comes back maximized.
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

/// Where to put `window` so that its caption can be reached in `work`, or
/// `None` if it already can be.
fn placed(window: Rect, work: Rect) -> Option<Rect> {
    // A window's rectangle includes invisible resize borders of a few
    // pixels, and a window snapped to an edge sits on them.
    const SLACK: i32 = 10;
    const CAPTION: i32 = 30;
    let (width, height) = (window.right - window.left, window.bottom - window.top);
    let caption_width = window.right.min(work.right) - window.left.max(work.left);
    let reachable = caption_width >= 120.min(width)
        && window.top >= work.top - SLACK
        && window.top + CAPTION <= work.bottom;
    if reachable {
        return None;
    }
    let width = width.min(work.right - work.left);
    let height = height.min(work.bottom - work.top);
    let left = window.left.clamp(work.left, work.right - width);
    let top = window.top.clamp(work.top, work.bottom - height);
    Some(Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, width: i32, height: i32) -> Rect {
        Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    #[test]
    fn a_window_on_screen_stays() {
        let work = rect(0, 0, 1920, 1040);
        assert_eq!(placed(rect(100, 100, 800, 600), work), None);
        // Snapped to the left edge, on its invisible border.
        assert_eq!(placed(rect(-7, 0, 960, 1047), work), None);
        // Mostly off to the right, but its caption can be dragged.
        assert_eq!(placed(rect(1700, 200, 800, 600), work), None);
        // On a monitor left of the primary one.
        let left = rect(-1920, 0, 1920, 1040);
        assert_eq!(placed(rect(-1500, 100, 800, 600), left), None);
    }

    #[test]
    fn a_window_off_screen_is_brought_back() {
        let work = rect(0, 0, 1920, 1040);
        // Where a removed monitor was.
        assert_eq!(
            placed(rect(5000, 300, 800, 600), work),
            Some(rect(1120, 300, 800, 600))
        );
        // Its caption above the screen.
        assert_eq!(
            placed(rect(100, -500, 800, 600), work),
            Some(rect(100, 0, 800, 600))
        );
        // Below the screen, and larger than it.
        assert_eq!(
            placed(rect(100, 2000, 2560, 1400), work),
            Some(rect(0, 0, 1920, 1040))
        );
    }
}
