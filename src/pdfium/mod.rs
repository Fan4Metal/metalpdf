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
        Some(Page { doc: Rc::clone(&self.inner), handle: Cell::new(handle), index: index as c_int, text: RefCell::new(None), rendering: Cell::new(false) })
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

    /// Whether the document lets a printer scale its pages
    /// (`/ViewerPreferences /PrintScaling`, not `/None`).
    pub fn print_scaling(&self) -> bool {
        unsafe { (self.api().FPDF_VIEWERREF_GetPrintScaling)(self.inner.handle) != 0 }
    }

    /// Whether the document is signed or encrypted: the editing refuses
    /// those. A tagged one is edited: `FPDFPage_GenerateContent` writes
    /// the marked content back, and a stand-in run takes the old one's
    /// marks (`Page::copy_marks`), so the tags keep their content.
    pub fn edit_obstacles(&self) -> Vec<&'static str> {
        let api = self.api();
        let mut out = Vec::new();
        unsafe {
            if (api.FPDF_GetSignatureCount)(self.inner.handle) > 0 {
                out.push("signed");
            }
            if (api.FPDF_GetSecurityHandlerRevision)(self.inner.handle) != -1 {
                out.push("encrypted");
            }
        }
        out
    }
}

impl Document {
    /// Remove page `index`.
    pub fn delete_page(&self, index: usize) -> Result<(), String> {
        if index >= self.page_count() {
            return Err("no such page".into());
        }
        unsafe { (self.api().FPDFPage_Delete)(self.inner.handle, index as c_int) };
        Ok(())
    }

    /// A blank page of `size` (points) inserted at `index` (the count
    /// puts it at the end).
    pub fn insert_blank_page(&self, index: usize, size: [f32; 2]) -> Result<(), String> {
        let index = index.min(self.page_count());
        let page = unsafe { (self.api().FPDFPage_New)(self.inner.handle, index as c_int, f64::from(size[0].max(1.0)), f64::from(size[1].max(1.0))) };
        if page.is_null() {
            return Err("the page could not be made".into());
        }
        unsafe { (self.api().FPDF_ClosePage)(page) };
        Ok(())
    }

    /// Every page of `source` inserted at `index`; how many came.
    pub fn import_pages(&self, source: &Document, index: usize) -> Result<usize, String> {
        let count = source.page_count();
        if count == 0 {
            return Err("the document has no pages".into());
        }
        let index = index.min(self.page_count());
        let ok = unsafe { (self.api().FPDF_ImportPagesByIndex)(self.inner.handle, source.inner.handle, std::ptr::null(), 0, index as c_int) };
        if ok == 0 {
            return Err("the pages could not be imported".into());
        }
        Ok(count)
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
    handle: Cell<sys::FPDF_PAGE>,
    /// Its index in the document, to parse it again from its content.
    index: c_int,
    text: RefCell<Option<sys::FPDF_TEXTPAGE>>,
    /// A progressive render is under way: `render_close` is owed.
    rendering: Cell<bool>,
}

impl Drop for Page {
    fn drop(&mut self) {
        let api = self.doc.api;
        unsafe {
            if self.rendering.get() {
                (api.FPDF_RenderPage_Close)(self.handle.get());
            }
            if !self.doc.form.is_null() {
                (api.FORM_OnBeforeClosePage)(self.handle.get(), self.doc.form);
            }
            if let Some(text) = self.text.take() {
                (api.FPDFText_ClosePage)(text);
            }
            (api.FPDF_ClosePage)(self.handle.get());
        }
    }
}

impl Page {
    fn api(&self) -> &'static Api {
        self.doc.api
    }

    /// Width and height in points, /Rotate applied.
    pub fn size(&self) -> [f32; 2] {
        unsafe { [(self.api().FPDF_GetPageWidthF)(self.handle.get()), (self.api().FPDF_GetPageHeightF)(self.handle.get())] }
    }

    /// Render the whole page (or a tile of it) into `bitmap` in one go.
    pub fn render(&self, bitmap: &mut Bitmap, at: Placement, flags: i32) {
        unsafe {
            (self.api().FPDF_RenderPageBitmap)(
                bitmap.handle,
                self.handle.get(),
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

    /// Draw the page into a device context (a printer's) as vectors and
    /// text. Form fields are not drawn: a page printed so is flattened
    /// first (`flatten_for_print`).
    pub fn render_dc(&self, dc: *mut c_void, at: Placement, flags: i32) {
        unsafe { (self.api().FPDF_RenderPage)(dc, self.handle.get(), at.start_x, at.start_y, at.size_x, at.size_y, at.rotate, flags) };
    }

    /// The page's form fields and printable annotations made part of its
    /// content, as printed; true when something was, and the page is to be
    /// loaded again to show it. Only for a copy of the document opened to
    /// be printed.
    pub fn flatten_for_print(&self) -> bool {
        unsafe { (self.api().FPDFPage_Flatten)(self.handle.get(), sys::FLAT_PRINT) == sys::FLATTEN_SUCCESS }
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
            unsafe { (self.api().FPDF_RenderPage_Close)(self.handle.get()) };
        }
        let status = unsafe {
            (self.api().FPDF_RenderPageBitmapWithColorScheme_Start)(
                bitmap.handle,
                self.handle.get(),
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
        let status = unsafe { (self.api().FPDF_RenderPage_Continue)(self.handle.get(), &mut *pause.raw) };
        self.finished(status, bitmap, at, flags)
    }

    /// Close a finished or abandoned progressive render.
    pub fn render_close(&self) {
        if self.rendering.replace(false) {
            unsafe { (self.api().FPDF_RenderPage_Close)(self.handle.get()) };
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
                self.handle.get(),
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
            let handle = unsafe { (self.api().FPDFText_LoadPage)(self.handle.get()) };
            if handle.is_null() {
                return None;
            }
            *text = Some(handle);
        }
        *text
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

    /// How many characters the page's text has.
    pub fn char_count(&self) -> usize {
        let api = self.api();
        match self.text_page() {
            Some(text) => unsafe { (api.FPDFText_CountChars)(text) }.max(0) as usize,
            None => 0,
        }
    }

    /// The text of characters `start..start + count` (fewer at the
    /// page's end).
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

    /// The page's characters, one per text page index (a UTF-16 unit
    /// each: a character beyond the BMP takes two indices, as PDFium
    /// counts them).
    pub fn chars(&self) -> Vec<char> {
        let api = self.api();
        let Some(text) = self.text_page() else {
            return Vec::new();
        };
        let count = unsafe { (api.FPDFText_CountChars)(text) }.max(0);
        (0..count).map(|i| char::from_u32(unsafe { (api.FPDFText_GetUnicode)(text, i) }).unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    }

    /// The character ranges of `query` on the page: `(first index,
    /// count)`. The search is the program's own (`search::matches`):
    /// PDFium's `FPDFText_FindStart` folds the case and bounds words in
    /// ASCII alone.
    pub fn find(&self, query: &str, match_case: bool, whole_words: bool) -> Vec<(usize, usize)> {
        crate::search::matches(&self.chars(), query, match_case, whole_words)
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
        while unsafe { (api.FPDFLink_Enumerate)(self.handle.get(), &mut pos, &mut link) } != 0 && out.len() < 10_000 {
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
            unsafe { (api.FPDF_PageToDevice)(self.handle.get(), 0, 0, sx, sy, 0, x, y, &mut dx, &mut dy) };
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

/// A text object of a page (a run of text drawn with one font at one
/// size), as the editing sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct TextObject {
    /// Its index among the page's objects (`FPDFPage_GetObject`).
    pub index: usize,
    /// left, bottom, right, top in page space.
    pub bounds: [f32; 4],
    pub text: String,
    pub font: String,
    pub font_size: f32,
    /// The font is embedded as a subset: new characters may have no
    /// glyph in it, so the text is replaced with a font of its own.
    pub subset: bool,
    /// The font's descriptor flags (`FPDFFont_GetFlags`).
    pub flags: i32,
    /// The installed font that would stand in for this one, by name
    /// (`fonts::Fonts::stand_in_name`, filled by the engine); `None` when
    /// only a generic family would.
    pub stand_in: Option<String>,
    /// Where its text starts (the matrix's translation), in page space.
    pub origin: [f32; 2],
    /// The direction its text runs in, a unit vector (the matrix's first
    /// column): the lines are gathered by it.
    pub direction: [f32; 2],
}

/// A font's base name (`ABCDEF+Name` for a subset).
fn base_font_name(api: &'static Api, font: sys::FPDF_FONT) -> String {
    // The length (with the terminator) first, then the name.
    let n = unsafe { (api.FPDFFont_GetBaseFontName)(font, std::ptr::null_mut(), 0) };
    let mut buf = vec![0u8; n.max(1)];
    let n = unsafe { (api.FPDFFont_GetBaseFontName)(font, buf.as_mut_ptr().cast(), buf.len()) };
    String::from_utf8_lossy(&buf[..n.saturating_sub(1).min(buf.len())]).into_owned()
}

/// How differently two pages of `size` (points, /Rotate applied) look:
/// each rendered at 72 dpi (the longer side capped), compared in boxes
/// of `BOX` pixels by their mean colour; the number of boxes whose mean
/// differs by more than `LEVELS` in a channel, those whose centre lies
/// in `skip` (page space, an unrotated page) left out. Glyphs, a colour
/// or a mask lost move a box's mean a lot; text whose edges shifted by
/// a fraction of a point (`content_changed`) moves it little.
#[cfg(test)]
fn page_mismatch(api: &'static Api, a: sys::FPDF_PAGE, b: sys::FPDF_PAGE, size: [f32; 2], skip: Option<[f32; 4]>) -> usize {
    mismatched_boxes(api, a, b, size, skip).len()
}

/// The centres (page space) of the boxes `page_mismatch` counts.
#[cfg(test)]
fn mismatched_boxes(api: &'static Api, a: sys::FPDF_PAGE, b: sys::FPDF_PAGE, size: [f32; 2], skip: Option<[f32; 4]>) -> Vec<[f32; 2]> {
    match (check_render(api, a, size), check_render(api, b, size)) {
        (Some(a), Some(b)) => boxes_apart(&a, &b, size, skip),
        _ => Vec::new(),
    }
}

/// Every text object's characters' origins (page space), by the object's
/// index, read through a text page loaded for it; the characters the
/// text page makes up (a space in a wide gap) are left out.
fn char_origins(api: &'static Api, page: sys::FPDF_PAGE) -> Vec<Vec<[f64; 2]>> {
    let count = unsafe { (api.FPDFPage_CountObjects)(page) }.max(0);
    let handles: Vec<sys::FPDF_PAGEOBJECT> = (0..count).map(|i| unsafe { (api.FPDFPage_GetObject)(page, i) }).collect();
    let mut out = vec![Vec::new(); count as usize];
    let text_page = unsafe { (api.FPDFText_LoadPage)(page) };
    if text_page.is_null() {
        return out;
    }
    let mut last = None;
    for i in 0..unsafe { (api.FPDFText_CountChars)(text_page) }.max(0) {
        // A character the text page made up (a space in a wide gap) is
        // given the object after it.
        if unsafe { (api.FPDFText_IsGenerated)(text_page, i) } != 0 {
            continue;
        }
        let object = unsafe { (api.FPDFText_GetTextObject)(text_page, i) };
        if object.is_null() {
            continue;
        }
        // The runs come in order: the one before is tried first.
        let k = match last {
            Some((h, k)) if h == object => k,
            _ => match handles.iter().position(|&h| h == object) {
                Some(k) => k,
                None => continue,
            },
        };
        last = Some((object, k));
        let (mut x, mut y) = (0f64, 0f64);
        unsafe { (api.FPDFText_GetCharOrigin)(text_page, i, &mut x, &mut y) };
        out[k].push([x, y]);
    }
    unsafe { (api.FPDFText_ClosePage)(text_page) };
    out
}

/// The longest side a page is rendered at for `boxes_apart`: 72 dpi
/// unless the page is huge.
const CHECK_SIDE: f32 = 1600.0;

fn check_scale(size: [f32; 2]) -> f32 {
    (CHECK_SIDE / size[0].max(size[1]).max(1.0)).min(1.0)
}

/// A page of `size` rendered for `boxes_apart`: on white, no annotations.
fn check_render(api: &'static Api, page: sys::FPDF_PAGE, size: [f32; 2]) -> Option<Bitmap> {
    let scale = check_scale(size);
    let (w, h) = (((size[0] * scale) as u32).max(1), ((size[1] * scale) as u32).max(1));
    let mut bitmap = Bitmap::new(api, w, h)?;
    bitmap.fill(0xffff_ffff);
    unsafe { (api.FPDF_RenderPageBitmap)(bitmap.handle, page, 0, 0, w as c_int, h as c_int, 0, 0) };
    Some(bitmap)
}

/// The boxes in which two `check_render`s of a page of `size` differ.
fn boxes_apart(a: &Bitmap, b: &Bitmap, size: [f32; 2], skip: Option<[f32; 4]>) -> Vec<[f32; 2]> {
    const BOX: usize = 8;
    const LEVELS: i64 = 32;
    let scale = check_scale(size);
    let (pa, pb) = (a.pixels(), b.pixels());
    let (w, h) = (a.width as usize, a.height as usize);
    if (b.width as usize, b.height as usize) != (w, h) {
        return Vec::new();
    }
    let mut boxes = Vec::new();
    for by in (0..h).step_by(BOX) {
        for bx in (0..w).step_by(BOX) {
            let centre = [(bx + BOX / 2) as f32 / scale, size[1] - (by + BOX / 2) as f32 / scale];
            if let Some([l, bottom, r, t]) = skip
                && centre[0] >= l
                && centre[0] <= r
                && centre[1] >= bottom
                && centre[1] <= t
            {
                continue;
            }
            let mut sum = [0i64; 3];
            let mut n = 0i64;
            for y in by..(by + BOX).min(h) {
                for x in bx..(bx + BOX).min(w) {
                    let i = (y * w + x) * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += i64::from(pa[i + c]) - i64::from(pb[i + c]);
                    }
                    n += 1;
                }
            }
            if sum.iter().any(|s| s.abs() > LEVELS * n) {
                boxes.push(centre);
            }
        }
    }
    boxes
}

/// Gives the stand-in font's bytes for a base font's name and descriptor
/// flags, when the document's own font cannot show new text.
pub type FontLookup<'a> = &'a mut dyn FnMut(&str, i32) -> Option<std::sync::Arc<Vec<u8>>>;

/// Whether a text read back from a page object says `wanted`: the text
/// page reads a subset font's space as a no-break space and a hyphen at a
/// line's end as U+0002, and runs of whitespace count as one.
fn same_text(shown: &str, wanted: &str) -> bool {
    let norm = |s: &str| s.replace('\u{2}', "-").split_whitespace().collect::<Vec<_>>().join(" ");
    norm(shown) == norm(wanted)
}

/// How a text object's text was replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replaced {
    /// In place, with the object's own font.
    SameFont,
    /// By a new object in the fallback font (the old one is gone).
    FallbackFont,
}

/// An image object of a page, as the editing sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageObject {
    /// Its index among the page's objects.
    pub index: usize,
    /// left, bottom, right, top in page space.
    pub bounds: [f32; 4],
}

/// The page's top-level objects the editing works on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageObjects {
    pub text: Vec<TextObject>,
    pub images: Vec<ImageObject>,
    /// The text objects gathered in lines (`textline::lines`, filled by
    /// the engine).
    pub lines: Vec<crate::textline::Line>,
}

/// Reads of a file held in memory, as PDFium asks for them (a JPEG loaded
/// into an image object).
unsafe extern "C" fn get_block_memory(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    // SAFETY: `param` is the slice `replace_image` passes, alive for the call.
    let data = unsafe { &*(param as *const Vec<u8>) };
    let (start, len) = (position as usize, size as usize);
    if start.checked_add(len).is_none_or(|end| end > data.len()) {
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(start), buf, len) };
    1
}

impl Page {
    /// The page's text and image objects, top level only.
    pub fn objects(&self) -> PageObjects {
        PageObjects { text: self.text_objects(), images: self.image_objects(), lines: Vec::new() }
    }

    /// The page's image objects, top level only.
    pub fn image_objects(&self) -> Vec<ImageObject> {
        let api = self.api();
        let count = unsafe { (api.FPDFPage_CountObjects)(self.handle.get()) }.max(0);
        let mut out = Vec::new();
        for index in 0..count {
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index) };
            if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_IMAGE {
                continue;
            }
            let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
            if unsafe { (api.FPDFPageObj_GetBounds)(object, &mut l, &mut b, &mut r, &mut t) } == 0 {
                continue;
            }
            out.push(ImageObject { index: index as usize, bounds: [l.min(r), b.min(t), l.max(r), b.max(t)] });
        }
        out
    }

    fn object_of_type(&self, index: usize, kind: c_int, what: &str) -> Result<sys::FPDF_PAGEOBJECT, String> {
        let object = unsafe { (self.api().FPDFPage_GetObject)(self.handle.get(), index as c_int) };
        if object.is_null() || unsafe { (self.api().FPDFPageObj_GetType)(object) } != kind {
            return Err(format!("not {what}"));
        }
        Ok(object)
    }

    /// After a change of the page's objects: the content written anew
    /// from them, the text page (which described the old content)
    /// closed, the new content parsed and checked against the objects,
    /// by count and by look (`page_mismatch`), and the parsed page taking
    /// the place of the one in memory. `FPDFPage_GenerateContent` leaves
    /// out text in Type 3 fonts, shadings (`sh`) and inline images, which
    /// the count catches (`has_type3_text` says so before an edit), and
    /// writes a pattern fill as black, drops a soft mask and tells fonts
    /// apart by name alone (`clashing_fonts`), which the look catches. An
    /// error leaves the page changed in memory: the caller puts the
    /// document back (the engine rebuilds its copy from the log).
    fn content_changed(&self) -> Result<(), String> {
        let api = self.api();
        let size = self.size();
        // The colours as PDFium reports them are set back as RGB so the
        // writer keeps them; an object whose reported colour is not what
        // it draws (a pattern fill reads as a flat grey) looks different
        // after that, and the edit is refused before anything is written.
        let before = check_render(api, self.handle.get(), size);
        self.colours_to_rgb();
        let after = check_render(api, self.handle.get(), size);
        if let (Some(before), Some(after)) = (&before, &after) {
            let changed = boxes_apart(before, after, size, None).len();
            if changed > 0 {
                return Err(tr!(
                    format!("the page's content cannot be written back as it is ({changed} parts of the page are filled with a pattern, which PDFium's writer does not keep)"),
                    format!("содержимое страницы нельзя записать как есть (частей страницы с заливкой узором, который PDFium не сохраняет: {changed})")
                ));
            }
        }
        if unsafe { (api.FPDFPage_GenerateContent)(self.handle.get()) } == 0 {
            return Err("the page's content could not be regenerated".into());
        }
        if let Some(text) = self.text.take() {
            unsafe { (api.FPDFText_ClosePage)(text) };
        }
        // The page parsed again from the content just written.
        let mut again = unsafe { (api.FPDF_LoadPage)(self.doc.handle, self.index) };
        if again.is_null() {
            return Err("the page's new content could not be read".into());
        }
        let count = |page: sys::FPDF_PAGE| {
            let mut counts = [0usize; 6];
            let n = unsafe { (api.FPDFPage_CountObjects)(page) }.max(0);
            for i in 0..n {
                let kind = unsafe { (api.FPDFPageObj_GetType)((api.FPDFPage_GetObject)(page, i)) };
                counts[usize::try_from(kind).unwrap_or(0).min(5)] += 1;
            }
            counts
        };
        let (wanted, written) = (count(self.handle.get()), count(again));
        if wanted != written {
            unsafe { (api.FPDF_ClosePage)(again) };
            const KINDS: [&str; 6] = ["objects", "text", "paths", "pictures", "shadings", "forms"];
            const KINDS_RU: [&str; 6] = ["объекты", "текст", "контуры", "изображения", "градиенты", "формы"];
            let kinds = tr!(KINDS, KINDS_RU);
            let lost: Vec<String> = (0..6).filter(|&k| written[k] < wanted[k]).map(|k| format!("{} {}", wanted[k] - written[k], kinds[k])).collect();
            return Err(tr!(
                format!("the page's content cannot be written back whole ({} would be lost)", lost.join(", ")),
                format!("содержимое страницы нельзя записать целиком (потерялось бы: {})", lost.join(", "))
            ));
        }
        // The writer leaves out the character and word spacing (`Tc`,
        // `Tw`) and keeps the `TJ` kerning: a run set with them parses
        // with its characters closer or further by the spacing each (a
        // book's 8 pt lines by 4.5 pt at their ends). Measured on the
        // parsed page and undone in memory, the content is written once
        // more and parses to where the characters were.
        let shifted = self.undo_drift(again);
        if shifted > 0 {
            unsafe { (api.FPDF_ClosePage)(again) };
            if unsafe { (api.FPDFPage_GenerateContent)(self.handle.get()) } == 0 {
                return Err("the page's content could not be regenerated".into());
            }
            again = unsafe { (api.FPDF_LoadPage)(self.doc.handle, self.index) };
            if again.is_null() {
                return Err("the page's new content could not be read".into());
            }
            log::debug!("page {}: {shifted} runs set with character spacing, which the writer leaves out, written at their places", self.index + 1);
        }
        // The same objects, but do they look the same? The writer leaves
        // out what it does not know (a pattern fill, a soft mask, a
        // colour not in RGB: `colours_to_rgb`), and tells fonts apart by
        // name alone (`clashing_fonts`): the page parsed from its content
        // is compared with the one in memory.
        let changed = match (&after, check_render(api, again, size)) {
            (Some(after), Some(parsed)) => boxes_apart(after, &parsed, size, None).len(),
            _ => 0,
        };
        log::debug!("page {}: content written anew, {changed} boxes look different", self.index + 1);
        if changed > 0 {
            unsafe { (api.FPDF_ClosePage)(again) };
            return Err(tr!(
                format!("the page's content cannot be written back as it is ({changed} parts of the page would look different: a soft mask or a font PDFium's writer does not keep)"),
                format!("содержимое страницы нельзя записать как есть (изменились бы части страницы: {changed}; мягкая маска или шрифт, которые PDFium не сохраняет)")
            ));
        }
        // The parsed page stands in for the one edited in memory from
        // here on: what is shown is what a save writes. The writer leaves
        // out the character and word spacing (`Tc`, `Tw`) of every run on
        // the page, so a run set with them drifts by that spacing a
        // character, a fraction of a point on a line; shown as it is,
        // the drift is in sight before the file is saved.
        unsafe {
            if !self.doc.form.is_null() {
                (api.FORM_OnAfterLoadPage)(again, self.doc.form);
                (api.FORM_OnBeforeClosePage)(self.handle.get(), self.doc.form);
            }
            (api.FPDF_ClosePage)(self.handle.replace(again));
        }
        Ok(())
    }

    /// The drift of every text run between this page and `parsed`, the
    /// same page parsed from the content written anew, undone: where a
    /// character's origin moved along the run (the writer leaves out
    /// `Tc` and `Tw`), the run's positions in memory are shifted the
    /// other way (`FPDFText_SetPositions`, relative to the run's origin
    /// in text space), so that the content written again parses to the
    /// origins the run had. How many runs were shifted. The page in
    /// memory is wrong by that shift until it is parsed again.
    fn undo_drift(&self, parsed: sys::FPDF_PAGE) -> usize {
        let api = self.api();
        let before = char_origins(api, self.handle.get());
        let after = char_origins(api, parsed);
        let mut shifted = 0;
        for (k, (b, a)) in before.iter().zip(&after).enumerate() {
            if b.len() < 2 || b.len() != a.len() {
                continue;
            }
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), k as c_int) };
            if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_TEXT {
                continue;
            }
            let mut matrix = sys::FS_MATRIX { a: 1.0, ..Default::default() };
            unsafe { (api.FPDFPageObj_GetMatrix)(object, &mut matrix) };
            let scale = (matrix.a * matrix.a + matrix.b * matrix.b).sqrt().max(1e-6);
            let along = |p: &[f64; 2], q: &[f64; 2]| ((q[0] - p[0]) as f32 * matrix.a + (q[1] - p[1]) as f32 * matrix.b) / scale;
            let drift: Vec<f32> = b.iter().zip(a).map(|(p, q)| along(p, q)).collect();
            if drift.iter().all(|x| x.abs() < 0.002) {
                continue;
            }
            // Each character's distance from the first, in text space,
            // less its drift.
            let positions: Vec<f32> = b.iter().zip(&drift).skip(1).map(|(p, x)| (along(&b[0], p) - x) / scale).collect();
            if unsafe { (api.FPDFText_SetPositions)(object, positions.as_ptr(), positions.len()) } != 0 {
                shifted += 1;
            }
        }
        shifted
    }

    /// Every object's fill and stroke colour set again as the RGB PDFium
    /// renders it with: the content writer puts a colour in the stream
    /// only when it is DeviceRGB, so text in CMYK black, in grey or in a
    /// spot colour (a book's headings in PANTONE 354) came out black when
    /// saved. A pattern has no RGB and is left alone (`page_mismatch`
    /// catches it); a form XObject keeps its own content.
    fn colours_to_rgb(&self) {
        let api = self.api();
        let count = unsafe { (api.FPDFPage_CountObjects)(self.handle.get()) }.max(0);
        for index in 0..count {
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index) };
            if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } == sys::FPDF_PAGEOBJ_FORM {
                continue;
            }
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 255u32);
            unsafe {
                if (api.FPDFPageObj_GetFillColor)(object, &mut r, &mut g, &mut b, &mut a) != 0 {
                    (api.FPDFPageObj_SetFillColor)(object, r, g, b, a);
                }
                if (api.FPDFPageObj_GetStrokeColor)(object, &mut r, &mut g, &mut b, &mut a) != 0 {
                    (api.FPDFPageObj_SetStrokeColor)(object, r, g, b, a);
                }
            }
        }
    }

    /// The base font name two different embedded fonts of the page's text
    /// objects share, if any: PDFium's content writer tells fonts apart by
    /// that name and writes every run of such a pair in one of them (a
    /// book made by LibreOffice had two subsets of PTSans-Bold, both
    /// `BAAAAA+`: the other's letters came out blank). Fonts of one size
    /// are taken for one font (a stand-in loaded twice); a pair not
    /// embedded is left to `content_changed`'s look check.
    pub fn clashing_fonts(&self) -> Option<String> {
        let api = self.api();
        let mut seen: std::collections::HashMap<String, (sys::FPDF_FONT, usize)> = std::collections::HashMap::new();
        let count = unsafe { (api.FPDFPage_CountObjects)(self.handle.get()) }.max(0);
        for index in 0..count {
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index) };
            if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_TEXT {
                continue;
            }
            let font = unsafe { (api.FPDFTextObj_GetFont)(object) };
            if font.is_null() || unsafe { (api.FPDFFont_GetIsEmbedded)(font) } != 1 {
                continue;
            }
            let name = base_font_name(api, font);
            let mut size = 0usize;
            unsafe { (api.FPDFFont_GetFontData)(font, std::ptr::null_mut(), 0, &mut size) };
            match seen.get(&name) {
                None => {
                    seen.insert(name, (font, size));
                }
                Some(&(other, other_size)) if other != font && other_size != size => return Some(name),
                _ => {}
            }
        }
        None
    }

    /// Remove object `index` (an image) from the page.
    pub fn remove_object(&self, index: usize) -> Result<(), String> {
        let api = self.api();
        let object = self.object_of_type(index, sys::FPDF_PAGEOBJ_IMAGE, "an image")?;
        if unsafe { (api.FPDFPage_RemoveObject)(self.handle.get(), object) } == 0 {
            return Err("the object could not be removed".into());
        }
        unsafe { (api.FPDFPageObj_Destroy)(object) };
        self.content_changed()
    }

    /// Put the picture in `path` (JPEG or PNG) into image object `index`,
    /// fitted into the old picture's box with its own proportions, centred.
    pub fn replace_image(&self, index: usize, path: &Path) -> Result<(), String> {
        let api = self.api();
        let object = self.object_of_type(index, sys::FPDF_PAGEOBJ_IMAGE, "an image")?;
        let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
        unsafe { (api.FPDFPageObj_GetBounds)(object, &mut l, &mut b, &mut r, &mut t) };
        let old = [l.min(r), b.min(t), l.max(r), b.max(t)];
        let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let is_jpeg = data.starts_with(&[0xff, 0xd8]);
        let is_png = data.starts_with(b"\x89PNG");
        let mut pages = [self.handle.get()];
        let (w, h) = if is_jpeg {
            let mut access = sys::FPDF_FILEACCESS {
                m_FileLen: c_ulong::try_from(data.len()).map_err(|_| "the picture is too large")?,
                m_GetBlock: Some(get_block_memory),
                m_Param: &data as *const Vec<u8> as *mut c_void,
            };
            if unsafe { (api.FPDFImageObj_LoadJpegFileInline)(pages.as_mut_ptr(), 1, object, &mut access) } == 0 {
                return Err("PDFium could not read the JPEG".into());
            }
            let (mut w, mut h) = (0u32, 0u32);
            unsafe { (api.FPDFImageObj_GetImagePixelSize)(object, &mut w, &mut h) };
            (w, h)
        } else if is_png {
            let (w, h, bgra, alpha) = decode_png(&data)?;
            // PDFium copies the picture out of the bitmap, which is freed here.
            let bitmap = Bitmap::from_bgra(api, w, h, bgra, alpha).ok_or("the picture is too large")?;
            if unsafe { (api.FPDFImageObj_SetBitmap)(pages.as_mut_ptr(), 1, object, bitmap.handle) } == 0 {
                return Err("PDFium could not take the picture".into());
            }
            (w, h)
        } else {
            return Err("only JPEG and PNG pictures can be put in".into());
        };
        // The new picture's proportions within the old box.
        let (bw, bh) = (old[2] - old[0], old[3] - old[1]);
        let (mut nw, mut nh) = (bw, bh);
        if w > 0 && h > 0 && bw > 0.0 && bh > 0.0 {
            let aspect = w as f32 / h as f32;
            if bw / bh > aspect {
                nw = bh * aspect;
            } else {
                nh = bw / aspect;
            }
        }
        let (x, y) = (old[0] + (bw - nw) / 2.0, old[1] + (bh - nh) / 2.0);
        unsafe { (api.FPDFImageObj_SetMatrix)(object, f64::from(nw), 0.0, 0.0, f64::from(nh), f64::from(x), f64::from(y)) };
        self.content_changed()
    }

    /// Move and resize object `index` so that its box becomes `bounds`
    /// (left, bottom, right, top in page space).
    pub fn place_object(&self, index: usize, bounds: [f32; 4]) -> Result<(), String> {
        let api = self.api();
        let object = self.object_of_type(index, sys::FPDF_PAGEOBJ_IMAGE, "an image")?;
        let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
        unsafe { (api.FPDFPageObj_GetBounds)(object, &mut l, &mut b, &mut r, &mut t) };
        let (w0, h0) = ((r - l).abs().max(1e-3), (t - b).abs().max(1e-3));
        let (w1, h1) = ((bounds[2] - bounds[0]).max(0.1), (bounds[3] - bounds[1]).max(0.1));
        let (l0, b0) = (l.min(r), b.min(t));
        unsafe {
            // To the origin, scaled, to the new place.
            (api.FPDFPageObj_Transform)(object, 1.0, 0.0, 0.0, 1.0, -f64::from(l0), -f64::from(b0));
            (api.FPDFPageObj_Transform)(object, f64::from(w1 / w0), 0.0, 0.0, f64::from(h1 / h0), 0.0, 0.0);
            (api.FPDFPageObj_Transform)(object, 1.0, 0.0, 0.0, 1.0, f64::from(bounds[0]), f64::from(bounds[1]));
        }
        self.content_changed()
    }

    /// Whether the page has text in a Type 3 font (glyphs drawn by
    /// content streams, as Chrome and Edge print variable fonts, Cascadia
    /// among them): `FPDFPage_GenerateContent` writes no such text, so an
    /// edit on the page would lose it when saved. Known by the font
    /// having no face, so no bytes (`FPDFFont_GetFontData`); a font not
    /// embedded gives its substitute's.
    pub fn has_type3_text(&self) -> bool {
        let api = self.api();
        let count = unsafe { (api.FPDFPage_CountObjects)(self.handle.get()) }.max(0);
        (0..count).any(|index| {
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index) };
            if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_TEXT {
                return false;
            }
            let font = unsafe { (api.FPDFTextObj_GetFont)(object) };
            if font.is_null() {
                return false;
            }
            let mut size = 0usize;
            unsafe { (api.FPDFFont_GetFontData)(font, std::ptr::null_mut(), 0, &mut size) != 0 && size == 0 }
        })
    }

    /// The page's text objects, top level only (text inside form
    /// XObjects is not reachable for editing).
    pub fn text_objects(&self) -> Vec<TextObject> {
        let api = self.api();
        let Some(text_page) = self.text_page() else {
            return Vec::new();
        };
        let count = unsafe { (api.FPDFPage_CountObjects)(self.handle.get()) }.max(0);
        let mut out = Vec::new();
        for index in 0..count {
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index) };
            if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_TEXT {
                continue;
            }
            // Invisible text (a scan's OCR layer) and text that only clips:
            // an edit would show nothing.
            let mode = unsafe { (api.FPDFTextObj_GetTextRenderMode)(object) };
            if mode == sys::FPDF_TEXTRENDERMODE_INVISIBLE || mode == sys::FPDF_TEXTRENDERMODE_CLIP {
                continue;
            }
            let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
            if unsafe { (api.FPDFPageObj_GetBounds)(object, &mut l, &mut b, &mut r, &mut t) } == 0 {
                continue;
            }
            let text = utf16_text(|buf, len| unsafe { (api.FPDFTextObj_GetText)(object, text_page, buf.cast(), len) }).unwrap_or_default();
            let mut size = 0f32;
            unsafe { (api.FPDFTextObj_GetFontSize)(object, &mut size) };
            let font = unsafe { (api.FPDFTextObj_GetFont)(object) };
            let (name, subset, flags) = if font.is_null() {
                (String::new(), false, 0)
            } else {
                // The length (with the terminator) first, then the name.
                let n = unsafe { (api.FPDFFont_GetBaseFontName)(font, std::ptr::null_mut(), 0) };
                let mut buf = vec![0u8; n.max(1)];
                let n = unsafe { (api.FPDFFont_GetBaseFontName)(font, buf.as_mut_ptr().cast(), buf.len()) };
                let name = String::from_utf8_lossy(&buf[..n.saturating_sub(1).min(buf.len())]).into_owned();
                let embedded = unsafe { (api.FPDFFont_GetIsEmbedded)(font) } == 1;
                // A subset's name carries a six-letter tag: `ABCDEF+Name`.
                let tagged = name.len() > 7 && name.as_bytes()[6] == b'+';
                let flags = unsafe { (api.FPDFFont_GetFlags)(font) };
                (name, embedded && tagged, flags)
            };
            // The size as drawn: the object's matrix scales the font (a
            // 1 pt font under a 24× matrix is how some generators write).
            let mut matrix = sys::FS_MATRIX { a: 1.0, ..Default::default() };
            let scale = if unsafe { (api.FPDFPageObj_GetMatrix)(object, &mut matrix) } != 0 { (matrix.a * matrix.a + matrix.b * matrix.b).sqrt().max(0.01) } else { 1.0 };
            let drawn = size * scale;
            let direction = [matrix.a / scale, matrix.b / scale];
            out.push(TextObject { index: index as usize, bounds: [l, b, r, t], text, font: name, font_size: drawn, subset, flags, stand_in: None, origin: [matrix.e, matrix.f], direction });
        }
        out
    }

    /// Replace the text of object `index` with `text`: in place when the
    /// object's font can show any text (not an embedded subset), else by
    /// a new object at the same place, size and colour in a stand-in
    /// font, which `fallback` gives (a TrueType font's bytes) for the
    /// base font's name and descriptor flags, so that the style (bold,
    /// italic, serif) is kept. The page's content is regenerated; the
    /// text page is reloaded.
    #[cfg(test)]
    pub fn set_text(&self, index: usize, text: &str, fallback: FontLookup<'_>) -> Result<Replaced, String> {
        self.set_line(&[index], text, &[], fallback)
    }

    /// Objects `objects` of a line made one, the first, with `text` (as
    /// `set_text` sets it); the others removed, and `followers`, the
    /// objects after them on the line, moved along it by the change in
    /// width, so the new text has its room and the gap after it stays.
    /// The width is measured on the ink: the edge of the last object's
    /// last glyph before, of the new object's after.
    pub fn set_line(&self, objects: &[usize], text: &str, followers: &[usize], fallback: FontLookup<'_>) -> Result<Replaced, String> {
        let api = self.api();
        let handle = |index: usize| {
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index as c_int) };
            (!object.is_null() && unsafe { (api.FPDFPageObj_GetType)(object) } == sys::FPDF_PAGEOBJ_TEXT).then_some(object).ok_or("not a text object")
        };
        // The handles first: the indices change as objects go.
        let merged = objects.iter().map(|&i| handle(i)).collect::<Result<Vec<_>, _>>()?;
        let moved = followers.iter().map(|&i| handle(i)).collect::<Result<Vec<_>, _>>()?;
        let Some((&first, rest)) = merged.split_first() else { return Err("no object".into()) };
        let mut matrix = sys::FS_MATRIX { a: 1.0, ..Default::default() };
        unsafe { (api.FPDFPageObj_GetMatrix)(first, &mut matrix) };
        let scale = (matrix.a * matrix.a + matrix.b * matrix.b).sqrt().max(0.01);
        let (dx, dy) = (matrix.a / scale, matrix.b / scale);
        // How far the object's ink reaches along the line.
        let reach = |object: sys::FPDF_PAGEOBJECT| {
            let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
            unsafe { (api.FPDFPageObj_GetBounds)(object, &mut l, &mut b, &mut r, &mut t) };
            [(l, b), (l, t), (r, b), (r, t)].iter().map(|&(x, y)| x * dx + y * dy).fold(f32::NEG_INFINITY, f32::max)
        };
        let old_reach = merged.iter().map(|&o| reach(o)).fold(f32::NEG_INFINITY, f32::max);
        // A stand-in fits its width a character to the objects merged.
        let span = (merged.len() > 1).then(|| {
            let chars = merged.iter().map(|&o| self.object_text(o).trim().chars().count()).sum::<usize>();
            let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
            for &o in &merged {
                let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
                unsafe { (api.FPDFPageObj_GetBounds)(o, &mut l, &mut b, &mut r, &mut t) };
                (left, right) = (left.min(l), right.max(r));
            }
            (chars, right - left)
        });
        // The first set first: what fails there leaves the page as it was.
        let (replaced, new) = self.replace_text(objects[0], text, span, fallback)?;
        for &object in rest {
            unsafe {
                if (api.FPDFPage_RemoveObject)(self.handle.get(), object) == 0 {
                    // Left in place it would show its text over the new
                    // one; the engine rebuilds its copy on the error.
                    return Err("a run of the line could not be removed".into());
                }
                (api.FPDFPageObj_Destroy)(object);
            }
        }
        let delta = reach(new) - old_reach;
        if delta.abs() > 0.01 {
            for &object in &moved {
                unsafe { (api.FPDFPageObj_Transform)(object, 1.0, 0.0, 0.0, 1.0, f64::from(dx * delta), f64::from(dy * delta)) };
            }
        }
        self.content_changed()?;
        Ok(replaced)
    }

    /// Object `index`'s text replaced, as `set_text` describes, the page's
    /// content not yet regenerated; the object that shows the text then.
    fn replace_text(&self, index: usize, text: &str, span: Option<(usize, f32)>, fallback: FontLookup<'_>) -> Result<(Replaced, sys::FPDF_PAGEOBJECT), String> {
        let api = self.api();
        let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index as c_int) };
        if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_TEXT {
            return Err("not a text object".into());
        }
        if text.is_empty() {
            return Err("the text is empty".into());
        }
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let font = unsafe { (api.FPDFTextObj_GetFont)(object) };
        let mut base_name = String::new();
        let mut flags = 0;
        let subset = !font.is_null() && {
            let mut buf = vec![0u8; 256];
            let n = unsafe { (api.FPDFFont_GetBaseFontName)(font, buf.as_mut_ptr().cast(), buf.len()) };
            let embedded = unsafe { (api.FPDFFont_GetIsEmbedded)(font) } == 1;
            base_name = String::from_utf8_lossy(&buf[..n.saturating_sub(1).min(buf.len())]).into_owned();
            flags = unsafe { (api.FPDFFont_GetFlags)(font) };
            embedded && n > 7 && buf[6] == b'+'
        };
        // The old text and its box: for the width fit, and to put back if
        // the font proves unable to show the new text.
        let old_text = self.object_text(object);
        let (mut ol, mut ob, mut or, mut ot) = (0f32, 0f32, 0f32, 0f32);
        unsafe { (api.FPDFPageObj_GetBounds)(object, &mut ol, &mut ob, &mut or, &mut ot) };
        // In place first, when the font has a glyph for every character
        // (a subset has the digits of a date): the run keeps its font and
        // its look. What came out is read back, since a font that cannot
        // encode a character (a subset's space) drops it: then the old
        // text is put back and the stand-in takes over. A space has no
        // outline, so no glyph path: it is left to the reading back, or
        // every edit with a space went to the stand-in.
        let has_glyph = |c: char| c.is_whitespace() || !font.is_null() && !unsafe { (api.FPDFFont_GetGlyphPath)(font, u32::from(c), 12.0) }.is_null();
        // The code PDFium encodes a space with may have no width (below,
        // `char_codes`): the words ran together.
        let has_width = |c: char| {
            let mut width = 0f32;
            !font.is_null() && unsafe { (api.FPDFFont_GetGlyphWidth)(font, u32::from(c), 1.0, &mut width) } != 0 && width > 0.0
        };
        let mut codes = None;
        if !subset || text.chars().all(has_glyph) {
            // A subset font written by PDFium maps its space to U+00A0
            // and drops U+0020: the no-break space is tried after.
            let attempts = if text.contains(' ') { vec![text.to_owned(), text.replace(' ', "\u{a0}")] } else { vec![text.to_owned()] };
            for attempt in &attempts {
                let set = if attempt.chars().filter(|c| c.is_whitespace()).all(has_width) {
                    let attempt_wide: Vec<u16> = attempt.encode_utf16().chain(std::iter::once(0)).collect();
                    unsafe { (api.FPDFText_SetText)(object, attempt_wide.as_ptr()) }
                } else {
                    // A whitespace with no width: the font's codes found
                    // by trying them, a whitespace without its own code
                    // given the space's.
                    let codes = codes.get_or_insert_with(|| self.char_codes(font));
                    let encoded: Option<Vec<u32>> = attempt.chars().map(|c| codes.get(&c).or_else(|| c.is_whitespace().then(|| codes.get(&' ')).flatten()).copied()).collect();
                    let Some(encoded) = encoded else { continue };
                    unsafe { (api.FPDFText_SetCharcodes)(object, encoded.as_ptr(), encoded.len()) }
                };
                if set == 0 {
                    return Err("the text could not be set".into());
                }
                if same_text(&self.object_text(object), text) {
                    return Ok((Replaced::SameFont, object));
                }
            }
            let old_wide: Vec<u16> = old_text.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe { (api.FPDFText_SetText)(object, old_wide.as_ptr()) };
        }
        let bytes = fallback(&base_name, flags).ok_or("the font cannot show the text, and there is no stand-in font")?;
        // The width a character to fit to: the old text's, or the span's
        // the text replaces (a line's merged objects).
        let (old_len, old_width) = span.unwrap_or((old_text.trim().chars().count(), or - ol));
        let new = {
            {
                // The same place, size, colour and mode in a font that has
                // every glyph.
                let mut size = 12f32;
                unsafe { (api.FPDFTextObj_GetFontSize)(object, &mut size) };
                let mut matrix = sys::FS_MATRIX::default();
                unsafe { (api.FPDFPageObj_GetMatrix)(object, &mut matrix) };
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 255u32);
                unsafe { (api.FPDFPageObj_GetFillColor)(object, &mut r, &mut g, &mut b, &mut a) };
                let mode = unsafe { (api.FPDFTextObj_GetTextRenderMode)(object) };
                let new_font = unsafe { (api.FPDFText_LoadFont)(self.doc.handle, bytes.as_ptr(), bytes.len() as u32, sys::FPDF_FONT_TRUETYPE, 1) };
                if new_font.is_null() {
                    return Err("the fallback font could not be loaded".into());
                }
                let new = unsafe { (api.FPDFPageObj_CreateTextObj)(self.doc.handle, new_font, size) };
                if new.is_null() {
                    unsafe { (api.FPDFFont_Close)(new_font) };
                    return Err("a text object could not be made".into());
                }
                unsafe {
                    (api.FPDFText_SetText)(new, wide.as_ptr());
                    (api.FPDFPageObj_SetMatrix)(new, &matrix);
                    // Fitted to the old run's width a character: a run of
                    // the same length fills the same box, whatever the
                    // stand-in's own widths.
                    let (mut nl, mut nb, mut nr, mut nt) = (0f32, 0f32, 0f32, 0f32);
                    let new_len = text.trim().chars().count();
                    if old_len > 0 && new_len > 0 && (api.FPDFPageObj_GetBounds)(new, &mut nl, &mut nb, &mut nr, &mut nt) != 0 {
                        let target = old_width * new_len as f32 / old_len as f32;
                        let sx = (target / (nr - nl).max(0.01)).clamp(0.5, 1.5);
                        if (sx - 1.0).abs() > 0.01 {
                            let fitted = sys::FS_MATRIX { a: matrix.a * sx, b: matrix.b * sx, c: matrix.c, d: matrix.d, e: matrix.e, f: matrix.f };
                            (api.FPDFPageObj_SetMatrix)(new, &fitted);
                        }
                    }
                    (api.FPDFPageObj_SetFillColor)(new, r, g, b, a);
                    (api.FPDFTextObj_SetTextRenderMode)(new, mode);
                    // A stroked mode draws with the stroke's colour and width.
                    let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 255u32);
                    if (api.FPDFPageObj_GetStrokeColor)(object, &mut sr, &mut sg, &mut sb, &mut sa) != 0 {
                        (api.FPDFPageObj_SetStrokeColor)(new, sr, sg, sb, sa);
                    }
                    let mut stroke_width = 1f32;
                    if (api.FPDFPageObj_GetStrokeWidth)(object, &mut stroke_width) != 0 {
                        (api.FPDFPageObj_SetStrokeWidth)(new, stroke_width);
                    }
                    self.copy_marks(object, new);
                    // In the old one's place: the content's order (the
                    // reading order of an untagged document) and the other
                    // objects' indices stay.
                    if (api.FPDFPage_RemoveObject)(self.handle.get(), object) == 0 {
                        // Both would show, one over the other.
                        (api.FPDFPageObj_Destroy)(new);
                        (api.FPDFFont_Close)(new_font);
                        return Err("the old text could not be removed".into());
                    }
                    (api.FPDFPageObj_Destroy)(object);
                    if (api.FPDFPage_InsertObjectAtIndex)(self.handle.get(), new, index) == 0 {
                        (api.FPDFPage_InsertObject)(self.handle.get(), new);
                    }
                    (api.FPDFFont_Close)(new_font);
                }
                new
            }
        };
        Ok((Replaced::FallbackFont, new))
    }

    /// The object's text as the text page reads it, the text page loaded
    /// afresh (the page's objects may have changed).
    fn object_text(&self, object: sys::FPDF_PAGEOBJECT) -> String {
        let api = self.api();
        if let Some(text) = self.text.take() {
            unsafe { (api.FPDFText_ClosePage)(text) };
        }
        self.text_page().and_then(|tp| utf16_text(|buf, len| unsafe { (api.FPDFTextObj_GetText)(object, tp, buf.cast(), len) })).unwrap_or_default()
    }

    /// The codes of `font`'s characters among codes 0 to 255, each with
    /// an advance. PDFium encodes a character with the first code its
    /// reverse lookup of the ToUnicode map finds, and a map may give a
    /// character two: a book's PetersburgC has its space at code 30 and
    /// maps code 32, no glyph and width 0, to U+0020 too, so `FPDFText_
    /// SetText` wrote every space with no width. So every code is tried
    /// in a scratch object, between two copies of a code with ink, which
    /// gives its advance (the box's growth) and, through the text page,
    /// its character (a space alone reads as nothing); of the codes of a
    /// character, the one as wide as PDFium's own wins, else the widest.
    /// A font with two-byte codes gives only what it has in that range.
    fn char_codes(&self, font: sys::FPDF_FONT) -> std::collections::HashMap<char, u32> {
        const SIZE: f32 = 10.0;
        let api = self.api();
        let mut out = std::collections::HashMap::new();
        if font.is_null() {
            return out;
        }
        let scratch = unsafe { (api.FPDFPageObj_CreateTextObj)(self.doc.handle, font, SIZE) };
        if scratch.is_null() {
            return out;
        }
        let bounds = |codes: &[u32]| {
            let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
            let ok = unsafe { (api.FPDFText_SetCharcodes)(scratch, codes.as_ptr(), codes.len()) != 0 && (api.FPDFPageObj_GetBounds)(scratch, &mut l, &mut b, &mut r, &mut t) != 0 };
            ok.then_some([l, b, r, t])
        };
        let ink = (0..=255u32).find(|&c| bounds(&[c]).is_some_and(|[l, b, r, t]| r - l > 0.01 && t - b > 0.01));
        let base = ink.and_then(|k| bounds(&[k, k]));
        let (Some(k), Some(base)) = (ink, base) else {
            unsafe { (api.FPDFPageObj_Destroy)(scratch) };
            return out;
        };
        let advances: Vec<(u32, f32)> = (0..=255u32).filter_map(|c| bounds(&[k, c, k]).map(|b| (c, b[2] - base[2]))).filter(|&(_, advance)| advance > 0.01).collect();
        unsafe { (api.FPDFPageObj_Destroy)(scratch) };
        // Each in its own place on the page, as the text page drops a run
        // drawn over another.
        let [width, _] = self.size();
        let columns = ((width / (4.0 * SIZE)) as usize).max(1);
        let mut probes = Vec::new();
        for (i, &(code, advance)) in advances.iter().enumerate() {
            let object = unsafe { (api.FPDFPageObj_CreateTextObj)(self.doc.handle, font, SIZE) };
            if object.is_null() {
                continue;
            }
            let at = sys::FS_MATRIX { a: 1.0, d: 1.0, e: (i % columns) as f32 * 4.0 * SIZE, f: (i / columns) as f32 * 1.5 * SIZE, ..Default::default() };
            unsafe {
                (api.FPDFText_SetCharcodes)(object, [k, code, k].as_ptr(), 3);
                (api.FPDFPageObj_SetMatrix)(object, &at);
                (api.FPDFPage_InsertObject)(self.handle.get(), object);
            }
            probes.push((object, code, advance));
        }
        if let Some(text) = self.text.take() {
            unsafe { (api.FPDFText_ClosePage)(text) };
        }
        let text_page = self.text_page();
        let read: Vec<Option<char>> = probes
            .iter()
            .map(|&(object, ..)| {
                let text = text_page.and_then(|tp| utf16_text(|buf, len| unsafe { (api.FPDFTextObj_GetText)(object, tp, buf.cast(), len) })).unwrap_or_default();
                // The text page may add a space of its own after the run;
                // the code with ink on either side is no whitespace.
                let text: Vec<char> = text.trim().chars().collect();
                // A subset's space reads as a no-break space: whatever
                // whitespace a code reads as, it is the space.
                (text.len() == 3).then(|| if text[1].is_whitespace() { ' ' } else { text[1] })
            })
            .collect();
        for &(object, ..) in &probes {
            unsafe {
                (api.FPDFPage_RemoveObject)(self.handle.get(), object);
                (api.FPDFPageObj_Destroy)(object);
            }
        }
        // The text page saw the probes: read again when next asked.
        if let Some(text) = self.text.take() {
            unsafe { (api.FPDFText_ClosePage)(text) };
        }
        let mut best: std::collections::HashMap<char, (bool, f32, u32)> = std::collections::HashMap::new();
        for (&(_, code, advance), c) in probes.iter().zip(read) {
            let Some(c) = c else { continue };
            let mut own = 0f32;
            let own = unsafe { (api.FPDFFont_GetGlyphWidth)(font, u32::from(c), SIZE, &mut own) } != 0 && (own - advance).abs() < 0.01;
            let entry = best.entry(c).or_insert((own, advance, code));
            if (own, advance) > (entry.0, entry.1) {
                *entry = (own, advance, code);
            }
        }
        out.extend(best.into_iter().map(|(c, (.., code))| (c, code)));
        out
    }

    /// The marked content `from` is in, put on `to`: a tagged document's
    /// tags point at their content through these marks' MCIDs, and a
    /// stand-in object without them dropped its run out of the structure
    /// tree. Numbers and strings are copied as they are; a name value
    /// becomes a string (PDFium sets no names).
    fn copy_marks(&self, from: sys::FPDF_PAGEOBJECT, to: sys::FPDF_PAGEOBJECT) {
        let api = self.api();
        let doc = self.doc.handle;
        let count = unsafe { (api.FPDFPageObj_CountMarks)(from) };
        for m in 0..count.max(0) as c_ulong {
            let mark = unsafe { (api.FPDFPageObj_GetMark)(from, m) };
            if mark.is_null() {
                continue;
            }
            let Some(name) = out_text(|buf, len, out| unsafe { (api.FPDFPageObjMark_GetName)(mark, buf.cast(), len, out) }) else { continue };
            let Ok(name) = CString::new(name) else { continue };
            let copy = unsafe { (api.FPDFPageObj_AddMark)(to, name.as_ptr()) };
            if copy.is_null() {
                continue;
            }
            let params = unsafe { (api.FPDFPageObjMark_CountParams)(mark) };
            for p in 0..params.max(0) as c_ulong {
                let Some(key) = out_text(|buf, len, out| unsafe { (api.FPDFPageObjMark_GetParamKey)(mark, p, buf.cast(), len, out) }) else { continue };
                let Ok(key) = CString::new(key) else { continue };
                let key = key.as_ptr();
                unsafe {
                    match (api.FPDFPageObjMark_GetParamValueType)(mark, key) {
                        sys::FPDF_OBJECT_NUMBER => {
                            let mut value = 0f32;
                            (api.FPDFPageObjMark_GetParamFloatValue)(mark, key, &mut value);
                            if value.fract() == 0.0 && value.abs() < i32::MAX as f32 {
                                (api.FPDFPageObjMark_SetIntParam)(doc, to, copy, key, value as c_int);
                            } else {
                                (api.FPDFPageObjMark_SetFloatParam)(doc, to, copy, key, value);
                            }
                        }
                        sys::FPDF_OBJECT_STRING => {
                            let mut len: c_ulong = 0;
                            (api.FPDFPageObjMark_GetParamBlobValue)(mark, key, std::ptr::null_mut(), 0, &mut len);
                            let mut bytes = vec![0u8; len as usize];
                            if (api.FPDFPageObjMark_GetParamBlobValue)(mark, key, bytes.as_mut_ptr(), len, &mut len) != 0 {
                                bytes.truncate(len as usize);
                                (api.FPDFPageObjMark_SetBlobParam)(doc, to, copy, key, bytes.as_ptr(), bytes.len() as c_ulong);
                            }
                        }
                        sys::FPDF_OBJECT_NAME => {
                            if let Some(value) = out_text(|buf, len, out| (api.FPDFPageObjMark_GetParamStringValue)(mark, key, buf.cast(), len, out))
                                && let Ok(value) = CString::new(value)
                            {
                                (api.FPDFPageObjMark_SetStringParam)(doc, to, copy, key, value.as_ptr());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

}

impl Page {
    /// The picture of image object `index` as shown (its matrix, masks and
    /// colour keys applied): width, height and RGBA pixels, straight
    /// alpha.
    pub fn image_pixels(&self, index: usize) -> Result<(u32, u32, Vec<u8>), String> {
        let api = self.api();
        let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index as c_int) };
        if object.is_null() || unsafe { (api.FPDFPageObj_GetType)(object) } != sys::FPDF_PAGEOBJ_IMAGE {
            return Err("not an image object".into());
        }
        // Rendered at the image's own size, not the size it has on the
        // page (PDFium renders a point as a pixel): the object's matrix is
        // scaled to that many pixels for the render and put back after.
        let mut matrix = sys::FS_MATRIX::default();
        let had_matrix = unsafe { (api.FPDFPageObj_GetMatrix)(object, &mut matrix) } != 0;
        let (mut pw, mut ph) = (0u32, 0u32);
        let native = unsafe { (api.FPDFImageObj_GetImagePixelSize)(object, &mut pw, &mut ph) } != 0 && pw > 0 && ph > 0;
        let rescaled = had_matrix && native;
        if rescaled {
            const LIMIT: u32 = 8192;
            let across = (matrix.a * matrix.a + matrix.b * matrix.b).sqrt().max(1e-3);
            let down = (matrix.c * matrix.c + matrix.d * matrix.d).sqrt().max(1e-3);
            let (sx, sy) = (pw.min(LIMIT) as f32 / across, ph.min(LIMIT) as f32 / down);
            let scaled = sys::FS_MATRIX { a: matrix.a * sx, b: matrix.b * sx, c: matrix.c * sy, d: matrix.d * sy, e: matrix.e, f: matrix.f };
            unsafe { (api.FPDFPageObj_SetMatrix)(object, &scaled) };
        }
        let bitmap = unsafe { (api.FPDFImageObj_GetRenderedBitmap)(self.doc.handle, self.handle.get(), object) };
        if rescaled {
            unsafe { (api.FPDFPageObj_SetMatrix)(object, &matrix) };
        }
        if bitmap.is_null() {
            return Err("the picture could not be rendered".into());
        }
        let (width, height, stride, format) = unsafe {
            ((api.FPDFBitmap_GetWidth)(bitmap), (api.FPDFBitmap_GetHeight)(bitmap), (api.FPDFBitmap_GetStride)(bitmap), (api.FPDFBitmap_GetFormat)(bitmap))
        };
        let result = if width <= 0 || height <= 0 || stride <= 0 {
            Err("an empty picture".to_owned())
        } else {
            let (w, h, stride) = (width as usize, height as usize, stride as usize);
            // SAFETY: the buffer is the bitmap's, `stride` × `height` bytes, alive until destroyed below.
            let buffer = unsafe { std::slice::from_raw_parts((api.FPDFBitmap_GetBuffer)(bitmap) as *const u8, stride * h) };
            let mut rgba = Vec::with_capacity(w * h * 4);
            for row in buffer.chunks_exact(stride).take(h) {
                for x in 0..w {
                    match format {
                        sys::FPDFBitmap_BGRA => {
                            let p = &row[x * 4..x * 4 + 4];
                            rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                        }
                        sys::FPDFBitmap_BGRx => {
                            let p = &row[x * 4..x * 4 + 4];
                            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                        }
                        sys::FPDFBitmap_BGR => {
                            let p = &row[x * 3..x * 3 + 3];
                            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                        }
                        _ => {
                            let g = row[x];
                            rgba.extend_from_slice(&[g, g, g, 255]);
                        }
                    }
                }
            }
            Ok((w as u32, h as u32, rgba))
        };
        unsafe { (api.FPDFBitmap_Destroy)(bitmap) };
        result
    }
}

impl Document {
    /// Write the document (its edits included) to `path`, through a
    /// temporary file beside it: the target is replaced only once the
    /// whole file is written.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        let tmp = path.with_extension("pdf.metalpdf-tmp");
        let file = std::fs::File::create(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        // `raw` first in memory: PDFium hands the callback a pointer to it,
        // which is the pointer to the whole writer.
        #[repr(C)]
        struct Writer {
            raw: sys::FPDF_FILEWRITE,
            out: std::io::BufWriter<std::fs::File>,
            failed: bool,
        }
        unsafe extern "C" fn write_block(this: *mut sys::FPDF_FILEWRITE, data: *const c_void, size: c_ulong) -> c_int {
            // SAFETY: `this` is the `raw` field at the start of a `Writer`,
            // which outlives the save.
            let writer = unsafe { &mut *(this as *mut Writer) };
            let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) };
            if writer.out.write_all(bytes).is_err() {
                writer.failed = true;
                return 0;
            }
            1
        }
        let mut writer = Box::new(Writer {
            raw: sys::FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) },
            out: std::io::BufWriter::new(file),
            failed: false,
        });
        let ok = unsafe {
            (self.api().FPDF_SaveAsCopy)(
                self.inner.handle,
                &mut writer.raw,
                sys::FPDF_NO_INCREMENTAL | sys::FPDF_SUBSET_NEW_FONTS,
            )
        };
        let flushed = writer.out.flush().is_ok();
        let failed = writer.failed;
        drop(writer);
        if ok == 0 || failed || !flushed {
            let _ = std::fs::remove_file(&tmp);
            return Err("PDFium could not write the document".into());
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot replace {}: {e}", path.display())
        })
    }
}

/// A BGRx bitmap PDFium paints into, over pixels this side owns. The pixel
/// buffer must not move while PDFium holds the bitmap, hence the `Box`.
pub struct Bitmap {
    api: &'static Api,
    handle: sys::FPDF_BITMAP,
    pixels: Box<[u8]>,
    /// Read by the tests and `fill`.
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
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

    /// A bitmap over `bgra` pixels (`width * height * 4` bytes), with an
    /// alpha channel when `alpha`: a picture to put into an image object.
    pub fn from_bgra(api: &'static Api, width: u32, height: u32, bgra: Vec<u8>, alpha: bool) -> Option<Bitmap> {
        if width == 0 || height == 0 || bgra.len() != width as usize * height as usize * 4 || i32::try_from(bgra.len()).is_err() {
            return None;
        }
        let mut pixels = bgra.into_boxed_slice();
        let format = if alpha { sys::FPDFBitmap_BGRA } else { sys::FPDFBitmap_BGRx };
        let handle = unsafe {
            (api.FPDFBitmap_CreateEx)(width as c_int, height as c_int, format, pixels.as_mut_ptr().cast(), (width * 4) as c_int)
        };
        if handle.is_null() {
            return None;
        }
        Some(Bitmap { api, handle, pixels, width, height })
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

/// A PNG decoded to BGRA 8-bit: width, height, pixels, whether it has
/// transparency.
fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>, bool), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("not a PNG that can be read: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("the PNG is too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("the PNG could not be decoded: {e}"))?;
    let (w, h) = (info.width, info.height);
    let n = w as usize * h as usize;
    let mut bgra = Vec::with_capacity(n * 4);
    let alpha = matches!(info.color_type, png::ColorType::Rgba | png::ColorType::GrayscaleAlpha);
    match info.color_type {
        png::ColorType::Rgba => {
            for p in buf[..n * 4].as_chunks::<4>().0 {
                bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        png::ColorType::Rgb => {
            for p in buf[..n * 3].as_chunks::<3>().0 {
                bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in &buf[..n] {
                bgra.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in buf[..n * 2].as_chunks::<2>().0 {
                bgra.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        png::ColorType::Indexed => return Err("an indexed PNG was not expanded".into()),
    }
    Ok((w, h, bgra, alpha))
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

/// A UTF-16 string from a PDFium call that takes a buffer, its length in
/// bytes and where to write the length it needs (with the terminator).
fn out_text(get: impl Fn(*mut c_void, c_ulong, *mut c_ulong) -> sys::FPDF_BOOL) -> Option<String> {
    utf16_text(|buf, len| {
        let mut needed: c_ulong = 0;
        if get(buf, len, &mut needed) == 0 { 0 } else { needed }
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A two-page PDF written by hand: Helvetica text on an A4 page, and a
    /// landscape page by /Rotate 90 with a link to the first one and a
    /// bookmark each.
    #[test]
    fn read_back_text_is_compared_loosely() {
        assert!(same_text("Bold\u{a0}Bold", "Bold Bold"));
        assert!(same_text("элемент\u{2}", "элемент-"));
        assert!(same_text(" 23.02.2026  11:32:33\r\n", "23.02.2026 11:32:33"));
        assert!(!same_text("BoldBold", "Bold Bold"));
        assert!(!same_text("Bold Bold", "Bold Bolder"));
    }

    pub fn sample_pdf() -> Vec<u8> {
        write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
            stream("BT /F1 24 Tf 72 770 Td (Hello MetalPDF) Tj ET BT /F1 12 Tf 72 740 Td (Second line of text) Tj ET"),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Rotate 90 /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> /XObject << /Im1 12 0 R >> >> /Annots [11 0 R] >>".into(),
            stream("BT /F1 18 Tf 100 700 Td (Page two) Tj ET q 100 0 0 50 300 400 cm /Im1 Do Q"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            "<< /Type /Outlines /First 9 0 R /Last 10 0 R /Count 2 >>".into(),
            "<< /Title (First) /Parent 8 0 R /Next 10 0 R /Dest [3 0 R /XYZ 0 842 0] >>".into(),
            "<< /Title (Second) /Parent 8 0 R /Prev 9 0 R /Dest [5 0 R /Fit] >>".into(),
            "<< /Type /Annot /Subtype /Link /Rect [100 690 200 720] /Border [0 0 0] /Dest [3 0 R /XYZ 0 700 0] >>".into(),
            // A 2 × 2 picture, its samples ASCII letters (the builder works in text).
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>\nstream\nABCDEFGHIJKL\nendstream".into(),
        ])
    }

    /// A PDF of `objects` (numbered from 1, the first the catalog), with
    /// its cross-reference table and trailer.
    fn write_pdf(objects: Vec<String>) -> Vec<u8> {
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

    /// The text of a run replaced in place (a standard font), the page
    /// regenerated, the document saved and read again.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn text_is_replaced_and_the_document_saved() {
        let api = api();
        let path = sample_file("edit");
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let objects = page.text_objects();
        assert_eq!(objects.len(), 2, "{objects:?}");
        assert_eq!(objects[0].text, "Hello MetalPDF");
        assert_eq!(objects[0].font, "Helvetica");
        assert_eq!(objects[0].font_size, 24.0);
        assert!(!objects[0].subset);
        assert!(objects[0].bounds[0] >= 71.0 && objects[0].bounds[1] < 770.0 && objects[0].bounds[3] > 770.0, "{:?}", objects[0].bounds);
        assert_eq!(page.set_text(objects[0].index, "Edited text", &mut |_, _| None).unwrap(), Replaced::SameFont);
        assert_eq!(page.text_objects()[0].text, "Edited text");
        assert_eq!(page.find("Edited", true, false).len(), 1);
        assert!(page.find("Hello", true, false).is_empty());
        let saved = std::env::temp_dir().join(format!("metalpdf_saved_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(page);
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        let page = again.load_page(0).unwrap();
        assert_eq!(page.text(0, 11), "Edited text");
        let mut bitmap = Bitmap::new(api, 300, 424).unwrap();
        page.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: 300, size_y: 424, rotate: 0 }, sys::FPDF_ANNOT);
        assert!(bitmap.pixels().chunks(4).filter(|p| p[0] < 128).count() > 100);
        drop(page);
        drop(again);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(saved).unwrap();
    }

    /// The picture on the sample's second page read back as shown: RGBA
    /// pixels, not all alike.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn a_pictures_pixels_are_read() {
        let api = api();
        let path = sample_file("pixels");
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(1).unwrap();
        let images = page.image_objects();
        assert_eq!(images.len(), 1, "{images:?}");
        let (w, h, rgba) = page.image_pixels(images[0].index).unwrap();
        println!("picture {w}x{h}");
        // The image's own size (2 × 2), not its 100 × 50 points on the page.
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        let pixels = rgba.as_chunks::<4>().0;
        assert!(pixels.iter().any(|p| *p != pixels[0]), "all one colour");
        assert!(page.image_pixels(0).is_err(), "object 0 is text");
        drop(page);
        drop(doc);
        std::fs::remove_file(path).unwrap();
    }

    /// The picture on the sample's second page moved and resized, replaced
    /// by a PNG written here (fitted with its proportions), removed, and
    /// the document saved without it.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn images_are_placed_replaced_and_removed() {
        let api = api();
        let path = sample_file("images");
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(1).unwrap();
        let objects = page.objects();
        assert_eq!(objects.images.len(), 1, "{objects:?}");
        let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.6);
        assert!(near(objects.images[0].bounds, [300.0, 400.0, 400.0, 450.0]), "{:?}", objects.images[0].bounds);
        let index = objects.images[0].index;
        page.place_object(index, [310.0, 410.0, 360.0, 435.0]).unwrap();
        let moved = page.image_objects();
        assert!(near(moved[0].bounds, [310.0, 410.0, 360.0, 435.0]), "{:?}", moved[0].bounds);
        // A 3 × 2 PNG: fitted into the 50 × 25 box as 37.5 × 25, centred.
        let png_path = std::env::temp_dir().join(format!("metalpdf_pic_{}.png", std::process::id()));
        crate::bench::write_rgba_png(&png_path, &[200, 30, 30, 255].repeat(6), 3, 2).unwrap();
        page.replace_image(index, &png_path).unwrap();
        let replaced = page.image_objects();
        assert!(near(replaced[0].bounds, [316.25, 410.0, 353.75, 435.0]), "{:?}", replaced[0].bounds);
        // The picture renders where it was put: a reddish pixel in the box.
        let mut bitmap = Bitmap::new(api, 842, 595).unwrap();
        page.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: 842, size_y: 595, rotate: 0 }, sys::FPDF_ANNOT);
        // /Rotate 90: page point (x, y) is drawn at device (842 - y... ) — just
        // look for red anywhere.
        let reddish = bitmap.pixels().chunks(4).filter(|p| p[2] > 150 && p[1] < 80 && p[0] < 80).count();
        assert!(reddish > 100, "{reddish} red pixels");
        page.remove_object(index).unwrap();
        assert!(page.image_objects().is_empty());
        assert!(page.remove_object(index).is_err(), "nothing there any more");
        let saved = std::env::temp_dir().join(format!("metalpdf_noimage_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(page);
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        assert!(again.load_page(1).unwrap().image_objects().is_empty());
        drop(again);
        for f in [path, png_path, saved] {
            std::fs::remove_file(f).unwrap();
        }
    }

    /// Pages removed, a blank one put in, another document's pages imported,
    /// and the result saved and read again.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pages_are_deleted_inserted_and_imported() {
        let api = api();
        let path = sample_file("pages");
        let other = sample_file("pages_other");
        let doc = Document::open(api, &path, None).unwrap();
        doc.delete_page(1).unwrap();
        assert_eq!(doc.page_count(), 1);
        assert!(doc.delete_page(5).is_err());
        doc.insert_blank_page(1, [400.0, 300.0]).unwrap();
        assert_eq!(doc.page_count(), 2);
        assert_eq!(doc.page_size(1), Some([400.0, 300.0]));
        let source = Document::open(api, &other, None).unwrap();
        assert_eq!(doc.import_pages(&source, 0).unwrap(), 2);
        assert_eq!(doc.page_count(), 4);
        // The imported pages come first: the sample's text page, then its
        // turned page; the original first page is third.
        assert_eq!(doc.page_size(1), Some([842.0, 595.0]));
        assert_eq!(doc.page_size(2), Some([595.0, 842.0]));
        assert_eq!(doc.load_page(0).unwrap().text(0, 5), "Hello");
        let saved = std::env::temp_dir().join(format!("metalpdf_pages_saved_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(source);
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        assert_eq!(again.page_count(), 4);
        assert_eq!(again.page_size(3), Some([400.0, 300.0]));
        drop(again);
        for f in [path, other, saved] {
            std::fs::remove_file(f).unwrap();
        }
    }

    /// A run in a font embedded as a subset (Arial Bold, put in by the
    /// test and subset by PDFium's save) cannot show new text: the
    /// replacement comes in a stand-in font that keeps the style, bold
    /// here.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn replaced_text_keeps_bold_through_the_stand_in_font() {
        let api = api();
        let src = sample_file("bold-src");
        let doc = Document::open(api, &src, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let bold = std::fs::read(r"C:\Windows\Fonts\arialbd.ttf").unwrap();
        unsafe {
            let font = (api.FPDFText_LoadFont)(doc.inner.handle, bold.as_ptr(), bold.len() as u32, sys::FPDF_FONT_TRUETYPE, 1);
            assert!(!font.is_null());
            let obj = (api.FPDFPageObj_CreateTextObj)(doc.inner.handle, font, 14.0);
            let wide: Vec<u16> = "Bold run".encode_utf16().chain(std::iter::once(0)).collect();
            (api.FPDFText_SetText)(obj, wide.as_ptr());
            let m = sys::FS_MATRIX { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 72.0, f: 500.0 };
            (api.FPDFPageObj_SetMatrix)(obj, &m);
            (api.FPDFPage_InsertObject)(page.handle.get(), obj);
            assert_ne!((api.FPDFPage_GenerateContent)(page.handle.get()), 0);
            (api.FPDFFont_Close)(font);
        }
        let subset_file = std::env::temp_dir().join(format!("metalpdf_bold_subset_{}.pdf", std::process::id()));
        doc.save_to(&subset_file).unwrap();
        drop(page);
        drop(doc);
        let doc = Document::open(api, &subset_file, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let objects = page.text_objects();
        // PDFium reads the subset font's space back as a no-break space.
        let plain = |o: &TextObject| o.text.replace('\u{a0}', " ").trim().to_owned();
        let run = objects.iter().find(|o| plain(o) == "Bold run").expect("the bold run");
        println!("the bold run's font: {} (subset {})", run.font, run.subset);
        assert!(run.subset, "PDFium's save subsets a new font");
        // Text the subset has every glyph of keeps the font and its look.
        let old_width = run.bounds[2] - run.bounds[0];
        assert_eq!(page.set_text(run.index, "Bold Bold", &mut |_, _| None).unwrap(), Replaced::SameFont);
        let objects = page.text_objects();
        let run = objects.iter().find(|o| plain(o) == "Bold Bold").expect("the run kept its font");
        assert!(run.subset && run.font.contains("Bold"), "{run:?}");
        // Text with glyphs it lacks comes in the stand-in, fitted to the
        // old width a character.
        let mut fonts = crate::fonts::Fonts::default();
        let mut asked = Vec::new();
        let replaced = page
            .set_text(run.index, "Bolder still", &mut |name, flags| {
                asked.push((name.to_owned(), flags));
                fonts.get(name, flags)
            })
            .unwrap();
        assert_eq!(replaced, Replaced::FallbackFont);
        let objects = page.text_objects();
        let new = objects.iter().find(|o| plain(o) == "Bolder still").expect("the replaced run");
        let per_char_old = old_width / "Bold run".len() as f32;
        let per_char_new = (new.bounds[2] - new.bounds[0]) / "Bolder still".len() as f32;
        println!("width a character: {per_char_old:.2} before, {per_char_new:.2} in the stand-in");
        assert!((per_char_new / per_char_old - 1.0).abs() < 0.08, "{per_char_old} vs {per_char_new}");
        assert_eq!(crate::fonts::file_of(crate::fonts::style_of(&asked[0].0, asked[0].1)), "arialbd.ttf", "{asked:?}");
        let objects = page.text_objects();
        let new = objects.iter().find(|o| plain(o) == "Bolder still").expect("the replaced run");
        println!("the replacement's font: {}", new.font);
        assert!(new.font.to_ascii_lowercase().contains("bold"), "the stand-in is bold: {}", new.font);
        drop(page);
        drop(doc);
        std::fs::remove_file(src).ok();
        std::fs::remove_file(subset_file).ok();
    }

    /// On a real document (`METALPDF_SAMPLE`): the first page with text
    /// objects, its first run replaced, in place or through a fallback
    /// font when the run's font is an embedded subset; the replaced run
    /// must leave ink where it was. With `--nocapture` it prints what it
    /// found.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_document_takes_an_edit() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        let mut fonts = crate::fonts::Fonts::default();
        let mut tried = 0;
        for index in 0..doc.page_count().min(20) {
            let page = doc.load_page(index).unwrap();
            let objects = page.text_objects();
            let Some(object) = objects.iter().find(|o| o.text.trim().chars().count() >= 3) else { continue };
            println!("page {}: {} text objects; editing {:?} ({}, {} pt, subset {})", index + 1, objects.len(), object.text, object.font, object.font_size, object.subset);
            let [w, h] = page.size();
            let scale = 2.0;
            let (bw, bh) = ((w * scale) as u32, (h * scale) as u32);
            let at = Placement { start_x: 0, start_y: 0, size_x: bw as i32, size_y: bh as i32, rotate: 0 };
            // The ink within the run's box before and after.
            let ink = |bitmap: &Bitmap| {
                let b = object.bounds;
                let (x0, x1) = ((b[0] * scale) as usize, (b[2] * scale) as usize);
                let (y0, y1) = (((h - b[3]) * scale) as usize, ((h - b[1]) * scale) as usize);
                let mut dark = 0;
                for y in y0..y1.min(bh as usize) {
                    let row = &bitmap.pixels()[y * bw as usize * 4..(y + 1) * bw as usize * 4];
                    dark += row.chunks(4).skip(x0).take(x1.saturating_sub(x0)).filter(|p| p[0] < 160).count();
                }
                dark
            };
            let mut before = Bitmap::new(api, bw, bh).unwrap();
            page.render(&mut before, at, sys::FPDF_ANNOT);
            let replaced = page
                .set_text(object.index, "Replaced by MetalPDF 123", &mut |name, flags| {
                    println!("stand-in for {name} (flags {flags:#x}): {}", crate::fonts::file_of(crate::fonts::style_of(name, flags)));
                    fonts.get(name, flags)
                })
                .unwrap();
            let mut after = Bitmap::new(api, bw, bh).unwrap();
            page.render(&mut after, at, sys::FPDF_ANNOT);
            let (a, b) = (ink(&before), ink(&after));
            println!("  {replaced:?}: ink {a} before, {b} after");
            assert!(b > a / 4, "the new text left too little ink ({b} against {a})");
            tried += 1;
            if tried >= 3 {
                break;
            }
        }
        assert!(tried > 0, "no page with text objects among the first 20");
    }

    /// On a real tagged document (`METALPDF_SAMPLE`): page 31's first runs
    /// replaced, in their own font (a Latin text) and in a stand-in (a
    /// Cyrillic one a subset has no glyphs for), saved and read back:
    /// every object keeps the marked content its tags point into.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_tagged_documents_marks_survive_an_edit() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        let mcids = |page: &Page| {
            let n = unsafe { (api.FPDFPage_CountObjects)(page.handle.get()) };
            let ids: Vec<i32> = (0..n).map(|i| unsafe { (api.FPDFPageObj_GetMarkedContentID)((api.FPDFPage_GetObject)(page.handle.get(), i)) }).collect();
            ids
        };
        let tagged = |doc: &Document| unsafe { (api.FPDFCatalog_IsTagged)(doc.inner.handle) } != 0;
        if !tagged(&doc) || doc.page_count() <= 30 {
            println!("not a tagged document of more than 30 pages");
            return;
        }
        let mut fonts = crate::fonts::Fonts::default();
        let index = 30;
        let page = doc.load_page(index).unwrap();
        let before = mcids(&page);
        let objects = page.text_objects();
        let runs: Vec<usize> = objects.iter().filter(|o| o.text.trim().chars().count() >= 3).take(4).map(|o| o.index).collect();
        for (n, &i) in runs.iter().enumerate() {
            let text = if n < 2 { "Replaced by MetalPDF" } else { "Замена текста" };
            let how = page.set_text(i, text, &mut |name, flags| fonts.get(name, flags)).unwrap();
            println!("object {i} (MCID {}): {how:?}", before[i]);
        }
        assert_eq!(mcids(&page), before, "the edited page");
        drop(page);
        let out = std::env::temp_dir().join("metalpdf_tagged_edit.pdf");
        doc.save_to(&out).unwrap();
        let saved = Document::open(api, &out, None).unwrap();
        assert!(tagged(&saved));
        let page = saved.load_page(index).unwrap();
        assert_eq!(mcids(&page), before, "the page saved and read back");
        println!("{} objects, {} marked: kept", before.len(), before.iter().filter(|&&i| i >= 0).count());
        drop(page);
        drop(saved);
        std::fs::remove_file(&out).ok();
    }

    /// On the OpenStax book (`METALPDF_SAMPLE`): the preface's lines, cut
    /// at every kerned pair, read as lines; one made longer in its own
    /// font, another set in a stand-in (Cyrillic), each read back as typed
    /// with the objects after the change moved, none overlapping.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_line_takes_an_edit() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        let Some(page) = (0..doc.page_count().min(40)).filter_map(|i| doc.load_page(i)).find(|p| p.text(0, p.char_count()).contains("Students are provided answers")) else {
            println!("not the OpenStax book");
            return;
        };
        let mut fonts = crate::fonts::Fonts::default();
        let line_from = |page: &Page, start: &str| crate::textline::lines(&page.text_objects()).into_iter().find(|l| l.text.starts_with(start)).unwrap();
        for (start, from, to) in [("Students are provided", "odd-numbered", "odd-numbered and more"), ("Manual on the Student", "Manual", "Руководство")] {
            let line = line_from(&page, start);
            println!("{} objects: {:?}", line.fragments.len(), line.text);
            let wanted = line.text.replacen(from, to, 1);
            let edit = line.edit(&wanted).unwrap();
            println!("  merging {:?} into {:?}, {} after", edit.objects, edit.text, edit.followers.len());
            let how = page.set_line(&edit.objects, &edit.text, &edit.followers, &mut |name, flags| fonts.get(name, flags)).unwrap();
            let after = line_from(&page, &wanted[..8]);
            println!("  {how:?}: {} objects: {:?}", after.fragments.len(), after.text);
            assert_eq!(after.text, wanted);
            assert_eq!(after.fragments.len(), line.fragments.len() - edit.objects.len() + 1);
            let objects = page.text_objects();
            let boxes: Vec<[f32; 4]> = after.fragments.iter().map(|f| objects.iter().find(|o| o.index == f.object).unwrap().bounds).collect();
            for pair in boxes.windows(2) {
                assert!(pair[1][0] > pair[0][2] - 0.5, "objects overlap: {pair:?}");
            }
        }
    }

    /// On a real document (`METALPDF_SAMPLE`) whose lines end in hyphens
    /// (PDFium reads them as U+0002): the first such line with its first
    /// word cut short stays in its own font and keeps its hyphen.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_line_ending_in_a_hyphen_keeps_it() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let doc = Document::open(api(), Path::new(&sample), None).unwrap();
        let mut fonts = crate::fonts::Fonts::default();
        for index in 0..doc.page_count().min(60) {
            let page = doc.load_page(index).unwrap();
            let lines = crate::textline::lines(&page.text_objects());
            let Some(line) = lines.iter().find(|l| l.raw.ends_with('-') && l.text.chars().take_while(|c| c.is_alphabetic()).count() >= 3) else { continue };
            // The first letter dropped: no character the font may lack.
            let wanted: String = line.text.chars().skip(1).collect();
            let edit = line.edit(&wanted).unwrap();
            let how = page.set_line(&edit.objects, &edit.text, &edit.followers, &mut |name, flags| fonts.get(name, flags)).unwrap();
            let after = crate::textline::lines(&page.text_objects()).into_iter().find(|l| l.text == wanted);
            println!("page {}: {:?} -> {how:?}, read back {:?}", index + 1, line.text, after.as_ref().map(|l| &l.text));
            assert_eq!(how, Replaced::SameFont);
            assert!(after.is_some(), "the line reads back as typed, hyphen and all");
            return;
        }
        println!("no line ends in a hyphen among the first 60 pages");
    }

    /// On a real document (`METALPDF_SAMPLE`): the page numbering read off
    /// the pages as the engine does it (`engine::numbering_shift`); prints
    /// the shift found. The OpenStax book's page 1 is the file's 17th.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_documents_numbering_is_read_off_its_pages() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        let count = doc.page_count();
        let samples = 24.min(count);
        let started = std::time::Instant::now();
        let mut texts = Vec::new();
        for s in 0..samples {
            let index = if samples == count { s } else { s * count / samples + count / (2 * samples) };
            let page = doc.load_page(index).unwrap();
            let n = page.char_count();
            let text = if n > 400 { format!("{}\n{}", page.text(0, 200), page.text(n - 200, 200)) } else { page.text(0, n) };
            texts.push((index, text));
        }
        let shift = crate::engine::numbering_shift(&texts);
        println!("numbering shift {shift:?} from {} pages in {:?}", texts.len(), started.elapsed());
        if sample.to_string_lossy().contains("anatomy-and-physiology") {
            assert_eq!(shift, Some(16));
        }
    }

    /// On a real document (`METALPDF_SAMPLE`): its first page deleted and
    /// a blank one put in at the end, saved, read back; prints what the
    /// document is (`edit_obstacles`) and the timings.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_document_takes_page_operations() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        println!("obstacles: {:?}", doc.edit_obstacles());
        let count = doc.page_count();
        assert!(count > 1, "a document of one page");
        let second = doc.page_size(1).unwrap();
        let started = std::time::Instant::now();
        doc.delete_page(0).unwrap();
        doc.insert_blank_page(count - 1, [400.0, 300.0]).unwrap();
        println!("delete and insert: {:?}", started.elapsed());
        assert_eq!(doc.page_count(), count);
        assert_eq!(doc.page_size(0).unwrap(), second);
        let out = std::env::temp_dir().join("metalpdf_real_pages.pdf");
        let started = std::time::Instant::now();
        doc.save_to(&out).unwrap();
        println!("saved {} bytes in {:?}", std::fs::metadata(&out).unwrap().len(), started.elapsed());
        let saved = Document::open(api, &out, None).unwrap();
        assert_eq!(saved.page_count(), count);
        assert_eq!(saved.page_size(0).unwrap(), second);
        assert_eq!(saved.page_size(count - 1).unwrap(), [400.0, 300.0]);
        drop(saved);
        std::fs::remove_file(&out).ok();
    }

    /// On a real document (`METALPDF_SAMPLE`): the first page rendered
    /// plainly, progressively without a pause and progressively pausing at
    /// every chance, all three alike; prints how dark each came out.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_page_renders_alike_progressively() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        let page = doc.load_page(0).unwrap();
        let [w, h] = page.size();
        let (bw, bh) = ((w * 1.5) as u32, (h * 1.5) as u32);
        let at = Placement { start_x: 0, start_y: 0, size_x: bw as i32, size_y: bh as i32, rotate: 0 };
        let dark = |b: &Bitmap| b.pixels().chunks(4).filter(|p| p[0] < 64 && p[1] < 64 && p[2] < 64).count();
        let mut plain = Bitmap::new(api, bw, bh).unwrap();
        page.render(&mut plain, at, sys::FPDF_ANNOT);
        let mut never = Bitmap::new(api, bw, bh).unwrap();
        let mut no_pause = Pause::new(|| false);
        let status = page.render_start(&mut never, at, sys::FPDF_ANNOT, None, &mut no_pause);
        assert_eq!(status, RenderStatus::Done);
        let mut always = Bitmap::new(api, bw, bh).unwrap();
        let mut pause = Pause::new(|| true);
        let mut status = page.render_start(&mut always, at, sys::FPDF_ANNOT, None, &mut pause);
        let mut rounds = 0;
        while status == RenderStatus::Paused {
            status = page.render_continue(&mut always, at, sys::FPDF_ANNOT, &mut pause);
            rounds += 1;
        }
        println!("plain dark {}, progressive dark {}, paused ({rounds} rounds) dark {}", dark(&plain), dark(&never), dark(&always));
        assert_eq!(status, RenderStatus::Done);
        assert_same_picture(plain.pixels(), never.pixels(), "progressive without pauses");
        assert_same_picture(plain.pixels(), always.pixels(), "progressive with pauses");
    }

    /// On a real document (`METALPDF_SAMPLE`) with non-ASCII text: the
    /// search without "match case" finds a capitalised word typed in
    /// lower case, and with "whole words" finds the word itself.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_documents_search_folds_case_beyond_ascii() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let doc = Document::open(api(), Path::new(&sample), None).unwrap();
        let mut tried = 0;
        for index in 0..doc.page_count().min(10) {
            let page = doc.load_page(index).unwrap();
            let text = page.text(0, page.char_count());
            let Some(word) = text.split(|c: char| !c.is_alphabetic()).find(|w| w.chars().count() >= 4 && w.chars().next().is_some_and(|c| c.is_uppercase() && !c.is_ascii())) else { continue };
            let lower = word.to_lowercase();
            let (exact, folded, whole) = (page.find(word, true, false).len(), page.find(&lower, false, false).len(), page.find(word, false, true).len());
            println!("page {}: {word:?} {exact} exact; {lower:?} folded {folded}; whole words {whole}", index + 1);
            assert!(exact >= 1);
            assert!(folded >= exact, "the search does not fold the case of {word:?}");
            assert!(whole >= 1, "whole words miss {word:?}");
            tried += 1;
            if tried >= 3 {
                break;
            }
        }
        assert!(tried > 0, "no capitalised non-ASCII word on the first pages");
    }

    /// What `FPDFPage_GenerateContent` leaves out, so that the editing
    /// refuses such pages: text in a Type 3 font (`Page::has_type3_text`,
    /// on a page whose glyphs are content streams). A shading object
    /// (`sh`) is checked too, for the record. Should PDFium one day write
    /// the text back, this test says so and the refusal can go.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn generate_content_drops_type3_text() {
        let api = api();
        let pdf = write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /T3 6 0 R >> /Shading << /Sh1 8 0 R >> >> >>".into(),
            stream("BT /F1 24 Tf 20 250 Td (Hello) Tj ET BT /T3 24 Tf 20 150 Td (ab) Tj ET q 0 0 300 100 re W n /Sh1 sh Q q 20 0 0 20 200 200 cm BI /W 2 /H 2 /CS /RGB /BPC 8 ID ABCDEFGHIJKL EI Q"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 750 750] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /square 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /square /square] >> /FirstChar 97 /LastChar 98 /Widths [1000 1000] /Resources << >> >>".into(),
            stream("1000 0 0 0 750 750 d1 0 0 375 375 re f"),
            "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 300 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> /Extend [true true] >>".into(),
        ]);
        let path = std::env::temp_dir().join(format!("metalpdf_type3_{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        let types = |page: &Page| -> Vec<c_int> {
            let n = unsafe { (api.FPDFPage_CountObjects)(page.handle.get()) };
            (0..n).map(|i| unsafe { (api.FPDFPageObj_GetType)((api.FPDFPage_GetObject)(page.handle.get(), i)) }).collect()
        };
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        assert_eq!(types(&page), vec![sys::FPDF_PAGEOBJ_TEXT, sys::FPDF_PAGEOBJ_TEXT, sys::FPDF_PAGEOBJ_SHADING, sys::FPDF_PAGEOBJ_IMAGE]);
        assert!(page.has_type3_text());
        // The sample's own page has none.
        let plain = Document::open(api, &sample_file("plain"), None).unwrap();
        assert!(!plain.load_page(0).unwrap().has_type3_text());
        drop(plain);
        // The Helvetica run edited: the content written anew comes up
        // short, and the edit is refused.
        let refused = page.set_text(0, "Edited", &mut |_, _| None).unwrap_err();
        println!("refused: {refused}");
        assert!(refused.contains("1 text") && refused.contains("1 shadings"), "{refused}");
        assert!(refused.contains("1 pictures"), "PDFium wrote the inline image back: {refused}");
        // Saved regardless (as the engine never does): the page has the
        // Helvetica run alone.
        let saved = std::env::temp_dir().join(format!("metalpdf_type3_saved_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(page);
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        let page = again.load_page(0).unwrap();
        let after = types(&page);
        println!("objects after the save: {after:?}; text {:?}", page.text(0, page.char_count()));
        assert_eq!(after, vec![sys::FPDF_PAGEOBJ_TEXT], "PDFium writes more back now: the checks in Page::content_changed may be eased");
        drop(page);
        drop(again);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(saved).unwrap();
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

    /// On a real document (`METALPDF_SAMPLE`): the first line of page
    /// `METALPDF_PAGE` (1-based, 1 by default) edited, the document saved
    /// and read back: every page's text is as it was, but for that line.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_SAMPLE"]
    fn a_real_documents_text_survives_a_save() {
        let Some(sample) = std::env::var_os("METALPDF_SAMPLE") else { return };
        let index = std::env::var("METALPDF_PAGE").ok().and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).saturating_sub(1);
        let api = api();
        let doc = Document::open(api, Path::new(&sample), None).unwrap();
        let texts = |doc: &Document| -> Vec<String> { (0..doc.page_count()).map(|i| doc.load_page(i).map_or_else(String::new, |p| p.text(0, p.char_count()))).collect() };
        let before = texts(&doc);
        let mut fonts = crate::fonts::Fonts::default();
        let (wanted, old) = {
            let page = doc.load_page(index).unwrap();
            println!("page {}: Type 3 text {}", index + 1, page.has_type3_text());
            let lines = crate::textline::lines(&page.text_objects());
            let line = lines.iter().find(|l| l.text.trim().chars().count() >= 3).expect("a line of text on the page");
            let wanted: String = line.text.chars().skip(1).collect();
            let edit = line.edit(&wanted).unwrap();
            let how = match page.set_line(&edit.objects, &edit.text, &edit.followers, &mut |name, flags| fonts.get(name, flags)) {
                Ok(how) => how,
                Err(refused) => {
                    // The page's content cannot be written back whole: the
                    // engine refuses such an edit, so nothing is lost.
                    println!("page {}: the edit is refused: {refused}", index + 1);
                    return;
                }
            };
            println!("page {}: {:?} -> {how:?}; {} characters in memory after the edit, {} before", index + 1, line.text, page.char_count(), before[index].chars().count());
            (wanted, line.text.clone())
        };
        let saved = std::env::temp_dir().join(format!("metalpdf_survives_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        let after = texts(&again);
        drop(again);
        // `METALPDF_KEEP` leaves the saved file in `%TEMP%` to look at.
        if std::env::var_os("METALPDF_KEEP").is_none() {
            std::fs::remove_file(&saved).unwrap();
        } else {
            println!("saved as {}", saved.display());
        }
        assert_eq!(before.len(), after.len());
        // As the editor shows it: the font's hyphen and the Unicode
        // spaces plain, a run of whitespace one space.
        let plain = |s: &str| {
            let plain: String = s.chars().map(|c| match c {
                '\u{2}' | '\u{ad}' | '\u{2010}' | '\u{2011}' => '-',
                c if c.is_whitespace() => ' ',
                c => c,
            }).collect();
            plain.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let (wanted, old) = (plain(&wanted), plain(&old));
        let mut lost = 0;
        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
            let (nb, na) = (b.chars().count(), a.chars().count());
            if i == index {
                let (a, b) = (plain(a), plain(b));
                assert!(a.contains(&wanted), "page {}: the edited line is not there", i + 1);
                assert!(!a.contains(&old) || b.matches(&old).count() > 1, "page {}: the old line is still there", i + 1);
            }
            if na + 2 < nb || (i != index && a != b) {
                lost += 1;
                let missing: Vec<&str> = b.split_whitespace().filter(|w| !a.contains(w)).take(12).collect();
                println!("page {}: {nb} characters before, {na} after; missing words: {missing:?}", i + 1);
            }
        }
        assert_eq!(lost, 0, "{lost} pages lost text");
    }

    /// A font whose ToUnicode gives U+0020 two codes, its space (30, with
    /// a width) and code 32 (no width), as a book's PetersburgC does:
    /// PDFium encodes a typed space with code 32 and the edited words ran
    /// together; the space's own code is written, and kept on saving.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn a_space_keeps_its_width_when_the_font_gives_it_two_codes() {
        let api = api();
        let widths: Vec<&str> = ["278", "0", "0"].into_iter().chain((33..=122).map(|_| "556")).collect();
        let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Adobe-Identity-UCS def /CMapType 2 def \
            1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfrange <21> <7A> <0021> endbfrange \
            2 beginbfchar <1E> <0020> <0020> <0020> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
        let pdf = write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("BT /F1 24 Tf 72 700 Td (Hello\\036world) Tj ET"),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 30 /LastChar 122 /Widths [{}] /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [30 /space] >> /ToUnicode 6 0 R >>",
                widths.join(" ")
            ),
            stream(cmap),
        ]);
        let path = std::env::temp_dir().join(format!("metalpdf_two_spaces_{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let object = page.text_objects().remove(0);
        assert_eq!(object.text, "Hello world");
        // PDFium's own code for a space has no width in this font.
        let font = unsafe { (api.FPDFTextObj_GetFont)((api.FPDFPage_GetObject)(page.handle.get(), object.index as c_int)) };
        let mut width = -1f32;
        unsafe { (api.FPDFFont_GetGlyphWidth)(font, 0x20, 24.0, &mut width) };
        assert_eq!(width, 0.0);
        assert_eq!(page.set_text(object.index, "Hello big world", &mut |_, _| None).unwrap(), Replaced::SameFont);
        // 12 letters and 2 spaces before the last letter, 24 pt: 173.5
        // points to its origin, 160.1 with spaces of no width; its ink
        // reaches about 12 points further.
        let reach = |page: &Page| {
            let edited = page.text_objects().remove(0);
            assert_eq!(edited.text, "Hello big world");
            edited.bounds[2] - edited.origin[0]
        };
        assert!(reach(&page) > 180.0, "{}", reach(&page));
        drop(page);
        let saved = std::env::temp_dir().join(format!("metalpdf_two_spaces_saved_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        assert!(reach(&again.load_page(0).unwrap()) > 180.0);
        drop(again);
        std::fs::remove_file(path).ok();
        std::fs::remove_file(saved).ok();
    }

    /// The writer leaves out `Tc` and `Tw`: a run set with them came out
    /// with its characters closer or further by the spacing each when
    /// another run of the page was edited (the whole content is written
    /// anew). `undo_drift` puts the characters back where they were, in
    /// the parsed page and in the file.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn character_spacing_survives_an_edit() {
        let api = api();
        let pdf = write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("BT /F1 24 Tf 72 700 Td (Hello) Tj ET BT /F1 24 Tf 3 Tc 5 Tw 72 650 Td (Sp aced) Tj ET BT /F1 1 Tf 3 Tc 24 0 0 24 72 600 Tm (Tm run) Tj ET"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ]);
        let path = std::env::temp_dir().join(format!("metalpdf_tc_{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        // The advances between each run's letters, in points (a space in
        // a wide gap reads as made up by the text page once the run is
        // written as a `TJ` array, so the spaces are left out).
        let advances = |page: &Page| -> Vec<Vec<f32>> {
            let text_page = page.text_page().unwrap();
            let count = unsafe { (api.FPDFPage_CountObjects)(page.handle.get()) };
            (0..count)
                .map(|k| {
                    let object = unsafe { (api.FPDFPage_GetObject)(page.handle.get(), k) };
                    let xs: Vec<f64> = (0..unsafe { (api.FPDFText_CountChars)(text_page) })
                        .filter(|&i| unsafe { (api.FPDFText_GetTextObject)(text_page, i) } == object && !char::from_u32(unsafe { (api.FPDFText_GetUnicode)(text_page, i) }).is_some_and(char::is_whitespace))
                        .map(|i| {
                            let (mut x, mut y) = (0f64, 0f64);
                            unsafe { (api.FPDFText_GetCharOrigin)(text_page, i, &mut x, &mut y) };
                            x
                        })
                        .collect();
                    xs.windows(2).map(|w| ((w[1] - w[0]) as f32 * 1000.0).round() / 1000.0).collect()
                })
                .collect()
        };
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let before = advances(&page);
        // S 667, p 556 and the space 278 with 5 of Tw, 3 of Tc each; "Tm
        // run" at 1 pt under a matrix of 24: 3 of Tc is 72 pt.
        assert_eq!(before[1], [19.008, 31.016, 16.344, 15.0, 16.344]);
        assert_eq!(before[2][0], 86.664);
        assert_eq!(page.set_text(0, "Hallo", &mut |_, _| None).unwrap(), Replaced::SameFont);
        let after = advances(&page);
        assert_eq!(after[1..], before[1..], "the runs set with Tc and Tw, parsed from the content written anew");
        assert_eq!(after[0], [17.328, 13.344, 5.328, 5.328]);
        drop(page);
        let saved = std::env::temp_dir().join(format!("metalpdf_tc_saved_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        assert_eq!(advances(&again.load_page(0).unwrap()), after, "saved and read back");
        drop(again);
        std::fs::remove_file(path).ok();
        std::fs::remove_file(saved).ok();
    }

    /// PDFium's content writer puts a fill colour in the stream only when
    /// it is DeviceRGB: grey text (`g`) and text in a spot colour (a
    /// Separation, as a book's PANTONE headings) came out black when
    /// saved. The colours are set again in RGB before the content is
    /// written (`colours_to_rgb`), so they survive a save.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn grey_and_spot_colours_survive_a_save() {
        let api = api();
        let pdf = write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /ColorSpace << /CS0 [/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0.6 0] /N 1 >>] >> >> >>".into(),
            stream("BT /F1 24 Tf 72 700 Td (Hello) Tj ET BT /F1 24 Tf 0.5 g 72 650 Td (Grey) Tj ET BT /F1 24 Tf /CS0 cs 1 scn 72 600 Td (Spot) Tj ET"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ]);
        let path = std::env::temp_dir().join(format!("metalpdf_colours_{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        let colours = |page: &Page| {
            (0..3)
                .map(|i| {
                    let object = unsafe { (api.FPDFPage_GetObject)(page.handle.get(), i) };
                    let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                    unsafe { (api.FPDFPageObj_GetFillColor)(object, &mut r, &mut g, &mut b, &mut a) };
                    [r, g, b]
                })
                .collect::<Vec<_>>()
        };
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let before = colours(&page);
        assert_eq!(before, [[0, 0, 0], [128, 128, 128], [0, 153, 0]], "as rendered");
        assert_eq!(page.set_text(0, "Hallo", &mut |_, _| None).unwrap(), Replaced::SameFont);
        drop(page);
        let saved = std::env::temp_dir().join(format!("metalpdf_colours_saved_{}.pdf", std::process::id()));
        doc.save_to(&saved).unwrap();
        drop(doc);
        let again = Document::open(api, &saved, None).unwrap();
        let page = again.load_page(0).unwrap();
        assert_eq!(colours(&page), before, "saved and read back");
        assert_eq!(page.text_objects().iter().map(|o| o.text.as_str()).collect::<Vec<_>>(), ["Hallo", "Grey", "Spot"]);
        drop(page);
        drop(again);
        std::fs::remove_file(path).ok();
        std::fs::remove_file(saved).ok();
    }

    /// Two font dictionaries of one base font name on a page (a book made
    /// by LibreOffice had two subsets of PTSans-Bold, both `BAAAAA+`):
    /// PDFium's content generator writes every run with one of them, and
    /// the other's glyphs come out wrong or not at all when saved. Here
    /// the second Helvetica's encoding shows code 65 as `B`.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn two_fonts_of_one_name_are_told_apart() {
        let api = api();
        let pdf = write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".into(),
            stream("BT /F1 24 Tf 72 700 Td (A) Tj ET BT /F2 24 Tf 72 650 Td (A) Tj ET"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /Differences [65 /B] >> >>".into(),
        ]);
        let path = std::env::temp_dir().join(format!("metalpdf_two_fonts_{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        let doc = Document::open(api, &path, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let texts = |page: &Page| page.text_objects().iter().map(|o| o.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&page), ["A", "B"]);
        // Neither is embedded, so the names alone do not tell them apart:
        // the look does (saved, the second read "A").
        assert_eq!(page.clashing_fonts(), None);
        let refused = page.set_text(0, "AA", &mut |_, _| None).unwrap_err();
        println!("the edit: {refused}");
        assert!(refused.contains("look different"), "{refused}");
        drop(page);
        drop(doc);
        std::fs::remove_file(path).ok();
    }

    /// A survey, not a check: `METALPDF_FILES` names PDFs (`;` between
    /// them); for each, the fonts of the first pages (subset, the width
    /// of a space in PDFium's code for it), the first line with two words
    /// edited (how), the document saved, and the saved page rendered
    /// against the original outside the line's box (what the content
    /// generator changed) and against the page in memory after the edit.
    /// Run with `--nocapture`.
    #[test]
    #[ignore = "needs vendor/pdfium and METALPDF_FILES"]
    fn fonts_and_fidelity_survey() {
        let Some(files) = std::env::var_os("METALPDF_FILES") else { return };
        let api = api();
        let scale = 2.0;
        let render = |page: &Page| {
            let [w, h] = page.size();
            let (bw, bh) = ((w * scale) as u32, (h * scale) as u32);
            let mut bitmap = Bitmap::new(api, bw, bh).unwrap();
            page.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: bw as i32, size_y: bh as i32, rotate: 0 }, 0);
            (bitmap, bw as usize, bh as usize, h)
        };
        // Pixels that differ by more than a little in any channel, outside `skip` (page space).
        let diff = |a: &Bitmap, b: &Bitmap, bw: usize, bh: usize, h: f32, skip: Option<[f32; 4]>| {
            let mut n = 0;
            let (pa, pb) = (a.pixels(), b.pixels());
            for y in 0..bh {
                for x in 0..bw {
                    if let Some([l, bo, r, t]) = skip {
                        let (px, py) = (x as f32 / scale, h - y as f32 / scale);
                        if px >= l - 2.0 && px <= r + 2.0 && py >= bo - 2.0 && py <= t + 2.0 {
                            continue;
                        }
                    }
                    let i = (y * bw + x) * 4;
                    if (0..3).any(|c| pa[i + c].abs_diff(pb[i + c]) > 40) {
                        n += 1;
                    }
                }
            }
            n
        };
        for file in files.to_string_lossy().split(';').map(str::trim).filter(|s| !s.is_empty()) {
            println!("\n=== {file}");
            let doc = match Document::open(api, Path::new(file), None) {
                Ok(doc) => doc,
                Err(e) => {
                    println!("cannot open: {e}");
                    continue;
                }
            };
            let mut fonts_seen = std::collections::BTreeMap::new();
            let mut edit_target = None;
            // `METALPDF_SURVEY_PAGE` (1-based) names the page to edit.
            let only: Option<usize> = std::env::var("METALPDF_SURVEY_PAGE").ok().and_then(|p| p.parse::<usize>().ok()).map(|p| p.saturating_sub(1));
            for index in only.map_or(0..doc.page_count().min(12), |p| p..(p + 1).min(doc.page_count())) {
                let Some(page) = doc.load_page(index) else { continue };
                let objects = page.text_objects();
                for o in &objects {
                    if fonts_seen.contains_key(&o.font) {
                        continue;
                    }
                    let object = unsafe { (api.FPDFPage_GetObject)(page.handle.get(), o.index as c_int) };
                    let font = unsafe { (api.FPDFTextObj_GetFont)(object) };
                    let mut space = -1f32;
                    unsafe { (api.FPDFFont_GetGlyphWidth)(font, 0x20, 1000.0, &mut space) };
                    let glyph_a = !unsafe { (api.FPDFFont_GetGlyphPath)(font, u32::from('a'), 12.0) }.is_null();
                    let glyph_cyr = !unsafe { (api.FPDFFont_GetGlyphPath)(font, u32::from('а'), 12.0) }.is_null();
                    let codes = page.char_codes(font);
                    fonts_seen.insert(o.font.clone(), format!("subset {} space width {space:.0} (code {:?}; {} codes found) glyph a {glyph_a} а {glyph_cyr} flags {:#x}", o.subset, codes.get(&' '), codes.len(), o.flags));
                }
                if edit_target.is_none() {
                    let lines = crate::textline::lines(&objects);
                    if let Some(line) = lines.into_iter().find(|l| l.text.split_whitespace().count() >= 3 && l.text.chars().all(|c| !c.is_control() || c == '\u{2}')) {
                        edit_target = Some((index, line));
                    }
                }
            }
            for (name, info) in &fonts_seen {
                println!("font {name}: {info}");
            }
            let Some((index, line)) = edit_target else {
                println!("no line of three words in the first pages");
                continue;
            };
            let mut fonts = crate::fonts::Fonts::default();
            let pristine = Document::open(api, Path::new(file), None).unwrap();
            let pristine_page = pristine.load_page(index).unwrap();
            let page = doc.load_page(index).unwrap();
            let (original, bw, bh, h) = render(&page);
            let first_word = line.text.split_whitespace().next().unwrap();
            let replacement = if first_word.chars().any(|c| c.is_alphabetic() && !c.is_ascii()) { "Проба" } else { "Tested" };
            let wanted = line.text.replacen(first_word, replacement, 1);
            let edit = line.edit(&wanted).unwrap();
            println!("page {}: {:?} -> {replacement:?}: {} objects merged, {} followers", index + 1, line.text, edit.objects.len(), edit.followers.len());
            let how = match page.set_line(&edit.objects, &edit.text, &edit.followers, &mut |name, flags| fonts.get(name, flags)) {
                Ok(how) => how,
                Err(e) => {
                    println!("  refused: {e}");
                    // Where: the page as left in memory (its content
                    // written anew) against the one parsed from it.
                    let parsed = unsafe { (api.FPDF_LoadPage)(doc.inner.handle, index as c_int) };
                    if !parsed.is_null() {
                        let boxes = mismatched_boxes(api, page.handle.get(), parsed, page.size(), None);
                        println!("  differing boxes (page points, from the bottom left): {:?}", &boxes[..boxes.len().min(20)]);
                        if std::env::var_os("METALPDF_KEEP").is_some() {
                            let dump = |name: &str, b: &Bitmap| {
                                let rgba: Vec<u8> = b.pixels().chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                                crate::bench::write_rgba_png(&std::env::temp_dir().join(name), &rgba, bw as u32, bh as u32).unwrap();
                            };
                            let (memory, ..) = render(&page);
                            dump("metalpdf_survey_memory.png", &memory);
                            // The parsed page rendered through a page of its own.
                            let stand_in = Page { doc: Rc::clone(&doc.inner), handle: Cell::new(parsed), index: index as c_int, text: RefCell::new(None), rendering: Cell::new(false) };
                            let (reparsed, ..) = render(&stand_in);
                            dump("metalpdf_survey_parsed.png", &reparsed);
                            println!("  kept the renders in {}", std::env::temp_dir().display());
                            drop(stand_in);
                        } else {
                            unsafe { (api.FPDF_ClosePage)(parsed) };
                        }
                    }
                    continue;
                }
            };
            let after = crate::textline::lines(&page.text_objects()).into_iter().find(|l| l.text.starts_with(replacement)).map(|l| l.text);
            println!("  {how:?}; the line reads {after:?}");
            // The runs that did not move (not the line's): their
            // characters where they were?
            let (pristine_origins, parsed_origins) = (char_origins(api, pristine_page.handle.get()), char_origins(api, page.handle.get()));
            let mut drift = 0f64;
            for (b, a) in pristine_origins.iter().zip(&parsed_origins) {
                if b.len() < 2 || b.len() != a.len() || (b[0][0] - a[0][0]).abs() > 0.01 || (b[0][1] - a[0][1]).abs() > 0.01 {
                    continue;
                }
                for (p, q) in b.iter().zip(a) {
                    drift = drift.max((q[0] - p[0]).abs().max((q[1] - p[1]).abs()));
                }
            }
            println!("  the runs that stayed: characters at most {drift:.4} pt from where they were");
            let (memory, ..) = render(&page);
            let bounds = line.bounds;
            // The guard's own measure, the page before the edit against
            // the one parsed from the new content, the line left out.
            let line_box = [bounds[0] - 1.0, bounds[1] - 1.0, 10_000.0, bounds[3] + 1.0];
            println!("  look check: {} boxes differ outside the line", page_mismatch(api, pristine_page.handle.get(), page.handle.get(), page.size(), Some(line_box)));
            drop(page);
            let saved = std::env::temp_dir().join(format!("metalpdf_survey_{}.pdf", std::process::id()));
            doc.save_to(&saved).unwrap();
            let again = Document::open(api, &saved, None).unwrap();
            let page = again.load_page(index).unwrap();
            let (reread, ..) = render(&page);
            println!(
                "  saved against the original outside the line: {} pixels of {} differ; saved against memory: {} pixels; original against memory outside the line: {}",
                diff(&original, &reread, bw, bh, h, Some(line_box)),
                bw * bh,
                diff(&memory, &reread, bw, bh, h, None),
                diff(&original, &memory, bw, bh, h, Some(line_box)),
            );
            if std::env::var_os("METALPDF_KEEP").is_some() {
                let dump = |name: &str, b: &Bitmap| {
                    let rgba: Vec<u8> = b.pixels().chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                    crate::bench::write_rgba_png(&std::env::temp_dir().join(name), &rgba, bw as u32, bh as u32).unwrap();
                };
                dump("metalpdf_survey_original.png", &original);
                dump("metalpdf_survey_memory.png", &memory);
                dump("metalpdf_survey_saved.png", &reread);
                println!("  kept {} and the renders in {}", saved.display(), std::env::temp_dir().display());
            } else {
                drop(page);
                drop(again);
                std::fs::remove_file(&saved).ok();
            }
        }
    }
}
