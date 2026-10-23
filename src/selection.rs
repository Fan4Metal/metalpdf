//! Text selection across the pages, worked out on the UI thread from the
//! text layouts the engine delivered: which character is under the
//! pointer, which characters lie between the anchor and the focus, the
//! rectangles to paint for them (the boxes of a line merged into one) and
//! the text to copy. Characters are addressed as `(page, index)`.

use crate::pdfium::TextLayout;

/// A character's place in the document.
pub type CharPos = (usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Where the drag began.
    pub anchor: CharPos,
    /// Where the pointer is.
    pub focus: CharPos,
}

impl Selection {
    /// The selected range, first to last.
    pub fn ordered(&self) -> (CharPos, CharPos) {
        if self.anchor <= self.focus { (self.anchor, self.focus) } else { (self.focus, self.anchor) }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    /// The characters of `page` within the selection: `start..end`
    /// (exclusive), given the page's character count.
    pub fn range_on(&self, page: usize, count: usize) -> Option<std::ops::Range<usize>> {
        let ((p0, i0), (p1, i1)) = self.ordered();
        if page < p0 || page > p1 || count == 0 {
            return None;
        }
        let start = if page == p0 { i0 } else { 0 };
        let end = if page == p1 { (i1 + 1).min(count) } else { count };
        (start < end).then_some(start..end)
    }
}

/// The character of `boxes` (each `[x, y, w, h]` on the unrotated page,
/// points, y down) nearest to `point`: on the line the point is in, the
/// box under it or the nearest one along the line; off every line, the
/// line nearest vertically. `None` for a page without text.
pub fn char_at(boxes: &[[f32; 4]], point: [f32; 2]) -> Option<usize> {
    if boxes.is_empty() {
        return None;
    }
    let [x, y] = point;
    let mut best: Option<(usize, f32, f32)> = None;
    for (i, b) in boxes.iter().enumerate() {
        if b[2] <= 0.0 || b[3] <= 0.0 {
            continue;
        }
        let dy = if y < b[1] {
            b[1] - y
        } else if y > b[1] + b[3] {
            y - b[1] - b[3]
        } else {
            0.0
        };
        let dx = if x < b[0] {
            b[0] - x
        } else if x > b[0] + b[2] {
            x - b[0] - b[2]
        } else {
            0.0
        };
        // Lines first: a vertical miss weighs far more than a horizontal one.
        let score = dy * 8.0 + dx;
        if best.is_none_or(|(_, s, _)| score < s) {
            best = Some((i, score, dy));
        }
    }
    best.map(|(i, _, _)| i)
}

/// Whether `point` lies on a character's box (the pointer is over text).
pub fn over_text(boxes: &[[f32; 4]], point: [f32; 2]) -> bool {
    boxes.iter().any(|b| b[2] > 0.0 && b[3] > 0.0 && point[0] >= b[0] && point[0] <= b[0] + b[2] && point[1] >= b[1] && point[1] <= b[1] + b[3])
}

/// The boxes of characters `range` merged line by line: consecutive boxes
/// whose vertical extents overlap are one rectangle.
pub fn line_rects(boxes: &[[f32; 4]], range: std::ops::Range<usize>) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    for b in boxes.iter().skip(range.start).take(range.len()) {
        if b[2] <= 0.0 || b[3] <= 0.0 {
            continue;
        }
        if let Some(last) = out.last_mut() {
            let overlap = b[1] < last[1] + last[3] && b[1] + b[3] > last[1];
            if overlap {
                let x0 = last[0].min(b[0]);
                let y0 = last[1].min(b[1]);
                let x1 = (last[0] + last[2]).max(b[0] + b[2]);
                let y1 = (last[1] + last[3]).max(b[1] + b[3]);
                *last = [x0, y0, x1 - x0, y1 - y0];
                continue;
            }
        }
        out.push(*b);
    }
    out
}

/// The text of characters `range` of `layout`: PDFium's UTF-16 units
/// decoded, its generated line breaks kept as `\n`.
/// The word round character `index`: the first and last character
/// (inclusive) of the run of letters and digits it is in; a punctuation
/// mark is a word of its own; nothing on whitespace or a line break.
pub fn word_at(layout: &TextLayout, index: usize) -> Option<(usize, usize)> {
    let chars = &layout.chars;
    // 2 a word character, 1 a mark, 0 whitespace or nothing.
    let class = |i: usize| -> u8 {
        let Some(c) = chars.get(i) else { return 0 };
        if c.generated {
            return 0;
        }
        match char::from_u32(c.code) {
            Some(ch) if ch.is_alphanumeric() || ch == '_' => 2,
            Some(ch) if ch.is_whitespace() => 0,
            Some(_) => 1,
            None => 0,
        }
    };
    match class(index) {
        0 => None,
        1 => Some((index, index)),
        _ => {
            let mut start = index;
            while start > 0 && class(start - 1) == 2 {
                start -= 1;
            }
            let mut end = index;
            while class(end + 1) == 2 {
                end += 1;
            }
            Some((start, end))
        }
    }
}

pub fn text_of(layout: &TextLayout, range: std::ops::Range<usize>) -> String {
    let units: Vec<u16> = layout.chars[range.start.min(layout.chars.len())..range.end.min(layout.chars.len())]
        .iter()
        .map(|c| c.code as u16)
        .collect();
    let text: String = char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
    // PDFium reads a hyphen at a line's end as U+0002: the word it cuts is
    // joined again, as Acrobat copies it; elsewhere it is a hyphen.
    text.replace("\u{2}\r\n", "").replace('\u{2}', "-").replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_cut_at_a_lines_end_is_copied_whole() {
        // PDFium's mark of the hyphen at the line's end, then the break.
        let text = "элемен\u{2}\r\nтарные 1\u{2}2";
        let layout = TextLayout {
            chars: text.encode_utf16().map(|u| crate::pdfium::Char { code: u32::from(u), rect: [0.0; 4], origin: [0.0; 2], generated: u == 13 || u == 10 }).collect(),
        };
        assert_eq!(text_of(&layout, 0..layout.chars.len()), "элементарные 1-2");
    }

    #[test]
    fn a_double_click_takes_the_word() {
        let text = "Hello, wide world\r\n2026-10";
        let layout = TextLayout {
            chars: text
                .encode_utf16()
                .map(|u| crate::pdfium::Char { code: u32::from(u), rect: [0.0; 4], origin: [0.0; 2], generated: u == 13 || u == 10 })
                .collect(),
        };
        assert_eq!(word_at(&layout, 0), Some((0, 4)), "Hello");
        assert_eq!(word_at(&layout, 3), Some((0, 4)));
        assert_eq!(word_at(&layout, 5), Some((5, 5)), "the comma");
        assert_eq!(word_at(&layout, 6), None, "a space");
        assert_eq!(word_at(&layout, 12), Some((12, 16)), "world");
        assert_eq!(word_at(&layout, 17), None, "the line break");
        assert_eq!(word_at(&layout, 19), Some((19, 22)), "2026, not across the hyphen");
        assert_eq!(word_at(&layout, 24), Some((24, 25)), "10, to the end");
        assert_eq!(word_at(&layout, 99), None);
    }
    use crate::pdfium::Char;

    fn boxes() -> Vec<[f32; 4]> {
        // Two lines of three characters, 10 wide, 12 high.
        let mut out = Vec::new();
        for line in 0..2 {
            for i in 0..3 {
                out.push([10.0 + 10.0 * i as f32, 20.0 + 15.0 * line as f32, 10.0, 12.0]);
            }
        }
        out
    }

    #[test]
    fn the_nearest_character_is_found_on_its_line() {
        let b = boxes();
        assert_eq!(char_at(&b, [15.0, 25.0]), Some(0));
        assert_eq!(char_at(&b, [38.0, 25.0]), Some(2));
        // Right of the first line: its last character, not the second line.
        assert_eq!(char_at(&b, [90.0, 26.0]), Some(2));
        // Between the lines, nearer the second.
        assert_eq!(char_at(&b, [12.0, 34.5]), Some(3));
        assert_eq!(char_at(&[], [0.0, 0.0]), None);
        assert!(over_text(&b, [15.0, 25.0]));
        assert!(!over_text(&b, [90.0, 26.0]));
    }

    #[test]
    fn ranges_and_line_rectangles() {
        let sel = Selection { anchor: (1, 4), focus: (0, 1) };
        assert_eq!(sel.ordered(), ((0, 1), (1, 4)));
        assert_eq!(sel.range_on(0, 6), Some(1..6));
        assert_eq!(sel.range_on(1, 6), Some(0..5));
        assert_eq!(sel.range_on(2, 6), None);
        let one_page = Selection { anchor: (0, 1), focus: (0, 4) };
        assert_eq!(one_page.range_on(0, 6), Some(1..5));
        assert_eq!(line_rects(&boxes(), 1..5), vec![[20.0, 20.0, 20.0, 12.0], [10.0, 35.0, 20.0, 12.0]]);
    }

    #[test]
    fn text_is_decoded_with_line_breaks() {
        let codes = "ab\r\ncd".encode_utf16();
        let layout = TextLayout {
            chars: codes.map(|c| Char { code: u32::from(c), rect: [0.0; 4], origin: [0.0; 2], generated: false }).collect(),
        };
        assert_eq!(text_of(&layout, 0..6), "ab\ncd");
        assert_eq!(text_of(&layout, 4..99), "cd");
    }
}
