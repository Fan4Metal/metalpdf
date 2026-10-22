//! The system font that stands in for a document's font when text is
//! replaced and the document's own font cannot show it (an embedded
//! subset has glyphs for the old text only). The stand-in is looked for
//! among the fonts installed on Windows by name: the PDF's base font name
//! is usually the font's PostScript name (`ArialNarrow-Bold`,
//! `TimesNewRomanPS-BoldMT`, `Roboto-Bold`), which the TrueType files'
//! name tables give, so the very font comes back when it is installed;
//! else the family in the wanted weight and slant; else Arial, Times New
//! Roman or Courier New after the look the name and the descriptor flags
//! tell, so that a bold run stays bold. The name tables are read once per
//! process (a few milliseconds for some hundred files) and the fonts'
//! bytes kept.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
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

/// The base font name without a subset tag (`ABCDEF+`).
pub fn untagged(base_font: &str) -> &str {
    match base_font.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base_font,
    }
}

/// The style of `base_font` with its descriptor `flags`. The serif flag
/// is not trusted: many sans fonts set it.
pub fn style_of(base_font: &str, flags: i32) -> Style {
    let name = untagged(base_font).to_ascii_lowercase();
    let has = |s: &str| name.contains(s);
    Style {
        serif: has("times") || has("georgia") || has("garamond") || has("roman") || has("book") || has("cambria") || (has("serif") && !has("sans")),
        mono: has("mono") || has("courier") || has("consol") || flags & FLAG_FIXED_PITCH != 0,
        bold: has("bold") || has("black") || has("heavy") || has("semibold") || has("demibold") || flags & FLAG_FORCE_BOLD != 0,
        italic: has("italic") || has("oblique") || flags & FLAG_ITALIC != 0,
    }
}

/// The file in `%WINDIR%\Fonts` of the generic family for `style`.
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

/// The generic family that stands in for `base_font` with `flags` when
/// it is not installed, named for people.
pub fn generic_name(base_font: &str, flags: i32) -> &'static str {
    match file_of(style_of(base_font, flags)) {
        "arialbd.ttf" => "Arial Bold",
        "ariali.ttf" => "Arial Italic",
        "arialbi.ttf" => "Arial Bold Italic",
        "times.ttf" => "Times New Roman",
        "timesbd.ttf" => "Times New Roman Bold",
        "timesi.ttf" => "Times New Roman Italic",
        "timesbi.ttf" => "Times New Roman Bold Italic",
        "cour.ttf" => "Courier New",
        "courbd.ttf" => "Courier New Bold",
        "couri.ttf" => "Courier New Italic",
        "courbi.ttf" => "Courier New Bold Italic",
        _ => "Arial",
    }
}

/// Letters and digits only, lower case: how names are compared.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The PDF's base font name as a PostScript-like name of an installed
/// font: the subset tag off, the standard 14 names turned into the
/// Windows families that stand for them.
fn installed_name(base_font: &str) -> String {
    let name = untagged(base_font);
    let (family, style) = name.split_once(['-', ',']).unwrap_or((name, ""));
    let family = match family.to_ascii_lowercase().as_str() {
        "helvetica" => "Arial",
        "times" | "timesnewroman" | "times new roman" => "TimesNewRoman",
        "courier" => "CourierNew",
        _ => family,
    };
    let style = match style {
        "Roman" => "",
        s => s,
    };
    if style.is_empty() { family.to_owned() } else { format!("{family}-{style}") }
}

/// A PostScript or full name with the `PS`, `MT` and `PSMT` tails some
/// names carry (`TimesNewRomanPS-BoldMT`) taken off each part.
fn plain_name(name: &str) -> String {
    name.split(['-', ',', ' '])
        .map(|part| {
            let mut p = part;
            for tail in ["PSMT", "MT", "PS"] {
                if let Some(s) = p.strip_suffix(tail)
                    && !s.is_empty()
                {
                    p = s;
                }
            }
            p
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// What a font file's name table says of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Names {
    pub family: String,
    pub subfamily: String,
    pub full: String,
    pub postscript: String,
}

impl Names {
    fn bold(&self) -> bool {
        let s = self.subfamily.to_ascii_lowercase();
        s.contains("bold") || s.contains("black") || s.contains("heavy")
    }

    fn italic(&self) -> bool {
        let s = self.subfamily.to_ascii_lowercase();
        s.contains("italic") || s.contains("oblique")
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

/// The names of the first font in a TrueType file (or collection): the
/// table directory, then the `name` table alone are read.
pub fn read_names(path: &Path) -> Option<Names> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 16];
    file.read_exact(&mut head).ok()?;
    // A collection: the first font's offset table.
    let base = if &head[..4] == b"ttcf" { be32(&head, 12)? as u64 } else { 0 };
    file.seek(SeekFrom::Start(base)).ok()?;
    let mut dir = [0u8; 12];
    file.read_exact(&mut dir).ok()?;
    let tables = be16(&dir, 4)? as usize;
    let mut records = vec![0u8; tables.min(64) * 16];
    file.read_exact(&mut records).ok()?;
    let (offset, length) = (0..tables.min(64)).find_map(|i| {
        let r = &records[i * 16..i * 16 + 16];
        (&r[..4] == b"name").then(|| (be32(r, 8), be32(r, 12)))
    })?;
    let (offset, length) = (offset? as u64, length? as usize);
    if length > 1 << 20 {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut table = vec![0u8; length];
    file.read_exact(&mut table).ok()?;
    parse_names(&table)
}

/// The family, subfamily, full and PostScript names from a `name` table:
/// Windows Unicode English strings preferred, Macintosh ones else.
fn parse_names(table: &[u8]) -> Option<Names> {
    let count = be16(table, 2)? as usize;
    let strings = be16(table, 4)? as usize;
    let mut names = Names::default();
    // (nameID, rank of the record taken) so that a better record replaces a worse one.
    let mut ranks = [0u8; 7];
    for i in 0..count {
        let r = 6 + i * 12;
        let (platform, encoding, language, id) = (be16(table, r)?, be16(table, r + 2)?, be16(table, r + 4)?, be16(table, r + 6)?);
        let (len, off) = (be16(table, r + 8)? as usize, be16(table, r + 10)? as usize);
        if !matches!(id, 1 | 2 | 4 | 6) {
            continue;
        }
        let rank = match (platform, encoding, language) {
            (3, 1, 0x409) => 3,
            (3, 1, _) | (3, 10, _) | (0, _, _) => 2,
            (1, 0, _) => 1,
            _ => 0,
        };
        if rank == 0 || rank <= ranks[id as usize] {
            continue;
        }
        let bytes = table.get(strings + off..strings + off + len)?;
        let text = if platform == 1 { bytes.iter().map(|&b| b as char).collect::<String>() } else { utf16be(bytes) };
        let text = text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        ranks[id as usize] = rank;
        match id {
            1 => names.family = text,
            2 => names.subfamily = text,
            4 => names.full = text,
            _ => names.postscript = text,
        }
    }
    (!names.family.is_empty()).then_some(names)
}

fn utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

/// An installed TrueType font, by its names.
#[derive(Clone, Debug)]
pub struct Installed {
    pub path: PathBuf,
    pub names: Names,
}

/// The folders fonts are installed in: the system's and the user's.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(windir) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(windir).join("Fonts"));
    } else {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
    }
    dirs
}

/// Every `.ttf` in `dirs` with its names (collections and OpenType CFF
/// fonts are left out: PDFium embeds TrueType outlines).
pub fn scan(dirs: &[PathBuf]) -> Vec<Installed> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ttf")) {
                continue;
            }
            if let Some(names) = read_names(&path) {
                out.push(Installed { path, names });
            }
        }
    }
    out
}

/// The installed font for `base_font` (a PDF's base font name) in
/// `style`: by PostScript name, by full name, by family and style, by
/// family alone.
pub fn find<'a>(installed: &'a [Installed], base_font: &str, style: Style) -> Option<&'a Installed> {
    let name = installed_name(base_font);
    let wanted_ps = [norm(&name), norm(&plain_name(&name))];
    let family = norm(&plain_name(name.split(['-', ',']).next().unwrap_or(&name)));
    if family.is_empty() {
        return None;
    }
    let by_ps = installed.iter().find(|f| wanted_ps.contains(&norm(&f.names.postscript)) || wanted_ps.contains(&norm(&plain_name(&f.names.postscript))));
    if by_ps.is_some() {
        return by_ps;
    }
    let by_full = installed.iter().find(|f| wanted_ps.contains(&norm(&f.names.full)) || wanted_ps.contains(&norm(&plain_name(&f.names.full))));
    if by_full.is_some() {
        return by_full;
    }
    let of_family: Vec<&Installed> = installed.iter().filter(|f| norm(&f.names.family) == family).collect();
    of_family
        .iter()
        .find(|f| f.names.bold() == style.bold && f.names.italic() == style.italic)
        .or_else(|| of_family.iter().find(|f| f.names.bold() == style.bold))
        .or_else(|| of_family.first())
        .copied()
}

/// The stand-in fonts: the installed fonts' names, read once, and the
/// bytes of those used so far.
#[derive(Default)]
pub struct Fonts {
    installed: Option<Vec<Installed>>,
    files: HashMap<PathBuf, Option<Arc<Vec<u8>>>>,
    /// The installed font's full name by base font name, for the editor's
    /// tooltip.
    names: HashMap<String, Option<String>>,
}

impl Fonts {
    /// The full name of the installed font that would stand in for
    /// `base_font`; `None` when only a generic family would.
    pub fn stand_in_name(&mut self, base_font: &str, flags: i32) -> Option<String> {
        if let Some(name) = self.names.get(base_font) {
            return name.clone();
        }
        let style = style_of(base_font, flags);
        let name = find(self.installed(), base_font, style).map(|f| f.names.full.clone());
        self.names.insert(base_font.to_owned(), name.clone());
        name
    }
}

impl Fonts {
    fn installed(&mut self) -> &[Installed] {
        self.installed.get_or_insert_with(|| {
            let started = std::time::Instant::now();
            let fonts = scan(&font_dirs());
            log::debug!("{} installed fonts read in {:.1} ms", fonts.len(), started.elapsed().as_secs_f64() * 1e3);
            fonts
        })
    }

    fn file(&mut self, path: PathBuf) -> Option<Arc<Vec<u8>>> {
        self.files.entry(path.clone()).or_insert_with(|| std::fs::read(&path).ok().map(Arc::new)).clone()
    }

    /// The stand-in for `base_font` with descriptor `flags`: the installed
    /// font of that name or family, else the generic family in that style,
    /// else plain Arial.
    pub fn get(&mut self, base_font: &str, flags: i32) -> Option<Arc<Vec<u8>>> {
        let style = style_of(base_font, flags);
        if let Some(found) = find(self.installed(), base_font, style).map(|f| f.path.clone()) {
            log::debug!("{base_font}: {} stands in", found.display());
            if let Some(bytes) = self.file(found) {
                return Some(bytes);
            }
        }
        let generic = file_of(style);
        let dir = font_dirs().into_iter().next().unwrap_or_default();
        log::debug!("{base_font}: no such family installed; {generic} stands in");
        if let Some(bytes) = self.file(dir.join(generic)) {
            return Some(bytes);
        }
        self.file(dir.join("arial.ttf"))
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
    fn the_generic_file_matches_the_style() {
        assert_eq!(file_of(Style::default()), "arial.ttf");
        assert_eq!(file_of(Style { bold: true, ..Default::default() }), "arialbd.ttf");
        assert_eq!(file_of(Style { italic: true, ..Default::default() }), "ariali.ttf");
        assert_eq!(file_of(Style { bold: true, italic: true, serif: true, ..Default::default() }), "timesbi.ttf");
        assert_eq!(file_of(Style { mono: true, bold: true, ..Default::default() }), "courbd.ttf");
    }

    #[test]
    fn names_are_made_comparable() {
        assert_eq!(installed_name("ABCDEF+ArialNarrow-Bold"), "ArialNarrow-Bold");
        assert_eq!(installed_name("Helvetica-Bold"), "Arial-Bold");
        assert_eq!(installed_name("Times-Roman"), "TimesNewRoman");
        assert_eq!(installed_name("Times-BoldItalic"), "TimesNewRoman-BoldItalic");
        assert_eq!(installed_name("Courier"), "CourierNew");
        assert_eq!(plain_name("TimesNewRomanPS-BoldMT"), "TimesNewRoman-Bold");
        assert_eq!(plain_name("ArialMT"), "Arial");
        assert_eq!(norm("Times New Roman Bold"), "timesnewromanbold");
    }

    #[test]
    fn windows_fonts_are_read_and_found_by_name() {
        let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("Fonts");
        let arial = read_names(&dir.join("arialbd.ttf")).unwrap();
        assert_eq!(arial.family, "Arial");
        assert_eq!(arial.subfamily, "Bold");
        assert_eq!(arial.postscript, "Arial-BoldMT");
        assert!(arial.bold() && !arial.italic());
        let installed = scan(std::slice::from_ref(&dir));
        assert!(installed.len() > 50, "{}", installed.len());
        let file = |f: &Installed| f.path.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
        // The PostScript name, with and without the subset tag and the MT tail.
        assert_eq!(file(find(&installed, "ABCDEF+Arial-BoldMT", style_of("Arial-BoldMT", 0)).unwrap()), "arialbd.ttf");
        assert_eq!(file(find(&installed, "Arial-Bold", style_of("Arial-Bold", 0)).unwrap()), "arialbd.ttf");
        assert_eq!(file(find(&installed, "Arial,BoldItalic", style_of("Arial,BoldItalic", 0)).unwrap()), "arialbi.ttf");
        // The standard 14.
        assert_eq!(file(find(&installed, "Helvetica-Bold", style_of("Helvetica-Bold", 0)).unwrap()), "arialbd.ttf");
        assert_eq!(file(find(&installed, "TimesNewRomanPS-BoldMT", style_of("TimesNewRomanPS-BoldMT", 0)).unwrap()), "timesbd.ttf");
        assert_eq!(file(find(&installed, "Times-Roman", style_of("Times-Roman", 0)).unwrap()), "times.ttf");
        assert_eq!(file(find(&installed, "CourierNewPSMT", style_of("CourierNewPSMT", 0)).unwrap()), "cour.ttf");
        // A family in a style: Calibri Bold ships with Windows.
        assert_eq!(file(find(&installed, "Calibri-Bold", style_of("Calibri-Bold", 0)).unwrap()), "calibrib.ttf");
        assert_eq!(file(find(&installed, "Calibri,Bold", style_of("Calibri,Bold", 0)).unwrap()), "calibrib.ttf");
        // Not installed: nothing, and the generic family stands in.
        assert!(find(&installed, "NoSuchFont-Bold", style_of("NoSuchFont-Bold", 0)).is_none());
        let mut fonts = Fonts::default();
        assert!(fonts.get("NoSuchFont-Bold", 0).is_some_and(|b| b.len() > 100_000));
        assert!(fonts.get("ABCDEF+Calibri-Bold", 0).is_some());
    }
}
