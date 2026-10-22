//! The page area: the pages of a document in a scroll area, each drawn
//! from the best texture at hand (the exact render, else the nearest one
//! scaled, else the preview, else white paper), the renders the frame
//! wants listed for the engine in order of urgency, links under the
//! pointer, the search's hits and the text selection painted over the
//! pages and the selection dragged out with the mouse. The geometry is
//! `layout`'s; the scroll position lives in the tab's `ViewState` as an
//! anchor and is handed to egui's scroll area each frame.

use egui::{Color32, CursorIcon, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::Tab;
use crate::cache::Textures;
use crate::engine::{Kind, PREVIEW_WIDTH, RenderKey};
use crate::layout;
use crate::pdfium::Target;
use crate::selection::{self, Selection};
use crate::style::Palette;

/// Pages rendered ahead in the direction of travel, and behind.
const AHEAD: usize = 2;
const BEHIND: usize = 1;

/// What the page area tells the app.
#[derive(Default)]
pub struct ViewOutput {
    /// The renders wanted, most urgent first: `(key, tier)`.
    pub wanted: Vec<(RenderKey, u8)>,
    /// The pages in view.
    pub visible: std::ops::Range<usize>,
    /// A link clicked.
    pub link: Option<Target>,
    /// Whether the pointer hovers the pages (for the wheel's zoom).
    pub hovered: bool,
    /// The pointer's position in the page area, if there.
    pub pointer: Option<[f32; 2]>,
}

/// Draw the pages of `tab` into `ui`.
pub fn show(ui: &mut Ui, tab: &mut Tab, textures: &mut Textures, frame: u64, palette: &Palette) -> ViewOutput {
    let mut out = ViewOutput::default();
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 0.0, palette.backdrop);
    tab.view.resized([area.width(), area.height()]);
    let ppp = ui.ctx().pixels_per_point();
    let layout = tab.view.layout(&tab.doc.sizes, tab.doc.fallback);
    let scroll_y = layout.scroll_of(tab.view.anchor);
    let scroll_x = tab.view.scroll_x.clamp(0.0, (layout.size[0] - area.width()).max(0.0));
    let settled = tab.view.settled();
    let doc_id = tab.doc.id;
    let turns = tab.view.turns;
    let mut candidates: Vec<(RenderKey, u8, u32)> = Vec::new();
    let mut visible = 0..0;
    let mut hovered_link: Option<(Rect, Target)> = None;
    let mut over_text = false;
    // The character under the pointer, for a selection being made.
    let mut char_under_pointer: Option<selection::CharPos> = None;
    let mut selection = tab.selection;
    let mut selecting = tab.selecting;
    let find = &tab.find;

    let scroll = egui::ScrollArea::both()
        .id_salt(("pages", tab.id))
        .auto_shrink([false, false])
        // The mouse drags the selection, not the pages.
        .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::SCROLL_BAR)
        .vertical_scroll_offset(scroll_y)
        .horizontal_scroll_offset(scroll_x)
        .show_viewport(ui, |ui, viewport| {
            ui.set_min_size(vec2(layout.size[0], layout.size[1]));
            let origin = ui.min_rect().min;
            visible = layout.visible(viewport.min.y, viewport.height());
            let middle = viewport.min.y + viewport.height() / 2.0;
            let response = ui.interact(ui.min_rect(), ui.id().with("pages-sense"), Sense::click_and_drag());
            out.hovered = response.hovered();
            let pointer = response.hover_pos().or_else(|| if selecting { response.interact_pointer_pos() } else { None });
            out.pointer = pointer.map(|p| [p.x - origin.x - scroll_x, p.y - origin.y - scroll_y]);
            let painter = ui.painter();
            for i in visible.clone() {
                let p = layout.pages[i];
                if p[3] <= 0.0 {
                    continue;
                }
                let rect = snap(Rect::from_min_size(origin + vec2(p[0], p[1]), vec2(p[2], p[3])), ppp);
                let size = tab.doc.size_of(i);
                let turned = layout::turned(size, turns);
                let key = RenderKey { doc: doc_id, page: i as u32, width: layout::pixel_width(turned, layout.scale, ppp), turns, kind: Kind::Full };
                let preview_key = RenderKey { width: PREVIEW_WIDTH, kind: Kind::Preview, ..key };
                let distance = ((p[1] + p[3] / 2.0 - middle).abs() / 100.0) as u32;
                // Shadow, picture, edge.
                painter.rect_filled(rect.translate(vec2(2.0, 3.0)), 2.0, palette.shadow);
                if let Some(tex) = textures.full.get(&key, frame) {
                    painter.image(tex.id(), rect, uv(), Color32::WHITE);
                } else if let Some((_, tex)) = textures.full.nearest(&key, frame) {
                    painter.image(tex.id(), rect, uv(), Color32::WHITE);
                    if settled {
                        candidates.push((key, 1, distance));
                    }
                } else if let Some(tex) = textures.preview.get(&preview_key, frame) {
                    painter.image(tex.id(), rect, uv(), Color32::WHITE);
                    if settled {
                        candidates.push((key, 1, distance));
                    }
                } else {
                    painter.rect_filled(rect, 0.0, palette.paper);
                    candidates.push((preview_key, 0, distance));
                    if settled {
                        candidates.push((key, 1, distance));
                    }
                }
                painter.rect_stroke(rect, 0.0, Stroke::new(1.0, palette.page_edge), StrokeKind::Outside);
                let text = tab.doc.text.get(&i);
                // The search's hits.
                let (hits, current_hit) = find.hits_on(i);
                if let Some(text) = text {
                    for (h, hit) in hits.iter().enumerate() {
                        let color = if current_hit == Some(h) { palette.search_current } else { palette.search };
                        for r in &hit.rects {
                            let screen = page_rect_to_screen(text.rect_to_page(*r), turns, size, rect);
                            painter.rect_filled(screen.expand(1.0), 2.0, color);
                        }
                    }
                }
                // The selection on this page.
                if let (Some(sel), Some(text)) = (selection, text)
                    && let Some(range) = sel.range_on(i, text.layout.chars.len())
                {
                    for r in selection::line_rects(text.boxes(), range) {
                        let screen = page_rect_to_screen(r, turns, size, rect);
                        painter.rect_filled(screen, 1.0, palette.selection);
                    }
                }
                // The pointer over the page: text under it, links.
                if let Some(pt) = pointer
                    && rect.contains(pt)
                {
                    if let Some(text) = text {
                        let on_page = screen_to_page_point(pt, turns, size, rect);
                        if selection::over_text(text.boxes(), on_page) {
                            over_text = true;
                        }
                        if let Some(c) = selection::char_at(text.boxes(), on_page) {
                            char_under_pointer = Some((i, c));
                        }
                    }
                    if let (Some(links), Some(text)) = (tab.doc.links.get(&i), text) {
                        for link in links.iter() {
                            let screen = page_rect_to_screen(text.rect_to_page(link.rect), turns, size, rect);
                            if screen.contains(pt) {
                                hovered_link = Some((screen, link.target.clone()));
                            }
                        }
                    }
                }
            }
            if let Some((screen, target)) = &hovered_link {
                painter.rect_stroke(*screen, 2.0, Stroke::new(1.0, palette.link), StrokeKind::Outside);
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if response.clicked() {
                    out.link = Some(target.clone());
                }
            } else if over_text || selecting {
                ui.ctx().set_cursor_icon(CursorIcon::Text);
            }
            // The selection: begun with a press on text, grown while
            // dragging, cleared by a plain click.
            if response.drag_started_by(PointerButton::Primary) && hovered_link.is_none() {
                selection = char_under_pointer.map(|c| Selection { anchor: c, focus: c });
                selecting = selection.is_some();
            }
            if selecting && response.dragged_by(PointerButton::Primary) {
                if let (Some(sel), Some(c)) = (selection.as_mut(), char_under_pointer) {
                    sel.focus = c;
                }
                ui.ctx().request_repaint();
            }
            if response.drag_stopped() {
                selecting = false;
                if selection.is_some_and(|s| s.is_empty()) {
                    selection = None;
                }
            }
            if response.clicked() && hovered_link.is_none() {
                selection = None;
            }
        });
    tab.selection = selection;
    tab.selecting = selecting;
    // Where the scroll area ended up (the wheel, the bar, a drag).
    let offset = scroll.state.offset;
    tab.view.set_scroll(&layout, offset.y);
    tab.view.scroll_x = offset.x;
    tab.view.current = layout.current(offset.y, area.height());
    out.visible = visible.clone();

    // Prefetch: the pages ahead in the direction of travel and behind.
    if settled && !visible.is_empty() && tab.doc.ready() {
        let count = tab.doc.page_count;
        let (ahead, behind) = if tab.view.forward { (AHEAD, BEHIND) } else { (BEHIND, AHEAD) };
        let mut near: Vec<usize> = Vec::new();
        for k in 1..=ahead {
            if visible.end - 1 + k < count {
                near.push(visible.end - 1 + k);
            }
        }
        for k in 1..=behind {
            if visible.start >= k {
                near.push(visible.start - k);
            }
        }
        for (rank, i) in near.into_iter().enumerate() {
            let size = tab.doc.size_of(i);
            let turned = layout::turned(size, turns);
            let key = RenderKey { doc: doc_id, page: i as u32, width: layout::pixel_width(turned, layout.scale, ppp), turns, kind: Kind::Full };
            let preview_key = RenderKey { width: PREVIEW_WIDTH, kind: Kind::Preview, ..key };
            if !textures.preview.contains(&preview_key) && !textures.full.contains(&key) {
                candidates.push((preview_key, 3, rank as u32));
            }
            if !textures.full.contains(&key) {
                candidates.push((key, 4, rank as u32));
            }
        }
    }
    candidates.sort_by_key(|(_, tier, distance)| (*tier, *distance));
    candidates.dedup_by_key(|(key, _, _)| *key);
    out.wanted = candidates.into_iter().map(|(k, t, _)| (k, t)).collect();
    out
}

/// The whole texture.
fn uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// `rect` on whole physical pixels, so that a render at the exact size is
/// drawn without resampling.
fn snap(rect: Rect, ppp: f32) -> Rect {
    let r = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(r(rect.min.x), r(rect.min.y)), pos2(r(rect.max.x), r(rect.max.y)))
}

/// A rectangle `[x, y, w, h]` in points on the unrotated page of `size`
/// (points), as drawn in `page` (the page's screen rectangle after
/// `turns` quarter turns).
pub fn page_rect_to_screen(r: [f32; 4], turns: u8, size: [f32; 2], page: Rect) -> Rect {
    let a = page_point_to_screen([r[0], r[1]], turns, size, page);
    let b = page_point_to_screen([r[0] + r[2], r[1] + r[3]], turns, size, page);
    Rect::from_two_pos(a, b)
}

/// A point in points on the unrotated page, on screen.
pub fn page_point_to_screen(pt: [f32; 2], turns: u8, size: [f32; 2], page: Rect) -> Pos2 {
    let [w, h] = size;
    let (x, y) = (pt[0], pt[1]);
    // The turned page's coordinates (its own width first).
    let (tx, ty, tw, th) = match turns % 4 {
        0 => (x, y, w, h),
        1 => (h - y, x, h, w),
        2 => (w - x, h - y, w, h),
        _ => (y, w - x, h, w),
    };
    page.min + Vec2::new(tx / tw.max(1e-3) * page.width(), ty / th.max(1e-3) * page.height())
}

/// A screen point back onto the unrotated page, in points.
pub fn screen_to_page_point(pos: Pos2, turns: u8, size: [f32; 2], page: Rect) -> [f32; 2] {
    let [w, h] = size;
    let (tw, th) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let tx = (pos.x - page.min.x) / page.width().max(1e-3) * tw;
    let ty = (pos.y - page.min.y) / page.height().max(1e-3) * th;
    match turns % 4 {
        0 => [tx, ty],
        1 => [ty, h - tx],
        2 => [w - tx, h - ty],
        _ => [w - ty, tx],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_points_turn_with_the_page() {
        let size = [100.0, 200.0];
        let page = Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 100.0)); // turned: 200 wide
        // 90° clockwise: the top left corner goes to the top right.
        let p = page_point_to_screen([0.0, 0.0], 1, size, page);
        assert_eq!(p, pos2(210.0, 10.0));
        let q = page_point_to_screen([0.0, 200.0], 1, size, page);
        assert_eq!(q, pos2(10.0, 10.0));
        for turns in 0..4 {
            let rect = if turns % 2 == 1 { page } else { Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 200.0)) };
            let back = screen_to_page_point(page_point_to_screen([30.0, 50.0], turns, size, rect), turns, size, rect);
            assert!((back[0] - 30.0).abs() < 1e-3 && (back[1] - 50.0).abs() < 1e-3, "turns {turns}: {back:?}");
        }
        let r = page_rect_to_screen([0.0, 0.0, 10.0, 20.0], 1, size, page);
        assert_eq!(r, Rect::from_min_max(pos2(190.0, 10.0), pos2(210.0, 20.0)));
    }

    #[test]
    fn snapping_lands_on_pixels() {
        let r = snap(Rect::from_min_max(pos2(0.3, 0.7), pos2(10.26, 20.74)), 2.0);
        assert_eq!(r, Rect::from_min_max(pos2(0.5, 0.5), pos2(10.5, 20.5)));
    }
}
