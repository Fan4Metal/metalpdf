//! Wide shaded form tables have explicit cell geometry even without rules.
//! Keep their values editable and leave page furniture outside the text flow.
use crate::export::{FontInfo, Glyph, GraphicKind, PageContent, layout};

type Rect = [f32; 4];

/// A cell's rule: its colour and thickness in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Border {
    pub color: [u8; 3],
    pub width: f32,
}

/// A cell's place in a cell merged down (Word's `vMerge`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Merge {
    None,
    /// The first band of a merged cell: its text.
    Restart,
    /// A band below it: empty.
    Continue,
}

#[derive(Clone, Debug)]
pub(super) struct Cell {
    pub first: usize,
    pub end: usize,
    pub rect: Rect,
    pub glyphs: Vec<Glyph>,
    pub fill: [u8; 3],
    /// Top, left, bottom, right; `None`: no rule drawn there.
    pub borders: [Option<Border>; 4],
    pub merge: Merge,
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub top: f32,
    pub bottom: f32,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug)]
pub(super) struct Table {
    pub rect: Rect,
    pub edges: Vec<f32>,
    pub rows: Vec<Row>,
}

pub(super) struct Form {
    pub table: Table,
    pub decorations: Vec<Rect>,
}

fn inside(g: &Glyph, r: Rect) -> bool {
    let x = (g.rect[0] + g.rect[2]) / 2.0;
    let y = (g.rect[1] + g.rect[3]) / 2.0;
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

fn covered(r: Rect, area: Rect) -> bool {
    r[0] >= area[0] - 1.0 && r[2] <= area[2] + 1.0 && r[1] >= area[1] - 1.0 && r[3] <= area[3] + 1.0
}

fn distinct(v: &mut Vec<f32>) {
    v.sort_by(f32::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 0.75);
}

/// Require several adjoining cells covering nearly the whole page width,
/// and at least two rows. Ordinary illustrations and book tables are left
/// to the general layout analyser.
pub(super) fn detect(content: &PageContent, fonts: &[FontInfo]) -> Option<Form> {
    let [w, h] = content.size;
    let mut rectangles: Vec<Rect> = content
        .graphics
        .iter()
        .filter(|g| {
            matches!(g.kind, GraphicKind::Path { .. })
                && g.rect[2] - g.rect[0] > 12.0
                && g.rect[3] - g.rect[1] > 5.0
                && g.rect[3] - g.rect[1] < 0.2 * h
                && g.rect[1] > 0.12 * h
                && g.rect[3] < 0.75 * h
        })
        .map(|g| g.rect)
        .collect();
    rectangles.sort_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])));
    let mut bands: Vec<Vec<Rect>> = Vec::new();
    for r in &rectangles {
        if let Some(row) = bands.last_mut().filter(|row| (row[0][1] - r[1]).abs() < 0.75 && (row[0][3] - r[3]).abs() < 0.75) {
            row.push(*r);
        } else {
            bands.push(vec![*r]);
        }
    }
    bands.retain(|row| row.len() >= 3 && row.last().unwrap()[2] - row[0][0] > 0.85 * w && row.windows(2).all(|p| (p[0][2] - p[1][0]).abs() < 1.0));
    let first = bands.first()?;
    let mut edges = vec![first[0][0]];
    edges.extend(first.iter().map(|r| r[2]));
    bands.retain(|row| row.len() + 1 == edges.len() && row.iter().enumerate().all(|(i, r)| (r[0] - edges[i]).abs() < 1.0 && (r[2] - edges[i + 1]).abs() < 1.0));
    if bands.len() < 2 {
        return None;
    }
    if bands[0].iter().filter(|r| content.glyphs.iter().filter(|g| inside(g, **r)).count() >= 2).count() < 3 {
        return None;
    }
    let left = edges[0];
    let right = *edges.last()?;
    let top = bands[0][0][1];
    let mut bottom = bands.last()?[0][3];
    if bottom - top > 0.5 * h {
        return None;
    }
    let mut ys = vec![top];
    for r in &rectangles {
        if r[1] >= top - 0.75 && r[1] <= bottom && r[0] >= left - 1.0 && r[2] <= right + 1.0 {
            ys.extend([r[1], r[3]]);
            bottom = bottom.max(r[3]);
        }
    }
    let in_width = |g: &Glyph| g.rect[0] >= left - 1.0 && g.rect[2] <= right + 1.0;
    let tail = layout::lines(content.glyphs.iter().filter(|g| in_width(g) && g.rect[1] >= bottom - 0.5 && g.rect[1] < bottom + 72.0).cloned().collect());
    let mut end = bottom;
    for line in &tail {
        if line.rect[1] - end > 1.5 * line.size {
            break;
        }
        end = end.max(line.rect[3]);
    }
    if end > bottom + 1.0 {
        bottom = end + 2.0;
        ys.push(bottom);
    }
    let rect = [left, top, right, bottom];
    if content.graphics.iter().any(|g| g.kind == GraphicKind::Image && g.rect[0] < right && g.rect[2] > left && g.rect[1] < bottom && g.rect[3] > top) {
        return None;
    }
    // A full-width heading between shaded rows is a merged cell, not a
    // value overflowing from the name column into the result column.
    let lines = layout::lines(content.glyphs.iter().filter(|g| inside(g, rect)).cloned().collect());
    let headings: Vec<_> = lines
        .iter()
        .filter(|line| {
            line.rect[0] < left + 8.0
                && line.rect[2] > edges[1] + 8.0
                && line.glyphs.iter().all(|g| fonts.get(g.font as usize).is_some_and(|f| f.weight >= 600 || f.faux_bold || f.name.to_ascii_lowercase().contains("bold")))
                && !lines.iter().any(|other| other.rect[0] > line.rect[2] + 4.0 && (other.baseline - line.baseline).abs() < line.size * 0.2)
        })
        .collect();
    for heading in &headings {
        ys.push((heading.rect[3] + 2.0).min(bottom));
    }
    distinct(&mut ys);
    // An unshaded band can hold a row of values and, under it, notes
    // across the whole table: a row of their own each, so that the notes
    // span the columns and the values keep theirs.
    let mut parts = Vec::new();
    for y in ys.windows(2) {
        let band = [left, y[0], right, y[1]];
        let mut lines = layout::lines(content.glyphs.iter().filter(|g| inside(g, band)).cloned().collect());
        lines.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
        // A short last line of a note goes with it: under a line across,
        // at its left edge, a line's pitch below.
        let mut across: Vec<bool> = lines.iter().map(|l| edges[1..edges.len() - 1].iter().any(|&e| runs_across(l, e))).collect();
        for i in 1..lines.len() {
            let (a, b) = (&lines[i - 1], &lines[i]);
            if across[i - 1] && !across[i] && (b.rect[0] - a.rect[0]).abs() < a.size && b.baseline - a.baseline < 1.6 * a.size.max(b.size) {
                across[i] = true;
            }
        }
        for (i, pair) in lines.windows(2).enumerate() {
            if across[i] != across[i + 1] && pair[1].rect[1] >= pair[0].rect[3] - 0.5 {
                parts.push((pair[0].rect[3] + pair[1].rect[1]) / 2.0);
            }
        }
    }
    ys.extend(parts);
    distinct(&mut ys);
    let rows = ys
        .windows(2)
        .filter(|y| y[1] - y[0] > 1.0)
        .map(|y| {
            let row_rect = [left, y[0], right, y[1]];
            let glyphs: Vec<_> = content.glyphs.iter().filter(|g| inside(g, row_rect)).cloned().collect();
            let merged = headings.iter().any(|l| l.baseline >= y[0] && l.baseline < y[1])
                || rectangles.iter().any(|r| (r[0] - left).abs() < 1.0 && (r[2] - right).abs() < 1.0 && r[1] <= y[0] + 0.75 && r[3] >= y[1] - 0.75);
            let spans: Vec<_> = if merged { vec![(0, edges.len() - 1)] } else { spans_of(&edges, &glyphs) };
            let cells = spans
                .into_iter()
                .map(|(first, end)| {
                    let r = [edges[first], y[0], edges[end], y[1]];
                    Cell { first, end, rect: r, glyphs: glyphs.iter().filter(|g| inside(g, r)).cloned().collect(), fill: [255; 3], borders: [None; 4], merge: Merge::None }
                })
                .collect();
            Row { top: y[0], bottom: y[1], cells }
        })
        .collect();
    let mut decorations = Vec::new();
    // A bordered metadata panel above the result grid, even when its outer
    // rules consist of small path fragments, is an independent composition.
    let rules: Vec<_> = content.graphics.iter().filter(|g| matches!(g.kind, GraphicKind::Path { .. }) && g.rect[3] < top && g.rect[1] > 0.1 * h).map(|g| g.rect).collect();
    if let Some(panel) = rules.into_iter().reduce(layout::union)
        && panel[2] - panel[0] > 0.85 * w
        && panel[3] - panel[1] > 25.0
        && panel[3] - panel[1] < 0.25 * h
    {
        decorations.push([panel[0] - 0.5, panel[1] - 0.5, panel[2] + 0.5, panel[3] + 0.5]);
    }
    let body_top = decorations.first().map_or(top, |r| r[1]);
    if content.graphics.iter().any(|g| g.kind == GraphicKind::Image && g.rect[3] < body_top - 8.0) {
        let mut upper: Vec<_> = content.graphics.iter().map(|g| g.rect).filter(|r| r[3] < body_top - 8.0).collect();
        upper.extend(content.glyphs.iter().filter(|g| g.rect[3] < body_top - 8.0).map(|g| g.rect));
        if let Some(r) = upper.into_iter().reduce(layout::union) {
            decorations.push(r);
        }
    }
    let footer: Vec<_> =
        content.graphics.iter().map(|g| g.rect).filter(|r| r[1] > 0.75 * h).chain(content.glyphs.iter().filter(|g| g.rect[1] > 0.75 * h).map(|g| g.rect)).collect();
    if let Some(r) = footer.into_iter().reduce(layout::union)
        && r[1] - bottom > 0.1 * h
        && content.graphics.iter().any(|g| g.kind == GraphicKind::Image && g.rect[1] >= r[1] - 1.0)
    {
        decorations.push(r);
    }
    Some(Form { table: Table { rect, edges, rows }, decorations })
}

/// Whether the text of `line` runs on over `x`: two neighbouring
/// characters on either side, no wider apart than a word space. Text in
/// two cells next to each other has the cells' padding between it.
pub(super) fn runs_across(line: &layout::Line, x: f32) -> bool {
    line.glyphs.windows(2).any(|p| {
        let (a, b) = (&p[0], &p[1]);
        (a.rect[0] + a.rect[2]) / 2.0 < x && (b.rect[0] + b.rect[2]) / 2.0 > x && b.rect[0] - a.rect[2] < 0.5 * a.size.max(b.size)
    })
}

/// A row's cells: one a column, joined where its text runs over the
/// columns' edge (a note under the results across the whole table).
fn spans_of(edges: &[f32], glyphs: &[Glyph]) -> Vec<(usize, usize)> {
    let lines = layout::lines(glyphs.to_vec());
    let mut spans = Vec::new();
    let mut start = 0;
    for (j, &edge) in edges.iter().enumerate().take(edges.len() - 1).skip(1) {
        if !lines.iter().any(|l| runs_across(l, edge)) {
            spans.push((start, j));
            start = j;
        }
    }
    spans.push((start, edges.len() - 1));
    spans
}

pub(super) fn remove(content: &mut PageContent, form: &Form) {
    let regions = std::iter::once(&form.table.rect).chain(&form.decorations);
    for &r in regions {
        content.glyphs.retain(|g| !inside(g, r));
        content.graphics.retain(|g| !covered(g.rect, r));
        content.rules.retain(|g| !covered(g.rect, r));
    }
    // The synthetic figure participates in reading order, but the writer
    // replaces it with the editable table instead of rendering a picture.
    content.graphics.push(crate::export::Graphic { rect: form.table.rect, kind: GraphicKind::Image, object: None });
}

pub(super) fn write(x: &mut String, table: &Table, margin_left: f32, out: &mut super::Out) {
    use super::{doc_para, paragraph_runs, tw};
    use std::fmt::Write;
    let width = table.rect[2] - table.rect[0];
    let _ = write!(
        x,
        r#"<w:tbl><w:tblPr><w:tblW w:w="{}" w:type="dxa"/><w:tblInd w:w="{}" w:type="dxa"/><w:tblLayout w:type="fixed"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:left w:w="0" w:type="dxa"/><w:right w:w="0" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid>"#,
        tw(width),
        tw(table.rect[0] - margin_left)
    );
    for pair in table.edges.windows(2) {
        let _ = write!(x, r#"<w:gridCol w:w="{}"/>"#, tw(pair[1] - pair[0]));
    }
    x.push_str("</w:tblGrid>");
    for row in &table.rows {
        let _ = write!(x, r#"<w:tr><w:trPr><w:cantSplit/><w:trHeight w:val="{}" w:hRule="atLeast"/></w:trPr>"#, tw(row.bottom - row.top));
        for cell in &row.cells {
            let _ = write!(x, r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/>"#, tw(table.edges[cell.end] - table.edges[cell.first]));
            if cell.end - cell.first > 1 {
                let _ = write!(x, r#"<w:gridSpan w:val="{}"/>"#, cell.end - cell.first);
            }
            match cell.merge {
                Merge::Restart => x.push_str(r#"<w:vMerge w:val="restart"/>"#),
                Merge::Continue => x.push_str("<w:vMerge/>"),
                Merge::None => {}
            }
            if cell.borders.iter().any(Option::is_some) {
                x.push_str("<w:tcBorders>");
                for (side, border) in ["top", "left", "bottom", "right"].iter().zip(&cell.borders) {
                    match border {
                        Some(Border { color: [r, g, b], width }) => {
                            let _ = write!(x, r#"<w:{side} w:val="single" w:sz="{}" w:space="0" w:color="{r:02X}{g:02X}{b:02X}"/>"#, ((width * 8.0).round() as i64).clamp(2, 96));
                        }
                        None => {
                            let _ = write!(x, r#"<w:{side} w:val="nil"/>"#);
                        }
                    }
                }
                x.push_str("</w:tcBorders>");
            }
            if cell.fill != [255; 3] {
                let [r, g, b] = cell.fill;
                let _ = write!(x, r#"<w:shd w:val="clear" w:color="auto" w:fill="{r:02X}{g:02X}{b:02X}"/>"#);
            }
            x.push_str(r#"<w:vAlign w:val="top"/></w:tcPr>"#);
            if cell.merge == Merge::Continue {
                x.push_str(r#"<w:p><w:pPr><w:spacing w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr></w:p></w:tc>"#);
                continue;
            }
            let items = layout::analyse(
                &PageContent { size: [table.rect[2] + 1.0, table.rect[3] + 1.0], glyphs: cell.glyphs.clone(), ..Default::default() },
                layout::Options { invisible: true, figures: false },
            );
            let mut bottom = cell.rect[1];
            if items.is_empty() {
                x.push_str(r#"<w:p><w:pPr><w:spacing w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr></w:p>"#);
            }
            // Code (a monospaced font): every line a paragraph of its own,
            // where it starts; joined, a program's output (right-aligned
            // names, a value a line) ran together.
            let mono = {
                let n = cell.glyphs.iter().filter(|g| out.fonts.get(g.font as usize).is_some_and(|f| crate::fonts::style_of(&f.name, f.flags).mono)).count();
                n * 3 > cell.glyphs.len() * 2
            };
            // A monospaced face is 0.6 em a character: the narrowing that
            // keeps the cell's longest line on one line as in the PDF (the
            // size rounded to half points, 9.3 to 9.5, made a full line
            // wrap), for the cell's fonts while it is written.
            let saved: Vec<(u16, Option<i64>)> = if mono {
                let lines = layout::lines(cell.glyphs.clone());
                let squeeze = lines
                    .iter()
                    .filter_map(|l| {
                        let chars = l.glyphs.len() + l.glyphs.iter().skip(1).filter(|g| g.space_before).count();
                        let size = l.glyphs.iter().map(|g| (g.size * 2.0).round() / 2.0).fold(0.0, f32::max);
                        let room = cell.rect[2] - l.rect[0] - 1.0;
                        (chars > 0 && size > 0.0 && room > 0.0).then(|| room / (chars as f32 * 0.6 * size))
                    })
                    .fold(1.0f32, f32::min);
                let mut fonts: Vec<u16> = cell.glyphs.iter().map(|g| g.font).collect();
                fonts.sort_unstable();
                fonts.dedup();
                let percent = ((squeeze * 100.0).floor() as i64).clamp(50, 100);
                fonts
                    .into_iter()
                    .map(|f| {
                        let old = out.scales.get(&f).copied();
                        if percent < 100 {
                            out.scales.insert(f, old.map_or(percent, |o| o.min(percent)));
                        }
                        (f, old)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let items: Vec<layout::Item> = if mono {
                items
                    .into_iter()
                    .flat_map(|item| match item {
                        layout::Item::Para(p) if p.lines.len() > 1 => p
                            .lines
                            .iter()
                            .map(|l| layout::Item::Para(layout::Para { lines: vec![l.clone()], rect: l.rect, marker: None, ..p.clone() }))
                            .collect(),
                        other => vec![other],
                    })
                    .collect()
            } else {
                items
            };
            // Values side by side on one line (a data sheet's MIN, TYP and
            // MAX in one cell with no rules between): one paragraph, each
            // value at a tab where it stood. A paragraph each, they went one
            // under another, the row twice as high and the table over to
            // the next page.
            let mut rows: Vec<Vec<layout::Para>> = Vec::new();
            for item in items {
                let layout::Item::Para(para) = item else { continue };
                let row = if mono || para.lines.len() != 1 {
                    None
                } else {
                    rows.iter_mut().find(|r| {
                        r.iter().all(|q| {
                            q.lines.len() == 1
                                && (q.lines[0].baseline - para.lines[0].baseline).abs() < 0.3 * q.size.max(para.size)
                                && (q.rect[2] <= para.rect[0] || para.rect[2] <= q.rect[0])
                        })
                    })
                };
                match row {
                    Some(row) => row.push(para),
                    None => rows.push(vec![para]),
                }
            }
            for mut row in rows {
                row.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
                let rest = row.split_off(1);
                let mut para = row.pop().expect("a row has a paragraph");
                para.column = [cell.rect[0], cell.rect[2]];
                para.align = if mono || !rest.is_empty() { layout::Align::Left } else { layout::alignment(&para.lines, para.column, para.size) };
                let mut p = doc_para(&para, None);
                if mono || !rest.is_empty() {
                    p.align = layout::Align::Left;
                    p.left = (para.rect[0] - cell.rect[0]).max(0.0);
                    p.first = 0.0;
                }
                if mono {
                    p.pitch = p.pitch.max(1.1 * p.size);
                }
                // A short PDF line does not imply an extra right indent in
                // a cell: that would shrink the column and rewrap the text.
                if matches!(p.align, layout::Align::Left | layout::Align::Justify) {
                    p.right = 0.0;
                }
                let gap = (p.first_baseline - 0.8 * p.pitch - bottom).max(0.0);
                let _ = write!(
                    x,
                    r#"<w:p><w:pPr><w:widowControl w:val="0"/><w:tabs>{}</w:tabs><w:spacing w:before="{}" w:after="0" w:line="{}" w:lineRule="exact"/><w:ind w:left="{}" w:right="{}" w:firstLine="{}"/><w:jc w:val="{}"/></w:pPr>"#,
                    rest.iter().map(|q| format!(r#"<w:tab w:val="left" w:pos="{}"/>"#, tw((q.rect[0] - cell.rect[0]).max(0.0)))).collect::<String>(),
                    tw(gap),
                    tw(p.pitch),
                    tw(p.left),
                    tw(p.right),
                    tw(p.first.max(0.0)),
                    super::jc(p.align)
                );
                paragraph_runs(x, &p, out);
                bottom = p.bottom;
                for mut q in rest {
                    q.column = [cell.rect[0], cell.rect[2]];
                    x.push_str("<w:r><w:tab/></w:r>");
                    let q = doc_para(&q, None);
                    paragraph_runs(x, &q, out);
                    bottom = bottom.max(q.bottom);
                }
                x.push_str("</w:p>");
            }
            for (f, old) in saved {
                match old {
                    Some(s) => out.scales.insert(f, s),
                    None => out.scales.remove(&f),
                };
            }
            x.push_str("</w:tc>");
        }
        x.push_str("</w:tr>");
    }
    x.push_str(r#"</w:tbl><w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/><w:rPr><w:sz w:val="2"/></w:rPr></w:pPr></w:p>"#);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Faces, Graphic};

    fn text(s: &str, x: f32, y: f32, font: u16) -> Vec<Glyph> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| Glyph {
                source: -1,
                ch,
                rect: [x + i as f32 * 5.0, y - 8.0, x + i as f32 * 5.0 + 5.0, y + 2.0],
                x: x + i as f32 * 5.0,
                baseline: y,
                size: 10.0,
                font,
                color: [0; 3],
                space_before: false,
                invisible: false,
            })
            .collect()
    }

    fn fixture() -> (PageContent, Vec<FontInfo>) {
        let mut p = PageContent { size: [600.0, 800.0], ..Default::default() };
        for (top, bottom) in [(200.0, 230.0), (260.0, 300.0)] {
            for (x, right) in [(20.0, 200.0), (200.0, 400.0), (400.0, 580.0)] {
                p.graphics.push(Graphic { rect: [x, top, right, bottom], kind: GraphicKind::Path { curved: false }, object: None });
            }
        }
        for (s, x) in [("Name", 25.0), ("Value", 205.0), ("Unit", 405.0)] {
            p.glyphs.extend(text(s, x, 215.0, 0));
        }
        p.glyphs.extend(text("A heading spanning the entire table width", 25.0, 245.0, 1));
        p.glyphs.extend(text("The first line", 25.0, 275.0, 0));
        p.glyphs.extend(text("and the second line", 25.0, 287.0, 0));
        p.glyphs.extend(text("Value", 250.0, 282.0, 0));
        p.glyphs.extend(text("Units", 420.0, 282.0, 0));
        p.glyphs.extend(text("Unshaded last row", 25.0, 314.0, 0));
        p.glyphs.extend(text("Last value", 250.0, 314.0, 0));
        let fonts = vec![FontInfo { name: "Arial".into(), ..Default::default() }, FontInfo { name: "Arial-Bold".into(), weight: 700, ..Default::default() }];
        (p, fonts)
    }

    #[test]
    fn adjacent_shaded_cells_define_columns_merged_headings_and_white_rows() {
        let (p, fonts) = fixture();
        let form = detect(&p, &fonts).unwrap();
        assert_eq!(form.table.edges, [20.0, 200.0, 400.0, 580.0]);
        let heading = form.table.rows.iter().find(|r| r.top <= 245.0 && r.bottom > 245.0).unwrap();
        assert_eq!(heading.cells.len(), 1);
        assert_eq!(heading.cells[0].end, 3);
        assert!(form.table.rect[3] > 314.0);
        assert_eq!(form.table.rows.iter().flat_map(|r| &r.cells).map(|c| c.glyphs.len()).sum::<usize>(), p.glyphs.len());
        let mut flow = p;
        remove(&mut flow, &form);
        assert!(flow.glyphs.is_empty());
        assert_eq!(flow.graphics.len(), 1);
    }

    #[test]
    fn notes_under_the_values_span_the_table_in_rows_of_their_own() {
        let (mut p, fonts) = fixture();
        // Under the last values: a note over every column, its short last
        // line, the text joined with no word spaces as in the fixture.
        p.glyphs.extend(text("Notes_running_on_over_every_column_of_the_whole_table_and_further_on_to_the_end", 25.0, 330.0, 0));
        p.glyphs.extend(text("and_its_end.", 25.0, 342.0, 0));
        let form = detect(&p, &fonts).unwrap();
        let row_of = |y: f32| form.table.rows.iter().find(|r| r.top <= y && r.bottom > y).unwrap();
        assert_eq!(row_of(314.0).cells.len(), 3);
        let notes = row_of(330.0);
        assert_eq!(notes.cells.len(), 1);
        assert!(notes.top > 316.0 && notes.bottom > 342.0);
        assert!(notes.cells[0].glyphs.iter().any(|g| g.baseline == 342.0));
    }

    #[test]
    fn narrow_book_tables_and_illustrations_are_not_form_grids() {
        let (mut p, fonts) = fixture();
        p.size[0] = 900.0;
        assert!(detect(&p, &fonts).is_none());
        p.size[0] = 600.0;
        p.glyphs.clear();
        assert!(detect(&p, &fonts).is_none());
        let (mut p, fonts) = fixture();
        p.graphics.push(Graphic { rect: [100.0, 270.0, 120.0, 290.0], kind: GraphicKind::Image, object: None });
        assert!(detect(&p, &fonts).is_none());
    }

    #[test]
    fn cell_text_is_editable_and_uses_the_full_column_width() {
        let (p, fonts) = fixture();
        let mut table = detect(&p, &fonts).unwrap().table;
        table.rows[0].cells[0].fill = [212; 3];
        let mut lookup = crate::fonts::Fonts::default();
        let mut faces = Faces::default();
        let mut out = super::super::Out { fonts: &fonts, lookup: &mut lookup, faces: &mut faces, doc_pr: 0, scales: Default::default(), margin_left: 20.0, textboxes:crate::export::docx::TextBoxes::new() };
        let mut xml = String::new();
        write(&mut xml, &table, 20.0, &mut out);
        assert!(xml.contains(r#"w:fill="D4D4D4""#));
        assert!(xml.contains(r#"<w:gridSpan w:val="3"/>"#));
        assert!(xml.contains("The first line and the second line"));
        assert!(xml.contains(r#"w:right="0""#));
        assert!(!xml.contains("drawing") && !xml.contains("framePr"));
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_shaded_form_has_editable_cells_and_anchors_outside_the_table() {
        let mut content = String::from("q 40 0 0 30 20 740 cm /I Do Q 20 650 560 50 re S ");
        for (y, height, gray) in [(570, 30, ".8"), (500, 40, ".93")] {
            content.push_str(&format!("{gray} g 20 {y} 180 {height} re f 200 {y} 200 {height} re f 400 {y} 180 {height} re f "));
        }
        content.push_str("0 g BT /F 10 Tf 25 585 Td (Name) Tj ET BT /F 10 Tf 205 585 Td (Value) Tj ET BT /F 10 Tf 405 585 Td (Units) Tj ET BT /F 10 Tf 25 520 Td (Editable sample) Tj ET BT /F 10 Tf 205 520 Td (42) Tj ET BT /F 10 Tf 405 520 Td (units) Tj ET BT /F 10 Tf 25 675 Td (Metadata panel) Tj ET q 40 0 0 30 20 60 cm /I Do Q BT /F 8 Tf 20 20 Td (Footer text) Tj ET");
        let pdf = crate::pdfium::tests::write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F 5 0 R >> /XObject << /I 6 0 R >> >> >>".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>\nstream\nABCDEFGHIJKL\nendstream".into(),
        ]);
        let input = std::env::temp_dir().join(format!("metalpdf_shaded_form_{}.pdf", std::process::id()));
        let output = input.with_extension("docx");
        std::fs::write(&input, pdf).unwrap();
        assert_eq!(crate::export::run_paragraphs(&input, &output, None).unwrap(), 1);
        let bytes = std::fs::read(&output).unwrap();
        let xml = String::from_utf8(crate::export::zip::read_entry(&bytes, "word/document.xml").unwrap()).unwrap();
        assert_eq!(xml.matches("<w:tbl>").count(), 1);
        assert_eq!(xml.matches("<wp:anchor ").count(), 3);
        let (outside, table) = xml.split_once("<w:tbl>").unwrap();
        assert_eq!(outside.matches("<wp:anchor ").count(), 3);
        assert!(!table.contains("<wp:anchor "));
        assert!(table.contains("Editable sample") && table.contains(">42</w:t>"));
        assert!(table.contains(r#"w:fill="CCCCCC""#));
        assert!(!xml.contains("w:framePr"));
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
    }
}
