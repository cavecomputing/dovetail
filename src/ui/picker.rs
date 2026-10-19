//! The Ctrl+O picker: a box holding a path, starting at the open file's folder, and what's in
//! that folder below it. Typing filters the folder by name; Tab, Enter or a click goes into the
//! highlighted folder (`..` is the one above), and Enter or a click opens the highlighted file in
//! this window. → goes into the highlighted folder and ← up to the one above, so the arrows alone
//! get around.

use std::path::PathBuf;

use iced::widget::operation::{focus, move_cursor_to_end};
use iced::widget::{column, text, text_input};
use iced::{Element, Task, Theme};

use crate::core::folder::{Entry, complete, picker_rows, split_typed, typed_folder};
use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{LINE, SIZE};
use crate::ui::editor;
use crate::ui::status_bar::keys;
use crate::ui::switcher::{comment, list_row, panel, prompt};
use crate::ui::theme::Roles;

pub const PICKER_ID: &str = "picker";
/// The most rows shown; typing narrows a bigger folder.
const MAX_ROWS: usize = 14;

pub struct Picker {
    pub typed: String,
    /// What's in the folder `typed` lists (see `picker_rows`).
    rows: Vec<Entry>,
    selected: usize,
}

#[derive(Debug, Clone)]
pub enum PickerAction {
    Typed(String),
    /// Tab: into the highlighted folder, or the highlighted file's name completed.
    Complete,
    /// Enter: into the highlighted folder, or opens the highlighted file.
    Choose,
    /// A row clicked: as Enter on it.
    Pick(usize),
    /// →: into the highlighted folder.
    Into,
    /// ←: up to the folder above the one listed.
    Up,
}

impl Picker {
    fn new(typed: String) -> Picker {
        let mut picker = Picker {
            typed: String::new(),
            rows: Vec::new(),
            selected: 0,
        };
        picker.set_typed(typed);
        picker
    }

    fn set_typed(&mut self, typed: String) {
        let (folder, name) = split_typed(&typed, &home());
        self.rows = picker_rows(&folder, &name);
        self.typed = typed;
        // The first entry is highlighted, not `..`, so Enter doesn't go up by surprise.
        self.selected =
            usize::from(self.rows.first().is_some_and(Entry::is_up) && self.rows.len() > 1);
    }

    pub fn move_selection(&mut self, step: i32) {
        let last = self.rows.len().min(MAX_ROWS).saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(step as isize).min(last);
    }
}

impl App {
    /// Ctrl+O: opens the picker at the open file's folder (the home folder in scratch), or
    /// closes it.
    pub fn open_picker(&mut self) -> Task<Message> {
        self.menu_open = false;
        self.switcher = None;
        self.palette = None;
        if self.picker.take().is_some() {
            return focus(editor::EDITOR_ID);
        }
        let start = match &self.doc {
            Doc::File { path, .. } => path.parent().map(display_path),
            Doc::Scratch => None,
        };
        self.picker = Some(Picker::new(typed_folder(start.unwrap_or("~/".into()))));
        Task::batch([focus(PICKER_ID), move_cursor_to_end(PICKER_ID)])
    }

    pub fn update_picker(&mut self, action: PickerAction) -> Task<Message> {
        let Some(picker) = &mut self.picker else {
            return Task::none();
        };
        match action {
            PickerAction::Typed(typed) => picker.set_typed(typed),
            PickerAction::Complete => {
                if let Some(entry) = picker.rows.get(picker.selected).cloned() {
                    picker.set_typed(complete(&picker.typed, &entry));
                    return move_cursor_to_end(PICKER_ID);
                }
            }
            PickerAction::Into => {
                if picker
                    .rows
                    .get(picker.selected)
                    .is_some_and(|row| row.is_dir)
                {
                    return self.update_picker(PickerAction::Complete);
                }
            }
            PickerAction::Up => {
                let (folder, _) = split_typed(&picker.typed, &home());
                if let Some(above) = folder.parent() {
                    picker.set_typed(typed_folder(display_path(above)));
                    // The folder just left is highlighted, so → goes back into it.
                    if let Some(at) = picker.rows.iter().position(|row| row.path == folder) {
                        picker.selected = at;
                    }
                    return move_cursor_to_end(PICKER_ID);
                }
            }
            PickerAction::Pick(index) => {
                picker.selected = index;
                return Task::batch([self.update_picker(PickerAction::Choose), focus(PICKER_ID)]);
            }
            PickerAction::Choose => match picker.rows.get(picker.selected) {
                Some(row) if row.is_dir => return self.update_picker(PickerAction::Complete),
                Some(row) => {
                    let path = row.path.clone();
                    self.picker = None;
                    let open = self.update(Message::OpenPath(path));
                    return Task::batch([open, focus(editor::EDITOR_ID)]);
                }
                None => {}
            },
        }
        Task::none()
    }
}

/// The home folder, which `~` and a relative path in the box start from.
fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().to_path_buf())
        .unwrap_or_default()
}

/// The picker panel near the top of the window: what it does, the `>` prompt holding the path,
/// then the folder's entries, folders in blue with a `/` after them.
pub fn view(picker: &Picker) -> Element<'_, Message> {
    let search = text_input("~/", &picker.typed)
        .id(PICKER_ID)
        .on_input(|typed| Message::Picker(PickerAction::Typed(typed)))
        .on_submit(Message::Picker(PickerAction::Choose));
    let mut rows = column![];
    for (index, entry) in picker.rows.iter().enumerate().take(MAX_ROWS) {
        let selected = index == picker.selected;
        let (label, folder) = match entry.is_dir {
            true => (format!("{}/", entry.name()), true),
            false => (entry.name(), false),
        };
        // The highlight's colour wins over a folder's blue.
        let name = text(label)
            .size(SIZE)
            .line_height(LINE)
            .style(move |theme: &Theme| text::Style {
                color: (folder && !selected).then_some(Roles::of(theme).accent_cool),
            });
        rows = rows.push(list_row(
            name,
            selected,
            Message::Picker(PickerAction::Pick(index)),
        ));
    }
    let foot = match picker.rows.len() {
        0 => "# nothing here matches".to_string(),
        shown => {
            let more = match shown.saturating_sub(MAX_ROWS) {
                0 => String::new(),
                more => format!("{more} more · "),
            };
            format!("# {more}→ goes in · ← goes up · {}", keys("Ctrl+O closes"))
        }
    };
    panel(column![prompt(search), rows, comment(foot)])
}
