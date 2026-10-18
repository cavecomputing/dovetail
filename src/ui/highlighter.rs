//! How the editor colours its text: XML tags and {{macros}}, and in review mode the struck and
//! ghost text of suggestions. iced asks for the colours one line at a time.

use std::ops::Range;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::{Color, Font, Theme};

use crate::core::markup::{Markup, markup_ranges};
use crate::ui::theme::{MONO, Roles};

/// What the highlighter colours: XML tags and macros when `markup` is on, and in review mode the
/// struck and ghost text and the room for the buttons, as (line, byte columns, look).
#[derive(Debug, Clone, PartialEq)]
pub struct Colouring {
    pub markup: bool,
    pub review: Vec<(usize, Range<usize>, Highlight)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Highlight {
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
        Highlight::Markup(Markup::Tag) | Highlight::Struck => Some(roles.fg_muted),
        Highlight::Markup(Markup::Macro) => Some(roles.accent_cool),
        Highlight::Hidden => Some(Color::TRANSPARENT),
        Highlight::Ghost => None,
    };
    let font = (*highlight == Highlight::Ghost).then_some(Font {
        style: iced::font::Style::Italic,
        ..MONO
    });
    highlighter::Format { color, font }
}

/// iced asks for colours one line at a time; `line` tracks how far it has got.
pub struct MarkupHighlighter {
    line: usize,
    colouring: Colouring,
}

impl Highlighter for MarkupHighlighter {
    type Settings = Colouring;
    type Highlight = Highlight;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Highlight)>;

    fn new(colouring: &Colouring) -> Self {
        MarkupHighlighter {
            line: 0,
            colouring: colouring.clone(),
        }
    }

    fn update(&mut self, colouring: &Colouring) {
        self.line = 0;
        self.colouring = colouring.clone();
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    /// Tags and macros first, so suggestion looks win where they overlap.
    fn highlight_line(&mut self, text: &str) -> Self::Iterator<'_> {
        let mut found = Vec::new();
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
