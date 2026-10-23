//! Find in the document (Ctrl+F): the query and its options, the hits as
//! the engine sends them page by page (`engine::SearchHit`, with their
//! rectangles in page space), the current hit and the steps between hits.
//! Pure state, tested; the find bar is drawn by the app.

use std::collections::BTreeMap;

use crate::engine::SearchHit;

/// The matches of `query` in a page's characters (one per text page
/// index): `(first index, count)`, in order, not overlapping. The case
/// is folded by Unicode's rules unless `match_case` (PDFium's own search
/// folds ASCII alone, so "быстрый" never found "Быстрый"); a word of
/// the query is matched across any run of whitespace, line breaks
/// included; `whole_words` wants no letter or digit against the match's
/// letters or digits, in any script.
pub fn matches(chars: &[char], query: &str, match_case: bool, whole_words: bool) -> Vec<(usize, usize)> {
    let fold = |c: char| -> Vec<char> { if match_case { vec![c] } else { c.to_lowercase().collect() } };
    let words: Vec<Vec<char>> = query.split_whitespace().map(|w| w.chars().flat_map(fold).collect()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    // The folded text, each character remembering the index it came from
    // (a character may fold to several).
    let mut folded = Vec::with_capacity(chars.len());
    let mut origin = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        for f in fold(c) {
            folded.push(f);
            origin.push(i);
        }
    }
    // The words one after another, whitespace between them; the folded
    // index after the match.
    let match_at = |mut p: usize| -> Option<usize> {
        for (k, word) in words.iter().enumerate() {
            if k > 0 {
                let s = p;
                while p < folded.len() && folded[p].is_whitespace() {
                    p += 1;
                }
                if p == s {
                    return None;
                }
            }
            if folded.len() < p + word.len() || folded[p..p + word.len()] != word[..] {
                return None;
            }
            p += word.len();
        }
        Some(p)
    };
    let starts_word = words[0][0].is_alphanumeric();
    let ends_word = words[words.len() - 1].last().is_some_and(|c| c.is_alphanumeric());
    let mut out = Vec::new();
    let mut i = 0;
    while i < folded.len() {
        if let Some(end) = match_at(i) {
            let (start, last) = (origin[i], origin[end - 1]);
            let before = !whole_words || !starts_word || start == 0 || !chars[start - 1].is_alphanumeric();
            let after = !whole_words || !ends_word || last + 1 >= chars.len() || !chars[last + 1].is_alphanumeric();
            if before && after {
                out.push((start, last - start + 1));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The end of `text` after its last line break, its whitespace collapsed:
/// what a hit's line says before the match (a space before the match
/// kept, the line's start trimmed).
pub fn line_before(text: &str) -> String {
    let text = crate::selection::unhyphenate(text);
    let tail = text.rsplit(['\r', '\n']).next().unwrap_or("");
    collapse(tail, false, true)
}

/// The start of `text` up to its first line break, its whitespace
/// collapsed: what a hit's line says after the match (a space after the
/// match kept, the line's end trimmed).
pub fn line_after(text: &str) -> String {
    let text = crate::selection::unhyphenate(text);
    let head = text.split(['\r', '\n']).next().unwrap_or("");
    collapse(head, true, false)
}

/// Runs of whitespace as one space, dropped at the ends unless kept.
fn collapse(s: &str, keep_leading: bool, keep_trailing: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if c.is_whitespace() {
            pending = true;
            continue;
        }
        if pending && (!out.is_empty() || keep_leading) {
            out.push(' ');
        }
        pending = false;
        out.push(c);
    }
    if pending && keep_trailing && (!out.is_empty() || keep_leading) {
        out.push(' ');
    }
    out
}

#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub request_focus: bool,
    /// The number of the search the hits belong to; answers of an earlier
    /// search are dropped.
    pub search: u32,
    /// The hits by page.
    pub hits: BTreeMap<usize, Vec<SearchHit>>,
    /// Every page has been searched.
    pub done: bool,
    /// The hit the reader is at: page and index in that page's hits.
    pub current: Option<(usize, usize)>,
    /// The view is to go to the current hit.
    pub jump: bool,
    /// What the hits were asked for: the query and the options; the search
    /// is started again when they change.
    asked: Option<(String, bool, bool)>,
}

impl FindState {
    pub fn open(&mut self) {
        self.open = true;
        self.request_focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.clear();
    }

    fn clear(&mut self) {
        self.hits.clear();
        self.done = false;
        self.current = None;
        self.asked = None;
    }

    /// A page's text changed (an edit): the hits name characters that
    /// are not there any more, so the search is made again.
    pub fn text_changed(&mut self) {
        if self.open {
            self.asked = None;
        }
    }

    /// Whether a search for `query` with these options must be started
    /// (the next search number is taken). `false` when the hits at hand
    /// are for them already.
    pub fn needs_search(&mut self, match_case: bool, whole_words: bool) -> Option<u32> {
        let key = (self.query.clone(), match_case, whole_words);
        if self.asked.as_ref() == Some(&key) {
            return None;
        }
        self.hits.clear();
        self.done = self.query.is_empty();
        self.current = None;
        self.asked = Some(key);
        if self.query.is_empty() {
            return None;
        }
        self.search += 1;
        Some(self.search)
    }

    /// Take a page's hits of search `search`; the first hits make the
    /// current one when there is none yet.
    pub fn take(&mut self, search: u32, page: usize, hits: Vec<SearchHit>, done: bool) {
        if search != self.search || !self.open {
            return;
        }
        if !hits.is_empty() {
            self.hits.insert(page, hits);
            if self.current.is_none() {
                self.current = Some((page, 0));
                self.jump = true;
            }
        }
        if done {
            self.done = true;
        }
    }

    pub fn total(&self) -> usize {
        self.hits.values().map(Vec::len).sum()
    }

    /// The current hit's ordinal, 1-based, among all the hits so far.
    pub fn ordinal(&self) -> Option<usize> {
        let (page, i) = self.current?;
        let before: usize = self.hits.range(..page).map(|(_, h)| h.len()).sum();
        Some(before + i + 1)
    }

    pub fn current_hit(&self) -> Option<(usize, &SearchHit)> {
        let (page, i) = self.current?;
        self.hits.get(&page)?.get(i).map(|h| (page, h))
    }

    /// The next or previous hit, round the document.
    pub fn step(&mut self, forward: bool) {
        let Some((page, i)) = self.current else {
            if let Some((&page, _)) = self.hits.iter().next() {
                self.current = Some((page, 0));
                self.jump = true;
            }
            return;
        };
        let next = if forward {
            match self.hits.get(&page) {
                Some(h) if i + 1 < h.len() => Some((page, i + 1)),
                _ => self
                    .hits
                    .range(page + 1..)
                    .next()
                    .or_else(|| self.hits.iter().next())
                    .map(|(&p, _)| (p, 0)),
            }
        } else if i > 0 {
            Some((page, i - 1))
        } else {
            self.hits
                .range(..page)
                .next_back()
                .or_else(|| self.hits.iter().next_back())
                .map(|(&p, h)| (p, h.len() - 1))
        };
        if let Some(next) = next {
            self.current = Some(next);
            self.jump = true;
        }
    }

    /// The hits on `page`, with the index of the current one if it is
    /// there.
    pub fn hits_on(&self, page: usize) -> (&[SearchHit], Option<usize>) {
        if !self.open {
            return (&[], None);
        }
        let hits = self.hits.get(&page).map_or(&[][..], Vec::as_slice);
        let current = match self.current {
            Some((p, i)) if p == page => Some(i),
            _ => None,
        };
        (hits, current)
    }

    /// What the find bar says of the hits.
    pub fn status(&self) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        let total = self.total();
        if total == 0 {
            return if self.done { tr!("No results", "Не найдено").to_owned() } else { tr!("Searching…", "Поиск…").to_owned() };
        }
        let more = if self.done { "" } else { "+" };
        match self.ordinal() {
            Some(n) => tr!(format!("{n} of {total}{more}"), format!("{n} из {total}{more}")),
            None => tr!(format!("{total}{more}"), format!("{total}{more}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_folds_case_in_any_script_and_knows_words() {
        let chars: Vec<char> = "Быстрый просмотрщик PDF. Кот, который\r\nбыстрый. İstanbul".chars().collect();
        assert_eq!(matches(&chars, "быстрый", false, false), vec![(0, 7), (39, 7)]);
        assert_eq!(matches(&chars, "быстрый", true, false), vec![(39, 7)]);
        assert_eq!(matches(&chars, "pdf", false, false), vec![(20, 3)]);
        // Whole words in Cyrillic: "кот" is not "который".
        assert_eq!(matches(&chars, "кот", false, true), vec![(25, 3)]);
        assert_eq!(matches(&chars, "кот", false, false), vec![(25, 3), (30, 3)]);
        // A query's space matches the line break; the indices are the
        // page's, whatever a character folds to (İ folds to two).
        assert_eq!(matches(&chars, "который быстрый", false, false), vec![(30, 16)]);
        assert_eq!(matches(&chars, "istanbul", false, false).len(), 0);
        assert_eq!(matches(&chars, "İstanbul", false, false), vec![(48, 8)]);
        assert!(matches(&chars, "  ", false, false).is_empty());
        // Matches do not overlap.
        let aa: Vec<char> = "aaaa".chars().collect();
        assert_eq!(matches(&aa, "aa", true, false), vec![(0, 2), (2, 2)]);
    }

    #[test]
    fn a_line_end_hyphen_reads_as_a_hyphen() {
        assert_eq!(line_before("x\r\nэлемен\u{2}"), "элемен-");
        // The word's rest on the next line: joined, the line goes on.
        assert_eq!(line_after(" элемен\u{2}\r\nтарные числа\r\nдальше"), " элементарные числа");
        assert_eq!(line_before("с одного переклю\u{fffe}чение "), "с одного переключение ");
    }

    fn hit(start: usize) -> SearchHit {
        SearchHit { start, count: 3, rects: vec![[0.0, 0.0, 10.0, 10.0]], before: String::new(), matched: String::new(), after: String::new() }
    }

    #[test]
    fn a_hits_line_is_cut_at_line_breaks_and_its_spaces_collapsed() {
        assert_eq!(line_before("first line\r\n  the  quick "), "the quick ");
        assert_eq!(line_before("no break here"), "no break here");
        assert_eq!(line_before("\n"), "");
        assert_eq!(line_after(" brown\tfox\r\nnext line"), " brown fox");
        assert_eq!(line_after("\nnext"), "");
        assert_eq!(line_after("tail   "), "tail");
    }

    #[test]
    fn hits_arrive_and_steps_go_round() {
        let mut find = FindState::default();
        find.open();
        find.query = "abc".into();
        let search = find.needs_search(false, false).unwrap();
        assert_eq!(find.needs_search(false, false), None, "asked already");
        // A late answer of an earlier search is dropped.
        find.take(search - 1, 0, vec![hit(0)], false);
        assert_eq!(find.total(), 0);
        find.take(search, 5, vec![hit(1), hit(2)], false);
        assert_eq!(find.current, Some((5, 0)));
        assert!(find.jump);
        find.take(search, 2, vec![hit(0)], true);
        assert!(find.done);
        assert_eq!(find.total(), 3);
        assert_eq!(find.ordinal(), Some(2));
        find.step(true);
        assert_eq!(find.current, Some((5, 1)));
        find.step(true);
        assert_eq!(find.current, Some((2, 0)), "round to the first page");
        find.step(false);
        assert_eq!(find.current, Some((5, 1)), "back round to the last");
        assert_eq!(find.hits_on(5).1, Some(1));
        assert_eq!(find.hits_on(2).0.len(), 1);
        assert_eq!(find.status(), "3 of 3");
        // Changed options start a new search.
        assert_eq!(find.needs_search(true, false), Some(search + 1));
        assert_eq!(find.total(), 0);
        find.close();
        assert!(!find.open);
    }

    #[test]
    fn an_empty_query_searches_nothing() {
        let mut find = FindState::default();
        find.open();
        assert_eq!(find.needs_search(false, false), None);
        assert!(find.done);
        assert_eq!(find.status(), "");
    }
}
