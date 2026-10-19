//! The Ctrl+O picker: a box holding a path, starting at the open file's folder, and what's in
//! that folder below it. Typing filters the folder by name; Tab (or a click on a folder) goes into
//! the highlighted folder; Enter opens the highlighted row, a file in this window or a folder in
//! the folder pane. With nothing typed after the last `/`, the first row is the folder itself.

use std::path::PathBuf;

use iced::widget::operation::{focus, move_cursor_to_end};
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Fill, Task, Theme};

use crate::core::folder::{Entry, complete, picker_rows, split_typed, typed_folder};
use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::ui::app::{App, Message};
use crate::ui::editor;
use crate::ui::status_bar::keys;
use crate::ui::switcher::{panel, row_style, search_style};
use crate::ui::theme::{GLYPH_FILE, GLYPH_FOLDER, Roles};

pub const PICKER_ID: &str = "picker";
/// The most rows shown; typing narrows a bigger folder.
const MAX_ROWS: usize = 14;

pub struct Picker {
    pub typed: String,
    /// The folder `typed` lists, and the rows for it (see `picker_rows`).
    folder: PathBuf,
    rows: Vec<Entry>,
    selected: usize,
}

#[derive(Debug, Clone)]
pub enum PickerAction {
    Typed(String),
    /// Tab: into the highlighted folder, or the highlighted file's name completed.
    Complete,
    /// Enter: opens the highlighted row.
    Choose,
    /// A row clicked: a folder goes into it, a file (or the folder itself) opens.
    Pick(usize),
}

impl Picker {
    fn new(typed: String) -> Picker {
        let mut picker = Picker {
            typed: String::new(),
            folder: PathBuf::new(),
            rows: Vec::new(),
            selected: 0,
        };
        picker.set_typed(typed);
        picker
    }

    fn set_typed(&mut self, typed: String) {
        let home = directories::BaseDirs::new()
            .map(|base| base.home_dir().to_path_buf())
            .unwrap_or_default();
        let (folder, name) = split_typed(&typed, &home);
        self.rows = picker_rows(&folder, &name);
        self.folder = folder;
        self.typed = typed;
        self.selected = 0;
    }

    /// Whether row `index` is the listed folder itself (the first row with no name typed).
    fn is_itself(&self, index: usize) -> bool {
        self.rows
            .get(index)
            .is_some_and(|row| row.path == self.folder)
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
                if let Some(entry) = picker.rows.get(picker.selected).cloned()
                    && !picker.is_itself(picker.selected)
                {
                    picker.set_typed(complete(&picker.typed, &entry));
                    return move_cursor_to_end(PICKER_ID);
                }
            }
            PickerAction::Pick(index)
                if picker.rows.get(index).is_some_and(|row| row.is_dir)
                    && !picker.is_itself(index) =>
            {
                picker.selected = index;
                return Task::batch([self.update_picker(PickerAction::Complete), focus(PICKER_ID)]);
            }
            PickerAction::Pick(index) => {
                picker.selected = index;
                return self.update_picker(PickerAction::Choose);
            }
            PickerAction::Choose => {
                let Some(path) = picker.rows.get(picker.selected).map(|row| row.path.clone())
                else {
                    return Task::none();
                };
                self.picker = None;
                let open = self.update(Message::OpenPath(path.clone()));
                return match path.is_dir() {
                    true => open,
                    false => Task::batch([open, focus(editor::EDITOR_ID)]),
                };
            }
        }
        Task::none()
    }
}

/// The picker panel near the top of the window: the path box, then the rows.
pub fn view(picker: &Picker) -> Element<'_, Message> {
    let search = text_input("~/", &picker.typed)
        .id(PICKER_ID)
        .on_input(|typed| Message::Picker(PickerAction::Typed(typed)))
        .on_submit(Message::Picker(PickerAction::Choose))
        .size(13)
        .padding([6, 10])
        .style(search_style);
    let mut rows = column![].spacing(2);
    for (index, entry) in picker.rows.iter().enumerate().take(MAX_ROWS) {
        let selected = index == picker.selected;
        let (glyph, label) = match (picker.is_itself(index), entry.is_dir) {
            (true, _) => (GLYPH_FOLDER, "./  open this folder".to_string()),
            (false, true) => (GLYPH_FOLDER, format!("{}/", entry.name())),
            (false, false) => (GLYPH_FILE, entry.name()),
        };
        rows = rows.push(
            button(
                row![text(glyph).size(14), text(label).size(13)]
                    .spacing(10)
                    .align_y(iced::Center),
            )
            .width(Fill)
            .padding([5, 10])
            .style(move |theme, status| row_style(theme, status, selected))
            .on_press(Message::Picker(PickerAction::Pick(index))),
        );
    }
    let hint = match picker.rows.len() {
        0 => "Nothing here matches".to_string(),
        shown => {
            let more = shown.saturating_sub(MAX_ROWS);
            let more = match more {
                0 => String::new(),
                _ => format!("{more} more · "),
            };
            format!(
                "{more}Tab goes into a folder · Enter opens · {}",
                keys("Ctrl+O closes")
            )
        }
    };
    let subtle = |theme: &Theme| text::Style {
        color: Some(Roles::of(theme).fg_subtle),
    };
    let hint = container(text(hint).size(11).style(subtle)).padding([2, 10]);
    panel(column![search, rows, hint])
}
