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

fn inside(g: &Glyph, r: [f32; 4], pad: f32) -> bool {
    let (x, y) = ((g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0);
    x >= r[0] - pad && x <= r[2] + pad && y >= r[1] - pad && y <= r[3] + pad
}

fn touches(a: [f32; 4], b: [f32; 4], pad: f32) -> bool {
    a[0] <= b[2] + pad && b[0] <= a[2] + pad && a[1] <= b[3] + pad && b[1] <= a[3] + pad
}

pub(super) fn prepare(original: &PageContent, fonts: &[crate::export::FontInfo]) -> Prepared {
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
    for g in &original.graphics {
        let r = g.rect;
        if r[2] - r[0] > 0.6 * w && r[3] - r[1] <= 1.5 && r[1] < 0.2 * h {
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
    content.graphics.retain(|g| !decorations.iter().any(|r| touches(g.rect, *r, 0.0) && g.rect[3] - g.rect[1] <= 1.5));
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
        && (content.glyphs.len() < 140 && art.is_some_and(|r| (r[2] - r[0]) * (r[3] - r[1]) > 0.25 * w * h)
            || size < 9.0 && !content.glyphs.is_empty() && content.glyphs.iter().all(|g| g.rect[1] > 0.4 * h));
    if whole_page {
        return Prepared { content, inline: Vec::new(), decorations: Vec::new(), whole_page };
    }
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
    let mut inline = Vec::new();
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
    // Adjacent panels and plots in the same row form one composition.
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
            space_before: false,
            invisible: false,
        });
    }
    if std::env::var_os("METALPDF_EXPORT_DUMP").is_some() {
        eprintln!("ART font={font} size={size} figures={figures:?}");
        for (i, art) in inline.iter().enumerate() {
            eprintln!("INLINE {i} {:?} baseline={}", art.rect, art.baseline);
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
        PageContent { size: [600.0, 800.0], glyphs, graphics, turns: 0 }
    }

    #[test]
    fn the_running_head_and_rule_leave_body_paragraphs_alone() {
        let mut glyphs = text("17 Chapter", 50.0, 40.0);
        glyphs.extend(text("An ordinary editable body paragraph", 50.0, 90.0));
        let p = prepare(&page(glyphs, vec![graphic([50.0, 50.0, 550.0, 51.0])]), &[]);
        assert_eq!(p.decorations.len(), 1);
        assert!(!p.whole_page);
        assert_eq!(p.content.glyphs.iter().map(|g| g.ch).collect::<String>(), "An ordinary editable body paragraph");
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
        let p = prepare(&page(glyphs, graphics), &[]);
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
        let p = prepare(&page(glyphs, vec![graphic([100.0, 200.0, 400.0, 320.0])]), &[]);
        assert!(!p.content.glyphs.iter().any(|g| g.baseline == 190.0));
        assert!(p.content.glyphs.iter().any(|g| g.baseline == 100.0));
        assert!(p.content.glyphs.iter().any(|g| g.baseline == 350.0));
        assert!(p.content.graphics.iter().any(|g| g.rect[0] <= 70.0 && g.rect[1] <= 182.0 && g.rect[3] >= 320.0));
    }

    #[test]
    fn outlined_fractions_are_inline_and_icons_do_not_capture_body_letters() {
        let mut glyphs = text("Before", 50.0, 250.0);
        glyphs.extend(text("after", 95.0, 250.0));
        let p = prepare(&page(glyphs, vec![graphic([86.0, 243.0, 92.0, 244.0]), graphic([87.0, 237.0, 91.0, 242.0]), graphic([87.0, 246.0, 91.0, 253.0])]), &[]);
        assert_eq!(p.inline.len(), 1);
        assert_eq!(p.inline[0].baseline, 250.0);
        assert!(p.inline[0].rect[1] <= 237.0 && p.inline[0].rect[3] >= 253.0);
        assert_eq!(layout::lines(p.content.glyphs).len(), 1);

        let mut glyphs = text("Before", 50.0, 250.0);
        glyphs.extend(text("after", 106.0, 250.0));
        let p = prepare(&page(glyphs, vec![graphic([86.0, 238.0, 100.0, 254.0]), graphic([89.0, 243.0, 96.0, 244.0])]), &[]);
        assert_eq!(p.inline.len(), 1);
        assert_eq!(p.content.glyphs.iter().filter(|g| g.source == -1).count(), 11);
    }

    #[test]
    fn a_vertical_ellipsis_is_one_text_line() {
        let mut glyphs = text("Ordinary paragraph", 50.0, 100.0);
        for y in [200.0, 204.0, 208.0] {
            glyphs.extend(text(".", 60.0, y));
        }
        let p = prepare(&page(glyphs, Vec::new()), &[]);
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
        let p = prepare(&page(glyphs, graphics), &[]);
        assert_eq!(p.inline.len(), 4);
        let row = layout::lines(p.content.glyphs);
        assert_eq!(row.len(), 1, "{row:?}");
        assert_eq!(row[0].glyphs.iter().filter(|g| g.source < -1).count(), 4);
    }
}
