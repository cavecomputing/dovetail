//! The App: its state, the Message enum, `update` (the only place state changes) and `view`.
//! One process shows one document in one window; Settings opens over it.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use iced::widget::{column, container, responsive, row, space, stack};
use iced::window::{self, Direction};
use iced::{Element, Fill, Size, Subscription, Task, Theme, event, keyboard};

use crate::core::config::yaml_indent;
use crate::core::counts::Counts;
use crate::core::json::reformat;
use crate::core::opened::{Doc, OpenedDoc, open_doc, write_doc};
use crate::core::paths::Paths;
use crate::core::recent::existing_recent_files;
use crate::core::rewrite::draft_path;
use crate::core::save::{Autosaver, SaveState};
use crate::core::scratch::ScratchView;
use crate::core::settings::{ReadingMode, ScratchSyntax, Settings, Theme as ThemeSetting};
use crate::core::shutdown::{self, Unsaved};
use crate::core::syntax::Syntax;
use crate::core::text_file::{DiskStamp, changed_on_disk, read_text};
use crate::core::typing::is_code;
use crate::ui::assistant_actions::{Assistant, AssistantAction, CardState};
use crate::ui::editor::{Decision, EDITOR_ID, ORIGINAL_ID, Pane};
use crate::ui::find_bar::{self, Find, FindAction};
use crate::ui::palette::{self, Palette};
use crate::ui::picker::{self, Picker, PickerAction};
use crate::ui::settings_panel::{self, Connection, KeyField};
use crate::ui::switcher::{self, Switcher};
use crate::ui::text_editor::{Action, Cursor, Edit, Motion, Position};
use crate::ui::theme::{self, Roles};
#[cfg(all(unix, not(target_os = "macos")))]
use crate::ui::wayland_drop;
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
    /// The text being edited. While rewriting, the draft.
    pub pane: Pane,
    /// While rewriting (Ctrl+R): the original, read-only on the left of the draft.
    pub original: Option<Pane>,
    /// Ctrl+R to act on once the save running now ends (see `toggle_rewrite`).
    pub toggling_rewrite: bool,
    /// The Ctrl+P switcher, while it is open.
    pub switcher: Option<Switcher>,
    /// The Ctrl+O picker, while it is open.
    pub picker: Option<Picker>,
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

    /// The text's word and token counts. They lag behind typing (see `COUNT_PAUSE`).
    pub counts: Counts,
    /// When the latest edit the counts don't include yet was made.
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
    /// Ctrl+Shift+Enter in the editor: in a Markdown file, tick the lines' task boxes, untick them,
    /// or give the lines boxes (`Pane::toggle_tasks`).
    ToggleTasks,
    Undo,
    Redo,
    /// Ctrl+R and the pen button: start a rewrite, or end the one under way.
    ToggleRewrite,
    /// Scrolling, selecting and the like in the original, read-only beside a rewrite's draft.
    Original(Action),
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
    /// The counts of the document with this number (`App::document`).
    Counted(u64, Counts),
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
    /// ↑ or ↓: moves the highlight in the picker, the switcher or the palette, whichever is open.
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

    /// The settings as the Settings panel (or a shortcut) just changed them.
    Setting(Settings),
    /// Ctrl+= and Ctrl+-: the font a size bigger or smaller.
    Zoom(f32),
    /// Ctrl+Shift+L and the menu: line numbers on or off.
    ToggleLineNumbers,
    /// Once a second: rereads settings.toml if it changed, and takes down an old notice.
    PollSettings,
    SystemTheme(iced::theme::Mode),

    Assistant(AssistantAction),
    Picker(PickerAction),
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

/// How long typing must pause before the counts are redone. Counting a long text takes a while
/// (about 2 ms for 1 MB), too long to do on every key.
const COUNT_PAUSE: Duration = Duration::from_millis(300);

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

        let mut app = App {
            settings_modified: modified_time(&paths),
            recent: existing_recent_files(&paths.data_dir),
            counts: Counts::of(&opened.text),
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
            original: None,
            toggling_rewrite: false,
            switcher: None,
            picker: None,
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
        app.resume_rewrite();
        let tasks = Task::batch([
            app.check_syntax(),
            open.map(|_| Message::WindowOpened),
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
        self.counts = Counts::of(&opened.text);
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
        self.assistant.new_chat();
        self.syntax_problem = None;
        self.toggling_rewrite = false;
        self.resume_rewrite();
        let focus = iced::widget::operation::focus(editor::EDITOR_ID);
        Task::batch([self.count(), self.check_syntax(), focus])
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
            Message::Edit(action) => {
                let edited = self.pane.perform(action);
                self.after_edit(edited);
            }
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
            | Message::ToggleTasks => {
                if !self.editable() {
                    return Task::none();
                }
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
                    Message::ToggleTasks if markdown => self.pane.toggle_tasks(),
                    Message::ToggleComment => match syntax.comment_marker() {
                        Some(marker) => self.pane.toggle_comment(marker),
                        None => Ok(false),
                    },
                    // Ctrl+B, Ctrl+I and Ctrl+Shift+X outside Markdown do nothing, and the arm's
                    // pattern lets no other message in.
                    _ => Ok(false),
                };
                self.after_edit(edited);
            }
            Message::Undo | Message::Redo => {
                if !self.editable() {
                    return Task::none();
                }
                let changed = match message {
                    Message::Undo => self.pane.undo(),
                    _ => self.pane.redo(),
                };
                if changed {
                    self.text_changed();
                }
            }
            Message::Decide(decision) => self.decide(decision),
            Message::ToggleRewrite => return self.toggle_rewrite(),
            Message::Original(action) => {
                if let Some(original) = &mut self.original
                    && !action.is_edit()
                {
                    let _ = original.perform(action);
                }
            }
            Message::Tick(now) => {
                self.now = now;
                let mut tasks = Vec::new();
                if self
                    .uncounted_edit
                    .is_some_and(|edited| now.duration_since(edited) >= COUNT_PAUSE)
                {
                    self.uncounted_edit = None;
                    tasks.push(self.count());
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
                // A rewrite's draft has a file of its own, not the document's.
                let rewriting = self.original.is_some();
                if let Ok(Some(stamp)) = result
                    && !rewriting
                {
                    self.disk_stamp = Some(stamp);
                }
                self.autosaver.finish(generation, result.map(|_| ()));
                if self.autosaver.state() == SaveState::Saved {
                    self.saving_since = None;
                    shutdown::forget();
                } else {
                    // Still unsaved, but as of the file this save wrote.
                    self.remember_unsaved();
                }
                self.disk_conflict = match &self.doc {
                    Doc::File { path, .. } if failed && !rewriting => {
                        changed_on_disk(path, self.disk_stamp)
                    }
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
                } else if std::mem::take(&mut self.toggling_rewrite) {
                    return self.toggle_rewrite();
                }
            }
            Message::Counted(document, counts) if document == self.document => self.counts = counts,
            Message::Counted(..) => {}
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
                // Under Wayland, files dropped on the window come through the clipboard crate.
                // Elsewhere winit hears them (`shortcut`).
                #[cfg(all(unix, not(target_os = "macos")))]
                let focus = Task::batch([focus, wayland_drop::listen()]);
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
                self.toggling_rewrite = false;
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
                let open = !self.menu_open;
                self.close_panels();
                self.menu_open = open;
                self.recent = existing_recent_files(&self.paths.data_dir);
            }
            // Esc, or a click outside: closes the menu, the picker, the switcher, the palette and
            // Settings. With none of them open, Esc closes the find bar.
            Message::CloseMenu => {
                let closed = self.close_panels() | std::mem::take(&mut self.settings_open);
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
            Message::OpenDialog => return self.open_picker(),
            Message::Picker(action) => return self.update_picker(action),
            Message::SwitcherMove(step) if self.picker.is_some() => {
                if let Some(picker) = &mut self.picker {
                    picker.move_selection(step);
                }
            }
            Message::FocusNext { .. } if self.picker.is_some() => {
                return self.update_picker(PickerAction::Complete);
            }
            Message::OpenPath(path) => {
                // A file dropped on Settings opens behind it, so Settings closes to show it.
                self.settings_open = false;
                return self.open_file(path);
            }
            Message::OpenSettings => {
                self.close_panels();
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
                            shutdown::forget();
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

            Message::Setting(mut settings) => {
                settings.clamp();
                if let ReadingMode::Comfort { chars } = settings.reading_mode {
                    self.last_comfort_chars = chars;
                }
                self.settings = settings;
                // A test result is about the settings it ran with.
                self.connection = None;
                self.save_settings();
                self.settings_changed();
            }
            Message::Zoom(step) => {
                let mut settings = self.settings.clone();
                settings.font_size += step;
                return self.update(Message::Setting(settings));
            }
            Message::ToggleLineNumbers => {
                self.menu_open = false;
                let mut settings = self.settings.clone();
                settings.line_numbers = !settings.line_numbers;
                return self.update(Message::Setting(settings));
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

    pub fn text_changed(&mut self) {
        let now = Instant::now();
        self.autosaver.edited(now);
        self.remember_unsaved();
        self.saving_since.get_or_insert(now);
        self.uncounted_edit = Some(now);
        if self.find.open {
            self.find_again(self.pane.caret());
        }
        // Undo, redo and a reload can drop a suggestion the change touched.
        self.settle_cards(CardState::Gone(
            "an edit to the text under it dropped it".to_string(),
        ));
    }

    /// Accepts or rejects suggestions (from the chat, Ctrl+Enter or Ctrl+Backspace, or new chat),
    /// and marks their cards.
    pub fn decide(&mut self, decision: Decision) {
        if !self.editable() {
            return;
        }
        let changed = self.pane.decide(decision);
        self.settle_cards(match decision {
            Decision::AcceptAll => CardState::Accepted,
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

    /// Counts the text's words and tokens in the background, as it takes a while on a long text;
    /// `Counted` brings the numbers back.
    fn count(&self) -> Task<Message> {
        let text = self.pane.model.text().to_string();
        let document = self.document;
        Task::perform(async move { Counts::of(&text) }, move |counts| {
            Message::Counted(document, counts)
        })
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
        let (doc, disk_stamp) = self.saved_to();
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
        let (doc, disk_stamp) = self.saved_to();
        let result = write_doc(&doc, text, view, &self.paths.data_dir, disk_stamp);
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

    /// Closes the menu, the picker, the switcher and the palette, so opening one never leaves
    /// another under it taking ↑ and ↓. True if any was open.
    pub fn close_panels(&mut self) -> bool {
        let open = self.menu_open
            || self.picker.is_some()
            || self.switcher.is_some()
            || self.palette.is_some();
        self.menu_open = false;
        self.picker = None;
        self.switcher = None;
        self.palette = None;
        open
    }

    /// Hands the text autosave hasn't written yet to `shutdown`, for logging out and the like.
    fn remember_unsaved(&self) {
        let (doc, disk_stamp) = self.saved_to();
        shutdown::remember(Unsaved {
            doc,
            text: self.pane.model.text().to_string(),
            view: self.scratch_view(),
            data_dir: self.paths.data_dir.clone(),
            disk_stamp,
        });
    }

    /// Where autosave writes the text, and the file on disk as last seen there: the document, or
    /// while rewriting the draft's own file (which nothing else writes, so it has no stamp).
    fn saved_to(&self) -> (Doc, Option<DiskStamp>) {
        match self.original {
            Some(_) => (Doc::file(draft_path(&self.paths.data_dir, &self.doc)), None),
            None => (self.doc.clone(), self.disk_stamp),
        }
    }

    pub fn scratch_view(&self) -> ScratchView {
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
        // counts' pause, and animate the dot.
        if self.autosaver.state() != SaveState::Saved || self.uncounted_edit.is_some() {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(Message::Tick));
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
            self.picker.as_ref().map(picker::view),
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
        let syntax = self.syntax();
        let editable = self.editable();
        let draft = editor::view(
            &self.pane,
            &self.settings,
            editable,
            syntax,
            paint,
            EDITOR_ID,
        );
        let Some(original) = &self.original else {
            return draft;
        };
        // The original only scrolls and selects, so its actions go to it, not the draft.
        let original = editor::view(
            original,
            &self.settings,
            false,
            syntax,
            Vec::new(),
            ORIGINAL_ID,
        )
        .map(|message| match message {
            Message::Edit(action) => Message::Original(action),
            other => other,
        });
        row![
            container(original).width(Fill),
            assistant_view::divider(),
            container(draft).width(Fill),
        ]
        .into()
    }

    /// The kind of text the document is, by its file's extension (or a script's `#!` line):
    /// coloured to match, and for a data file (JSON, YAML, TOML, INI) or a shell script checked
    /// as you type. Scratch is plain or Markdown, as Settings says.
    pub fn syntax(&self) -> Syntax {
        match &self.doc {
            Doc::File { path, .. } => Syntax::of(path, self.pane.model.text()),
            Doc::Scratch => match self.settings.scratch_syntax {
                ScratchSyntax::Plain => Syntax::Plain,
                ScratchSyntax::Markdown => Syntax::Markdown,
            },
        }
    }
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

/// App-wide keys. Up and Down move through the picker's, the switcher's or the palette's list
/// (and in the picker ← and → go up a folder and into one), Enter steps through the find bar's
/// matches (each ignored while its panel is closed), and Tab outside the editor moves between
/// text boxes.
fn shortcut(event: iced::Event, status: event::Status, _window: window::Id) -> Option<Message> {
    let (key, modifiers) = match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            (key, modifiers)
        }
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            return Some(Message::Modifiers(modifiers));
        }
        // A file dropped on the window opens in it, as Ctrl+O would (X11, macOS and Windows;
        // Wayland's drops come from `wayland_drop`).
        iced::Event::Window(window::Event::FileDropped(path)) => {
            return Some(Message::OpenPath(path));
        }
        _ => return None,
    };
    // Ctrl+Enter accepts every suggested edit, Ctrl+Backspace rejects the current one, and
    // Ctrl+Shift+Backspace rejects them all. The editor and the message box leave these keys
    // alone while any wait. Ctrl+Shift+Enter is the editor's (Markdown task boxes).
    if modifiers.command() && status == event::Status::Ignored {
        let all = modifiers.shift();
        let decision = match key.as_ref() {
            keyboard::Key::Named(keyboard::key::Named::Enter) if !all => Some(Decision::AcceptAll),
            keyboard::Key::Named(keyboard::key::Named::Backspace) if all => {
                Some(Decision::RejectAll)
            }
            keyboard::Key::Named(keyboard::key::Named::Backspace) => Some(Decision::Reject(None)),
            _ => None,
        };
        if let Some(decision) = decision {
            return Some(Message::Decide(decision));
        }
    }
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
        // The Ctrl+O picker's way in and out of folders; with no picker open they do nothing.
        keyboard::Key::Named(keyboard::key::Named::ArrowRight) if modifiers.is_empty() => {
            return Some(Message::Picker(PickerAction::Into));
        }
        keyboard::Key::Named(keyboard::key::Named::ArrowLeft) if modifiers.is_empty() => {
            return Some(Message::Picker(PickerAction::Up));
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
        keyboard::Key::Character("r") => Some(Message::ToggleRewrite),
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

/// The keys that still work while Settings is open: Esc and Ctrl+, close it, Ctrl+W closes the
/// window and ⌘M (macOS) minimizes it, and the zoom keys and Ctrl+Shift+L change a setting the
/// panel shows. The rest would act on the document behind it.
fn settings_key(message: Message) -> Option<Message> {
    match message {
        Message::OpenSettings => Some(Message::CloseMenu),
        Message::CloseMenu
        | Message::CloseRequested
        | Message::Minimize
        | Message::Zoom(_)
        | Message::ToggleLineNumbers
        | Message::OpenPath(_)
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
    use crate::core::rewrite::read_draft;
    use crate::core::scratch::{ScratchState, scratch_open_elsewhere};
    use crate::core::text_file::LineEnding;
    use crate::ui::text_editor::Content;
    use iced::widget::text_editor::Motion;
    use std::path::Path;

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

    /// What `shortcut` makes of a press of `key` (`modified_key` with Shift's change) that no
    /// widget took.
    fn shortcut_for(
        key: &str,
        modified_key: &str,
        modifiers: keyboard::Modifiers,
    ) -> Option<Message> {
        use keyboard::key::{NativeCode, Physical};
        let event = iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(key.into()),
            modified_key: keyboard::Key::Character(modified_key.into()),
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        });
        shortcut(event, event::Status::Ignored, window::Id::unique())
    }

    #[test]
    fn ctrl_shift_l_toggles_line_numbers_and_ctrl_l_the_assistant() {
        // iced gives the key without Shift's change ("l"), and `modified_key` with it ("L").
        let press = |modifiers| shortcut_for("l", "L", modifiers);
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

    #[test]
    fn ctrl_comma_opens_settings_and_closes_them_again() {
        let press = shortcut_for(",", ",", keyboard::Modifiers::COMMAND);
        assert!(matches!(press, Some(Message::OpenSettings)));
        assert!(matches!(
            press.and_then(settings_key),
            Some(Message::CloseMenu)
        ));
    }

    fn file_doc(path: &Path) -> Doc {
        Doc::file(path.to_path_buf())
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
    fn a_file_dropped_on_settings_opens_and_closes_settings() {
        let (_dir, _paths, mut app, notes) = scratch_window("");
        let _ = app.update(Message::OpenSettings);
        let dropped = shortcut(
            iced::Event::Window(window::Event::FileDropped(notes.clone())),
            event::Status::Ignored,
            app.main_window,
        );
        let message = dropped
            .and_then(settings_key)
            .expect("a drop gets past Settings");
        let _ = app.update(message);
        assert!(!app.settings_open);
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == notes));
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
        let press = |digit, modifiers| shortcut_for(digit, digit, modifiers);
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
        let shift = keyboard::Modifiers::COMMAND | keyboard::Modifiers::SHIFT;
        let pressed = shortcut_for("P", "P", shift);
        assert!(matches!(pressed, Some(Message::OpenPalette)));
    }

    #[test]
    fn opening_a_panel_closes_the_one_under_it() {
        let (_dir, _paths, mut app, _notes) = scratch_window("");
        let _ = app.update(Message::OpenDialog);
        let _ = app.update(Message::OpenSwitcher);
        assert!(app.picker.is_none() && app.switcher.is_some());
        let _ = app.update(Message::OpenPalette);
        assert!(app.switcher.is_none() && app.palette.is_some());
        let _ = app.update(Message::OpenDialog);
        assert!(app.palette.is_none() && app.picker.is_some());
        let _ = app.update(Message::OpenSettings);
        assert!(app.picker.is_none() && app.settings_open);
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

    /// A window on notes.md with a rewrite started, after `typed` was typed into the original.
    fn rewriting_notes(typed: &str) -> (tempfile::TempDir, Paths, App, PathBuf) {
        let (dir, paths, mut app, notes) = scratch_window("");
        let _ = app.update(Message::OpenPath(notes.clone()));
        for letter in typed.chars() {
            let _ = app.update(Message::Edit(Action::Edit(Edit::Insert(letter))));
        }
        let _ = app.update(Message::ToggleRewrite);
        (dir, paths, app, notes)
    }

    fn type_in(app: &mut App, typed: &str) {
        for letter in typed.chars() {
            let _ = app.update(Message::Edit(Action::Edit(Edit::Insert(letter))));
        }
    }

    #[test]
    fn a_rewrite_saves_to_its_draft_until_ctrl_s() {
        // Typed before the rewrite, inside the autosave pause: that goes to the file first.
        let (_dir, paths, mut app, notes) = rewriting_notes("1");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "1# notes\r\n");
        type_in(&mut app, "2");
        assert!(app.save_before_leaving());
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "1# notes\r\n");
        let draft = read_draft(&paths.data_dir, &app.doc);
        assert_eq!(draft.as_deref(), Some("12# notes\n"));
        // Ctrl+S writes it over the original, with the file's line endings, and the left shows it.
        let _ = app.update(Message::Save);
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "12# notes\r\n");
        assert_eq!(app.original.as_ref().unwrap().model.text(), "12# notes\n");
        // The original only scrolls and selects.
        let _ = app.update(Message::Original(Action::Edit(Edit::Insert('x'))));
        assert_eq!(app.original.as_ref().unwrap().model.text(), "12# notes\n");
    }

    #[test]
    fn ending_a_rewrite_drops_what_ctrl_s_didnt_save_and_ctrl_z_brings_it_back() {
        let (_dir, paths, mut app, notes) = rewriting_notes("");
        type_in(&mut app, "new ");
        let _ = app.update(Message::ToggleRewrite);
        assert!(app.original.is_none());
        assert_eq!(app.pane.model.text(), "# notes\n");
        assert_eq!(read_draft(&paths.data_dir, &app.doc), None);
        assert!(!app.autosaver.pending(), "the file already holds it");
        let _ = app.update(Message::Undo);
        assert_eq!(app.pane.model.text(), "new # notes\n");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# notes\r\n");
    }

    #[test]
    fn a_rewrite_left_open_comes_back_with_its_document() {
        let (_dir, paths, mut app, notes) = rewriting_notes("");
        type_in(&mut app, "draft ");
        let _ = app.update(Message::OpenScratch);
        assert!(app.original.is_none());
        let _ = app.update(Message::OpenPath(notes.clone()));
        assert_eq!(app.pane.model.text(), "draft # notes\n");
        assert_eq!(app.original.as_ref().unwrap().model.text(), "# notes\n");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# notes\r\n");
        // Another window can't edit it, so it shows the file and leaves the draft alone.
        let (other, _) = App::with_paths(file_doc(&notes), paths.clone());
        assert!(other.original.is_none());
        assert!(read_draft(&paths.data_dir, &app.doc).is_some());
    }

    #[test]
    fn save_as_ends_a_rewrite_with_the_draft_in_the_new_file() {
        let (dir, paths, mut app, notes) = rewriting_notes("");
        type_in(&mut app, "draft ");
        let old = app.doc.clone();
        let copy = dir.path().join("copy.md");
        let _ = app.update(Message::SaveAsPicked(app.document, copy.clone()));
        assert!(app.original.is_none());
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "draft # notes\r\n");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# notes\r\n");
        assert_eq!(read_draft(&paths.data_dir, &old), None);
    }

    #[test]
    fn ctrl_r_toggles_a_rewrite() {
        let press = shortcut_for("r", "r", keyboard::Modifiers::COMMAND);
        assert!(matches!(press, Some(Message::ToggleRewrite)));
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
