//! The sidebar: thumbnails of the pages, or the document's outline (its
//! bookmarks), each a list that follows the page the reader is on. What
//! the reader clicks comes back as a `SidebarAction`.

use egui::{Align, CursorIcon, Label, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::app::Tab;
use crate::cache::Textures;
use crate::engine::{Kind, PREVIEW_WIDTH, RenderKey};
use crate::layout;
use crate::pdfium::{OutlineItem, Target};
use crate::settings::SidebarTab;
use crate::style::Palette;

pub enum SidebarAction {
    GoToPage(usize),
    Follow(Target),
    Tab(SidebarTab),
    /// A blank page before (`at` = the page) or after (`at` = the page + 1).
    InsertBlank { at: usize },
    /// The pages of a PDF chosen in a dialog, put in at `at`.
    InsertPdf { at: usize },
    /// The page replaced by the pages of a PDF chosen in a dialog.
    ReplacePdf { page: usize },
    DeletePage { page: usize },
}

/// The space round a thumbnail and under it (its label).
const THUMB_MARGIN: f32 = 12.0;
const THUMB_LABEL: f32 = 18.0;

/// Draw the sidebar's content; the previews it wants go to `wanted`.
pub fn show(
    ui: &mut Ui,
    tab: &Tab,
    which: SidebarTab,
    textures: &mut Textures,
    frame: u64,
    palette: &Palette,
    wanted: &mut Vec<(RenderKey, u8)>,
) -> Vec<SidebarAction> {
    let mut actions = Vec::new();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if ui.selectable_label(which == SidebarTab::Thumbnails, tr!("Pages", "Страницы")).clicked() {
            actions.push(SidebarAction::Tab(SidebarTab::Thumbnails));
        }
        if ui.selectable_label(which == SidebarTab::Outline, tr!("Outline", "Оглавление")).clicked() {
            actions.push(SidebarAction::Tab(SidebarTab::Outline));
        }
    });
    ui.add_space(4.0);
    ui.separator();
    match which {
        SidebarTab::Thumbnails => thumbnails(ui, tab, textures, frame, palette, wanted, &mut actions),
        SidebarTab::Outline => outline(ui, tab, palette, &mut actions),
    }
    actions
}

/// The pages as small pictures, the current one framed; a click goes to
/// the page. Only the visible ones are laid out and rendered.
fn thumbnails(
    ui: &mut Ui,
    tab: &Tab,
    textures: &mut Textures,
    frame: u64,
    palette: &Palette,
    wanted: &mut Vec<(RenderKey, u8)>,
    actions: &mut Vec<SidebarAction>,
) {
    let doc = &tab.doc;
    let count = doc.page_count;
    if count == 0 {
        return;
    }
    let turns = tab.view.turns;
    let width = (ui.available_width() - 2.0 * THUMB_MARGIN - 14.0).clamp(40.0, 400.0);
    // Each thumbnail's height from its page's shape; the list's rows.
    let height_of = |i: usize| {
        let s = layout::turned(doc.size_of(i), turns);
        (width * s[1] / s[0].max(1.0)).clamp(10.0, 3.0 * width)
    };
    let mut tops = Vec::with_capacity(count + 1);
    let mut y = THUMB_MARGIN;
    for i in 0..count {
        tops.push(y);
        y += height_of(i) + THUMB_LABEL + THUMB_MARGIN;
    }
    tops.push(y);
    let total = y;
    let current = tab.view.current;
    // Follow the reader: scroll to the current page when it changes.
    let last_id = ui.id().with("thumbs-last");
    let changed = ui.data(|d| d.get_temp::<usize>(last_id)) != Some(current);
    if changed {
        ui.data_mut(|d| d.insert_temp(last_id, current));
    }
    ScrollArea::vertical().id_salt(("thumbs", tab.id)).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        ui.set_min_height(total);
        let origin = ui.min_rect().min;
        let first = tops.partition_point(|&t| t + 1.0 < viewport.min.y).saturating_sub(1).min(count - 1);
        let last = tops.partition_point(|&t| t < viewport.max.y).min(count);
        for (i, &top) in tops.iter().enumerate().take(last).skip(first) {
            let h = height_of(i);
            let x = origin.x + (ui.available_width() - width) / 2.0;
            let rect = Rect::from_min_size(pos2(x, origin.y + top), vec2(width, h));
            let response = ui.interact(rect.expand(4.0), ui.id().with(("thumb", i)), Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if response.clicked() {
                actions.push(SidebarAction::GoToPage(i));
            }
            // The page operations (edits: they go to the copy that is saved).
            crate::ui::menu::context_menu(&response, |ui| {
                ui.set_min_width(240.0);
                if ui.button(tr!("Insert a blank page before", "Вставить пустую страницу перед")).clicked() {
                    actions.push(SidebarAction::InsertBlank { at: i });
                    ui.close();
                }
                if ui.button(tr!("Insert a blank page after", "Вставить пустую страницу после")).clicked() {
                    actions.push(SidebarAction::InsertBlank { at: i + 1 });
                    ui.close();
                }
                if ui.button(tr!("Insert pages from a PDF before…", "Вставить страницы из PDF перед…")).clicked() {
                    actions.push(SidebarAction::InsertPdf { at: i });
                    ui.close();
                }
                if ui.button(tr!("Insert pages from a PDF after…", "Вставить страницы из PDF после…")).clicked() {
                    actions.push(SidebarAction::InsertPdf { at: i + 1 });
                    ui.close();
                }
                ui.separator();
                if ui.button(tr!("Replace with pages from a PDF…", "Заменить страницами из PDF…")).clicked() {
                    actions.push(SidebarAction::ReplacePdf { page: i });
                    ui.close();
                }
                if ui.add_enabled(count > 1, egui::Button::new(tr!("Delete the page", "Удалить страницу"))).clicked() {
                    actions.push(SidebarAction::DeletePage { page: i });
                    ui.close();
                }
            });
            if i == current && changed {
                ui.scroll_to_rect(rect.expand(THUMB_MARGIN), Some(Align::Center));
            }
            let painter = ui.painter();
            painter.rect_filled(rect.translate(vec2(1.0, 2.0)), 1.0, palette.shadow);
            let key = RenderKey { doc: doc.id, page: i as u32, width: PREVIEW_WIDTH, turns, kind: Kind::Preview };
            if let Some(tex) = textures.preview.get(&key, frame) {
                painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
            } else {
                painter.rect_filled(rect, 0.0, palette.paper);
                wanted.push((key, 5));
            }
            let stroke = if i == current { Stroke::new(2.0, palette.accent) } else { Stroke::new(1.0, palette.page_edge) };
            painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Outside);
            let label = doc.label(i);
            let color = if i == current { palette.accent } else { palette.muted };
            painter.text(
                pos2(rect.center().x, rect.max.y + 3.0),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(11.0),
                color,
            );
        }
    });
}

/// The bookmarks as a tree; the one the reader is in (the last whose page
/// is at or before the current one) in the accent colour.
fn outline(ui: &mut Ui, tab: &Tab, palette: &Palette, actions: &mut Vec<SidebarAction>) {
    let Some(items) = tab.doc.outline.as_ref() else {
        ui.add_space(8.0);
        ui.label(RichText::new(tr!("Loading…", "Загрузка…")).color(palette.muted));
        return;
    };
    if items.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(tr!("No outline", "Оглавления нет")).color(palette.muted));
        return;
    }
    let current = tab.view.current;
    let active = active_item(items, current);
    let last_id = ui.id().with("outline-last");
    let changed = ui.data(|d| d.get_temp::<Option<Vec<usize>>>(last_id)) != Some(active.clone());
    if changed {
        ui.data_mut(|d| d.insert_temp(last_id, active.clone()));
    }
    ScrollArea::vertical().id_salt(("outline", tab.id)).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add_space(4.0);
        let mut path = Vec::new();
        for (i, item) in items.iter().enumerate() {
            path.push(i);
            outline_item(ui, item, &mut path, active.as_deref(), changed, palette, actions);
            path.pop();
        }
    });
}

/// One bookmark with its children under a fold.
fn outline_item(
    ui: &mut Ui,
    item: &OutlineItem,
    path: &mut Vec<usize>,
    active: Option<&[usize]>,
    changed: bool,
    palette: &Palette,
    actions: &mut Vec<SidebarAction>,
) {
    let is_active = active == Some(path.as_slice());
    let on_active_path = active.is_some_and(|a| a.starts_with(path));
    let depth = path.len() - 1;
    let id = ui.id().with(("outline-item", path.clone()));
    let mut text = RichText::new(&item.title).size(if depth == 0 { 13.5 } else { 13.0 });
    text = if is_active {
        text.color(palette.accent).strong()
    } else if depth > 1 {
        text.color(palette.muted)
    } else {
        text
    };
    let row = |ui: &mut Ui, actions: &mut Vec<SidebarAction>| {
        let response = ui.add(Label::new(text.clone()).truncate().sense(Sense::click()));
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if is_active && changed {
            response.scroll_to_me(Some(Align::Center));
        }
        if response.clicked()
            && let Some(target) = &item.target
        {
            actions.push(SidebarAction::Follow(target.clone()));
        }
    };
    if item.children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0 + 18.0);
            row(ui, actions);
        });
        return;
    }
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0);
    if on_active_path && changed {
        state.set_open(true);
    }
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 12.0);
        state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
        row(ui, actions);
    });
    state.show_body_unindented(ui, |ui| {
        for (i, child) in item.children.iter().enumerate() {
            path.push(i);
            outline_item(ui, child, path, active, changed, palette, actions);
            path.pop();
        }
    });
}

/// The path (indices down the tree) of the bookmark the reader is in: the
/// last one, in reading order, whose page is at or before `page`.
fn active_item(items: &[OutlineItem], page: usize) -> Option<Vec<usize>> {
    fn walk(items: &[OutlineItem], page: usize, path: &mut Vec<usize>, best: &mut Option<(usize, Vec<usize>)>) {
        for (i, item) in items.iter().enumerate() {
            path.push(i);
            if let Some(Target::Page { index, .. }) = &item.target
                && *index <= page
                && best.as_ref().is_none_or(|(p, _)| *index >= *p)
            {
                *best = Some((*index, path.clone()));
            }
            walk(&item.children, page, path, best);
            path.pop();
        }
    }
    let mut best = None;
    walk(items, page, &mut Vec::new(), &mut best);
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, page: usize, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem { title: title.into(), target: Some(Target::Page { index: page, x: None, y: None }), children }
    }

    #[test]
    fn the_active_bookmark_is_the_last_at_or_before_the_page() {
        let items = vec![item("1", 0, vec![item("1.1", 2, vec![]), item("1.2", 5, vec![])]), item("2", 9, vec![])];
        assert_eq!(active_item(&items, 0), Some(vec![0]));
        assert_eq!(active_item(&items, 3), Some(vec![0, 0]));
        assert_eq!(active_item(&items, 5), Some(vec![0, 1]));
        assert_eq!(active_item(&items, 20), Some(vec![1]));
        assert_eq!(active_item(&[], 3), None);
    }
}
