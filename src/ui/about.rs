//! The About dialog: icon, name, version, a short description, the author,
//! the homepage, the license, where the settings are kept, the interface
//! language and the update check. The same in every app of the family.

use egui::{Key, TextureHandle, Ui};

use crate::i18n::LangChoice;
use crate::update::{Status, Updates, release_url, version_of};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = env!("CARGO_PKG_LICENSE");
/// Cargo joins several authors with `:`.
const AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
/// Displayed icon size in points.
const ICON: f32 = 64.0;
/// Width of the language list, enough for its longest entry.
const LANG_WIDTH: f32 = 220.0;

#[derive(Default)]
pub struct AboutDialog {
    pub open: bool,
    /// Opened this frame: the key that opened it must not close it.
    pub fresh: bool,
    /// Interface language (kept in the settings).
    pub lang: LangChoice,
    icon: Option<TextureHandle>,
    /// The Licenses window, shown in place of About.
    licenses: Option<super::licenses::LicensesView>,
}

impl AboutDialog {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.fresh = self.open;
    }

    /// Draws the dialog while it is open; closes it on the cross, Esc,
    /// Enter or a click outside. A language picked in it is applied at
    /// once and left in `lang` for the settings.
    pub fn show(&mut self, ctx: &egui::Context, updates: &mut Updates) {
        if !self.open {
            return;
        }
        let fresh = std::mem::take(&mut self.fresh);
        if let Some(licenses) = self.licenses.as_mut() {
            if licenses.show(ctx) {
                self.licenses = None;
            }
            return;
        }
        let mut open_licenses = false;
        let icon = super::app_icon(ctx, ICON, &mut self.icon);
        let modal = egui::Modal::new(egui::Id::unique("about")).show(ctx, |ui| {
            ui.set_width(340.0);
            // The cross in the top right corner, over the centred content
            // (a child Ui takes no room in the layout).
            let corner = egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().right() - 24.0, ui.cursor().top()),
                egui::vec2(24.0, 24.0),
            );
            let closed = super::close_cross(&mut ui.new_child(egui::UiBuilder::new().max_rect(corner)));
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.image((icon.id(), egui::vec2(ICON, ICON)));
                ui.add_space(6.0);
                ui.heading(crate::APP_NAME);
                ui.label(tr!(format!("Version {}", crate::VERSION), format!("Версия {}", crate::VERSION)));
                ui.add_space(8.0);
                // One sentence, the same as Cargo.toml's description and
                // the README tagline; the Russian one is written by hand.
                ui.label(tr!(
                    "Short description of the program for Windows.",
                    "Краткое описание программы для Windows."
                ));
                ui.add_space(8.0);
                if !AUTHORS.is_empty() {
                    let authors = AUTHORS.replace(':', ", ");
                    ui.label(tr!(format!("Author: {authors}"), format!("Автор: {authors}")));
                }
                if !REPOSITORY.is_empty()
                    && ui.link(tr!("Homepage", "Сайт проекта")).on_hover_text(REPOSITORY).clicked()
                    && !crate::win::shell_open(REPOSITORY)
                {
                    log::warn!("could not open {REPOSITORY}");
                }
                ui.horizontal(|ui| {
                    // The licence line, with the texts behind a link; centred
                    // by hand (a horizontal row ignores `vertical_centered`).
                    let text = tr!(format!("{LICENSE} License"), format!("Лицензия {LICENSE}"));
                    let link = tr!("Licenses…", "Лицензии…");
                    let width = ui.fonts_mut(|f| {
                        f.layout_no_wrap(format!("{text} · {link}"), egui::FontId::proportional(14.0), egui::Color32::WHITE).size().x
                    });
                    ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                    ui.weak(text);
                    ui.weak("·");
                    if ui.link(link).on_hover_text(tr!("PDFium, the libraries and the fonts", "PDFium, библиотеки и шрифты")).clicked() {
                        open_licenses = true;
                    }
                });
                let stored = crate::settings::location();
                let place = ui.weak(if stored.portable {
                    tr!("Settings: next to the program (portable)", "Настройки: рядом с программой (портативно)")
                } else {
                    tr!("Settings: in the user profile", "Настройки: в профиле пользователя")
                });
                match &stored.file {
                    Some(path) => place.on_hover_text(path.display().to_string()),
                    None => place.on_hover_text(tr!("Settings are not saved", "Настройки не сохраняются")),
                };
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                // The label over the list, both centred; a fixed width
                // keeps the list from jumping when the language changes.
                ui.weak(tr!("Interface language", "Язык интерфейса"));
                let mut choice = self.lang;
                // A combo box lays itself out left to right, ignoring the
                // centring: indented by hand (`width` is its outer width).
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - LANG_WIDTH) / 2.0).max(0.0));
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(choice.label())
                        .width(LANG_WIDTH)
                        .show_ui(ui, |ui| {
                            for c in LangChoice::ALL {
                                ui.selectable_value(&mut choice, c, c.label());
                            }
                        });
                });
                if choice != self.lang {
                    self.lang = choice;
                    crate::i18n::set_lang(choice.resolve());
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                update_section(ui, updates);
                ui.add_space(4.0);
            });
            closed
        });
        if open_licenses {
            self.licenses = Some(super::licenses::LicensesView::default());
            return;
        }
        let closed = modal.inner
            || (!fresh
                && (ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
                    || modal.should_close()));
        if closed {
            self.open = false;
        }
    }
}

/// The update check: the start-up option, a button to check now and the
/// outcome.
fn update_section(ui: &mut Ui, updates: &mut Updates) {
    let before = updates.enabled;
    ui.checkbox(
        &mut updates.enabled,
        tr!("Check for updates at start-up (once a day)", "Проверять обновления при запуске (раз в сутки)"),
    )
    .on_hover_text(tr!(
        "Asks api.github.com for the latest release; nothing else is sent, and nothing is downloaded",
        "Запрашивает у api.github.com последний выпуск; больше ничего не отправляется и не скачивается"
    ));
    if updates.enabled && !before {
        updates.start_if_due(ui.ctx());
    }
    let checking = updates.status == Status::Checking;
    if ui.add_enabled(!checking, egui::Button::new(tr!("Check now", "Проверить сейчас"))).clicked() {
        updates.start(ui.ctx());
    }
    match &updates.status {
        Status::Unknown => {}
        Status::Checking => {
            ui.horizontal(|ui| {
                // Centred by hand, as the language list above.
                ui.add_space(((ui.available_width() - 110.0) / 2.0).max(0.0));
                ui.add(egui::Spinner::new());
                ui.weak(tr!("Checking…", "Проверка…"));
            });
        }
        Status::UpToDate => {
            ui.weak(tr!("This is the latest version", "Установлена последняя версия"));
        }
        Status::Newer(tag) => {
            let version = version_of(tag);
            let url = release_url(tag);
            if ui
                .link(tr!(format!("Version {version} is available"), format!("Доступна версия {version}")))
                .on_hover_text(url.as_str())
                .clicked()
                && !crate::win::shell_open(&url)
            {
                log::warn!("could not open {url}");
            }
        }
        Status::Failed(e) => {
            ui.weak(tr!("Could not check for updates", "Не удалось проверить обновления")).on_hover_text(e);
        }
    }
}
