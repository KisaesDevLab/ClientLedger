//! The native "Browseâ€¦" dialogs: choose a folder (Document intake, A01
//! Q-A01m) or files (attachments, Q-P13k).
//!
//! - **The egui thread never waits.** [`pick_folder`] and [`pick_files`]
//!   return a [`Receiver`] at once; the dialog runs on its own thread and
//!   the answer arrives there (polled each frame with [`poll`]). The
//!   dialog is `rfd`'s asynchronous one, driven on that thread: on Windows
//!   the system IFileDialog, on macOS NSOpenPanel shown as a sheet (so the
//!   window keeps drawing), on Linux the xdg-desktop-portal file chooser.
//!   On Linux `rfd` loads the system `libdbus-1` at run time to reach the
//!   portal and, when no portal answers, starts the system `zenity`
//!   instead (as `webbrowser` starts the system browser for links); no GTK
//!   library is linked, and no process code lives in this crate.
//! - **A chosen path is owner data.** It goes to the field that asked and
//!   is never logged, put in `Debug` output or in the control channel's
//!   state. The last folder each use opened stays in this app's memory
//!   (the egui context) for the next Browse; nothing is written to disk.
//! - **Tests and the control channel never open a dialog.** A
//!   [`PickerHook`] sits in the egui context (always in unit tests;
//!   installed by the control channel) and answers each dialog from the
//!   answers queued with [`PickerHook::answer`] â€” the scenario step
//!   `dialog_answer` â€” or as cancelled when none is queued.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Wake, Waker};

use eframe::egui::{self, Context};

/// What a dialog is for: each use remembers its own last folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PickUse {
    /// Preferences â€º Document intake: a folder to watch.
    IntakeFolder,
    /// A file to attach to a bill or a check.
    Attachment,
}

impl PickUse {
    /// The dialog's title (an i18n key).
    fn title_key(self) -> &'static str {
        match self {
            PickUse::IntakeFolder => "picker.title.intake_folder",
            PickUse::Attachment => "picker.title.attachment",
        }
    }
}

/// A file-type choice in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    /// The choice's name (an i18n key).
    pub name_key: &'static str,
    /// Extensions without the dot.
    pub extensions: &'static [&'static str],
}

/// The files an attachment may be: PDF, JPEG, PNG or TIFF (the bytes are
/// still checked before anything is sent).
pub const DOCUMENTS: Filter = Filter {
    name_key: "picker.filter.documents",
    extensions: &["pdf", "jpg", "jpeg", "png", "tif", "tiff"],
};

/// One queued answer for the [`PickerHook`].
#[derive(Clone, PartialEq, Eq)]
pub enum Answer {
    /// The owner chose these (a folder dialog takes the first).
    Paths(Vec<PathBuf>),
    /// The owner cancelled.
    Cancel,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Paths are owner data: only how many.
            Answer::Paths(p) => write!(f, "Paths({})", p.len()),
            Answer::Cancel => f.write_str("Cancel"),
        }
    }
}

#[derive(Default)]
struct HookInner {
    answers: VecDeque<Answer>,
    asked: usize,
}

/// Answers dialogs instead of the OS (unit tests, the control channel).
#[derive(Clone, Default)]
pub struct PickerHook(Arc<Mutex<HookInner>>);

fn hook_id() -> egui::Id {
    egui::Id::unique("cl.picker_hook")
}

impl PickerHook {
    /// Puts a hook into `ctx`: from now on no dialog opens.
    #[must_use]
    pub fn install(ctx: &Context) -> PickerHook {
        if let Some(h) = PickerHook::of(ctx) {
            return h;
        }
        let h = PickerHook::default();
        ctx.data_mut(|d| d.insert_temp(hook_id(), h.clone()));
        h
    }

    /// The hook in `ctx`, if one was installed.
    #[must_use]
    pub fn of(ctx: &Context) -> Option<PickerHook> {
        ctx.data(|d| d.get_temp::<PickerHook>(hook_id()))
    }

    /// Queues the answer for the next dialog.
    pub fn answer(&self, a: Answer) {
        if let Ok(mut s) = self.0.lock() {
            s.answers.push_back(a);
        }
    }

    /// How many dialogs were asked for so far.
    #[must_use]
    pub fn asked(&self) -> usize {
        self.0.lock().map_or(0, |s| s.asked)
    }

    fn take(&self) -> Answer {
        match self.0.lock() {
            Ok(mut s) => {
                s.asked += 1;
                s.answers.pop_front().unwrap_or(Answer::Cancel)
            }
            Err(_) => Answer::Cancel,
        }
    }
}

/// The hook that answers in `ctx`: always one in unit tests.
fn active_hook(ctx: &Context) -> Option<PickerHook> {
    if cfg!(test) {
        Some(PickerHook::install(ctx))
    } else {
        PickerHook::of(ctx)
    }
}

/// The last folder each use opened (in memory only; `Debug` is not
/// derived, the paths are owner data).
#[derive(Clone, Default)]
struct LastFolders(Arc<Mutex<BTreeMap<PickUse, PathBuf>>>);

fn last_id() -> egui::Id {
    egui::Id::unique("cl.picker_last_folders")
}

fn last_folders(ctx: &Context) -> LastFolders {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<LastFolders>(last_id()).clone())
}

/// The folder `use_`'s dialog opened in last (where the owner chose).
#[must_use]
pub fn last_folder(ctx: &Context, use_: PickUse) -> Option<PathBuf> {
    last_folders(ctx)
        .0
        .lock()
        .ok()
        .and_then(|m| m.get(&use_).cloned())
}

fn remember(last: &LastFolders, use_: PickUse, folder: Option<&Path>) {
    if let (Some(dir), Ok(mut m)) = (folder, last.0.lock()) {
        m.insert(use_, dir.to_path_buf());
    }
}

/// Asks for one folder. `start`: where the dialog opens (else the last
/// folder chosen for `use_`). The answer is `None` when cancelled.
#[must_use]
pub fn pick_folder(
    ctx: &Context,
    use_: PickUse,
    start: Option<&Path>,
) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = channel();
    let last = last_folders(ctx);
    if let Some(hook) = active_hook(ctx) {
        let chosen = match hook.take() {
            Answer::Paths(p) => p.into_iter().next(),
            Answer::Cancel => None,
        };
        remember(&last, use_, chosen.as_deref());
        let _ = tx.send(chosen);
        return rx;
    }
    let start = start
        .map(Path::to_path_buf)
        .or_else(|| last_folder(ctx, use_));
    let title = crate::i18n::t(use_.title_key());
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("cl-browse".into())
        .spawn(move || {
            let mut d = rfd::AsyncFileDialog::new().set_title(title);
            if let Some(dir) = start {
                d = d.set_directory(dir);
            }
            let chosen = block_on(d.pick_folder()).map(|h| h.path().to_path_buf());
            remember(&last, use_, chosen.as_deref());
            let _ = tx.send(chosen);
            repaint.request_repaint();
        });
    if spawned.is_err() {
        // No thread: the sender is gone, so the answer reads as cancelled.
        tracing::warn!("the Browse dialog could not start");
    }
    rx
}

/// Asks for files of the `filters` types (one, or several when `many`),
/// opening in the last folder chosen for `use_`. The answer is empty when
/// cancelled.
#[must_use]
pub fn pick_files(
    ctx: &Context,
    use_: PickUse,
    filters: &[Filter],
    many: bool,
) -> Receiver<Vec<PathBuf>> {
    let (tx, rx) = channel();
    let last = last_folders(ctx);
    if let Some(hook) = active_hook(ctx) {
        let mut chosen = match hook.take() {
            Answer::Paths(p) => p,
            Answer::Cancel => Vec::new(),
        };
        if !many {
            chosen.truncate(1);
        }
        remember(&last, use_, chosen.first().and_then(|p| p.parent()));
        let _ = tx.send(chosen);
        return rx;
    }
    let start = last_folder(ctx, use_);
    let title = crate::i18n::t(use_.title_key());
    let filters: Vec<(String, &'static [&'static str])> = filters
        .iter()
        .map(|f| (crate::i18n::t(f.name_key), f.extensions))
        .collect();
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("cl-browse".into())
        .spawn(move || {
            let mut d = rfd::AsyncFileDialog::new().set_title(title);
            for (name, ext) in &filters {
                d = d.add_filter(name.as_str(), ext);
            }
            if let Some(dir) = start {
                d = d.set_directory(dir);
            }
            let chosen: Vec<PathBuf> = if many {
                block_on(d.pick_files())
                    .unwrap_or_default()
                    .iter()
                    .map(|h| h.path().to_path_buf())
                    .collect()
            } else {
                block_on(d.pick_file())
                    .map(|h| h.path().to_path_buf())
                    .into_iter()
                    .collect()
            };
            remember(&last, use_, chosen.first().and_then(|p| p.parent()));
            let _ = tx.send(chosen);
            repaint.request_repaint();
        });
    if spawned.is_err() {
        tracing::warn!("the Browse dialog could not start");
    }
    rx
}

/// The answer in `slot`, once there is one (the slot is then emptied). A
/// dialog whose thread ended without answering counts as cancelled.
pub fn poll<T: Default>(slot: &mut Option<Receiver<T>>) -> Option<T> {
    let rx = slot.as_ref()?;
    let r = match rx.try_recv() {
        Ok(v) => v,
        Err(TryRecvError::Empty) => return None,
        Err(TryRecvError::Disconnected) => T::default(),
    };
    *slot = None;
    Some(r)
}

/// Wakes the dialog thread when its future is ready.
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `fut` to the end on this (dialog) thread.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_chosen_through_the_hook_arrives_and_is_remembered() {
        let ctx = Context::default();
        let hook = PickerHook::install(&ctx);
        let dir = std::env::temp_dir().join("cl-picker-folder");
        hook.answer(Answer::Paths(vec![dir.clone()]));
        let mut slot = Some(pick_folder(&ctx, PickUse::IntakeFolder, None));
        assert_eq!(poll(&mut slot), Some(Some(dir.clone())));
        assert!(slot.is_none(), "answered once");
        assert_eq!(hook.asked(), 1);
        assert_eq!(last_folder(&ctx, PickUse::IntakeFolder), Some(dir));
        assert_eq!(last_folder(&ctx, PickUse::Attachment), None, "per use");
    }

    #[test]
    fn a_cancelled_dialog_answers_none_and_keeps_the_last_folder() {
        let ctx = Context::default();
        let hook = PickerHook::install(&ctx);
        let dir = std::env::temp_dir().join("cl-picker-kept");
        hook.answer(Answer::Paths(vec![dir.clone()]));
        let mut slot = Some(pick_folder(&ctx, PickUse::IntakeFolder, None));
        let _ = poll(&mut slot);
        hook.answer(Answer::Cancel);
        let mut slot = Some(pick_folder(&ctx, PickUse::IntakeFolder, None));
        assert_eq!(poll(&mut slot), Some(None));
        // Nothing queued: also cancelled, and still no dialog opens.
        let mut slot = Some(pick_folder(&ctx, PickUse::IntakeFolder, None));
        assert_eq!(poll(&mut slot), Some(None));
        assert_eq!(hook.asked(), 3);
        assert_eq!(last_folder(&ctx, PickUse::IntakeFolder), Some(dir));
    }

    #[test]
    fn files_chosen_remember_their_folder() {
        let ctx = Context::default();
        let hook = PickerHook::install(&ctx);
        let dir = std::env::temp_dir().join("cl-picker-files");
        let file = dir.join("bill.pdf");
        let other = dir.join("receipt.png");
        hook.answer(Answer::Paths(vec![file.clone(), other.clone()]));
        let mut slot = Some(pick_files(&ctx, PickUse::Attachment, &[DOCUMENTS], true));
        assert_eq!(poll(&mut slot), Some(vec![file.clone(), other]));
        assert_eq!(last_folder(&ctx, PickUse::Attachment), Some(dir));
        // One file only: the first.
        hook.answer(Answer::Paths(vec![file.clone(), file.clone()]));
        let mut slot = Some(pick_files(&ctx, PickUse::Attachment, &[DOCUMENTS], false));
        assert_eq!(poll(&mut slot), Some(vec![file]));
        hook.answer(Answer::Cancel);
        let mut slot = Some(pick_files(&ctx, PickUse::Attachment, &[DOCUMENTS], false));
        assert_eq!(poll(&mut slot), Some(Vec::new()));
    }

    #[test]
    fn a_dialog_that_ends_without_answering_is_cancelled() {
        let (tx, rx) = channel::<Option<PathBuf>>();
        let mut slot = Some(rx);
        assert_eq!(poll(&mut slot), None, "still open");
        drop(tx);
        assert_eq!(poll(&mut slot), Some(None));
        assert!(slot.is_none());
        let mut none: Option<Receiver<Vec<PathBuf>>> = None;
        assert_eq!(poll(&mut none), None);
    }

    /// A future that is woken from another thread.
    struct Later(Option<std::thread::JoinHandle<()>>, Arc<Mutex<bool>>);

    impl Future for Later {
        type Output = u8;
        fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<u8> {
            if *self.1.lock().expect("lock") {
                return Poll::Ready(1);
            }
            if self.0.is_none() {
                let (flag, waker) = (Arc::clone(&self.1), cx.waker().clone());
                self.0 = Some(std::thread::spawn(move || {
                    *flag.lock().expect("lock") = true;
                    waker.wake();
                }));
            }
            Poll::Pending
        }
    }

    #[test]
    fn block_on_runs_a_future_to_its_end() {
        assert_eq!(block_on(async { 7 }), 7);
        assert_eq!(block_on(Later(None, Arc::new(Mutex::new(false)))), 1);
    }

    #[test]
    fn answers_hide_their_paths_and_the_texts_exist() {
        let a = Answer::Paths(vec![PathBuf::from("C:/Owner/Secret")]);
        assert_eq!(format!("{a:?}"), "Paths(1)");
        for key in [
            PickUse::IntakeFolder.title_key(),
            PickUse::Attachment.title_key(),
            DOCUMENTS.name_key,
        ] {
            assert!(crate::i18n::has(key), "{key}");
        }
    }
}
