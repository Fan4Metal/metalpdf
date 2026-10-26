//! The lines of a page's text objects. Generators often write a line as
//! many objects, cut wherever a pair of letters is kerned ("Answ", "ers t",
//! "o"), so the editing works on lines: the objects on one baseline, one
//! after another, read as one text. An edit of the line merges only the
//! objects the change touches into one; the others stay as they are, those
//! after it moved along the line by the change in width.

use crate::pdfium::TextObject;

/// One object of a line: its index among the page's objects and the
/// characters of the line's text it gives.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    pub object: usize,
    pub start: usize,
    pub end: usize,
}

/// A line of text objects, in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub fragments: Vec<Fragment>,
    /// The objects' texts joined, a space put between two objects apart by
    /// a word's gap with no space of their own, as the editor shows it
    /// (`shown`).
    pub text: String,
    /// The same as it is written back (`written`), a character for each
    /// of `text`'s.
    pub raw: String,
    /// left, bottom, right, top in page space: the objects' boxes joined.
    pub bounds: [f32; 4],
    /// The font size as drawn of the line's first object.
    pub font_size: f32,
}

/// What an edit of a line does: `objects` (in order) become one object,
/// the first, with `text`; `followers`, the objects after them on the
/// line, move along it by the change in width.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEdit {
    pub objects: Vec<usize>,
    pub text: String,
    pub followers: Vec<usize>,
}

/// A gap between two objects wider than this share of the font size is a
/// word's gap: kerned pairs come apart by less.
const WORD_GAP: f32 = 0.18;
/// Objects further apart than this share of the font size are on separate
/// lines (table columns), even on one baseline.
const LINE_GAP: f32 = 1.5;
/// An object that starts further back than this share of the font size
/// before the last one's ink ends is drawn over it (a fake bold, a
/// shadow), not after it: kerned pairs overlap by a fraction of a point.
const OVERLAP: f32 = 0.3;

/// Where an object lies: along its direction (the ink's start and end)
/// and across it (the baseline).
struct Placed<'a> {
    object: &'a TextObject,
    direction: [f32; 2],
    baseline: f32,
    start: f32,
    end: f32,
}

fn place(object: &TextObject) -> Placed<'_> {
    let [dx, dy] = object.direction;
    let along = |x: f32, y: f32| x * dx + y * dy;
    let [l, b, r, t] = object.bounds;
    let corners = [along(l, b), along(l, t), along(r, b), along(r, t)];
    Placed {
        object,
        direction: object.direction,
        baseline: object.origin[1] * dx - object.origin[0] * dy,
        start: corners.iter().copied().fold(f32::INFINITY, f32::min),
        end: corners.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    }
}

/// Whether `next` continues the line `prev` is on.
fn continues(prev: &Placed, next: &Placed) -> bool {
    let size = prev.object.font_size.max(next.object.font_size).max(0.1);
    let same_direction = (prev.direction[0] - next.direction[0]).abs() < 0.01 && (prev.direction[1] - next.direction[1]).abs() < 0.01;
    let ratio = prev.object.font_size.max(0.1) / next.object.font_size.max(0.1);
    let gap = next.start - prev.end;
    // Kerned pairs overlap by a fraction of a point; an object that starts
    // well inside the one before is drawn over it (a fake bold, a shadow:
    // the same text again), not after it.
    same_direction && (prev.baseline - next.baseline).abs() < 0.2 * size && (0.8..=1.25).contains(&ratio) && gap < LINE_GAP * size && gap > -OVERLAP * size
}

/// The page's lines, every text object in one.
pub fn lines(objects: &[TextObject]) -> Vec<Line> {
    let mut placed: Vec<Placed> = objects.iter().map(place).collect();
    // By direction, then baseline, then along it: a line's objects come
    // one after another.
    placed.sort_by(|a, b| {
        let key = |p: &Placed| ((p.direction[0] * 100.0).round() as i32, (p.direction[1] * 100.0).round() as i32);
        key(a).cmp(&key(b)).then(b.baseline.total_cmp(&a.baseline)).then(a.start.total_cmp(&b.start))
    });
    // Baselines a little apart sort apart: a line is gathered from every
    // object still free that continues it, not from neighbours alone. The
    // objects before `first` are all taken, and those after it in the
    // sort with a baseline further down than the line can reach (or of
    // another direction) end the search: a page of 20 000 runs (a chart)
    // looked through every object for every one.
    let mut used = vec![false; placed.len()];
    let mut out = Vec::new();
    let direction_of = |p: &Placed| ((p.direction[0] * 100.0).round() as i32, (p.direction[1] * 100.0).round() as i32);
    for first in 0..placed.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut members = vec![first];
        let direction = direction_of(&placed[first]);
        let mut lowest = placed[first].baseline;
        let mut largest = placed[first].object.font_size.max(0.1);
        loop {
            let last = &placed[*members.last().expect("one at least")];
            // `continues` takes a baseline within 0.2 of the larger size,
            // which a size 1.25 times larger widens: half the size bounds it.
            let floor = lowest - 0.5 * largest * 1.25;
            let next = (first + 1..placed.len())
                .take_while(|&j| direction_of(&placed[j]) == direction && placed[j].baseline >= floor)
                .filter(|&j| !used[j] && placed[j].start >= last.start && continues(last, &placed[j]))
                .min_by(|&a, &b| placed[a].start.total_cmp(&placed[b].start));
            let Some(next) = next else { break };
            used[next] = true;
            members.push(next);
            lowest = lowest.min(placed[next].baseline);
            largest = largest.max(placed[next].object.font_size);
        }
        out.push(join(&members.iter().map(|&m| &placed[m]).collect::<Vec<_>>()));
    }
    out
}

/// The line of objects `members`, in order.
/// A character of a run as it is written back: PDFium reads a hyphen at
/// a line's end as U+0002, which is the font's own hyphen; a stand-in
/// font's space reads as a no-break space (`set_text` tries both when it
/// writes).
fn written(c: char) -> char {
    match c {
        '\u{2}' => '-',
        '\u{a0}' => ' ',
        c => c,
    }
}

/// A character as the inline editor shows it: the hyphens and the
/// spaces egui's font has no glyph for (a box was drawn) as `-` and ` `.
/// One for one, so an edit's place is the same in `raw`.
fn shown(c: char) -> char {
    match written(c) {
        '\u{ad}' | '\u{2010}' | '\u{2011}' => '-',
        '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
        c => c,
    }
}

fn join(members: &[&Placed]) -> Line {
    let mut raw = String::new();
    let mut fragments = Vec::new();
    let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    let mut count = 0;
    for (k, p) in members.iter().enumerate() {
        if k > 0 {
            let prev = members[k - 1];
            let gap = p.start - prev.end;
            let spaced = raw.ends_with(char::is_whitespace) || p.object.text.starts_with(char::is_whitespace);
            if !spaced && gap > WORD_GAP * p.object.font_size.max(0.1) {
                raw.push(' ');
                count += 1;
            }
        }
        let n = p.object.text.chars().count();
        fragments.push(Fragment { object: p.object.index, start: count, end: count + n });
        raw.extend(p.object.text.chars().map(written));
        count += n;
        let b = p.object.bounds;
        bounds = [bounds[0].min(b[0]), bounds[1].min(b[1]), bounds[2].max(b[2]), bounds[3].max(b[3])];
    }
    let text = raw.chars().map(shown).collect();
    Line { fragments, text, raw, bounds, font_size: members[0].object.font_size }
}

impl Line {
    pub fn contains(&self, object: usize) -> bool {
        self.fragments.iter().any(|f| f.object == object)
    }

    /// What `new`, the line's text edited, changes: the objects from the
    /// first the change touches to the last, merged, and those after
    /// them; `None` when nothing changed or nothing would be left.
    pub fn edit(&self, new: &str) -> Option<LineEdit> {
        // Compared as shown, the characters kept written back as they were.
        let old: Vec<char> = self.text.chars().collect();
        let raw: Vec<char> = self.raw.chars().collect();
        let new: Vec<char> = new.chars().collect();
        if old == new || self.fragments.is_empty() {
            return None;
        }
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let room = old.len().min(new.len()) - prefix;
        let suffix = old.iter().rev().zip(new.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
        let (p, end) = (prefix, old.len() - suffix);
        let at = |i: usize| self.fragments.iter().position(|f| f.start <= i && i < f.end);
        // A character between two objects (a space put in) belongs to
        // neither: a change there takes in the one before, and the one
        // after when the change goes past it.
        let before = |i: usize| self.fragments.iter().rposition(|f| f.start <= i).unwrap_or(0);
        let after = |i: usize| self.fragments.iter().position(|f| f.end > i).unwrap_or(self.fragments.len() - 1);
        let (mut lo, mut hi) = if end == p {
            // Typed in: at the end of the object before, else the start
            // of the one after.
            let k = p.checked_sub(1).and_then(at).or_else(|| at(p)).unwrap_or_else(|| before(p));
            (k, k)
        } else {
            (at(p).unwrap_or_else(|| before(p)), at(end - 1).unwrap_or_else(|| after(end - 1)))
        };
        let merged = |lo: usize, hi: usize| -> String {
            let (a, b) = (self.fragments[lo].start, self.fragments[hi].end);
            let (a, b) = (a.min(p), b.max(end));
            raw[a..p].iter().chain(&new[p..new.len() - suffix]).chain(&raw[end..b]).collect()
        };
        let mut text = merged(lo, hi);
        // The objects touched were all deleted: a neighbour takes the
        // place, so that what is left still has an object to be.
        while text.trim().is_empty() {
            if lo > 0 {
                lo -= 1;
            } else if hi + 1 < self.fragments.len() {
                hi += 1;
            } else {
                return None;
            }
            text = merged(lo, hi);
        }
        Some(LineEdit {
            objects: self.fragments[lo..=hi].iter().map(|f| f.object).collect(),
            text,
            followers: self.fragments[hi + 1..].iter().map(|f| f.object).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run at `x` on baseline `y`, 9 pt, each character 4.5 points wide.
    fn run(index: usize, x: f32, y: f32, text: &str) -> TextObject {
        let width = text.chars().count() as f32 * 4.5;
        TextObject {
            index,
            bounds: [x + 0.3, y - 0.1, x + width - 0.3, y + 6.5],
            text: text.into(),
            font: "IBMPlexSans-Regular".into(),
            font_size: 9.0,
            subset: true,
            flags: 0,
            stand_in: None,
            origin: [x, y],
            direction: [1.0, 0.0],
        }
    }

    /// Cut as a kerning generator cuts: "Answ|ers t|o e|v|en".
    fn kerned() -> Vec<TextObject> {
        vec![run(4, 10.0, 100.0, "Answ"), run(5, 28.2, 100.0, "ers t"), run(6, 50.8, 100.0, "o e"), run(7, 64.4, 100.0, "v"), run(8, 68.9, 100.0, "en")]
    }

    #[test]
    fn kerned_runs_make_one_line() {
        let lines = lines(&kerned());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Answers to even");
        assert_eq!(lines[0].fragments.iter().map(|f| f.object).collect::<Vec<_>>(), [4, 5, 6, 7, 8]);
        assert_eq!(lines[0].fragments[1], Fragment { object: 5, start: 4, end: 9 });
    }

    #[test]
    fn a_run_drawn_over_another_is_not_its_continuation() {
        // A fake bold: the same text again, a fifth of a point along.
        let mut objects = vec![run(0, 10.0, 100.0, "Heading"), run(1, 10.2, 100.0, "Heading")];
        let texts = |objects: &[TextObject]| lines(objects).iter().map(|l| l.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&objects), ["Heading", "Heading"]);
        // A kerned pair, overlapping by a fraction of a point, is one line.
        objects[1] = run(1, 10.0 + 7.0 * 4.5 - 0.4, 100.0, "s");
        assert_eq!(texts(&objects), ["Headings"]);
    }

    #[test]
    fn baselines_and_columns_part_lines() {
        let mut objects = kerned();
        objects.push(run(9, 10.0, 88.0, "Review"));
        // On the first baseline, but a column away.
        objects.push(run(10, 300.0, 100.0, "Table"));
        // Out of order in the content: found by place.
        objects.insert(0, run(11, 40.0, 88.0, "Questions"));
        let lines = lines(&objects);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["Answers to even", "Table", "Review Questions"]);
    }

    #[test]
    fn a_page_of_many_runs_is_read_in_lines_quickly() {
        // 200 lines of 100 runs, a chart's labels or a page set glyph by
        // glyph: each run looked through every other one before.
        let mut objects = Vec::new();
        for line in 0..200 {
            for k in 0..100 {
                objects.push(run(objects.len(), 10.0 + k as f32 * 9.0, 800.0 - line as f32 * 12.0, "ab"));
            }
        }
        let started = std::time::Instant::now();
        let lines = lines(&objects);
        assert_eq!(lines.len(), 200);
        assert!(lines.iter().all(|l| l.fragments.len() == 100));
        assert!(started.elapsed().as_secs_f32() < 0.5, "{:?}", started.elapsed());
    }

    #[test]
    fn a_word_gap_reads_as_a_space() {
        let lines = lines(&[run(1, 10.0, 100.0, "Students"), run(2, 49.0, 100.0, "are")]);
        assert_eq!(lines[0].text, "Students are");
        assert_eq!(lines[0].fragments[1], Fragment { object: 2, start: 9, end: 12 });
    }

    #[test]
    fn a_change_merges_only_the_runs_it_touches() {
        let line = &lines(&kerned())[0];
        // Within one run.
        assert_eq!(line.edit("Answers to Even"), Some(LineEdit { objects: vec![6], text: "o E".into(), followers: vec![7, 8] }));
        // Across a cut: "Answ|ers" both.
        assert_eq!(line.edit("Replies to even"), Some(LineEdit { objects: vec![4, 5], text: "Replies t".into(), followers: vec![6, 7, 8] }));
        // The last runs.
        assert_eq!(line.edit("Answers to odd"), Some(LineEdit { objects: vec![6, 7, 8], text: "o odd".into(), followers: vec![] }));
        assert_eq!(line.edit("Answers to even"), None);
    }

    #[test]
    fn typing_extends_the_run_before() {
        let line = &lines(&kerned())[0];
        // At the end of "Answ", before "ers t".
        assert_eq!(line.edit("AnswXers to even"), Some(LineEdit { objects: vec![4], text: "AnswX".into(), followers: vec![5, 6, 7, 8] }));
        assert_eq!(line.edit("XAnswers to even"), Some(LineEdit { objects: vec![4], text: "XAnsw".into(), followers: vec![5, 6, 7, 8] }));
        assert_eq!(line.edit("Answers to even!"), Some(LineEdit { objects: vec![8], text: "en!".into(), followers: vec![] }));
    }

    #[test]
    fn a_space_between_runs_joins_them_when_changed() {
        let line = &lines(&[run(1, 10.0, 100.0, "Students"), run(2, 49.0, 100.0, "are"), run(3, 64.0, 100.0, "here")])[0];
        assert_eq!(line.text, "Students are here");
        // Typed after the space: the run after.
        assert_eq!(line.edit("Students not are here"), Some(LineEdit { objects: vec![2], text: "not are".into(), followers: vec![3] }));
        // The space itself removed: both sides.
        assert_eq!(line.edit("Studentsare here"), Some(LineEdit { objects: vec![1, 2], text: "Studentsare".into(), followers: vec![3] }));
    }

    #[test]
    fn a_hyphen_at_a_lines_end_is_shown_and_written_as_one() {
        // PDFium's mark of a line-end hyphen, and an em space.
        let line = &lines(&[run(1, 10.0, 100.0, "элемен\u{2}"), run(2, 50.0, 100.0, "a\u{2003}b")])[0];
        assert_eq!(line.text, "элемен- a b");
        assert_eq!(line.raw, "элемен- a\u{2003}b");
        // The hyphen kept, written as the font's hyphen; the em space kept.
        assert_eq!(line.edit("элемент- a b"), Some(LineEdit { objects: vec![1], text: "элемент-".into(), followers: vec![2] }));
        assert_eq!(line.edit("элемен- a c"), Some(LineEdit { objects: vec![2], text: "a\u{2003}c".into(), followers: vec![] }));
    }

    #[test]
    fn a_run_deleted_whole_is_merged_into_a_neighbour() {
        let line = &lines(&kerned())[0];
        // "v" gone: "o e" takes its place.
        assert_eq!(line.edit("Answers to een"), Some(LineEdit { objects: vec![6, 7], text: "o e".into(), followers: vec![8] }));
        assert_eq!(line.edit(""), None);
        assert_eq!(line.edit("   "), None);
    }
}
