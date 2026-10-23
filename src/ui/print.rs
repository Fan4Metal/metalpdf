//! The print dialog: the printer and its paper, orientation and sides
//! (the driver's own dialog behind Properties…), the pages, the copies,
//! the scale, and a preview of each sheet as it will print, drawn from the
//! engine's renders. What a printer's driver is asked (the list, its
//! settings, the sheet's size) is asked on a thread of its own, as it can
//! take a while for a network printer.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{Align, Color32, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::icons::Icon;
use crate::cache::Textures;
use crate::document::DocState;
use crate::engine::{Engine, Kind, PREVIEW_WIDTH, PrintJob, RenderKey};
use crate::print::{self, DevMode, PrinterState, Sheet};
use crate::settings::{PRINT_PERCENT, PrintScale, PrintSettings};
use crate::style::Palette;

/// Which pages are printed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pages {
    All,
    Current,
    Range,
}

pub enum PrintAction {
    Print(PrintJob),
    Close,
}

/// A printer query's answer: the printers (the first query only) and the
/// printer's state (none when there is no printer).
struct Answer {
    printers: Option<(Vec<String>, Option<String>)>,
    state: Option<PrinterState>,
}

/// The preview's sheet until the printer's is known: A4 at 600 dpi, all
/// of it printable.
const DEFAULT_SHEET: Sheet = Sheet { dpi: [600.0, 600.0], size: [4961.0, 7016.0], printable: [0.0, 0.0, 4961.0, 7016.0] };
/// The room the preview's sheet is fitted into.
const PREVIEW: egui::Vec2 = egui::Vec2::new(300.0, 400.0);

pub struct PrintDialog {
    /// The tab whose document is printed.
    pub tab: u64,
    /// The name the spooler shows.
    title: String,
    /// The page shown when the dialog opened.
    current: usize,
    printers: Vec<String>,
    /// The printers were listed (and none may be there).
    listed: bool,
    /// The printer, its settings, papers and sheet, once asked.
    state: Option<PrinterState>,
    pending: Option<Receiver<Answer>>,
    /// The driver's dialog is open: its answer.
    properties: Option<Receiver<Option<DevMode>>>,
    pages: Pages,
    range: String,
    copies: u32,
    collate: bool,
    scale: PrintScale,
    /// The custom scale, in percent.
    percent: u32,
    auto_rotate: bool,
    as_image: bool,
    /// The sheet the preview shows, among the pages chosen.
    shown: usize,
}

impl PrintDialog {
    /// The dialog for the document of tab `tab`, the page `current` shown;
    /// the printer last used, else Windows' default. A document that asks
    /// not to be scaled (`print_scaling` false) starts at its actual size.
    pub fn open(ctx: &egui::Context, tab: u64, title: String, current: usize, settings: &PrintSettings, print_scaling: bool) -> PrintDialog {
        let (send, receive) = channel();
        let last = settings.printer.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("printers".into()).spawn(move || {
            let (names, default) = print::printers();
            let name = last.filter(|l| names.contains(l)).or_else(|| default.clone()).or_else(|| names.first().cloned());
            let state = name.map(|n| print::query(&n, None));
            let _ = send.send(Answer { printers: Some((names, default)), state });
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            log::warn!("cannot list the printers: {e}");
        }
        PrintDialog {
            tab,
            title,
            current,
            printers: Vec::new(),
            listed: false,
            state: None,
            pending: Some(receive),
            properties: None,
            pages: Pages::All,
            range: String::new(),
            copies: 1,
            collate: true,
            scale: if print_scaling { settings.scale } else { PrintScale::Actual },
            percent: settings.percent.clamp(*PRINT_PERCENT.start(), *PRINT_PERCENT.end()),
            auto_rotate: settings.auto_rotate,
            as_image: settings.as_image,
            shown: 0,
        }
    }

    /// The choices to keep for the next time.
    pub fn remembered(&self, settings: &PrintSettings) -> PrintSettings {
        PrintSettings {
            printer: self.state.as_ref().map(|s| s.name.clone()).or_else(|| settings.printer.clone()),
            scale: self.scale,
            percent: self.percent,
            auto_rotate: self.auto_rotate,
            as_image: self.as_image,
        }
    }

    /// Ask `printer` with `devmode` (its defaults when `None`) for its
    /// papers and sheet; the answer replaces the state when it comes.
    fn ask(&mut self, ctx: &egui::Context, printer: String, devmode: Option<DevMode>) {
        let (send, receive) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("printer".into()).spawn(move || {
            let state = print::query(&printer, devmode);
            let _ = send.send(Answer { printers: None, state: Some(state) });
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => self.pending = Some(receive),
            Err(e) => log::warn!("cannot ask the printer: {e}"),
        }
    }

    /// The printer's settings changed here: shown at once, the sheet asked
    /// again.
    fn change(&mut self, ctx: &egui::Context, edit: impl FnOnce(&mut DevMode)) {
        let Some(state) = self.state.as_mut() else { return };
        let Some(devmode) = state.devmode.as_mut() else { return };
        edit(devmode);
        let (name, devmode) = (state.name.clone(), devmode.clone());
        self.ask(ctx, name, Some(devmode));
    }

    fn take_answers(&mut self, ctx: &egui::Context) {
        match self.pending.as_ref().map(|r| r.try_recv()) {
            Some(Ok(answer)) => {
                self.pending = None;
                if let Some((names, _)) = answer.printers {
                    self.printers = names;
                    self.listed = true;
                }
                if answer.state.is_some() {
                    self.state = answer.state;
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                // The thread died without an answer: not waited for.
                self.pending = None;
                self.listed = true;
            }
            _ => {}
        }
        if let Some(answer) = self.properties.as_ref().and_then(|r| r.try_recv().ok()) {
            self.properties = None;
            if let (Some(devmode), Some(state)) = (answer, self.state.as_ref()) {
                let name = state.name.clone();
                self.ask(ctx, name, Some(devmode));
            }
        }
    }

    /// The pages chosen, or why there are none.
    fn chosen(&self, doc: &DocState, resolve: &dyn Fn(&str) -> Option<usize>) -> Result<Vec<usize>, String> {
        match self.pages {
            Pages::All => Ok((0..doc.page_count).collect()),
            Pages::Current => Ok(vec![self.current.min(doc.page_count.saturating_sub(1))]),
            Pages::Range => print::parse_ranges(&self.range, doc.page_count, resolve).ok_or_else(|| {
                if self.range.trim().is_empty() {
                    tr!("Type the pages to print", "Введите страницы для печати").to_owned()
                } else {
                    tr!(format!("No such pages: {}", self.range.trim()), format!("Таких страниц нет: {}", self.range.trim()))
                }
            }),
        }
    }

    /// Draw the dialog; what the reader chose, if anything. `owner` is the
    /// window the driver's dialog belongs to.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        doc: &mut DocState,
        engine: &Engine,
        resolve: &dyn Fn(&str) -> Option<usize>,
        textures: &mut Textures,
        frame: u64,
        wanted: &mut Vec<(RenderKey, u8)>,
        owner: Option<isize>,
    ) -> Option<PrintAction> {
        self.take_answers(ctx);
        let palette = Palette::of(ctx);
        let chosen = self.chosen(doc, resolve);
        let pages = chosen.clone().unwrap_or_default();
        self.shown = self.shown.min(pages.len().saturating_sub(1));
        if let Some(&page) = pages.get(self.shown) {
            doc.need_sizes_near(engine, page);
        }
        let mut action = None;
        let mut print = false;
        let modal = egui::Modal::new(egui::Id::unique("print")).show(ctx, |ui| {
            ui.set_width(PREVIEW.x + 470.0);
            ui.horizontal(|ui| {
                ui.heading(tr!("Print", "Печать"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Not while the driver's dialog is open: its answer
                    // would go to a dialog that is gone.
                    if self.properties.is_none() && super::close_cross(ui) {
                        action = Some(PrintAction::Close);
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(PREVIEW.x);
                    self.preview(ui, doc, &pages, textures, frame, &palette, wanted);
                    self.preview_nav(ui, doc, &pages);
                });
                ui.add_space(20.0);
                ui.vertical(|ui| self.choices(ui, doc, owner));
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let text = |s: &str| RichText::new(s).size(16.0);
                let ready = self.state.is_some() && chosen.is_ok() && self.properties.is_none();
                if ui.add_enabled(ready, egui::Button::new(text(tr!("Print", "Печать"))).min_size(vec2(160.0, 34.0))).clicked() {
                    print = true;
                }
                if ui.add_enabled(self.properties.is_none(), egui::Button::new(text(tr!("Cancel", "Отмена"))).min_size(vec2(100.0, 34.0))).clicked() {
                    action = Some(PrintAction::Close);
                }
                ui.add_space(6.0);
                match (&chosen, &self.state) {
                    (Err(e), _) => {
                        ui.colored_label(palette.error, e);
                    }
                    (_, None) if self.listed && self.pending.is_none() => {
                        ui.colored_label(palette.error, tr!("No printer is installed", "Принтеры не установлены"));
                    }
                    (_, None) => {
                        ui.weak(tr!("Asking the printers…", "Опрос принтеров…"));
                    }
                    (Ok(pages), Some(_)) => {
                        let sheets = pages.len() * self.copies.max(1) as usize;
                        ui.weak(tr!(format!("{sheets} sheets"), format!("Листов: {sheets}")));
                    }
                }
            });
        });
        if action.is_none() && self.state.is_some() && chosen.is_ok() && self.properties.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            print = true;
        }
        if print
            && let (Some(state), Ok(pages)) = (&self.state, &chosen)
        {
            return Some(PrintAction::Print(PrintJob {
                printer: state.name.clone(),
                devmode: state.devmode.clone(),
                sheets: print::sheets(pages, self.copies as usize, self.collate),
                scale: self.scale,
                percent: self.percent,
                auto_rotate: self.auto_rotate,
                as_image: self.as_image,
                title: self.title.clone(),
                output: None,
            }));
        }
        // Esc or a click outside; not while the driver's dialog is up.
        if action.is_none() && self.properties.is_none() && modal.should_close() {
            action = Some(PrintAction::Close);
        }
        action
    }

    /// The printer, paper, orientation, sides, pages, copies and scale.
    fn choices(&mut self, ui: &mut Ui, doc: &DocState, owner: Option<isize>) {
        let ctx = ui.ctx().clone();
        egui::Grid::new("print-choices").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
            ui.label(tr!("Printer", "Принтер"));
            ui.horizontal(|ui| {
                let name = self.state.as_ref().map_or("—", |s| s.name.as_str()).to_owned();
                let mut picked = None;
                egui::ComboBox::from_id_salt("printer").width(250.0).selected_text(name.clone()).truncate().show_ui(ui, |ui| {
                    for p in &self.printers {
                        if ui.selectable_label(*p == name, p).clicked() && *p != name {
                            picked = Some(p.clone());
                        }
                    }
                });
                if let Some(p) = picked {
                    self.ask(&ctx, p, None);
                }
                let can = self.state.as_ref().is_some_and(|s| s.devmode.is_some()) && self.properties.is_none();
                let button = ui.add_enabled(can, egui::Button::new(tr!("Properties…", "Свойства…")));
                if button.on_hover_text(tr!("The printer's own settings: paper, quality, colour", "Собственные настройки принтера: бумага, качество, цвет")).clicked()
                    && let Some(state) = &self.state
                    && let Some(devmode) = state.devmode.clone()
                {
                    let (send, receive) = channel();
                    let (name, ctx) = (state.name.clone(), ctx.clone());
                    let spawned = std::thread::Builder::new().name("printer properties".into()).spawn(move || {
                        let _ = send.send(print::properties(owner.unwrap_or(0), &name, &devmode));
                        ctx.request_repaint();
                    });
                    if spawned.is_ok() {
                        self.properties = Some(receive);
                    }
                }
            });
            ui.end_row();

            let devmode = self.state.as_ref().and_then(|s| s.devmode.clone());
            let papers = self.state.as_ref().map(|s| s.papers.clone()).unwrap_or_default();
            if let Some(dm) = &devmode
                && !papers.is_empty()
            {
                ui.label(tr!("Paper", "Бумага"));
                let current = dm.paper();
                let name = papers.iter().find(|(id, _)| *id == current).map_or("—", |(_, n)| n.as_str()).to_owned();
                let mut picked = None;
                egui::ComboBox::from_id_salt("paper").width(250.0).selected_text(name).truncate().show_ui(ui, |ui| {
                    for (id, n) in &papers {
                        if ui.selectable_label(*id == current, n).clicked() && *id != current {
                            picked = Some(*id);
                        }
                    }
                });
                if let Some(id) = picked {
                    self.change(&ctx, |dm| dm.set_paper(id));
                }
                ui.end_row();
            }
            if let Some(dm) = &devmode {
                ui.label(tr!("Orientation", "Ориентация"));
                ui.horizontal(|ui| {
                    let landscape = dm.landscape();
                    if ui.selectable_label(!landscape, tr!("Portrait", "Книжная")).clicked() && landscape {
                        self.change(&ctx, |dm| dm.set_landscape(false));
                    }
                    if ui.selectable_label(landscape, tr!("Landscape", "Альбомная")).clicked() && !landscape {
                        self.change(&ctx, |dm| dm.set_landscape(true));
                    }
                });
                ui.end_row();
                if self.state.as_ref().is_some_and(|s| s.duplex) {
                    ui.label(tr!("Sides", "Стороны"));
                    let mut both = dm.duplex();
                    if ui.checkbox(&mut both, tr!("Print on both sides", "Печатать на обеих сторонах")).changed() {
                        self.change(&ctx, |dm| dm.set_duplex(both));
                    }
                    ui.end_row();
                }
            }

            ui.label(tr!("Pages", "Страницы"));
            ui.vertical(|ui| {
                ui.radio_value(&mut self.pages, Pages::All, tr!(format!("All ({})", doc.page_count), format!("Все ({})", doc.page_count)));
                let label = doc.label(self.current);
                ui.radio_value(&mut self.pages, Pages::Current, tr!(format!("This page ({label})"), format!("Текущая ({label})")));
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.pages, Pages::Range, tr!("Pages", "Диапазон"));
                    let field = egui::TextEdit::singleline(&mut self.range).desired_width(140.0).hint_text(tr!("1-5, 8, 11-", "1-5, 8, 11-"));
                    let response = ui.add(field).on_hover_text(tr!(
                        "Page numbers as the page field reads them, ranges with a dash",
                        "Номера страниц, как их понимает поле страницы; диапазоны через дефис"
                    ));
                    if response.gained_focus() || response.changed() {
                        self.pages = Pages::Range;
                        self.shown = 0;
                    }
                });
            });
            ui.end_row();

            ui.label(tr!("Copies", "Копии"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.copies).range(1..=999).speed(0.1));
                ui.add_enabled(self.copies > 1, egui::Checkbox::new(&mut self.collate, tr!("Collate", "Разобрать по копиям")))
                    .on_hover_text(tr!("Each copy printed whole, one after another", "Каждая копия печатается целиком, одна за другой"));
            });
            ui.end_row();

            ui.label(tr!("Scale", "Масштаб"));
            ui.vertical(|ui| {
                ui.radio_value(&mut self.scale, PrintScale::Shrink, tr!("Shrink large pages", "Уменьшать крупные страницы"))
                    .on_hover_text(tr!("Pages larger than the sheet fit into it; the others print at their size", "Страницы больше листа вписываются в него, остальные печатаются в своём размере"));
                ui.radio_value(&mut self.scale, PrintScale::Fit, tr!("Fit to the sheet", "Вписать в лист"));
                ui.radio_value(&mut self.scale, PrintScale::Actual, tr!("Actual size", "Истинный размер"));
                // A scale of one's own: the field chooses it as it changes.
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.scale, PrintScale::Custom, tr!("Custom", "Свой"));
                    let field = egui::DragValue::new(&mut self.percent).range(PRINT_PERCENT).speed(1.0).suffix(" %");
                    let response = ui.add(field).on_hover_text(tr!("The pages at this share of their size, centred on the sheet", "Страницы в этой доле своего размера, по центру листа"));
                    if response.changed() || response.gained_focus() {
                        self.scale = PrintScale::Custom;
                    }
                });
                if !doc.print_scaling {
                    ui.weak(tr!("The document asks to be printed at its size", "Документ просит печатать его в истинном размере"));
                }
            });
            ui.end_row();

            ui.label("");
            ui.vertical(|ui| {
                ui.checkbox(&mut self.auto_rotate, tr!("Turn pages to the sheet", "Поворачивать страницы по листу"))
                    .on_hover_text(tr!("A landscape page on a portrait sheet is turned, and the reverse", "Альбомная страница на книжном листе поворачивается, и наоборот"));
                ui.checkbox(&mut self.as_image, tr!("Print as an image", "Печатать как изображение")).on_hover_text(tr!(
                    "Each page is sent as a picture of 300 dpi: slower and larger, for a printer that prints the pages wrong",
                    "Каждая страница отправляется картинкой 300 dpi: медленнее и объёмнее, для принтера, который печатает страницы неверно"
                ));
            });
            ui.end_row();
        });
    }

    /// The sheet shown as it will print: the paper, the part the printer
    /// reaches, the page placed on it.
    #[allow(clippy::too_many_arguments)]
    fn preview(&self, ui: &mut Ui, doc: &DocState, pages: &[usize], textures: &mut Textures, frame: u64, palette: &Palette, wanted: &mut Vec<(RenderKey, u8)>) {
        let (area, _) = ui.allocate_exact_size(PREVIEW, Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(area, 4.0, palette.backdrop);
        let sheet = self.state.as_ref().and_then(|s| s.sheet).unwrap_or(DEFAULT_SHEET);
        let room = area.shrink(14.0);
        let k = (room.width() / sheet.size[0].max(1.0)).min(room.height() / sheet.size[1].max(1.0));
        let paper = Rect::from_center_size(area.center(), vec2(sheet.size[0] * k, sheet.size[1] * k));
        painter.rect_filled(paper.translate(vec2(1.0, 2.0)), 1.0, palette.shadow);
        painter.rect_filled(paper, 0.0, Color32::WHITE);
        let [px, py, pw, ph] = sheet.printable;
        let printable = Rect::from_min_size(paper.min + vec2(px * k, py * k), vec2(pw * k, ph * k));
        if printable.width() < paper.width() - 1.0 || printable.height() < paper.height() - 1.0 {
            painter.rect_stroke(printable, 0.0, Stroke::new(1.0, Color32::from_gray(225)), StrokeKind::Inside);
        }
        let Some(&page) = pages.get(self.shown) else { return };
        let placed = print::place(doc.size_of(page), &sheet, self.scale, self.percent, self.auto_rotate);
        let [x, y, w, h] = placed.rect;
        let rect = Rect::from_min_size(printable.min + vec2(x * k, y * k), vec2(w * k, h * k));
        let clipped = painter.with_clip_rect(paper);
        let ppp = ui.ctx().pixels_per_point();
        let width = ((rect.width() * ppp).round() as u32).clamp(16, 4096);
        let key = RenderKey { doc: doc.id, page: page as u32, width, turns: placed.rotate as u8, kind: Kind::Full };
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if let Some(tex) = textures.full.get(&key, frame) {
            clipped.image(tex.id(), rect, uv, Color32::WHITE);
        } else {
            wanted.push((key, 1));
            let preview_key = RenderKey { width: PREVIEW_WIDTH, kind: Kind::Preview, ..key };
            if let Some((_, tex)) = textures.full.nearest(&key, frame) {
                clipped.image(tex.id(), rect, uv, Color32::WHITE);
            } else if let Some(tex) = textures.preview.get(&preview_key, frame) {
                clipped.image(tex.id(), rect, uv, Color32::WHITE);
            } else {
                wanted.push((preview_key, 0));
            }
        }
        clipped.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(200)), StrokeKind::Outside);
    }

    /// The sheet shown, among the pages chosen, and the steps to the others.
    fn preview_nav(&mut self, ui: &mut Ui, doc: &DocState, pages: &[usize]) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let n = pages.len();
            let text = match pages.get(self.shown) {
                Some(&page) => tr!(format!("Sheet {} of {n} · page {}", self.shown + 1, doc.label(page)), format!("Лист {} из {n} · стр. {}", self.shown + 1, doc.label(page))),
                None => String::new(),
            };
            if ui.add_enabled_ui(self.shown > 0, |ui| super::icon_button(ui, Icon::Prev, false, tr!("Previous sheet", "Предыдущий лист"))).inner.clicked() {
                self.shown -= 1;
            }
            // Between the steps, centred under the sheet.
            let room = vec2(PREVIEW.x - 2.0 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.x) - 8.0, ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(room, Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| ui.label(text));
            if ui.add_enabled_ui(self.shown + 1 < n, |ui| super::icon_button(ui, Icon::Next, false, tr!("Next sheet", "Следующий лист"))).inner.clicked() {
                self.shown += 1;
            }
        });
    }
}
