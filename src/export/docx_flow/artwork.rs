//! Keep diagrams intact and attach small artwork and fractions to text.
use crate::export::{Glyph, Graphic, GraphicKind, PageContent, layout};

pub(super) struct Inline {
    pub rect: [f32; 4],
    pub baseline: f32,
}

pub(super) struct Prepared {
    pub content: PageContent,
    pub inline: Vec<Inline>,
    pub decorations: Vec<[f32; 4]>,
    pub whole_page: bool,
}

pub(super) fn marker(index: usize) -> char {
    char::from_u32(0xf0000 + index as u32).expect("bounded artwork count")
}

/// The most a running head's or foot's rule is high: PDFium's bounds of a
/// stroked line take its width on each side, so a 0.94 pt rule is 1.88
/// high (Apex's feet, which were not taken at 1.5).
const RULE: f32 = 3.0;

fn inside(g: &Glyph, r: [f32; 4], pad: f32) -> bool {
    let (x, y) = ((g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0);
    x >= r[0] - pad && x <= r[2] + pad && y >= r[1] - pad && y <= r[3] + pad
}

fn touches(a: [f32; 4], b: [f32; 4], pad: f32) -> bool {
    a[0] <= b[2] + pad && b[0] <= a[2] + pad && a[1] <= b[3] + pad && b[1] <= a[3] + pad
}

/// A running foot's region with the artwork on its level: a logo at the
/// end of the rule or of the page number's line (Maxim's feet: left a
/// figure in the flow, it went over to a page of its own).
fn foot_art(mut region: [f32; 4], graphics: &[Graphic], size: f32, [w, h]: [f32; 2]) -> [f32; 4] {
    for o in graphics {
        let q = o.rect;
        if q[1] > 0.8 * h && q[3] - q[1] <= 4.0 * size.max(8.0) && q[2] - q[0] < 0.3 * w && q[3] > region[1] && q[1] < region[3] {
            region = layout::union(region, q);
        }
    }
    region
}

/// Whether a line is a page number, or holds one: at most 25 characters
/// with a number in them (`945`, `– 3 –`, `REV. A 3`, `xvii`).
fn page_number_like(line: &layout::Line) -> bool {
    let text = line.text();
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    let roman = text.split_whitespace().any(|w| !w.is_empty() && w.len() <= 6 && w.chars().all(|c| matches!(c, 'i' | 'v' | 'x' | 'l' | 'c' | 'I' | 'V' | 'X' | 'L' | 'C')));
    chars <= 25 && (text.chars().any(|c| c.is_ascii_digit()) || roman)
}

/// Running heads and feet with no rule: the lines at the very top or the
/// very bottom of the page, apart from the text by more than a line's gap,
/// with a page number among them. Page furniture, kept in their places
/// like those with rules; left in the flow, a page number went over to a
/// page of its own whenever the body came out a line longer in Word.
fn furniture_without_rules(content: &mut PageContent, decorations: &mut Vec<[f32; 4]>, size: f32) {
    let [w, h] = content.size;
    // On the page: a page cropped smaller than its sheet has text outside.
    let lines: Vec<layout::Line> = layout::lines(content.glyphs.iter().filter(|g| !g.invisible && g.rect[1] >= 0.0 && g.rect[3] <= h && g.rect[0] >= 0.0 && g.rect[2] <= w).cloned().collect());
    // A page that is mostly one figure (screenshots with text over them)
    // keeps its text with the figure.
    if lines.len() < 3 || layout::labelled_figures(content).iter().any(|f| (f.rect[2] - f.rect[0]) * (f.rect[3] - f.rect[1]) > 0.5 * w * h) {
        return;
    }
    for bottom in [true, false] {
        // The band: from the edge-most line inwards, the lines close on
        // one another (side by side, or a line under another).
        let mut order: Vec<&layout::Line> = lines.iter().collect();
        order.sort_by(|a, b| if bottom { b.rect[3].total_cmp(&a.rect[3]) } else { a.rect[1].total_cmp(&b.rect[1]) });
        let mut band: Vec<&layout::Line> = vec![order[0]];
        let mut region = order[0].rect;
        for l in &order[1..] {
            let apart = if bottom { region[1] - l.rect[3] } else { l.rect[1] - region[3] };
            if apart > 0.6 * l.size {
                break;
            }
            band.push(l);
            region = layout::union(region, l.rect);
        }
        let in_place = if bottom { region[1] > 0.86 * h } else { region[3] < 0.14 * h };
        let rest: Vec<&layout::Line> = lines.iter().filter(|l| !band.iter().any(|b| std::ptr::eq(*b, *l))).collect();
        let gap = if bottom { region[1] - rest.iter().map(|l| l.rect[3]).fold(f32::MIN, f32::max) } else { rest.iter().map(|l| l.rect[1]).fold(f32::MAX, f32::min) - region[3] };
        if !in_place
            || rest.is_empty()
            || gap < 1.2 * size
            || band.len() > 3
            || band.iter().any(|l| l.size > 1.25 * size)
            || !band.iter().any(|l| page_number_like(l))
            || decorations.iter().any(|d| touches(*d, region, 0.0))
        {
            continue;
        }
        let mut region = [(region[0] - 1.0).max(0.0), (region[1] - 1.0).max(0.0), (region[2] + 1.0).min(w), (region[3] + 1.0).min(h)];
        if bottom {
            let with_art = foot_art(region, &content.graphics, size, [w, h]);
            if with_art != region {
                region = with_art;
                content.graphics.retain(|g| !(g.rect[0] >= region[0] - 0.5 && g.rect[1] >= region[1] - 0.5 && g.rect[2] <= region[2] + 0.5 && g.rect[3] <= region[3] + 0.5));
            }
        }
        content.glyphs.retain(|g| !inside(g, region, 0.0));
        decorations.push(region);
    }
}

/// Artwork on a page's edge: a band or a bar that runs off the sheet at
/// the top, the bottom or a side (a brochure's swoosh under its web
/// address, a thumb index down the side), wholly in the outer fifth of
/// the page and at least a quarter of it long. Page furniture, kept in
/// its place with the text over it; in the flow it was a figure, and
/// went over to a Word page of its own behind a page that came out full.
fn edge_art(content: &mut PageContent, decorations: &mut Vec<[f32; 4]>) {
    let [w, h] = content.size;
    // Each with its edge: a bar down the side and a band along the bottom
    // that meet in a corner are two (one box round both took the page).
    let mut regions: Vec<(usize, [f32; 4])> = Vec::new();
    for g in &content.graphics {
        let r = g.rect;
        let (gw, gh) = (r[2] - r[0], r[3] - r[1]);
        // At the edge, or a few points short of it (a sheet cropped
        // inside its bleed).
        let near = 6.0;
        let edge = [
            r[3] >= h - near && r[1] > 0.8 * h && gw >= 0.25 * w,
            r[1] <= near && r[3] < 0.2 * h && gw >= 0.25 * w,
            r[0] <= near && r[2] < 0.2 * w && gh >= 0.25 * h,
            r[2] >= w - near && r[0] > 0.8 * w && gh >= 0.25 * h,
        ]
        .iter()
        .position(|&e| e);
        // On the page: what lies wholly off it is not drawn.
        let r = [r[0].max(0.0), r[1].max(0.0), r[2].min(w), r[3].min(h)];
        if let Some(edge) = edge
            && r[2] - r[0] >= 1.0
            && r[3] - r[1] >= 1.0
        {
            match regions.iter_mut().find(|(e, q)| *e == edge && touches(*q, r, 0.0)) {
                Some((_, q)) => *q = layout::union(*q, r),
                None => regions.push((edge, r)),
            }
        }
    }
    let earlier = decorations.len();
    for (_, region) in regions {
        if decorations[..earlier].iter().any(|d| touches(*d, region, 0.0)) {
            continue;
        }
        content.glyphs.retain(|g| !inside(g, region, 0.0));
        // What lies in it, the parts off the sheet aside.
        content.graphics.retain(|g| {
            let r = [g.rect[0].max(0.0), g.rect[1].max(0.0), g.rect[2].min(w), g.rect[3].min(h)];
            !(r[0] >= region[0] - 0.5 && r[1] >= region[1] - 0.5 && r[2] <= region[2] + 0.5 && r[3] <= region[3] + 0.5)
        });
        decorations.push(region);
    }
}

pub(super) fn prepare(original: &PageContent, fonts: &[crate::export::FontInfo], extra: Vec<Inline>, repeated: &[[f32; 4]]) -> Prepared {
    let mut content = original.clone();
    let mut decorations = Vec::new();
    let mut counts = std::collections::HashMap::new();
    for g in &content.glyphs {
        if !g.invisible && g.ch.is_alphabetic() {
            *counts.entry((g.font, (g.size * 2.0).round() as i32)).or_insert(0usize) += 1;
        }
    }
    let narrative: Vec<_> = layout::lines(content.glyphs.clone()).into_iter().filter(|l| l.glyphs.len() >= 40 && l.rect[2] - l.rect[0] > 0.5 * content.size[0]).collect();
    if !narrative.is_empty() {
        counts.clear();
        for g in narrative.iter().flat_map(|l| &l.glyphs).filter(|g| !g.invisible && g.ch.is_alphabetic()) {
            *counts.entry((g.font, (g.size * 2.0).round() as i32)).or_insert(0usize) += 1;
        }
    }
    let (font, half) = counts.iter().max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f))).map_or((0, 22), |(f, _)| *f);
    let size = (half as f32 / 2.0).max(6.0);
    let [w, h] = content.size;
    // Running heads are independent of body reading order. Preserve the
    // number, title, and rule together, outside the flowing paragraphs.
    // Running feet likewise: a rule near the bottom with small text on and
    // under it (a page number beside the rule). Left in the flow, they went
    // over to a page of their own whenever the body came out longer.
    for g in &original.graphics {
        let r = g.rect;
        if r[2] - r[0] > 0.6 * w && r[3] - r[1] <= RULE && r[1] > 0.8 * h {
            let below: Vec<_> = original.glyphs.iter().filter(|g| (g.rect[1] + g.rect[3]) / 2.0 > r[1] - 1.0).collect();
            // A page number may be larger than a body set small (a bold
            // 10 pt "2" under 7.5 pt columns); a line of such text is not.
            let body_below = original.glyphs.iter().filter(|g| g.rect[1] > r[3] && g.size > 1.25 * size).count() > 6;
            // A foot is a few lines: a rule low on a short sheet with a
            // block of text under it (a receipt's last part) is not one.
            let deep = below.iter().map(|g| g.rect[3]).fold(r[3], f32::max) - r[3] > 4.5 * size.max(8.0);
            if !below.is_empty() && !body_below && !deep && below.iter().all(|g| !g.invisible) {
                let region = below.iter().map(|g| g.rect).fold([r[0] - 1.0, r[1] - 1.0, r[2] + 1.0, r[3] + 1.0], layout::union);
                let region = foot_art([region[0] - 1.0, region[1] - 1.0, region[2] + 1.0, region[3] + 1.0], &original.graphics, size, [w, h]);
                if region[1] > 0.75 * h {
                    content.glyphs.retain(|g| !inside(g, region, 0.0));
                    content.graphics.retain(|g| !(g.rect[0] >= region[0] - 0.5 && g.rect[1] >= region[1] - 0.5 && g.rect[2] <= region[2] + 0.5 && g.rect[3] <= region[3] + 0.5));
                    decorations.push(region);
                }
            }
        }
    }
    for g in &original.graphics {
        let r = g.rect;
        if r[2] - r[0] > 0.6 * w && r[3] - r[1] <= RULE && r[1] < 0.2 * h {
            let above: Vec<_> = original.glyphs.iter().filter(|g| g.rect[3] < r[1] + 1.0).collect();
            if !above.is_empty() && above.iter().all(|g| g.size <= 1.25 * size && !g.invisible) {
                let top = above.iter().map(|g| g.rect[1]).fold(r[1], f32::min);
                let region =
                    [r[0].min(above.iter().map(|g| g.rect[0]).fold(w, f32::min)) - 1.0, top - 1.0, r[2].max(above.iter().map(|g| g.rect[2]).fold(0.0, f32::max)) + 1.0, r[3] + 1.0];
                content.glyphs.retain(|g| !inside(g, region, 0.0));
                decorations.push(region);
            } else {
                decorations.push([r[0], r[1] - 0.75, r[2], r[3] + 0.75]);
            }
        }
    }
    content.graphics.retain(|g| !decorations.iter().any(|r| touches(g.rect, *r, 0.0) && g.rect[3] - g.rect[1] <= RULE));
    // The lines that recur on the pages around this one, at the same
    // height: running heads and feet whatever they look like (two lines of
    // an address with no number, a number a line's gap under the text).
    // Those on one level or one under another are one piece.
    let mut bands: Vec<[f32; 4]> = Vec::new();
    for &r in repeated {
        if decorations.iter().any(|d| touches(*d, r, 0.0)) || !content.glyphs.iter().any(|g| inside(g, r, 0.0)) {
            continue;
        }
        match bands.iter_mut().find(|b| touches(**b, r, 0.6 * size)) {
            Some(b) => *b = layout::union(*b, r),
            None => bands.push(r),
        }
    }
    // Not from a page with nothing else on it (a page left blank but for
    // its head and foot): the decorations hang on the first paragraph, and
    // with none the page came out empty.
    if content.glyphs.iter().all(|g| bands.iter().any(|b| inside(g, *b, 1.0))) {
        bands.clear();
    }
    for b in bands {
        let region = [(b[0] - 1.0).max(0.0), (b[1] - 1.0).max(0.0), (b[2] + 1.0).min(w), (b[3] + 1.0).min(h)];
        content.glyphs.retain(|g| !inside(g, region, 0.0));
        decorations.push(region);
    }
    // Pages consisting of a large illustration and only a few labels have
    // no body paragraph to reflow. This also preserves rotated captions.
    let art = content
        .graphics
        .iter()
        .filter(|g| {
            let r = g.rect;
            r[2] - r[0] > 8.0 && r[3] - r[1] > 8.0 && (r[2] - r[0]) * (r[3] - r[1]) < 0.85 * w * h
        })
        .map(|g| g.rect)
        .reduce(layout::union);
    let whole_page = !content.glyphs.iter().any(|g| g.invisible)
        // Counted with the running head and foot: without them, a page of
        // text short of 140 characters once its foot was taken became one
        // picture, nothing in it to edit.
        && (original.glyphs.len() < 140 && art.is_some_and(|r| (r[2] - r[0]) * (r[3] - r[1]) > 0.25 * w * h)
            || size < 9.0 && !original.glyphs.is_empty() && original.glyphs.iter().all(|g| g.rect[1] > 0.4 * h));
    if whole_page {
        // With the running head and foot found so far: a page not written
        // as one picture after all (a form) has their text nowhere else.
        return Prepared { content, inline: Vec::new(), decorations, whole_page };
    }
    edge_art(&mut content, &mut decorations);
    furniture_without_rules(&mut content, &mut decorations, size);
    // A vertical ellipsis is often three separate text objects. Three
    // ordinary Word paragraphs would multiply its height by the body pitch.
    let dots = layout::lines(content.glyphs.clone());
    for group in dots.windows(3) {
        if group.iter().all(|l| l.glyphs.len() == 1 && l.glyphs[0].ch == '.')
            && group.windows(2).all(|p| (p[0].rect[0] - p[1].rect[0]).abs() < 0.2 * size && p[1].baseline - p[0].baseline < 0.6 * size)
        {
            let mut ellipsis = group[1].glyphs[0].clone();
            ellipsis.ch = '⋮';
            ellipsis.size = size;
            ellipsis.font = font;
            ellipsis.rect = group.iter().map(|l| l.rect).reduce(layout::union).unwrap();
            content.glyphs.retain(|g| !group.iter().any(|l| l.glyphs[0] == *g));
            content.glyphs.push(ellipsis);
        }
    }
    // Ruled tables are a single composition. Their cells can span rows or
    // contain outlined formulae; XY cuts cannot recover that cell structure.
    // Keep the rules and all the cell contents together, without rasterising
    // the caption or the surrounding body paragraphs.
    let mut rule_pieces: Vec<_> =
        content.graphics.iter().filter(|g| matches!(g.kind, GraphicKind::Path { .. }) && g.rect[2] - g.rect[0] > 4.0 && g.rect[3] - g.rect[1] <= 1.5).map(|g| g.rect).collect();
    rule_pieces.sort_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])));
    let mut rules: Vec<[f32; 4]> = Vec::new();
    for r in rule_pieces {
        if let Some(last) = rules.last_mut().filter(|last| (last[1] - r[1]).abs() < 0.5 && r[0] <= last[2] + 1.5) {
            *last = layout::union(*last, r);
        } else {
            rules.push(r);
        }
    }
    rules.retain(|r| r[2] - r[0] > 0.25 * w);
    let mut tables: Vec<[f32; 4]> = Vec::new();
    for (i, top) in rules.iter().enumerate() {
        let matching: Vec<_> = rules[i..].iter().filter(|r| (r[0] - top[0]).abs() < 2.0 && (r[2] - top[2]).abs() < 2.0).collect();
        if matching.len() >= 3 {
            let bottom = matching[matching.len() - 1];
            let r = [top[0] - 0.5, top[1] - 0.5, top[2] + 0.5, bottom[3] + 0.5];
            if r[3] - r[1] > 2.0 * size
                && r[3] - r[1] < 0.65 * h
                && !tables.iter().any(|t| touches(*t, r, 0.0))
                && content.glyphs.iter().filter(|g| inside(g, r, 0.0)).count() >= 12
            {
                tables.push(r);
            }
        }
    }
    for &r in &tables {
        content.glyphs.retain(|g| !inside(g, r, 0.0));
        content.graphics.retain(|g| !(g.rect[0] >= r[0] && g.rect[2] <= r[2] && g.rect[1] >= r[1] && g.rect[3] <= r[3]));
        content.graphics.push(Graphic { rect: r, kind: GraphicKind::Image, object: None });
    }
    // Handwritten program listings have font metrics much taller than the
    // printed line pitch. Preserve those small display compositions, while
    // regular monospaced code remains editable text.
    let mut listings: Vec<[f32; 4]> = Vec::new();
    for line in layout::lines(content.glyphs.clone()) {
        if !line.glyphs.is_empty() && line.glyphs.iter().all(|g| fonts.get(g.font as usize).is_some_and(|f| f.name.to_ascii_lowercase().contains("segoeprint"))) {
            if let Some(last) = listings.last_mut().filter(|r| line.rect[1] - r[3] < 1.5 * line.size) {
                *last = layout::union(*last, line.rect);
            } else {
                listings.push(line.rect);
            }
        }
    }
    for &r in &listings {
        content.glyphs.retain(|g| !inside(g, r, 0.0));
        content.graphics.push(Graphic { rect: r, kind: GraphicKind::Image, object: None });
    }
    // A figure caption supplies a reliable lower boundary for diagrams
    // whose labels extend beyond the paths (axes, keys, database schemas).
    // Expand the artwork to its labels before the general XY analysis.
    let text_lines = layout::lines(content.glyphs.clone());
    let mut captioned = Vec::new();
    for caption in &text_lines {
        let title: String = caption.glyphs.iter().take(4).map(|g| g.ch).collect();
        if !title.starts_with("Рис.") {
            continue;
        }
        let candidates: Vec<_> = content
            .graphics
            .iter()
            .filter(|g| {
                g.rect[3] < caption.rect[1] + 1.0
                    && g.rect[1] > 0.12 * h
                    && (g.rect[2] - g.rect[0] > 4.0 * size || g.rect[3] - g.rect[1] > 3.0 * size)
                    && g.rect[3] - g.rect[1] > 0.5
            })
            .map(|g| g.rect)
            .collect();
        let Some(mut region) = candidates.iter().max_by(|a, b| a[3].total_cmp(&b[3])).copied() else { continue };
        if caption.rect[1] - region[3] > 5.0 * size {
            continue;
        }
        loop {
            let previous = region;
            for &r in &candidates {
                if r[3] >= region[1] - 1.5 * size {
                    let u = layout::union(region, r);
                    if content.glyphs.iter().filter(|g| g.font == font && (g.size - size).abs() < 0.5 && inside(g, u, 0.0)).count() < 12 {
                        region = u;
                    }
                }
            }
            if previous == region {
                break;
            }
        }
        let labels: Vec<_> = content
            .glyphs
            .iter()
            .filter(|g| {
                g.rect[1] >= region[1] - 2.0 * size
                    && g.rect[3] <= caption.rect[1]
                    && (g.font != font || (g.size - size).abs() > 0.5)
                    && !narrative.iter().any(|line| (line.baseline - g.baseline).abs() < 0.2 * size)
            })
            .collect();
        for g in labels {
            region = layout::union(region, g.rect);
        }
        if region[3] - region[1] > 3.0 * size {
            region = [(region[0] - 1.5 * size).max(0.0), (region[1] - 1.0).max(0.0), (region[2] + 1.5 * size).min(w), region[3] + 0.5];
            captioned.push(region);
        }
    }
    for &r in &captioned {
        content.glyphs.retain(|g| !inside(g, r, 0.0));
        content.graphics.retain(|g| !(g.rect[0] >= r[0] - 0.5 && g.rect[2] <= r[2] + 0.5 && g.rect[1] >= r[1] - 0.5 && g.rect[3] <= r[3] + 0.5));
        content.graphics.push(Graphic { rect: r, kind: GraphicKind::Image, object: None });
    }
    let lines = layout::lines(content.glyphs.clone());
    // What the equations found in prose first.
    let mut inline = extra;
    let mut figures = Vec::new();
    for figure in layout::labelled_figures(&content) {
        let r = figure.rect;
        let centre = (r[1] + r[3]) / 2.0;
        let mut rows: Vec<_> = lines
            .iter()
            .filter(|l| {
                l.size >= 0.7 * size
                    && l.size <= 1.3 * size
                    && l.glyphs.len() >= 3
                    && l.baseline > r[1] - 0.2 * size
                    && l.baseline < r[3] + 0.6 * size
                    && (r[0] - l.rect[2]).abs().min((r[2] - l.rect[0]).abs()) < 2.0 * size
            })
            .collect();
        rows.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
        if r[2] - r[0] < 3.0 * size && r[3] - r[1] > 1.7 * size && r[3] - r[1] < 5.0 * size && rows.len() >= 2 {
            for row in &rows {
                let piece = content
                    .graphics
                    .iter()
                    .filter(|g| g.rect[0] >= r[0] - 0.5 && g.rect[2] <= r[2] + 0.5 && g.rect[1] >= r[1] - 0.5 && g.rect[3] <= r[3] + 0.5)
                    .filter(|g| {
                        rows.iter()
                            .min_by(|a, b| {
                                let y = (g.rect[1] + g.rect[3]) / 2.0;
                                (y - a.baseline + 0.3 * size).abs().total_cmp(&(y - b.baseline + 0.3 * size).abs())
                            })
                            .is_some_and(|l| (l.baseline - row.baseline).abs() < 0.1)
                    })
                    .map(|g| g.rect)
                    .reduce(layout::union);
                if let Some(rect) = piece {
                    inline.push(Inline { rect, baseline: row.baseline });
                }
            }
            continue;
        }
        let baseline = lines
            .iter()
            .filter(|l| {
                l.size >= 0.7 * size
                    && l.size <= 1.3 * size
                    && l.glyphs.len() >= 3
                    && (centre - l.baseline + 0.3 * l.size).abs() < 0.8 * l.size
                    && (r[0] - l.rect[2]).abs().min((r[2] - l.rect[0]).abs()) < 2.0 * size
            })
            .min_by(|a, b| (centre - a.baseline).abs().total_cmp(&(centre - b.baseline).abs()))
            .map(|l| l.baseline);
        if r[2] - r[0] < 12.0 * size
            && r[3] - r[1] < 3.5 * size
            && let Some(baseline) = baseline
        {
            inline.push(Inline { rect: r, baseline });
        } else {
            figures.push(r)
        }
    }
    // Adjacent panels and plots in the same row form one composition; not
    // when the box round both would run into a third figure that neither
    // touches (a drawing in the left column and a caption drawn as outlines
    // under the right column's taller drawing: the box round the two took
    // in the text beside it, and both drawings were left out of the page).
    let mut changed = true;
    while changed {
        changed = false;
        'pairs: for i in 0..figures.len() {
            for j in i + 1..figures.len() {
                let (a, b) = (figures[i], figures[j]);
                let overlap = a[3].min(b[3]) - a[1].max(b[1]);
                let union = layout::union(a, b);
                let gap = (a[0] - b[2]).max(b[0] - a[2]).max(0.0);
                if overlap > 0.6 * (a[3] - a[1]).min(b[3] - b[1])
                    && gap < 0.2 * w
                    && !figures.iter().enumerate().any(|(k, c)| k != i && k != j && touches(*c, union, 0.0) && !touches(*c, a, 0.0) && !touches(*c, b, 0.0))
                    && !content.glyphs.iter().any(|g| g.font == font && (g.size - size).abs() < 1.0 && inside(g, union, 0.0) && !inside(g, a, 1.0) && !inside(g, b, 1.0))
                {
                    figures[i] = union;
                    figures.remove(j);
                    changed = true;
                    break 'pairs;
                }
            }
        }
    }
    // Labels just outside the drawing (a schematic's IN and OUT beside its
    // terminals, a part's name over its symbol) are the figure's: short,
    // smaller than the text, against its box. Left out, they were lines
    // of the text, out of their places, read after the figure.
    let label_lines = layout::lines(content.glyphs.clone());
    let numbered = |text: &str| {
        text.starts_with('(') && text.ends_with(')') && text.chars().any(|c| c.is_ascii_digit())
            && text.chars().all(|c| c.is_ascii_digit() || "().-–, ".contains(c) || c.is_ascii_alphabetic())
            && text.chars().count() <= 10
    };
    let punctuation_only = |text: &str| text.chars().all(|c| ",.;:".contains(c));
    // The characters of numbers and punctuation, which a figure may take
    // with it (one number's line does not keep the next one out).
    let loose: Vec<([f32; 4], char)> = label_lines
        .iter()
        .filter(|l| {
            let text: String = l.glyphs.iter().map(|g| g.ch).collect();
            numbered(&text) || punctuation_only(&text)
        })
        .flat_map(|l| l.glyphs.iter().map(|g| (g.rect, g.ch)))
        .collect();
    for r in figures.iter_mut() {
        loop {
            let before = *r;
            for l in &label_lines {
                let text: String = l.glyphs.iter().map(|g| g.ch).collect();
                let caption = ["Fig", "Рис", "Table", "Табл"].iter().any(|p| text.starts_with(p));
                let near = 0.8 * l.size.max(2.5);
                let gap_x = (r[0] - l.rect[2]).max(l.rect[0] - r[2]);
                let gap_y = (r[1] - l.rect[3]).max(l.rect[1] - r[3]);
                // Outside, or across its edge (a label half over a block
                // diagram's first box was cut in the middle of a letter).
                let within = l.rect[0] >= r[0] - 1.0 && l.rect[2] <= r[2] + 1.0 && l.rect[1] >= r[1] - 1.0 && l.rect[3] <= r[3] + 1.0;
                let outside = !within && !l.glyphs.iter().any(|g| g.invisible);
                // Short, or a longer note over or under the drawing, no
                // wider than it (a block diagram's "the output strobe may
                // strobe many DACs" under its arrow).
                let short = l.glyphs.len() <= 12 || l.glyphs.len() <= 40 && gap_y > 0.0 && l.rect[0] >= r[0] - near && l.rect[2] <= r[2] + near;
                let label = l.size < 0.9 * size && short && !caption && gap_x <= near && gap_y <= near;
                // An expression drawn in part (a sum's sign, a fraction's
                // bar) is a figure: its closing punctuation against it, and
                // its number at the right on its level, are its too.
                let punctuation = punctuation_only(&text) && gap_x <= 0.5 * l.size && gap_y <= 0.0;
                let number = numbered(&text) && l.rect[0] > 0.55 * w && gap_x > 0.0 && gap_y < -0.3 * l.size;
                if outside && (label || punctuation || number) {
                    // Not when the grown box takes in text of the body size
                    // other than the line's own and other numbers'.
                    let u = layout::union(*r, l.rect);
                    let own = |g: &Glyph| l.glyphs.iter().any(|o| o.rect == g.rect && o.ch == g.ch) || loose.iter().any(|(rect, ch)| *rect == g.rect && *ch == g.ch);
                    if !content.glyphs.iter().any(|g| g.size >= 0.9 * size && inside(g, u, 0.0) && !inside(g, *r, 1.0) && !own(g)) {
                        *r = u;
                    }
                }
            }
            if *r == before {
                break;
            }
        }
    }
    content.glyphs.retain(|g| !figures.iter().any(|r| inside(g, *r, 1.0)));
    content.graphics.retain(|g| !figures.iter().any(|r| g.rect[0] >= r[0] - 1.0 && g.rect[2] <= r[2] + 1.0 && g.rect[1] >= r[1] - 1.0 && g.rect[3] <= r[3] + 1.0));
    for &rect in &figures {
        content.graphics.push(Graphic { rect, kind: GraphicKind::Image, object: None });
    }
    let lines = layout::lines(content.glyphs.clone());
    // Small vector clusters are emoji, pictorial list markers, or bullets.
    let mut small: Vec<_> = content
        .graphics
        .iter()
        .filter(|g| {
            let r = g.rect;
            r[2] - r[0] >= 2.0
                && r[3] - r[1] >= 2.0
                && r[2] - r[0] <= 2.2 * size
                && r[3] - r[1] <= 2.2 * size
                && !figures.iter().any(|f| touches(r, *f, 0.0))
                && !inline.iter().any(|f| touches(r, f.rect, 1.0))
        })
        .map(|g| g.rect)
        .collect();
    let mut i = 0;
    while i < small.len() {
        let mut j = i + 1;
        while j < small.len() {
            let u = layout::union(small[i], small[j]);
            if touches(small[i], small[j], 1.0) && u[2] - u[0] < 2.4 * size && u[3] - u[1] < 2.4 * size {
                small[i] = u;
                small.remove(j);
                j = i + 1;
            } else {
                j += 1
            }
        }
        i += 1;
    }
    for r in small {
        let centre = (r[1] + r[3]) / 2.0;
        let line = lines
            .iter()
            .filter(|l| l.size >= 0.7 * size && l.size <= 1.4 * size && l.glyphs.len() >= 3)
            .filter(|l| (centre - l.baseline + 0.3 * l.size).abs() < 0.55 * l.size)
            .filter(|l| {
                (r[0] >= l.rect[0] - 3.0 * size && r[2] <= l.rect[2] + 3.0 * size)
                    || content.glyphs.iter().any(|g| (g.baseline - l.baseline).abs() < 0.2 * size && (g.rect[0] - r[2]).abs().min((g.rect[2] - r[0]).abs()) < size)
            })
            .min_by(|a, b| (centre - a.baseline).abs().total_cmp(&(centre - b.baseline).abs()));
        if let Some(line) = line
            && !content.glyphs.iter().any(|g| inside(g, r, -0.5))
            && (r[3] - r[1] > 0.75 * size || content.graphics.iter().any(|g| g.rect == r && g.kind == GraphicKind::Image))
        {
            inline.push(Inline { rect: [r[0] - 0.3, r[1] - 0.3, r[2] + 0.3, r[3] + 0.3], baseline: line.baseline });
        }
    }
    // Fraction bars identify structured math that must not become several
    // unrelated body lines. Retain it as an inline image at its baseline.
    for graphic in &original.graphics {
        let r = graphic.rect;
        if r[2] - r[0] < 3.0
            || r[2] - r[0] > 12.0 * size
            || r[3] - r[1] > 1.2
            || figures.iter().any(|f| touches(r, *f, 0.0))
            || inline.iter().any(|art| touches(r, art.rect, 0.0))
            || r[1] < 0.12 * h
        {
            continue;
        }
        if original.graphics.iter().any(|g| {
            let b = g.rect;
            b[0] <= r[0] + 0.1
                && b[2] >= r[2] - 0.1
                && b[1] < r[1]
                && b[3] > r[3]
                && b[2] - b[0] > 0.65 * size
                && b[2] - b[0] < 3.0 * size
                && b[3] - b[1] > 0.75 * size
                && b[3] - b[1] < 3.0 * size
        }) {
            continue;
        }
        let near: Vec<_> = content
            .glyphs
            .iter()
            .filter(|g| {
                let x = (g.rect[0] + g.rect[2]) / 2.0;
                let dy = g.baseline - r[1];
                x >= r[0] - 1.5 && x <= r[2] + 1.5 && dy > -0.75 * size && dy < 1.35 * size && (dy < 0.2 * size || dy > 0.45 * size)
            })
            .collect();
        let outlines: Vec<_> = content
            .graphics
            .iter()
            .filter(|g| g.rect[0] >= r[0] - 1.5 && g.rect[2] <= r[2] + 1.5 && g.rect[1] > r[1] - size && g.rect[3] < r[3] + 1.4 * size && g.rect[3] - g.rect[1] > 1.5)
            .collect();
        let above = near.iter().any(|g| g.baseline < r[1] + 0.2 * size) || outlines.iter().any(|g| g.rect[3] < r[1] + 0.2 * size);
        let below = near.iter().any(|g| g.baseline > r[3] + 0.4 * size) || outlines.iter().any(|g| g.rect[1] > r[3] + 0.15 * size);
        if !above || !below {
            continue;
        }
        let mut bounds = near.iter().map(|g| g.rect).fold(r, layout::union);
        bounds = outlines.iter().map(|g| g.rect).fold(bounds, layout::union);
        for g in &content.glyphs {
            if g.rect[0] >= bounds[0] - 0.6 * size && g.rect[2] <= bounds[2] + 0.6 * size && g.size > 1.2 * size && touches(g.rect, bounds, 0.0) {
                bounds = layout::union(bounds, g.rect);
            }
        }
        if bounds[3] - bounds[1] > 2.8 * size {
            continue;
        }
        let baseline = lines
            .iter()
            .filter(|l| {
                l.glyphs.len() >= 5
                    && l.size < 1.2 * size
                    && l.rect[2] >= r[0] - 2.0 * size
                    && l.rect[0] <= r[2] + 2.0 * size
                    && (l.baseline - r[1] - 0.3 * size).abs() < 0.6 * size
            })
            .max_by_key(|l| l.glyphs.len())
            .map_or(r[1] + 0.3 * size, |l| l.baseline);
        inline.push(Inline { rect: [bounds[0] - 0.5, bounds[1] - 0.5, bounds[2] + 0.5, bounds[3] + 0.5], baseline });
    }
    // Overlapping fraction pieces are one inline object.
    let mut i = 0;
    while i < inline.len() {
        let mut j = i + 1;
        while j < inline.len() {
            if touches(inline[i].rect, inline[j].rect, 0.2) {
                inline[i].rect = layout::union(inline[i].rect, inline[j].rect);
                inline.remove(j);
            } else {
                j += 1
            }
        }
        i += 1;
    }
    for (i, art) in inline.iter().enumerate() {
        // The space before the first character it stands for, before it.
        let space_before = content.glyphs.iter().filter(|g| inside(g, art.rect, 0.0)).min_by(|a, b| a.x.total_cmp(&b.x)).is_some_and(|g| g.space_before);
        content.glyphs.retain(|g| !inside(g, art.rect, 0.0));
        content.graphics.retain(|g| !(g.rect[0] >= art.rect[0] - 0.5 && g.rect[1] >= art.rect[1] - 0.5 && g.rect[2] <= art.rect[2] + 0.5 && g.rect[3] <= art.rect[3] + 0.5));
        content.glyphs.push(Glyph {
            source: -(i as i32) - 2,
            ch: marker(i),
            rect: art.rect,
            x: art.rect[0],
            baseline: art.baseline,
            size,
            font,
            color: [0; 3],
            space_before,
            invisible: false,
        });
    }
    if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
        esay!("ART font={font} size={size} figures={figures:?}");
        for (i, art) in inline.iter().enumerate() {
            esay!("INLINE {i} {:?} baseline={}", art.rect, art.baseline);
        }
    }
    Prepared { content, inline, decorations, whole_page: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str, x: f32, baseline: f32) -> Vec<Glyph> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| {
                let at = x + 5.0 * i as f32;
                Glyph {
                    source: -1,
                    ch,
                    rect: [at, baseline - 8.0, at + 5.0, baseline + 2.0],
                    x: at,
                    baseline,
                    size: 10.0,
                    font: 0,
                    color: [0; 3],
                    space_before: false,
                    invisible: false,
                }
            })
            .collect()
    }

    fn graphic(rect: [f32; 4]) -> Graphic {
        Graphic { rect, kind: GraphicKind::Path { curved: true }, object: None }
    }

    fn page(glyphs: Vec<Glyph>, graphics: Vec<Graphic>) -> PageContent {
        PageContent { size: [600.0, 800.0], glyphs, graphics, turns: 0, ..Default::default() }
    }

    fn prepare_page(p: &PageContent, fonts: &[crate::export::FontInfo]) -> Prepared {
        prepare(p, fonts, Vec::new(), &[])
    }

    #[test]
    fn the_running_head_and_rule_leave_body_paragraphs_alone() {
        let mut glyphs = text("17 Chapter", 50.0, 40.0);
        glyphs.extend(text("An ordinary editable body paragraph", 50.0, 90.0));
        let p = prepare_page(&page(glyphs, vec![graphic([50.0, 50.0, 550.0, 51.0])]), &[]);
        assert_eq!(p.decorations.len(), 1);
        assert!(!p.whole_page);
        assert_eq!(p.content.glyphs.iter().map(|g| g.ch).collect::<String>(), "An ordinary editable body paragraph");
    }

    #[test]
    fn the_running_foot_and_its_page_number_are_page_furniture() {
        let mut glyphs = text("An ordinary editable body paragraph", 50.0, 90.0);
        glyphs.extend(text("406", 20.0, 748.0));
        glyphs.extend(text("Reference manual", 400.0, 752.0));
        let p = prepare_page(&page(glyphs, vec![graphic([50.0, 742.0, 550.0, 742.5])]), &[]);
        assert_eq!(p.decorations.len(), 1);
        assert!(p.decorations[0][0] <= 20.0 && p.decorations[0][3] >= 754.0);
        assert_eq!(p.content.glyphs.iter().map(|g| g.ch).collect::<String>(), "An ordinary editable body paragraph");
    }

    #[test]
    fn a_thick_foot_rule_takes_its_larger_page_number_and_the_logo_at_its_end() {
        let mut glyphs = Vec::new();
        for k in 0..20 {
            glyphs.extend(text("An ordinary editable body paragraph line", 50.0, 100.0 + 14.0 * k as f32));
        }
        glyphs.extend(text("A company and its address", 50.0, 756.0));
        // The number larger than the body, under the foot's line.
        glyphs.extend(text("2", 50.0, 772.0).into_iter().map(|mut g| {
            g.size = 14.0;
            g
        }));
        // A 0.94 pt rule as PDFium bounds it, a logo at its end, a figure.
        let graphics = vec![graphic([50.0, 743.0, 470.0, 744.88]), graphic([472.0, 730.0, 550.0, 752.0]), graphic([50.0, 420.0, 300.0, 600.0])];
        let p = prepare_page(&page(glyphs, graphics), &[]);
        assert_eq!(p.decorations.len(), 1);
        assert!(p.decorations[0][2] >= 550.0 && p.decorations[0][3] >= 772.0);
        assert!(!p.content.glyphs.iter().any(|g| g.baseline > 700.0));
        assert_eq!(p.content.graphics.len(), 1);
        assert!(!p.whole_page);
    }

    #[test]
    fn a_row_of_figures_does_not_run_into_a_third() {
        let mut glyphs = Vec::new();
        for k in 0..8 {
            glyphs.extend(text("An ordinary editable body paragraph line here", 50.0, 60.0 + 14.0 * k as f32));
        }
        // A tall drawing in the right column, a drawing lower in the left
        // one, and a strip of outlines (a caption) under the right one, on
        // the left one's level.
        let graphics = vec![graphic([340.0, 40.0, 520.0, 210.0]), graphic([50.0, 180.0, 260.0, 280.0]), graphic([312.0, 268.0, 522.0, 276.0])];
        let p = prepare_page(&page(glyphs, graphics), &[]);
        let figures: Vec<_> = p.content.graphics.iter().filter(|g| g.kind == GraphicKind::Image).map(|g| g.rect).collect();
        assert!(figures.iter().any(|r| r[0] < 60.0 && r[2] < 300.0), "{figures:?}");
        assert_eq!(p.content.glyphs.len(), 8 * 45);
    }

    #[test]
    fn a_schematic_s_labels_outside_its_lines_are_its_own() {
        let small = |s: &str, x: f32, baseline: f32| -> Vec<Glyph> {
            text(s, x, baseline).into_iter().enumerate().map(|(i, mut g)| {
                let at = x + 3.5 * i as f32;
                g.rect = [at, baseline - 5.0, at + 3.5, baseline + 1.5];
                g.x = at;
                g.size = 6.0;
                g
            }).collect()
        };
        let mut glyphs = Vec::new();
        for k in 0..6 {
            glyphs.extend(text("An ordinary editable body paragraph line above the figure", 50.0, 60.0 + 14.0 * k as f32));
        }
        glyphs.extend(small("IN", 190.0, 252.0));
        glyphs.extend(small("OUT", 304.0, 252.0));
        glyphs.extend(small("C1", 240.0, 196.0));
        glyphs.extend(text("Figure 1: A filter", 200.0, 330.0));
        let graphics = vec![graphic([198.0, 200.0, 260.0, 300.0]), graphic([250.0, 240.0, 302.0, 300.0])];
        let p = prepare_page(&page(glyphs, graphics), &[]);
        let left: String = p.content.glyphs.iter().filter(|g| g.size < 9.0).map(|g| g.ch).collect();
        assert_eq!(left, "");
        assert!(p.content.glyphs.iter().any(|g| g.ch == 'F'));
    }

    #[test]
    fn an_expression_drawn_in_part_takes_its_punctuation_and_number() {
        let mut glyphs = Vec::new();
        for k in 0..6 {
            glyphs.extend(text("An ordinary editable body paragraph line above the figure", 50.0, 60.0 + 14.0 * k as f32));
        }
        glyphs.extend(text(";", 301.0, 220.0));
        glyphs.extend(text("(12.8)", 500.0, 220.0));
        glyphs.extend(text("(12.9)", 500.0, 260.0));
        for k in 0..6 {
            glyphs.extend(text("An ordinary editable body paragraph line below the figure", 50.0, 320.0 + 14.0 * k as f32));
        }
        let graphics = vec![graphic([200.0, 200.0, 260.0, 280.0]), graphic([250.0, 210.0, 300.0, 280.0])];
        let p = prepare_page(&page(glyphs, graphics), &[]);
        let left: String = p.content.glyphs.iter().filter(|g| g.baseline > 150.0 && g.baseline < 300.0).map(|g| g.ch).collect();
        assert_eq!(left, "");
    }

    #[test]
    fn artwork_off_the_sheet_s_edge_is_page_furniture() {
        let mut glyphs = Vec::new();
        for k in 0..20 {
            glyphs.extend(text("An ordinary editable body paragraph line", 50.0, 100.0 + 14.0 * k as f32));
        }
        glyphs.extend(text("www.example.com", 150.0, 785.0));
        glyphs.extend(text("295", 570.0, 760.0));
        // A swoosh under the address, a bar down the side, a figure.
        // And one wholly off the sheet, under it.
        let graphics = vec![graphic([20.0, 760.0, 450.0, 810.0]), graphic([560.0, 10.0, 600.0, 790.0]), graphic([50.0, 420.0, 300.0, 600.0]), graphic([20.0, 805.0, 400.0, 830.0])];
        let p = prepare_page(&page(glyphs, graphics), &[]);
        assert_eq!(p.decorations.len(), 2);
        assert!(p.decorations.iter().all(|d| d[2] > d[0] && d[3] > d[1]));
        assert!(!p.content.glyphs.iter().any(|g| g.baseline >= 760.0));
        assert_eq!(p.content.graphics.len(), 1);
        // A bar down the side meeting a band along the bottom: two, the
        // text between them left in the flow.
        let mut glyphs = Vec::new();
        for k in 0..20 {
            glyphs.extend(text("An ordinary editable body paragraph line", 80.0, 100.0 + 14.0 * k as f32));
        }
        let p = prepare_page(&page(glyphs, vec![graphic([3.0, 4.0, 64.0, 797.0]), graphic([52.0, 754.0, 492.0, 803.0])]), &[]);
        assert_eq!(p.decorations.len(), 2);
        assert_eq!(p.content.glyphs.len(), 20 * 40);
        // Small marks at an edge are not a band.
        let p = prepare_page(&page(text("An ordinary editable body paragraph line", 50.0, 100.0), vec![graphic([20.0, 790.0, 60.0, 800.0])]), &[]);
        assert!(p.decorations.is_empty());
    }

    #[test]
    fn a_page_number_with_no_rule_is_page_furniture() {
        let body = |glyphs: &mut Vec<Glyph>| {
            for k in 0..20 {
                glyphs.extend(text("An ordinary editable body paragraph line", 50.0, 100.0 + 14.0 * k as f32));
            }
        };
        let mut glyphs = Vec::new();
        body(&mut glyphs);
        glyphs.extend(text("945", 290.0, 770.0));
        let p = prepare_page(&page(glyphs, Vec::new()), &[]);
        assert_eq!(p.decorations.len(), 1);
        assert!(!p.content.glyphs.iter().any(|g| g.baseline == 770.0));
        // A last short line with no number in it is the text's.
        let mut glyphs = Vec::new();
        body(&mut glyphs);
        glyphs.extend(text("See the appendix.", 50.0, 770.0));
        let p = prepare_page(&page(glyphs, Vec::new()), &[]);
        assert!(p.decorations.is_empty());
    }

    #[test]
    fn split_horizontal_rules_keep_every_table_cell_in_one_image() {
        let mut glyphs = text("Ordinary text before the table", 50.0, 100.0);
        glyphs.extend(text("A   B   C", 100.0, 225.0));
        glyphs.extend(text("1   2   3", 100.0, 265.0));
        glyphs.extend(text("Ordinary text after the table", 50.0, 350.0));
        let mut graphics = Vec::new();
        for y in [200.0, 240.0, 300.0] {
            for x in [100.0, 200.0, 300.0] {
                graphics.push(graphic([x, y, x + 100.0, y + 1.0]));
            }
        }
        let p = prepare_page(&page(glyphs, graphics), &[]);
        let pictures: Vec<_> = p.content.graphics.iter().filter(|g| g.kind == GraphicKind::Image).collect();
        assert_eq!(pictures.len(), 1);
        assert!(pictures[0].rect[0] <= 100.0 && pictures[0].rect[2] >= 400.0 && pictures[0].rect[3] >= 301.0);
        assert!(!p.content.glyphs.iter().any(|g| g.baseline > 200.0 && g.baseline < 300.0));
        assert!(p.content.glyphs.iter().any(|g| g.baseline == 350.0));
    }

    #[test]
    fn a_caption_keeps_external_axis_labels_with_the_plot() {
        let mut glyphs = text("This ordinary body paragraph stays editable before the figure", 50.0, 100.0);
        let mut label = text("axis label", 70.0, 190.0);
        label.iter_mut().for_each(|g| g.font = 1);
        glyphs.extend(label);
        let mut caption = text("Рис. 1. A plot", 100.0, 350.0);
        caption.iter_mut().for_each(|g| g.font = 1);
        glyphs.extend(caption);
        let p = prepare_page(&page(glyphs, vec![graphic([100.0, 200.0, 400.0, 320.0])]), &[]);
        assert!(!p.content.glyphs.iter().any(|g| g.baseline == 190.0));
        assert!(p.content.glyphs.iter().any(|g| g.baseline == 100.0));
        assert!(p.content.glyphs.iter().any(|g| g.baseline == 350.0));
        assert!(p.content.graphics.iter().any(|g| g.rect[0] <= 70.0 && g.rect[1] <= 182.0 && g.rect[3] >= 320.0));
    }

    #[test]
    fn outlined_fractions_are_inline_and_icons_do_not_capture_body_letters() {
        let mut glyphs = text("Before", 50.0, 250.0);
        glyphs.extend(text("after", 95.0, 250.0));
        let p = prepare_page(&page(glyphs, vec![graphic([86.0, 243.0, 92.0, 244.0]), graphic([87.0, 237.0, 91.0, 242.0]), graphic([87.0, 246.0, 91.0, 253.0])]), &[]);
        assert_eq!(p.inline.len(), 1);
        assert_eq!(p.inline[0].baseline, 250.0);
        assert!(p.inline[0].rect[1] <= 237.0 && p.inline[0].rect[3] >= 253.0);
        assert_eq!(layout::lines(p.content.glyphs).len(), 1);

        let mut glyphs = text("Before", 50.0, 250.0);
        glyphs.extend(text("after", 106.0, 250.0));
        let p = prepare_page(&page(glyphs, vec![graphic([86.0, 238.0, 100.0, 254.0]), graphic([89.0, 243.0, 96.0, 244.0])]), &[]);
        assert_eq!(p.inline.len(), 1);
        assert_eq!(p.content.glyphs.iter().filter(|g| g.source == -1).count(), 11);
    }

    #[test]
    fn a_vertical_ellipsis_is_one_text_line() {
        let mut glyphs = text("Ordinary paragraph", 50.0, 100.0);
        for y in [200.0, 204.0, 208.0] {
            glyphs.extend(text(".", 60.0, y));
        }
        let p = prepare_page(&page(glyphs, Vec::new()), &[]);
        assert_eq!(p.content.glyphs.iter().filter(|g| g.ch == '⋮').count(), 1);
        assert!(!p.content.glyphs.iter().any(|g| g.ch == '.'));
    }

    #[test]
    fn a_sequence_of_icons_separated_by_commas_keeps_every_icon() {
        let mut glyphs = text("S={", 50.0, 250.0);
        let mut graphics = Vec::new();
        for x in [70.0, 100.0, 130.0, 160.0] {
            graphics.push(graphic([x, 238.0, x + 15.0, 253.0]));
            glyphs.extend(text(",", x + 16.0, 250.0));
        }
        let p = prepare_page(&page(glyphs, graphics), &[]);
        assert_eq!(p.inline.len(), 4);
        let row = layout::lines(p.content.glyphs);
        assert_eq!(row.len(), 1, "{row:?}");
        assert_eq!(row[0].glyphs.iter().filter(|g| g.source < -1).count(), 4);
    }
}
