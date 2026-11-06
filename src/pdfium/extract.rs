//! What the export to Word and PowerPoint reads off a page: every
//! character of horizontal text with its box, font, size and colour, the
//! graphics (paths, pictures, shadings) with their boxes, and the page
//! rendered without the text and the pictures that go out as their own
//! (PowerPoint's slide background). Everything is in view space: points
//! from the top left corner of the page as shown (/Rotate applied), y
//! down, as `Page::page_to_view` maps it.

use std::collections::HashMap;
use std::ffi::{c_int, c_uint, c_ulong};

use super::{Bitmap, Page, Placement, sys};
use crate::export::{FontInfo, Glyph, Graphic, GraphicKind, PageContent};

/// The fonts of a whole document, numbered as the glyphs name them.
#[derive(Default)]
pub struct FontTable {
    pub fonts: Vec<FontInfo>,
    index: HashMap<(String, i32, i32, bool), u16>,
}

impl FontTable {
    fn id(&mut self, name: String, flags: i32, weight: i32, faux_bold: bool) -> u16 {
        let key = (name, flags, weight, faux_bold);
        if let Some(&id) = self.index.get(&key) {
            return id;
        }
        let id = u16::try_from(self.fonts.len()).unwrap_or(u16::MAX);
        if id < u16::MAX {
            self.fonts.push(FontInfo { name: key.0.clone(), flags, weight, faux_bold });
            self.index.insert(key, id);
        }
        id
    }
}

/// How a text object's characters are taken: as text, or left in the
/// picture of the page.
#[derive(Clone, Copy)]
struct ObjectInfo {
    font: u16,
    invisible: bool,
}

/// At most this many objects are looked at in a page's graphics, and
/// this many segments in a path: a map drawn with a million paths is a
/// picture anyway.
const MAX_GRAPHICS: usize = 20_000;
const MAX_SEGMENTS: c_int = 5_000;

impl Page {
    /// The page's form fields and annotations drawn into its content, the
    /// page loaded again; for a document opened for the export only.
    pub fn flattened(self) -> Option<Page> {
        if !self.flatten_for_print() {
            return Some(self);
        }
        let (doc, index) = (std::rc::Rc::clone(&self.doc), self.index);
        drop(self);
        let handle = unsafe { (doc.api.FPDF_LoadPage)(doc.handle, index) };
        if handle.is_null() {
            return None;
        }
        if !doc.form.is_null() {
            unsafe { (doc.api.FORM_OnAfterLoadPage)(handle, doc.form) };
        }
        Some(Page { doc, handle: std::cell::Cell::new(handle), index, text: Default::default(), rendering: Default::default() })
    }

    /// Whether character `i` of the text page goes out as text: a real
    /// character (not one PDFium made up), of horizontal text on the page
    /// as shown. Its matrix's direction is turned into view space by
    /// `to_view`.
    fn horizontal(&self, text: sys::FPDF_TEXTPAGE, i: c_int, to_view: &[f32; 6]) -> bool {
        let Some(m) = self.char_matrix(text, i) else { return true };
        let (dx, dy) = (m.a, m.b);
        let (vx, vy) = (to_view[0] * dx + to_view[2] * dy, to_view[1] * dx + to_view[3] * dy);
        vx > 0.0 && vy.abs() <= 0.08 * vx
    }

    /// Character `i`'s matrix: the text matrix and the page's, the font
    /// size not applied.
    fn char_matrix(&self, text: sys::FPDF_TEXTPAGE, i: c_int) -> Option<sys::FS_MATRIX> {
        let mut m = sys::FS_MATRIX::default();
        (unsafe { (self.api().FPDFText_GetMatrix)(text, i, &mut m) } != 0).then_some(m)
    }

    /// The quarter turns clockwise that make most of the page's text, as
    /// shown, run left to right (a table printed sideways in a book): 0
    /// when most of it does already, or when there is little text.
    fn text_turns(&self, text: sys::FPDF_TEXTPAGE, to_view: &[f32; 6]) -> u8 {
        let count = unsafe { (self.api().FPDFText_CountChars)(text) }.max(0);
        // Right, down, left, up: the direction the text runs on screen.
        let mut runs = [0usize; 4];
        for i in 0..count {
            let Some(m) = self.char_matrix(text, i) else { continue };
            let (vx, vy) = (to_view[0] * m.a + to_view[2] * m.b, to_view[1] * m.a + to_view[3] * m.b);
            let way = if vx.abs() >= vy.abs() { if vx > 0.0 { 0 } else { 2 } } else if vy > 0.0 { 1 } else { 3 };
            runs[way] += 1;
        }
        let total: usize = runs.iter().sum();
        let (way, most) = runs.iter().enumerate().max_by_key(|(_, n)| **n).map_or((0, 0), |(w, n)| (w, *n));
        if total < 4 || way == 0 || most * 5 < total * 3 {
            return 0;
        }
        // Text running down is turned back a quarter counter-clockwise
        // (three clockwise), text running up a quarter clockwise.
        [0, 3, 2, 1][way]
    }

    /// The text, the fonts and the graphics of the page, for the export.
    /// `turn`: the page may be turned so that most of its text runs left
    /// to right (`PageContent::turns`), for Word; PowerPoint keeps the
    /// page as shown.
    pub fn export_content(&self, fonts: &mut FontTable, turn: bool) -> PageContent {
        let api = self.api();
        let shown = self.page_to_view();
        let [w, h] = self.size();
        let turns = match self.text_page() {
            Some(text) if turn => self.text_turns(text, &shown),
            _ => 0,
        };
        // The view turned: x, y on the page as shown to x, y on it turned.
        let (turned, size) = match turns {
            1 => ([0.0, 1.0, -1.0, 0.0, h, 0.0], [h, w]),
            2 => ([-1.0, 0.0, 0.0, -1.0, w, h], [w, h]),
            3 => ([0.0, -1.0, 1.0, 0.0, 0.0, w], [h, w]),
            _ => ([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [w, h]),
        };
        let then = |a: [f32; 6], b: [f32; 6]| {
            // b after a.
            [
                b[0] * a[0] + b[2] * a[1],
                b[1] * a[0] + b[3] * a[1],
                b[0] * a[2] + b[2] * a[3],
                b[1] * a[2] + b[3] * a[3],
                b[0] * a[4] + b[2] * a[5] + b[4],
                b[1] * a[4] + b[3] * a[5] + b[5],
            ]
        };
        let to_view = then(shown, turned);
        let map = |x: f32, y: f32| [to_view[0] * x + to_view[2] * y + to_view[4], to_view[1] * x + to_view[3] * y + to_view[5]];
        let view_rect = |l: f32, b: f32, r: f32, t: f32| {
            let corners = [map(l, b), map(l, t), map(r, b), map(r, t)];
            corners.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, &[x, y]| [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)])
        };
        let mut content = PageContent { size, turns, ..Default::default() };
        if let Some(text) = self.text_page() {
            let count = unsafe { (api.FPDFText_CountChars)(text) }.max(0);
            let mut objects: HashMap<sys::FPDF_PAGEOBJECT, ObjectInfo> = HashMap::new();
            let mut space = false;
            // The origin of the last character taken, as PDFium gave it.
            let mut last_origin: Option<[f32; 2]> = None;
            let mut i = 0;
            while i < count {
                let code = unsafe { (api.FPDFText_GetUnicode)(text, i) };
                let index = i;
                i += 1;
                // A pair of surrogates is one character, with the first's box.
                let ch = if (0xD800..0xDC00).contains(&code) && i < count {
                    let low = unsafe { (api.FPDFText_GetUnicode)(text, i) };
                    if (0xDC00..0xE000).contains(&low) {
                        i += 1;
                        char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00))
                    } else {
                        None
                    }
                } else {
                    char::from_u32(code)
                };
                let Some(ch) = ch else { continue };
                // A line break PDFium worked out says nothing the boxes do
                // not; a space it worked out, or one in the text, parts
                // the words around it.
                if ch == '\r' || ch == '\n' {
                    continue;
                }
                if ch.is_whitespace() {
                    // Symbol fonts sometimes map a visible bullet to U+0020.
                    // Keep its ink as artwork instead of discarding it as a
                    // word separator (or emitting a wrong Unicode symbol).
                    if unsafe { (api.FPDFText_IsGenerated)(text, index) } == 0 {
                        let mut flags = 0;
                        let mut buf = [0u8; 256];
                        let len = unsafe { (api.FPDFText_GetFontInfo)(text, index, buf.as_mut_ptr().cast(), buf.len() as c_ulong, &mut flags) };
                        let name = String::from_utf8_lossy(&buf[..(len as usize).saturating_sub(1).min(buf.len())]).to_ascii_lowercase();
                        if name.contains("ding") || name.contains("symbol") {
                            let (mut l,mut r,mut b,mut t) = (0.0,0.0,0.0,0.0);
                            if unsafe { (api.FPDFText_GetCharBox)(text, index, &mut l, &mut r, &mut b, &mut t) } != 0 && r-l > 0.1 && t-b > 0.1 {
                                content.graphics.push(Graphic { rect: view_rect(l as f32,b as f32,r as f32,t as f32), kind: GraphicKind::Image, object: None });
                            }
                        }
                    }
                    space = true;
                    continue;
                }
                if unsafe { (api.FPDFText_IsGenerated)(text, index) } == 1 {
                    continue;
                }
                // Legacy math fonts can map a visible summation, integral,
                // or delimiter to a control code. Preserve its tight ink box
                // as artwork; it is not a discretionary word-ending hyphen.
                let symbol_control=ch=='\u{2}' && {
                    let mut flags=0;let mut buf=[0u8;256];
                    let len=unsafe {(api.FPDFText_GetFontInfo)(text,index,buf.as_mut_ptr().cast(),buf.len() as c_ulong,&mut flags)};
                    let name=String::from_utf8_lossy(&buf[..(len as usize).saturating_sub(1).min(buf.len())]).to_ascii_uppercase();
                    ["CMEX","CMSY","CMMI","MSBM","SYMBOL"].iter().any(|n|name.contains(n))
                };
                if ch.is_control() && (ch!='\u{2}' || symbol_control) || matches!(ch,'\u{fffd}'|'\u{fffe}'|'\u{ffff}') {
                    if unsafe {(api.FPDFText_IsGenerated)(text,index)}==0 && self.horizontal(text,index,&to_view) {
                        let (mut l,mut r,mut b,mut t)=(0.0,0.0,0.0,0.0);
                        if unsafe {(api.FPDFText_GetCharBox)(text,index,&mut l,&mut r,&mut b,&mut t)}!=0 && r-l>0.1 && t-b>0.1 {
                            content.graphics.push(Graphic {rect:view_rect(l as f32,b as f32,r as f32,t as f32),kind:GraphicKind::Image,object:None});
                        }
                    }
                    continue;
                }
                // PDFium reads a hyphen at a line's end as U+0002: a soft
                // hyphen here, dropped where the lines are joined.
                let ch = match ch {
                    '\u{2}' => '\u{ad}',
                    '\u{fffd}' | '\u{fffe}' | '\u{ffff}' => continue,
                    c if c.is_control() => continue,
                    c => c,
                };
                if !self.horizontal(text, index, &to_view) {
                    space = false;
                    continue;
                }
                let object = unsafe { (api.FPDFText_GetTextObject)(text, index) };
                let info = *objects.entry(object).or_insert_with(|| {
                    let mode = if object.is_null() { 0 } else { unsafe { (api.FPDFTextObj_GetTextRenderMode)(object) } };
                    let mut flags: c_int = 0;
                    let len = unsafe { (api.FPDFText_GetFontInfo)(text, index, std::ptr::null_mut(), 0, &mut flags) };
                    let mut buf = vec![0u8; len.max(1) as usize];
                    let len = unsafe { (api.FPDFText_GetFontInfo)(text, index, buf.as_mut_ptr().cast(), buf.len() as c_ulong, &mut flags) };
                    let name = String::from_utf8_lossy(&buf[..(len as usize).saturating_sub(1).min(buf.len())]).into_owned();
                    let weight = unsafe { (api.FPDFText_GetFontWeight)(text, index) };
                    let faux_bold = mode == sys::FPDF_TEXTRENDERMODE_FILL_STROKE;
                    ObjectInfo { font: fonts.id(name, flags, weight, faux_bold), invisible: mode == sys::FPDF_TEXTRENDERMODE_INVISIBLE || mode == sys::FPDF_TEXTRENDERMODE_CLIP }
                });
                let mut rect = sys::FS_RECTF::default();
                if unsafe { (api.FPDFText_GetLooseCharBox)(text, index, &mut rect) } == 0 {
                    continue;
                }
                let (mut ox, mut oy) = (0f64, 0f64);
                unsafe { (api.FPDFText_GetCharOrigin)(text, index, &mut ox, &mut oy) };
                let mut origin = map(ox as f32, oy as f32);
                let mut rect = view_rect(rect.left, rect.bottom, rect.right, rect.top);
                // The characters of a ligature (fi, fl, ffi) share its glyph's
                // origin and box: the second and later ones are put, with no
                // width, at its end, or the line broke there and a doubled
                // letter was taken for a duplicate.
                let raw_origin = origin;
                if !space
                    && let (Some(prev), Some(last)) = (content.glyphs.last(), last_origin)
                    && (last[0] - origin[0]).abs() < 0.01
                    && (last[1] - origin[1]).abs() < 0.01
                {
                    rect = [prev.rect[2], prev.rect[1], prev.rect[2], prev.rect[3]];
                    origin[0] = prev.rect[2];
                }
                last_origin = Some(raw_origin);
                // The size as drawn: the font's (`Tf`, often 1 under a
                // scaling matrix) times the matrix's vertical scale; one
                // far from what the box says (a font with odd metrics, no
                // matrix) gives way to the box's height.
                let raw = unsafe { (api.FPDFText_GetFontSize)(text, index) } as f32;
                let scale = self.char_matrix(text, index).map_or(1.0, |m| (m.c * m.c + m.d * m.d).sqrt());
                let from_box = ((rect[3] - rect[1]) / 1.15).max(1.0);
                let mut size = raw * scale;
                if !size.is_finite() || size <= 0.1 || !(0.5..=2.0).contains(&(size / from_box)) {
                    size = from_box;
                }
                let (mut r, mut g, mut b, mut a): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 255);
                unsafe { (api.FPDFText_GetFillColor)(text, index, &mut r, &mut g, &mut b, &mut a) };
                content.glyphs.push(Glyph {
                    source: index,
                    ch,
                    rect,
                    x: origin[0],
                    baseline: origin[1],
                    size,
                    font: info.font,
                    color: [r.min(255) as u8, g.min(255) as u8, b.min(255) as u8],
                    space_before: std::mem::take(&mut space),
                    invisible: info.invisible,
                });
            }
        }
        // The graphics, form XObjects looked into with their matrices.
        let count = unsafe { (api.FPDFPage_CountObjects)(self.handle.get()) }.max(0);
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        for index in 0..count {
            if content.graphics.len() >= MAX_GRAPHICS {
                break;
            }
            let object = unsafe { (api.FPDFPage_GetObject)(self.handle.get(), index) };
            self.graphics_of(object, identity, Some(index as usize), (0, None), &mut content.graphics, &view_rect);
        }
        content
    }

    /// `object`'s graphics into `out`: `matrix` takes its space (a form
    /// XObject's) to page space; `top` is its index on the page when it
    /// is a top-level object.
    fn graphics_of(&self, object: sys::FPDF_PAGEOBJECT, matrix: [f32; 6], top: Option<usize>, traversal: (u32, Option<[f32; 4]>), out: &mut Vec<Graphic>, view_rect: &dyn Fn(f32, f32, f32, f32) -> [f32; 4]) {
        let (depth, inherited_clip) = traversal;
        let api = self.api();
        if object.is_null() || out.len() >= MAX_GRAPHICS {
            return;
        }
        let kind = unsafe { (api.FPDFPageObj_GetType)(object) };
        let apply = |m: &[f32; 6], x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let intersect = |a: [f32; 4], b: [f32; 4]| {
            let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
            (r[2] >= r[0] && r[3] >= r[1]).then_some(r)
        };
        // Curves can extend far beyond a plot's clipping rectangle. Their
        // unclipped bounds otherwise swallow neighbouring paragraphs or are
        // discarded as a page background, losing the graph entirely.
        let mut clip = inherited_clip;
        let path = unsafe { (api.FPDFPageObj_GetClipPath)(object) };
        if !path.is_null() {
            for p in 0..unsafe { (api.FPDFClipPath_CountPaths)(path) }.clamp(0, 1024) {
                let n = unsafe { (api.FPDFClipPath_CountPathSegments)(path, p) }.clamp(0, MAX_SEGMENTS);
                let mut r = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
                for s in 0..n {
                    let segment = unsafe { (api.FPDFClipPath_GetPathSegment)(path, p, s) };
                    let (mut x, mut y) = (0.0, 0.0);
                    if unsafe { (api.FPDFPathSegment_GetPoint)(segment, &mut x, &mut y) } != 0 {
                        let (x, y) = apply(&matrix, x, y);
                        r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
                    }
                }
                if r.iter().all(|v| v.is_finite()) {
                    let r = view_rect(r[0], r[1], r[2], r[3]);
                    clip = match clip { Some(c) => match intersect(c, r) { Some(c) => Some(c), None => return }, None => Some(r) };
                }
            }
        }
        let bounds = || {
            let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
            if unsafe { (api.FPDFPageObj_GetBounds)(object, &mut l, &mut b, &mut r, &mut t) } == 0 {
                return None;
            }
            let pts = [apply(&matrix, l, b), apply(&matrix, l, t), apply(&matrix, r, b), apply(&matrix, r, t)];
            let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for (x, y) in pts {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let r = view_rect(x0, y0, x1, y1);
            match clip { Some(c) => intersect(r, c), None => Some(r) }
        };
        match kind {
            sys::FPDF_PAGEOBJ_PATH => {
                let Some(rect) = bounds() else { return };
                // Curves, or lines neither level nor upright: a drawing,
                // not a rule or a box.
                let n = unsafe { (api.FPDFPath_CountSegments)(object) }.clamp(0, MAX_SEGMENTS);
                let mut curved = false;
                let mut last: Option<(f32, f32)> = None;
                for s in 0..n {
                    let segment = unsafe { (api.FPDFPath_GetPathSegment)(object, s) };
                    if segment.is_null() {
                        continue;
                    }
                    let (mut x, mut y) = (0f32, 0f32);
                    unsafe { (api.FPDFPathSegment_GetPoint)(segment, &mut x, &mut y) };
                    match unsafe { (api.FPDFPathSegment_GetType)(segment) } {
                        sys::FPDF_SEGMENT_BEZIERTO => {
                            curved = true;
                            break;
                        }
                        sys::FPDF_SEGMENT_LINETO => {
                            if let Some((px, py)) = last
                                && (x - px).abs() > 0.5
                                && (y - py).abs() > 0.5
                            {
                                curved = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                    last = Some((x, y));
                }
                out.push(Graphic { rect, kind: GraphicKind::Path { curved }, object: None });
            }
            sys::FPDF_PAGEOBJ_IMAGE => {
                if let Some(rect) = bounds() {
                    out.push(Graphic { rect, kind: GraphicKind::Image, object: top });
                }
            }
            sys::FPDF_PAGEOBJ_SHADING => {
                if let Some(rect) = bounds() {
                    out.push(Graphic { rect, kind: GraphicKind::Shading, object: None });
                }
            }
            sys::FPDF_PAGEOBJ_FORM if depth < 8 => {
                let mut m = sys::FS_MATRIX { a: 1.0, d: 1.0, ..Default::default() };
                unsafe { (api.FPDFPageObj_GetMatrix)(object, &mut m) };
                // The form's matrix, then the one above it.
                let inner = [
                    matrix[0] * m.a + matrix[2] * m.b,
                    matrix[1] * m.a + matrix[3] * m.b,
                    matrix[0] * m.c + matrix[2] * m.d,
                    matrix[1] * m.c + matrix[3] * m.d,
                    matrix[0] * m.e + matrix[2] * m.f + matrix[4],
                    matrix[1] * m.e + matrix[3] * m.f + matrix[5],
                ];
                let n = unsafe { (api.FPDFFormObj_CountObjects)(object) }.max(0);
                for i in 0..n {
                    let child = unsafe { (api.FPDFFormObj_GetObject)(object, i as c_ulong) };
                    self.graphics_of(child, inner, None, (depth + 1, clip), out, view_rect);
                }
            }
            _ => {}
        }
    }

    /// Hide complete text objects represented by the selected character
    /// indices. Partial objects (figure labels, shadows, unmapped glyphs)
    /// stay in the artwork in their entirety. Return the indices hidden.
    pub fn hide_selected_text(&self, selected: &std::collections::HashSet<i32>) -> std::collections::HashSet<i32> {
        let Some(text) = self.text_page() else { return Default::default() };
        let api = self.api();
        let count = unsafe { (api.FPDFText_CountChars)(text) }.max(0);
        let mut objects: HashMap<sys::FPDF_PAGEOBJECT, (bool, Vec<i32>)> = HashMap::new();
        for i in 0..count {
            let code = unsafe { (api.FPDFText_GetUnicode)(text, i) };
            if unsafe { (api.FPDFText_IsGenerated)(text, i) } == 1 || char::from_u32(code).is_some_and(char::is_whitespace) || matches!(code, 0xDC00..=0xDFFF) {
                continue;
            }
            let object = unsafe { (api.FPDFText_GetTextObject)(text, i) };
            if object.is_null() {
                continue;
            }
            let entry = objects.entry(object).or_insert_with(|| {
                let mode = unsafe { (api.FPDFTextObj_GetTextRenderMode)(object) };
                (matches!(mode, sys::FPDF_TEXTRENDERMODE_FILL | sys::FPDF_TEXTRENDERMODE_FILL_STROKE | sys::FPDF_TEXTRENDERMODE_INVISIBLE), Vec::new())
            });
            entry.0 &= selected.contains(&i);
            let (mut r, mut g, mut b, mut alpha) = (0, 0, 0, 0);
            entry.0 &= unsafe { (api.FPDFText_GetFillColor)(text, i, &mut r, &mut g, &mut b, &mut alpha) } != 0 && alpha == 255;
            entry.1.push(i);
        }
        let mut hidden = std::collections::HashSet::new();
        for (object, (complete, indices)) in objects {
            if complete && unsafe { (api.FPDFTextObj_SetTextRenderMode)(object, sys::FPDF_TEXTRENDERMODE_INVISIBLE) } != 0 {
                hidden.extend(indices);
            }
        }
        hidden
    }

    /// For the picture of the page under PowerPoint's text: the text that
    /// goes out as text made invisible (what `export_content` took: real,
    /// horizontal, visible characters) and the top-level image objects
    /// `images` (indices) taken out. Only for a document opened for the
    /// export: nothing of this is saved.
    pub fn hide_for_background(&self, images: &[usize]) {
        let api = self.api();
        let to_view = self.page_to_view();
        if let Some(text) = self.text_page() {
            let count = unsafe { (api.FPDFText_CountChars)(text) }.max(0);
            let mut done = std::collections::HashSet::new();
            for i in 0..count {
                let code = unsafe { (api.FPDFText_GetUnicode)(text, i) };
                if char::from_u32(code).is_none_or(|c| c.is_whitespace() || c.is_control() && c != '\u{2}' || c == '\u{fffd}') {
                    continue;
                }
                if unsafe { (api.FPDFText_IsGenerated)(text, i) } == 1 || !self.horizontal(text, i, &to_view) {
                    continue;
                }
                let object = unsafe { (api.FPDFText_GetTextObject)(text, i) };
                if object.is_null() || !done.insert(object) {
                    continue;
                }
                unsafe { (api.FPDFTextObj_SetTextRenderMode)(object, sys::FPDF_TEXTRENDERMODE_INVISIBLE) };
            }
        }
        // The handles first: the indices move as objects go.
        let handles: Vec<sys::FPDF_PAGEOBJECT> = images.iter().map(|&i| unsafe { (api.FPDFPage_GetObject)(self.handle.get(), i as c_int) }).collect();
        for object in handles {
            if !object.is_null() && unsafe { (api.FPDFPageObj_GetType)(object) } == sys::FPDF_PAGEOBJ_IMAGE && unsafe { (api.FPDFPage_RemoveObject)(self.handle.get(), object) } != 0 {
                unsafe { (api.FPDFPageObj_Destroy)(object) };
            }
        }
    }

    /// The page as shown, turned `turns` quarters clockwise, `width` ×
    /// `height` pixels (of the page turned), BGRx on white.
    pub fn render_whole(&self, width: u32, height: u32, turns: u8) -> Option<Bitmap> {
        let mut bitmap = Bitmap::new(self.api(), width, height)?;
        self.render(&mut bitmap, Placement { start_x: 0, start_y: 0, size_x: width as i32, size_y: height as i32, rotate: i32::from(turns % 4) }, sys::FPDF_ANNOT);
        Some(bitmap)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn visible_legacy_control_glyphs_remain_artwork() {
        let stream=|s:&str|format!("<< /Length {} >>\nstream\n{s}\nendstream",s.len());
        let cmap="/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /Symbols def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <41> <0002> <42> <0003> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
        let bytes=crate::pdfium::tests::write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F 5 0 R >> >> >>".into(),
            stream("BT /F 20 Tf 100 650 Td (AB) Tj ET"),"<< /Type /Font /Subtype /Type1 /BaseFont /Symbol /ToUnicode 6 0 R >>".into(),stream(cmap)]);
        let path=std::env::temp_dir().join(format!("metalpdf_legacy_math_{}.pdf",std::process::id()));std::fs::write(&path,bytes).unwrap();
        let doc=crate::pdfium::Document::open(crate::pdfium::init().unwrap(),&path,None).unwrap();let page=doc.load_page(0).unwrap();
        let content=page.export_content(&mut Default::default(),false);
        assert!(content.glyphs.is_empty(),"{:?}",content.glyphs);assert_eq!(content.graphics.len(),2);
        assert!(content.graphics.iter().all(|g|g.rect[2]>g.rect[0]&&g.rect[3]>g.rect[1]));
        drop(page);drop(doc);std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn clipped_curves_and_translated_forms_keep_their_visible_bounds() {
        let stream = |s: &str| format!("<< /Length {} >>\nstream\n{s}\nendstream",s.len());
        let form = "q 10 20 90 60 re W n 0 0 m 50 120 100 -10000 140 100 c S Q";
        let bytes = crate::pdfium::tests::write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /XObject << /F 5 0 R >> >> >>".into(),
            stream("q 40 300 120 100 re W n 0 0 m 100 400 120 -10000 180 360 c S Q /F Do"),
            format!("<< /Type /XObject /Subtype /Form /BBox [0 0 150 150] /Matrix [1 0 0 1 200 100] /Resources << >> /Length {} >>\nstream\n{form}\nendstream",form.len()),
        ]);
        let path = std::env::temp_dir().join(format!("metalpdf_clipped_plot_{}.pdf",std::process::id()));
        std::fs::write(&path,bytes).unwrap();
        let doc = crate::pdfium::Document::open(crate::pdfium::init().unwrap(),&path,None).unwrap();
        let page = doc.load_page(0).unwrap();
        let content = page.export_content(&mut Default::default(),false);
        assert_eq!(content.graphics.len(),2,"{:?}",content.graphics);
        for (g,wanted) in content.graphics.iter().zip([[40.0,400.0,160.0,500.0],[210.0,620.0,300.0,680.0]]) {
            assert!(g.rect[0]>=wanted[0]-0.1 && g.rect[1]>=wanted[1]-0.1 && g.rect[2]<=wanted[2]+0.1 && g.rect[3]<=wanted[3]+0.1,"{:?}",g.rect);
            assert!(g.rect[2]>g.rect[0] && g.rect[3]>g.rect[1]);
        }
        drop(page);drop(doc);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_docx_preserves_translucent_text_in_artwork() {
        let input = crate::pdfium::tests::sample_file("export-translucent");
        let api = crate::pdfium::init().unwrap();
        let doc = crate::pdfium::Document::open(api, &input, None).unwrap();
        let page = doc.load_page(0).unwrap();
        let object = unsafe { (api.FPDFPage_GetObject)(page.handle.get(), 0) };
        assert_ne!(unsafe { (api.FPDFPageObj_SetFillColor)(object, 0, 0, 0, 96) }, 0);
        let content = page.export_content(&mut Default::default(), false);
        let selected = content.glyphs.iter().map(|g| g.source).collect();
        let hidden = page.hide_selected_text(&selected);
        assert!(!hidden.is_empty()); // the ordinary second line is editable
        assert!(content.glyphs.iter().filter(|g| g.size > 20.0).all(|g| !hidden.contains(&g.source)));
        assert_eq!(unsafe { (api.FPDFTextObj_GetTextRenderMode)(object) }, super::sys::FPDF_TEXTRENDERMODE_FILL);
        drop(page);
        drop(doc);
        std::fs::remove_file(input).unwrap();
    }
}
