//! The start screen, shown while nothing is open: the app icon, the name,
//! the version and one line on how to begin, as in qview, MetalMD and Metal
//! Player. It says which program this is and which version without opening
//! About, and what to do first without opening the shortcuts.

use egui::{Align, Layout, RichText, TextureHandle, Ui, UiBuilder, vec2};

/// The icon's side in points: 112 in qview and MetalMD (the embedded
/// 128-pixel layer at 100 %, 256 above).
const ICON: f32 = 112.0;

/// Fill the rest of `ui` with the start screen. `hint` is one line, the one
/// or two ways to begin with their keys ("Open an image with Ctrl+O or drop
/// a file here; G opens the gallery").
pub fn show(ui: &mut Ui, icon_cache: &mut Option<TextureHandle>, hint: &str) {
    let icon = super::app_icon(ui.ctx(), ICON, icon_cache);
    let rect = ui.available_rect_before_wrap();
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), |ui| {
        // The icon just above the middle, the text just below it, so the
        // screen stays put whatever the hint's length.
        ui.add_space((rect.height() * 0.5 - ICON).max(0.0));
        ui.image((icon.id(), vec2(ICON, ICON)));
        ui.add_space(14.0);
        ui.label(RichText::new(crate::APP_NAME).size(30.0).strong());
        ui.weak(tr!(format!("Version {}", crate::VERSION), format!("Версия {}", crate::VERSION)));
        ui.add_space(18.0);
        ui.label(hint);
    });
}
