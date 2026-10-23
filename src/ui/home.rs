//! The start page, shown when no document is open: the icon, the name, the
//! version and what to do, then the recent files (as MetalMD's). When the
//! engine could not be loaded, the reason takes the hint's place.

use std::path::{Path, PathBuf};

use egui::{CursorIcon, Label, RichText, ScrollArea, Sense, TextureHandle, Ui};

use crate::style::Palette;

const ICON: f32 = 112.0;

pub enum HomeAction {
    Open(PathBuf),
    ClearRecent,
}

/// `error`: the engine is missing; shown instead of the hint.
pub fn show(
    ui: &mut Ui,
    recent: &[PathBuf],
    error: Option<&str>,
    palette: &Palette,
    icon_cache: &mut Option<TextureHandle>,
) -> Option<HomeAction> {
    let mut action = None;
    let icon = super::app_icon(ui.ctx(), ICON, icon_cache);
    ScrollArea::vertical().id_salt("home-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            // The whole page (with the recent files) a little above the
            // middle of the window. Its height comes from the previous
            // pass; an unknown or changed height makes egui lay the frame
            // out again before painting, so the page does not jump.
            let height_id = ui.scope_id().with("home-height");
            let known: Option<f32> = ui.data(|d| d.get_temp(height_id));
            let top = known.map_or(16.0, |h| ((ui.available_height() - h) * 0.4).max(16.0));
            ui.add_space(top);
            let start = ui.cursor().min.y;
            ui.image((icon.id(), egui::vec2(ICON, ICON)));
            ui.add_space(14.0);
            ui.label(RichText::new(crate::APP_NAME).size(30.0).strong());
            ui.weak(tr!(format!("Version {}", crate::VERSION), format!("Версия {}", crate::VERSION)));
            ui.add_space(18.0);
            match error {
                Some(text) => {
                    ui.set_max_width(560.0);
                    ui.label(RichText::new(text).color(palette.error));
                }
                None => {
                    ui.label(tr!(
                        "Open a PDF with Ctrl+O or drop it here",
                        "Откройте PDF через Ctrl+O или перетащите файл сюда"
                    ));
                }
            }
            if !recent.is_empty() && error.is_none() {
                ui.add_space(32.0);
                ui.set_max_width(520.0);
                ui.label(RichText::new(tr!("RECENT", "НЕДАВНИЕ")).size(11.0).color(palette.muted));
                ui.add_space(6.0);
                for path in recent {
                    if let Some(clicked) = recent_entry(ui, path, palette) {
                        action = Some(HomeAction::Open(clicked));
                    }
                }
                ui.add_space(10.0);
                if clear_recent(ui, palette) {
                    action = Some(HomeAction::ClearRecent);
                }
                ui.add_space(16.0);
            }
            let height = ui.cursor().min.y - start;
            if known.is_none_or(|h| (h - height).abs() > 0.5) {
                ui.data_mut(|d| d.insert_temp(height_id, height));
                ui.ctx().request_discard("start page height");
            }
        });
    });
    action
}

/// "Clear recent" below the list: muted, the link colour under the pointer.
/// True when clicked.
fn clear_recent(ui: &mut Ui, palette: &Palette) -> bool {
    // The label's hover in the previous pass picks the colour before it is
    // laid out, as egui's Button does it.
    let id = ui.next_auto_id();
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    let color = if hovered { palette.link } else { palette.muted };
    let response = ui
        .add(Label::new(RichText::new(tr!("Clear recent", "Очистить список")).size(12.0).color(color)).sense(Sense::click()))
        .on_hover_text(tr!("Forget the recent files", "Забыть недавние файлы"));
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.clicked()
}

/// One recent file: its name as a link, its folder below. The path when
/// clicked.
fn recent_entry(ui: &mut Ui, path: &Path, palette: &Palette) -> Option<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let dir = path.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let response = ui
        .add(Label::new(RichText::new(name).size(14.0).color(palette.link)).sense(Sense::click()))
        .on_hover_text(path.display().to_string());
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    ui.add(Label::new(RichText::new(dir).size(11.0).color(palette.muted)).truncate());
    ui.add_space(4.0);
    response.clicked().then(|| path.to_path_buf())
}
