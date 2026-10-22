//! The window caption in the app's theme. egui passes the theme to the
//! window at the end of a frame, and Windows 10 shows a changed caption
//! colour only at the next repaint of the frame (a resize, a change of
//! focus). So the caption is set while the window is still hidden
//! (`App::new`), and repainted after each change of theme
//! (`App::follow_theme`). Shared by metalmd (`caption.rs`), qview
//! (`win::dark_caption`) and disk_flashlight (`scan::win::refresh_caption`).

/// The window's handle, if it is a Win32 window. `eframe::CreationContext`
/// and `eframe::Frame` both implement `HasWindowHandle`.
pub fn hwnd(window: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

/// Give window `hwnd` the dark or light caption and frame
/// (`DWMWA_USE_IMMERSIVE_DARK_MODE`). Called while the window is hidden, it
/// is shown in these colours from the start.
#[cfg(windows)]
pub fn set_dark(hwnd: isize, dark: bool) {
    // dwmapi is declared by hand, to spare another `windows-sys` feature.
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    // The attribute's number before Windows 10 20H1 (1809 to 1909).
    const DWMWA_USE_IMMERSIVE_DARK_MODE_OLD: u32 = 19;
    let value = i32::from(dark);
    let set = |attribute| unsafe {
        DwmSetWindowAttribute(hwnd, attribute, (&raw const value).cast(), 4) >= 0
    };
    if !set(DWMWA_USE_IMMERSIVE_DARK_MODE) {
        set(DWMWA_USE_IMMERSIVE_DARK_MODE_OLD);
    }
}

/// Repaint the caption and frame of window `hwnd` after their colours
/// changed: flipping the caption to the other activation state and back
/// forces the repaint without changing the focus. Not at start-up: there it
/// visibly flickers (qview, always dark, sets the caption once and never
/// refreshes it).
#[cfg(windows)]
pub fn refresh(hwnd: isize) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetActiveWindow() -> isize;
    }
    const WM_NCACTIVATE: u32 = 0x0086;
    unsafe {
        let active = GetActiveWindow() == hwnd;
        SendMessageW(hwnd, WM_NCACTIVATE, usize::from(!active), 0);
        SendMessageW(hwnd, WM_NCACTIVATE, usize::from(active), 0);
    }
}

/// Hide window `hwnd` from the screen while it keeps working and painting
/// (`DWMWA_CLOAK`), or show it again. A window shown before its first
/// OpenGL frame reaches DWM flashes white: `App::new` cloaks the window
/// until it has painted a few frames (`App::uncloak`), and a child
/// viewport's window is cloaked the same way once found (SKILL.md 3.10).
#[cfg(windows)]
pub fn cloak(hwnd: isize, on: bool) {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_CLOAK: u32 = 13;
    let value = i32::from(on);
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, (&raw const value).cast(), 4) };
}

/// The top-level window of this thread titled `title`, hidden or not: the
/// window egui makes for a child viewport, for which eframe gives no
/// handle. By the title, not "the thread's other window": the thread has
/// hidden windows of its own (the IME's), and the viewport's window is
/// made hidden, so as not to flash white (SKILL.md 3.10).
#[cfg(windows)]
#[allow(dead_code)]
pub fn thread_window(title: &str) -> Option<isize> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumThreadWindows(
            thread: u32,
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            lparam: isize,
        ) -> i32;
        fn GetWindowTextW(hwnd: isize, text: *mut u16, max: i32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    unsafe extern "system" fn collect(hwnd: isize, lparam: isize) -> i32 {
        // SAFETY: `lparam` is the pointer to `out` passed below, and the
        // callback runs only during the call.
        let out = unsafe { &mut *(lparam as *mut Vec<isize>) };
        out.push(hwnd);
        1
    }
    let mut all: Vec<isize> = Vec::new();
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), collect, (&raw mut all) as isize);
    }
    all.into_iter().find(|&h| {
        let mut text = [0u16; 128];
        let n = unsafe { GetWindowTextW(h, text.as_mut_ptr(), text.len() as i32) };
        n > 0 && String::from_utf16_lossy(&text[..n as usize]) == title
    })
}

#[cfg(not(windows))]
pub fn set_dark(_hwnd: isize, _dark: bool) {}

#[cfg(not(windows))]
pub fn cloak(_hwnd: isize, _on: bool) {}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn thread_window(_title: &str) -> Option<isize> {
    None
}

#[cfg(not(windows))]
pub fn refresh(_hwnd: isize) {}
