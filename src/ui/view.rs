//! The page area: the pages of a document in a scroll area, each drawn
//! from the best texture at hand (the exact render, else the nearest one
//! scaled, else the preview, else white paper), the renders the frame
//! wants listed for the engine in order of urgency, links under the
//! pointer, the search's hits and the text selection painted over the
//! pages and the selection dragged out with the mouse. The geometry is
//! `layout`'s; the scroll position lives in the tab's `ViewState` as an
//! anchor and is handed to egui's scroll area each frame.

use egui::scroll_area::ScrollSource;
use egui::{Color32, CursorIcon, Event, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::{ImageDrag, ImageSel, InlineEditor, Tab};
use crate::cache::Textures;
use crate::engine::{Kind, PREVIEW_WIDTH, RenderKey};
use crate::layout;
use crate::pdfium::Target;
use crate::selection::{self, Selection};
use crate::settings::ViewMode;
use crate::style::Palette;
use crate::view::{ViewState, WHEEL_STEP};

/// The wheel's notches this frame, from the raw events (positive up or
/// in): `scroll` without a modifier, `zoom` with Ctrl or from a pinch.
#[derive(Default)]
struct Wheel {
    scroll: f32,
    zoom: f32,
}

fn wheel_notches(ui: &Ui) -> Wheel {
    let mut wheel = Wheel::default();
    ui.input(|i| {
        for event in &i.events {
            match event {
                Event::MouseWheel { unit, delta, modifiers, .. } => {
                    // Shift scrolls sideways: the scroll area's business.
                    if modifiers.shift {
                        continue;
                    }
                    let notches = match unit {
                        MouseWheelUnit::Line => delta.y,
                        MouseWheelUnit::Point => delta.y / WHEEL_STEP,
                        MouseWheelUnit::Page => delta.y * 3.0,
                    };
                    if modifiers.command || modifiers.ctrl {
                        wheel.zoom += notches;
                    } else {
                        wheel.scroll += notches;
                    }
                }
                Event::Zoom(factor) => wheel.zoom += ViewState::pinch_notches(*factor),
                _ => {}
            }
        }
    });
    wheel
}

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
    /// Ctrl+wheel notches (positive in) and pinches over the pages this
    /// frame, from the raw events: one notch is one zoom step.
    pub zoom_notches: f32,
    /// The pointer's position in the page area, if there.
    pub pointer: Option<[f32; 2]>,
    /// In the editing mode, a text object clicked: page and object.
    pub edit_click: Option<(usize, usize)>,
    /// The inline editor was confirmed with this text.
    pub edit_commit: Option<(usize, usize, String)>,
    pub edit_cancel: bool,
    /// In the editing mode, an image clicked: page, object and its box on
    /// the unrotated page.
    pub image_click: Option<(usize, usize, [f32; 4])>,
    /// The selected image was dragged to a new box (unrotated page points).
    pub image_place: Option<(usize, usize, [f32; 4])>,
    /// The selected image's Replace… or Delete button.
    pub image_replace: bool,
    pub image_delete: bool,
}

/// Draw the pages of `tab` into `ui`.
pub fn show(ui: &mut Ui, tab: &mut Tab, textures: &mut Textures, frame: u64, palette: &Palette) -> ViewOutput {
    let mut out = ViewOutput::default();
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 0.0, palette.backdrop);
    tab.view.resized([area.width(), area.height()]);
    let ppp = ui.ctx().pixels_per_point();
    // The wheel from its raw events: egui smooths a notch over several
    // frames, which would make a zoom step a frame.
    let single = tab.view.mode == ViewMode::Single;
    let wheel = if ui.rect_contains_pointer(area) { wheel_notches(ui) } else { Wheel::default() };
    out.zoom_notches = wheel.zoom;
    if single && wheel.scroll != 0.0 {
        let layout = tab.view.layout(&tab.doc.sizes, tab.doc.fallback);
        tab.view.wheel_single(&layout, wheel.scroll, tab.doc.page_count);
    }
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
    let editing = tab.editing;
    let editor: &mut Option<InlineEditor> = &mut tab.editor;
    let image: &mut Option<ImageSel> = &mut tab.image;

    let scroll = egui::ScrollArea::both()
        .id_salt(("pages", tab.id))
        .auto_shrink([false, false])
        // The mouse drags the selection, not the pages; in single-page
        // mode the wheel turns pages (above), so it is not the area's.
        .scroll_source(if single { ScrollSource::SCROLL_BAR } else { ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR })
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
            let painter = ui.painter().clone();
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
                // The editing mode: the text objects outlined, the one under
                // the pointer stronger, a click opens the inline editor.
                if editing
                    && let (Some(objects), Some(text)) = (tab.doc.objects.get(&i), text)
                {
                    for object in objects.text.iter() {
                        let screen = page_rect_to_screen(text.rect_to_page(object.bounds), turns, size, rect).expand(1.0);
                        let under = pointer.is_some_and(|pt| screen.contains(pt));
                        let being_edited = editor.as_ref().is_some_and(|e| e.page == i && e.object == object.index);
                        if being_edited {
                            continue;
                        }
                        let color = if under { palette.accent } else { palette.muted.gamma_multiply(0.5) };
                        painter.rect_stroke(screen, 1.0, Stroke::new(1.0, color), StrokeKind::Outside);
                        if under {
                            ui.ctx().set_cursor_icon(CursorIcon::Text);
                            if response.clicked() {
                                out.edit_click = Some((i, object.index));
                            }
                        }
                    }
                }
                // The images, outlined; the selected one with handles to move
                // and resize it, and its buttons.
                if editing
                    && let (Some(objects), Some(text)) = (tab.doc.objects.get(&i), text)
                {
                    for img in objects.images.iter() {
                        let selected = image.as_ref().is_some_and(|s| s.page == i && s.object == img.index);
                        if selected {
                            continue;
                        }
                        let screen = page_rect_to_screen(text.rect_to_page(img.bounds), turns, size, rect);
                        let under = pointer.is_some_and(|pt| screen.contains(pt));
                        let color = if under { palette.link } else { palette.link.gamma_multiply(0.45) };
                        painter.rect_stroke(screen, 1.0, Stroke::new(1.0, color), StrokeKind::Outside);
                        if under && hovered_link.is_none() {
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                            if response.clicked() {
                                out.image_click = Some((i, img.index, text.rect_to_page(img.bounds)));
                            }
                        }
                    }
                    if let Some(sel) = image.as_mut()
                        && sel.page == i
                    {
                        let screen = page_rect_to_screen(sel.rect, turns, size, rect);
                        painter.rect_stroke(screen, 0.0, Stroke::new(2.0, palette.accent), StrokeKind::Outside);
                        let handle = 8.0;
                        let corners = [screen.left_top(), screen.right_top(), screen.right_bottom(), screen.left_bottom()];
                        for c in corners {
                            painter.rect_filled(Rect::from_center_size(c, vec2(handle, handle)), 1.0, palette.accent);
                        }
                        // A press on a corner resizes, inside moves; on release
                        // the new box goes to the engine.
                        let press = response.interact_pointer_pos();
                        if response.drag_started_by(PointerButton::Primary)
                            && let Some(pt) = press
                        {
                            let corner = corners.iter().position(|c| (c.to_vec2() - pt.to_vec2()).length() <= handle);
                            if corner.is_some() || screen.contains(pt) {
                                sel.drag = Some(ImageDrag { corner: corner.map(|c| c as u8), start: pt, start_rect: sel.rect });
                            }
                        }
                        if let Some(drag) = sel.drag.as_ref()
                            && let Some(pt) = press
                        {
                            // The pointer's travel in unrotated page points: the
                            // page's own axes, whatever the turns.
                            let start = screen_to_page_point(drag.start, turns, size, rect);
                            let now = screen_to_page_point(pt, turns, size, rect);
                            let (dx, dy) = (now[0] - start[0], now[1] - start[1]);
                            let r0 = drag.start_rect;
                            sel.rect = match drag.corner {
                                None => [r0[0] + dx, r0[1] + dy, r0[2], r0[3]],
                                Some(c) => {
                                    // The corner follows the pointer, the opposite one
                                    // stays; the proportions are kept.
                                    let (x0, y0, x1, y1) = (r0[0], r0[1], r0[0] + r0[2], r0[1] + r0[3]);
                                    let c = corner_on_page(c, turns);
                                    let (fixed_x, fixed_y) = match c {
                                        0 => (x1, y1),
                                        1 => (x0, y1),
                                        2 => (x0, y0),
                                        _ => (x1, y0),
                                    };
                                    let (moving_x, moving_y) = match c {
                                        0 => (x0 + dx, y0 + dy),
                                        1 => (x1 + dx, y0 + dy),
                                        2 => (x1 + dx, y1 + dy),
                                        _ => (x0 + dx, y1 + dy),
                                    };
                                    let aspect = r0[2] / r0[3].max(1e-3);
                                    let w = (moving_x - fixed_x).abs().max(4.0);
                                    let h = (w / aspect).max(4.0);
                                    let left = if moving_x < fixed_x { fixed_x - w } else { fixed_x };
                                    let top = if moving_y < fixed_y { fixed_y - h } else { fixed_y };
                                    [left, top, w, h]
                                }
                            };
                            ui.ctx().set_cursor_icon(if drag.corner.is_some() { CursorIcon::ResizeNwSe } else { CursorIcon::Grabbing });
                            ui.ctx().request_repaint();
                        }
                        if response.drag_stopped()
                            && let Some(drag) = sel.drag.take()
                            && sel.rect != drag.start_rect
                        {
                            out.image_place = Some((sel.page, sel.object, sel.rect));
                        }
                        if sel.drag.is_none()
                            && let Some(pt) = pointer
                        {
                            if corners.iter().any(|c| (c.to_vec2() - pt.to_vec2()).length() <= handle) {
                                ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
                            } else if screen.contains(pt) {
                                ui.ctx().set_cursor_icon(CursorIcon::Grab);
                            }
                        }
                        // The buttons under the picture (above it at the page's foot).
                        let below = screen.max.y + 30.0 < rect.max.y;
                        let y = if below { screen.max.y + 6.0 } else { screen.min.y - 30.0 };
                        let bar = Rect::from_min_size(pos2(screen.min.x, y), vec2(200.0, 24.0));
                        let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
                        if bar_ui.button(tr!("Replace…", "Заменить…")).clicked() {
                            out.image_replace = true;
                        }
                        if bar_ui.button(tr!("Delete", "Удалить")).on_hover_text("Del").clicked() {
                            out.image_delete = true;
                        }
                    }
                }
                // The inline editor over the object being edited.
                if let Some(e) = editor.as_mut()
                    && e.page == i
                {
                    let screen = page_rect_to_screen(e.rect, turns, size, rect);
                    let scale = rect.height() / size[1].max(1.0);
                    let font = egui::FontId::proportional((e.font_size * scale).clamp(8.0, 64.0));
                    let field = Rect::from_min_size(
                        screen.min - vec2(4.0, 3.0),
                        vec2((screen.width() + 40.0).max(120.0), screen.height().max(font.size + 8.0) + 6.0),
                    );
                    let widget = egui::TextEdit::singleline(&mut e.text).font(font).margin(vec2(4.0, 3.0));
                    let edit = ui.put(field, widget);
                    if std::mem::take(&mut e.fresh) {
                        edit.request_focus();
                    }
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if edit.lost_focus() || enter || escape {
                        if enter && !escape {
                            out.edit_commit = Some((e.page, e.object, e.text.clone()));
                        } else {
                            out.edit_cancel = true;
                        }
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
            if response.drag_started_by(PointerButton::Primary) && hovered_link.is_none() && !editing {
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
            if response.clicked() && hovered_link.is_none() && out.edit_click.is_none() && out.image_click.is_none() {
                selection = None;
                if editor.is_some() {
                    out.edit_cancel = true;
                }
                // A click off the selected image lets go of it (not a drag's end).
                if image.as_ref().is_some_and(|s| s.drag.is_none()) {
                    *image = None;
                }
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

/// Which corner of the unrotated page's box a screen corner (0 top left,
/// clockwise) is, after `turns` quarter turns.
fn corner_on_page(screen_corner: u8, turns: u8) -> u8 {
    (screen_corner + 4 - turns % 4) % 4
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
