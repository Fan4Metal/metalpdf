//! Text put into the XML of a Word or PowerPoint file.

/// `text` escaped for an element's text or an attribute's value, the
/// characters XML 1.0 forbids (control characters but tab, line feed
/// and carriage return; U+FFFE, U+FFFF) left out: one of them makes
/// Office call the file damaged.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The XML declaration every part starts with.
pub const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_escaped_and_forbidden_characters_dropped() {
        assert_eq!(esc("a<b & \"c\"\u{2}\u{fffe}д"), "a&lt;b &amp; &quot;c&quot;д");
    }
}
