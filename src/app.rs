//! The egui windows: the root and the ones tabs were dragged out into,
//! each with its tabs (a document and how it is looked at), the engine's
//! answers taken in every frame, the keys, the tab bar and the status bar,
//! the start page with the recent files, and the pieces every app of the
//! family has (theme switch, About, update check, dark caption, settings).
//! The model of windows and tabs is MetalMD's.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egui::{
    Align, CursorIcon, FontId, Id, Key, LayerId, Layout, Margin, Modifiers, Order, Panel, PointerButton, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, ThemePreference, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId, vec2,
};

use crate::cache::Textures;
use crate::dialog::{Ask, Dialogs};
use crate::document::{DocState, Status};
use crate::engine::{DocId, Edit, Engine, Event, Kind, RenderKey, Request};
use crate::pdfium::Target;
use crate::pixels::Pixels;
use crate::settings::{FilePosition, Settings, ThemeSetting, ViewMode, WindowGeometry, ZoomSetting, same_path};
use crate::single_instance::Server;
use crate::style::Palette;
use crate::textures::Texture;
use crate::ui::about::AboutDialog;
use crate::ui::home::HomeAction;
use crate::ui::icons::Icon;
use crate::ui::menu::RightClick;
use crate::ui::status::{PageField, StatusAction, StatusInfo};
use crate::update::Updates;
use crate::layout::Anchor;
use crate::search::FindState;
use crate::selection::{self, Selection};
use crate::settings::SidebarTab;
use crate::ui::sidebar::SidebarAction;
use crate::view::ViewState;

/// What `main` hands the app: the files to open (or the last session), the
/// settings it read, the engine already opening the first one, the
/// single-instance server.
pub struct Startup {
    pub tabs: Vec<PathBuf>,
    /// Which of `tabs` is shown; the engine opened it under `first_id`.
    pub active: usize,
    /// `tabs` are the last run's session, not files asked for.
    pub restored: bool,
    pub settings: Settings,
    pub engine: Engine,
    pub first_id: DocId,
    pub server: Option<Server>,
}

/// An open document and how it is looked at.
pub struct Tab {
    pub id: u64,
    pub path: PathBuf,
    pub title: String,
    pub doc: DocState,
    pub view: ViewState,
    /// The password being typed for an encrypted document.
    pub password: String,
    pub find: FindState,
    pub selection: Option<Selection>,
    /// The mouse is dragging the selection out.
    pub selecting: bool,
    /// Where the reader was before each jump (a link, a bookmark, a
    /// thumbnail, the page field, a hit), for Alt+Left; and the jumps
    /// gone back from, for Alt+Right.
    pub back: Vec<Anchor>,
    pub forward: Vec<Anchor>,
    /// The text editing mode: text objects are outlined, a click edits.
    pub editing: bool,
    pub editor: Option<InlineEditor>,
    /// The image selected in the editing mode.
    pub image: Option<ImageSel>,
    /// A short message for the status bar (an edit refused, a save).
    pub notice: Option<(String, Instant)>,
}

/// A text object being edited in place.
pub struct InlineEditor {
    pub page: usize,
    pub object: usize,
    pub text: String,
    /// The object's box on the unrotated page, in points.
    pub rect: [f32; 4],
    pub font_size: f32,
    /// Just opened: takes the focus.
    pub fresh: bool,
}

/// An image selected in the editing mode: moved and resized by dragging.
pub struct ImageSel {
    pub page: usize,
    pub object: usize,
    /// Its box on the unrotated page, in points, as dragged so far.
    pub rect: [f32; 4],
    pub drag: Option<ImageDrag>,
}

pub struct ImageDrag {
    /// The corner being dragged (0 top left, clockwise, on screen), or
    /// `None` when the whole picture is moved.
    pub corner: Option<u8>,
    pub start: Pos2,
    pub start_rect: [f32; 4],
}

/// Something to confirm before it happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    /// Closing tab `tab` of window `window`, which has unsaved edits.
    CloseTab { window: u64, tab: u64 },
    /// Quitting with unsaved edits.
    Quit,
}

/// Jumps remembered for Alt+Left.
const MAX_HISTORY: usize = 100;

impl Tab {
    fn new(id: u64, path: PathBuf, doc: DocState, view: ViewState) -> Tab {
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        Tab {
            id,
            path,
            title,
            doc,
            view,
            password: String::new(),
            find: FindState::default(),
            selection: None,
            selecting: false,
            back: Vec::new(),
            forward: Vec::new(),
            editing: false,
            editor: None,
            image: None,
            notice: None,
        }
    }

    /// Say something in the status bar for a few seconds.
    pub fn notify(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    /// Remember where the reader is before a jump.
    pub fn remember(&mut self) {
        self.back.push(self.view.anchor);
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// Back to where the reader was before the last jump.
    pub fn go_back(&mut self) {
        if let Some(anchor) = self.back.pop() {
            self.forward.push(self.view.anchor);
            self.view.anchor = anchor;
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(anchor) = self.forward.pop() {
            self.back.push(self.view.anchor);
            self.view.anchor = anchor;
        }
    }

    /// The selected text, page by page.
    pub fn selected_text(&self) -> String {
        let Some(sel) = self.selection else { return String::new() };
        let ((p0, _), (p1, _)) = sel.ordered();
        let mut out = String::new();
        for page in p0..=p1 {
            let Some(text) = self.doc.text.get(&page) else { continue };
            let Some(range) = sel.range_on(page, text.layout.chars.len()) else { continue };
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&selection::text_of(&text.layout, range));
        }
        out
    }

    pub fn layout(&self) -> crate::layout::Layout {
        self.view.layout(&self.doc.sizes, self.doc.fallback)
    }

    fn position(&self) -> FilePosition {
        self.view.position(&self.path)
    }
}

/// A window of the app with its tabs. The first one is the root viewport;
/// a tab dragged out of a window gets a window of its own, an egui viewport
/// that lives as long as the root does (when the root is closed while such
/// windows exist, it takes one of them over instead, see `adopt_window`).
struct Window {
    /// Unique for the run; the viewport id is made from it.
    id: u64,
    viewport: ViewportId,
    tabs: Vec<Tab>,
    active: usize,
    /// The caption shown.
    title: String,
    hwnd: Option<isize>,
    /// How a detached window is made; `None` for the root.
    builder: Option<ViewportBuilder>,
    /// How far a detached window is from being seen.
    appear: Appear,
    /// Its frame and its content on the screen, in points, from the last
    /// frame it drew.
    outer: Option<Rect>,
    inner: Option<Rect>,
    /// Where the first tab starts, from the window's left edge, so a
    /// window made for a dragged tab puts that tab under the pointer.
    tab_lead: f32,
    /// Its close button was pressed (detached windows only).
    close_requested: bool,
    page_field: PageField,
}

impl Window {
    fn new(id: u64, builder: Option<ViewportBuilder>) -> Window {
        let viewport = match builder {
            None => ViewportId::ROOT,
            Some(_) => ViewportId::from_hash_of(("metalpdf-window", id)),
        };
        Window {
            id,
            viewport,
            tabs: Vec::new(),
            active: 0,
            title: String::new(),
            hwnd: None,
            appear: match builder {
                None => Appear::Seen,
                Some(_) => Appear::Looking(0),
            },
            builder,
            outer: None,
            inner: None,
            tab_lead: 76.0,
            close_requested: false,
            page_field: PageField::default(),
        }
    }

    fn is_root(&self) -> bool {
        self.viewport == ViewportId::ROOT
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Takes tab `index` out, keeping the active tab in range.
    fn remove_tab(&mut self, index: usize) -> Tab {
        let tab = self.tabs.remove(index);
        if self.active > index || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        tab
    }

    /// Puts `tab` at the end and shows it.
    fn add_tab(&mut self, tab: Tab) {
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    /// Moves tab `from` to position `to`, the active tab staying the same.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if self.active == from {
            self.active = to;
        } else if from < self.active && self.active <= to {
            self.active -= 1;
        } else if to <= self.active && self.active < from {
            self.active += 1;
        }
    }
}

/// A detached window on its way to the screen. Windows shows a new window
/// white until its first frame reaches DWM, so it is made hidden under
/// `hidden_title`, found by that title, cloaked, given its caption and
/// title, shown, and uncloaked once it has painted a few frames
/// (`App::appear`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Appear {
    /// Its native window not found yet: the frames looked.
    Looking(u32),
    /// Shown, cloaked if it was found: the frames painted since.
    Painting(u64),
    Seen,
}

/// A tab being dragged by its title.
struct TabDrag {
    /// The window it is in.
    window: u64,
    tab: u64,
    /// The pointer's offset from the tab's top left corner.
    grab: Vec2,
    /// Where the pointer was last seen, on the screen in points.
    screen_pos: Option<Pos2>,
}

/// What a tab's context menu asked for.
enum TabMenu {
    Detach(usize),
    ToRoot(usize),
    Close(usize),
    CloseOthers(usize),
}

pub struct App {
    /// The root window first, then the detached ones.
    windows: Vec<Window>,
    /// The window whose frame is being drawn.
    current: usize,
    /// The window that last had the focus: files handed over by another
    /// launch open there.
    focused: u64,
    next_window_id: u64,
    next_tab: u64,
    next_doc: DocId,
    drag: Option<TabDrag>,
    /// Paths of the tabs closed in this run, the last closed last.
    closed_tabs: Vec<PathBuf>,
    settings: Settings,
    settings_dirty: bool,
    engine: Engine,
    /// PDFium could not be loaded: the reason.
    engine_error: Option<String>,
    textures: Textures,
    /// Renders arrived, not yet uploaded: one full render a frame.
    uploads: VecDeque<(RenderKey, Pixels)>,
    /// The renders every window wants this frame.
    wanted: Vec<(RenderKey, u8)>,
    theme: ThemePreference,
    about: AboutDialog,
    updates: Updates,
    dialogs: Dialogs,
    right_click: RightClick,
    /// Files another launch handed over, and the flag to raise the window.
    inbox: Receiver<Vec<PathBuf>>,
    raise: Arc<AtomicBool>,
    /// The theme the caption was last painted in.
    shown_theme: Option<egui::Theme>,
    /// The caption must be repainted in the next frame (the theme changed).
    refresh_caption: bool,
    /// The app icon of the start screen, as a texture.
    start_icon: Option<egui::TextureHandle>,
    /// The root window is cloaked until it has painted a few frames: shown
    /// before its first frame reached DWM, it flashed white. The time it is
    /// shown anyway and the frame it was cloaked in.
    cloak: Option<(Instant, u64)>,
    frame: u64,
    /// The first page has been drawn (a trace mark).
    first_drawn: bool,
    /// The root window moved or resized: its place is saved after a pause.
    window_changed_at: Option<Instant>,
    last_geometry: Option<WindowGeometry>,
    confirm: Option<Confirm>,
    /// The reader chose to quit with unsaved edits: the close goes on.
    quitting: bool,
}

/// Closed tabs Ctrl+Shift+T can open again.
const MAX_CLOSED_TABS: usize = 25;
/// Frames the window paints, cloaked, before it is shown, and the longest
/// it stays cloaked.
const PAINTED_TO_SHOW: u64 = 3;
const UNCLOAK_TIMEOUT: Duration = Duration::from_secs(1);
/// Frames a detached window's native window is looked for before it is
/// shown as it is.
const LOOK_FRAMES: u32 = 10;
/// The window's place is saved this long after it last moved.
const WINDOW_SAVE_DELAY: Duration = Duration::from_secs(1);

/// What the status bar's page field resolves a typed label or number with.
type PageResolver = Box<dyn Fn(&str) -> Option<usize>>;

/// The title a detached window is made with, hidden, until it is found
/// (`App::appear`): unique, as two windows may show the same document.
fn hidden_title(id: u64) -> String {
    format!("MetalPDF window {id}")
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        let Startup { tabs, active, restored, settings, engine, first_id, server } = startup;
        // Before any text is made.
        crate::i18n::set_lang(settings.language.resolve());
        cc.egui_ctx.set_theme(settings.theme.preference());
        cc.egui_ctx.options_mut(|o| {
            // Ctrl+plus and Ctrl+minus are the app's, not egui's zoom.
            o.zoom_with_keyboard = false;
            // A double click as slow as Windows allows (egui: 0.3 s).
            o.input_options.max_double_click_delay = crate::win::double_click_time();
            // A wheel notch scrolls 100 points instead of egui's 40, about
            // what Windows and Chromium scroll (the same in every app of
            // the family).
            o.input_options.line_scroll_speed = 100.0;
        });
        engine.set_context(cc.egui_ctx.clone());
        // The window is still hidden (eframe shows it after the first
        // frame), so a caption set now is shown from the start; set after
        // egui's first frame, Windows 10 would keep the caption of the
        // Windows theme. Following Windows, winit has already done it.
        let hwnd = crate::caption::hwnd(cc);
        let mut cloak = None;
        if let Some(hwnd) = hwnd {
            crate::caption::cloak(hwnd, true);
            crate::placement::keep_on_screen(hwnd);
            if crate::MAXIMIZE_WHEN_SHOWN.swap(false, Ordering::Relaxed) {
                crate::placement::show_maximized(hwnd);
            }
            cloak = Some((Instant::now() + UNCLOAK_TIMEOUT, cc.egui_ctx.cumulative_frame_nr()));
            match settings.theme.preference() {
                ThemePreference::Dark => crate::caption::set_dark(hwnd, true),
                ThemePreference::Light => crate::caption::set_dark(hwnd, false),
                ThemePreference::System => {}
            }
        }
        // Files handed over by later launches.
        let (inbox_tx, inbox): (Sender<Vec<PathBuf>>, Receiver<Vec<PathBuf>>) = std::sync::mpsc::channel();
        let raise = Arc::new(AtomicBool::new(false));
        if let Some(server) = server {
            let raise = Arc::clone(&raise);
            let ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
            let _ = ctx.set(cc.egui_ctx.clone());
            server.spawn(move |paths| {
                let _ = inbox_tx.send(paths);
                raise.store(true, Ordering::Release);
                if let Some(ctx) = ctx.get() {
                    ctx.request_repaint();
                }
            });
        }
        let mut updates = Updates::new(settings.check_updates, settings.last_update_check);
        updates.start_if_due(&cc.egui_ctx);
        let mut about = AboutDialog::default();
        about.lang = settings.language;
        let mut root = Window::new(0, None);
        root.hwnd = hwnd;
        let mut app = Self {
            windows: vec![root],
            current: 0,
            focused: 0,
            next_window_id: 1,
            next_tab: 1,
            next_doc: first_id + 1,
            drag: None,
            closed_tabs: Vec::new(),
            theme: settings.theme.preference(),
            settings,
            settings_dirty: false,
            engine,
            engine_error: None,
            textures: Textures::default(),
            uploads: VecDeque::new(),
            wanted: Vec::new(),
            about,
            updates,
            dialogs: Dialogs::default(),
            right_click: RightClick::default(),
            inbox,
            raise,
            shown_theme: None,
            refresh_caption: false,
            start_icon: None,
            cloak,
            frame: 0,
            first_drawn: false,
            window_changed_at: None,
            last_geometry: None,
            confirm: None,
            quitting: false,
        };
        // The tabs: the active one is being opened by the engine already
        // under `first_id`; the others are asked for now.
        for (i, path) in tabs.iter().enumerate() {
            let doc_id = if i == active { first_id } else { app.take_doc_id() };
            if i != active {
                app.engine.request(Request::Open { id: doc_id, path: path.clone(), password: None, first: None });
            }
            let view = app.view_for(path);
            let tab_id = app.take_tab_id();
            app.windows[0].tabs.push(Tab::new(tab_id, path.clone(), DocState::new(doc_id), view));
            if !restored {
                app.settings.add_recent(path);
                app.settings_dirty = true;
            }
        }
        app.windows[0].active = active.min(tabs.len().saturating_sub(1));
        app
    }

    fn take_doc_id(&mut self) -> DocId {
        let id = self.next_doc;
        self.next_doc += 1;
        id
    }

    fn take_tab_id(&mut self) -> u64 {
        let id = self.next_tab;
        self.next_tab += 1;
        id
    }

    /// The window being drawn.
    fn win(&mut self) -> &mut Window {
        &mut self.windows[self.current]
    }

    fn window_index(&self, id: u64) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.windows.iter().flat_map(|w| w.tabs.iter())
    }

    /// The view a document at `path` opens with: where it was left, else
    /// the defaults.
    fn view_for(&self, path: &Path) -> ViewState {
        let mut view = ViewState::new(self.settings.view_mode, self.settings.zoom);
        if let Some(position) = self.settings.position(path) {
            view.restore(position);
        }
        view
    }

    /// Opens `paths` in window `window` (the current one by default).
    fn open_paths(&mut self, paths: Vec<PathBuf>, window: Option<usize>) {
        let target = window.unwrap_or(self.current).min(self.windows.len() - 1);
        for path in paths {
            self.open_path(crate::normalize(&path), target);
        }
    }

    /// Opens `path` in a tab of window `target` (or activates its tab, in
    /// whichever window it is).
    fn open_path(&mut self, path: PathBuf, target: usize) {
        if self.engine_error.is_some() {
            return;
        }
        let open = self
            .windows
            .iter()
            .enumerate()
            .find_map(|(w, window)| window.tabs.iter().position(|t| same_path(&t.path, &path)).map(|t| (w, t)));
        if let Some((w, index)) = open {
            self.windows[w].active = index;
            return;
        }
        let doc_id = self.take_doc_id();
        let tab_id = self.take_tab_id();
        self.engine.request(Request::Open { id: doc_id, path: path.clone(), password: None, first: None });
        let view = self.view_for(&path);
        self.windows[target].add_tab(Tab::new(tab_id, path.clone(), DocState::new(doc_id), view));
        self.settings.add_recent(&path);
        self.settings_dirty = true;
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.win().tabs.len() {
            return;
        }
        let (window, tab_id, modified) = {
            let w = self.win();
            (w.id, w.tabs[index].id, w.tabs[index].doc.modified)
        };
        if modified && self.confirm.is_none() {
            self.confirm = Some(Confirm::CloseTab { window, tab: tab_id });
            return;
        }
        let tab = self.win().remove_tab(index);
        self.drop_tab(tab);
    }

    /// Close tab `tab` of window `window` whatever its edits.
    fn force_close_tab(&mut self, window: u64, tab: u64) {
        if let Some(w) = self.window_index(window)
            && let Some(i) = self.windows[w].tab_index(tab)
        {
            let tab = self.windows[w].remove_tab(i);
            self.drop_tab(tab);
        }
    }

    /// Ask where to save the edited document of tab `tab` of window
    /// `window`.
    fn save_as(&mut self, ctx: &egui::Context, window: u64, tab: u64) {
        let Some(w) = self.window_index(window) else { return };
        let Some(i) = self.windows[w].tab_index(tab) else { return };
        let path = self.windows[w].tabs[i].path.clone();
        let hwnd = self.windows[w].hwnd;
        self.dialogs.start(
            ctx,
            hwnd,
            Ask::SaveAs { tab },
            move || {
                let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
                if let Some(folder) = path.parent() {
                    dialog = dialog.set_directory(folder);
                }
                if let Some(name) = path.file_name() {
                    dialog = dialog.set_file_name(name.to_string_lossy());
                }
                dialog
            },
            crate::dialog::save_file,
        );
    }

    /// Ask for the picture to put into image `object` of `page` of tab
    /// `tab`.
    fn replace_image_dialog(&mut self, ctx: &egui::Context, tab: u64, page: usize, object: usize) {
        let hwnd = self.windows[self.current].hwnd;
        self.dialogs.start(
            ctx,
            hwnd,
            Ask::ReplaceImage { tab, page, object },
            || rfd::FileDialog::new().add_filter("JPEG and PNG", &["jpg", "jpeg", "png"]),
            crate::dialog::pick_file,
        );
    }

    /// Ask for the PDF whose pages go in at `page` of tab `tab`'s document
    /// (or replace that page).
    fn import_pdf_dialog(&mut self, ctx: &egui::Context, tab: u64, page: usize, replace: bool) {
        let hwnd = self.windows[self.current].hwnd;
        self.dialogs.start(
            ctx,
            hwnd,
            Ask::ImportPdf { tab, page, replace },
            || rfd::FileDialog::new().add_filter("PDF", &["pdf"]),
            crate::dialog::pick_file,
        );
    }

    /// The edited document of tab `tab` written to `path`.
    fn save_to(&mut self, tab: u64, path: PathBuf) {
        for window in &mut self.windows {
            if let Some(t) = window.tabs.iter_mut().find(|t| t.id == tab) {
                self.engine.request(Request::Save { id: t.doc.id, path });
                return;
            }
        }
    }

    /// Open the tab's file again after it was saved over: the viewed
    /// document read the old file; the view stays where it is.
    fn reopen_tab(&mut self, doc: DocId) {
        let new_id = self.take_doc_id();
        for window in &mut self.windows {
            if let Some(t) = window.tabs.iter_mut().find(|t| t.doc.id == doc) {
                self.textures.remove_doc(doc);
                self.uploads.retain(|(k, _)| k.doc != doc);
                self.engine.request(Request::Close(doc));
                self.engine.request(Request::Open { id: new_id, path: t.path.clone(), password: None, first: None });
                t.doc = DocState::new(new_id);
                t.editor = None;
                t.image = None;
                t.selection = None;
                t.find.close();
                return;
            }
        }
    }

    /// The confirmation asked for, as a modal.
    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm else { return };
        let name = match confirm {
            Confirm::CloseTab { window, tab } => self
                .window_index(window)
                .and_then(|w| self.windows[w].tab_index(tab).map(|i| self.windows[w].tabs[i].title.clone()))
                .unwrap_or_default(),
            Confirm::Quit => String::new(),
        };
        let mut choice = None;
        let modal = egui::Modal::new(Id::new("confirm")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.add_space(4.0);
            let text = match confirm {
                Confirm::CloseTab { .. } => tr!(format!("{name} has unsaved changes."), format!("В {name} есть несохранённые изменения.")),
                Confirm::Quit => tr!("Some documents have unsaved changes.", "В некоторых документах есть несохранённые изменения.").to_owned(),
            };
            ui.label(text);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if matches!(confirm, Confirm::CloseTab { .. }) && ui.button(tr!("Save as…", "Сохранить как…")).clicked() {
                    choice = Some(0);
                }
                if ui.button(tr!("Discard", "Не сохранять")).clicked() {
                    choice = Some(1);
                }
                if ui.button(tr!("Cancel", "Отмена")).clicked() {
                    choice = Some(2);
                }
            });
            ui.add_space(4.0);
        });
        if modal.should_close() {
            choice = Some(2);
        }
        let Some(choice) = choice else { return };
        self.confirm = None;
        match (confirm, choice) {
            (Confirm::CloseTab { window, tab }, 0) => self.save_as(ctx, window, tab),
            (Confirm::CloseTab { window, tab }, 1) => self.force_close_tab(window, tab),
            (Confirm::Quit, 1) => {
                self.quitting = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            _ => {}
        }
    }

    /// Closes every tab of the current window but tab `index`.
    fn close_other_tabs(&mut self, index: usize) {
        let window = self.win();
        if index >= window.tabs.len() {
            return;
        }
        let kept = window.tabs.remove(index);
        let closed = std::mem::replace(&mut window.tabs, vec![kept]);
        window.active = 0;
        for tab in closed {
            self.drop_tab(tab);
        }
    }

    /// Lets go of a tab taken out of its window: its position kept, its
    /// textures and the engine's handles freed.
    fn drop_tab(&mut self, tab: Tab) {
        if tab.doc.ready() {
            self.settings.set_position(tab.position());
        }
        self.settings_dirty = true;
        self.closed_tabs.retain(|p| !same_path(p, &tab.path));
        self.closed_tabs.push(tab.path.clone());
        if self.closed_tabs.len() > MAX_CLOSED_TABS {
            self.closed_tabs.remove(0);
        }
        self.textures.remove_doc(tab.doc.id);
        self.uploads.retain(|(k, _)| k.doc != tab.doc.id);
        self.engine.request(Request::Close(tab.doc.id));
    }

    /// Opens the last closed tab again (Ctrl+Shift+T).
    fn reopen_closed_tab(&mut self) {
        while let Some(path) = self.closed_tabs.pop() {
            if path.is_file() {
                let cur = self.current;
                self.open_path(path, cur);
                return;
            }
        }
    }

    /// Take the engine's answers.
    fn poll_engine(&mut self) {
        while let Some(event) = self.engine.poll() {
            match event {
                Event::Unavailable(reason) => {
                    self.engine_error = Some(reason);
                    for tab in self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()) {
                        tab.doc.status = Status::Failed(crate::pdfium::OpenError::File("no engine".into()));
                    }
                }
                Event::Rendered { key, pixels, took_ms } => {
                    if self.all_tabs().any(|t| t.doc.id == key.doc) {
                        if took_ms > 0.0 {
                            log::debug!("page {} at {} px arrived after {took_ms:.1} ms", key.page + 1, key.width);
                        }
                        // The newest render of a key replaces an older one waiting.
                        self.uploads.retain(|(k, _)| *k != key);
                        self.uploads.push_back((key, pixels));
                    }
                }
                Event::RenderFailed(key) => log::warn!("page {} could not be rendered", key.page + 1),
                other => {
                    let doc = match &other {
                        Event::Opened { id, .. }
                        | Event::Sizes { id, .. }
                        | Event::Outline { id, .. }
                        | Event::TextLayout { id, .. }
                        | Event::Links { id, .. }
                        | Event::SearchHits { id, .. }
                        | Event::Objects { id, .. }
                        | Event::Edited { id, .. }
                        | Event::EditFailed { id, .. }
                        | Event::Restructured { id, .. }
                        | Event::Saved { id, .. } => *id,
                        _ => continue,
                    };
                    if let Event::EditFailed { reason, .. } = &other {
                        if let Some(tab) = self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|t| t.doc.id == doc) {
                            tab.notify(tr!(format!("The text could not be edited: {reason}"), format!("Текст не удалось изменить: {reason}")));
                        }
                        continue;
                    }
                    if let Event::Saved { path, result, .. } = &other {
                        let mut reopen = false;
                        if let Some(tab) = self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|t| t.doc.id == doc) {
                            match result {
                                Ok(()) => {
                                    tab.doc.modified = false;
                                    tab.notify(tr!(format!("Saved to {}", path.display()), format!("Сохранено в {}", path.display())));
                                    reopen = same_path(path, &tab.path);
                                }
                                Err(e) => tab.notify(tr!(format!("Could not save: {e}"), format!("Не удалось сохранить: {e}"))),
                            }
                        }
                        if reopen {
                            self.reopen_tab(doc);
                        }
                        continue;
                    }
                    if let Event::Restructured { page_count, .. } = &other {
                        // Every page may be another now: the renders and
                        // everything read of the pages go.
                        self.textures.remove_doc(doc);
                        self.uploads.retain(|(k, _)| k.doc != doc);
                        let engine = &self.engine;
                        if let Some(tab) = self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|t| t.doc.id == doc) {
                            tab.doc.restructure(engine, *page_count);
                            let count = page_count.max(&1) - 1;
                            if tab.view.anchor.page > count {
                                tab.view.anchor.page = count;
                            }
                            tab.view.current = tab.view.current.min(count);
                            tab.image = None;
                            tab.editor = None;
                            tab.selection = None;
                            tab.find.close();
                        }
                        continue;
                    }
                    if let Event::Edited { page, .. } = &other {
                        // The page's renders show the old content; the objects'
                        // indices may have changed, so nothing stays selected.
                        let page = *page as u32;
                        self.textures.full.remove_page(doc, page);
                        self.textures.preview.remove_page(doc, page);
                        self.uploads.retain(|(k, _)| !(k.doc == doc && k.page == page));
                        if let Some(tab) = self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|t| t.doc.id == doc) {
                            tab.image = None;
                            tab.editor = None;
                        }
                    }
                    let engine = &self.engine;
                    if let Event::SearchHits { search, page, hits, done, .. } = &other {
                        if let Some(tab) = self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|t| t.doc.id == doc) {
                            tab.find.take(*search, *page, hits.clone(), *done);
                        }
                        continue;
                    }
                    if let Some(tab) = self.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|t| t.doc.id == doc)
                        && tab.doc.take(&other, engine)
                        && matches!(other, Event::Opened { .. })
                        && tab.doc.ready()
                    {
                        // The position restored before the sizes are known
                        // still names the page; the sizes round it first.
                        let page = tab.view.anchor.page;
                        tab.doc.need_sizes_near(engine, page);
                        log::debug!("{} opened: {} pages", tab.title, tab.doc.page_count);
                    }
                }
            }
        }
    }

    /// Upload the renders that arrived: every preview (small), one full
    /// render a frame (a page takes a few milliseconds of the UI thread),
    /// the one nearest the current page first.
    fn upload_pending(&mut self, frame: &mut eframe::Frame) {
        let Some(gl) = frame.gl().cloned() else { return };
        let current: Vec<(DocId, u32)> = self.windows.iter().filter_map(|w| w.active_tab()).map(|t| (t.doc.id, t.view.current as u32)).collect();
        let distance = |k: &RenderKey| current.iter().find(|(d, _)| *d == k.doc).map_or(u32::MAX, |(_, p)| k.page.abs_diff(*p));
        let mut full_done = false;
        let mut rest = VecDeque::new();
        self.uploads.make_contiguous().sort_by_key(|(k, _)| (k.kind == Kind::Full, distance(k)));
        while let Some((key, pixels)) = self.uploads.pop_front() {
            if key.kind == Kind::Full && full_done {
                rest.push_back((key, pixels));
                continue;
            }
            match Texture::new(gl.clone(), &pixels, |t| frame.register_native_glow_texture(t)) {
                Ok(texture) => {
                    let bytes = texture.bytes;
                    self.textures.pool(key.kind).insert(key, texture, bytes, self.frame);
                    if key.kind == Kind::Full {
                        full_done = true;
                        if !self.first_drawn {
                            self.first_drawn = true;
                            log::debug!("first page uploaded at {:.0} ms", crate::since_start().as_secs_f64() * 1e3);
                        }
                    }
                }
                Err(e) => log::warn!("texture of page {}: {e}", key.page + 1),
            }
        }
        self.uploads = rest;
    }

    /// Show the root window once it has painted `PAINTED_TO_SHOW` frames.
    fn uncloak(&mut self, ctx: &egui::Context) {
        let Some((until, frame)) = self.cloak else { return };
        if ctx.cumulative_frame_nr() >= frame + PAINTED_TO_SHOW || Instant::now() >= until {
            if let Some(hwnd) = self.windows[0].hwnd {
                crate::caption::cloak(hwnd, false);
            }
            self.cloak = None;
            log::debug!("window shown at {:.0} ms", crate::since_start().as_secs_f64() * 1e3);
        } else {
            ctx.request_repaint();
        }
    }

    /// Bring the window captions in line with the theme. egui passes a new
    /// theme to the window at the end of the frame it changed in, and
    /// Windows applies it to the caption only on the next repaint of the
    /// frame, so the frame after the change asks for one.
    fn follow_theme(&mut self, ctx: &egui::Context) {
        let theme = ctx.theme();
        if std::mem::take(&mut self.refresh_caption) {
            for hwnd in self.windows.iter().filter_map(|w| w.hwnd) {
                crate::caption::set_dark(hwnd, theme == egui::Theme::Dark);
                crate::caption::refresh(hwnd);
            }
        }
        if self.shown_theme.is_some_and(|t| t != theme) {
            self.refresh_caption = true;
            ctx.request_repaint();
        }
        self.shown_theme = Some(theme);
    }

    /// Brings the current detached window to the screen (`Appear`), at the
    /// start of each of its frames (`ctx` is its viewport's). eframe gives
    /// no handle for a child viewport, so its window is found by the title
    /// it is made with; once found it is cloaked and gets the dark caption
    /// while still hidden, then its real title, and is shown. Not found
    /// within `LOOK_FRAMES`, it is shown as it is.
    fn appear(&mut self, ctx: &egui::Context) {
        let more = || {
            ctx.request_repaint();
            ctx.request_repaint_of(ViewportId::ROOT);
        };
        let cur = self.current;
        match self.windows[cur].appear {
            Appear::Looking(looked) => {
                let known: Vec<isize> = self.windows.iter().filter_map(|w| w.hwnd).collect();
                let found = crate::caption::thread_window(&hidden_title(self.windows[cur].id)).filter(|h| !known.contains(h));
                let window = &mut self.windows[cur];
                if let Some(hwnd) = found {
                    crate::caption::cloak(hwnd, true);
                    crate::caption::set_dark(hwnd, ctx.theme() == egui::Theme::Dark);
                    window.hwnd = Some(hwnd);
                } else if looked < LOOK_FRAMES {
                    window.appear = Appear::Looking(looked + 1);
                    more();
                    return;
                }
                window.appear = Appear::Painting(0);
                // The title before the window shows, so the taskbar never
                // has the one it was made with.
                self.update_title(ctx);
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                more();
            }
            Appear::Painting(painted) => {
                let window = &mut self.windows[cur];
                if painted + 1 >= PAINTED_TO_SHOW {
                    if let Some(hwnd) = window.hwnd {
                        crate::caption::cloak(hwnd, false);
                    }
                    window.appear = Appear::Seen;
                } else {
                    window.appear = Appear::Painting(painted + 1);
                }
                more();
            }
            Appear::Seen => {}
        }
    }

    /// Restores and focuses the window that files handed over by another
    /// launch open in.
    fn raise_window(&self, ctx: &egui::Context) {
        let window = &self.windows[self.window_index(self.focused).unwrap_or(0)];
        if let Some(hwnd) = window.hwnd {
            crate::placement::bring_to_front(hwnd);
            return;
        }
        ctx.send_viewport_cmd_to(window.viewport, ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(window.viewport, ViewportCommand::Focus);
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        let window = self.windows[self.current].id;
        let folder = self.win().active_tab().and_then(|t| t.path.parent().map(Path::to_path_buf));
        let hwnd = self.win().hwnd;
        self.dialogs.start(
            ctx,
            hwnd,
            Ask::Open { window },
            move || {
                let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).add_filter("All files", &["*"]);
                if let Some(folder) = folder {
                    dialog = dialog.set_directory(folder);
                }
                dialog
            },
            crate::dialog::pick_files,
        );
    }

    /// The keys of the current window, when no text field has the focus.
    fn keys(&mut self, ctx: &egui::Context) {
        if self.about.open {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let consume = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if consume(Modifiers::NONE, Key::F1) {
            self.about.toggle();
        }
        if consume(Modifiers::COMMAND, Key::O) {
            self.open_dialog(ctx);
        }
        if consume(Modifiers::NONE, Key::F11) {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!full));
        }
        if consume(Modifiers::COMMAND, Key::G) {
            self.win().page_field.focus = true;
        }
        if consume(Modifiers::NONE, Key::F4) {
            self.settings.sidebar_visible = !self.settings.sidebar_visible;
            self.settings_dirty = true;
        }
        if consume(Modifiers::COMMAND, Key::F)
            && let Some(tab) = self.win().active_tab_mut()
        {
            tab.find.open();
        }
        if consume(Modifiers::NONE, Key::F3)
            && let Some(tab) = self.win().active_tab_mut()
            && tab.find.open
        {
            tab.find.step(true);
        }
        if consume(Modifiers::SHIFT, Key::F3)
            && let Some(tab) = self.win().active_tab_mut()
            && tab.find.open
        {
            tab.find.step(false);
        }
        if consume(Modifiers::NONE, Key::Escape)
            && let Some(tab) = self.win().active_tab_mut()
        {
            if tab.find.open {
                tab.find.close();
            } else if tab.image.is_some() {
                tab.image = None;
            } else {
                tab.selection = None;
            }
        }
        // Ctrl+C comes as an event, not a key.
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))
            && !typing
            && let Some(tab) = self.win().active_tab()
        {
            let text = tab.selected_text();
            if !text.is_empty() {
                ctx.copy_text(text);
            }
        }
        if (consume(Modifiers::ALT, Key::ArrowLeft) || ctx.input(|i| i.pointer.button_pressed(PointerButton::Extra1)))
            && let Some(tab) = self.win().active_tab_mut() {
                tab.go_back();
            }
        if (consume(Modifiers::ALT, Key::ArrowRight) || ctx.input(|i| i.pointer.button_pressed(PointerButton::Extra2)))
            && let Some(tab) = self.win().active_tab_mut() {
                tab.go_forward();
            }
        if consume(Modifiers::COMMAND, Key::A)
            && !typing
            && let Some(tab) = self.win().active_tab_mut()
        {
            let page = tab.view.current;
            if let Some(text) = tab.doc.text.get(&page)
                && !text.layout.chars.is_empty()
            {
                tab.selection = Some(Selection { anchor: (page, 0), focus: (page, text.layout.chars.len() - 1) });
            }
        }
        if consume(Modifiers::COMMAND, Key::W) {
            let active = self.win().active;
            self.close_tab(active);
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::T) {
            self.reopen_closed_tab();
        }
        if consume(Modifiers::COMMAND, Key::S) || consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            let w = self.win();
            if let Some(tab) = w.active_tab()
                && tab.doc.edits > 0
            {
                let (window, tab) = (w.id, tab.id);
                self.save_as(ctx, window, tab);
            }
        }
        if consume(Modifiers::COMMAND, Key::Z) && !typing {
            let engine = &self.engine;
            if let Some(tab) = self.windows[self.current].active_tab_mut()
                && tab.doc.edits > 0
            {
                tab.editor = None;
                engine.request(Request::UndoEdit(tab.doc.id));
            }
        }
        if consume(Modifiers::NONE, Key::E)
            && !typing
            && let Some(tab) = self.win().active_tab_mut()
        {
            tab.editing = !tab.editing;
            tab.editor = None;
            tab.image = None;
            tab.selection = None;
        }
        if consume(Modifiers::NONE, Key::Delete) && !typing {
            let engine = &self.engine;
            if let Some(tab) = self.windows[self.current].active_tab_mut()
                && let Some(sel) = tab.image.take()
            {
                engine.request(Request::Edit { id: tab.doc.id, edit: Edit::Remove { page: sel.page, object: sel.object } });
            }
        }
        let count = self.win().tabs.len();
        if count > 1 {
            if consume(Modifiers::COMMAND, Key::Tab) {
                let w = self.win();
                w.active = (w.active + 1) % count;
            }
            if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
                let w = self.win();
                w.active = (w.active + count - 1) % count;
            }
        }
        if typing {
            return;
        }
        let Some(tab) = self.win().active_tab_mut() else {
            return;
        };
        if !tab.doc.ready() {
            return;
        }
        let count = tab.doc.page_count;
        let layout = tab.layout();
        let sizes = tab.doc.sizes.clone();
        let fallback = tab.doc.fallback;
        let view = &mut tab.view;
        if consume(Modifiers::NONE, Key::PageDown) || consume(Modifiers::NONE, Key::Space) {
            view.page_step(&layout, true, count);
        }
        if consume(Modifiers::NONE, Key::PageUp) || consume(Modifiers::SHIFT, Key::Space) {
            view.page_step(&layout, false, count);
        }
        if consume(Modifiers::NONE, Key::Home) || consume(Modifiers::COMMAND, Key::Home) {
            view.go_to_page(0, count);
        }
        if consume(Modifiers::NONE, Key::End) || consume(Modifiers::COMMAND, Key::End) {
            view.go_to_page(count.saturating_sub(1), count);
            view.anchor.offset = 1.0;
        }
        if consume(Modifiers::COMMAND, Key::PageDown) || consume(Modifiers::NONE, Key::N) {
            view.turn_page(true, count);
        }
        if consume(Modifiers::COMMAND, Key::PageUp) || consume(Modifiers::NONE, Key::P) {
            view.turn_page(false, count);
        }
        if consume(Modifiers::NONE, Key::ArrowDown) {
            view.scroll_by(&layout, 60.0);
        }
        if consume(Modifiers::NONE, Key::ArrowUp) {
            view.scroll_by(&layout, -60.0);
        }
        if consume(Modifiers::NONE, Key::ArrowRight) {
            if view.mode == ViewMode::Single {
                view.turn_page(true, count);
            } else {
                view.scroll_x += 60.0;
            }
        }
        if consume(Modifiers::NONE, Key::ArrowLeft) {
            if view.mode == ViewMode::Single {
                view.turn_page(false, count);
            } else {
                view.scroll_x = (view.scroll_x - 60.0).max(0.0);
            }
        }
        if consume(Modifiers::COMMAND, Key::Num0) {
            view.set_zoom(ZoomSetting::FitWidth, None, &sizes, fallback);
        }
        if consume(Modifiers::COMMAND, Key::Num1) {
            view.set_zoom(ZoomSetting::Scale(1.0), None, &sizes, fallback);
        }
        if consume(Modifiers::COMMAND, Key::Num2) {
            view.set_zoom(ZoomSetting::FitPage, None, &sizes, fallback);
        }
        if consume(Modifiers::COMMAND, Key::Plus)
            || consume(Modifiers::COMMAND, Key::Equals)
            || consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::Plus)
        {
            view.zoom_step(true, None, &sizes, fallback);
        }
        if consume(Modifiers::COMMAND, Key::Minus) {
            view.zoom_step(false, None, &sizes, fallback);
        }
        if consume(Modifiers::NONE, Key::R) {
            view.rotate(true);
        }
        if consume(Modifiers::SHIFT, Key::R) {
            view.rotate(false);
        }
    }

    /// Apply what the status bar asked.
    fn apply_status(&mut self, actions: Vec<StatusAction>) {
        let engine = &self.engine;
        let mut mode_change = None;
        if let Some(tab) = self.windows[self.current].active_tab_mut() {
            let count = tab.doc.page_count;
            let sizes = tab.doc.sizes.clone();
            let fallback = tab.doc.fallback;
            for action in actions {
                match action {
                    StatusAction::GoToPage(page) => {
                        tab.remember();
                        tab.view.go_to_page(page, count);
                        tab.doc.need_sizes_near(engine, page);
                    }
                    StatusAction::ZoomIn => tab.view.zoom_step(true, None, &sizes, fallback),
                    StatusAction::ZoomOut => tab.view.zoom_step(false, None, &sizes, fallback),
                    StatusAction::Zoom(z) => tab.view.set_zoom(z, None, &sizes, fallback),
                    StatusAction::Rotate { clockwise } => tab.view.rotate(clockwise),
                    StatusAction::Mode(mode) => {
                        tab.view.set_mode(mode);
                        mode_change = Some(mode);
                    }
                    StatusAction::ToggleEdit => {
                        tab.editing = !tab.editing;
                        tab.editor = None;
                        tab.image = None;
                        tab.selection = None;
                    }
                }
            }
        }
        if let Some(mode) = mode_change {
            self.settings.view_mode = mode;
            self.settings_dirty = true;
        }
    }

    /// Follow a link's target in the current window's tab.
    fn follow(&mut self, target: Target) {
        match target {
            Target::Uri(uri) => {
                if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:") {
                    crate::win::shell_open(&uri);
                } else {
                    log::info!("link to {uri} not followed");
                }
            }
            Target::Page { index, y, .. } => {
                let engine = &self.engine;
                if let Some(tab) = self.windows[self.current].active_tab_mut() {
                    let count = tab.doc.page_count;
                    tab.remember();
                    match (y, tab.doc.text.get(&index)) {
                        // The point on the page, through the page's transform.
                        (Some(y), Some(text)) => {
                            let p = text.to_page_points(0.0, y);
                            let h = tab.doc.size_of(index)[1];
                            tab.view.go_to_point(index, p[1], count, h);
                        }
                        _ => tab.view.go_to_page(index, count),
                    }
                    tab.doc.need_sizes_near(engine, index);
                }
            }
        }
    }

    /// The find bar under the tab bar while the active tab's search is
    /// open: the query, the options, the count, previous and next.
    fn find_bar(&mut self, ui: &mut Ui, palette: &Palette) {
        let cur = self.current;
        let wid = self.windows[cur].id;
        let (case, words) = (self.settings.find_case, self.settings.find_words);
        let engine = &self.engine;
        let mut toggle_case = false;
        let mut toggle_words = false;
        let mut close = false;
        let Some(tab) = self.windows[cur].active_tab_mut() else { return };
        if !tab.find.open {
            return;
        }
        Panel::top(Id::new(("find-bar", wid)))
            .frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(Margin::symmetric(10, 5)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let input = ui.add(
                        egui::TextEdit::singleline(&mut tab.find.query)
                            .id(Id::new(("find-input", wid)))
                            .hint_text(tr!("Find in document", "Найти в документе"))
                            .desired_width(280.0),
                    );
                    if std::mem::take(&mut tab.find.request_focus) {
                        input.request_focus();
                    }
                    if input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let forward = !ui.input(|i| i.modifiers.shift);
                        tab.find.step(forward);
                        input.request_focus();
                    }
                    let toggle = |ui: &mut Ui, on: bool, text: RichText, tip: &str| {
                        ui.add(egui::Button::selectable(on, text.size(13.0))).on_hover_text(tip).clicked()
                    };
                    toggle_case = toggle(ui, case, RichText::new("Aa"), tr!("Match case", "Учитывать регистр"));
                    toggle_words = toggle(ui, words, RichText::new("ab").underline(), tr!("Whole words", "Слово целиком"));
                    ui.label(RichText::new(tab.find.status()).size(12.5).color(palette.muted));
                    if crate::ui::icon_button(ui, Icon::Up, false, tr!("Previous (Shift+F3)", "Предыдущее (Shift+F3)")).clicked() {
                        tab.find.step(false);
                    }
                    if crate::ui::icon_button(ui, Icon::Next, false, tr!("Next (F3, Enter)", "Следующее (F3, Enter)")).clicked() {
                        tab.find.step(true);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = crate::ui::close_cross(ui);
                    });
                });
            });
        // A changed query or option starts the search over, from the
        // page the reader is on.
        let (case, words) = (case != toggle_case, words != toggle_words);
        if let Some(search) = tab.find.needs_search(case, words) {
            engine.request(Request::StopSearch(tab.doc.id));
            engine.request(Request::Search {
                id: tab.doc.id,
                search,
                query: tab.find.query.clone(),
                match_case: case,
                whole_words: words,
                from: tab.view.current,
            });
        }
        if close {
            tab.find.close();
            engine.request(Request::StopSearch(tab.doc.id));
        }
        if toggle_case {
            self.settings.find_case = !self.settings.find_case;
            self.settings_dirty = true;
        }
        if toggle_words {
            self.settings.find_words = !self.settings.find_words;
            self.settings_dirty = true;
        }
    }

    /// Scroll to the current hit once its page's text (the transform to
    /// the page) is there.
    fn jump_to_hit(&mut self) {
        let engine = &self.engine;
        let Some(tab) = self.windows[self.current].active_tab_mut() else { return };
        if !tab.find.jump {
            return;
        }
        let Some((page, hit)) = tab.find.current_hit() else {
            tab.find.jump = false;
            return;
        };
        let Some(rect) = hit.rects.first().copied() else {
            tab.find.jump = false;
            return;
        };
        match tab.doc.text.get(&page) {
            Some(text) => {
                let r = text.rect_to_page(rect);
                let count = tab.doc.page_count;
                let h = tab.doc.size_of(page)[1];
                tab.find.jump = false;
                tab.remember();
                // The hit a third of the way down the view.
                let above = tab.view.viewport[1] / 3.0 / tab.layout().scale.max(0.01);
                tab.view.go_to_point(page, r[1] - above, count, h);
                tab.doc.need_sizes_near(engine, page);
            }
            None => tab.doc.need_text(engine, page),
        }
    }

    /// The sidebar at the left of the pages: thumbnails or the outline.
    fn sidebar(&mut self, ui: &mut Ui, palette: &Palette) {
        let cur = self.current;
        let wid = self.windows[cur].id;
        let which = self.settings.sidebar_tab;
        let frame = self.frame;
        let engine = &self.engine;
        if !self.settings.sidebar_visible {
            return;
        }
        let Some(tab) = self.windows[cur].active_tab_mut() else { return };
        if !tab.doc.ready() {
            return;
        }
        if which == SidebarTab::Outline {
            tab.doc.need_outline(engine);
        }
        let mut actions = Vec::new();
        let response = Panel::left(Id::new(("sidebar", wid)))
            .resizable(true)
            .default_size(self.settings.sidebar_width)
            .size_range(120.0..=800.0)
            .show(ui, |ui| {
                actions = crate::ui::sidebar::show(ui, tab, which, &mut self.textures, frame, palette, &mut self.wanted);
            });
        let width = response.response.rect.width();
        if (width - self.settings.sidebar_width).abs() > 0.5 && !ui.input(|i| i.pointer.any_down()) {
            self.settings.sidebar_width = width;
            self.settings_dirty = true;
        }
        for action in actions {
            match action {
                SidebarAction::GoToPage(page) => {
                    if let Some(tab) = self.windows[cur].active_tab_mut() {
                        let count = tab.doc.page_count;
                        tab.remember();
                        tab.view.go_to_page(page, count);
                        tab.doc.need_sizes_near(&self.engine, page);
                    }
                }
                SidebarAction::Follow(target) => self.follow(target),
                SidebarAction::Tab(which) => {
                    self.settings.sidebar_tab = which;
                    self.settings_dirty = true;
                }
                SidebarAction::InsertBlank { at } => {
                    if let Some(tab) = self.windows[cur].active_tab() {
                        // The size of the page it goes next to.
                        let size = tab.doc.size_of(at.min(tab.doc.page_count.saturating_sub(1)));
                        self.engine.request(Request::Edit { id: tab.doc.id, edit: Edit::InsertBlank { page: at, size } });
                    }
                }
                SidebarAction::InsertPdf { at } => {
                    if let Some(tab) = self.windows[cur].active_tab() {
                        let id = tab.id;
                        self.import_pdf_dialog(ui.ctx(), id, at, false);
                    }
                }
                SidebarAction::ReplacePdf { page } => {
                    if let Some(tab) = self.windows[cur].active_tab() {
                        let id = tab.id;
                        self.import_pdf_dialog(ui.ctx(), id, page, true);
                    }
                }
                SidebarAction::DeletePage { page } => {
                    if let Some(tab) = self.windows[cur].active_tab() {
                        self.engine.request(Request::Edit { id: tab.doc.id, edit: Edit::DeletePage { page } });
                    }
                }
            }
        }
    }

    /// The tab bar: Open, the tabs with their close buttons, the family's
    /// buttons on the right.
    fn tab_bar(&mut self, ui: &mut Ui, palette: &Palette) {
        let mut activate = None;
        let mut close = None;
        let mut open_dialog = false;
        let mut menu_action = None;
        let mut theme_click = false;
        let mut sidebar_click = false;
        let sidebar_visible = self.settings.sidebar_visible;
        let mut about_click = false;
        let mut update_tag = None;
        let cur = self.current;
        let (wid, is_root) = (self.windows[cur].id, self.windows[cur].is_root());
        let newer = self.updates.newer().map(str::to_owned);
        let theme = self.theme;
        Panel::top(Id::new(("tab-bar", wid)))
            .frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(Margin::symmetric(8, 5)))
            .show(ui, |ui| {
                let bar = ui.max_rect();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if crate::ui::icon_button(ui, Icon::Open, false, tr!("Open… (Ctrl+O)", "Открыть… (Ctrl+O)")).clicked() {
                        open_dialog = true;
                    }
                    let sidebar_tip = tr!("Pages and outline (F4)", "Страницы и оглавление (F4)");
                    if crate::ui::icon_button(ui, Icon::Sidebar, sidebar_visible, sidebar_tip).clicked() {
                        sidebar_click = true;
                    }
                    ui.separator();
                    self.windows[cur].tab_lead = ui.cursor().min.x - bar.min.x;
                    let tab_count = self.windows[cur].tabs.len();
                    let active = self.windows[cur].active;
                    let mut rects = Vec::with_capacity(tab_count);
                    let mut drag_start = None;
                    for i in 0..tab_count {
                        let tab = &self.windows[cur].tabs[i];
                        let (tab_id, path_text) = (tab.id, tab.path.display().to_string());
                        let shown = if tab.doc.modified { format!("{} •", tab.title) } else { tab.title.clone() };
                        let title = RichText::new(shown).size(13.5);
                        // Under the tab's own id, so a drag follows the tab
                        // when it is moved along the bar.
                        let response = ui
                            .push_id(tab_id, |ui| ui.add(egui::Button::selectable(i == active, title).sense(Sense::click_and_drag())))
                            .inner
                            .on_hover_text(path_text);
                        rects.push(response.rect);
                        if response.clicked() {
                            activate = Some(i);
                        }
                        if response.middle_clicked() {
                            close = Some(i);
                        }
                        if response.drag_started_by(PointerButton::Primary) {
                            activate = Some(i);
                            // Where the button went down, not where the
                            // pointer is now that egui calls it a drag.
                            let grab = ui.input(|i| i.pointer.press_origin()).map_or(Vec2::ZERO, |p| p - response.rect.min);
                            drag_start = Some((tab_id, grab));
                        }
                        crate::ui::menu::context_menu(&response, |ui| {
                            if tab_count > 1 && ui.button(tr!("Move to a new window", "В новое окно")).clicked() {
                                menu_action = Some(TabMenu::Detach(i));
                                ui.close();
                            }
                            if !is_root && ui.button(tr!("Move to the main window", "В главное окно")).clicked() {
                                menu_action = Some(TabMenu::ToRoot(i));
                                ui.close();
                            }
                            if ui.add(egui::Button::new(tr!("Close", "Закрыть")).shortcut_text("Ctrl+W")).clicked() {
                                menu_action = Some(TabMenu::Close(i));
                                ui.close();
                            }
                            ui.separator();
                            if ui.add_enabled(tab_count > 1, egui::Button::new(tr!("Close others", "Закрыть остальные"))).clicked() {
                                menu_action = Some(TabMenu::CloseOthers(i));
                                ui.close();
                            }
                        });
                        let cross = ui.add(egui::Button::new(RichText::new("×").size(13.0).color(palette.muted)).small().frame(false));
                        if cross.on_hover_text(tr!("Close (Ctrl+W)", "Закрыть (Ctrl+W)")).clicked() {
                            close = Some(i);
                        }
                        ui.add_space(6.0);
                    }
                    if let Some((tab, grab)) = drag_start {
                        self.drag = Some(TabDrag { window: wid, tab, grab, screen_pos: None });
                    }
                    self.drag_along_bar(ui, palette, bar, &rects);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        about_click = crate::ui::about_button(ui).clicked();
                        if let Some(tag) = newer.as_deref()
                            && crate::ui::update_button(ui, tag).clicked()
                        {
                            update_tag = Some(tag.to_owned());
                        }
                        theme_click = crate::ui::theme_button(ui, theme).clicked();
                    });
                });
            });
        if let Some(i) = activate
            && i != self.win().active
        {
            self.win().active = i;
        }
        if let Some(i) = close {
            self.close_tab(i);
        }
        match menu_action {
            Some(TabMenu::Detach(i)) => self.detach_tab(cur, i, None),
            Some(TabMenu::ToRoot(i)) => {
                self.move_tab_to_window(cur, i, 0);
                let root = self.windows[0].viewport;
                ui.ctx().send_viewport_cmd_to(root, ViewportCommand::Focus);
            }
            Some(TabMenu::Close(i)) => self.close_tab(i),
            Some(TabMenu::CloseOthers(i)) => self.close_other_tabs(i),
            None => {}
        }
        if open_dialog {
            self.open_dialog(ui.ctx());
        }
        if about_click {
            self.about.toggle();
        }
        if sidebar_click {
            self.settings.sidebar_visible = !self.settings.sidebar_visible;
            self.settings_dirty = true;
        }
        if let Some(tag) = update_tag {
            crate::win::shell_open(crate::update::release_url(&tag));
        }
        if theme_click {
            self.theme = crate::ui::next_theme(self.theme);
            ui.ctx().set_theme(self.theme);
            self.settings.theme = ThemeSetting::from_preference(self.theme);
            self.settings_dirty = true;
        }
    }

    /// While a tab of this window is dragged: along the bar it changes
    /// places with its neighbours, away from it a ghost of its title
    /// follows the pointer. `rects` are the tabs' places in the bar.
    fn drag_along_bar(&mut self, ui: &mut Ui, palette: &Palette, bar: Rect, rects: &[Rect]) {
        let cur = self.current;
        let Some(drag) = self.drag.as_ref() else { return };
        if drag.window != self.windows[cur].id {
            return;
        }
        let Some(from) = self.windows[cur].tab_index(drag.tab) else { return };
        let Some(pointer) = ui.ctx().pointer_latest_pos() else { return };
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if bar.y_range().contains(pointer.y) {
            // Its place is after every other tab whose middle is left of
            // the pointer.
            let to = rects.iter().enumerate().filter(|(i, r)| *i != from && r.center().x < pointer.x).count();
            self.windows[cur].move_tab(from, to);
            return;
        }
        let title = self.windows[cur].tabs[from].title.clone();
        let grab = drag.grab;
        let painter = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new(("tab-drag", cur))));
        let galley = painter.layout_no_wrap(title, FontId::proportional(13.5), palette.text);
        let rect = Rect::from_min_size(pointer - grab, galley.size() + vec2(16.0, 8.0));
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        painter.rect_stroke(rect, 4.0, Stroke::new(1.0, palette.muted), StrokeKind::Inside);
        painter.galley(rect.min + vec2(8.0, 4.0), galley, palette.text);
    }

    /// Follows a drag of this window's tab until the button is released,
    /// then drops the tab where the pointer is: into another window, or
    /// into a new one when outside every window.
    fn track_drag(&mut self, ctx: &egui::Context) {
        let cur = self.current;
        let Some(drag) = self.drag.as_mut() else { return };
        if drag.window != self.windows[cur].id {
            return;
        }
        // The pointer on the screen: Windows keeps sending its position
        // to the window the button went down in, even outside it.
        let on_screen = ctx.input(|i| {
            let pos = i.pointer.latest_pos()?;
            let inner = i.viewport().inner_rect?;
            Some(inner.min + pos.to_vec2())
        });
        if on_screen.is_some() {
            drag.screen_pos = on_screen;
        }
        if ctx.input(|i| i.pointer.any_down()) {
            ctx.request_repaint();
            return;
        }
        let Some(drag) = self.drag.take() else { return };
        let Some(from) = self.windows[cur].tab_index(drag.tab) else { return };
        let Some(pos) = drag.screen_pos else { return };
        if self.windows[cur].outer.is_some_and(|r| r.contains(pos)) {
            // Dropped in its own window: it was moved along the bar.
            return;
        }
        let target = self.windows.iter().position(|w| w.id != drag.window && w.outer.is_some_and(|r| r.contains(pos)));
        if let Some(to) = target {
            self.move_tab_to_window(cur, from, to);
            ctx.send_viewport_cmd_to(self.windows[to].viewport, ViewportCommand::Focus);
        } else if self.windows[cur].tabs.len() > 1 {
            self.detach_tab(cur, from, Some(pos - drag.grab));
        }
    }

    /// Moves tab `index` of window `from` to the end of window `to`.
    fn move_tab_to_window(&mut self, from: usize, index: usize, to: usize) {
        if from == to || index >= self.windows[from].tabs.len() {
            return;
        }
        let tab = self.windows[from].remove_tab(index);
        self.windows[to].add_tab(tab);
    }

    /// Gives tab `index` of window `from` a window of its own, the size of
    /// the one it leaves. With `tab_pos`, the top left corner of the tab
    /// on the screen, the new window is placed so that the tab lies there;
    /// otherwise it is offset from the window it leaves.
    fn detach_tab(&mut self, from: usize, index: usize, tab_pos: Option<Pos2>) {
        let source = &self.windows[from];
        if index >= source.tabs.len() || source.tabs.len() < 2 {
            return;
        }
        let size = source.inner.map(|r| r.size()).unwrap_or(vec2(980.0, 780.0)).max(crate::MIN_SIZE.into());
        // The window's frame: caption above the content, border left of it.
        let frame = match (source.inner, source.outer) {
            (Some(inner), Some(outer)) => inner.min - outer.min,
            _ => vec2(8.0, 31.0),
        };
        let position = match tab_pos {
            Some(tab) => tab - frame - vec2(source.tab_lead, 5.0),
            None => source.outer.map_or(Pos2::new(100.0, 100.0), |o| o.min + vec2(40.0, 40.0)),
        };
        let id = self.next_window_id;
        self.next_window_id += 1;
        // Made hidden, under a title of its own, until found (`appear`).
        let builder = ViewportBuilder::default()
            .with_visible(false)
            .with_title(hidden_title(id))
            .with_inner_size(size)
            .with_position(position)
            .with_min_inner_size(crate::MIN_SIZE)
            .with_drag_and_drop(true)
            .with_icon(crate::window_icon());
        let mut window = Window::new(id, Some(builder));
        let tab = self.windows[from].remove_tab(index);
        window.add_tab(tab);
        self.windows.push(window);
        self.focused = id;
    }

    /// The root window was told to close while detached windows exist: its
    /// tabs close, and it takes the place and the tabs of the window the
    /// reader used last, which goes away. Child viewports cannot outlive
    /// the root, so this keeps that window open as the reader expects.
    fn adopt_window(&mut self, ctx: &egui::Context, root_maximized: bool) {
        let child = self.window_index(self.focused).filter(|&i| i != 0).unwrap_or(1);
        let closed = std::mem::take(&mut self.windows[0].tabs);
        for tab in closed {
            self.drop_tab(tab);
        }
        let tabs = std::mem::take(&mut self.windows[child].tabs);
        let active = self.windows[child].active;
        let (outer, inner) = (self.windows[child].outer, self.windows[child].inner);
        self.windows[child].close_requested = true;
        let root = &mut self.windows[0];
        root.tabs = tabs;
        root.active = active;
        self.focused = root.id;
        if root_maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        }
        if let Some(outer) = outer {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min));
        }
        if let Some(inner) = inner {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(inner.size()));
        }
    }

    /// Closes the detached windows whose button was pressed or that have
    /// no tab left.
    fn close_windows(&mut self) {
        let mut i = 1;
        while i < self.windows.len() {
            if !self.windows[i].close_requested && !self.windows[i].tabs.is_empty() {
                i += 1;
                continue;
            }
            let window = self.windows.remove(i);
            for tab in window.tabs {
                self.drop_tab(tab);
            }
            if self.focused == window.id {
                self.focused = self.windows[0].id;
            }
            if self.drag.as_ref().is_some_and(|d| d.window == window.id) {
                self.drag = None;
            }
        }
    }

    /// The document's view, the password field or the error, or the start
    /// page when the window has no tab.
    fn central(&mut self, ui: &mut Ui, palette: &Palette) {
        let cur = self.current;
        let frame = self.frame;
        let engine = &self.engine;
        let mut replace_image = None;
        let Some(tab) = self.windows[cur].active_tab_mut() else {
            let action = crate::ui::home::show(ui, &self.settings.recent, self.engine_error.as_deref(), palette, &mut self.start_icon);
            match action {
                Some(HomeAction::Open(path)) => self.open_paths(vec![path], Some(cur)),
                Some(HomeAction::ClearRecent) => {
                    self.settings.recent.clear();
                    self.settings_dirty = true;
                }
                None => {}
            }
            return;
        };
        match tab.doc.status.clone() {
            Status::Ready => {
                let out = crate::ui::view::show(ui, tab, &mut self.textures, frame, palette);
                // Ctrl+wheel over the pages: a zoom step a notch round the pointer.
                if out.zoom_notches != 0.0 {
                    let sizes = tab.doc.sizes.clone();
                    let fallback = tab.doc.fallback;
                    tab.view.zoom_wheel(out.zoom_notches, out.pointer, &sizes, fallback);
                }
                for page in out.visible.clone() {
                    tab.doc.need_text(engine, page);
                    tab.doc.need_links(engine, page);
                    if tab.editing {
                        tab.doc.need_objects(engine, page);
                    }
                }
                if let Some((page, object)) = out.edit_click
                    && let (Some(objects), Some(text)) = (tab.doc.objects.get(&page), tab.doc.text.get(&page))
                    && let Some(o) = objects.text.iter().find(|o| o.index == object)
                {
                    tab.image = None;
                    tab.editor = Some(InlineEditor {
                        page,
                        object,
                        text: o.text.clone(),
                        rect: text.rect_to_page(o.bounds),
                        font_size: o.font_size.max(1.0),
                        fresh: true,
                    });
                }
                if let Some((page, object, text)) = out.edit_commit {
                    tab.editor = None;
                    let unchanged = tab.doc.objects.get(&page).and_then(|os| os.text.iter().find(|o| o.index == object)).is_some_and(|o| o.text == text);
                    if !unchanged && !text.trim().is_empty() {
                        engine.request(Request::Edit { id: tab.doc.id, edit: Edit::Text { page, object, text } });
                    }
                } else if out.edit_cancel {
                    tab.editor = None;
                }
                if let Some((page, object, rect)) = out.image_click {
                    tab.editor = None;
                    tab.image = Some(ImageSel { page, object, rect, drag: None });
                }
                if let Some((page, object, rect)) = out.image_place
                    && let Some(text) = tab.doc.text.get(&page)
                {
                    let bounds = text.rect_to_page_space(rect);
                    engine.request(Request::Edit { id: tab.doc.id, edit: Edit::Place { page, object, bounds } });
                }
                if out.image_delete
                    && let Some(sel) = tab.image.take()
                {
                    engine.request(Request::Edit { id: tab.doc.id, edit: Edit::Remove { page: sel.page, object: sel.object } });
                }
                if out.image_replace
                    && let Some(sel) = tab.image.as_ref()
                {
                    replace_image = Some((tab.id, sel.page, sel.object));
                }
                if let Some(first) = out.visible.clone().next() {
                    tab.doc.need_sizes_near(engine, first);
                }
                if !tab.view.settled() {
                    ui.ctx().request_repaint_after(Duration::from_millis(30));
                }
                self.wanted.extend(out.wanted);
                if let Some(target) = out.link {
                    self.follow(target);
                }
                if let Some((tab, page, object)) = replace_image {
                    self.replace_image_dialog(ui.ctx(), tab, page, object);
                }
            }
            Status::Loading => {
                ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, palette.backdrop);
            }
            Status::NeedsPassword { wrong } => {
                ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, palette.backdrop);
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.label(RichText::new(&tab.title).strong());
                    ui.label(tr!("The document is protected by a password", "Документ защищён паролем"));
                    if wrong {
                        ui.colored_label(palette.error, tr!("Wrong password", "Неверный пароль"));
                    }
                    ui.add_space(8.0);
                    let field = ui.add(egui::TextEdit::singleline(&mut tab.password).password(true).desired_width(220.0));
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (ui.button(tr!("Open", "Открыть")).clicked() || enter) && !tab.password.is_empty() {
                        tab.doc.status = Status::Loading;
                        engine.request(Request::Open {
                            id: tab.doc.id,
                            path: tab.path.clone(),
                            password: Some(tab.password.clone()),
                            first: None,
                        });
                    }
                });
            }
            Status::Failed(e) => {
                ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, palette.backdrop);
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.label(RichText::new(&tab.title).strong());
                    let text: String = match self.engine_error.as_deref() {
                        Some(_) => tr!(
                            "The rendering engine, pdfium.dll, was not found or could not be used.\nIt belongs in the same folder as metalpdf.exe; reinstalling the program or unpacking the archive again restores it.",
                            "Движок рендеринга pdfium.dll не найден или не может быть использован.\nОн должен лежать в одной папке с metalpdf.exe; переустановка программы или повторная распаковка архива вернёт его."
                        )
                        .to_owned(),
                        None => tr!(format!("The file could not be opened: {e}"), format!("Файл не удалось открыть: {e}")),
                    };
                    ui.set_max_width(560.0);
                    ui.colored_label(palette.error, text);
                });
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let cur = self.current;
        let (info, resolve): (StatusInfo, PageResolver) = match self.windows[cur].active_tab() {
            Some(tab) if tab.doc.ready() => {
                let layout = tab.layout();
                let note = match (&tab.notice, tab.doc.modified) {
                    (Some((text, since)), _) if since.elapsed() < Duration::from_secs(6) => text.clone(),
                    (_, true) => tr!("Edited, not saved (Ctrl+S)", "Изменено, не сохранено (Ctrl+S)").to_owned(),
                    _ => match tab.doc.title.as_deref() {
                        Some(t) if !t.is_empty() => t.to_owned(),
                        _ => String::new(),
                    },
                };
                let labels = tab.doc.labels.clone();
                let count = tab.doc.page_count;
                let info = StatusInfo {
                    page: Some((tab.view.current, count)),
                    label: tab.doc.label(tab.view.current),
                    zoom_factor: crate::layout::zoom_of_scale(layout.scale),
                    zoom: tab.view.zoom,
                    mode: tab.view.mode,
                    editing: tab.editing,
                    note,
                    note_is_error: false,
                };
                let resolve = move |text: &str| -> Option<usize> {
                    let text = text.trim();
                    if let Some(i) = labels.iter().position(|l| l.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(text))) {
                        return Some(i);
                    }
                    let n: usize = text.parse().ok()?;
                    (1..=count).contains(&n).then(|| n - 1)
                };
                (info, Box::new(resolve))
            }
            _ => (
                StatusInfo {
                    page: None,
                    label: String::new(),
                    zoom_factor: 1.0,
                    zoom: self.settings.zoom,
                    mode: self.settings.view_mode,
                    editing: false,
                    note: String::new(),
                    note_is_error: false,
                },
                Box::new(|_: &str| None),
            ),
        };
        let actions = crate::ui::status::show(ui, &info, &mut self.windows[cur].page_field, resolve);
        if !actions.is_empty() {
            self.apply_status(actions);
        }
    }

    /// The window's title: the file, then the app.
    fn update_title(&mut self, ctx: &egui::Context) {
        let window = self.win();
        let title = match window.active_tab() {
            Some(tab) => format!("{} — {}", tab.title, crate::APP_NAME),
            None => crate::APP_NAME.to_owned(),
        };
        if title != window.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            window.title = title;
        }
    }

    /// One window's frame: its tab bar, status bar and pages. `ui` is the
    /// root `Ui` of its viewport.
    fn window_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let cur = self.current;
        if !self.windows[cur].is_root() {
            self.appear(&ctx);
        }
        let (outer, inner, focused, close_requested, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.focused.unwrap_or(false), v.close_requested(), v.maximized.unwrap_or(false))
        });
        self.windows[cur].outer = outer;
        self.windows[cur].inner = inner;
        if focused {
            self.focused = self.windows[cur].id;
        }
        if close_requested {
            if cur == 0 {
                if !self.quitting && self.all_tabs().any(|t| t.doc.modified) {
                    ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                    self.confirm = Some(Confirm::Quit);
                } else if self.windows.len() > 1 {
                    ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                    self.adopt_window(&ctx, maximized);
                }
            } else {
                self.windows[cur].close_requested = true;
            }
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if !dropped.is_empty() {
            self.open_paths(dropped, Some(cur));
        }
        self.track_drag(&ctx);
        self.keys(&ctx);
        let palette = Palette::of(&ctx);
        self.tab_bar(ui, &palette);
        self.find_bar(ui, &palette);
        let wid = self.windows[cur].id;
        Panel::bottom(Id::new(("status", wid))).show(ui, |ui| self.status_bar(ui));
        self.sidebar(ui, &palette);
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.central(ui, &palette));
        self.jump_to_hit();
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new(("drop-overlay", wid))));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(110));
            painter.rect_stroke(rect.shrink(12.0), 8.0, Stroke::new(2.0, palette.accent), StrokeKind::Inside);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                tr!("Drop to open", "Отпустите, чтобы открыть"),
                FontId::proportional(26.0),
                egui::Color32::WHITE,
            );
        }
        // A window not found yet keeps the title it is looked for by.
        if !matches!(self.windows[cur].appear, Appear::Looking(_)) {
            self.update_title(&ctx);
        }
    }

    /// `--screenshot`: ask for the window's picture once the pages have
    /// been rendered and uploaded, write it and end.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = crate::SCREENSHOT.get() else { return };
        if self.frame == 60 {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        if self.frame < 120 {
            ctx.request_repaint();
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match crate::bench::write_rgba_png(path, &rgba, w as u32, h as u32) {
                Ok(()) => log::info!("screenshot written to {}", path.display()),
                Err(e) => log::error!("screenshot: {e}"),
            }
            self.quitting = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// Remember the root window's place, a second after it last changed.
    fn track_window(&mut self, ctx: &egui::Context) {
        if self.cloak.is_some() {
            return;
        }
        // Nothing of the context is read inside the `input` closure: a
        // nested read while the engine thread asks for a repaint (a write)
        // deadlocks egui's lock.
        let scale_factor = ctx.pixels_per_point();
        let geometry = ctx.input(|i| {
            let v = i.viewport();
            let maximized = v.maximized.unwrap_or(false);
            let fullscreen = v.fullscreen.unwrap_or(false);
            v.outer_rect.map(|r| WindowGeometry {
                x: r.min.x.round() as i32,
                y: r.min.y.round() as i32,
                width: v.inner_rect.map_or(r.width(), |ir| ir.width()).round() as i32,
                height: v.inner_rect.map_or(r.height(), |ir| ir.height()).round() as i32,
                maximized: maximized || fullscreen,
                scale_factor,
            })
        });
        let Some(mut geometry) = geometry else { return };
        if geometry.maximized {
            // Keep the restored place; only the flag changes.
            if let Some(saved) = self.settings.window {
                geometry = WindowGeometry { maximized: true, scale_factor: geometry.scale_factor, ..saved };
            }
        }
        if self.last_geometry != Some(geometry) {
            self.last_geometry = Some(geometry);
            self.window_changed_at = Some(Instant::now());
        }
        if self.window_changed_at.is_some_and(|t| t.elapsed() >= WINDOW_SAVE_DELAY) {
            self.window_changed_at = None;
            if self.settings.window != Some(geometry) {
                self.settings.window = Some(geometry);
                self.settings_dirty = true;
            }
        } else if self.window_changed_at.is_some() {
            ctx.request_repaint_after(WINDOW_SAVE_DELAY);
        }
    }

    /// Puts the open tabs and where each is read into the settings: the
    /// session the next start without files opens again.
    fn remember_session(&mut self) {
        let mut session = Vec::new();
        let mut active = 0;
        for (w, window) in self.windows.iter().enumerate() {
            for (i, tab) in window.tabs.iter().enumerate() {
                if w == 0 && i == window.active {
                    active = session.len();
                }
                session.push(tab.path.clone());
            }
        }
        let positions: Vec<FilePosition> = self.all_tabs().filter(|t| t.doc.ready()).map(Tab::position).collect();
        for position in positions {
            self.settings.set_position(position);
        }
        self.settings.session = session;
        self.settings.session_active = active;
        self.settings.language = self.about.lang;
        self.settings.check_updates = self.updates.enabled;
        self.settings.last_update_check = self.updates.last_check;
    }

    fn save_settings_if_needed(&mut self) {
        if self.about.lang != self.settings.language
            || self.updates.enabled != self.settings.check_updates
            || self.updates.last_check != self.settings.last_update_check
        {
            self.settings_dirty = true;
        }
        if std::mem::take(&mut self.settings_dirty) {
            self.remember_session();
            self.settings.save_in_background();
        }
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.right_click.hook(&mut raw_input.events);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Files from another launch, even while minimized (no frame then).
        let mut handed = Vec::new();
        while let Ok(paths) = self.inbox.try_recv() {
            handed.extend(paths);
        }
        if !handed.is_empty() {
            let target = self.window_index(self.focused).unwrap_or(0);
            self.open_paths(handed, Some(target));
        }
        if self.raise.swap(false, Ordering::Acquire) {
            self.raise_window(ctx);
        }
    }

    fn ui(&mut self, root_ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.frame += 1;
        crate::textures::delete_dropped();
        self.updates.poll();
        self.uncloak(&ctx);
        self.poll_engine();
        self.upload_pending(frame);
        if let Some(answer) = self.dialogs.take() {
            match answer.ask {
                Ask::Open { window } => {
                    let target = self.window_index(window);
                    self.open_paths(answer.paths, target);
                }
                Ask::SaveAs { tab } => {
                    if let Some(path) = answer.paths.first() {
                        let path = if path.extension().is_none() { path.with_extension("pdf") } else { path.clone() };
                        self.save_to(tab, path);
                    }
                }
                Ask::ImportPdf { tab, page, replace } => {
                    if let Some(path) = answer.paths.first() {
                        for window in &self.windows {
                            if let Some(t) = window.tabs.iter().find(|t| t.id == tab) {
                                let edit = if replace {
                                    Edit::ReplacePdf { page, path: path.clone() }
                                } else {
                                    Edit::InsertPdf { page, path: path.clone() }
                                };
                                self.engine.request(Request::Edit { id: t.doc.id, edit });
                            }
                        }
                    }
                }
                Ask::ReplaceImage { tab, page, object } => {
                    if let Some(path) = answer.paths.first() {
                        for window in &self.windows {
                            if let Some(t) = window.tabs.iter().find(|t| t.id == tab) {
                                self.engine.request(Request::Edit { id: t.doc.id, edit: Edit::ReplaceImage { page, object, path: path.clone() } });
                            }
                        }
                    }
                }
            }
        }
        self.wanted.clear();

        self.current = 0;
        self.window_ui(root_ui);
        // The detached windows, each in a viewport of its own. A window
        // made during this frame (a tab dragged out) is shown at once.
        let mut i = 1;
        while i < self.windows.len() {
            self.current = i;
            let viewport = self.windows[i].viewport;
            let builder = self.windows[i].builder.clone().unwrap_or_default();
            ctx.show_viewport_immediate(viewport, builder, |ui, _class| self.window_ui(ui));
            i += 1;
        }
        self.current = 0;
        self.close_windows();

        self.about.show(&ctx, &mut self.updates);
        self.confirm_dialog(&ctx);
        self.follow_theme(&ctx);
        self.screenshot(&ctx);
        // The renders every window wants, the most urgent first.
        let mut wanted = std::mem::take(&mut self.wanted);
        wanted.sort_by_key(|(_, tier)| *tier);
        wanted.dedup_by_key(|(k, _)| *k);
        self.engine.want(wanted);
        if !self.uploads.is_empty() {
            ctx.request_repaint();
        }
        self.textures.trim(self.frame);
        self.track_window(&ctx);
        self.save_settings_if_needed();
    }

    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        self.remember_session();
        self.settings.save();
        crate::textures::delete_dropped();
    }
}
