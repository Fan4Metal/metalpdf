//! The status bar under the pages: the page field and count, the zoom,
//! the turns, the view mode, and on the right what is going on (loading,
//! an error, the search). What the reader does here comes back as
//! `StatusAction`s; the app applies them.

use egui::{Align, Key, Layout, RichText, Ui};

use super::icons::Icon;
use crate::settings::{ViewMode, ZoomSetting};

#[derive(Clone, Debug, PartialEq)]
pub enum StatusAction {
    GoToPage(usize),
    ZoomIn,
    ZoomOut,
    Zoom(ZoomSetting),
    Rotate { clockwise: bool },
    Mode(ViewMode),
    ToggleEdit,
}

/// What the bar shows of the document.
pub struct StatusInfo {
    /// The page the reader is on and the count; `None` with no document.
    pub page: Option<(usize, usize)>,
    /// The label of the page shown in the field.
    pub label: String,
    /// The zoom factor shown (1.0 = 100 %), and the setting.
    pub zoom_factor: f32,
    pub zoom: ZoomSetting,
    pub mode: ViewMode,
    /// The text editing mode is on.
    pub editing: bool,
    /// Text on the right: what is going on.
    pub note: String,
    pub note_is_error: bool,
}

/// The page field's text between frames, and whether it is to take the
/// focus (`Ctrl+G`).
#[derive(Default)]
pub struct PageField {
    pub text: String,
    pub focus: bool,
    /// The label the text was set from: retyped when the page changes.
    shown: String,
}

const ZOOM_CHOICES: [f32; 9] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];

/// Draw the bar; the actions the reader took.
pub fn show(ui: &mut Ui, info: &StatusInfo, field: &mut PageField, resolve: impl Fn(&str) -> Option<usize>) -> Vec<StatusAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let enabled = info.page.is_some();
        // The page field: the label of the current page, editable; Enter
        // goes there.
        if field.shown != info.label && !field.focus {
            field.text = info.label.clone();
            field.shown = info.label.clone();
        }
        let width = (field.text.chars().count().max(2) as f32 * 8.0 + 12.0).clamp(36.0, 90.0);
        let edit = egui::TextEdit::singleline(&mut field.text).desired_width(width).horizontal_align(Align::Center);
        let response = ui.add_enabled(enabled, edit);
        if std::mem::take(&mut field.focus) {
            response.request_focus();
        }
        let typing = response.has_focus();
        if response.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Enter))
                && let Some(page) = resolve(&field.text) {
                    actions.push(StatusAction::GoToPage(page));
                }
            field.text = info.label.clone();
            field.shown = info.label.clone();
        }
        if typing && ui.input(|i| i.key_pressed(Key::Escape)) {
            field.text = info.label.clone();
            response.surrender_focus();
        }
        if let Some((_, count)) = info.page {
            ui.label(tr!(format!("of {count}"), format!("из {count}")));
        } else {
            ui.label("—");
        }
        ui.separator();
        // The zoom.
        if super::icon_button(ui, Icon::ZoomOut, false, tr!("Zoom out (Ctrl+−)", "Уменьшить (Ctrl+−)")).clicked() {
            actions.push(StatusAction::ZoomOut);
        }
        let percent = format!("{:.0} %", info.zoom_factor * 100.0);
        let zoom_label = match info.zoom {
            ZoomSetting::FitWidth => tr!(format!("{percent} ▾"), format!("{percent} ▾")),
            _ => format!("{percent} ▾"),
        };
        let zoom_button = ui.add_enabled(enabled, egui::Button::new(RichText::new(zoom_label).monospace()).frame(false));
        let zoom_button = zoom_button.on_hover_text(match info.zoom {
            ZoomSetting::FitWidth => tr!("Zoom: fit width", "Масштаб: по ширине"),
            ZoomSetting::FitPage => tr!("Zoom: fit page", "Масштаб: страница целиком"),
            ZoomSetting::Scale(_) => tr!("Zoom", "Масштаб"),
        });
        egui::Popup::menu(&zoom_button).show(|ui| {
            ui.set_min_width(170.0);
            let fit = |ui: &mut Ui, setting: ZoomSetting, text: String, on: bool| {
                ui.selectable_label(on, text).clicked().then_some(StatusAction::Zoom(setting))
            };
            if let Some(a) = fit(ui, ZoomSetting::FitWidth, tr!("Fit width\tCtrl+0", "По ширине\tCtrl+0").to_owned(), info.zoom == ZoomSetting::FitWidth) {
                actions.push(a);
                ui.close();
            }
            if let Some(a) = fit(ui, ZoomSetting::FitPage, tr!("Fit page\tCtrl+2", "Страница целиком\tCtrl+2").to_owned(), info.zoom == ZoomSetting::FitPage) {
                actions.push(a);
                ui.close();
            }
            ui.separator();
            for z in ZOOM_CHOICES {
                let on = matches!(info.zoom, ZoomSetting::Scale(s) if (s - z).abs() < 0.001);
                let text = if z == 1.0 { "100 %\tCtrl+1".to_owned() } else { format!("{:.0} %", z * 100.0) };
                if ui.selectable_label(on, text).clicked() {
                    actions.push(StatusAction::Zoom(ZoomSetting::Scale(z)));
                    ui.close();
                }
            }
        });
        if super::icon_button(ui, Icon::ZoomIn, false, tr!("Zoom in (Ctrl++)", "Увеличить (Ctrl++)")).clicked() {
            actions.push(StatusAction::ZoomIn);
        }
        ui.separator();
        if super::icon_button(ui, Icon::RotateLeft, false, tr!("Rotate left (Shift+R)", "Повернуть влево (Shift+R)")).clicked() {
            actions.push(StatusAction::Rotate { clockwise: false });
        }
        if super::icon_button(ui, Icon::RotateRight, false, tr!("Rotate right (R)", "Повернуть вправо (R)")).clicked() {
            actions.push(StatusAction::Rotate { clockwise: true });
        }
        ui.separator();
        let continuous = info.mode == ViewMode::Continuous;
        if super::icon_button(ui, Icon::Continuous, continuous, tr!("Continuous pages", "Непрерывная прокрутка")).clicked() {
            actions.push(StatusAction::Mode(ViewMode::Continuous));
        }
        if super::icon_button(ui, Icon::SinglePage, !continuous, tr!("One page at a time", "По одной странице")).clicked() {
            actions.push(StatusAction::Mode(ViewMode::Single));
        }
        ui.separator();
        let edit_tip = tr!("Edit text (E): click a line of text to change it", "Правка текста (E): щёлкните строку текста, чтобы изменить её");
        if ui.add_enabled_ui(enabled, |ui| super::icon_button(ui, Icon::Edit, info.editing, edit_tip)).inner.clicked() {
            actions.push(StatusAction::ToggleEdit);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !info.note.is_empty() {
                let color = if info.note_is_error { crate::style::Palette::of(ui.ctx()).error } else { ui.visuals().weak_text_color() };
                ui.add(egui::Label::new(RichText::new(&info.note).size(12.0).color(color)).truncate());
            }
        });
    });
    actions
}
