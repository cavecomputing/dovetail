//! The Ctrl+F find bar, just above the status bar: every match lit up in the text, Enter and
//! Shift+Enter (or the arrows) to step through them, and Esc to close with the match selected.

use std::ops::Range;

use iced::widget::{row, space, text_input};
use iced::{Background, Border, Color, Element, Fill, Task, Theme};

use crate::core::find::find_all;
use crate::ui::app::{App, Message};
use crate::ui::status_bar::{self, glyph_button, hint, small};
use crate::ui::text_editor::{PaintLook, PaintRun};
use crate::ui::theme::{GLYPH_CLOSE, GLYPH_DOWN, GLYPH_UP, Roles};

pub const FIND_ID: &str = "find";

/// The find bar's query and what it found. The query stays for the next Ctrl+F.
#[derive(Debug, Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    /// Every match in the text being edited, as byte ranges of the model's text, in order.
    pub matches: Vec<Range<usize>>,
    /// The selected match, an index into `matches`.
    pub current: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum FindAction {
    /// Ctrl+F: open the bar (or go back to it), with a selection on one line as the query.
    Open,
    Query(String),
    /// Select the next match (1) or the one before (-1), wrapping around.
    Step(isize),
}

impl App {
    /// Handles the find bar's messages; Esc closes it through `Message::CloseMenu`.
    pub fn update_find(&mut self, action: FindAction) -> Task<Message> {
        match action {
            FindAction::Open => {
                self.menu_open = false;
                if let Some(range) = self.pane.selection() {
                    let selected = &self.pane.model.text()[range];
                    if !selected.contains('\n') {
                        self.find.query = selected.to_string();
                    }
                }
                self.find.open = true;
                self.find_again(self.match_from());
                return Task::batch([
                    iced::widget::operation::focus(FIND_ID),
                    iced::widget::operation::select_all(FIND_ID),
                ]);
            }
            FindAction::Query(query) => {
                self.find.query = query;
                // Typing on keeps the match it is on, if it still matches.
                self.find_again(self.match_from());
                self.select_match();
            }
            FindAction::Step(step) => {
                let count = self.find.matches.len() as isize;
                if count > 0 {
                    let next = self.find.current.map_or(0, |current| {
                        (current as isize + step).rem_euclid(count) as usize
                    });
                    self.find.current = Some(next);
                    self.select_match();
                }
            }
        }
        Task::none()
    }

    /// Finds the query again in the text, choosing the first match at or after byte `from`, or
    /// the first of all when none is.
    pub fn find_again(&mut self, from: usize) {
        self.find.matches = find_all(self.pane.model.text(), &self.find.query);
        self.find.current = match self.find.matches.is_empty() {
            true => None,
            false => Some(
                self.find
                    .matches
                    .iter()
                    .position(|m| m.start >= from)
                    .unwrap_or(0),
            ),
        };
    }

    /// Where a search starts: the selection (often the match it is on) or the caret.
    fn match_from(&self) -> usize {
        self.pane
            .selection()
            .map_or(self.pane.caret(), |range| range.start)
    }

    /// Selects the current match in the editor, so the view scrolls to it.
    fn select_match(&mut self) {
        if let Some(current) = self.find.current {
            self.pane.select(self.find.matches[current].clone());
        }
    }

    /// The matches as the editor draws them while the bar is open: each lit up in the text
    /// colour, the selected one outlined in yellow too.
    pub fn find_runs(&self) -> Vec<PaintRun> {
        if !self.find.open {
            return Vec::new();
        }
        let roles = Roles::of(&self.theme);
        self.pane
            .widget_ranges(&self.find.matches)
            .into_iter()
            .enumerate()
            .map(|(n, (start, end))| PaintRun {
                start,
                end,
                color: roles.fg,
                look: PaintLook::Found {
                    outline: (Some(n) == self.find.current).then_some(roles.accent_text),
                },
            })
            .collect()
    }
}

/// The bar: the query, "2 of 5", the arrows and a close glyph, styled like the status bar.
pub fn view(app: &App) -> Element<'_, Message> {
    let find = &app.find;
    let count = match (find.current, find.matches.len()) {
        _ if find.query.is_empty() => String::new(),
        (_, 0) => "no matches".to_string(),
        (Some(current), total) => format!("{} of {total}", current + 1),
        (None, total) => format!("{total} matches"),
    };
    let input = text_input("Find", &find.query)
        .id(FIND_ID)
        .on_input(|query| Message::Find(FindAction::Query(query)))
        .size(12)
        .padding([1, 6])
        .width(260)
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
    let step = |by| (!find.matches.is_empty()).then_some(Message::Find(FindAction::Step(by)));
    status_bar::bar(
        row![
            input,
            small(count),
            hint(glyph_button(GLYPH_UP, step(-1)), "Previous (Shift+Enter)"),
            hint(glyph_button(GLYPH_DOWN, step(1)), "Next (Enter)"),
            space().width(Fill),
            hint(
                glyph_button(GLYPH_CLOSE, Some(Message::CloseMenu)),
                "Close (Esc)"
            ),
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into(),
    )
}
