//! The Ctrl+P quick switcher: type a few letters of a recent file or project, Enter opens it in a
//! new window, Esc closes.

use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Background, Border, Color, Element, Fill, Padding, Task, Theme};

use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::recent::match_score;
use crate::project::{RecentItem, recent_items};
use crate::ui::app::{App, Message, open_in_new_window};
use crate::ui::editor;
use crate::ui::theme::{GLYPH_FILE, GLYPH_PROJECT, Roles, raised_box};

pub const SWITCHER_ID: &str = "switcher";

pub struct Switcher {
    pub query: String,
    /// The recent files and projects, newest first, without this window's own document.
    pub items: Vec<RecentItem>,
    /// Index into `matches()`.
    pub selected: usize,
}

impl Switcher {
    /// The items matching the query, best first (newest first among equals).
    pub fn matches(&self) -> Vec<&RecentItem> {
        let mut scored: Vec<(i32, &RecentItem)> = self
            .items
            .iter()
            .filter_map(|item| Some((match_score(&self.query, &item.label)?, item)))
            .collect();
        // A stable sort keeps the newest first among equal scores.
        scored.sort_by_key(|(score, _)| -score);
        scored.into_iter().map(|(_, item)| item).collect()
    }

    /// Moves the highlight by `step` rows, staying on the list.
    pub fn move_selection(&mut self, step: i32) {
        let last = self.matches().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(step as isize).min(last);
    }
}

impl App {
    /// Handles the switcher's messages; `App::update` hands every one of them here.
    pub fn update_switcher(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenSwitcher => {
                self.menu_open = false;
                if self.switcher.take().is_some() {
                    return iced::widget::operation::focus(editor::EDITOR_ID);
                }
                let current = match &self.doc {
                    Doc::File { path, .. } | Doc::Project(path) => Some(path),
                    Doc::Scratch => None,
                };
                let items = recent_items(&self.paths.data_dir)
                    .into_iter()
                    .filter(|item| Some(&item.path) != current)
                    .collect();
                self.switcher = Some(Switcher {
                    query: String::new(),
                    items,
                    selected: 0,
                });
                return iced::widget::operation::focus(SWITCHER_ID);
            }
            Message::SwitcherQuery(query) => {
                if let Some(switcher) = &mut self.switcher {
                    switcher.query = query;
                    switcher.selected = 0;
                }
            }
            Message::SwitcherMove(step) => {
                if let Some(switcher) = &mut self.switcher {
                    switcher.move_selection(step);
                }
            }
            Message::SwitcherOpen => {
                let picked = self.switcher.as_ref().and_then(|switcher| {
                    let matches = switcher.matches();
                    matches.get(switcher.selected).map(|item| item.path.clone())
                });
                if let Some(path) = picked {
                    self.switcher = None;
                    self.notice = open_in_new_window(Some(path)).err();
                    return iced::widget::operation::focus(editor::EDITOR_ID);
                }
            }
            _ => {}
        }
        Task::none()
    }
}

/// The switcher panel near the top of the window: the search box, then the matches.
pub fn view(switcher: &Switcher) -> Element<'_, Message> {
    let search = text_input("Open a recent file or project", &switcher.query)
        .id(SWITCHER_ID)
        .on_input(Message::SwitcherQuery)
        .on_submit(Message::SwitcherOpen)
        .size(13)
        .padding([6, 10])
        .style(|theme: &Theme, _status| {
            let roles = Roles::of(theme);
            text_input::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default()
                    .rounded(4)
                    .width(1)
                    .color(roles.accent_text),
                icon: roles.fg_muted,
                placeholder: roles.fg_subtle,
                value: roles.fg,
                selection: roles.accent_cool.scale_alpha(0.35),
            }
        });

    let mut rows = column![].spacing(2);
    let matches = switcher.matches();
    if matches.is_empty() {
        let words = match switcher.items.is_empty() {
            true => "No recent files or projects yet",
            false => "Nothing matches",
        };
        rows = rows.push(container(text(words).size(12)).padding([6, 10]));
    }
    for (index, item) in matches.into_iter().enumerate().take(12) {
        rows = rows.push(match_row(item, index == switcher.selected));
    }

    let panel = container(column![search, rows].spacing(6).width(560))
        .padding(6)
        .style(raised_box);
    // Clicking outside the panel closes it, like the menu.
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

/// One match: a glyph for file or project, its name, and where it lives.
fn match_row(item: &RecentItem, selected: bool) -> Element<'_, Message> {
    let glyph = if item.is_project {
        GLYPH_PROJECT
    } else {
        GLYPH_FILE
    };
    let folder = item.path.parent().map(display_path).unwrap_or_default();
    button(
        row![
            text(glyph).size(14),
            text(item.label.as_str()).size(13),
            space::horizontal(),
            text(folder).size(11).style(|theme: &Theme| text::Style {
                color: Some(Roles::of(theme).fg_subtle),
            }),
        ]
        .spacing(10)
        .align_y(iced::Center),
    )
    .width(Fill)
    .padding([5, 10])
    .style(move |theme: &Theme, status| {
        let roles = Roles::of(theme);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: (selected || hovered).then_some(Background::Color(roles.bg_inset)),
            text_color: if selected {
                roles.accent_text
            } else {
                roles.fg
            },
            border: Border::default().rounded(4),
            ..button::Style::default()
        }
    })
    .on_press(Message::OpenPath(item.path.clone()))
    .into()
}
