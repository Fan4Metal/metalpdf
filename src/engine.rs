//! The engine thread: the one place PDFium is called from. The UI hands it
//! documents to open and, every frame, the list of renders it wants, in
//! order of urgency; the thread takes the head of the list, renders it
//! progressively and, at every pause PDFium offers, looks at the list
//! again: a render nobody wants any more is dropped, a small preview that
//! became urgent is made in between. Results come back as plain data
//! through a channel, with a repaint request.
//!
//! The list is replaced wholesale each frame (as qview's loader), so a
//! page the reader flipped past is never rendered; what is being rendered
//! finishes unless the list no longer names it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use crate::pdfium::{self, Bitmap, Document, Link, OpenError, OutlineItem, Page, Pause, Placement, RenderStatus, TextLayout, TextObject};
use crate::pixels::Pixels;
use crate::settings::ZoomSetting;

pub type DocId = u32;

/// Previews are rendered this wide (and taken from a full render's mip
/// levels when there is one).
pub const PREVIEW_WIDTH: u32 = 200;
/// Page sizes are read in batches of this many.
pub const SIZE_BATCH: usize = 128;
/// Render flags: annotations drawn; no LCD text (fringes when scaled).
const FLAGS: i32 = pdfium::sys::FPDF_ANNOT;

/// What a render is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The page at its size on screen.
    Full,
    /// A small picture of the page ([`PREVIEW_WIDTH`]) for the sidebar and
    /// for the time the full render takes.
    Preview,
}

/// A render, as the cache keys it: the page, its width in pixels (of the
/// turned page) and the viewer's quarter turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub doc: DocId,
    pub page: u32,
    pub width: u32,
    pub turns: u8,
    pub kind: Kind,
}

/// The render the first document is to get before the window exists: the
/// view's probable size, so that the first frame finds the page ready.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FirstRender {
    /// The page area in points, as predicted from the saved window.
    pub viewport: [f32; 2],
    pub pixels_per_point: f32,
    pub zoom: ZoomSetting,
    pub turns: u8,
    pub page: usize,
}

/// A job for the engine other than a render.
#[derive(Debug)]
pub enum Request {
    Open { id: DocId, path: PathBuf, password: Option<String>, first: Option<FirstRender> },
    Close(DocId),
    /// The sizes (and labels) of pages `start..start + SIZE_BATCH`.
    Sizes { id: DocId, start: usize },
    Outline(DocId),
    TextLayout { id: DocId, page: usize },
    Links { id: DocId, page: usize },
    /// Find `query` in every page from `from` on, round the document.
    Search { id: DocId, search: u32, query: String, match_case: bool, whole_words: bool, from: usize },
    StopSearch(DocId),
    /// The text objects of a page, for the editing.
    TextObjects { id: DocId, page: usize },
    /// Replace the text of object `object` on `page`.
    EditText { id: DocId, page: usize, object: usize, text: String },
    /// Take the last edit back.
    UndoEdit(DocId),
    /// Write the edited document to `path`.
    Save { id: DocId, path: PathBuf },
}

/// What the engine says back.
#[derive(Debug)]
pub enum Event {
    /// PDFium could not be loaded: the technical reason.
    Unavailable(String),
    Opened { id: DocId, result: Result<DocInfo, OpenError> },
    Sizes { id: DocId, start: usize, sizes: Vec<Option<[f32; 2]>>, labels: Vec<Option<String>> },
    Rendered { key: RenderKey, pixels: Pixels, took_ms: f32 },
    RenderFailed(RenderKey),
    Outline { id: DocId, items: Vec<OutlineItem> },
    TextLayout { id: DocId, page: usize, layout: Arc<TextLayout>, to_view: [f32; 6] },
    Links { id: DocId, page: usize, links: Arc<Vec<Link>> },
    /// The hits on one page; `done` after the last page searched.
    SearchHits { id: DocId, search: u32, page: usize, hits: Vec<SearchHit>, done: bool },
    TextObjects { id: DocId, page: usize, objects: Arc<Vec<TextObject>> },
    /// `page` changed (an edit, or one taken back); `edits` is how many
    /// remain.
    Edited { id: DocId, page: usize, edits: usize },
    /// An edit could not be made (the reason, technical).
    EditFailed { id: DocId, reason: String },
    Saved { id: DocId, path: PathBuf, result: Result<(), String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub start: usize,
    pub count: usize,
    /// left, bottom, right, top in page space, one per line.
    pub rects: Vec<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocInfo {
    pub page_count: usize,
    /// The first page's size, for the layout until the others arrive.
    pub first_size: Option<[f32; 2]>,
    pub title: Option<String>,
}

/// The engine's list of renders, in the order they are wanted.
#[derive(Default)]
struct Queue {
    /// `(key, tier)`: tier 0 is wanted before anything else.
    wanted: Vec<(RenderKey, u8)>,
    requests: VecDeque<Request>,
    /// Renders sent but not yet taken by the UI: not started again.
    done: HashSet<RenderKey>,
    busy: Option<RenderKey>,
    quit: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    /// Bumped whenever the list changes: the render in progress looks at
    /// the list again at its next pause.
    generation: AtomicU64,
    ctx: OnceLock<egui::Context>,
}

/// The UI's handle on the engine thread.
pub struct Engine {
    shared: Arc<Shared>,
    events: Receiver<Event>,
}

impl Engine {
    /// Start the thread; `initial` is the first document and its first
    /// render, begun at once (before the window exists).
    pub fn start(initial: Option<(DocId, PathBuf, FirstRender)>) -> Engine {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            ctx: OnceLock::new(),
        });
        let (tx, events) = channel();
        if let Some((id, path, first)) = initial {
            shared.queue.lock().unwrap().requests.push_back(Request::Open { id, path, password: None, first: Some(first) });
        }
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("pdf".into())
            .spawn(move || Worker::run(worker, tx))
            .expect("spawn the engine thread");
        Engine { shared, events }
    }

    /// The context the thread wakes up with results.
    pub fn set_context(&self, ctx: egui::Context) {
        let _ = self.shared.ctx.set(ctx);
    }

    pub fn request(&self, request: Request) {
        let mut q = self.shared.queue.lock().unwrap();
        q.requests.push_back(request);
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }

    /// Replace the list of wanted renders. Keys already rendered and on
    /// their way are left out.
    pub fn want(&self, wanted: Vec<(RenderKey, u8)>) {
        let mut q = self.shared.queue.lock().unwrap();
        let wanted: Vec<(RenderKey, u8)> = wanted.into_iter().filter(|(k, _)| !q.done.contains(k)).collect();
        if q.wanted.iter().map(|(k, _)| *k).eq(wanted.iter().map(|(k, _)| *k)) {
            return;
        }
        q.wanted = wanted;
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }

    /// The next result, if one is there.
    pub fn poll(&self) -> Option<Event> {
        let event = self.events.try_recv().ok()?;
        if let Event::Rendered { key, .. } | Event::RenderFailed(key) = &event {
            self.shared.queue.lock().unwrap().done.remove(key);
        }
        Some(event)
    }

    /// Whether `key` is being rendered or is on its way.
    pub fn in_flight(&self, key: &RenderKey) -> bool {
        let q = self.shared.queue.lock().unwrap();
        q.busy == Some(*key) || q.done.contains(key)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let mut q = self.shared.queue.lock().unwrap();
        q.quit = true;
        self.shared.wake.notify_all();
    }
}

/// A search going on, a page per turn of the loop.
struct SearchState {
    id: DocId,
    search: u32,
    query: String,
    match_case: bool,
    whole_words: bool,
    next: usize,
    first: usize,
    searched: usize,
}

/// One replacement of a text object's text.
#[derive(Clone, Debug)]
struct EditEntry {
    page: usize,
    object: usize,
    text: String,
}

/// A document's edits: a second copy of the file they are applied to,
/// the viewed one staying as it is; undo rebuilds it from the log.
struct EditState {
    doc: Document,
    path: PathBuf,
    log: Vec<EditEntry>,
    /// The pages rendered from the edited copy.
    edited: HashSet<usize>,
}

/// The thread's side.
struct Worker {
    shared: Arc<Shared>,
    tx: Sender<Event>,
    api: &'static pdfium::sys::Api,
    docs: HashMap<DocId, Document>,
    /// Open pages: loading one parses its content, and it keeps its
    /// decoded images, so the recently used ones stay (`PAGE_CACHE`).
    pages: HashMap<(DocId, usize), Page>,
    page_order: VecDeque<(DocId, usize)>,
    search: Option<SearchState>,
    /// The documents' paths, for the edited copies.
    paths: HashMap<DocId, PathBuf>,
    edits: HashMap<DocId, EditState>,
    /// A TrueType font every character can be set in (Arial), read when
    /// an edit first needs it.
    fallback_font: Option<Vec<u8>>,
}

/// Open pages kept per engine: the decoded images of a scanned page are
/// tens of megabytes, so not many.
const PAGE_CACHE: usize = 12;

impl Worker {
    fn run(shared: Arc<Shared>, tx: Sender<Event>) {
        let api = match pdfium::init() {
            Ok(api) => api,
            Err(e) => {
                log::error!("PDFium unavailable: {e}");
                let _ = tx.send(Event::Unavailable(e.to_string()));
                if let Some(ctx) = shared.ctx.get() {
                    ctx.request_repaint();
                }
                return;
            }
        };
        let mut worker = Worker {
            shared,
            tx,
            api,
            docs: HashMap::new(),
            pages: HashMap::new(),
            page_order: VecDeque::new(),
            search: None,
            paths: HashMap::new(),
            edits: HashMap::new(),
            fallback_font: None,
        };
        worker.serve();
    }

    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        if let Some(ctx) = self.shared.ctx.get() {
            ctx.request_repaint();
        }
    }

    fn serve(&mut self) {
        loop {
            // Requests first (they are quick and the UI waits for them),
            // then the most wanted render, then a page of the search.
            let next = {
                let mut q = self.shared.queue.lock().unwrap();
                loop {
                    if q.quit {
                        return;
                    }
                    if let Some(request) = q.requests.pop_front() {
                        break Work::Request(request);
                    }
                    if let Some(&(key, _)) = q.wanted.first() {
                        q.wanted.remove(0);
                        q.busy = Some(key);
                        break Work::Render(key);
                    }
                    if self.search.is_some() {
                        break Work::SearchPage;
                    }
                    q = self.shared.wake.wait(q).unwrap();
                }
            };
            match next {
                Work::Request(request) => self.handle(request),
                Work::Render(key) => {
                    self.render(key);
                    self.shared.queue.lock().unwrap().busy = None;
                }
                Work::SearchPage => self.search_page(),
            }
        }
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Open { id, path, password, first } => {
                let started = Instant::now();
                let result = Document::open(self.api, &path, password.as_deref());
                match result {
                    Ok(doc) => {
                        let page_count = doc.page_count();
                        let first_size = doc.page_size(0);
                        let title = doc.meta_text("Title");
                        log::debug!("opened {} ({page_count} pages) in {:.1} ms", path.display(), started.elapsed().as_secs_f64() * 1e3);
                        self.docs.insert(id, doc);
                        self.paths.insert(id, path.clone());
                        self.send(Event::Opened { id, result: Ok(DocInfo { page_count, first_size, title }) });
                        if let (Some(first), Some(size)) = (first, first_size) {
                            let page = first.page.min(page_count.saturating_sub(1));
                            let size = self.docs[&id].page_size(page).unwrap_or(size);
                            let width = first_width(size, first);
                            let key = RenderKey { doc: id, page: page as u32, width, turns: first.turns, kind: Kind::Full };
                            self.render(key);
                        }
                    }
                    Err(e) => {
                        log::info!("cannot open {}: {e}", path.display());
                        self.send(Event::Opened { id, result: Err(e) });
                    }
                }
            }
            Request::Close(id) => {
                self.page_order.retain(|k| k.0 != id);
                self.pages.retain(|k, _| k.0 != id);
                self.edits.remove(&id);
                self.paths.remove(&id);
                self.docs.remove(&id);
                if self.search.as_ref().is_some_and(|s| s.id == id) {
                    self.search = None;
                }
            }
            Request::Sizes { id, start } => {
                let Some(doc) = self.docs.get(&id) else { return };
                let end = (start + SIZE_BATCH).min(doc.page_count());
                let sizes: Vec<Option<[f32; 2]>> = (start..end).map(|i| doc.page_size(i)).collect();
                let labels: Vec<Option<String>> = (start..end).map(|i| doc.page_label(i)).collect();
                self.send(Event::Sizes { id, start, sizes, labels });
            }
            Request::Outline(id) => {
                let Some(doc) = self.docs.get(&id) else { return };
                let items = doc.outline();
                self.send(Event::Outline { id, items });
            }
            Request::TextLayout { id, page } => {
                let Some(p) = self.page(id, page) else { return };
                let layout = Arc::new(p.text_layout());
                let to_view = p.page_to_view();
                self.send(Event::TextLayout { id, page, layout, to_view });
            }
            Request::Links { id, page } => {
                let Some(p) = self.page(id, page) else { return };
                let links = Arc::new(p.links());
                self.send(Event::Links { id, page, links });
            }
            Request::Search { id, search, query, match_case, whole_words, from } => {
                if !self.docs.contains_key(&id) || query.is_empty() {
                    return;
                }
                let count = self.docs[&id].page_count();
                let from = from.min(count.saturating_sub(1));
                self.search = Some(SearchState { id, search, query, match_case, whole_words, next: from, first: from, searched: 0 });
            }
            Request::StopSearch(id) => {
                if self.search.as_ref().is_some_and(|s| s.id == id) {
                    self.search = None;
                }
            }
            Request::TextObjects { id, page } => {
                let Some(p) = self.page(id, page) else { return };
                let objects = Arc::new(p.text_objects());
                self.send(Event::TextObjects { id, page, objects });
            }
            Request::EditText { id, page, object, text } => match self.edit_text(id, page, object, text) {
                Ok(edits) => self.send(Event::Edited { id, page, edits }),
                Err(reason) => {
                    log::info!("edit refused: {reason}");
                    self.send(Event::EditFailed { id, reason });
                }
            },
            Request::UndoEdit(id) => {
                let Some(state) = self.edits.get_mut(&id) else { return };
                let Some(undone) = state.log.pop() else { return };
                let log = state.log.clone();
                match self.rebuild_edits(id, log) {
                    Ok(edits) => self.send(Event::Edited { id, page: undone.page, edits }),
                    Err(reason) => self.send(Event::EditFailed { id, reason }),
                }
            }
            Request::Save { id, path } => {
                let result = match self.edits.get(&id) {
                    Some(state) => state.doc.save_to(&path),
                    None => Err("nothing was edited".into()),
                };
                self.send(Event::Saved { id, path, result });
            }
        }
    }

    /// Replace the text of `object` on `page` in the document's edited
    /// copy (opened at the first edit); how many edits there are then.
    fn edit_text(&mut self, id: DocId, page: usize, object: usize, text: String) -> Result<usize, String> {
        if text.trim().is_empty() {
            return Err("the text is empty".into());
        }
        if !self.edits.contains_key(&id) {
            let path = self.paths.get(&id).cloned().ok_or("no such document")?;
            let doc = Document::open(self.api, &path, None).map_err(|e| e.to_string())?;
            let obstacles = doc.edit_obstacles();
            if !obstacles.is_empty() {
                return Err(format!("the document is {}", obstacles.join(" and ")));
            }
            self.edits.insert(id, EditState { doc, path, log: Vec::new(), edited: HashSet::new() });
        }
        if self.fallback_font.is_none() {
            let fonts = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            self.fallback_font = std::fs::read(fonts.join("Fonts").join("arial.ttf")).ok();
        }
        // The page comes from the edited copy from now on.
        self.pages.remove(&(id, page));
        self.page_order.retain(|k| *k != (id, page));
        self.edits.get_mut(&id).expect("inserted above").edited.insert(page);
        let fallback = self.fallback_font.clone();
        let p = self.page(id, page).ok_or("the page could not be loaded")?;
        p.set_text(object, &text, fallback.as_deref())?;
        let state = self.edits.get_mut(&id).expect("inserted above");
        state.log.push(EditEntry { page, object, text });
        Ok(state.log.len())
    }

    /// The edited copy made again from the file with `log` applied.
    fn rebuild_edits(&mut self, id: DocId, log: Vec<EditEntry>) -> Result<usize, String> {
        let path = self.paths.get(&id).cloned().ok_or("no such document")?;
        self.pages.retain(|k, _| k.0 != id);
        self.page_order.retain(|k| k.0 != id);
        self.edits.remove(&id);
        if log.is_empty() {
            return Ok(0);
        }
        let doc = Document::open(self.api, &path, None).map_err(|e| e.to_string())?;
        self.edits.insert(id, EditState { doc, path, log: Vec::new(), edited: HashSet::new() });
        let count = log.len();
        for entry in log {
            self.edit_text(id, entry.page, entry.object, entry.text)?;
        }
        Ok(count)
    }

    /// One page of the search going on, round the document from where it
    /// started.
    fn search_page(&mut self) {
        let Some(state) = self.search.as_mut() else { return };
        let (id, search, page) = (state.id, state.search, state.next);
        let Some(count) = self.docs.get(&id).map(Document::page_count) else {
            self.search = None;
            return;
        };
        let (query, match_case, whole_words) = (state.query.clone(), state.match_case, state.whole_words);
        state.searched += 1;
        state.next = (state.next + 1) % count.max(1);
        let done = state.searched >= count || state.next == state.first;
        if done {
            self.search = None;
        }
        let hits = match self.page(id, page) {
            Some(p) => p
                .find(&query, match_case, whole_words)
                .into_iter()
                .map(|(start, n)| SearchHit { start, count: n, rects: p.text_rects(start, n) })
                .collect(),
            None => Vec::new(),
        };
        self.send(Event::SearchHits { id, search, page, hits, done });
    }

    /// Page `index` of document `id`, loaded if it is not open: from the
    /// edited copy when the page was edited.
    fn page(&mut self, id: DocId, index: usize) -> Option<&Page> {
        let key = (id, index);
        if !self.pages.contains_key(&key) {
            let edited = self.edits.get(&id).filter(|e| e.edited.contains(&index)).map(|e| &e.doc);
            let page = edited.or_else(|| self.docs.get(&id))?.load_page(index)?;
            self.pages.insert(key, page);
            self.page_order.push_back(key);
            while self.page_order.len() > PAGE_CACHE {
                let old = self.page_order.pop_front().expect("not empty");
                // Not the page being rendered (none is, between renders).
                self.pages.remove(&old);
            }
        } else {
            self.page_order.retain(|k| *k != key);
            self.page_order.push_back(key);
        }
        self.pages.get(&key)
    }

    /// Whether `key` is still wanted (or busy, which it is while rendering).
    fn still_wanted(&self, key: &RenderKey) -> bool {
        let q = self.shared.queue.lock().unwrap();
        q.busy == Some(*key) && !q.quit
    }

    /// Render `key` progressively, dropping it if the UI stops wanting it;
    /// a preview that becomes the most urgent render is made in a pause.
    fn render(&mut self, key: RenderKey) {
        let started = Instant::now();
        let Some(size) = self.docs.get(&key.doc).and_then(|d| d.page_size(key.page as usize)) else {
            self.send(Event::RenderFailed(key));
            return;
        };
        let turned = crate::layout::turned(size, key.turns);
        let width = key.width.max(1);
        let height = ((width as f32) * turned[1] / turned[0]).round().clamp(1.0, 1.0e5) as u32;
        let Some(mut bitmap) = Bitmap::new(self.api, width, height) else {
            self.send(Event::RenderFailed(key));
            return;
        };
        let at = Placement { start_x: 0, start_y: 0, size_x: width as i32, size_y: height as i32, rotate: i32::from(key.turns % 4) };
        if self.page(key.doc, key.page as usize).is_none() {
            self.send(Event::RenderFailed(key));
            return;
        }
        let page = self.pages.get(&(key.doc, key.page as usize)).expect("loaded just now");
        // The pause callback: pause when the list changed since the last
        // look (the generation moved).
        let shared = Arc::clone(&self.shared);
        let mut seen = shared.generation.load(Ordering::Relaxed);
        let mut pause = Pause::new(move || {
            let now = shared.generation.load(Ordering::Relaxed);
            if now != seen {
                seen = now;
                true
            } else {
                false
            }
        });
        let mut status = page.render_start(&mut bitmap, at, FLAGS, None, &mut pause);
        let mut interleaved: Vec<RenderKey> = Vec::new();
        while status == RenderStatus::Paused {
            if !self.still_wanted(&key) {
                page.render_close();
                log::debug!("render of page {} at {} px dropped after {:.1} ms", key.page + 1, key.width, started.elapsed().as_secs_f64() * 1e3);
                return;
            }
            // A preview wanted before everything else is made now, on
            // another page handle (the one rendering keeps its context).
            let urgent = {
                let mut q = self.shared.queue.lock().unwrap();
                match q.wanted.first() {
                    Some(&(k, 0)) if k.kind == Kind::Preview && (k.doc, k.page) != (key.doc, key.page) => {
                        q.wanted.remove(0);
                        Some(k)
                    }
                    _ => None,
                }
            };
            if let Some(k) = urgent {
                interleaved.push(k);
            }
            status = page.render_continue(&mut bitmap, at, FLAGS, &mut pause);
        }
        let took = started.elapsed().as_secs_f32() * 1e3;
        if status == RenderStatus::Done {
            let bgrx = bitmap.into_pixels();
            let pixels = match key.kind {
                Kind::Full => Pixels::with_levels(width, height, bgrx),
                Kind::Preview => Pixels::flat(width, height, bgrx),
            };
            if key.kind == Kind::Full && width > PREVIEW_WIDTH {
                // The preview comes free with the full render.
                let preview = pixels.level_at_most(PREVIEW_WIDTH);
                let pk = RenderKey { width: PREVIEW_WIDTH, kind: Kind::Preview, ..key };
                self.shared.queue.lock().unwrap().done.insert(pk);
                self.send(Event::Rendered { key: pk, pixels: preview, took_ms: 0.0 });
            }
            log::debug!("page {} rendered at {}x{} in {took:.1} ms", key.page + 1, width, height);
            self.shared.queue.lock().unwrap().done.insert(key);
            self.send(Event::Rendered { key, pixels, took_ms: took });
        } else {
            self.send(Event::RenderFailed(key));
        }
        // The previews that queued up during the pauses.
        for k in interleaved {
            self.render_preview(k);
        }
    }

    /// A preview in one go, between the pauses of a larger render.
    fn render_preview(&mut self, key: RenderKey) {
        let started = Instant::now();
        let Some(size) = self.docs.get(&key.doc).and_then(|d| d.page_size(key.page as usize)) else {
            self.send(Event::RenderFailed(key));
            return;
        };
        let turned = crate::layout::turned(size, key.turns);
        let width = key.width.max(1);
        let height = ((width as f32) * turned[1] / turned[0]).round().clamp(1.0, 1.0e5) as u32;
        let (Some(mut bitmap), Some(page)) = (Bitmap::new(self.api, width, height), self.page(key.doc, key.page as usize)) else {
            self.send(Event::RenderFailed(key));
            return;
        };
        let at = Placement { start_x: 0, start_y: 0, size_x: width as i32, size_y: height as i32, rotate: i32::from(key.turns % 4) };
        page.render(&mut bitmap, at, FLAGS);
        let pixels = Pixels::flat(width, height, bitmap.into_pixels());
        self.shared.queue.lock().unwrap().done.insert(key);
        self.send(Event::Rendered { key, pixels, took_ms: started.elapsed().as_secs_f32() * 1e3 });
    }
}

enum Work {
    Request(Request),
    Render(RenderKey),
    SearchPage,
}

/// The pixel width of a page of `size` for the first render: the same sum
/// the UI does once it knows the view (`layout::render_width`).
pub fn first_width(size: [f32; 2], first: FirstRender) -> u32 {
    crate::layout::render_width(size, first.zoom, first.turns, first.viewport, first.pixels_per_point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_render_takes_the_predicted_view() {
        let first = FirstRender {
            viewport: [1000.0, 800.0],
            pixels_per_point: 2.0,
            zoom: ZoomSetting::FitWidth,
            turns: 0,
            page: 0,
        };
        // Fit to the width less the gaps, at two pixels a point.
        assert_eq!(first_width([595.0, 842.0], first), ((1000.0 - 16.0) * 2.0f32).round() as u32);
        let scaled = FirstRender { zoom: ZoomSetting::Scale(1.0), ..first };
        assert_eq!(first_width([595.0, 842.0], scaled), (595.0 * crate::layout::POINT_SCALE * 2.0f32).round() as u32);
    }

    /// The thread serves without PDFium: it reports that and ends.
    #[test]
    fn wanted_list_leaves_out_what_is_on_its_way() {
        let engine = Engine::start(None);
        let key = RenderKey { doc: 1, page: 0, width: 100, turns: 0, kind: Kind::Full };
        engine.shared.queue.lock().unwrap().done.insert(key);
        engine.want(vec![(key, 1), (RenderKey { page: 1, ..key }, 1)]);
        let q = engine.shared.queue.lock().unwrap();
        assert_eq!(q.wanted.len(), 1);
        assert_eq!(q.wanted[0].0.page, 1);
    }
}
