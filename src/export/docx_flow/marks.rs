//! Printer's marks: the crop marks of a print-ready page, in its corners
//! outside the text (LaTeX's `crop` package draws each as a quarter
//! circle of LCIRCLE10 and two hairlines, 1×1 stencil masks stretched).
//! Left in, they made the page's extent the whole sheet (margins of a
//! quarter inch, the text pushed down), Word columns of their own and a
//! table that took the running head, and the last line of the page
//! came out a letter a line.

use super::super::PageContent;

/// Take the marks out of `content`: what lies wholly in a corner square
/// of the page (an eighth of its shorter side, at most an inch), when at
/// least three corners hold a hairline and no more than a few
/// characters each (a page number in a corner is not a mark).
pub fn remove(content: &mut PageContent) {
    let [w, h] = content.size;
    let m = (0.12 * w.min(h)).min(72.0);
    let corner = |r: [f32; 4]| -> Option<usize> {
        let left = r[2] <= m;
        let right = r[0] >= w - m;
        let top = r[3] <= m;
        let bottom = r[1] >= h - m;
        match (left, right, top, bottom) {
            (true, _, true, _) => Some(0),
            (_, true, true, _) => Some(1),
            (true, _, _, true) => Some(2),
            (_, true, _, true) => Some(3),
            _ => None,
        }
    };
    let hairline = |r: [f32; 4]| {
        let (a, b) = (r[2] - r[0], r[3] - r[1]);
        a.min(b) <= 1.5 && a.max(b) >= 4.0
    };
    let mut lines = [0; 4];
    let mut chars = [0; 4];
    for g in &content.graphics {
        if let Some(c) = corner(g.rect).filter(|_| hairline(g.rect)) {
            lines[c] += 1;
        }
    }
    for r in &content.rules {
        if let Some(c) = corner(r.rect).filter(|_| hairline(r.rect)) {
            lines[c] += 1;
        }
    }
    for g in content.glyphs.iter().filter(|g| !g.invisible && !g.ch.is_whitespace()) {
        if let Some(c) = corner(g.rect) {
            chars[c] += 1;
        }
    }
    if (0..4).filter(|&c| lines[c] > 0 && chars[c] <= 4).count() < 3 {
        return;
    }
    let marked = |r: [f32; 4]| corner(r).is_some_and(|c| lines[c] > 0 && chars[c] <= 4);
    content.glyphs.retain(|g| !marked(g.rect));
    content.graphics.retain(|g| !marked(g.rect));
    content.rules.retain(|r| !marked(r.rect));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Glyph, Graphic, GraphicKind};

    fn glyph(ch: char, rect: [f32; 4]) -> Glyph {
        Glyph { source: -1, ch, rect, x: rect[0], baseline: rect[3] - 2.0, size: 10.0, font: 0, color: [0; 3], space_before: false, invisible: false }
    }

    fn hair(rect: [f32; 4]) -> Graphic {
        Graphic { rect, kind: GraphicKind::Image, object: None }
    }

    #[test]
    fn crop_marks_in_the_corners_go() {
        let mut content = PageContent { size: [612.0, 792.0], ..Default::default() };
        for (x, y) in [(18.0, 20.0), (596.0, 20.0), (18.0, 760.0), (596.0, 760.0)] {
            content.glyphs.push(glyph('i', [x, y, x + 10.0, y + 15.0]));
            content.graphics.push(hair([x - 5.0, y, x + 25.0, y + 0.5]));
        }
        // The text, and a page number near a corner but not in it.
        content.glyphs.push(glyph('A', [100.0, 100.0, 107.0, 112.0]));
        content.glyphs.push(glyph('7', [530.0, 740.0, 536.0, 752.0]));
        remove(&mut content);
        assert_eq!(content.glyphs.iter().map(|g| g.ch).collect::<String>(), "A7");
        assert!(content.graphics.is_empty());
    }

    #[test]
    fn a_rule_and_a_page_number_in_one_corner_stay() {
        let mut content = PageContent { size: [612.0, 792.0], ..Default::default() };
        content.graphics.push(hair([10.0, 770.0, 50.0, 770.5]));
        content.glyphs.push(glyph('7', [20.0, 775.0, 26.0, 787.0]));
        remove(&mut content);
        assert_eq!(content.glyphs.len(), 1);
        assert_eq!(content.graphics.len(), 1);
    }
}
