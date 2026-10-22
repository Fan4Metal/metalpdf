//! What the UI knows of an open document: the engine's answers, kept as
//! plain data (page count and sizes, labels, bookmarks, the text and links
//! of the pages asked for), and which answers are still awaited, so that
//! nothing is asked twice.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::engine::{DocId, DocInfo, Engine, Event, Request, SIZE_BATCH};
use crate::pdfium::{Link, OpenError, OutlineItem, TextLayout};

/// A page's text with the transform that puts it on the page as shown.
#[derive(Debug)]
pub struct PageText {
    pub layout: Arc<TextLayout>,
    /// `pdfium::Page::page_to_view`: page space to the unrotated page's
    /// points from its top left corner.
    pub to_view: [f32; 6],
}

impl PageText {
    /// A point of page space on the page as shown, in points from the top
    /// left corner of the unrotated page.
    pub fn to_page_points(&self, x: f32, y: f32) -> [f32; 2] {
        let m = self.to_view;
        [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
    }

    /// A page-space rectangle (left, bottom, right, top) as a rectangle on
    /// the unrotated page, `[x, y, w, h]` in points from its top left.
    pub fn rect_to_page(&self, r: [f32; 4]) -> [f32; 4] {
        let a = self.to_page_points(r[0], r[1]);
        let b = self.to_page_points(r[2], r[3]);
        let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
        let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
        [x0, y0, x1 - x0, y1 - y0]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Asked of the engine, no answer yet.
    Loading,
    Ready,
    /// Encrypted: the password field is shown; `wrong` after a refused one.
    NeedsPassword { wrong: bool },
    Failed(OpenError),
}

pub struct DocState {
    pub id: DocId,
    pub status: Status,
    pub page_count: usize,
    /// Page sizes in PDF points, `None` until the engine sent them.
    pub sizes: Vec<Option<[f32; 2]>>,
    pub labels: Vec<Option<String>>,
    /// The first page's size: what unknown pages are laid out as.
    pub fallback: [f32; 2],
    pub title: Option<String>,
    /// The next size batch to ask for; past `page_count` when all are there.
    sizes_next: usize,
    sizes_waiting: bool,
    pub outline: Option<Vec<OutlineItem>>,
    outline_asked: bool,
    pub text: HashMap<usize, Arc<PageText>>,
    text_asked: HashSet<usize>,
    pub links: HashMap<usize, Arc<Vec<Link>>>,
    links_asked: HashSet<usize>,
}

/// A page size to lay a document out with before its first page is known
/// (A4, 595 × 842 pt).
pub const DEFAULT_PAGE: [f32; 2] = [595.0, 842.0];

impl DocState {
    pub fn new(id: DocId) -> DocState {
        DocState {
            id,
            status: Status::Loading,
            page_count: 0,
            sizes: Vec::new(),
            labels: Vec::new(),
            fallback: DEFAULT_PAGE,
            title: None,
            sizes_next: 0,
            sizes_waiting: false,
            outline: None,
            outline_asked: false,
            text: HashMap::new(),
            text_asked: HashSet::new(),
            links: HashMap::new(),
            links_asked: HashSet::new(),
        }
    }

    pub fn ready(&self) -> bool {
        self.status == Status::Ready
    }

    /// Take an answer meant for this document; `true` if it was.
    pub fn take(&mut self, event: &Event, engine: &Engine) -> bool {
        match event {
            Event::Opened { id, result } if *id == self.id => {
                match result {
                    Ok(DocInfo { page_count, first_size, title }) => {
                        self.status = Status::Ready;
                        self.page_count = *page_count;
                        self.sizes = vec![None; *page_count];
                        self.labels = vec![None; *page_count];
                        if let Some(size) = first_size {
                            self.fallback = *size;
                            if !self.sizes.is_empty() {
                                self.sizes[0] = Some(*size);
                            }
                        }
                        self.title = title.clone();
                        self.ask_sizes(engine, 0);
                    }
                    Err(OpenError::Password) => {
                        let wrong = matches!(self.status, Status::NeedsPassword { .. });
                        self.status = Status::NeedsPassword { wrong };
                    }
                    Err(e) => self.status = Status::Failed(e.clone()),
                }
                true
            }
            Event::Sizes { id, start, sizes, labels } if *id == self.id => {
                for (i, size) in sizes.iter().enumerate() {
                    if let Some(slot) = self.sizes.get_mut(start + i) {
                        *slot = *size;
                    }
                }
                for (i, label) in labels.iter().enumerate() {
                    if let Some(slot) = self.labels.get_mut(start + i) {
                        *slot = label.clone();
                    }
                }
                self.sizes_waiting = false;
                // The next batch, until every size is known.
                if let Some(next) = (0..self.page_count).step_by(SIZE_BATCH).find(|&s| self.sizes[s].is_none()) {
                    self.ask_sizes(engine, next);
                }
                true
            }
            Event::Outline { id, items } if *id == self.id => {
                self.outline = Some(items.clone());
                true
            }
            Event::TextLayout { id, page, layout, to_view } if *id == self.id => {
                self.text.insert(*page, Arc::new(PageText { layout: Arc::clone(layout), to_view: *to_view }));
                true
            }
            Event::Links { id, page, links } if *id == self.id => {
                self.links.insert(*page, Arc::clone(links));
                true
            }
            _ => false,
        }
    }

    fn ask_sizes(&mut self, engine: &Engine, start: usize) {
        if self.sizes_waiting || start >= self.page_count {
            return;
        }
        self.sizes_waiting = true;
        self.sizes_next = start;
        engine.request(Request::Sizes { id: self.id, start });
    }

    /// Make sure the sizes round `page` are asked for first (a jump far
    /// into the document).
    pub fn need_sizes_near(&mut self, engine: &Engine, page: usize) {
        if page < self.page_count && self.sizes.get(page).is_some_and(Option::is_none) && !self.sizes_waiting {
            let start = page / SIZE_BATCH * SIZE_BATCH;
            self.ask_sizes(engine, start);
        }
    }

    /// Ask for the bookmarks once.
    pub fn need_outline(&mut self, engine: &Engine) {
        if !self.outline_asked && self.ready() {
            self.outline_asked = true;
            engine.request(Request::Outline(self.id));
        }
    }

    /// Ask for the text of `page` once.
    pub fn need_text(&mut self, engine: &Engine, page: usize) {
        if self.ready() && page < self.page_count && !self.text.contains_key(&page) && self.text_asked.insert(page) {
            engine.request(Request::TextLayout { id: self.id, page });
        }
    }

    /// Ask for the links of `page` once.
    pub fn need_links(&mut self, engine: &Engine, page: usize) {
        if self.ready() && page < self.page_count && !self.links.contains_key(&page) && self.links_asked.insert(page) {
            engine.request(Request::Links { id: self.id, page });
        }
    }

    /// The size of `page`, or the fallback.
    pub fn size_of(&self, page: usize) -> [f32; 2] {
        self.sizes.get(page).copied().flatten().unwrap_or(self.fallback)
    }

    /// The page's label if the document names it, else its number.
    pub fn label(&self, page: usize) -> String {
        self.labels.get(page).cloned().flatten().unwrap_or_else(|| (page + 1).to_string())
    }

    /// The page with `label` (a document's own numbering), or a number.
    pub fn page_by_label(&self, text: &str) -> Option<usize> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(i) = self.labels.iter().position(|l| l.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(text))) {
            return Some(i);
        }
        let n: usize = text.parse().ok()?;
        (1..=self.page_count).contains(&n).then(|| n - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_text_maps_rectangles_through_the_affine() {
        // An unrotated A4 page: y flipped.
        let text = PageText { layout: Arc::new(TextLayout::default()), to_view: [1.0, 0.0, 0.0, -1.0, 0.0, 842.0] };
        assert_eq!(text.rect_to_page([72.0, 770.0, 172.0, 794.0]), [72.0, 48.0, 100.0, 24.0]);
        // Turned 90° clockwise (page x runs down the view).
        let turned = PageText { layout: Arc::new(TextLayout::default()), to_view: [0.0, 1.0, 1.0, 0.0, 0.0, 0.0] };
        assert_eq!(turned.rect_to_page([10.0, 20.0, 30.0, 50.0]), [20.0, 10.0, 30.0, 20.0]);
    }

    #[test]
    fn labels_and_numbers_find_pages() {
        let mut doc = DocState::new(1);
        doc.page_count = 5;
        doc.labels = vec![Some("i".into()), Some("ii".into()), Some("1".into()), None, None];
        assert_eq!(doc.page_by_label("ii"), Some(1));
        assert_eq!(doc.page_by_label("1"), Some(2));
        assert_eq!(doc.page_by_label("5"), Some(4));
        assert_eq!(doc.page_by_label("6"), None);
        assert_eq!(doc.page_by_label(""), None);
        assert_eq!(doc.label(3), "4");
        assert_eq!(doc.label(0), "i");
    }
}
