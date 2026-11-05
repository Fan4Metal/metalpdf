//! Dotted contents rows stay separate paragraphs. Their leaders are Word
//! tab leaders, not hundreds of literal periods competing with the title.

use super::{Item, Span, layout};

struct Entry {
    label_end: usize,
    page_start: usize,
    page_end: usize,
}

fn entry(line: &layout::Line) -> Option<Entry> {
    let glyphs = &line.glyphs;
    // OCR over a scan supplies approximate fonts and leader positions.
    // Keep its existing paragraph interpretation rather than reflowing
    // those estimates as if they were the visible PDF's typesetting.
    if glyphs.iter().all(|g| g.invisible) {
        return None;
    }
    let page_end = glyphs.iter().rposition(|g| !g.ch.is_whitespace())? + 1;
    let number = |c: char| c.is_ascii_digit() || "ivxlcdmIVXLCDM-–—".contains(c);
    let page_start = glyphs[..page_end].iter().rposition(|g| !number(g.ch)).map_or(0, |i| i + 1);
    let page: String = glyphs[page_start..page_end].iter().map(|g| g.ch).collect();
    if page.is_empty() || page.split(['-', '–', '—']).any(|part| {
        part.is_empty() || !(part.chars().all(|c| c.is_ascii_digit()) || part.len() <= 8 && part.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)))
    }) {
        return None;
    }
    let dots = |c| match c {
        '.' | '\u{2024}' | '·' => 1,
        '\u{2025}' => 2,
        '…' => 3,
        _ => 0,
    };
    let mut label_end = page_start;
    let mut count = 0;
    while label_end > 0 {
        let ch = glyphs[label_end - 1].ch;
        let n = dots(ch);
        if n == 0 && !ch.is_whitespace() {
            break;
        }
        count += n;
        label_end -= 1;
    }
    // Ordinary prose ellipses, decimal numbers and list markers are not
    // contents leaders. A title must precede a genuinely long leader.
    (count >= 6 && glyphs[..label_end].iter().any(|g| g.ch.is_alphabetic())).then_some(Entry { label_end, page_start, page_end })
}

/// Split only at recognised leaders. Leave the remaining text's original
/// paragraph grouping intact, and keep the column and the PDF's indents.
pub(super) fn separate(items: Vec<Item>) -> Vec<Item> {
    let mut result = Vec::new();
    for item in items {
        let Item::Para(p) = item else { result.push(item); continue };
        if !p.lines.iter().any(|line| entry(line).is_some()) {
            result.push(Item::Para(p));
            continue;
        }
        let mut pending = Vec::new();
        for line in &p.lines {
            if entry(line).is_some() {
                flush(&p, &mut pending, false, &mut result);
                pending.push(line.clone());
                flush(&p, &mut pending, true, &mut result);
            } else {
                pending.push(line.clone());
            }
        }
        flush(&p, &mut pending, false, &mut result);
    }
    result
}

fn flush(original: &layout::Para, lines: &mut Vec<layout::Line>, contents: bool, out: &mut Vec<Item>) {
    if lines.is_empty() {
        return;
    }
    let mut p = original.clone();
    p.rect = lines.iter().map(|l| l.rect).reduce(layout::union).unwrap();
    p.align = if contents { layout::Align::Left } else { layout::alignment(lines, p.column, p.size) };
    // Section numbers in contents are literal title text, not list markers.
    if contents || lines[0].baseline != original.lines[0].baseline {
        p.marker = None;
    }
    p.lines = std::mem::take(lines);
    out.push(Item::Para(p));
}

/// A separated row has lost the paragraph's line pitch. Recover it from
/// nearby rows of its own column instead of imposing a 1.2-em line box
/// on tightly set contents or doubling it across a row without leaders.
pub(super) fn pitch(p: &layout::Para, items: &[Item]) -> f32 {
    items.iter().filter_map(|item| match item { Item::Para(other) => Some(other), _ => None }).filter(|other| {
        (other.column[0] - p.column[0]).abs() <= p.size && (other.column[1] - p.column[1]).abs() <= p.size
    }).flat_map(|other| &other.lines).filter_map(|line| {
        if !(0.85..=1.18).contains(&(line.size / p.size))
        {
            return None;
        }
        let delta = (line.baseline - p.lines[0].baseline).abs();
        (delta > 0.5 * p.size && delta < 2.5 * p.size).then_some(delta)
    }).min_by(f32::total_cmp).unwrap_or_else(|| p.pitch())
}

/// Preserve the title and page label's runs, inserting one native tab.
pub(super) fn spans(p: &layout::Para) -> Option<Vec<Span>> {
    if p.lines.len() != 1 {
        return None;
    }
    let line = &p.lines[0];
    let e = entry(line)?;
    let mut p = p.clone();
    p.marker = None;
    let mut glyphs = line.glyphs[..e.label_end].to_vec();
    // A coloured heading can have black leaders. The tab's own run
    // controls the leader's colour, font and weight in Word.
    let mut tab = line.glyphs[e.label_end..e.page_start].iter().find(|g| !g.ch.is_whitespace())?.clone();
    tab.ch = '\t';
    tab.space_before = false;
    glyphs.push(tab);
    let start = glyphs.len();
    glyphs.extend_from_slice(&line.glyphs[e.page_start..e.page_end]);
    glyphs[start].space_before = false;
    p.lines[0].glyphs = glyphs;
    Some(layout::spans(&p, "\t"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::Glyph;

    fn line(text: &str, x: f32, baseline: f32) -> layout::Line {
        layout::lines(text.chars().enumerate().map(|(i, ch)| {
            let x = x + i as f32 * 4.0;
            Glyph { source: -1, ch, rect: [x, baseline - 9.0, x + 4.0, baseline + 2.0], x, baseline, size: 9.0, font: if ch.is_ascii_digit() { 1 } else { 0 }, color: [0; 3], space_before: false, invisible: false }
        }).collect()).remove(0)
    }

    #[test]
    fn leaders_split_merged_contents_rows_and_keep_section_numbers() {
        let lines = vec![line("1.1. Ideas................15", 90.0, 100.0), line("Flowcharts................15", 110.0, 114.4), line("Pseudocode................17", 110.0, 128.8)];
        let rect = lines.iter().map(|l| l.rect).reduce(layout::union).unwrap();
        let p = layout::Para { lines, column: [50.0, 400.0], align: layout::Align::Justify, marker: Some(5), size: 9.0, rect };
        let rows = separate(vec![Item::Para(p)]);
        assert_eq!(rows.len(), 3);
        for (i, row) in rows.iter().enumerate() {
            let Item::Para(p) = row else { panic!() };
            assert_eq!(p.lines.len(), 1);
            assert_eq!(p.marker, None);
            assert_eq!(p.align, layout::Align::Left);
            let runs = spans(p).unwrap();
            assert_eq!(runs.last().unwrap().font, 1);
            let text: String = runs.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(text, ["1.1. Ideas\t15", "Flowcharts\t15", "Pseudocode\t17"][i]);
        }
        assert_eq!(rows[0].rect()[0], 90.0);
        assert_eq!(rows[1].rect()[0], 110.0);
    }

    #[test]
    fn roman_pages_ranges_and_unicode_leaders_are_recognised() {
        for text in ["Preface...... xii", "Chapter…… 12–15", "Chapter . . . . . . 10", "Index······ 217"] {
            assert!(entry(&line(text, 50.0, 100.0)).is_some(), "{text}");
        }
        for text in ["He paused... 12", "Version 1.2.3", "3. A normal list item", "Filled........text", "......12", "Title......1i", "Title......12-"] {
            assert!(entry(&line(text, 50.0, 100.0)).is_none(), "{text}");
        }
    }

    #[test]
    fn a_coloured_title_does_not_recolour_its_leader() {
        let mut line = line("Preface...........11", 50.0, 100.0);
        for glyph in line.glyphs.iter_mut().take(7) {
            glyph.color = [0, 176, 80];
        }
        let rect = line.rect;
        let p = layout::Para { lines: vec![line], column: [50.0, 400.0], align: layout::Align::Left, marker: None, size: 9.0, rect };
        let runs = spans(&p).unwrap();
        assert_eq!(runs[0].color, [0, 176, 80]);
        assert_eq!(runs.iter().find(|s| s.text.contains('\t')).unwrap().color, [0; 3]);
    }

    #[test]
    fn ordinary_paragraphs_are_unchanged() {
        let lines = vec![line("The ordinary paragraph has a normal first line,", 50.0, 100.0), line("and a second line which ends here.", 50.0, 114.4)];
        let rect = lines.iter().map(|l| l.rect).reduce(layout::union).unwrap();
        let p = layout::Para { lines, column: [50.0, 400.0], align: layout::Align::Justify, marker: None, size: 9.0, rect };
        let original = vec![Item::Para(p)];
        assert_eq!(separate(original.clone()), original);
    }

    #[test]
    fn dense_contents_keep_their_pitch_without_using_the_other_column() {
        let lines = vec![line("First title..........11", 50.0, 100.0), line("A row without printed leaders", 50.0, 109.4), line("Third title..........13", 50.0, 118.8)];
        let rect = lines.iter().map(|l| l.rect).reduce(layout::union).unwrap();
        let p = layout::Para { lines, column: [50.0, 200.0], align: layout::Align::Left, marker: None, size: 9.0, rect };
        let mut rows = separate(vec![Item::Para(p.clone())]);
        let other_line = line("Other column.........21", 250.0, 105.0);
        let other = layout::Para { rect: other_line.rect, lines: vec![other_line], column: [250.0, 400.0], ..p };
        rows.push(Item::Para(other));
        for row in &rows[..3] {
            let Item::Para(p) = row else { panic!() };
            assert!((pitch(p, &rows) - 9.4).abs() < 0.01);
            assert!((p.pitch() - 10.8).abs() < 0.01);
        }
    }

    #[test]
    fn invisible_scan_text_keeps_its_existing_paragraphs() {
        let mut line = line("Recognised contents........12", 50.0, 100.0);
        for glyph in &mut line.glyphs {
            glyph.invisible = true;
        }
        let rect = line.rect;
        let p = layout::Para { lines: vec![line], column: [50.0, 400.0], align: layout::Align::Left, marker: None, size: 9.0, rect };
        assert!(spans(&p).is_none());
        let original = vec![Item::Para(p)];
        assert_eq!(separate(original.clone()), original);
    }
}
