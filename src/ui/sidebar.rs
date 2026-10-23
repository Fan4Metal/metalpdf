//! The sidebar: thumbnails of the pages, or the document's outline (its
//! bookmarks), each a list that follows the page the reader is on. What
//! the reader clicks comes back as a `SidebarAction`.

use egui::{Align, CursorIcon, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::app::Tab;
use crate::cache::Textures;
use crate::engine::{Kind, PREVIEW_WIDTH, RenderKey};
use crate::layout;
use crate::pdfium::{OutlineItem, Target};
use crate::settings::{SidebarSide, SidebarTab};
use crate::style::Palette;

pub enum SidebarAction {
    GoToPage(usize),
    Follow(Target),
    Tab(SidebarTab),
    /// A hit in the search list: page and index among the page's hits.
    GoToHit(usize, usize),
    /// A blank page before (`at` = the page) or after (`at` = the page + 1).
    InsertBlank { at: usize },
    /// The pages of a PDF chosen in a dialog, put in at `at`.
    InsertPdf { at: usize },
    /// The page replaced by the pages of a PDF chosen in a dialog.
    ReplacePdf { page: usize },
    DeletePage { page: usize },
    /// The side of the window chosen in the tabs' context menu.
    Side(SidebarSide),
}

/// The space round a thumbnail and under it (its label).
const THUMB_MARGIN: f32 = 12.0;
const THUMB_LABEL: f32 = 18.0;

/// The sidebar's smallest width, in points: the row of its tabs, which
/// cannot wrap, plus the panel's margins. Narrower, the row ran past the
/// panel and under the pages. Search counts while it is hidden too: the
/// panel does not widen when a search shows it.
pub fn min_width(ui: &Ui) -> f32 {
    let spacing = ui.spacing();
    let text = |s: &str| {
        egui::WidgetText::from(s)
            .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button)
            .size()
            .x
    };
    let labels = [tr!("Pages", "Страницы"), tr!("Outline", "Оглавление"), tr!("Search", "Поиск")];
    let row: f32 = labels.iter().map(|l| text(l) + 2.0 * spacing.button_padding.x).sum::<f32>()
        + 2.0 * spacing.item_spacing.x
        + 4.0;
    // Margins of the panel's frame, and a little slack.
    row + 2.0 * 8.0 + 4.0
}

/// Draw the sidebar's content; the previews it wants go to `wanted`.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    tab: &Tab,
    which: SidebarTab,
    side: SidebarSide,
    textures: &mut Textures,
    frame: u64,
    palette: &Palette,
    wanted: &mut Vec<(RenderKey, u8)>,
) -> Vec<SidebarAction> {
    let mut actions = Vec::new();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        // Each tab's context menu chooses the side of the window.
        let mut chosen = side;
        // Search only while the find bar is open: without it the tab has
        // nothing to show.
        let searching = tab.find.open;
        let tabs = [(SidebarTab::Thumbnails, tr!("Pages", "Страницы")), (SidebarTab::Outline, tr!("Outline", "Оглавление")), (SidebarTab::Search, tr!("Search", "Поиск"))];
        for (tab, label) in tabs.into_iter().filter(|&(t, _)| t != SidebarTab::Search || searching) {
            let response = ui.selectable_label(which == tab, label);
            if response.clicked() {
                actions.push(SidebarAction::Tab(tab));
            }
            crate::ui::sidebar_side_menu(&response, &mut chosen);
        }
        if chosen != side {
            actions.push(SidebarAction::Side(chosen));
        }
    });
    ui.add_space(4.0);
    ui.separator();
    match which {
        SidebarTab::Thumbnails => thumbnails(ui, tab, textures, frame, palette, wanted, &mut actions),
        SidebarTab::Outline => outline(ui, tab, palette, &mut actions),
        SidebarTab::Search => search(ui, tab, palette, &mut actions),
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
        if changed && current < count {
            // Whether its row is in view or not (the list opens at the top).
            let x = origin.x + (ui.available_width() - width) / 2.0;
            let rect = Rect::from_min_size(pos2(x, origin.y + tops[current]), vec2(width, height_of(current)));
            ui.scroll_to_rect(rect.expand(THUMB_MARGIN), Some(Align::Center));
        }
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
    // The page a bookmark goes to, as the document numbers it (labels or
    // the page numbering's shift).
    let label = |target: &Option<Target>| match target {
        Some(Target::Page { index, .. }) => Some(tab.doc.label(*index)),
        _ => None,
    };
    let mut cx = OutlineCx { active: active.as_deref(), changed, palette, label: &label, actions };
    ScrollArea::vertical().id_salt(("outline", tab.id)).auto_shrink([false, false]).show(ui, |ui| {
        // Rows touch, so the indent guides run on unbroken.
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(4.0);
        let mut path = Vec::new();
        for (i, item) in items.iter().enumerate() {
            path.push(i);
            outline_item(ui, item, &mut path, &mut cx);
            path.pop();
        }
        ui.add_space(8.0);
    });
}

/// What every row of the outline needs.
struct OutlineCx<'a> {
    active: Option<&'a [usize]>,
    changed: bool,
    palette: &'a Palette,
    label: &'a dyn Fn(&Option<Target>) -> Option<String>,
    actions: &'a mut Vec<SidebarAction>,
}

/// The outline's indent a level, and the column the fold triangle is in.
const OUTLINE_INDENT: f32 = 16.0;

/// The most rows the search list lays out.
const SEARCH_ROWS: usize = 5000;
/// The height of a row of the search list (a page's or a hit's), the
/// indent of a hit under its page and the most characters kept before a
/// match, so that it stays near the start of its line.
const SEARCH_ROW: f32 = 22.0;
const SEARCH_INDENT: f32 = 14.0;
const SEARCH_LEAD: usize = 24;

/// The end of `before` (the text before a match on its line) kept to
/// about `max` characters: cut at a word's start when there is one, with
/// an ellipsis; whole when short enough.
fn lead(before: &str, max: usize) -> String {
    let count = before.chars().count();
    if count <= max {
        return before.to_owned();
    }
    let tail: String = before.chars().skip(count - max).collect();
    // A word's start within the tail, unless that leaves too little.
    let tail = match tail.find(' ') {
        Some(i) if tail[i..].chars().count() > max / 2 => tail[i + 1..].to_owned(),
        _ => tail,
    };
    format!("…{tail}")
}

/// The search's hits by page: a header a page with its hit count, then
/// each hit on one line with the match highlighted as on the page, the
/// current one on a bar; a click goes to it.
fn search(ui: &mut Ui, tab: &Tab, palette: &Palette, actions: &mut Vec<SidebarAction>) {
    let find = &tab.find;
    ui.add_space(8.0);
    if !find.open || find.query.is_empty() {
        ui.label(RichText::new(tr!("Ctrl+F finds text; the hits are listed here.", "Ctrl+F ищет текст; находки будут здесь.")).color(palette.muted));
        return;
    }
    ui.label(RichText::new(find.status()).color(palette.muted));
    ui.add_space(4.0);
    let (rows, earlier) = search_rows(&find.hits, find.current, SEARCH_ROWS);
    if earlier {
        ui.label(RichText::new(tr!("Earlier hits are not listed", "Предыдущие находки не показаны")).color(palette.muted).size(12.0));
    }
    let current = find.current;
    let spacing = ui.spacing().item_spacing.y;
    // Follow the current hit when it changes.
    let last_id = ui.id().with("search-last");
    let changed = ui.data(|d| d.get_temp::<Option<(usize, usize)>>(last_id)) != Some(current);
    if changed {
        ui.data_mut(|d| d.insert_temp(last_id, current));
    }
    let mut area = ScrollArea::vertical().id_salt(("search", tab.id)).auto_shrink([false, false]);
    if changed
        && let Some(c) = current
        && let Some(row) = rows.iter().position(|r| matches!(r, Row::Hit(p, i) if (*p, *i) == c))
    {
        let offset = row as f32 * (SEARCH_ROW + spacing) - ui.available_height() / 2.0;
        area = area.vertical_scroll_offset(offset.max(0.0));
    }
    let text_color = ui.visuals().text_color();
    let font = egui::TextStyle::Body.resolve(ui.style());
    area.show_rows(ui, SEARCH_ROW, rows.len(), |ui, range| {
        for row in &rows[range] {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), SEARCH_ROW), Sense::click());
            let painter = ui.painter();
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            match *row {
                // The page: its number, a rule to the edge, its hit count;
                // a click goes there.
                Row::Page(page) => {
                    let small = egui::FontId::proportional(11.5);
                    let color = if response.hovered() { palette.text } else { palette.muted };
                    let label = tab.doc.label(page);
                    let name = painter.layout_no_wrap(tr!(format!("Page {label}"), format!("Стр. {label}")), small.clone(), color);
                    let count = painter.layout_no_wrap(find.hits[&page].len().to_string(), small, palette.muted);
                    // Low in the row: the room above parts the groups.
                    let y = rect.bottom() - 3.0 - name.size().y;
                    let mid = (y + name.size().y / 2.0).round() + 0.5;
                    let (x0, x1) = (rect.left() + 4.0 + name.size().x + 6.0, rect.right() - 6.0 - count.size().x - 6.0);
                    if x1 > x0 {
                        painter.line_segment([pos2(x0, mid), pos2(x1, mid)], Stroke::new(1.0, palette.muted.gamma_multiply(0.3)));
                    }
                    painter.galley(pos2(rect.right() - 6.0 - count.size().x, y), count, palette.muted);
                    painter.galley(pos2(rect.left() + 4.0, y), name, color);
                    if response.clicked() {
                        actions.push(SidebarAction::GoToPage(page));
                    }
                }
                // A hit: one line, indented under its page, the match on
                // the search's own highlight, the line round it cut so the
                // match stays near the start; the current one on a bar, as
                // the outline's current bookmark.
                Row::Hit(page, i) => {
                    let hit = &find.hits[&page][i];
                    let is_current = current == Some((page, i));
                    if is_current {
                        painter.rect_filled(rect.shrink2(vec2(2.0, 1.0)), 3.0, palette.accent.gamma_multiply(0.16));
                    } else if response.hovered() {
                        painter.rect_filled(rect.shrink2(vec2(2.0, 1.0)), 3.0, palette.muted.gamma_multiply(0.12));
                    }
                    let text_x = rect.left() + SEARCH_INDENT;
                    let before = lead(&hit.before, SEARCH_LEAD);
                    let plain = egui::text::TextFormat { font_id: font.clone(), color: text_color, ..Default::default() };
                    let matched = egui::text::TextFormat { font_id: font.clone(), color: text_color, background: palette.search, ..Default::default() };
                    let mut job = egui::text::LayoutJob::default();
                    job.append(&before, 0.0, plain.clone());
                    job.append(&hit.matched, 0.0, matched);
                    job.append(&hit.after, 0.0, plain);
                    job.wrap.max_width = (rect.right() - 6.0 - text_x).max(20.0);
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    job.wrap.overflow_character = Some('…');
                    let galley = painter.layout_job(job);
                    let cut = galley.elided || before.len() != hit.before.len();
                    painter.galley(pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, text_color);
                    let response = if cut { response.on_hover_text(format!("{}{}{}", hit.before, hit.matched, hit.after)) } else { response };
                    if response.clicked() {
                        actions.push(SidebarAction::GoToHit(page, i));
                    }
                }
            }
        }
    });
    if rows.len() >= SEARCH_ROWS {
        ui.label(RichText::new(tr!("…and more", "…и ещё")).color(palette.muted).size(12.0));
    }
}

/// A row of the search list: a page's header, or hit `.1` of page `.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    Page(usize),
    Hit(usize, usize),
}

/// The search list's rows, at most `cap`: from the start, or, when the
/// current hit lies further, from about half the cap before it, so that
/// it is always listed (F3 went on through hits the list did not have);
/// and whether hits before the first row were left out.
fn search_rows(hits: &std::collections::BTreeMap<usize, Vec<crate::engine::SearchHit>>, current: Option<(usize, usize)>, cap: usize) -> (Vec<Row>, bool) {
    // The rows before the current hit's own row.
    let before = current.map_or(0, |(cp, ci)| hits.range(..cp).map(|(_, h)| 1 + h.len()).sum::<usize>() + 1 + ci);
    let skip = if before < cap { 0 } else { before - cap / 2 };
    let mut rows = Vec::new();
    let mut seen = 0;
    'pages: for (&page, page_hits) in hits {
        if seen >= skip {
            rows.push(Row::Page(page));
        }
        seen += 1;
        for i in 0..page_hits.len() {
            if rows.len() >= cap {
                break 'pages;
            }
            if seen >= skip {
                rows.push(Row::Hit(page, i));
            }
            seen += 1;
        }
    }
    (rows, skip > 0)
}

/// One bookmark, and its children when its fold is open: a row of its
/// own, drawn by hand. The levels differ in size, weight and colour, the
/// text of a level in one column whether the row has a fold or not (the
/// triangle sits in its own column to the left), faint guides run down
/// the levels, the page number is on the right, and the current bookmark
/// has a bar behind it.
fn outline_item(ui: &mut Ui, item: &OutlineItem, path: &mut Vec<usize>, cx: &mut OutlineCx<'_>) {
    let is_active = cx.active == Some(path.as_slice());
    let on_active_path = cx.active.is_some_and(|a| a.starts_with(path));
    let depth = path.len() - 1;
    let has_children = !item.children.is_empty();
    let palette = cx.palette;
    // Chapters get air above them.
    if depth == 0 && path[0] > 0 {
        ui.add_space(6.0);
    }
    let id = ui.id().with(("outline-item", path.clone()));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    if on_active_path && has_children && cx.changed && !is_active {
        state.set_open(true);
    }
    let height = if depth == 0 { 24.0 } else { 21.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter();
    let left = rect.left() + 4.0;
    // The bar behind the current bookmark, else the hover.
    if is_active {
        painter.rect_filled(rect.shrink2(vec2(2.0, 1.0)), 3.0, palette.accent.gamma_multiply(0.16));
    } else if response.hovered() {
        painter.rect_filled(rect.shrink2(vec2(2.0, 1.0)), 3.0, palette.muted.gamma_multiply(0.12));
    }
    // The guides of the levels above.
    let guide = Stroke::new(1.0, palette.muted.gamma_multiply(0.3));
    for level in 0..depth {
        let x = (left + level as f32 * OUTLINE_INDENT + OUTLINE_INDENT / 2.0).round() + 0.5;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], guide);
    }
    // The fold's triangle, in its column.
    let fold = Rect::from_min_size(pos2(left + depth as f32 * OUTLINE_INDENT, rect.top()), vec2(OUTLINE_INDENT, height));
    if has_children {
        let c = fold.center();
        let s = 3.6;
        let points = if state.is_open() {
            vec![pos2(c.x - s, c.y - s * 0.55), pos2(c.x + s, c.y - s * 0.55), pos2(c.x, c.y + s * 0.75)]
        } else {
            vec![pos2(c.x - s * 0.55, c.y - s), pos2(c.x + s * 0.75, c.y), pos2(c.x - s * 0.55, c.y + s)]
        };
        let over = response.hover_pos().is_some_and(|p| fold.contains(p));
        let color = if over { palette.text } else { palette.muted };
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }
    // The page number on the right.
    let number = (cx.label)(&item.target);
    let number_font = egui::FontId::proportional(11.5);
    let number_galley = number.map(|n| painter.layout_no_wrap(n, number_font, palette.muted));
    let number_width = number_galley.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    // The title: chapters larger and darker, sections plain, deeper levels
    // smaller and muted; the current one in the accent colour.
    let (size, color) = match depth {
        0 => (14.0, ui.visuals().strong_text_color()),
        1 => (13.0, palette.text),
        _ => (12.5, palette.muted),
    };
    let color = if is_active { palette.accent } else { color };
    let text_x = fold.right() + 2.0;
    let mut job = egui::text::LayoutJob::single_section(item.title.clone(), egui::text::TextFormat { font_id: egui::FontId::proportional(size), color, ..Default::default() });
    job.wrap.max_width = (rect.right() - 6.0 - number_width - text_x).max(20.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    let elided = galley.elided;
    let text_pos = pos2(text_x, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley.clone(), color);
    // Chapters, the current bookmark and the chapters round it heavier: the
    // text drawn again a pixel to the right (the default font has no bold).
    if depth == 0 || is_active || (on_active_path && depth == 0) {
        let px = 1.0 / ui.ctx().pixels_per_point();
        painter.galley(text_pos + vec2(px, 0.0), galley, color);
    }
    if let Some(g) = number_galley {
        let pos = pos2(rect.right() - 6.0 - g.size().x, rect.center().y - g.size().y / 2.0);
        painter.galley(pos, g, palette.muted);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let response = if elided { response.on_hover_text(&item.title) } else { response };
    if is_active && cx.changed {
        response.scroll_to_me(Some(Align::Center));
    }
    if response.clicked() {
        let on_fold = response.interact_pointer_pos().is_some_and(|p| fold.contains(p));
        match (&item.target, has_children && (on_fold || item.target.is_none())) {
            (_, true) => state.toggle(ui),
            (Some(target), false) => cx.actions.push(SidebarAction::Follow(target.clone())),
            (None, false) => {}
        }
    }
    state.store(ui.ctx());
    if has_children && state.is_open() {
        for (i, child) in item.children.iter().enumerate() {
            path.push(i);
            outline_item(ui, child, path, cx);
            path.pop();
        }
    }
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

    #[test]
    fn the_search_list_always_holds_the_current_hit() {
        let hit = crate::engine::SearchHit { start: 0, count: 1, rects: Vec::new(), before: String::new(), matched: String::new(), after: String::new() };
        let hits: std::collections::BTreeMap<usize, Vec<crate::engine::SearchHit>> = (0..10).map(|p| (p, vec![hit.clone(); 100])).collect();
        let (rows, earlier) = search_rows(&hits, Some((0, 3)), 50);
        assert_eq!((rows.len(), rows[0], earlier), (50, Row::Page(0), false));
        let (rows, earlier) = search_rows(&hits, Some((5, 30)), 50);
        assert!(earlier);
        assert_eq!(rows.len(), 50);
        assert!(rows.contains(&Row::Hit(5, 30)), "{rows:?}");
        let (rows, _) = search_rows(&hits, Some((9, 99)), 50);
        assert_eq!(rows.last(), Some(&Row::Hit(9, 99)));
        assert_eq!(search_rows(&hits, None, 50).0.len(), 50);
    }

    #[test]
    fn the_text_before_a_match_is_cut_at_a_word() {
        assert_eq!(lead("Short ", 16), "Short ");
        assert_eq!(lead("10.9 Muscle Tissue and the ", 16), "…Tissue and the ");
        // No word start that leaves enough: cut mid-word.
        assert_eq!(lead("Pneumonoultramicroscopic ", 16), "…ltramicroscopic ");
    }

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
