//! The sheet of the painted icons (`src/ui/icons.rs`): every icon at the
//! toolbar size and larger, with its name, in the light or dark theme. A new
//! icon is looked at here next to the others before it goes into the app.
//!
//! cargo run --example icons

#[path = "../src/ui/icons.rs"]
mod icons;

use egui::{Align2, FontId, Sense, ThemePreference, vec2};
use icons::Icon;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Icons").with_inner_size([820.0, 620.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("icons", options, Box::new(|_| Ok(Box::new(Sheet { size: icons::SIZE }))))
}

struct Sheet {
    /// The size of the second, larger copy of each icon.
    size: f32,
}

impl eframe::App for Sheet {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("controls").show(root_ui, |ui| {
            ui.horizontal(|ui| {
                let mut theme = ui.ctx().options(|o| o.theme_preference);
                ui.selectable_value(&mut theme, ThemePreference::Light, "Light");
                ui.selectable_value(&mut theme, ThemePreference::Dark, "Dark");
                ui.ctx().set_theme(theme);
                ui.separator();
                ui.add(egui::Slider::new(&mut self.size, 14.0..=56.0).text("size"));
            });
        });
        egui::CentralPanel::default().show(root_ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for &icon in Icon::ALL {
                        cell(ui, icon, self.size);
                    }
                });
            });
        });
    }
}

/// An icon in a button at `icons::SIZE` (as in a toolbar), the same at
/// `size`, and its name.
fn cell(ui: &mut egui::Ui, icon: Icon, size: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(size.max(28.0) + 64.0, size.max(28.0) + 26.0), Sense::hover());
    let visuals = ui.style().interact(&response);
    let (color, fill) = (visuals.text_color(), ui.visuals().panel_fill);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
    }
    let button = egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(24.0, 24.0));
    let button_fill = ui.visuals().widgets.inactive.weak_bg_fill;
    painter.rect_filled(button, 3.0, button_fill);
    icons::paint(painter, icon, button.center(), icons::SIZE, color, button_fill);
    let large = egui::pos2(button.right() + 10.0 + size / 2.0, button.top() + size.max(24.0) / 2.0);
    icons::paint(painter, icon, large, size, color, fill);
    let name = match icon {
        Icon::Volume(n) => format!("Volume({n})"),
        _ => format!("{icon:?}"),
    };
    painter.text(rect.left_bottom() + vec2(6.0, -4.0), Align2::LEFT_BOTTOM, name, FontId::proportional(11.0), color);
}
