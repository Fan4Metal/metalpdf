//! The Word document: flowing paragraphs from `layout`, written as
//! WordprocessingML by hand.
//!
//! The pictures go into the archive as the pages are read; the text is
//! kept (as runs, small) until the last page, since the headings need the
//! body's size and the margins every page of one size. Each part of a
//! page in its own columns (`layout::segments`: a title over two columns,
//! the columns) is a section of Word's with that many columns, a column
//! break where the next column starts; the first section of a page starts
//! a new page, so each PDF page starts a Word page. A paragraph's line
//! pitch is the PDF's, as an exact line spacing, and the space before it
//! the PDF's gap, so that a page of text fills about a Word page and the
//! pagination holds; indents are counted from the paragraph's column.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;

use super::layout::{self, Align, Item, Options, Span, Vert};
use super::xml::{HEAD, esc};
use super::zip::ZipWriter;
use super::{Faces, FontInfo, Meta, PageContent, crop_bgrx, encode_png, shrink_rgba};
use crate::pdfium::{Page, sys::Api};

/// A paragraph as Word gets it, its geometry in points.
struct DocPara {
    spans: Vec<Span>,
    align: Align,
    /// From the column's left edge: the body's indent, the first line's
    /// offset against it (negative: hanging), and from its right edge.
    left: f32,
    first: f32,
    right: f32,
    size: f32,
    pitch: f32,
    lines: usize,
    first_baseline: f32,
    last_baseline: f32,
    top: f32,
    bottom: f32,
    chars: usize,
    /// Tab stops from the column's left edge, right-aligned ones flagged.
    tabs: Vec<(f32, bool)>,
}

enum Block {
    Para(DocPara),
    Picture { rel: usize, width: f32, height: f32, align: Align, left: f32, top: f32, bottom: f32 },
}

impl Block {
    fn top(&self) -> f32 {
        match self {
            Block::Para(p) => p.top,
            Block::Picture { top, .. } => *top,
        }
    }

    fn bottom(&self) -> f32 {
        match self {
            Block::Para(p) => p.bottom,
            Block::Picture { bottom, .. } => *bottom,
        }
    }
}

/// A part of a page in one or more columns (`layout::Segment`): the
/// blocks with the column each is in.
struct SegmentOut {
    columns: Vec<[f32; 2]>,
    blocks: Vec<(usize, Block)>,
    table: Option<TableOut>,
}

/// A table (`layout::Table`) with its cells' blocks; `borders` when the
/// PDF draws its rules.
struct TableOut {
    columns: Vec<[f32; 2]>,
    rows: Vec<([f32; 2], Vec<Vec<Block>>)>,
    borders: bool,
}

struct PageOut {
    size: [f32; 2],
    segments: Vec<SegmentOut>,
    /// The extent of what is on it: left, top, right, bottom.
    extent: Option<[f32; 4]>,
}

pub struct Writer {
    zip: ZipWriter<BufWriter<File>>,
    meta: Meta,
    pages: Vec<PageOut>,
    /// The pictures put in, by number (`media/imageN.png`).
    pictures: usize,
    /// Characters by size in half points, for the body's size.
    sizes: HashMap<i32, usize>,
    /// Characters by font, for the default font.
    font_use: HashMap<u16, usize>,
}

/// Word's units: twentieths of a point, and EMUs for drawings.
fn tw(pt: f32) -> i64 {
    (pt * 20.0).round() as i64
}

fn emu(pt: f32) -> i64 {
    (pt * 12700.0).round() as i64
}

const CONTENT_TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#;

/// Word 2013 and later's own mode, not "compatibility mode".
const SETTINGS: &str = r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#;

const APP: &str = r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>MetalPDF</Application></Properties>"#;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;

/// The core properties: the PDF's title and author.
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

impl Writer {
    pub fn new(mut zip: ZipWriter<BufWriter<File>>, meta: Meta) -> std::io::Result<Writer> {
        zip.add("[Content_Types].xml", &part(CONTENT_TYPES), true)?;
        zip.add("_rels/.rels", &part(ROOT_RELS), true)?;
        zip.add("word/settings.xml", &part(SETTINGS), true)?;
        zip.add("docProps/app.xml", &part(APP), true)?;
        zip.add("docProps/core.xml", core_xml(&meta).as_bytes(), true)?;
        Ok(Writer { zip, meta, pages: Vec::new(), pictures: 0, sizes: HashMap::new(), font_use: HashMap::new() })
    }

    /// A picture into the archive; its number.
    fn picture(&mut self, png: &[u8]) -> Result<usize, String> {
        self.pictures += 1;
        self.zip.add(&format!("word/media/image{}.png", self.pictures), png, false).map_err(|e| e.to_string())?;
        Ok(self.pictures)
    }

    pub fn page(&mut self, _api: &'static Api, page: &Page, content: &PageContent, _fonts: &[FontInfo], _faces: &mut crate::fonts::Fonts) -> Result<(), String> {
        let items = layout::analyse(content, Options { invisible: true, figures: true });
        let [w, h] = content.size;
        let scale = super::PICTURE_DPI / 72.0;
        let (pw, ph) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
        let mut render: Option<Option<crate::pdfium::Bitmap>> = None;
        let mut blocks: Vec<Option<Block>> = Vec::with_capacity(items.len());
        let mut extent: Option<[f32; 4]> = None;
        for item in &items {
            let r = item.rect();
            extent = Some(extent.map_or(r, |e| layout::union(e, r)));
            match item {
                Item::Para(p) => {
                    for g in p.lines.iter().flat_map(|l| &l.glyphs) {
                        *self.sizes.entry((g.size * 2.0).round() as i32).or_default() += 1;
                        *self.font_use.entry(g.font).or_default() += 1;
                    }
                    blocks.push(Some(Block::Para(doc_para(p))));
                }
                Item::Figure(f) => {
                    let (fw, fh) = (f.rect[2] - f.rect[0], f.rect[3] - f.rect[1]);
                    // A picture alone at its own resolution, at most as
                    // fine as the figures; anything else cut out of the
                    // page rendered.
                    let own = f.image.and_then(|i| page.image_pixels(i).ok()).map(|(iw, ih, rgba)| {
                        let (iw, ih, rgba) = shrink_rgba(iw, ih, rgba, (fw * scale).ceil() as u32, (fh * scale).ceil() as u32);
                        encode_png(iw, ih, &rgba, true)
                    });
                    let png = match own {
                        Some(png) => png?,
                        None => {
                            let bitmap = render.get_or_insert_with(|| page.render_whole(pw, ph, content.turns));
                            let Some(bitmap) = bitmap else {
                                blocks.push(None);
                                continue;
                            };
                            let x0 = ((f.rect[0] * scale).floor().max(0.0) as u32).min(pw - 1);
                            let y0 = ((f.rect[1] * scale).floor().max(0.0) as u32).min(ph - 1);
                            let x1 = ((f.rect[2] * scale).ceil() as u32).clamp(x0 + 1, pw);
                            let y1 = ((f.rect[3] * scale).ceil() as u32).clamp(y0 + 1, ph);
                            encode_png(x1 - x0, y1 - y0, &crop_bgrx(bitmap.pixels(), pw, [x0, y0, x1, y1]), false)?
                        }
                    };
                    let rel = self.picture(&png)?;
                    let mid = (f.column[0] + f.column[1]) / 2.0;
                    let col_w = (f.column[1] - f.column[0]).max(1.0);
                    let centre = (f.rect[0] + f.rect[2]) / 2.0;
                    let align = if (centre - mid).abs() < 0.08 * col_w {
                        Align::Center
                    } else if (f.rect[2] - f.column[1]).abs() < 0.04 * col_w && f.rect[0] > f.column[0] + 0.1 * col_w {
                        Align::Right
                    } else {
                        Align::Left
                    };
                    blocks.push(Some(Block::Picture { rel, width: fw, height: fh, align, left: (f.rect[0] - f.column[0]).max(0.0), top: f.rect[1], bottom: f.rect[3] }));
                }
            }
        }
        // A table has its rules drawn when paths lie along it.
        let ruled = |r: [f32; 4]| {
            content.graphics.iter().any(|g| {
                matches!(g.kind, super::GraphicKind::Path { .. }) && g.rect[0] >= r[0] - 8.0 && g.rect[2] <= r[2] + 8.0 && g.rect[1] >= r[1] - 8.0 && g.rect[3] <= r[3] + 8.0
            })
        };
        let segments = layout::segments(&items, &content.graphics)
            .into_iter()
            .map(|s| match s.table {
                Some(t) => {
                    let (first, last) = (t.columns[0], t.columns[t.columns.len() - 1]);
                    let rect = [first[0], t.rows[0].0[0], last[1], t.rows[t.rows.len() - 1].0[1]];
                    let rows = t.rows.iter().map(|(span, cells)| (*span, cells.iter().map(|ks| ks.iter().filter_map(|&k| blocks[k].take()).collect()).collect())).collect();
                    SegmentOut { columns: vec![[first[0], last[1]]], blocks: Vec::new(), table: Some(TableOut { columns: t.columns, rows, borders: ruled(rect) }) }
                }
                None if s.line => {
                    // One paragraph, the parts after the first behind tabs:
                    // the last one right-aligned when it ends at the right.
                    let left = s.columns[0][0];
                    let right = s.columns[s.columns.len() - 1][1];
                    let mut parts = s.items.iter().filter_map(|&(_, k)| match blocks[k].take() {
                        Some(Block::Para(p)) => Some((p, items[k].rect())),
                        _ => None,
                    });
                    let Some((mut line, _)) = parts.next() else {
                        return SegmentOut { columns: s.columns, blocks: Vec::new(), table: None };
                    };
                    let rest: Vec<(DocPara, [f32; 4])> = parts.collect();
                    let n = rest.len();
                    for (i, (p, r)) in rest.into_iter().enumerate() {
                        let flush_right = i + 1 == n && (right - r[2]).abs() < p.size;
                        line.tabs.push(if flush_right { (r[2] - left, true) } else { (r[0] - left, false) });
                        if let Some(last) = line.spans.last() {
                            line.spans.push(Span { text: "\t".into(), ..last.clone() });
                        }
                        line.spans.extend(p.spans);
                        line.chars += p.chars;
                        line.bottom = line.bottom.max(p.bottom);
                    }
                    line.align = Align::Left;
                    line.right = 0.0;
                    SegmentOut { columns: vec![[left, right]], blocks: vec![(0, Block::Para(line))], table: None }
                }
                None => SegmentOut { blocks: s.items.iter().filter_map(|&(c, k)| blocks[k].take().map(|b| (c, b))).collect(), columns: s.columns, table: None },
            })
            .filter(|s| !s.blocks.is_empty() || s.table.is_some())
            .collect();
        self.pages.push(PageOut { size: content.size, segments, extent });
        Ok(())
    }

    pub fn finish(mut self, fonts: &[FontInfo], lookup: &mut crate::fonts::Fonts) -> Result<(), String> {
        let mut faces = Faces::default();
        // The body: the size and the font most characters have.
        let body_size = self.sizes.iter().max_by_key(|(s, n)| (**n, -**s)).map_or(11.0, |(s, _)| *s as f32 / 2.0);
        let body_font = self.font_use.iter().max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f))).map(|(f, _)| *f);
        let body_face = body_font.map(|f| faces.get(f, fonts, lookup));
        // The headings: paragraphs set larger than the body, short; their
        // sizes ranked, the largest Heading 1.
        let is_heading = |p: &DocPara| p.size >= body_size * 1.15 && p.lines <= 3 && p.chars <= 200 && p.spans.iter().any(|s| s.text.chars().any(char::is_alphabetic));
        let mut heading_sizes: Vec<i32> = self
            .pages
            .iter()
            .flat_map(|pg| &pg.segments)
            .flat_map(|s| &s.blocks)
            .filter_map(|(_, b)| match b {
                Block::Para(p) if is_heading(p) => Some((p.size * 2.0).round() as i32),
                _ => None,
            })
            .collect();
        heading_sizes.sort_unstable_by(|a, b| b.cmp(a));
        heading_sizes.dedup();
        let level = |p: &DocPara| is_heading(p).then(|| heading_sizes.iter().position(|&s| s == (p.size * 2.0).round() as i32).unwrap_or(2).min(2) + 1);

        // Runs of pages of one size share their margins: the middle value
        // of their text's extents, so that a stray footer does not decide.
        let mut runs: Vec<(usize, usize, [f32; 4])> = Vec::new();
        let mut start = 0;
        for i in 1..=self.pages.len() {
            let same = i < self.pages.len() && (0..2).all(|k| (self.pages[i].size[k] - self.pages[start].size[k]).abs() < 1.0);
            if same {
                continue;
            }
            let size = self.pages[start].size;
            let mut ms: [Vec<f32>; 4] = Default::default();
            for pg in &self.pages[start..i] {
                if let Some(e) = pg.extent {
                    ms[0].push(e[0]);
                    ms[1].push(e[1]);
                    ms[2].push(size[0] - e[2]);
                    ms[3].push(size[1] - e[3]);
                }
            }
            let pick = |v: &mut Vec<f32>, limit: f32| {
                v.sort_by(f32::total_cmp);
                v.get(v.len() / 2).copied().unwrap_or(72.0).clamp(18.0, limit.max(18.0))
            };
            // The bottom margin places nothing (the gaps are counted from
            // the top): half of it is room for lines that come out taller
            // in Word, so that a footer does not go over to a page alone.
            let bottom = (pick(&mut ms[3], 0.3 * size[1]) / 2.0).max(14.0);
            let margins = [pick(&mut ms[0], 0.3 * size[0]), pick(&mut ms[1], 0.3 * size[1]), pick(&mut ms[2], 0.3 * size[0]), bottom];
            runs.push((start, i, margins));
            start = i;
        }

        // A segment in columns is a section of Word's with those columns;
        // the one-column segments and the tables between them share one.
        // The first section of a page starts a new page, the others go on
        // on it (continuous). A section ends in its last paragraph, the
        // document's last one in the body.
        let mut out = Out { fonts, lookup, faces: &mut faces, doc_pr: 0 };
        let mut body = String::new();
        let mut last_sect = String::new();
        for &(from, to, margins) in &runs {
            for pi in from..to {
                let pg = &self.pages[pi];
                let size = pg.size;
                let text_w = (size[0] - margins[0] - margins[2]).max(72.0);
                if pg.segments.is_empty() {
                    // An empty page: a paragraph of its own carries it.
                    if !last_sect.is_empty() {
                        close_section(&mut body, &last_sect);
                    }
                    body.push_str("<w:p/>");
                    last_sect = sect_pr(size, margins, &[], text_w, true);
                    continue;
                }
                let segs = &pg.segments;
                // The bottom of what came before on the page.
                let mut above: Option<f32> = None;
                let mut si = 0;
                while si < segs.len() {
                    let multi = |s: &SegmentOut| s.columns.len() >= 2 && s.table.is_none();
                    let mut sj = si + 1;
                    if !multi(&segs[si]) {
                        while sj < segs.len() && !multi(&segs[sj]) {
                            sj += 1;
                        }
                    }
                    let cols = if multi(&segs[si]) { word_columns(&segs[si].columns, text_w) } else { vec![(text_w, 0.0)] };
                    let mut content = String::new();
                    let mut ends_in_table = false;
                    for seg in &segs[si..sj] {
                        if let Some(t) = &seg.table {
                            above = Some(table_xml(&mut content, t, above.unwrap_or(margins[1]), margins[0], text_w, &level, &mut out));
                            ends_in_table = true;
                            continue;
                        }
                        ends_in_table = false;
                        let seg_top = seg.blocks.iter().map(|(_, b)| b.top()).fold(f32::INFINITY, f32::min);
                        let mut column = usize::MAX;
                        let mut prev: Prev = Prev::default();
                        for (c, block) in &seg.blocks {
                            let col_w = cols.get(*c).map_or(text_w, |c| c.0);
                            let new_column = *c != column;
                            // The first in a column: from what is above it,
                            // or the segment's top in the second column on,
                            // with a column break before it.
                            if new_column {
                                prev = Prev { baseline: None, bottom: Some(if *c == 0 { above.unwrap_or(margins[1]) } else { seg_top.min(block.top()) }) };
                            }
                            column = *c;
                            block_xml(&mut content, block, &mut prev, col_w, new_column && *c > 0, &level, &mut out);
                        }
                        above = Some(seg.blocks.iter().map(|(_, b)| b.bottom()).fold(f32::NEG_INFINITY, f32::max));
                    }
                    if ends_in_table {
                        // A section cannot end in a table: a paragraph a
                        // point high follows it.
                        content.push_str(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/></w:pPr></w:p>"#);
                    }
                    // The section before ends in the last paragraph so far.
                    if !last_sect.is_empty() {
                        close_section(&mut body, &last_sect);
                    }
                    body.push_str(&content);
                    last_sect = sect_pr(size, margins, &cols, text_w, si == 0);
                    si = sj;
                }
            }
        }
        let document = format!("{HEAD}<w:document {NS}><w:body>{body}{last_sect}</w:body></w:document>");
        self.zip.add("word/document.xml", document.as_bytes(), true).map_err(|e| e.to_string())?;
        self.zip.add("word/styles.xml", styles_xml(body_face.as_ref(), body_size).as_bytes(), true).map_err(|e| e.to_string())?;
        let mut rels = format!(r#"{HEAD}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdSettings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#);
        for n in 1..=self.pictures {
            let _ = write!(rels, r#"<Relationship Id="rIdImg{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image{n}.png"/>"#);
        }
        rels.push_str("</Relationships>");
        self.zip.add("word/_rels/document.xml.rels", rels.as_bytes(), true).map_err(|e| e.to_string())?;
        let _ = &self.meta;
        self.zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// What writing the blocks needs: the fonts and the drawings' numbers.
struct Out<'a> {
    fonts: &'a [FontInfo],
    lookup: &'a mut crate::fonts::Fonts,
    faces: &'a mut Faces,
    doc_pr: usize,
}

/// Where the block before ended, for the space before the next: its last
/// baseline (a paragraph's) or its bottom.
#[derive(Default)]
struct Prev {
    baseline: Option<f32>,
    bottom: Option<f32>,
}

/// A block as a paragraph: `col_w` the width of its column, a column
/// break before it when asked.
fn block_xml(x: &mut String, block: &Block, prev: &mut Prev, col_w: f32, column_break: bool, level: &dyn Fn(&DocPara) -> Option<usize>, out: &mut Out) {
    x.push_str("<w:p><w:pPr>");
    match block {
        Block::Para(p) => {
            let height = if p.lines >= 2 { p.pitch } else { 1.17 * p.size };
            let before = match (prev.baseline, prev.bottom) {
                (Some(b), _) => p.first_baseline - b - height,
                (None, Some(bottom)) => p.top - bottom,
                (None, None) => 0.0,
            };
            if let Some(n) = level(p) {
                let _ = write!(x, r#"<w:pStyle w:val="Heading{n}"/>"#);
            }
            if !p.tabs.is_empty() {
                x.push_str("<w:tabs>");
                for &(pos, right) in &p.tabs {
                    let _ = write!(x, r#"<w:tab w:val="{}" w:pos="{}"/>"#, if right { "right" } else { "left" }, tw(pos.clamp(0.0, col_w)));
                }
                x.push_str("</w:tabs>");
            }
            let before = slack(before, p.size);
            let _ = write!(x, r#"<w:spacing w:before="{}" w:after="0""#, tw(before.clamp(0.0, 400.0)));
            if p.lines >= 2 && p.pitch >= 0.9 * p.size {
                let _ = write!(x, r#" w:line="{}" w:lineRule="exact""#, tw(p.pitch));
            }
            x.push_str("/>");
            let limit = 0.8 * col_w;
            let left = p.left.clamp(0.0, limit);
            let right = p.right.clamp(0.0, limit);
            let first_line = p.first.clamp(-left, limit);
            let _ = write!(x, r#"<w:ind w:left="{}" w:right="{}""#, tw(left), tw(right));
            if first_line >= 0.0 {
                let _ = write!(x, r#" w:firstLine="{}"/>"#, tw(first_line));
            } else {
                let _ = write!(x, r#" w:hanging="{}"/>"#, tw(-first_line));
            }
            let _ = write!(x, r#"<w:jc w:val="{}"/>"#, jc(p.align));
            x.push_str("</w:pPr>");
            if column_break {
                x.push_str(r#"<w:r><w:br w:type="column"/></w:r>"#);
            }
            for span in &p.spans {
                run_xml(x, span, &out.faces.get(span.font, out.fonts, out.lookup));
            }
            *prev = Prev { baseline: Some(p.last_baseline), bottom: Some(p.bottom) };
        }
        Block::Picture { rel, width, height, align, left, top, bottom } => {
            let k = (col_w / width).min(1.0);
            let (cx, cy) = (emu(width * k), emu(height * k));
            let before = slack(prev.bottom.map_or(0.0, |b| top - b), 10.0);
            out.doc_pr += 1;
            let n = out.doc_pr;
            let _ = write!(x, r#"<w:spacing w:before="{}" w:after="0"/>"#, tw(before.clamp(0.0, 400.0)));
            if *align == Align::Left && *left > 0.0 {
                let _ = write!(x, r#"<w:ind w:left="{}"/>"#, tw(left.min(col_w - width * k).max(0.0)));
            }
            let _ = write!(x, r#"<w:jc w:val="{}"/></w:pPr>"#, jc(*align));
            if column_break {
                x.push_str(r#"<w:r><w:br w:type="column"/></w:r>"#);
            }
            let _ = write!(
                x,
                r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Picture {n}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="image{rel}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg{rel}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
            );
            *prev = Prev { baseline: None, bottom: Some(*bottom) };
        }
    }
    x.push_str("</w:p>");
}

/// A gap of more than three ems 15 % smaller: Word's lines come out a
/// little taller than the PDF's now and then, and a footer at the page's
/// foot, after a large gap, went over to a page of its own.
fn slack(gap: f32, size: f32) -> f32 {
    if gap > 3.0 * size { gap * 0.85 } else { gap }
}

/// Word's cell margin, left and right (0.075 inch).
const CELL_PAD: f32 = 5.4;

/// A table: its columns where the PDF's are, its rows as high, the cells'
/// text centred in them; rules when the PDF draws them. `above` is the
/// bottom of what came before (a spacer paragraph makes the gap: a table
/// has no space before). The table's bottom comes back.
fn table_xml(x: &mut String, t: &TableOut, above: f32, margin_left: f32, text_w: f32, level: &dyn Fn(&DocPara) -> Option<usize>, out: &mut Out) -> f32 {
    let k = t.columns.len();
    let mut edges = vec![t.columns[0][0] - CELL_PAD];
    for i in 1..k {
        edges.push((t.columns[i - 1][1] + t.columns[i][0]) / 2.0);
    }
    edges.push(t.columns[k - 1][1] + CELL_PAD);
    let mut widths: Vec<f32> = edges.windows(2).map(|w| (w[1] - w[0]).max(18.0)).collect();
    let total: f32 = widths.iter().sum();
    if total > text_w {
        widths.iter_mut().for_each(|w| *w *= text_w / total);
    }
    let total: f32 = widths.iter().sum();
    let indent = (edges[0] - margin_left).clamp(-margin_left, (text_w - total).max(0.0));
    // The rows' edges: halfway between one row's text and the next's, the
    // outer ones as far out as the inner gaps are on average.
    let rows = &t.rows;
    let n = rows.len();
    let gaps: Vec<f32> = (1..n).map(|r| (rows[r].0[0] - rows[r - 1].0[1]).max(0.0) / 2.0).collect();
    let pad = if gaps.is_empty() { 3.0 } else { gaps.iter().sum::<f32>() / gaps.len() as f32 };
    let mut ys = vec![rows[0].0[0] - pad];
    for r in 1..n {
        ys.push((rows[r - 1].0[1] + rows[r].0[0]) / 2.0);
    }
    ys.push(rows[n - 1].0[1] + pad);
    let before = slack(ys[0] - above, 10.0);
    if before > 1.0 {
        let _ = write!(x, r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="{}" w:lineRule="exact"/></w:pPr></w:p>"#, tw(before.min(400.0)));
    }
    let _ = write!(x, r#"<w:tbl><w:tblPr><w:tblW w:w="{}" w:type="dxa"/><w:tblInd w:w="{}" w:type="dxa"/>"#, tw(total), tw(indent));
    if t.borders {
        x.push_str("<w:tblBorders>");
        for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
            let _ = write!(x, r#"<w:{side} w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#);
        }
        x.push_str("</w:tblBorders>");
    }
    x.push_str(r#"<w:tblLayout w:type="fixed"/></w:tblPr><w:tblGrid>"#);
    for w in &widths {
        let _ = write!(x, r#"<w:gridCol w:w="{}"/>"#, tw(*w));
    }
    x.push_str("</w:tblGrid>");
    for (r, (_, cells)) in rows.iter().enumerate() {
        let _ = write!(x, r#"<w:tr><w:trPr><w:trHeight w:val="{}" w:hRule="atLeast"/></w:trPr>"#, tw(ys[r + 1] - ys[r]));
        for (c, blocks) in cells.iter().enumerate() {
            let _ = write!(x, r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/><w:vAlign w:val="center"/></w:tcPr>"#, tw(widths[c]));
            if blocks.is_empty() {
                x.push_str("<w:p/>");
            }
            let mut prev = Prev::default();
            for block in blocks {
                block_xml(x, block, &mut prev, widths[c] - 2.0 * CELL_PAD, false, level, out);
            }
            x.push_str("</w:tc>");
        }
        x.push_str("</w:tr>");
    }
    x.push_str("</w:tbl>");
    ys[n]
}

/// The section properties `sect` put into the last paragraph of `body`
/// (every paragraph written has a `w:pPr`; an empty one gets one).
fn close_section(body: &mut String, sect: &str) {
    if body.ends_with("<w:p/>") {
        body.truncate(body.len() - "<w:p/>".len());
        body.push_str(&format!("<w:p><w:pPr>{sect}</w:pPr></w:p>"));
        return;
    }
    let Some(start) = body.rfind("<w:p>") else {
        body.push_str(&format!("<w:p><w:pPr>{sect}</w:pPr></w:p>"));
        return;
    };
    match body[start..].find("</w:pPr>") {
        Some(at) => body.insert_str(start + at, sect),
        None => body.insert_str(start + "<w:p>".len(), &format!("<w:pPr>{sect}</w:pPr>")),
    }
}

/// The PDF's columns as Word's: widths and the space after each, scaled
/// so that they fill the text's width.
fn word_columns(columns: &[[f32; 2]], text_w: f32) -> Vec<(f32, f32)> {
    if columns.len() < 2 {
        return vec![(text_w, 0.0)];
    }
    let span = (columns[columns.len() - 1][1] - columns[0][0]).max(1.0);
    let k = text_w / span;
    columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let space = columns.get(i + 1).map_or(0.0, |n| (n[0] - c[1]).max(0.0));
            (((c[1] - c[0]) * k).max(36.0), space * k)
        })
        .collect()
}

/// A paragraph of `layout` with its indents counted from its column.
fn doc_para(p: &layout::Para) -> DocPara {
    let n = p.lines.len();
    let col = p.column;
    let lefts: Vec<f32> = p.lines.iter().map(|l| l.rect[0]).collect();
    let max_right = p.lines.iter().map(|l| l.rect[2]).fold(f32::NEG_INFINITY, f32::max);
    let (left, first, right) = match (p.align, p.text_start()) {
        (_, Some(start)) => (start - col[0], lefts[0] - start, 0.0),
        (Align::Center | Align::Right, None) => (0.0, 0.0, 0.0),
        (_, None) => {
            let body = if n >= 2 { lefts[1..].iter().copied().fold(f32::INFINITY, f32::min) } else { lefts[0] };
            let right = if n >= 2 && col[1] - max_right > 2.0 * p.size { col[1] - max_right } else { 0.0 };
            (body - col[0], lefts[0] - body, right)
        }
    };
    let spans = layout::spans(p, "\t");
    DocPara {
        spans,
        align: p.align,
        left: left.max(0.0),
        first,
        right,
        size: p.size,
        pitch: p.pitch(),
        lines: n,
        first_baseline: p.lines[0].baseline,
        last_baseline: p.lines[n - 1].baseline,
        top: p.rect[1],
        bottom: p.rect[3],
        chars: p.chars(),
        tabs: Vec::new(),
    }
}

fn jc(align: Align) -> &'static str {
    match align {
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
        Align::Justify => "both",
    }
}

/// A section's properties: how it starts (a new page, or on the page
/// going on), the page, the margins, the columns (Word's widths and the
/// space after each).
fn sect_pr(size: [f32; 2], m: [f32; 4], cols: &[(f32, f32)], text_w: f32, new_page: bool) -> String {
    // Word takes pages up to 22 inches a side.
    let (w, h) = (size[0].min(1584.0), size[1].min(1584.0));
    let orient = if w > h { r#" w:orient="landscape""# } else { "" };
    let kind = if new_page { "nextPage" } else { "continuous" };
    let columns = if cols.len() >= 2 {
        // The widths scaled to the text's width once more, after the
        // narrow ones were widened.
        let total: f32 = cols.iter().map(|c| c.0 + c.1).sum();
        let k = text_w / total.max(1.0);
        let mut s = format!(r#"<w:cols w:num="{}" w:equalWidth="0">"#, cols.len());
        for c in cols {
            let _ = write!(s, r#"<w:col w:w="{}" w:space="{}"/>"#, tw(c.0 * k), tw(c.1 * k));
        }
        s.push_str("</w:cols>");
        s
    } else {
        String::new()
    };
    format!(
        r#"<w:sectPr><w:type w:val="{kind}"/><w:pgSz w:w="{}" w:h="{}"{orient}/><w:pgMar w:top="{}" w:right="{}" w:bottom="{}" w:left="{}" w:header="0" w:footer="0" w:gutter="0"/>{columns}</w:sectPr>"#,
        tw(w),
        tw(h),
        tw(m[1]),
        tw(m[2]),
        tw(m[3]),
        tw(m[0])
    )
}

/// A run: its font, weight, slant, colour, size and position; a tab in
/// the text as Word's own tab.
fn run_xml(x: &mut String, span: &Span, face: &super::Face) {
    let mut rpr = String::from("<w:rPr>");
    let f = esc(&face.family);
    let _ = write!(rpr, r#"<w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/>"#);
    if face.bold {
        rpr.push_str("<w:b/><w:bCs/>");
    }
    if face.italic {
        rpr.push_str("<w:i/><w:iCs/>");
    }
    if span.color != [0, 0, 0] {
        let [r, g, b] = span.color;
        let _ = write!(rpr, r#"<w:color w:val="{r:02X}{g:02X}{b:02X}"/>"#);
    }
    let hp = ((span.size * 2.0).round() as i64).clamp(2, 3276);
    let _ = write!(rpr, r#"<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>"#);
    match span.vert {
        Vert::Super => rpr.push_str(r#"<w:vertAlign w:val="superscript"/>"#),
        Vert::Sub => rpr.push_str(r#"<w:vertAlign w:val="subscript"/>"#),
        Vert::Base => {}
    }
    rpr.push_str("</w:rPr>");
    for (i, part) in span.text.split('\t').enumerate() {
        if i > 0 {
            let _ = write!(x, "<w:r>{rpr}<w:tab/></w:r>");
        }
        if !part.is_empty() {
            let _ = write!(x, r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#, esc(part));
        }
    }
}

/// The styles: the body's font and size as the default, and Word's own
/// Heading 1–3 (the runs carry their look; the styles give the outline).
fn styles_xml(face: Option<&super::Face>, size: f32) -> String {
    let family = esc(&face.map_or("Arial".to_owned(), |f| f.family.clone()));
    let hp = ((size * 2.0).round() as i64).clamp(2, 3276);
    let mut s = format!(
        r#"{HEAD}<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{family}" w:hAnsi="{family}" w:eastAsia="{family}" w:cs="{family}"/><w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/><w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/><w:unhideWhenUsed/></w:style>"#
    );
    for n in 1..=3 {
        let _ = write!(
            s,
            r#"<w:style w:type="paragraph" w:styleId="Heading{n}"><w:name w:val="heading {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:outlineLvl w:val="{}"/></w:pPr></w:style>"#,
            n - 1
        );
    }
    s.push_str("</w:styles>");
    s
}
