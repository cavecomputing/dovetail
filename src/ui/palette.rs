//! The Ctrl+Shift+P command palette: the tools for the kind of file open (Format JSON and Minify
//! JSON in a .json file). Type a few letters of a tool's name, ↑ and ↓ choose, Enter runs it, Esc
//! closes.

use iced::widget::{button, column, container, text, text_input};
use iced::{Element, Fill, Task};

use crate::core::recent::match_score;
use crate::core::syntax::Syntax;
use crate::ui::app::{App, Message};
use crate::ui::editor;
use crate::ui::switcher::{panel, row_style, search_style};

pub const PALETTE_ID: &str = "palette";

#[derive(Default)]
pub struct Palette {
    pub query: String,
    /// Index into `matches()`.
    pub selected: usize,
}

/// The tools for a file of this syntax, as (name, what it runs).
fn tools(syntax: Syntax) -> Vec<(&'static str, Message)> {
    match syntax {
        Syntax::Json => vec![
            ("Format JSON", Message::ReformatJson { minify: false }),
            ("Minify JSON", Message::ReformatJson { minify: true }),
        ],
        _ => Vec::new(),
    }
}

impl Palette {
    /// The tools for `syntax` whose names match the query, best first.
    pub fn matches(&self, syntax: Syntax) -> Vec<(&'static str, Message)> {
        let mut scored: Vec<_> = tools(syntax)
            .into_iter()
            .filter_map(|tool| Some((match_score(&self.query, tool.0)?, tool)))
            .collect();
        // A stable sort keeps the tools' own order among equal scores.
        scored.sort_by_key(|(score, _)| -score);
        scored.into_iter().map(|(_, tool)| tool).collect()
    }
}

impl App {
    /// Handles the palette's messages; `App::update` hands every one of them here.
    pub fn update_palette(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenPalette => {
                self.menu_open = false;
                self.switcher = None;
                if self.palette.take().is_some() {
                    return iced::widget::operation::focus(editor::EDITOR_ID);
                }
                self.palette = Some(Palette::default());
                return iced::widget::operation::focus(PALETTE_ID);
            }
            Message::PaletteQuery(query) => {
                if let Some(palette) = &mut self.palette {
                    palette.query = query;
                    palette.selected = 0;
                }
            }
            // ↑ and ↓, which move the switcher's highlight while it's open instead.
            Message::SwitcherMove(step) => {
                let syntax = self.syntax();
                if let Some(palette) = &mut self.palette {
                    let last = palette.matches(syntax).len().saturating_sub(1);
                    let moved = palette.selected.saturating_add_signed(step as isize);
                    palette.selected = moved.min(last);
                }
            }
            Message::PaletteRun(index) => {
                let palette = self.palette.take();
                let tool = palette
                    .and_then(|palette| palette.matches(self.syntax()).into_iter().nth(index));
                return match tool {
                    Some((_, run)) => self.update(run),
                    None => iced::widget::operation::focus(editor::EDITOR_ID),
                };
            }
            _ => {}
        }
        Task::none()
    }
}

/// The palette panel near the top of the window: the search box, then the matching tools.
pub fn view(palette: &Palette, syntax: Syntax) -> Element<'_, Message> {
    let search = text_input("Run a tool for this file", &palette.query)
        .id(PALETTE_ID)
        .on_input(Message::PaletteQuery)
        .on_submit(Message::PaletteRun(palette.selected))
        .size(13)
        .padding([6, 10])
        .style(search_style);

    let mut rows = column![].spacing(2);
    let matches = palette.matches(syntax);
    for (index, (name, _)) in matches.iter().enumerate() {
        let selected = index == palette.selected;
        rows = rows.push(
            button(text(*name).size(13))
                .width(Fill)
                .padding([5, 10])
                .style(move |theme, status| row_style(theme, status, selected))
                .on_press(Message::PaletteRun(index)),
        );
    }
    if matches.is_empty() {
        let words = match tools(syntax).is_empty() {
            true => "No tools for this kind of file",
            false => "Nothing matches",
        };
        rows = rows.push(container(text(words).size(12)).padding([6, 10]));
    }

    panel(column![search, rows])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(query: &str, syntax: Syntax) -> Vec<&'static str> {
        let palette = Palette {
            query: query.to_string(),
            selected: 0,
        };
        palette
            .matches(syntax)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn only_a_json_file_has_tools_and_typing_narrows_them() {
        assert_eq!(names("", Syntax::Json), ["Format JSON", "Minify JSON"]);
        assert_eq!(names("min", Syntax::Json), ["Minify JSON"]);
        assert_eq!(names("zzz", Syntax::Json), Vec::<&str>::new());
        assert_eq!(names("", Syntax::Yaml), Vec::<&str>::new());
        assert_eq!(names("", Syntax::Plain), Vec::<&str>::new());
    }
}
