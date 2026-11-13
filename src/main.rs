//! MetalPDF: a fast PDF viewer for Windows on PDFium, with tabs.

// The release build is a GUI program without a console; the debug build
// keeps it, so `eprintln!`, logs and panics are seen while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// First, so that `tr!` is in scope in the modules below.
/// A line on standard output, for the command line modes. A reader that
/// went away (`| Select-Object -First 1`, `| head`) is no error: `println!`
/// panicked on the closed pipe, and the panic hook showed a message box.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// The same on standard error.
macro_rules! esay {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

#[macro_use]
mod i18n;

mod app;
mod assoc;
mod bench;
mod cache;
mod caption;
mod dialog;
mod document;
mod engine;
mod export;
mod fonts;
mod icon;
mod layout;
mod pdfium;
mod pixels;
mod placement;
mod print;
mod search;
mod selection;
mod settings;
mod single_instance;
mod style;
mod textline;
mod textures;
mod ui;
mod update;
mod view;
mod win;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// The display name, as people read it: the window title, About, the
/// panic message, the User-Agent of the update check. The identifier
/// (`metalpdf`) is the Cargo package, the exe and the repository.
pub const APP_NAME: &str = "MetalPDF";
/// eframe app id.
pub const APP_ID: &str = "MetalPDF";
/// Version from Cargo.toml, shown in About; a development version
/// ("0.1.0-dev") carries the commit it was built from (`build.rs`).
pub const VERSION: &str = env!("METALPDF_VERSION");
/// The window's size in points on the first run.
pub const DEFAULT_SIZE: [f32; 2] = [1100.0, 800.0];
pub const MIN_SIZE: [f32; 2] = [480.0, 320.0];

/// The window was saved maximized: `App::new` maximizes it while it is
/// still hidden (made maximized, winit shows it white before its first
/// frame).
pub static MAXIMIZE_WHEN_SHOWN: AtomicBool = AtomicBool::new(false);

/// `--screenshot FILE.png`: the window's picture is written there after a
/// few frames and the program ends (the README's screenshot; on an NVIDIA
/// machine the OpenGL area cannot be captured from outside).
pub static SCREENSHOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Time since `main` began, for the trace marks.
pub fn since_start() -> std::time::Duration {
    START.get().map_or(std::time::Duration::ZERO, |s| s.elapsed())
}

fn main() -> eframe::Result {
    let _ = START.set(Instant::now());
    init_logging();
    install_panic_hook();

    // The command line modes. `args_os`: `args` panics on an argument that
    // is not valid Unicode.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut new_instance = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--export-icon") => {
                // For tools/make_release.py: the installer's icon, then exit.
                // A `.png` name gets the 256-pixel layer (the README's icon).
                win::attach_parent_console();
                let out = args.next().unwrap_or_else(|| "app.ico".into());
                let result = if Path::new(&out).extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
                    bench::write_rgba_png(Path::new(&out), embedded_icon(256), 256, 256)
                } else {
                    std::fs::write(&out, icon::ico(&icon::ICO_SIZES)).map_err(|e| e.to_string())
                };
                if let Err(e) = result {
                    esay!("cannot write {}: {e}", out.to_string_lossy());
                    std::process::exit(1);
                }
                return Ok(());
            }
            Some("-V" | "--version") => {
                win::attach_parent_console();
                say!("{APP_NAME} {VERSION}");
                return Ok(());
            }
            Some("-h" | "--help") => {
                win::attach_parent_console();
                say!("{APP_NAME} {VERSION}");
                say!("metalpdf [FILE.pdf ...]");
                say!("  --register | --unregister   offer MetalPDF for .pdf files in Windows (per user) / withdraw it");
                say!("  --new-instance              a window of its own instead of a tab in the running one");
                say!("  --bench FILE.pdf            the engine's timings for one file, no window");
                say!("  --export FILE.pdf OUT.docx|OUT.pptx   save as a Word document or a PowerPoint presentation, no window");
                say!("  --export-paragraphs FILE.pdf OUT.docx   save as ordinary Word paragraphs, no window");
                say!("  --export-icon FILE.ico|png  the app icon");
                say!("  --screenshot FILE.png       the window's picture after a few frames, then exit");
                say!("  --version, --help");
                return Ok(());
            }
            Some("--bench") => {
                // The engine's timings for one file, without a window.
                win::attach_parent_console();
                let Some(file) = args.next() else {
                    esay!("--bench needs a PDF file");
                    std::process::exit(2);
                };
                std::process::exit(bench::run(Path::new(&file)));
            }
            Some(flag @ ("--export" | "--export-paragraphs")) => {
                // Saved as a Word document or a PowerPoint presentation,
                // without a window.
                win::attach_parent_console();
                let (Some(input), Some(output)) = (args.next(), args.next()) else {
                    esay!("{flag} needs a PDF file and an output file (.docx for paragraphs; .docx or .pptx for --export)");
                    std::process::exit(2);
                };
                let started = std::time::Instant::now();
                let result = if flag == "--export-paragraphs" {
                    export::run_paragraphs(Path::new(&input), Path::new(&output), None)
                } else {
                    export::run(Path::new(&input), Path::new(&output), None)
                };
                match result {
                    Ok(pages) => {
                        say!("{pages} pages written to {} in {:.2} s", output.to_string_lossy(), started.elapsed().as_secs_f64());
                        return Ok(());
                    }
                    Err(e) => {
                        esay!("cannot export: {e}");
                        std::process::exit(1);
                    }
                }
            }
            Some("--register" | "--unregister") => {
                let register = arg.to_str() == Some("--register");
                // The ProgID's names in the chosen language.
                i18n::set_lang(settings::Settings::load().language.resolve());
                let result = match std::env::current_exe() {
                    Ok(exe) if register => assoc::register(&exe),
                    Ok(_) => assoc::unregister(),
                    Err(e) => Err(e.to_string()),
                };
                if let Err(e) = result {
                    win::error_box(APP_NAME, &e);
                    std::process::exit(1);
                }
                return Ok(());
            }
            Some("--new-instance") => new_instance = true,
            Some("--screenshot") => {
                let Some(file) = args.next() else {
                    win::attach_parent_console();
                    esay!("--screenshot needs a PNG file name");
                    std::process::exit(2);
                };
                let _ = SCREENSHOT.set(PathBuf::from(file));
                new_instance = true;
            }
            Some(flag) if flag.starts_with("--") => log::warn!("unknown option {flag}"),
            _ => files.push(normalize(Path::new(&arg))),
        }
    }
    // Each file once (`a b a` as well as `a a`).
    let mut once: Vec<PathBuf> = Vec::new();
    for file in files {
        if !once.iter().any(|f| settings::same_path(f, &file)) {
            once.push(file);
        }
    }
    let files = once;

    // A running copy takes the files (a few milliseconds instead of a
    // second window); without one, this copy becomes the one that listens.
    if !new_instance && single_instance::hand_off(&files) {
        log::debug!("handed over to the running instance");
        return Ok(());
    }
    let mut server = if new_instance { None } else { single_instance::Server::bind() };
    if !new_instance && server.is_none() {
        // Another copy bound the pipe between the hand-off and the bind
        // (Explorer starts one process per file selected): it takes the
        // files, once it listens.
        // For a second at most (a running copy whose pipe stays busy).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if single_instance::hand_off(&files) {
                log::debug!("handed over to the copy that bound first");
                return Ok(());
            }
            server = single_instance::Server::bind();
            if server.is_some() {
                break;
            }
        }
    }

    let settings = settings::Settings::load();
    // The language before anything is shown (About may change it later).
    i18n::set_lang(settings.language.resolve());
    let stored = settings::location();

    // Without files, the last session's tabs (those still there).
    let (tabs, active, restored) = if files.is_empty() {
        // A network share that is away blocks `is_file` for its timeout,
        // before the window: a path on the network is taken on trust (a
        // file gone makes a tab with the error). `Path::starts_with(r"\\")`
        // compares components and never matched a UNC path.
        let session: Vec<PathBuf> = settings.session.iter().filter(|p| win::is_remote(p) || p.is_file()).cloned().collect();
        let active = settings.session_active.min(session.len().saturating_sub(1));
        (session, active, true)
    } else {
        (files, 0, false)
    };

    // The engine starts now, opening and rendering the shown tab's file
    // while the window is made (~150 ms): the first frame finds the page
    // ready.
    let first_id: engine::DocId = 1;
    let initial = tabs.get(active).map(|path| {
        let position = settings.position(path);
        let first = engine::FirstRender {
            viewport: predicted_viewport(&settings),
            pixels_per_point: settings.window.map_or_else(system_scale, |w| w.scale_factor),
            zoom: position.map_or(settings.zoom, |p| p.zoom),
            turns: position.map_or(0, |p| p.rotation % 4),
            two_pages: settings.two_pages,
            page: position.map_or(0, |p| p.page),
        };
        (first_id, path.clone(), first)
    });
    let engine = engine::Engine::start(initial);
    log::debug!("engine started at {:.0} ms", since_start().as_secs_f64() * 1e3);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_drag_and_drop(true)
        .with_icon(window_icon());
    if let Some(w) = settings.window {
        viewport = viewport
            .with_position(w.corner_points())
            .with_inner_size([w.width.max(MIN_SIZE[0] as i32) as f32, w.height.max(MIN_SIZE[1] as i32) as f32]);
        if w.maximized {
            MAXIMIZE_WHEN_SHOWN.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        // Centre only on the first run; later runs restore the saved window.
        centered: !stored.has_saved() || settings.window.is_none(),
        ..Default::default()
    };
    let startup = app::Startup { tabs, active, restored, settings, engine, first_id, server };
    eframe::run_native(APP_ID, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, startup)))))
}

/// The page area's probable size in points, from the saved window: the
/// window less the toolbar, the status bar and the scroll bar.
fn predicted_viewport(settings: &settings::Settings) -> [f32; 2] {
    let (w, h) = settings.window.map_or((DEFAULT_SIZE[0], DEFAULT_SIZE[1]), |g| (g.width as f32, g.height as f32));
    // The scroll bar floats over the pages, so it takes no width.
    let sidebar = if settings.sidebar_visible { settings.sidebar_width } else { 0.0 };
    [(w - sidebar).max(100.0), (h - 34.0 - 30.0).max(100.0)]
}

/// The primary display's scale factor, for a first run. The process is
/// made DPI-aware first (winit does the same later, harmlessly): an
/// unaware process is told 96 dpi whatever the display.
fn system_scale() -> f32 {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetProcessDpiAwarenessContext(value: isize) -> i32;
        fn GetDpiForSystem() -> u32;
    }
    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
    let dpi = unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        GetDpiForSystem()
    };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// An absolute path as given; Explorer passes a drive root as `C:"`
/// (the closing quote escaped by the backslash), mended here.
pub fn normalize(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let path = match text.strip_suffix('"') {
        Some(root) => PathBuf::from(format!("{root}\\")),
        None => path.to_path_buf(),
    };
    std::path::absolute(&path).unwrap_or(path)
}

/// `env_logger` at `warn`, or this crate at `debug` with
/// `METALPDF_TRACE=1` (start-up and render timings).
fn init_logging() {
    let mut builder = env_logger::Builder::from_env("METALPDF_LOG");
    builder.filter_level(log::LevelFilter::Warn);
    if std::env::var_os("METALPDF_TRACE").is_some_and(|v| v != "0") {
        builder.filter_module("metalpdf", log::LevelFilter::Debug);
    }
    builder.format_timestamp_millis().init();
}

/// The icon of the window, from the embedded 64-pixel rendering.
pub fn window_icon() -> Arc<egui::IconData> {
    Arc::new(egui::IconData { rgba: embedded_icon(64).to_vec(), width: 64, height: 64 })
}

/// The app icon rasterised at build time (`build.rs`) as straight RGBA,
/// `size` x `size` pixels for 64, 128 and 256, so that nothing is drawn at
/// start-up (drawing it took 6 ms before the window could be created).
pub fn embedded_icon(size: u32) -> &'static [u8] {
    match size {
        64 => include_bytes!(concat!(env!("OUT_DIR"), "/app_icon_64.rgba")),
        128 => include_bytes!(concat!(env!("OUT_DIR"), "/app_icon_128.rgba")),
        _ => include_bytes!(concat!(env!("OUT_DIR"), "/app_icon_256.rgba")),
    }
}

/// The release build aborts on a panic and has no console, so the window
/// would vanish without a word: say what happened in a message box. The
/// default hook still prints it (seen in a debug build). A stack overflow
/// is not a panic and still ends the process silently. A command line mode
/// (`--export` in a script) prints it alone: a message box waited for a
/// click no one would give.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if win::command_line() {
            esay!("{APP_NAME} stopped because of an internal error: {info}");
            return;
        }
        let text = tr!(
            format!("{APP_NAME} stopped because of an internal error.\n\n{info}"),
            format!("{APP_NAME} остановлен из-за внутренней ошибки.\n\n{info}")
        );
        win::error_box(APP_NAME, &text);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_drive_roots_are_mended() {
        assert_eq!(normalize(Path::new("C:\"")), PathBuf::from("C:\\"));
        assert!(normalize(Path::new("a.pdf")).is_absolute());
    }
}
