//! The egui window: the document shown, the engine's answers taken in
//! every frame, the keys, the toolbar and the status bar, the start page
//! with the recent files, and the pieces every app of the family has
//! (theme switch, About, update check, dark caption, settings).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers, StrokeKind, ThemePreference};

use crate::cache::Textures;
use crate::dialog::{Ask, Dialogs};
use crate::document::{DocState, Status};
use crate::engine::{DocId, Engine, Event, Kind, RenderKey, Request};
use crate::pdfium::Target;
use crate::pixels::Pixels;
use crate::settings::{FilePosition, Settings, ThemeSetting, ViewMode, WindowGeometry, ZoomSetting};
use crate::style::Palette;
use crate::textures::Texture;
use crate::ui::about::AboutDialog;
use crate::ui::home::HomeAction;
use crate::ui::status::{PageField, StatusAction, StatusInfo};
use crate::update::Updates;
use crate::view::ViewState;

/// What `main` hands the app: the files to open, the settings it read, the
/// engine already opening the first file.
pub struct Startup {
    pub files: Vec<PathBuf>,
    pub settings: Settings,
    pub engine: Engine,
    /// The document id the engine opened the first file under.
    pub first_id: DocId,
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
}

impl Tab {
    fn new(id: u64, path: PathBuf, doc: DocState, view: ViewState) -> Tab {
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        Tab { id, path, title, doc, view, password: String::new() }
    }

    pub fn layout(&self) -> crate::layout::Layout {
        self.view.layout(&self.doc.sizes, self.doc.fallback)
    }

    fn position(&self) -> FilePosition {
        self.view.position(&self.path)
    }
}

pub struct App {
    settings: Settings,
    settings_dirty: bool,
    engine: Engine,
    /// PDFium could not be loaded: the reason.
    engine_error: Option<String>,
    textures: Textures,
    /// Renders arrived, not yet uploaded: one full render a frame.
    uploads: VecDeque<(RenderKey, Pixels)>,
    tab: Option<Tab>,
    next_doc: DocId,
    next_tab: u64,
    theme: ThemePreference,
    about: AboutDialog,
    updates: Updates,
    dialogs: Dialogs,
    page_field: PageField,
    /// The window's handle, for the caption; `None` off Windows.
    hwnd: Option<isize>,
    /// The theme the caption was last painted in.
    shown_theme: Option<egui::Theme>,
    /// The caption must be repainted in the next frame (the theme changed).
    refresh_caption: bool,
    /// The app icon of the start screen, as a texture.
    start_icon: Option<egui::TextureHandle>,
    /// The window is cloaked until it has painted a few frames: shown
    /// before its first frame reached DWM, it flashed white. The time it is
    /// shown anyway and the frame it was cloaked in.
    cloak: Option<(Instant, u64)>,
    frame: u64,
    title: String,
    /// The first page has been drawn (a trace mark).
    first_drawn: bool,
    /// The window moved or resized: its place is saved after a pause.
    window_changed_at: Option<Instant>,
    last_geometry: Option<WindowGeometry>,
}

/// What the status bar's page field resolves a typed label or number with.
type PageResolver = Box<dyn Fn(&str) -> Option<usize>>;

/// Frames the window paints, cloaked, before it is shown, and the longest
/// it stays cloaked.
const PAINTED_TO_SHOW: u64 = 3;
const UNCLOAK_TIMEOUT: Duration = Duration::from_secs(1);
/// The window's place is saved this long after it last moved.
const WINDOW_SAVE_DELAY: Duration = Duration::from_secs(1);

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        let Startup { files, settings, engine, first_id } = startup;
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
            if crate::MAXIMIZE_WHEN_SHOWN.swap(false, std::sync::atomic::Ordering::Relaxed) {
                crate::placement::show_maximized(hwnd);
            }
            cloak = Some((Instant::now() + UNCLOAK_TIMEOUT, cc.egui_ctx.cumulative_frame_nr()));
            match settings.theme.preference() {
                ThemePreference::Dark => crate::caption::set_dark(hwnd, true),
                ThemePreference::Light => crate::caption::set_dark(hwnd, false),
                ThemePreference::System => {}
            }
        }
        let mut updates = Updates::new(settings.check_updates, settings.last_update_check);
        updates.start_if_due(&cc.egui_ctx);
        let mut about = AboutDialog::default();
        about.lang = settings.language;
        let mut app = Self {
            theme: settings.theme.preference(),
            settings,
            settings_dirty: false,
            engine,
            engine_error: None,
            textures: Textures::default(),
            uploads: VecDeque::new(),
            tab: None,
            next_doc: first_id + 1,
            next_tab: 1,
            about,
            updates,
            dialogs: Dialogs::default(),
            page_field: PageField::default(),
            hwnd,
            shown_theme: None,
            refresh_caption: false,
            start_icon: None,
            cloak,
            frame: 0,
            title: String::new(),
            first_drawn: false,
            window_changed_at: None,
            last_geometry: None,
        };
        // The first file: the engine is already opening it under `first_id`.
        if let Some(path) = files.first() {
            let mut doc = DocState::new(first_id);
            doc.status = Status::Loading;
            let view = app.view_for(path);
            app.tab = Some(Tab::new(0, path.clone(), doc, view));
            app.settings.add_recent(path);
            app.settings_dirty = true;
        }
        app
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

    /// Open `path` in place of the current document.
    pub fn open_path(&mut self, path: &Path) {
        if self.engine_error.is_some() {
            return;
        }
        let path = crate::normalize(path);
        if self.tab.as_ref().is_some_and(|t| crate::settings::same_path(&t.path, &path)) {
            return;
        }
        self.close_tab();
        let id = self.next_doc;
        self.next_doc += 1;
        let tab_id = self.next_tab;
        self.next_tab += 1;
        self.engine.request(Request::Open { id, path: path.clone(), password: None, first: None });
        let view = self.view_for(&path);
        self.tab = Some(Tab::new(tab_id, path.clone(), DocState::new(id), view));
        self.settings.add_recent(&path);
        self.settings_dirty = true;
    }

    /// Close the document: its position kept, its textures and the
    /// engine's handles freed.
    fn close_tab(&mut self) {
        let Some(tab) = self.tab.take() else { return };
        if tab.doc.ready() {
            self.settings.set_position(tab.position());
            self.settings_dirty = true;
        }
        self.textures.remove_doc(tab.doc.id);
        self.uploads.retain(|(k, _)| k.doc != tab.doc.id);
        self.engine.request(Request::Close(tab.doc.id));
    }

    /// Take the engine's answers.
    fn poll_engine(&mut self) {
        while let Some(event) = self.engine.poll() {
            match event {
                Event::Unavailable(reason) => {
                    self.engine_error = Some(reason);
                    if let Some(tab) = &mut self.tab {
                        tab.doc.status = Status::Failed(crate::pdfium::OpenError::File("no engine".into()));
                    }
                }
                Event::Rendered { key, pixels, took_ms } => {
                    if self.tab.as_ref().is_some_and(|t| t.doc.id == key.doc) {
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
                    if let Some(tab) = &mut self.tab
                        && tab.doc.take(&other, &self.engine)
                        && matches!(other, Event::Opened { .. })
                        && tab.doc.ready()
                    {
                        // The position restored before the sizes are known
                        // still names the page; the sizes round it first.
                        let page = tab.view.anchor.page;
                        tab.doc.need_sizes_near(&self.engine, page);
                        log::debug!("{} opened: {} pages", tab.title, tab.doc.page_count);
                    }
                }
            }
        }
    }

    /// Upload the renders that arrived: every preview (small), one full
    /// render a frame (a page takes a few milliseconds of the UI thread),
    /// the visible ones first.
    fn upload_pending(&mut self, frame: &mut eframe::Frame) {
        let Some(gl) = frame.gl().cloned() else { return };
        let visible = self.tab.as_ref().map(|t| t.view.current).unwrap_or(0) as u32;
        let mut full_done = false;
        let mut rest = VecDeque::new();
        // The full render nearest the current page goes first.
        self.uploads.make_contiguous().sort_by_key(|(k, _)| (k.kind == Kind::Full, k.page.abs_diff(visible)));
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

    /// Show the window once it has painted `PAINTED_TO_SHOW` frames.
    fn uncloak(&mut self, ctx: &egui::Context) {
        let Some((until, frame)) = self.cloak else { return };
        if ctx.cumulative_frame_nr() >= frame + PAINTED_TO_SHOW || Instant::now() >= until {
            if let Some(hwnd) = self.hwnd {
                crate::caption::cloak(hwnd, false);
            }
            self.cloak = None;
            log::debug!("window shown at {:.0} ms", crate::since_start().as_secs_f64() * 1e3);
        } else {
            ctx.request_repaint();
        }
    }

    /// Bring the window caption in line with the theme. egui passes a new
    /// theme to the window at the end of the frame it changed in, and
    /// Windows applies it to the caption only on the next repaint of the
    /// frame, so the frame after the change asks for one.
    fn follow_theme(&mut self, ctx: &egui::Context) {
        let theme = ctx.theme();
        if std::mem::take(&mut self.refresh_caption)
            && let Some(hwnd) = self.hwnd
        {
            crate::caption::set_dark(hwnd, theme == egui::Theme::Dark);
            crate::caption::refresh(hwnd);
        }
        if self.shown_theme.is_some_and(|t| t != theme) {
            self.refresh_caption = true;
            ctx.request_repaint();
        }
        self.shown_theme = Some(theme);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if crate::ui::icon_button(ui, crate::ui::icons::Icon::Open, false, tr!("Open… (Ctrl+O)", "Открыть… (Ctrl+O)")).clicked() {
                self.open_dialog(ui.ctx());
            }
            if let Some(tab) = &self.tab {
                ui.separator();
                let label = ui.add(egui::Label::new(egui::RichText::new(&tab.title).strong()).truncate());
                label.on_hover_text(tab.path.display().to_string());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(tag) = self.updates.newer().map(str::to_owned)
                    && crate::ui::update_button(ui, &tag).clicked()
                {
                    crate::win::shell_open(crate::update::release_url(&tag));
                }
                if crate::ui::about_button(ui).clicked() {
                    self.about.toggle();
                }
                if crate::ui::theme_button(ui, self.theme).clicked() {
                    self.theme = crate::ui::next_theme(self.theme);
                    ui.ctx().set_theme(self.theme);
                    self.settings.theme = ThemeSetting::from_preference(self.theme);
                    self.settings_dirty = true;
                }
            });
        });
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        let folder = self.tab.as_ref().and_then(|t| t.path.parent().map(Path::to_path_buf));
        self.dialogs.start(
            ctx,
            self.hwnd,
            Ask::Open { window: 0 },
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

    /// The keys, when no text field has the focus.
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
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
        }
        if consume(Modifiers::COMMAND, Key::G) {
            self.page_field.focus = true;
        }
        if typing {
            return;
        }
        let Some(tab) = &mut self.tab else {
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
        let mut changed = false;
        if consume(Modifiers::COMMAND, Key::W) {
            self.close_tab();
            return;
        }
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
            changed = true;
        }
        if consume(Modifiers::COMMAND, Key::Num1) {
            view.set_zoom(ZoomSetting::Scale(1.0), None, &sizes, fallback);
            changed = true;
        }
        if consume(Modifiers::COMMAND, Key::Num2) {
            view.set_zoom(ZoomSetting::FitPage, None, &sizes, fallback);
            changed = true;
        }
        if consume(Modifiers::COMMAND, Key::Plus) || consume(Modifiers::COMMAND, Key::Equals) || consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::Plus) {
            view.zoom_step(true, None, &sizes, fallback);
            changed = true;
        }
        if consume(Modifiers::COMMAND, Key::Minus) {
            view.zoom_step(false, None, &sizes, fallback);
            changed = true;
        }
        if consume(Modifiers::NONE, Key::R) {
            view.rotate(true);
            changed = true;
        }
        if consume(Modifiers::SHIFT, Key::R) {
            view.rotate(false);
            changed = true;
        }
        if changed {
            ctx.request_repaint();
        }
    }

    /// Apply what the status bar asked.
    fn apply_status(&mut self, actions: Vec<StatusAction>) {
        let Some(tab) = &mut self.tab else { return };
        let count = tab.doc.page_count;
        let sizes = tab.doc.sizes.clone();
        let fallback = tab.doc.fallback;
        for action in actions {
            match action {
                StatusAction::GoToPage(page) => {
                    tab.view.go_to_page(page, count);
                    tab.doc.need_sizes_near(&self.engine, page);
                }
                StatusAction::ZoomIn => tab.view.zoom_step(true, None, &sizes, fallback),
                StatusAction::ZoomOut => tab.view.zoom_step(false, None, &sizes, fallback),
                StatusAction::Zoom(z) => tab.view.set_zoom(z, None, &sizes, fallback),
                StatusAction::Rotate { clockwise } => tab.view.rotate(clockwise),
                StatusAction::Mode(mode) => {
                    tab.view.set_mode(mode);
                    self.settings.view_mode = mode;
                    self.settings_dirty = true;
                }
            }
        }
    }

    /// Follow a link's target.
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
                if let Some(tab) = &mut self.tab {
                    let count = tab.doc.page_count;
                    match (y, tab.doc.text.get(&index)) {
                        // The point on the page, through the page's transform.
                        (Some(y), Some(text)) => {
                            let p = text.to_page_points(0.0, y);
                            let h = crate::layout::turned(tab.doc.size_of(index), 0)[1];
                            tab.view.go_to_point(index, p[1], count, h);
                        }
                        _ => tab.view.go_to_page(index, count),
                    }
                    tab.doc.need_sizes_near(&self.engine, index);
                }
            }
        }
    }

    /// The document's view, the password field or the error.
    fn central(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let Some(tab) = &mut self.tab else {
            let action = crate::ui::home::show(ui, &self.settings.recent, self.engine_error.as_deref(), palette, &mut self.start_icon);
            match action {
                Some(HomeAction::Open(path)) => self.open_path(&path),
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
                let out = crate::ui::view::show(ui, tab, &mut self.textures, self.frame, palette);
                // Ctrl+wheel over the pages: a zoom step round the pointer.
                let zoom = ui.input(|i| i.zoom_delta());
                if out.hovered && (zoom - 1.0).abs() > 0.01 {
                    let sizes = tab.doc.sizes.clone();
                    let fallback = tab.doc.fallback;
                    tab.view.zoom_step(zoom > 1.0, out.pointer, &sizes, fallback);
                }
                for page in out.visible.clone() {
                    tab.doc.need_text(&self.engine, page);
                    tab.doc.need_links(&self.engine, page);
                }
                if let Some(first) = out.visible.clone().next() {
                    tab.doc.need_sizes_near(&self.engine, first);
                }
                self.engine.want(out.wanted);
                if !tab.view.settled() {
                    ui.ctx().request_repaint_after(Duration::from_millis(30));
                }
                if let Some(target) = out.link {
                    self.follow(target);
                }
            }
            Status::Loading => {
                ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, palette.backdrop);
            }
            Status::NeedsPassword { wrong } => {
                ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, palette.backdrop);
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.label(egui::RichText::new(&tab.title).strong());
                    ui.label(tr!("The document is protected by a password", "Документ защищён паролем"));
                    if wrong {
                        ui.colored_label(palette.error, tr!("Wrong password", "Неверный пароль"));
                    }
                    ui.add_space(8.0);
                    let field = ui.add(egui::TextEdit::singleline(&mut tab.password).password(true).desired_width(220.0));
                    if self.frame < 3 || field.lost_focus() && !ui.input(|i| i.key_pressed(Key::Enter)) {
                        field.request_focus();
                    }
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (ui.button(tr!("Open", "Открыть")).clicked() || enter) && !tab.password.is_empty() {
                        tab.doc.status = Status::Loading;
                        self.engine.request(Request::Open {
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
                    ui.label(egui::RichText::new(&tab.title).strong());
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

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let (info, resolve): (StatusInfo, PageResolver) = match &self.tab {
            Some(tab) if tab.doc.ready() => {
                let layout = tab.layout();
                let note = match tab.doc.title.as_deref() {
                    Some(t) if !t.is_empty() => t.to_owned(),
                    _ => String::new(),
                };
                let labels = tab.doc.labels.clone();
                let count = tab.doc.page_count;
                let info = StatusInfo {
                    page: Some((tab.view.current, count)),
                    label: tab.doc.label(tab.view.current),
                    zoom_factor: crate::layout::zoom_of_scale(layout.scale),
                    zoom: tab.view.zoom,
                    mode: tab.view.mode,
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
                    note: String::new(),
                    note_is_error: false,
                },
                Box::new(|_: &str| None),
            ),
        };
        let actions = crate::ui::status::show(ui, &info, &mut self.page_field, resolve);
        if !actions.is_empty() {
            self.apply_status(actions);
        }
    }

    /// Files dropped on the window, and the overlay while they hover.
    fn drops(&mut self, ctx: &egui::Context, palette: &Palette) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(path) = dropped.into_iter().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) || p.is_file()) {
            self.open_path(&path);
        }
        if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(110));
            painter.rect_stroke(rect.shrink(12.0), 8.0, egui::Stroke::new(2.0, palette.accent), StrokeKind::Inside);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                tr!("Drop to open", "Отпустите, чтобы открыть"),
                egui::FontId::proportional(26.0),
                egui::Color32::WHITE,
            );
        }
    }

    /// The window's title: the file, then the app.
    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match &self.tab {
            Some(tab) => format!("{} — {}", tab.title, crate::APP_NAME),
            None => crate::APP_NAME.to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Remember the window's place, a second after it last changed.
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

    fn save_settings_if_needed(&mut self) {
        if !self.settings_dirty {
            return;
        }
        self.settings_dirty = false;
        self.settings.language = self.about.lang;
        self.settings.check_updates = self.updates.enabled;
        self.settings.last_update_check = self.updates.last_check;
        if let Some(tab) = &self.tab
            && tab.doc.ready()
        {
            self.settings.set_position(tab.position());
        }
        self.settings.save_in_background();
    }
}

impl eframe::App for App {
    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.frame += 1;
        crate::textures::delete_dropped();
        self.updates.poll();
        self.uncloak(&ctx);
        self.follow_theme(&ctx);
        self.poll_engine();
        self.upload_pending(frame);
        if let Some(answer) = self.dialogs.take() {
            match answer.ask {
                Ask::Open { .. } => {
                    if let Some(path) = answer.paths.first() {
                        self.open_path(path);
                    }
                }
                Ask::SaveAs { .. } => {}
            }
        }
        self.keys(&ctx);
        let palette = Palette::of(&ctx);
        egui::Panel::top("toolbar").show(root_ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(root_ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root_ui, |ui| self.central(ui, &palette));
        self.drops(&ctx, &palette);
        self.about.show(&ctx, &mut self.updates);
        if self.about.lang != self.settings.language || self.updates.enabled != self.settings.check_updates || self.updates.last_check != self.settings.last_update_check {
            self.settings_dirty = true;
        }
        if !self.uploads.is_empty() {
            ctx.request_repaint();
        }
        self.textures.trim(self.frame);
        self.update_title(&ctx);
        self.track_window(&ctx);
        self.save_settings_if_needed();
    }

    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        if let Some(tab) = &self.tab
            && tab.doc.ready()
        {
            self.settings.set_position(tab.position());
        }
        self.settings.language = self.about.lang;
        self.settings.check_updates = self.updates.enabled;
        self.settings.last_update_check = self.updates.last_check;
        self.settings.save();
        crate::textures::delete_dropped();
    }
}
