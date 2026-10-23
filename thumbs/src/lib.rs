//! MetalPDF's thumbnail provider: the COM in-process server Explorer loads
//! (in its surrogate, `dllhost.exe`, so that a crash here costs a
//! thumbnail, not Explorer) to draw a PDF's first page as its thumbnail.
//! `metalpdf --register` (`src/assoc.rs`) registers the class under
//! HKEY_CURRENT_USER and names it on the ProgID `MetalPDF.pdf`, so that
//! files have it while MetalPDF is their default program.
//!
//! The objects are written by hand (no `windows` crate): the class
//! factory, a static, and the provider, which gives `IInitializeWithStream`
//! (Explorer hands over the file as a stream) and `IThumbnailProvider`.
//! The thumbnail is opaque (`WTSAT_RGB`): the extractor it replaced,
//! Acrobat's, gave a bitmap with every alpha 0, which Explorer turned
//! black when it scaled the cached 96-pixel thumbnail down to medium
//! icons.

// The interfaces' methods keep the names of the headers.
#![allow(non_snake_case)]

mod pdfium;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOINTERFACE, E_POINTER, E_UNEXPECTED, S_FALSE, S_OK,
};
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP};
use windows_sys::core::{GUID, HRESULT, IUnknown_Vtbl};

/// The provider's class; `src/assoc.rs` registers the same one.
pub const CLSID_THUMBNAIL_PROVIDER: GUID = GUID::from_u128(0x46b6cbb3_fb97_4c67_9c29_8151c9dafe44);

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
const IID_IINITIALIZEWITHSTREAM: GUID = GUID::from_u128(0xb824b49d_22ac_4161_ac8a_9916e8fa3f7f);
const IID_ITHUMBNAILPROVIDER: GUID = GUID::from_u128(0xe357fccd_a995_4576_b01f_234630154e96);

/// `WTS_ALPHATYPE`: no alpha channel.
const WTSAT_RGB: u32 = 1;
/// `STREAM_SEEK_SET`, `STREAM_SEEK_END`.
const SEEK_SET: u32 = 0;
const SEEK_END: u32 = 2;

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// `IStream` as far as it is used: `Read` and `Seek` (`Write` between them
/// only holds its place).
#[repr(C)]
struct IStreamVtbl {
    base: IUnknown_Vtbl,
    Read: unsafe extern "system" fn(this: *mut c_void, buf: *mut c_void, size: u32, read: *mut u32) -> HRESULT,
    Write: *const c_void,
    Seek: unsafe extern "system" fn(this: *mut c_void, offset: i64, origin: u32, position: *mut u64) -> HRESULT,
}

/// A COM object's interface pointer: its first field is the table.
unsafe fn vtbl<T>(object: *mut c_void) -> &'static T {
    unsafe { &**(object as *mut *const T) }
}

// The class factory: a static, never freed.

#[repr(C)]
struct ClassFactoryVtbl {
    base: IUnknown_Vtbl,
    CreateInstance: unsafe extern "system" fn(this: *mut c_void, outer: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT,
    LockServer: unsafe extern "system" fn(this: *mut c_void, lock: i32) -> HRESULT,
}

#[repr(C)]
struct ClassFactory {
    vtbl: *const ClassFactoryVtbl,
}

// Only a pointer to a static table, never written.
unsafe impl Sync for ClassFactory {}

static FACTORY_VTBL: ClassFactoryVtbl = ClassFactoryVtbl {
    base: IUnknown_Vtbl { QueryInterface: factory_query, AddRef: static_add_ref, Release: static_release },
    CreateInstance: factory_create,
    LockServer: factory_lock,
};
static FACTORY: ClassFactory = ClassFactory { vtbl: &FACTORY_VTBL };

unsafe extern "system" fn factory_query(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { &*iid };
    if same(iid, &IID_IUNKNOWN) || same(iid, &IID_ICLASSFACTORY) {
        unsafe { *out = this };
        S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn static_add_ref(_: *mut c_void) -> u32 {
    2
}

unsafe extern "system" fn static_release(_: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn factory_create(_: *mut c_void, outer: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = std::ptr::null_mut() };
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    let provider = Box::into_raw(Box::new(Provider {
        init: &INIT_VTBL,
        thumbnail: &THUMBNAIL_VTBL,
        refs: AtomicU32::new(1),
        stream: AtomicPtr::new(std::ptr::null_mut()),
    }));
    // The object's own reference is let go after the caller has taken its
    // own: a refused interface frees the object.
    unsafe {
        let hr = provider_query(provider.cast(), iid, out);
        provider_release(provider.cast());
        hr
    }
}

unsafe extern "system" fn factory_lock(_: *mut c_void, _: i32) -> HRESULT {
    S_OK
}

// The provider: two interfaces, each a table pointer within the object;
// `this` is the object for `IInitializeWithStream` (and `IUnknown`), the
// second field for `IThumbnailProvider`.

#[repr(C)]
struct InitializeWithStreamVtbl {
    base: IUnknown_Vtbl,
    Initialize: unsafe extern "system" fn(this: *mut c_void, stream: *mut c_void, mode: u32) -> HRESULT,
}

#[repr(C)]
struct ThumbnailProviderVtbl {
    base: IUnknown_Vtbl,
    GetThumbnail: unsafe extern "system" fn(this: *mut c_void, side: u32, bitmap: *mut HBITMAP, alpha: *mut u32) -> HRESULT,
}

#[repr(C)]
struct Provider {
    init: *const InitializeWithStreamVtbl,
    thumbnail: *const ThumbnailProviderVtbl,
    refs: AtomicU32,
    /// The document, an `IStream` with a reference of ours.
    stream: AtomicPtr<c_void>,
}

static INIT_VTBL: InitializeWithStreamVtbl = InitializeWithStreamVtbl {
    base: IUnknown_Vtbl { QueryInterface: provider_query, AddRef: provider_add_ref, Release: provider_release },
    Initialize: initialize,
};

static THUMBNAIL_VTBL: ThumbnailProviderVtbl = ThumbnailProviderVtbl {
    base: IUnknown_Vtbl { QueryInterface: thumbnail_query, AddRef: thumbnail_add_ref, Release: thumbnail_release },
    GetThumbnail: get_thumbnail,
};

/// The object from its `IThumbnailProvider` pointer.
fn from_thumbnail(this: *mut c_void) -> *mut c_void {
    unsafe { this.cast::<u8>().sub(std::mem::offset_of!(Provider, thumbnail)).cast() }
}

unsafe extern "system" fn provider_query(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let provider = this as *mut Provider;
    let iid = unsafe { &*iid };
    let interface: *mut c_void = if same(iid, &IID_IUNKNOWN) || same(iid, &IID_IINITIALIZEWITHSTREAM) {
        this
    } else if same(iid, &IID_ITHUMBNAILPROVIDER) {
        unsafe { (&raw mut (*provider).thumbnail).cast() }
    } else {
        unsafe { *out = std::ptr::null_mut() };
        return E_NOINTERFACE;
    };
    unsafe {
        provider_add_ref(this);
        *out = interface;
    }
    S_OK
}

unsafe extern "system" fn provider_add_ref(this: *mut c_void) -> u32 {
    unsafe { (*(this as *mut Provider)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn provider_release(this: *mut c_void) -> u32 {
    let provider = this as *mut Provider;
    let left = unsafe { (*provider).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if left == 0 {
        let provider = unsafe { Box::from_raw(provider) };
        let stream = provider.stream.load(Ordering::Acquire);
        if !stream.is_null() {
            unsafe { (vtbl::<IStreamVtbl>(stream).base.Release)(stream) };
        }
    }
    left
}

unsafe extern "system" fn thumbnail_query(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    unsafe { provider_query(from_thumbnail(this), iid, out) }
}

unsafe extern "system" fn thumbnail_add_ref(this: *mut c_void) -> u32 {
    unsafe { provider_add_ref(from_thumbnail(this)) }
}

unsafe extern "system" fn thumbnail_release(this: *mut c_void) -> u32 {
    unsafe { provider_release(from_thumbnail(this)) }
}

unsafe extern "system" fn initialize(this: *mut c_void, stream: *mut c_void, _mode: u32) -> HRESULT {
    if stream.is_null() {
        return E_POINTER;
    }
    let provider = unsafe { &*(this as *mut Provider) };
    unsafe { (vtbl::<IStreamVtbl>(stream).base.AddRef)(stream) };
    if provider.stream.compare_exchange(std::ptr::null_mut(), stream, Ordering::AcqRel, Ordering::Acquire).is_err() {
        // Initialised once only.
        unsafe { (vtbl::<IStreamVtbl>(stream).base.Release)(stream) };
        return E_UNEXPECTED;
    }
    S_OK
}

unsafe extern "system" fn get_thumbnail(this: *mut c_void, side: u32, bitmap: *mut HBITMAP, alpha: *mut u32) -> HRESULT {
    if bitmap.is_null() || alpha.is_null() {
        return E_POINTER;
    }
    let provider = unsafe { &*(from_thumbnail(this) as *mut Provider) };
    let stream = provider.stream.load(Ordering::Acquire);
    if stream.is_null() {
        return E_UNEXPECTED;
    }
    let Some(picture) = (unsafe { picture_of(stream, side) }) else { return E_FAIL };
    let Some(hbitmap) = dib_of(&picture) else { return E_FAIL };
    unsafe {
        *bitmap = hbitmap;
        *alpha = WTSAT_RGB;
    }
    S_OK
}

/// The first page of the document in `stream`, its longer side `side`.
unsafe fn picture_of(stream: *mut c_void, side: u32) -> Option<pdfium::Picture> {
    let table = unsafe { vtbl::<IStreamVtbl>(stream) };
    let mut len = 0u64;
    if unsafe { (table.Seek)(stream, 0, SEEK_END, &mut len) } < 0 {
        return None;
    }
    let mut read = |position: u64, buf: &mut [u8]| -> bool {
        let Ok(offset) = i64::try_from(position) else { return false };
        if unsafe { (table.Seek)(stream, offset, SEEK_SET, std::ptr::null_mut()) } < 0 {
            return false;
        }
        let mut done = 0;
        while done < buf.len() {
            let mut got = 0u32;
            let want = u32::try_from(buf.len() - done).unwrap_or(u32::MAX);
            let hr = unsafe { (table.Read)(stream, buf[done..].as_mut_ptr().cast(), want, &mut got) };
            if hr < 0 || got == 0 {
                return false;
            }
            done += got as usize;
        }
        true
    };
    pdfium::first_page(&mut read, len, side)
}

/// The picture as a top-down 32-bit DIB section, opaque.
fn dib_of(picture: &pdfium::Picture) -> Option<HBITMAP> {
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: picture.width as i32,
        biHeight: -(picture.height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let hbitmap = unsafe { CreateDIBSection(std::ptr::null_mut(), &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0) };
    if hbitmap.is_null() || bits.is_null() {
        return None;
    }
    let out = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), picture.pixels.len()) };
    out.copy_from_slice(&picture.pixels);
    // BGRx: the fourth byte is whatever PDFium left.
    for pixel in out.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    Some(hbitmap)
}

/// COM asks for the class factory.
///
/// # Safety
/// Called by COM with valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = std::ptr::null_mut() };
    if !same(unsafe { &*clsid }, &CLSID_THUMBNAIL_PROVIDER) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    unsafe { factory_query((&raw const FACTORY).cast_mut().cast(), iid, out) }
}

/// Never unloaded: PDFium, once loaded and initialised, stays.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF of one page `w` × `h` points with a black square in its
    /// lower left quarter.
    fn sample_pdf(w: u32, h: u32) -> Vec<u8> {
        let content = format!("0 0 0 rg 0 0 {} {} re f", w / 2, h / 2);
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
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
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    /// `src/assoc.rs` registers the class the DLL serves.
    #[test]
    fn the_app_registers_this_class() {
        let assoc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/assoc.rs")).unwrap();
        assert!(assoc.contains("\"{46B6CBB3-FB97-4C67-9C29-8151C9DAFE44}\""));
    }

    #[test]
    fn only_its_own_class_and_interfaces() {
        let other = GUID::from_u128(0x12345678_0000_0000_0000_000000000000);
        let mut out = std::ptr::null_mut();
        assert_eq!(unsafe { DllGetClassObject(&other, &IID_ICLASSFACTORY, &mut out) }, CLASS_E_CLASSNOTAVAILABLE);
        assert_eq!(unsafe { DllGetClassObject(&CLSID_THUMBNAIL_PROVIDER, &other, &mut out) }, E_NOINTERFACE);
        assert_eq!(unsafe { DllGetClassObject(&CLSID_THUMBNAIL_PROVIDER, &IID_ICLASSFACTORY, &mut out) }, S_OK);
        let factory = unsafe { vtbl::<ClassFactoryVtbl>(out) };
        let mut object = std::ptr::null_mut();
        assert_eq!(unsafe { (factory.CreateInstance)(out, std::ptr::null_mut(), &other, &mut object) }, E_NOINTERFACE);
        assert!(object.is_null());
        assert_eq!(unsafe { (factory.CreateInstance)(out, std::ptr::null_mut(), &IID_ITHUMBNAILPROVIDER, &mut object) }, S_OK);
        // Not initialised: no thumbnail.
        let provider = unsafe { vtbl::<ThumbnailProviderVtbl>(object) };
        let (mut bitmap, mut alpha) = (std::ptr::null_mut(), 0);
        assert_eq!(unsafe { (provider.GetThumbnail)(object, 64, &mut bitmap, &mut alpha) }, E_UNEXPECTED);
        assert_eq!(unsafe { (provider.base.Release)(object) }, 0);
    }

    /// The provider through COM's calls, over a stream of a PDF in
    /// memory, as Explorer drives it: `cargo test pdfium -- --ignored`.
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_thumbnail_of_a_page() {
        use windows_sys::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW};
        use windows_sys::Win32::UI::Shell::SHCreateMemStream;
        let thumbnail = |pdf: &[u8], side: u32| -> Result<(BITMAP, Vec<u8>), HRESULT> {
            let stream = unsafe { SHCreateMemStream(pdf.as_ptr(), pdf.len() as u32) };
            assert!(!stream.is_null());
            let mut factory = std::ptr::null_mut();
            assert_eq!(unsafe { DllGetClassObject(&CLSID_THUMBNAIL_PROVIDER, &IID_ICLASSFACTORY, &mut factory) }, S_OK);
            let mut init = std::ptr::null_mut();
            let create = unsafe { vtbl::<ClassFactoryVtbl>(factory).CreateInstance };
            assert_eq!(unsafe { create(factory, std::ptr::null_mut(), &IID_IINITIALIZEWITHSTREAM, &mut init) }, S_OK);
            assert_eq!(unsafe { (vtbl::<InitializeWithStreamVtbl>(init).Initialize)(init, stream, 0) }, S_OK);
            // The provider keeps its own reference to the stream.
            unsafe { (vtbl::<IStreamVtbl>(stream).base.Release)(stream) };
            let mut object = std::ptr::null_mut();
            assert_eq!(unsafe { (vtbl::<InitializeWithStreamVtbl>(init).base.QueryInterface)(init, &IID_ITHUMBNAILPROVIDER, &mut object) }, S_OK);
            let (mut hbitmap, mut alpha) = (std::ptr::null_mut(), 0);
            let hr = unsafe { (vtbl::<ThumbnailProviderVtbl>(object).GetThumbnail)(object, side, &mut hbitmap, &mut alpha) };
            unsafe {
                (vtbl::<ThumbnailProviderVtbl>(object).base.Release)(object);
                assert_eq!((vtbl::<InitializeWithStreamVtbl>(init).base.Release)(init), 0);
            }
            if hr != S_OK {
                return Err(hr);
            }
            assert_eq!(alpha, WTSAT_RGB);
            let mut info: BITMAP = unsafe { std::mem::zeroed() };
            assert_ne!(unsafe { GetObjectW(hbitmap, size_of::<BITMAP>() as i32, (&raw mut info).cast()) }, 0);
            let bits = unsafe { std::slice::from_raw_parts(info.bmBits as *const u8, (info.bmWidthBytes * info.bmHeight) as usize) }.to_vec();
            unsafe { DeleteObject(hbitmap) };
            Ok((info, bits))
        };
        // A landscape page: 256 wide, half as high.
        let (info, bits) = thumbnail(&sample_pdf(400, 200), 256).unwrap();
        assert_eq!((info.bmWidth, info.bmHeight, info.bmBitsPixel), (256, 128, 32));
        let pixel = |x: i32, y: i32| &bits[(y * info.bmWidthBytes + x * 4) as usize..][..4];
        // Opaque everywhere; the black square at the lower left, white
        // paper above it and to its right.
        assert!(bits.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert_eq!(pixel(10, 120), [0, 0, 0, 255]);
        assert_eq!(pixel(10, 10), [255, 255, 255, 255]);
        assert_eq!(pixel(200, 120), [255, 255, 255, 255]);
        // A portrait page at a small size.
        let (info, _) = thumbnail(&sample_pdf(200, 400), 48).unwrap();
        assert_eq!((info.bmWidth, info.bmHeight), (24, 48));
        // Not a PDF: no thumbnail, Explorer shows the icon.
        assert_eq!(thumbnail(b"not a PDF at all", 96).err(), Some(E_FAIL));
    }
}
