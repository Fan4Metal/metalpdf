//! Colours that depend on the theme, in one place: the area round the
//! pages, the page's edge and shadow, the highlights of the search and the
//! selection. The interface's own colours are egui's, in the dark theme
//! with the panels and bars of the dark style chosen (`apply_dark_style`):
//! MetalMD's greys or GitHub's near black.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Theme, Visuals};

use crate::settings::DarkStyle;

/// The dark theme is the black one: `Palette::of` reads it, where only
/// the context is at hand.
static BLACK: AtomicBool = AtomicBool::new(false);

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
    pub fn new(dark: bool, style: DarkStyle) -> Palette {
        let rgb = Color32::from_rgb;
        if dark && style == DarkStyle::Black {
            // GitHub's: the pages on a backdrop a shade lighter than the
            // panels, as in the grey one.
            let blue = rgb(0x44, 0x93, 0xf8);
            Palette {
                dark,
                backdrop: rgb(0x16, 0x1b, 0x22),
                page_edge: rgb(0x01, 0x04, 0x09),
                shadow: Color32::from_black_alpha(120),
                paper: rgb(0xf4, 0xf4, 0xf4),
                text: rgb(0xe6, 0xed, 0xf3),
                muted: rgb(0x91, 0x98, 0xa1),
                link: blue,
                search: Color32::from_rgba_unmultiplied(0xff, 0xd7, 0x00, 0x60),
                search_current: Color32::from_rgba_unmultiplied(0xff, 0x8c, 0x00, 0x90),
                selection: Color32::from_rgba_unmultiplied(0x38, 0x8b, 0xfd, 0x66),
                error: rgb(0xf8, 0x51, 0x49),
                accent: blue,
            }
        } else if dark {
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
        let style = if BLACK.load(Ordering::Relaxed) { DarkStyle::Black } else { DarkStyle::Grey };
        Palette::new(ctx.theme() == Theme::Dark, style)
    }
}

/// The dark theme in `style`: egui's dark visuals with the panels, the
/// bars (and text fields) and the lines of MetalMD's palette, grey or
/// black; the page area follows through `Palette::of`. The light theme is
/// egui's own.
pub fn apply_dark_style(ctx: &egui::Context, style: DarkStyle) {
    BLACK.store(style == DarkStyle::Black, Ordering::Relaxed);
    let rgb = Color32::from_rgb;
    // The panels, the bars (a shade darker), the faint rows, the lines.
    let (panel, chrome, faint, border) = match style {
        DarkStyle::Grey => (rgb(0x20, 0x22, 0x27), rgb(0x1c, 0x1e, 0x22), rgb(0x29, 0x2c, 0x32), rgb(0x3a, 0x3e, 0x46)),
        DarkStyle::Black => (rgb(0x0d, 0x11, 0x17), rgb(0x01, 0x04, 0x09), rgb(0x15, 0x1b, 0x23), rgb(0x3d, 0x44, 0x4d)),
    };
    let palette = Palette::new(true, style);
    ctx.style_mut_of(Theme::Dark, |s| {
        let mut visuals = Visuals::dark();
        visuals.panel_fill = panel;
        visuals.window_fill = panel;
        visuals.extreme_bg_color = chrome;
        visuals.faint_bg_color = faint;
        visuals.widgets.noninteractive.fg_stroke.color = palette.text;
        visuals.widgets.noninteractive.bg_stroke.color = border;
        visuals.window_stroke.color = border;
        visuals.hyperlink_color = palette.link;
        s.visuals = visuals;
    });
}
