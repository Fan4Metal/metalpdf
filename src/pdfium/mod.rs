//! PDFium behind a safe face: the library, a document, its pages and what
//! the viewer asks of them (sizes, pixels, text, links, bookmarks). Every
//! type here holds raw PDFium handles and is neither `Send` nor `Sync`:
//! PDFium is not thread-safe, so all of this lives on the one engine
//! thread (`engine.rs`); the UI only ever sees the plain data these
//! functions return.

pub mod sys;

use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_int, c_uchar, c_ulong, c_void};
use std::fs::File;
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use sys::Api;

/// Why a document could not be opened.
#[derive(Clone, Debug, PartialEq)]
pub enum OpenError {
    /// The file could not be read (missing, no access).
    File(String),
    /// Not a PDF, or too damaged to read.
    Format,
    /// Encrypted: a password is needed, or the one given is wrong.
    Password,
    /// An encryption PDFium does not support.
    Security,
    /// Larger than `FPDF_FILEACCESS` can address (4 GiB).
    TooLarge,
    Other(c_ulong),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::File(e) => write!(f, "cannot read the file: {e}"),
            OpenError::Format => f.write_str("not a PDF file, or too damaged to read"),
            OpenError::Password => f.write_str("the document is protected by a password"),
            OpenError::Security => f.write_str("the document's encryption is not supported"),
            OpenError::TooLarge => f.write_str("the file is larger than 4 GiB"),
            OpenError::Other(code) => write!(f, "PDFium error {code}"),
        }
    }
}

/// The library, loaded and initialised once per process. Never destroyed:
/// `FPDF_DestroyLibrary` at exit would only cost time.
pub fn init() -> Result<&'static Api, &'static sys::LoadError> {
    static INIT: OnceLock<Result<&'static Api, &'static sys::LoadError>> = OnceLock::new();
    *INIT.get_or_init(|| {
        let api = sys::api()?;
        let config = sys::FPDF_LIBRARY_CONFIG {
            version: 2,
            m_pUserFontPaths: std::ptr::null(),
            m_pIsolate: std::ptr::null_mut(),
            m_v8EmbedderSlot: 0,
            m_pPlatform: std::ptr::null_mut(),
            m_RendererType: 0,
            m_FontLibraryType: 0,
            m_BrotliEnabled: 0,
            m_IsolatePerDocument: 0,
        };
        unsafe { (api.FPDF_InitLibraryWithConfig)(&config) };
        Ok(api)
    })
}

/// The file a document is read from, as PDFium asks for it: positional
/// reads, so that a large scan opens at once and the file is not locked
/// against other programs (it is opened with every sharing flag; a block
/// that cannot be read any more fails the operation that needed it, no
/// more).
struct Reader {
    file: File,
}

unsafe extern "C" fn get_block(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    use std::os::windows::fs::FileExt;
    // SAFETY: `param` is the `Reader` the document owns, alive as long as the
    // document; `buf` has `size` bytes, as PDFium promises.
    let reader = unsafe { &*(param as *const Reader) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
    let mut done = 0;
    while done < out.len() {
        match reader.file.seek_read(&mut out[done..], u64::from(position) + done as u64) {
            Ok(0) | Err(_) => return 0,
            Ok(n) => done += n,
        }
    }
    1
}

/// Rendering geometry as PDFium takes it: where the page's top left corner
/// goes and how large the whole page is, in the bitmap's pixels (both may
/// exceed the bitmap: that is a tile), and the viewer's quarter turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub start_x: i32,
    pub start_y: i32,
    pub size_x: i32,
    pub size_y: i32,
    /// 0..=3 quarter turns clockwise, on top of the page's own /Rotate.
    pub rotate: i32,
}

/// A page's appearance in flat colours, for the dark page mode.
pub type ColorScheme = sys::FPDF_COLORSCHEME;

struct DocumentInner {
    api: &'static Api,
    handle: sys::FPDF_DOCUMENT,
    form: sys::FPDF_FORMHANDLE,
    /// Kept for the callbacks' sake; PDFium keeps pointers into them.
    _form_info: Box<sys::FPDF_FORMFILLINFO>,
    _access: Box<sys::FPDF_FILEACCESS>,
    _reader: Box<Reader>,
}

impl Drop for DocumentInner {
    fn drop(&mut self) {
        unsafe {
            if !self.form.is_null() {
                (self.api.FPDFDOC_ExitFormFillEnvironment)(self.form);
            }
            (self.api.FPDF_CloseDocument)(self.handle);
        }
    }
}

/// An open document. Pages taken from it keep it alive.
pub struct Document {
    inner: Rc<DocumentInner>,
}

impl Document {
    /// Open the PDF at `path`; `password` for an encrypted one.
    pub fn open(api: &'static Api, path: &Path, password: Option<&str>) -> Result<Document, OpenError> {
        use std::os::windows::fs::OpenOptionsExt;
        const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4; // FILE_SHARE_READ | WRITE | DELETE
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(SHARE_ALL)
            .open(path)
            .map_err(|e| OpenError::File(e.to_string()))?;
        let len = file.metadata().map_err(|e| OpenError::File(e.to_string()))?.len();
        let len = c_ulong::try_from(len).map_err(|_| OpenError::TooLarge)?;
        let reader = Box::new(Reader { file });
        let mut access = Box::new(sys::FPDF_FILEACCESS {
            m_FileLen: len,
            m_GetBlock: Some(get_block),
            m_Param: &*reader as *const Reader as *mut c_void,
        });
        let password = password.map(|p| CString::new(p).unwrap_or_default());
        let handle = unsafe {
            (api.FPDF_LoadCustomDocument)(
                &mut *access,
                password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        };
        if handle.is_null() {
            return Err(match unsafe { (api.FPDF_GetLastError)() } {
                sys::FPDF_ERR_FILE => OpenError::File("PDFium could not read it".into()),
                sys::FPDF_ERR_FORMAT => OpenError::Format,
                sys::FPDF_ERR_PASSWORD => OpenError::Password,
                sys::FPDF_ERR_SECURITY => OpenError::Security,
                code => OpenError::Other(code),
            });
        }
        // Form fields are drawn only through a form-fill environment
        // (`FPDF_RenderPageBitmap` leaves widgets out whatever the flags).
        let mut form_info = Box::new(sys::FPDF_FORMFILLINFO { version: 1, ..Default::default() });
        let form = unsafe { (api.FPDFDOC_InitFormFillEnvironment)(handle, &mut *form_info) };
        if !form.is_null() {
            unsafe { (api.FPDF_SetFormFieldHighlightAlpha)(form, 0) };
        }
        Ok(Document {
            inner: Rc::new(DocumentInner { api, handle, form, _form_info: form_info, _access: access, _reader: reader }),
        })
    }

    fn api(&self) -> &'static Api {
        self.inner.api
    }

    pub fn page_count(&self) -> usize {
        unsafe { (self.api().FPDF_GetPageCount)(self.inner.handle) }.max(0) as usize
    }

    /// The size of page `index` in points, with the page's /Rotate applied,
    /// without loading its content.
    pub fn page_size(&self, index: usize) -> Option<[f32; 2]> {
        let mut size = sys::FS_SIZEF::default();
        let ok = unsafe { (self.api().FPDF_GetPageSizeByIndexF)(self.inner.handle, index as c_int, &mut size) };
        (ok != 0 && size.width > 0.0 && size.height > 0.0).then_some([size.width, size.height])
    }

    pub fn load_page(&self, index: usize) -> Option<Page> {
        let handle = unsafe { (self.api().FPDF_LoadPage)(self.inner.handle, index as c_int) };
        if handle.is_null() {
            return None;
        }
        if !self.inner.form.is_null() {
            unsafe { (self.api().FORM_OnAfterLoadPage)(handle, self.inner.form) };
        }
        Some(Page { doc: Rc::clone(&self.inner), handle, index, text: RefCell::new(None), rendering: Cell::new(false) })
    }

    /// A document information field: `Title`, `Author`, `Subject`,
    /// `Keywords`, `Creator`, `Producer`, `CreationDate`, `ModDate`.
    pub fn meta_text(&self, tag: &str) -> Option<String> {
        let tag = CString::new(tag).ok()?;
        let text = utf16_text(|buf, len| unsafe { (self.api().FPDF_GetMetaText)(self.inner.handle, tag.as_ptr(), buf, len) });
        text.filter(|t| !t.is_empty())
    }

    /// The label of page `index` ("iv", "A-3"), if the document names its
    /// pages.
    pub fn page_label(&self, index: usize) -> Option<String> {
        let text = utf16_text(|buf, len| unsafe { (self.api().FPDF_GetPageLabel)(self.inner.handle, index as c_int, buf, len) });
        text.filter(|t| !t.is_empty())
    }

    /// The bookmarks, as a tree.
    pub fn outline(&self) -> Vec<OutlineItem> {
        let mut budget = 100_000usize;
        self.outline_children(std::ptr::null_mut(), 0, &mut budget)
    }

    fn outline_children(&self, parent: sys::FPDF_BOOKMARK, depth: usize, budget: &mut usize) -> Vec<OutlineItem> {
        let api = self.api();
        let doc = self.inner.handle;
        let mut items = Vec::new();
        if depth > 32 {
            return items;
        }
        let mut bookmark = unsafe { (api.FPDFBookmark_GetFirstChild)(doc, parent) };
        while !bookmark.is_null() && *budget > 0 {
            *budget -= 1;
            let title = utf16_text(|buf, len| unsafe { (api.FPDFBookmark_GetTitle)(bookmark, buf, len) }).unwrap_or_default();
            let mut dest = unsafe { (api.FPDFBookmark_GetDest)(doc, bookmark) };
            let mut uri = None;
            if dest.is_null() {
                let action = unsafe { (api.FPDFBookmark_GetAction)(bookmark) };
                if !action.is_null() {
                    match unsafe { (api.FPDFAction_GetType)(action) } {
                        sys::PDFACTION_GOTO => dest = unsafe { (api.FPDFAction_GetDest)(doc, action) },
                        sys::PDFACTION_URI => uri = self.action_uri(action),
                        _ => {}
                    }
                }
            }
            let target = if !dest.is_null() {
                self.dest_target(dest)
            } else {
                uri.map(Target::Uri)
            };
            let children = self.outline_children(bookmark, depth + 1, budget);
            items.push(OutlineItem { title, target, children });
            bookmark = unsafe { (api.FPDFBookmark_GetNextSibling)(doc, bookmark) };
        }
        items
    }

    fn action_uri(&self, action: sys::FPDF_ACTION) -> Option<String> {
        let api = self.api();
        let len = unsafe { (api.FPDFAction_GetURIPath)(self.inner.handle, action, std::ptr::null_mut(), 0) };
        if len <= 1 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        unsafe { (api.FPDFAction_GetURIPath)(self.inner.handle, action, buf.as_mut_ptr().cast(), len) };
        buf.truncate(len as usize - 1);
        String::from_utf8(buf).ok().filter(|s| !s.is_empty())
    }

    /// Where a destination points: a page, and the point on it when given.
    fn dest_target(&self, dest: sys::FPDF_DEST) -> Option<Target> {
        let api = self.api();
        let page = unsafe { (api.FPDFDest_GetDestPageIndex)(self.inner.handle, dest) };
        if page < 0 {
            return None;
        }
        let (mut has_x, mut has_y, mut has_zoom) = (0, 0, 0);
        let (mut x, mut y, mut zoom) = (0f32, 0f32, 0f32);
        let ok = unsafe {
            (api.FPDFDest_GetLocationInPage)(dest, &mut has_x, &mut has_y, &mut has_zoom, &mut x, &mut y, &mut zoom)
        };
        let point = (ok != 0).then(|| [(has_x != 0).then_some(x), (has_y != 0).then_some(y)]);
        Some(Target::Page { index: page as usize, x: point.and_then(|p| p[0]), y: point.and_then(|p| p[1]) })
    }

    /// Whether the document is signed, tagged or encrypted: the text
    /// editing refuses those.
    pub fn edit_obstacles(&self) -> Vec<&'static str> {
        let api = self.api();
        let mut out = Vec::new();
        unsafe {
            if (api.FPDF_GetSignatureCount)(self.inner.handle) > 0 {
                out.push("signed");
            }
            if (api.FPDFCatalog_IsTagged)(self.inner.handle) != 0 {
                out.push("tagged");
            }
            if (api.FPDF_GetSecurityHandlerRevision)(self.inner.handle) != -1 {
                out.push("encrypted");
            }
        }
        out
    }
}

/// Where a link or a bookmark leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A page, and a point on it in page space (points, origin bottom left)
    /// when the destination names one.
    Page { index: usize, x: Option<f32>, y: Option<f32> },
    Uri(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    pub target: Option<Target>,
    pub children: Vec<OutlineItem>,
}

/// A link annotation on a page: its rectangle in page space and its target.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// left, bottom, right, top in points, page space.
    pub rect: [f32; 4],
    pub target: Target,
}

/// One character of a page's text: its code (a UTF-16 unit as PDFium gives
/// it; surrogates come in pairs), its loose box in page space (left,
/// bottom, right, top) and its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Char {
    pub code: u32,
    pub rect: [f32; 4],
    pub origin: [f32; 2],
    /// Inserted by PDFium (a line break it worked out), not in the file.
    pub generated: bool,
}

/// The text of a page as laid out: every character with its box. The
/// selection and the search work on this, on the UI thread, without PDFium.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    pub chars: Vec<Char>,
}

/// Where the progressive renderer stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    /// Paused: `Page::render_continue` goes on.
    Paused,
    Done,
    Failed,
}

/// The pause callback's state: `should_pause` is asked by PDFium every
/// hundred objects.
pub struct Pause {
    raw: Box<sys::IFSDK_PAUSE>,
    _should: Box<Box<dyn FnMut() -> bool>>,
}

impl Pause {
    pub fn new(should_pause: impl FnMut() -> bool + 'static) -> Pause {
        let mut should: Box<Box<dyn FnMut() -> bool>> = Box::new(Box::new(should_pause));
        let raw = Box::new(sys::IFSDK_PAUSE {
            version: 1,
            NeedToPauseNow: Some(need_to_pause),
            user: &mut *should as *mut Box<dyn FnMut() -> bool> as *mut c_void,
        });
        Pause { raw, _should: should }
    }
}

unsafe extern "C" fn need_to_pause(this: *mut sys::IFSDK_PAUSE) -> sys::FPDF_BOOL {
    // SAFETY: `user` is the boxed closure `Pause::new` stored, alive with
    // the `Pause`, which outlives the render it is passed to.
    let should = unsafe { &mut *((*this).user as *mut Box<dyn FnMut() -> bool>) };
    c_int::from(should())
}

/// A page of a document, with its text page once it was asked for.
pub struct Page {
    doc: Rc<DocumentInner>,
    handle: sys::FPDF_PAGE,
    pub index: usize,
    text: RefCell<Option<sys::FPDF_TEXTPAGE>>,
    /// A progressive render is under way: `render_close` is owed.
    rendering: Cell<bool>,
}

impl Drop for Page {
    fn drop(&mut self) {
        let api = self.doc.api;
        unsafe {
            if self.rendering.get() {
                (api.FPDF_RenderPage_Close)(self.handle);
            }
            if !self.doc.form.is_null() {
                (api.FORM_OnBeforeClosePage)(self.handle, self.doc.form);
            }
            if let Some(text) = self.text.take() {
                (api.FPDFText_ClosePage)(text);
            }
            (api.FPDF_ClosePage)(self.handle);
        }
    }
}

impl Page {
    fn api(&self) -> &'static Api {
        self.doc.api
    }

    /// Width and height in points, /Rotate applied.
    pub fn size(&self) -> [f32; 2] {
        unsafe { [(self.api().FPDF_GetPageWidthF)(self.handle), (self.api().FPDF_GetPageHeightF)(self.handle)] }
    }

    /// Render the whole page (or a tile of it) into `bitmap` in one go.
    pub fn render(&self, bitmap: &mut Bitmap, at: Placement, flags: i32) {
        unsafe {
            (self.api().FPDF_RenderPageBitmap)(
                bitmap.handle,
                self.handle,
                at.start_x,
                at.start_y,
                at.size_x,
                at.size_y,
                at.rotate,
                flags,
            );
        }
        self.draw_forms(bitmap, at, flags);
    }

    /// Start a render that `pause` may interrupt; `scheme` paints
    /// everything in its colours. `render_continue` goes on after a pause,
    /// `render_close` ends it, finished or not.
    pub fn render_start(
        &self,
        bitmap: &mut Bitmap,
        at: Placement,
        flags: i32,
        scheme: Option<&ColorScheme>,
        pause: &mut Pause,
    ) -> RenderStatus {
        if self.rendering.replace(true) {
            // A second start on a page still rendering fails in PDFium.
            unsafe { (self.api().FPDF_RenderPage_Close)(self.handle) };
        }
        let status = unsafe {
            (self.api().FPDF_RenderPageBitmapWithColorScheme_Start)(
                bitmap.handle,
                self.handle,
                at.start_x,
                at.start_y,
                at.size_x,
                at.size_y,
                at.rotate,
                flags,
                scheme.map_or(std::ptr::null(), |s| s as *const ColorScheme),
                &mut *pause.raw,
            )
        };
        self.finished(status, bitmap, at, flags)
    }

    pub fn render_continue(&self, bitmap: &mut Bitmap, at: Placement, flags: i32, pause: &mut Pause) -> RenderStatus {
        if !self.rendering.get() {
            return RenderStatus::Failed;
        }
        let status = unsafe { (self.api().FPDF_RenderPage_Continue)(self.handle, &mut *pause.raw) };
        self.finished(status, bitmap, at, flags)
    }

    /// Close a finished or abandoned progressive render.
    pub fn render_close(&self) {
        if self.rendering.replace(false) {
            unsafe { (self.api().FPDF_RenderPage_Close)(self.handle) };
        }
    }

    fn finished(&self, status: c_int, bitmap: &mut Bitmap, at: Placement, flags: i32) -> RenderStatus {
        match status {
            sys::FPDF_RENDER_TOBECONTINUED => RenderStatus::Paused,
            sys::FPDF_RENDER_DONE => {
                // The form fields over the finished page, before the
                // render is closed.
                self.draw_forms(bitmap, at, flags);
                self.render_close();
                RenderStatus::Done
            }
            _ => {
                self.render_close();
                RenderStatus::Failed
            }
        }
    }

    /// Draw the form fields with the same geometry as the page.
    fn draw_forms(&self, bitmap: &mut Bitmap, at: Placement, flags: i32) {
        if self.doc.form.is_null() {
            return;
        }
        unsafe {
            (self.api().FPDF_FFLDraw)(
                self.doc.form,
                bitmap.handle,
                self.handle,
                at.start_x,
                at.start_y,
                at.size_x,
                at.size_y,
                at.rotate,
                flags,
            );
        }
    }

    fn text_page(&self) -> Option<sys::FPDF_TEXTPAGE> {
        let mut text = self.text.borrow_mut();
        if text.is_none() {
            let handle = unsafe { (self.api().FPDFText_LoadPage)(self.handle) };
            if handle.is_null() {
                return None;
            }
            *text = Some(handle);
        }
        *text
    }

    /// How many characters the page's text has.
    pub fn char_count(&self) -> usize {
        self.text_page().map_or(0, |t| unsafe { (self.api().FPDFText_CountChars)(t) }.max(0) as usize)
    }

    /// Every character with its box (page space, points).
    pub fn text_layout(&self) -> TextLayout {
        let api = self.api();
        let Some(text) = self.text_page() else {
            return TextLayout::default();
        };
        let count = unsafe { (api.FPDFText_CountChars)(text) }.max(0);
        let mut chars = Vec::with_capacity(count as usize);
        for i in 0..count {
            let code = unsafe { (api.FPDFText_GetUnicode)(text, i) };
            let mut rect = sys::FS_RECTF::default();
            let boxed = unsafe { (api.FPDFText_GetLooseCharBox)(text, i, &mut rect) } != 0;
            let (mut ox, mut oy) = (0f64, 0f64);
            unsafe { (api.FPDFText_GetCharOrigin)(text, i, &mut ox, &mut oy) };
            let generated = unsafe { (api.FPDFText_IsGenerated)(text, i) } == 1;
            chars.push(Char {
                code,
                rect: if boxed { [rect.left, rect.bottom, rect.right, rect.top] } else { [0.0; 4] },
                origin: [ox as f32, oy as f32],
                generated,
            });
        }
        TextLayout { chars }
    }

    /// The text of characters `start..start + count`.
    pub fn text(&self, start: usize, count: usize) -> String {
        let api = self.api();
        let Some(text) = self.text_page() else {
            return String::new();
        };
        if count == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; count + 1];
        let written = unsafe { (api.FPDFText_GetText)(text, start as c_int, count as c_int, buf.as_mut_ptr()) };
        let written = (written.max(0) as usize).saturating_sub(1).min(count);
        String::from_utf16_lossy(&buf[..written])
    }

    /// The character ranges of `query` on the page: `(first index, count)`.
    pub fn find(&self, query: &str, match_case: bool, whole_words: bool) -> Vec<(usize, usize)> {
        let api = self.api();
        let Some(text) = self.text_page() else {
            return Vec::new();
        };
        let wide: Vec<u16> = query.encode_utf16().chain(std::iter::once(0)).collect();
        let flags = if match_case { sys::FPDF_MATCHCASE } else { 0 } | if whole_words { sys::FPDF_MATCHWHOLEWORD } else { 0 };
        let search = unsafe { (api.FPDFText_FindStart)(text, wide.as_ptr(), flags, 0) };
        if search.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        unsafe {
            while (api.FPDFText_FindNext)(search) != 0 {
                let index = (api.FPDFText_GetSchResultIndex)(search);
                let count = (api.FPDFText_GetSchCount)(search);
                if index >= 0 && count > 0 {
                    out.push((index as usize, count as usize));
                }
                if out.len() > 100_000 {
                    break;
                }
            }
            (api.FPDFText_FindClose)(search);
        }
        out
    }

    /// The rectangles (left, bottom, right, top, page space) covering
    /// characters `start..start + count`, one per line.
    pub fn text_rects(&self, start: usize, count: usize) -> Vec<[f32; 4]> {
        let api = self.api();
        let Some(text) = self.text_page() else {
            return Vec::new();
        };
        let n = unsafe { (api.FPDFText_CountRects)(text, start as c_int, count as c_int) }.max(0);
        (0..n)
            .filter_map(|i| {
                let (mut l, mut t, mut r, mut b) = (0f64, 0f64, 0f64, 0f64);
                let ok = unsafe { (api.FPDFText_GetRect)(text, i, &mut l, &mut t, &mut r, &mut b) };
                (ok != 0).then_some([l as f32, b as f32, r as f32, t as f32])
            })
            .collect()
    }

    /// The link annotations of the page with a target the viewer can
    /// follow.
    pub fn links(&self) -> Vec<Link> {
        let api = self.api();
        let doc = Document { inner: Rc::clone(&self.doc) };
        let mut out = Vec::new();
        let mut pos = 0;
        let mut link: sys::FPDF_LINK = std::ptr::null_mut();
        while unsafe { (api.FPDFLink_Enumerate)(self.handle, &mut pos, &mut link) } != 0 && out.len() < 10_000 {
            let mut rect = sys::FS_RECTF::default();
            if unsafe { (api.FPDFLink_GetAnnotRect)(link, &mut rect) } == 0 {
                continue;
            }
            let mut dest = unsafe { (api.FPDFLink_GetDest)(self.doc.handle, link) };
            let mut target = None;
            if dest.is_null() {
                let action = unsafe { (api.FPDFLink_GetAction)(link) };
                if !action.is_null() {
                    match unsafe { (api.FPDFAction_GetType)(action) } {
                        sys::PDFACTION_GOTO => dest = unsafe { (api.FPDFAction_GetDest)(self.doc.handle, action) },
                        sys::PDFACTION_URI => target = doc.action_uri(action).map(Target::Uri),
                        _ => {}
                    }
                }
            }
            if !dest.is_null() {
                target = doc.dest_target(dest);
            }
            if let Some(target) = target {
                let (l, r) = (rect.left.min(rect.right), rect.left.max(rect.right));
                let (b, t) = (rect.bottom.min(rect.top), rect.bottom.max(rect.top));
                out.push(Link { rect: [l, b, r, t], target });
            }
        }
        out
    }

    /// The transform from page space (points, origin bottom left, before
    /// the page's /Rotate) to the unrotated page as shown, in points from
    /// its top left corner: `[a, b, c, d, e, f]` with
    /// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`. It carries the /Rotate
    /// and the CropBox offset, so text boxes and links land on the pixels.
    pub fn page_to_view(&self) -> [f32; 6] {
        let api = self.api();
        let [w, h] = self.size();
        // PDFium maps to integer device pixels: a fine grid keeps the
        // rounding below a hundredth of a point.
        const SCALE: f64 = 100.0;
        let (sx, sy) = ((w as f64 * SCALE).round() as c_int, (h as f64 * SCALE).round() as c_int);
        let map = |x: f64, y: f64| {
            let (mut dx, mut dy) = (0 as c_int, 0 as c_int);
            unsafe { (api.FPDF_PageToDevice)(self.handle, 0, 0, sx, sy, 0, x, y, &mut dx, &mut dy) };
            (f64::from(dx) / SCALE, f64::from(dy) / SCALE)
        };
        // Points far apart, so that the integer rounding is small against
        // the differences.
        let (ox, oy) = map(0.0, 0.0);
        let (xx, xy) = map(1000.0, 0.0);
        let (yx, yy) = map(0.0, 1000.0);
        [
            ((xx - ox) / 1000.0) as f32,
            ((xy - oy) / 1000.0) as f32,
            ((yx - ox) / 1000.0) as f32,
            ((yy - oy) / 1000.0) as f32,
            ox as f32,
            oy as f32,
        ]
    }
}

/// A BGRx bitmap PDFium paints into, over pixels this side owns. The pixel
/// buffer must not move while PDFium holds the bitmap, hence the `Box`.
pub struct Bitmap {
    api: &'static Api,
    handle: sys::FPDF_BITMAP,
    pixels: Box<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl Bitmap {
    /// A white bitmap of `width` × `height`; `None` when the size is empty
    /// or too large for PDFium (`int` strides).
    pub fn new(api: &'static Api, width: u32, height: u32) -> Option<Bitmap> {
        let stride = width.checked_mul(4)?;
        let bytes = usize::try_from(stride).ok()?.checked_mul(height as usize)?;
        if width == 0 || height == 0 || i32::try_from(bytes).is_err() {
            return None;
        }
        let mut pixels = vec![0xffu8; bytes].into_boxed_slice();
        let handle = unsafe {
            (api.FPDFBitmap_CreateEx)(
                width as c_int,
                height as c_int,
                sys::FPDFBitmap_BGRx,
                pixels.as_mut_ptr().cast(),
                stride as c_int,
            )
        };
        if handle.is_null() {
            return None;
        }
        Some(Bitmap { api, handle, pixels, width, height })
    }

    /// Fill the whole bitmap with `argb` (0xFFFFFFFF: white).
    pub fn fill(&mut self, argb: u32) {
        unsafe {
            (self.api.FPDFBitmap_FillRect)(self.handle, 0, 0, self.width as c_int, self.height as c_int, argb);
        }
    }

    /// The pixels, BGRx row by row, `width * 4` bytes a row.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Take the pixels; the PDFium bitmap is destroyed.
    pub fn into_pixels(mut self) -> Vec<u8> {
        unsafe { (self.api.FPDFBitmap_Destroy)(self.handle) };
        self.handle = std::ptr::null_mut();
        std::mem::take(&mut self.pixels).into_vec()
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.api.FPDFBitmap_Destroy)(self.handle) };
        }
    }
}

/// A UTF-16LE string PDFium writes into a buffer the caller sizes: asked
/// for its length first (bytes, the terminator included), then read.
fn utf16_text(get: impl Fn(*mut c_void, c_ulong) -> c_ulong) -> Option<String> {
    let bytes = get(std::ptr::null_mut(), 0);
    if bytes < 2 {
        return None;
    }
    let mut buf = vec![0u16; bytes.div_ceil(2) as usize];
    let written = get(buf.as_mut_ptr().cast(), bytes);
    let units = (written.min(bytes) as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..units]))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A two-page PDF written by hand: Helvetica text on an A4 page, and a
    /// landscape page by /Rotate 90 with a link to the first one and a
    /// bookmark each.
    pub fn sample_pdf() -> Vec<u8> {
        let objects: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
            stream("BT /F1 24 Tf 72 770 Td (Hello MetalPDF) Tj ET BT /F1 12 Tf 72 740 Td (Second line of text) Tj ET"),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Rotate 90 /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> /Annots [11 0 R] >>".into(),
            stream("BT /F1 18 Tf 100 700 Td (Page two) Tj ET"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            "<< /Type /Outlines /First 9 0 R /Last 10 0 R /Count 2 >>".into(),
            "<< /Title (First) /Parent 8 0 R /Next 10 0 R /Dest [3 0 R /XYZ 0 842 0] >>".into(),
            "<< /Title (Second) /Parent 8 0 R /Prev 9 0 R /Dest [5 0 R /Fit] >>".into(),
            "<< /Type /Annot /Subtype /Link /Rect [100 690 200 720] /Border [0 0 0] /Dest [3 0 R /XYZ 0 700 0] >>".into(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
        );
        out
    }

    fn stream(content: &str) -> String {
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
    }

    /// The sample written to a temporary file, for the tests that need
    /// the DLL.
    pub fn sample_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("metalpdf_{name}_{}.pdf", std::process::id()));
        std::fs::write(&path, sample_pdf()).unwrap();
        path
    }

    fn api() -> &'static Api {
        // The tests run from target\debug\deps: the vendored DLL is a
        // candidate in debug builds.
        init().expect("pdfium.dll from tools/get_pdfium.py")
    }

    #[test]
    fn utf16_helper_handles_terminators() {
        let text: Vec<u16> = "Привет".encode_utf16().chain([0]).collect();
        let bytes = (text.len() * 2) as c_ulong;
        let got = utf16_text(|buf, len| {
            if !buf.is_null() && len >= bytes {
                unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), buf.cast(), text.len()) };
            }
            bytes
        });
        assert_eq!(got.as_deref(), Some("Привет"));
        assert_eq!(utf16_text(|_, _| 0), None);
        assert_eq!(utf16_text(|_, _| 2).as_deref(), Some(""));
    }

    #[test]
    fn the_sample_is_a_pdf() {
        let pdf = sample_pdf();
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert!(pdf.ends_with(b"%%EOF\n"));
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn opens_renders_and_reads_the_sample() {
        let api = api();
        let path = sample_file("basic");
        let doc = Document::open(api, &path, None).unwrap();
        assert_eq!(doc.page_count(), 2);
        assert_eq!(doc.page_size(0), Some([595.0, 842.0]));
        // /Rotate 90 swaps the sides.
        assert_eq!(doc.page_size(1), Some([842.0, 595.0]));
        assert_eq!(doc.page_size(2), None);

        let page = doc.load_page(0).unwrap();
        assert_eq!(page.size(), [595.0, 842.0]);
        let mut bitmap = Bitmap::new(api, 595, 842).unwrap();
        bitmap.fill(0xffff_ffff);
        page.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: 595, size_y: 842, rotate: 0 }, sys::FPDF_ANNOT);
        let dark = bitmap.pixels().chunks(4).filter(|p| p[0] < 128).count();
        assert!(dark > 500, "the text left {dark} dark pixels");
        // The dark pixels lie where the text is: the first line's box.
        let row = |y: usize| &bitmap.pixels()[y * 595 * 4..(y + 1) * 595 * 4];
        assert!(row(842 - 780).chunks(4).any(|p| p[0] < 128), "the first line is at 770..794 pt");
        assert!(row(842 - 600).chunks(4).all(|p| p[0] == 255), "nothing below the second line");
        // The text is where the content stream put it.
        let layout = page.text_layout();
        let text: String = char::decode_utf16(layout.chars.iter().map(|c| c.code as u16))
            .map(|c| c.unwrap_or('?'))
            .collect();
        assert!(text.starts_with("Hello MetalPDF"), "{text:?}");
        let h = &layout.chars[0];
        assert!(h.rect[0] >= 70.0 && h.rect[0] <= 74.0, "{:?}", h.rect);
        assert!(h.rect[3] > 770.0 && h.rect[1] < 770.0, "{:?}", h.rect);
        assert_eq!(page.text(0, 5), "Hello");
        assert_eq!(page.find("metalpdf", false, false), vec![(6, 8)]);
        assert_eq!(page.find("metalpdf", true, false), vec![]);
        assert_eq!(page.text_rects(6, 8).len(), 1);
        // An unrotated page: identity but for the flipped y.
        let m = page.page_to_view();
        assert!((m[0] - 1.0).abs() < 0.01 && (m[3] + 1.0).abs() < 0.01 && (m[5] - 842.0).abs() < 0.1, "{m:?}");

        let page2 = doc.load_page(1).unwrap();
        let links = page2.links();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rect, [100.0, 690.0, 200.0, 720.0]);
        assert_eq!(links[0].target, Target::Page { index: 0, x: Some(0.0), y: Some(700.0) });
        // Rotated 90° clockwise: page x runs down the view, page y runs
        // along it from the right.
        let m = page2.page_to_view();
        assert!(m[0].abs() < 0.01 && (m[1] - 1.0).abs() < 0.01, "{m:?}");
        assert!((m[2] - 1.0).abs() < 0.01 && m[3].abs() < 0.01, "{m:?}");

        let outline = doc.outline();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "First");
        assert_eq!(outline[0].target, Some(Target::Page { index: 0, x: Some(0.0), y: Some(842.0) }));
        assert_eq!(outline[1].target, Some(Target::Page { index: 1, x: None, y: None }));
        assert_eq!(doc.meta_text("Title"), None);
        assert_eq!(doc.page_label(0), None);
        assert!(doc.edit_obstacles().is_empty());
        drop(page);
        drop(page2);
        drop(doc);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn progressive_render_matches_the_plain_one() {
        let api = api();
        let path = sample_file("progressive");
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let at = Placement { start_x: 0, start_y: 0, size_x: 300, size_y: 424, rotate: 0 };
        let mut plain = Bitmap::new(api, 300, 424).unwrap();
        page.render(&mut plain, at, sys::FPDF_ANNOT);
        // Paused at every chance, then continued to the end.
        let mut step = Bitmap::new(api, 300, 424).unwrap();
        let mut pause = Pause::new(|| true);
        let mut status = page.render_start(&mut step, at, sys::FPDF_ANNOT, None, &mut pause);
        let mut rounds = 0;
        while status == RenderStatus::Paused {
            status = page.render_continue(&mut step, at, sys::FPDF_ANNOT, &mut pause);
            rounds += 1;
            assert!(rounds < 10_000);
        }
        assert_eq!(status, RenderStatus::Done);
        assert!(rounds > 0, "the pause was never taken");
        assert_same_picture(plain.pixels(), step.pixels(), "paused");
        // Abandoned, then started again on the same page.
        let mut again = Bitmap::new(api, 300, 424).unwrap();
        let _ = page.render_start(&mut again, at, sys::FPDF_ANNOT, None, &mut pause);
        page.render_close();
        again.fill(0xffff_ffff);
        let mut never = Pause::new(|| false);
        let status = page.render_start(&mut again, at, sys::FPDF_ANNOT, None, &mut never);
        assert_eq!(status, RenderStatus::Done);
        assert_same_picture(plain.pixels(), again.pixels(), "restarted");
        drop(page);
        drop(doc);
        std::fs::remove_file(path).unwrap();
    }

    /// Two renders of one page agree pixel for pixel, or nearly: the
    /// numbers tell how far apart they are.
    fn assert_same_picture(a: &[u8], b: &[u8], what: &str) {
        assert_eq!(a.len(), b.len());
        let differing = a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p[..3] != q[..3]).count();
        let max_delta = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        let dark = |px: &[u8]| px.chunks(4).filter(|p| p[0] < 128).count();
        assert!(
            differing == 0,
            "{what}: {differing} of {} pixels differ (largest channel delta {max_delta}; dark pixels {} against {})",
            a.len() / 4,
            dark(a),
            dark(b)
        );
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn a_wrong_file_is_refused_cleanly() {
        let api = api();
        let path = std::env::temp_dir().join(format!("metalpdf_notpdf_{}.pdf", std::process::id()));
        std::fs::write(&path, b"this is not a pdf").unwrap();
        assert_eq!(Document::open(api, &path, None).err(), Some(OpenError::Format));
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(Document::open(api, &path, None).err(), Some(OpenError::File(_))));
    }
}
