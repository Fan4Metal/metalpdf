//! User settings, recent files, the open tabs and where each document was
//! left, stored as JSON (`%APPDATA%\MetalPDF\config\settings.json`, or
//! [`PORTABLE_FILE`] next to the exe, see [`location`]), as MetalMD keeps
//! them. `#[serde(default)]` makes the file of an older version readable,
//! and unknown fields of a newer one are ignored. Written through a `.tmp`
//! file and a rename; saves are numbered, so a later one wins whichever
//! thread finishes first.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::i18n::LangChoice;

pub const MAX_RECENT: usize = 9;
/// Documents whose reading position is kept, the last read first.
pub const MAX_POSITIONS: usize = 100;

/// The number of the last save asked for, and of the last one written.
static SAVES: AtomicU64 = AtomicU64::new(0);
static WRITTEN: Mutex<u64> = Mutex::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeSetting {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeSetting::System => egui::ThemePreference::System,
            ThemeSetting::Light => egui::ThemePreference::Light,
            ThemeSetting::Dark => egui::ThemePreference::Dark,
        }
    }

    pub fn from_preference(p: egui::ThemePreference) -> ThemeSetting {
        match p {
            egui::ThemePreference::System => ThemeSetting::System,
            egui::ThemePreference::Light => ThemeSetting::Light,
            egui::ThemePreference::Dark => ThemeSetting::Dark,
        }
    }
}

/// How the pages are laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    /// All the pages one under another, scrolled through.
    #[default]
    Continuous,
    /// One page at a time.
    Single,
}

/// The zoom: fitted to the view, or a factor (1.0 = 100 %, a point of the
/// page a 96/72 of a pixel at 100 % display scaling, as Acrobat counts it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoomSetting {
    #[default]
    FitWidth,
    FitPage,
    Scale(f32),
}

/// What the left mouse button does over the pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    /// Dragging selects text.
    #[default]
    Select,
    /// Dragging scrolls the pages.
    Hand,
}

/// How a page is sized on the sheet when printed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintScale {
    /// Pages larger than the printable area are fitted into it; the
    /// others print at their size.
    #[default]
    Shrink,
    /// Every page fills the printable area.
    Fit,
    /// Every page at its size, cut off where the sheet ends.
    Actual,
}

/// The print dialog's choices kept between runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintSettings {
    /// The printer last printed on; `None` is Windows' default.
    pub printer: Option<String>,
    pub scale: PrintScale,
    /// A landscape page turned on a portrait sheet, and the reverse.
    pub auto_rotate: bool,
    /// Pages sent as pictures, for printers that print the vector output
    /// wrong.
    pub as_image: bool,
}

impl Default for PrintSettings {
    fn default() -> Self {
        PrintSettings { printer: None, scale: PrintScale::Shrink, auto_rotate: true, as_image: false }
    }
}

/// Which of the two dark themes the dark one is, chosen in the context menu
/// of the theme button (as MetalMD).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DarkStyle {
    /// Neutral greys, as MetalMD and metalmpv.
    #[default]
    Grey,
    /// GitHub's, nearly black.
    Black,
}

/// The side of the window the sidebar is on, chosen in the context menu
/// of its button and of its tabs (as MetalMD's table of contents).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSide {
    #[default]
    Left,
    Right,
}

/// Which list the sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarTab {
    #[default]
    Thumbnails,
    Outline,
    /// The search's hits, with the line each is on.
    Search,
}

/// The main window's place, in physical pixels of the screen, and the
/// monitor's scale factor it was last shown at (to predict the first
/// page's pixel size before the window exists).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    #[serde(default = "one")]
    pub scale_factor: f32,
}

fn one() -> f32 {
    1.0
}

/// Where a document was left.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilePosition {
    pub path: PathBuf,
    /// The page at the top of the view (0-based).
    pub page: usize,
    /// How far down that page the view's top is, as a share of its height.
    pub offset: f32,
    pub zoom: ZoomSetting,
    /// Quarter turns clockwise.
    #[serde(default)]
    pub rotation: u8,
    /// The file's page numbers less the ones printed on the pages: the
    /// file's page `shift + 1` is page 1. 0: the file's own numbering.
    #[serde(default)]
    pub page_shift: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSetting,
    /// Interface language, chosen in About.
    pub language: LangChoice,
    /// Check for a newer release at start-up, at most once a day; off
    /// until turned on in About.
    pub check_updates: bool,
    /// Unix seconds of the last update check that got an answer.
    pub last_update_check: u64,
    pub recent: Vec<PathBuf>,
    /// Where recently read documents were left, the last read first.
    pub positions: Vec<FilePosition>,
    /// The tabs open when the app was last closed, in the order of their
    /// windows; opened again when it starts without files.
    pub session: Vec<PathBuf>,
    /// Which of `session` was shown in the main window.
    pub session_active: usize,
    pub window: Option<WindowGeometry>,
    pub view_mode: ViewMode,
    /// What the mouse does over the pages: select text or drag them.
    pub tool: Tool,
    /// The zoom a document opens with when it has no saved position.
    pub zoom: ZoomSetting,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub sidebar_tab: SidebarTab,
    pub sidebar_side: SidebarSide,
    /// The dark theme's look: grey or black.
    pub dark_style: DarkStyle,
    /// Find matches case.
    pub find_case: bool,
    /// Find matches whole words only.
    pub find_words: bool,
    pub print: PrintSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeSetting::System,
            language: LangChoice::System,
            check_updates: false,
            last_update_check: 0,
            recent: Vec::new(),
            positions: Vec::new(),
            session: Vec::new(),
            session_active: 0,
            window: None,
            view_mode: ViewMode::Continuous,
            tool: Tool::Select,
            zoom: ZoomSetting::FitWidth,
            sidebar_visible: false,
            sidebar_width: 220.0,
            sidebar_tab: SidebarTab::Thumbnails,
            sidebar_side: SidebarSide::Left,
            dark_style: DarkStyle::Grey,
            find_case: false,
            find_words: false,
            print: PrintSettings::default(),
        }
    }
}

/// Settings file that makes a copy portable: when it lies next to the exe
/// (even empty) and can be written, the settings are kept in it instead of
/// the user profile. The portable zip ships an empty one
/// (`PORTABLE_SETTINGS` in tools/make_release.py).
pub const PORTABLE_FILE: &str = "metalpdf.json";

/// Where the settings of this run are kept.
#[derive(Debug)]
pub struct Location {
    /// The settings file, `None` if there is nowhere to keep it.
    pub file: Option<PathBuf>,
    /// `file` is [`PORTABLE_FILE`] next to the exe.
    pub portable: bool,
}

impl Location {
    /// Whether settings of an earlier run are there (a portable file starts
    /// empty). The window is centred only when there are none.
    pub fn has_saved(&self) -> bool {
        self.file.as_ref().is_some_and(|f| f.metadata().is_ok_and(|m| m.len() > 0))
    }
}

/// Where the settings are kept, decided once per run.
pub fn location() -> &'static Location {
    static LOCATION: OnceLock<Location> = OnceLock::new();
    LOCATION.get_or_init(|| {
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
        match exe_dir.and_then(|d| portable_file_in(&d)) {
            Some(file) => Location { file: Some(file), portable: true },
            None => Location {
                file: directories::ProjectDirs::from("", "", crate::APP_NAME)
                    .map(|dirs| dirs.config_dir().join("settings.json")),
                portable: false,
            },
        }
    })
}

/// [`PORTABLE_FILE`] in `dir` if it is there and can be written; a copy on
/// a read-only medium keeps its settings in the profile instead of losing
/// them.
fn portable_file_in(dir: &Path) -> Option<PathBuf> {
    let file = dir.join(PORTABLE_FILE);
    // Opening for appending checks the permission without changing the file.
    std::fs::OpenOptions::new().append(true).open(&file).ok().map(|_| file)
}

impl Settings {
    /// Reads the settings file; missing, empty or malformed files give
    /// defaults.
    pub fn load() -> Settings {
        location()
            .file
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .map(|mut s: Settings| {
                if let ZoomSetting::Scale(z) = s.zoom
                    && !(crate::layout::MIN_ZOOM..=crate::layout::MAX_ZOOM).contains(&z)
                {
                    s.zoom = ZoomSetting::FitWidth;
                }
                if !(120.0..=800.0).contains(&s.sidebar_width) {
                    s.sidebar_width = Settings::default().sidebar_width;
                }
                s
            })
            .unwrap_or_default()
    }

    /// Writes the file now.
    pub fn save(&self) {
        self.save_numbered(SAVES.fetch_add(1, Ordering::SeqCst) + 1);
    }

    /// Saves a copy on a background thread. Saves are numbered when asked
    /// for, so one asked for later, here or in [`Settings::save`], wins even
    /// if this one finishes after it.
    pub fn save_in_background(&self) {
        let number = SAVES.fetch_add(1, Ordering::SeqCst) + 1;
        let settings = self.clone();
        std::thread::spawn(move || settings.save_numbered(number));
    }

    /// Writes the file unless a later save was written already. One writer
    /// at a time: they share the temporary file.
    fn save_numbered(&self, number: u64) {
        let Some(path) = &location().file else { return };
        let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        if *written > number {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                *written = number;
            }
        }
    }

    /// Moves `path` to the front of the recent list.
    pub fn add_recent(&mut self, path: &Path) {
        self.recent.retain(|p| !same_path(p, path));
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    /// Where the document at `path` was left.
    pub fn position(&self, path: &Path) -> Option<&FilePosition> {
        self.positions.iter().find(|p| same_path(&p.path, path))
    }

    /// Keeps where the document at `path` was left, first in the list.
    pub fn set_position(&mut self, position: FilePosition) {
        self.positions.retain(|p| !same_path(&p.path, &position.path));
        self.positions.insert(0, position);
        self.positions.truncate(MAX_POSITIONS);
    }
}

/// Path equality as the file system sees it (case-insensitive on Windows).
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_are_deduplicated_and_capped() {
        let mut settings = Settings::default();
        for i in 0..12 {
            settings.add_recent(Path::new(&format!("/docs/{i}.pdf")));
        }
        settings.add_recent(Path::new("/docs/5.pdf"));
        assert_eq!(settings.recent.len(), MAX_RECENT);
        assert_eq!(settings.recent[0], PathBuf::from("/docs/5.pdf"));
        assert_eq!(settings.recent.iter().filter(|p| p.ends_with("5.pdf")).count(), 1);
        settings.add_recent(Path::new("/DOCS/5.PDF"));
        assert_eq!(settings.recent.iter().filter(|p| p.to_string_lossy().to_lowercase().ends_with("5.pdf")).count(), 1);
    }

    #[test]
    fn positions_are_kept_last_first_and_capped() {
        let mut settings = Settings::default();
        let at = |i: usize| FilePosition {
            path: PathBuf::from(format!("/docs/{i}.pdf")),
            page: i,
            offset: 0.25,
            zoom: ZoomSetting::Scale(1.5),
            rotation: 0,
            page_shift: 0,
        };
        for i in 0..MAX_POSITIONS + 5 {
            settings.set_position(at(i));
        }
        assert_eq!(settings.positions.len(), MAX_POSITIONS);
        settings.set_position(at(50));
        assert_eq!(settings.positions[0].path, PathBuf::from("/docs/50.pdf"));
        assert_eq!(settings.position(Path::new("/docs/50.pdf")).map(|p| p.page), Some(50));
        assert_eq!(settings.position(Path::new("/docs/0.pdf")), None);
    }

    #[test]
    fn portable_file_only_when_present_and_writable() {
        let dir = std::env::temp_dir().join(format!("metalpdf_portable_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PORTABLE_FILE);
        assert_eq!(portable_file_in(&dir), None);
        std::fs::write(&file, "").unwrap();
        assert_eq!(portable_file_in(&dir), Some(file.clone()));
        let mut perm = std::fs::metadata(&file).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&file, perm.clone()).unwrap();
        assert_eq!(portable_file_in(&dir), None);
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&file, perm).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_round_trip_and_tolerate_unknown_fields() {
        let settings = Settings {
            theme: ThemeSetting::Dark,
            language: LangChoice::Ru,
            zoom: ZoomSetting::Scale(1.25),
            view_mode: ViewMode::Single,
            sidebar_tab: SidebarTab::Outline,
            sidebar_side: SidebarSide::Right,
            dark_style: DarkStyle::Black,
            window: Some(WindowGeometry { x: 10, y: 20, width: 900, height: 700, maximized: false, scale_factor: 1.5 }),
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""zoom":{"scale":1.25}"#), "{json}");
        assert!(json.contains(r#""sidebar_side":"right""#) && json.contains(r#""dark_style":"black""#), "{json}");
        assert!(json.contains(r#""language":"ru""#), "{json}");
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), settings);
        let partial: Settings = serde_json::from_str(r#"{"theme":"light","future":1}"#).unwrap();
        assert_eq!(partial.theme, ThemeSetting::Light);
        assert_eq!(partial.zoom, ZoomSetting::FitWidth);
    }

    /// The file of version 0.1.0 reads with defaults for whatever came later.
    #[test]
    fn settings_of_version_0_1_take_defaults_for_what_came_later() {
        let old = r#"{
            "theme": "dark",
            "language": "ru",
            "recent": ["C:\\docs\\a.pdf"],
            "positions": [{"path": "C:\\docs\\a.pdf", "page": 3, "offset": 0.5, "zoom": "fitpage"}],
            "window": {"x": 10, "y": 20, "width": 900, "height": 700, "maximized": false},
            "view_mode": "continuous",
            "zoom": "fitwidth"
        }"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.theme, ThemeSetting::Dark);
        assert_eq!(settings.language, LangChoice::Ru);
        assert_eq!(settings.recent, [PathBuf::from(r"C:\docs\a.pdf")]);
        assert_eq!(settings.positions[0].zoom, ZoomSetting::FitPage);
        assert_eq!(settings.positions[0].rotation, 0);
        assert_eq!(settings.window.unwrap().scale_factor, 1.0);
        assert!(!settings.check_updates);
        assert_eq!(settings.sidebar_width, 220.0);
        assert_eq!(settings.sidebar_side, SidebarSide::Left);
        assert_eq!(settings.dark_style, DarkStyle::Grey);
    }
}
