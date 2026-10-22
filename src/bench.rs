//! `--bench <file.pdf>`: the engine's timings for one document, printed to
//! the console, without a window. The measurements the viewer's priorities
//! rest on: how long the DLL takes to load, the document to open, the
//! first page to render at screen size and as a preview, its text to be
//! laid out. The first page's pixels go to a PNG in the temporary folder
//! for a look. The first cold run of a machine is slower (the DLL comes
//! from disk): run it twice.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use crate::pdfium::{self, Bitmap, Placement};

/// Width in pixels the first page is rendered at: a letter page at a 2×
/// display (roughly 1700 × 2200 pixels).
const SCREEN_WIDTH: u32 = 1700;
const PREVIEW_WIDTH: u32 = 200;

pub fn run(path: &Path) -> i32 {
    let mut out = std::io::stdout().lock();
    let mut mark = |what: &str, since: Instant| {
        let _ = writeln!(out, "{:>8.2} ms  {what}", since.elapsed().as_secs_f64() * 1e3);
    };
    let total = Instant::now();
    let t = Instant::now();
    let api = match pdfium::init() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("cannot load PDFium: {e}");
            return 1;
        }
    };
    mark("pdfium.dll loaded and initialised", t);

    let t = Instant::now();
    let doc = match pdfium::Document::open(api, path, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("cannot open {}: {e}", path.display());
            return 1;
        }
    };
    let pages = doc.page_count();
    mark(&format!("document opened: {pages} pages"), t);

    let t = Instant::now();
    let Some(first) = doc.page_size(0) else {
        eprintln!("the first page has no size");
        return 1;
    };
    mark(&format!("size of page 1: {:.0} x {:.0} pt", first[0], first[1]), t);

    let t = Instant::now();
    let Some(page) = doc.load_page(0) else {
        eprintln!("cannot load page 1");
        return 1;
    };
    mark("page 1 loaded", t);

    let scale = SCREEN_WIDTH as f32 / first[0];
    let height = (first[1] * scale).round() as u32;
    let t = Instant::now();
    let Some(mut bitmap) = Bitmap::new(api, SCREEN_WIDTH, height) else {
        eprintln!("cannot make a {SCREEN_WIDTH} x {height} bitmap");
        return 1;
    };
    let at = Placement { start_x: 0, start_y: 0, size_x: SCREEN_WIDTH as i32, size_y: height as i32, rotate: 0 };
    page.render(&mut bitmap, at, pdfium::sys::FPDF_ANNOT);
    mark(&format!("page 1 rendered at {SCREEN_WIDTH} x {height}"), t);

    let t = Instant::now();
    page.render(&mut bitmap, at, pdfium::sys::FPDF_ANNOT);
    mark("page 1 rendered again (decoded images cached)", t);

    let t = Instant::now();
    let ph = (first[1] * PREVIEW_WIDTH as f32 / first[0]).round().max(1.0) as u32;
    if let Some(mut preview) = Bitmap::new(api, PREVIEW_WIDTH, ph) {
        let at = Placement { start_x: 0, start_y: 0, size_x: PREVIEW_WIDTH as i32, size_y: ph as i32, rotate: 0 };
        page.render(&mut preview, at, pdfium::sys::FPDF_ANNOT);
    }
    mark(&format!("page 1 preview at {PREVIEW_WIDTH} x {ph}"), t);

    let t = Instant::now();
    let layout = page.text_layout();
    mark(&format!("text layout of page 1: {} characters", layout.chars.len()), t);

    let t = Instant::now();
    let links = page.links();
    mark(&format!("links of page 1: {}", links.len()), t);

    let t = Instant::now();
    let known = (0..pages).filter(|&i| doc.page_size(i).is_some()).count();
    mark(&format!("sizes of all {known} pages"), t);

    let t = Instant::now();
    let outline = doc.outline();
    mark(&format!("outline: {} top-level entries", outline.len()), t);

    if pages > 1 {
        let t = Instant::now();
        if let Some(last) = doc.load_page(pages - 1) {
            let [w, h] = last.size();
            let lh = (h * SCREEN_WIDTH as f32 / w).round().max(1.0) as u32;
            if let Some(mut b) = Bitmap::new(api, SCREEN_WIDTH, lh) {
                let at = Placement { start_x: 0, start_y: 0, size_x: SCREEN_WIDTH as i32, size_y: lh as i32, rotate: 0 };
                last.render(&mut b, at, pdfium::sys::FPDF_ANNOT);
            }
        }
        mark(&format!("last page loaded and rendered at {SCREEN_WIDTH} px"), t);
    }
    mark("total", total);

    // Into the temporary folder, not beside the user's file.
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let png_path = std::env::temp_dir().join(format!("metalpdf_bench_{stem}_page1.png"));
    match write_png(&png_path, bitmap.pixels(), SCREEN_WIDTH, height) {
        Ok(()) => {
            let _ = writeln!(out, "page 1 written to {}", png_path.display());
        }
        Err(e) => {
            let _ = writeln!(out, "cannot write {}: {e}", png_path.display());
        }
    }
    0
}

/// BGRx pixels as an RGB PNG.
fn write_png(path: &Path, bgrx: &[u8], width: u32, height: u32) -> Result<(), String> {
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for p in bgrx.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    encode_png(path, &rgb, width, height, png::ColorType::Rgb)
}

/// Straight RGBA pixels as a PNG (`--export-icon file.png`).
pub fn write_rgba_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    encode_png(path, rgba, width, height, png::ColorType::Rgba)
}

fn encode_png(path: &Path, data: &[u8], width: u32, height: u32, color: png::ColorType) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(data).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}
