//! The file dialogs, each on a thread of its own: shown on the UI thread, a
//! dialog runs its own message loop inside the frame that opened it, and
//! the pages stood still until it closed. The app's window is still
//! the dialog's owner, so Windows disables it meanwhile, as for a modal
//! dialog; the app goes on painting. One dialog at a time; the answer is
//! taken in `App::logic` ([`Dialogs::take`]).

use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle,
};

/// What a dialog was opened for, and what the answer needs.
#[derive(Debug)]
pub enum Ask {
    /// PDF files to open, in the window that asked.
    Open { window: u64 },
    /// Where to save the edited document of a tab.
    SaveAs { tab: u64 },
    /// The picture to put into image `object` of `page` of a tab.
    ReplaceImage { tab: u64, page: usize, object: usize },
    /// A PDF whose pages go in at `page` of a tab's document, or replace
    /// that page.
    ImportPdf { tab: u64, page: usize, replace: bool },
}

impl Ask {
    /// For the log.
    fn name(&self) -> &'static str {
        match self {
            Ask::Open { .. } => "open",
            Ask::SaveAs { .. } => "save as",
            Ask::ReplaceImage { .. } => "replace image",
            Ask::ImportPdf { .. } => "import pages",
        }
    }
}

/// A dialog's answer: what it was for and the paths chosen (none when it
/// was cancelled).
#[derive(Debug)]
pub struct Answer {
    pub ask: Ask,
    pub paths: Vec<PathBuf>,
}

#[derive(Default)]
pub struct Dialogs {
    pending: Option<Receiver<Answer>>,
}

impl Dialogs {
    /// A dialog is open.
    pub fn open(&self) -> bool {
        self.pending.is_some()
    }

    /// Show the dialog `make` builds, owned by window `owner`, and `run` it
    /// on a thread of its own; `ctx` is woken with the answer. Nothing
    /// while another is open.
    pub fn start(
        &mut self,
        ctx: &egui::Context,
        owner: Option<isize>,
        ask: Ask,
        make: impl FnOnce() -> rfd::FileDialog + Send + 'static,
        run: fn(rfd::FileDialog) -> Vec<PathBuf>,
    ) {
        if self.open() {
            return;
        }
        let (send, receive) = channel();
        let ctx = ctx.clone();
        log::debug!("file dialog for {}", ask.name());
        let spawned = std::thread::Builder::new()
            .name("file dialog".into())
            .spawn(move || {
                let mut dialog = make();
                if let Some(owner) = owner.and_then(NonZeroIsize::new) {
                    dialog = dialog.set_parent(&Owner(owner));
                }
                let paths = run(dialog);
                log::debug!("file dialog closed: {} chosen", paths.len());
                let _ = send.send(Answer { ask, paths });
                ctx.request_repaint();
            });
        match spawned {
            Ok(_) => self.pending = Some(receive),
            Err(e) => log::warn!("cannot show a file dialog: {e}"),
        }
    }

    /// The answer of the dialog, once it is closed.
    pub fn take(&mut self) -> Option<Answer> {
        let answer = match self.pending.as_ref()?.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        self.pending = None;
        answer
    }
}

/// The files chosen in an Open dialog.
pub fn pick_files(dialog: rfd::FileDialog) -> Vec<PathBuf> {
    dialog.pick_files().unwrap_or_default()
}

pub fn pick_file(dialog: rfd::FileDialog) -> Vec<PathBuf> {
    dialog.pick_file().into_iter().collect()
}

pub fn save_file(dialog: rfd::FileDialog) -> Vec<PathBuf> {
    dialog.save_file().into_iter().collect()
}

/// The app's window as a dialog's owner, from another thread.
struct Owner(NonZeroIsize);

impl HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // The window lives as long as the app, longer than its dialogs.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}
