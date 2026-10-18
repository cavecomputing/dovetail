//! The App: its state, the Message enum, `update` (the only place state changes) and `view`.
//! One process shows one document in one window; Settings opens over it.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use iced::widget::{column, container, responsive, space, stack};
use iced::window::{self, Direction};
use iced::{Element, Size, Subscription, Task, Theme, event, keyboard};

use crate::brand;
use crate::core::counts::{Counts, load_tokenizer_in_background, word_count};
use crate::core::opened::{Doc, open_doc};
use crate::core::paths::Paths;
use crate::core::save::{Autosaver, SaveState, atomic_write, failure_reason};
use crate::core::scratch::{ScratchState, ScratchView, scratch_open_elsewhere};
use crate::core::settings::{ReadingMode, Settings, Theme as ThemeSetting};
use crate::core::text_file::{DiskStamp, changed_on_disk, read_text, save_text};
use crate::project::{DRAFT_FILE, Project, RecentItem, open_project, recent_items, save_draft};
use crate::ui::assistant_actions::{Assistant, AssistantAction, CardState};
use crate::ui::editor::{Decision, Pane};
use crate::ui::find_bar::{self, Find, FindAction};
use crate::ui::idea_actions::IdeaAction;
use crate::ui::project_actions::load_version;
use crate::ui::project_view::{self, ProjectView};
use crate::ui::settings_panel::{self, Change, Connection, KeyField};
use crate::ui::switcher::{self, Switcher};
use crate::ui::text_editor::{Action, Cursor, Position};
use crate::ui::theme::{self, Roles};
use crate::ui::{
    assistant_view, commit_dialog, editor, idea_panel, ideas_view, key_guard, status_bar,
    weights_view, window_controls,
};

pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    pub settings_error: Option<String>,
    settings_modified: Option<SystemTime>,
    pub last_comfort_chars: u16,
    system_is_dark: bool,
    pub theme: Theme,

    main_window: window::Id,

    pub doc: Doc,
    /// The text being edited: the whole window, or a project's draft (the right pane).
    pub pane: Pane,
    /// The project this window shows, with its left pane; `None` for scratch and plain files.
    pub project: Option<ProjectView>,
    /// The Commit dialog's note, while the dialog is open.
    pub commit_note: Option<String>,
    /// The Ctrl+P switcher, while it is open.
    pub switcher: Option<Switcher>,
    /// Whether the Settings panel is open over the document.
    pub settings_open: bool,
    /// True while git is creating or committing a project, so it isn't started twice.
    pub busy: bool,
    /// Whether the view opens at a saved top line, rather than around the cursor.
    scroll_restored: bool,
    /// Why the text must not be edited (a project, or open in another window); `None` when it
    /// can be.
    pub read_only_reason: Option<String>,
    /// Keeps other windows off this document until the process ends.
    pub _doc_lock: Option<File>,
    /// The file on disk as this window last read or wrote it.
    pub disk_stamp: Option<DiskStamp>,
    /// A save was refused because the file changed on disk; the status bar offers a way out.
    pub disk_conflict: bool,
    /// A short-lived message for the status bar, such as why a new window didn't open.
    pub notice: Option<String>,
    pub autosaver: Autosaver,
    pub saving_since: Option<Instant>,
    /// When a save last finished, for the draft's "autosaved just now".
    pub saved_at: Option<Instant>,
    pub now: Instant,
    closing: bool,

    /// The draft's word and token counts.
    pub counts: Counts,
    /// The recent files and projects, for the menu.
    pub recent: Vec<RecentItem>,
    pub menu_open: bool,
    /// The Ctrl+F find bar.
    pub find: Find,

    /// The assistant pane and its conversation, kept while the window is open.
    pub assistant: Assistant,
    /// The Settings panel's API key field, and how its last connection test went.
    pub key_field: KeyField,
    pub connection: Option<Connection>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(Action),
    /// Scrolling, selecting or copying in a project's left pane.
    ReferenceEdit(Action),
    Unindent,
    Undo,
    Redo,
    /// Ctrl+X or Ctrl+C in the editor with this id: keep the selection's paint for a paste.
    Clip(&'static str),
    /// Accept or reject suggested edits (their buttons in the text, Tab or Esc).
    Decide(Decision),
    Tick(Instant),
    Saved(u64, Result<Option<DiskStamp>, String>),
    TokenizerReady,

    WindowOpened,
    CursorToMiddle(Size),
    CloseRequested,
    WindowClosed,
    Drag,
    Resize(Direction),
    Minimize,
    ToggleMaximize,

    ToggleMenu,
    CloseMenu,
    /// Enter outside any text box: commits from the Commit dialog, or steps through the find
    /// bar's matches (back with Shift).
    EnterKey {
        shift: bool,
    },
    NewFile,
    NewFilePicked(PathBuf),
    OpenDialog,
    Save,
    SaveAs,
    SaveAsPicked(PathBuf),
    OpenPath(PathBuf),
    OpenSettings,
    OpenScratch,
    OpenSwitcher,
    SwitcherQuery(String),
    SwitcherMove(i32),
    SwitcherOpen,

    Rewrite,
    ProjectCreated(Result<(Project, String), String>),
    ShowVersion(u32),
    VersionLoaded(u32, Result<String, String>),
    OpenCommit,
    CommitNote(String),
    Commit,
    Committed(Result<(Project, String), String>),
    ReloadFromDisk,
    KeepMine,

    Setting(Change),
    /// Ctrl+= and Ctrl+-: the font a size bigger or smaller.
    Zoom(f32),
    PollSettings,
    SystemTheme(iced::theme::Mode),

    Idea(IdeaAction),
    Assistant(AssistantAction),
    Find(FindAction),
}

/// Runs the app on `doc` until its window closes.
pub fn run(doc: Doc) -> iced::Result {
    iced::daemon(move || App::new(doc.clone()), App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .font(theme::FONT_REGULAR)
        .font(theme::FONT_BOLD)
        .font(theme::FONT_ITALIC)
        .default_font(theme::MONO)
        .run()
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(960.0, 720.0),
        min_size: Some(Size::new(360.0, 240.0)),
        decorations: false,
        exit_on_close_request: false,
        // X11 shows this in task bars; Wayland finds the icon through the .desktop file instead.
        icon: window::icon::from_rgba(APP_ICON_RGBA.to_vec(), 64, 64).ok(),
        platform_specific: window::settings::PlatformSpecific {
            application_id: brand::APP_ID.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The app icon at 64x64, as raw RGBA bytes (made from assets/icon/icon.svg, see its README).
const APP_ICON_RGBA: &[u8] = include_bytes!("../../assets/icon/icon-64.rgba");

impl App {
    fn new(doc: Doc) -> (App, Task<Message>) {
        let paths = Paths::detect();
        let (settings, settings_error) = Settings::load(&paths.settings_file());
        let opened = open_doc(doc, &paths);
        // Scratch cleared by a Rewrite opens empty, with Ctrl+Z bringing the old text back.
        let mut pane = Pane::new(opened.cleared_scratch.as_deref().unwrap_or(&opened.text));
        if let Some(cleared) = &opened.cleared_scratch {
            pane.replace(0..cleared.len(), &opened.text);
        }
        if let Some((line, column)) = opened.cursor {
            pane.content.move_to(Cursor {
                position: Position { line, column },
                selection: None,
            });
        }
        if let Some(line) = opened.scroll_line {
            pane.content.scroll_to_line(line);
        }
        // A project shows its draft at once; the newest version for the left pane comes from git
        // in the background.
        let mut read_only_reason = opened.read_only_reason;
        let mut project = None;
        let mut load_reference = Task::none();
        if let Doc::Project(path) = &opened.doc {
            match open_project(path) {
                Ok(found) => {
                    pane.model.paint = found.draft_paint(pane.model.text());
                    load_reference = load_version(&found, found.latest_version());
                    project = Some(ProjectView::new(found, Pane::new("")));
                }
                Err(error) => read_only_reason = Some(format!("Couldn't open project: {error}")),
            }
        }

        let (main_window, open) = window::open(window_settings());
        let (tokenizer_sender, tokenizer_ready) = iced::futures::channel::oneshot::channel();
        load_tokenizer_in_background(move || {
            let _ = tokenizer_sender.send(());
        });

        let mut app = App {
            settings_modified: modified_time(&paths),
            recent: recent_items(&paths.data_dir),
            counts: Counts {
                words: word_count(&opened.text),
                tokens: None,
            },
            last_comfort_chars: match settings.reading_mode {
                ReadingMode::Comfort { chars } => chars,
                ReadingMode::FullWidth => 80,
            },
            autosaver: Autosaver::new(Duration::from_millis(settings.autosave_delay_ms)),
            theme: theme::gruvbox(settings.theme != ThemeSetting::Light),
            system_is_dark: true,
            paths,
            settings,
            settings_error,
            main_window,
            doc: opened.doc,
            pane,
            project,
            commit_note: None,
            switcher: None,
            settings_open: false,
            busy: false,
            scroll_restored: opened.scroll_line.is_some(),
            read_only_reason,
            _doc_lock: opened.lock,
            disk_stamp: opened.disk_stamp,
            disk_conflict: false,
            notice: None,
            saving_since: None,
            saved_at: None,
            now: Instant::now(),
            closing: false,
            menu_open: false,
            find: Find::default(),
            assistant: Assistant::default(),
            key_field: KeyField::default(),
            connection: None,
        };
        app.refresh_ideas();
        let tasks = Task::batch([
            open.map(|_| Message::WindowOpened),
            Task::perform(tokenizer_ready, |_| Message::TokenizerReady),
            iced::system::theme().map(Message::SystemTheme),
            load_reference,
        ]);
        (app, tasks)
    }

    fn title(&self, _window: window::Id) -> String {
        status_bar::title(&self.doc)
    }

    fn theme(&self, _window: window::Id) -> Theme {
        self.theme.clone()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // A read-only window still scrolls, selects and copies, but never changes the text.
            Message::Edit(action) if action.is_edit() && !self.editable() => {}
            Message::Edit(action) => {
                // Selecting in one pane drops the other's selection, so nothing ever acts on a
                // selection that isn't shown.
                if let Some(project) = &mut self.project
                    && selects(&action)
                {
                    project.reference.deselect();
                }
                if self.pane.perform(action) {
                    self.text_changed();
                }
            }
            Message::ReferenceEdit(action) => {
                if let Some(project) = &mut self.project
                    && !action.is_edit()
                {
                    if selects(&action) {
                        self.pane.deselect();
                        project.mouse_selecting = matches!(
                            action,
                            Action::Drag(_) | Action::SelectWord | Action::SelectLine
                        );
                    }
                    project.reference.perform(action);
                }
            }
            Message::Clip(id) => {
                let source = match &self.project {
                    Some(project) if id == editor::REFERENCE_ID => &project.reference,
                    _ => &self.pane,
                };
                let clip = source.copied();
                // While reviewing, the widget's text has the suggestions in it, so the clipboard
                // gets the text from here instead.
                let copy = match (&clip, source.reviewing()) {
                    (Some((text, _)), true) => iced::clipboard::write(text.clone()),
                    _ => Task::none(),
                };
                self.pane.model.clip = clip;
                return copy;
            }
            Message::Unindent => {
                let tab_spaces = usize::from(self.settings.tab_spaces);
                if self.editable() && self.pane.unindent(tab_spaces) {
                    self.text_changed();
                }
            }
            // A step may change only the paint, which still needs saving and drawing.
            Message::Undo | Message::Redo if self.editable() => {
                let changed = match message {
                    Message::Undo => self.pane.undo(),
                    _ => self.pane.redo(),
                };
                if changed {
                    self.text_changed();
                } else if self.project.is_some() {
                    self.ideas_changed();
                }
            }
            Message::Undo | Message::Redo => {}
            // Esc in the editor rejects the suggestion at the caret, except while the find bar
            // is open: then that Esc only closes the bar (`CloseMenu`).
            Message::Decide(Decision::Reject(None)) if self.find.open => {}
            Message::Decide(decision) => self.decide(decision),
            Message::Tick(now) => {
                self.now = now;
                if let Some(project) = &mut self.project
                    && project.slide_done(now)
                {
                    project.slide_started = None;
                }
                self.assistant.finish_slide(now);
                // Not while git is busy: a commit writes `draft.txt` itself.
                if self.autosaver.should_save_now(now) && !self.busy {
                    return self.save(now);
                }
            }
            Message::Saved(generation, result) => {
                let failed = result.is_err();
                if let Ok(Some(stamp)) = result {
                    self.disk_stamp = Some(stamp);
                }
                self.autosaver.finish(generation, result.map(|_| ()));
                if self.autosaver.state() == SaveState::Saved {
                    self.saving_since = None;
                    self.saved_at = Some(Instant::now());
                }
                self.disk_conflict = match &self.doc {
                    Doc::File { path, .. } if failed => changed_on_disk(path, self.disk_stamp),
                    _ => false,
                };
                if self.closing && failed {
                    // Stay open; the status bar shows why the save failed.
                    self.closing = false;
                } else if self.closing {
                    return self.continue_closing();
                }
            }
            Message::TokenizerReady => {
                self.counts = Counts::of(self.pane.model.text());
                if let Some(project) = &mut self.project {
                    project.reference_counts = Counts::of(project.reference.model.text());
                }
                self.refresh_ideas();
            }

            Message::WindowOpened => {
                let focus = iced::widget::operation::focus(editor::EDITOR_ID);
                if self.scroll_restored {
                    return focus;
                }
                let size = window::size(self.main_window).map(Message::CursorToMiddle);
                return Task::batch([focus, size]);
            }
            Message::CursorToMiddle(size) => return self.scroll_cursor_to_middle(size),
            Message::CloseRequested => {
                self.closing = true;
                // A reply on its way is cancelled; it never holds the window open.
                self.assistant.stop();
                if matches!(self.doc, Doc::Scratch) && self.editable() {
                    let _ = self.scratch_view().save(&self.paths.data_dir);
                }
                return self.continue_closing();
            }
            Message::WindowClosed => return iced::exit(),
            Message::Drag => return window::drag(self.main_window),
            Message::Resize(direction) => return window::drag_resize(self.main_window, direction),
            Message::Minimize => return window::minimize(self.main_window, true),
            Message::ToggleMaximize => return window::toggle_maximize(self.main_window),

            Message::ToggleMenu => {
                self.menu_open = !self.menu_open;
                self.recent = recent_items(&self.paths.data_dir);
            }
            // Esc, or a click outside: closes the menu, the switcher, the Commit dialog, Settings,
            // the idea panel and the weights view. With none of them open, Esc closes the find bar,
            // and after that puts the brush down.
            Message::CloseMenu => {
                let mut closed = self.menu_open || self.switcher.is_some();
                closed |= self.commit_note.is_some();
                closed |= std::mem::take(&mut self.settings_open);
                self.menu_open = false;
                self.switcher = None;
                self.commit_note = None;
                if let Some(project) = &mut self.project {
                    closed |= project.idea_panel.take().is_some();
                    closed |= std::mem::take(&mut project.weights_open);
                    project.commit_check = None;
                }
                // The find bar sits under all of those, so it closes on an Esc of its own.
                if !closed {
                    closed = std::mem::take(&mut self.find.open);
                }
                if !closed {
                    self.pane.model.brush = None;
                }
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }
            Message::EnterKey { shift } => {
                if self.find.open && self.commit_note.is_none() {
                    let step = if shift { -1 } else { 1 };
                    return self.update_find(FindAction::Step(step));
                }
                return self.update_project(Message::Commit);
            }
            Message::NewFile => return self.new_file(),
            Message::NewFilePicked(path) => return self.start_new_file(path),
            Message::Save => return self.save_now(),
            Message::SaveAs => return self.pick_save_path(),
            Message::SaveAsPicked(path) => return self.save_as(path),
            Message::OpenDialog => {
                self.menu_open = false;
                let dialog = rfd::AsyncFileDialog::new()
                    .add_filter("Text and projects", &["txt", brand::PROJECT_EXT])
                    .add_filter("All files", &["*"])
                    .pick_file();
                // Cancelling leaves everything as it was.
                return Task::perform(dialog, |picked| picked)
                    .and_then(|file| Task::done(Message::OpenPath(file.path().to_path_buf())));
            }
            Message::OpenPath(path) => {
                self.menu_open = false;
                self.switcher = None;
                self.notice = open_in_new_window(Some(path)).err();
            }
            Message::OpenSettings => {
                self.menu_open = false;
                if std::mem::replace(&mut self.settings_open, true) {
                    return Task::none();
                }
                // Nothing behind the panel keeps the keyboard, so typing reaches only its fields.
                let unfocus = iced::advanced::widget::operate(
                    iced::advanced::widget::operation::focusable::unfocus(),
                );
                return Task::batch([unfocus, self.load_key()]);
            }
            Message::OpenScratch => {
                self.menu_open = false;
                if matches!(self.doc, Doc::Scratch) {
                    return Task::none();
                }
                self.notice = if scratch_open_elsewhere(&self.paths.data_dir) {
                    Some("Scratch is already open in another window".to_string())
                } else {
                    open_in_new_window(None).err()
                };
            }
            Message::OpenSwitcher
            | Message::SwitcherQuery(_)
            | Message::SwitcherMove(_)
            | Message::SwitcherOpen => return self.update_switcher(message),
            Message::Rewrite
            | Message::ProjectCreated(_)
            | Message::ShowVersion(_)
            | Message::VersionLoaded(..)
            | Message::OpenCommit
            | Message::CommitNote(_)
            | Message::Commit
            | Message::Committed(_) => return self.update_project(message),
            Message::ReloadFromDisk => {
                if let Doc::File { path, line_ending } = &mut self.doc {
                    match read_text(path) {
                        Ok(file) => {
                            *line_ending = file.line_ending;
                            self.disk_stamp = DiskStamp::of(path);
                            // One undo step, so Ctrl+Z brings this window's text back.
                            let length = self.pane.model.text().len();
                            self.pane.replace(0..length, &file.text);
                            self.text_changed();
                            self.autosaver.mark_saved();
                            self.saving_since = None;
                            self.disk_conflict = false;
                        }
                        Err(error) => self.notice = Some(format!("Couldn't reload: {error}")),
                    }
                }
            }
            Message::KeepMine => {
                if let Doc::File { path, .. } = &self.doc {
                    // Take the file on disk as seen, so the next save writes over it.
                    self.disk_stamp = DiskStamp::of(path);
                    self.disk_conflict = false;
                    return self.save(Instant::now());
                }
            }

            Message::Setting(change) => {
                if let Change::ReadingMode(ReadingMode::Comfort { chars }) = change {
                    self.last_comfort_chars = chars;
                }
                settings_panel::apply(change, &mut self.settings);
                // A test result is about the settings it ran with.
                self.connection = None;
                self.save_settings();
                self.settings_changed();
            }
            Message::Zoom(step) => {
                let size = self.settings.font_size + step;
                return self.update(Message::Setting(Change::FontSize(size)));
            }
            Message::PollSettings => {
                // Once a second: also keeps the draft's "autosaved 3 min ago" current.
                self.now = Instant::now();
                let modified = modified_time(&self.paths);
                if modified != self.settings_modified {
                    self.settings_modified = modified;
                    let (settings, warning) = Settings::load(&self.paths.settings_file());
                    self.settings = settings;
                    self.settings_error = warning;
                    self.settings_changed();
                }
            }
            Message::SystemTheme(mode) => {
                self.system_is_dark = mode != iced::theme::Mode::Light;
                self.settings_changed();
            }
            Message::Idea(action) => return self.update_ideas(action),
            Message::Assistant(action) => return self.update_assistant(action),
            Message::Find(action) => return self.update_find(action),
        }
        Task::none()
    }

    fn text_changed(&mut self) {
        let now = Instant::now();
        self.autosaver.edited(now);
        self.saving_since.get_or_insert(now);
        self.counts = Counts::of(self.pane.model.text());
        if self.find.open {
            self.find_again(self.pane.caret());
        }
        // Undo, redo and a reload can drop a suggestion the change touched.
        self.settle_cards(CardState::Gone(
            "an edit to the text under it dropped it".to_string(),
        ));
        self.refresh_ideas();
    }

    /// Accepts or rejects suggestions (from their buttons, Tab or Esc, a card, or New chat), and
    /// marks their cards.
    pub fn decide(&mut self, decision: Decision) {
        if !self.editable() {
            return;
        }
        let changed = self.pane.decide(decision);
        self.settle_cards(match decision {
            Decision::Accept(_) | Decision::AcceptAll => CardState::Accepted,
            Decision::Reject(_) | Decision::RejectAll => CardState::Rejected,
        });
        if changed {
            self.text_changed();
        } else {
            self.refresh_ideas();
        }
    }

    /// Writes the settings file, unless the user's copy has a mistake (the status bar says so).
    pub fn save_settings(&mut self) {
        self.settings_error = self
            .settings
            .save_unless_broken(&self.paths.settings_file())
            .err();
        self.settings_modified = modified_time(&self.paths);
    }

    /// Weighs the ideas and rebuilds the paint the editors draw (a project only).
    pub fn refresh_ideas(&mut self) {
        if let Some(project) = &mut self.project {
            project.refresh_ideas(&self.pane, Roles::of(&self.theme));
        }
    }

    fn settings_changed(&mut self) {
        self.autosaver
            .set_delay(Duration::from_millis(self.settings.autosave_delay_ms));
        let dark = match self.settings.theme {
            ThemeSetting::Dark => true,
            ThemeSetting::Light => false,
            ThemeSetting::System => self.system_is_dark,
        };
        self.theme = theme::gruvbox(dark);
        self.refresh_ideas();
    }

    /// Writes the current text in the background; `Saved` reports back.
    pub fn save(&mut self, now: Instant) -> Task<Message> {
        let generation = self.autosaver.begin(now);
        let text = self.pane.model.text().to_string();
        let data_dir = self.paths.data_dir.clone();
        let view = self.scratch_view();
        let doc = self.doc.clone();
        let disk_stamp = self.disk_stamp;
        // A project saves its paint and ideas too, in the manifest, after the text.
        let project = self.project.as_mut().map(|view| {
            view.project.set_draft_paint(&self.pane.model.paint, &text);
            (view.project.dir.clone(), view.project.manifest.clone())
        });
        Task::perform(
            async move {
                let result = match (doc, project) {
                    (Doc::Scratch, _) => ScratchState { text, view }.save(&data_dir).map(|_| None),
                    (Doc::File { path, line_ending }, _) => {
                        save_text(&path, &text, line_ending, disk_stamp)
                    }
                    (Doc::Project(_), Some((dir, manifest))) => {
                        return save_draft(&dir, &manifest, &text)
                            .map(|_| None)
                            .map_err(|error| match error.downcast_ref() {
                                Some(error) => failure_reason(error),
                                None => error.to_string(),
                            });
                    }
                    (Doc::Project(path), None) => {
                        atomic_write(&path.with_file_name(DRAFT_FILE), text.as_bytes())
                            .map(|_| None)
                    }
                };
                result.map_err(|error| failure_reason(&error))
            },
            move |result| Message::Saved(generation, result),
        )
    }

    /// Closes the window once the latest text is safely written. If the save failed, the
    /// window stays open and the status bar shows why.
    fn continue_closing(&mut self) -> Task<Message> {
        if !self.editable() || self.autosaver.close_allowed() {
            return window::close(self.main_window);
        }
        if self.autosaver.is_saving() {
            return Task::none();
        }
        self.save(Instant::now())
    }

    pub fn editable(&self) -> bool {
        self.read_only_reason.is_none()
    }

    fn scratch_view(&self) -> ScratchView {
        let position = self.pane.content.cursor().position;
        ScratchView {
            cursor_line: position.line,
            cursor_column: position.column,
            scroll_line: Some(self.pane.content.scroll_line()),
        }
    }

    /// iced puts a restored cursor at the bottom edge of the view; scroll it up to mid-view,
    /// using the window's real `size` (a tiling window manager may have resized it).
    fn scroll_cursor_to_middle(&self, size: Size) -> Task<Message> {
        let line_height = self.settings.font_size * editor::LINE_HEIGHT;
        let text_height =
            size.height - editor::PADDING.top - editor::PADDING.bottom - status_bar::HEIGHT;
        let visible_lines = (text_height / line_height).max(0.0) as usize;
        // Fewer lines than a screen above the cursor means the view hasn't scrolled at all.
        if self.pane.content.cursor().position.line < visible_lines {
            return Task::none();
        }
        let lines = (visible_lines / 2) as i32;
        Task::done(Message::Edit(Action::Scroll { lines }))
    }

    fn subscription(&self) -> Subscription<Message> {
        let keys = event::listen_with(shortcut);
        let mut subscriptions = vec![
            window::close_requests().map(|_| Message::CloseRequested),
            window::close_events().map(|_| Message::WindowClosed),
            match self.settings_open {
                true => keys.filter_map(settings_key),
                false => keys,
            },
            iced::time::every(Duration::from_secs(1)).map(|_| Message::PollSettings),
            iced::system::theme_changes().map(Message::SystemTheme),
        ];
        // Letting go of the mouse ends a selection, which the brush paints in the left pane.
        if self.project.is_some() {
            subscriptions.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) => Some(Message::Idea(IdeaAction::PaintSelection)),
                _ => None,
            }));
        }
        // While anything is waiting to be saved: check the autosave delay and animate the dot.
        if self.autosaver.state() != SaveState::Saved {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(Message::Tick));
        }
        // While the Rewrite slide or the assistant pane's slide plays: about 60 frames a second.
        let project_sliding = self
            .project
            .as_ref()
            .is_some_and(|p| p.slide_started.is_some());
        if project_sliding || self.assistant.slide_started.is_some() {
            subscriptions.push(iced::time::every(Duration::from_millis(16)).map(Message::Tick));
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let show_controls = window_controls::visible(self.settings.window_controls);
        // Always inside `responsive`, so opening the pane keeps the editor's state.
        let document = responsive(move |size| assistant_view::beside(self, size.width));
        let mut body = column![document];
        if self.find.open {
            body = body.push(find_bar::view(self));
        }
        let body = body.push(status_bar::view(self));
        // Everything that comes and goes keeps its place, left empty while it's hidden: iced keeps
        // a widget's state (focus, selection, scroll offsets) only while the widget tree keeps its
        // shape. So opening Settings or a panel, or hiding the window controls, resets nothing.
        let strip = match show_controls {
            true => window_controls::strip(),
            false => space().into(),
        };
        let page = column![strip, body];

        // Each overlay has its own layer over the page.
        let (mut card, mut panel, mut weights, mut commit) = (None, None, None, None);
        if let Some(project) = &self.project {
            let covered = project.idea_panel.is_some() || project.weights_open;
            if !covered && self.commit_note.is_none() {
                card = ideas_view::note_card(self, project);
            }
            if let Some(idea_panel) = &project.idea_panel {
                panel = Some(idea_panel::panel(self, project, idea_panel));
            }
            if project.weights_open {
                weights = Some(weights_view::view(self, project));
            }
            if let Some(note) = &self.commit_note {
                commit = Some(commit_dialog::commit_dialog(self, project, note));
            }
        }
        let layers = [
            self.menu_open.then(|| status_bar::menu(self)),
            card,
            panel,
            weights,
            commit,
            self.switcher.as_ref().map(switcher::view),
            self.settings_open.then(|| settings_panel::view(self)),
        ];
        let page = stack![page].extend(layers.map(|layer| layer.unwrap_or_else(|| space().into())));
        let page = container(page).style(|theme: &Theme| theme::fill(Roles::of(theme).bg_surface));
        let page = key_guard::guard(page);
        window_controls::with_resize_edges(page.into(), show_controls)
    }
}

impl App {
    /// The document: the editor, or a project's two panes and its idea strip.
    pub fn document_view(&self) -> Element<'_, Message> {
        match &self.project {
            Some(project) => project_view::view(self, project),
            None => {
                let mut paint = self
                    .pane
                    .suggestion_runs(Roles::of(&self.theme), |_| None, false);
                paint.extend(self.find_runs());
                editor::view(
                    &self.pane,
                    &self.settings,
                    self.editable(),
                    editor::EDITOR_ID,
                    Message::Edit,
                    editor::PADDING,
                    paint,
                )
            }
        }
    }
}

/// App-wide keys. Up and Down move through the switcher's list, and Enter commits from anywhere
/// in the Commit dialog or steps through the find bar's matches (each ignored while its panel is
/// closed).
fn shortcut(event: iced::Event, status: event::Status, _window: window::Id) -> Option<Message> {
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    match key {
        keyboard::Key::Named(keyboard::key::Named::Escape) => return Some(Message::CloseMenu),
        // A focused note field has already sent its own Commit.
        keyboard::Key::Named(keyboard::key::Named::Enter) if status == event::Status::Ignored => {
            let shift = modifiers.shift();
            return Some(Message::EnterKey { shift });
        }
        keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
            return Some(Message::SwitcherMove(-1));
        }
        keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
            return Some(Message::SwitcherMove(1));
        }
        _ => {}
    }
    if !modifiers.command() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character(",") => Some(Message::OpenSettings),
        keyboard::Key::Character("0") => Some(Message::OpenScratch),
        keyboard::Key::Character("p") => Some(Message::OpenSwitcher),
        keyboard::Key::Character("i") => Some(Message::Idea(IdeaAction::FromSelection)),
        keyboard::Key::Character("e") => Some(Message::Idea(IdeaAction::Erase(None))),
        keyboard::Key::Character("l") => Some(Message::Assistant(AssistantAction::Toggle)),
        keyboard::Key::Character("w" | "W") if modifiers.shift() => {
            Some(Message::Idea(IdeaAction::ToggleWeights))
        }
        keyboard::Key::Character("w" | "W") => Some(Message::CloseRequested),
        keyboard::Key::Character("s" | "S") if modifiers.shift() => Some(Message::SaveAs),
        keyboard::Key::Character("s" | "S") => Some(Message::Save),
        keyboard::Key::Character("o") => Some(Message::OpenDialog),
        keyboard::Key::Character("n") => Some(Message::NewFile),
        keyboard::Key::Character("f") => Some(Message::Find(FindAction::Open)),
        keyboard::Key::Character("=" | "+") => Some(Message::Zoom(1.0)),
        keyboard::Key::Character("-") => Some(Message::Zoom(-1.0)),
        _ => None,
    }
}

/// The keys that still work while Settings is open: Esc closes it, Ctrl+W the window, and the
/// zoom keys change a setting the panel shows. The rest would act on the document behind it.
fn settings_key(message: Message) -> Option<Message> {
    match message {
        Message::CloseMenu | Message::CloseRequested | Message::Zoom(_) => Some(message),
        _ => None,
    }
}

/// Whether `action` clicks or selects in a pane, so the user is now working in that pane.
fn selects(action: &Action) -> bool {
    matches!(
        action,
        Action::Click(_)
            | Action::Drag(_)
            | Action::SelectWord
            | Action::SelectLine
            | Action::SelectAll
            | Action::Select(_)
    )
}

/// Starts another copy of the app on `path` (or on scratch), so every file gets its own window.
pub fn open_in_new_window(path: Option<PathBuf>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = std::process::Command::new(exe);
    command.args(path);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't open a new window: {error}"))
}

fn modified_time(paths: &Paths) -> Option<SystemTime> {
    std::fs::metadata(paths.settings_file())
        .and_then(|meta| meta.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_editor::{Content, Edit};
    use iced::widget::text_editor::Motion;

    #[test]
    fn restored_cursor_after_a_multi_byte_character_can_be_typed_at() {
        let mut first: Content = Content::with_text("café");
        first.perform(Action::Move(Motion::End));
        let view = ScratchView {
            cursor_line: 0,
            cursor_column: first.cursor().position.column,
            scroll_line: None,
        };
        let scratch = ScratchState {
            text: "café".to_string(),
            view,
        };
        let (line, column) = scratch.cursor();
        let mut second: Content = Content::with_text("café");
        second.move_to(Cursor {
            position: Position { line, column },
            selection: None,
        });
        second.perform(Action::Edit(Edit::Insert('!')));
        assert_eq!(second.text(), "café!");
    }
}
