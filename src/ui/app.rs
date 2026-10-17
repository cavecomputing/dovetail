//! The App: its state, the Message enum, `update` (the only place state changes) and `view`.
//! One process shows one document; the Settings window is a second window in the same process.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use iced::widget::text_editor::{Action, Content, Cursor, Position};
use iced::widget::{column, container, stack};
use iced::window::{self, Direction};
use iced::{Element, Size, Subscription, Task, Theme, event, keyboard};

use crate::brand;
use crate::core::counts::{load_tokenizer_in_background, token_count, word_count};
use crate::core::opened::{Doc, open_doc};
use crate::core::paths::Paths;
use crate::core::recent::recent_files;
use crate::core::save::{Autosaver, SaveState, failure_reason};
use crate::core::scratch::{ScratchState, ScratchView, scratch_open_elsewhere};
use crate::core::settings::{ReadingMode, Settings, Theme as ThemeSetting};
use crate::core::text_file::{DiskStamp, save_text};
use crate::ui::settings_window::{self, Change};
use crate::ui::theme::{self, Roles};
use crate::ui::{editor, status_bar, window_controls};

pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    pub settings_error: Option<String>,
    settings_modified: Option<SystemTime>,
    pub last_comfort_chars: u16,
    system_is_dark: bool,
    pub theme: Theme,

    main_window: window::Id,
    settings_window: Option<window::Id>,

    pub doc: Doc,
    pub content: Content,
    /// Why the text must not be edited (a project, or open in another window); `None` when it
    /// can be.
    pub read_only_reason: Option<String>,
    /// Keeps other windows off this document until the process ends.
    _doc_lock: Option<File>,
    /// The file on disk as this window last read or wrote it.
    disk_stamp: Option<DiskStamp>,
    /// A short-lived message for the status bar, such as why a new window didn't open.
    pub notice: Option<String>,
    pub autosaver: Autosaver,
    pub saving_since: Option<Instant>,
    pub now: Instant,
    closing: bool,

    pub words: usize,
    pub tokens: Option<usize>,
    pub recent: Vec<PathBuf>,
    pub menu_open: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(Action),
    Unindent,
    Tick(Instant),
    Saved(u64, Result<Option<DiskStamp>, String>),
    TokenizerReady,

    WindowOpened(window::Id),
    CursorToMiddle(Size),
    CloseRequested(window::Id),
    WindowClosed(window::Id),
    Drag(window::Id),
    Resize(window::Id, Direction),
    Minimize(window::Id),
    ToggleMaximize(window::Id),

    ToggleMenu,
    CloseMenu,
    OpenDialog,
    OpenPath(PathBuf),
    OpenSettings,
    OpenScratch,

    Setting(Change),
    PollSettings,
    SystemTheme(iced::theme::Mode),
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

fn window_settings(size: Size) -> window::Settings {
    window::Settings {
        size,
        min_size: Some(Size::new(360.0, 240.0)),
        decorations: false,
        exit_on_close_request: false,
        platform_specific: window::settings::PlatformSpecific {
            application_id: brand::APP_ID.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

const DOC_WINDOW_SIZE: Size = Size::new(960.0, 720.0);

impl App {
    fn new(doc: Doc) -> (App, Task<Message>) {
        let paths = Paths::detect();
        let (settings, settings_error) = Settings::load(&paths.settings_file());
        let opened = open_doc(doc, &paths);
        let mut content = Content::with_text(&opened.text);
        if let Some((line, column)) = opened.cursor {
            content.move_to(Cursor {
                position: Position { line, column },
                selection: None,
            });
        }

        let (main_window, open) = window::open(window_settings(DOC_WINDOW_SIZE));
        let (tokenizer_sender, tokenizer_ready) = iced::futures::channel::oneshot::channel();
        load_tokenizer_in_background(move || {
            let _ = tokenizer_sender.send(());
        });

        let app = App {
            settings_modified: modified_time(&paths),
            recent: recent_files(&paths.data_dir),
            words: word_count(&content.text()),
            tokens: None,
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
            settings_window: None,
            doc: opened.doc,
            content,
            read_only_reason: opened.read_only_reason,
            _doc_lock: opened.lock,
            disk_stamp: opened.disk_stamp,
            notice: None,
            saving_since: None,
            now: Instant::now(),
            closing: false,
            menu_open: false,
        };
        let tasks = Task::batch([
            open.map(Message::WindowOpened),
            Task::perform(tokenizer_ready, |_| Message::TokenizerReady),
            iced::system::theme().map(Message::SystemTheme),
        ]);
        (app, tasks)
    }

    fn title(&self, window: window::Id) -> String {
        if Some(window) == self.settings_window {
            "Settings".to_string()
        } else {
            status_bar::title(&self.doc)
        }
    }

    fn theme(&self, _window: window::Id) -> Theme {
        self.theme.clone()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // A read-only window still scrolls, selects and copies, but never changes the text.
            Message::Edit(action) if action.is_edit() && !self.editable() => {}
            Message::Edit(action) => {
                let edited = action.is_edit();
                self.content.perform(action);
                if edited {
                    self.text_changed();
                }
            }
            Message::Unindent => {
                if self.editable() {
                    editor::unindent(&mut self.content, usize::from(self.settings.tab_spaces));
                    self.text_changed();
                }
            }
            Message::Tick(now) => {
                self.now = now;
                if self.autosaver.should_save_now(now) {
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
                }
                if self.closing && failed {
                    // Stay open; the status bar shows why the save failed.
                    self.closing = false;
                } else if self.closing {
                    return self.continue_closing();
                }
            }
            Message::TokenizerReady => self.tokens = token_count(&self.content.text()),

            Message::WindowOpened(window) if window == self.main_window => {
                return Task::batch([
                    iced::widget::operation::focus(editor::EDITOR_ID),
                    window::size(window).map(Message::CursorToMiddle),
                ]);
            }
            Message::WindowOpened(_) => {}
            Message::CursorToMiddle(size) => return self.scroll_cursor_to_middle(size),
            Message::CloseRequested(window) if window == self.main_window => {
                self.closing = true;
                if matches!(self.doc, Doc::Scratch) && self.editable() {
                    let _ = self.scratch_view().save(&self.paths.data_dir);
                }
                return self.continue_closing();
            }
            Message::CloseRequested(window) => return window::close(window),
            Message::WindowClosed(window) if window == self.main_window => return iced::exit(),
            Message::WindowClosed(_) => self.settings_window = None,
            Message::Drag(window) => return window::drag(window),
            Message::Resize(window, direction) => return window::drag_resize(window, direction),
            Message::Minimize(window) => return window::minimize(window, true),
            Message::ToggleMaximize(window) => return window::toggle_maximize(window),

            Message::ToggleMenu => {
                self.menu_open = !self.menu_open;
                self.recent = recent_files(&self.paths.data_dir);
            }
            Message::CloseMenu => {
                self.menu_open = false;
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }
            Message::OpenDialog => {
                self.menu_open = false;
                let dialog = rfd::AsyncFileDialog::new()
                    .add_filter("Text and projects", &["txt", brand::PROJECT_EXT])
                    .add_filter("All files", &["*"])
                    .pick_file();
                return Task::perform(dialog, |picked| match picked {
                    Some(file) => Message::OpenPath(file.path().to_path_buf()),
                    None => Message::CloseMenu,
                });
            }
            Message::OpenPath(path) => {
                self.menu_open = false;
                self.notice = open_in_new_window(Some(path)).err();
            }
            Message::OpenSettings => {
                self.menu_open = false;
                return match self.settings_window {
                    Some(window) => window::gain_focus(window),
                    None => {
                        let (window, open) = window::open(window_settings(Size::new(760.0, 780.0)));
                        self.settings_window = Some(window);
                        open.map(Message::WindowOpened)
                    }
                };
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

            Message::Setting(change) => {
                if let Change::ReadingMode(ReadingMode::Comfort { chars }) = change {
                    self.last_comfort_chars = chars;
                }
                settings_window::apply(change, &mut self.settings);
                self.settings_error = self
                    .settings
                    .save_unless_broken(&self.paths.settings_file())
                    .err();
                self.settings_modified = modified_time(&self.paths);
                self.settings_changed();
            }
            Message::PollSettings => {
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
        }
        Task::none()
    }

    fn text_changed(&mut self) {
        let now = Instant::now();
        self.autosaver.edited(now);
        self.saving_since.get_or_insert(now);
        let text = self.content.text();
        self.words = word_count(&text);
        self.tokens = token_count(&text);
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
    }

    /// Writes the current text in the background; `Saved` reports back.
    fn save(&mut self, now: Instant) -> Task<Message> {
        let generation = self.autosaver.begin(now);
        let text = self.content.text();
        let data_dir = self.paths.data_dir.clone();
        let view = self.scratch_view();
        let doc = self.doc.clone();
        let disk_stamp = self.disk_stamp;
        Task::perform(
            async move {
                let result = match doc {
                    Doc::Scratch => ScratchState { text, view }.save(&data_dir).map(|_| None),
                    Doc::File { path, line_ending } => {
                        save_text(&path, &text, line_ending, disk_stamp)
                    }
                    Doc::Project(_) => Ok(None),
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
        let position = self.content.cursor().position;
        ScratchView {
            cursor_line: position.line,
            cursor_column: position.column,
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
        if self.content.cursor().position.line < visible_lines {
            return Task::none();
        }
        let lines = (visible_lines / 2) as i32;
        Task::done(Message::Edit(Action::Scroll { lines }))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
            event::listen_with(shortcut),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::PollSettings),
            iced::system::theme_changes().map(Message::SystemTheme),
        ];
        // While anything is waiting to be saved: check the autosave delay and animate the dot.
        if self.autosaver.state() != SaveState::Saved {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(Message::Tick));
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        let show_controls = window_controls::visible(self.settings.window_controls);
        let body = if Some(window) == self.settings_window {
            settings_window::view(self)
        } else {
            column![
                editor::view(&self.content, &self.settings, self.editable()),
                status_bar::view(self)
            ]
            .into()
        };
        let mut page = if show_controls {
            column![window_controls::strip(window), body].into()
        } else {
            body
        };
        if self.menu_open && window == self.main_window {
            page = stack![page, status_bar::menu(self)].into();
        }
        let page = container(page).style(|theme: &Theme| theme::fill(Roles::of(theme).bg_surface));
        if show_controls {
            window_controls::with_resize_edges(page.into(), window)
        } else {
            page.into()
        }
    }
}

/// App-wide keys. Ctrl+P opens the menu with its recent files until the phase-2 switcher.
fn shortcut(event: iced::Event, _status: event::Status, _window: window::Id) -> Option<Message> {
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    if key == keyboard::Key::Named(keyboard::key::Named::Escape) {
        return Some(Message::CloseMenu);
    }
    if !modifiers.command() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character(",") => Some(Message::OpenSettings),
        keyboard::Key::Character("0") => Some(Message::OpenScratch),
        keyboard::Key::Character("p") => Some(Message::ToggleMenu),
        _ => None,
    }
}

/// Starts another copy of the app on `path` (or on scratch), so every file gets its own window.
fn open_in_new_window(path: Option<PathBuf>) -> Result<(), String> {
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
    use iced::widget::text_editor::{Edit, Motion};

    #[test]
    fn restored_cursor_after_a_multi_byte_character_can_be_typed_at() {
        let mut first: Content = Content::with_text("café");
        first.perform(Action::Move(Motion::End));
        let view = ScratchView {
            cursor_line: 0,
            cursor_column: first.cursor().position.column,
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
