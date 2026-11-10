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
//! pitch is the PDF's minimum line spacing, and the space before it
//! the PDF's original gap; indents are counted
//! from the paragraph's column. Word controls the resulting pagination.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;

use super::docx::{self, NS};
use super::layout::{self, Align, Item, Options, Span, Vert};
use super::xml::{HEAD, esc};
use super::zip::ZipWriter;
use super::{Faces, FontInfo, Glyph, Meta, PageContent, crop_bgrx, encode_png};
use crate::pdfium::{Page, sys::Api};

mod toc;
mod artwork;
mod forms;
mod grid;
mod overlay;
mod wrapping;
mod captions;
mod equations;
mod marks;
mod scan;

struct InlinePicture {
    marker: char,
    rel: usize,
    width: f32,
    height: f32,
    shift: f32,
}

struct FloatingPicture {
    rel: usize,
    placement: wrapping::Picture,
    region: overlay::Region,
}

struct Decoration {
    rel: usize,
    region: overlay::Region,
}

struct Tab {
    pos: f32,
    right: bool,
    leader: bool,
}

/// A separate initial-letter frame, anchored to the following paragraph.
/// Its font and baseline must never become those of the surrounding text.
#[derive(Clone, Debug)]
struct DropCap {
    glyph: Glyph,
    first_baseline: f32,
    text_left: f32,
    lines: usize,
}

impl DropCap {
    fn starts(&self, p: &layout::Para) -> bool {
        p.marker.is_none()
            && p.lines.len() >= self.lines
            && (p.lines[0].baseline - self.first_baseline).abs() < 0.3 * p.size
            && (p.lines[0].rect[0] - self.text_left).abs() < 0.5 * p.size
    }
}

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
    /// Tab stops from the column's left edge, including contents leaders.
    tabs: Vec<Tab>,
    drop_cap: Option<DropCap>,
    artwork: Vec<InlinePicture>,
    floating: Vec<FloatingPicture>,
    /// A rule drawn above and below it, as its borders.
    rules: [Option<ParaRule>; 2],
}

/// A rule across the page drawn as a paragraph's border, so that it goes
/// with its text: its colour, thickness and distance from the text.
#[derive(Clone, Copy, Debug)]
struct ParaRule {
    color: [u8; 3],
    width: f32,
    space: f32,
}

enum Block {
    Para(DocPara),
    Picture { rel: usize, width: f32, height: f32, align: Align, left: f32, top: f32, bottom: f32, region: overlay::Region },
    Grid(forms::Table),
}

impl Block {
    fn top(&self) -> f32 {
        match self {
            Block::Para(p) => p.top,
            Block::Picture { top, .. } => *top,
            Block::Grid(table) => table.rect[1],
        }
    }

    fn bottom(&self) -> f32 {
        match self {
            Block::Para(p) => p.bottom,
            Block::Picture { bottom, .. } => *bottom,
            Block::Grid(table) => table.rect[3],
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
    /// Side heads: the cells' text at the top, the second column's where
    /// the PDF's starts.
    side_heads: bool,
}

/// A segment of two columns whose left one is short labels each at the top
/// of a part of the right one (a manual's "Description", "Example" beside
/// their text) as a table without borders, a row a label: as Word's
/// columns, the right column ran on after the labels', its tables split
/// over pages.
fn side_heads(s: &layout::Segment, items: &[Item], blocks: &mut [Option<Block>]) -> Option<TableOut> {
    if !s.side || s.columns.len() != 2 {
        return None;
    }
    let (left, right) = (s.columns[0], s.columns[1]);
    let mut labels: Vec<usize> = s.items.iter().filter(|m| m.0 == 0).map(|m| m.1).collect();
    let body: Vec<usize> = s.items.iter().filter(|m| m.0 == 1).map(|m| m.1).collect();
    let size = |k: usize| match &items[k] {
        Item::Para(p) => p.size,
        Item::Figure(_) => 10.0,
    };
    labels.sort_by(|&a, &b| items[a].rect()[1].total_cmp(&items[b].rect()[1]));
    // A row from a label to the next; what is above the first label is
    // a row of its own with no label.
    let starts: Vec<f32> = labels.iter().map(|&k| items[k].rect()[1] - 1.2 * size(k)).collect();
    let row_of = |top: f32| starts.iter().rposition(|&s| s <= top);
    let mut rows: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
    if body.iter().any(|&k| row_of(items[k].rect()[1]).is_none()) {
        rows.push((None, Vec::new()));
    }
    let lead = rows.len();
    rows.extend(labels.iter().map(|&k| (Some(k), Vec::new())));
    // The parts of the right column by row, each with its box: a
    // paragraph whose lines run past a label's start (values a line each,
    // joined as their pitch was the group's usual one) cut there.
    let mut parts: Vec<Vec<([f32; 4], f32, Block)>> = (0..rows.len()).map(|_| Vec::new()).collect();
    for &k in &body {
        let Some(block) = blocks[k].take() else { continue };
        let column = items[k].column();
        if let (Item::Para(p), Block::Para(d)) = (&items[k], &block)
            && d.artwork.is_empty()
            && d.floating.is_empty()
            && d.drop_cap.is_none()
            && p.lines.iter().any(|l| row_of(l.rect[1]) != row_of(p.lines[0].rect[1]))
        {
            let mut from = 0;
            for n in 1..=p.lines.len() {
                if n == p.lines.len() || row_of(p.lines[n].rect[1]) != row_of(p.lines[from].rect[1]) {
                    let lines = p.lines[from..n].to_vec();
                    let rect = lines.iter().fold(lines[0].rect, |a, l| layout::union(a, l.rect));
                    let piece = layout::Para { lines, rect, marker: if from == 0 { p.marker } else { None }, ..p.clone() };
                    let r = row_of(rect[1]).map_or(0, |r| r + lead);
                    parts[r].push((rect, column[0], Block::Para(doc_para(&piece, None))));
                    from = n;
                }
            }
            continue;
        }
        let rect = items[k].rect();
        let r = row_of(rect[1]).map_or(0, |r| r + lead);
        parts[r].push((rect, column[0], block));
    }
    let out = rows
        .into_iter()
        .zip(parts)
        .map(|((label, _), mut ks)| {
            let span = label.iter().map(|&k| items[k].rect()).chain(ks.iter().map(|k| k.0)).fold([f32::INFINITY, f32::NEG_INFINITY], |a, r| [a[0].min(r[1]), a[1].max(r[3])]);
            ks.sort_by(|a, b| a.0[1].total_cmp(&b.0[1]).then(a.0[0].total_cmp(&b.0[0])));
            let mut body: Vec<([f32; 4], Block)> = Vec::new();
            for (r, column_left, mut block) in ks {
                // Indented from the cell as the item's column is from the
                // right column (less when its column is the page's).
                let shift = column_left - right[0];
                match &mut block {
                    Block::Para(p) => {
                        p.left = (p.left + shift).max(0.0);
                        p.first = p.first.max(-p.left);
                    }
                    Block::Picture { left, .. } => *left = (*left + shift).max(0.0),
                    Block::Grid(_) => {}
                }
                // A one-line label beside its text (a note's "Note:"): the
                // text's paragraph, the label hanging before a tab.
                if let (Some((pr, Block::Para(prev))), Block::Para(p)) = (body.last_mut(), &mut block)
                    && prev.lines == 1
                    && prev.tabs.is_empty()
                    && pr[2] < r[0]
                    && (pr[1] - r[1]).abs() < 0.5 * p.size
                {
                    let label_left = prev.left;
                    let mut spans = std::mem::take(&mut prev.spans);
                    if let Some(last) = spans.last() {
                        spans.push(Span { text: "\t".into(), ..last.clone() });
                    }
                    spans.append(&mut p.spans);
                    p.spans = spans;
                    p.artwork.append(&mut prev.artwork);
                    p.first = label_left - p.left;
                    p.tabs.insert(0, Tab { pos: p.left, right: false, leader: false });
                    p.chars += prev.chars;
                    p.top = p.top.min(prev.top);
                    body.pop();
                }
                body.push((r, block));
            }
            // The label as it is in its cell: from the left, on one line
            // when it fits (no justification, no indents from the PDF's
            // column).
            let label = label.and_then(|k| blocks[k].take()).map(|mut b| {
                if let Block::Para(p) = &mut b {
                    p.align = layout::Align::Left;
                    p.left = 0.0;
                    p.right = 0.0;
                    p.first = 0.0;
                }
                b
            });
            let cells = vec![label.into_iter().collect(), body.into_iter().map(|(_, b)| b).collect()];
            (span, cells)
        })
        .collect();
    Some(TableOut { columns: vec![left, right], rows: out, borders: false, side_heads: true })
}

struct PageOut {
    size: [f32; 2],
    segments: Vec<SegmentOut>,
    /// The extent of what is on it: left, top, right, bottom.
    extent: Option<[f32; 4]>,
    /// A graphics-only page, including titles drawn as letter outlines.
    picture: Option<usize>,
    decorations: Vec<Decoration>,
}

pub struct Writer {
    zip: ZipWriter<BufWriter<File>>,
    pages: Vec<PageOut>,
    /// The pictures put in, by number (`media/imageN.png`).
    pictures: usize,
    /// Characters by size in half points, for the body's size.
    sizes: HashMap<i32, usize>,
    /// Characters by font, for the default font.
    font_use: HashMap<u16, usize>,
    /// Unjustified word advances, for matching replacement font metrics.
    advances: HashMap<u16, Vec<(String, f32, f32)>>,
}

/// Word's units: twentieths of a point, and EMUs for drawings.
fn tw(pt: f32) -> i64 {
    (pt * 20.0).round() as i64
}

fn emu(pt: f32) -> i64 {
    (pt * 12700.0).round() as i64
}

impl Writer {
    pub fn new(mut zip: ZipWriter<BufWriter<File>>, meta: Meta) -> std::io::Result<Writer> {
        docx::start_package(&mut zip, &meta)?;
        Ok(Writer { zip, pages: Vec::new(), pictures: 0, sizes: HashMap::new(), font_use: HashMap::new(), advances: HashMap::new() })
    }

    /// A picture into the archive; its number.
    fn picture(&mut self, png: &[u8]) -> Result<usize, String> {
        self.pictures += 1;
        self.zip.add(&format!("word/media/image{}.png", self.pictures), png, false).map_err(|e| e.to_string())?;
        Ok(self.pictures)
    }

    fn remember_line(&mut self, line:&layout::Line) {
        let mut start = 0;
        for end in 1..=line.glyphs.len() {
            let g = &line.glyphs[start];
            if end < line.glyphs.len() && !line.glyphs[end].space_before
                && line.glyphs[end].font == g.font && (line.glyphs[end].size - g.size).abs() < 0.1 {
                continue;
            }
            let word = &line.glyphs[start..end];
            let samples = self.advances.entry(g.font).or_default();
            if samples.len() < 250 && word.len() >= 4 && word.iter().all(|g| g.ch.is_alphabetic()) {
                let width = word.last().unwrap().rect[2] - g.x;
                samples.push((word.iter().map(|g| g.ch).collect(), g.size, width));
            }
            start = end;
        }
    }

    pub fn page(&mut self, _api: &'static Api, page: &Page, content: &PageContent, fonts: &[FontInfo], _faces: &mut crate::fonts::Fonts) -> Result<(), String> {
        let mut unmarked = content.clone();
        marks::remove(&mut unmarked);
        let content = &unmarked;
        let [w, h] = content.size;
        let scale = super::PICTURE_DPI / 72.0;
        let mut form = forms::detect(content, fonts);
        let mut flow_content = content.clone();
        // A scan's figures, out of the page-wide picture its text lies over.
        for rect in scan::figures(page, content) {
            flow_content.graphics.push(super::Graphic { rect, kind: super::GraphicKind::Image, object: None });
        }
        if let Some(form) = &form {forms::remove(&mut flow_content,form);}
        // Ruled and shaded tables, as Word's tables: out of what flows and
        // of the figures, back in below as a stand-in each takes the place of.
        let grids = grid::detect(&flow_content);
        if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
            esay!("grid: {} rules, tables {:?}", flow_content.rules.len(), grids.iter().map(|t| (t.rect, t.rows.len(), t.edges.len() - 1)).collect::<Vec<_>>());
        }
        grid::remove(&mut flow_content, &grids);
        captions::prepare(&mut flow_content);
        let mut equations = equations::prepare(&mut flow_content, fonts);
        if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
            esay!("equations: blocks {:?}, inline {:?}", equations.blocks, equations.inline.iter().map(|i| i.rect).collect::<Vec<_>>());
        }
        let mut prepared = artwork::prepare(&flow_content, fonts, std::mem::take(&mut equations.inline));
        if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
            esay!("artwork: decorations {:?}, images {:?}", prepared.decorations, prepared.content.graphics.iter().filter(|g| g.kind == super::GraphicKind::Image && g.object.is_none()).map(|g| g.rect).collect::<Vec<_>>());
        }
        for &rect in &equations.blocks {
            prepared.content.graphics.push(super::Graphic {rect,kind:super::GraphicKind::Image,object:None});
        }
        for t in &grids {
            prepared.content.graphics.push(super::Graphic { rect: t.rect, kind: super::GraphicKind::Image, object: None });
        }
        if graphics_page(content) || prepared.whole_page && form.is_none() && grids.is_empty() && equations.blocks.is_empty() {
            // Letter outlines have no editable paragraphs. Clustering them
            // into inline figures tears a designed cover into separate words
            // or letters. Keep the entire composition, including its margins.
            let size = picture_page_size(content.size);
            let (pw, ph) = (((size[0] * scale).round() as u32).max(1), ((size[1] * scale).round() as u32).max(1));
            let bitmap = page.render_whole(pw, ph, content.turns).ok_or("could not render the graphics-only page")?;
            let png = encode_png(pw, ph, &crop_bgrx(bitmap.pixels(), pw, [0, 0, pw, ph]), false)?;
            let rel = self.picture(&png)?;
            self.pages.push(PageOut { size, segments: Vec::new(), extent: None, picture: Some(rel), decorations: Vec::new() });
            return Ok(());
        }
        let (items, drop_caps) = flowing_items(&prepared.content);
        let (items, floating) = wrapping::prepare(items, &prepared.content, &grids.iter().map(|t| t.rect).collect::<Vec<_>>());
        let mut render: Option<Option<crate::pdfium::Bitmap>> = None;
        // Word refuses a file with a drawing of no size or less.
        let decoration_regions: Vec<_> = prepared.decorations.iter().chain(form.iter().flat_map(|f| &f.decorations)).copied().filter(|r| r[2] - r[0] >= 0.5 && r[3] - r[1] >= 0.5).collect();
        let regions = decoration_regions.iter().copied().chain(items.iter().filter_map(|item| match item {
            Item::Figure(f) if !form.as_ref().is_some_and(|form| same_rect(form.table.rect, f.rect)) && !grids.iter().any(|t| same_rect(t.rect, f.rect)) => Some(f.rect),
            _ => None,
        })).chain(floating.iter().map(|p| p.figure.rect)).collect();
        let mut layers = overlay::Layers::prepare(page, content, fonts, regions, &mut render)?;
        let mut blocks: Vec<Option<Block>> = Vec::with_capacity(items.len());
        let mut extent = floating.iter().map(|p| p.figure.rect).reduce(layout::union);
        let mut decorations = Vec::new();
        for rect in decoration_regions {
            let (png, region) = layers.take(page, content, rect, &mut render)?;
            decorations.push(Decoration { rel: self.picture(&png)?, region });
        }
        for (index, item) in items.iter().enumerate() {
            let r = item.rect();
            extent = Some(extent.map_or(r, |e| layout::union(e, r)));
            match item {
                Item::Para(p) => {
                    for line in &p.lines {self.remember_line(line);}
                    for g in p.lines.iter().flat_map(|l| &l.glyphs) {
                        *self.sizes.entry((g.size * 2.0).round() as i32).or_default() += 1;
                        *self.font_use.entry(g.font).or_default() += 1;
                    }
                    let normalized = wrapping::paragraph(p, &floating);
                    let mut para = doc_para(&normalized, drop_caps.iter().find(|cap| cap.starts(p)));
                    for placement in floating.iter().filter(|picture| picture.anchor == index) {
                        let (png, region) = layers.take(page, content, placement.figure.rect, &mut render)?;
                        para.floating.push(FloatingPicture { rel: self.picture(&png)?, placement: placement.clone(), region });
                    }
                    if para.tabs.iter().any(|t| t.leader) {
                        para.pitch = toc::pitch(p, &items);
                    }
                    for g in p.lines.iter().flat_map(|l| &l.glyphs).filter(|g| g.source < -1) {
                        let index = (-g.source - 2) as usize;
                        if let Some(art) = prepared.inline.get(index) {
                            let png = render_crop(page, content, art.rect, &mut render)?;
                            para.artwork.push(InlinePicture { marker: g.ch, rel: self.picture(&png)?, width: art.rect[2] - art.rect[0], height: art.rect[3] - art.rect[1], shift: art.baseline - art.rect[3] });
                        }
                    }
                    blocks.push(Some(Block::Para(para)));
                }
                Item::Figure(f) => {
                    if let Some(table) = form.as_mut().filter(|form| (0..4).all(|k|(form.table.rect[k]-f.rect[k]).abs()<1.0)) {
                        let pw = ((w*scale).round() as u32).max(1);
                        let ph = ((h*scale).round() as u32).max(1);
                        let bitmap = render.get_or_insert_with(||page.render_whole(pw,ph,content.turns)).as_ref().ok_or("could not render table shading")?;
                        for cell in table.table.rows.iter_mut().flat_map(|r|&mut r.cells) {
                            let x=(((cell.rect[0]+1.0)*scale) as u32).min(pw-1);
                            let y=(((cell.rect[1]+1.0)*scale) as u32).min(ph-1);
                            let at=((y*pw+x)*4) as usize;
                            let pixel=&bitmap.pixels()[at..at+4];
                            cell.fill=[pixel[2],pixel[1],pixel[0]];
                            for line in layout::lines(cell.glyphs.clone()) {self.remember_line(&line);}
                            for g in &cell.glyphs {
                                *self.sizes.entry((g.size*2.0).round() as i32).or_default()+=1;
                                *self.font_use.entry(g.font).or_default()+=1;
                            }
                        }
                        blocks.push(Some(Block::Grid(table.table.clone())));
                        continue;
                    }
                    if let Some(table) = grids.iter().find(|t| same_rect(t.rect, f.rect)) {
                        for cell in table.rows.iter().flat_map(|r| &r.cells) {
                            for line in layout::lines(cell.glyphs.clone()) {
                                self.remember_line(&line);
                            }
                            for g in &cell.glyphs {
                                *self.sizes.entry((g.size * 2.0).round() as i32).or_default() += 1;
                                *self.font_use.entry(g.font).or_default() += 1;
                            }
                        }
                        blocks.push(Some(Block::Grid(table.clone())));
                        continue;
                    }
                    let (fw, fh) = (f.rect[2] - f.rect[0], f.rect[3] - f.rect[1]);
                    let (png, mut region) = layers.take(page, content, f.rect, &mut render)?;
                    if equations.blocks.iter().any(|r|f.rect[0]<=r[0]+1.0&&f.rect[2]>=r[2]-1.0&&f.rect[1]<=r[1]+1.0&&f.rect[3]>=r[3]-1.0) {
                        equations::editable(&mut region);
                    }
                    let rel = self.picture(&png)?;
                    let mid = (f.column[0] + f.column[1]) / 2.0;
                    let col_w = (f.column[1] - f.column[0]).max(1.0);
                    let centre = (f.rect[0] + f.rect[2]) / 2.0;
                    // A formula set apart can be a group of its own whose
                    // column starts at it: centred on the text's width too.
                    let body = items.iter().filter_map(|i| match i { Item::Para(p) => Some([p.column[0], p.column[1]]), _ => None }).reduce(|a, b| [a[0].min(b[0]), a[1].max(b[1])]);
                    let on_body = body.is_some_and(|b| fw < 0.8 * (b[1] - b[0]) && (centre - (b[0] + b[1]) / 2.0).abs() < 0.04 * (b[1] - b[0]));
                    let align = if (centre - mid).abs() < 0.08 * col_w || on_body {
                        Align::Center
                    } else if (f.rect[2] - f.column[1]).abs() < 0.04 * col_w && f.rect[0] > f.column[0] + 0.1 * col_w {
                        Align::Right
                    } else {
                        Align::Left
                    };
                    // Leave room for Word's margins and the inline picture's
                    // paragraph on image-only pages, including landscape scans.
                    let fit = if fw > 0.95 * w && fh > 0.95 * h { ((h - 64.0).max(1.0) / fh).min(1.0) } else { 1.0 };
                    blocks.push(Some(Block::Picture { rel, width: fw * fit, height: fh * fit, align, left: (f.rect[0] - f.column[0]).max(0.0), top: f.rect[1], bottom: f.rect[3], region }));
                }
            }
        }
        // Rules across the text that no table, figure or running head took:
        // the borders of the paragraph just above or below them.
        let taken: Vec<[f32; 4]> = items
            .iter()
            .filter_map(|i| match i {
                Item::Figure(f) => Some(f.rect),
                Item::Para(_) => None,
            })
            .chain(floating.iter().map(|p| p.figure.rect))
            .chain(prepared.decorations.iter().copied())
            .chain(form.iter().flat_map(|f| f.decorations.iter().copied().chain([f.table.rect])))
            .chain(grids.iter().map(|t| t.rect))
            .collect();
        let mut rules: Vec<&super::Rule> = flow_content
            .rules
            .iter()
            .filter(|r| {
                let (rw, rh) = (r.rect[2] - r.rect[0], r.rect[3] - r.rect[1]);
                rh <= 3.0 && rw >= (0.25 * w).max(60.0) && r.color.iter().any(|&c| c < 245) && !taken.iter().any(|t| r.rect[0] < t[2] + 1.0 && t[0] < r.rect[2] - 1.0 && r.rect[1] < t[3] + 1.0 && t[1] < r.rect[3] - 1.0)
            })
            .collect();
        rules.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
        rules.dedup_by(|b, a| (b.rect[1] - a.rect[1]).abs() < 1.5 && b.rect[0] < a.rect[2] && a.rect[0] < b.rect[2]);
        for rule in rules {
            let r = rule.rect;
            let mut above: Option<(usize, f32)> = None;
            let mut below: Option<(usize, f32)> = None;
            for (k, b) in blocks.iter().enumerate() {
                let Some(Block::Para(p)) = b else { continue };
                let pr = items[k].rect();
                if !(pr[0] < r[2] && r[0] < pr[2]) {
                    continue;
                }
                let reach = 2.5 * p.size;
                let gap_above = r[1] - p.bottom;
                if (-1.0..reach).contains(&gap_above) && above.is_none_or(|(_, g)| gap_above < g) {
                    above = Some((k, gap_above));
                }
                let gap_below = p.top - r[3];
                if (-1.0..reach).contains(&gap_below) && below.is_none_or(|(_, g)| gap_below < g) {
                    below = Some((k, gap_below));
                }
            }
            let thickness = (r[3] - r[1]).max(0.25);
            let (side, (k, gap)) = match (above, below) {
                (Some(a), Some(b)) if b.1 < a.1 => (0, b),
                (Some(a), _) => (1, a),
                (None, Some(b)) => (0, b),
                (None, None) => continue,
            };
            if let Some(Some(Block::Para(p))) = blocks.get_mut(k)
                && p.rules[side].is_none()
            {
                p.rules[side] = Some(ParaRule { color: rule.color, width: thickness, space: gap.max(0.0) });
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
                    SegmentOut { columns: vec![[first[0], last[1]]], blocks: Vec::new(), table: Some(TableOut { columns: t.columns, rows, borders: ruled(rect), side_heads: false }) }
                }
                None if s.line
                    && matches!(s.items.first().and_then(|&(_, k)| blocks[k].as_ref()), Some(Block::Para(_)))
                    && s.items.iter().all(|&(_, k)| matches!(blocks[k], Some(Block::Para(_) | Block::Picture { .. }))) =>
                {
                    // One paragraph, the parts after the first behind tabs:
                    // the last one right-aligned when it ends at the right;
                    // a picture (a number set a digit an image) in the line.
                    let left = s.columns[0][0];
                    let right = s.columns[s.columns.len() - 1][1];
                    let mut parts = s.items.iter().filter_map(|&(_, k)| blocks[k].take().map(|b| (b, items[k].rect())));
                    let Some((Block::Para(mut line), _)) = parts.next() else {
                        return SegmentOut { columns: s.columns, blocks: Vec::new(), table: None };
                    };
                    let rest: Vec<(Block, [f32; 4])> = parts.collect();
                    let n = rest.len();
                    for (i, (b, r)) in rest.into_iter().enumerate() {
                        let flush_right = i + 1 == n && (right - r[2]).abs() < line.size;
                        line.tabs.push(Tab { pos: if flush_right { r[2] - left } else { r[0] - left }, right: flush_right, leader: false });
                        if let Some(last) = line.spans.last() {
                            line.spans.push(Span { text: "\t".into(), ..last.clone() });
                        }
                        match b {
                            Block::Para(p) => {
                                for side in 0..2 {
                                    line.rules[side] = line.rules[side].or(p.rules[side]);
                                }
                                line.spans.extend(p.spans);
                                line.artwork.extend(p.artwork);
                                line.floating.extend(p.floating);
                                line.chars += p.chars;
                                line.bottom = line.bottom.max(p.bottom);
                            }
                            Block::Picture { rel, width, height, bottom, .. } => {
                                let marker = char::from_u32(0x10_0000 + line.artwork.len() as u32).unwrap_or('\u{10ffff}');
                                if let Some(last) = line.spans.last() {
                                    line.spans.push(Span { text: marker.to_string(), ..last.clone() });
                                }
                                line.artwork.push(InlinePicture { marker, rel, width, height, shift: line.last_baseline - bottom });
                                line.bottom = line.bottom.max(bottom);
                            }
                            Block::Grid(_) => {}
                        }
                    }
                    line.align = Align::Left;
                    line.right = 0.0;
                    SegmentOut { columns: vec![[left, right]], blocks: vec![(0, Block::Para(line))], table: None }
                }
                None => match side_heads(&s, &items, &mut blocks) {
                    Some(t) => SegmentOut { columns: vec![[s.columns[0][0], s.columns[1][1]]], blocks: Vec::new(), table: Some(t) },
                    None => SegmentOut { blocks: s.items.iter().filter_map(|&(c, k)| blocks[k].take().map(|b| (c, b))).collect(), columns: s.columns, table: None },
                },
            })
            .filter(|s| !s.blocks.is_empty() || s.table.is_some())
            .collect();
        self.pages.push(PageOut { size: content.size, segments, extent, picture: None, decorations });
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
        let is_heading = |p: &DocPara| !p.tabs.iter().any(|t| t.leader) && p.size >= body_size * 1.15 && p.lines <= 3 && p.chars <= 200 && p.spans.iter().any(|s| s.text.chars().any(char::is_alphabetic));
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
        let mut measurements = docx::Measurements::new();
        let mut scales = HashMap::new();
        for (font, samples) in &self.advances {
            let face = faces.get(*font, fonts, lookup);
            let mut ratios: Vec<_> = samples.iter().filter_map(|(text, size, advance)| {
                measurements.width(*font, &face, text, *size).map(|width| advance / width)
            }).filter(|r| r.is_finite() && *r > 0.5 && *r < 1.6).collect();
            ratios.sort_by(f32::total_cmp);
            if let Some(ratio) = ratios.get(ratios.len()/2) {
                scales.insert(*font, (100.0 * ratio).round().min(100.0) as i64);
            }
        }
        let mut out = Out { fonts, lookup, faces: &mut faces, doc_pr: 0, scales, margin_left:0.0, textboxes:docx::TextBoxes::new() };
        let mut body = String::new();
        let mut last_sect = String::new();
        for &(from, to, margins) in &runs {
            for pi in from..to {
                let pg = &self.pages[pi];
                let size = pg.size;
                let mut margins = pg.extent.map_or(margins, |e| [e[0].clamp(18.0, 0.3 * size[0]), e[1].clamp(18.0, 0.3 * size[1]), (size[0]-e[2]).clamp(18.0, 0.3*size[0]), ((size[1]-e[3])/2.0).clamp(14.0, 0.15*size[1])]);
                let grid_carrier = !pg.decorations.is_empty() && pg.segments.first().is_some_and(|s| s.table.is_some() || s.blocks.first().is_some_and(|(_, b)| matches!(b, Block::Grid(_))));
                if grid_carrier {margins[1]=(margins[1]-1.0).max(0.0);}
                out.margin_left=margins[0];
                let text_w = (size[0] - margins[0] - margins[2]).max(72.0);
                if let Some(rel) = pg.picture {
                    if !last_sect.is_empty() {
                        close_section(&mut body, &last_sect);
                    }
                    picture_page_xml(&mut body, rel, size, &mut out);
                    last_sect = sect_pr(size, [0.0; 4], &[], size[0], true);
                    continue;
                }
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
                let mut carry: Option<Prev> = None;
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
                            carry = None;
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
                                prev = match carry.take() {
                                    // After a segment of one column, from its
                                    // last baseline, as within a segment.
                                    Some(p) if *c == 0 => p,
                                    _ => Prev { baseline: None, bottom: Some(if *c == 0 { above.unwrap_or(margins[1]) } else { seg_top.min(block.top()) }) },
                                };
                            }
                            column = *c;
                            // A table is indented from its Word column: the
                            // PDF's column it is in, in a section of columns.
                            out.margin_left = if multi(seg) { seg.columns.get(*c).map_or(margins[0], |col| col[0]) } else { margins[0] };
                            block_xml(&mut content, block, &mut prev, col_w, new_column && *c > 0, &level, &mut out);
                        }
                        above = Some(seg.blocks.iter().map(|(_, b)| b.bottom()).fold(f32::NEG_INFINITY, f32::max));
                        carry = (!multi(seg)).then_some(prev);
                    }
                    if ends_in_table {
                        // A section cannot end in a table: a paragraph a
                        // point high follows it.
                        content.push_str(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/></w:pPr></w:p>"#);
                    }
                    if si==0 && grid_carrier {
                        // Word positions drawings anchored inside a table
                        // against its cell even with layoutInCell="0". Use
                        // a tiny paragraph outside the table as the anchor.
                        content.insert_str(0,r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr></w:p>"#);
                    }
                    // The section before ends in the last paragraph so far.
                    if !last_sect.is_empty() {
                        close_section(&mut body, &last_sect);
                    }
                    body.push_str(&content);
                    if si == 0 && !pg.decorations.is_empty() {
                        let mut drawing = String::new();
                        for d in &pg.decorations {
                            decoration_xml(&mut drawing, d, &mut out);
                        }
                        // The drawing belongs to the first body paragraph;
                        // it contributes no extra paragraph or line height.
                        let start = body.len() - content.len();
                        if let Some(first) = body[start..].find("</w:pPr>") {
                            body.insert_str(start + first + "</w:pPr>".len(), &drawing);
                        }
                    }
                    last_sect = sect_pr(size, margins, &cols, text_w, si == 0);
                    si = sj;
                }
            }
        }
        let document = format!("{HEAD}<w:document {NS}><w:body>{body}{last_sect}</w:body></w:document>");
        self.zip.add("word/document.xml", document.as_bytes(), true).map_err(|e| e.to_string())?;
        self.zip.add("word/styles.xml", styles_xml(body_face.as_ref(), body_size).as_bytes(), true).map_err(|e| e.to_string())?;
        self.zip.add("word/_rels/document.xml.rels", docx::document_rels(self.pictures).as_bytes(), true).map_err(|e| e.to_string())?;
        self.zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn same_rect(a: [f32; 4], b: [f32; 4]) -> bool {
    (0..4).all(|k| (a[k] - b[k]).abs() < 1.0)
}

fn graphics_page(content: &PageContent) -> bool {
    content.glyphs.is_empty() && !content.graphics.is_empty()
}

fn picture_page_size(size: [f32; 2]) -> [f32; 2] {
    // Word's 22-inch limit, applied uniformly to retain the aspect ratio.
    let scale = (1584.0 / size[0].max(size[1]).max(1.0)).min(1.0);
    size.map(|n| n * scale)
}

/// A page-sized, selectable image. Anchoring it to the page avoids inline
/// picture padding, shrinking to text margins, and an extra blank page.
fn picture_page_xml(x: &mut String, rel: usize, size: [f32; 2], out: &mut Out) {
    x.push_str(r#"<w:p><w:pPr><w:widowControl w:val="0"/><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr>"#);
    decoration_xml(x, &Decoration { rel, region:overlay::Region { rect: [0.0, 0.0, size[0], size[1]], lines:Vec::new(), exact:Vec::new() } }, out);
    x.push_str("</w:p>");
}

fn decoration_xml(x: &mut String, d: &Decoration, out: &mut Out) {
    out.doc_pr += 1;
    let n = out.doc_pr;
    let rel = d.rel;
    let r = d.region.rect;
    let (cx, cy) = (emu(r[2] - r[0]), emu(r[3] - r[1]));
    let (left, top) = (emu(r[0]), emu(r[1]));
    let _ = write!(x,
        r#"<w:r><w:rPr><w:sz w:val="2"/></w:rPr><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="0" behindDoc="0" locked="1" layoutInCell="0" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>{left}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>{top}</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/><wp:wrapNone/><wp:docPr id="{n}" name="Page image {n}"/>"#);
    overlay::graphic(x, rel, n, [cx,cy], &d.region, out);
    x.push_str("</wp:anchor></w:drawing></w:r>");
}

fn render_crop(page: &Page, content: &PageContent, r: [f32; 4], render: &mut Option<Option<crate::pdfium::Bitmap>>) -> Result<Vec<u8>, String> {
    let scale = super::PICTURE_DPI / 72.0;
    let (w, h) = (((content.size[0] * scale).round() as u32).max(1), ((content.size[1] * scale).round() as u32).max(1));
    let bitmap = render.get_or_insert_with(|| page.render_whole(w, h, content.turns)).as_ref().ok_or("could not render page artwork")?;
    let x = ((r[0] * scale).floor().max(0.0) as u32).min(w-1);
    let y = ((r[1] * scale).floor().max(0.0) as u32).min(h-1);
    let right = ((r[2]*scale).ceil() as u32).clamp(x+1,w);
    let bottom = ((r[3]*scale).ceil() as u32).clamp(y+1,h);
    encode_png(right-x, bottom-y, &crop_bgrx(bitmap.pixels(), w, [x,y,right,bottom]), false)
}

fn paragraph_runs(x: &mut String, p: &DocPara, out: &mut Out) {
    for span in &p.spans {
        let mut text = String::new();
        for ch in span.text.chars() {
            if let Some(pic) = p.artwork.iter().find(|a| a.marker == ch) {
                if !text.is_empty() {
                    scaled_run_xml(x, &Span { text: std::mem::take(&mut text), ..span.clone() }, &out.faces.get(span.font, out.fonts, out.lookup), 0, *out.scales.get(&span.font).unwrap_or(&100));
                }
                out.doc_pr += 1;
                let n = out.doc_pr;
                let rel = pic.rel;
                let (cx,cy,shift) = (emu(pic.width),emu(pic.height),tw(pic.shift)/10);
                let _ = write!(x, r#"<w:r><w:rPr><w:sz w:val="2"/><w:position w:val="{shift}"/></w:rPr><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Inline artwork {n}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="image{rel}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg{rel}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#);
            } else { text.push(ch) }
        }
        if !text.is_empty() {
            scaled_run_xml(x, &Span { text, ..span.clone() }, &out.faces.get(span.font, out.fonts, out.lookup), 0, *out.scales.get(&span.font).unwrap_or(&100));
        }
    }
}

/// Recognise an isolated large letter beside several indented body lines.
/// Do this before line grouping: a dropped baseline otherwise attaches the
/// cap to a later line and turns its neighbours into giant superscripts.
fn drop_caps(content: &PageContent) -> Vec<(usize, DropCap)> {
    let mut sizes: HashMap<i32, usize> = HashMap::new();
    for g in &content.glyphs {
        if !g.invisible && g.ch.is_alphabetic() && g.size.is_finite() && g.size > 0.0 {
            *sizes.entry((g.size * 2.0).round() as i32).or_default() += 1;
        }
    }
    let Some((&size, _)) = sizes.iter().max_by_key(|(s, n)| (**n, -**s)) else { return Vec::new() };
    let body = size as f32 / 2.0;
    let candidates: Vec<_> = content.glyphs.iter().enumerate().filter(|(_, g)| {
        !g.invisible && g.ch.is_alphabetic() && g.size >= 2.0 * body && g.size <= 8.0 * body
    }).collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let lines = layout::lines(content.glyphs.iter().filter(|g| !g.invisible && g.size >= 0.8 * body && g.size <= 1.2 * body).cloned().collect());
    let mut caps = Vec::new();
    for (i, g) in candidates {
        // A word in a large heading or logo is not a drop cap.
        if content.glyphs.iter().enumerate().any(|(j, other)| {
            i != j && !other.ch.is_whitespace() && other.size >= 0.75 * g.size
                && (other.baseline - g.baseline).abs() < 0.5 * body
                && other.rect[0] < g.rect[2] + g.size && other.rect[2] > g.rect[0] - g.size
        }) {
            continue;
        }
        let mut beside: Vec<_> = lines.iter().filter(|l| {
            l.glyphs.len() >= 8 && l.rect[0] >= g.rect[2] - 0.3 * body && l.rect[0] <= g.rect[2] + 1.5 * body
                && l.rect[2] > g.rect[2] + 4.0 * body
                && l.baseline > g.rect[1] + 0.4 * body && l.baseline <= g.baseline + 0.5 * body
        }).collect();
        beside.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
        if !(2..=8).contains(&beside.len())
            || beside[0].baseline > g.rect[1] + 2.0 * body
            || (beside.last().unwrap().baseline - g.baseline).abs() > 0.6 * body
            || beside.windows(2).any(|pair| {
                let pitch = pair[1].baseline - pair[0].baseline;
                !(0.85 * body..=1.8 * body).contains(&pitch) || (pair[1].rect[0] - pair[0].rect[0]).abs() > 0.5 * body
            })
        {
            continue;
        }
        caps.push((i, DropCap { glyph: g.clone(), first_baseline: beside[0].baseline, text_left: beside[0].rect[0], lines: beside.len() }));
    }
    caps
}

/// Keep cap frames out of body analysis, and page fills out of figures.
fn flowing_items(content: &PageContent) -> (Vec<Item>, Vec<DropCap>) {
    let candidates = drop_caps(content);
    let mut analysed = content.clone();
    let mut index = 0;
    analysed.glyphs.retain(|_| {
        let keep = !candidates.iter().any(|(i, _)| *i == index);
        index += 1;
        keep
    });
    let area = content.size[0] * content.size[1];
    analysed.graphics.retain(|g| {
        matches!(g.kind, super::GraphicKind::Image) || (g.rect[2] - g.rect[0]) * (g.rect[3] - g.rect[1]) < 0.5 * area
    });
    let mut items = layout::analyse(&analysed, Options { invisible: true, figures: true });
    let mut caps = Vec::new();
    let mut restored = false;
    for (_, cap) in candidates {
        if items.iter().any(|item| matches!(item, Item::Para(p) if cap.starts(p))) {
            caps.push(cap);
        } else {
            // A figure or an unusual reading order: preserve the letter
            // as ordinary text rather than silently dropping it.
            analysed.glyphs.push(cap.glyph);
            restored = true;
        }
    }
    if restored {
        items = layout::analyse(&analysed, Options { invisible: true, figures: true });
    }
    // Image-only pages (including full-page scans) still need their image.
    if items.is_empty() && !content.graphics.is_empty() {
        let [w, h] = content.size;
        items.push(Item::Figure(layout::Figure { rect: [0.0, 0.0, w, h], column: [0.0, w], image: None }));
    }
    (toc::separate(items), caps)
}

/// What writing the blocks needs: the fonts and the drawings' numbers.
struct Out<'a> {
    fonts: &'a [FontInfo],
    lookup: &'a mut crate::fonts::Fonts,
    faces: &'a mut Faces,
    doc_pr: usize,
    scales: HashMap<u16, i64>,
    margin_left: f32,
    textboxes: docx::TextBoxes,
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
    if let Block::Grid(table)=block {
        if column_break {
            // A table has no run to carry the break: a paragraph a point
            // high carries it, and the table starts the next column.
            x.push_str(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr><w:r><w:rPr><w:sz w:val="2"/></w:rPr><w:br w:type="column"/></w:r></w:p>"#);
        }
        let before=(table.rect[1]-prev.bottom.unwrap_or(table.rect[1])).max(0.0);
        if before>1.0 {let _=write!(x,r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="{}" w:lineRule="exact"/></w:pPr></w:p>"#,tw(before));}
        forms::write(x,table,out.margin_left,out);
        *prev=Prev {baseline:None,bottom:Some(table.rect[3])};
        return;
    }
    if let Block::Para(p) = block
        && let Some(cap) = &p.drop_cap
    {
        drop_cap_xml(x, cap, p, para_before(p, prev), out);
    }
    x.push_str("<w:p><w:pPr>");
    match block {
        Block::Grid(_)=>unreachable!(),
        Block::Para(p) => {
            if let Some(n) = level(p) {
                let _ = write!(x, r#"<w:pStyle w:val="Heading{n}"/>"#);
            }
            if p.rules.iter().any(Option::is_some) {
                x.push_str("<w:pBdr>");
                for (side, rule) in ["top", "bottom"].iter().zip(&p.rules) {
                    if let Some(r) = rule {
                        let [cr, cg, cb] = r.color;
                        let _ = write!(x, r#"<w:{side} w:val="single" w:sz="{}" w:space="{}" w:color="{cr:02X}{cg:02X}{cb:02X}"/>"#, ((r.width * 8.0).round() as i32).clamp(2, 96), (r.space.round() as i32).clamp(0, 31));
                    }
                }
                x.push_str("</w:pBdr>");
            }
            if !p.tabs.is_empty() {
                x.push_str("<w:tabs>");
                for tab in &p.tabs {
                    let _ = write!(x, r#"<w:tab w:val="{}" w:pos="{}"{}/>"#, if tab.right { "right" } else { "left" }, tw(tab.pos.clamp(0.0, col_w)), if tab.leader { r#" w:leader="dot""# } else { "" });
                }
                x.push_str("</w:tabs>");
            }
            let before = para_before(p, prev);
            let _ = write!(x, r#"<w:spacing w:before="{}" w:after="0""#, tw(before.clamp(0.0, 400.0)));
            if p.tabs.iter().any(|t| t.leader) {
                let _ = write!(x, r#" w:line="{}" w:lineRule="exact""#, tw(p.pitch));
            } else if p.lines >= 2 && p.pitch >= 0.9 * p.size {
                let _ = write!(x, r#" w:line="{}" w:lineRule="atLeast""#, tw(p.pitch));
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
            for picture in &p.floating { wrapping::write(x, picture, p.top, before, out); }
            paragraph_runs(x, p, out);
            // A rule below makes the paragraph that much taller.
            let below = p.rules[1].map_or(0.0, |r| r.space + r.width);
            *prev = Prev { baseline: Some(p.last_baseline + below), bottom: Some(p.bottom + below) };
        }
        Block::Picture { rel, width, height, align, left, top, bottom, region } => {
            let k = (col_w / width).min(1.0);
            let (cx, cy) = (emu(width * k), emu(height * k));
            let before = paragraph_gap(prev.bottom.map_or(0.0, |b| top - b), 10.0);
            out.doc_pr += 1;
            let n = out.doc_pr;
            let _ = write!(x, r#"<w:spacing w:before="{}" w:after="0"/>"#, tw(before.clamp(0.0, 400.0)));
            if *align == Align::Left && *left > 0.0 {
                let _ = write!(x, r#"<w:ind w:left="{}"/>"#, tw(left.min(col_w - width * k).max(0.0)));
            }
            let _ = write!(x, r#"<w:jc w:val="{}"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr>"#, jc(*align));
            if column_break {
                x.push_str(r#"<w:r><w:br w:type="column"/></w:r>"#);
            }
            let _ = write!(
                x,
                r#"<w:r><w:rPr><w:sz w:val="2"/></w:rPr><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Picture {n}"/>"#
            );
            overlay::graphic(x, *rel, n, [cx,cy], region, out);
            x.push_str("</wp:inline></w:drawing></w:r>");
            *prev = Prev { baseline: None, bottom: Some(*bottom) };
        }
    }
    x.push_str("</w:p>");
}

/// Word's native initial-letter frame wraps only its opening body lines
/// and follows the paragraph when earlier text is edited. All body runs
/// remain ordinary text in the main story, at their original body size.
fn drop_cap_xml(x: &mut String, cap: &DropCap, p: &DocPara, before: f32, out: &mut Out) {
    let width = (cap.text_left - cap.glyph.x).max(1.0);
    let height = p.pitch * cap.lines as f32;
    // The frame and the body need the same preceding gap. Otherwise only
    // the body moves down, leaving the cap above its opening lines.
    let _ = write!(x, r#"<w:p><w:pPr><w:keepNext/><w:framePr w:dropCap="drop" w:lines="{}" w:w="{}" w:hSpace="0" w:wrap="around" w:vAnchor="text" w:hAnchor="text"/><w:spacing w:before="{}" w:after="0" w:line="{}" w:lineRule="exact"/><w:ind w:left="{}"/><w:textAlignment w:val="baseline"/></w:pPr>"#,
        cap.lines, tw(width), tw(before), tw(height), tw(p.left));
    let span = Span { text: cap.glyph.ch.to_string(), font: cap.glyph.font, size: cap.glyph.size, color: cap.glyph.color, vert: Vert::Base };
    // An exact frame's baseline is at 4/5 of its line box, as in the
    // fixed-layout writer. Retain the PDF's baseline drop against the body.
    let shift = (cap.glyph.baseline - cap.first_baseline) - 0.8 * (height - p.pitch);
    run_xml(x, &span, &out.faces.get(span.font, out.fonts, out.lookup), (-2.0 * shift).round() as i64);
    x.push_str("</w:p>");
}

fn para_before(p: &DocPara, prev: &Prev) -> f32 {
    let contents = p.tabs.iter().any(|t| t.leader);
    let height = if contents || p.lines >= 2 { p.pitch } else { 1.17 * p.size };
    let gap = match (prev.baseline, prev.bottom) {
        (Some(b), _) => p.first_baseline - b - height,
        (None, Some(bottom)) => p.top - bottom,
        (None, None) => 0.0,
    };
    // A rule above takes its room out of the gap.
    let gap = gap - p.rules[0].map_or(0.0, |r| r.space + r.width);
    if contents { gap.max(0.0) } else { paragraph_gap(gap, p.size) }
}

/// Preserve deliberate white space, including epigraphs and footnotes.
fn paragraph_gap(gap: f32, _size: f32) -> f32 {
    gap.clamp(0.0, 400.0)
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
        edges.push(if t.side_heads { t.columns[i][0] - CELL_PAD } else { (t.columns[i - 1][1] + t.columns[i][0]) / 2.0 });
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
        // Text at the top of its cell: the row starts where its text does.
        ys.push(if t.side_heads { rows[r].0[0] - 1.0 } else { (rows[r - 1].0[1] + rows[r].0[0]) / 2.0 });
    }
    ys.push(rows[n - 1].0[1] + pad);
    if t.side_heads {
        ys[0] = rows[0].0[0] - 1.0;
        ys[n] = rows[n - 1].0[1] + 1.0;
    }
    let before = paragraph_gap(ys[0] - above, 10.0);
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
            let _ = write!(x, r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/><w:vAlign w:val="{}"/></w:tcPr>"#, tw(widths[c]), if t.side_heads { "top" } else { "center" });
            if blocks.is_empty() {
                x.push_str("<w:p/>");
            }
            let mut prev = Prev::default();
            // A table in the cell is indented from the cell's text, not
            // from the page's margin.
            let margin = std::mem::replace(&mut out.margin_left, edges[c] + CELL_PAD);
            for block in blocks {
                block_xml(x, block, &mut prev, widths[c] - 2.0 * CELL_PAD, false, level, out);
            }
            out.margin_left = margin;
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
    // Text boxes contain nested paragraphs. The section belongs to the
    // surrounding body paragraph, never the last label inside a drawing.
    let mut depth = 0usize;
    let start = body.rmatch_indices('<').find_map(|(at, _)| {
        let tag = &body[at..];
        if tag.starts_with("</w:p>") { depth += 1; }
        if tag.starts_with("<w:p>") {
            if depth <= 1 { return Some(at); }
            depth -= 1;
        }
        None
    });
    let Some(start) = start else {
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
fn doc_para(p: &layout::Para, cap: Option<&DropCap>) -> DocPara {
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
    let spans = layout::flowing_spans(p, "\t");
    let mut para = DocPara {
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
        drop_cap: None,
        artwork: Vec::new(),
        floating: Vec::new(),
        rules: [None, None],
    };
    let centre = (col[0]+col[1])/2.0;
    if n <= 3 && p.marker.is_none() && p.rect[2]-p.rect[0] < 0.8*(col[1]-col[0])
        && p.lines.iter().all(|l| ((l.rect[0]+l.rect[2])/2.0-centre).abs() < 0.7*p.size)
    {
        para.align = Align::Center;
        para.left = (p.rect[0]-col[0]-0.25*p.size).max(0.0);
        para.right = (col[1]-p.rect[2]-0.25*p.size).max(0.0);
        para.first = 0.0;
    }
    if p.marker.is_some() {
        para.tabs.push(Tab { pos: para.left, right: false, leader: false });
    }
    if let Some(cap) = cap {
        // The initial lines were inset to wrap the PDF's cap. Word's
        // separate initial-letter frame supplies their inset now.
        para.left = (cap.glyph.x - p.column[0]).max(0.0);
        para.first = 0.0;
        let mut lines = p.lines.clone();
        for line in lines.iter_mut().take(cap.lines) {
            line.rect[0] = cap.glyph.x;
        }
        para.align = layout::alignment(&lines, p.column, p.size);
        para.drop_cap = Some(cap.clone());
    }
    if let Some(spans) = toc::spans(p) {
        para.spans = spans;
        para.align = Align::Left;
        para.left = (p.rect[0] - col[0]).max(0.0);
        para.first = 0.0;
        para.right = 0.0;
        para.tabs.push(Tab { pos: p.rect[2] - col[0], right: true, leader: true });
    }
    para
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
fn run_xml(x: &mut String, span: &Span, face: &super::Face, position: i64) {
    scaled_run_xml(x, span, face, position, 100);
}

fn scaled_run_xml(x: &mut String, span: &Span, face: &super::Face, position: i64, scale: i64) {
    let face = face.for_text(&span.text);
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
    if scale != 100 {
        let _ = write!(rpr, r#"<w:w w:val="{scale}"/>"#);
    }
    let _ = write!(rpr, r#"<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>"#);
    if position != 0 {
        let _ = write!(rpr, r#"<w:position w:val="{position}"/>"#);
    }
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
        for (j, text) in part.split('\u{ad}').enumerate() {
            if j > 0 { let _ = write!(x, "<w:r>{rpr}<w:softHyphen/></w:r>"); }
            if !text.is_empty() {
                let _ = write!(x, r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#, esc(text));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Graphic, GraphicKind};

    #[test]
    fn section_break_follows_the_body_instead_of_a_nested_textbox() {
        let mut body = String::from("<w:p><w:pPr/></w:p><w:p><w:pPr></w:pPr><w:r><w:drawing><w:txbxContent><w:p><w:pPr></w:pPr><w:r><w:t>Label</w:t></w:r></w:p></w:txbxContent></w:drawing></w:r></w:p>");
        close_section(&mut body,"<w:sectPr/>");
        assert!(body.contains("<w:pPr><w:sectPr/></w:pPr><w:r><w:drawing>"));
        assert!(!body.contains("<w:txbxContent><w:p><w:pPr><w:sectPr/>"));
    }

    #[test]
    fn outlined_pages_are_pictures_but_visible_and_ocr_text_stay_editable() {
        let mut page = PageContent { size: [600.0, 800.0], ..Default::default() };
        assert!(!graphics_page(&page));
        page.graphics.push(Graphic { rect: [70.0, 100.0, 500.0, 650.0], kind: GraphicKind::Path { curved: true }, object: None });
        assert!(graphics_page(&page));
        page.glyphs = initial_page().glyphs;
        assert!(!graphics_page(&page));
        for g in &mut page.glyphs {
            g.invisible = true;
        }
        assert!(!graphics_page(&page));
        assert_eq!(picture_page_size([467.717, 660.472]), [467.717, 660.472]);
        assert_eq!(picture_page_size([3168.0, 1584.0]), [1584.0, 792.0]);
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_outlined_page_is_one_image_and_the_next_page_remains_editable() {
        let stream = |text: &str| format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len());
        let pdf = crate::pdfium::tests::write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 450 300] /Contents 5 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 450] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
            stream("0 .7 .3 rg 30 200 70 50 re f 120 200 70 50 re f 210 200 70 50 re f"),
            stream("BT /F1 12 Tf 40 400 Td (Ordinary body text remains editable) Tj ET 0 0 1 rg 120 180 30 30 re f 150 180 30 30 re f 135 210 30 30 re f"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ]);
        let input = std::env::temp_dir().join(format!("metalpdf_vector_cover_{}.pdf", std::process::id()));
        let output = input.with_extension("docx");
        std::fs::write(&input, pdf).unwrap();
        assert_eq!(crate::export::run_paragraphs(&input, &output, None).unwrap(), 2);
        let bytes = std::fs::read(&output).unwrap();
        let read = |name| crate::export::zip::read_entry(&bytes, name).unwrap();
        let xml = String::from_utf8(read("word/document.xml")).unwrap();
        assert_eq!(xml.matches("<wp:anchor ").count(), 1);
        assert_eq!(xml.matches("<wp:inline ").count(), 1);
        assert!(xml.contains(r#"<wp:docPr id="1" name="Page image 1""#));
        assert!(xml.contains(r#"<wp:docPr id="2" name="Picture 2""#));
        assert!(xml.contains(r#"<w:pgSz w:w="9000" w:h="6000" w:orient="landscape""#));
        assert!(xml.contains(r#"<w:pgMar w:top="0" w:right="0" w:bottom="0" w:left="0""#));
        assert!(xml.contains("Ordinary body text remains editable"));
        assert!(!xml.contains("w:framePr"));
        let png = read("word/media/image1.png");
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
        assert_eq!((decoder.info().width, decoder.info().height), (1250, 833));
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        let frame = decoder.next_frame(&mut pixels).unwrap();
        assert!(pixels[..frame.buffer_size()].iter().any(|&v| v < 200));
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    fn initial_page() -> PageContent {
        let mut glyphs = vec![Glyph { source: -1, ch: 'O', rect: [55.0, 80.0, 95.0, 140.0], x: 55.0, baseline: 128.0, size: 51.0, font: 0, color: [0, 176, 80], space_before: false, invisible: false }];
        for (i, text) in [
            "nce upon a time the computers changed the world,",
            "opening new possibilities for the people who used",
            "them. This ordinary paragraph keeps its normal",
            "type size and reading order when exported to Word.",
            "Every line after the initial is ordinary body text,",
            "and the final line ends here.",
            "A separate paragraph follows after a gap.",
        ].iter().enumerate() {
            let left = if i < 3 { 95.0 } else { 55.0 };
            let right = if i == 5 { 250.0 } else { 395.0 };
            let baseline = if i == 6 { 190.0 } else { 100.0 + 14.0 * i as f32 };
            let advance = (right - left) / text.len() as f32;
            glyphs.extend(text.chars().enumerate().map(|(j, ch)| {
                let x = left + advance * j as f32;
                Glyph { source: -1, ch, rect: [x, baseline - 10.4, x + advance, baseline + 2.5], x, baseline, size: 11.0, font: 0, color: [0; 3], space_before: false, invisible: false }
            }));
        }
        PageContent { size: [468.0, 660.0], glyphs, ..Default::default() }
    }

    #[test]
    fn a_drop_cap_does_not_change_body_size_or_reading_order() {
        let (items, caps) = flowing_items(&initial_page());
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].lines, 3);
        let Item::Para(p) = &items[0] else { panic!() };
        assert!(caps[0].starts(p));
        assert_eq!(p.lines.len(), 6);
        let para = doc_para(p, Some(&caps[0]));
        assert_eq!((para.size, para.left, para.first), (11.0, 0.0, 0.0));
        assert_eq!(para.align, Align::Justify);
        assert!(para.spans.iter().all(|s| s.size == 11.0 && s.vert == Vert::Base));
        let text: String = para.spans.iter().map(|s| s.text.as_str()).collect();
        assert!(text.starts_with("nce upon a time the computers changed the world, opening new possibilities"));
        assert!(text.ends_with("and the final line ends here."));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn only_the_initial_is_framed_and_shares_the_body_gap() {
        let (items, caps) = flowing_items(&initial_page());
        let Item::Para(p) = &items[0] else { panic!() };
        let fonts = [FontInfo { name: "PetersburgC".into(), ..Default::default() }];
        let mut lookup = crate::fonts::Fonts::default();
        let mut faces = Faces::default();
        let mut out = Out { fonts: &fonts, lookup: &mut lookup, faces: &mut faces, doc_pr: 0, scales: HashMap::new(), margin_left:0.0, textboxes:docx::TextBoxes::new() };
        let mut xml = String::new();
        let mut prev = Prev { baseline: Some(60.0), bottom: Some(62.0) };
        block_xml(&mut xml, &Block::Para(doc_para(p, Some(&caps[0]))), &mut prev, 340.0, false, &|_| None, &mut out);
        assert_eq!(xml.matches("<w:framePr").count(), 1);
        assert!(xml.contains(r#"w:dropCap="drop" w:lines="3" w:w="800""#));
        let before = tw(para_before(&doc_para(p, Some(&caps[0])), &Prev { baseline: Some(60.0), bottom: Some(62.0) }));
        assert_eq!(xml.matches(&format!(r#"w:before="{before}""#)).count(), 2);
        assert!(xml.contains(r#"<w:sz w:val="102""#));
        let body = xml.split_once("</w:p>").unwrap().1;
        assert!(!body.contains("framePr") && !body.contains("vertAlign"));
        assert!(!body.contains(r#"<w:sz w:val="102""#));
        assert!(body.contains(r#"<w:sz w:val="22""#));
        assert_eq!(prev.baseline, Some(170.0));
    }

    #[test]
    fn a_rule_under_a_paragraph_is_its_border_and_takes_its_room() {
        let (items, _) = flowing_items(&initial_page());
        let Item::Para(p) = &items[1] else { panic!() };
        let fonts = [FontInfo { name: "Arial".into(), ..Default::default() }];
        let mut lookup = crate::fonts::Fonts::default();
        let mut faces = Faces::default();
        let mut out = Out { fonts: &fonts, lookup: &mut lookup, faces: &mut faces, doc_pr: 0, scales: HashMap::new(), margin_left: 0.0, textboxes: docx::TextBoxes::new() };
        let mut para = doc_para(p, None);
        para.rules[1] = Some(ParaRule { color: [0xFF, 0xDD, 0x2D], width: 1.0, space: 8.0 });
        let (baseline, bottom) = (para.last_baseline, para.bottom);
        let mut xml = String::new();
        let mut prev = Prev::default();
        block_xml(&mut xml, &Block::Para(para), &mut prev, 340.0, false, &|_| None, &mut out);
        assert!(xml.contains(r#"<w:pBdr><w:bottom w:val="single" w:sz="8" w:space="8" w:color="FFDD2D"/></w:pBdr>"#), "{xml}");
        // The pBdr goes before the spacing, as WordprocessingML orders it.
        assert!(xml.find("<w:pBdr>").unwrap() < xml.find("<w:spacing").unwrap());
        assert_eq!((prev.baseline, prev.bottom), (Some(baseline + 9.0), Some(bottom + 9.0)));
    }

    #[test]
    fn a_large_word_or_raised_initial_is_not_a_drop_cap() {
        let mut page = initial_page();
        let mut next = page.glyphs[0].clone();
        next.ch = 'N';
        next.x = 96.0;
        next.rect[0] = 96.0;
        next.rect[2] = 136.0;
        page.glyphs.push(next);
        assert!(drop_caps(&page).is_empty());
        page.glyphs.pop();
        page.glyphs[0].baseline = 100.0;
        assert!(drop_caps(&page).is_empty());
    }

    #[test]
    fn contents_use_native_dotted_tabs_and_preserve_the_opening_gap() {
        let glyphs = "1.1. Ideas................15".chars().enumerate().map(|(i, ch)| {
            let x = 95.0 + i as f32 * 5.0;
            Glyph { source: -1, ch, rect: [x, 181.0, x + 5.0, 193.0], x, baseline: 191.0, size: 9.0, font: 0, color: [0; 3], space_before: false, invisible: false }
        }).collect();
        let page = PageContent { size: [468.0, 660.0], glyphs, ..Default::default() };
        let (items, _) = flowing_items(&page);
        let Item::Para(p) = &items[0] else { panic!() };
        let para = doc_para(p, None);
        let text: String = para.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "1.1. Ideas\t15");
        assert_eq!(para.first, 0.0);
        assert_eq!(para.align, Align::Left);
        let fonts = [FontInfo { name: "Tahoma".into(), ..Default::default() }];
        let mut lookup = crate::fonts::Fonts::default();
        let mut faces = Faces::default();
        let mut out = Out { fonts: &fonts, lookup: &mut lookup, faces: &mut faces, doc_pr: 0, scales: HashMap::new(), margin_left:0.0, textboxes:docx::TextBoxes::new() };
        let mut xml = String::new();
        let mut prev = Prev { baseline: Some(94.0), bottom: Some(100.0) };
        assert!(para_before(&para, &prev) > 80.0);
        block_xml(&mut xml, &Block::Para(para), &mut prev, 340.0, false, &|_| None, &mut out);
        assert!(xml.contains(r#"w:val="right""#) && xml.contains(r#"w:leader="dot""#));
        assert_eq!(xml.matches("<w:tab/>").count(), 1);
        assert!(xml.contains(r#"w:lineRule="exact""#));
        assert!(!xml.contains("........") && !xml.contains("framePr"));
    }

    #[test]
    fn page_decoration_does_not_turn_editable_text_into_a_picture() {
        let glyphs = "Ordinary paragraph".chars().enumerate().map(|(i, ch)| {
            let x = 50.0 + 6.0 * i as f32;
            Glyph { source: -1, ch, rect: [x, 80.0, x + 6.0, 92.0], x, baseline: 90.0, size: 10.0, font: 0, color: [0; 3], space_before: false, invisible: false }
        }).collect();
        let page = PageContent { size: [600.0, 800.0], glyphs, graphics: vec![Graphic { rect: [30.0, 40.0, 570.0, 760.0], kind: GraphicKind::Path { curved: true }, object: None }], turns: 0, ..Default::default() };
        let (items, _) = flowing_items(&page);
        assert!(items.iter().all(|item| matches!(item, Item::Para(_))));
        let Item::Para(p) = &items[0] else { panic!() };
        assert_eq!(p.lines[0].text(), "Ordinary paragraph");
    }

    #[test]
    fn an_image_only_page_is_not_lost_in_paragraph_mode() {
        let page = PageContent { size: [600.0, 800.0], graphics: vec![Graphic { rect: [0.0, 0.0, 600.0, 800.0], kind: GraphicKind::Image, object: Some(0) }], ..Default::default() };
        let (items, _) = flowing_items(&page);
        assert_eq!(items.len(), 1);
        let Item::Figure(f) = &items[0] else { panic!() };
        assert_eq!(f.rect, [0.0, 0.0, 600.0, 800.0]);
    }
    #[test]
    fn replacement_metrics_and_optional_breaks_remain_editable_word_runs() {
        let span = Span {text:"para\u{ad}graph".into(),font:0,size:11.0,color:[0;3],vert:Vert::Base};
        let face = super::super::Face {family:"Arial".into(),bold:false,italic:false};
        let mut xml = String::new();
        scaled_run_xml(&mut xml,&span,&face,0,88);
        assert!(xml.contains(r#"<w:w w:val="88"/>"#));
        assert!(xml.contains("<w:softHyphen/>"));
        assert!(xml.contains(">para</w:t>") && xml.contains(">graph</w:t>"));
        assert!(!xml.contains("drawing") && !xml.contains("fitText"));
    }

}
