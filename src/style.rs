//! Colours that depend on the theme, in one place: the area round the
//! pages, the page's edge and shadow, the highlights of the search and the
//! selection. The interface's own colours are egui's.

use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Behind the pages.
    pub backdrop: Color32,
    /// A page's edge.
    pub page_edge: Color32,
    /// A page's shadow.
    pub shadow: Color32,
    /// The placeholder of a page not yet rendered.
    pub paper: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub link: Color32,
    /// Found text.
    pub search: Color32,
    /// The found text the reader is at.
    pub search_current: Color32,
    pub selection: Color32,
    pub error: Color32,
    /// The current page in the sidebar.
    pub accent: Color32,
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        let rgb = Color32::from_rgb;
        if dark {
            Palette {
                dark,
                backdrop: rgb(0x2a, 0x2c, 0x31),
                page_edge: rgb(0x16, 0x17, 0x1a),
                shadow: Color32::from_black_alpha(90),
                paper: rgb(0xf4, 0xf4, 0xf4),
                text: rgb(0xd4, 0xd7, 0xdc),
                muted: rgb(0x8a, 0x8f, 0x98),
                link: rgb(0x58, 0xa6, 0xff),
                search: Color32::from_rgba_unmultiplied(0xff, 0xd7, 0x00, 0x60),
                search_current: Color32::from_rgba_unmultiplied(0xff, 0x8c, 0x00, 0x90),
                selection: Color32::from_rgba_unmultiplied(0x38, 0x8b, 0xfd, 0x66),
                error: rgb(0xf0, 0x6a, 0x6a),
                accent: rgb(0x58, 0xa6, 0xff),
            }
        } else {
            Palette {
                dark,
                backdrop: rgb(0xe3, 0xe5, 0xe8),
                page_edge: rgb(0xb8, 0xbc, 0xc2),
                shadow: Color32::from_black_alpha(40),
                paper: Color32::WHITE,
                text: rgb(0x1f, 0x23, 0x28),
                muted: rgb(0x59, 0x63, 0x6e),
                link: rgb(0x09, 0x69, 0xda),
                search: Color32::from_rgba_unmultiplied(0xff, 0xd7, 0x00, 0x70),
                search_current: Color32::from_rgba_unmultiplied(0xff, 0x8c, 0x00, 0xa0),
                selection: Color32::from_rgba_unmultiplied(0x54, 0xae, 0xff, 0x66),
                error: rgb(0xc8, 0x2b, 0x2b),
                accent: rgb(0x09, 0x69, 0xda),
            }
        }
    }

    pub fn of(ctx: &egui::Context) -> Palette {
        Palette::new(ctx.theme() == egui::Theme::Dark)
    }
}
