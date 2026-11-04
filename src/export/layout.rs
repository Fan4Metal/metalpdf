//! A page's characters made into what a document has: lines, columns in
//! reading order, paragraphs with their alignment, and figures. Pure
//! geometry on `PageContent`, no PDFium: tested on made-up pages.
//!
//! 1. **Lines.** The characters come in the content's order, which is
//!    the reading order in nearly every file: a character goes on with
//!    the one before when it sits on the same baseline (a superscript a
//!    little off it) and starts at most 1.5 em after it. The pieces so
//!    made are then joined along a baseline when they meet within an em
//!    (a word drawn later, a bold run written apart).
//! 2. **Figures.** The graphics are gathered into clusters of touching
//!    boxes on a 3-point grid. A cluster with a picture, a curve or a
//!    shading, or with at least three boxes (a bar chart), is a figure
//!    when the text inside it is sparse (labels); a box with a paragraph
//!    in it, a table's rules, a page-wide background are not. The
//!    characters inside a figure belong to its picture.
//! 3. **Reading order**, a recursive XY cut: the lines and figures are
//!    cut into rows where nothing overlaps vertically; rows whose free
//!    vertical bands line up (two columns, even with their baselines
//!    level) are cut at the widest common band, the left side read
//!    first, each side cut again. What no band divides is a group read
//!    top down: a column's text.
//! 4. **Paragraphs** in a group: a line starts a new one after a gap
//!    larger than the group's usual line pitch, a change of size, a
//!    short line before it, a first-line indent, or a list marker.
//!    Alignment from the lines' edges against the column's.

use super::{Glyph, GraphicKind, PageContent};

/// left, top, right, bottom, in view space (points, y down).
pub type Rect = [f32; 4];

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub glyphs: Vec<Glyph>,
    pub rect: Rect,
    /// The baseline and the size of its largest glyphs (not a
    /// superscript's).
    pub baseline: f32,
    pub size: f32,
}

impl Line {
    fn new(g: Glyph) -> Line {
        Line { rect: g.rect, baseline: g.baseline, size: g.size, glyphs: vec![g] }
    }

    fn push(&mut self, g: Glyph) {
        self.rect = union(self.rect, g.rect);
        if g.size > self.size * 1.05 {
            self.size = g.size;
            self.baseline = g.baseline;
        }
        self.glyphs.push(g);
    }

    /// The glyphs sorted along the line again after pieces were joined;
    /// the box, the size and the baseline made again.
    fn settle(&mut self) {
        self.glyphs.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        let main = self.glyphs.iter().max_by(|a, b| a.size.total_cmp(&b.size)).expect("a line has glyphs");
        (self.size, self.baseline) = (main.size, main.baseline);
        self.rect = self.glyphs.iter().fold(self.glyphs[0].rect, |r, g| union(r, g.rect));
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for g in &self.glyphs {
            if g.space_before && !s.is_empty() {
                s.push(' ');
            }
            s.push(g.ch);
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Para {
    pub lines: Vec<Line>,
    /// The column it is in: left and right.
    pub column: [f32; 2],
    pub align: Align,
    /// A list item: the index in the first line of the first glyph after
    /// the marker (a bullet, `1.`, `a)`).
    pub marker: Option<usize>,
    /// The size most of its characters have.
    pub size: f32,
    pub rect: Rect,
}

impl Para {
    /// The baseline to baseline distance of its lines; 1.2 em for one.
    pub fn pitch(&self) -> f32 {
        if self.lines.len() < 2 {
            return 1.2 * self.size;
        }
        let n = self.lines.len() - 1;
        (self.lines[n].baseline - self.lines[0].baseline) / n as f32
    }

    /// Where the text after a list marker starts.
    pub fn text_start(&self) -> Option<f32> {
        self.marker.and_then(|k| self.lines[0].glyphs.get(k)).map(|g| g.rect[0])
    }

    pub fn chars(&self) -> usize {
        self.lines.iter().map(|l| l.glyphs.len()).sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub rect: Rect,
    pub column: [f32; 2],
    /// The figure is one top-level picture: its index on the page, to be
    /// read at its own resolution.
    pub image: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Para(Para),
    Figure(Figure),
}

impl Item {
    pub fn rect(&self) -> Rect {
        match self {
            Item::Para(p) => p.rect,
            Item::Figure(f) => f.rect,
        }
    }
}

/// What a format wants of the page.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Take invisible text (a scan's recognised text) as text.
    pub invisible: bool,
    /// Find figures, and leave the text inside them to their pictures.
    pub figures: bool,
}

pub fn union(a: Rect, b: Rect) -> Rect {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn centre(r: Rect) -> [f32; 2] {
    [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
}

fn inside(p: [f32; 2], r: Rect, margin: f32) -> bool {
    p[0] >= r[0] - margin && p[0] <= r[2] + margin && p[1] >= r[1] - margin && p[1] <= r[3] + margin
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// A spacing accent TeX and some other typesetters draw apart, over the
/// letter: the combining accent it stands for.
fn accent(c: char) -> Option<char> {
    Some(match c {
        '\u{a8}' => '\u{308}',
        '\u{b4}' | '\u{2dd}' => '\u{301}',
        '`' => '\u{300}',
        '^' | '\u{2c6}' => '\u{302}',
        '~' | '\u{2dc}' => '\u{303}',
        '\u{af}' | '\u{2c9}' => '\u{304}',
        '\u{2d8}' => '\u{306}',
        '\u{2d9}' => '\u{307}',
        '\u{2da}' => '\u{30a}',
        '\u{2c7}' => '\u{30c}',
        _ => return None,
    })
}

/// A letter with a combining accent as one character, for the pairs
/// Latin and Cyrillic text has.
fn compose(base: char, mark: char) -> Option<char> {
    const TABLE: &[(char, &str, &str)] = &[
        ('\u{308}', "aeiouyAEIOUеЕиИ", "äëïöüÿÄËÏÖÜёЁӥӤ"),
        ('\u{301}', "aeiouyAEIOUYcnszCNSZ", "áéíóúýÁÉÍÓÚÝćńśźĆŃŚŹ"),
        ('\u{300}', "aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        ('\u{302}', "aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        ('\u{303}', "anoANO", "ãñõÃÑÕ"),
        ('\u{306}', "иИуУaAgG", "йЙўЎăĂğĞ"),
        ('\u{30c}', "cszrendtCSZRENDT", "čšžřěňďťČŠŽŘĚŇĎŤ"),
        ('\u{30a}', "auAU", "åůÅŮ"),
        ('\u{307}', "zZ", "żŻ"),
        ('\u{304}', "aeiouAEIOU", "āēīōūĀĒĪŌŪ"),
    ];
    let (_, from, to) = TABLE.iter().find(|(m, _, _)| *m == mark)?;
    let at = from.chars().position(|c| c == base)?;
    to.chars().nth(at)
}

/// Accents drawn apart joined to the letters under them: a spacing
/// accent whose middle is over a letter's goes into that letter (`е` and `¨` are
/// `ё`), or is dropped when the pair has no character of its own. Else it
/// made a line of its own between the text's lines and broke the
/// paragraph there.
fn join_accents(glyphs: &mut Vec<Glyph>) {
    let marks: Vec<usize> = (0..glyphs.len()).filter(|&i| accent(glyphs[i].ch).is_some()).collect();
    if marks.is_empty() {
        return;
    }
    let mut gone = vec![false; glyphs.len()];
    for &m in &marks {
        let a = glyphs[m].rect;
        let (ac, size) = ((a[0] + a[2]) / 2.0, glyphs[m].size.max(1.0));
        // The letter under it: nearest in the content's order first.
        let near = (m.saturating_sub(4)..(m + 5).min(glyphs.len())).chain(0..glyphs.len());
        // Over the letter's middle (glyphs side by side never overlap so),
        // on its baseline or above it: TeX raises the accent's box, or
        // draws it from the letter's baseline, the mark high in its em.
        let base = near.filter(|&i| i != m && !gone[i] && accent(glyphs[i].ch).is_none()).find(|&i| {
            let r = glyphs[i].rect;
            let inset = 0.15 * (r[2] - r[0]);
            let dy = glyphs[i].baseline - glyphs[m].baseline;
            ac > r[0] + inset && ac < r[2] - inset && dy > -0.1 * size && dy < 1.2 * size
        });
        let Some(base) = base else { continue };
        if let Some(c) = accent(glyphs[m].ch).and_then(|mark| compose(glyphs[base].ch, mark)) {
            glyphs[base].ch = c;
        }
        gone[m] = true;
    }
    let mut k = 0;
    glyphs.retain(|_| {
        k += 1;
        !gone[k - 1]
    });
}

/// The page as paragraphs and figures in reading order.
pub fn analyse(content: &PageContent, opts: Options) -> Vec<Item> {
    let [w, h] = content.size;
    // The glyphs to take: on the page, visible unless asked, each once
    // (some files draw a run twice, a hair apart, to make it look bold).
    let mut seen = std::collections::HashSet::new();
    let mut glyphs: Vec<Glyph> = content
        .glyphs
        .iter()
        .filter(|g| opts.invisible || !g.invisible)
        .filter(|g| g.rect[2] > 0.0 && g.rect[0] < w && g.rect[3] > 0.0 && g.rect[1] < h)
        .filter(|g| seen.insert((g.ch, (g.x / 1.5).round() as i32, (g.baseline / 1.5).round() as i32)))
        .cloned()
        .collect();
    join_accents(&mut glyphs);
    let figures = if opts.figures { figures(content, &glyphs) } else { Vec::new() };
    if !figures.is_empty() {
        glyphs.retain(|g| !figures.iter().any(|f| inside(centre(g.rect), f.rect, 1.0)));
    }
    let lines = lines(glyphs);
    let mut rects: Vec<Rect> = lines.iter().map(|l| l.rect).collect();
    rects.extend(figures.iter().map(|f| f.rect));
    if rects.is_empty() {
        return Vec::new();
    }
    let size = median(lines.iter().map(|l| l.size).collect()).unwrap_or(10.0);
    let gutter = (0.9 * size).max(4.0);
    let bounds = rects.iter().fold(rects[0], |a, &b| union(a, b));
    if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
        for (i, r) in rects.iter().enumerate() {
            let what = lines.get(i).map_or("[figure]".to_owned(), |l| l.text().chars().take(50).collect());
            eprintln!("{i:3} [{:6.1} {:6.1} {:6.1} {:6.1}] {what}", r[0], r[1], r[2], r[3]);
        }
    }
    let mut groups = Vec::new();
    let all: Vec<usize> = (0..rects.len()).collect();
    cut(&all, &rects, [bounds[0], bounds[2]], gutter, &mut groups);
    if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
        for g in &groups {
            eprintln!("group {:?}: {:?}", g.column, g.members);
        }
    }
    let mut items = Vec::new();
    for group in groups {
        paragraphs(&group, &lines, &figures, &mut items);
    }
    items
}

/// The characters in lines (step 1 above).
pub fn lines(glyphs: Vec<Glyph>) -> Vec<Line> {
    // Pieces in the content's order.
    let mut pieces: Vec<Line> = Vec::new();
    for mut g in glyphs {
        if let Some(cur) = pieces.last_mut()
            && let Some(gap) = follows(cur, &g, 1.5)
        {
            if gap > 0.22 * g.size.min(cur.size) {
                g.space_before = true;
            }
            cur.push(g);
            continue;
        }
        pieces.push(Line::new(g));
    }
    // Joined along their baselines.
    pieces.sort_by(|a, b| a.baseline.total_cmp(&b.baseline).then(a.rect[0].total_cmp(&b.rect[0])));
    let mut lines: Vec<Line> = Vec::new();
    'piece: for piece in pieces {
        for line in lines.iter_mut().rev() {
            let s = line.size.max(piece.size);
            if piece.baseline - line.baseline > 0.6 * s {
                break;
            }
            if !level(line, piece.baseline, piece.size) {
                continue;
            }
            // Next to it, either side, within an em; not over it.
            let after = piece.rect[0] - line.rect[2];
            let before = line.rect[0] - piece.rect[2];
            let goes_after = after > -0.3 * s;
            let gap = if goes_after { after } else if before > -0.3 * s { before } else { continue };
            if gap > 1.0 * s || apart_in_size(line.size, piece.size, gap) {
                continue;
            }
            let mut glyphs = piece.glyphs;
            if gap > 0.22 * s {
                if goes_after {
                    glyphs[0].space_before = true;
                } else {
                    // The line's first glyph now follows the piece.
                    line.glyphs[0].space_before = true;
                }
            }
            line.glyphs.extend(glyphs);
            line.settle();
            continue 'piece;
        }
        lines.push(piece);
    }
    lines.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]).then(a.rect[0].total_cmp(&b.rect[0])));
    lines
}

/// Whether a glyph at `baseline` of `size` is on `line`'s baseline: within
/// a fifth of an em, or half an em for a smaller one (a superscript, a
/// subscript) or a larger one (a drop of size back to the line).
fn level(line: &Line, baseline: f32, size: f32) -> bool {
    let s = line.size.max(size);
    let dy = (baseline - line.baseline).abs();
    let unlike = size < 0.85 * line.size || line.size < 0.85 * size;
    dy < 0.2 * s || unlike && dy < 0.5 * s
}

/// The gap from `line`'s last glyph to `g` when `g` goes on the line: on
/// its baseline, forward, at most `ems` em after it.
fn follows(line: &Line, g: &Glyph, ems: f32) -> Option<f32> {
    let last = line.glyphs.last()?;
    let s = line.size.max(g.size);
    if !level(line, g.baseline, g.size) {
        return None;
    }
    let gap = g.rect[0] - last.rect[2];
    (gap > -0.3 * s && g.x >= last.x - 0.1 * s && gap < ems * s && !apart_in_size(line.size, g.size, gap)).then_some(gap)
}

/// Text of unlike sizes (a caption beside the body) a space apart or more
/// is two columns' on one baseline, not one line: a superscript or small
/// capitals follow their text closely.
fn apart_in_size(a: f32, b: f32, gap: f32) -> bool {
    let (small, large) = (a.min(b), a.max(b));
    small < 0.88 * large && gap > 0.5 * large
}

/// The figures of the page (step 2 above).
fn figures(content: &PageContent, glyphs: &[Glyph]) -> Vec<Figure> {
    let [w, h] = content.size;
    if !(w > 1.0 && h > 1.0) {
        return Vec::new();
    }
    let page_area = w * h;
    // The graphics worth looking at, clipped to the page.
    let mut boxes = Vec::new();
    for (i, g) in content.graphics.iter().enumerate() {
        let r = [g.rect[0].max(0.0), g.rect[1].max(0.0), g.rect[2].min(w), g.rect[3].min(h)];
        let (bw, bh) = (r[2] - r[0], r[3] - r[1]);
        if !(bw > 0.0 && bh > 0.0) && !(bw >= 0.0 && bh >= 0.0 && (bw > 2.0 || bh > 2.0)) {
            continue;
        }
        if bw * bh >= 0.85 * page_area {
            continue;
        }
        boxes.push((i, r));
    }
    if boxes.is_empty() {
        return Vec::new();
    }
    // Union-find over a grid: boxes sharing a cell (each grown by a cell)
    // are one cluster.
    let cell = (page_area / 250_000.0).sqrt().max(3.0);
    let (cols, rows) = ((w / cell).ceil() as usize + 1, (h / cell).ceil() as usize + 1);
    let mut owner = vec![u32::MAX; cols * rows];
    let mut parent: Vec<usize> = (0..boxes.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut visits = 0usize;
    for (b, &(_, r)) in boxes.iter().enumerate() {
        let (x0, y0) = (((r[0] / cell) as usize).saturating_sub(1), ((r[1] / cell) as usize).saturating_sub(1));
        let (x1, y1) = (((r[2] / cell) as usize + 1).min(cols - 1), ((r[3] / cell) as usize + 1).min(rows - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                visits += 1;
                let o = &mut owner[y * cols + x];
                if *o == u32::MAX {
                    *o = b as u32;
                } else {
                    let (a, c) = (find(&mut parent, *o as usize), find(&mut parent, b));
                    parent[a] = c;
                }
            }
        }
        if visits > 8_000_000 {
            log::debug!("figures: too many graphics to cluster");
            return Vec::new();
        }
    }
    #[derive(Default)]
    struct Cluster {
        rect: Option<Rect>,
        seeds: usize,
        rich: bool,
        members: usize,
        image: Option<usize>,
        images: usize,
    }
    let mut clusters: std::collections::HashMap<usize, Cluster> = std::collections::HashMap::new();
    for (b, &(i, r)) in boxes.iter().enumerate() {
        let root = find(&mut parent, b);
        let c = clusters.entry(root).or_default();
        let g = &content.graphics[i];
        c.rect = Some(c.rect.map_or(r, |a| union(a, r)));
        c.members += 1;
        let (bw, bh) = (r[2] - r[0], r[3] - r[1]);
        match g.kind {
            GraphicKind::Image => {
                if bw * bh >= 64.0 {
                    c.seeds += 1;
                    c.rich = true;
                    c.images += 1;
                    c.image = g.object;
                }
            }
            GraphicKind::Shading => {
                c.seeds += 1;
                c.rich = true;
            }
            GraphicKind::Path { curved } => {
                if bw >= 1.5 && bh >= 1.5 {
                    c.seeds += 1;
                    c.rich |= curved;
                }
            }
        }
    }
    let mut out: Vec<Figure> = Vec::new();
    for c in clusters.into_values() {
        let Some(rect) = c.rect else { continue };
        let (fw, fh) = (rect[2] - rect[0], rect[3] - rect[1]);
        let area = fw * fh;
        if c.seeds == 0 || fw < 12.0 || fh < 12.0 || area < 400.0 {
            continue;
        }
        let text = glyphs.iter().filter(|g| inside(centre(g.rect), rect, 1.0)).count() as f32;
        let figure = if c.rich { text <= (0.004 * area).max(40.0) } else { c.seeds >= 3 && text <= 0.0015 * area };
        if figure {
            let image = if c.members == 1 && c.images == 1 { c.image } else { None };
            out.push(Figure { rect, column: [rect[0], rect[2]], image });
        }
    }
    // A figure inside another is part of it.
    let all = out.clone();
    out.retain(|f| !all.iter().any(|o| o.rect != f.rect && inside([f.rect[0], f.rect[1]], o.rect, 0.5) && inside([f.rect[2], f.rect[3]], o.rect, 0.5)));
    out.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
    out
}

/// A run of lines and figures read top down, in a column.
#[derive(Debug, PartialEq)]
pub struct Group {
    pub column: [f32; 2],
    pub members: Vec<usize>,
}

/// The free vertical bands of a row at least `gutter` wide, between its
/// boxes.
fn gaps(row: &[usize], rects: &[Rect], gutter: f32) -> Vec<[f32; 2]> {
    let mut xs: Vec<[f32; 2]> = row.iter().map(|&i| [rects[i][0], rects[i][2]]).collect();
    xs.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut out = Vec::new();
    let mut right = xs[0][1];
    for x in &xs[1..] {
        if x[0] - right >= gutter {
            out.push([right, x[0]]);
        }
        right = right.max(x[1]);
    }
    out
}

/// The bands both sets share, at least `gutter` wide.
fn common(a: &[[f32; 2]], b: &[[f32; 2]], gutter: f32) -> Vec<[f32; 2]> {
    let mut out = Vec::new();
    for x in a {
        for y in b {
            let (l, r) = (x[0].max(y[0]), x[1].min(y[1]));
            if r - l >= gutter {
                out.push([l, r]);
            }
        }
    }
    out
}

/// The recursive XY cut of `idx` (step 3 above), its groups into `out` in
/// reading order.
pub fn cut(idx: &[usize], rects: &[Rect], column: [f32; 2], gutter: f32, out: &mut Vec<Group>) {
    if idx.is_empty() {
        return;
    }
    let mut sorted = idx.to_vec();
    sorted.sort_by(|&a, &b| rects[a][1].total_cmp(&rects[b][1]).then(rects[a][0].total_cmp(&rects[b][0])));
    // Rows: what overlaps vertically by more than a point.
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut bottom = f32::NEG_INFINITY;
    for i in sorted {
        let r = rects[i];
        match rows.last_mut() {
            Some(row) if r[1] < bottom - 1.0 => {
                row.push(i);
                bottom = bottom.max(r[3]);
            }
            _ => {
                rows.push(vec![i]);
                bottom = r[3];
            }
        }
    }
    // The rows not yet in a group start at `from`.
    let mut from = 0;
    let flush = |rows: &[Vec<usize>], out: &mut Vec<Group>| {
        let members: Vec<usize> = rows.iter().flatten().copied().collect();
        if !members.is_empty() {
            out.push(Group { column, members });
        }
    };
    let widest = |bands: &[[f32; 2]]| bands.iter().copied().max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])));
    // Whether nothing of a row crosses the middle of a band: it can go
    // with the columns the band parts (the end of the longer column, a
    // heading over one of them, lines a little longer further down).
    // A line centred on the whole region (a title, a link under it) is the
    // region's, not a column's, though the band does not cross it.
    let region_mid = (column[0] + column[1]) / 2.0;
    let centred = |r: Rect| ((r[0] + r[2]) / 2.0 - region_mid).abs() < gutter.max(0.02 * (column[1] - column[0])) && r[0] > column[0] + 2.0 * gutter;
    let clear = |row: &[usize], band: [f32; 2]| {
        let mid = (band[0] + band[1]) / 2.0;
        row.iter().all(|&m| (rects[m][2] <= mid || rects[m][0] >= mid) && !centred(rects[m]))
    };
    let mut i = 0;
    while i < rows.len() {
        let first = gaps(&rows[i], rects, gutter);
        if first.is_empty() {
            i += 1;
            continue;
        }
        // The rows after it whose bands line up with its (narrowed to
        // what they share), or that nothing crosses the band in.
        let mut shared = first;
        let mut j = i + 1;
        while j < rows.len() {
            let next = gaps(&rows[j], rects, gutter);
            let both = common(&shared, &next, gutter / 2.0);
            if !both.is_empty() {
                shared = both;
            } else if !clear(&rows[j], widest(&shared).expect("a band")) {
                break;
            }
            j += 1;
        }
        let band = widest(&shared).expect("a band");
        let mut start = i;
        while start > from && clear(&rows[start - 1], band) {
            start -= 1;
        }
        let mid = (band[0] + band[1]) / 2.0;
        let members: Vec<usize> = rows[start..j].iter().flatten().copied().collect();
        let (left, right): (Vec<usize>, Vec<usize>) = members.iter().partition(|&&m| centre(rects[m])[0] < mid);
        if !left.is_empty() && !right.is_empty() {
            flush(&rows[from..start], out);
            cut(&left, rects, [column[0], band[0]], gutter, out);
            cut(&right, rects, [band[1], column[1]], gutter, out);
            from = j;
        }
        i = j;
    }
    flush(&rows[from..], out);
}

/// Characters that mark a list item when a gap follows them; the private
/// use area holds the bullets of the Symbol and Wingdings fonts.
fn is_bullet(c: char) -> bool {
    matches!(c, '•' | '◦' | '▪' | '▫' | '‣' | '⁃' | '●' | '○' | '■' | '□' | '–' | '—' | '-' | '*' | '·' | '►' | '▶' | '✓' | '✔' | '➢' | '➤' | '❖' | '♦' | '◆' | '◇' | '→')
        || ('\u{f000}'..='\u{f0ff}').contains(&c)
}

/// Where the text after a list marker starts on `line`, if it has one: a
/// bullet, or `1.` `1)` `a.` `(a)` `iv.` `1.2.` and the like, then a gap.
pub fn marker(line: &Line) -> Option<usize> {
    let g = &line.glyphs;
    let token_end = (1..g.len()).find(|&k| g[k].space_before || g[k].rect[0] - g[k - 1].rect[2] > 0.3 * line.size)?;
    if token_end >= g.len() {
        return None;
    }
    let token: String = g[..token_end].iter().map(|g| g.ch).collect();
    if token_end == 1 && is_bullet(g[0].ch) {
        return Some(1);
    }
    let body = token.trim_start_matches('(');
    let stem = body.strip_suffix(['.', ')'])?;
    let numbered = !stem.is_empty() && stem.len() <= 8 && stem.split('.').all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()));
    let lettered = stem.chars().count() == 1 && stem.chars().all(char::is_alphabetic);
    let roman = (1..=5).contains(&stem.len()) && stem.chars().all(|c| matches!(c, 'i' | 'v' | 'x' | 'I' | 'V' | 'X'));
    (numbered || lettered || roman).then_some(token_end)
}

/// The paragraphs and figures of a group (step 4 above).
fn paragraphs(group: &Group, lines: &[Line], figures: &[super::layout::Figure], out: &mut Vec<Item>) {
    let column = group.column;
    let ls: Vec<&Line> = group.members.iter().filter(|&&m| m < lines.len()).map(|&m| &lines[m]).collect();
    // The group's own extent and usual pitch.
    let right = ls.iter().map(|l| l.rect[2]).fold(f32::NEG_INFINITY, f32::max);
    let left = ls.iter().map(|l| l.rect[0]).fold(f32::INFINITY, f32::min);
    let pitches: Vec<f32> = ls
        .windows(2)
        .filter_map(|w| {
            let (a, b) = (w[0], w[1]);
            let p = b.baseline - a.baseline;
            let s = a.size.max(b.size);
            (p > 0.5 * s && p < 2.5 * s && a.size.min(b.size) > 0.85 * s).then_some(p / s)
        })
        .collect();
    let usual = median(pitches);
    let mut current: Vec<Line> = Vec::new();
    let mut current_marker: Option<usize> = None;
    let finish = |current: &mut Vec<Line>, marker: &mut Option<usize>, out: &mut Vec<Item>| {
        if !current.is_empty() {
            out.push(Item::Para(make_para(std::mem::take(current), column, marker.take())));
        }
    };
    for &m in &group.members {
        if m >= lines.len() {
            finish(&mut current, &mut current_marker, out);
            let f = &figures[m - lines.len()];
            out.push(Item::Figure(Figure { rect: f.rect, column, image: f.image }));
            continue;
        }
        let line = &lines[m];
        let line_marker = marker(line);
        if let Some(prev) = current.last() {
            let s = prev.size.max(line.size);
            let pitch = line.baseline - prev.baseline;
            let normal = usual.map_or(1.25 * s, |u| u * s);
            let ratio = line.size / prev.size;
            let col_w = (right - left).max(1.0);
            let soft = prev.glyphs.last().is_some_and(|g| g.ch == '\u{ad}' || g.ch == '-');
            let short = prev.rect[2] < right - (3.5 * s).max(0.14 * col_w) && !soft;
            let mid = (column[0] + column[1]) / 2.0;
            let centred = |l: &Line| (centre(l.rect)[0] - mid).abs() < 1.5 * s && l.rect[0] > column[0] + 1.5 * s;
            let continues_list = current_marker.is_some() && current[0].glyphs.get(current_marker.unwrap()).is_some_and(|g| (line.rect[0] - g.rect[0]).abs() < 1.0 * s);
            let first_left = current[0].rect[0];
            let body_left = if current.len() >= 2 { current[1].rect[0] } else { first_left };
            // Indented against the paragraph's body: a new paragraph's
            // first line (not a list item's continuation).
            let indented = line.rect[0] > body_left + 0.8 * s && !continues_list && (current.len() >= 2 || first_left < left + 0.8 * s);
            let breaks = pitch <= 0.3 * s
                || pitch > normal * 1.3 + 0.5
                || !(0.85..=1.18).contains(&ratio)
                || line_marker.is_some()
                || short && !(centred(prev) && centred(line))
                || indented && !(centred(prev) && centred(line))
                || line.rect[0] > prev.rect[2];
            if breaks {
                finish(&mut current, &mut current_marker, out);
            }
        }
        if current.is_empty() {
            current_marker = line_marker;
        }
        current.push(line.clone());
    }
    finish(&mut current, &mut current_marker, out);
}

fn make_para(lines: Vec<Line>, column: [f32; 2], marker: Option<usize>) -> Para {
    let rect = lines.iter().fold(lines[0].rect, |r, l| union(r, l.rect));
    // The size most characters have.
    let mut sizes: Vec<(f32, usize)> = Vec::new();
    for g in lines.iter().flat_map(|l| &l.glyphs) {
        let s = (g.size * 2.0).round() / 2.0;
        match sizes.iter_mut().find(|(k, _)| *k == s) {
            Some((_, n)) => *n += 1,
            None => sizes.push((s, 1)),
        }
    }
    let size = sizes.iter().max_by_key(|(_, n)| *n).map_or(lines[0].size, |(s, _)| *s);
    // A list item is not centred, whatever its one line's place says.
    let align = match alignment(&lines, column, size) {
        Align::Center | Align::Right if marker.is_some() => Align::Left,
        a => a,
    };
    Para { lines, column, align, marker, size, rect }
}

/// The alignment the lines' edges show against the column's.
pub fn alignment(lines: &[Line], column: [f32; 2], size: f32) -> Align {
    let tol = (0.6 * size).max(2.0);
    let mid = (column[0] + column[1]) / 2.0;
    let n = lines.len();
    if n == 1 {
        let r = lines[0].rect;
        if ((r[0] + r[2]) / 2.0 - mid).abs() < 1.5 * tol && r[0] > column[0] + 2.0 * size {
            return Align::Center;
        }
        if (r[2] - column[1]).abs() < tol && r[0] > column[0] + 4.0 * size {
            return Align::Right;
        }
        return Align::Left;
    }
    let lefts: Vec<f32> = lines.iter().map(|l| l.rect[0]).collect();
    let rights: Vec<f32> = lines.iter().map(|l| l.rect[2]).collect();
    let centres: Vec<f32> = lines.iter().map(|l| (l.rect[0] + l.rect[2]) / 2.0).collect();
    let spread = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max) - v.iter().copied().fold(f32::INFINITY, f32::min);
    let max_right = rights.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let body_left = spread(&lefts[1..]) < tol;
    // Justified lines end where the others do to a point or so; ragged
    // ones come within half an em of each other often enough.
    let flush = (0.15 * size).max(1.0);
    let full = rights[..n - 1].iter().all(|&r| max_right - r < flush);
    if body_left && full && (n >= 3 || (max_right - column[1]).abs() < tol) {
        return Align::Justify;
    }
    if spread(&centres) < tol && spread(&lefts) > 2.0 * tol {
        return Align::Center;
    }
    if spread(&rights) < tol && spread(&lefts) > 2.0 * tol {
        return Align::Right;
    }
    Align::Left
}

/// A part of a page in columns side by side (a newspaper's, a paper's
/// two), or in one: Word gets a section of that many columns for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// The columns, left to right: left and right edges.
    pub columns: Vec<[f32; 2]>,
    /// The items in reading order: the column each is in, its index.
    pub items: Vec<(usize, usize)>,
    /// The columns' items line up in rows, and are short: a table.
    pub table: Option<Table>,
    /// One line read left to right (a running head with the title at the
    /// left and the chapter at the right): one paragraph, its parts
    /// parted by tabs.
    pub line: bool,
}

/// A table found in a segment: its columns' edges and its rows, each a
/// cell a column with the items in it (none in an empty cell).
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub columns: Vec<[f32; 2]>,
    /// Per row: the top and bottom of what is in it, and the cells.
    pub rows: Vec<([f32; 2], Vec<Vec<usize>>)>,
}

/// The rows of a segment's items when they make a table. A row starts
/// with an item whose top is below the row's first by more than half an
/// em; an item lower in a column than the row's is in its cell still
/// when the row has not ended (a cell's second paragraph). A table has
/// two rows at least, three in five of them with two cells or more, and
/// either its rules drawn (as many paths in it as it has rows) or one
/// line in each cell: columns of text line their tops up now and then,
/// but their paragraphs are longer and nothing rules them.
fn table_of(items: &[Item], columns: &[[f32; 2]], members: &[(usize, usize)], graphics: &[super::Graphic]) -> Option<Table> {
    if members.iter().any(|&(_, k)| !matches!(items[k], Item::Para(_)) || items[k].lines() > 5) {
        return None;
    }
    let mut sorted = members.to_vec();
    sorted.sort_by(|a, b| items[a.1].rect()[1].total_cmp(&items[b.1].rect()[1]));
    let mut rows: Vec<([f32; 2], Vec<Vec<usize>>)> = Vec::new();
    for (c, k) in sorted {
        let r = items[k].rect();
        let size = match &items[k] {
            Item::Para(p) => p.size,
            Item::Figure(_) => 10.0,
        };
        match rows.last_mut() {
            // Level with the row's top, or under its own cell's text
            // within the row's height.
            Some((span, cells)) if r[1] < span[0] + 0.5 * size || (!cells[c].is_empty() && r[1] < span[1] + 0.5 * size && cells.iter().enumerate().all(|(o, cell)| o == c || cell.iter().all(|&m| items[m].rect()[3] >= r[1]))) => {
                span[1] = span[1].max(r[3]);
                cells[c].push(k);
            }
            _ => {
                let mut cells = vec![Vec::new(); columns.len()];
                cells[c].push(k);
                rows.push(([r[1], r[3]], cells));
            }
        }
    }
    // Rows of one cell at its ends (a page number under a table, a caption
    // over it) are not the table's.
    let filled = |cells: &Vec<Vec<usize>>| cells.iter().filter(|c| !c.is_empty()).count();
    while rows.last().is_some_and(|(_, cells)| filled(cells) < 2) {
        rows.pop();
    }
    while rows.first().is_some_and(|(_, cells)| filled(cells) < 2) {
        rows.remove(0);
    }
    let full = rows.iter().filter(|(_, cells)| filled(cells) >= 2).count();
    if rows.len() < 2 || full * 5 < rows.len() * 3 {
        return None;
    }
    let rect = [columns[0][0], rows[0].0[0], columns[columns.len() - 1][1], rows[rows.len() - 1].0[1]];
    let ruled = graphics
        .iter()
        .filter(|g| matches!(g.kind, super::GraphicKind::Path { .. }) && g.rect[0] >= rect[0] - 12.0 && g.rect[2] <= rect[2] + 12.0 && g.rect[1] >= rect[1] - 12.0 && g.rect[3] <= rect[3] + 12.0)
        .count()
        >= rows.len();
    let one_line = members.iter().all(|&(_, k)| items[k].lines() == 1);
    (ruled || one_line && rows.len() >= 3).then(|| Table { columns: columns.to_vec(), rows })
}

impl Item {
    pub fn column(&self) -> [f32; 2] {
        match self {
            Item::Para(p) => p.column,
            Item::Figure(f) => f.column,
        }
    }

    fn lines(&self) -> usize {
        match self {
            Item::Para(p) => p.lines.len(),
            Item::Figure(_) => 3,
        }
    }
}

/// The page's items in segments: runs of items whose columns have
/// another beside them make a segment of those columns, read left to
/// right (two to four, each with two lines at least: else a row of a
/// table, kept in one column); the rest one-column segments.
pub fn segments(items: &[Item], graphics: &[super::Graphic]) -> Vec<Segment> {
    let same = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() < 1.0;
    let apart = |a: [f32; 2], b: [f32; 2]| a[1] <= b[0] + 1.0 || a[0] >= b[1] - 1.0;
    let columns: Vec<[f32; 2]> = items.iter().map(Item::column).collect();
    let side = |c: [f32; 2]| columns.iter().any(|&o| apart(o, c));
    let single = |range: std::ops::Range<usize>| {
        let column = range.clone().map(|k| columns[k]).fold([f32::INFINITY, f32::NEG_INFINITY], |a, c| [a[0].min(c[0]), a[1].max(c[1])]);
        Segment { columns: vec![column], items: range.map(|k| (0, k)).collect(), table: None, line: false }
    };
    let mut out: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if !side(columns[i]) {
            let mut j = i + 1;
            while j < items.len() && !side(columns[j]) {
                j += 1;
            }
            out.push(single(i..j));
            i = j;
            continue;
        }
        let mut cols = vec![columns[i]];
        let mut members = vec![(0, i)];
        let mut j = i + 1;
        while j < items.len() && side(columns[j]) {
            let c = columns[j];
            let cur = cols.len() - 1;
            if same(c, cols[cur]) {
                members.push((cur, j));
            } else if c[0] >= cols[cur][1] - 1.0 {
                cols.push(c);
                members.push((cur + 1, j));
            } else {
                break;
            }
            j += 1;
        }
        let lines_in = |col: usize| members.iter().filter(|m| m.0 == col).map(|m| items[m.1].lines()).sum::<usize>();
        let first_top = items[members[0].1].rect()[1];
        let one_line = cols.len() >= 2
            && members.iter().all(|&(_, k)| match &items[k] {
                Item::Para(p) => p.lines.len() == 1 && (p.rect[1] - first_top).abs() < 0.5 * p.size,
                Item::Figure(_) => false,
            });
        if one_line {
            out.push(Segment { columns: cols, items: members, table: None, line: true });
        } else if cols.len() >= 2
            && let Some(table) = table_of(items, &cols, &members, graphics)
        {
            // What the table left out goes before or after it, by place.
            let inside: std::collections::HashSet<usize> = table.rows.iter().flat_map(|(_, cells)| cells.iter().flatten().copied()).collect();
            let top = table.rows[0].0[0];
            let (mut before, mut after): (Vec<usize>, Vec<usize>) = members.iter().map(|m| m.1).filter(|k| !inside.contains(k)).partition(|&k| items[k].rect()[1] < top);
            before.sort_by(|&a, &b| items[a].rect()[1].total_cmp(&items[b].rect()[1]));
            after.sort_by(|&a, &b| items[a].rect()[1].total_cmp(&items[b].rect()[1]));
            let loose = |ks: Vec<usize>| {
                let column = ks.iter().map(|&k| columns[k]).fold([f32::INFINITY, f32::NEG_INFINITY], |a, c| [a[0].min(c[0]), a[1].max(c[1])]);
                Segment { columns: vec![column], items: ks.into_iter().map(|k| (0, k)).collect(), table: None, line: false }
            };
            if !before.is_empty() {
                out.push(loose(before));
            }
            out.push(Segment { columns: cols, items: inside.iter().map(|&k| (0, k)).collect(), table: Some(table), line: false });
            if !after.is_empty() {
                out.push(loose(after));
            }
        } else if (2..=4).contains(&cols.len()) && (0..cols.len()).all(|c| lines_in(c) >= 2) {
            out.push(Segment { columns: cols, items: members, table: None, line: false });
        } else {
            out.push(single(i..j));
        }
        i = j;
    }
    // Neighbouring one-column segments are one.
    let mut merged: Vec<Segment> = Vec::new();
    for s in out {
        match merged.last_mut() {
            Some(last) if last.columns.len() == 1 && s.columns.len() == 1 && last.table.is_none() && s.table.is_none() && !last.line && !s.line => {
                last.columns[0] = [last.columns[0][0].min(s.columns[0][0]), last.columns[0][1].max(s.columns[0][1])];
                last.items.extend(s.items);
            }
            _ => merged.push(s),
        }
    }
    merged
}

/// How a span sits against its line's baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vert {
    Base,
    Super,
    Sub,
}

/// A run of text in one style: what Word's `w:r` and PowerPoint's `a:r`
/// are made of.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub font: u16,
    pub size: f32,
    pub color: [u8; 3],
    pub vert: Vert,
}

/// The paragraph's text in runs, its lines joined: with a space, with
/// nothing after a hyphen, the hyphen dropped where PDFium found it at a
/// line's end (U+00AD). `marker_sep` goes between a list marker and its
/// text (a tab for Word).
pub fn spans(para: &Para, marker_sep: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut pending: Option<&str> = None;
    for (n, line) in para.lines.iter().enumerate() {
        if n > 0 {
            let last = out.last_mut();
            match last {
                Some(span) if span.text.ends_with('\u{ad}') => {
                    span.text.pop();
                }
                Some(span) if span.text.ends_with('-') => {}
                Some(_) => pending = Some(" "),
                None => {}
            }
        }
        for (k, g) in line.glyphs.iter().enumerate() {
            if n == 0 && para.marker == Some(k) {
                pending = Some(marker_sep);
            } else if g.space_before && (k > 0 || n > 0) && pending.is_none() {
                pending = Some(" ");
            }
            let vert = if g.size < 0.85 * line.size {
                let dy = line.baseline - g.baseline;
                if dy > 0.12 * line.size {
                    Vert::Super
                } else if dy < -0.08 * line.size {
                    Vert::Sub
                } else {
                    Vert::Base
                }
            } else {
                Vert::Base
            };
            let size = if vert == Vert::Base { (g.size * 2.0).round() / 2.0 } else { (line.size * 2.0).round() / 2.0 };
            if let (Some(sep), Some(span)) = (pending.take(), out.last_mut()) {
                span.text.push_str(sep);
            }
            // A hyphen PDFium found that is not at a line's end after all.
            let ch = if g.ch == '\u{ad}' && k + 1 < line.glyphs.len() { '-' } else { g.ch };
            match out.last_mut() {
                Some(span) if span.font == g.font && span.size == size && span.color == g.color && span.vert == vert => span.text.push(ch),
                _ => out.push(Span { text: ch.to_string(), font: g.font, size, color: g.color, vert }),
            }
        }
    }
    if let Some(span) = out.last_mut()
        && span.text.ends_with('\u{ad}')
    {
        span.text.pop();
        span.text.push('-');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Graphic, GraphicKind};

    /// A line of text set at `x`, `baseline`, 10 pt, 5 pt a character,
    /// words parted by a 2.5 pt gap.
    fn text(s: &str, x: f32, baseline: f32) -> Vec<Glyph> {
        let mut out = Vec::new();
        let mut at = x;
        let mut space = false;
        for c in s.chars() {
            if c == ' ' {
                at += 2.5;
                space = true;
                continue;
            }
            out.push(Glyph { ch: c, rect: [at, baseline - 9.0, at + 5.0, baseline + 2.5], x: at, baseline, size: 10.0, font: 0, color: [0; 3], space_before: std::mem::take(&mut space), invisible: false });
            at += 5.0;
        }
        out
    }

    fn page(glyphs: Vec<Glyph>) -> PageContent {
        PageContent { size: [600.0, 800.0], glyphs, graphics: Vec::new(), turns: 0 }
    }

    fn texts(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                Item::Para(p) => spans(p, "\t").iter().map(|s| s.text.as_str()).collect::<String>(),
                Item::Figure(_) => "[figure]".to_owned(),
            })
            .collect()
    }

    const OPTS: Options = Options { invisible: false, figures: true };

    #[test]
    fn a_paragraph_wraps_into_one_and_a_gap_starts_another() {
        let mut g = text("The first line of a para-", 50.0, 100.0);
        g.last_mut().unwrap().ch = '\u{ad}';
        g.extend(text("graph goes on here and ends", 50.0, 112.0));
        g.extend(text("short.", 50.0, 124.0));
        g.extend(text("Another one starts after a gap", 50.0, 150.0));
        let items = analyse(&page(g), OPTS);
        assert_eq!(texts(&items), ["The first line of a paragraph goes on here and ends short.", "Another one starts after a gap"]);
    }

    #[test]
    fn two_columns_are_read_one_after_the_other_even_with_level_baselines() {
        let mut g = Vec::new();
        for (k, y) in [100.0, 112.0, 124.0].into_iter().enumerate() {
            g.extend(text(&format!("left column line {k} goes on"), 40.0, y));
            g.extend(text(&format!("right column line {k} goes on"), 320.0, y));
        }
        let items = analyse(&page(g), OPTS);
        let t = texts(&items);
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(t[0].starts_with("left column line 0") && t[0].ends_with("line 2 goes on"), "{t:?}");
        assert!(t[1].starts_with("right column line 0"), "{t:?}");
    }

    #[test]
    fn the_longer_columns_end_is_read_with_it() {
        let mut g = text("Title over the left", 40.0, 60.0);
        for k in 0..5 {
            g.extend(text(&format!("left column line {k} goes on"), 40.0, 100.0 + 12.0 * k as f32));
        }
        for k in 0..2 {
            g.extend(text(&format!("right column line {k} goes on"), 320.0, 100.0 + 12.0 * k as f32));
        }
        let t = texts(&analyse(&page(g), OPTS));
        assert_eq!(t.len(), 3, "{t:?}");
        assert!(t[1].starts_with("left column line 0") && t[1].ends_with("line 4 goes on"), "{t:?}");
        assert!(t[2].starts_with("right column line 0"), "{t:?}");
    }

    #[test]
    fn a_title_over_two_columns_makes_two_segments() {
        let mut g = text("A title across the whole page width, long enough to span", 40.0, 60.0);
        for k in 0..4 {
            g.extend(text(&format!("left column line {k} goes on"), 40.0, 100.0 + 12.0 * k as f32));
            g.extend(text(&format!("right column line {k} goes on"), 320.0, 100.0 + 12.0 * k as f32));
        }
        let items = analyse(&page(g), OPTS);
        let segs = segments(&items, &[]);
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert_eq!(segs[0].columns.len(), 1);
        assert_eq!(segs[1].columns.len(), 2);
        assert_eq!(segs[1].items.iter().map(|m| m.0).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn cells_in_rows_make_a_table_and_columns_of_text_do_not() {
        let mut g = text("A paragraph above the table that runs across the page", 40.0, 60.0);
        for (k, (a, b)) in [("Name", "Value"), ("Width", "12 pt"), ("Height", "40 pt")].into_iter().enumerate() {
            let y = 100.0 + 30.0 * k as f32;
            g.extend(text(a, 50.0, y));
            g.extend(text(b, 250.0, y));
        }
        let content = page(g);
        let segs = segments(&analyse(&content, OPTS), &content.graphics);
        let table = segs.iter().find_map(|s| s.table.as_ref()).expect("a table");
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.rows.len(), 3);
        assert!(table.rows.iter().all(|(_, cells)| cells.iter().all(|c| c.len() == 1)));
        // Two columns of long paragraphs are columns.
        let mut g = Vec::new();
        for k in 0..8 {
            g.extend(text(&format!("left column line {k} goes on"), 40.0, 100.0 + 12.0 * k as f32));
            g.extend(text(&format!("right column line {k} goes on"), 320.0, 100.0 + 12.0 * k as f32));
        }
        let content = page(g);
        let segs = segments(&analyse(&content, OPTS), &content.graphics);
        assert!(segs.iter().all(|s| s.table.is_none()), "{segs:?}");
    }

    #[test]
    fn an_accent_drawn_apart_joins_its_letter() {
        // "еще" with a diaeresis over the "е", as TeX sets "ещё".
        let mut g = text("ещ", 50.0, 100.0);
        let mut e = text("е", 60.0, 100.0);
        let mut mark = text("\u{a8}", 60.5, 100.0);
        mark[0].rect = [60.5, 91.0, 64.5, 102.5];
        g.append(&mut e);
        g.append(&mut mark);
        g.extend(text("Next line", 50.0, 112.0));
        let items = analyse(&page(g), OPTS);
        assert_eq!(texts(&items), ["ещё Next line"]);
        // A caret between letters is a caret.
        let g = text("a^b", 50.0, 100.0);
        assert_eq!(texts(&analyse(&page(g), OPTS)), ["a^b"]);
    }

    #[test]
    fn pieces_written_apart_join_into_their_line() {
        // "world" drawn before "hello", on the same baseline.
        let mut g = text("world", 80.0, 100.0);
        g[0].space_before = false;
        g.extend(text("hello", 50.0, 100.0));
        let lines = lines(g);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "hello world");
    }

    #[test]
    fn a_bullet_with_a_gap_is_a_list_marker() {
        let mut g = text("• Item text that wraps onto", 50.0, 100.0);
        g.extend(text("the next line", 57.5, 112.0));
        g.extend(text("2) Second item", 50.0, 124.0));
        let items = analyse(&page(g), OPTS);
        assert_eq!(texts(&items), ["•\tItem text that wraps onto the next line", "2)\tSecond item"]);
        let Item::Para(p) = &items[0] else { panic!() };
        assert_eq!(p.marker, Some(1));
    }

    #[test]
    fn a_centred_title_and_a_justified_body() {
        // The body spans 50 to 227.5: the title is centred on 138.75.
        let mut g = text("Title", 126.25, 60.0);
        for k in 0..3 {
            g.extend(text("aaaa bbbb cccc dddd eeee ffff gggg hhhh", 50.0, 100.0 + 12.0 * k as f32));
        }
        // The last line shorter.
        g.extend(text("end", 50.0, 136.0));
        let items = analyse(&page(g), OPTS);
        let Item::Para(title) = &items[0] else { panic!() };
        assert_eq!(title.align, Align::Center);
        let Item::Para(body) = &items[1] else { panic!() };
        assert_eq!(body.lines.len(), 4);
        assert_eq!(body.align, Align::Justify);
    }

    #[test]
    fn a_picture_is_a_figure_and_its_labels_stay_in_it() {
        let mut content = page(text("Text above the picture", 50.0, 100.0));
        content.glyphs.extend(text("label", 200.0, 200.0));
        content.glyphs.extend(text("Text below the picture", 50.0, 400.0));
        content.graphics.push(Graphic { rect: [150.0, 150.0, 450.0, 350.0], kind: GraphicKind::Image, object: Some(3) });
        let items = analyse(&content, OPTS);
        assert_eq!(texts(&items), ["Text above the picture", "[figure]", "Text below the picture"]);
        let Item::Figure(f) = &items[1] else { panic!() };
        assert_eq!(f.image, Some(3));
    }

    #[test]
    fn a_box_round_a_paragraph_is_not_a_figure() {
        let mut content = page(Vec::new());
        for k in 0..6 {
            content.glyphs.extend(text("a boxed paragraph of text inside a frame", 60.0, 110.0 + 12.0 * k as f32));
        }
        content.graphics.push(Graphic { rect: [50.0, 95.0, 300.0, 190.0], kind: GraphicKind::Path { curved: false }, object: None });
        let items = analyse(&content, OPTS);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Item::Para(_)));
    }

    #[test]
    fn superscripts_stay_on_their_line() {
        let mut g = text("E=mc", 50.0, 100.0);
        let mut two = text("2", 70.0, 96.0);
        two[0].size = 6.0;
        two[0].rect = [70.0, 90.0, 73.0, 97.0];
        g.extend(two);
        let items = analyse(&page(g), OPTS);
        let Item::Para(p) = &items[0] else { panic!() };
        let s = spans(p, "\t");
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].vert, Vert::Super);
        assert_eq!(s[1].text, "2");
    }

    #[test]
    fn list_markers_are_recognised() {
        let m = |s: &str| marker(&lines(text(s, 0.0, 10.0))[0]);
        assert_eq!(m("1. One"), Some(2));
        assert_eq!(m("(a) Letter"), Some(3));
        assert_eq!(m("iv) Roman"), Some(3));
        assert_eq!(m("1.2. Nested"), Some(4));
        assert_eq!(m("- dash"), Some(1));
        assert_eq!(m("Plain words"), None);
        assert_eq!(m("2024 was"), None);
    }
}
