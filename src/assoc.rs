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
//! - the thumbnail and preview handlers of the other PDF programs, on the
//!   ProgID (`borrow_handlers`): Explorer asks the default ProgID for them,
//!   and MetalPDF has none of its own, so once it was the default the
//!   files showed its icon where Acrobat had drawn the first page (its
//!   handler is on its ProgID, `Acrobat.Document.DC`, not on `.pdf`).
//!   Acrobat lists its class as an `IExtractImage` only, and the
//!   `IThumbnailProvider` on `.pdf` (here a 2011 bridge for 32-bit Reader)
//!   comes first and fails, so with no thumbnail provider to borrow the
//!   image extractor is put down as one too: Explorer takes a class there
//!   that gives only `IExtractImage` and wraps it (measured: Acrobat's
//!   class answers E_NOINTERFACE for `IThumbnailProvider`, and the files
//!   got their thumbnails).
//!
//! Windows does not let a program make itself the default: the choice
//! (`UserChoice`) is protected and belongs to the user. So `register` only
//! offers MetalPDF, and `open_default_apps` opens the page where the user
//! picks it. The installer runs `--register` and the uninstaller
//! `--unregister`.

use std::path::Path;

/// Where the keys go, under HKEY_CURRENT_USER, and where the other
/// programs' classes are read (HKEY_CLASSES_ROOT, the machine's merged
/// with the user's). Tests use a scratch key for both.
struct Roots {
    classes: String,
    software: String,
    registered_apps: String,
    lookup: (reg::Hkey, String),
}

impl Roots {
    fn real() -> Self {
        Self {
            classes: r"Software\Classes".into(),
            software: format!(r"Software\{}", crate::APP_NAME),
            registered_apps: r"Software\RegisteredApplications".into(),
            lookup: (reg::CLASSES_ROOT, String::new()),
        }
    }

    /// The class key `key` where the other programs' classes are read.
    fn class_key(&self, key: &str) -> String {
        let prefix = &self.lookup.1;
        if prefix.is_empty() { key.to_string() } else { format!(r"{prefix}\{key}") }
    }

    /// Value `name` of the class key `key`, as Explorer sees it.
    fn class_value(&self, key: &str, name: Option<&str>) -> Option<String> {
        reg::get_in(self.lookup.0, &self.class_key(key), name)
    }

    /// The value names of the class key `key`.
    fn class_value_names(&self, key: &str) -> Vec<String> {
        reg::value_names(self.lookup.0, &self.class_key(key))
    }
}

const PROG_ID: &str = "MetalPDF.pdf";
const APP_KEY: &str = r"Applications\metalpdf.exe";
const EXTENSION: &str = ".pdf";

/// The shell extensions Explorer looks up on a file's ProgID that draw a
/// file: the thumbnail (`IThumbnailProvider`, and the older
/// `IExtractImage`, Acrobat's) and the preview pane (`IPreviewHandler`).
const HANDLERS: [&str; 3] = [THUMBNAIL_PROVIDER, EXTRACT_IMAGE, PREVIEW_HANDLER];
const THUMBNAIL_PROVIDER: &str = "{E357FCCD-A995-4576-B01F-234630154E96}";
const EXTRACT_IMAGE: &str = "{BB2E617C-0920-11D1-9A0B-00C04FC2D6C1}";
const PREVIEW_HANDLER: &str = "{8895B1C6-B41F-4C1C-A562-0D564250836F}";

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
    borrow_handlers(r)?;
    Ok(())
}

/// Put on our ProgID the thumbnail and preview handlers that the other
/// PDF programs (the ProgIDs in `.pdf`'s `OpenWithProgids` and its
/// default) have on theirs, each from the first that has one whose DLL is
/// there; those borrowed before are dropped first, so a register after
/// another program came or went brings them up to date. With no
/// thumbnail provider to borrow, the image extractor is put down as one
/// (see the module's notes). They go with the ProgID on unregistering.
fn borrow_handlers(r: &Roots) -> Result<(), String> {
    let mut sources: Vec<String> = r.class_value_names(&format!(r"{EXTENSION}\OpenWithProgids"));
    sources.extend(r.class_value(EXTENSION, None));
    sources.retain(|p| !p.is_empty() && !p.eq_ignore_ascii_case(PROG_ID));
    let lent = |handler: &str| -> Vec<String> {
        sources
            .iter()
            .filter_map(|p| r.class_value(&format!(r"{p}\ShellEx\{handler}"), None))
            .filter(|clsid| handler_installed(r, clsid))
            .collect()
    };
    for handler in HANDLERS {
        let key = format!(r"{}\{PROG_ID}\ShellEx\{handler}", r.classes);
        reg::delete_tree(&key)?;
        let mut found = lent(handler).into_iter().next();
        if found.is_none() && handler == THUMBNAIL_PROVIDER {
            found = lent(EXTRACT_IMAGE).into_iter().next();
        }
        if let Some(clsid) = found {
            reg::set(&key, None, &clsid)?;
        }
    }
    Ok(())
}

/// Whether the COM class `clsid` is registered with a DLL that exists (a
/// program uninstalled carelessly leaves its keys behind).
fn handler_installed(r: &Roots, clsid: &str) -> bool {
    r.class_value(&format!(r"CLSID\{clsid}\InprocServer32"), None)
        .is_some_and(|dll| Path::new(dll.trim().trim_matches('"')).is_file())
}

fn unregister_in(r: &Roots) -> Result<(), String> {
    reg::delete_tree(&format!(r"{}\{PROG_ID}", r.classes))?;
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
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW,
        RegDeleteTreeW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
    };

    /// A root key, to read from.
    pub type Hkey = HKEY;
    pub const CLASSES_ROOT: Hkey = HKEY_CLASSES_ROOT;
    #[cfg(test)]
    pub const CURRENT_USER: Hkey = HKEY_CURRENT_USER;

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
        get_in(HKEY_CURRENT_USER, key, name)
    }

    /// Value `name` of `key` under `root`, if it is there (an expandable
    /// string comes expanded).
    pub fn get_in(root: Hkey, key: &str, name: Option<&str>) -> Option<String> {
        let (k, n) = (wide(key), name.map(wide));
        let mut buf = vec![0u16; 1024];
        loop {
            let mut bytes = (buf.len() * 2) as u32;
            let code = unsafe {
                RegGetValueW(
                    root,
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

    /// The names of `key`'s values under `root` (the default value's is
    /// empty); none for a missing key.
    pub fn value_names(root: Hkey, key: &str) -> Vec<String> {
        let k = wide(key);
        let mut handle: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, k.as_ptr(), 0, KEY_READ, &mut handle) } != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut names = Vec::new();
        // Value names are at most 16383 characters.
        let mut buf = vec![0u16; 16384];
        for index in 0.. {
            let mut len = buf.len() as u32;
            let code = unsafe {
                RegEnumValueW(
                    handle,
                    index,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if code != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        unsafe { RegCloseKey(handle) };
        names
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
            lookup: (reg::CURRENT_USER, format!(r"{root}\Classes")),
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

    /// The other PDF programs' thumbnail and preview handlers come over to
    /// our ProgID, only those whose DLL is there, and go with it.
    #[test]
    fn handlers_are_borrowed() {
        let root = format!(r"Software\metalpdf_test_handlers_{}", std::process::id());
        let classes = format!(r"{root}\Classes");
        let r = Roots {
            classes: classes.clone(),
            software: format!(r"{root}\Software\MetalPDF"),
            registered_apps: format!(r"{root}\RegisteredApplications"),
            lookup: (reg::CURRENT_USER, classes.clone()),
        };
        let set = |key: &str, name: Option<&str>, value: &str| reg::set(&format!(r"{classes}\{key}"), name, value).unwrap();
        let get = |key: &str| reg::get(&format!(r"{classes}\{key}"), None);
        let dll = std::env::current_exe().unwrap().display().to_string();
        let [thumbnail, extract, preview] = HANDLERS;
        let (gone, reader_extract, reader_preview, default_thumbnail) = (
            "{11111111-0000-0000-0000-000000000000}",
            "{22222222-0000-0000-0000-000000000000}",
            "{33333333-0000-0000-0000-000000000000}",
            "{44444444-0000-0000-0000-000000000000}",
        );
        // A program uninstalled with its keys left behind, a reader with a
        // thumbnail extractor and a preview handler on its ProgID, and the
        // extension's default with a thumbnail provider.
        set(r".pdf\OpenWithProgids", Some("Gone.pdf"), "");
        set(r".pdf\OpenWithProgids", Some("Reader.pdf"), "");
        set(".pdf", None, "Default.pdf");
        set(&format!(r"Gone.pdf\ShellEx\{extract}"), None, gone);
        set(&format!(r"CLSID\{gone}\InprocServer32"), None, r"C:\nowhere\gone.dll");
        set(&format!(r"Reader.pdf\ShellEx\{extract}"), None, reader_extract);
        set(&format!(r"Reader.pdf\ShellEx\{preview}"), None, reader_preview);
        set(&format!(r"Default.pdf\ShellEx\{thumbnail}"), None, default_thumbnail);
        for clsid in [reader_extract, reader_preview, default_thumbnail] {
            set(&format!(r"CLSID\{clsid}\InprocServer32"), None, &format!("\"{dll}\""));
        }
        let exe = Path::new(r"C:\Programs\MetalPDF\metalpdf.exe");
        register_in(&r, exe).unwrap();
        assert_eq!(get(&format!(r"MetalPDF.pdf\ShellEx\{extract}")).unwrap(), reader_extract);
        assert_eq!(get(&format!(r"MetalPDF.pdf\ShellEx\{preview}")).unwrap(), reader_preview);
        assert_eq!(get(&format!(r"MetalPDF.pdf\ShellEx\{thumbnail}")).unwrap(), default_thumbnail);
        // With no thumbnail provider lent, the reader's image extractor
        // stands in as one.
        reg::delete_tree(&format!(r"{classes}\Default.pdf")).unwrap();
        register_in(&r, exe).unwrap();
        assert_eq!(get(&format!(r"MetalPDF.pdf\ShellEx\{thumbnail}")).unwrap(), reader_extract);
        // The reader uninstalled: registering again drops what it lent.
        reg::delete_tree(&format!(r"{classes}\Reader.pdf")).unwrap();
        register_in(&r, exe).unwrap();
        for handler in HANDLERS {
            assert_eq!(get(&format!(r"MetalPDF.pdf\ShellEx\{handler}")), None);
        }
        unregister_in(&r).unwrap();
        assert_eq!(get("MetalPDF.pdf"), None);
        reg::delete_tree(&root).unwrap();
    }
}
