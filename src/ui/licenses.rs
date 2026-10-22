//! The Licenses window (from About): the program's own licence, PDFium's
//! and those of the libraries built into it, and the notices of the Rust
//! crates and fonts compiled into the exe. The texts are embedded, so the
//! exe carries them wherever it goes; the installer and the portable
//! archive ship them as files too. Long lines are wrapped at words to the
//! text's width beforehand ([`wrap`]): the lines are shown with
//! `show_rows`, which wants rows of one height. From MetalMPV.

use std::borrow::Cow;

use egui::{Color32, FontId, Key, Rect, RichText, Sense, pos2, vec2};

const METALPDF: &str = include_str!("../../LICENSE");
const THIRD_PARTY: &str = include_str!("../../THIRD_PARTY_LICENSES.md");
/// Written by tools/get_pdfium.py.
const PDFIUM: &str = include_str!("../../licenses/PDFium-LICENSE.txt");
/// Written by tools/notices.py.
pub const NOTICES: &str = include_str!("../../licenses/THIRD_PARTY_NOTICES.txt");

/// The window's content size in points.
const SIZE: egui::Vec2 = vec2(860.0, 540.0);
/// The width of the list on the left.
const LIST: f32 = 200.0;
const ROW: f32 = 15.0;
/// The font of the texts.
const FONT: f32 = 12.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Part {
    #[default]
    Program,
    Pdfium,
    Crates,
}

impl Part {
    const ALL: [Part; 3] = [Part::Program, Part::Pdfium, Part::Crates];

    fn title(self) -> &'static str {
        match self {
            Part::Program => "MetalPDF: MIT",
            Part::Pdfium => "PDFium: BSD",
            Part::Crates => tr!("Libraries and fonts", "Библиотеки и шрифты"),
        }
    }

    fn texts(self) -> &'static [&'static str] {
        match self {
            Part::Program => &[METALPDF],
            Part::Pdfium => &[THIRD_PARTY, PDFIUM],
            Part::Crates => &[NOTICES],
        }
    }
}

#[derive(Default)]
pub struct LicensesView {
    part: Part,
    /// The lines of the part shown, wrapped to `columns`.
    lines: Vec<Cow<'static, str>>,
    columns: usize,
}

impl LicensesView {
    /// Draw the window; whether it is to close (the cross, Esc, a click
    /// outside).
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let modal = egui::Modal::new(egui::Id::new("licenses")).show(ctx, |ui| {
            let screen = ctx.content_rect().size() - vec2(24.0, 24.0);
            let size = vec2(SIZE.x.min(screen.x.max(300.0)), SIZE.y.min(screen.y.max(200.0)));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let list = Rect::from_min_size(rect.min, vec2(LIST, rect.height()));
            let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list));
            list_ui.label(RichText::new(tr!("Licenses", "Лицензии")).size(18.0).strong());
            list_ui.add_space(8.0);
            for part in Part::ALL {
                if list_ui.selectable_label(self.part == part, part.title()).clicked() && self.part != part {
                    self.part = part;
                    self.lines.clear();
                }
            }
            list_ui.add_space(12.0);
            list_ui.label(
                RichText::new(tr!(
                    "MetalPDF's own code is under the MIT License. It is distributed together with PDFium, the PDF engine of Chromium, under the BSD 3-Clause License.",
                    "Собственный код MetalPDF — под лицензией MIT. Программа распространяется вместе с PDFium, движком PDF из Chromium, под лицензией BSD 3-Clause."
                ))
                .size(12.0)
                .weak(),
            );

            let text = Rect::from_min_max(pos2(list.right() + 12.0, rect.top() + 28.0), rect.max);
            let cross = Rect::from_min_size(pos2(rect.right() - 24.0, rect.top()), vec2(24.0, 24.0));
            let closed = super::close_cross(&mut ui.new_child(egui::UiBuilder::new().max_rect(cross)));
            ui.painter().rect_filled(text, 4.0, Color32::from_black_alpha(40));
            let inner = text.shrink(6.0);
            // As many characters as fit beside the scroll bar.
            let font = FontId::monospace(FONT);
            let glyph = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
            let room = inner.width() - ui.spacing().scroll.bar_width - 8.0;
            let columns = ((room / glyph).floor() as usize).max(20);
            if self.lines.is_empty() || columns != self.columns {
                self.columns = columns;
                self.split();
            }
            let mut text_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
            egui::ScrollArea::vertical().id_salt(("licenses_text", self.part as u8)).auto_shrink([false, false]).show_rows(
                &mut text_ui,
                ROW,
                self.lines.len(),
                |ui, rows| {
                    for line in &self.lines[rows] {
                        ui.add(egui::Label::new(RichText::new(line.as_ref()).font(font.clone())).extend());
                    }
                },
            );
            closed
        });
        modal.inner || modal.should_close() || ctx.input(|i| i.key_pressed(Key::Escape))
    }

    fn split(&mut self) {
        self.lines.clear();
        for (i, text) in self.part.texts().iter().enumerate() {
            if i > 0 {
                self.lines.extend([Cow::Borrowed(""), Cow::Borrowed("")]);
            }
            for line in text.lines() {
                wrap(line.trim_end_matches('\r'), self.columns, &mut self.lines);
            }
        }
    }
}

/// `line` in pieces of at most `columns` characters, broken after spaces
/// where it can be (else inside a word); the pieces after the first keep
/// the line's indent.
fn wrap(line: &'static str, columns: usize, out: &mut Vec<Cow<'static, str>>) {
    if line.chars().count() <= columns {
        out.push(Cow::Borrowed(line));
        return;
    }
    let indent = line.chars().take_while(|c| *c == ' ').count().min(columns / 2);
    let mut rest = line;
    let mut first = true;
    while !rest.is_empty() {
        let room = if first { columns } else { columns - indent };
        // The byte where the piece of `room` characters ends.
        let end = rest.char_indices().nth(room).map_or(rest.len(), |(i, _)| i);
        let cut = if end == rest.len() {
            end
        } else {
            // After the last space, if it leaves something on the line.
            match rest[..end].rfind(' ') {
                Some(space) if !rest[..space].trim().is_empty() => space + 1,
                _ => end,
            }
        };
        let piece = rest[..cut].trim_end();
        out.push(if first { Cow::Borrowed(piece) } else { Cow::Owned(format!("{}{piece}", " ".repeat(indent))) });
        rest = rest[cut..].trim_start_matches(' ');
        first = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tools/notices.py's hash of Cargo.lock: without carriage returns and
    /// without this package's own version (a release changes it).
    fn lock_hash(lock: &str) -> String {
        let lock = lock.replace('\r', "");
        let lines: Vec<&str> = lock.split('\n').collect();
        let kept: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter(|&(i, line)| !(i > 0 && lines[i - 1] == "name = \"metalpdf\"" && line.starts_with("version = ")))
            .map(|(_, line)| *line)
            .collect();
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        for b in kept.join("\n").bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        format!("{h:016x}")
    }

    #[test]
    fn notices_follow_cargo_lock() {
        let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock")).unwrap();
        let hash = lock_hash(&lock);
        assert!(
            NOTICES.contains(&format!("Cargo.lock FNV-1a 64: {hash}")),
            "licenses/THIRD_PARTY_NOTICES.txt is older than Cargo.lock: run python tools/notices.py"
        );
    }

    #[test]
    fn own_version_leaves_the_hash() {
        let lock = |v: &str| format!("[[package]]\r\nname = \"metalpdf\"\r\nversion = \"{v}\"\r\n");
        assert_eq!(lock_hash(&lock("0.1.0-dev")), lock_hash(&lock("0.1.0")));
        let other = |v: &str| format!("[[package]]\nname = \"egui\"\nversion = \"{v}\"\n");
        assert_ne!(lock_hash(&other("0.36.1")), lock_hash(&other("0.36.2")));
    }

    #[test]
    fn long_lines_wrap_at_words() {
        let mut out = Vec::new();
        wrap("short", 10, &mut out);
        wrap("  one two three four", 10, &mut out);
        wrap("abcdefghijklmnop", 6, &mut out);
        assert_eq!(out, ["short", "  one two", "  three", "  four", "abcdef", "ghijkl", "mnop"]);
        // Every line of the texts fits.
        let mut all = Vec::new();
        for text in [METALPDF, THIRD_PARTY, PDFIUM, NOTICES] {
            for line in text.lines() {
                wrap(line.trim_end_matches('\r'), 80, &mut all);
            }
        }
        assert!(all.iter().all(|l| l.chars().count() <= 80));
    }

    #[test]
    fn texts_are_there() {
        assert!(METALPDF.contains("MIT License"));
        assert!(PDFIUM.contains("Redistribution and use in source and binary forms"));
        assert!(NOTICES.contains("egui") && NOTICES.contains("OFL"));
    }
}
