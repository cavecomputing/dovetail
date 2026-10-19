//! The Ctrl+P quick switcher: scratch, then the recent files, numbered 0 to 9. Type a few letters
//! of a file's name, Enter opens the highlighted row in this window, a row's number (Ctrl+number
//! once something is typed) opens that row, Esc closes.

use std::path::PathBuf;

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Background, Border, Color, Element, Fill, Padding, Task, Theme};

use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::recent::{existing_recent_files, match_score};
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{CH, LINE, SIZE, accent_text, line, subtle};
use crate::ui::editor;
use crate::ui::status_bar::{file_name, keys, title};
use crate::ui::theme::{Roles, raised_box};

pub const SWITCHER_ID: &str = "switcher";

pub struct Switcher {
    pub query: String,
    /// The recent files, newest first, without this window's own file.
    pub items: Vec<PathBuf>,
    /// Index into `rows()`.
    pub selected: usize,
}

impl Switcher {
    /// The switcher at `items`, with the most recent file highlighted.
    fn new(items: Vec<PathBuf>) -> Switcher {
        let mut switcher = Switcher {
            query: String::new(),
            items,
            selected: 0,
        };
        switcher.selected = switcher.first_row();
        switcher
    }

    /// The rows, top to bottom: scratch always first (row 0, whatever is typed), then the items
    /// matching the query, best first.
    pub fn rows(&self) -> Vec<Doc> {
        let files = self.matches().into_iter().map(|path| Doc::File {
            path: path.clone(),
            line_ending: Default::default(),
            bom: false,
        });
        std::iter::once(Doc::Scratch).chain(files).collect()
    }

    /// The row highlighted when the list changes: the best match, or scratch when nothing
    /// matches.
    fn first_row(&self) -> usize {
        usize::from(!self.matches().is_empty())
    }

    /// The items matching the query, best first (newest first among equals).
    pub fn matches(&self) -> Vec<&PathBuf> {
        let mut scored: Vec<(i32, &PathBuf)> = self
            .items
            .iter()
            .filter_map(|path| Some((match_score(&self.query, &file_name(path))?, path)))
            .collect();
        // A stable sort keeps the newest first among equal scores.
        scored.sort_by_key(|(score, _)| -score);
        scored.into_iter().map(|(_, item)| item).collect()
    }

    /// Moves the highlight by `step` rows, staying on the list.
    pub fn move_selection(&mut self, step: i32) {
        let last = self.rows().len() - 1;
        self.selected = self.selected.saturating_add_signed(step as isize).min(last);
    }
}

impl App {
    /// Handles the switcher's messages; `App::update` hands every one of them here.
    pub fn update_switcher(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenSwitcher => {
                self.menu_open = false;
                self.palette = None;
                if self.switcher.take().is_some() {
                    return iced::widget::operation::focus(editor::EDITOR_ID);
                }
                let current = match &self.doc {
                    Doc::File { path, .. } => Some(path),
                    Doc::Scratch => None,
                };
                let items = existing_recent_files(&self.paths.data_dir)
                    .into_iter()
                    .filter(|path| Some(path) != current)
                    .collect();
                self.switcher = Some(Switcher::new(items));
                return iced::widget::operation::focus(SWITCHER_ID);
            }
            Message::SwitcherQuery(query) => {
                if let Some(switcher) = &mut self.switcher {
                    // A number typed into the empty box opens its row instead of searching.
                    if switcher.query.is_empty()
                        && query.len() == 1
                        && let Ok(row) = query.parse::<usize>()
                        && row < switcher.rows().len()
                    {
                        return self.open_row(row);
                    }
                    switcher.query = query;
                    switcher.selected = switcher.first_row();
                }
            }
            Message::SwitcherMove(step) => {
                if let Some(switcher) = &mut self.switcher {
                    switcher.move_selection(step);
                }
            }
            Message::SwitcherOpen => {
                if let Some(switcher) = &self.switcher {
                    return self.open_row(switcher.selected);
                }
            }
            Message::SwitcherPick(row) => return self.open_row(row),
            _ => {}
        }
        Task::none()
    }

    /// Opens the switcher's `row` in this window (scratch, in scratch, just closes the switcher).
    fn open_row(&mut self, row: usize) -> Task<Message> {
        let picked = self.switcher.as_ref().map(Switcher::rows);
        match picked.and_then(|rows| rows.into_iter().nth(row)) {
            Some(doc) => {
                let open = self.open_here(doc);
                Task::batch([open, iced::widget::operation::focus(editor::EDITOR_ID)])
            }
            None => Task::none(),
        }
    }
}

/// The switcher panel near the top of the window: what it lists, the `>` prompt, then the rows.
pub fn view(switcher: &Switcher) -> Element<'_, Message> {
    let search = text_input("type part of a name", &switcher.query)
        .id(SWITCHER_ID)
        .on_input(Message::SwitcherQuery)
        .on_submit(Message::SwitcherOpen);

    let mut rows = column![];
    let docs = switcher.rows();
    let nothing = docs.len() == 1;
    for (index, doc) in docs.into_iter().enumerate().take(12) {
        rows = rows.push(match_row(doc, index, index == switcher.selected));
    }
    let foot = match (nothing, switcher.items.is_empty()) {
        (true, true) => "# no recent files yet".to_string(),
        (true, false) => "# nothing matches".to_string(),
        (false, _) => format!("# a row's number opens it · {}", keys("Ctrl+P closes")),
    };
    panel(column![prompt(search), rows, comment(foot)])
}

/// A panel near the top of the window, as the switcher, the picker and the palette show, drawn
/// like the assistant pane: `#` lines, a `>` prompt, plain rows. Clicking outside it closes it.
pub fn panel<'a>(content: iced::widget::Column<'a, Message>) -> Element<'a, Message> {
    let panel = container(content.spacing(SIZE * LINE * 0.5).width(PANEL_WIDTH))
        .padding([12, 0])
        .style(raised_box);
    iced::widget::stack![
        iced::widget::mouse_area(space().width(Fill).height(Fill)).on_press(Message::CloseMenu),
        container(iced::widget::opaque(panel))
            .center_x(Fill)
            .padding(Padding {
                top: 72.0,
                ..Padding::ZERO
            }),
    ]
    .into()
}

const PANEL_WIDTH: f32 = 560.0;
/// How far a panel's lines sit in from its edges.
const INSET: f32 = 16.0;

/// A `#` line in a panel, in the comment colour.
pub fn comment<'a>(words: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    container(line(words, subtle)).padding([0.0, INSET]).into()
}

/// The `>` prompt and the box typed into after it, with no frame, as the assistant's is.
pub fn prompt<'a>(input: text_input::TextInput<'a, Message>) -> Element<'a, Message> {
    let input = input
        .size(SIZE)
        .line_height(LINE)
        .padding(0)
        .style(search_style);
    container(row![line(">", accent_text), input].spacing(CH))
        .padding([0.0, INSET])
        .into()
}

/// A row of a panel's list, lined up with the text typed after the `>`: lit while hovered, and
/// in the focus colour on a raised band while `selected`.
pub fn list_row<'a>(
    content: impl Into<Element<'a, Message>>,
    selected: bool,
    on_press: Message,
) -> Element<'a, Message> {
    button(content)
        .width(Fill)
        .padding(Padding {
            top: 0.0,
            right: INSET,
            bottom: 0.0,
            left: INSET + 2.0 * CH,
        })
        .style(move |theme, status| row_style(theme, status, selected))
        .on_press(on_press)
        .into()
}

/// One row: its number (rows 0 to 9), the name, and the folder it's in.
fn match_row<'a>(doc: Doc, index: usize, selected: bool) -> Element<'a, Message> {
    let folder = match &doc {
        Doc::File { path, .. } => path.parent().map(display_path).unwrap_or_default(),
        Doc::Scratch => String::new(),
    };
    let number = match index {
        0..=9 => index.to_string(),
        _ => String::new(),
    };
    let name = title(&doc);
    // The folder's end, which names it, is kept when the row is too narrow for all of it.
    let room = (PANEL_WIDTH - 2.0 * INSET) / CH - 2.0 - 3.0 - 2.0;
    let room = (room as usize).saturating_sub(name.chars().count());
    let skip = folder.chars().count().saturating_sub(room);
    let folder = match skip {
        0 => folder,
        _ => format!("…{}", folder.chars().skip(skip + 1).collect::<String>()),
    };
    let quiet = |words: String| line(words, subtle).wrapping(Wrapping::None);
    let name = text(name)
        .size(SIZE)
        .line_height(LINE)
        .wrapping(Wrapping::None);
    let words = row![
        quiet(number).width(3.0 * CH),
        name,
        space::horizontal(),
        quiet(folder)
    ];
    list_row(words.spacing(CH), selected, Message::SwitcherPick(index))
}

/// The prompt's box: no frame and no fill, as the assistant's.
fn search_style(theme: &Theme, _status: text_input::Status) -> text_input::Style {
    let roles = Roles::of(theme);
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: roles.fg_muted,
        placeholder: roles.fg_subtle,
        value: roles.fg,
        selection: roles.accent_cool.scale_alpha(0.35),
    }
}

/// A row's look: lit while hovered, and in the focus colour while `selected`.
fn row_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let roles = Roles::of(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: (selected || hovered).then_some(Background::Color(roles.bg_inset)),
        text_color: if selected {
            roles.accent_text
        } else {
            roles.fg
        },
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switcher(query: &str) -> Switcher {
        let mut switcher = Switcher::new(vec![PathBuf::from("/a/notes.md"), "/b/todo.txt".into()]);
        switcher.query = query.to_string();
        switcher
    }

    fn names(switcher: &Switcher) -> Vec<String> {
        switcher.rows().iter().map(title).collect()
    }

    #[test]
    fn scratch_is_row_zero_whatever_is_typed() {
        assert_eq!(names(&switcher("")), ["Scratch", "notes.md", "todo.txt"]);
        assert_eq!(names(&switcher("todo")), ["Scratch", "todo.txt"]);
        assert_eq!(names(&switcher("zzz")), ["Scratch"]);
    }

    #[test]
    fn the_first_file_is_highlighted_and_scratch_without_one() {
        assert_eq!(switcher("").selected, 1);
        assert_eq!(switcher("zzz").first_row(), 0);
        assert_eq!(Switcher::new(Vec::new()).selected, 0);
        let mut moving = switcher("");
        moving.move_selection(-5);
        assert_eq!(moving.selected, 0);
        moving.move_selection(5);
        assert_eq!(moving.selected, 2);
    }
}
