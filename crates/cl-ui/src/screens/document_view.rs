//! A document in its own tab (P17, `nav.document_view`, plan §3 WP1,
//! Q-P17l): "Open full size" from a preview, with zoom and page controls.
//!
//! # Handoff (WP0 skeleton, WP1 filled it)
//!
//! A preview's screen pushes `CompanyEvent::OpenWith(nav.document_view,
//! OpenPayload::Document(source))`; [`DocumentViewScreen::open_with`] takes
//! it and puts the keyboard in the viewer. The tab reports `Scope::Viewer`
//! while the viewer has the keyboard, so the `viewer.*` keys reach
//! [`ViewerState::command`]. Opened bare (the palette, a restored layout) it
//! shows the viewer's empty state. "Save a copy" (a PDF that cannot be
//! previewed) writes the bytes it was handed to the Downloads folder.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocViewer, ViewerEvent, ViewerState};
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "nav.document_view";

/// The document tab for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    let _ = (link, open, today);
    (kind.as_str() == KIND).then(|| Box::new(DocumentViewScreen::default()) as Box<_>)
}

/// The document tab.
#[derive(Debug, Default)]
pub struct DocumentViewScreen {
    viewer: ViewerState,
    /// Where "Save a copy" put the file, or the i18n key of its problem.
    saved: Option<Result<PathBuf, &'static str>>,
}

/// Where "Save a copy" puts a file: the Downloads folder, else the
/// temporary folder.
fn copy_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    home.map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// `path` as shown on screen: the owner's home folder (which carries the
/// sign-in name) reads `~`, so no user name is on screen or in a
/// screenshot.
#[must_use]
pub(crate) fn shown_path(path: &Path) -> String {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    shown_path_in(path, home.as_deref().map(Path::new))
}

/// [`shown_path`] with the home folder given.
fn shown_path_in(path: &Path, home: Option<&Path>) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    match home
        .filter(|h| !h.as_os_str().is_empty())
        .map(|h| path.strip_prefix(h))
    {
        Some(Ok(rest)) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(Ok(rest)) => format!("~{sep}{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// Writes `bytes` (a PDF, PNG or JPEG, checked again here) into `dir` as a
/// new file named "Document preview" with the detected type's extension,
/// never overwriting (` (2)`, ` (3)`, … instead). The owner's file name is
/// not used, so nothing in it can steer the path.
///
/// # Errors
/// An i18n key for the owner.
pub(crate) fn save_copy(dir: &Path, bytes: &[u8]) -> Result<PathBuf, &'static str> {
    let ext = match cl_api::check_file(bytes) {
        Ok(kind) => kind.extension(),
        Err(_) => return Err("check.attach.download_wrong_type"),
    };
    for n in 1..1000 {
        let file = if n == 1 {
            format!("Document preview.{ext}")
        } else {
            format!("Document preview ({n}).{ext}")
        };
        let path = dir.join(file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                if f.write_all(bytes).and_then(|()| f.sync_all()).is_err() {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err("check.attach.download_failed");
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("check.attach.download_failed"),
        }
    }
    Err("check.attach.download_failed")
}

impl Screen for DocumentViewScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        egui::Frame::new()
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                if let Some(saved) = &self.saved {
                    let (text, color) = match saved {
                        Ok(p) => (
                            t_with("check.attach.saved_to", &[("path", &shown_path(p))]),
                            tokens.text_secondary,
                        ),
                        Err(key) => (t(key), tokens.warn_text),
                    };
                    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap());
                }
                if DocViewer::show(ui, &mut self.viewer, &tokens) == ViewerEvent::SaveCopy
                    && let Some(src) = self.viewer.source()
                {
                    self.saved = Some(save_copy(&copy_dir(), &src.bytes));
                }
            });
    }

    fn fixed_layout(&self) -> bool {
        // The viewer scrolls the page itself; the toolbar stays put.
        true
    }

    fn scope(&self) -> Scope {
        if self.viewer.has_focus() {
            Scope::Viewer
        } else {
            Scope::Window
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        self.viewer.command(id)
    }

    /// The tab's document (A02): the pop-out viewer follows it. The tab
    /// is already full size, so it draws the viewer without Pop out.
    fn document(&self) -> Option<crate::windows::doc_viewer::DocRef> {
        self.viewer
            .source()
            .cloned()
            .map(crate::windows::doc_viewer::DocRef::new)
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::Document(source) => {
                self.viewer.open(source);
                self.viewer.focus();
                self.saved = None;
                true
            }
            OpenPayload::BillPrefill(_)
            | OpenPayload::InboxDraft(_)
            | OpenPayload::DocumentDraft(_)
            | OpenPayload::StatementDraft(_)
            | OpenPayload::PrintChecks(_)
            | OpenPayload::ReceiveFrom(_)
            | OpenPayload::RefundFrom(..)
            | OpenPayload::ForClient(_)
            | OpenPayload::ReprintCheck(..)
            | OpenPayload::PrintDeposit(..) => false,
        }
    }

    fn state(&self) -> Value {
        json!({
            "screen": "document_view",
            "loaded": true,
            "viewer": self.viewer.state_json(),
            "saved_copy": self.saved.as_ref().is_some_and(Result::is_ok),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::pdf_viewer::DocSource;

    /// The home folder (with the sign-in name) reads `~`; other paths are
    /// shown as they are.
    #[test]
    fn paths_under_home_hide_the_user_name() {
        let home = std::env::temp_dir().join("home-of-someone");
        let file = home.join("Downloads").join("Document preview.pdf");
        let shown = shown_path_in(&file, Some(&home));
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(shown, format!("~{sep}Downloads{sep}Document preview.pdf"));
        assert!(!shown.contains("home-of-someone"));
        assert_eq!(shown_path_in(&home, Some(&home)), "~");
        let other = std::env::temp_dir().join("elsewhere").join("a.pdf");
        assert_eq!(
            shown_path_in(&other, Some(&home)),
            other.display().to_string()
        );
        assert_eq!(shown_path_in(&file, None), file.display().to_string());
    }

    #[test]
    fn takes_a_document_and_nothing_else() {
        let mut s = DocumentViewScreen::default();
        assert_eq!(s.scope(), Scope::Window);
        assert_eq!(s.state()["viewer"]["open"], false);
        assert!(s.open_with(OpenPayload::Document(DocSource {
            name: "a.pdf".into(),
            bytes: Arc::from(&b"%PDF-1.7"[..]),
        })));
        assert_eq!(s.state()["viewer"]["open"], true);
        assert_eq!(s.title(), "Document Preview");
        assert!(s.command(CommandId::new("viewer.fit")));
        assert!(!s.state().to_string().contains("a.pdf"));
        let doc = s.document().expect("the window follows the tab");
        assert!(!format!("{doc:?}").contains("a.pdf"));
    }

    #[test]
    fn save_a_copy_never_overwrites_and_checks_the_type() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = b"%PDF-1.7\n%%EOF\n";
        let a = save_copy(dir.path(), pdf).unwrap();
        let b = save_copy(dir.path(), pdf).unwrap();
        assert_eq!(a.file_name().unwrap(), "Document preview.pdf");
        assert_eq!(b.file_name().unwrap(), "Document preview (2).pdf");
        assert_eq!(std::fs::read(&a).unwrap(), pdf);
        assert_eq!(
            save_copy(dir.path(), b"MZ not a document"),
            Err("check.attach.download_wrong_type")
        );
    }
}
