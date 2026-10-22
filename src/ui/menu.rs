//! Context menus that behave as Windows' do, as in qview.
//!
//! egui counts a press and a release as a click only within 6 points and
//! 0.8 s, so a right click with the mouse on the move, or held a moment,
//! opened no context menu, and egui closes a menu on a click only, so a
//! press followed by a drag left an open menu in place.

use egui::{Event, PointerButton, Popup, SetOpenCommand};

/// The right button's press, held back until it is let go (`App`'s
/// `raw_input_hook`): as Windows sends WM_CONTEXTMENU on the release, egui
/// gets the press together with the release, where the button is let go,
/// which is always a click there. Nothing in MetalPDF drags with the right
/// button.
#[derive(Default)]
pub struct RightClick {
    held: bool,
}

impl RightClick {
    pub fn hook(&mut self, events: &mut Vec<Event>) {
        let right = |e: &Event| {
            matches!(
                e,
                Event::PointerButton {
                    button: PointerButton::Secondary,
                    ..
                }
            )
        };
        if !events.iter().any(right) {
            return;
        }
        let mut out = Vec::with_capacity(events.len() + 1);
        for e in events.drain(..) {
            match e {
                Event::PointerButton {
                    button: PointerButton::Secondary,
                    pressed: true,
                    ..
                } => self.held = true,
                Event::PointerButton {
                    pos,
                    button: PointerButton::Secondary,
                    pressed: false,
                    modifiers,
                } if self.held => {
                    self.held = false;
                    out.push(Event::PointerButton {
                        pos,
                        button: PointerButton::Secondary,
                        pressed: true,
                        modifiers,
                    });
                    out.push(e);
                }
                e => out.push(e),
            }
        }
        *events = out;
    }
}

/// `response.context_menu`, as Windows' menus behave: it opens on the right
/// button's release wherever the pointer went (`RightClick`), and a press
/// anywhere off the menus closes it, whatever the pointer does next. A
/// right click on the widget while its menu is shown opens it anew at the
/// pointer in the next frame: egui closes a menu shown in the last frame on
/// any click, so it would only close.
pub fn context_menu(
    response: &egui::Response,
    add: impl FnOnce(&mut egui::Ui),
) -> Option<egui::InnerResponse<()>> {
    let ctx = &response.ctx;
    let menu = Popup::default_response_id(response);
    let reopen = menu.with("reopen");
    let (pressed, pos) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
    let on_menus = pos
        .and_then(|p| ctx.layer_id_at(p))
        .is_some_and(|layer| layer.order == egui::Order::Foreground);
    if pressed && !on_menus && Popup::is_id_open(ctx, menu) {
        Popup::close_id(ctx, menu);
    }
    let mut open = ctx
        .data_mut(|d| d.remove_temp::<bool>(reopen))
        .unwrap_or(false);
    if response.secondary_clicked() {
        if ctx.read_response(menu).is_some() {
            ctx.data_mut(|d| d.insert_temp(reopen, true));
            ctx.request_repaint();
        } else {
            open = true;
        }
    }
    let command = if open {
        Some(SetOpenCommand::Bool(true))
    } else if response.clicked() {
        Some(SetOpenCommand::Bool(false))
    } else {
        None
    };
    Popup::menu(response)
        .open_memory(command)
        .at_pointer_fixed()
        .show(add)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    #[test]
    fn the_right_press_waits_for_the_release() {
        let button = |x: f32, pressed: bool| Event::PointerButton {
            pos: egui::pos2(x, 0.0),
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let moved = |x: f32| Event::PointerMoved(egui::pos2(x, 0.0));
        let mut right = RightClick::default();
        let mut events = vec![moved(10.0), button(10.0, true)];
        right.hook(&mut events);
        assert_eq!(events, vec![moved(10.0)]);
        let mut events = vec![moved(40.0)];
        right.hook(&mut events);
        assert_eq!(events, vec![moved(40.0)]);
        // Pressed and let go where the button is released.
        let mut events = vec![moved(50.0), button(50.0, false)];
        right.hook(&mut events);
        assert_eq!(
            events,
            vec![moved(50.0), button(50.0, true), button(50.0, false)]
        );
        // A release with no press seen is passed on as it is.
        let mut events = vec![button(50.0, false)];
        right.hook(&mut events);
        assert_eq!(events, vec![button(50.0, false)]);
        // Other buttons are untouched.
        let mut events = vec![Event::PointerButton {
            pos: egui::pos2(0.0, 0.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }];
        let before = events.clone();
        right.hook(&mut events);
        assert_eq!(events, before);
    }
}
