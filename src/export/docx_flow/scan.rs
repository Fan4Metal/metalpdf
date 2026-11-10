//! The figures of a scanned page with a recognised text layer.
//!
//! A scan with OCR (ABBYY FineReader, a scanner's searchable PDF) is a
//! picture of the whole page with the text, invisible, laid over it. The
//! text makes the paragraphs; the picture, page-wide, was left out as a
//! background, and every figure in it with it. The figures are what ink
//! the page has away from the text: the page rendered small, the text's
//! lines blanked out, the ink left gathered into regions.
use crate::export::{Glyph, GraphicKind, PageContent, layout};
use crate::pdfium::Page;

type Rect = [f32; 4];

/// The rendering's pixels a point.
const SCALE: f32 = 1.0;
/// The grid the ink is gathered on, in pixels.
const CELL: usize = 4;

/// Whether the page is a scan with its text recognised: a picture over
/// nearly all of it, and most of its text invisible.
pub(super) fn is_scan(content: &PageContent) -> bool {
    let [w, h] = content.size;
    let page = (w * h).max(1.0);
    let covered = content.graphics.iter().any(|g| {
        let r = g.rect;
        g.kind == GraphicKind::Image && (r[2].min(w) - r[0].max(0.0)) * (r[3].min(h) - r[1].max(0.0)) >= 0.85 * page
    });
    let text: Vec<&Glyph> = content.glyphs.iter().filter(|g| !g.ch.is_whitespace()).collect();
    covered && text.len() >= 20 && text.iter().filter(|g| g.invisible).count() * 5 >= text.len() * 3
}

/// The figures of a scanned page (`is_scan`), in view space.
pub(super) fn figures(page: &Page, content: &PageContent) -> Vec<Rect> {
    if !is_scan(content) {
        return Vec::new();
    }
    let [w, h] = content.size;
    let (pw, ph) = (((w * SCALE).round() as u32).max(1), ((h * SCALE).round() as u32).max(1));
    let Some(bitmap) = page.render_whole(pw, ph, content.turns) else { return Vec::new() };
    let px = bitmap.pixels();
    let stride = px.len() / ph as usize;
    let grey: Vec<u8> = (0..ph as usize)
        .flat_map(|y| (0..pw as usize).map(move |x| (y, x)))
        .map(|(y, x)| {
            let p = &px[y * stride + 4 * x..y * stride + 4 * x + 3];
            ((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8
        })
        .collect();
    let lines: Vec<Rect> = layout::lines(content.glyphs.iter().filter(|g| !g.ch.is_whitespace()).cloned().collect())
        .iter()
        .map(|l| [l.rect[0] - 0.5 * l.size, l.rect[1] - 0.3 * l.size, l.rect[2] + 0.5 * l.size, l.rect[3] + 0.3 * l.size])
        .collect();
    ink_regions(&grey, pw as usize, ph as usize, content.size, &lines)
}

/// The regions of ink in `grey` (`pw` × `ph`, a page of `size` points)
/// away from `text` (rectangles in points).
fn ink_regions(grey: &[u8], pw: usize, ph: usize, size: [f32; 2], text: &[Rect]) -> Vec<Rect> {
    let to_px = [pw as f32 / size[0].max(1.0), ph as f32 / size[1].max(1.0)];
    // The paper: the brightness most of the page is at least as bright as.
    let mut histogram = [0usize; 256];
    grey.iter().for_each(|&v| histogram[v as usize] += 1);
    let mut seen = 0;
    let paper = (0..256).rev().find(|&v| {
        seen += histogram[v];
        seen * 2 >= grey.len()
    }).unwrap_or(255) as i32;
    let ink_below = (paper - 70).max(40);
    let mut blank = vec![false; pw * ph];
    for r in text {
        let (x0, x1) = (((r[0] * to_px[0]).floor().max(0.0) as usize).min(pw), ((r[2] * to_px[0]).ceil().max(0.0) as usize).min(pw));
        let (y0, y1) = (((r[1] * to_px[1]).floor().max(0.0) as usize).min(ph), ((r[3] * to_px[1]).ceil().max(0.0) as usize).min(ph));
        for y in y0..y1 {
            blank[y * pw + x0..y * pw + x1].iter_mut().for_each(|b| *b = true);
        }
    }
    // Ink a cell.
    let (cw, ch) = (pw.div_ceil(CELL), ph.div_ceil(CELL));
    let mut ink = vec![0u16; cw * ch];
    for y in 0..ph {
        for x in 0..pw {
            if !blank[y * pw + x] && (grey[y * pw + x] as i32) < ink_below {
                ink[(y / CELL) * cw + x / CELL] += 1;
            }
        }
    }
    let on: Vec<bool> = ink.iter().map(|&n| n >= 2).collect();
    // Cells within two of one another are one region.
    const REACH: usize = 2;
    let mut region = vec![usize::MAX; cw * ch];
    let mut out = Vec::new();
    for start in 0..cw * ch {
        if !on[start] || region[start] != usize::MAX {
            continue;
        }
        let id = out.len();
        let mut box_ = [start % cw, start / cw, start % cw, start / cw];
        let mut members = Vec::new();
        let mut stack = vec![start];
        region[start] = id;
        while let Some(c) = stack.pop() {
            let (x, y) = (c % cw, c / cw);
            members.push((x, y));
            box_ = [box_[0].min(x), box_[1].min(y), box_[2].max(x), box_[3].max(y)];
            for ny in y.saturating_sub(REACH)..(y + REACH + 1).min(ch) {
                for nx in x.saturating_sub(REACH)..(x + REACH + 1).min(cw) {
                    let n = ny * cw + nx;
                    if on[n] && region[n] == usize::MAX {
                        region[n] = id;
                        stack.push(n);
                    }
                }
            }
        }
        // Ink only along the edges of its box, nothing inside: a frame
        // round text, a bracket, a frame's corner; not a figure.
        let inner = members.iter().filter(|&&(x, y)| x > box_[0] + 2 && x + 2 < box_[2] && y > box_[1] + 2 && y + 2 < box_[3]).count();
        if inner * 10 >= members.len() {
            out.push((box_, members.len()));
        }
    }
    let (w, h) = (size[0], size[1]);
    out.into_iter()
        .filter_map(|(b, cells)| {
            let r = [
                (b[0] * CELL) as f32 / to_px[0] - 2.0,
                (b[1] * CELL) as f32 / to_px[1] - 2.0,
                ((b[2] + 1) * CELL) as f32 / to_px[0] + 2.0,
                ((b[3] + 1) * CELL) as f32 / to_px[1] + 2.0,
            ];
            let r = [r[0].max(0.0), r[1].max(0.0), r[2].min(w), r[3].min(h)];
            let (rw, rh) = (r[2] - r[0], r[3] - r[1]);
            // A scan's shadow down a page's edge, a rule, specks: not figures.
            let edge = r[0] < 0.015 * w || r[1] < 0.015 * h || r[2] > 0.985 * w || r[3] > 0.985 * h;
            let thin = rw < 0.1 * w || rh < 0.1 * h;
            (rw >= 36.0 && rh >= 24.0 && cells >= 12 && !(edge && thin) && rw * rh < 0.85 * w * h).then_some(r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_s_figure_is_its_ink_away_from_the_text() {
        // A 200 × 300 page on grey paper: a text line, a drawing below it,
        // a shadow down the left edge.
        let (pw, ph) = (200, 300);
        let mut grey = vec![200u8; pw * ph];
        let mut paint = |x0: usize, y0: usize, x1: usize, y1: usize, step: usize| {
            for y in (y0..y1).step_by(step) {
                for x in x0..x1 {
                    grey[y * pw + x] = 30;
                }
            }
        };
        paint(20, 20, 180, 30, 2); // the text's ink
        paint(50, 100, 150, 180, 6); // a plot's grid lines
        paint(0, 0, 4, 300, 1); // the shadow
        // A frame round a text block lower down, the text blanked.
        paint(30, 200, 170, 202, 1);
        paint(30, 280, 170, 282, 1);
        paint(30, 200, 32, 282, 1);
        paint(168, 200, 170, 282, 1);
        let text = [[18.0, 18.0, 182.0, 32.0], [36.0, 206.0, 164.0, 276.0]];
        let got = ink_regions(&grey, pw, ph, [200.0, 300.0], &text);
        assert_eq!(got.len(), 1, "{got:?}");
        let r = got[0];
        assert!(r[0] <= 50.0 && r[1] <= 100.0 && r[2] >= 150.0 && r[3] >= 175.0 && r[1] > 40.0, "{r:?}");
    }
}
