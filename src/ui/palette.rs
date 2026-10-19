//! The Ctrl+Shift+P command palette: the tools for the kind of file open (Format JSON and Minify
//! JSON in a .json file). Type a few letters of a tool's name, ↑ and ↓ choose, Enter runs it, Esc
//! closes.

use iced::widget::{column, text, text_input};
use iced::{Element, Task};

use crate::core::recent::match_score;
use crate::core::syntax::Syntax;
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{LINE, SIZE};
use crate::ui::editor;
use crate::ui::status_bar::keys;
use crate::ui::switcher::{comment, list_row, panel, prompt};

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

/// The palette panel near the top of the window: what it lists, the `>` prompt, then the tools.
pub fn view(palette: &Palette, syntax: Syntax) -> Element<'_, Message> {
    let search = text_input("type part of a tool's name", &palette.query)
        .id(PALETTE_ID)
        .on_input(Message::PaletteQuery)
        .on_submit(Message::PaletteRun(palette.selected));

    let mut rows = column![];
    let matches = palette.matches(syntax);
    for (index, (name, _)) in matches.iter().enumerate() {
        let name = text(*name).size(SIZE).line_height(LINE);
        rows = rows.push(list_row(
            name,
            index == palette.selected,
            Message::PaletteRun(index),
        ));
    }
    let foot = match (matches.is_empty(), tools(syntax).is_empty()) {
        (false, _) => format!("# Enter runs it · {}", keys("Ctrl+Shift+P closes")),
        (true, true) => "# no tools for this kind of file".to_string(),
        (true, false) => "# nothing matches".to_string(),
    };
    panel(column![
        comment("# tools for this file"),
        prompt(search),
        rows,
        comment(foot)
    ])
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
