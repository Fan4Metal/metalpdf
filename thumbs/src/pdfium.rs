//! The first page of a document, rendered by PDFium for a thumbnail: the
//! few functions this needs, declared by hand (a subset of the app's
//! `src/pdfium/sys.rs`, same pinned build), `pdfium.dll` loaded from this
//! DLL's folder.
//!
//! PDFium is not thread-safe, and the surrogate process Explorer runs
//! thumbnail providers in may ask for several at once on different
//! threads: every call is made under one lock.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::ffi::{c_int, c_uchar, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

type FPDF_DOCUMENT = *mut c_void;
type FPDF_PAGE = *mut c_void;
type FPDF_BITMAP = *mut c_void;

#[repr(C)]
struct FPDF_LIBRARY_CONFIG {
    version: c_int,
    m_pUserFontPaths: *const *const i8,
    m_pIsolate: *mut c_void,
    m_v8EmbedderSlot: u32,
    m_pPlatform: *mut c_void,
    m_RendererType: c_int,
    m_FontLibraryType: c_int,
    m_BrotliEnabled: c_int,
    m_IsolatePerDocument: c_int,
}

#[repr(C)]
struct FPDF_FILEACCESS {
    m_FileLen: c_ulong,
    m_GetBlock: Option<unsafe extern "C" fn(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int>,
    m_Param: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct FS_SIZEF {
    width: f32,
    height: f32,
}

const FPDFBitmap_BGRx: c_int = 3;
/// Annotations drawn, as on screen (form fields need a form environment,
/// which a thumbnail does without).
const FPDF_ANNOT: c_int = 0x01;

macro_rules! api {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            /// The functions looked up in `module`; None if one is missing.
            unsafe fn resolve(module: windows_sys::Win32::Foundation::HMODULE) -> Option<Api> {
                use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
                Some(Api {
                    $($name: unsafe {
                        let f = GetProcAddress(module, concat!(stringify!($name), "\0").as_ptr())?;
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(f)
                    },)*
                })
            }
        }
    };
}

api! {
    FPDF_InitLibraryWithConfig: fn(*const FPDF_LIBRARY_CONFIG);
    FPDF_LoadCustomDocument: fn(*mut FPDF_FILEACCESS, *const i8) -> FPDF_DOCUMENT;
    FPDF_CloseDocument: fn(FPDF_DOCUMENT);
    FPDF_GetPageSizeByIndexF: fn(FPDF_DOCUMENT, c_int, *mut FS_SIZEF) -> c_int;
    FPDF_LoadPage: fn(FPDF_DOCUMENT, c_int) -> FPDF_PAGE;
    FPDF_ClosePage: fn(FPDF_PAGE);
    FPDFBitmap_CreateEx: fn(c_int, c_int, c_int, *mut c_void, c_int) -> FPDF_BITMAP;
    FPDFBitmap_FillRect: fn(FPDF_BITMAP, c_int, c_int, c_int, c_int, u32) -> c_int;
    FPDFBitmap_Destroy: fn(FPDF_BITMAP);
    FPDF_RenderPageBitmap: fn(FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int);
}

// The function pointers are plain addresses in a DLL that stays loaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// Every PDFium call is made holding this.
static LOCK: Mutex<()> = Mutex::new(());

/// PDFium loaded and initialised, once; None when the DLL is not there or
/// not usable (no thumbnails then, Explorer shows the icon).
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let api = dll_candidates().iter().find(|p| p.is_file()).and_then(|p| load(p))?;
        let config = FPDF_LIBRARY_CONFIG {
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
        Some(api)
    })
    .as_ref()
}

/// `pdfium.dll` next to this DLL (where the installer puts both); a debug
/// build also takes the one `tools/get_pdfium.py` fetched (the tests run
/// from `target\debug\deps`).
fn dll_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = module_path().and_then(|p| p.parent().map(Path::to_path_buf)) {
        out.push(dir.join("pdfium.dll"));
    }
    if cfg!(debug_assertions) {
        out.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("vendor").join("pdfium").join("pdfium.dll"));
    }
    out
}

/// The file this code was loaded from (the DLL, or a test's exe).
fn module_path() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, module_path as *const u16, &mut module) } == 0 {
        return None;
    }
    let mut buf = vec![0u16; 32768];
    let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (len > 0 && len < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])))
}

/// Load the DLL at `path` and look the functions up.
fn load(path: &Path) -> Option<Api> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // The DLL's own dependencies are looked for in its folder first; it
    // stays loaded for the life of the process.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
    if module.is_null() {
        return None;
    }
    unsafe { Api::resolve(module) }
}

/// A rendered page: BGRx rows, `width * 4` bytes each, top down.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Reads `buf.len()` bytes of the document at a position; false when it
/// cannot.
pub type Read<'a> = dyn FnMut(u64, &mut [u8]) -> bool + 'a;

unsafe extern "C" fn get_block(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    let read = unsafe { &mut *(param as *mut &mut Read) };
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
    read(u64::from(position), buf) as c_int
}

/// The first page of the document of `len` bytes, read through `read`,
/// rendered on white with its longer side `side` pixels (/Rotate
/// applied). None for a document PDFium cannot open (damaged, encrypted,
/// 4 GiB or more) or without pages, or without PDFium.
pub fn first_page(read: &mut Read, len: u64, side: u32) -> Option<Picture> {
    let api = api()?;
    let len = c_ulong::try_from(len).ok()?;
    if side == 0 {
        return None;
    }
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut read = read;
    let mut access = FPDF_FILEACCESS { m_FileLen: len, m_GetBlock: Some(get_block), m_Param: (&mut read as *mut &mut Read).cast() };
    let doc = unsafe { (api.FPDF_LoadCustomDocument)(&mut access, std::ptr::null()) };
    if doc.is_null() {
        return None;
    }
    let picture = render(api, doc, side);
    unsafe { (api.FPDF_CloseDocument)(doc) };
    picture
}

/// The longest side a thumbnail is drawn at.
const MAX_SIDE: u32 = 4096;

fn render(api: &Api, doc: FPDF_DOCUMENT, side: u32) -> Option<Picture> {
    let mut size = FS_SIZEF::default();
    if unsafe { (api.FPDF_GetPageSizeByIndexF)(doc, 0, &mut size) } == 0 || !(size.width > 0.0 && size.height > 0.0) {
        return None;
    }
    // Explorer asks at most 2560; another caller may ask any size, and a
    // buffer of gigabytes would end the surrogate (`panic = "abort"`).
    let side = side.min(MAX_SIDE);
    let scale = side as f32 / size.width.max(size.height);
    let width = ((size.width * scale).round() as u32).clamp(1, side);
    let height = ((size.height * scale).round() as u32).clamp(1, side);
    let page = unsafe { (api.FPDF_LoadPage)(doc, 0) };
    if page.is_null() {
        return None;
    }
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let bitmap = unsafe { (api.FPDFBitmap_CreateEx)(width as c_int, height as c_int, FPDFBitmap_BGRx, pixels.as_mut_ptr().cast(), (width * 4) as c_int) };
    if !bitmap.is_null() {
        unsafe {
            (api.FPDFBitmap_FillRect)(bitmap, 0, 0, width as c_int, height as c_int, 0xFFFF_FFFF);
            (api.FPDF_RenderPageBitmap)(bitmap, page, 0, 0, width as c_int, height as c_int, 0, FPDF_ANNOT);
            (api.FPDFBitmap_Destroy)(bitmap);
        }
    }
    unsafe { (api.FPDF_ClosePage)(page) };
    (!bitmap.is_null()).then_some(Picture { width, height, pixels })
}
