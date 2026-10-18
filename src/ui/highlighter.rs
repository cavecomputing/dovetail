//! How the editor colours its text: Markdown in .md files; JSON, YAML, TOML and INI files; XML
//! tags and {{macros}}; and in review mode the struck and ghost text of suggestions. iced asks
//! for the colours one line at a time.

use std::ops::Range;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::{Color, Font, Theme};

use crate::core::config::{ini_ranges, toml_ranges, yaml_ranges};
use crate::core::json::json_ranges;
use crate::core::markdown::{Block, Markdown, markdown_ranges};
use crate::core::markup::{Markup, markup_ranges};
use crate::core::syntax::{Syntax, Token};
use crate::ui::theme::{MONO, Roles, bold};

/// What the highlighter colours: the file's `syntax` (Markdown, or a data file's keys and values),
/// XML tags and macros when `markup` is on, and in review mode the struck and ghost text and the
/// room for the buttons, as (line, byte columns, look).
#[derive(Debug, Clone, PartialEq)]
pub struct Colouring {
    pub syntax: Syntax,
    pub markup: bool,
    pub review: Vec<(usize, Range<usize>, Highlight)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Highlight {
    Markdown(Markdown),
    /// A part of a JSON, YAML, TOML or INI line.
    Data(Token),
    Markup(Markup),
    /// Text a suggestion would replace: muted (the line through it is drawn as paint).
    Struck,
    /// A suggestion's new text: italic.
    Ghost,
    /// The room for a suggestion's buttons: invisible, as the buttons are drawn over it.
    Hidden,
}

pub fn highlight_format(highlight: &Highlight, theme: &Theme) -> highlighter::Format<Font> {
    let roles = Roles::of(theme);
    let color = match highlight {
        Highlight::Markdown(markdown) => match markdown {
            Markdown::Heading => Some(roles.accent_text),
            Markdown::Quote => Some(roles.fg_muted),
            Markdown::ListMarker | Markdown::LinkText => Some(roles.accent_cool),
            Markdown::Code => Some(roles.accent_aqua),
            Markdown::Rule | Markdown::Fence | Markdown::Strike | Markdown::LinkUrl => {
                Some(roles.fg_subtle)
            }
            Markdown::Bold | Markdown::Italic => None,
        },
        Highlight::Data(token) => Some(match token {
            Token::Key => roles.accent_cool,
            Token::String => roles.accent_aqua,
            Token::Number => roles.accent_text,
            Token::Literal => roles.accent_warm,
            Token::Comment | Token::Marker => roles.fg_subtle,
            Token::Section => roles.accent_text,
        }),
        Highlight::Markup(Markup::Tag) | Highlight::Struck => Some(roles.fg_muted),
        Highlight::Markup(Markup::Macro) => Some(roles.accent_cool),
        Highlight::Hidden => Some(Color::TRANSPARENT),
        Highlight::Ghost => None,
    };
    let font = match highlight {
        Highlight::Markdown(Markdown::Heading | Markdown::Bold)
        | Highlight::Data(Token::Section) => Some(bold()),
        Highlight::Markdown(Markdown::Italic) | Highlight::Ghost => Some(Font {
            style: iced::font::Style::Italic,
            ..MONO
        }),
        _ => None,
    };
    highlighter::Format { color, font }
}

/// iced asks for colours one line at a time, from `current_line` down to the last line in view,
/// and says which line an edit changed first; `line` tracks how far it has got.
pub struct MarkupHighlighter {
    line: usize,
    /// What each line up to `line` starts inside, so colouring can carry on from any of them.
    carried: Vec<Carry>,
    colouring: Colouring,
}

/// What a line starts inside, from the lines above it: a Markdown code fence, a YAML block
/// scalar (the indent of the line that opened it), or a TOML multi-line string (its closing
/// quotes). Only the file's own syntax's part is used.
#[derive(Debug, Clone, Copy)]
struct Carry {
    block: Block,
    yaml_block: Option<usize>,
    toml_string: Option<&'static str>,
}

const START: Carry = Carry {
    block: Block::Prose,
    yaml_block: None,
    toml_string: None,
};

impl Highlighter for MarkupHighlighter {
    type Settings = Colouring;
    type Highlight = Highlight;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Highlight)>;

    fn new(colouring: &Colouring) -> Self {
        MarkupHighlighter {
            line: 0,
            carried: vec![START],
            colouring: colouring.clone(),
        }
    }

    fn update(&mut self, colouring: &Colouring) {
        self.change_line(0);
        self.colouring = colouring.clone();
    }

    /// An edit changed `line`: colouring starts again from there, if it had got that far, and
    /// what it knew about the lines after it is dropped, as the edit may open or close a fence.
    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
        self.carried.truncate(self.line + 1);
    }

    /// The file's syntax first, then tags and macros (so a {{macro}} inside a JSON or YAML string
    /// still shows), then suggestions, so suggestion looks win where they overlap.
    fn highlight_line(&mut self, text: &str) -> Self::Iterator<'_> {
        let mut carry = self.carried[self.line];
        let mut found: Vec<_> = match self.colouring.syntax {
            Syntax::Plain => Vec::new(),
            Syntax::Markdown => {
                let (markdown, next) = markdown_ranges(text, carry.block);
                carry.block = next;
                markdown
                    .into_iter()
                    .map(|(range, markdown)| (range, Highlight::Markdown(markdown)))
                    .collect()
            }
            Syntax::Json => data(json_ranges(text)),
            Syntax::Yaml => {
                let (found, next) = yaml_ranges(text, carry.yaml_block);
                carry.yaml_block = next;
                data(found)
            }
            Syntax::Toml => {
                let (found, next) = toml_ranges(text, carry.toml_string);
                carry.toml_string = next;
                data(found)
            }
            Syntax::Ini => data(ini_ranges(text)),
        };
        self.carried.push(carry);
        if self.colouring.markup {
            for (range, markup) in markup_ranges(text) {
                found.push((range, Highlight::Markup(markup)));
            }
        }
        for (line, range, highlight) in &self.colouring.review {
            if *line == self.line {
                found.push((range.clone(), *highlight));
            }
        }
        self.line += 1;
        found.into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

/// A data file's coloured parts as highlights.
fn data(found: Vec<(Range<usize>, Token)>) -> Vec<(Range<usize>, Highlight)> {
    found
        .into_iter()
        .map(|(range, token)| (range, Highlight::Data(token)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markdown_on() -> Colouring {
        Colouring {
            syntax: Syntax::Markdown,
            markup: false,
            review: Vec::new(),
        }
    }

    /// Colours `lines` as iced's editor does when it draws: from the highlighter's current line
    /// down to `last_in_view`. Returns each line it coloured, with the looks it got.
    fn draw(
        highlighter: &mut MarkupHighlighter,
        lines: &[&str],
        last_in_view: usize,
    ) -> Vec<(usize, Vec<Highlight>)> {
        (highlighter.current_line()..=last_in_view)
            .map(|line| {
                let looks = highlighter.highlight_line(lines[line]);
                (line, looks.map(|(_, look)| look).collect())
            })
            .collect()
    }

    const CODE: Highlight = Highlight::Markdown(Markdown::Code);
    const FENCE: Highlight = Highlight::Markdown(Markdown::Fence);

    #[test]
    fn a_fence_above_the_view_colours_the_lines_below_it() {
        let lines = ["```", "a", "b", "c", "```", "*d*"];
        let mut highlighter = MarkupHighlighter::new(&markdown_on());
        // Scrolled down to lines 2 and 3, iced still colours from the top.
        let drawn = draw(&mut highlighter, &lines, 3);
        assert_eq!(drawn[0], (0, vec![FENCE]));
        assert_eq!(drawn[3], (3, vec![CODE]));
        // Scrolling further carries on from where it stopped.
        assert_eq!(
            draw(&mut highlighter, &lines, 5),
            vec![
                (4, vec![FENCE]),
                (5, vec![Highlight::Markdown(Markdown::Italic)])
            ]
        );
    }

    #[test]
    fn typing_or_deleting_a_fence_recolours_everything_after_it() {
        let mut lines = ["intro", "```", "a", "b", "```", "after"];
        let mut highlighter = MarkupHighlighter::new(&markdown_on());
        draw(&mut highlighter, &lines, 5);
        // Deleting the opening fence: iced says line 1 changed, and draws from there.
        lines[1] = "`x`";
        highlighter.change_line(1);
        let drawn = draw(&mut highlighter, &lines, 5);
        assert_eq!(drawn.first(), Some(&(1, vec![CODE])), "now a code span");
        assert_eq!(drawn[1], (2, vec![]));
        assert_eq!(drawn[3], (4, vec![FENCE]), "now opens a fence");
        assert_eq!(drawn[4], (5, vec![CODE]));
        // Typing it back, with only the lines in view drawn (lines 0 to 2).
        lines[1] = "```";
        highlighter.change_line(1);
        assert_eq!(
            draw(&mut highlighter, &lines, 2),
            vec![(1, vec![FENCE]), (2, vec![CODE])]
        );
        // An edit below where colouring has got to leaves it where it is.
        highlighter.change_line(4);
        assert_eq!(highlighter.current_line(), 3);
        assert_eq!(draw(&mut highlighter, &lines, 5)[2], (5, vec![]));
    }

    #[test]
    fn markdown_then_tags_then_suggestions_and_none_outside_markdown() {
        let line = "# Hi <b>";
        let colouring = Colouring {
            syntax: Syntax::Markdown,
            markup: true,
            review: vec![(0, 2..4, Highlight::Ghost)],
        };
        let mut highlighter = MarkupHighlighter::new(&colouring);
        let found: Vec<_> = highlighter.highlight_line(line).collect();
        assert_eq!(
            found,
            vec![
                (0..8, Highlight::Markdown(Markdown::Heading)),
                (5..8, Highlight::Markup(Markup::Tag)),
                (2..4, Highlight::Ghost),
            ]
        );
        // Turning Markdown off (another file) starts again from the top, without it.
        highlighter.update(&Colouring {
            syntax: Syntax::Plain,
            markup: true,
            review: Vec::new(),
        });
        assert_eq!(highlighter.current_line(), 0);
        let found: Vec<_> = highlighter.highlight_line("```").collect();
        assert_eq!(found, vec![]);
        assert_eq!(highlighter.highlight_line("# no").count(), 0);
    }

    #[test]
    fn a_macro_inside_a_json_string_is_still_coloured() {
        let colouring = Colouring {
            syntax: Syntax::Json,
            markup: true,
            review: Vec::new(),
        };
        let mut highlighter = MarkupHighlighter::new(&colouring);
        let found: Vec<_> = highlighter
            .highlight_line(r#""greeting": "Hi {{user}}""#)
            .collect();
        assert_eq!(
            found,
            vec![
                (0..10, Highlight::Data(Token::Key)),
                (12..25, Highlight::Data(Token::String)),
                (16..24, Highlight::Markup(Markup::Macro)),
            ]
        );
    }
}
