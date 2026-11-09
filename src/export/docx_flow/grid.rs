//! Tables drawn with rules or shaded cells, as Word's own tables.
//!
//! The rules come from `PageContent::rules`: every level or upright segment
//! of a stroked path and every rectangle of a filled one, so that a grid
//! drawn as one path is still seen line by line. Rules and shaded boxes
//! that touch make one structure; its distinct x and y positions are the
//! grid, and a cell spans the grid lines no rule or box side marks inside
//! it (merged cells, across or down). Text keeps flowing in the cells:
//! editing it rewraps the cell and grows the row, where a picture of the
//! table with labels over it (what such a table was before) cannot.
//!
//! Only what is plainly a table is taken: text inside, no picture or
//! drawing in it, at least two cells or one closed box, and no line of
//! text running across a ruled cell edge. The rest is left to the figures.

use super::forms::{Border, Cell, Merge, Row, Table};
use crate::export::{Glyph, GraphicKind, PageContent, layout};

type Rect = [f32; 4];

/// Positions closer than this are one grid line (a rule's two edges, a
/// box's side on a rule).
const SNAP: f32 = 2.0;
/// Grid lines closer than this make no row or column of their own (a
/// double rule).
const MIN_BAND: f32 = 3.5;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A level rule.
    H,
    /// An upright rule.
    V,
    /// A shaded box: its sides part cells, its colour fills them.
    Box,
}

#[derive(Clone, Copy, Debug)]
struct El {
    r: Rect,
    kind: Kind,
    color: [u8; 3],
}

impl El {
    fn width(&self) -> f32 {
        match self.kind {
            Kind::H => self.r[3] - self.r[1],
            Kind::V => self.r[2] - self.r[0],
            Kind::Box => 0.0,
        }
    }
}

fn centre(g: &Glyph) -> [f32; 2] {
    [(g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0]
}

fn inside(p: [f32; 2], r: Rect) -> bool {
    p[0] >= r[0] && p[0] < r[2] && p[1] >= r[1] && p[1] < r[3]
}

fn touch(a: Rect, b: Rect, pad: f32) -> bool {
    a[0] <= b[2] + pad && b[0] <= a[2] + pad && a[1] <= b[3] + pad && b[1] <= a[3] + pad
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Values within `SNAP` of each other as one, their mean; then lines
/// closer than `MIN_BAND` as one.
fn snap(mut v: Vec<f32>) -> Vec<f32> {
    v.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, usize)> = Vec::new();
    for x in v {
        match out.last_mut() {
            Some((sum, n)) if x - *sum / *n as f32 <= SNAP => {
                *sum += x;
                *n += 1;
            }
            _ => out.push((x, 1)),
        }
    }
    let mut lines: Vec<f32> = Vec::new();
    for (sum, n) in out {
        let x = sum / n as f32;
        match lines.last_mut() {
            Some(last) if x - *last < MIN_BAND => *last = (*last + x) / 2.0,
            _ => lines.push(x),
        }
    }
    lines
}

/// The marks tables are made of: rules thin and visible, shaded boxes.
fn elements(content: &PageContent) -> Vec<El> {
    let [w, h] = content.size;
    content
        .rules
        .iter()
        .filter(|r| r.color.iter().any(|&c| c < 235))
        .filter_map(|r| {
            let (rw, rh) = (r.rect[2] - r.rect[0], r.rect[3] - r.rect[1]);
            let kind = if rh <= 3.0 && rw >= 4.0 && rw > 2.0 * rh {
                Kind::H
            } else if rw <= 3.0 && rh >= 4.0 && rh > 2.0 * rw {
                Kind::V
            } else if r.filled && rw > 3.0 && rh > 3.0 && rw * rh < 0.6 * w * h {
                Kind::Box
            } else {
                return None;
            };
            Some(El { r: r.rect, kind, color: r.color })
        })
        .collect()
}

/// The structures: elements that touch, as groups.
fn components(els: &[El]) -> Vec<Vec<usize>> {
    let n = els.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    // Sorted by left edge, each compared with those that can reach it.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| els[a].r[0].total_cmp(&els[b].r[0]));
    for (k, &a) in order.iter().enumerate() {
        for &b in &order[k + 1..] {
            if els[b].r[0] > els[a].r[2] + 1.5 {
                break;
            }
            if touch(els[a].r, els[b].r, 1.5) {
                let (x, y) = (find(&mut parent, a), find(&mut parent, b));
                parent[x] = y;
            }
        }
    }
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    groups.into_values().collect()
}

/// The page's ruled or shaded tables, top down.
pub(super) fn detect(content: &PageContent) -> Vec<Table> {
    let els = elements(content);
    if els.is_empty() || els.len() > 6000 {
        return Vec::new();
    }
    let mut tables: Vec<Table> = components(&els).into_iter().filter_map(|group| table(content, &group.iter().map(|&i| els[i]).collect::<Vec<_>>())).collect();
    // A table inside another (a box drawn in a cell) is part of it.
    let all: Vec<Rect> = tables.iter().map(|t| t.rect).collect();
    tables.retain(|t| !all.iter().any(|&o| o != t.rect && o[0] <= t.rect[0] + 0.5 && o[1] <= t.rect[1] + 0.5 && o[2] >= t.rect[2] - 0.5 && o[3] >= t.rect[3] - 0.5));
    tables.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
    tables
}

fn table(content: &PageContent, els: &[El]) -> Option<Table> {
    let [pw, ph] = content.size;
    let rect = els.iter().map(|e| e.r).reduce(layout::union)?;
    let (tw, th) = (rect[2] - rect[0], rect[3] - rect[1]);
    if tw < 20.0 || th < 8.0 || tw > 0.97 * pw && th > 0.9 * ph {
        return None;
    }
    let glyphs: Vec<&Glyph> = content.glyphs.iter().filter(|g| !g.invisible && !g.ch.is_whitespace() && inside(centre(g), rect)).collect();
    if glyphs.is_empty() {
        return None;
    }
    // Pictures, drawings and shadings belong to figures.
    let area = tw * th;
    if content.graphics.iter().any(|g| {
        let r = g.rect;
        let shared = overlap(r[0], r[2], rect[0], rect[2]) * overlap(r[1], r[3], rect[1], rect[3]);
        shared > 0.02 * area && !matches!(g.kind, GraphicKind::Path { curved: false })
    }) {
        return None;
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for e in els {
        let r = e.r;
        match e.kind {
            Kind::H => {
                ys.push((r[1] + r[3]) / 2.0);
                xs.extend([r[0], r[2]]);
            }
            Kind::V => {
                xs.push((r[0] + r[2]) / 2.0);
                ys.extend([r[1], r[3]]);
            }
            Kind::Box => {
                xs.extend([r[0], r[2]]);
                ys.extend([r[1], r[3]]);
            }
        }
    }
    let xs = snap(xs);
    let ys = snap(ys);
    if xs.len() < 2 || ys.len() < 2 {
        return None;
    }
    let (cols, rows) = (xs.len() - 1, ys.len() - 1);
    let near = |a: f32, b: f32| (a - b).abs() <= SNAP + 0.5;
    // What marks the upright grid line `j` along band `i`, and the level
    // one `i` along column `c`: a rule (drawn), a box's side (a boundary
    // with no line), or nothing.
    let upright = |i: usize, j: usize| -> (bool, Option<El>) {
        let (y0, y1) = (ys[i], ys[i + 1]);
        let mut boundary = false;
        let mut rule = None;
        for e in els {
            let covers = overlap(e.r[1], e.r[3], y0, y1) >= 0.5 * (y1 - y0);
            match e.kind {
                Kind::V if covers && near((e.r[0] + e.r[2]) / 2.0, xs[j]) => rule = Some(*e),
                Kind::Box if covers && (near(e.r[0], xs[j]) || near(e.r[2], xs[j])) => boundary = true,
                _ => {}
            }
        }
        (boundary || rule.is_some(), rule)
    };
    let level = |i: usize, c: usize| -> (bool, Option<El>) {
        let (x0, x1) = (xs[c], xs[c + 1]);
        let mut boundary = false;
        let mut rule = None;
        for e in els {
            let covers = overlap(e.r[0], e.r[2], x0, x1) >= 0.5 * (x1 - x0);
            match e.kind {
                Kind::H if covers && near((e.r[1] + e.r[3]) / 2.0, ys[i]) => rule = Some(*e),
                Kind::Box if covers && (near(e.r[1], ys[i]) || near(e.r[3], ys[i])) => boundary = true,
                _ => {}
            }
        }
        (boundary || rule.is_some(), rule)
    };
    let lines = layout::lines(glyphs.iter().map(|g| (*g).clone()).collect());
    // Cells across: a band's columns joined where nothing parts them, or
    // where a line of text runs over an unruled boundary.
    let mut spans: Vec<Vec<(usize, usize)>> = Vec::with_capacity(rows);
    for i in 0..rows {
        let mut band = Vec::new();
        let mut start = 0;
        #[allow(clippy::needless_range_loop)] // `j` names the grid line `upright` looks at too
        for j in 1..cols {
            let (parted, rule) = upright(i, j);
            let in_band = |l: &&layout::Line| l.baseline > ys[i] && l.baseline < ys[i + 1];
            if rule.is_some() {
                // A drawn rule parts the cells even with their text close
                // up to it; one through the letters is not a cell's edge.
                if lines.iter().filter(in_band).flat_map(|l| &l.glyphs).any(|g| {
                    // The middle half of the letter: loose boxes reach out.
                    let quarter = (g.rect[2] - g.rect[0]) / 4.0;
                    g.rect[0] + quarter < xs[j] && g.rect[2] - quarter > xs[j] && quarter > 0.4
                }) {
                    return None;
                }
                band.push((start, j));
                start = j;
                continue;
            }
            let crossed = lines.iter().filter(in_band).any(|l| super::forms::runs_across(l, xs[j]));
            if parted && !crossed {
                band.push((start, j));
                start = j;
            }
        }
        band.push((start, cols));
        spans.push(band);
    }
    // Cells down: a cell goes on into the band below when that band has
    // the same columns and nothing parts the two.
    let mut merge = vec![Vec::new(); rows];
    for i in 0..rows {
        for &(a, b) in &spans[i] {
            let open = i > 0 && spans[i - 1].contains(&(a, b)) && (a..b).all(|c| !level(i, c).0);
            merge[i].push(open);
        }
    }
    // A closed box at least: one cell needs all four sides or a fill.
    let filled = els.iter().any(|e| e.kind == Kind::Box);
    let cells: usize = merge.iter().flatten().filter(|open| !**open).count();
    if cells < 2 {
        let closed = (0..cols).all(|c| level(0, c).1.is_some() && level(rows, c).1.is_some()) && upright_edge(els, xs[0], ys[0], ys[rows]) && upright_edge(els, xs[cols], ys[0], ys[rows]);
        if !(filled || closed) || th > 0.5 * ph {
            return None;
        }
    }
    // Only horizontal rules, no upright one nor a box: a list set between
    // rules, or a table whose columns only its text shows; left as it was.
    if !filled && !els.iter().any(|e| e.kind == Kind::V) {
        return None;
    }
    let border = |e: Option<El>| e.map(|e| Border { color: e.color, width: e.width() });
    let mut out_rows = Vec::with_capacity(rows);
    for i in 0..rows {
        let mut cells = Vec::new();
        for (k, &(a, b)) in spans[i].iter().enumerate() {
            let continued = merge[i][k];
            // The cell's extent: this band and those it goes on into.
            let mut last = i;
            while last + 1 < rows && spans[last + 1].iter().position(|s| *s == (a, b)).is_some_and(|p| merge[last + 1][p]) {
                last += 1;
            }
            let r = [xs[a], ys[i], xs[b], ys[last + 1]];
            let glyphs = if continued { Vec::new() } else { content.glyphs.iter().filter(|g| !g.invisible && inside(centre(g), r)).cloned().collect() };
            // The last box drawn over the cell's middle gives its fill.
            let mid = [(r[0] + r[2]) / 2.0, (ys[i] + ys[i + 1]) / 2.0];
            let fill = els.iter().rev().find(|e| e.kind == Kind::Box && inside(mid, e.r)).map_or([255; 3], |e| e.color);
            let top = (a..b).filter_map(|c| level(i, c).1).next();
            let bottom = (a..b).filter_map(|c| level(last + 1, c).1).next();
            let left = (i..=last).filter_map(|k| upright_rule(els, xs[a], ys[k], ys[k + 1])).next();
            let right = (i..=last).filter_map(|k| upright_rule(els, xs[b], ys[k], ys[k + 1])).next();
            cells.push(Cell {
                first: a,
                end: b,
                rect: r,
                glyphs,
                fill,
                borders: [border(top), border(left), border(bottom), border(right)],
                merge: if continued { Merge::Continue } else if last > i { Merge::Restart } else { Merge::None },
            });
        }
        out_rows.push(Row { top: ys[i], bottom: ys[i + 1], cells });
    }
    Some(Table { rect: [xs[0], ys[0], xs[cols], ys[rows]], edges: xs, rows: out_rows })
}

/// The upright rule at `x` along `y0..y1`, covering most of it.
fn upright_rule(els: &[El], x: f32, y0: f32, y1: f32) -> Option<El> {
    els.iter().find(|e| e.kind == Kind::V && ((e.r[0] + e.r[2]) / 2.0 - x).abs() <= SNAP + 0.5 && overlap(e.r[1], e.r[3], y0, y1) >= 0.5 * (y1 - y0)).copied()
}

fn upright_edge(els: &[El], x: f32, y0: f32, y1: f32) -> bool {
    upright_rule(els, x, y0, y1).is_some()
}

/// Take the tables' text and marks out of what flows.
pub(super) fn remove(content: &mut PageContent, tables: &[Table]) {
    for t in tables {
        let r = [t.rect[0] - 1.0, t.rect[1] - 1.0, t.rect[2] + 1.0, t.rect[3] + 1.0];
        content.glyphs.retain(|g| !inside(centre(g), t.rect));
        content.graphics.retain(|g| !(g.rect[0] >= r[0] && g.rect[2] <= r[2] && g.rect[1] >= r[1] && g.rect[3] <= r[3]));
        content.rules.retain(|g| !(g.rect[0] >= r[0] && g.rect[2] <= r[2] && g.rect[1] >= r[1] && g.rect[3] <= r[3]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::Rule;

    fn text(s: &str, x: f32, y: f32) -> Vec<Glyph> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| Glyph { source: -1, ch, rect: [x + i as f32 * 5.0, y - 8.0, x + i as f32 * 5.0 + 5.0, y + 2.0], x: x + i as f32 * 5.0, baseline: y, size: 10.0, font: 0, color: [0; 3], space_before: false, invisible: false })
            .collect()
    }

    fn hline(x0: f32, x1: f32, y: f32) -> Rule {
        Rule { rect: [x0, y - 0.25, x1, y + 0.25], filled: false, color: [0; 3] }
    }

    fn vline(x: f32, y0: f32, y1: f32) -> Rule {
        Rule { rect: [x - 0.25, y0, x + 0.25, y1], filled: false, color: [0; 3] }
    }

    /// A ruled grid of three columns and three rows; the last column of
    /// the first two body rows one cell (no rule between them).
    fn grid() -> PageContent {
        let mut p = PageContent { size: [600.0, 800.0], ..Default::default() };
        for y in [100.0, 120.0] {
            p.rules.push(hline(50.0, 450.0, y));
        }
        p.rules.push(hline(50.0, 300.0, 150.0));
        p.rules.push(hline(50.0, 450.0, 180.0));
        for x in [50.0, 150.0, 300.0, 450.0] {
            p.rules.push(vline(x, 100.0, 180.0));
        }
        // The grid as one path's bounds, as `graphics` has it.
        p.graphics.push(crate::export::Graphic { rect: [50.0, 100.0, 450.0, 180.0], kind: GraphicKind::Path { curved: false }, object: None });
        for (s, x, y) in [("Name", 55.0, 114.0), ("Value", 155.0, 114.0), ("Notes", 305.0, 114.0), ("a", 55.0, 140.0), ("1", 155.0, 140.0), ("One note for two rows", 305.0, 150.0), ("b", 55.0, 170.0), ("2", 155.0, 170.0)] {
            p.glyphs.extend(text(s, x, y));
        }
        p
    }

    #[test]
    fn a_ruled_grid_is_a_table_with_a_cell_merged_down() {
        let p = grid();
        let tables = detect(&p);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.edges, [50.0, 150.0, 300.0, 450.0]);
        assert_eq!(t.rows.len(), 3);
        let notes = &t.rows[1].cells[2];
        assert_eq!(notes.merge, Merge::Restart);
        assert_eq!(notes.glyphs.len(), "One note for two rows".len());
        assert_eq!(t.rows[2].cells[2].merge, Merge::Continue);
        assert!(t.rows[2].cells[2].glyphs.is_empty());
        assert!(t.rows.iter().flat_map(|r| &r.cells).filter(|c| c.merge != Merge::Continue).all(|c| c.borders.iter().all(Option::is_some)));
        let mut flow = p.clone();
        remove(&mut flow, &tables);
        assert!(flow.glyphs.is_empty() && flow.graphics.is_empty() && flow.rules.is_empty());
    }

    #[test]
    fn a_shaded_code_box_is_one_cell_and_text_across_shading_joins_cells() {
        let mut p = PageContent { size: [600.0, 800.0], ..Default::default() };
        p.rules.push(Rule { rect: [50.0, 100.0, 450.0, 140.0], filled: true, color: [224; 3] });
        p.glyphs.extend(text("(config)> show version", 55.0, 115.0));
        p.glyphs.extend(text("Release: 4.1", 55.0, 130.0));
        let t = detect(&p);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].rows.len(), 1);
        assert_eq!(t[0].rows[0].cells[0].fill, [224; 3]);
        assert!(t[0].rows[0].cells[0].borders.iter().all(Option::is_none));
        // Two shaded halves with a line of text over both: one cell.
        let mut p = PageContent { size: [600.0, 800.0], ..Default::default() };
        p.rules.push(Rule { rect: [50.0, 100.0, 250.0, 120.0], filled: true, color: [224; 3] });
        p.rules.push(Rule { rect: [250.0, 100.0, 450.0, 120.0], filled: true, color: [224; 3] });
        p.glyphs.extend(text("A long line of text running across the middle", 52.0, 114.0));
        let t = detect(&p);
        assert_eq!(t[0].rows[0].cells.len(), 1);
    }

    #[test]
    fn text_close_up_to_a_rule_keeps_its_cell_and_text_through_one_is_no_table() {
        let mut p = grid();
        // Up to the rule at 300 and on just after it, a word space apart.
        p.glyphs.extend(text("Keyword", 263.0, 170.0));
        p.glyphs.extend(text("Next", 302.0, 170.0));
        let t = detect(&p);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].rows[2].cells.len(), 3);
        // A word drawn through the rule.
        p.glyphs.extend(text("Struck", 288.0, 130.0));
        assert!(detect(&p).is_empty());
    }

    #[test]
    fn rules_alone_drawings_and_frames_are_not_tables() {
        // Rules between paragraphs, with no upright one.
        let mut p = PageContent { size: [600.0, 800.0], ..Default::default() };
        for y in [100.0, 130.0, 160.0] {
            p.rules.push(hline(50.0, 450.0, y));
        }
        p.glyphs.extend(text("Between rules", 55.0, 120.0));
        assert!(detect(&p).is_empty());
        // A grid with a picture in it.
        let mut p = grid();
        p.graphics.push(crate::export::Graphic { rect: [160.0, 125.0, 290.0, 175.0], kind: GraphicKind::Image, object: None });
        assert!(detect(&p).is_empty());
        // A frame round the page.
        let mut p = PageContent { size: [600.0, 800.0], ..Default::default() };
        p.rules.extend([hline(10.0, 590.0, 10.0), hline(10.0, 590.0, 790.0), vline(10.0, 10.0, 790.0), vline(590.0, 10.0, 790.0)]);
        p.glyphs.extend(text("Body", 55.0, 120.0));
        assert!(detect(&p).is_empty());
        // An empty grid.
        let mut p = grid();
        p.glyphs.clear();
        assert!(detect(&p).is_empty());
    }
}
