//! Printing on Windows: the printers, their settings (a DEVMODE, which the
//! driver's own dialog edits), the sheet's geometry, where a page goes on
//! it, the page ranges typed in the dialog, and the print job's device
//! context. PDFium draws into that context on the engine thread
//! (`engine::Worker::print_page`); the dialog is `ui/print.rs`.
//!
//! The calls that ask a printer's driver (the list, a DEVMODE, the papers,
//! the sheet) can take a while for a network printer, so the dialog makes
//! them on a thread of its own ([`query`]).

use std::ffi::c_void;
use std::path::Path;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, CreateICW, DEVMODEW, DIB_RGB_COLORS, DM_DUPLEX, DM_IN_BUFFER, DM_IN_PROMPT,
    DM_ORIENTATION, DM_OUT_BUFFER, DM_PAPERSIZE, DMDUP_HORIZONTAL, DMDUP_SIMPLEX, DMDUP_VERTICAL, DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT,
    DeleteDC, GetDeviceCaps, HALFTONE, HDC, HORZRES, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY,
    PHYSICALWIDTH, SRCCOPY, SetStretchBltMode, StretchDIBits, VERTRES,
};
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
    PRINTER_HANDLE, PRINTER_INFO_4W,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DC_DUPLEX, DC_PAPERNAMES, DC_PAPERS, DOCINFOW, DeviceCapabilitiesW, EndDoc, EndPage, StartDocW, StartPage};

use crate::settings::PrintScale;
use crate::win::wide;

/// A printer's settings as its driver keeps them: a DEVMODEW followed by
/// the driver's own bytes (held in `u64`s, so the structure is aligned).
#[derive(Clone)]
pub struct DevMode(Vec<u64>);

impl DevMode {
    fn with_size(bytes: usize) -> DevMode {
        DevMode(vec![0; bytes.div_ceil(8).max(std::mem::size_of::<DEVMODEW>().div_ceil(8))])
    }

    pub fn as_ptr(&self) -> *const DEVMODEW {
        self.0.as_ptr().cast()
    }

    fn as_mut_ptr(&mut self) -> *mut DEVMODEW {
        self.0.as_mut_ptr().cast()
    }

    fn fields(&self) -> &DEVMODEW {
        // SAFETY: the buffer is at least a DEVMODEW, aligned, and written
        // by the driver.
        unsafe { &*self.as_ptr() }
    }

    fn fields_mut(&mut self) -> &mut DEVMODEW {
        unsafe { &mut *self.as_mut_ptr() }
    }

    pub fn landscape(&self) -> bool {
        let dm = self.fields();
        dm.dmFields & DM_ORIENTATION != 0 && unsafe { dm.Anonymous1.Anonymous1.dmOrientation } == DMORIENT_LANDSCAPE as i16
    }

    pub fn set_landscape(&mut self, landscape: bool) {
        let duplex = self.duplex();
        let dm = self.fields_mut();
        dm.dmFields |= DM_ORIENTATION;
        dm.Anonymous1.Anonymous1.dmOrientation = if landscape { DMORIENT_LANDSCAPE } else { DMORIENT_PORTRAIT } as i16;
        // Two-sided turns on the long edge, which the orientation names.
        if duplex {
            self.set_duplex(true);
        }
    }

    /// The paper, a `DMPAPER_*` number.
    pub fn paper(&self) -> i16 {
        let dm = self.fields();
        if dm.dmFields & DM_PAPERSIZE != 0 { unsafe { dm.Anonymous1.Anonymous1.dmPaperSize } } else { 0 }
    }

    pub fn set_paper(&mut self, paper: i16) {
        let dm = self.fields_mut();
        dm.dmFields |= DM_PAPERSIZE;
        dm.Anonymous1.Anonymous1.dmPaperSize = paper;
    }

    /// Printed on both sides.
    pub fn duplex(&self) -> bool {
        let dm = self.fields();
        dm.dmFields & DM_DUPLEX != 0 && dm.dmDuplex != DMDUP_SIMPLEX && dm.dmDuplex != 0
    }

    /// Both sides, turned on the long edge (as a book), or one side.
    pub fn set_duplex(&mut self, on: bool) {
        let landscape = self.landscape();
        let dm = self.fields_mut();
        dm.dmFields |= DM_DUPLEX;
        dm.dmDuplex = match (on, landscape) {
            (false, _) => DMDUP_SIMPLEX,
            (true, false) => DMDUP_VERTICAL,
            (true, true) => DMDUP_HORIZONTAL,
        };
    }
}

/// A printer opened for its driver's settings.
struct Printer(PRINTER_HANDLE);

impl Printer {
    fn open(name: &str) -> Option<Printer> {
        let name = wide(name);
        let mut handle: PRINTER_HANDLE = unsafe { std::mem::zeroed() };
        (unsafe { OpenPrinterW(name.as_ptr(), &mut handle, std::ptr::null()) } != 0).then_some(Printer(handle))
    }

    /// The DEVMODE's size this driver wants.
    fn devmode_size(&self, name: &[u16]) -> Option<usize> {
        let size = unsafe { DocumentPropertiesW(std::ptr::null_mut(), self.0, name.as_ptr(), std::ptr::null_mut(), std::ptr::null(), 0) };
        (size > 0).then_some(size as usize)
    }
}

impl Drop for Printer {
    fn drop(&mut self) {
        unsafe { ClosePrinter(self.0) };
    }
}

/// The printers Windows knows, local and connected, and the default one.
pub fn printers() -> (Vec<String>, Option<String>) {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut needed, mut count) = (0u32, 0u32);
    unsafe { EnumPrintersW(flags, std::ptr::null(), 4, std::ptr::null_mut(), 0, &mut needed, &mut count) };
    let mut names = Vec::new();
    if needed > 0 {
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if unsafe { EnumPrintersW(flags, std::ptr::null(), 4, buf.as_mut_ptr().cast(), needed, &mut needed, &mut count) } != 0 {
            let infos = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<PRINTER_INFO_4W>(), count as usize) };
            names = infos.iter().filter_map(|i| from_wide_ptr(i.pPrinterName)).collect();
        }
    }
    let mut len = 0u32;
    unsafe { GetDefaultPrinterW(std::ptr::null_mut(), &mut len) };
    let default = (len > 0)
        .then(|| {
            let mut buf = vec![0u16; len as usize];
            (unsafe { GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) } != 0).then(|| from_wide(&buf))
        })
        .flatten();
    (names, default)
}

/// The printer's settings as its driver sets them by default.
pub fn default_devmode(printer: &str) -> Option<DevMode> {
    let handle = Printer::open(printer)?;
    let name = wide(printer);
    let mut devmode = DevMode::with_size(handle.devmode_size(&name)?);
    let ok = unsafe { DocumentPropertiesW(std::ptr::null_mut(), handle.0, name.as_ptr(), devmode.as_mut_ptr(), std::ptr::null(), DM_OUT_BUFFER) };
    (ok > 0).then_some(devmode)
}

/// The driver's own dialog over `devmode`, owned by window `owner`: the
/// settings chosen, or `None` when it was cancelled. Modal: it is called
/// on a thread of its own.
pub fn properties(owner: isize, printer: &str, devmode: &DevMode) -> Option<DevMode> {
    let handle = Printer::open(printer)?;
    let name = wide(printer);
    let mut out = DevMode::with_size(handle.devmode_size(&name)?);
    const IDOK: i32 = 1;
    let answer = unsafe { DocumentPropertiesW(owner as _, handle.0, name.as_ptr(), out.as_mut_ptr(), devmode.as_ptr(), DM_IN_BUFFER | DM_IN_PROMPT | DM_OUT_BUFFER) };
    (answer == IDOK).then_some(out)
}

/// The papers the printer takes: `DMPAPER_*` numbers and names.
pub fn papers(printer: &str, devmode: &DevMode) -> Vec<(i16, String)> {
    let name = wide(printer);
    let count = unsafe { DeviceCapabilitiesW(name.as_ptr(), std::ptr::null(), DC_PAPERS, std::ptr::null_mut(), devmode.as_ptr()) };
    if count <= 0 {
        return Vec::new();
    }
    let count = count as usize;
    let mut ids = vec![0u16; count];
    let mut names = vec![0u16; count * 64];
    unsafe {
        DeviceCapabilitiesW(name.as_ptr(), std::ptr::null(), DC_PAPERS, ids.as_mut_ptr(), devmode.as_ptr());
        DeviceCapabilitiesW(name.as_ptr(), std::ptr::null(), DC_PAPERNAMES, names.as_mut_ptr(), devmode.as_ptr());
    }
    ids.iter().zip(names.chunks(64)).map(|(&id, n)| (id as i16, from_wide(n))).filter(|(_, n)| !n.is_empty()).collect()
}

/// Whether the printer prints on both sides.
pub fn duplex_capable(printer: &str, devmode: &DevMode) -> bool {
    let name = wide(printer);
    unsafe { DeviceCapabilitiesW(name.as_ptr(), std::ptr::null(), DC_DUPLEX, std::ptr::null_mut(), devmode.as_ptr()) == 1 }
}

/// A sheet as the printer prints it, in its pixels: the resolution, the
/// paper's size and the part it can print on (from the paper's corner).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sheet {
    pub dpi: [f32; 2],
    pub size: [f32; 2],
    /// x, y, width, height.
    pub printable: [f32; 4],
}

impl Sheet {
    fn of(dc: HDC) -> Option<Sheet> {
        let cap = |index| unsafe { GetDeviceCaps(dc, index as i32) } as f32;
        let sheet = Sheet {
            dpi: [cap(LOGPIXELSX), cap(LOGPIXELSY)],
            size: [cap(PHYSICALWIDTH), cap(PHYSICALHEIGHT)],
            printable: [cap(PHYSICALOFFSETX), cap(PHYSICALOFFSETY), cap(HORZRES), cap(VERTRES)],
        };
        (sheet.dpi[0] > 0.0 && sheet.dpi[1] > 0.0 && sheet.printable[2] > 0.0 && sheet.printable[3] > 0.0).then_some(sheet)
    }

    /// Asked of the printer with `devmode`, without printing.
    pub fn query(printer: &str, devmode: Option<&DevMode>) -> Option<Sheet> {
        let name = wide(printer);
        let dc = unsafe { CreateICW(std::ptr::null(), name.as_ptr(), std::ptr::null(), devmode.map_or(std::ptr::null(), DevMode::as_ptr)) };
        if dc.is_null() {
            return None;
        }
        let sheet = Sheet::of(dc);
        unsafe { DeleteDC(dc) };
        sheet
    }
}

/// Where a page goes on a sheet: a quarter turn (clockwise) or none, and
/// its box as printed, turned, in the printer's pixels from the corner of
/// the printable area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub rotate: i32,
    /// x, y, width, height.
    pub rect: [f32; 4],
}

/// Page `page` (points, /Rotate applied) on `sheet`: turned when
/// `auto_rotate` and its shape is the other way round from the printable
/// area's, sized by `scale`, centred on the printable area.
pub fn place(page: [f32; 2], sheet: &Sheet, scale: PrintScale, auto_rotate: bool) -> Placed {
    let [_, _, pw, ph] = sheet.printable;
    let page = [page[0].max(1.0), page[1].max(1.0)];
    let turn = auto_rotate && page[0] != page[1] && (page[0] > page[1]) != (pw > ph);
    let (w, h) = if turn { (page[1], page[0]) } else { (page[0], page[1]) };
    let natural = [w * sheet.dpi[0] / 72.0, h * sheet.dpi[1] / 72.0];
    let fit = (pw / natural[0]).min(ph / natural[1]);
    let k = match scale {
        PrintScale::Shrink => fit.min(1.0),
        PrintScale::Fit => fit,
        PrintScale::Actual => 1.0,
    };
    let (w, h) = (natural[0] * k, natural[1] * k);
    Placed { rotate: if turn { 1 } else { 0 }, rect: [(pw - w) / 2.0, (ph - h) / 2.0, w, h] }
}

/// The pages `text` names, in the order typed: numbers or labels as the
/// page field reads them (`resolve`), ranges with a dash ("1-5", "8-" up
/// to the last page, "-3" from the first, "5-3" backwards), separated by
/// commas or semicolons; `None` when a part names no page.
pub fn parse_ranges(text: &str, count: usize, resolve: impl Fn(&str) -> Option<usize>) -> Option<Vec<usize>> {
    let mut pages = Vec::new();
    for part in text.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        // A label may have a dash of its own: the whole part first.
        if let Some(page) = resolve(part) {
            pages.push(page);
            continue;
        }
        let (from, to) = part.split_once(['-', '–', '—'])?;
        let (from, to) = (from.trim(), to.trim());
        let from = if from.is_empty() { 0 } else { resolve(from)? };
        let to = if to.is_empty() { count.checked_sub(1)? } else { resolve(to)? };
        if from <= to {
            pages.extend(from..=to);
        } else {
            pages.extend((to..=from).rev());
        }
    }
    (!pages.is_empty()).then_some(pages)
}

/// The sheets to print: `pages` `copies` times, the whole run again for
/// each copy when `collate`, else each page its copies in a row.
pub fn sheets(pages: &[usize], copies: usize, collate: bool) -> Vec<usize> {
    let copies = copies.max(1);
    if collate {
        (0..copies).flat_map(|_| pages.iter().copied()).collect()
    } else {
        pages.iter().flat_map(|&p| std::iter::repeat_n(p, copies)).collect()
    }
}

/// A print job: the printer's device context with a document started on
/// it. Dropped before `finish`, the document is abandoned (`AbortDoc`).
pub struct Job {
    dc: HDC,
    sheet: Sheet,
    open: bool,
}

// The context is used from the engine thread only, after being made there.
unsafe impl Send for Job {}

impl Job {
    /// Start a document titled `title` on `printer`; with `output`, the
    /// printer's output goes to that file (a test prints to PDF so).
    pub fn start(printer: &str, devmode: Option<&DevMode>, title: &str, output: Option<&Path>) -> Result<Job, String> {
        let name = wide(printer);
        let dc = unsafe { CreateDCW(std::ptr::null(), name.as_ptr(), std::ptr::null(), devmode.map_or(std::ptr::null(), DevMode::as_ptr)) };
        if dc.is_null() {
            return Err(format!("the printer {printer} could not be opened"));
        }
        let Some(sheet) = Sheet::of(dc) else {
            unsafe { DeleteDC(dc) };
            return Err(format!("the printer {printer} gave no sheet size"));
        };
        let title = wide(title);
        let output = output.map(wide);
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: title.as_ptr(),
            lpszOutput: output.as_ref().map_or(std::ptr::null(), |o| o.as_ptr()),
            lpszDatatype: std::ptr::null(),
            fwType: 0,
        };
        if unsafe { StartDocW(dc, &info) } <= 0 {
            unsafe { DeleteDC(dc) };
            return Err(format!("the printer {printer} did not start the document"));
        }
        Ok(Job { dc, sheet, open: true })
    }

    pub fn sheet(&self) -> &Sheet {
        &self.sheet
    }

    /// The context, for PDFium's `FPDF_RenderPage`.
    pub fn dc(&self) -> *mut c_void {
        self.dc
    }

    pub fn start_page(&self) -> Result<(), String> {
        if unsafe { StartPage(self.dc) } <= 0 { Err("the printer did not start a sheet".into()) } else { Ok(()) }
    }

    pub fn end_page(&self) -> Result<(), String> {
        if unsafe { EndPage(self.dc) } <= 0 { Err("the printer did not take the sheet".into()) } else { Ok(()) }
    }

    /// A picture (BGRx rows, top first) stretched over `rect` (x, y,
    /// width, height in the printer's pixels from the printable area's
    /// corner).
    pub fn draw_bgrx(&self, pixels: &[u8], width: u32, height: u32, rect: [i32; 4]) -> Result<(), String> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative: the rows top first.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = unsafe {
            SetStretchBltMode(self.dc, HALFTONE);
            StretchDIBits(self.dc, rect[0], rect[1], rect[2], rect[3], 0, 0, width as i32, height as i32, pixels.as_ptr().cast(), &info, DIB_RGB_COLORS, SRCCOPY)
        };
        if lines == 0 { Err("the printer did not take the picture".into()) } else { Ok(()) }
    }

    /// The document ended: the spooler prints it.
    pub fn finish(mut self) -> Result<(), String> {
        self.open = false;
        if unsafe { EndDoc(self.dc) } <= 0 { Err("the printer did not finish the document".into()) } else { Ok(()) }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            if self.open {
                AbortDoc(self.dc);
            }
            DeleteDC(self.dc);
        }
    }
}

/// What the dialog learns of a printer, asked on a thread of its own.
#[derive(Clone)]
pub struct PrinterState {
    pub name: String,
    pub devmode: Option<DevMode>,
    pub papers: Vec<(i16, String)>,
    pub duplex: bool,
    pub sheet: Option<Sheet>,
}

/// `printer` with `devmode` (its default settings when `None`): its
/// papers, whether it prints on both sides, its sheet.
pub fn query(printer: &str, devmode: Option<DevMode>) -> PrinterState {
    let devmode = devmode.or_else(|| default_devmode(printer));
    let (papers, duplex) = match &devmode {
        Some(dm) => (papers(printer, dm), duplex_capable(printer, dm)),
        None => (Vec::new(), false),
    };
    let sheet = Sheet::query(printer, devmode.as_ref());
    PrinterState { name: printer.to_owned(), devmode, papers, duplex, sheet }
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn from_wide_ptr(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A4 at 600 dpi with a quarter inch the printer cannot reach.
    fn a4() -> Sheet {
        Sheet { dpi: [600.0, 600.0], size: [4960.0, 7016.0], printable: [150.0, 150.0, 4660.0, 6716.0] }
    }

    #[test]
    fn a_small_page_prints_at_its_size_centred() {
        // A5 on A4.
        let p = place([420.0, 595.0], &a4(), PrintScale::Shrink, true);
        assert_eq!(p.rotate, 0);
        assert!((p.rect[2] - 420.0 * 600.0 / 72.0).abs() < 0.01);
        assert!((p.rect[0] - (4660.0 - p.rect[2]) / 2.0).abs() < 0.01);
        // Fitted, it fills the width.
        let p = place([420.0, 595.0], &a4(), PrintScale::Fit, true);
        assert!((p.rect[2] - 4660.0).abs() < 0.5 || (p.rect[3] - 6716.0).abs() < 0.5);
    }

    #[test]
    fn a_large_page_is_shrunk_unless_actual_size() {
        // A3 on A4.
        let p = place([842.0, 1191.0], &a4(), PrintScale::Shrink, true);
        assert!(p.rect[2] <= 4660.0 + 0.01 && p.rect[3] <= 6716.0 + 0.01);
        let p = place([842.0, 1191.0], &a4(), PrintScale::Actual, true);
        assert!(p.rect[0] < 0.0, "cut off on both sides, centred");
    }

    #[test]
    fn a_landscape_page_is_turned_on_a_portrait_sheet() {
        let p = place([842.0, 595.0], &a4(), PrintScale::Shrink, true);
        assert_eq!(p.rotate, 1);
        assert!(p.rect[3] > p.rect[2], "the turned box is tall");
        assert_eq!(place([842.0, 595.0], &a4(), PrintScale::Shrink, false).rotate, 0);
        assert_eq!(place([500.0, 500.0], &a4(), PrintScale::Shrink, true).rotate, 0);
    }

    #[test]
    fn ranges_are_read_as_typed() {
        let resolve = |t: &str| t.parse::<usize>().ok().filter(|n| (1..=10).contains(n)).map(|n| n - 1);
        assert_eq!(parse_ranges("1-3, 5", 10, resolve), Some(vec![0, 1, 2, 4]));
        assert_eq!(parse_ranges("8-", 10, resolve), Some(vec![7, 8, 9]));
        assert_eq!(parse_ranges("-2; 4–3", 10, resolve), Some(vec![0, 1, 3, 2]));
        assert_eq!(parse_ranges("1, 11", 10, resolve), None);
        assert_eq!(parse_ranges(" , ", 10, resolve), None);
        // A label with a dash of its own.
        let labelled = |t: &str| if t == "A-1" { Some(0) } else { resolve(t) };
        assert_eq!(parse_ranges("A-1, 2", 10, labelled), Some(vec![0, 1]));
    }

    #[test]
    fn copies_are_collated_or_kept_together() {
        assert_eq!(sheets(&[0, 1], 2, true), [0, 1, 0, 1]);
        assert_eq!(sheets(&[0, 1], 2, false), [0, 0, 1, 1]);
        assert_eq!(sheets(&[3], 0, true), [3]);
    }
}
