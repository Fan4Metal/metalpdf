//! Saving a PDF as a Word document (`.docx`) or a PowerPoint presentation
//! (`.pptx`), both made by the program itself: no Office is needed.
//!
//! A PDF has no paragraphs, only characters at places. `pdfium::extract`
//! reads every character of horizontal text with its box, font, size and
//! colour, and the graphics with their boxes; `layout` gathers the
//! characters into lines, orders them by columns (a recursive XY cut) and
//! makes paragraphs of them, with their alignment, indents and spacing,
//! and finds the figures (pictures and drawings, with the labels inside
//! them). The two formats use this differently:
//!
//! - **Word** gets editable lines in page-anchored text frames, with their
//!   original positions and word advances. The artwork is rendered without
//!   that text, behind the frames. Page sizes, orientations and breaks are
//!   preserved; font substitution cannot reflow text onto another page.
//!   The optional paragraph mode (`docx_flow`) writes ordinary paragraphs,
//!   columns and tables; Word then controls their line and page breaks.
//! - **PowerPoint** gets one slide a page, laid out as the page: a text
//!   box a paragraph where the paragraph was, the pictures as pictures,
//!   and what is neither (the drawings, rules, shadings, text at an
//!   angle) as one picture of the page behind them, so the slide looks
//!   as the page did.
//!
//! The job runs on the engine thread a page per turn of its loop, as
//! printing does (`engine::Worker::export_page`), on a copy of the
//! document of its own: its pages are flattened (form fields made
//! content) and have their exported text hidden for the artwork picture.

pub mod docx;
mod docx_flow;
pub mod layout;
pub mod pptx;
mod xml;
pub mod zip;

use std::path::{Path, PathBuf};

use crate::pdfium::extract::FontTable;
use crate::pdfium::{Document, sys::Api};

/// What a PDF is saved as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Docx,
    /// Ordinary Word paragraphs, columns and tables, which reflow when edited.
    DocxFlow,
    Pptx,
}

impl Format {
    /// The format a file name asks for, by its extension.
    pub fn of(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "docx" => Some(Format::Docx),
            "pptx" => Some(Format::Pptx),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Docx | Format::DocxFlow => "docx",
            Format::Pptx => "pptx",
        }
    }

    /// A typed extension changes the file format, but not the chosen Word
    /// layout mode: both Word modes use .docx.
    pub fn for_path(self, path: &Path) -> Option<Format> {
        Self::of(path).map(|format| match (self, format) {
            (Format::DocxFlow, Format::Docx) => Format::DocxFlow,
            (_, format) => format,
        })
    }
}

/// A character as the export reads it, in view space (points from the
/// top left corner of the page as shown, y down).
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    /// Index in PDFium's text page; -1 in geometry-only fixtures.
    pub source: i32,
    /// U+00AD for a hyphen PDFium found at a line's end.
    pub ch: char,
    /// left, top, right, bottom: the loose box (the font's ascent and
    /// descent, the advance).
    pub rect: [f32; 4],
    /// The origin: where the advance starts, on the baseline.
    pub x: f32,
    pub baseline: f32,
    /// The font size as drawn, in points.
    pub size: f32,
    /// The font, an index into the document's `FontTable`.
    pub font: u16,
    pub color: [u8; 3],
    /// A space (in the text or worked out by PDFium) came before it.
    pub space_before: bool,
    /// Drawn invisibly: the text layer a recognition put over a scan.
    pub invisible: bool,
}

/// A font of the document as the text page names it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontInfo {
    /// The base font name, a subset's tag included.
    pub name: String,
    /// The descriptor flags.
    pub flags: i32,
    /// The descriptor's weight; -1 when unknown.
    pub weight: i32,
    /// Filled and stroked: a regular font made to look bold.
    pub faux_bold: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GraphicKind {
    Image,
    /// `curved`: curves or slanted lines, a drawing; else rules and
    /// boxes.
    Path { curved: bool },
    Shading,
}

/// A graphic object of the page (form XObjects looked into).
#[derive(Clone, Debug, PartialEq)]
pub struct Graphic {
    /// left, top, right, bottom in view space.
    pub rect: [f32; 4],
    pub kind: GraphicKind,
    /// A picture's index among the page's top-level objects, which can be
    /// read alone and taken out of the page.
    pub object: Option<usize>,
}

/// What the export reads off a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageContent {
    /// Width and height as shown, in points.
    pub size: [f32; 2],
    pub glyphs: Vec<Glyph>,
    pub graphics: Vec<Graphic>,
    /// Quarter turns clockwise of the page as shown that the content is
    /// read in (most of its text sideways): `size` is the turned page's.
    pub turns: u8,
}

/// A font as Word and PowerPoint name it: an installed family, or the
/// generic one in that style.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// Pictures are kept at most this fine (PowerPoint's page picture, a
/// figure, a picture of the PDF's scaled down to it).
pub const PICTURE_DPI: f32 = 200.0;
/// The page picture under PowerPoint's text.
pub const BACKGROUND_DPI: f32 = 150.0;

/// Encode RGBA (`alpha`) or RGB pixels as PNG.
pub fn encode_png(width: u32, height: u32, pixels: &[u8], alpha: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(pixels).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// RGB pixels of a part of a BGRx bitmap `width` pixels wide: columns
/// `x0..x1`, rows `y0..y1`.
pub fn crop_bgrx(bgrx: &[u8], width: u32, [x0, y0, x1, y1]: [u32; 4]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut rgb = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize * 3);
    for y in y0..y1 {
        let row = &bgrx[y as usize * stride..(y as usize + 1) * stride];
        for x in x0..x1 {
            let p = &row[x as usize * 4..x as usize * 4 + 4];
            rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    rgb
}

/// Whether every pixel of a BGRx bitmap is white or nearly.
pub fn all_white(bgrx: &[u8]) -> bool {
    bgrx.as_chunks::<4>().0.iter().all(|p| p[0] >= 250 && p[1] >= 250 && p[2] >= 250)
}

/// RGBA pixels scaled down by whole box averages so that neither side
/// passes `max_w` × `max_h` (the size kept as it is when it fits).
pub fn shrink_rgba(width: u32, height: u32, rgba: Vec<u8>, max_w: u32, max_h: u32) -> (u32, u32, Vec<u8>) {
    let k = (width as f32 / max_w.max(1) as f32).max(height as f32 / max_h.max(1) as f32);
    if k <= 1.0 {
        return (width, height, rgba);
    }
    let (nw, nh) = (((width as f32 / k).round() as u32).max(1), ((height as f32 / k).round() as u32).max(1));
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    for ny in 0..nh {
        let (y0, y1) = (ny * height / nh, ((ny + 1) * height / nh).max(ny * height / nh + 1));
        for nx in 0..nw {
            let (x0, x1) = (nx * width / nw, ((nx + 1) * width / nw).max(nx * width / nw + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1.min(height) {
                for x in x0..x1.min(width) {
                    let i = (y as usize * width as usize + x as usize) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[i + c]);
                    }
                }
            }
            let n = ((y1.min(height) - y0) * (x1.min(width) - x0)).max(1);
            out.extend(sum.iter().map(|&s| (s / n) as u8));
        }
    }
    (nw, nh, out)
}

enum Writer {
    Docx(docx::Writer),
    DocxFlow(docx_flow::Writer),
    Pptx(pptx::Writer),
}

/// An export under way: the document's copy, the file being written and
/// how far it got.
pub struct Job {
    doc: Document,
    api: &'static Api,
    /// The file asked for; written under a temporary name beside it until
    /// the last page.
    path: PathBuf,
    temp: PathBuf,
    writer: Option<Writer>,
    fonts: FontTable,
    pub next: usize,
    pub count: usize,
}

impl Job {
    /// An export of `doc` (a copy opened for it) to `path` in `format`.
    pub fn start(api: &'static Api, doc: Document, path: &Path, format: Format) -> Result<Job, String> {
        let count = doc.page_count();
        if count == 0 {
            return Err("the document has no pages".into());
        }
        let temp = path.with_extension(format!("{}.metalpdf-tmp", format.extension()));
        let file = std::fs::File::create(&temp).map_err(|e| format!("cannot create {}: {e}", temp.display()))?;
        let out = zip::ZipWriter::new(std::io::BufWriter::new(file));
        let title = doc.meta_text("Title").filter(|t| !t.trim().is_empty());
        let author = doc.meta_text("Author").filter(|t| !t.trim().is_empty());
        let meta = Meta { title, author };
        let writer = match format {
            Format::Docx => docx::Writer::new(out, meta).map(Writer::Docx),
            Format::DocxFlow => docx_flow::Writer::new(out, meta).map(Writer::DocxFlow),
            Format::Pptx => {
                let first = doc.page_size(0).unwrap_or([612.0, 792.0]);
                pptx::Writer::new(out, meta, first, count).map(Writer::Pptx)
            }
        };
        let writer = match writer {
            Ok(w) => w,
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                return Err(e.to_string());
            }
        };
        Ok(Job { doc, api, path: path.to_owned(), temp, writer: Some(writer), fonts: FontTable::default(), next: 0, count })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The next page; true once every page is in (then `finish`).
    pub fn step(&mut self, faces: &mut crate::fonts::Fonts) -> Result<bool, String> {
        if self.next >= self.count {
            return Ok(true);
        }
        let index = self.next;
        let page = self.doc.load_page(index).and_then(|p| p.flattened()).ok_or_else(|| format!("page {} could not be loaded", index + 1))?;
        let content = page.export_content(&mut self.fonts, false);
        let result = match self.writer.as_mut() {
            Some(Writer::Docx(w)) => w.page(self.api, &page, &content, &self.fonts.fonts, faces),
            Some(Writer::DocxFlow(w)) => w.page(self.api, &page, &content, &self.fonts.fonts, faces),
            Some(Writer::Pptx(w)) => w.page(self.api, &page, &content, &self.fonts.fonts, faces),
            None => Err("the export is over".into()),
        };
        drop(page);
        result.map_err(|e| format!("page {}: {e}", index + 1))?;
        self.next += 1;
        Ok(self.next >= self.count)
    }

    /// The file finished and put in place.
    pub fn finish(mut self, faces: &mut crate::fonts::Fonts) -> Result<(), String> {
        let written = match self.writer.take() {
            Some(Writer::Docx(w)) => w.finish(&self.fonts.fonts, faces),
            Some(Writer::DocxFlow(w)) => w.finish(&self.fonts.fonts, faces),
            Some(Writer::Pptx(w)) => w.finish(),
            None => Err("the export is over".into()),
        };
        if let Err(e) = written {
            let _ = std::fs::remove_file(&self.temp);
            return Err(e);
        }
        std::fs::rename(&self.temp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&self.temp);
            format!("cannot write {}: {e}", self.path.display())
        })
    }
}

impl Drop for Job {
    /// Abandoned (cancelled, failed): the half-written file goes.
    fn drop(&mut self) {
        if self.writer.is_some() {
            self.writer = None;
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

/// The document's title and author, for the file's properties.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub title: Option<String>,
    pub author: Option<String>,
}

/// The face of every font of the document, looked up once each.
#[derive(Default)]
pub struct Faces {
    faces: Vec<Option<Face>>,
}

impl Faces {
    pub fn get(&mut self, id: u16, fonts: &[FontInfo], lookup: &mut crate::fonts::Fonts) -> Face {
        let i = id as usize;
        if self.faces.len() <= i {
            self.faces.resize(i + 1, None);
        }
        if let Some(face) = &self.faces[i] {
            return face.clone();
        }
        let face = match fonts.get(i) {
            Some(f) => {
                let face = lookup.face(&f.name, f.flags, f.weight, f.faux_bold);
                log::debug!("export: {} (flags {:#x}, weight {}) as {face:?}", f.name, f.flags, f.weight);
                face
            }
            None => Face { family: "Arial".into(), bold: false, italic: false },
        };
        self.faces[i] = Some(face.clone());
        face
    }
}

/// Export `input` to `output` (the format by its extension) on this
/// thread: the command line's `--export`.
pub fn run(input: &Path, output: &Path, password: Option<&str>) -> Result<usize, String> {
    let format = Format::of(output).ok_or("the output must be a .docx or a .pptx file")?;
    run_as(input, output, password, format)
}

/// The command line's --export-paragraphs: an explicit, reflowing Word mode.
pub fn run_paragraphs(input: &Path, output: &Path, password: Option<&str>) -> Result<usize, String> {
    if Format::of(output) != Some(Format::Docx) {
        return Err("paragraph export requires a .docx output file".into());
    }
    run_as(input, output, password, Format::DocxFlow)
}

fn run_as(input: &Path, output: &Path, password: Option<&str>, format: Format) -> Result<usize, String> {
    let api = crate::pdfium::init().map_err(|e| e.to_string())?;
    let doc = Document::open(api, input, password).map_err(|e| e.to_string())?;
    let mut job = Job::start(api, doc, output, format)?;
    let mut faces = crate::fonts::Fonts::default();
    while !job.step(&mut faces)? {}
    let pages = job.count;
    job.finish(&mut faces)?;
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::zip::read_entry;

    #[test]
    fn a_docx_filename_keeps_the_chosen_word_layout() {
        use super::Format::{Docx, DocxFlow, Pptx};
        let docx = std::path::Path::new("document.DOCX");
        assert_eq!(DocxFlow.for_path(docx), Some(DocxFlow));
        assert_eq!(Docx.for_path(docx), Some(Docx));
        assert_eq!(Pptx.for_path(docx), Some(Docx));
        assert_eq!(DocxFlow.for_path(std::path::Path::new("slides.pptx")), Some(Pptx));
        assert_eq!(DocxFlow.for_path(std::path::Path::new("document")), None);
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_paragraph_export_contains_ordinary_word_text() {
        let input = crate::pdfium::tests::sample_file("export-paragraphs");
        let out = std::env::temp_dir().join(format!("metalpdf_paragraphs_{}.docx", std::process::id()));
        assert_eq!(super::run_paragraphs(&input, &out, None).unwrap(), 2);
        let bytes = std::fs::read(&out).unwrap();
        let doc = String::from_utf8(read_entry(&bytes, "word/document.xml").unwrap()).unwrap();
        assert!(doc.contains("Hello MetalPDF") && doc.contains("Second line of text"));
        // The second page's text is sideways in the PDF. Keep the shown
        // landscape composition instead of turning the page to reflow it.
        assert!(doc.contains(r#"<w:pgSz w:w="16840" w:h="11900" w:orient="landscape"/>"#));
        // These are paragraphs in the main story, not positioned frames or
        // text boxes. Edits can rewrap lines and move subsequent paragraphs.
        assert!(!doc.contains("w:framePr") && !doc.contains("w:txbxContent") && !doc.contains("w:fitText"));
        assert!(doc.contains(r#"<w:pStyle w:val="Heading1"/>"#));
        assert!(read_entry(&bytes, "word/media/image1.png").is_some());
        std::fs::remove_file(out).unwrap();
        std::fs::remove_file(input).unwrap();
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_docx_hides_only_fully_exported_text_objects() {
        let input = crate::pdfium::tests::sample_file("export-complete-objects");
        let api = crate::pdfium::init().unwrap();
        let doc = crate::pdfium::Document::open(api, &input, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let content = page.export_content(&mut Default::default(), false);
        let before = page.render_whole(595, 842, 0).unwrap();
        // One letter of a text object is insufficient: the remaining letters
        // must not disappear from the artwork when a label is not exported.
        let partial = [content.glyphs[0].source].into_iter().collect();
        assert!(page.hide_selected_text(&partial).is_empty());
        let after = page.render_whole(595, 842, 0).unwrap();
        assert_eq!(before.pixels(), after.pixels());
        let all = content.glyphs.iter().map(|g| g.source).collect();
        assert_eq!(page.hide_selected_text(&all), all);
        assert!(super::all_white(page.render_whole(595, 842, 0).unwrap().pixels()));
        drop(page);
        drop(doc);
        std::fs::remove_file(input).unwrap();
    }

    /// The hand-written sample (`pdfium::tests::sample_pdf`: text on an A4
    /// page, a landscape page by /Rotate 90 with a picture) as a Word
    /// document and as a presentation: the archives hold their parts, the
    /// text is there, styled, and the picture went in.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_the_sample_saves_as_word_and_powerpoint() {
        let input = crate::pdfium::tests::sample_file("export");
        let text = |bytes: &[u8], part: &str| String::from_utf8(read_entry(bytes, part).unwrap_or_else(|| panic!("no {part}"))).unwrap();
        let out = std::env::temp_dir().join(format!("metalpdf_export_{}.docx", std::process::id()));
        assert_eq!(super::run(&input, &out, None).unwrap(), 2);
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_file(&out);
        let doc = text(&bytes, "word/document.xml");
        let plain: String = doc.split("<w:t ").skip(1).filter_map(|s| s.split_once('>').and_then(|(_, s)| s.split_once("</w:t>").map(|(t, _)| t))).collect();
        assert!(plain.contains("Hello MetalPDF") && plain.contains("Second line of text"), "{plain}");
        // The title is editable and a heading; the rotated landscape
        // page retains its paper orientation and sideways text as artwork.
        assert!(doc.contains(r#"<w:pStyle w:val="Heading1"/>"#));
        assert_eq!(doc.matches(r#"<w:type w:val="nextPage"/>"#).count(), 2);
        assert!(doc.contains(r#"w:orient="landscape""#));
        assert!(doc.contains("<w:framePr") && doc.contains("<w:fitText"));
        assert!(doc.contains("<w:drawing>") && read_entry(&bytes, "word/media/image1.png").is_some_and(|png| png.starts_with(b"\x89PNG")));
        for part in ["[Content_Types].xml", "_rels/.rels", "word/styles.xml", "word/_rels/document.xml.rels", "docProps/core.xml"] {
            assert!(read_entry(&bytes, part).is_some(), "no {part}");
        }
        let out = out.with_extension("pptx");
        assert_eq!(super::run(&input, &out, None).unwrap(), 2);
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_file(&out);
        let first = text(&bytes, "ppt/slides/slide1.xml");
        assert!(first.contains("<a:t>Hello MetalPDF</a:t>") && first.contains(r#"sz="2400""#), "{first}");
        // Page 2's text runs sideways as shown: it stays in the page's
        // picture, under the picture of its own.
        let second = text(&bytes, "ppt/slides/slide2.xml");
        assert!(!second.contains("Page two") && second.matches("<p:pic>").count() == 2, "{second}");
        assert!(text(&bytes, "ppt/presentation.xml").contains(r#"<p:sldSz cx="7556500" cy="10693400"/>"#));
        let _ = std::fs::remove_file(&input);
    }
}
