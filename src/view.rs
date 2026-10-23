//! How a document is being looked at: the mode, the zoom, the turns and
//! where the reader is (an anchor, `layout::Anchor`), plus the zoom
//! gesture's timer, which holds new renders back while the wheel is
//! turning. Pure state with the sums that move it; the drawing is in
//! `ui/view.rs`.

use std::time::{Duration, Instant};

use crate::layout::{self, Anchor, Layout};
use crate::settings::{FilePosition, ViewMode, ZoomSetting};

/// New full renders wait this long after the last zoom step or resize:
/// meanwhile the renders at hand are drawn scaled.
pub const GESTURE_SETTLE: Duration = Duration::from_millis(120);

#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    pub mode: ViewMode,
    pub zoom: ZoomSetting,
    /// Quarter turns clockwise.
    pub turns: u8,
    pub anchor: Anchor,
    /// Horizontal scroll offset in points (a page wider than the view).
    pub scroll_x: f32,
    /// The page area's size in points, as last laid out.
    pub viewport: [f32; 2],
    /// Until when renders at a new scale are held back.
    pub settle_until: Option<Instant>,
    /// The direction of the last page change: prefetch goes that way.
    pub forward: bool,
    /// The page the reader is on, from the last frame.
    pub current: usize,
    /// Wheel notches not yet turned into a page flip (single-page mode;
    /// a high-resolution wheel sends a notch in pieces).
    pub wheel_acc: f32,
    /// Wheel notches not yet turned into a zoom step.
    pub zoom_acc: f32,
    /// Single-page mode: how far a drag of the pages has pulled past the
    /// page's edge, down positive (`drag_single`).
    pub pull: f32,
}

/// How far the hand pulls past a page's edge, in points, to turn it in
/// single-page mode.
pub const PULL: f32 = 80.0;

/// What a wheel notch scrolls within a page taller than the view, in
/// points (egui's `line_scroll_speed`, as set in `App::new`).
pub const WHEEL_STEP: f32 = 100.0;
/// The pinch gesture's factor that counts as one notch.
const PINCH_NOTCH: f32 = 0.4;

impl ViewState {
    pub fn new(mode: ViewMode, zoom: ZoomSetting) -> ViewState {
        ViewState {
            mode,
            zoom,
            turns: 0,
            anchor: Anchor::default(),
            scroll_x: 0.0,
            viewport: [800.0, 600.0],
            settle_until: None,
            forward: true,
            current: 0,
            wheel_acc: 0.0,
            pull: 0.0,
            zoom_acc: 0.0,
        }
    }

    /// Where the reader left off, from a saved position.
    pub fn restore(&mut self, position: &FilePosition) {
        self.zoom = position.zoom;
        self.turns = position.rotation % 4;
        self.anchor = Anchor { page: position.page, offset: position.offset };
        self.current = position.page;
    }

    pub fn position(&self, path: &std::path::Path) -> FilePosition {
        FilePosition {
            path: path.to_path_buf(),
            page: self.anchor.page,
            offset: self.anchor.offset,
            zoom: self.zoom,
            rotation: self.turns,
            page_shift: 0,
        }
    }

    /// The layout for `sizes` in the view as last seen.
    pub fn layout(&self, sizes: &[Option<[f32; 2]>], fallback: [f32; 2]) -> Layout {
        Layout::new(self.mode, self.zoom, self.turns, sizes, fallback, self.viewport, self.anchor.page)
    }

    /// Whether renders at the current scale may be asked for.
    pub fn settled(&self) -> bool {
        self.settle_until.is_none_or(|t| Instant::now() >= t)
    }

    fn unsettle(&mut self) {
        self.settle_until = Some(Instant::now() + GESTURE_SETTLE);
    }

    /// Go to the top of `page`.
    pub fn go_to_page(&mut self, page: usize, count: usize) {
        let page = page.min(count.saturating_sub(1));
        self.forward = page >= self.anchor.page;
        self.anchor = Layout::anchor_to_page(page);
    }

    /// Go to `page` with the point `y` points down the page (unrotated
    /// page points) at the top of the view.
    pub fn go_to_point(&mut self, page: usize, y_points: f32, count: usize, page_height_points: f32) {
        let page = page.min(count.saturating_sub(1));
        self.forward = page >= self.anchor.page;
        let offset = if page_height_points > 0.0 { (y_points / page_height_points).clamp(-0.01, 1.0) } else { 0.0 };
        self.anchor = Anchor { page, offset };
    }

    /// Scroll by `delta` points (positive down) in `layout`.
    pub fn scroll_by(&mut self, layout: &Layout, delta: f32) {
        let scroll = (layout.scroll_of(self.anchor) + delta).clamp(0.0, (layout.size[1] - self.viewport[1]).max(0.0));
        // Single-page mode: within the page shown.
        let scroll = layout.clamp_scroll(scroll, self.anchor.page, self.viewport[1]);
        self.set_scroll(layout, scroll);
    }

    /// Take the scroll offset the scroll area ended the frame with.
    pub fn set_scroll(&mut self, layout: &Layout, scroll: f32) {
        let anchor = layout.anchor_at(scroll, self.viewport[1]);
        if anchor.page != self.anchor.page {
            self.forward = anchor.page > self.anchor.page;
        }
        self.anchor = anchor;
    }

    /// Single-page mode, the pages scrolled to `scroll` by the hand (or
    /// its inertia, `dragging` false): kept within the page shown, what
    /// went past its edge gathered while the drag goes on, a page turned
    /// once it is past `PULL`, the next from its top, the previous from
    /// its bottom, either way alike. Without this, the least pull up went
    /// to the previous page (the offset was on it) and none down did.
    pub fn drag_single(&mut self, layout: &Layout, scroll: f32, dragging: bool, count: usize) {
        let (top, bottom) = layout.page_scroll_range(self.anchor.page, self.viewport[1]);
        let over = if scroll > bottom { scroll - bottom } else if scroll < top { scroll - top } else { 0.0 };
        self.set_scroll(layout, scroll.clamp(top, bottom.max(top)));
        if !dragging || over == 0.0 || (self.pull != 0.0 && (over > 0.0) != (self.pull > 0.0)) {
            self.pull = if dragging { over } else { 0.0 };
        } else {
            self.pull += over;
        }
        if self.pull.abs() >= PULL {
            let down = self.pull > 0.0;
            self.pull = 0.0;
            self.flip(down, count);
        }
    }

    /// A screen of pages down or up (`Page Down`, `Space`), with a little
    /// overlap; in single-page mode the next page once the bottom is
    /// reached.
    pub fn page_step(&mut self, layout: &Layout, down: bool, count: usize) {
        let step = (self.viewport[1] - 24.0).max(self.viewport[1] * 0.5);
        match self.mode {
            ViewMode::Continuous => self.scroll_by(layout, if down { step } else { -step }),
            ViewMode::Single => {
                if self.at_edge(layout, down) {
                    self.flip(down, count);
                } else {
                    self.scroll_by(layout, if down { step } else { -step });
                }
            }
        }
    }

    /// Single-page mode: whether the page is scrolled to its bottom (`down`)
    /// or its top, so that the next step is the next page.
    fn at_edge(&self, layout: &Layout, down: bool) -> bool {
        let scroll = layout.scroll_of(self.anchor);
        let (top, bottom) = layout.page_scroll_range(self.anchor.page, self.viewport[1]);
        if down { scroll + 1.0 >= bottom } else { scroll <= top + 1.0 }
    }

    /// Single-page mode: the next page from its top, or the previous one
    /// from its bottom.
    fn flip(&mut self, down: bool, count: usize) {
        if down {
            if self.anchor.page + 1 < count {
                self.go_to_page(self.anchor.page + 1, count);
            }
        } else if self.anchor.page > 0 {
            self.go_to_page(self.anchor.page - 1, count);
            self.anchor.offset = 1.0;
        }
    }

    /// The wheel in single-page mode: `notches` turned (positive up, as
    /// egui counts them). A page taller than the view scrolls by
    /// `WHEEL_STEP` a notch; at its edge, and on a page that fits, every
    /// whole notch is a page.
    pub fn wheel_single(&mut self, layout: &Layout, notches: f32, count: usize) {
        if notches == 0.0 {
            return;
        }
        let down = notches < 0.0;
        if self.wheel_acc != 0.0 && (self.wheel_acc < 0.0) != down {
            self.wheel_acc = 0.0;
        }
        if self.at_edge(layout, down) {
            self.wheel_acc += notches;
            while self.wheel_acc.abs() >= 0.999 {
                self.wheel_acc -= if down { -1.0 } else { 1.0 };
                self.flip(down, count);
            }
        } else {
            self.wheel_acc = 0.0;
            self.scroll_by(layout, -notches * WHEEL_STEP);
        }
    }

    /// Ctrl+wheel or a pinch: `notches` turned (positive in), one zoom
    /// step per whole notch round `cursor`.
    pub fn zoom_wheel(&mut self, notches: f32, cursor: Option<[f32; 2]>, sizes: &[Option<[f32; 2]>], fallback: [f32; 2]) {
        if notches == 0.0 {
            return;
        }
        if self.zoom_acc != 0.0 && (self.zoom_acc < 0.0) != (notches < 0.0) {
            self.zoom_acc = 0.0;
        }
        self.zoom_acc += notches;
        while self.zoom_acc.abs() >= 0.999 {
            let up = self.zoom_acc > 0.0;
            self.zoom_acc -= if up { 1.0 } else { -1.0 };
            self.zoom_step(up, cursor, sizes, fallback);
        }
    }

    /// A pinch gesture's factor as wheel notches.
    pub fn pinch_notches(factor: f32) -> f32 {
        if factor > 0.0 { factor.ln() / PINCH_NOTCH } else { 0.0 }
    }

    /// Next or previous page (`Right`/`Left` in single-page mode, the
    /// sidebar, `Ctrl+PgDn`).
    pub fn turn_page(&mut self, forward: bool, count: usize) {
        let page = if forward { self.current + 1 } else { self.current.saturating_sub(1) };
        if page < count {
            self.go_to_page(page, count);
        }
    }

    /// Change the zoom, keeping the document point under `cursor` (view
    /// points from the page area's top left; `None` for the centre) where
    /// it is. `sizes` and `fallback` make the layouts before and after.
    pub fn set_zoom(&mut self, zoom: ZoomSetting, cursor: Option<[f32; 2]>, sizes: &[Option<[f32; 2]>], fallback: [f32; 2]) {
        if zoom == self.zoom {
            return;
        }
        let before = self.layout(sizes, fallback);
        let cursor = cursor.unwrap_or([self.viewport[0] / 2.0, self.viewport[1] / 2.0]);
        let scroll = before.scroll_of(self.anchor);
        let doc = [self.scroll_x + cursor[0], scroll + cursor[1]];
        // The page under the cursor and the point's share of it.
        let page = before.anchor_at(doc[1], self.viewport[1]).page;
        let p = before.pages.get(page).copied().unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let frac = [(doc[0] - p[0]) / p[2].max(1.0), (doc[1] - p[1]) / p[3].max(1.0)];
        self.zoom = zoom;
        let after = self.layout(sizes, fallback);
        let q = after.pages.get(page).copied().unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let new_scroll = q[1] + frac[1] * q[3] - cursor[1];
        self.anchor = after.anchor_at(new_scroll.max(0.0), self.viewport[1]);
        self.scroll_x = (q[0] + frac[0] * q[2] - cursor[0]).clamp(0.0, (after.size[0] - self.viewport[0]).max(0.0));
        self.unsettle();
    }

    /// One step up or down the zoom ladder from the current scale.
    pub fn zoom_step(&mut self, up: bool, cursor: Option<[f32; 2]>, sizes: &[Option<[f32; 2]>], fallback: [f32; 2]) {
        let current = layout::zoom_of_scale(self.layout(sizes, fallback).scale);
        let next = layout::zoom_step(current, up);
        self.set_zoom(ZoomSetting::Scale(next), cursor, sizes, fallback);
    }

    /// Turn the pages a quarter turn clockwise or back.
    pub fn rotate(&mut self, clockwise: bool) {
        self.turns = if clockwise { (self.turns + 1) % 4 } else { (self.turns + 3) % 4 };
        self.unsettle();
    }

    pub fn set_mode(&mut self, mode: ViewMode) {
        if mode != self.mode {
            self.mode = mode;
            // The page the reader was on, from its top.
            self.anchor = Layout::anchor_to_page(self.current);
            self.unsettle();
        }
    }

    /// The view was resized: fitting zooms change scale.
    pub fn resized(&mut self, viewport: [f32; 2]) {
        if (viewport[0] - self.viewport[0]).abs() > 0.5 || (viewport[1] - self.viewport[1]).abs() > 0.5 {
            let first = self.viewport == [800.0, 600.0] && self.settle_until.is_none();
            self.viewport = viewport;
            // The first frame: no gesture, the first render is wanted now.
            if !first && !matches!(self.zoom, ZoomSetting::Scale(_)) {
                self.unsettle();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: [f32; 2] = [595.0, 842.0];

    fn sizes(n: usize) -> Vec<Option<[f32; 2]>> {
        vec![Some(A4); n]
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor() {
        let mut view = ViewState::new(ViewMode::Continuous, ZoomSetting::Scale(1.0));
        view.viewport = [1000.0, 800.0];
        view.anchor = Anchor { page: 2, offset: 0.4 };
        let s = sizes(10);
        let before = view.layout(&s, A4);
        let cursor = [300.0, 500.0];
        let doc_y = before.scroll_of(view.anchor) + cursor[1];
        let page = before.anchor_at(doc_y, 800.0).page;
        let frac = (doc_y - before.pages[page][1]) / before.pages[page][3];
        view.set_zoom(ZoomSetting::Scale(2.0), Some(cursor), &s, A4);
        let after = view.layout(&s, A4);
        let doc_y2 = after.scroll_of(view.anchor) + cursor[1];
        let frac2 = (doc_y2 - after.pages[page][1]) / after.pages[page][3];
        assert!((frac - frac2).abs() < 1e-3, "{frac} against {frac2}");
        assert!(!view.settled());
    }

    #[test]
    fn page_steps_in_both_modes() {
        let mut view = ViewState::new(ViewMode::Continuous, ZoomSetting::Scale(1.0));
        view.viewport = [1000.0, 800.0];
        let s = sizes(3);
        let layout = view.layout(&s, A4);
        view.page_step(&layout, true, 3);
        assert!((layout.scroll_of(view.anchor) - 776.0).abs() < 0.01);
        view.page_step(&layout, false, 3);
        assert_eq!(layout.scroll_of(view.anchor), 0.0);
        view.set_mode(ViewMode::Single);
        view.go_to_page(0, 3);
        let layout = view.layout(&s, A4);
        // Down the page, then on to the next one.
        view.page_step(&layout, true, 3);
        assert_eq!(view.anchor.page, 0);
        view.page_step(&layout, true, 3);
        assert_eq!(view.anchor.page, 1);
        assert_eq!(view.anchor.offset, 0.0);
        // Back: the bottom of the previous page.
        view.page_step(&layout, false, 3);
        assert_eq!(view.anchor.page, 0);
        assert_eq!(view.anchor.offset, 1.0);
        view.turn_page(true, 3);
        assert_eq!(view.anchor.page, 1);
    }

    #[test]
    fn the_hand_turns_pages_both_ways_once_pulled_far_enough() {
        let mut view = ViewState::new(ViewMode::Single, ZoomSetting::FitPage);
        view.viewport = [1000.0, 800.0];
        let s = sizes(3);
        view.go_to_page(1, 3);
        let layout = view.layout(&s, A4);
        let (top, bottom) = layout.page_scroll_range(1, 800.0);
        // A little past the top is not a page.
        view.drag_single(&layout, top - 5.0, true, 3);
        assert_eq!(view.anchor.page, 1);
        // Pulled down past the bottom, a frame at a time: the next page.
        for _ in 0..2 {
            view.drag_single(&layout, bottom + 30.0, true, 3);
            assert_eq!(view.anchor.page, 1);
        }
        view.drag_single(&layout, bottom + 30.0, true, 3);
        assert_eq!(view.anchor.page, 2);
        assert_eq!(view.anchor.offset, 0.0);
        // Up past the top: the previous page, from its bottom.
        let (top, _) = layout.page_scroll_range(2, 800.0);
        for _ in 0..3 {
            view.drag_single(&layout, top - 30.0, true, 3);
        }
        assert_eq!(view.anchor.page, 1);
        assert_eq!(view.anchor.offset, 1.0);
        // Inertia after the button is let go turns nothing.
        let (_, bottom) = layout.page_scroll_range(1, 800.0);
        for _ in 0..10 {
            view.drag_single(&layout, bottom + 30.0, false, 3);
        }
        assert_eq!(view.anchor.page, 1);
    }

    #[test]
    fn a_wheel_notch_is_one_zoom_step_however_it_arrives() {
        let s = sizes(3);
        let mut view = ViewState::new(ViewMode::Continuous, ZoomSetting::Scale(1.0));
        view.viewport = [1000.0, 800.0];
        // A notch in four pieces (a high-resolution wheel, or egui's
        // smoothing): one step, not four.
        for _ in 0..4 {
            view.zoom_wheel(0.25, None, &s, A4);
        }
        assert_eq!(view.zoom, ZoomSetting::Scale(1.1));
        // A whole notch at once: one step; two notches: two.
        view.zoom_wheel(1.0, None, &s, A4);
        assert_eq!(view.zoom, ZoomSetting::Scale(1.25));
        view.zoom_wheel(-2.0, None, &s, A4);
        assert_eq!(view.zoom, ZoomSetting::Scale(1.0));
        // Half a notch one way is forgotten when the wheel turns back.
        view.zoom_wheel(0.5, None, &s, A4);
        view.zoom_wheel(-0.5, None, &s, A4);
        view.zoom_wheel(-0.5, None, &s, A4);
        assert_eq!(view.zoom, ZoomSetting::Scale(0.9));
    }

    #[test]
    fn the_wheel_turns_pages_in_single_page_mode() {
        let s = sizes(3);
        // A page that fits: every notch is a page.
        let mut view = ViewState::new(ViewMode::Single, ZoomSetting::FitPage);
        view.viewport = [1000.0, 800.0];
        let layout = view.layout(&s, A4);
        view.wheel_single(&layout, -1.0, 3);
        assert_eq!(view.anchor.page, 1);
        view.wheel_single(&layout, -0.5, 3);
        assert_eq!(view.anchor.page, 1);
        view.wheel_single(&layout, -0.5, 3);
        assert_eq!(view.anchor.page, 2);
        view.wheel_single(&layout, -1.0, 3);
        assert_eq!(view.anchor.page, 2, "the last page stays");
        view.wheel_single(&layout, 2.0, 3);
        assert_eq!(view.anchor.page, 0);
        // A page taller than the view scrolls first, then turns.
        let mut view = ViewState::new(ViewMode::Single, ZoomSetting::Scale(1.0));
        view.viewport = [1000.0, 400.0];
        let layout = view.layout(&s, A4);
        view.wheel_single(&layout, -1.0, 3);
        assert_eq!(view.anchor.page, 0);
        assert!(layout.scroll_of(view.anchor) > 90.0);
        let mut notches = 1;
        while view.anchor.page == 0 && notches < 40 {
            view.wheel_single(&layout, -1.0, 3);
            notches += 1;
        }
        assert_eq!(view.anchor.page, 1);
        assert_eq!(view.anchor.offset, 0.0);
        // The page's own scroll range, a notch at a time, then one to turn.
        let (top, bottom) = layout.page_scroll_range(0, 400.0);
        let expected = ((bottom - top) / WHEEL_STEP).ceil() as usize + 1;
        assert_eq!(notches, expected);
    }

    #[test]
    fn positions_round_trip() {
        let mut view = ViewState::new(ViewMode::Continuous, ZoomSetting::FitWidth);
        view.restore(&FilePosition { path: "a.pdf".into(), page: 4, offset: 0.25, zoom: ZoomSetting::FitPage, rotation: 1, page_shift: 0 });
        assert_eq!(view.anchor, Anchor { page: 4, offset: 0.25 });
        assert_eq!(view.turns, 1);
        let p = view.position(std::path::Path::new("a.pdf"));
        assert_eq!(p.zoom, ZoomSetting::FitPage);
        assert_eq!(p.rotation, 1);
        view.rotate(false);
        assert_eq!(view.turns, 0);
        view.rotate(false);
        assert_eq!(view.turns, 3);
    }
}
