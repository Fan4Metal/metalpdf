//! The system font that stands in for a document's font when text is
//! replaced and the document's own font is an embedded subset (it has
//! glyphs for the old text only). The stand-in keeps the style the base
//! font's name and flags tell: bold, italic, serif or monospaced, so that
//! a bold run stays bold. Windows' Arial, Times New Roman and Courier New
//! families, read from `%WINDIR%\Fonts` once and kept.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// What a base font's name and descriptor flags say of its look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub serif: bool,
    pub mono: bool,
    pub bold: bool,
    pub italic: bool,
}

// PDF font descriptor flags (`FPDFFont_GetFlags`).
const FLAG_FIXED_PITCH: i32 = 1;
const FLAG_ITALIC: i32 = 1 << 6;
const FLAG_FORCE_BOLD: i32 = 1 << 18;

/// The style of `base_font` (`ABCDEF+NotoSans-BoldItalic`, `Arial,Bold`,
/// `TimesNewRomanPS-BoldMT`…) with its descriptor `flags`. The serif
/// flag is not trusted: many sans fonts set it.
pub fn style_of(base_font: &str, flags: i32) -> Style {
    let name = base_font.split_once('+').map_or(base_font, |(_, n)| n).to_ascii_lowercase();
    let has = |s: &str| name.contains(s);
    Style {
        serif: has("times") || has("georgia") || has("garamond") || has("roman") || has("book") || has("cambria") || (has("serif") && !has("sans")),
        mono: has("mono") || has("courier") || has("consol") || flags & FLAG_FIXED_PITCH != 0,
        bold: has("bold") || has("black") || has("heavy") || has("semibold") || has("demibold") || flags & FLAG_FORCE_BOLD != 0,
        italic: has("italic") || has("oblique") || flags & FLAG_ITALIC != 0,
    }
}

/// The file in `%WINDIR%\Fonts` for `style`.
pub fn file_of(style: Style) -> &'static str {
    let table: [&str; 4] = if style.mono {
        ["cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf"]
    } else if style.serif {
        ["times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf"]
    } else {
        ["arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf"]
    };
    table[usize::from(style.bold) + 2 * usize::from(style.italic)]
}

/// The stand-in fonts read so far.
#[derive(Default)]
pub struct Fonts {
    files: HashMap<&'static str, Option<Arc<Vec<u8>>>>,
}

impl Fonts {
    fn dir() -> PathBuf {
        std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts")
    }

    fn file(&mut self, name: &'static str) -> Option<Arc<Vec<u8>>> {
        self.files.entry(name).or_insert_with(|| std::fs::read(Self::dir().join(name)).ok().map(Arc::new)).clone()
    }

    /// The stand-in for `base_font` with `flags`: the family and style
    /// nearest to it that is installed, else plain Arial.
    pub fn get(&mut self, base_font: &str, flags: i32) -> Option<Arc<Vec<u8>>> {
        let style = style_of(base_font, flags);
        let wanted = file_of(style);
        if let Some(bytes) = self.file(wanted) {
            return Some(bytes);
        }
        log::info!("font {wanted} not installed; Arial stands in for {base_font}");
        self.file("arial.ttf")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_follows_the_name_and_the_flags() {
        let plain = Style::default();
        assert_eq!(style_of("ABCDEF+NotoSans-Regular", 0), plain);
        assert_eq!(style_of("ABCDEF+NotoSans-Bold", 0), Style { bold: true, ..plain });
        assert_eq!(style_of("Arial,BoldItalic", 0), Style { bold: true, italic: true, ..plain });
        assert_eq!(style_of("TimesNewRomanPS-BoldMT", 0), Style { serif: true, bold: true, ..plain });
        assert_eq!(style_of("CourierNewPSMT", 0), Style { mono: true, ..plain });
        assert_eq!(style_of("Helvetica-Oblique", 0), Style { italic: true, ..plain });
        assert_eq!(style_of("DejaVuSansMono", 0), Style { mono: true, ..plain });
        assert_eq!(style_of("Foo", FLAG_ITALIC | FLAG_FORCE_BOLD), Style { bold: true, italic: true, ..plain });
        // The serif flag alone does not make a serif.
        assert_eq!(style_of("Calibri", 2), plain);
    }

    #[test]
    fn the_file_matches_the_style() {
        assert_eq!(file_of(Style::default()), "arial.ttf");
        assert_eq!(file_of(Style { bold: true, ..Default::default() }), "arialbd.ttf");
        assert_eq!(file_of(Style { italic: true, ..Default::default() }), "ariali.ttf");
        assert_eq!(file_of(Style { bold: true, italic: true, serif: true, ..Default::default() }), "timesbi.ttf");
        assert_eq!(file_of(Style { mono: true, bold: true, ..Default::default() }), "courbd.ttf");
    }

    #[test]
    fn windows_has_the_families() {
        let mut fonts = Fonts::default();
        assert!(fonts.get("ABCDEF+NotoSans-Bold", 0).is_some_and(|b| b.len() > 100_000));
        assert!(fonts.get("TimesNewRoman", 0).is_some());
        assert!(fonts.get("Courier", 0).is_some());
    }
}
