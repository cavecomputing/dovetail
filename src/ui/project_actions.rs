//! The project half of `App::update`: Rewrite (a plain document becomes a project), showing an
//! older version in the left pane, and the Commit dialog that turns the draft into a new version.

use std::time::Instant;

use iced::Task;

use crate::core::counts::Counts;
use crate::core::lock::{claim_lock, file_lock_path};
use crate::core::opened::Doc;
use crate::core::paths::expand_home;
use crate::core::recent::remember;
use crate::core::save::failure_reason;
use crate::core::scratch::clear_scratch;
use crate::core::text_file::save_text;
use crate::project::{Project, create_project};
use crate::ui::app::{App, Message};
use crate::ui::editor::{self, Pane};
use crate::ui::project_view::{COMMIT_NOTE_ID, ProjectView};

impl App {
    /// Handles the project messages; `App::update` hands every one of them here.
    pub fn update_project(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Rewrite => {
                let plain = !matches!(self.doc, Doc::Project(_));
                // Not mid-write either, so a scratch save can't land after scratch is cleared.
                if plain && self.editable() && !self.busy && !self.autosaver.is_saving() {
                    self.busy = true;
                    let text = self.pane.model.text().to_string();
                    let projects_dir = match &self.settings.projects_folder {
                        Some(folder) => expand_home(folder),
                        None => self.paths.projects_dir.clone(),
                    };
                    return Task::perform(
                        async move {
                            create_project(&projects_dir, &text)
                                .map(|project| (project, text))
                                .map_err(|error| error.to_string())
                        },
                        Message::ProjectCreated,
                    );
                }
            }
            Message::ProjectCreated(Ok((project, text))) => {
                self.busy = false;
                self.become_project(project, &text);
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }
            Message::ProjectCreated(Err(error)) => {
                self.busy = false;
                self.notice = Some(format!("Couldn't start the rewrite: {error}"));
            }
            Message::ShowVersion(number) => {
                self.menu_open = false;
                if let Some(project) = &self.project {
                    return load_version(&project.project, number);
                }
            }
            Message::VersionLoaded(number, Ok(text)) => {
                if let Some(project) = &mut self.project {
                    project.show_version(number, &text);
                }
            }
            Message::VersionLoaded(number, Err(error)) => {
                self.notice = Some(format!("Couldn't read v{number}: {error}"));
            }
            Message::OpenCommit => {
                if self.project.is_some() && self.editable() && !self.busy {
                    self.menu_open = false;
                    self.commit_note = Some(String::new());
                    return iced::widget::operation::focus(COMMIT_NOTE_ID);
                }
            }
            Message::CommitNote(note) => {
                if self.commit_note.is_some() {
                    self.commit_note = Some(note);
                }
            }
            Message::Commit => {
                // Not mid-autosave either: both write `draft.txt`, and git must get this text.
                if let (Some(project), Some(note), false, false) = (
                    &self.project,
                    &self.commit_note,
                    self.busy,
                    self.autosaver.is_saving(),
                ) {
                    self.busy = true;
                    let mut project = project.project.clone();
                    let note = note.clone();
                    let draft = self.pane.model.text().to_string();
                    return Task::perform(
                        async move {
                            project
                                .commit(&draft, &note)
                                .map(|_| (project, draft))
                                .map_err(|error| error.to_string())
                        },
                        Message::Committed,
                    );
                }
            }
            Message::Committed(Ok((committed, draft))) => {
                self.busy = false;
                self.commit_note = None;
                self.notice = None;
                if let Some(project) = &mut self.project {
                    project.project = committed;
                    project.show_version(project.project.latest_version(), &draft);
                }
                // The commit wrote `draft.txt`; anything typed since still needs saving.
                if self.pane.model.text() == draft {
                    self.autosaver.mark_saved();
                } else {
                    self.autosaver.edited(Instant::now());
                }
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }
            Message::Committed(Err(error)) => {
                self.busy = false;
                self.notice = Some(format!("Commit failed: {error}"));
            }
            _ => {}
        }
        Task::none()
    }

    /// After a Rewrite: the text so far slides left as v1 (read-only), a copy becomes the draft
    /// on the right, and this window now shows `project`. Scratch is cleared, keeping its text for
    /// undo; a `.txt` is left as it was (with any pending typing saved first).
    fn become_project(&mut self, project: Project, text: &str) {
        let data_dir = &self.paths.data_dir;
        let problem = match &self.doc {
            Doc::Scratch => clear_scratch(data_dir, self.pane.model.text()).err(),
            Doc::File { path, line_ending } if self.autosaver.pending() => {
                let saved = save_text(path, self.pane.model.text(), *line_ending, self.disk_stamp);
                saved.err()
            }
            _ => None,
        };
        self.notice = problem.map(|error| failure_reason(&error));
        let manifest = project.manifest_path();
        let _ = remember(data_dir, &manifest);
        self._doc_lock = claim_lock(&file_lock_path(data_dir, &manifest)).ok();
        self.doc = Doc::Project(manifest);
        self.disk_stamp = None;
        self.disk_conflict = false;

        let mut draft = Pane::new(text);
        if self.pane.model.text() == text {
            draft.content.move_to(self.pane.content.cursor());
            draft
                .content
                .scroll_to_line(self.pane.content.scroll_line());
        }
        let reference = std::mem::replace(&mut self.pane, draft);
        let mut view = ProjectView::new(project, reference);
        view.slide_started = Some(Instant::now());
        self.project = Some(view);
        self.autosaver.mark_saved();
        self.saving_since = None;
        self.saved_at = Some(Instant::now());
        self.counts = Counts::of(text);
    }
}

/// Reads version `number`'s text from git in the background; `VersionLoaded` brings it back.
pub fn load_version(project: &Project, number: u32) -> Task<Message> {
    let project = project.clone();
    Task::perform(
        async move {
            project
                .version_text(number)
                .map_err(|error| error.to_string())
        },
        move |text| Message::VersionLoaded(number, text),
    )
}
