//! The toolbar under the tab bar: the page field and count, the zoom
//! with its menu and the three fitting presets (fit width, fit page, 100 %,
//! the one in force lit), the turns, the view mode, the editing mode's
//! pencil, and on the right a note on what is going on (an edit refused, a
//! save, unsaved changes). What the reader does here comes back as
//! `ToolbarAction`s; the app applies them.

use egui::{Align, Key, Layout, RichText, Ui};

use super::icons::Icon;
use crate::settings::{Tool, ViewMode, ZoomSetting};

#[derive(Clone, Debug, PartialEq)]
pub enum ToolbarAction {
    GoToPage(usize),
    ZoomIn,
    ZoomOut,
    /// A zoom chosen (a preset or the menu): remembered for new documents.
    Zoom(ZoomSetting),
    Rotate { clockwise: bool },
    Mode(ViewMode),
    Tool(Tool),
    ToggleEdit,
    /// The page numbering: read off the pages, set by the number of the
    /// page shown, or the file's own.
    DetectNumbering,
    ThisPageIs { page: usize, number: i64 },
    FileNumbering,
    /// The print dialog.
    Print,
    /// The print job under way abandoned.
    CancelPrint,
}

/// What the bar shows of the document.
pub struct ToolbarInfo {
    /// The page the reader is on and the count; `None` with no document.
    pub page: Option<(usize, usize)>,
    /// The label of the page shown in the field.
    pub label: String,
    /// The zoom factor shown (1.0 = 100 %), and the setting.
    pub zoom_factor: f32,
    pub zoom: ZoomSetting,
    pub mode: ViewMode,
    pub tool: Tool,
    /// The page numbering's shift (`DocState::page_shift`).
    pub page_shift: i32,
    /// The text editing mode is on.
    pub editing: bool,
    /// Text on the right: what is going on.
    pub note: String,
    pub note_is_error: bool,
    /// A print job under way: sheets sent, sheets in all.
    pub printing: Option<(usize, usize)>,
}

/// The page field's text between frames, and whether it is to take the
/// focus (`Ctrl+G`).
#[derive(Default)]
pub struct PageField {
    pub text: String,
    pub focus: bool,
    /// The number typed in the numbering menu ("This page is number…").
    pub number_entry: String,
    /// The label the text was set from: retyped when the page changes.
    shown: String,
}

const ZOOM_CHOICES: [f32; 9] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];

/// Whether `zoom` is the 100 % preset.
pub fn is_actual_size(zoom: ZoomSetting) -> bool {
    matches!(zoom, ZoomSetting::Scale(s) if (s - 1.0).abs() < 0.001)
}

/// Draw the bar; the actions the reader took.
pub fn show(ui: &mut Ui, info: &ToolbarInfo, field: &mut PageField, resolve: impl Fn(&str) -> Option<usize>) -> Vec<ToolbarAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
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
                && let Some(page) = resolve(&field.text)
            {
                actions.push(ToolbarAction::GoToPage(page));
            }
            field.text = info.label.clone();
            field.shown = info.label.clone();
        }
        if typing && ui.input(|i| i.key_pressed(Key::Escape)) {
            field.text = info.label.clone();
            response.surrender_focus();
        }
        if let Some((current, _)) = info.page {
            let response = if info.page_shift != 0 {
                response.on_hover_text(tr!(format!("Page {} of the file", current + 1), format!("Страница {} файла", current + 1)))
            } else {
                response
            };
            // The numbering: the right button on the field.
            super::menu::context_menu_with_fields(&response, |ui| {
                ui.set_min_width(280.0);
                // The title and the row with the field are labels: indented
                // as a button's text, so that every line starts at one edge.
                let pad = ui.spacing().button_padding.x;
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.label(RichText::new(tr!("Page numbering", "Нумерация страниц")).weak());
                });
                ui.separator();
                if ui.button(tr!("Read the numbers printed on the pages", "Найти номера, напечатанные на страницах")).clicked() {
                    actions.push(ToolbarAction::DetectNumbering);
                    ui.close();
                }
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.label(tr!("This page is number", "Эта страница — номер"));
                    let entry = ui.add(egui::TextEdit::singleline(&mut field.number_entry).hint_text(info.label.as_str()).desired_width(44.0).horizontal_align(egui::Align::Center));
                    let number = field.number_entry.trim().parse::<i64>().ok();
                    let ok = ui.add_enabled(number.is_some(), egui::Button::new(tr!("OK", "ОК")));
                    if let Some(number) = number
                        && (ok.clicked() || entry.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                    {
                        actions.push(ToolbarAction::ThisPageIs { page: current, number });
                        field.number_entry.clear();
                        ui.close();
                    }
                });
                ui.separator();
                if ui.add_enabled(info.page_shift != 0, egui::Button::new(tr!("The file's own numbering", "Нумерация как в файле"))).clicked() {
                    actions.push(ToolbarAction::FileNumbering);
                    ui.close();
                }
            });
        }
        if let Some((_, count)) = info.page {
            ui.label(tr!(format!("of {count}"), format!("из {count}")));
        } else {
            ui.label("—");
        }
        ui.separator();
        // The zoom: a step out, the factor with its menu, a step in.
        if super::icon_button(ui, Icon::ZoomOut, false, tr!("Zoom out (Ctrl+−)", "Уменьшить (Ctrl+−)")).clicked() {
            actions.push(ToolbarAction::ZoomOut);
        }
        let zoom_label = format!("{:.0} % ▾", info.zoom_factor * 100.0);
        let zoom_button = ui.add_enabled(enabled, egui::Button::new(RichText::new(zoom_label).monospace()).frame(false));
        let zoom_button = zoom_button.on_hover_text(match info.zoom {
            ZoomSetting::FitWidth => tr!("Zoom: fit width", "Масштаб: по ширине"),
            ZoomSetting::FitPage => tr!("Zoom: fit page", "Масштаб: страница целиком"),
            ZoomSetting::Scale(_) => tr!("Zoom", "Масштаб"),
        });
        egui::Popup::menu(&zoom_button).show(|ui| {
            ui.set_min_width(170.0);
            let fit = |ui: &mut Ui, setting: ZoomSetting, text: String, on: bool| {
                ui.selectable_label(on, text).clicked().then_some(ToolbarAction::Zoom(setting))
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
                    actions.push(ToolbarAction::Zoom(ZoomSetting::Scale(z)));
                    ui.close();
                }
            }
        });
        if super::icon_button(ui, Icon::ZoomIn, false, tr!("Zoom in (Ctrl++)", "Увеличить (Ctrl++)")).clicked() {
            actions.push(ToolbarAction::ZoomIn);
        }
        ui.separator();
        // The presets: the one in force is lit; a choice is remembered.
        let presets = [
            (Icon::FitWidth, ZoomSetting::FitWidth, info.zoom == ZoomSetting::FitWidth, tr!("Fit width (Ctrl+0)", "По ширине (Ctrl+0)")),
            (Icon::FitPage, ZoomSetting::FitPage, info.zoom == ZoomSetting::FitPage, tr!("Fit page (Ctrl+2)", "Страница целиком (Ctrl+2)")),
            (Icon::ActualSize, ZoomSetting::Scale(1.0), is_actual_size(info.zoom), tr!("Actual size, 100 % (Ctrl+1)", "Истинный размер, 100 % (Ctrl+1)")),
        ];
        for (icon, setting, on, tip) in presets {
            if super::icon_button(ui, icon, on, tip).clicked() {
                actions.push(ToolbarAction::Zoom(setting));
            }
        }
        ui.separator();
        if super::icon_button(ui, Icon::RotateLeft, false, tr!("Rotate left (Shift+R)", "Повернуть влево (Shift+R)")).clicked() {
            actions.push(ToolbarAction::Rotate { clockwise: false });
        }
        if super::icon_button(ui, Icon::RotateRight, false, tr!("Rotate right (R)", "Повернуть вправо (R)")).clicked() {
            actions.push(ToolbarAction::Rotate { clockwise: true });
        }
        ui.separator();
        let continuous = info.mode == ViewMode::Continuous;
        if super::icon_button(ui, Icon::Continuous, continuous, tr!("Continuous pages", "Непрерывная прокрутка")).clicked() {
            actions.push(ToolbarAction::Mode(ViewMode::Continuous));
        }
        if super::icon_button(ui, Icon::SinglePage, !continuous, tr!("One page at a time", "По одной странице")).clicked() {
            actions.push(ToolbarAction::Mode(ViewMode::Single));
        }
        ui.separator();
        // The tool: what the left button does over the pages.
        if super::icon_button(ui, Icon::SelectText, info.tool == Tool::Select, tr!("Select text (V): drag over the text", "Выделение текста (V): протяните по тексту")).clicked() {
            actions.push(ToolbarAction::Tool(Tool::Select));
        }
        if super::icon_button(ui, Icon::Move, info.tool == Tool::Hand, tr!("Hand (H): drag the pages to scroll", "Рука (H): тяните страницы, чтобы прокручивать")).clicked() {
            actions.push(ToolbarAction::Tool(Tool::Hand));
        }
        ui.separator();
        let edit_tip = tr!("Edit text and pictures (E): click a run of text or a picture to change it", "Правка текста и картинок (E): щёлкните строку текста или картинку, чтобы изменить её");
        if ui.add_enabled_ui(enabled, |ui| super::icon_button(ui, Icon::Edit, info.editing, edit_tip)).inner.clicked() {
            actions.push(ToolbarAction::ToggleEdit);
        }
        if ui.add_enabled_ui(enabled, |ui| super::icon_button(ui, Icon::Print, false, tr!("Print (Ctrl+P)", "Печать (Ctrl+P)"))).inner.clicked() {
            actions.push(ToolbarAction::Print);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some((done, total)) = info.printing {
                if super::icon_button(ui, Icon::Close, false, tr!("Cancel printing", "Отменить печать")).clicked() {
                    actions.push(ToolbarAction::CancelPrint);
                }
                ui.label(RichText::new(tr!(format!("Printing: {done} of {total}"), format!("Печать: {done} из {total}"))).size(12.0));
                ui.add_space(8.0);
            }
            if !info.note.is_empty() {
                let color = if info.note_is_error { crate::style::Palette::of(ui.ctx()).error } else { ui.visuals().weak_text_color() };
                ui.add(egui::Label::new(RichText::new(&info.note).size(12.0).color(color)).truncate()).on_hover_text(&info.note);
            }
        });
    });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_actual_size_preset_is_100_percent() {
        assert!(is_actual_size(ZoomSetting::Scale(1.0)));
        assert!(!is_actual_size(ZoomSetting::Scale(1.25)));
        assert!(!is_actual_size(ZoomSetting::FitWidth));
    }
}
