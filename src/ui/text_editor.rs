//! A copy of iced's multi-line text editor widget, so the app can reach the text layout.
//!
//! Copied from iced_widget 0.14.2 `src/text_editor.rs` (MIT licence, Copyright 2019 Héctor Ramón,
//! Iced contributors). Re-copy it at each iced upgrade, then re-apply the blocks marked `ours:`.
//! What changed from upstream:
//! - `Binding`, `KeyPress`, `Status`, `Style`, `Catalog` and `StyleFn` are iced's own, re-exported.
//! - The renderer must use iced's `graphics::text::Editor`, so the layout (`buffer()`) is reachable.
//! - Horizontal scroll: with wrapping off, text, caret, selection and clicks follow the scroll
//!   cosmic-text already keeps on the cursor (upstream ignores it, so long lines were cut off).
//! - `Content::scroll_line` reports the top line and `Content::scroll_to_line` restores it;
//!   `Content::scroll_rows` says where the view sits, for the scroll bar.
//! - `Content::move_to` with no selection drops the one the editor had (upstream keeps it).
//! - `Content::cursor` reports a double or triple click's selection as the whole words or lines
//!   shown selected (upstream reports only the points clicked and dragged to).
//! - Paint: `TextEditor::paint` draws runs under the glyphs: find matches, and in review mode the
//!   looks of suggested edits (struck, ghost) and each suggestion's Accept and Reject buttons,
//!   drawn and clicked inside the widget (`TextEditor::on_decide`). The widget uses iced's own
//!   theme, for the palette roles.
//! - Line numbers: with `TextEditor::line_numbers` on, each line's number is drawn in the left
//!   padding, which the caller widens by `gutter_width`, on its first row (wrapped rows get none).
use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::graphics;
use iced::advanced::graphics::text::cosmic_text;
use iced::advanced::input_method;
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::renderer;
use iced::advanced::text::editor::Editor as _;
use iced::advanced::text::highlighter::{self, Highlighter};
use iced::advanced::text::{self, LineHeight, Text, Wrapping};
use iced::advanced::widget::{self, Widget, operation};
use iced::advanced::{InputMethod, Shell};
use iced::alignment;
use iced::keyboard;
use iced::time::{Duration, Instant};
use iced::window;
use iced::{
    Color, Element, Event, Length, Padding, Pixels, Point, Rectangle, Size, Vector, border,
};

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::ops;
use std::ops::DerefMut;
use std::sync::Arc;

// ours: word bounds for a double click's selection, as cosmic-text finds them.
use unicode_segmentation::UnicodeSegmentation;
// ours: the palette for the buttons and line numbers, and the width of a character.
use crate::ui::theme::{CHAR_WIDTH_RATIO, Roles, bold};

pub use iced::advanced::text::editor::{
    Action, Cursor, Edit, Line, LineEnding, Motion, Position, Selection,
};
// ours: iced's own types, so key bindings and styles work unchanged.
pub use iced::widget::text_editor::{Binding, Catalog, KeyPress, Status, Style, StyleFn};

/// A multi-line text input.
#[allow(clippy::type_complexity)] // upstream's field types, kept as they are
pub struct TextEditor<'a, Highlighter, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer: text::Renderer,
{
    id: Option<widget::Id>,
    content: &'a Content<Renderer>,
    placeholder: Option<text::Fragment<'a>>,
    font: Option<Renderer::Font>,
    text_size: Option<Pixels>,
    line_height: LineHeight,
    width: Length,
    height: Length,
    min_height: f32,
    max_height: f32,
    padding: Padding,
    wrapping: Wrapping,
    class: Theme::Class<'a>,
    key_binding: Option<Box<dyn Fn(KeyPress) -> Option<Binding<Message>> + 'a>>,
    on_edit: Option<Box<dyn Fn(Action) -> Message + 'a>>,
    highlighter_settings: Highlighter::Settings,
    highlighter_format: fn(&Highlighter::Highlight, &Theme) -> highlighter::Format<Renderer::Font>,
    last_status: Option<Status>,
    // ours: what is drawn under the text, and who hears which suggestions are decided.
    paint: Vec<PaintRun>,
    on_decide: Option<Box<dyn Fn(u64, bool) -> Message + 'a>>,
    // ours: draw each line's number left of the text.
    line_numbers: bool,
}

/// ours: one painted range, in the editor's own positions (the column is a byte index).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintRun {
    pub start: Position,
    pub end: Position,
    /// Its colour, opaque: a fill is drawn see-through.
    pub color: Color,
    pub look: PaintLook,
}

/// ours: how a painted range is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaintLook {
    /// A suggestion's new text: a faint fill and a dashed underline.
    Ghost,
    /// Text a suggestion would replace: a line through it in this colour.
    Struck(Color),
    /// The room after suggestion `id` where its Accept and Reject buttons are drawn.
    Buttons { id: u64 },
    /// A find match: a fill, and an outline in this colour for the selected one.
    Found { outline: Option<Color> },
    /// A caret at `start` (one of Ctrl+D's other places), in its colour.
    Caret,
}

impl<'a, Message, Theme, Renderer> TextEditor<'a, highlighter::PlainText, Message, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: text::Renderer,
{
    /// Creates new [`TextEditor`] with the given [`Content`].
    pub fn new(content: &'a Content<Renderer>) -> Self {
        Self {
            id: None,
            content,
            placeholder: None,
            font: None,
            text_size: None,
            line_height: LineHeight::default(),
            width: Length::Fill,
            height: Length::Shrink,
            min_height: 0.0,
            max_height: f32::INFINITY,
            padding: Padding::new(5.0),
            wrapping: Wrapping::default(),
            class: <Theme as Catalog>::default(),
            key_binding: None,
            on_edit: None,
            highlighter_settings: (),
            highlighter_format: |_highlight, _theme| highlighter::Format::default(),
            last_status: None,
            paint: Vec::new(),
            on_decide: None,
            line_numbers: false,
        }
    }

    /// Sets the [`Id`](widget::Id) of the [`TextEditor`].
    pub fn id(mut self, id: impl Into<widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl<'a, Highlighter, Message, Theme, Renderer>
    TextEditor<'a, Highlighter, Message, Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer: text::Renderer,
{
    /// Sets the placeholder of the [`TextEditor`].
    pub fn placeholder(mut self, placeholder: impl text::IntoFragment<'a>) -> Self {
        self.placeholder = Some(placeholder.into_fragment());
        self
    }

    /// Sets the width of the [`TextEditor`].
    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.width = Length::from(width.into());
        self
    }

    /// Sets the height of the [`TextEditor`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Sets the minimum height of the [`TextEditor`].
    pub fn min_height(mut self, min_height: impl Into<Pixels>) -> Self {
        self.min_height = min_height.into().0;
        self
    }

    /// Sets the maximum height of the [`TextEditor`].
    pub fn max_height(mut self, max_height: impl Into<Pixels>) -> Self {
        self.max_height = max_height.into().0;
        self
    }

    /// Sets the message that should be produced when some action is performed in
    /// the [`TextEditor`].
    ///
    /// If this method is not called, the [`TextEditor`] will be disabled.
    pub fn on_action(mut self, on_edit: impl Fn(Action) -> Message + 'a) -> Self {
        self.on_edit = Some(Box::new(on_edit));
        self
    }

    /// Sets the [`Font`] of the [`TextEditor`].
    ///
    /// [`Font`]: text::Renderer::Font
    pub fn font(mut self, font: impl Into<Renderer::Font>) -> Self {
        self.font = Some(font.into());
        self
    }

    /// Sets the text size of the [`TextEditor`].
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.text_size = Some(size.into());
        self
    }

    /// Sets the [`text::LineHeight`] of the [`TextEditor`].
    pub fn line_height(mut self, line_height: impl Into<text::LineHeight>) -> Self {
        self.line_height = line_height.into();
        self
    }

    /// Sets the [`Padding`] of the [`TextEditor`].
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Sets the [`Wrapping`] strategy of the [`TextEditor`].
    pub fn wrapping(mut self, wrapping: Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    /// Highlights the [`TextEditor`] with the given [`Highlighter`] and
    /// a strategy to turn its highlights into some text format.
    pub fn highlight_with<H: text::Highlighter>(
        self,
        settings: H::Settings,
        to_format: fn(&H::Highlight, &Theme) -> highlighter::Format<Renderer::Font>,
    ) -> TextEditor<'a, H, Message, Theme, Renderer> {
        TextEditor {
            id: self.id,
            content: self.content,
            placeholder: self.placeholder,
            font: self.font,
            text_size: self.text_size,
            line_height: self.line_height,
            width: self.width,
            height: self.height,
            min_height: self.min_height,
            max_height: self.max_height,
            padding: self.padding,
            wrapping: self.wrapping,
            class: self.class,
            key_binding: self.key_binding,
            on_edit: self.on_edit,
            highlighter_settings: settings,
            highlighter_format: to_format,
            last_status: self.last_status,
            paint: self.paint,
            on_decide: self.on_decide,
            line_numbers: self.line_numbers,
        }
    }

    /// ours: draws these runs under the text.
    pub fn paint(mut self, paint: Vec<PaintRun>) -> Self {
        self.paint = paint;
        self
    }

    /// ours: reports a click on a suggestion's Accept (`true`) or Reject (`false`) button, with
    /// the suggestion's id (from its `PaintLook::Buttons` run).
    pub fn on_decide(mut self, on_decide: impl Fn(u64, bool) -> Message + 'a) -> Self {
        self.on_decide = Some(Box::new(on_decide));
        self
    }

    /// ours: draws each line's number in the left padding, right-aligned against the text. The
    /// left padding must leave room for them: add `gutter_width` to it.
    pub fn line_numbers(mut self, on: bool) -> Self {
        self.line_numbers = on;
        self
    }

    /// Sets the closure to produce key bindings on key presses.
    ///
    /// See [`Binding`] for the list of available bindings.
    pub fn key_binding(
        mut self,
        key_binding: impl Fn(KeyPress) -> Option<Binding<Message>> + 'a,
    ) -> Self {
        self.key_binding = Some(Box::new(key_binding));
        self
    }

    /// Sets the style of the [`TextEditor`].
    #[must_use]
    pub fn style(mut self, style: impl Fn(&Theme, Status) -> Style + 'a) -> Self
    where
        Theme::Class<'a>: From<StyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as StyleFn<'a, Theme>).into();
        self
    }

    fn input_method<'b>(
        &self,
        state: &'b State<Highlighter>,
        renderer: &Renderer,
        layout: Layout<'_>,
        horizontal_scroll: f32,
    ) -> InputMethod<&'b str> {
        let Some(Focus {
            is_window_focused: true,
            ..
        }) = &state.focus
        else {
            return InputMethod::Disabled;
        };

        let bounds = layout.bounds();
        let internal = self.content.0.borrow_mut();

        let text_bounds = bounds.shrink(self.padding);
        let translation = text_bounds.position() - Point::ORIGIN;

        let cursor = match internal.editor.selection() {
            Selection::Caret(position) => position,
            Selection::Range(ranges) => ranges.first().cloned().unwrap_or_default().position(),
        };

        let line_height = self
            .line_height
            .to_absolute(self.text_size.unwrap_or_else(|| renderer.default_size()));

        let position = cursor + translation - Vector::new(horizontal_scroll, 0.0); // ours

        InputMethod::Enabled {
            cursor: Rectangle::new(position, Size::new(1.0, f32::from(line_height))),
            purpose: input_method::Purpose::Normal,
            preedit: state.preedit.as_ref().map(input_method::Preedit::as_ref),
        }
    }
}

/// The content of a [`TextEditor`].
pub struct Content<R = iced::Renderer>(RefCell<Internal<R>>)
where
    R: text::Renderer;

struct Internal<R>
where
    R: text::Renderer,
{
    editor: R::Editor,
    // ours: a top line to scroll to at the next layout, from `Content::scroll_to_line`.
    scroll_to: Option<usize>,
    // ours: the first line any edit since the last layout changed. iced's editor keeps only the
    // last edit's, but an undo or an indent can make several edits at once, and Markdown colours
    // depend on the lines above, so colouring must start again from the first.
    changed_from: Option<usize>,
    // ours: what a double or triple click selected, whole words or whole lines, kept while a drag
    // or Shift+arrow grows it. iced's `cursor()` reports such a selection as only the points
    // clicked and dragged to, so `Content::cursor` widens it to what is shown selected.
    whole: Option<Whole>,
}

/// ours: the unit a double click (words) or a triple click (lines) selects in.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Whole {
    Words,
    Lines,
}

impl<R> Content<R>
where
    R: text::Renderer,
{
    /// Creates an empty [`Content`].
    pub fn new() -> Self {
        Self::with_text("")
    }

    /// Creates a [`Content`] with the given text.
    pub fn with_text(text: &str) -> Self {
        Self(RefCell::new(Internal {
            editor: R::Editor::with_text(text),
            scroll_to: None,
            changed_from: None,
            whole: None,
        }))
    }

    /// Performs an [`Action`] on the [`Content`].
    pub fn perform(&mut self, action: Action) {
        let internal = self.0.get_mut();

        // ours: the first line an edit changes is the top of the selection, or the line the
        // caret lands on (Backspace at a line's start joins it to the one above).
        let before = internal.editor.cursor();
        let edit = action.is_edit();
        // ours: a drag, Shift+arrow or scroll keeps a word or line selection's unit; any other
        // action ends it, and so does the selection going.
        let whole = match action {
            Action::SelectWord => Some(Whole::Words),
            Action::SelectLine => Some(Whole::Lines),
            Action::Drag(_) | Action::Select(_) | Action::Scroll { .. } => internal.whole,
            _ => None,
        };
        internal.editor.perform(action);
        internal.whole = whole.filter(|_| internal.editor.cursor().selection.is_some());
        if edit {
            let anchor = before.selection.unwrap_or(before.position);
            let first =
                (before.position.line.min(anchor.line)).min(internal.editor.cursor().position.line);
            internal.changed_from = Some(internal.changed_from.map_or(first, |l| l.min(first)));
        }
    }

    /// Moves the current cursor to reflect the given one.
    pub fn move_to(&mut self, cursor: Cursor) {
        let internal = self.0.get_mut();

        // ours: iced keeps the old selection when `cursor` has none. Moving left ends it, so a
        // cursor read with `cursor()` can always be put back as it was.
        if cursor.selection.is_none() {
            internal.editor.perform(Action::Move(Motion::Left));
        }
        internal.editor.move_to(cursor);
        internal.whole = None;
    }

    /// Returns the current cursor position of the [`Content`].
    pub fn cursor(&self) -> Cursor {
        let internal = self.0.borrow();
        let cursor = internal.editor.cursor();
        // ours: a word or line selection reaches out to the whole words or lines, as cosmic-text
        // finds them for drawing and copying it (`Editor::selection_bounds`).
        let (Some(whole), Some(anchor)) = (internal.whole, cursor.selection) else {
            return cursor;
        };
        let key = |at: Position| (at.line, at.column);
        let backwards = key(cursor.position) < key(anchor);
        let (mut start, mut end) = match backwards {
            true => (cursor.position, anchor),
            false => (anchor, cursor.position),
        };
        let text_of = |index| {
            internal
                .editor
                .line(index)
                .map_or(String::new(), |line| line.text.into_owned())
        };
        let end_text = text_of(end.line);
        match whole {
            Whole::Lines => (start.column, end.column) = (0, end_text.len()),
            Whole::Words => {
                start.column = text_of(start.line)
                    .unicode_word_indices()
                    .map(|(at, _)| at)
                    .rfind(|&at| at < start.column)
                    .unwrap_or(0);
                end.column = end_text
                    .unicode_word_indices()
                    .map(|(at, word)| at + word.len())
                    .find(|&at| at > end.column)
                    .unwrap_or(end_text.len());
            }
        }
        match backwards {
            true => Cursor {
                position: start,
                selection: Some(end),
            },
            false => Cursor {
                position: end,
                selection: Some(start),
            },
        }
    }

    /// Returns the amount of lines of the [`Content`].
    pub fn line_count(&self) -> usize {
        self.0.borrow().editor.line_count()
    }

    /// Returns the text of the line at the given index, if it exists.
    pub fn line(&self, index: usize) -> Option<Line<'_>> {
        let internal = self.0.borrow();
        let line = internal.editor.line(index)?;

        Some(Line {
            text: Cow::Owned(line.text.into_owned()),
            ending: line.ending,
        })
    }

    /// Returns an iterator of the text of the lines in the [`Content`].
    pub fn lines(&self) -> impl Iterator<Item = Line<'_>> {
        (0..)
            .map(|i| self.line(i))
            .take_while(Option::is_some)
            .flatten()
    }

    /// Returns the text of the [`Content`].
    pub fn text(&self) -> String {
        let mut contents = String::new();
        let mut lines = self.lines().peekable();

        while let Some(line) = lines.next() {
            contents.push_str(&line.text);

            if lines.peek().is_some() {
                contents.push_str(if line.ending == LineEnding::None {
                    LineEnding::default().as_str()
                } else {
                    line.ending.as_str()
                });
            }
        }

        contents
    }

    /// Returns the selected text of the [`Content`].
    pub fn selection(&self) -> Option<String> {
        self.0.borrow().editor.copy()
    }

    /// Returns the kind of [`LineEnding`] used for separating lines in the [`Content`].
    pub fn line_ending(&self) -> Option<LineEnding> {
        Some(self.line(0)?.ending)
    }

    /// Returns whether or not the the [`Content`] is empty.
    pub fn is_empty(&self) -> bool {
        self.0.borrow().editor.is_empty()
    }
}

// ours: reading and restoring the scroll position.
impl<R> Content<R>
where
    R: text::Renderer<Editor = graphics::text::Editor>,
{
    /// The line (paragraph) at the top of the view.
    pub fn scroll_line(&self) -> usize {
        self.0.borrow().editor.buffer().scroll().line
    }

    /// Scrolls so `line` is at the top of the view, at the next layout (it needs the text's
    /// layout, which the widget makes).
    pub fn scroll_to_line(&mut self, line: usize) {
        self.0.get_mut().scroll_to = Some(line);
    }

    /// Where the view sits, for the scroll bar: its top row, the rows it shows and the rows in
    /// the whole text. cosmic-text lays out only lines it has shown, so one it hasn't counts as
    /// one row, however far it would wrap.
    pub fn scroll_rows(&self) -> ScrollRows {
        let internal = self.0.borrow();
        let buffer = internal.editor.buffer();
        let row_height = buffer.metrics().line_height;
        let rows = |line: &cosmic_text::BufferLine| line.layout_opt().map_or(1, Vec::len);
        let scroll = buffer.scroll();
        let above: usize = buffer.lines.iter().take(scroll.line).map(rows).sum();
        ScrollRows {
            top: above as f32 + scroll.vertical / row_height,
            shown: buffer.size().1.unwrap_or(0.0) / row_height,
            total: buffer.lines.iter().map(rows).sum::<usize>() as f32,
        }
    }
}

/// ours: where the view sits in the text, in rows (`Content::scroll_rows`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollRows {
    pub top: f32,
    pub shown: f32,
    pub total: f32,
}

/// ours: how far long lines are scrolled sideways. cosmic-text keeps this on the cursor, but iced
/// draws and hit-tests without it. With wrapping on it stays at zero.
fn horizontal_scroll(editor: &graphics::text::Editor, wrapping: Wrapping) -> f32 {
    if wrapping == Wrapping::None {
        editor.buffer().scroll().horizontal
    } else {
        0.0
    }
}

// ours: drawing suggestions and find matches.
const GHOST_ALPHA: f32 = 0.12;
const FOUND_ALPHA: f32 = 0.24;
const PAINT_RADIUS: f32 = 3.0;
const DASH: f32 = 4.0;
const DASH_GAP: f32 = 3.0;
const DASH_THICKNESS: f32 = 1.5;

/// ours: the top and height of a row's text box: like a CSS inline background, the text's box,
/// not the whole (tall) line.
fn text_box(
    row: &cosmic_text::LayoutRun<'_>,
    text_bounds: Rectangle,
    text_size: f32,
) -> (f32, f32) {
    let height = (text_size * 1.25 + 2.0).min(row.line_height);
    (
        text_bounds.y + row.line_top + (row.line_height - height) / 2.0,
        height,
    )
}

/// ours: a dashed line across from `start`, `length` long, as small rectangles.
fn dashed(start: Point, length: f32) -> Vec<Rectangle> {
    let mut dashes = Vec::new();
    let mut along = 0.0;
    while along < length {
        let dash = Size::new(DASH.min(length - along), DASH_THICKNESS);
        dashes.push(Rectangle::new(Point::new(start.x + along, start.y), dash));
        along += DASH + DASH_GAP;
    }
    dashes
}

/// ours: draws `paint` on the visible rows only.
fn draw_paint<R: renderer::Renderer>(
    renderer: &mut R,
    buffer: &cosmic_text::Buffer,
    paint: &[PaintRun],
    text_bounds: Rectangle,
    scroll_x: f32,
    text_size: f32,
) {
    // Fill a rectangle clipped to the text area.
    let mut fill = |rect: Rectangle, radius: border::Radius, color: Color| {
        if let Some(bounds) = text_bounds.intersection(&rect) {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: border::rounded(radius),
                    ..renderer::Quad::default()
                },
                color,
            );
        }
    };
    let square = border::Radius::default();
    for row in buffer.layout_runs() {
        let (top, box_height) = text_box(&row, text_bounds, text_size);
        for run in paint {
            if run.start.line > row.line_i || run.end.line < row.line_i {
                continue;
            }
            if run.look == PaintLook::Caret {
                if let Some(x) = caret_x(&row, run.start) {
                    let x = text_bounds.x + x - scroll_x;
                    let caret = Rectangle::new(Point::new(x, top), Size::new(1.0, box_height));
                    fill(caret, square, run.color);
                }
                continue;
            }
            let start = cosmic_text::Cursor::new(run.start.line, run.start.column);
            let end = cosmic_text::Cursor::new(run.end.line, run.end.column);
            let Some((x, width)) = row.highlight(start, end).filter(|(_, width)| *width > 0.0)
            else {
                continue;
            };
            let x = text_bounds.x + x - scroll_x;
            match run.look {
                PaintLook::Buttons { .. } | PaintLook::Caret => {}
                PaintLook::Found { outline: color } => {
                    let outline = Rectangle::new(Point::new(x, top), Size::new(width, box_height));
                    let lit = run.color.scale_alpha(FOUND_ALPHA);
                    fill(outline, border::Radius::new(PAINT_RADIUS), lit);
                    let Some(color) = color else { continue };
                    let sides = [
                        Rectangle::new(outline.position(), Size::new(outline.width, 1.0)),
                        Rectangle::new(
                            Point::new(outline.x, outline.y + outline.height - 1.0),
                            Size::new(outline.width, 1.0),
                        ),
                        Rectangle::new(outline.position(), Size::new(1.0, outline.height)),
                        Rectangle::new(
                            Point::new(outline.x + outline.width - 1.0, outline.y),
                            Size::new(1.0, outline.height),
                        ),
                    ];
                    for side in sides {
                        fill(side, square, color);
                    }
                }
                PaintLook::Struck(color) => {
                    let y = text_bounds.y + row.line_y - text_size * 0.3;
                    let line = Rectangle::new(Point::new(x, y), Size::new(width, 1.0));
                    fill(line, square, color);
                }
                PaintLook::Ghost => {
                    let ghost = Rectangle::new(Point::new(x, top), Size::new(width, box_height));
                    let faint = run.color.scale_alpha(GHOST_ALPHA);
                    fill(ghost, border::Radius::new(PAINT_RADIUS), faint);
                    let bottom = Point::new(x, top + box_height - DASH_THICKNESS);
                    for dash in dashed(bottom, width) {
                        fill(dash, square, run.color);
                    }
                }
            }
        }
    }
}

/// ours: how far along `row` a caret at `at` sits, if it's on that row: before the glyph starting
/// there, or after the row's last glyph.
fn caret_x(row: &cosmic_text::LayoutRun<'_>, at: Position) -> Option<f32> {
    if row.line_i != at.line {
        return None;
    }
    let Some(last) = row.glyphs.last() else {
        return (at.column == 0).then_some(0.0);
    };
    match row.glyphs.iter().find(|glyph| glyph.end > at.column) {
        Some(glyph) if glyph.start <= at.column => Some(glyph.x),
        _ if last.end == at.column => Some(last.x + last.w),
        _ => None,
    }
}

/// ours: where each suggestion's Accept and Reject buttons are, on the first visible row of the
/// room left for them (its `PaintLook::Buttons` run).
fn suggestion_buttons(
    buffer: &cosmic_text::Buffer,
    paint: &[PaintRun],
    text_bounds: Rectangle,
    scroll_x: f32,
    text_size: f32,
) -> Vec<(u64, Rectangle, Rectangle)> {
    let mut buttons: Vec<(u64, Rectangle, Rectangle)> = Vec::new();
    let gap = text_size * 0.3;
    for row in buffer.layout_runs() {
        let (top, height) = text_box(&row, text_bounds, text_size);
        for run in paint {
            let PaintLook::Buttons { id } = run.look else {
                continue;
            };
            if run.start.line > row.line_i
                || run.end.line < row.line_i
                || buttons.iter().any(|(found, ..)| *found == id)
            {
                continue;
            }
            let start = cosmic_text::Cursor::new(run.start.line, run.start.column);
            let end = cosmic_text::Cursor::new(run.end.line, run.end.column);
            if let Some((x, width)) = row.highlight(start, end)
                && width > gap * 3.0
            {
                let left = text_bounds.x + x - scroll_x + gap;
                let size = Size::new((width - gap * 3.0) / 2.0, height);
                let accept = Rectangle::new(Point::new(left, top), size);
                let reject = Rectangle::new(Point::new(left + size.width + gap, top), size);
                buttons.push((id, accept, reject));
            }
        }
    }
    buttons
}

// ours: line numbers, vim's `set number`.
/// The gap between a line's number and its text, in characters.
const GUTTER_GAP: f32 = 2.0;

/// The room line numbers take left of the text: the widest number, then the gap.
pub fn gutter_width(line_count: usize, text_size: f32) -> f32 {
    let digits = line_count.max(1).to_string().len() as f32;
    (digits + GUTTER_GAP) * text_size * CHAR_WIDTH_RATIO
}

impl<Renderer> Clone for Content<Renderer>
where
    Renderer: text::Renderer,
{
    fn clone(&self) -> Self {
        Self::with_text(&self.text())
    }
}

impl<Renderer> Default for Content<Renderer>
where
    Renderer: text::Renderer,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Renderer> fmt::Debug for Content<Renderer>
where
    Renderer: text::Renderer,
    Renderer::Editor: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let internal = self.0.borrow();

        f.debug_struct("Content")
            .field("editor", &internal.editor)
            .finish()
    }
}

/// The state of a [`TextEditor`].
#[derive(Debug)]
pub struct State<Highlighter: text::Highlighter> {
    focus: Option<Focus>,
    preedit: Option<input_method::Preedit>,
    last_click: Option<mouse::Click>,
    drag_click: Option<mouse::click::Kind>,
    partial_scroll: f32,
    last_theme: RefCell<Option<String>>,
    highlighter: RefCell<Highlighter>,
    highlighter_settings: Highlighter::Settings,
    highlighter_format_address: usize,
    // ours: flips every frame drawn while scrolled sideways (see `draw`).
    repaint_flip: std::cell::Cell<bool>,
}

#[derive(Debug, Clone)]
struct Focus {
    updated_at: Instant,
    now: Instant,
    is_window_focused: bool,
}

impl Focus {
    const CURSOR_BLINK_INTERVAL_MILLIS: u128 = 500;

    fn now() -> Self {
        let now = Instant::now();

        Self {
            updated_at: now,
            now,
            is_window_focused: true,
        }
    }

    fn is_cursor_visible(&self) -> bool {
        self.is_window_focused
            && ((self.now - self.updated_at).as_millis() / Self::CURSOR_BLINK_INTERVAL_MILLIS)
                .is_multiple_of(2)
    }
}

impl<Highlighter: text::Highlighter> operation::Focusable for State<Highlighter> {
    fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    fn focus(&mut self) {
        self.focus = Some(Focus::now());
    }

    fn unfocus(&mut self) {
        self.focus = None;
    }
}

// ours: iced's own editor, so its text layout (`buffer()`) can be read, and iced's own theme,
// so the suggestion buttons can use the palette roles.
impl<Highlighter, Message, Renderer> Widget<Message, iced::Theme, Renderer>
    for TextEditor<'_, Highlighter, Message, iced::Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Renderer: text::Renderer<Editor = graphics::text::Editor, Font = iced::Font>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State<Highlighter>>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State {
            focus: None,
            preedit: None,
            last_click: None,
            drag_click: None,
            partial_scroll: 0.0,
            last_theme: RefCell::default(),
            highlighter: RefCell::new(Highlighter::new(&self.highlighter_settings)),
            highlighter_settings: self.highlighter_settings.clone(),
            highlighter_format_address: self.highlighter_format as usize,
            repaint_flip: std::cell::Cell::new(false),
        })
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let mut internal = self.content.0.borrow_mut();
        let state = tree.state.downcast_mut::<State<Highlighter>>();

        if state.highlighter_format_address != self.highlighter_format as usize {
            state.highlighter.borrow_mut().change_line(0);

            state.highlighter_format_address = self.highlighter_format as usize;
        }

        if state.highlighter_settings != self.highlighter_settings {
            state
                .highlighter
                .borrow_mut()
                .update(&self.highlighter_settings);

            state.highlighter_settings = self.highlighter_settings.clone();
        }

        // ours: colour again from the first line edited since the last layout.
        if let Some(line) = internal.changed_from.take() {
            state.highlighter.borrow_mut().change_line(line);
        }

        let limits = limits
            .width(self.width)
            .height(self.height)
            .min_height(self.min_height)
            .max_height(self.max_height);

        internal.editor.update(
            limits.shrink(self.padding).max(),
            self.font.unwrap_or_else(|| renderer.default_font()),
            self.text_size.unwrap_or_else(|| renderer.default_size()),
            self.line_height,
            self.wrapping,
            state.highlighter.borrow_mut().deref_mut(),
        );

        // ours: a saved top line is restored once the text has a layout. Scrolling far up
        // makes cosmic-text lay out every line above on its way to the top; from there the
        // visual lines down to `top` can be counted exactly. The caret is drawn from its line's
        // layout, which only lines in view are sure to keep, so if the saved top would leave
        // the cursor out of view (a smaller window, say), the cursor's line goes at the top.
        if let Some(saved_top) = internal.scroll_to.take() {
            let update = |editor: &mut graphics::text::Editor, lines| {
                editor.perform(Action::Scroll { lines });
                editor.update(
                    limits.shrink(self.padding).max(),
                    self.font.unwrap_or_else(|| renderer.default_font()),
                    self.text_size.unwrap_or_else(|| renderer.default_size()),
                    self.line_height,
                    self.wrapping,
                    state.highlighter.borrow_mut().deref_mut(),
                );
            };
            let cursor_line = internal.editor.cursor().position.line;
            for top in [saved_top, cursor_line] {
                update(&mut internal.editor, -1_000_000);
                let buffer = internal.editor.buffer();
                let top = top.min(buffer.lines.len().saturating_sub(1));
                let lines: usize = buffer.lines[..top]
                    .iter()
                    .map(|line| line.layout_opt().map_or(1, Vec::len))
                    .sum();
                update(&mut internal.editor, lines as i32);
                let buffer = internal.editor.buffer();
                if buffer.layout_runs().any(|run| run.line_i == cursor_line) {
                    break;
                }
            }
        }

        match self.height {
            Length::Fill | Length::FillPortion(_) | Length::Fixed(_) => {
                layout::Node::new(limits.max())
            }
            Length::Shrink => {
                let min_bounds = internal.editor.min_bounds();

                layout::Node::new(
                    limits
                        .height(min_bounds.height)
                        .max()
                        .expand(Size::new(0.0, self.padding.y())),
                )
            }
        }
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Some(on_edit) = self.on_edit.as_ref() else {
            return;
        };

        let state = tree.state.downcast_mut::<State<Highlighter>>();
        let is_redraw = matches!(event, Event::Window(window::Event::RedrawRequested(_now)),);

        // ours: a click on a suggestion's Accept or Reject button decides it, and goes no further.
        if let Some(on_decide) = self.on_decide.as_ref()
            && let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && let Some(point) = cursor.position()
        {
            let internal = self.content.0.borrow();
            let size = self.text_size.unwrap_or_else(|| renderer.default_size());
            let buttons = suggestion_buttons(
                internal.editor.buffer(),
                &self.paint,
                layout.bounds().shrink(self.padding),
                horizontal_scroll(&internal.editor, self.wrapping),
                size.0,
            );
            for (id, accept, reject) in buttons {
                if accept.contains(point) || reject.contains(point) {
                    shell.publish(on_decide(id, accept.contains(point)));
                    shell.capture_event();
                    return;
                }
            }
        }

        match event {
            Event::Window(window::Event::Unfocused) => {
                if let Some(focus) = &mut state.focus {
                    focus.is_window_focused = false;
                }
            }
            Event::Window(window::Event::Focused) => {
                if let Some(focus) = &mut state.focus {
                    focus.is_window_focused = true;
                    focus.updated_at = Instant::now();

                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                if let Some(focus) = &mut state.focus
                    && focus.is_window_focused
                {
                    focus.now = *now;

                    let millis_until_redraw = Focus::CURSOR_BLINK_INTERVAL_MILLIS
                        - (focus.now - focus.updated_at).as_millis()
                            % Focus::CURSOR_BLINK_INTERVAL_MILLIS;

                    shell.request_redraw_at(
                        focus.now + Duration::from_millis(millis_until_redraw as u64),
                    );
                }
            }
            _ => {}
        }

        if let Some(update) = Update::from_event(
            event,
            state,
            layout.bounds(),
            self.padding,
            cursor,
            self.key_binding.as_deref(),
        ) {
            // ours: clicks land on the text as drawn, scrolled sideways.
            let scrolled = Vector::new(
                horizontal_scroll(&self.content.0.borrow().editor, self.wrapping),
                0.0,
            );
            match update {
                Update::Click(click) => {
                    let action = match click.kind() {
                        mouse::click::Kind::Single => Action::Click(click.position() + scrolled),
                        mouse::click::Kind::Double => Action::SelectWord,
                        mouse::click::Kind::Triple => Action::SelectLine,
                    };

                    state.focus = Some(Focus::now());
                    state.last_click = Some(click);
                    state.drag_click = Some(click.kind());

                    shell.publish(on_edit(action));
                    shell.capture_event();
                }
                Update::Drag(position) => {
                    shell.publish(on_edit(Action::Drag(position + scrolled)));
                }
                Update::Release => {
                    state.drag_click = None;
                }
                Update::Scroll(lines) => {
                    let bounds = self.content.0.borrow().editor.bounds();

                    if bounds.height >= i32::MAX as f32 {
                        return;
                    }

                    let lines = lines + state.partial_scroll;
                    state.partial_scroll = lines.fract();

                    shell.publish(on_edit(Action::Scroll {
                        lines: lines as i32,
                    }));
                    shell.capture_event();
                }
                Update::InputMethod(update) => match update {
                    Ime::Toggle(is_open) => {
                        state.preedit = is_open.then(input_method::Preedit::new);

                        shell.request_redraw();
                    }
                    Ime::Preedit { content, selection } => {
                        state.preedit = Some(input_method::Preedit {
                            content,
                            selection,
                            text_size: self.text_size,
                        });

                        shell.request_redraw();
                    }
                    Ime::Commit(text) => {
                        shell.publish(on_edit(Action::Edit(Edit::Paste(Arc::new(text)))));
                    }
                },
                Update::Binding(binding) => {
                    fn apply_binding<H: text::Highlighter, R: text::Renderer, Message>(
                        binding: Binding<Message>,
                        content: &Content<R>,
                        state: &mut State<H>,
                        on_edit: &dyn Fn(Action) -> Message,
                        clipboard: &mut dyn Clipboard,
                        shell: &mut Shell<'_, Message>,
                    ) {
                        let mut publish = |action| shell.publish(on_edit(action));

                        match binding {
                            Binding::Unfocus => {
                                state.focus = None;
                                state.drag_click = None;
                            }
                            Binding::Copy => {
                                if let Some(selection) = content.selection() {
                                    clipboard.write(clipboard::Kind::Standard, selection);
                                }
                            }
                            Binding::Cut => {
                                if let Some(selection) = content.selection() {
                                    clipboard.write(clipboard::Kind::Standard, selection);

                                    publish(Action::Edit(Edit::Delete));
                                }
                            }
                            Binding::Paste => {
                                if let Some(contents) = clipboard.read(clipboard::Kind::Standard) {
                                    publish(Action::Edit(Edit::Paste(Arc::new(contents))));
                                }
                            }
                            Binding::Move(motion) => {
                                publish(Action::Move(motion));
                            }
                            Binding::Select(motion) => {
                                publish(Action::Select(motion));
                            }
                            Binding::SelectWord => {
                                publish(Action::SelectWord);
                            }
                            Binding::SelectLine => {
                                publish(Action::SelectLine);
                            }
                            Binding::SelectAll => {
                                publish(Action::SelectAll);
                            }
                            Binding::Insert(c) => {
                                publish(Action::Edit(Edit::Insert(c)));
                            }
                            Binding::Enter => {
                                publish(Action::Edit(Edit::Enter));
                            }
                            Binding::Backspace => {
                                publish(Action::Edit(Edit::Backspace));
                            }
                            Binding::Delete => {
                                publish(Action::Edit(Edit::Delete));
                            }
                            Binding::Sequence(sequence) => {
                                for binding in sequence {
                                    apply_binding(
                                        binding, content, state, on_edit, clipboard, shell,
                                    );
                                }
                            }
                            Binding::Custom(message) => {
                                shell.publish(message);
                            }
                        }
                    }

                    if !matches!(binding, Binding::Unfocus) {
                        shell.capture_event();
                    }

                    apply_binding(binding, self.content, state, on_edit, clipboard, shell);

                    if let Some(focus) = &mut state.focus {
                        focus.updated_at = Instant::now();
                    }
                }
            }
        }

        let status = {
            let is_disabled = self.on_edit.is_none();
            let is_hovered = cursor.is_over(layout.bounds());

            if is_disabled {
                Status::Disabled
            } else if state.focus.is_some() {
                Status::Focused { is_hovered }
            } else if is_hovered {
                Status::Hovered
            } else {
                Status::Active
            }
        };

        if is_redraw {
            self.last_status = Some(status);

            let horizontal_scroll =
                horizontal_scroll(&self.content.0.borrow().editor, self.wrapping); // ours
            shell.request_input_method(&self.input_method(
                state,
                renderer,
                layout,
                horizontal_scroll,
            ));
        } else if self
            .last_status
            .is_some_and(|last_status| status != last_status)
        {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        _defaults: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        let mut internal = self.content.0.borrow_mut();
        let state = tree.state.downcast_ref::<State<Highlighter>>();

        let font = self.font.unwrap_or_else(|| renderer.default_font());

        let theme_name = iced::theme::Base::name(theme); // ours: the theme is iced's own

        if state
            .last_theme
            .borrow()
            .as_ref()
            .is_none_or(|last_theme| last_theme != theme_name)
        {
            state.highlighter.borrow_mut().change_line(0);
            let _ = state.last_theme.borrow_mut().replace(theme_name.to_owned());
        }

        internal.editor.highlight(
            font,
            state.highlighter.borrow_mut().deref_mut(),
            |highlight| (self.highlighter_format)(highlight, theme),
        );

        let style = theme.style(&self.class, self.last_status.unwrap_or(Status::Active));

        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: style.border,
                ..renderer::Quad::default()
            },
            style.background,
        );

        let text_bounds = bounds.shrink(self.padding);
        // ours: draw long lines scrolled sideways, as cosmic-text keeps the cursor in view.
        let scrolled = Vector::new(horizontal_scroll(&internal.editor, self.wrapping), 0.0);
        // ours: tiny-skia repaints only where the editor's own rectangle changed, and the
        // sideways shift moves that rectangle left of the text area. While scrolled sideways,
        // an invisible quad that changes every frame makes it repaint the whole text area.
        if scrolled.x > 0.0 {
            let flip = !state.repaint_flip.get();
            state.repaint_flip.set(flip);
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        height: text_bounds.height - if flip { 0.5 } else { 0.0 },
                        ..text_bounds
                    },
                    ..renderer::Quad::default()
                },
                iced::Color::TRANSPARENT,
            );
        }

        // ours: paint goes under the glyphs (a layer draws its quads before its text).
        if !self.paint.is_empty() {
            let size = self.text_size.unwrap_or_else(|| renderer.default_size());
            draw_paint(
                renderer,
                internal.editor.buffer(),
                &self.paint,
                text_bounds,
                scrolled.x,
                size.0,
            );
        }

        if internal.editor.is_empty() {
            if let Some(placeholder) = self.placeholder.clone() {
                renderer.fill_text(
                    Text {
                        content: placeholder.into_owned(),
                        bounds: text_bounds.size(),
                        size: self.text_size.unwrap_or_else(|| renderer.default_size()),
                        line_height: self.line_height,
                        font,
                        align_x: text::Alignment::Default,
                        align_y: alignment::Vertical::Top,
                        shaping: text::Shaping::Advanced,
                        wrapping: self.wrapping,
                    },
                    text_bounds.position(),
                    style.placeholder,
                    text_bounds,
                );
            }
        } else {
            renderer.fill_editor(
                &internal.editor,
                text_bounds.position() - scrolled,
                style.value,
                text_bounds,
            );
        }

        // ours: each visible line's number, on its first row only (a wrapped line's later rows
        // start part way through it), right-aligned against the text in the left padding. The
        // caret's line is in `fg`, the rest in `fg_subtle`, and the gap alone sets them apart
        // from the text, as in vim (no divider, and no yellow to vie with headings). tiny-skia
        // clips a text only to its layer, and only when the text's own clip pokes out of it,
        // hence the layer and each number's own box as its clip. It also repaints a
        // right-aligned text's box as if it started where it ends, so each number is placed by
        // its width in the mono font.
        if self.line_numbers {
            let size = self.text_size.unwrap_or_else(|| renderer.default_size());
            let gutter = Rectangle {
                x: bounds.x,
                width: text_bounds.x - bounds.x,
                ..text_bounds
            };
            let char_width = size.0 * CHAR_WIDTH_RATIO;
            let right = text_bounds.x - GUTTER_GAP * char_width;
            let caret_line = internal.editor.cursor().position.line;
            let roles = Roles::of(theme);
            renderer.with_layer(gutter, |renderer| {
                for row in internal.editor.buffer().layout_runs() {
                    if row.glyphs.first().is_some_and(|glyph| glyph.start > 0) {
                        continue;
                    }
                    let number = (row.line_i + 1).to_string();
                    let left = right - number.len() as f32 * char_width;
                    let number_box = Rectangle::new(
                        Point::new(left, text_bounds.y + row.line_top),
                        Size::new(right - left, row.line_height),
                    );
                    renderer.fill_text(
                        Text {
                            content: number,
                            bounds: number_box.size(),
                            size,
                            line_height: self.line_height,
                            font,
                            align_x: text::Alignment::Left,
                            align_y: alignment::Vertical::Top,
                            shaping: text::Shaping::Basic,
                            wrapping: Wrapping::None,
                        },
                        number_box.position(),
                        match row.line_i == caret_line {
                            true => roles.fg,
                            false => roles.fg_subtle,
                        },
                        number_box,
                    );
                }
            });
        }

        // ours: each suggestion's Accept and Reject buttons, in the room left for them after it,
        // in a layer of their own so they sit over the text and stay inside it.
        let size = self.text_size.unwrap_or_else(|| renderer.default_size());
        let buttons = suggestion_buttons(
            internal.editor.buffer(),
            &self.paint,
            text_bounds,
            scrolled.x,
            size.0,
        );
        if !buttons.is_empty() {
            let roles = Roles::of(theme);
            renderer.with_layer(text_bounds, |renderer| {
                for (_, accept, reject) in buttons {
                    let looks = [
                        (accept, "Accept", roles.btn_bg, roles.btn_bg, roles.btn_fg),
                        (reject, "Reject", roles.bg_raised, roles.border, roles.fg),
                    ];
                    for (bounds, label, fill, edge, color) in looks {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds,
                                border: border::rounded(5).width(1).color(edge),
                                ..renderer::Quad::default()
                            },
                            fill,
                        );
                        renderer.fill_text(
                            Text {
                                content: label.to_string(),
                                bounds: bounds.size(),
                                size: size * 0.73,
                                line_height: LineHeight::default(),
                                font: bold(),
                                align_x: text::Alignment::Center,
                                align_y: alignment::Vertical::Center,
                                shaping: text::Shaping::Basic,
                                wrapping: Wrapping::None,
                            },
                            bounds.center(),
                            color,
                            bounds,
                        );
                    }
                }
            });
        }

        let translation = text_bounds.position() - Point::ORIGIN - scrolled;

        if let Some(focus) = state.focus.as_ref() {
            match internal.editor.selection() {
                Selection::Caret(position) if focus.is_cursor_visible() => {
                    let cursor = Rectangle::new(
                        position + translation,
                        Size::new(
                            1.0,
                            self.line_height
                                .to_absolute(
                                    self.text_size.unwrap_or_else(|| renderer.default_size()),
                                )
                                .into(),
                        ),
                    );

                    if let Some(clipped_cursor) = text_bounds.intersection(&cursor) {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: clipped_cursor,
                                ..renderer::Quad::default()
                            },
                            style.value,
                        );
                    }
                }
                Selection::Range(ranges) => {
                    for range in ranges
                        .into_iter()
                        .filter_map(|range| text_bounds.intersection(&(range + translation)))
                    {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: range,
                                ..renderer::Quad::default()
                            },
                            style.selection,
                        );
                    }
                }
                Selection::Caret(_) => {}
            }
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let is_disabled = self.on_edit.is_none();

        // ours: a hand over a suggestion's buttons.
        if let Some(point) = cursor.position() {
            let internal = self.content.0.borrow();
            let size = self.text_size.unwrap_or_else(|| renderer.default_size());
            let buttons = suggestion_buttons(
                internal.editor.buffer(),
                &self.paint,
                layout.bounds().shrink(self.padding),
                horizontal_scroll(&internal.editor, self.wrapping),
                size.0,
            );
            if buttons
                .iter()
                .any(|(_, accept, reject)| accept.contains(point) || reject.contains(point))
            {
                return mouse::Interaction::Pointer;
            }
        }

        if cursor.is_over(layout.bounds()) {
            if is_disabled {
                mouse::Interaction::NotAllowed
            } else {
                mouse::Interaction::Text
            }
        } else {
            mouse::Interaction::default()
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<State<Highlighter>>();

        operation.focusable(self.id.as_ref(), layout.bounds(), state);
    }
}

// ours: iced's own editor and theme, as for `Widget`.
impl<'a, Highlighter, Message, Renderer>
    From<TextEditor<'a, Highlighter, Message, iced::Theme, Renderer>>
    for Element<'a, Message, iced::Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Message: 'a,
    Renderer: text::Renderer<Editor = graphics::text::Editor, Font = iced::Font>,
{
    fn from(text_editor: TextEditor<'a, Highlighter, Message, iced::Theme, Renderer>) -> Self {
        Self::new(text_editor)
    }
}

enum Update<Message> {
    Click(mouse::Click),
    Drag(Point),
    Release,
    Scroll(f32),
    InputMethod(Ime),
    Binding(Binding<Message>),
}

enum Ime {
    Toggle(bool),
    Preedit {
        content: String,
        selection: Option<ops::Range<usize>>,
    },
    Commit(String),
}

impl<Message> Update<Message> {
    fn from_event<H: Highlighter>(
        event: &Event,
        state: &State<H>,
        bounds: Rectangle,
        padding: Padding,
        cursor: mouse::Cursor,
        key_binding: Option<&dyn Fn(KeyPress) -> Option<Binding<Message>>>,
    ) -> Option<Self> {
        let binding = |binding| Some(Update::Binding(binding));

        match event {
            Event::Mouse(event) => match event {
                mouse::Event::ButtonPressed(mouse::Button::Left) => {
                    if let Some(cursor_position) = cursor.position_in(bounds) {
                        let cursor_position =
                            cursor_position - Vector::new(padding.left, padding.top);

                        let click = mouse::Click::new(
                            cursor_position,
                            mouse::Button::Left,
                            state.last_click,
                        );

                        Some(Update::Click(click))
                    } else if state.focus.is_some() {
                        binding(Binding::Unfocus)
                    } else {
                        None
                    }
                }
                mouse::Event::ButtonReleased(mouse::Button::Left) => Some(Update::Release),
                mouse::Event::CursorMoved { .. } => match state.drag_click {
                    Some(mouse::click::Kind::Single) => {
                        let cursor_position =
                            cursor.position_in(bounds)? - Vector::new(padding.left, padding.top);

                        Some(Update::Drag(cursor_position))
                    }
                    _ => None,
                },
                mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => {
                    Some(Update::Scroll(match delta {
                        mouse::ScrollDelta::Lines { y, .. } => {
                            if y.abs() > 0.0 {
                                y.signum() * -(y.abs() * 4.0).max(1.0)
                            } else {
                                0.0
                            }
                        }
                        mouse::ScrollDelta::Pixels { y, .. } => -y / 4.0,
                    }))
                }
                _ => None,
            },
            Event::InputMethod(event) => match event {
                input_method::Event::Opened | input_method::Event::Closed => Some(
                    Update::InputMethod(Ime::Toggle(matches!(event, input_method::Event::Opened))),
                ),
                input_method::Event::Preedit(content, selection) if state.focus.is_some() => {
                    Some(Update::InputMethod(Ime::Preedit {
                        content: content.clone(),
                        selection: selection.clone(),
                    }))
                }
                input_method::Event::Commit(content) if state.focus.is_some() => {
                    Some(Update::InputMethod(Ime::Commit(content.clone())))
                }
                _ => None,
            },
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                physical_key,
                modifiers,
                text,
                ..
            }) => {
                let status = if state.focus.is_some() {
                    Status::Focused {
                        is_hovered: cursor.is_over(bounds),
                    }
                } else {
                    Status::Active
                };

                let key_press = KeyPress {
                    key: key.clone(),
                    modified_key: modified_key.clone(),
                    physical_key: *physical_key,
                    modifiers: *modifiers,
                    text: text.clone(),
                    status,
                };

                if let Some(key_binding) = key_binding {
                    key_binding(key_press)
                } else {
                    Binding::from_key_press(key_press)
                }
                .map(Self::Binding)
            }
            _ => None,
        }
    }
}

// ours: tests of the changes above.
#[cfg(test)]
mod tests {
    use super::*;

    fn caret(line: usize, column: usize) -> Cursor {
        Cursor {
            position: Position { line, column },
            selection: None,
        }
    }

    #[test]
    fn several_edits_colour_again_from_the_first_line_they_changed() {
        let mut content = Content::with_text("a\nb\nc\nd");
        let changed_from = |content: &Content| content.0.borrow().changed_from;
        content.perform(Action::Move(Motion::Down));
        assert_eq!(changed_from(&content), None, "moving changes nothing");
        // An indent of lines 1 and 3, top line first: iced would only say line 3.
        for line in [1, 3] {
            content.move_to(caret(line, 0));
            content.perform(Action::Edit(Edit::Insert(' ')));
        }
        assert_eq!(changed_from(&content), Some(1));
        // Backspace at the start of line 1 joins it to line 0.
        content.0.get_mut().changed_from = None;
        content.move_to(caret(1, 0));
        content.perform(Action::Edit(Edit::Backspace));
        assert_eq!(changed_from(&content), Some(0));
    }

    #[test]
    fn the_gutter_fits_the_widest_line_number_and_a_gap() {
        let chars =
            |line_count| (gutter_width(line_count, 10.0) / (10.0 * CHAR_WIDTH_RATIO)).round();
        assert_eq!(chars(0), 3.0, "empty text is still line 1");
        assert_eq!(chars(9), 3.0);
        assert_eq!(chars(10), 4.0);
        assert_eq!(chars(120), 5.0);
    }

    #[test]
    fn moving_to_a_caret_drops_the_selection() {
        let mut content: Content = Content::with_text("one two");
        content.perform(Action::Select(Motion::End));
        assert_eq!(
            content.cursor().selection,
            Some(Position { line: 0, column: 0 })
        );
        content.move_to(caret(0, 4));
        assert_eq!(content.cursor(), caret(0, 4));
    }
}
