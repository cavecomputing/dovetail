//! The Ctrl+F find bar, just above the status bar: every match lit up in the text, Enter and
//! Shift+Enter (or the arrows) to step through them, and Esc to close with the match selected.
//! Ctrl+Shift+F opens it with a Replace box under it.

use std::ops::Range;
use std::sync::Arc;

use iced::widget::{column, row, space, text_input};
use iced::{Background, Border, Color, Element, Fill, Task, Theme};

use crate::core::find::find_all;
use crate::ui::app::{App, Message};
use crate::ui::status_bar::{self, glyph_button, glyph_toggle, hint, keys, small, word_button};
use crate::ui::text_editor::{Action, Edit, PaintLook, PaintRun};
use crate::ui::theme::{GLYPH_CASE, GLYPH_CLOSE, GLYPH_DOWN, GLYPH_UP, GLYPH_WHOLE_WORD, Roles};

pub const FIND_ID: &str = "find";
const REPLACE_ID: &str = "replace";

/// The find bar's query and what it found. The query, the replacement and the switches stay for
/// the next Ctrl+F.
#[derive(Debug, Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    /// Whether the Replace box shows (Ctrl+Shift+F), and what it holds.
    pub replacing: bool,
    pub replacement: String,
    pub match_case: bool,
    pub whole_word: bool,
    /// Every match in the text being edited, as byte ranges of the model's text, in order.
    pub matches: Vec<Range<usize>>,
    /// The selected match, an index into `matches`.
    pub current: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum FindAction {
    /// Ctrl+F: open the bar (or go back to it), with a selection on one line as the query.
    Open,
    /// Ctrl+Shift+F: the same, with the Replace box.
    OpenReplace,
    Query(String),
    Replacement(String),
    /// Select the next match (1) or the one before (-1), wrapping around.
    Step(isize),
    MatchCase,
    WholeWord,
    /// Enter in the Replace box: replace the selected match and select the next. With Ctrl held,
    /// replace them all.
    ReplaceOne,
    ReplaceAll,
}

impl App {
    /// Handles the find bar's messages; Esc closes it through `Message::CloseMenu`.
    pub fn update_find(&mut self, action: FindAction) -> Task<Message> {
        match action {
            FindAction::Open | FindAction::OpenReplace => {
                self.menu_open = false;
                if let Some(range) = self.pane.selection() {
                    let selected = &self.pane.model.text()[range];
                    if !selected.contains('\n') {
                        self.find.query = selected.to_string();
                    }
                }
                self.find.open = true;
                self.find.replacing = matches!(action, FindAction::OpenReplace);
                self.find_again(self.match_from());
                // With something to find, Ctrl+Shift+F goes straight to the Replace box.
                let input = match self.find.replacing && !self.find.query.is_empty() {
                    true => REPLACE_ID,
                    false => FIND_ID,
                };
                return Task::batch([
                    iced::widget::operation::focus(input),
                    iced::widget::operation::select_all(input),
                ]);
            }
            FindAction::Replacement(replacement) => self.find.replacement = replacement,
            FindAction::MatchCase | FindAction::WholeWord => {
                match action {
                    FindAction::MatchCase => self.find.match_case = !self.find.match_case,
                    _ => self.find.whole_word = !self.find.whole_word,
                }
                self.find_again(self.match_from());
                self.select_match();
                // Clicking the switch took the keyboard from the find box; give it back.
                return iced::widget::operation::focus(FIND_ID);
            }
            FindAction::ReplaceOne if self.modifiers.command() => {
                return self.update_find(FindAction::ReplaceAll);
            }
            FindAction::ReplaceOne => {
                let Some(current) = self.find.current else {
                    return Task::none();
                };
                self.pane.select(self.find.matches[current].clone());
                let edit = match self.find.replacement.is_empty() {
                    true => Edit::Delete,
                    false => Edit::Paste(Arc::new(self.find.replacement.clone())),
                };
                let edited = match self.editable() {
                    true => self.pane.perform(Action::Edit(edit)),
                    false => Ok(false),
                };
                // The search starts again after the new text, at the next match.
                self.after_edit(edited);
                self.select_match();
            }
            FindAction::ReplaceAll => {
                let count = self.find.matches.len();
                if count == 0 || !self.editable() {
                    return Task::none();
                }
                let matches = self.find.matches.clone();
                let replaced = self.pane.replace_all(&matches, &self.find.replacement);
                let worked = replaced.is_ok();
                self.after_edit(replaced);
                if worked {
                    let words = match count {
                        1 => "Replaced 1 match".to_string(),
                        _ => format!("Replaced {count} matches"),
                    };
                    self.set_notice(Some(words));
                }
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
        self.find.matches = find_all(
            self.pane.model.text(),
            &self.find.query,
            self.find.match_case,
            self.find.whole_word,
        );
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

/// The bar: the query, "2 of 5", the switches, the arrows and a close glyph, styled like the
/// status bar, with the Replace box under it while replacing.
pub fn view(app: &App) -> Element<'_, Message> {
    let find = &app.find;
    let count = match (find.current, find.matches.len()) {
        _ if find.query.is_empty() => String::new(),
        (_, 0) => "no matches".to_string(),
        (Some(current), total) => format!("{} of {total}", current + 1),
        (None, total) => format!("{total} matches"),
    };
    let input = bar_input("Find", &find.query)
        .id(FIND_ID)
        .on_input(|query| Message::Find(FindAction::Query(query)));
    let step = |by| (!find.matches.is_empty()).then_some(Message::Find(FindAction::Step(by)));
    let toggle =
        |glyph, on, action, words| hint(glyph_toggle(glyph, on, Message::Find(action)), words);
    let find_row = status_bar::bar(
        row![
            input,
            small(count),
            toggle(
                GLYPH_CASE,
                find.match_case,
                FindAction::MatchCase,
                "Match case"
            ),
            toggle(
                GLYPH_WHOLE_WORD,
                find.whole_word,
                FindAction::WholeWord,
                "Whole word"
            ),
            hint(
                glyph_button(GLYPH_UP, step(-1)),
                keys("Previous (Shift+Enter)")
            ),
            hint(glyph_button(GLYPH_DOWN, step(1)), keys("Next (Enter)")),
            space().width(Fill),
            hint(
                glyph_button(GLYPH_CLOSE, Some(Message::CloseMenu)),
                "Close (Esc)"
            ),
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into(),
    );
    if !find.replacing {
        return find_row;
    }
    let any = (!find.matches.is_empty() && app.editable()).then_some(());
    let replace_row = status_bar::bar(
        row![
            bar_input("Replace", &find.replacement)
                .id(REPLACE_ID)
                .on_input(|replacement| Message::Find(FindAction::Replacement(replacement)))
                .on_submit(Message::Find(FindAction::ReplaceOne)),
            hint(
                word_button(
                    "Replace".to_string(),
                    any.map(|_| Message::Find(FindAction::ReplaceOne))
                ),
                keys("Replace this match and find the next (Enter)"),
            ),
            hint(
                word_button(
                    "All".to_string(),
                    any.map(|_| Message::Find(FindAction::ReplaceAll))
                ),
                keys("Replace every match (Ctrl+Enter)"),
            ),
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into(),
    );
    column![find_row, replace_row].into()
}

/// A text box for the bar, outlined in the focus colour while it has the keyboard.
fn bar_input<'a>(placeholder: &str, value: &str) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .size(12)
        .padding([1, 6])
        .width(260)
        .style(|theme: &Theme, status| {
            let roles = Roles::of(theme);
            let outline = match status {
                text_input::Status::Focused { .. } => roles.accent_text,
                _ => roles.border,
            };
            text_input::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default().rounded(4).width(1).color(outline),
                icon: roles.fg_muted,
                placeholder: roles.fg_subtle,
                value: roles.fg,
                selection: roles.accent_cool.scale_alpha(0.35),
            }
        })
}
