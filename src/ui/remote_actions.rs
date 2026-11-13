//! App's handlers for a file on a host (`core::remote_file`): fetching it off the UI thread
//! before the window shows it, and Reload and Keep mine when it changed there. Saving it is
//! `App::save`, which sends it on a thread of its own too: a host can take seconds to answer,
//! and the window never waits on one.

use std::time::Instant;

use iced::Task;
use iced::futures::channel::oneshot;
use iced::widget::operation::focus;

use crate::core::opened::{Doc, OPEN_ELSEWHERE, open_elsewhere};
use crate::core::remote::{Remote, Ssh};
use crate::core::remote_file::{self, Fetched};
use crate::ui::app::{App, Message, Opening};
use crate::ui::editor;

/// Runs `work` on a thread of its own and brings back what it returns. iced's own threads are
/// for quick work; a host that doesn't answer would hold one for as long as ssh waits.
pub fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Task<Result<T, String>> {
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    Task::perform(receiver, |answer| {
        answer.unwrap_or_else(|_| Err("the work was cut short".to_string()))
    })
}

impl App {
    /// Opens `remote` in this window (`open_here`): it is fetched first, and `Fetched` brings it
    /// to `show_fetched`. Until then the window stays on the document it shows, and the status
    /// bar says what is on its way.
    pub fn open_remote(&mut self, remote: Remote) -> Task<Message> {
        if open_elsewhere(&self.paths.data_dir, &remote.as_path()) {
            self.set_notice(Some(OPEN_ELSEWHERE.to_string()));
            return Task::none();
        }
        self.fetches += 1;
        self.fetching = Some(format!("Opening {remote}…"));
        let fetch = self.fetches;
        let ssh = Ssh::of(&self.paths);
        let data_dir = self.paths.data_dir.clone();
        off_thread(move || remote_file::fetch(&ssh, &data_dir, &remote))
            .map(move |fetched| Message::Fetched(fetch, fetched))
    }

    /// Forgets a file on a host still on its way: the window has been pointed somewhere else,
    /// and what arrives is dropped.
    pub fn call_off_fetch(&mut self) {
        self.fetches += 1;
        self.fetching = None;
    }

    /// A file arrived from its host, or didn't: shows it in the window's place, once the
    /// document shown is saved (`leave_for`), or says why it couldn't be opened.
    pub fn show_fetched(&mut self, fetched: Result<Fetched, String>) -> Task<Message> {
        self.fetching = None;
        match fetched {
            Ok(fetched) => self.leave_for(Opening::Fetched(fetched)),
            Err(reason) => {
                self.set_notice(Some(reason));
                Task::none()
            }
        }
    }

    /// Reload, for a file that changed on its host: reads it again; `Reloaded` brings it back.
    pub fn reload_remote(&mut self, remote: Remote) -> Task<Message> {
        self.set_notice(Some(format!("Reading {remote} again…")));
        let ssh = Ssh::of(&self.paths);
        let document = self.document;
        off_thread(move || remote_file::read(&ssh, &remote))
            .map(move |fetched| Message::Reloaded(document, fetched))
    }

    /// Puts the host's file, read again, in place of this window's text, as the file a save may
    /// now replace.
    pub fn reloaded(&mut self, fetched: Result<Fetched, String>) {
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(reason) => return self.set_notice(Some(format!("Couldn't reload: {reason}"))),
        };
        if let Doc::Remote {
            line_ending, bom, ..
        } = &mut self.doc
        {
            *line_ending = fetched.line_ending;
            *bom = fetched.bom;
            remote_file::begin(&self.paths.data_dir, &fetched);
            self.set_notice(None);
            self.take_reloaded(&fetched.text);
        }
    }

    /// Keep mine, for a file that changed on its host: the next save replaces whatever is
    /// there, and is made now.
    pub fn keep_mine_on_host(&mut self, remote: &Remote) -> Task<Message> {
        remote_file::keep_mine(&self.paths.data_dir, remote);
        self.disk_conflict = false;
        let focus = focus(editor::EDITOR_ID);
        // One save at a time: a running one is followed by the autosave.
        if self.autosaver.is_saving() {
            return focus;
        }
        Task::batch([self.save(Instant::now()), focus])
    }
}

// The stand-in for ssh is a shell script, so these run where there is an `sh`: not on Windows.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::core::paths::Paths;
    use crate::core::remote::tests::fake_ssh;
    use crate::core::remote_file::CHANGED;
    use crate::core::save::SaveState;
    use crate::ui::text_editor::{Action, Edit};
    use std::fs;
    use std::path::PathBuf;

    /// A scratch window, and a "host" (a folder here, reached through the stand-in ssh) holding
    /// `notes.md`.
    struct Desk {
        _dir: tempfile::TempDir,
        root: PathBuf,
        ssh: Ssh,
        paths: Paths,
        app: App,
        remote: Remote,
    }

    impl Desk {
        fn new() -> Desk {
            let dir = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(dir.path()).unwrap();
            fs::create_dir(root.join("host")).unwrap();
            fs::write(root.join("host/notes.md"), "# notes\r\n").unwrap();
            let paths = Paths::under(&root);
            let (app, _) = App::with_paths(Doc::Scratch, paths.clone());
            Desk {
                ssh: fake_ssh(&root),
                remote: Remote::parse(&format!("web1:{}/host/notes.md", root.display())).unwrap(),
                paths,
                app,
                root,
                _dir: dir,
            }
        }

        fn on_host(&self) -> String {
            fs::read_to_string(&self.remote.path).unwrap()
        }

        /// How the connection fails from now on (see `fake_ssh`); empty for a good one.
        fn connection(&self, mode: &str) {
            fs::write(self.root.join("mode"), mode).unwrap();
        }

        /// Ctrl+O on the host's file, and what `open_remote`'s thread does: the file arrives.
        fn open(&mut self) {
            let _ = self.app.open_file(self.remote.as_path());
            let fetched = remote_file::fetch(&self.ssh, &self.paths.data_dir, &self.remote);
            let _ = self.app.update(Message::Fetched(self.app.fetches, fetched));
        }

        fn type_text(&mut self, typed: &str) {
            for letter in typed.chars() {
                let _ = (self.app).update(Message::Edit(Action::Edit(Edit::Insert(letter))));
            }
        }

        /// What `App::save`'s thread does: sends the window's text and brings the answer back.
        fn send(&mut self) {
            let Doc::Remote {
                remote,
                line_ending,
                bom,
            } = self.app.doc.clone()
            else {
                panic!("the window isn't on a file on a host");
            };
            let generation = self.app.autosaver.begin(Instant::now());
            let saved = remote_file::save(
                &self.ssh,
                &self.paths.data_dir,
                &remote,
                self.app.pane.model.text(),
                line_ending,
                bom,
                &self.app.pane.model.history(),
            );
            let saved = Message::Saved(self.app.document, generation, saved.map(|()| None));
            let _ = self.app.update(saved);
        }

        fn shows_host_file(&self) -> bool {
            matches!(&self.app.doc, Doc::Remote { remote, .. } if *remote == self.remote)
        }
    }

    #[test]
    fn a_file_on_a_host_opens_here_once_it_arrives_and_saves_back() {
        let mut desk = Desk::new();
        let _ = desk.app.open_file(desk.remote.as_path());
        assert!(matches!(desk.app.doc, Doc::Scratch), "until it arrives");
        assert!(desk.app.fetching.as_ref().unwrap().contains("web1:"));
        let asked = desk.app.fetches;
        let fetched = remote_file::fetch(&desk.ssh, &desk.paths.data_dir, &desk.remote);
        let _ = desk.app.update(Message::Fetched(asked, fetched.clone()));
        assert!(desk.shows_host_file() && desk.app.fetching.is_none());
        assert_eq!(desk.app.pane.model.text(), "# notes\n");
        assert!(desk.app.editable() && !desk.app.autosaver.pending());
        assert_eq!(desk.app.recent, [desk.remote.as_path()]);
        assert!(open_elsewhere(&desk.paths.data_dir, &desk.remote.as_path()));

        desk.type_text("My ");
        assert!(desk.app.autosaver.pending());
        desk.send();
        assert_eq!(desk.app.autosaver.state(), SaveState::Saved);
        assert_eq!(
            desk.on_host(),
            "My # notes\r\n",
            "its line endings are kept"
        );
        assert!(remote_file::unsent(&desk.paths.data_dir).is_empty());

        // Launched on the file (`alias:/path` on the command line), the window starts on
        // scratch and asks for it.
        let named = Doc::recent(&desk.remote.as_path());
        let (launched, _) = App::with_paths(named, Paths::under(&desk.root.join("other")));
        assert!(matches!(launched.doc, Doc::Scratch) && launched.fetching.is_some());

        // The same answer coming a second time (its request is long over) changes nothing.
        desk.type_text("x");
        let _ = desk.app.update(Message::Fetched(asked, fetched));
        assert_eq!(desk.app.pane.model.text(), "My x# notes\n");
    }

    #[test]
    fn a_file_that_arrives_after_the_window_moved_on_is_dropped() {
        let mut desk = Desk::new();
        let _ = desk.app.open_file(desk.remote.as_path());
        let asked = desk.app.fetches;
        let fetched = remote_file::fetch(&desk.ssh, &desk.paths.data_dir, &desk.remote);
        // Ctrl+0 meanwhile: back to scratch, which is what the window shows.
        let _ = desk.app.update(Message::OpenScratch);
        assert!(desk.app.fetching.is_none());
        let _ = desk.app.update(Message::Fetched(asked, fetched));
        assert!(matches!(desk.app.doc, Doc::Scratch));

        // One that can't be fetched leaves the window as it is, and says why.
        desk.connection("down");
        desk.open();
        assert!(matches!(desk.app.doc, Doc::Scratch));
        let (words, _) = desk.app.notice.clone().unwrap();
        assert_eq!(words, "Couldn't open: can't reach web1");
    }

    #[test]
    fn closing_with_the_host_away_waits_once_then_leaves_the_text_here() {
        let mut desk = Desk::new();
        desk.open();
        desk.type_text("Mine. ");
        desk.connection("down");
        let _ = desk.app.update(Message::CloseRequested);
        assert!(desk.app.autosaver.is_saving(), "the close sends it first");
        desk.send();
        assert!(matches!(desk.app.autosaver.state(), SaveState::Failed(_)));
        assert!(desk.app.leave_unsent && desk.app._doc_lock.is_some());
        let (words, _) = desk.app.notice.clone().unwrap();
        assert_eq!(words, "Kept on this computer: close again to send it later");
        // Typed after that, and closed again: the window goes, with all of it kept. The
        // autosave trying the host again meanwhile changes nothing.
        desk.type_text("More. ");
        desk.send();
        assert!(desk.app.leave_unsent);
        let _ = desk.app.update(Message::CloseRequested);
        assert!(desk.app._doc_lock.is_none(), "closed");
        assert_eq!(
            remote_file::unsent(&desk.paths.data_dir),
            [desk.remote.clone()]
        );
        assert_eq!(desk.on_host(), "# notes\r\n");

        // The next launch says so; opening the file brings the text and its undo steps back,
        // and sends it.
        let (app, _) = App::with_paths(Doc::Scratch, desk.paths.clone());
        desk.app = app;
        let (words, _) = desk.app.notice.clone().unwrap();
        assert!(words.contains("never reached it"), "{words}");
        desk.open();
        assert_eq!(desk.app.pane.model.text(), "Mine. More. # notes\n");
        assert!(desk.app.autosaver.pending(), "still to be sent");
        let _ = desk.app.update(Message::Undo);
        assert_eq!(desk.app.pane.model.text(), "Mine. # notes\n");
        let _ = desk.app.update(Message::Redo);
        desk.connection("");
        desk.send();
        assert_eq!(desk.on_host(), "Mine. More. # notes\r\n");
        assert!(remote_file::unsent(&desk.paths.data_dir).is_empty());
        let _ = desk.app.update(Message::CloseRequested);
        assert!(desk.app._doc_lock.is_none());
    }

    #[test]
    fn leaving_for_another_file_sends_this_one_first() {
        let mut desk = Desk::new();
        let here = desk.root.join("here.txt");
        fs::write(&here, "here\n").unwrap();
        desk.open();
        desk.type_text("a");
        let _ = desk.app.open_file(here.clone());
        assert!(desk.shows_host_file() && desk.app.autosaver.is_saving());
        desk.send();
        assert!(matches!(&desk.app.doc, Doc::File { path, .. } if *path == here));
        assert_eq!(desk.on_host(), "a# notes\r\n");

        // With the host away the window stays, once; asked again, it goes and the text waits
        // on this computer.
        desk.open();
        desk.type_text("b");
        desk.connection("down");
        let _ = desk.app.open_file(here.clone());
        desk.send();
        assert!(desk.shows_host_file() && desk.app.leave_unsent);
        let _ = desk.app.open_file(here.clone());
        assert!(matches!(&desk.app.doc, Doc::File { path, .. } if *path == here));
        assert_eq!(
            remote_file::unsent(&desk.paths.data_dir),
            [desk.remote.clone()]
        );
        assert_eq!(desk.on_host(), "a# notes\r\n");
    }

    #[test]
    fn a_file_changed_on_its_host_waits_for_reload_or_keep_mine() {
        let mut desk = Desk::new();
        desk.open();
        desk.type_text("Mine. ");
        fs::write(&desk.remote.path, "theirs\n").unwrap();
        desk.send();
        assert!(desk.app.disk_conflict);
        assert_eq!(
            desk.app.autosaver.state(),
            SaveState::Failed(CHANGED.to_string())
        );
        assert_eq!(desk.on_host(), "theirs\n", "not written over");
        // Nor by closing, or by leaving for another document.
        let _ = desk.app.update(Message::CloseRequested);
        desk.send();
        let _ = desk.app.update(Message::OpenScratch);
        assert!(desk.shows_host_file() && !desk.app.autosaver.is_saving());
        assert_eq!(desk.on_host(), "theirs\n");
        assert!(!desk.app.leave_unsent && desk.app._doc_lock.is_some());

        // Keep mine: the next save replaces what is there.
        let _ = desk.app.update(Message::KeepMine);
        assert!(!desk.app.disk_conflict && desk.app.autosaver.is_saving());
        desk.send();
        assert_eq!(desk.app.autosaver.state(), SaveState::Saved);
        assert_eq!(desk.on_host(), "Mine. # notes\r\n");

        // Reload: the host's file takes this text's place, as one undo step.
        desk.type_text("x");
        fs::write(&desk.remote.path, "theirs again\n").unwrap();
        desk.send();
        assert!(desk.app.disk_conflict);
        let _ = desk.app.update(Message::ReloadFromDisk);
        let read = remote_file::read(&desk.ssh, &desk.remote);
        let _ = desk.app.update(Message::Reloaded(desk.app.document, read));
        assert_eq!(desk.app.pane.model.text(), "theirs again\n");
        assert!(!desk.app.disk_conflict && !desk.app.autosaver.pending());
        let _ = desk.app.update(Message::Undo);
        assert_eq!(desk.app.pane.model.text(), "Mine. x# notes\n");
        desk.send();
        assert_eq!(
            desk.on_host(),
            "Mine. x# notes\n",
            "in the host file's line endings"
        );
    }
}
