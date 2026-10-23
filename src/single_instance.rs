//! One window per copy of MetalPDF in a user's session: a second launch hands
//! its files to the first one.
//!
//! The first process listens on a local socket (a named pipe on Windows).
//! A later process connects, writes its paths one per line and exits, which
//! takes a few milliseconds instead of starting a second window. The name
//! holds the user, the Windows session and a hash of the exe's path, as
//! qview's does: pipe names are machine-wide, so without the session a file
//! opened at the console would go to the same user's window in a remote
//! desktop session, and an installed and a portable copy would take each
//! other's files.

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, prelude::*,
};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

fn socket_name() -> std::io::Result<Name<'static>> {
    let file = socket_file_name(std::env::current_exe().unwrap_or_default().as_path());
    if GenericNamespaced::is_supported() {
        file.to_ns_name::<GenericNamespaced>()
    } else {
        std::env::temp_dir()
            .join(file)
            .to_fs_name::<GenericFilePath>()
            .map(|n| n.into_owned())
    }
}

/// `metalpdf-<user>-<session>-<hash of the exe's path>.sock`.
fn socket_file_name(exe: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into());
    let user: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    exe.to_string_lossy().to_lowercase().hash(&mut hasher);
    format!(
        "metalpdf-{user}-{}-{:016x}.sock",
        session_id(),
        hasher.finish()
    )
}

/// The Windows session this process runs in (0 elsewhere).
#[cfg(windows)]
fn session_id() -> u32 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ProcessIdToSessionId(process: u32, session: *mut u32) -> i32;
    }
    let mut session = 0;
    // SAFETY: plain Win32 call writing one u32.
    unsafe {
        ProcessIdToSessionId(std::process::id(), &mut session);
    }
    session
}

#[cfg(not(windows))]
fn session_id() -> u32 {
    0
}

/// Sends `paths` to a running instance. `Ok(true)` when one took them.
pub fn hand_off(paths: &[PathBuf]) -> bool {
    let Ok(name) = socket_name() else {
        return false;
    };
    // Not `Stream::connect`: on a busy pipe (every instance taken, the
    // running copy not accepting) it waits with `WaitNamedPipeW` forever,
    // and this launch would hang with no window. A timeout makes a busy
    // pipe an error at once; `main` tries again for a moment.
    let options = interprocess::local_socket::ConnectOptions::new()
        .name(name)
        .wait_mode(interprocess::ConnectWaitMode::Timeout(std::time::Duration::from_secs(1)));
    let Ok(mut stream) = options.connect_sync() else {
        return false;
    };
    #[cfg(windows)]
    allow_foreground();
    let mut message = String::new();
    for path in paths {
        message.push_str(&path.to_string_lossy());
        message.push('\n');
    }
    // An empty message still means "bring the window to the front".
    message.push('\n');
    stream.write_all(message.as_bytes()).is_ok() && stream.flush().is_ok()
}

/// Lets the running instance take focus when it gets our files. Windows
/// otherwise only flashes its taskbar button.
#[cfg(windows)]
fn allow_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    // SAFETY: plain Win32 call with a constant argument.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

pub struct Server {
    listener: interprocess::local_socket::Listener,
}

impl Server {
    /// Claims the instance name. `None` when another process holds it.
    pub fn bind() -> Option<Server> {
        let name = socket_name().ok()?;
        let listener = ListenerOptions::new()
            .name(name)
            .try_overwrite(true)
            .create_sync()
            .ok()?;
        Some(Server { listener })
    }

    /// Serves hand-offs on a background thread. Each connection is read on
    /// a thread of its own: a client that connects and never finishes its
    /// message (pipe reads have no timeout) would otherwise block every
    /// later launch.
    pub fn spawn(self, on_paths: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) {
        let on_paths = std::sync::Arc::new(on_paths);
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for connection in self.listener.incoming().flatten() {
                    let on_paths = std::sync::Arc::clone(&on_paths);
                    let _ = std::thread::Builder::new()
                        .name("hand-off".into())
                        .spawn(move || {
                            let mut paths = Vec::new();
                            for line in BufReader::new(connection).lines() {
                                let Ok(line) = line else { break };
                                if line.is_empty() {
                                    break;
                                }
                                paths.push(PathBuf::from(line));
                            }
                            on_paths(paths);
                        });
                }
            })
            .expect("spawn single-instance thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_copy_of_the_exe_has_its_own_name() {
        let installed = socket_file_name(Path::new(r"C:\Programs\MetalPDF\metalpdf.exe"));
        let portable = socket_file_name(Path::new(r"D:\Tools\MetalPDF\metalpdf.exe"));
        assert_ne!(installed, portable);
        // Windows paths differ in case only by how they were typed.
        let shouted = socket_file_name(Path::new(r"C:\PROGRAMS\MetalPDF\METALPDF.EXE"));
        assert_eq!(installed, shouted);
        assert!(installed.starts_with("metalpdf-") && installed.ends_with(".sock"));
    }
}
