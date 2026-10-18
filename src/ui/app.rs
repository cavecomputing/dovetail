//! The App: its state, the Message enum, `update` (the only place state changes) and `view`.
//! One process shows one document in one window; Settings opens over it.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use iced::widget::{column, container, responsive, space, stack};
use iced::window::{self, Direction};
use iced::{Element, Size, Subscription, Task, Theme, event, keyboard};

use crate::core::config::yaml_indent;
use crate::core::counts::{Counts, load_tokenizer_in_background, token_count, word_count};
use crate::core::json::reformat;
use crate::core::opened::{Doc, OpenedDoc, open_doc};
use crate::core::paths::Paths;
use crate::core::recent::existing_recent_files;
use crate::core::save::{Autosaver, SaveState, failure_reason};
use crate::core::scratch::{ScratchState, ScratchView};
use crate::core::settings::{ReadingMode, Settings, Theme as ThemeSetting};
use crate::core::syntax::Syntax;
use crate::core::text_file::{DiskStamp, TEXT_EXTENSIONS, changed_on_disk, read_text, save_text};
use crate::core::typing::is_code;
use crate::ui::assistant_actions::{Assistant, AssistantAction, CardState};
use crate::ui::editor::{Decision, Pane};
use crate::ui::find_bar::{self, Find, FindAction};
use crate::ui::palette::{self, Palette};
use crate::ui::settings_panel::{self, Change, Connection, KeyField};
use crate::ui::switcher::{self, Switcher};
use crate::ui::text_editor::{Action, Cursor, Edit, Motion, Position};
use crate::ui::theme::{self, Roles};
use crate::ui::{assistant_view, editor, key_guard, status_bar, window_controls};

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
    /// Counts the documents this window has shown (`show`), so an answer meant for an earlier
    /// one, such as a save dialog's, is dropped.
    pub document: u64,
    /// The text being edited.
    pub pane: Pane,
    /// The Ctrl+P switcher, while it is open.
    pub switcher: Option<Switcher>,
    /// The Ctrl+Shift+P palette, while it is open.
    pub palette: Option<Palette>,
    /// Whether the Settings panel is open over the document.
    pub settings_open: bool,
    /// Whether the view opens at a saved top line, rather than around the cursor.
    scroll_restored: bool,
    /// Why the text must not be edited (open in another window, or it can't be saved); `None`
    /// when it can be.
    pub read_only_reason: Option<String>,
    /// In a JSON, YAML, TOML or INI file, why the text isn't valid (checked after a pause in
    /// typing).
    pub syntax_problem: Option<String>,
    /// Keeps other windows off this document until the process ends.
    pub _doc_lock: Option<File>,
    /// The file on disk as this window last read or wrote it.
    pub disk_stamp: Option<DiskStamp>,
    /// A save was refused because the file changed on disk; the status bar offers a way out.
    pub disk_conflict: bool,
    /// A short-lived message for the status bar, such as why a file didn't open, and when
    /// it went up (see `set_notice`).
    pub notice: Option<(String, Instant)>,
    pub autosaver: Autosaver,
    pub saving_since: Option<Instant>,
    pub now: Instant,
    closing: bool,
    /// A document to open in this one's place once the save running now ends (see `open_here`).
    pub opening: Option<Doc>,
    /// A Save as to make once the save running now ends (see `save_as`).
    pub saving_as: Option<PathBuf>,

    /// The text's word and token counts. The token count lags behind typing (see `TOKEN_PAUSE`).
    pub counts: Counts,
    /// When the latest edit the token count doesn't include yet was made.
    uncounted_edit: Option<Instant>,
    /// The recent files, for the menu.
    pub recent: Vec<PathBuf>,
    pub menu_open: bool,
    /// The Ctrl+F find bar.
    pub find: Find,
    /// The modifier keys held now, for the Replace box's Ctrl+Enter.
    pub modifiers: keyboard::Modifiers,

    /// The assistant pane and its conversation, kept while the window is open.
    pub assistant: Assistant,
    /// The Settings panel's API key field, and how its last connection test went.
    pub key_field: KeyField,
    pub connection: Option<Connection>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(Action),
    Indent,
    Unindent,
    /// Enter in the editor: a new line, indented like the caret's (`Pane::new_line`). In a
    /// Markdown file Enter without Shift also continues a list (`Pane::markdown_new_line`).
    NewLine {
        shift: bool,
    },
    /// Backspace in the editor, which takes back a whole indent in a line's leading spaces
    /// (`Pane::backspace`).
    Backspace,
    /// Ctrl+Backspace, Ctrl+Delete and their macOS keys: delete from the caret to where the
    /// motion goes, such as the start of the word (`Pane::delete_to`).
    DeleteTo(Motion),
    /// Alt+↑ or Alt+↓ in the editor: move the caret's line, or the selected lines, up (`up`) or
    /// down one line (`Pane::move_lines`).
    MoveLines {
        up: bool,
    },
    /// Ctrl+Shift+D in the editor: copy the caret's line, or the selected lines, to just below
    /// (`Pane::duplicate_lines`).
    DuplicateLines,
    /// Ctrl+Shift+K in the editor: delete the caret's line, or the selected lines
    /// (`Pane::delete_lines`).
    DeleteLines,
    /// Ctrl+D in the editor: select the word at the caret, or the next place the selected text
    /// appears as well (`Pane::select_next_match`).
    SelectNextMatch,
    /// Ctrl+B or Ctrl+I in the editor: in a Markdown file, put this marker (bold's `**` or
    /// italic's `_`, a `*` inside a word) around the selection, or take it off
    /// (`Pane::toggle_marker`).
    ToggleMarker(&'static str),
    /// Ctrl+/ in the editor: in a script or config file, comment out the lines or uncomment them
    /// (`Pane::toggle_comment`).
    ToggleComment,
    Undo,
    Redo,
    /// Ctrl+X or Ctrl+C while reviewing: copy the selected text, without the suggestions, and
    /// delete it for Ctrl+X (`cut`).
    Clip {
        cut: bool,
    },
    /// Accept or reject suggested edits (their buttons in the text, Tab or Esc).
    Decide(Decision),
    Tick(Instant),
    /// A save of the document with this number (`App::document`) ended: its generation and
    /// how it went.
    Saved(u64, u64, Result<Option<DiskStamp>, String>),
    TokenizerReady,
    TokensCounted(Option<usize>),
    /// The syntax check of the document with this number (`App::document`) ended.
    SyntaxChecked(u64, Option<String>),
    /// Format (`minify` off) or Minify a .json file's text.
    ReformatJson {
        minify: bool,
    },

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
    /// Enter outside the editor, or ⌘G on macOS: steps through the find bar's matches (back with
    /// Shift).
    EnterKey {
        shift: bool,
    },
    NewFile,
    /// The New file dialog's answer, for the document with this number (`App::document`).
    NewFilePicked(u64, PathBuf),
    OpenDialog,
    Save,
    SaveAs,
    /// The Save as dialog's answer, for the document with this number (`App::document`).
    SaveAsPicked(u64, PathBuf),
    OpenPath(PathBuf),
    OpenSettings,
    OpenScratch,
    OpenSwitcher,
    SwitcherQuery(String),
    /// ↑ or ↓: moves the highlight in the switcher or the palette, whichever is open.
    SwitcherMove(i32),
    SwitcherOpen,
    /// Open the switcher's row with this number: Ctrl+1 to Ctrl+9 (Ctrl+0 is `OpenScratch`, its
    /// row 0), or a click.
    SwitcherPick(usize),
    OpenPalette,
    PaletteQuery(String),
    /// Run the palette's tool with this number (Enter runs the highlighted one).
    PaletteRun(usize),

    ReloadFromDisk,
    KeepMine,

    Setting(Change),
    /// Ctrl+= and Ctrl+-: the font a size bigger or smaller.
    Zoom(f32),
    /// Ctrl+Shift+L and the menu: line numbers on or off.
    ToggleLineNumbers,
    /// Once a second: rereads settings.toml if it changed, and takes down an old notice.
    PollSettings,
    SystemTheme(iced::theme::Mode),

    Assistant(AssistantAction),
    Find(FindAction),
    /// The modifier keys held changed.
    Modifiers(keyboard::Modifiers),
    /// Tab outside the editor, such as in the find bar: the keyboard to the next text box, or
    /// the one before (`back`, with Shift).
    FocusNext {
        back: bool,
    },
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

// macOS sets all three of its fields below, which clippy would call a needless `..Default`.
#[cfg_attr(target_os = "macos", allow(clippy::needless_update))]
fn window_settings() -> window::Settings {
    window::Settings {
        // Fits a 1080p laptop screen at Windows' usual 150% scale: 720 high, less the taskbar.
        size: Size::new(960.0, 640.0),
        min_size: Some(Size::new(360.0, 240.0)),
        // macOS keeps its own title bar, see-through and untitled: its buttons sit in our top
        // strip, and it resizes the window itself (see `window_controls`).
        decorations: cfg!(target_os = "macos"),
        exit_on_close_request: false,
        // X11 and Windows show this on the taskbar (Windows also in Alt+Tab, scaled up from its
        // small icon). Wayland finds the icon through the .desktop file, and macOS, which has no
        // window icons, through the app bundle.
        icon: window::icon::from_rgba(APP_ICON_RGBA.to_vec(), 64, 64).ok(),
        platform_specific: window::settings::PlatformSpecific {
            // Each OS has its own fields here, and only Linux has an app id.
            #[cfg(target_os = "linux")]
            application_id: crate::brand::APP_ID.to_string(),
            #[cfg(target_os = "macos")]
            title_hidden: true,
            #[cfg(target_os = "macos")]
            titlebar_transparent: true,
            #[cfg(target_os = "macos")]
            fullsize_content_view: true,
            // Windows draws no edge round an undecorated window unless asked: a drop shadow
            // (iced warns of a 1px line along the top), and rounded corners on Windows 11.
            #[cfg(target_os = "windows")]
            undecorated_shadow: true,
            #[cfg(target_os = "windows")]
            corner_preference: window::settings::platform::CornerPreference::Round,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// How long typing must pause before the token count is redone. Counting a long text takes a
/// while (about 25 ms for 1 MB), too long to do on every key.
const TOKEN_PAUSE: Duration = Duration::from_millis(300);

/// How long a notice stays in the status bar: long enough to read, gone before it goes stale.
const NOTICE_TIME: Duration = Duration::from_secs(6);

/// The app icon at 64x64, as raw RGBA bytes (made from assets/icon/icon.svg, see its README).
const APP_ICON_RGBA: &[u8] = include_bytes!("../../assets/icon/icon-64.rgba");

impl App {
    fn new(doc: Doc) -> (App, Task<Message>) {
        App::with_paths(doc, Paths::detect())
    }

    /// `new` with its folders given, for tests.
    fn with_paths(doc: Doc, paths: Paths) -> (App, Task<Message>) {
        let (settings, settings_error) = Settings::load(&paths.settings_file());
        let opened = open_doc(doc, &paths);
        let pane = pane_for(&opened);

        let (main_window, open) = window::open(window_settings());
        let (tokenizer_sender, tokenizer_ready) = iced::futures::channel::oneshot::channel();
        load_tokenizer_in_background(move || {
            let _ = tokenizer_sender.send(());
        });

        let app = App {
            settings_modified: modified_time(&paths),
            recent: existing_recent_files(&paths.data_dir),
            counts: Counts {
                words: word_count(&opened.text),
                tokens: None,
            },
            uncounted_edit: None,
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
            document: 0,
            pane,
            switcher: None,
            palette: None,
            settings_open: false,
            scroll_restored: opened.scroll_line.is_some(),
            read_only_reason: opened.read_only_reason,
            syntax_problem: None,
            _doc_lock: opened.lock,
            disk_stamp: opened.disk_stamp,
            disk_conflict: false,
            notice: None,
            saving_since: None,
            now: Instant::now(),
            closing: false,
            opening: None,
            saving_as: None,
            menu_open: false,
            find: Find::default(),
            modifiers: keyboard::Modifiers::default(),
            assistant: Assistant::default(),
            key_field: KeyField::default(),
            connection: None,
        };
        let tasks = Task::batch([
            app.check_syntax(),
            open.map(|_| Message::WindowOpened),
            Task::perform(tokenizer_ready, |_| Message::TokenizerReady),
            iced::system::theme().map(Message::SystemTheme),
        ]);
        (app, tasks)
    }

    /// Makes `opened` this window's document in place of the one it showed, as if the window had
    /// been launched on it: its text, cursor, lock (the old one's goes), and a fresh undo history
    /// and save state. The find bar closes, and the assistant starts a new chat (its pane stays as
    /// it was), since both were about the old text.
    pub fn show(&mut self, opened: OpenedDoc) -> Task<Message> {
        self.document += 1;
        self.pane = pane_for(&opened);
        self.counts = Counts {
            words: word_count(&opened.text),
            tokens: None,
        };
        self.uncounted_edit = None;
        self.doc = opened.doc;
        self.read_only_reason = opened.read_only_reason;
        self._doc_lock = opened.lock;
        self.disk_stamp = opened.disk_stamp;
        self.disk_conflict = false;
        self.autosaver = Autosaver::new(Duration::from_millis(self.settings.autosave_delay_ms));
        self.saving_since = None;
        self.recent = existing_recent_files(&self.paths.data_dir);
        self.find = Find::default();
        self.assistant.stop();
        self.assistant.conversation = Default::default();
        self.assistant.cards.clear();
        self.syntax_problem = None;
        let focus = iced::widget::operation::focus(editor::EDITOR_ID);
        Task::batch([self.count_tokens(), self.check_syntax(), focus])
    }

    fn title(&self, _window: window::Id) -> String {
        status_bar::title(&self.doc)
    }

    fn theme(&self, _window: window::Id) -> Theme {
        self.theme.clone()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // A read-only window still scrolls, selects and copies, but never changes the text.
            Message::Edit(action) if action.is_edit() && !self.editable() => {}
            // In code, brackets and quotes close themselves.
            Message::Edit(Action::Edit(Edit::Insert(typed))) if is_code(self.syntax()) => {
                let edited = self.pane.type_in_code(typed, self.syntax());
                self.after_edit(edited);
            }
            Message::Edit(action) => match self.pane.perform(action) {
                Ok(true) => self.text_changed(),
                Ok(false) => {}
                Err(reason) => self.set_notice(Some(reason)),
            },
            // The widget's text has the suggestions in it, so the clipboard gets the text from
            // the model instead.
            Message::Clip { cut } => {
                if let Some(text) = self.pane.copied() {
                    let delete = match cut {
                        true => self.update(Message::Edit(Action::Edit(Edit::Delete))),
                        false => Task::none(),
                    };
                    return Task::batch([delete, iced::clipboard::write(text)]);
                }
            }
            // The editing keys `editor::key_binding` binds. A read-only window takes none of them.
            Message::Indent
            | Message::Unindent
            | Message::NewLine { .. }
            | Message::Backspace
            | Message::DeleteTo(_)
            | Message::MoveLines { .. }
            | Message::DuplicateLines
            | Message::DeleteLines
            | Message::SelectNextMatch
            | Message::ToggleMarker(_)
            | Message::ToggleComment
                if self.editable() =>
            {
                // Ctrl+D's other places take Backspace and Enter; any other key lets them go.
                if !matches!(
                    message,
                    Message::Backspace | Message::NewLine { .. } | Message::SelectNextMatch
                ) {
                    self.pane.clear_others();
                }
                let syntax = self.syntax();
                // A YAML file nests by its own step, 2 when it has none yet.
                let tab_spaces = match syntax {
                    Syntax::Yaml => yaml_indent(self.pane.model.text()),
                    _ => usize::from(self.settings.tab_spaces),
                };
                let markdown = syntax == Syntax::Markdown;
                let edited = match message {
                    Message::Indent => self.pane.indent(tab_spaces, markdown),
                    Message::Unindent => self.pane.unindent(tab_spaces),
                    Message::NewLine { shift: false } if markdown => {
                        self.pane.markdown_new_line(tab_spaces)
                    }
                    Message::NewLine { shift: false }
                        if is_code(syntax) || syntax == Syntax::Yaml =>
                    {
                        self.pane.code_new_line(tab_spaces, syntax)
                    }
                    Message::NewLine { .. } => self.pane.new_line(),
                    Message::Backspace => self.pane.backspace(tab_spaces, syntax),
                    Message::DeleteTo(motion) => self.pane.delete_to(motion),
                    Message::MoveLines { up } => self.pane.move_lines(up),
                    Message::DuplicateLines => self.pane.duplicate_lines(),
                    Message::DeleteLines => self.pane.delete_lines(),
                    Message::SelectNextMatch => self.pane.select_next_match(),
                    Message::ToggleMarker(marker) if markdown => self.pane.toggle_marker(marker),
                    Message::ToggleComment => match syntax.comment_marker() {
                        Some(marker) => self.pane.toggle_comment(marker),
                        None => Ok(false),
                    },
                    // Ctrl+B and Ctrl+I outside Markdown do nothing, and the arm's pattern lets
                    // no other message in.
                    _ => Ok(false),
                };
                self.after_edit(edited);
            }
            Message::Indent
            | Message::Unindent
            | Message::NewLine { .. }
            | Message::Backspace
            | Message::DeleteTo(_)
            | Message::MoveLines { .. }
            | Message::DuplicateLines
            | Message::DeleteLines
            | Message::SelectNextMatch
            | Message::ToggleMarker(_)
            | Message::ToggleComment => {}
            Message::Undo | Message::Redo if self.editable() => {
                let changed = match message {
                    Message::Undo => self.pane.undo(),
                    _ => self.pane.redo(),
                };
                if changed {
                    self.text_changed();
                }
            }
            Message::Undo | Message::Redo => {}
            // Tab in the editor accepts the suggestion the caret is on; anywhere else it indents.
            Message::Decide(Decision::Accept(None))
                if self.pane.suggestion_at_caret().is_none() =>
            {
                return self.update(Message::Indent);
            }
            // Esc in the editor rejects the suggestion the caret is on. With the caret anywhere
            // else, or while the find bar is open, it does nothing here (that Esc only closes the
            // bar, through `CloseMenu`).
            Message::Decide(Decision::Reject(None))
                if self.find.open || self.pane.suggestion_at_caret().is_none() => {}
            Message::Decide(decision) => self.decide(decision),
            Message::Tick(now) => {
                self.now = now;
                self.assistant.finish_slide(now);
                let mut tasks = Vec::new();
                if self
                    .uncounted_edit
                    .is_some_and(|edited| now.duration_since(edited) >= TOKEN_PAUSE)
                {
                    self.uncounted_edit = None;
                    tasks.push(self.count_tokens());
                    tasks.push(self.check_syntax());
                }
                // Over a file changed on disk, saves wait for Reload or Keep mine.
                if !self.disk_conflict && self.autosaver.should_save_now(now) {
                    tasks.push(self.save(now));
                }
                return Task::batch(tasks);
            }
            // A save of a document the window has left; its own saver went with it.
            Message::Saved(document, ..) if document != self.document => {}
            Message::Saved(_, generation, result) => {
                let failed = result.is_err();
                if let Ok(Some(stamp)) = result {
                    self.disk_stamp = Some(stamp);
                }
                self.autosaver.finish(generation, result.map(|_| ()));
                if self.autosaver.state() == SaveState::Saved {
                    self.saving_since = None;
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
                } else if let Some(doc) = self.opening.take() {
                    self.saving_as = None;
                    return self.open_here(doc);
                } else if let Some(path) = self.saving_as.take() {
                    return self.save_as(path);
                }
            }
            Message::TokenizerReady => return self.count_tokens(),
            Message::TokensCounted(tokens) => self.counts.tokens = tokens,
            Message::SyntaxChecked(document, problem) if document == self.document => {
                self.syntax_problem = problem;
            }
            Message::SyntaxChecked(..) => {}
            Message::ReformatJson { minify } => {
                self.menu_open = false;
                if !self.editable() {
                    return Task::none();
                }
                let indent = (!minify).then_some(usize::from(self.settings.tab_spaces));
                let text = self.pane.model.text();
                match reformat(text, indent) {
                    // Already laid out that way: no undo step for nothing.
                    Ok(reformatted) if reformatted == text => {}
                    // One undo step, so Ctrl+Z brings the old layout back.
                    Ok(reformatted) => {
                        let whole = 0..text.len();
                        let edited = self.pane.replace_all(&[whole], &reformatted);
                        self.after_edit(edited);
                    }
                    // The problem goes where the JSON check puts it, so it isn't said twice.
                    Err(problem) => {
                        self.syntax_problem = Some(problem);
                        let action = if minify { "minify" } else { "format" };
                        self.set_notice(Some(format!("Can't {action} invalid JSON")));
                    }
                }
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }

            Message::WindowOpened => {
                #[cfg(target_os = "macos")]
                window_controls::quit_closes_window();
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
                // Closing replaces a pending open or Save as; a close that fails doesn't bring
                // them back.
                self.opening = None;
                self.saving_as = None;
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
                self.recent = existing_recent_files(&self.paths.data_dir);
            }
            // Esc, or a click outside: closes the menu, the switcher, the palette and Settings.
            // With none of them open, Esc closes the find bar.
            Message::CloseMenu => {
                let mut closed = self.menu_open || self.switcher.is_some();
                closed |= self.palette.take().is_some();
                closed |= std::mem::take(&mut self.settings_open);
                self.menu_open = false;
                self.switcher = None;
                // The find bar sits under all of those, so it closes on an Esc of its own, after
                // the go to line box over it. Then Esc lets go of Ctrl+D's other places.
                if !closed
                    && self.find.line.take().is_none()
                    && !std::mem::take(&mut self.find.open)
                {
                    self.pane.clear_others();
                }
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }
            Message::EnterKey { shift } => {
                if self.find.open {
                    let step = if shift { -1 } else { 1 };
                    return self.update_find(FindAction::Step(step));
                }
            }
            Message::NewFile => return self.new_file(),
            // A dialog answered after the window moved on would act on the wrong document.
            Message::NewFilePicked(document, _) | Message::SaveAsPicked(document, _)
                if document != self.document =>
            {
                let words = "That dialog was for the document this window showed before";
                self.set_notice(Some(words.to_string()));
            }
            Message::NewFilePicked(_, path) => return self.start_new_file(path),
            Message::Save => return self.save_now(),
            Message::SaveAs => return self.pick_save_path(),
            Message::SaveAsPicked(_, path) => return self.save_as(path),
            Message::OpenDialog => {
                self.menu_open = false;
                let mut dialog = rfd::AsyncFileDialog::new();
                // macOS puts every filter's extensions in one list, where "*" isn't a wildcard,
                // so only .txt and .md could be picked; with no filter it allows any file.
                if !cfg!(target_os = "macos") {
                    dialog = dialog
                        .add_filter("Text, config and script files", &TEXT_EXTENSIONS)
                        .add_filter("All files", &["*"]);
                }
                let dialog = dialog.pick_file();
                // Cancelling leaves everything as it was.
                return Task::perform(dialog, |picked| picked)
                    .and_then(|file| Task::done(Message::OpenPath(file.path().to_path_buf())));
            }
            Message::OpenPath(path) => return self.open_file(path),
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
            Message::OpenScratch => return self.open_here(Doc::Scratch),
            Message::SwitcherMove(_) if self.palette.is_some() => {
                return self.update_palette(message);
            }
            Message::OpenPalette | Message::PaletteQuery(_) | Message::PaletteRun(_) => {
                return self.update_palette(message);
            }
            Message::OpenSwitcher
            | Message::SwitcherQuery(_)
            | Message::SwitcherMove(_)
            | Message::SwitcherOpen
            | Message::SwitcherPick(_) => return self.update_switcher(message),
            Message::ReloadFromDisk => {
                if let Doc::File {
                    path,
                    line_ending,
                    bom,
                } = &mut self.doc
                {
                    match read_text(path) {
                        Ok(file) => {
                            *line_ending = file.line_ending;
                            *bom = file.bom;
                            self.disk_stamp = DiskStamp::of(path);
                            // One undo step, so Ctrl+Z brings this window's text back.
                            let length = self.pane.model.text().len();
                            self.pane.replace(0..length, &file.text);
                            self.text_changed();
                            self.autosaver.mark_saved();
                            self.saving_since = None;
                            self.disk_conflict = false;
                        }
                        Err(error) => self.set_notice(Some(format!("Couldn't reload: {error}"))),
                    }
                }
                // Clicking the button took the keyboard from the editor; give it back.
                return iced::widget::operation::focus(editor::EDITOR_ID);
            }
            Message::KeepMine => {
                if let Doc::File { path, .. } = &self.doc {
                    // Take the file on disk as seen, so the next save writes over it.
                    self.disk_stamp = DiskStamp::of(path);
                    self.disk_conflict = false;
                    let focus = iced::widget::operation::focus(editor::EDITOR_ID);
                    // One save at a time: a running one is followed by the autosave.
                    if self.autosaver.is_saving() {
                        return focus;
                    }
                    return Task::batch([self.save(Instant::now()), focus]);
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
            Message::ToggleLineNumbers => {
                self.menu_open = false;
                let on = !self.settings.line_numbers;
                return self.update(Message::Setting(Change::LineNumbers(on)));
            }
            Message::PollSettings => {
                if let Some((_, shown)) = &self.notice
                    && shown.elapsed() >= NOTICE_TIME
                {
                    self.notice = None;
                }
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
            Message::Assistant(action) => return self.update_assistant(action),
            Message::Find(action) => return self.update_find(action),
            Message::Modifiers(modifiers) => self.modifiers = modifiers,
            Message::FocusNext { back: true } => return iced::widget::operation::focus_previous(),
            Message::FocusNext { back: false } => return iced::widget::operation::focus_next(),
        }
        Task::none()
    }

    /// Puts `notice` up in the status bar for `NOTICE_TIME` (`None` takes the last one down).
    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice.map(|words| (words, Instant::now()));
    }

    /// Follows up an edit to the pane: what changed, or why it couldn't, in the status bar.
    pub fn after_edit(&mut self, edited: Result<bool, String>) {
        match edited {
            Ok(true) => self.text_changed(),
            Ok(false) => {}
            Err(reason) => self.set_notice(Some(reason)),
        }
    }

    fn text_changed(&mut self) {
        let now = Instant::now();
        self.autosaver.edited(now);
        self.saving_since.get_or_insert(now);
        self.counts.words = word_count(self.pane.model.text());
        self.uncounted_edit = Some(now);
        if self.find.open {
            self.find_again(self.pane.caret());
        }
        // Undo, redo and a reload can drop a suggestion the change touched.
        self.settle_cards(CardState::Gone(
            "an edit to the text under it dropped it".to_string(),
        ));
    }

    /// Accepts or rejects suggestions (from their buttons, Tab or Esc, the chat, or new chat), and
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

    /// Counts the text's tokens in the background, as it takes a while on a long text;
    /// `TokensCounted` brings the number back.
    fn count_tokens(&self) -> Task<Message> {
        let text = self.pane.model.text().to_string();
        Task::perform(async move { token_count(&text) }, Message::TokensCounted)
    }

    /// In a JSON, YAML, TOML or INI file or a shell script, checks the text is valid in the
    /// background; `SyntaxChecked` brings any problem back.
    fn check_syntax(&self) -> Task<Message> {
        let syntax = self.syntax();
        if matches!(syntax, Syntax::Plain | Syntax::Markdown) {
            return Task::none();
        }
        let text = self.pane.model.text().to_string();
        let document = self.document;
        Task::perform(async move { syntax.problem(&text) }, move |problem| {
            Message::SyntaxChecked(document, problem)
        })
    }

    /// Writes the current text in the background; `Saved` reports back.
    pub fn save(&mut self, now: Instant) -> Task<Message> {
        let generation = self.autosaver.begin(now);
        let text = self.pane.model.text().to_string();
        let data_dir = self.paths.data_dir.clone();
        let view = self.scratch_view();
        let doc = self.doc.clone();
        let disk_stamp = self.disk_stamp;
        let document = self.document;
        Task::perform(
            async move { write_doc(&doc, text, view, &data_dir, disk_stamp) },
            move |result| Message::Saved(document, generation, result),
        )
    }

    /// Writes the current text now, on this thread, before the window leaves it for another
    /// document (`open_here`), handled as an autosave's result is. Returns whether it worked.
    pub fn save_before_leaving(&mut self) -> bool {
        let generation = self.autosaver.begin(Instant::now());
        let text = self.pane.model.text().to_string();
        let view = self.scratch_view();
        let result = write_doc(&self.doc, text, view, &self.paths.data_dir, self.disk_stamp);
        let written = result.is_ok();
        let _ = self.update(Message::Saved(self.document, generation, result));
        written
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
        // While anything is waiting to be saved or counted: check the autosave delay and the
        // token count's pause, and animate the dot.
        if self.autosaver.state() != SaveState::Saved || self.uncounted_edit.is_some() {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(Message::Tick));
        }
        // While the assistant pane's slide plays: about 60 frames a second.
        if self.assistant.slide_started.is_some() {
            subscriptions.push(iced::time::every(Duration::from_millis(16)).map(Message::Tick));
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let show_controls = window_controls::visible(self.settings.window_controls);
        // Always inside `responsive`, so opening the pane keeps the editor's state.
        let document = responsive(move |size| assistant_view::beside(self, size.width));
        let mut text_area = column![document];
        if self.find.open || self.find.line.is_some() {
            text_area = text_area.push(find_bar::view(self));
        }
        // Settings covers the text area, between the window controls and the status bar, which
        // then says where the settings are saved.
        let (settings, status) = match self.settings_open {
            true => (settings_panel::view(self), settings_panel::status(self)),
            false => (space().into(), status_bar::view(self)),
        };
        let body = column![stack![text_area, settings], status];
        // Everything that comes and goes keeps its place, left empty while it's hidden: iced keeps
        // a widget's state (focus, selection, scroll offsets) only while the widget tree keeps its
        // shape. So opening Settings or a panel, or hiding the window controls, resets nothing.
        let strip = match show_controls {
            true => window_controls::strip(),
            false => space().into(),
        };
        let page = column![strip, body];

        // Each overlay has its own layer over the page.
        let layers = [
            self.menu_open.then(|| status_bar::menu(self)),
            self.switcher.as_ref().map(switcher::view),
            (self.palette.as_ref()).map(|open| palette::view(open, self.syntax())),
        ];
        let page = stack![page].extend(layers.map(|layer| layer.unwrap_or_else(|| space().into())));
        let page = container(page).style(|theme: &Theme| theme::fill(Roles::of(theme).bg_surface));
        let page = key_guard::guard(page);
        window_controls::with_resize_edges(page.into(), show_controls)
    }
}

impl App {
    /// The document: the editor, with any suggestions and find matches drawn in it, and
    /// Markdown coloured in a .md file.
    pub fn document_view(&self) -> Element<'_, Message> {
        let roles = Roles::of(&self.theme);
        let mut paint = self.pane.suggestion_runs(roles);
        paint.extend(self.find_runs());
        paint.extend(self.pane.other_runs(roles));
        editor::view(
            &self.pane,
            &self.settings,
            self.editable(),
            self.syntax(),
            paint,
        )
    }

    /// The kind of text the document is, by its file's extension (or a script's `#!` line):
    /// coloured to match, and for a data file (JSON, YAML, TOML, INI) or a shell script checked
    /// as you type. Scratch is always plain.
    pub fn syntax(&self) -> Syntax {
        match &self.doc {
            Doc::File { path, .. } => Syntax::of(path, self.pane.model.text()),
            Doc::Scratch => Syntax::Plain,
        }
    }
}

/// Writes `text` as `doc`'s (with `view`, scratch's cursor, for scratch), as of the file on disk
/// at `disk_stamp`. Returns the file as written, or the reason it failed.
fn write_doc(
    doc: &Doc,
    text: String,
    view: ScratchView,
    data_dir: &Path,
    disk_stamp: Option<DiskStamp>,
) -> Result<Option<DiskStamp>, String> {
    let result = match doc {
        Doc::Scratch => ScratchState { text, view }.save(data_dir).map(|_| None),
        Doc::File {
            path,
            line_ending,
            bom,
        } => save_text(path, &text, *line_ending, *bom, disk_stamp),
    };
    result.map_err(|error| failure_reason(&error))
}

/// The editor for `opened`: its text, cursor and view. Scratch emptied by Save to a file opens
/// empty, with Ctrl+Z bringing the old text back.
fn pane_for(opened: &OpenedDoc) -> Pane {
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
    pane
}

/// App-wide keys. Up and Down move through the switcher's or the palette's list, Enter steps through the find
/// bar's matches (each ignored while its panel is closed), and Tab outside the editor moves
/// between text boxes.
fn shortcut(event: iced::Event, status: event::Status, _window: window::Id) -> Option<Message> {
    let (key, modifiers) = match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            (key, modifiers)
        }
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            return Some(Message::Modifiers(modifiers));
        }
        _ => return None,
    };
    match key {
        keyboard::Key::Named(keyboard::key::Named::Escape) => return Some(Message::CloseMenu),
        // Not an Enter the editor or a text box has already taken.
        keyboard::Key::Named(keyboard::key::Named::Enter) if status == event::Status::Ignored => {
            let shift = modifiers.shift();
            return Some(Message::EnterKey { shift });
        }
        keyboard::Key::Named(keyboard::key::Named::Tab) if status == event::Status::Ignored => {
            let back = modifiers.shift();
            return Some(Message::FocusNext { back });
        }
        keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
            return Some(Message::SwitcherMove(-1));
        }
        keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
            return Some(Message::SwitcherMove(1));
        }
        _ => {}
    }
    // Go to line is ⌃G on macOS, as in VS Code there, since ⌘G is the next find match.
    #[cfg(target_os = "macos")]
    if modifiers.control() && key.as_ref() == keyboard::Key::Character("g") {
        return Some(Message::Find(FindAction::OpenLine));
    }
    if !modifiers.command() {
        return None;
    }
    // Ctrl+1 to Ctrl+9 open the switcher's numbered rows (ignored while it's closed).
    if let keyboard::Key::Character(digit) = key.as_ref()
        && let Ok(row @ 1..=9) = digit.parse::<usize>()
    {
        return Some(Message::SwitcherPick(row));
    }
    match key.as_ref() {
        keyboard::Key::Character(",") => Some(Message::OpenSettings),
        keyboard::Key::Character("0") => Some(Message::OpenScratch),
        keyboard::Key::Character("p" | "P") if modifiers.shift() => Some(Message::OpenPalette),
        keyboard::Key::Character("p") => Some(Message::OpenSwitcher),
        keyboard::Key::Character("l" | "L") if modifiers.shift() => {
            Some(Message::ToggleLineNumbers)
        }
        keyboard::Key::Character("l") => Some(Message::Assistant(AssistantAction::Toggle)),
        keyboard::Key::Character("w" | "W") => Some(Message::CloseRequested),
        keyboard::Key::Character("s" | "S") if modifiers.shift() => Some(Message::SaveAs),
        keyboard::Key::Character("s" | "S") => Some(Message::Save),
        keyboard::Key::Character("o") => Some(Message::OpenDialog),
        keyboard::Key::Character("n") => Some(Message::NewFile),
        keyboard::Key::Character("f" | "F") if modifiers.shift() => {
            Some(Message::Find(FindAction::OpenReplace))
        }
        keyboard::Key::Character("f" | "F") => Some(Message::Find(FindAction::Open)),
        keyboard::Key::Character("g") if !cfg!(target_os = "macos") => {
            Some(Message::Find(FindAction::OpenLine))
        }
        keyboard::Key::Character("=" | "+") => Some(Message::Zoom(1.0)),
        keyboard::Key::Character("-") => Some(Message::Zoom(-1.0)),
        // macOS's own: ⌘M minimizes, and ⌘G and ⇧⌘G step through the find bar's matches.
        keyboard::Key::Character("m") if cfg!(target_os = "macos") => Some(Message::Minimize),
        keyboard::Key::Character("g") if cfg!(target_os = "macos") => Some(Message::EnterKey {
            shift: modifiers.shift(),
        }),
        _ => None,
    }
}

/// The keys that still work while Settings is open: Esc closes it, Ctrl+W closes the window and
/// ⌘M (macOS) minimizes it, and the zoom keys and Ctrl+Shift+L change a setting the panel shows.
/// The rest would act on the document behind it.
fn settings_key(message: Message) -> Option<Message> {
    match message {
        Message::CloseMenu
        | Message::CloseRequested
        | Message::Minimize
        | Message::Zoom(_)
        | Message::ToggleLineNumbers
        | Message::Modifiers(_) => Some(message),
        _ => None,
    }
}

fn modified_time(paths: &Paths) -> Option<SystemTime> {
    std::fs::metadata(paths.settings_file())
        .and_then(|meta| meta.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scratch::scratch_open_elsewhere;
    use crate::core::text_file::LineEnding;
    use crate::ui::text_editor::Content;
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

    #[test]
    fn ctrl_shift_l_toggles_line_numbers_and_ctrl_l_the_assistant() {
        use keyboard::key::{NativeCode, Physical};
        // iced gives the key without Shift's change ("l"), and `modified_key` with it ("L").
        let press = |modifiers| {
            let event = iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Character("l".into()),
                modified_key: keyboard::Key::Character("L".into()),
                physical_key: Physical::Unidentified(NativeCode::Unidentified),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            });
            shortcut(event, event::Status::Ignored, window::Id::unique())
        };
        let toggle = press(keyboard::Modifiers::COMMAND | keyboard::Modifiers::SHIFT);
        assert!(matches!(toggle, Some(Message::ToggleLineNumbers)));
        // It works with Settings open too, which shows the setting.
        assert!(matches!(
            toggle.and_then(settings_key),
            Some(Message::ToggleLineNumbers)
        ));
        assert!(matches!(
            press(keyboard::Modifiers::COMMAND),
            Some(Message::Assistant(AssistantAction::Toggle))
        ));
    }

    fn file_doc(path: &Path) -> Doc {
        Doc::File {
            path: path.to_path_buf(),
            line_ending: LineEnding::Lf,
            bom: false,
        }
    }

    /// A scratch window over folders in a temp dir, with `typed` typed into it, and a file
    /// `notes.md` beside it to open.
    fn scratch_window(typed: &str) -> (tempfile::TempDir, Paths, App, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let notes = dir.path().join("notes.md");
        std::fs::write(&notes, "# notes\r\n").unwrap();
        let (mut app, _) = App::with_paths(Doc::Scratch, paths.clone());
        for letter in typed.chars() {
            let _ = app.update(Message::Edit(Action::Edit(Edit::Insert(letter))));
        }
        (dir, paths, app, notes)
    }

    #[test]
    fn opening_from_scratch_saves_it_and_shows_the_file_in_its_place() {
        let (_dir, paths, mut app, notes) = scratch_window("hello");
        // Typed inside the autosave pause: nothing is on disk yet.
        assert!(app.autosaver.pending());
        let _ = app.update(Message::OpenPath(notes.clone()));
        let scratch = ScratchState::load(&paths.data_dir).unwrap();
        assert_eq!(scratch.text, "hello");
        assert_eq!(scratch.cursor(), (0, 5));
        // Scratch's lock went with it, so the next scratch window can edit it.
        assert!(!scratch_open_elsewhere(&paths.data_dir));
        assert!(matches!(
            &app.doc,
            Doc::File { path, line_ending: LineEnding::CrLf, .. } if *path == notes
        ));
        assert_eq!(status_bar::title(&app.doc), "notes.md");
        assert_eq!(app.pane.model.text(), "# notes\n");
        assert!(app.editable());
        assert_eq!(app.autosaver.state(), SaveState::Saved);
        assert_eq!(app.recent, vec![notes.clone()]);
    }

    #[test]
    fn scratchs_undo_history_stays_out_of_the_file() {
        let (_dir, _paths, mut app, notes) = scratch_window("scratch text");
        let _ = app.update(Message::OpenPath(notes.clone()));
        let _ = app.update(Message::Undo);
        assert_eq!(app.pane.model.text(), "# notes\n");
        assert!(!app.autosaver.pending(), "nothing for the file to save");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# notes\r\n");
    }

    #[test]
    fn a_failed_scratch_save_keeps_the_window_on_scratch() {
        let (_dir, paths, mut app, notes) = scratch_window("keep me");
        // A folder where scratch's text goes makes every save of it fail.
        std::fs::create_dir_all(crate::core::scratch::scratch_text_path(&paths.data_dir)).unwrap();
        let _ = app.update(Message::OpenPath(notes.clone()));
        assert!(matches!(app.doc, Doc::Scratch));
        assert_eq!(app.pane.model.text(), "keep me");
        assert!(matches!(app.autosaver.state(), SaveState::Failed(_)));
        // The file wasn't claimed, so another window can still edit it.
        let other = open_doc(file_doc(&notes), &paths);
        assert!(other.read_only_reason.is_none());
    }

    #[test]
    fn opening_waits_for_an_autosave_already_running() {
        let (_dir, paths, mut app, notes) = scratch_window("a");
        let generation = app.autosaver.begin(Instant::now());
        let _ = app.update(Message::OpenPath(notes.clone()));
        assert!(matches!(app.doc, Doc::Scratch));
        let _ = app.update(Message::Edit(Action::Edit(Edit::Insert('b'))));
        let _ = app.update(Message::Saved(0, generation, Ok(None)));
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == notes));
        assert_eq!(ScratchState::load(&paths.data_dir).unwrap().text, "ab");
    }

    #[test]
    fn save_as_from_scratch_waits_for_an_autosave_already_running() {
        let (dir, paths, mut app, _notes) = scratch_window("a");
        let generation = app.autosaver.begin(Instant::now());
        let saved = dir.path().join("saved.txt");
        let _ = app.update(Message::SaveAsPicked(0, saved.clone()));
        assert!(
            matches!(app.doc, Doc::Scratch),
            "not while scratch is being written"
        );
        // The autosave lands, with the text as it was when it began.
        ScratchState {
            text: "a".to_string(),
            view: app.scratch_view(),
        }
        .save(&paths.data_dir)
        .unwrap();
        let _ = app.update(Message::Saved(0, generation, Ok(None)));
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == saved));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "a");
        assert_eq!(ScratchState::load(&paths.data_dir).unwrap().text, "");
    }

    #[test]
    fn a_file_window_saves_its_file_and_switches_too() {
        let (dir, paths, mut app, notes) = scratch_window("in scratch");
        let _ = app.update(Message::OpenPath(notes.clone()));
        let _ = app.update(Message::Edit(Action::Edit(Edit::Insert('!'))));
        let other = dir.path().join("other.txt");
        let _ = app.update(Message::OpenPath(other.clone()));
        // Typed inside the autosave pause, and kept with the file's line ending.
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "!# notes\r\n");
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == other));
        // notes.md's lock went with it.
        let again = open_doc(file_doc(&notes), &paths);
        assert!(again.read_only_reason.is_none());
        drop(again);
        // Ctrl+0 brings scratch back here, where it was left.
        let _ = app.update(Message::OpenScratch);
        assert!(matches!(app.doc, Doc::Scratch));
        assert_eq!(app.pane.model.text(), "in scratch");
        assert_eq!(app.pane.content.cursor().position.column, 10);
    }

    #[test]
    fn opening_the_shown_file_or_over_a_disk_conflict_stays_put() {
        let (dir, _paths, mut app, notes) = scratch_window("");
        let _ = app.update(Message::OpenPath(notes.clone()));
        let _ = app.update(Message::Edit(Action::Edit(Edit::Insert('!'))));
        let _ = app.update(Message::OpenPath(notes.clone()));
        assert!(app.autosaver.pending(), "its own file: nothing happens");
        assert!(app.notice.is_none());
        // Someone else changes the file, so the autosave is refused.
        std::fs::write(&notes, "theirs").unwrap();
        assert!(!app.save_before_leaving());
        assert!(app.disk_conflict);
        let _ = app.update(Message::OpenPath(dir.path().join("other.txt")));
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == notes));
        assert_eq!(app.pane.model.text(), "!# notes\n");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "theirs");
        assert!(app.notice.is_some());
    }

    #[test]
    fn a_number_opens_its_switcher_row_until_something_is_typed() {
        let (_dir, _paths, mut app, notes) = scratch_window("");
        std::fs::write(
            app.paths.data_dir.join("recent.txt"),
            notes.display().to_string(),
        )
        .unwrap();
        let _ = app.update(Message::OpenSwitcher);
        let _ = app.update(Message::SwitcherQuery("n".to_string()));
        let _ = app.update(Message::SwitcherQuery("n1".to_string()));
        assert!(matches!(app.doc, Doc::Scratch), "typed, so it searches");
        let _ = app.update(Message::SwitcherQuery(String::new()));
        let _ = app.update(Message::SwitcherQuery("1".to_string()));
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == notes));
        assert!(app.switcher.is_none());
        // Ctrl+number works with a search typed; row 0 is scratch.
        let _ = app.update(Message::OpenSwitcher);
        let _ = app.update(Message::SwitcherQuery("zzz".to_string()));
        let _ = app.update(Message::SwitcherPick(0));
        assert!(matches!(app.doc, Doc::Scratch));
    }

    #[test]
    fn ctrl_and_a_digit_picks_a_switcher_row_and_ctrl_0_is_scratch() {
        let press = |digit: &str, modifiers| {
            let event = iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Character(digit.into()),
                modified_key: keyboard::Key::Character(digit.into()),
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            });
            shortcut(event, event::Status::Ignored, window::Id::unique())
        };
        let command = keyboard::Modifiers::COMMAND;
        assert!(matches!(
            press("3", command),
            Some(Message::SwitcherPick(3))
        ));
        assert!(matches!(press("0", command), Some(Message::OpenScratch)));
        assert!(press("3", keyboard::Modifiers::empty()).is_none());
    }

    #[test]
    fn the_palette_runs_a_json_tool_and_ctrl_shift_p_opens_it() {
        let (dir, _paths, mut app, _notes) = scratch_window("");
        let data = dir.path().join("export.json");
        std::fs::write(&data, "{ \"a\": [1, 2] }").unwrap();
        let _ = app.update(Message::OpenPath(data));
        let _ = app.update(Message::OpenPalette);
        let _ = app.update(Message::SwitcherMove(1));
        let _ = app.update(Message::PaletteRun(app.palette.as_ref().unwrap().selected));
        assert_eq!(
            app.pane.model.text(),
            r#"{"a":[1,2]}"#,
            "Minify, the second tool"
        );
        assert!(app.palette.is_none());
        let event = iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character("P".into()),
            modified_key: keyboard::Key::Character("P".into()),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::COMMAND | keyboard::Modifiers::SHIFT,
            text: None,
            repeat: false,
        });
        let pressed = shortcut(event, event::Status::Ignored, window::Id::unique());
        assert!(matches!(pressed, Some(Message::OpenPalette)));
    }

    #[test]
    fn a_save_dialog_answered_after_a_switch_is_dropped() {
        // Save as asked in scratch, answered once notes.md has taken its place.
        let (dir, paths, mut app, notes) = scratch_window("from scratch");
        let asked = app.document;
        let _ = app.update(Message::OpenPath(notes.clone()));
        let picked = dir.path().join("picked.txt");
        let _ = app.update(Message::SaveAsPicked(asked, picked.clone()));
        assert!(!picked.exists());
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == notes));
        // Asked in notes.md, answered back in scratch: scratch isn't moved or emptied.
        let asked = app.document;
        let _ = app.update(Message::OpenScratch);
        let _ = app.update(Message::SaveAsPicked(asked, picked.clone()));
        let _ = app.update(Message::NewFilePicked(asked, picked.clone()));
        assert!(!picked.exists());
        assert!(matches!(app.doc, Doc::Scratch));
        assert_eq!(app.pane.model.text(), "from scratch");
        assert_eq!(
            ScratchState::load(&paths.data_dir).unwrap().text,
            "from scratch"
        );
        // An answer for the document shown still works.
        let _ = app.update(Message::SaveAsPicked(app.document, picked.clone()));
        assert_eq!(std::fs::read_to_string(&picked).unwrap(), "from scratch");
    }

    #[test]
    fn a_save_of_the_document_left_doesnt_touch_the_next_ones() {
        let (_dir, _paths, mut app, notes) = scratch_window("");
        let _ = app.update(Message::OpenPath(notes.clone()));
        let _ = app.update(Message::Edit(Action::Edit(Edit::Insert('!'))));
        // Someone else changes the file, so the save is refused, and no retry starts over it.
        std::fs::write(&notes, "theirs").unwrap();
        assert!(!app.save_before_leaving());
        let _ = app.update(Message::Tick(Instant::now() + Duration::from_secs(60)));
        assert!(!app.autosaver.is_saving());
        // A save running when Keep mine is chosen: Keep mine doesn't start a second one.
        let file = app.document;
        let running = app.autosaver.begin(Instant::now());
        let _ = app.update(Message::KeepMine);
        let refused = Err("Save failed: changed on disk".to_string());
        let _ = app.update(Message::Saved(file, running, refused));
        assert!(!app.disk_conflict && !app.autosaver.is_saving());
        let _ = app.update(Message::OpenScratch);
        assert!(matches!(app.doc, Doc::Scratch));
        // Typed in scratch, then a save of notes.md lands late: scratch's edits still count.
        for letter in "abcdefg".chars() {
            let _ = app.update(Message::Edit(Action::Edit(Edit::Insert(letter))));
        }
        // (notes.md's generations count its own edits, which may well be more.)
        let _ = app.update(Message::Saved(file, 100, Ok(None)));
        assert!(app.autosaver.pending());
        assert_eq!(app.autosaver.state(), SaveState::Saving);
    }

    #[test]
    fn a_pending_open_ends_with_a_close_or_the_shown_document() {
        let (_dir, _paths, mut app, notes) = scratch_window("a");
        // Waiting on a save, then closing, which fails: the window stays, and on scratch.
        let running = app.autosaver.begin(Instant::now());
        let _ = app.update(Message::OpenPath(notes.clone()));
        let _ = app.update(Message::CloseRequested);
        let _ = app.update(Message::Saved(
            0,
            running,
            Err("Save failed: disk full".into()),
        ));
        assert!(app.opening.is_none());
        let later = app.autosaver.begin(Instant::now());
        let _ = app.update(Message::Saved(0, later, Ok(None)));
        assert!(matches!(app.doc, Doc::Scratch));
        // Waiting on a save, then choosing scratch, which the window shows: nothing opens.
        let running = app.autosaver.begin(Instant::now());
        let _ = app.update(Message::OpenPath(notes));
        let _ = app.update(Message::OpenScratch);
        let _ = app.update(Message::Saved(0, running, Ok(None)));
        assert!(matches!(app.doc, Doc::Scratch));
    }

    #[test]
    fn leaving_a_file_whose_last_save_failed_saves_it_again() {
        let (_dir, _paths, mut app, notes) = scratch_window("");
        let _ = app.update(Message::OpenPath(notes.clone()));
        let _ = app.update(Message::Edit(Action::Edit(Edit::Insert('!'))));
        // Two saves of the same text end out of order: the one that failed comes back last,
        // so nothing is pending but the last save failed (and the file on disk lacks the "!").
        let document = app.document;
        let first = app.autosaver.begin(Instant::now());
        let second = app.autosaver.begin(Instant::now());
        let _ = app.update(Message::Saved(document, first, Ok(None)));
        let _ = app.update(Message::Saved(
            document,
            second,
            Err("Save failed: disk full".into()),
        ));
        assert!(!app.autosaver.pending() && !app.autosaver.close_allowed());
        let _ = app.update(Message::OpenScratch);
        assert!(matches!(app.doc, Doc::Scratch));
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "!# notes\r\n");
    }

    #[test]
    fn a_file_that_cant_be_opened_leaves_scratch_as_it_was() {
        let (dir, _paths, mut app, _notes) = scratch_window("mine");
        let binary = dir.path().join("binary.txt");
        std::fs::write(&binary, [0xff, 0xfe]).unwrap();
        let _ = app.update(Message::OpenPath(binary));
        assert!(matches!(app.doc, Doc::Scratch));
        assert_eq!(app.pane.model.text(), "mine");
        let notice = app.notice.as_ref().map(|(words, _)| words.as_str());
        assert_eq!(notice, Some("Couldn't open: not a UTF-8 text file"));
    }
}
