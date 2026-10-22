//! MetalPDF: a fast PDF viewer for Windows on PDFium, with tabs.

// The release build is a GUI program without a console; the debug build
// keeps it, so `eprintln!`, logs and panics are seen while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// First, so that `tr!` is in scope in the modules below.
#[macro_use]
mod i18n;

mod app;
mod bench;
mod caption;
mod icon;
mod layout;
mod pdfium;
mod settings;
mod ui;
mod update;
mod win;

use std::sync::Arc;

/// The display name, as people read it: the window title, About, the
/// panic message, the User-Agent of the update check. The identifier
/// (`metalpdf`) is the Cargo package, the exe and the repository.
pub const APP_NAME: &str = "MetalPDF";
/// eframe app id; also names the settings folder in `%APPDATA%` (unless the
/// copy is portable, see `settings::location`).
pub const APP_ID: &str = "MetalPDF";
/// Version from Cargo.toml, shown in About; a development version
/// ("0.1.0-dev") carries the commit it was built from (`build.rs`).
pub const VERSION: &str = env!("METALPDF_VERSION");

fn main() -> eframe::Result {
    env_logger::init();
    install_panic_hook();

    // The command line modes. `args_os`: `args` panics on an argument that
    // is not valid Unicode.
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--export-icon") => {
                // For tools/make_release.py: the installer's icon, then exit.
                // A `.png` name gets the 256-pixel layer (the README's icon).
                win::attach_parent_console();
                let out = args.next().unwrap_or_else(|| "app.ico".into());
                let result = if std::path::Path::new(&out).extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
                    bench::write_rgba_png(std::path::Path::new(&out), embedded_icon(256), 256, 256)
                } else {
                    std::fs::write(&out, icon::ico(&icon::ICO_SIZES)).map_err(|e| e.to_string())
                };
                if let Err(e) = result {
                    eprintln!("cannot write {}: {e}", out.to_string_lossy());
                    std::process::exit(1);
                }
                return Ok(());
            }
            Some("-V" | "--version") => {
                win::attach_parent_console();
                println!("{APP_NAME} {VERSION}");
                return Ok(());
            }
            Some("-h" | "--help") => {
                win::attach_parent_console();
                println!("{APP_NAME} {VERSION}");
                println!("metalpdf [--version] [--help] [--export-icon FILE] [--bench FILE.pdf] [FILE.pdf ...]");
                return Ok(());
            }
            Some("--bench") => {
                // The engine's timings for one file, without a window.
                win::attach_parent_console();
                let Some(file) = args.next() else {
                    eprintln!("--bench needs a PDF file");
                    std::process::exit(2);
                };
                std::process::exit(bench::run(std::path::Path::new(&file)));
            }
            _ => {}
        }
    }

    let stored = settings::location();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1000.0, 700.0])
            .with_min_inner_size([480.0, 320.0])
            .with_icon(window_icon()),
        renderer: eframe::Renderer::Glow,
        // Centre only on the first run; later runs restore the saved window.
        centered: !stored.has_saved(),
        ..Default::default()
    };
    eframe::run_native(APP_ID, options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
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
/// is not a panic and still ends the process silently. disk_flashlight also
/// appends the report to `crash.log` next to the settings file.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let text = tr!(
            format!("{APP_NAME} stopped because of an internal error.\n\n{info}"),
            format!("{APP_NAME} остановлен из-за внутренней ошибки.\n\n{info}")
        );
        win::error_box(APP_NAME, &text);
    }));
}
