//! The folder pane left of the document (Ctrl+Shift+E), drawn like the assistant pane: a `#` line
//! naming the folder, a `>` filter, then the folder as a tree. Folders open and close in place
//! and a file opens in this window. Typing in the filter lists every file under the folder whose
//! path matches instead.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use iced::keyboard::{Key, key::Named};
use iced::widget::operation::{RelativeOffset, focus, snap_to};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{self, Binding, KeyPress, Status};
use iced::widget::{button, column, container, rich_text, row, scrollable, space, span};
use iced::{Background, Border, Color, Element, Fill, Font, Padding, Task, Theme};

use crate::core::folder::{Entry, Row, files_under, matching, tree};
use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::recent::{existing_recent_files, remember};
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{CH, LINE, SIZE, fg_muted, line, subtle, word};
use crate::ui::editor;
use crate::ui::status_bar::keys;
use crate::ui::theme::{GLYPH_CLOSE, Roles};

pub const PANE_WIDTH: f32 = 260.0;
const FILTER_ID: &str = "folder-filter";
const LIST_ID: &str = "folder-list";

pub struct FolderPane {
    pub root: PathBuf,
    /// Shown; Ctrl+Shift+E hides the pane and keeps the folder.
    pub open: bool,
    /// The folders open in the tree.
    expanded: BTreeSet<PathBuf>,
    /// The tree as last read from disk.
    tree: Vec<Row>,
    filter: text_editor::Content,
    /// Every file under the folder, relative to it, once read for the filter; and whether there
    /// were more than it reads.
    files: Option<(Vec<PathBuf>, bool)>,
    /// The highlighted row, which Enter opens.
    selected: usize,
}

#[derive(Debug, Clone)]
pub enum FolderAction {
    /// Ctrl+Shift+E: show or hide the pane, opening the document's own folder when none is open.
    Toggle,
    /// Shows this folder in the pane (from Ctrl+O, Ctrl+P, Recent or the command line).
    Open(PathBuf),
    /// The close glyph: puts the folder away.
    Close,
    /// A row clicked: a folder opens or closes in the tree, a file opens in the window.
    Pick(usize),
    Filter(text_editor::Action),
    /// ↑ or ↓ in the filter.
    Move(i32),
    /// Enter in the filter: the highlighted row, as `Pick`.
    Choose,
    /// Esc in the filter: clears it, or back to the document when it's empty.
    Escape,
    /// The files under `root`, read off the UI thread.
    FilesRead(PathBuf, (Vec<PathBuf>, bool)),
}

impl FolderPane {
    fn new(root: PathBuf) -> FolderPane {
        let mut pane = FolderPane {
            root,
            open: true,
            expanded: BTreeSet::new(),
            tree: Vec::new(),
            filter: text_editor::Content::new(),
            files: None,
            selected: 0,
        };
        pane.refresh(None);
        pane
    }

    /// Reads the tree from disk again, and highlights `current` (the open file) if it shows.
    pub fn refresh(&mut self, current: Option<&Path>) {
        self.tree = tree(&self.root, &self.expanded);
        if let Some(at) = self
            .tree
            .iter()
            .position(|row| Some(row.entry.path.as_path()) == current)
        {
            self.selected = at;
        }
    }

    fn query(&self) -> String {
        self.filter.text().trim().to_string()
    }

    /// The rows shown: the tree, or with a filter typed, the files matching it.
    fn rows(&self) -> Vec<Row> {
        let query = self.query();
        if query.is_empty() {
            return self.tree.clone();
        }
        let files = self.files.as_ref().map_or(&[][..], |(files, _)| files);
        matching(files, &query)
            .into_iter()
            .map(|relative| Row {
                entry: Entry {
                    path: self.root.join(relative),
                    is_dir: false,
                },
                depth: 0,
            })
            .collect()
    }
}

impl App {
    pub fn update_folder(&mut self, action: FolderAction) -> Task<Message> {
        match action {
            FolderAction::Toggle => {
                let Some(pane) = &mut self.folder else {
                    let root = match &self.doc {
                        Doc::File { path, .. } => path.parent().map(Path::to_path_buf),
                        Doc::Scratch => {
                            directories::BaseDirs::new().map(|base| base.home_dir().to_path_buf())
                        }
                    };
                    return match root {
                        Some(root) => self.update_folder(FolderAction::Open(root)),
                        None => Task::none(),
                    };
                };
                pane.open = !pane.open;
                if !pane.open {
                    return focus(editor::EDITOR_ID);
                }
                pane.refresh(current_file(&self.doc));
                let root = pane.root.clone();
                return Task::batch([focus(FILTER_ID), read_files(root)]);
            }
            FolderAction::Open(root) => {
                let _ = remember(&self.paths.data_dir, &root);
                self.recent = existing_recent_files(&self.paths.data_dir);
                self.folder = Some(FolderPane::new(root.clone()));
                return Task::batch([focus(FILTER_ID), read_files(root)]);
            }
            FolderAction::Close => {
                self.folder = None;
                return focus(editor::EDITOR_ID);
            }
            FolderAction::FilesRead(root, files) => {
                if let Some(pane) = self.folder.as_mut().filter(|pane| pane.root == root) {
                    pane.files = Some(files);
                }
            }
            FolderAction::Filter(action) => {
                let Some(pane) = &mut self.folder else {
                    return Task::none();
                };
                // One line only: Enter chooses (see `filter_keys`), so a pasted line break goes.
                pane.filter.perform(action);
                let text = pane.filter.text();
                if text.contains('\n') {
                    pane.filter = text_editor::Content::with_text(&text.replace('\n', ""));
                    pane.filter
                        .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
                }
                pane.selected = 0;
                return snap_to(LIST_ID, RelativeOffset::START);
            }
            FolderAction::Move(step) => {
                let Some(pane) = &mut self.folder else {
                    return Task::none();
                };
                let last = pane.rows().len().saturating_sub(1);
                pane.selected = pane.selected.saturating_add_signed(step as isize).min(last);
                let y = pane.selected as f32 / last.max(1) as f32;
                return snap_to(LIST_ID, RelativeOffset { x: 0.0, y });
            }
            FolderAction::Choose => {
                if let Some(pane) = &self.folder {
                    return self.update_folder(FolderAction::Pick(pane.selected));
                }
            }
            FolderAction::Escape => {
                let Some(pane) = &mut self.folder else {
                    return Task::none();
                };
                if pane.query().is_empty() {
                    return focus(editor::EDITOR_ID);
                }
                pane.filter = text_editor::Content::new();
                pane.selected = 0;
            }
            FolderAction::Pick(index) => {
                let Some(pane) = &mut self.folder else {
                    return Task::none();
                };
                let Some(row) = pane.rows().into_iter().nth(index) else {
                    return Task::none();
                };
                pane.selected = index;
                if row.entry.is_dir {
                    if !pane.expanded.remove(&row.entry.path) {
                        pane.expanded.insert(row.entry.path);
                    }
                    pane.refresh(None);
                    return Task::none();
                }
                // A file found by the filter clears it, back to the tree.
                if !pane.query().is_empty() {
                    pane.filter = text_editor::Content::new();
                    pane.selected = 0;
                }
                let open = self.open_file(row.entry.path);
                return Task::batch([open, focus(editor::EDITOR_ID)]);
            }
        }
        Task::none()
    }
}

/// The open file's path; scratch has none.
pub fn current_file(doc: &Doc) -> Option<&Path> {
    match doc {
        Doc::File { path, .. } => Some(path),
        Doc::Scratch => None,
    }
}

/// Reads every file under `root` off the UI thread, for the filter.
fn read_files(root: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            let files = files_under(&root);
            (root, files)
        },
        |(root, files)| Message::Folder(FolderAction::FilesRead(root, files)),
    )
}

/// The pane `width` pixels wide, or an empty slot while it's hidden, so the document keeps its
/// place in the widget tree either way.
pub fn view(app: &App, width: f32) -> Element<'_, Message> {
    let Some(pane) = app.folder.as_ref().filter(|pane| pane.open) else {
        return space().width(0).into();
    };
    let close = word(
        GLYPH_CLOSE,
        fg_muted,
        Some(Message::Folder(FolderAction::Close)),
    );
    // The end of a long path, which names the folder, is what's kept.
    let path = display_path(&pane.root);
    let room = ((PANE_WIDTH.min(width * 0.4) - 32.0) / CH) as usize - 5;
    let skip = path.chars().count().saturating_sub(room);
    let path = match skip {
        0 => path,
        _ => format!("…{}", path.chars().skip(skip + 1).collect::<String>()),
    };
    let head = row![
        line(format!("# {path}"), subtle)
            .wrapping(Wrapping::None)
            .width(Fill),
        close,
    ];
    let filter = iced::widget::text_editor(&pane.filter)
        .id(FILTER_ID)
        .placeholder("filter")
        .on_action(|action| Message::Folder(FolderAction::Filter(action)))
        .key_binding(filter_keys)
        .size(SIZE)
        .line_height(LINE)
        .padding(0)
        .style(|theme: &Theme, _status| {
            let roles = Roles::of(theme);
            text_editor::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default(),
                placeholder: roles.fg_subtle,
                value: roles.fg,
                selection: roles.accent_cool.scale_alpha(0.35),
            }
        });
    let prompt = row![line(">", |roles| roles.accent_text), filter].spacing(CH);

    let current = current_file(&app.doc);
    let rows = pane.rows();
    let filtering = !pane.query().is_empty();
    let quiet = Roles::of(&app.theme).fg_subtle;
    let mut list = column![];
    for (index, row) in rows.iter().enumerate() {
        let open = current == Some(row.entry.path.as_path());
        list = list.push(row_view(pane, row, index, filtering, open, quiet));
    }
    let foot = match (filtering, &pane.files) {
        (false, _) => keys("# Ctrl+Shift+E hides"),
        (true, None) => "# reading the folder…".to_string(),
        (true, Some((files, more))) => {
            let plus = if *more { "+" } else { "" };
            format!("# {} of {}{plus} · ↑↓ and Enter", rows.len(), files.len())
        }
    };
    let pad = |top: f32, bottom: f32| Padding {
        top,
        right: 16.0,
        bottom,
        left: 16.0,
    };
    let content = column![
        container(column![head, prompt].spacing(SIZE * LINE * 0.5)).padding(pad(14.0, 8.0)),
        scrollable(list).id(LIST_ID).height(Fill),
        container(line(foot, subtle)).padding(pad(8.0, 12.0)),
    ];
    let divider = container(space().width(1).height(Fill))
        .style(|theme: &Theme| crate::ui::theme::fill(Roles::of(theme).border));
    row![
        container(content)
            .width(PANE_WIDTH.min(width * 0.4))
            .clip(true),
        divider
    ]
    .into()
}

/// One row: indented by its depth, a folder with ▸ or ▾ and a trailing `/` in blue, a file in
/// the text colour, the open file in the accent. While filtering, a file's folder shows in
/// `quiet` before its name.
fn row_view<'a>(
    pane: &FolderPane,
    row: &Row,
    index: usize,
    filtering: bool,
    current: bool,
    quiet: Color,
) -> Element<'a, Message> {
    let name = row.entry.name();
    let indent = "  ".repeat(row.depth);
    let (lead, name) = match (filtering, row.entry.is_dir) {
        (true, _) => {
            let relative = row
                .entry
                .path
                .strip_prefix(&pane.root)
                .unwrap_or(&row.entry.path);
            let folder = relative
                .parent()
                .map(|folder| folder.to_string_lossy().into_owned())
                .filter(|folder| !folder.is_empty())
                .map(|folder| format!("{folder}/"))
                .unwrap_or_default();
            (folder, name)
        }
        (false, true) => {
            let arrow = if pane.expanded.contains(&row.entry.path) {
                "▾ "
            } else {
                "▸ "
            };
            (format!("{indent}{arrow}"), format!("{name}/"))
        }
        (false, false) => (format!("{indent}  "), name),
    };
    let is_dir = row.entry.is_dir;
    let words = rich_text([span::<(), Font>(lead).color(quiet), span(name)])
        .size(SIZE)
        .line_height(LINE)
        .wrapping(Wrapping::None);
    let selected = index == pane.selected;
    button(words)
        .width(Fill)
        .padding(Padding {
            top: 0.0,
            right: 16.0,
            bottom: 0.0,
            left: 16.0,
        })
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: (selected || hovered).then_some(Background::Color(roles.bg_raised)),
                text_color: if current {
                    roles.accent_text
                } else if is_dir {
                    roles.accent_cool
                } else {
                    roles.fg
                },
                border: Border::default(),
                ..button::Style::default()
            }
        })
        .on_press(Message::Folder(FolderAction::Pick(index)))
        .into()
}

/// ↑ and ↓ move the highlight, Enter opens it, Esc clears the filter; Ctrl shortcuts are the
/// app's, never typing.
fn filter_keys(press: KeyPress) -> Option<Binding<Message>> {
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    let folder = |action| Some(Binding::Custom(Message::Folder(action)));
    match press.key.as_ref() {
        Key::Named(Named::ArrowUp) => return folder(FolderAction::Move(-1)),
        Key::Named(Named::ArrowDown) => return folder(FolderAction::Move(1)),
        Key::Named(Named::Enter) => return folder(FolderAction::Choose),
        Key::Named(Named::Escape) => return folder(FolderAction::Escape),
        _ => {}
    }
    let command = press.modifiers.command();
    match Binding::from_key_press(press) {
        Some(Binding::Insert(_)) if command => None,
        binding => binding,
    }
}
