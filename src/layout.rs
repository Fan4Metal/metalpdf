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

/// The zoom steps of the keyboard and the wheel: each step hits a cached
/// render again on the way back.
pub const ZOOM_STEPS: [f32; 17] =
    [0.1, 0.125, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0, 32.0];

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
    /// as its widest page plus the gaps.
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
        // The widest known page decides the fit, not the current one: the
        // zoom must not change while scrolling through pages of several
        // sizes.
        let widest = (0..count).map(page_size).fold(fallback, |w, s| if s[0] > w[0] { s } else { w });
        let fit_page = match mode {
            ViewMode::Single => page_size(current.min(count.saturating_sub(1))),
            ViewMode::Continuous => widest,
        };
        let scale = match zoom {
            ZoomSetting::Scale(z) => z.clamp(MIN_ZOOM, MAX_ZOOM) * POINT_SCALE,
            ZoomSetting::FitWidth => fit_width_scale(widest, viewport),
            ZoomSetting::FitPage => fit_page_scale(fit_page, viewport),
        };
        let mut pages = Vec::with_capacity(count);
        let mut y = GAP;
        let doc_width = (widest[0] * scale + 2.0 * GAP).max(viewport[0]);
        match mode {
            ViewMode::Continuous => {
                for i in 0..count {
                    let s = page_size(i);
                    let (w, h) = (s[0] * scale, s[1] * scale);
                    pages.push([(doc_width - w) / 2.0, y, w, h]);
                    y += h + GAP;
                }
            }
            ViewMode::Single => {
                // Only the current page has a place; the rest are off the
                // document (a zero-size rectangle keeps the indices).
                for i in 0..count {
                    if i == current {
                        let s = page_size(i);
                        let (w, h) = (s[0] * scale, s[1] * scale);
                        pages.push([(doc_width - w) / 2.0, GAP, w, h]);
                        y = GAP + h + GAP;
                    } else {
                        pages.push([0.0, 0.0, 0.0, 0.0]);
                    }
                }
            }
        }
        let height = y.max(viewport[1]);
        Layout { mode, scale, turns, pages, size: [doc_width, height] }
    }

    /// The scroll offset (document y at the top of the view) for `anchor`.
    pub fn scroll_of(&self, anchor: Anchor) -> f32 {
        let Some(page) = self.pages.get(anchor.page) else { return 0.0 };
        let y = page[1] + anchor.offset * page[3];
        y.clamp(0.0, (self.size[1] - 1.0).max(0.0))
    }

    /// The anchor for a scroll offset: the page the top of the view is on
    /// (or the gap above), and how far down it.
    pub fn anchor_at(&self, scroll: f32, viewport_height: f32) -> Anchor {
        if self.pages.is_empty() {
            return Anchor::default();
        }
        if self.mode == ViewMode::Single {
            let i = self.pages.iter().position(|p| p[3] > 0.0).unwrap_or(0);
            let p = self.pages[i];
            return Anchor { page: i, offset: if p[3] > 0.0 { (scroll - p[1]) / p[3] } else { 0.0 } };
        }
        let scroll = scroll.max(0.0);
        // The first page whose bottom is below the view's top; the gap above
        // a page belongs to that page (a negative offset).
        let i = self.pages.partition_point(|p| p[1] + p[3] <= scroll).min(self.pages.len() - 1);
        let p = self.pages[i];
        let offset = if p[3] > 0.0 { (scroll - p[1]) / p[3] } else { 0.0 };
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
            let i = self.pages.iter().position(|p| p[3] > 0.0).unwrap_or(0);
            return i..i + 1;
        }
        let top = scroll;
        let bottom = scroll + viewport_height;
        let first = self.pages.partition_point(|p| p[1] + p[3] <= top);
        let last = self.pages.partition_point(|p| p[1] < bottom);
        first.min(self.pages.len())..last.max(first).min(self.pages.len())
    }

    /// The page the reader is "on": the one covering the middle of the
    /// view, else the first visible one.
    pub fn current(&self, scroll: f32, viewport_height: f32) -> usize {
        let range = self.visible(scroll, viewport_height);
        if range.is_empty() {
            return 0;
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
        // Within the gap above page 3.
        let a = layout.anchor_at(layout.pages[3][1] - GAP / 2.0, 800.0);
        assert_eq!(a.page, 3);
        assert!(a.offset < 0.0);
        // The page's own top.
        let top = layout.scroll_of(Layout::anchor_to_page(7));
        assert!((top - layout.pages[7][1]).abs() < 0.01);
        assert_eq!(layout.anchor_at(top, 800.0).page, 7);
    }

    #[test]
    fn single_page_mode_places_one_page() {
        let layout = Layout::new(ViewMode::Single, ZoomSetting::FitPage, 0, &pages(5), A4, [1000.0, 800.0], 2);
        assert_eq!(layout.visible(0.0, 800.0), 2..3);
        assert_eq!(layout.pages[0], [0.0, 0.0, 0.0, 0.0]);
        assert!((layout.pages[2][3] - (800.0 - 2.0 * GAP)).abs() < 0.01);
        assert_eq!(layout.anchor_at(0.0, 800.0).page, 2);
    }

    #[test]
    fn zoom_steps_go_up_and_down_the_ladder() {
        assert_eq!(zoom_step(1.0, true), 1.25);
        assert_eq!(zoom_step(1.0, false), 0.75);
        assert_eq!(zoom_step(1.1, true), 1.25);
        assert_eq!(zoom_step(1.1, false), 1.0);
        assert_eq!(zoom_step(32.0, true), MAX_ZOOM);
        assert_eq!(zoom_step(0.1, false), MIN_ZOOM);
        assert_eq!(pixel_width(A4, 2.0 * POINT_SCALE, 1.5), 2380);
        assert_eq!(turned(A4, 1), [842.0, 595.0]);
        assert_eq!(turned(A4, 2), A4);
    }
}
