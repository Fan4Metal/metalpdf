//! A fixed page layout with editable text. Each PDF line is a page-anchored
//! Word text frame; its words retain their advances with `w:fitText`.
//! Reflowing the PDF's text, even with exact paragraph spacing, changes its
//! line breaks and pagination as soon as a font's metrics differ. Frames
//! keep columns, tables, side headings and footers at their original places.
//! The page rendered without the exported text preserves all other artwork.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;

use super::layout::{self, Item, Options};
use super::xml::{HEAD, esc};
use super::zip::ZipWriter;
use super::{Faces, FontInfo, Glyph, Meta, PageContent, crop_bgrx, encode_png};
use crate::pdfium::{Page, sys::Api};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateFontW, DEFAULT_CHARSET, DeleteDC, DeleteObject,
    GetTextExtentPoint32W, HDC, HFONT, SelectObject,
};

/// Measure the actual replacement font at a large em, independently of the
/// screen DPI. Explicit character scaling also keeps viewers which ignore
/// `fitText` from wrapping a PDF line into the frame underneath it.
struct Measurements {
    dc: HDC,
    fonts: HashMap<u16, HFONT>,
}

impl Measurements {
    fn new() -> Self {
        Self {
            dc: unsafe { CreateCompatibleDC(std::ptr::null_mut()) },
            fonts: HashMap::new(),
        }
    }

    fn width(&mut self, id: u16, face: &super::Face, text: &str, size: f32) -> Option<f32> {
        if self.dc.is_null() {
            return None;
        }
        let font = *self.fonts.entry(id).or_insert_with(|| {
            let family: Vec<u16> = face.family.encode_utf16().chain(Some(0)).collect();
            unsafe {
                CreateFontW(
                    -2048,
                    0,
                    0,
                    0,
                    if face.bold { 700 } else { 400 },
                    u32::from(face.italic),
                    0,
                    0,
                    DEFAULT_CHARSET.into(),
                    0,
                    0,
                    0,
                    0,
                    family.as_ptr(),
                )
            }
        });
        if font.is_null() || unsafe { SelectObject(self.dc, font) }.is_null() {
            return None;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut extent = windows_sys::Win32::Foundation::SIZE::default();
        let ok = unsafe {
            GetTextExtentPoint32W(
                self.dc,
                units.as_ptr(),
                i32::try_from(units.len()).ok()?,
                &mut extent,
            )
        } != 0;
        (ok && extent.cx > 0).then_some(extent.cx as f32 * size / 2048.0)
    }
}

impl Drop for Measurements {
    fn drop(&mut self) {
        // Release the selected font before deleting the owned font handles.
        if !self.dc.is_null() {
            unsafe { DeleteDC(self.dc) };
        }
        for font in self.fonts.values() {
            if !font.is_null() {
                unsafe { DeleteObject(*font) };
            }
        }
    }
}

struct Out<'a> {
    fonts: &'a [FontInfo],
    lookup: &'a mut crate::fonts::Fonts,
    faces: Faces,
    measurements: Measurements,
    id: usize,
}

struct Run {
    text: String,
    font: u16,
    size: f32,
    color: [u8; 3],
    baseline: f32,
    width: f32,
}

struct Line {
    runs: Vec<Run>,
    x: f32,
    baseline: f32,
    size: f32,
    width: f32,
    /// Only the first line of a short paragraph can carry its heading.
    heading: Option<f32>,
    invisible: bool,
}

struct PageOut {
    size: [f32; 2],
    lines: Vec<Line>,
    background: Option<usize>,
}

pub struct Writer {
    zip: ZipWriter<BufWriter<File>>,
    pages: Vec<PageOut>,
    pictures: usize,
    sizes: HashMap<i32, usize>,
    font_use: HashMap<u16, usize>,
}

fn tw(pt: f32) -> i64 {
    (pt * 20.0).round() as i64
}

fn emu(pt: f32) -> i64 {
    (pt * 12700.0).round() as i64
}

const CONTENT_TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#;
const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#;
const SETTINGS: &str = r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:characterSpacingControl w:val="doNotCompress"/><w:autoHyphenation w:val="0"/><w:doNotTrackMoves/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#;
const APP: &str = r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>MetalPDF</Application></Properties>"#;
pub(super) const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;

pub fn core_xml(meta: &Meta) -> String {
    let mut s = String::from(HEAD);
    s.push_str(r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#);
    if let Some(t) = &meta.title {
        let _ = write!(s, "<dc:title>{}</dc:title>", esc(t));
    }
    if let Some(a) = &meta.author {
        let _ = write!(s, "<dc:creator>{}</dc:creator>", esc(a));
    }
    s.push_str("</cp:coreProperties>");
    s
}

fn part(xml: &str) -> Vec<u8> {
    format!("{HEAD}{xml}").into_bytes()
}

/// The parts shared by both Word layout modes.
pub(super) fn start_package(zip: &mut ZipWriter<BufWriter<File>>, meta: &Meta) -> std::io::Result<()> {
    zip.add("[Content_Types].xml", &part(CONTENT_TYPES), true)?;
    zip.add("_rels/.rels", &part(ROOT_RELS), true)?;
    zip.add("word/settings.xml", &part(SETTINGS), true)?;
    zip.add("docProps/app.xml", &part(APP), true)?;
    zip.add("docProps/core.xml", core_xml(meta).as_bytes(), true)
}

pub(super) fn document_rels(pictures: usize) -> String {
    let mut rels = format!(r#"{HEAD}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdSettings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#);
    for n in 1..=pictures {
        let _ = write!(rels, r#"<Relationship Id="rIdImg{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image{n}.png"/>"#);
    }
    rels.push_str("</Relationships>");
    rels
}

impl Writer {
    pub fn new(mut zip: ZipWriter<BufWriter<File>>, meta: Meta) -> std::io::Result<Writer> {
        start_package(&mut zip, &meta)?;
        Ok(Writer {
            zip,
            pages: Vec::new(),
            pictures: 0,
            sizes: HashMap::new(),
            font_use: HashMap::new(),
        })
    }

    pub fn page(
        &mut self,
        _api: &'static Api,
        page: &Page,
        content: &PageContent,
        _fonts: &[FontInfo],
        _faces: &mut crate::fonts::Fonts,
    ) -> Result<(), String> {
        let mut lines = Vec::new();
        let selected = layout::analyse(
            content,
            Options {
                invisible: true,
                figures: false,
            },
        )
        .into_iter()
        .filter_map(|item| match item {
            Item::Para(p) => Some(p),
            _ => None,
        })
        .flat_map(|p| {
            p.lines
                .into_iter()
                .flat_map(|l| l.glyphs.into_iter().map(|g| g.source))
        })
        .collect();
        let hidden = page.hide_selected_text(&selected);
        if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
            eprintln!(
                "docx: {} glyphs, {} selected, {} hidden",
                content.glyphs.len(),
                selected.len(),
                hidden.len()
            );
        }
        // Word's paper limit is 22 inches. Scale oversized pages uniformly,
        // including their artwork, coordinates and font sizes.
        let k = page_scale(content.size);
        let size = content.size.map(|v| v * k);
        for invisible in [false, true] {
            let mut layer = content.clone();
            layer
                .glyphs
                .retain(|g| g.invisible == invisible && hidden.contains(&g.source));
            for item in layout::analyse(
                &layer,
                Options {
                    invisible,
                    figures: false,
                },
            ) {
                let Item::Para(p) = item else { continue };
                let short = p.lines.len() <= 3
                    && p.chars() <= 200
                    && p.lines
                        .iter()
                        .flat_map(|l| &l.glyphs)
                        .any(|g| g.ch.is_alphabetic());
                for (i, l) in p.lines.iter().enumerate() {
                    for g in &l.glyphs {
                        *self
                            .sizes
                            .entry((g.size * k * 2.0).round() as i32)
                            .or_default() += 1;
                        *self.font_use.entry(g.font).or_default() += 1;
                    }
                    lines.push(line_out(
                        l,
                        k,
                        (i == 0 && short && !invisible).then_some(p.size * k),
                        invisible,
                    ));
                }
            }
        }
        // Unlike cropping figures out of the original page, this retains
        // table rules, shading, clipping, rotated text and image transforms,
        // without a second copy of the editable text painted underneath it.
        let scale = super::PICTURE_DPI / 72.0;
        let (bw, bh) = (
            ((size[0] * scale).round() as u32).max(1),
            ((size[1] * scale).round() as u32).max(1),
        );
        let bitmap = page
            .render_whole(bw, bh, content.turns)
            .ok_or("could not render the page artwork")?;
        let background = if super::all_white(bitmap.pixels()) {
            None
        } else {
            let png = encode_png(
                bw,
                bh,
                &crop_bgrx(bitmap.pixels(), bw, [0, 0, bw, bh]),
                false,
            )?;
            self.pictures += 1;
            self.zip
                .add(
                    &format!("word/media/image{}.png", self.pictures),
                    &png,
                    false,
                )
                .map_err(|e| e.to_string())?;
            Some(self.pictures)
        };
        self.pages.push(PageOut {
            size,
            lines,
            background,
        });
        Ok(())
    }

    pub fn finish(
        mut self,
        fonts: &[FontInfo],
        lookup: &mut crate::fonts::Fonts,
    ) -> Result<(), String> {
        let mut faces = Faces::default();
        let body_size = self
            .sizes
            .iter()
            .max_by_key(|(s, n)| (**n, -**s))
            .map_or(11.0, |(s, _)| *s as f32 / 2.0);
        let body_font = self
            .font_use
            .iter()
            .max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f)))
            .map(|(f, _)| *f);
        let body_face = body_font.map(|f| faces.get(f, fonts, lookup));
        let mut heading_sizes: Vec<i32> = self
            .pages
            .iter()
            .flat_map(|p| &p.lines)
            .filter_map(|l| {
                l.heading
                    .filter(|&s| s >= body_size * 1.15)
                    .map(|s| (s * 2.0).round() as i32)
            })
            .collect();
        heading_sizes.sort_unstable_by(|a, b| b.cmp(a));
        heading_sizes.dedup();
        let mut body = String::new();
        let mut out = Out {
            fonts,
            lookup,
            faces,
            measurements: Measurements::new(),
            id: 0,
        };
        for (i, pg) in self.pages.iter().enumerate() {
            for line in &pg.lines {
                let heading = line
                    .heading
                    .and_then(|s| {
                        heading_sizes
                            .iter()
                            .position(|&h| h == (s * 2.0).round() as i32)
                    })
                    .map(|n| n.min(2) + 1);
                line_xml(&mut body, line, heading, &mut out);
            }
            // Only this tiny, unframed paragraph participates in pagination.
            // Section properties belong here, never inside a text frame.
            body.push_str(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr>"#);
            if i + 1 < self.pages.len() {
                body.push_str(&sect_pr(pg.size));
            }
            body.push_str("</w:pPr>");
            if let Some(rel) = pg.background {
                picture_xml(&mut body, rel, pg.size);
            }
            body.push_str("</w:p>");
        }
        if let Some(pg) = self.pages.last() {
            body.push_str(&sect_pr(pg.size));
        }
        let document = format!("{HEAD}<w:document {NS}><w:body>{body}</w:body></w:document>");
        self.zip
            .add("word/document.xml", document.as_bytes(), true)
            .map_err(|e| e.to_string())?;
        self.zip
            .add(
                "word/styles.xml",
                styles_xml(body_face.as_ref(), body_size).as_bytes(),
                true,
            )
            .map_err(|e| e.to_string())?;
        self.zip
            .add("word/_rels/document.xml.rels", document_rels(self.pictures).as_bytes(), true)
            .map_err(|e| e.to_string())?;
        self.zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn page_scale(size: [f32; 2]) -> f32 {
    (1584.0 / size[0].max(size[1]).max(1.0)).min(1.0)
}

fn same_run(a: &Glyph, b: &Glyph) -> bool {
    a.font == b.font
        && (a.size - b.size).abs() < 0.01
        && a.color == b.color
        && (a.baseline - b.baseline).abs() < 0.01
}

fn line_out(line: &layout::Line, k: f32, heading: Option<f32>, invisible: bool) -> Line {
    let gs = &line.glyphs;
    let mut runs = Vec::new();
    let mut start = 0;
    for end in 1..=gs.len() {
        if end < gs.len() && !gs[end].space_before && same_run(&gs[start], &gs[end]) {
            continue;
        }
        let g = &gs[start];
        let text: String = gs[start..end]
            .iter()
            .map(|g| if g.ch == '\u{ad}' { '-' } else { g.ch })
            .collect();
        let edge = gs[start..end].iter().map(|g| g.rect[2]).fold(g.x, f32::max);
        let right = if let Some(next) = gs.get(end) {
            if next.space_before {
                edge.min(next.x)
            } else {
                next.x
            }
        } else {
            edge
        };
        runs.push(Run {
            text,
            font: g.font,
            size: g.size * k,
            color: g.color,
            baseline: g.baseline * k,
            width: ((right - g.x) * k).max(0.05),
        });
        // Justification widens the space, not the word's glyphs.
        if let Some(next) = gs
            .get(end)
            .filter(|next| next.space_before && next.x > right)
        {
            runs.push(Run {
                text: " ".into(),
                font: g.font,
                size: g.size * k,
                color: g.color,
                baseline: g.baseline * k,
                width: (next.x - right) * k,
            });
        }
        start = end;
    }
    Line {
        runs,
        x: gs[0].x * k,
        baseline: line.baseline * k,
        size: line.size * k,
        width: (line.rect[2] - gs[0].x) * k,
        heading,
        invisible,
    }
}

fn line_xml(x: &mut String, l: &Line, heading: Option<usize>, out: &mut Out) {
    // Exact spacing in Word puts the baseline at 4/5 of the line box.
    // Two ems leave room for ascenders, descenders and shifted small runs.
    let height = tw(2.0 * l.size).max(20);
    let top = tw(l.baseline) - (height * 4 / 5);
    x.push_str("<w:p><w:pPr>");
    if let Some(n) = heading {
        let _ = write!(x, r#"<w:pStyle w:val="Heading{n}"/>"#);
    }
    let _ = write!(
        x,
        r#"<w:keepNext w:val="0"/><w:keepLines w:val="0"/><w:framePr w:w="{}" w:h="{height}" w:x="{}" w:y="{top}" w:hAnchor="page" w:vAnchor="page" w:wrap="none" w:hRule="atLeast" w:hSpace="0" w:vSpace="0"/><w:widowControl w:val="0"/><w:suppressAutoHyphens/><w:snapToGrid w:val="0"/><w:spacing w:before="0" w:after="0" w:line="{height}" w:lineRule="exact"/><w:ind w:left="0" w:right="0" w:firstLine="0"/><w:jc w:val="left"/></w:pPr>"#,
        tw(l.width.max(1.0) + l.size.max(2.0)),
        tw(l.x)
    );
    for run in &l.runs {
        let face = out.faces.get(run.font, out.fonts, out.lookup);
        out.id += 1;
        let id = out.id;
        let f = esc(&face.family);
        let _ = write!(
            x,
            r#"<w:r><w:rPr><w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/>"#
        );
        if face.bold {
            x.push_str("<w:b/><w:bCs/>");
        }
        if face.italic {
            x.push_str("<w:i/><w:iCs/>");
        }
        if l.invisible {
            x.push_str("<w:vanish/>");
        }
        let [r, g, b] = run.color;
        let hp = ((run.size * 2.0).round() as i64).clamp(2, 3276);
        let scale = out
            .measurements
            .width(run.font, &face, &run.text, hp as f32 / 2.0)
            .map_or(100, |width| {
                (100.0 * run.width / width).round().clamp(1.0, 600.0) as i64
            });
        let shift = ((l.baseline - run.baseline) * 2.0).round() as i64;
        let _ = write!(
            x,
            r#"<w:color w:val="{r:02X}{g:02X}{b:02X}"/><w:w w:val="{scale}"/><w:position w:val="{shift}"/><w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/><w:fitText w:val="{}" w:id="{id}"/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r>"#,
            tw(run.width).max(1),
            esc(&run.text)
        );
    }
    x.push_str("</w:p>");
}

fn sect_pr(size: [f32; 2]) -> String {
    let orient = if size[0] > size[1] {
        r#" w:orient="landscape""#
    } else {
        ""
    };
    format!(
        r#"<w:sectPr><w:type w:val="nextPage"/><w:pgSz w:w="{}" w:h="{}"{orient}/><w:pgMar w:top="0" w:right="0" w:bottom="0" w:left="0" w:header="0" w:footer="0" w:gutter="0"/></w:sectPr>"#,
        tw(size[0]),
        tw(size[1])
    )
}

fn picture_xml(x: &mut String, rel: usize, size: [f32; 2]) {
    let (cx, cy) = (emu(size[0]), emu(size[1]));
    let _ = write!(
        x,
        r#"<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="0" behindDoc="1" locked="1" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/><wp:wrapNone/><wp:docPr id="{rel}" name="Page artwork {rel}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{rel}" name="image{rel}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg{rel}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"#
    );
}

fn styles_xml(face: Option<&super::Face>, size: f32) -> String {
    let family = esc(face.map_or("Arial", |f| f.family.as_str()));
    let hp = ((size * 2.0).round() as i64).clamp(2, 3276);
    let mut s = format!(
        r#"{HEAD}<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{family}" w:hAnsi="{family}" w:eastAsia="{family}" w:cs="{family}"/><w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:before="0" w:after="0"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#
    );
    for n in 1..=3 {
        let _ = write!(
            s,
            r#"<w:style w:type="paragraph" w:styleId="Heading{n}"><w:name w:val="heading {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:outlineLvl w:val="{}"/></w:pPr></w:style>"#,
            n - 1
        );
    }
    s.push_str("</w:styles>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_pages_scale_uniformly() {
        assert_eq!(page_scale([595.0, 842.0]), 1.0);
        assert_eq!(page_scale([1000.0, 3168.0]), 0.5);
        assert_eq!(page_scale([3168.0, 1000.0]), 0.5);
    }

    #[test]
    fn words_keep_their_positions_and_printed_hyphens() {
        let glyph = |ch, x, space_before| Glyph {
            source: -1,
            ch,
            rect: [x, 40.0, x + 5.0, 52.0],
            x,
            baseline: 50.0,
            size: 10.0,
            font: 0,
            color: [0, 0, 0],
            space_before,
            invisible: false,
        };
        let l = layout::Line {
            glyphs: vec![
                glyph('a', 20.0, false),
                glyph('b', 25.0, false),
                glyph('c', 37.0, true),
                glyph('\u{ad}', 42.0, false),
            ],
            rect: [20.0, 40.0, 47.0, 52.0],
            baseline: 50.0,
            size: 10.0,
        };
        let out = line_out(&l, 0.5, None, false);
        assert_eq!(out.runs.len(), 3);
        assert_eq!((&*out.runs[0].text, out.runs[0].width), ("ab", 5.0));
        assert_eq!((&*out.runs[1].text, out.runs[1].width), (" ", 3.5));
        assert_eq!((&*out.runs[2].text, out.runs[2].width), ("c-", 5.0));
        assert_eq!((out.x, out.baseline), (10.0, 25.0));
    }
}
