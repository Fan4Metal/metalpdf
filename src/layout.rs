//! The geometry of the pages on screen, pure and tested: where every page
//! of a document goes at a zoom and a rotation, which pages are in view,
//! and the anchor the scroll position is kept as, so that neither page
//! sizes arriving late nor a change of zoom moves what the reader looks at.
//! Lengths are in points of the view (egui's logical points) unless named
//! otherwise; page sizes come in PDF points (1/72 inch).

use crate::settings::{ViewMode, ZoomSetting};

/// Zoom factor 1.0 is 100 %: a PDF point as 96/72 of a pixel at 100 %
/// display scaling (a logical point of the view), as Acrobat counts it.
pub const POINT_SCALE: f32 = 96.0 / 72.0;
pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 64.0;
/// The gap between pages and round them.
pub const GAP: f32 = 8.0;
/// How far above the view's top a page's bottom may be, in points, and
/// still count as passed (`Layout::anchor_at`): rounding, far less than
/// the gap.
const ANCHOR_SLACK: f32 = 0.5;

/// The zoom steps of the keyboard and the wheel: each step hits a cached
/// render again on the way back.
pub const ZOOM_STEPS: &[f32] = &[
    0.1, 0.125, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6, 1.8, 2.0, 2.25, 2.5, 3.0, 3.5, 4.0, 5.0, 6.0, 7.0, 8.0, 10.0, 12.0, 14.0,
    16.0, 20.0, 24.0, 32.0, 48.0, 64.0,
];

/// The next step up or down from `zoom`; a zoom between steps goes to the
/// nearest step in that direction.
pub fn zoom_step(zoom: f32, up: bool) -> f32 {
    if up {
        ZOOM_STEPS.iter().copied().find(|&s| s > zoom * 1.001).unwrap_or(MAX_ZOOM).min(MAX_ZOOM)
    } else {
        ZOOM_STEPS.iter().rev().copied().find(|&s| s < zoom * 0.999).unwrap_or(MIN_ZOOM).max(MIN_ZOOM)
    }
}

/// A page's size in PDF points after `turns` quarter turns.
pub fn turned(size: [f32; 2], turns: u8) -> [f32; 2] {
    if turns % 2 == 1 { [size[1], size[0]] } else { size }
}

/// Where the reader is: a page and how far down it the top of the view
/// lies, as a share of the page's height (negative within the gap above
/// it). Kept instead of an absolute scroll offset, so that pages above it
/// may change size (as their sizes arrive) without moving the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub page: usize,
    pub offset: f32,
}

impl Default for Anchor {
    fn default() -> Self {
        Anchor { page: 0, offset: 0.0 }
    }
}

/// The pages laid out for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub mode: ViewMode,
    /// Points of the view per PDF point.
    pub scale: f32,
    pub turns: u8,
    /// Each page's rectangle in document space: `[x, y, w, h]`, y from the
    /// top of the document, x from its left edge; the document is as wide
    /// as its widest page plus the gaps (in single-page mode, as the page
    /// shown), and at least as the view.
    pub pages: Vec<[f32; 4]>,
    /// The whole document's size in view points.
    pub size: [f32; 2],
}

impl Layout {
    /// Lay the pages out. `sizes` are the pages' sizes in PDF points,
    /// `None` for those not yet known (they take `fallback`); `viewport`
    /// is the view's size in points, which the fitting zooms refer to.
    pub fn new(
        mode: ViewMode,
        zoom: ZoomSetting,
        turns: u8,
        sizes: &[Option<[f32; 2]>],
        fallback: [f32; 2],
        viewport: [f32; 2],
        current: usize,
    ) -> Layout {
        let page_size = |i: usize| turned(sizes.get(i).copied().flatten().unwrap_or(fallback), turns);
        let count = sizes.len();
        let widest = (0..count).map(page_size).fold(turned(fallback, turns), |w, s| if s[0] > w[0] { s } else { w });
        // What the view is made for, the fits and the document's width: in
        // continuous mode the widest known page, not the current one, so
        // that the zoom does not change while scrolling through pages of
        // several sizes; in single-page mode the page shown, each fitted
        // on its own (fitted to the widest, a book's pages came out less
        // than half the view's width behind a wide cover, and stood off to
        // the side of a document as wide as it).
        let column = match mode {
            ViewMode::Single => page_size(current.min(count.saturating_sub(1))),
            ViewMode::Continuous => widest,
        };
        let scale = match zoom {
            ZoomSetting::Scale(z) => z.clamp(MIN_ZOOM, MAX_ZOOM) * POINT_SCALE,
            ZoomSetting::FitWidth => fit_width_scale(column, viewport),
            ZoomSetting::FitPage => fit_page_scale(column, viewport),
        };
        let mut pages = Vec::with_capacity(count);
        let mut y = GAP;
        let doc_width = (column[0] * scale + 2.0 * GAP).max(viewport[0]);
        // Every page has its place in both modes, so that the scroll bar
        // shows where in the document the reader is; single-page mode
        // shows the one page the view's top is on and keeps the scroll
        // within it (`page_scroll_range`). Pages are centred on the
        // document's axis (in single-page mode a page wider than the one
        // shown, never drawn, starts left of 0).
        for i in 0..count {
            let s = page_size(i);
            let (w, h) = (s[0] * scale, s[1] * scale);
            pages.push([(doc_width - w) / 2.0, y, w, h]);
            y += h + GAP;
        }
        let height = y.max(viewport[1]);
        Layout { mode, scale, turns, pages, size: [doc_width, height] }
    }

    /// The scroll offset (document y at the top of the view) for `anchor`:
    /// offset 0 puts the gap above the page at the top of the view, so a
    /// page jumped to keeps its margin.
    pub fn scroll_of(&self, anchor: Anchor) -> f32 {
        let Some(page) = self.pages.get(anchor.page) else { return 0.0 };
        let y = page[1] - GAP + anchor.offset * page[3];
        y.clamp(0.0, (self.size[1] - 1.0).max(0.0))
    }

    /// The anchor for a scroll offset: the page the top of the view is on
    /// (or the gap above), and how far down it.
    pub fn anchor_at(&self, scroll: f32, viewport_height: f32) -> Anchor {
        if self.pages.is_empty() {
            return Anchor::default();
        }
        let scroll = scroll.max(0.0);
        // The first page whose bottom is below the view's top; the gap above
        // a page belongs to that page. A page's top (`scroll_of`, `p[1] -
        // GAP`) and the bottom of the page before (`p[1] + p[3]`) are sums
        // that may differ in the last bit, and the top read back as the
        // page before, to which single-page mode then went back: the wheel
        // could not leave a page. A bottom within `ANCHOR_SLACK` counts as
        // passed.
        let i = self.pages.partition_point(|p| p[1] + p[3] <= scroll + ANCHOR_SLACK).min(self.pages.len() - 1);
        let p = self.pages[i];
        let offset = if p[3] > 0.0 { (scroll - p[1] + GAP) / p[3] } else { 0.0 };
        let _ = viewport_height;
        Anchor { page: i, offset }
    }

    /// The pages that show in a view `viewport_height` tall scrolled to
    /// `scroll`, in order.
    pub fn visible(&self, scroll: f32, viewport_height: f32) -> std::ops::Range<usize> {
        if self.pages.is_empty() {
            return 0..0;
        }
        if self.mode == ViewMode::Single {
            // The one page the view's top is on.
            let i = self.anchor_at(scroll, viewport_height).page;
            return i..i + 1;
        }
        let top = scroll;
        let bottom = scroll + viewport_height;
        let first = self.pages.partition_point(|p| p[1] + p[3] <= top);
        let last = self.pages.partition_point(|p| p[1] < bottom);
        first.min(self.pages.len())..last.max(first).min(self.pages.len())
    }

    /// Single-page mode: the scroll offsets that show page `index` alone,
    /// from its top (with the gap above) to its bottom (with the gap
    /// below) at the view's bottom; a page shorter than the view sits at
    /// the top.
    pub fn page_scroll_range(&self, index: usize, viewport_height: f32) -> (f32, f32) {
        let Some(p) = self.pages.get(index) else { return (0.0, 0.0) };
        let top = p[1] - GAP;
        let bottom = (p[1] + p[3] + GAP - viewport_height).max(top);
        (top, bottom)
    }

    /// `scroll` kept within page `page`, in single-page mode; as it is in
    /// continuous mode.
    pub fn clamp_scroll(&self, scroll: f32, page: usize, viewport_height: f32) -> f32 {
        if self.mode != ViewMode::Single || self.pages.is_empty() {
            return scroll;
        }
        let (top, bottom) = self.page_scroll_range(page.min(self.pages.len() - 1), viewport_height);
        scroll.clamp(top, bottom.max(top))
    }

    /// The page the reader is "on": the one covering the middle of the
    /// view, else the first visible one; in single-page mode the one shown.
    pub fn current(&self, scroll: f32, viewport_height: f32) -> usize {
        let range = self.visible(scroll, viewport_height);
        if range.is_empty() {
            return 0;
        }
        if self.mode == ViewMode::Single {
            return range.start;
        }
        let middle = scroll + viewport_height / 2.0;
        range
            .clone()
            .find(|&i| {
                let p = self.pages[i];
                middle >= p[1] - GAP / 2.0 && middle < p[1] + p[3] + GAP / 2.0
            })
            .unwrap_or(range.start)
    }

    /// The anchor that puts the top of page `index` at the top of the view.
    pub fn anchor_to_page(index: usize) -> Anchor {
        Anchor { page: index, offset: 0.0 }
    }
}

/// The scale (view points per PDF point) that fits `size` into the width.
pub fn fit_width_scale(size: [f32; 2], viewport: [f32; 2]) -> f32 {
    let room = (viewport[0] - 2.0 * GAP).max(1.0);
    (room / size[0].max(1.0)).clamp(MIN_ZOOM * POINT_SCALE, MAX_ZOOM * POINT_SCALE)
}

/// The scale that fits `size` whole into the view.
pub fn fit_page_scale(size: [f32; 2], viewport: [f32; 2]) -> f32 {
    let room = [(viewport[0] - 2.0 * GAP).max(1.0), (viewport[1] - 2.0 * GAP).max(1.0)];
    (room[0] / size[0].max(1.0)).min(room[1] / size[1].max(1.0)).clamp(MIN_ZOOM * POINT_SCALE, MAX_ZOOM * POINT_SCALE)
}

/// The zoom factor shown for a scale (1.0 = 100 %).
pub fn zoom_of_scale(scale: f32) -> f32 {
    scale / POINT_SCALE
}

/// A render's pixel width for a page `size` (PDF points) at `scale` on a
/// display of `ppp` pixels per point: the key renders are made and cached
/// by.
pub fn pixel_width(size: [f32; 2], scale: f32, ppp: f32) -> u32 {
    (size[0] * scale * ppp).round().clamp(1.0, 1.0e6) as u32
}

/// The pixel width a page of `size` (PDF points, before the viewer's
/// `turns`) is rendered at for `zoom` in a view of `viewport` points on a
/// display of `ppp` pixels per point. The engine's first render and the
/// UI's first frame both come here, so they agree on the key.
pub fn render_width(size: [f32; 2], zoom: ZoomSetting, turns: u8, viewport: [f32; 2], ppp: f32) -> u32 {
    let turned = turned(size, turns);
    let scale = match zoom {
        ZoomSetting::Scale(z) => z.clamp(MIN_ZOOM, MAX_ZOOM) * POINT_SCALE,
        ZoomSetting::FitWidth => fit_width_scale(turned, viewport),
        ZoomSetting::FitPage => fit_page_scale(turned, viewport),
    };
    pixel_width(turned, scale, ppp)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: [f32; 2] = [595.0, 842.0];

    fn pages(n: usize) -> Vec<Option<[f32; 2]>> {
        vec![Some(A4); n]
    }

    #[test]
    fn fit_width_fills_the_view() {
        let layout = Layout::new(ViewMode::Continuous, ZoomSetting::FitWidth, 0, &pages(3), A4, [1000.0, 800.0], 0);
        assert!((layout.pages[0][2] - (1000.0 - 2.0 * GAP)).abs() < 0.01);
        assert_eq!(layout.pages[0][0], GAP);
        assert_eq!(layout.pages[1][1], GAP + layout.pages[0][3] + GAP);
        assert_eq!(layout.size[0], 1000.0);
        assert!((layout.size[1] - (GAP + 3.0 * (layout.pages[0][3] + GAP))).abs() < 0.01);
    }

    #[test]
    fn turned_landscape_pages_fit_their_turned_width() {
        // A slide deck turned upright: the fit follows the turned size,
        // not the unturned fallback's (which was the widest before).
        let slide = [792.0, 612.0];
        let layout = Layout::new(ViewMode::Continuous, ZoomSetting::FitWidth, 1, &[Some(slide); 2], slide, [1000.0, 800.0], 0);
        assert!((layout.pages[0][2] - (1000.0 - 2.0 * GAP)).abs() < 0.01, "{:?}", layout.pages[0]);
        let page = Layout::new(ViewMode::Continuous, ZoomSetting::FitPage, 1, &[Some(slide); 2], slide, [1000.0, 800.0], 0);
        assert!((page.pages[0][3] - (800.0 - 2.0 * GAP)).abs() < 0.01, "{:?}", page.pages[0]);
    }

    #[test]
    fn fit_page_fits_the_height_of_a_portrait_page() {
        let layout = Layout::new(ViewMode::Continuous, ZoomSetting::FitPage, 0, &pages(1), A4, [1000.0, 800.0], 0);
        assert!((layout.pages[0][3] - (800.0 - 2.0 * GAP)).abs() < 0.01);
        // Turned on its side it fits the width instead.
        let turned = Layout::new(ViewMode::Continuous, ZoomSetting::FitPage, 1, &pages(1), A4, [1000.0, 800.0], 0);
        assert!(turned.pages[0][2] > turned.pages[0][3]);
        assert!(turned.pages[0][3] <= 800.0 - 2.0 * GAP + 0.01);
    }

    #[test]
    fn a_fixed_zoom_counts_points_as_acrobat() {
        let layout = Layout::new(ViewMode::Continuous, ZoomSetting::Scale(1.0), 0, &pages(1), A4, [400.0, 300.0], 0);
        assert!((layout.pages[0][2] - 595.0 * POINT_SCALE).abs() < 0.01);
        // Narrower than the page: the document is as wide as the page.
        assert!((layout.size[0] - (595.0 * POINT_SCALE + 2.0 * GAP)).abs() < 0.01);
        assert_eq!(zoom_of_scale(layout.scale), 1.0);
    }

    #[test]
    fn unknown_sizes_take_the_fallback_and_the_anchor_holds() {
        let viewport = [1000.0, 800.0];
        let mut sizes = pages(10);
        sizes[1] = None;
        sizes[2] = None;
        let before = Layout::new(ViewMode::Continuous, ZoomSetting::Scale(1.0), 0, &sizes, A4, viewport, 0);
        let anchor = Anchor { page: 5, offset: 0.3 };
        let scroll = before.scroll_of(anchor);
        // Page 5 is at the same place in the view whatever pages 1 and 2
        // turn out to be.
        sizes[1] = Some([595.0, 1500.0]);
        sizes[2] = Some([300.0, 300.0]);
        let after = Layout::new(ViewMode::Continuous, ZoomSetting::Scale(1.0), 0, &sizes, A4, viewport, 0);
        let scroll2 = after.scroll_of(anchor);
        assert!((after.pages[5][1] - scroll2 - (before.pages[5][1] - scroll)).abs() < 0.01);
        assert_ne!(scroll, scroll2);
        // And the anchor is read back from the scroll offset.
        let read = after.anchor_at(scroll2, 800.0);
        assert_eq!(read.page, 5);
        assert!((read.offset - 0.3).abs() < 1e-4);
    }

    #[test]
    fn visible_pages_and_the_current_one() {
        let viewport = [1000.0, 800.0];
        let layout = Layout::new(ViewMode::Continuous, ZoomSetting::Scale(1.0), 0, &pages(10), A4, viewport, 0);
        let h = layout.pages[0][3] + GAP;
        assert_eq!(layout.visible(0.0, 800.0), 0..1);
        // Scrolled a little into page 0: pages 0 and 1 show.
        assert_eq!(layout.visible(h * 0.5, 800.0), 0..2);
        assert_eq!(layout.current(h * 0.5, 800.0), 0);
        assert_eq!(layout.current(h * 0.9, 800.0), 1);
        // Within the gap above page 3: that page, less than a gap down.
        let a = layout.anchor_at(layout.pages[3][1] - GAP / 2.0, 800.0);
        assert_eq!(a.page, 3);
        assert!(a.offset > 0.0 && a.offset * layout.pages[3][3] < GAP);
        // The page jumped to keeps its gap above.
        let top = layout.scroll_of(Layout::anchor_to_page(7));
        assert!((top - (layout.pages[7][1] - GAP)).abs() < 0.01);
        assert_eq!(layout.anchor_at(top, 800.0).page, 7);
        assert_eq!(layout.scroll_of(Anchor::default()), 0.0);
    }

    #[test]
    fn single_page_mode_shows_one_page_of_the_whole_document() {
        let layout = Layout::new(ViewMode::Single, ZoomSetting::FitPage, 0, &pages(5), A4, [1000.0, 800.0], 2);
        // Every page has its place (the scroll bar spans the document),
        // the current one fits the view.
        assert!(layout.pages[0][3] > 0.0 && layout.pages[4][1] > layout.pages[2][1]);
        assert!((layout.pages[2][3] - (800.0 - 2.0 * GAP)).abs() < 0.01);
        let at_page_2 = layout.scroll_of(Layout::anchor_to_page(2));
        assert_eq!(layout.visible(at_page_2, 800.0), 2..3);
        assert_eq!(layout.current(at_page_2, 800.0), 2);
        assert_eq!(layout.anchor_at(at_page_2, 800.0).page, 2);
        // The scroll stays within the page shown: a fitting page at its top.
        let (top, bottom) = layout.page_scroll_range(2, 800.0);
        assert_eq!(top, layout.pages[2][1] - GAP);
        assert!((bottom - top).abs() < 0.01, "a page that fits has one position");
        assert_eq!(layout.clamp_scroll(top + 300.0, 2, 800.0), top);
        // A page taller than the view scrolls from its top to its bottom.
        let tall = Layout::new(ViewMode::Single, ZoomSetting::Scale(2.0), 0, &pages(5), A4, [1000.0, 800.0], 1);
        let (top, bottom) = tall.page_scroll_range(1, 800.0);
        assert!(bottom > top);
        assert!((bottom - (tall.pages[1][1] + tall.pages[1][3] + GAP - 800.0)).abs() < 0.01);
        assert_eq!(tall.clamp_scroll(bottom + 500.0, 1, 800.0), bottom);
        // Continuous mode clamps nothing.
        let flow = Layout::new(ViewMode::Continuous, ZoomSetting::Scale(2.0), 0, &pages(5), A4, [1000.0, 800.0], 1);
        assert_eq!(flow.clamp_scroll(1234.5, 1, 800.0), 1234.5);
    }

    #[test]
    fn a_page_top_reads_back_as_that_page() {
        // A book whose cover is a fifth of a point wider than its pages,
        // fitted in single-page mode: the offset of a page's top, the
        // previous page's bottom, read back as the previous page for some
        // view sizes (`p[1] - GAP` against `p[1] + p[3]` of the one before
        // differ in the last bit), and the wheel could not leave page 1.
        let mut sizes = vec![Some([576.0, 720.0]); 304];
        sizes[0] = Some([576.2, 720.0]);
        sizes[3] = Some([576.1, 719.9]);
        let mut bad = Vec::new();
        for mode in [ViewMode::Single, ViewMode::Continuous] {
            for zoom in [ZoomSetting::FitPage, ZoomSetting::FitWidth, ZoomSetting::Scale(1.05)] {
                for tenth in 0..2000 {
                    let viewport = [1000.0 + tenth as f32 * 0.37, 700.0 + tenth as f32 * 0.1];
                    for page in [1, 2, 4, 150, 303] {
                        let layout = Layout::new(mode, zoom, 0, &sizes, [576.0, 720.0], viewport, page);
                        let scroll = layout.clamp_scroll(layout.scroll_of(Layout::anchor_to_page(page)), page, viewport[1]);
                        if layout.anchor_at(scroll, viewport[1]).page != page {
                            bad.push((mode, zoom, viewport, page));
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{} of them, first {:?}", bad.len(), bad.first());
    }

    #[test]
    fn single_page_mode_is_as_wide_as_the_page_shown() {
        // A book of narrow pages behind a wide cover.
        let mut sizes = vec![Some([373.0, 574.0]); 5];
        sizes[0] = Some([800.0, 1177.0]);
        let viewport = [900.0, 900.0];
        let layout = Layout::new(ViewMode::Single, ZoomSetting::FitPage, 0, &sizes, [373.0, 574.0], viewport, 2);
        // The page fits, so the document is the view, the page in its middle.
        assert_eq!(layout.size[0], viewport[0]);
        let p = layout.pages[2];
        assert!((p[0] + p[2] / 2.0 - viewport[0] / 2.0).abs() < 0.01, "{p:?}");
        // The cover shown at the same scale is wider than the view: the
        // document is as wide as it.
        let scale = ZoomSetting::Scale(zoom_of_scale(layout.scale));
        let cover = Layout::new(ViewMode::Single, scale, 0, &sizes, [373.0, 574.0], viewport, 0);
        assert!((cover.size[0] - (cover.pages[0][2] + 2.0 * GAP)).abs() < 0.01);
        // Continuous mode keeps the widest page's width.
        let flow = Layout::new(ViewMode::Continuous, scale, 0, &sizes, [373.0, 574.0], viewport, 2);
        assert_eq!(flow.size[0], cover.size[0]);
        // Fit width fits the page shown, the narrow one and the cover
        // alike; continuous mode fits the widest, wherever the reader is.
        for page in [0, 2] {
            let single = Layout::new(ViewMode::Single, ZoomSetting::FitWidth, 0, &sizes, [373.0, 574.0], viewport, page);
            assert!((single.pages[page][2] - (viewport[0] - 2.0 * GAP)).abs() < 0.01, "{page}: {:?}", single.pages[page]);
            let flow = Layout::new(ViewMode::Continuous, ZoomSetting::FitWidth, 0, &sizes, [373.0, 574.0], viewport, page);
            assert!((flow.pages[0][2] - (viewport[0] - 2.0 * GAP)).abs() < 0.01);
        }
    }

    #[test]
    fn zoom_steps_go_up_and_down_the_ladder() {
        assert_eq!(zoom_step(1.0, true), 1.1);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(1.15, true), 1.25);
        assert_eq!(zoom_step(1.15, false), 1.1);
        assert_eq!(zoom_step(48.0, true), MAX_ZOOM);
        assert_eq!(zoom_step(64.0, true), MAX_ZOOM);
        assert_eq!(zoom_step(0.1, false), MIN_ZOOM);
        assert_eq!(pixel_width(A4, 2.0 * POINT_SCALE, 1.5), 2380);
        assert_eq!(turned(A4, 1), [842.0, 595.0]);
        assert_eq!(turned(A4, 2), A4);
    }
}
