//! Registering MetalPDF with Windows for PDF files, per user (under
//! HKEY_CURRENT_USER, no administrator rights), as qview and MetalMPV do:
//!
//! - a ProgID `MetalPDF.pdf` with its name, the app icon and the open
//!   command;
//! - the ProgID in `.pdf`'s `OpenWithProgids`, so that MetalPDF is offered
//!   in "Open with";
//! - `Applications\metalpdf.exe` and `Capabilities` listed in
//!   `RegisteredApplications`, so that MetalPDF appears in Settings →
//!   Default apps;
//! - the thumbnail provider (`thumbs/`, `metalpdf_thumbs.dll` next to the
//!   exe) as a COM class, named on the ProgID: Explorer asks the default
//!   ProgID for one, and without it the files showed MetalPDF's icon once
//!   it was the default (Acrobat's extractor, borrowed for a while, gave
//!   bitmaps Explorer turned black at medium icons).
//!
//! Windows does not let a program make itself the default: the choice
//! (`UserChoice`) is protected and belongs to the user. So `register` only
//! offers MetalPDF, and `open_default_apps` opens the page where the user
//! picks it. The installer runs `--register` and the uninstaller
//! `--unregister`.

use std::path::Path;

/// Where the keys go, under HKEY_CURRENT_USER. Tests use a scratch key.
struct Roots {
    classes: String,
    software: String,
    registered_apps: String,
}

impl Roots {
    fn real() -> Self {
        Self {
            classes: r"Software\Classes".into(),
            software: format!(r"Software\{}", crate::APP_NAME),
            registered_apps: r"Software\RegisteredApplications".into(),
        }
    }
}

const PROG_ID: &str = "MetalPDF.pdf";
const APP_KEY: &str = r"Applications\metalpdf.exe";
const EXTENSION: &str = ".pdf";
/// The thumbnail provider's class (`thumbs/src/lib.rs`, the same GUID)
/// and file.
const THUMBNAIL_CLSID: &str = "{46B6CBB3-FB97-4C67-9C29-8151C9DAFE44}";
const THUMBNAIL_DLL: &str = "metalpdf_thumbs.dll";
/// The ShellEx key Explorer looks a thumbnail provider up under
/// (`IThumbnailProvider`'s IID).
const THUMBNAIL_PROVIDER: &str = "{E357FCCD-A995-4576-B01F-234630154E96}";

/// The command Explorer runs to open a file with `exe`.
fn open_command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotRegistered,
    /// Registered for this executable.
    Registered,
    /// Registered for another copy, at this path.
    OtherCopy(String),
}

/// Register `exe` (the running MetalPDF) for PDF files.
pub fn register(exe: &Path) -> Result<(), String> {
    register_in(&Roots::real(), exe)?;
    notify_shell();
    Ok(())
}

/// Remove everything `register` wrote.
pub fn unregister() -> Result<(), String> {
    unregister_in(&Roots::real())?;
    notify_shell();
    Ok(())
}

/// Whether MetalPDF is registered, and for which copy.
#[allow(dead_code)]
pub fn status(exe: &Path) -> Status {
    status_in(&Roots::real(), exe)
}

/// Open Settings → Default apps (on Windows 11 at MetalPDF's own page).
#[allow(dead_code)]
pub fn open_default_apps() -> bool {
    crate::win::shell_open(format!("ms-settings:defaultapps?registeredAppUser={}", crate::APP_NAME))
}

fn register_in(r: &Roots, exe: &Path) -> Result<(), String> {
    let command = open_command(exe);
    let exe_text = exe.display();
    let ru = crate::i18n::lang() == crate::i18n::Lang::Ru;
    let name = if ru { "Документ PDF" } else { "PDF Document" };
    let icon = format!("\"{exe_text}\",0");
    let capabilities = format!(r"{}\Capabilities", r.software);
    let key = format!(r"{}\{PROG_ID}", r.classes);
    reg::set(&key, None, name)?;
    reg::set(&format!(r"{key}\DefaultIcon"), None, &icon)?;
    reg::set(&format!(r"{key}\shell\open\command"), None, &command)?;
    reg::set(&format!(r"{}\{EXTENSION}\OpenWithProgids", r.classes), Some(PROG_ID), "")?;
    let app = format!(r"{}\{APP_KEY}", r.classes);
    reg::set(&app, Some("FriendlyAppName"), crate::APP_NAME)?;
    reg::set(&format!(r"{app}\shell\open\command"), None, &command)?;
    reg::set(&format!(r"{app}\SupportedTypes"), Some(EXTENSION), "")?;
    reg::set(&capabilities, Some("ApplicationName"), crate::APP_NAME)?;
    reg::set(
        &capabilities,
        Some("ApplicationDescription"),
        if ru { "Быстрый просмотрщик PDF" } else { "Fast PDF viewer" },
    )?;
    reg::set(&capabilities, Some("ApplicationIcon"), &icon)?;
    reg::set(&format!(r"{capabilities}\FileAssociations"), Some(EXTENSION), PROG_ID)?;
    reg::set(&r.registered_apps, Some(crate::APP_NAME), &capabilities)?;
    // The thumbnail provider, when its DLL is there (a portable copy has
    // it too); Explorer runs it in its surrogate, dllhost.exe.
    let dll = exe.with_file_name(THUMBNAIL_DLL);
    let class = format!(r"{}\CLSID\{THUMBNAIL_CLSID}", r.classes);
    let shellex = format!(r"{key}\ShellEx\{THUMBNAIL_PROVIDER}");
    if dll.is_file() {
        reg::set(&class, None, if ru { "Миниатюры PDF MetalPDF" } else { "MetalPDF PDF thumbnails" })?;
        reg::set(&format!(r"{class}\InprocServer32"), None, &dll.display().to_string())?;
        reg::set(&format!(r"{class}\InprocServer32"), Some("ThreadingModel"), "Apartment")?;
        reg::set(&shellex, None, THUMBNAIL_CLSID)?;
    } else {
        reg::delete_tree(&shellex)?;
        reg::delete_tree(&class)?;
    }
    Ok(())
}

fn unregister_in(r: &Roots) -> Result<(), String> {
    reg::delete_tree(&format!(r"{}\{PROG_ID}", r.classes))?;
    reg::delete_tree(&format!(r"{}\CLSID\{THUMBNAIL_CLSID}", r.classes))?;
    reg::delete_value(&format!(r"{}\{EXTENSION}\OpenWithProgids", r.classes), PROG_ID)?;
    // The extension's default, if it was ever set to ours.
    let key = format!(r"{}\{EXTENSION}", r.classes);
    if reg::get(&key, None).is_some_and(|p| p.eq_ignore_ascii_case(PROG_ID)) {
        reg::delete_value(&key, "")?;
    }
    reg::delete_tree(&format!(r"{}\{APP_KEY}", r.classes))?;
    reg::delete_tree(&format!(r"{}\Capabilities", r.software))?;
    reg::delete_value(&r.registered_apps, crate::APP_NAME)?;
    Ok(())
}

fn status_in(r: &Roots, exe: &Path) -> Status {
    match reg::get(&format!(r"{}\{APP_KEY}\shell\open\command", r.classes), None) {
        None => Status::NotRegistered,
        Some(c) if c.eq_ignore_ascii_case(&open_command(exe)) => Status::Registered,
        // `"path" "%1"`: the path between the first pair of quotes.
        Some(c) => Status::OtherCopy(c.split('"').nth(1).unwrap_or(&c).to_string()),
    }
}

/// Tell Explorer that associations changed, so that it shows the new
/// icons and names without a restart.
fn notify_shell() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

/// String values under HKEY_CURRENT_USER.
mod reg {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
    };

    use crate::win::wide;

    fn name_ptr(name: &Option<Vec<u16>>) -> *const u16 {
        name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr())
    }

    fn check(code: u32, what: &str, key: &str) -> Result<(), String> {
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("{what} {key}: {}", std::io::Error::from_raw_os_error(code as i32)))
        }
    }

    /// Set value `name` (None: the key's default value) of `key`, creating
    /// the key if needed.
    pub fn set(key: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let (k, n, v) = (wide(key), name.map(wide), wide(value));
        let code = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), name_ptr(&n), REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32)
        };
        check(code, "cannot write", key)
    }

    /// Value `name` of `key`, if it is there.
    pub fn get(key: &str, name: Option<&str>) -> Option<String> {
        let (k, n) = (wide(key), name.map(wide));
        let mut buf = vec![0u16; 1024];
        loop {
            let mut bytes = (buf.len() * 2) as u32;
            let code = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    name_ptr(&n),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
            match code {
                ERROR_SUCCESS => {
                    let len = (bytes as usize / 2).saturating_sub(1);
                    return Some(String::from_utf16_lossy(&buf[..len]));
                }
                // ERROR_MORE_DATA: `bytes` holds the size needed.
                234 => buf.resize(bytes as usize / 2 + 1, 0),
                _ => return None,
            }
        }
    }

    /// Delete `key` with everything under it; a missing key is fine.
    pub fn delete_tree(key: &str) -> Result<(), String> {
        let k = wide(key);
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
        if code == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(code, "cannot delete", key) }
    }

    /// Delete value `name` of `key`; a missing one is fine.
    pub fn delete_value(key: &str, name: &str) -> Result<(), String> {
        let (k, n) = (wide(key), wide(name));
        let code = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
        if code == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(code, "cannot delete", key) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registration into a scratch key, never the real associations.
    #[test]
    fn register_and_unregister() {
        let root = format!(r"Software\metalpdf_test_{}", std::process::id());
        let r = Roots {
            classes: format!(r"{root}\Classes"),
            software: format!(r"{root}\Software\MetalPDF"),
            registered_apps: format!(r"{root}\RegisteredApplications"),
        };
        let exe = Path::new(r"C:\Programs\MetalPDF\metalpdf.exe");
        assert_eq!(status_in(&r, exe), Status::NotRegistered);
        register_in(&r, exe).unwrap();
        assert_eq!(status_in(&r, exe), Status::Registered);
        assert_eq!(
            status_in(&r, Path::new(r"D:\other\metalpdf.exe")),
            Status::OtherCopy(r"C:\Programs\MetalPDF\metalpdf.exe".into())
        );
        let get = |key: &str, name: Option<&str>| reg::get(&format!(r"{}\{key}", r.classes), name);
        assert_eq!(get(r"MetalPDF.pdf\shell\open\command", None).unwrap(), r#""C:\Programs\MetalPDF\metalpdf.exe" "%1""#);
        assert_eq!(get(r"MetalPDF.pdf\DefaultIcon", None).unwrap(), r#""C:\Programs\MetalPDF\metalpdf.exe",0"#);
        assert_eq!(get(r".pdf\OpenWithProgids", Some("MetalPDF.pdf")).unwrap(), "");
        assert_eq!(
            reg::get(&format!(r"{}\Capabilities\FileAssociations", r.software), Some(".pdf")).unwrap(),
            "MetalPDF.pdf"
        );
        assert_eq!(reg::get(&r.registered_apps, Some("MetalPDF")).unwrap(), format!(r"{}\Capabilities", r.software));
        // Another program's entry in OpenWithProgids survives.
        reg::set(&format!(r"{}\.pdf\OpenWithProgids", r.classes), Some("Other.pdf"), "").unwrap();
        unregister_in(&r).unwrap();
        assert_eq!(status_in(&r, exe), Status::NotRegistered);
        assert_eq!(get("MetalPDF.pdf", None), None);
        assert_eq!(get(r".pdf\OpenWithProgids", Some("MetalPDF.pdf")), None);
        assert_eq!(get(r".pdf\OpenWithProgids", Some("Other.pdf")).unwrap(), "");
        assert_eq!(reg::get(&r.registered_apps, Some("MetalPDF")), None);
        // Unregistering twice is fine.
        unregister_in(&r).unwrap();
        reg::delete_tree(&root).unwrap();
    }

    /// The thumbnail provider is registered when its DLL is next to the
    /// exe, dropped when it is not, removed on unregistering.
    #[test]
    fn thumbnail_provider() {
        let root = format!(r"Software\metalpdf_test_thumbs_{}", std::process::id());
        let r = Roots {
            classes: format!(r"{root}\Classes"),
            software: format!(r"{root}\Software\MetalPDF"),
            registered_apps: format!(r"{root}\RegisteredApplications"),
        };
        let dir = std::env::temp_dir().join(format!("metalpdf_test_thumbs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("metalpdf.exe");
        let dll = dir.join(THUMBNAIL_DLL);
        std::fs::write(&dll, b"").unwrap();
        let get = |key: &str, name: Option<&str>| reg::get(&format!(r"{}\{key}", r.classes), name);
        let shellex = format!(r"MetalPDF.pdf\ShellEx\{THUMBNAIL_PROVIDER}");
        let server = format!(r"CLSID\{THUMBNAIL_CLSID}\InprocServer32");
        register_in(&r, &exe).unwrap();
        assert_eq!(get(&shellex, None).unwrap(), THUMBNAIL_CLSID);
        assert_eq!(get(&server, None).unwrap(), dll.display().to_string());
        assert_eq!(get(&server, Some("ThreadingModel")).unwrap(), "Apartment");
        // A copy without the DLL takes it away again.
        std::fs::remove_file(&dll).unwrap();
        register_in(&r, &exe).unwrap();
        assert_eq!(get(&shellex, None), None);
        assert_eq!(get(&server, None), None);
        std::fs::write(&dll, b"").unwrap();
        register_in(&r, &exe).unwrap();
        unregister_in(&r).unwrap();
        assert_eq!(get(&format!(r"CLSID\{THUMBNAIL_CLSID}"), None), None);
        reg::delete_tree(&root).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
