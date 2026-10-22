//! Find in the document (Ctrl+F): the query and its options, the hits as
//! the engine sends them page by page (`engine::SearchHit`, with their
//! rectangles in page space), the current hit and the steps between hits.
//! Pure state, tested; the find bar is drawn by the app.

use std::collections::BTreeMap;

use crate::engine::SearchHit;

/// The end of `text` after its last line break, its whitespace collapsed:
/// what a hit's line says before the match (a space before the match
/// kept, the line's start trimmed).
pub fn line_before(text: &str) -> String {
    let tail = text.rsplit(['\r', '\n']).next().unwrap_or("");
    collapse(tail, false, true)
}

/// The start of `text` up to its first line break, its whitespace
/// collapsed: what a hit's line says after the match (a space after the
/// match kept, the line's end trimmed).
pub fn line_after(text: &str) -> String {
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
