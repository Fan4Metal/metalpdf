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

use crate::pdfium::{self, Bitmap, Document, Link, OpenError, OutlineItem, Page, PageObjects, Pause, Placement, RenderStatus, TextLayout};
use crate::pixels::Pixels;
use crate::settings::ZoomSetting;

pub type DocId = u32;

/// Previews are rendered this wide (and taken from a full render's mip
/// levels when there is one).
pub const PREVIEW_WIDTH: u32 = 200;
/// A page whose whole render would have more pixels than this (or a side
/// longer than the GPU takes) is drawn from tiles of the part in view.
pub const TILE_PIXELS: u64 = 16 << 20;

/// Whether a page of `width` × `height` pixels is drawn from tiles.
pub fn too_large(width: u32, height: u32, max_side: u64) -> bool {
    let (w, h) = (u64::from(width), u64::from(height));
    w * h > TILE_PIXELS || w > max_side || h > max_side
}

/// The height in pixels of a page of `turned` size rendered `width` wide
/// (the sum `Worker::render` makes).
pub fn pixel_height(turned: [f32; 2], width: u32) -> u32 {
    ((width as f32) * turned[1] / turned[0].max(1.0)).round().clamp(1.0, 1.0e5) as u32
}
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
    /// A part of the page at its size on screen, when the whole would be
    /// too large a texture (a high zoom): the rectangle in pixels of the
    /// page rendered at the key's width, from its top left corner.
    Tile { x: u32, y: u32, w: u32, h: u32 },
}

impl Kind {
    /// Full renders and tiles: the large ones, uploaded one a frame.
    pub fn is_large(self) -> bool {
        self != Kind::Preview
    }
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
    /// Look for the numbers printed on the pages (`Event::Numbering`).
    DetectNumbering(DocId),
    /// The sizes (and labels) of pages `start..start + SIZE_BATCH`.
    Sizes { id: DocId, start: usize },
    Outline(DocId),
    TextLayout { id: DocId, page: usize },
    Links { id: DocId, page: usize },
    /// Find `query` in every page from `from` on, round the document.
    Search { id: DocId, search: u32, query: String, match_case: bool, whole_words: bool, from: usize },
    StopSearch(DocId),
    /// The text and image objects of a page, for the editing.
    Objects { id: DocId, page: usize },
    /// The pixels of an image object as shown (`Event::ImagePixels`).
    ImagePixels { id: DocId, page: usize, object: usize },
    /// An edit of a page's object.
    Edit { id: DocId, edit: Edit },
    /// Take the last edit back.
    UndoEdit(DocId),
    /// Write the edited document to `path`.
    Save { id: DocId, path: PathBuf },
    /// Print the document as `job` says (`Event::PrintProgress`, then
    /// `Event::Printed`); a page per turn of the loop, between renders.
    Print { id: DocId, job: PrintJob },
    /// Abandon the print job under way.
    CancelPrint,
}

/// A print job as the dialog sets it up.
pub struct PrintJob {
    pub printer: String,
    pub devmode: Option<crate::print::DevMode>,
    /// The pages, one a sheet, in the order printed (copies included).
    pub sheets: Vec<usize>,
    pub scale: crate::settings::PrintScale,
    pub auto_rotate: bool,
    /// Each page sent as a picture (`AS_IMAGE_DPI` at most).
    pub as_image: bool,
    /// The name the spooler shows.
    pub title: String,
    /// The printer's output to this file instead (a test's).
    pub output: Option<PathBuf>,
}

impl std::fmt::Debug for PrintJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrintJob").field("printer", &self.printer).field("sheets", &self.sheets.len()).field("as_image", &self.as_image).finish()
    }
}

/// How a print job ended.
#[derive(Debug, PartialEq)]
pub enum Printed {
    /// Every sheet went to the spooler: how many.
    Done(usize),
    Cancelled,
    Failed(String),
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
    /// The page shift the numbers printed on the pages agree on, `None`
    /// when they do not (`Request::DetectNumbering`).
    Numbering { id: DocId, shift: Option<i32> },
    Objects { id: DocId, page: usize, objects: Arc<PageObjects> },
    /// An image object's picture: width, height, RGBA; or why not.
    ImagePixels { id: DocId, picture: Result<(u32, u32, Vec<u8>), String> },
    /// `page` changed (an edit, or one taken back); `edits` is how many
    /// remain.
    Edited { id: DocId, page: usize, edits: usize },
    /// An edit could not be made (the reason, technical).
    EditFailed { id: DocId, reason: String },
    /// The pages changed (one put in, removed or replaced): the document
    /// has `page_count` pages now, sizes and all to be read again.
    Restructured { id: DocId, page_count: usize },
    Saved { id: DocId, path: PathBuf, result: Result<(), String> },
    /// `done` sheets of `total` sent to the printer.
    PrintProgress { id: DocId, done: usize, total: usize },
    Printed { id: DocId, outcome: Printed },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub start: usize,
    pub count: usize,
    /// left, bottom, right, top in page space, one per line.
    pub rects: Vec<[f32; 4]>,
    /// The line the hit is on, for the sidebar's list: the text before
    /// the match on its line, the match, the text after it.
    pub before: String,
    pub matched: String,
    pub after: String,
}

/// How many characters round a hit the sidebar's line may show.
pub const HIT_CONTEXT: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct DocInfo {
    pub page_count: usize,
    /// The first page's size, for the layout until the others arrive.
    pub first_size: Option<[f32; 2]>,
    pub title: Option<String>,
    /// The document lets a printer scale its pages (`Document::print_scaling`).
    pub print_scaling: bool,
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
        // The render under way is dropped at its next pause when the
        // list no longer names it (`Worker::still_wanted`).
        if let Some(busy) = q.busy
            && !wanted.iter().any(|(k, _)| *k == busy)
        {
            q.busy = None;
        }
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

/// One change of a page's object. Objects are named by their index at
/// the time of the edit; the log is replayed in order, so the indices
/// stay right.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Text objects of a line made one, the first, with `text`; the
    /// objects after them on the line moved by the change in width
    /// (`textline::LineEdit`, `Page::set_line`).
    Text { page: usize, objects: Vec<usize>, text: String, followers: Vec<usize> },
    /// An image removed.
    Remove { page: usize, object: usize },
    /// An image's picture replaced by the one in `path` (JPEG or PNG).
    ReplaceImage { page: usize, object: usize, path: PathBuf },
    /// An image moved and resized to `bounds` (page space).
    Place { page: usize, object: usize, bounds: [f32; 4] },
    /// A blank page of `size` (points) put in at `page`.
    InsertBlank { page: usize, size: [f32; 2] },
    /// Every page of the PDF at `path` put in at `page`.
    InsertPdf { page: usize, path: PathBuf },
    /// Page `page` replaced by every page of the PDF at `path`.
    ReplacePdf { page: usize, path: PathBuf },
    DeletePage { page: usize },
}

impl Edit {
    pub fn page(&self) -> usize {
        match self {
            Edit::Text { page, .. }
            | Edit::Remove { page, .. }
            | Edit::ReplaceImage { page, .. }
            | Edit::Place { page, .. }
            | Edit::InsertBlank { page, .. }
            | Edit::InsertPdf { page, .. }
            | Edit::ReplacePdf { page, .. }
            | Edit::DeletePage { page } => *page,
        }
    }

    /// Whether the edit changes which pages there are: after it, every
    /// page of the document comes from the edited copy.
    pub fn restructures(&self) -> bool {
        matches!(self, Edit::InsertBlank { .. } | Edit::InsertPdf { .. } | Edit::ReplacePdf { .. } | Edit::DeletePage { .. })
    }
}

/// A document's edits: a second copy of the file they are applied to,
/// the viewed one staying as it is; undo rebuilds it from the log.
/// Whether the document's `obstacles` (`Document::edit_obstacles`) rule
/// edits out: a signed or encrypted document takes none.
fn refuse(obstacles: &[&'static str]) -> Result<(), String> {
    if obstacles.is_empty() { Ok(()) } else { Err(format!("the document is {}", obstacles.join(" and "))) }
}

struct EditState {
    doc: Document,
    /// What the document is that stands in the way of edits
    /// (`Document::edit_obstacles`), checked against each edit.
    obstacles: Vec<&'static str>,
    log: Vec<Edit>,
    /// The pages rendered from the edited copy.
    edited: HashSet<usize>,
    /// Pages were put in or removed: every page comes from the copy.
    all_pages: bool,
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
    /// The passwords documents were opened with, for the copy a print job
    /// opens.
    passwords: HashMap<DocId, String>,
    /// The print job under way.
    printing: Option<Printing>,
    /// A TrueType font every character can be set in (Arial), read when
    /// an edit first needs it.
    /// The stand-in fonts for replaced text (`fonts::Fonts`).
    fonts: crate::fonts::Fonts,
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
            passwords: HashMap::new(),
            printing: None,
            fonts: Default::default(),
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
                    // The first key not rendered meanwhile: the list the
                    // UI sent while a render ran names that render too.
                    let mut picked = None;
                    while let Some(&(key, _)) = q.wanted.first() {
                        q.wanted.remove(0);
                        if !q.done.contains(&key) {
                            picked = Some(key);
                            break;
                        }
                    }
                    if let Some(key) = picked {
                        q.busy = Some(key);
                        break Work::Render(key);
                    }
                    if self.printing.is_some() {
                        break Work::PrintPage;
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
                Work::PrintPage => self.print_page(),
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
                        let print_scaling = doc.print_scaling();
                        if let Some(password) = &password {
                            self.passwords.insert(id, password.clone());
                        }
                        log::debug!("opened {} ({page_count} pages) in {:.1} ms", path.display(), started.elapsed().as_secs_f64() * 1e3);
                        self.docs.insert(id, doc);
                        self.paths.insert(id, path.clone());
                        self.send(Event::Opened { id, result: Ok(DocInfo { page_count, first_size, title, print_scaling }) });
                        if let (Some(first), Some(size)) = (first, first_size) {
                            let page = first.page.min(page_count.saturating_sub(1));
                            let size = self.docs[&id].page_size(page).unwrap_or(size);
                            let width = first_width(size, first);
                            let height = pixel_height(crate::layout::turned(size, first.turns), width);
                            if too_large(width, height, u64::MAX) {
                                // A high zoom: the UI asks for the tile in view.
                                log::debug!("first render skipped: {width}x{height} px is tiled");
                            } else {
                                let key = RenderKey { doc: id, page: page as u32, width, turns: first.turns, kind: Kind::Full };
                                self.render(key);
                            }
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
                self.passwords.remove(&id);
                self.docs.remove(&id);
                if self.search.as_ref().is_some_and(|s| s.id == id) {
                    self.search = None;
                }
            }
            Request::Sizes { id, start } => {
                let Some(doc) = self.view_doc(id) else { return };
                let end = (start + SIZE_BATCH).min(doc.page_count());
                let sizes: Vec<Option<[f32; 2]>> = (start..end).map(|i| doc.page_size(i)).collect();
                let labels: Vec<Option<String>> = (start..end).map(|i| doc.page_label(i)).collect();
                self.send(Event::Sizes { id, start, sizes, labels });
            }
            Request::Outline(id) => {
                let Some(doc) = self.view_doc(id) else { return };
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
                let count = self.view_doc(id).map_or(0, Document::page_count);
                let from = from.min(count.saturating_sub(1));
                self.search = Some(SearchState { id, search, query, match_case, whole_words, next: from, first: from, searched: 0 });
            }
            Request::StopSearch(id) => {
                if self.search.as_ref().is_some_and(|s| s.id == id) {
                    self.search = None;
                }
            }
            Request::DetectNumbering(id) => {
                // The start and the end of two dozen pages spread over the
                // document, where page numbers are printed.
                let count = self.view_doc(id).map_or(0, Document::page_count);
                let samples = NUMBERING_SAMPLES.min(count);
                let mut texts = Vec::with_capacity(samples);
                for s in 0..samples {
                    let index = if samples == count { s } else { s * count / samples + count / (2 * samples) };
                    if let Some(p) = self.page(id, index) {
                        let n = p.char_count();
                        let text = if n > 2 * NUMBERING_CHARS {
                            format!("{}\n{}", p.text(0, NUMBERING_CHARS), p.text(n - NUMBERING_CHARS, NUMBERING_CHARS))
                        } else {
                            p.text(0, n)
                        };
                        texts.push((index, text));
                    }
                }
                let shift = numbering_shift(&texts);
                log::debug!("page numbering of document {id}: shift {shift:?} from {} pages", texts.len());
                self.send(Event::Numbering { id, shift });
            }
            Request::Objects { id, page } => {
                let Some(p) = self.page(id, page) else { return };
                let mut objects = p.objects();
                // Which installed font would stand in for each run's, for
                // the editor's tooltip.
                for o in &mut objects.text {
                    o.stand_in = self.fonts.stand_in_name(&o.font, o.flags);
                }
                objects.lines = crate::textline::lines(&objects.text);
                self.send(Event::Objects { id, page, objects: Arc::new(objects) });
            }
            Request::ImagePixels { id, page, object } => {
                let picture = match self.page(id, page) {
                    Some(p) => p.image_pixels(object),
                    None => Err("the page could not be loaded".to_owned()),
                };
                self.send(Event::ImagePixels { id, picture });
            }
            Request::Edit { id, edit } => match self.edit(id, edit.clone()) {
                Ok(edits) => {
                    self.send(Event::Edited { id, page: edit.page(), edits });
                    if edit.restructures() {
                        self.send_restructured(id);
                    }
                }
                Err(reason) => {
                    log::info!("edit refused: {reason}");
                    self.send(Event::EditFailed { id, reason });
                }
            },
            Request::UndoEdit(id) => {
                let Some(state) = self.edits.get_mut(&id) else { return };
                let Some(undone) = state.log.pop() else { return };
                let log = state.log.clone();
                let restructured = undone.restructures();
                match self.rebuild_edits(id, log) {
                    Ok(edits) => {
                        self.send(Event::Edited { id, page: undone.page(), edits });
                        if restructured {
                            self.send_restructured(id);
                        }
                    }
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
            Request::Print { id, job } => {
                if let Some(old) = self.printing.take() {
                    self.send(Event::Printed { id: old.id, outcome: Printed::Cancelled });
                }
                match self.start_print(id, job) {
                    Ok(printing) => {
                        self.send(Event::PrintProgress { id, done: 0, total: printing.sheets.len() });
                        self.printing = Some(printing);
                    }
                    Err(e) => {
                        log::warn!("print job not started: {e}");
                        self.send(Event::Printed { id, outcome: Printed::Failed(e) });
                    }
                }
            }
            Request::CancelPrint => {
                if let Some(printing) = self.printing.take() {
                    log::info!("print job cancelled after {} of {} sheets", printing.next, printing.sheets.len());
                    self.send(Event::Printed { id: printing.id, outcome: Printed::Cancelled });
                }
            }
        }
    }

    /// Apply `edit` in the document's edited copy (opened at the first
    /// edit); how many edits there are then.
    fn edit(&mut self, id: DocId, edit: Edit) -> Result<usize, String> {
        let page = edit.page();
        if let Edit::Text { text, .. } = &edit
            && text.trim().is_empty()
        {
            return Err("the text is empty".into());
        }
        if let Some(state) = self.edits.get(&id) {
            refuse(&state.obstacles)?;
        } else {
            let path = self.paths.get(&id).cloned().ok_or("no such document")?;
            let doc = Document::open(self.api, &path, None).map_err(|e| e.to_string())?;
            let obstacles = doc.edit_obstacles();
            refuse(&obstacles)?;
            self.edits.insert(id, EditState { doc, obstacles, log: Vec::new(), edited: HashSet::new(), all_pages: false });
        }
        if edit.restructures() {
            // Every open page of the document is stale (its index may
            // name another page now); every page comes from the copy.
            self.pages.retain(|k, _| k.0 != id);
            self.page_order.retain(|k| k.0 != id);
            let state = self.edits.get_mut(&id).expect("inserted above");
            state.all_pages = true;
            let doc = &state.doc;
            match &edit {
                Edit::InsertBlank { page, size } => doc.insert_blank_page(*page, *size)?,
                Edit::InsertPdf { page, path } => {
                    let source = Document::open(self.api, path, None).map_err(|e| e.to_string())?;
                    doc.import_pages(&source, *page)?;
                }
                Edit::ReplacePdf { page, path } => {
                    let source = Document::open(self.api, path, None).map_err(|e| e.to_string())?;
                    doc.import_pages(&source, *page + 1)?;
                    doc.delete_page(*page)?;
                }
                Edit::DeletePage { page } => {
                    if doc.page_count() <= 1 {
                        return Err("the last page stays".into());
                    }
                    doc.delete_page(*page)?;
                }
                _ => unreachable!("not a page edit"),
            }
            state.log.push(edit);
            return Ok(state.log.len());
        }
        // The page comes from the edited copy from now on.
        self.pages.remove(&(id, page));
        self.page_order.retain(|k| *k != (id, page));
        self.edits.get_mut(&id).expect("inserted above").edited.insert(page);
        // The stand-in fonts, lent to the page while it edits.
        let mut fonts = std::mem::take(&mut self.fonts);
        let applied = match self.page(id, page) {
            None => Err("the page could not be loaded".to_owned()),
            Some(p) => match &edit {
                Edit::Text { objects, text, followers, .. } => p.set_line(objects, text, followers, &mut |name, flags| fonts.get(name, flags)).map(|_| ()),
                Edit::Remove { object, .. } => p.remove_object(*object),
                Edit::ReplaceImage { object, path, .. } => p.replace_image(*object, path),
                Edit::Place { object, bounds, .. } => p.place_object(*object, *bounds),
                _ => unreachable!("a page edit"),
            },
        };
        self.fonts = fonts;
        applied?;
        let state = self.edits.get_mut(&id).expect("inserted above");
        state.log.push(edit);
        Ok(state.log.len())
    }

    /// The document whose pages are shown: the edited copy once pages
    /// were put in or removed, else the original.
    fn view_doc(&self, id: DocId) -> Option<&Document> {
        match self.edits.get(&id) {
            Some(state) if state.all_pages => Some(&state.doc),
            _ => self.docs.get(&id),
        }
    }

    fn send_restructured(&self, id: DocId) {
        if let Some(doc) = self.view_doc(id) {
            self.send(Event::Restructured { id, page_count: doc.page_count() });
        }
    }

    /// A print job begun: its own copy of the document (the edited one,
    /// saved, when there are edits: what is printed is what is shown) and
    /// the printer's document started.
    fn start_print(&mut self, id: DocId, job: PrintJob) -> Result<Printing, String> {
        if job.sheets.is_empty() {
            return Err("no pages to print".into());
        }
        let path = self.paths.get(&id).cloned().ok_or("no such document")?;
        let (doc, temp) = match self.edits.get(&id) {
            Some(state) if !state.log.is_empty() => {
                let temp = TempFile(std::env::temp_dir().join(format!("metalpdf-print-{}-{id}.pdf", std::process::id())));
                state.doc.save_to(&temp.0)?;
                (Document::open(self.api, &temp.0, None).map_err(|e| e.to_string())?, Some(temp))
            }
            _ => (Document::open(self.api, &path, self.passwords.get(&id).map(String::as_str)).map_err(|e| e.to_string())?, None),
        };
        if job.sheets.iter().any(|&p| p >= doc.page_count()) {
            return Err("a page past the document's end".into());
        }
        let printer = crate::print::Job::start(&job.printer, job.devmode.as_ref(), &job.title, job.output.as_deref())?;
        log::info!("printing {} sheets on {} ({:?})", job.sheets.len(), job.printer, printer.sheet());
        Ok(Printing { id, doc, job: printer, sheets: job.sheets, next: 0, scale: job.scale, auto_rotate: job.auto_rotate, as_image: job.as_image, _temp: temp })
    }

    /// The next sheet of the print job; the job finished after the last.
    fn print_page(&mut self) {
        let Some(printing) = self.printing.as_mut() else { return };
        let started = Instant::now();
        let index = printing.sheets[printing.next];
        let result = print_sheet(self.api, printing, index);
        log::debug!("page {} printed in {:.1} ms", index + 1, started.elapsed().as_secs_f64() * 1e3);
        let (id, total) = (printing.id, printing.sheets.len());
        if let Err(e) = result {
            log::warn!("printing page {} failed: {e}", index + 1);
            self.printing = None;
            self.send(Event::Printed { id, outcome: Printed::Failed(e) });
            return;
        }
        printing.next += 1;
        let done = printing.next;
        if done < total {
            self.send(Event::PrintProgress { id, done, total });
            return;
        }
        let Some(Printing { job, .. }) = self.printing.take() else { return };
        let outcome = match job.finish() {
            Ok(()) => Printed::Done(total),
            Err(e) => Printed::Failed(e),
        };
        self.send(Event::Printed { id, outcome });
    }

    /// The edited copy made again from the file with `log` applied.
    fn rebuild_edits(&mut self, id: DocId, log: Vec<Edit>) -> Result<usize, String> {
        let path = self.paths.get(&id).cloned().ok_or("no such document")?;
        self.pages.retain(|k, _| k.0 != id);
        self.page_order.retain(|k| k.0 != id);
        self.edits.remove(&id);
        if log.is_empty() {
            return Ok(0);
        }
        let doc = Document::open(self.api, &path, None).map_err(|e| e.to_string())?;
        let obstacles = doc.edit_obstacles();
        self.edits.insert(id, EditState { doc, obstacles, log: Vec::new(), edited: HashSet::new(), all_pages: false });
        let count = log.len();
        for entry in log {
            self.edit(id, entry)?;
        }
        Ok(count)
    }

    /// One page of the search going on, round the document from where it
    /// started.
    fn search_page(&mut self) {
        let Some(id) = self.search.as_ref().map(|s| s.id) else { return };
        let Some(count) = self.view_doc(id).map(Document::page_count) else {
            self.search = None;
            return;
        };
        let Some(state) = self.search.as_mut() else { return };
        let (search, page) = (state.search, state.next);
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
                .map(|(start, n)| {
                    let from = start.saturating_sub(HIT_CONTEXT);
                    SearchHit {
                        start,
                        count: n,
                        rects: p.text_rects(start, n),
                        before: crate::search::line_before(&p.text(from, start - from)),
                        matched: crate::selection::unhyphenate(&p.text(start, n)),
                        after: crate::search::line_after(&p.text(start + n, HIT_CONTEXT)),
                    }
                })
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
            let edited = self.edits.get(&id).filter(|e| e.all_pages || e.edited.contains(&index)).map(|e| &e.doc);
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
        let Some(size) = self.view_doc(key.doc).and_then(|d| d.page_size(key.page as usize)) else {
            self.send(Event::RenderFailed(key));
            return;
        };
        let turned = crate::layout::turned(size, key.turns);
        let width = key.width.max(1);
        let height = pixel_height(turned, width);
        // A tile: a bitmap of its own size, the page placed so that the
        // tile's corner is the bitmap's; PDFium draws what falls inside.
        let (bw, bh, origin) = match key.kind {
            Kind::Tile { x, y, w, h } => (w.min(width.saturating_sub(x)).max(1), h.min(height.saturating_sub(y)).max(1), (-(x as i32), -(y as i32))),
            _ => (width, height, (0, 0)),
        };
        let Some(mut bitmap) = Bitmap::new(self.api, bw, bh) else {
            self.send(Event::RenderFailed(key));
            return;
        };
        let at = Placement { start_x: origin.0, start_y: origin.1, size_x: width as i32, size_y: height as i32, rotate: i32::from(key.turns % 4) };
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
                Kind::Full | Kind::Tile { .. } => Pixels::with_levels(bw, bh, bgrx),
                Kind::Preview => Pixels::flat(bw, bh, bgrx),
            };
            if key.kind == Kind::Full && width > PREVIEW_WIDTH {
                // The preview comes free with the full render.
                let preview = pixels.level_at_most(PREVIEW_WIDTH);
                let pk = RenderKey { width: PREVIEW_WIDTH, kind: Kind::Preview, ..key };
                self.shared.queue.lock().unwrap().done.insert(pk);
                self.send(Event::Rendered { key: pk, pixels: preview, took_ms: 0.0 });
            }
            match key.kind {
                Kind::Tile { x, y, .. } => log::debug!("page {} tile {bw}x{bh} at {x},{y} of {width}x{height} rendered in {took:.1} ms", key.page + 1),
                _ => log::debug!("page {} rendered at {width}x{height} in {took:.1} ms", key.page + 1),
            }
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
        let Some(size) = self.view_doc(key.doc).and_then(|d| d.page_size(key.page as usize)) else {
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

/// A print job under way: its own copy of the document (its form fields
/// are flattened into its pages, which the one on screen must not see),
/// the printer's document, the sheets and how far it got.
struct Printing {
    id: DocId,
    doc: Document,
    job: crate::print::Job,
    sheets: Vec<usize>,
    next: usize,
    scale: crate::settings::PrintScale,
    auto_rotate: bool,
    as_image: bool,
    /// The edited copy saved for the job.
    _temp: Option<TempFile>,
}

/// A file removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Pages sent as pictures are rendered at most this fine, and with at most
/// this many pixels.
const AS_IMAGE_DPI: f32 = 300.0;
const AS_IMAGE_PIXELS: f32 = 64.0e6;

/// Page `index` of the job's copy on a sheet of its own: flattened (form
/// fields are drawn into bitmaps only, `FPDF_RenderPage` leaves them out),
/// placed, drawn as vectors or as a picture.
fn print_sheet(api: &'static pdfium::sys::Api, p: &Printing, index: usize) -> Result<(), String> {
    let mut page = p.doc.load_page(index).ok_or("the page could not be loaded")?;
    if page.flatten_for_print() {
        // A flattened page shows its new content once loaded again.
        drop(page);
        page = p.doc.load_page(index).ok_or("the page could not be loaded")?;
    }
    let sheet = *p.job.sheet();
    let placed = crate::print::place(page.size(), &sheet, p.scale, p.auto_rotate);
    let [x, y, w, h] = placed.rect;
    let rect = [x.round() as i32, y.round() as i32, w.round().max(1.0) as i32, h.round().max(1.0) as i32];
    let flags = FLAGS | pdfium::sys::FPDF_PRINTING;
    p.job.start_page()?;
    if p.as_image {
        let k = (AS_IMAGE_DPI / sheet.dpi[0].max(sheet.dpi[1])).min(1.0).min((AS_IMAGE_PIXELS / (w * h).max(1.0)).sqrt());
        let (bw, bh) = ((w * k).round().max(1.0) as u32, (h * k).round().max(1.0) as u32);
        // White to start with.
        let mut bitmap = Bitmap::new(api, bw, bh).ok_or("no memory for the page's picture")?;
        page.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: bw as i32, size_y: bh as i32, rotate: placed.rotate }, flags);
        p.job.draw_bgrx(bitmap.pixels(), bw, bh, rect)?;
    } else {
        page.render_dc(p.job.dc(), Placement { start_x: rect[0], start_y: rect[1], size_x: rect[2], size_y: rect[3], rotate: placed.rotate }, flags);
    }
    p.job.end_page()
}

enum Work {
    Request(Request),
    Render(RenderKey),
    SearchPage,
    PrintPage,
}

/// How many pages the numbering is read from, and how many characters at
/// each end of a page.
const NUMBERING_SAMPLES: usize = 24;
const NUMBERING_CHARS: usize = 200;

/// The shift between the file's page numbers and the ones printed on the
/// pages, from the numbers found at the start and the end of sampled
/// pages' text (`samples`: the page's index and that text): the shift
/// most pages agree on, when at least three and half of them do. 0 when
/// the printed numbers are the file's.
pub fn numbering_shift(samples: &[(usize, String)]) -> Option<i32> {
    let mut votes: HashMap<i32, usize> = HashMap::new();
    for (index, text) in samples {
        let head: String = text.chars().take(NUMBERING_CHARS).collect();
        let tail: String = text.chars().rev().take(NUMBERING_CHARS).collect::<Vec<_>>().into_iter().rev().collect();
        let mut seen = HashSet::new();
        for part in [head, tail] {
            for token in part.split(|c: char| !c.is_ascii_digit()) {
                if token.is_empty() || token.len() > 5 || (token.len() > 1 && token.starts_with('0')) {
                    continue;
                }
                let Ok(n) = token.parse::<i32>() else { continue };
                if seen.insert(n) {
                    *votes.entry(*index as i32 + 1 - n).or_default() += 1;
                }
            }
        }
    }
    let (shift, count) = votes.into_iter().max_by_key(|(s, c)| (*c, -s.abs()))?;
    (count >= 3 && count * 2 >= samples.len()).then_some(shift)
}

/// The pixel width of a page of `size` for the first render: the same sum
/// the UI does once it knows the view (`layout::render_width`).
pub fn first_width(size: [f32; 2], first: FirstRender) -> u32 {
    crate::layout::render_width(size, first.zoom, first.turns, first.viewport, first.pixels_per_point)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample printed on Microsoft Print to PDF into a file, as vectors
    /// and as pictures, as a print job prints it (`print_sheet`): the file
    /// read back has both pages portrait (page 2, landscape, turned) with
    /// ink on them. The text comes out as outlines, not text: PDFium draws
    /// it through GDI only when built with `PDFIUM_PRINT_TEXT_WITH_GDI`.
    #[test]
    #[ignore = "needs vendor/pdfium and the Microsoft Print to PDF printer"]
    fn the_sample_prints_to_pdf() {
        const PRINTER: &str = "Microsoft Print to PDF";
        if !crate::print::printers().0.iter().any(|n| n == PRINTER) {
            println!("no {PRINTER} here");
            return;
        }
        let api = pdfium::init().expect("pdfium.dll from tools/get_pdfium.py");
        let source = pdfium::tests::sample_file("metalpdf_print_source.pdf");
        for as_image in [false, true] {
            let out = std::env::temp_dir().join(format!("metalpdf_printed_{as_image}.pdf"));
            let _ = std::fs::remove_file(&out);
            let started = Instant::now();
            let doc = Document::open(api, &source, None).unwrap();
            let job = crate::print::Job::start(PRINTER, None, "MetalPDF test", Some(&out)).unwrap();
            println!("sheet {:?}", job.sheet());
            let printing = Printing { id: 1, doc, job, sheets: vec![0, 1], next: 0, scale: crate::settings::PrintScale::Shrink, auto_rotate: true, as_image, _temp: None };
            for &index in &printing.sheets {
                print_sheet(api, &printing, index).unwrap();
            }
            let Printing { job, .. } = printing;
            job.finish().unwrap();
            // The spooler writes the file after the document ends.
            let deadline = Instant::now() + std::time::Duration::from_secs(30);
            let printed = loop {
                if let Ok(doc) = Document::open(api, &out, None) {
                    break doc;
                }
                assert!(Instant::now() < deadline, "no file from the printer");
                std::thread::sleep(std::time::Duration::from_millis(200));
            };
            println!("as image {as_image}: {} bytes in {:?}", std::fs::metadata(&out).unwrap().len(), started.elapsed());
            assert_eq!(printed.page_count(), 2);
            for i in 0..2 {
                let [w, h] = printed.page_size(i).unwrap();
                assert!(h > w, "sheet {} is portrait", i + 1);
                let page = printed.load_page(i).unwrap();
                let mut bitmap = Bitmap::new(api, 300, 424).unwrap();
                page.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: 300, size_y: 424, rotate: 0 }, FLAGS);
                let ink = bitmap.pixels().chunks(4).filter(|p| p[0] < 128).count();
                assert!(ink > 50, "sheet {} has ink: {ink} dark pixels", i + 1);
            }
            drop(printed);
            std::fs::remove_file(&out).ok();
        }
    }

    #[test]
    fn the_printed_numbering_is_read_off_the_pages() {
        // A book whose page 1 is the file's 17th page: the number is in
        // the header or the footer, with a year and a figure number about.
        let samples: Vec<(usize, String)> = (16..40)
            .map(|i| (i, format!("Chapter 3 • Cells {}\nSome text about figure 3.{} from 2026.\nMore text\n{}", i + 1 - 16, i % 4, i + 1 - 16)))
            .collect();
        assert_eq!(numbering_shift(&samples), Some(16));
        // The file's own numbering.
        let own: Vec<(usize, String)> = (0..10).map(|i| (i, format!("page {}\n…", i + 1))).collect();
        assert_eq!(numbering_shift(&own), Some(0));
        // Nothing that agrees.
        let noise: Vec<(usize, String)> = (0..10).map(|i| (i, format!("figure {} of 2026", i * 7))).collect();
        assert_eq!(numbering_shift(&noise), None);
        assert_eq!(numbering_shift(&[]), None);
    }

    #[test]
    fn large_pages_are_tiled() {
        // A4 at fit width on a 4K screen: whole. At 800 % on a 150 % screen:
        // tiles. A long, narrow page over the GPU's side: tiles.
        let a4 = [595.0, 842.0];
        assert!(!too_large(2200, pixel_height(a4, 2200), 16384));
        let w = (595.0 * crate::layout::POINT_SCALE * 8.0 * 1.5) as u32;
        assert!(too_large(w, pixel_height(a4, w), 16384));
        assert_eq!(pixel_height([100.0, 1000.0], 1000), 10000);
        assert!(too_large(1000, 10000, 8192));
        assert!(!too_large(1000, 10000, 16384));
    }

    #[test]
    fn signed_or_encrypted_documents_take_no_edit() {
        assert!(refuse(&[]).is_ok());
        assert_eq!(refuse(&["signed"]).unwrap_err(), "the document is signed");
        assert_eq!(refuse(&["signed", "encrypted"]).unwrap_err(), "the document is signed and encrypted");
    }

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
