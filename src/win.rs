//! Win32 helpers shared by the apps of this family: a message box that
//! needs no window, the parent console for the command line modes, the
//! shell's "open", Windows' double-click time, and an HTTPS GET through
//! WinHTTP for the update check.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// A NUL-terminated UTF-16 string for the Win32 `W` functions.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Show `text` in a system message box with an error icon, waiting until
/// it is closed. It needs no window of the app, so it works from any thread
/// and while the app is failing (the panic hook). user32 is declared by
/// hand, to spare a `windows-sys` feature.
pub fn error_box(title: &str, text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    const MB_ICONERROR: u32 = 0x10;
    const MB_SETFOREGROUND: u32 = 0x10000;
    let (text, title) = (wide(text), wide(title));
    unsafe {
        MessageBoxW(0, text.as_ptr(), title.as_ptr(), MB_ICONERROR | MB_SETFOREGROUND);
    }
}

/// The release build is a GUI-subsystem program with no console of its own,
/// so text printed by the command line modes (`--version`, `--help`,
/// `--bench`) would be lost when started from cmd or PowerShell. Attach to
/// the parent's console in that case; output already redirected (a pipe or
/// a file) is left untouched. A GUI program started by Python's
/// `subprocess` gets no handles either, hence the check.
pub fn attach_parent_console() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    if std::io::stdout().as_raw_handle().is_null() {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Open `target` (a file, a folder or a URI such as `https://…` or
/// `ms-settings:…`) as Explorer would. eframe opens links only with its
/// `links` feature (the webbrowser crate), which is off, so About and the
/// update button go through this.
pub fn shell_open(target: impl AsRef<OsStr>) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    let file = wide(target);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    result as isize > 32
}

/// Windows' double-click time in seconds (Control Panel: Mouse), for
/// egui's `max_double_click_delay`, which is 0.3 s by default: a slower
/// double click set by the user was two single clicks.
pub fn double_click_time() -> f64 {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDoubleClickTime() -> u32;
    }
    unsafe { GetDoubleClickTime() as f64 / 1000.0 }
}

/// Whether `path` is on the network: a UNC path (`\\server\share`, and
/// `\\?\UNC\…`) or a mapped drive. Asking such a path whether its file is
/// there waits for the share's timeout when the share is away.
pub fn is_remote(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    let Some(Component::Prefix(prefix)) = path.components().next() else { return false };
    match prefix.kind() {
        Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            // DRIVE_REMOTE (winbase.h; its feature is not worth one constant).
            const DRIVE_REMOTE: u32 = 4;
            let root = wide(format!("{}:\\", letter as char));
            unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
        }
        _ => false,
    }
}

/// The mouse pointer's position in the client area of `hwnd`, in
/// physical pixels, as the system has it now.
pub fn cursor_pos(hwnd: isize) -> Option<[f32; 2]> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
        fn ScreenToClient(hwnd: isize, point: *mut Point) -> i32;
    }
    let mut point = Point { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut point) != 0 && ScreenToClient(hwnd, &mut point) != 0 };
    ok.then_some([point.x as f32, point.y as f32])
}

/// HTTPS GET of `path` on `host` through WinHTTP, which brings the system's
/// TLS and proxy settings (no HTTP crate); `headers` are extra request
/// lines joined with `\r\n`. Returns the status code and the body, refused
/// past `limit` bytes. Blocks for up to `timeout_ms` per step, so call it
/// off the UI thread.
pub fn https_get(
    host: &str,
    path: &str,
    agent: &str,
    headers: &str,
    timeout_ms: i32,
    limit: usize,
) -> Result<(u32, Vec<u8>), String> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Networking::WinHttp::*;

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
    fn handle(h: *mut c_void) -> Result<Handle, String> {
        if h.is_null() { Err(winhttp_error()) } else { Ok(Handle(h)) }
    }
    fn check(ok: windows_sys::core::BOOL) -> Result<(), String> {
        if ok == 0 { Err(winhttp_error()) } else { Ok(()) }
    }

    let (agent, host, path, headers) = (wide(agent), wide(host), wide(path), wide(headers));
    let verb = wide("GET");
    let t = timeout_ms;
    // Declared in this order, the handles close request first.
    unsafe {
        let session = handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ))?;
        check(WinHttpSetTimeouts(session.0, t, t, t, t))?;
        let connection =
            handle(WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request = handle(WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        check(WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            (headers.len() - 1) as u32,
            null(),
            0,
            0,
            0,
        ))?;
        check(WinHttpReceiveResponse(request.0, null_mut()))?;
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        check(WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&raw mut status).cast(),
            &mut len,
            null_mut(),
        ))?;
        let mut body = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            check(WinHttpReadData(request.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > limit {
                return Err(format!("the answer is larger than {limit} bytes"));
            }
        }
        Ok((status, body))
    }
}

/// The last WinHTTP error in words; the system's message table lacks them.
fn winhttp_error() -> String {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    match code {
        12002 => "the server did not answer in time".into(),
        12007 => "the server name could not be resolved".into(),
        12029 | 12030 => "the connection to the server failed".into(),
        12175 => "the secure connection failed".into(),
        _ => format!("WinHTTP error {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn network_paths_are_told_apart() {
        assert!(is_remote(Path::new(r"\\server\share\a.pdf")));
        assert!(is_remote(Path::new(r"\\?\UNC\server\share\a.pdf")));
        assert!(!is_remote(Path::new(r"relative\a.pdf")));
        // The system drive is local.
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(!is_remote(Path::new(&format!(r"{system}\Windows"))));
    }
}
