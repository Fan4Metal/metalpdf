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
        // Artwork was appended after the text and may have first joined
        // another image across a comma. Recompute its separator against
        // its actual neighbour once the intervening text has been inserted.
        for i in 1..self.glyphs.len() {
            if self.glyphs[i].source < -1 {
                self.glyphs[i].space_before = self.glyphs[i].rect[0]-self.glyphs[i-1].rect[2] > 0.22*self.glyphs[i].size;
            }
        }
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
        '\u{b4}' | '\u{2dd}' | '\u{e088}' => '\u{301}',
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
pub(super) fn join_accents(glyphs: &mut Vec<Glyph>) {
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
            esay!("{i:3} [{:6.1} {:6.1} {:6.1} {:6.1}] {what}", r[0], r[1], r[2], r[3]);
        }
    }
    let mut groups = Vec::new();
    let all: Vec<usize> = (0..rects.len()).collect();
    cut(&all, &rects, [bounds[0], bounds[2]], gutter, &mut groups);
    if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
        for g in &groups {
            esay!("group {:?}: {:?}", g.column, g.members);
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
    'piece: for mut piece in pieces {
        // Pictures standing in for characters, sorted first by a baseline
        // a hair above the text's, go into the holes the text left them:
        // they were left lines of their own under the text running over.
        if piece.glyphs.iter().any(|g| g.source >= -1) {
            let mut k = lines.len();
            while k > 0 {
                k -= 1;
                let line = &lines[k];
                let s = line.size.max(piece.size);
                if piece.baseline - line.baseline > 0.6 * s {
                    break;
                }
                if line.glyphs.iter().all(|g| g.source < -1)
                    && level(&piece, line.baseline, line.size)
                    && line.rect[0] >= piece.rect[0]
                    && line.rect[2] <= piece.rect[2]
                    && piece.glyphs.iter().all(|g| g.rect[2] <= line.rect[0] + 0.2 * s || g.rect[0] >= line.rect[2] - 0.2 * s)
                {
                    let art = lines.remove(k);
                    piece.glyphs.extend(art.glyphs);
                    piece.settle();
                }
            }
        }
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
            // An inline picture is inserted after extraction. Text on its
            // two sides may already be one line; insert it into that hole
            // rather than giving it a separate paragraph below the text.
            if (piece.glyphs.iter().all(|g| g.source < -1) || line.glyphs.iter().any(|g| g.source < -1))
                && piece.rect[0] >= line.rect[0] && piece.rect[2] <= line.rect[2]
                && line.glyphs.iter().all(|g| g.rect[2] <= piece.rect[0]+0.2*s || g.rect[0] >= piece.rect[2]-0.2*s)
            {
                line.glyphs.extend(piece.glyphs);
                line.settle();
                continue 'piece;
            }
            let goes_after = after > -0.3 * s;
            let gap = if goes_after { after } else if before > -0.3 * s { before } else { continue };
            let artwork_marker = line.glyphs.iter().chain(&piece.glyphs).any(|g| g.source < -1);
            // A contents line: the page number after a dot leader, the
            // entry after its number, a tab's width apart.
            let (left, right) = if goes_after { (&*line, &piece) } else { (&piece, &*line) };
            let page_number = right.glyphs.len() <= 6 && right.glyphs.iter().all(|g| g.ch.is_ascii_digit() || matches!(g.ch, 'i' | 'v' | 'x' | 'l' | 'c'));
            let contents = ends_with_leader(left) && page_number || left.glyphs.len() <= 8 && left.glyphs.last().is_some_and(|g| g.ch == '.' || g.ch.is_ascii_digit()) && ends_with_leader_before_number(right);
            // A leader's dots, each a piece of its own when spaced wide,
            // join the line whatever size their small boxes give them (a
            // TeX contents page: a dot was 4 pt in a line of 10, and every
            // one was a line of its own).
            let dots = piece.glyphs.iter().all(|g| matches!(g.ch, '.' | '·' | '…'));
            if gap > (if artwork_marker { 2.5 } else if contents { 4.0 } else { 1.0 }) * s || !dots && apart_in_size(line.size, piece.size, gap) && (line.baseline - piece.baseline).abs() < 0.12 * s {
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

/// Whether a line ends with a dot leader (`Title . . . .`).
fn ends_with_leader(line: &Line) -> bool {
    let tail: Vec<char> = line.glyphs.iter().rev().map(|g| g.ch).filter(|c| !c.is_whitespace()).take(3).collect();
    tail.len() == 3 && tail.iter().all(|&c| c == '.' || c == '…')
}

/// Whether a line is a contents entry: a dot leader, then the page
/// number (digits or roman), at its end.
fn ends_with_leader_before_number(line: &Line) -> bool {
    let chars: Vec<char> = line.glyphs.iter().map(|g| g.ch).filter(|c| !c.is_whitespace()).collect();
    let number = chars.iter().rev().take_while(|c| c.is_ascii_digit() || matches!(c, 'i' | 'v' | 'x' | 'l' | 'c')).count();
    let rest = &chars[..chars.len() - number];
    number > 0 && rest.len() >= 3 && rest[rest.len() - 3..].iter().all(|&c| c == '.' || c == '…')
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
    (gap > -0.3 * s && g.x >= last.x - 0.1 * s && gap < ems * s && (!apart_in_size(line.size, g.size, gap) || (line.baseline - g.baseline).abs() >= 0.12 * s)).then_some(gap)
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
    figures_with_labels(content, glyphs, false)
}

/// In paragraph exports, diagram labels must stay with their artwork.
pub(super) fn labelled_figures(content: &PageContent) -> Vec<Figure> {
    figures_with_labels(content, &content.glyphs, true)
}

fn figures_with_labels(content: &PageContent, glyphs: &[Glyph], labels: bool) -> Vec<Figure> {
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
        /// The area of the images too small to count alone (a number set
        /// as a picture a digit).
        crumbs: f32,
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
                } else if bw >= 2.0 && bh >= 2.0 {
                    c.crumbs += bw * bh;
                    if c.crumbs >= 64.0 && c.images == 0 {
                        c.seeds += 1;
                        c.rich = true;
                        c.images += 1;
                    }
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
    let mut fonts = std::collections::HashMap::new();
    for g in glyphs.iter().filter(|g| !g.invisible && g.ch.is_alphabetic()) {
        *fonts.entry((g.font, (g.size * 2.0).round() as i32)).or_insert(0usize) += 1;
    }
    let body = fonts.iter().max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f))).map(|(f, _)| *f);
    let mut out: Vec<Figure> = Vec::new();
    for c in clusters.into_values() {
        let Some(rect) = c.rect else { continue };
        let (fw, fh) = (rect[2] - rect[0], rect[3] - rect[1]);
        let area = fw * fh;
        let min_height = if c.images > 0 { 6.0 } else { 12.0 };
        if c.seeds == 0 || fw < 12.0 || fh < min_height || area < 400.0 {
            continue;
        }
        let text = glyphs.iter().filter(|g| inside(centre(g.rect), rect, 1.0)).count() as f32;
        // A short image strip (badges and their labels) stays one picture;
        // its dense tiny lettering is artwork rather than a body paragraph.
        let label_strip = c.images > 0 && fh <= 30.0 && fw > 3.0 * fh && text <= 300.0;
        let inside: Vec<_> = glyphs.iter().filter(|g| inside(centre(g.rect), rect, 1.0)).collect();
        let narrative = inside.iter().filter(|g| body.is_some_and(|(font, size)| g.font == font && (g.size * 2.0 - size as f32).abs() < 1.0)).count();
        let labelled = labels && c.rich && c.members >= 2 && area >= 1500.0
            && inside.iter().all(|g| !g.invisible) && narrative as f32 <= 0.25 * text
            && text <= (0.018 * area).max(80.0);
        let figure = label_strip || labelled || if c.rich { text <= (0.004 * area).max(40.0) } else { c.seeds >= 3 && text <= 0.0015 * area };
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

/// A row's label and value: one box each side of `mid`.
fn pair(row: &[usize], rects: &[Rect], mid: f32) -> Option<(usize, usize)> {
    let [a, b] = row else { return None };
    let (l, r) = if rects[*a][0] <= rects[*b][0] { (*a, *b) } else { (*b, *a) };
    (rects[l][2] <= mid && rects[r][0] >= mid).then_some((l, r))
}

/// Whether the rows a band parts are labels and their values (a receipt,
/// a manual's list) rather than two columns of text or a table: every row
/// a narrow label with its value close after it, or most rows a box each
/// side with the values set flush right, their starts ragged.
fn key_value(rows: &[Vec<usize>], rects: &[Rect], mid: f32, column: [f32; 2]) -> bool {
    let pairs: Vec<(usize, usize)> = rows.iter().filter_map(|r| pair(r, rects, mid)).collect();
    // Every row a narrow label and its value (a manual's "Changes
    // settings: Yes"), however few: two columns of text are not narrow.
    // The values start soon after the labels: a gutter wider than the
    // labels' column parts columns.
    let label_w = pairs.iter().map(|&(l, _)| rects[l][2] - column[0]).fold(0.0, f32::max);
    let gap = pairs.iter().map(|&(_, r)| rects[r][0]).fold(f32::INFINITY, f32::min) - pairs.iter().map(|&(l, _)| rects[l][2]).fold(f32::NEG_INFINITY, f32::max);
    let narrow = pairs.len() >= 2 && pairs.len() == rows.len() && label_w <= 0.3 * (column[1] - column[0]) && gap < label_w;
    if narrow {
        return true;
    }
    if pairs.len() < 3 || pairs.len() * 3 < rows.len() * 2 {
        return false;
    }
    let mut heights: Vec<f32> = pairs.iter().flat_map(|&(l, r)| [rects[l][3] - rects[l][1], rects[r][3] - rects[r][1]]).collect();
    heights.sort_by(f32::total_cmp);
    let h = heights[heights.len() / 2].max(1.0);
    let spread = |xs: &mut dyn Iterator<Item = f32>| {
        let (lo, hi) = xs.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)));
        hi - lo
    };
    // Values set flush right, their starts ragged (a receipt): a table's
    // columns start level.
    spread(&mut pairs.iter().map(|&(_, r)| rects[r][2])) <= 0.5 * h && spread(&mut pairs.iter().map(|&(_, r)| rects[r][0])) >= 2.0 * h
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
        if !left.is_empty() && !right.is_empty() && key_value(&rows[start..j], rects, mid, column) {
            // A label and its value a row: each row a group of its own (a
            // row with both is one line, the value behind a tab).
            flush(&rows[from..start], out);
            let mut loose: Vec<Vec<usize>> = Vec::new();
            for row in &rows[start..j] {
                if let Some((l, r)) = pair(row, rects, mid) {
                    flush(&loose, out);
                    loose.clear();
                    out.push(Group { column: [column[0], band[0]], members: vec![l] });
                    out.push(Group { column: [band[1], column[1]], members: vec![r] });
                } else {
                    loose.push(row.clone());
                }
            }
            flush(&loose, out);
            from = j;
        } else if !left.is_empty() && !right.is_empty() {
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
        || ('\u{f000}'..='\u{f0ff}').contains(&c) || ('\u{f0000}'..='\u{ffffd}').contains(&c)
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
    // The lower quartile, not the median: a paragraph's lines are the
    // closest, and lone lines spaced apart (a list of values beside their
    // labels) must not make its pitch the usual one.
    let usual = (!pitches.is_empty()).then(|| {
        let mut p = pitches;
        p.sort_by(f32::total_cmp);
        p[(p.len() - 1) / 4]
    });
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
            // A few words a line, spaced apart: labels or values a line
            // each, not a paragraph (prose double spaced has long lines).
            let words = |l: &Line| 1 + l.glyphs.iter().skip(1).filter(|g| g.space_before).count();
            let listed = pitch > 1.6 * s && words(prev) <= 3 && words(line) <= 3 && !soft;
            let breaks = pitch <= 0.3 * s
                || listed
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
    /// Two columns, the left one labels each at the top of a part of the
    /// right one (`side_heads`).
    pub side: bool,
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
    if members.iter().any(|&(_, k)| matches!(&items[k], Item::Para(p) if p.lines.iter().flat_map(|l| &l.glyphs).all(|g| g.source < -1))) {
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
        .filter(|g| matches!(g.kind, super::GraphicKind::Path { curved: false }) && (g.rect[2] - g.rect[0] > 24.0 || g.rect[3] - g.rect[1] > 24.0) && g.rect[0] >= rect[0] - 12.0 && g.rect[2] <= rect[2] + 12.0 && g.rect[1] >= rect[1] - 12.0 && g.rect[3] <= rect[3] + 12.0)
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

/// Whether two columns are short labels, each at the top of a part of the
/// other, wider column (a manual's "Description", "Example" beside their
/// text): `members` are (column, item).
pub fn side_heads(items: &[Item], cols: &[[f32; 2]], members: &[(usize, usize)]) -> bool {
    let [left, right] = cols else { return false };
    if left[1] - left[0] > 0.35 * (right[1] - left[0]) {
        return false;
    }
    let labels: Vec<&Para> = members
        .iter()
        .filter(|m| m.0 == 0)
        .filter_map(|m| match &items[m.1] {
            Item::Para(p) => Some(p),
            Item::Figure(_) => None,
        })
        .collect();
    let body: Vec<[f32; 4]> = members.iter().filter(|m| m.0 == 1).map(|m| items[m.1].rect()).collect();
    let n_labels = members.iter().filter(|m| m.0 == 0).count();
    // Parts of text beside the labels, not a value a label (a receipt's
    // lines, a formula after the words that lead to it): a figure counts
    // as a line.
    let body_lines: usize = members
        .iter()
        .filter(|m| m.0 == 1)
        .map(|m| match &items[m.1] {
            Item::Para(p) => p.lines.len(),
            Item::Figure(_) => 1,
        })
        .sum();
    if labels.is_empty() || labels.len() < n_labels || body.is_empty() || body_lines < n_labels + 2 {
        return false;
    }
    // Words, not a list's bullets or numbers set apart from their items.
    let label = |p: &Para| {
        let glyphs = || p.lines.iter().flat_map(|l| &l.glyphs);
        p.lines.len() <= 2 && glyphs().count() <= 40 && glyphs().filter(|g| g.ch.is_alphabetic()).count() >= 2
    };
    if !labels.iter().all(|p| label(p)) {
        return false;
    }
    let aligned = labels.iter().filter(|p| body.iter().any(|r| (r[1] - p.rect[1]).abs() < 1.2 * p.size)).count();
    aligned * 3 >= labels.len() * 2
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
        Segment { columns: vec![column], items: range.map(|k| (0, k)).collect(), table: None, line: false, side: false }
    };
    let mut out: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        // A row of three or more one-line parts on one level, left to
        // right (a register's bit numbers): one line. Not a table's head,
        // whose parts' edges the rows under it share.
        if let Item::Para(first) = &items[i]
            && first.lines.len() == 1
        {
            let top = first.rect[1];
            let mut j = i + 1;
            while let Some(Item::Para(p)) = items.get(j)
                && p.lines.len() == 1
                && (p.rect[1] - top).abs() < 0.5 * first.size
                && p.rect[0] >= items[j - 1].rect()[2] - 1.0
            {
                j += 1;
            }
            let edges: Vec<f32> = (i + 1..j).map(|k| columns[k][0]).collect();
            let head = items[j..].iter().take(2 * (j - i)).any(|it| edges.iter().any(|e| (it.column()[0] - e).abs() < 1.0));
            if j - i >= 3 && !head {
                out.push(Segment { columns: (i..j).map(|k| columns[k]).collect(), items: (i..j).enumerate().collect(), table: None, line: true, side: false });
                i = j;
                continue;
            }
        }
        if !side(columns[i]) {
            let mut j = i + 1;
            while j < items.len() && !side(columns[j]) {
                j += 1;
            }
            out.push(single(i..j));
            i = j;
            continue;
        }
        // Side heads: labels in a narrow column on the left, the rest to its
        // right, in columns of its own or not (a note's label beside it).
        {
            let labels = columns[i];
            let label = |c: [f32; 2]| (c[0] - labels[0]).abs() < 1.0;
            let mut label_col = labels;
            let mut right: Option<[f32; 2]> = None;
            let mut run = vec![(0, i)];
            let mut j = i + 1;
            while j < items.len() {
                let c = columns[j];
                if c[0] >= label_col[1] - 1.0 {
                    right = Some(right.map_or(c, |r| [r[0].min(c[0]), r[1].max(c[1])]));
                    run.push((1, j));
                } else if label(c) && right.is_none_or(|r| c[1] < r[0] - 1.0) {
                    label_col = [label_col[0], label_col[1].max(c[1])];
                    run.push((0, j));
                } else {
                    break;
                }
                j += 1;
            }
            if let Some(right) = right
                && side_heads(items, &[label_col, right], &run)
            {
                out.push(Segment { columns: vec![label_col, right], items: run, table: None, line: false, side: true });
                i = j;
                continue;
            }
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
        let first = items[members[0].1].rect();
        let first_size = match &items[members[0].1] {
            Item::Para(p) if p.lines.len() == 1 => Some(p.size),
            _ => None,
        };
        // A row of one-line parts; after the first, a picture no taller
        // than a line or two on the same line (a number set as images).
        let one_line = cols.len() >= 2
            && first_size.is_some_and(|size| {
                members.iter().all(|&(_, k)| match &items[k] {
                    Item::Para(p) => p.lines.len() == 1 && (p.rect[1] - first[1]).abs() < 0.5 * p.size,
                    Item::Figure(f) => f.rect[3] - f.rect[1] <= 2.5 * size && f.rect[1] < first[3] && f.rect[3] > first[1],
                })
            });
        if one_line {
            out.push(Segment { columns: cols, items: members, table: None, line: true, side: false });
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
                Segment { columns: vec![column], items: ks.into_iter().map(|k| (0, k)).collect(), table: None, line: false, side: false }
            };
            if !before.is_empty() {
                out.push(loose(before));
            }
            out.push(Segment { columns: cols, items: inside.iter().map(|&k| (0, k)).collect(), table: Some(table), line: false, side: false });
            if !after.is_empty() {
                out.push(loose(after));
            }
        } else if side_heads(items, &cols, &members) {
            out.push(Segment { columns: cols, items: members, table: None, line: false, side: true });
        } else if (2..=4).contains(&cols.len()) && (0..cols.len()).all(|c| lines_in(c) >= 2) {
            out.push(Segment { columns: cols, items: members, table: None, line: false, side: false });
        } else {
            // Side by side but no columns: one column, read top down (the
            // cut gave the left one first).
            let mut s = single(i..j);
            s.items.sort_by(|a, b| items[a.1].rect()[1].total_cmp(&items[b.1].rect()[1]));
            out.push(s);
        }
        i = j;
    }
    // Neighbouring one-column segments are one; so are neighbouring rows
    // of one line on one level, left to right (a register's bit numbers,
    // which the cut gave as pairs, went down a line a pair).
    let top = |s: &Segment| s.items.iter().map(|m| items[m.1].rect()[1]).fold(f32::INFINITY, f32::min);
    let line_size = |s: &Segment| s.items.iter().find_map(|m| match &items[m.1] {
        Item::Para(p) => Some(p.size),
        Item::Figure(_) => None,
    });
    let mut merged: Vec<Segment> = Vec::new();
    for s in out {
        match merged.last_mut() {
            Some(last) if last.columns.len() == 1 && s.columns.len() == 1 && last.table.is_none() && s.table.is_none() && !last.line && !s.line => {
                last.columns[0] = [last.columns[0][0].min(s.columns[0][0]), last.columns[0][1].max(s.columns[0][1])];
                last.items.extend(s.items);
            }
            Some(last)
                if last.line
                    && s.line
                    && line_size(last).is_some_and(|size| (top(last) - top(&s)).abs() < 0.5 * size)
                    && last.columns.last().is_some_and(|c| s.columns[0][0] >= c[1] - 1.0) =>
            {
                let offset = last.columns.len();
                last.columns.extend(s.columns);
                last.items.extend(s.items.into_iter().map(|(c, k)| (c + offset, k)));
            }
            _ => merged.push(s),
        }
    }
    merged.into_iter().flat_map(|s| side_pairs(s, items)).collect()
}

/// A one-column segment cut round its side heads too close to their part
/// for the cut to part them (a manual's "Change history" a few points
/// from its table): a short label, and on its top line to its right a
/// figure (a table) or a paragraph of lines, make a segment of side heads.
fn side_pairs(s: Segment, items: &[Item]) -> Vec<Segment> {
    if s.columns.len() != 1 || s.table.is_some() || s.line {
        return vec![s];
    }
    let right_edge = s.columns[0][1];
    let mut out = Vec::new();
    let mut rest: Vec<(usize, usize)> = Vec::new();
    let mut n = 0;
    while n < s.items.len() {
        let k = s.items[n].1;
        let pair = s.items.get(n + 1).map(|m| m.1).filter(|&next| {
            let Item::Para(p) = &items[k] else { return false };
            let glyphs = || p.lines.iter().flat_map(|l| &l.glyphs);
            let label = p.lines.len() <= 2 && glyphs().count() <= 40 && glyphs().filter(|g| g.ch.is_alphabetic()).count() >= 2;
            let r = items[next].rect();
            let body = match &items[next] {
                Item::Figure(_) => true,
                Item::Para(q) => q.lines.len() >= 2,
            };
            label && body && r[0] > p.rect[2] + 2.0 && (r[1] - p.rect[1]).abs() < 0.8 * p.size && r[0] - s.columns[0][0] < 0.35 * (right_edge - s.columns[0][0])
        });
        match pair {
            Some(next) => {
                if !rest.is_empty() {
                    out.push(Segment { columns: s.columns.clone(), items: std::mem::take(&mut rest), table: None, line: false, side: false });
                }
                let (l, r) = (items[k].rect(), items[next].rect());
                out.push(Segment { columns: vec![[l[0], l[2]], [r[0], right_edge.max(r[2])]], items: vec![(0, k), (1, next)], table: None, line: false, side: true });
                n += 2;
            }
            None => {
                rest.push(s.items[n]);
                n += 1;
            }
        }
    }
    if !rest.is_empty() {
        out.push(Segment { columns: s.columns, items: rest, table: None, line: false, side: false });
    }
    out
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
    joined_spans(para, marker_sep, false)
}

/// Preserve the PDF's discretionary word breaks for Word's flowing text.
pub(super) fn flowing_spans(para: &Para, marker_sep: &str) -> Vec<Span> {
    joined_spans(para, marker_sep, true)
}

/// Whether a plain hyphen between `before` (the end of a line) and `after`
/// (the start of the next) breaks one Russian word rather than joins a
/// compound: both Cyrillic, the next part in lower case, and neither a
/// particle nor a prefix that takes a hyphen.
fn broken_word(before: &str, after: &str) -> bool {
    const PARTICLES: [&str; 7] = ["то", "либо", "нибудь", "таки", "ка", "де", "с"];
    const PREFIXES: [&str; 5] = ["из", "по", "кое", "во", "кой"];
    let cyrillic = |s: &str| !s.is_empty() && s.chars().all(|c| ('\u{400}'..='\u{4ff}').contains(&c));
    cyrillic(before)
        && cyrillic(after)
        && before.chars().count() >= 2
        && after.chars().next().is_some_and(char::is_lowercase)
        && !PARTICLES.contains(&after)
        && !PREFIXES.contains(&before.to_lowercase().as_str())
}

fn joined_spans(para: &Para, marker_sep: &str, soft_hyphens: bool) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut pending: Option<&str> = None;
    for (n, line) in para.lines.iter().enumerate() {
        if n > 0 {
            let last = out.last_mut();
            match last {
                Some(span) if span.text.ends_with('\u{ad}') => {
                    if !soft_hyphens { span.text.pop(); }
                }
                Some(span) if span.text.ends_with('-') => {
                    // A Russian word broken at the line's end with a plain
                    // hyphen (a TeX book's legacy font): a soft one, or
                    // `при-менении` stood in the middle of Word's line. Not
                    // a compound's (`что-то`, `из-за`, `кое-где`).
                    let before: String = span.text[..span.text.len() - 1].chars().rev().take_while(|c| c.is_alphabetic()).collect::<Vec<_>>().into_iter().rev().collect();
                    let mut word = line.glyphs.iter().skip_while(|g| g.ch.is_whitespace());
                    let first = word.next();
                    let after: String = first.into_iter().chain(word.take_while(|g| !g.space_before)).map(|g| g.ch).take_while(|c| c.is_alphabetic()).collect();
                    if broken_word(&before, &after) {
                        span.text.pop();
                        if soft_hyphens {
                            span.text.push('\u{ad}');
                        }
                    }
                }
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
            out.push(Glyph {
                source: -1, ch: c, rect: [at, baseline - 9.0, at + 5.0, baseline + 2.5], x: at, baseline, size: 10.0, font: 0, color: [0; 3], space_before: std::mem::take(&mut space), invisible: false });
            at += 5.0;
        }
        out
    }

    fn page(glyphs: Vec<Glyph>) -> PageContent {
        PageContent { size: [600.0, 800.0], glyphs, ..Default::default() }
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
    fn a_raised_footnote_number_stays_with_its_smaller_body_text() {
        let mut marker = text("1",50.0,98.0);
        marker[0].size = 6.0;
        marker[0].rect[2] = 53.0;
        marker.extend(text("Footnote text",58.5,101.0));
        let got = lines(marker);
        assert_eq!(got.len(),1);
        assert_eq!(got[0].text(),"1 Footnote text");
    }

    #[test]
    fn a_picture_a_hair_above_the_line_goes_into_its_hole() {
        // "w = [E/h] phase", the text one piece over the fraction's hole.
        let mut glyphs = text("w =", 50.0, 100.0);
        glyphs.extend(text("phase", 74.0, 100.0));
        let mut art = text("X", 64.0, 99.6).remove(0);
        art.source = -2;
        art.rect = [64.0, 92.0, 70.0, 104.0];
        glyphs.push(art);
        let got = lines(glyphs);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].glyphs.iter().map(|g| g.ch).collect::<String>(), "w=Xphase");
    }

    #[test]
    fn a_contents_entry_is_one_line_with_its_number_and_page() {
        // "1." a tab before its entry; the page number a tab after the leader.
        let mut glyphs = text("1.", 60.0, 100.0);
        glyphs.extend(text("Thanks . . . . . . xvii", 85.0, 100.0));
        glyphs.extend(text("Chapter one . . . . .", 50.0, 120.0));
        glyphs.extend(text("1", 160.0, 120.0));
        // Columns of prose a gutter apart stay apart.
        glyphs.extend(text("left column ends...", 50.0, 140.0));
        glyphs.extend(text("right column", 170.0, 140.0));
        let got: Vec<String> = lines(glyphs).iter().map(|l| l.text()).collect();
        assert_eq!(got, ["1. Thanks . . . . . . xvii", "Chapter one . . . . . 1", "left column ends...", "right column"]);
    }

    #[test]
    fn flowing_text_retains_discretionary_hyphens() {
        let mut glyphs = text("para",50.0,100.0);
        glyphs.extend(text("\u{ad}",70.0,100.0));
        glyphs.extend(text("graph continues",50.0,112.0));
        let items = analyse(&page(glyphs),OPTS);
        let Item::Para(p) = &items[0] else {panic!()};
        assert_eq!(spans(p,"\t").iter().map(|s|s.text.as_str()).collect::<String>(),"paragraph continues");
        assert_eq!(flowing_spans(p,"\t").iter().map(|s|s.text.as_str()).collect::<String>(),"para\u{ad}graph continues");
    }

    #[test]
    fn a_plain_hyphen_breaking_a_russian_word_is_a_soft_one() {
        let mut glyphs = text("при-", 50.0, 100.0);
        glyphs.extend(text("менении что-", 50.0, 112.0));
        glyphs.extend(text("то ещё", 50.0, 124.0));
        let items = analyse(&page(glyphs), OPTS);
        let Item::Para(p) = &items[0] else { panic!() };
        assert_eq!(spans(p, "\t").iter().map(|s| s.text.as_str()).collect::<String>(), "применении что-то ещё");
        assert_eq!(flowing_spans(p, "\t").iter().map(|s| s.text.as_str()).collect::<String>(), "при\u{ad}менении что-то ещё");
        assert!(!broken_word("state", "art"));
        assert!(!broken_word("из", "за"));
    }

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

    /// `s` set at `baseline` so that it ends at `right`.
    fn flush_right(s: &str, right: f32, baseline: f32) -> Vec<Glyph> {
        let width: f32 = s.chars().map(|c| if c == ' ' { 2.5 } else { 5.0 }).sum();
        text(s, right - width, baseline)
    }

    #[test]
    fn a_receipt_s_labels_and_values_are_lines_not_columns() {
        let mut g = Vec::new();
        for (k, (label, value)) in [("Transfer", "To a card"), ("Status", "Done"), ("Sum", "87 000"), ("Fee", "1 005"), ("Sender", "Ivan Petrov")].into_iter().enumerate() {
            let y = 100.0 + 20.0 * k as f32;
            g.extend(text(label, 20.0, y));
            g.extend(flush_right(value, 250.0, y));
        }
        let items = analyse(&page(g), OPTS);
        let segs = segments(&items, &[]);
        assert_eq!(segs.len(), 5, "{segs:?}");
        assert!(segs.iter().all(|s| s.line && s.columns.len() == 2), "{segs:?}");
        assert_eq!(texts(&items).iter().filter(|t| t.as_str() == "Transfer" || t.as_str() == "To a card").count(), 2);
    }

    #[test]
    fn labels_beside_their_parts_are_side_heads() {
        let mut g = text("Description", 40.0, 100.0);
        for k in 0..3 {
            g.extend(text(&format!("The command does something useful line {k} and more"), 150.0, 100.0 + 12.0 * k as f32));
        }
        g.extend(text("Example", 40.0, 160.0));
        for k in 0..2 {
            g.extend(text(&format!("(config)> command number {k} with its output"), 150.0, 160.0 + 12.0 * k as f32));
        }
        let items = analyse(&page(g), OPTS);
        let segs = segments(&items, &[]);
        let side: Vec<&Segment> = segs.iter().filter(|s| s.side).collect();
        assert_eq!(side.len(), 1, "{segs:?}");
        assert_eq!(side[0].columns.len(), 2);
        assert_eq!(side[0].items.iter().filter(|m| m.0 == 0).count(), 2);
    }

    #[test]
    fn short_lines_spaced_apart_are_paragraphs_of_their_own() {
        let mut g = text("Yes", 40.0, 100.0);
        g.extend(text("No", 40.0, 122.0));
        g.extend(text("Interface type", 40.0, 144.0));
        assert_eq!(texts(&analyse(&page(g), OPTS)), ["Yes", "No", "Interface type"]);
    }

    #[test]
    fn a_number_set_a_digit_a_picture_is_a_figure() {
        let mut content = page(text("Card", 20.0, 243.0));
        for k in 0..16 {
            let x = 142.0 + 6.0 * k as f32 + if k >= 4 { 4.0 } else { 0.0 };
            content.graphics.push(Graphic { rect: [x, 235.0, x + 5.0, 245.0], kind: GraphicKind::Image, object: None });
        }
        let found = figures(&content, &content.glyphs);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].rect[0] <= 142.0 && found[0].rect[2] >= 230.0);
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
    fn a_leader_of_small_dots_spaced_wide_is_in_its_line() {
        let mut g = text("12.4. The role of dead ends", 95.0, 140.0);
        for k in 0..24 {
            let x = 225.0 + 11.0 * k as f32;
            let mut dot = text(".", x, 140.0);
            dot[0].rect = [x, 135.4, x + 4.0, 140.0];
            dot[0].size = 4.0;
            g.extend(dot);
        }
        g.extend(text("222", 492.0, 140.0));
        let found = lines(g);
        assert_eq!(found.len(), 1, "{:?}", found.iter().map(|l| l.text()).collect::<Vec<_>>());
    }

    #[test]
    fn a_row_of_labels_is_one_line() {
        let mut g = text("A paragraph above the register that runs across the whole width of the page", 40.0, 60.0);
        for (k, s) in ["Bit 7", "6", "5", "4", "3", "2", "1", "Bit 0"].into_iter().enumerate() {
            g.extend(text(s, 50.0 + 40.0 * k as f32, 100.0));
        }
        g.extend(text("A paragraph below the register that runs across the whole width of the page", 40.0, 140.0));
        let items = analyse(&page(g), OPTS);
        let segs = segments(&items, &[]);
        let row: Vec<&Segment> = segs.iter().filter(|s| s.items.iter().any(|m| (items[m.1].rect()[1] - 92.0).abs() < 3.0)).collect();
        assert_eq!(row.len(), 1, "{segs:?}");
        assert_eq!(row[0].items.len(), 8, "{segs:?}");
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
    fn a_badge_strip_keeps_dense_labels_in_its_picture() {
        let mut content = page(text("A badge strip has dense tiny text inside its image", 50.0, 100.0));
        content.graphics.push(Graphic { rect: [45.0, 90.0, 400.0, 101.25], kind: GraphicKind::Image, object: Some(0) });
        let items = analyse(&content, OPTS);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Item::Figure(_)));
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
