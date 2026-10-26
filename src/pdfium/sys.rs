//! The part of the PDFium C API the viewer uses (the `public/` headers of
//! the pdfium-binaries build pinned in `tools/get_pdfium.py`), declared by
//! hand. The DLL is loaded at run time from the exe's folder with
//! `LoadLibraryExW` and its functions are looked up one by one, so the exe
//! has no import of it: without `pdfium.dll` the program still starts and
//! says what is missing, and no import library is needed to build it.
//!
//! x64 only: `FPDF_CALLCONV` is `__stdcall`, which on x64 is the one
//! calling convention, so `extern "C"` is right. `unsigned long` is 32 bits
//! on Windows (`c_ulong`), hence the 4 GiB limit of `FPDF_FILEACCESS`.
//!
//! PDFium is not thread-safe: every function here is called on the one
//! engine thread (`engine.rs`), never on the UI thread.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_double, c_float, c_int, c_uchar, c_uint, c_ulong, c_ushort, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The DLL's file name, next to the exe (`build.rs` copies it there).
pub const DLL_NAME: &str = "pdfium.dll";

// Opaque handles: pointers PDFium gives out and takes back.
pub type FPDF_DOCUMENT = *mut c_void;
pub type FPDF_PAGE = *mut c_void;
pub type FPDF_BITMAP = *mut c_void;
pub type FPDF_TEXTPAGE = *mut c_void;
pub type FPDF_SCHHANDLE = *mut c_void;
pub type FPDF_PAGELINK = *mut c_void;
pub type FPDF_BOOKMARK = *mut c_void;
pub type FPDF_DEST = *mut c_void;
pub type FPDF_ACTION = *mut c_void;
pub type FPDF_LINK = *mut c_void;
pub type FPDF_FORMHANDLE = *mut c_void;
pub type FPDF_PAGEOBJECT = *mut c_void;
pub type FPDF_FONT = *mut c_void;
pub type FPDF_PAGEOBJECTMARK = *mut c_void;

pub type FPDF_BOOL = c_int;
pub type FPDF_DWORD = c_ulong;
pub type FPDF_WCHAR = c_ushort;
pub type FPDF_BYTESTRING = *const c_char;
pub type FPDF_WIDESTRING = *const FPDF_WCHAR;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FS_MATRIX {
    pub a: c_float,
    pub b: c_float,
    pub c: c_float,
    pub d: c_float,
    pub e: c_float,
    pub f: c_float,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FS_RECTF {
    pub left: c_float,
    pub top: c_float,
    pub right: c_float,
    pub bottom: c_float,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FS_SIZEF {
    pub width: c_float,
    pub height: c_float,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FS_QUADPOINTSF {
    pub x1: c_float,
    pub y1: c_float,
    pub x2: c_float,
    pub y2: c_float,
    pub x3: c_float,
    pub y3: c_float,
    pub x4: c_float,
    pub y4: c_float,
}

/// `FPDF_InitLibraryWithConfig`'s argument; version 2 covers the fields up
/// to `m_v8EmbedderSlot`, the later ones are read for later versions only.
#[repr(C)]
pub struct FPDF_LIBRARY_CONFIG {
    pub version: c_int,
    pub m_pUserFontPaths: *const *const c_char,
    pub m_pIsolate: *mut c_void,
    pub m_v8EmbedderSlot: c_uint,
    pub m_pPlatform: *mut c_void,
    pub m_RendererType: c_int,
    pub m_FontLibraryType: c_int,
    pub m_BrotliEnabled: FPDF_BOOL,
    pub m_IsolatePerDocument: FPDF_BOOL,
}

/// Reads of a document PDFium does itself, through `m_GetBlock`
/// (`FPDF_LoadCustomDocument`). `m_FileLen` is 32 bits on Windows.
#[repr(C)]
pub struct FPDF_FILEACCESS {
    pub m_FileLen: c_ulong,
    pub m_GetBlock: Option<
        unsafe extern "C" fn(
            param: *mut c_void,
            position: c_ulong,
            buf: *mut c_uchar,
            size: c_ulong,
        ) -> c_int,
    >,
    pub m_Param: *mut c_void,
}

/// Asked by the progressive renderer whether to pause (every hundred page
/// objects, and inside the image decoders that can pause).
#[repr(C)]
pub struct IFSDK_PAUSE {
    /// Must be 1.
    pub version: c_int,
    pub NeedToPauseNow: Option<unsafe extern "C" fn(this: *mut IFSDK_PAUSE) -> FPDF_BOOL>,
    pub user: *mut c_void,
}

/// Where `FPDF_SaveAsCopy` writes.
#[repr(C)]
pub struct FPDF_FILEWRITE {
    /// Must be 1.
    pub version: c_int,
    pub WriteBlock: Option<
        unsafe extern "C" fn(this: *mut FPDF_FILEWRITE, data: *const c_void, size: c_ulong) -> c_int,
    >,
}

/// Colours the progressive renderer paints everything in
/// (`FPDF_RenderPageBitmapWithColorScheme_Start`): the viewer's dark page
/// mode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FPDF_COLORSCHEME {
    pub path_fill_color: FPDF_DWORD,
    pub path_stroke_color: FPDF_DWORD,
    pub text_fill_color: FPDF_DWORD,
    pub text_stroke_color: FPDF_DWORD,
}

/// The callbacks of a form-fill environment. Every one is optional (PDFium
/// checks for null); the viewer sets none and version 1: it only wants the
/// form fields drawn (`FPDF_FFLDraw`). All the fields are pointers, so
/// the exact callback types do not matter for the layout.
#[repr(C)]
pub struct FPDF_FORMFILLINFO {
    pub version: c_int,
    pub Release: *const c_void,
    pub FFI_Invalidate: *const c_void,
    pub FFI_OutputSelectedRect: *const c_void,
    pub FFI_SetCursor: *const c_void,
    pub FFI_SetTimer: *const c_void,
    pub FFI_KillTimer: *const c_void,
    pub FFI_GetLocalTime: *const c_void,
    pub FFI_OnChange: *const c_void,
    pub FFI_GetPage: *const c_void,
    pub FFI_GetCurrentPage: *const c_void,
    pub FFI_GetRotation: *const c_void,
    pub FFI_ExecuteNamedAction: *const c_void,
    pub FFI_SetTextFieldFocus: *const c_void,
    pub FFI_DoURIAction: *const c_void,
    pub FFI_DoGoToAction: *const c_void,
    pub m_pJsPlatform: *mut c_void,
    // Version 2 and later.
    pub xfa_disabled: FPDF_BOOL,
    pub FFI_DisplayCaret: *const c_void,
    pub FFI_GetCurrentPageIndex: *const c_void,
    pub FFI_SetCurrentPage: *const c_void,
    pub FFI_GotoURL: *const c_void,
    pub FFI_GetPageViewRect: *const c_void,
    pub FFI_PageEvent: *const c_void,
    pub FFI_PopupMenu: *const c_void,
    pub FFI_OpenFile: *const c_void,
    pub FFI_EmailTo: *const c_void,
    pub FFI_UploadTo: *const c_void,
    pub FFI_GetPlatform: *const c_void,
    pub FFI_GetLanguage: *const c_void,
    pub FFI_DownloadFromURL: *const c_void,
    pub FFI_PostRequestURL: *const c_void,
    pub FFI_PutRequestURL: *const c_void,
    pub FFI_OnFocusChange: *const c_void,
    pub FFI_DoURIActionWithKeyboardModifier: *const c_void,
}

impl Default for FPDF_FORMFILLINFO {
    fn default() -> Self {
        // SAFETY: a struct of integers and pointers; zero is a valid value of
        // each (null callbacks, version 0 to be set by the caller).
        unsafe { std::mem::zeroed() }
    }
}

// FPDF_GetLastError.
pub const FPDF_ERR_SUCCESS: c_ulong = 0;
pub const FPDF_ERR_UNKNOWN: c_ulong = 1;
pub const FPDF_ERR_FILE: c_ulong = 2;
pub const FPDF_ERR_FORMAT: c_ulong = 3;
pub const FPDF_ERR_PASSWORD: c_ulong = 4;
pub const FPDF_ERR_SECURITY: c_ulong = 5;
pub const FPDF_ERR_PAGE: c_ulong = 6;

// FPDFBitmap_CreateEx formats.
pub const FPDFBitmap_Gray: c_int = 1;
pub const FPDFBitmap_BGR: c_int = 2;
pub const FPDFBitmap_BGRx: c_int = 3;
pub const FPDFBitmap_BGRA: c_int = 4;

// Render flags.
pub const FPDF_ANNOT: c_int = 0x01;
pub const FPDF_LCD_TEXT: c_int = 0x02;
pub const FPDF_NO_NATIVETEXT: c_int = 0x04;
pub const FPDF_GRAYSCALE: c_int = 0x08;
pub const FPDF_REVERSE_BYTE_ORDER: c_int = 0x10;
pub const FPDF_CONVERT_FILL_TO_STROKE: c_int = 0x20;
pub const FPDF_RENDER_LIMITEDIMAGECACHE: c_int = 0x200;
pub const FPDF_RENDER_FORCEHALFTONE: c_int = 0x400;
pub const FPDF_PRINTING: c_int = 0x800;
pub const FPDF_RENDER_NO_SMOOTHTEXT: c_int = 0x1000;
pub const FPDF_RENDER_NO_SMOOTHIMAGE: c_int = 0x2000;
pub const FPDF_RENDER_NO_SMOOTHPATH: c_int = 0x4000;

// Progressive rendering: what Start and Continue return.
pub const FPDF_RENDER_READY: c_int = 0;
pub const FPDF_RENDER_TOBECONTINUED: c_int = 1;
pub const FPDF_RENDER_DONE: c_int = 2;
pub const FPDF_RENDER_FAILED: c_int = 3;

// FPDFText_FindStart flags.
pub const FPDF_MATCHCASE: c_ulong = 1;
pub const FPDF_MATCHWHOLEWORD: c_ulong = 2;
pub const FPDF_CONSECUTIVE: c_ulong = 4;

// FPDFAction_GetType.
pub const PDFACTION_UNSUPPORTED: c_ulong = 0;
pub const PDFACTION_GOTO: c_ulong = 1;
pub const PDFACTION_REMOTEGOTO: c_ulong = 2;
pub const PDFACTION_URI: c_ulong = 3;
pub const PDFACTION_LAUNCH: c_ulong = 4;
pub const PDFACTION_EMBEDDEDGOTO: c_ulong = 5;

// Page object types (FPDFPageObj_GetType).
pub const FPDF_PAGEOBJ_UNKNOWN: c_int = 0;
pub const FPDF_PAGEOBJ_TEXT: c_int = 1;
pub const FPDF_PAGEOBJ_PATH: c_int = 2;
pub const FPDF_PAGEOBJ_IMAGE: c_int = 3;
pub const FPDF_PAGEOBJ_SHADING: c_int = 4;
pub const FPDF_PAGEOBJ_FORM: c_int = 5;
// Text render modes (fpdf_edit.h): fill, stroke, fill and stroke,
// invisible, then the same four clipping as well.
pub const FPDF_TEXTRENDERMODE_INVISIBLE: c_int = 3;
pub const FPDF_TEXTRENDERMODE_CLIP: c_int = 7;

// FPDFPage_Flatten: what for, and what came of it.
pub const FLAT_PRINT: c_int = 1;
pub const FLATTEN_SUCCESS: c_int = 1;

// Object types (FPDFPageObjMark_GetParamValueType).
pub const FPDF_OBJECT_NUMBER: c_int = 2;
pub const FPDF_OBJECT_STRING: c_int = 3;
pub const FPDF_OBJECT_NAME: c_int = 4;

// FPDFText_LoadFont font types.
pub const FPDF_FONT_TYPE1: c_int = 1;
pub const FPDF_FONT_TRUETYPE: c_int = 2;

// FPDF_SaveAsCopy flags.
pub const FPDF_INCREMENTAL: FPDF_DWORD = 1;
pub const FPDF_NO_INCREMENTAL: FPDF_DWORD = 2;
pub const FPDF_REMOVE_SECURITY: FPDF_DWORD = 4;
pub const FPDF_SUBSET_NEW_FONTS: FPDF_DWORD = 8;

/// Declares the functions as fields of [`Api`] and the lookup of each.
macro_rules! api {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// The functions of a loaded pdfium.dll.
        pub struct Api {
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            /// Look every function up in `module`; the first one missing
            /// is named in the error.
            unsafe fn resolve(module: *mut c_void) -> Result<Self, String> {
                use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
                Ok(Self {
                    $($name: {
                        let symbol = unsafe { GetProcAddress(module, concat!(stringify!($name), "\0").as_ptr()) };
                        match symbol {
                            // SAFETY: the declaration above follows the C
                            // header; both are plain function pointers.
                            Some(f) => unsafe {
                                std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(f)
                            },
                            None => return Err(format!("{} has no function {}", DLL_NAME, stringify!($name))),
                        }
                    },)*
                })
            }
        }
    };
}

api! {
    // fpdfview.h
    FPDF_InitLibraryWithConfig: fn(*const FPDF_LIBRARY_CONFIG);
    FPDF_DestroyLibrary: fn();
    FPDF_GetLastError: fn() -> c_ulong;
    FPDF_LoadCustomDocument: fn(*mut FPDF_FILEACCESS, FPDF_BYTESTRING) -> FPDF_DOCUMENT;
    FPDF_CloseDocument: fn(FPDF_DOCUMENT);
    FPDF_GetFileVersion: fn(FPDF_DOCUMENT, *mut c_int) -> FPDF_BOOL;
    FPDF_GetDocPermissions: fn(FPDF_DOCUMENT) -> c_ulong;
    FPDF_GetSecurityHandlerRevision: fn(FPDF_DOCUMENT) -> c_int;
    FPDF_GetPageCount: fn(FPDF_DOCUMENT) -> c_int;
    FPDF_GetPageSizeByIndexF: fn(FPDF_DOCUMENT, c_int, *mut FS_SIZEF) -> FPDF_BOOL;
    FPDF_LoadPage: fn(FPDF_DOCUMENT, c_int) -> FPDF_PAGE;
    // Windows only: the page drawn into a device context (a printer's),
    // as vectors and text.
    FPDF_RenderPage: fn(*mut c_void, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int);
    FPDF_VIEWERREF_GetPrintScaling: fn(FPDF_DOCUMENT) -> FPDF_BOOL;
    // fpdf_flatten.h: form fields and annotations made page content.
    FPDFPage_Flatten: fn(FPDF_PAGE, c_int) -> c_int;
    FPDF_ClosePage: fn(FPDF_PAGE);
    FPDF_GetPageWidthF: fn(FPDF_PAGE) -> c_float;
    FPDF_GetPageHeightF: fn(FPDF_PAGE) -> c_float;
    FPDF_GetPageBoundingBox: fn(FPDF_PAGE, *mut FS_RECTF) -> FPDF_BOOL;
    FPDFBitmap_CreateEx: fn(c_int, c_int, c_int, *mut c_void, c_int) -> FPDF_BITMAP;
    FPDFBitmap_FillRect: fn(FPDF_BITMAP, c_int, c_int, c_int, c_int, FPDF_DWORD) -> FPDF_BOOL;
    FPDFBitmap_Destroy: fn(FPDF_BITMAP);
    FPDFBitmap_GetBuffer: fn(FPDF_BITMAP) -> *mut c_void;
    FPDFBitmap_GetWidth: fn(FPDF_BITMAP) -> c_int;
    FPDFBitmap_GetHeight: fn(FPDF_BITMAP) -> c_int;
    FPDFBitmap_GetStride: fn(FPDF_BITMAP) -> c_int;
    FPDFBitmap_GetFormat: fn(FPDF_BITMAP) -> c_int;
    // An image object rendered as shown (its matrix, masks and colour keys
    // applied) into a bitmap of PDFium's, to be destroyed by the caller.
    FPDFImageObj_GetRenderedBitmap: fn(FPDF_DOCUMENT, FPDF_PAGE, FPDF_PAGEOBJECT) -> FPDF_BITMAP;
    FPDF_RenderPageBitmap: fn(FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int);
    FPDF_RenderPageBitmapWithMatrix: fn(FPDF_BITMAP, FPDF_PAGE, *const FS_MATRIX, *const FS_RECTF, c_int);
    FPDF_PageToDevice: fn(FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_double, c_double, *mut c_int, *mut c_int) -> FPDF_BOOL;
    FPDF_DeviceToPage: fn(FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int, c_int, *mut c_double, *mut c_double) -> FPDF_BOOL;
    // fpdf_progressive.h
    FPDF_RenderPageBitmap_Start: fn(FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int, *mut IFSDK_PAUSE) -> c_int;
    FPDF_RenderPageBitmapWithColorScheme_Start: fn(FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int, *const FPDF_COLORSCHEME, *mut IFSDK_PAUSE) -> c_int;
    FPDF_RenderPage_Continue: fn(FPDF_PAGE, *mut IFSDK_PAUSE) -> c_int;
    FPDF_RenderPage_Close: fn(FPDF_PAGE);
    // fpdf_formfill.h
    FPDFDOC_InitFormFillEnvironment: fn(FPDF_DOCUMENT, *mut FPDF_FORMFILLINFO) -> FPDF_FORMHANDLE;
    FPDFDOC_ExitFormFillEnvironment: fn(FPDF_FORMHANDLE);
    FORM_OnAfterLoadPage: fn(FPDF_PAGE, FPDF_FORMHANDLE);
    FORM_OnBeforeClosePage: fn(FPDF_PAGE, FPDF_FORMHANDLE);
    FPDF_FFLDraw: fn(FPDF_FORMHANDLE, FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int);
    FPDF_SetFormFieldHighlightAlpha: fn(FPDF_FORMHANDLE, c_uchar);
    // fpdf_text.h
    FPDFText_LoadPage: fn(FPDF_PAGE) -> FPDF_TEXTPAGE;
    FPDFText_ClosePage: fn(FPDF_TEXTPAGE);
    FPDFText_CountChars: fn(FPDF_TEXTPAGE) -> c_int;
    FPDFText_GetUnicode: fn(FPDF_TEXTPAGE, c_int) -> c_uint;
    FPDFText_IsGenerated: fn(FPDF_TEXTPAGE, c_int) -> c_int;
    FPDFText_GetFontSize: fn(FPDF_TEXTPAGE, c_int) -> c_double;
    FPDFText_GetCharBox: fn(FPDF_TEXTPAGE, c_int, *mut c_double, *mut c_double, *mut c_double, *mut c_double) -> FPDF_BOOL;
    FPDFText_GetLooseCharBox: fn(FPDF_TEXTPAGE, c_int, *mut FS_RECTF) -> FPDF_BOOL;
    FPDFText_GetCharOrigin: fn(FPDF_TEXTPAGE, c_int, *mut c_double, *mut c_double) -> FPDF_BOOL;
    FPDFText_GetCharIndexAtPos: fn(FPDF_TEXTPAGE, c_double, c_double, c_double, c_double) -> c_int;
    FPDFText_GetText: fn(FPDF_TEXTPAGE, c_int, c_int, *mut c_ushort) -> c_int;
    FPDFText_CountRects: fn(FPDF_TEXTPAGE, c_int, c_int) -> c_int;
    FPDFText_GetRect: fn(FPDF_TEXTPAGE, c_int, *mut c_double, *mut c_double, *mut c_double, *mut c_double) -> FPDF_BOOL;
    FPDFText_FindStart: fn(FPDF_TEXTPAGE, FPDF_WIDESTRING, c_ulong, c_int) -> FPDF_SCHHANDLE;
    FPDFText_FindNext: fn(FPDF_SCHHANDLE) -> FPDF_BOOL;
    FPDFText_FindPrev: fn(FPDF_SCHHANDLE) -> FPDF_BOOL;
    FPDFText_GetSchResultIndex: fn(FPDF_SCHHANDLE) -> c_int;
    FPDFText_GetSchCount: fn(FPDF_SCHHANDLE) -> c_int;
    FPDFText_FindClose: fn(FPDF_SCHHANDLE);
    FPDFText_GetTextObject: fn(FPDF_TEXTPAGE, c_int) -> FPDF_PAGEOBJECT;
    FPDFLink_LoadWebLinks: fn(FPDF_TEXTPAGE) -> FPDF_PAGELINK;
    FPDFLink_CountWebLinks: fn(FPDF_PAGELINK) -> c_int;
    FPDFLink_GetURL: fn(FPDF_PAGELINK, c_int, *mut c_ushort, c_int) -> c_int;
    FPDFLink_CountRects: fn(FPDF_PAGELINK, c_int) -> c_int;
    FPDFLink_GetRect: fn(FPDF_PAGELINK, c_int, c_int, *mut c_double, *mut c_double, *mut c_double, *mut c_double) -> FPDF_BOOL;
    FPDFLink_CloseWebLinks: fn(FPDF_PAGELINK);
    // fpdf_doc.h
    FPDFBookmark_GetFirstChild: fn(FPDF_DOCUMENT, FPDF_BOOKMARK) -> FPDF_BOOKMARK;
    FPDFBookmark_GetNextSibling: fn(FPDF_DOCUMENT, FPDF_BOOKMARK) -> FPDF_BOOKMARK;
    FPDFBookmark_GetTitle: fn(FPDF_BOOKMARK, *mut c_void, c_ulong) -> c_ulong;
    FPDFBookmark_GetDest: fn(FPDF_DOCUMENT, FPDF_BOOKMARK) -> FPDF_DEST;
    FPDFBookmark_GetAction: fn(FPDF_BOOKMARK) -> FPDF_ACTION;
    FPDFAction_GetType: fn(FPDF_ACTION) -> c_ulong;
    FPDFAction_GetDest: fn(FPDF_DOCUMENT, FPDF_ACTION) -> FPDF_DEST;
    FPDFAction_GetURIPath: fn(FPDF_DOCUMENT, FPDF_ACTION, *mut c_void, c_ulong) -> c_ulong;
    FPDFDest_GetDestPageIndex: fn(FPDF_DOCUMENT, FPDF_DEST) -> c_int;
    FPDFDest_GetLocationInPage: fn(FPDF_DEST, *mut FPDF_BOOL, *mut FPDF_BOOL, *mut FPDF_BOOL, *mut c_float, *mut c_float, *mut c_float) -> FPDF_BOOL;
    FPDFLink_Enumerate: fn(FPDF_PAGE, *mut c_int, *mut FPDF_LINK) -> FPDF_BOOL;
    FPDFLink_GetAnnotRect: fn(FPDF_LINK, *mut FS_RECTF) -> FPDF_BOOL;
    FPDFLink_GetDest: fn(FPDF_DOCUMENT, FPDF_LINK) -> FPDF_DEST;
    FPDFLink_GetAction: fn(FPDF_LINK) -> FPDF_ACTION;
    FPDF_GetMetaText: fn(FPDF_DOCUMENT, FPDF_BYTESTRING, *mut c_void, c_ulong) -> c_ulong;
    FPDF_GetPageLabel: fn(FPDF_DOCUMENT, c_int, *mut c_void, c_ulong) -> c_ulong;
    // fpdf_edit.h (the text editing)
    FPDFPage_CountObjects: fn(FPDF_PAGE) -> c_int;
    FPDFPage_GetObject: fn(FPDF_PAGE, c_int) -> FPDF_PAGEOBJECT;
    FPDFPage_InsertObject: fn(FPDF_PAGE, FPDF_PAGEOBJECT);
    FPDFPage_RemoveObject: fn(FPDF_PAGE, FPDF_PAGEOBJECT) -> FPDF_BOOL;
    FPDFPage_GenerateContent: fn(FPDF_PAGE) -> FPDF_BOOL;
    FPDFPageObj_Destroy: fn(FPDF_PAGEOBJECT);
    FPDFPageObj_GetType: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_GetBounds: fn(FPDF_PAGEOBJECT, *mut c_float, *mut c_float, *mut c_float, *mut c_float) -> FPDF_BOOL;
    FPDFPageObj_Transform: fn(FPDF_PAGEOBJECT, c_double, c_double, c_double, c_double, c_double, c_double);
    FPDFImageObj_LoadJpegFileInline: fn(*mut FPDF_PAGE, c_int, FPDF_PAGEOBJECT, *mut FPDF_FILEACCESS) -> FPDF_BOOL;
    FPDFImageObj_SetBitmap: fn(*mut FPDF_PAGE, c_int, FPDF_PAGEOBJECT, FPDF_BITMAP) -> FPDF_BOOL;
    FPDFImageObj_SetMatrix: fn(FPDF_PAGEOBJECT, c_double, c_double, c_double, c_double, c_double, c_double) -> FPDF_BOOL;
    FPDFImageObj_GetImagePixelSize: fn(FPDF_PAGEOBJECT, *mut c_uint, *mut c_uint) -> FPDF_BOOL;
    FPDFPageObj_GetMatrix: fn(FPDF_PAGEOBJECT, *mut FS_MATRIX) -> FPDF_BOOL;
    FPDFPageObj_SetMatrix: fn(FPDF_PAGEOBJECT, *const FS_MATRIX) -> FPDF_BOOL;
    FPDFPageObj_GetFillColor: fn(FPDF_PAGEOBJECT, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> FPDF_BOOL;
    FPDFPageObj_SetFillColor: fn(FPDF_PAGEOBJECT, c_uint, c_uint, c_uint, c_uint) -> FPDF_BOOL;
    FPDFPageObj_GetStrokeColor: fn(FPDF_PAGEOBJECT, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> FPDF_BOOL;
    FPDFPageObj_SetStrokeColor: fn(FPDF_PAGEOBJECT, c_uint, c_uint, c_uint, c_uint) -> FPDF_BOOL;
    FPDFPageObj_GetStrokeWidth: fn(FPDF_PAGEOBJECT, *mut c_float) -> FPDF_BOOL;
    FPDFPageObj_SetStrokeWidth: fn(FPDF_PAGEOBJECT, c_float) -> FPDF_BOOL;
    FPDFPage_InsertObjectAtIndex: fn(FPDF_PAGE, FPDF_PAGEOBJECT, usize) -> FPDF_BOOL;
    // Marked content: what ties a tagged document's tags to the objects.
    FPDFPageObj_GetMarkedContentID: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_CountMarks: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_GetMark: fn(FPDF_PAGEOBJECT, c_ulong) -> FPDF_PAGEOBJECTMARK;
    FPDFPageObj_AddMark: fn(FPDF_PAGEOBJECT, FPDF_BYTESTRING) -> FPDF_PAGEOBJECTMARK;
    FPDFPageObjMark_GetName: fn(FPDF_PAGEOBJECTMARK, *mut FPDF_WCHAR, c_ulong, *mut c_ulong) -> FPDF_BOOL;
    FPDFPageObjMark_CountParams: fn(FPDF_PAGEOBJECTMARK) -> c_int;
    FPDFPageObjMark_GetParamKey: fn(FPDF_PAGEOBJECTMARK, c_ulong, *mut FPDF_WCHAR, c_ulong, *mut c_ulong) -> FPDF_BOOL;
    FPDFPageObjMark_GetParamValueType: fn(FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING) -> c_int;
    FPDFPageObjMark_GetParamIntValue: fn(FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, *mut c_int) -> FPDF_BOOL;
    FPDFPageObjMark_GetParamFloatValue: fn(FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, *mut c_float) -> FPDF_BOOL;
    FPDFPageObjMark_GetParamStringValue: fn(FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, *mut FPDF_WCHAR, c_ulong, *mut c_ulong) -> FPDF_BOOL;
    FPDFPageObjMark_GetParamBlobValue: fn(FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, *mut u8, c_ulong, *mut c_ulong) -> FPDF_BOOL;
    FPDFPageObjMark_SetIntParam: fn(FPDF_DOCUMENT, FPDF_PAGEOBJECT, FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, c_int) -> FPDF_BOOL;
    FPDFPageObjMark_SetFloatParam: fn(FPDF_DOCUMENT, FPDF_PAGEOBJECT, FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, c_float) -> FPDF_BOOL;
    FPDFPageObjMark_SetStringParam: fn(FPDF_DOCUMENT, FPDF_PAGEOBJECT, FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, FPDF_BYTESTRING) -> FPDF_BOOL;
    FPDFPageObjMark_SetBlobParam: fn(FPDF_DOCUMENT, FPDF_PAGEOBJECT, FPDF_PAGEOBJECTMARK, FPDF_BYTESTRING, *const u8, c_ulong) -> FPDF_BOOL;
    FPDFPageObj_NewTextObj: fn(FPDF_DOCUMENT, FPDF_BYTESTRING, c_float) -> FPDF_PAGEOBJECT;
    FPDFPageObj_CreateTextObj: fn(FPDF_DOCUMENT, FPDF_FONT, c_float) -> FPDF_PAGEOBJECT;
    FPDFText_SetText: fn(FPDF_PAGEOBJECT, FPDF_WIDESTRING) -> FPDF_BOOL;
    FPDFText_SetCharcodes: fn(FPDF_PAGEOBJECT, *const u32, usize) -> FPDF_BOOL;
    FPDFText_LoadFont: fn(FPDF_DOCUMENT, *const u8, u32, c_int, FPDF_BOOL) -> FPDF_FONT;
    FPDFText_LoadStandardFont: fn(FPDF_DOCUMENT, FPDF_BYTESTRING) -> FPDF_FONT;
    FPDFFont_Close: fn(FPDF_FONT);
    FPDFTextObj_GetText: fn(FPDF_PAGEOBJECT, FPDF_TEXTPAGE, *mut FPDF_WCHAR, c_ulong) -> c_ulong;
    FPDFTextObj_GetFont: fn(FPDF_PAGEOBJECT) -> FPDF_FONT;
    FPDFTextObj_GetFontSize: fn(FPDF_PAGEOBJECT, *mut c_float) -> FPDF_BOOL;
    FPDFTextObj_GetTextRenderMode: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFTextObj_SetTextRenderMode: fn(FPDF_PAGEOBJECT, c_int) -> FPDF_BOOL;
    FPDFFont_GetBaseFontName: fn(FPDF_FONT, *mut c_char, usize) -> usize;
    FPDFFont_GetIsEmbedded: fn(FPDF_FONT) -> c_int;
    FPDFFont_GetFlags: fn(FPDF_FONT) -> c_int;
    // The font's bytes (the substitute's for a font not embedded), the
    // size through the last pointer; a Type 3 font, with no face, has
    // none.
    FPDFFont_GetFontData: fn(FPDF_FONT, *mut u8, usize, *mut usize) -> FPDF_BOOL;
    // The glyph's path for a character (Unicode), or null when the font
    // has no glyph for it; the path is the font's, never freed.
    FPDFFont_GetGlyphPath: fn(FPDF_FONT, u32, c_float) -> *const c_void;
    // The advance of a character (Unicode) in the code the font encodes
    // it with, at the size given.
    FPDFFont_GetGlyphWidth: fn(FPDF_FONT, u32, c_float, *mut c_float) -> FPDF_BOOL;
    // Pages: fpdf_edit.h, fpdf_ppo.h
    FPDFPage_Delete: fn(FPDF_DOCUMENT, c_int);
    FPDFPage_New: fn(FPDF_DOCUMENT, c_int, c_double, c_double) -> FPDF_PAGE;
    FPDF_ImportPagesByIndex: fn(FPDF_DOCUMENT, FPDF_DOCUMENT, *const c_int, c_ulong, c_int) -> FPDF_BOOL;
    // fpdf_save.h, fpdf_signature.h, fpdf_catalog.h
    FPDF_SaveAsCopy: fn(FPDF_DOCUMENT, *mut FPDF_FILEWRITE, FPDF_DWORD) -> FPDF_BOOL;
    FPDF_GetSignatureCount: fn(FPDF_DOCUMENT) -> c_int;
    FPDFCatalog_IsTagged: fn(FPDF_DOCUMENT) -> FPDF_BOOL;
}

/// Why PDFium could not be loaded.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    /// No DLL in the places it is looked for.
    NotFound { looked_in: Vec<PathBuf> },
    /// A DLL is there but cannot be used (not a 64-bit DLL, a function
    /// missing); the reason, technical.
    Unusable(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound { looked_in } => {
                let places: Vec<String> = looked_in.iter().map(|p| p.display().to_string()).collect();
                write!(f, "{DLL_NAME} not found (looked in {})", places.join(", "))
            }
            LoadError::Unusable(reason) => f.write_str(reason),
        }
    }
}

/// PDFium, loaded on first use; the error says why it could not be.
pub fn api() -> Result<&'static Api, &'static LoadError> {
    static API: OnceLock<Result<Api, LoadError>> = OnceLock::new();
    API.get_or_init(|| {
        let candidates = dll_candidates();
        match candidates.iter().find(|p| p.is_file()) {
            Some(path) => load(path).map_err(LoadError::Unusable),
            None => Err(LoadError::NotFound { looked_in: candidates }),
        }
    })
    .as_ref()
}

/// Next to the exe; a debug build also takes the one `tools/get_pdfium.py`
/// fetched into the project (tests run from `target\debug\deps`).
fn dll_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        out.push(dir.join(DLL_NAME));
    }
    if cfg!(debug_assertions) {
        out.push(vendored_dll());
    }
    out
}

/// The DLL `tools/get_pdfium.py` fetches, for the tests.
pub fn vendored_dll() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("pdfium").join(DLL_NAME)
}

/// Load the DLL at `path` and look its functions up.
pub fn load(path: &Path) -> Result<Api, String> {
    use windows_sys::Win32::System::LibraryLoader::{LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
    let wide = crate::win::wide(path);
    // The DLL's own dependencies are looked for in its folder first.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
    if module.is_null() {
        return Err(format!("cannot load {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    // The module stays loaded for the life of the process.
    unsafe { Api::resolve(module) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layouts of the C structs on x64 (checked against the headers).
    #[test]
    fn struct_sizes() {
        assert_eq!(size_of::<FS_MATRIX>(), 24);
        assert_eq!(size_of::<FS_RECTF>(), 16);
        assert_eq!(size_of::<FS_SIZEF>(), 8);
        assert_eq!(size_of::<FS_QUADPOINTSF>(), 32);
        assert_eq!(size_of::<FPDF_FILEACCESS>(), 24);
        assert_eq!(size_of::<IFSDK_PAUSE>(), 24);
        assert_eq!(size_of::<FPDF_FILEWRITE>(), 16);
        assert_eq!(size_of::<FPDF_COLORSCHEME>(), 16);
        // version, 15 callbacks, the JS platform; then xfa_disabled (padded)
        // and 17 more callbacks.
        assert_eq!(size_of::<FPDF_FORMFILLINFO>(), 8 + 16 * 8 + 8 + 17 * 8);
        assert_eq!(size_of::<FPDF_LIBRARY_CONFIG>(), 8 + 8 + 8 + 8 + 8 + 16);
        assert_eq!(size_of::<c_ulong>(), 4, "unsigned long is 32 bits on Windows");
    }

    #[test]
    fn missing_dll_is_an_error() {
        let path = std::env::temp_dir().join("no_such_dir_metalpdf").join(DLL_NAME);
        let err = load(&path).err().expect("must fail");
        assert!(err.contains("no_such_dir_metalpdf"), "{err}");
    }

    /// Loads the fetched DLL for real: `cargo test pdfium -- --ignored`.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn loads_the_dll() {
        load(&vendored_dll()).unwrap();
    }
}
