# Editor engine design

How the editor holds text, undo and paint, and how paint is drawn. Phases are the build order in
[spec.md](spec.md) section 13. Line numbers below point into the crate sources in
`~/.cargo/registry/src/*/` and were checked against iced 0.14.

> **On `main` (2026-10-18):** idea paint is on the `rewrite` branch with the rest of project
> mode: `project/paint.rs`, paint in `TextModel` and its undo steps, the brush, paint kept through
> cut and paste, gutter stripes and the Rewrite clear. `main` keeps phase 2, the copied widget
> (phase 3's drawing) for find matches and phase 5's review mode, without idea colours or the
> pending stripe.

## The short version

- iced's editor (`Content`) stays in charge of what is on screen: cursor, selection, wrapping,
  clicks, IME. We do not rewrite any of that.
- A plain-Rust `TextModel` in `core/` keeps a copy of the text, the undo history and (from phase 3)
  the paint. After every edit, the app works out the exact change by comparing the old and new text,
  and records it.
- Undo, unindent and the Rewrite clear are changes made by `TextModel` first, then replayed into
  `Content` as "select this range, paste this text". That keeps the scroll position.
- To draw paint, we copy iced's `text_editor.rs` into `src/ui/text_editor.rs` (phase 3), because the
  stock widget hides the text layout. Our additions are small and marked `// ours:`.

## Facts this rests on

- `Content` is `Content(RefCell<Internal<R>>)` with a private `editor` field
  (`iced_widget-0.14.2/src/text_editor.rs:384`). A widget wrapped around it cannot see the layout,
  so paint needs a copy of the widget file.
- Both renderers use one editor type, `iced::advanced::graphics::text::Editor`
  (`iced_renderer-0.14.0/src/fallback.rs:85-96`, `iced_tiny_skia-0.14.1/src/lib.rs:252`,
  `iced_wgpu-0.14.0/src/lib.rs:720`). That type has `buffer() -> &cosmic_text::Buffer`
  (`iced_graphics-0.14.0/src/text/editor.rs:36`).
- From the buffer: `layout_runs()` gives only the visible wrapped lines; each `LayoutRun` has
  `line_i` (paragraph), `line_top` (already scrolled, `cosmic-text-0.15.0/src/buffer.rs:134`) and
  `line_height`. `LayoutRun::highlight(Cursor, Cursor) -> Option<(x, width)>` (`buffer.rs:48`) is
  the same call iced uses to draw the selection.
- `Position.column` is a **byte** index within the line. So `core/` uses byte offsets everywhere.
- `Edit::Paste` deletes the current selection first, and `Edit::Delete` with a selection deletes
  just the selection. So `move_to(range)` then Paste or Delete is a full replace.
- iced's `move_to` keeps the old selection when the cursor it is given has none, so our copy's
  `Content::move_to` ends it first; a cursor read with `cursor()` can then always be put back.
- There is no way to set scroll directly; only `Action::Scroll { lines }` (as `app.rs` already
  does).
- In a `Binding::Sequence`, bindings publish their messages in order (`text_editor.rs:876`), and
  `App` handles them in that order, so a `Custom` message placed before `Cut` sees the selection
  before it is deleted.
- iced draws glyphs and hit-tests clicks without cosmic-text's horizontal scroll, which cosmic-text
  does update to follow the cursor (`buffer.rs:381-401`). That is why text is cut off today when word
  wrap is off.

## Phase 2: `core/text_model.rs` (no iced)

```rust
pub struct Change { pub at: usize, pub removed: String, pub inserted: String } // byte offsets

pub struct TextModel {
    text: String,            // mirror of Content's text, '\n' line endings
    undo: Vec<Step>,         // oldest first, at most MAX_UNDO = 1000; drop the oldest
    redo: Vec<Step>,
    replaying: bool,         // set while undo is copied into Content, so it isn't recorded again
}
struct Step { changes: Vec<Change>, kind: StepKind, at: Instant }
enum StepKind { Typing, Other }

impl TextModel {
    pub fn new(text: String) -> Self;
    pub fn text(&self) -> &str;
    pub fn change_between(old: &str, new: &str, edit_start: usize) -> Option<Change>;
    pub fn record(&mut self, new_text: String, edit_start: usize, now: Instant);
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Change;   // starts its own step
    pub fn unindent(&mut self, lines: RangeInclusive<usize>, tab_spaces: usize) -> Vec<Change>;
    pub fn undo(&mut self) -> Vec<Change>;  // the changes to replay into Content, in order
    pub fn redo(&mut self) -> Vec<Change>;
    pub fn offset(&self, line: usize, column: usize) -> usize; // Position -> byte offset
    pub fn position(&self, offset: usize) -> (usize, usize);   // byte offset -> (line, column)
}
```

**Recording an edit from the widget.** In `App::update`, for `Message::Edit(action)` where
`action.is_edit()`:

1. Read `content.cursor()` before `perform`. `edit_start` is the smaller of the cursor and the
   selection anchor, as a byte offset (`offset(line, column)` on the old text).
2. `content.perform(action)`, then `content.text()` (already called in `text_changed`).
3. `change_between(old, new, edit_start)` trims the common prefix (never past `edit_start`) and the
   common suffix (never into the prefix). The cap matters: typing "a" into "aa" must land where the
   cursor was, or paint would move unpredictably. A Backspace has `edit_start` at the cursor, so the
   removed character is the one before it. Every widget edit (typing, Enter, Backspace, Delete,
   Paste, Cut, IME commit) touches one contiguous region, so one `Change` is always exact.
4. `record` pushes it. It joins the current step only when all of these hold: both are `Typing`
   (a single inserted character, or a single Backspace/Delete), it continues where the last one
   ended, less than 1 second passed, and it does not start a new word (non-space after space).
   Anything else starts a new step. Every new step clears redo.

A `debug_assert_eq!(model.text(), content.text())` after recording catches drift in tests and debug
builds.

**Changes made by the model** (undo, redo, unindent, moving lines, the Rewrite clear): `TextModel`
changes its text and returns `Vec<Change>`. A small `ui` function
`replay(&mut Content, &TextModel, &[Change])` copies each one into `Content`:
`move_to(Cursor { position: end, selection: Some(start) })`, then
`perform(Edit(Paste(Arc::new(inserted))))`, or `Edit(Delete)` when nothing is inserted. It then puts
the caret at the end of the last change. Undo of a step applies its changes in reverse order with
`removed` and `inserted` swapped.

- `unindent` moves out of `ui/editor.rs` into `TextModel`. It returns one `Change` per line, so
  paint inside the lines is untouched. It is now plain text and is tested without iced.
- Ctrl+Z goes to `Binding::Custom(Message::Undo)`, Ctrl+Shift+Z and Ctrl+Y to `Message::Redo`, in
  `key_binding`.
- Enter goes to `Message::NewLine` and Backspace to `Message::Backspace`. `Pane::new_line` and
  `Pane::backspace` work out the edit (a newline and the line's indent; the spaces back to the
  last indent stop) and do it as a widget edit through `Pane::perform`: a paste or delete, over a
  selection they make first where needed. So it is recorded like typing, review mode checks it,
  and a refused edit puts the caret back. Ctrl+Backspace and Ctrl+Delete go to
  `Message::DeleteTo(motion)` the same way: `Pane::delete_to` selects with the widget's own word
  motion, so a word is what Ctrl+← steps over, then deletes.
- In a Markdown file, Enter without Shift goes to `Pane::markdown_new_line` instead. It asks
  `core::markdown::list_enter` (no iced; it reads the caret's line, and scans the fences from the
  top to know whether the line is code) what to do: type a newline and the next item's prefix,
  delete the empty item's marker, or unindent it with `Pane::unindent`. Each goes through the
  same paths as above, so it is one undo step and review mode checks it.
- Alt+↑ and Alt+↓ go to `Message::MoveLines`, a model change: `TextModel::move_lines` swaps the
  lines with the one above or below as one `Change` and one undo step, and `Pane::move_lines`
  replays it and puts the caret and selection back on the same text, a line up or down.
- **Rewrite clear** (spec 7.4) is `replace(0..len, "")` on the scratch model, so Ctrl+Z restores it.
- **Two panes** are two `{ content: Content, model: TextModel }` pairs, a small `Pane` struct in
  `app.rs`. v1 is read-only exactly as `editable` works today.
- `text_changed`, saving and counts read `model.text()`.

Phase 2 keeps the **stock** iced widget. Nothing in phase 2 needs the layout.

Tests at the bottom of the file: prefix cap ("aa" cases, Backspace, forward Delete, paste over a
selection), grouping (a word is one step, Enter breaks it), undo-all restores the original, redo
after undo, a new edit clears redo, the 1000-step cap, unindent over several lines.

## Phase 3: paint in `project/paint.rs` (no iced)

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Span { pub idea: IdeaId, pub start: usize, pub end: usize, pub quote: String }

pub fn shift(spans: &mut Vec<Span>, change: &Change);
pub fn brush(spans: &mut Vec<Span>, range: Range<usize>, idea: IdeaId); // add, merge same idea
pub fn relocate(spans: &mut Vec<Span>, text: &str);                    // after an outside edit
pub fn quotes(spans: &mut [Span], text: &str);                          // refresh before saving
```

Spans are a flat list per version. Each is independent, so two ideas on the same text are two
spans and overlaps cost nothing.

**How a change moves a span** (`shift`), with `removed = at..at + removed.len()` and
`delta = inserted.len() - removed.len()`. For each end at position `p`:

- `p` before `at` stays.
- `p` at or after the end of `removed` moves by `delta`.
- `p` inside `removed`: a start moves to `at`, an end to `at + inserted.len()`. So retyping a word
  inside a painted sentence keeps it painted.
- A span lying wholly inside `removed` is dropped, unless it is exactly `removed` (retyping
  exactly the painted words). So pasting over a paragraph never hands its ideas the pasted text;
  they show as missing until painted again.
- A pure insert grows a span only when strictly inside it. Typing at the edge of paint stays
  unpainted unless a brush is active, with one exception (`continue_word`): text with no spaces
  typed straight after a painted word continues that word, so retyping a span's last word one
  key at a time keeps it painted.
- A span that ends up empty is dropped. The idea then shows as **missing**, which is what the
  Commit check wants.

**Undo restores paint exactly.** `TextModel` gains `pub paint: Vec<Span>` and `pub brush:
Option<IdeaId>` (`core` may use these plain types from `project/`; neither imports iced), and each
`Step` gains `paint_before: Vec<Span>` and `paint_after: Vec<Span>`.
Undo replays the text and sets `paint = paint_before`; redo sets `paint_after`. No guessing what a
deleted span looked like. Painting a selection is a `Step` with no text changes, so it is undoable
too. A copy per step is small (hundreds of spans, at most 1000 steps).

**Other cases.**

- **Brush typing:** after `record` with `brush = Some(id)`, call `brush(&mut paint, at..at +
  inserted.len(), id)`. It joins a touching or overlapping span of the same idea. Switching brush
  mid-sentence starts a new span.
- **Merging paragraphs:** deleting a `\n` is an ordinary change. Nothing special.
- **Moves:** `key_binding` maps Ctrl+X and Ctrl+C to `Binding::Sequence(vec![Binding::Custom(
  Message::Clip(id)), Binding::Cut])` (or `Copy`), in both panes. `Clip` stores `(selected text,
  spans relative to it)` in the draft's `TextModel::clip`. When a recorded insert equals that
  text, those spans are added at `at` instead of the brush's paint.
- **Merging ideas:** relabel B's spans as A, then join same-idea overlaps. Splitting is repainting.
- **Commit:** the draft's spans are copied into the new version, unchanged.
- **Painting old v1:** adds spans to that version's list, and to the draft only where the draft
  has the same words at the same offset (as right after a Rewrite), never elsewhere.

**Manifest format** (one entry per version; byte offsets into that version's text):

```json
"versions": [{ "n": 1, "paint": [{ "idea": 3, "start": 0, "end": 42, "quote": "The fox…" }] }],
"draft": { "paint": [ … ] }
```

`quote` is the painted text at save time. Save order is `draft.txt`, then the manifest.

**Edits made outside the app, or a crash between the two saves.** On load, `relocate` checks each
span: if `text[start..end] == quote` it is kept. Otherwise it moves to the occurrence of `quote`
nearest the old `start`. If there is none, the span is dropped and the Commit check reports the idea
as missing. Nothing is silently attached to the wrong text, and no diff crate is needed.

Tests: every `shift` rule, brush merging, undo of a delete restores the paint, `relocate` with an
unchanged text, a moved quote and a deleted quote, serde round-trip.

## Phase 3: drawing, in `src/ui/text_editor.rs`

Copy `iced_widget-0.14.2/src/text_editor.rs` into `src/ui/text_editor.rs` with an MIT credit
header. Then:

- Rewrite `crate::` imports to `iced::advanced::{clipboard, input_method, layout, mouse, renderer,
  text, widget, Shell, InputMethod}` and `iced::{Background, Color, …}`.
- Delete `Binding`, `KeyPress`, `Status`, `Style`, `Catalog`, `StyleFn` and `default` from the
  copy and re-export them with `pub use iced::widget::text_editor::{…}`. This also avoids needing
  `SmolStr`, which iced does not export. `key_binding` changes only its import.
- Narrow the `Widget` and `From` impls to
  `Renderer: text::Renderer<Editor = iced::advanced::graphics::text::Editor, Font = iced::Font>`.
  This is what makes `internal.editor.buffer()` callable.
- `app.rs` and `editor.rs` switch their `text_editor` import to `crate::ui::text_editor`.
  `MarkupHighlighter` is unchanged.

**Our additions** (about 150 lines, each block marked `// ours:`):

```rust
pub struct PaintRun { pub start: Position, pub end: Position, pub color: Color }
// on TextEditor:
pub fn paint(mut self, runs: Vec<PaintRun>) -> Self;
```

`App::view` builds the runs from `model.paint` with `model.position()` and the idea colours at about
0.3 alpha.

**Draw order** in `draw()`:

1. Background quad, as upstream (ours is transparent).
2. **Gutter stripes.** For each paragraph a run covers, take the top of its first visible
   `LayoutRun` and the bottom of its last. Draw one 3px `fill_quad` per idea, side by side, in the
   left padding.
3. **Paint.** For each `run` in `buffer.layout_runs()` and each `PaintRun` whose lines include
   `run.line_i`, call `run.highlight(Cursor::new(start.line, start.column), Cursor::new(end.line,
   end.column))` (`cosmic_text` is `iced::advanced::graphics::text::cosmic_text`). Fill
   `Rectangle::new(text_origin + Vector::new(x - hscroll, run.line_top), Size::new(width,
   run.line_height))`, intersected with `text_bounds`. Drawn before the text, so it sits under the
   glyphs; overlaps blend through alpha (shown working in the probe).
4. `fill_editor(&editor, text_origin - Vector::new(hscroll, 0.0), color, text_bounds)`.
5. Selection, caret and IME preedit as upstream, shifted by `-hscroll`.

**Horizontal scroll** (fixes the cut-off text when word wrap is off; can land as its own commit any
time after the copy): `hscroll = buffer.scroll().horizontal`, which cosmic-text already keeps
following the cursor. Subtract it in draw (above) and add it to click and drag points before
`Action::Click` / `Action::Drag`.

**Recolouring after several edits:** iced's editor tells the highlighter only the line of the last
edit since the last layout, and an undo or indent replays several. `Content::perform` keeps the
first line any of them changed and the widget's `layout` passes it to `change_line`, as Markdown
colours depend on the lines above (an open code fence).

**Scroll report/restore:** `Content::scroll_line()` returns `buffer().scroll().line`, so scratch
and projects can save the exact top line. Restore stays `Action::Scroll { lines }` after the first
layout, as today.

## Phase 5: suggestions as a review mode

Suggestions never enter `TextModel`, so saves, counts, undo and paint can never contain ghost text.
It works in any editable pane: scratch, a `.txt` and a project's draft.

- `core/review.rs` (no iced) keeps a pane's waiting suggestions: `Review`, in text order and never
  overlapping, each an id, a byte range of the text, the new text and an optional idea. Its
  `DisplayMap` is the text with each suggestion spliced in as pieces: `Text`, `Struck` (what it
  replaces), `Ghost` (the new text) and `Buttons` (room for its mini buttons), and maps offsets
  both ways.
  - The ghost text starts with a zero-width space: cosmic-text shapes a plain ASCII word in one
    font (`shape.rs`, the fast path in `ShapeWord::build`), so italics couldn't start mid-word.
  - The button room is `_` characters drawn invisible, not spaces: cosmic-text splits trailing
    whitespace (even no-break spaces) into words of its own that wrap apart.
- While any suggestion waits, `Pane` shows the display string in `Content` (review mode). Edits
  to the text around the suggestions go through and move them with the text (`Review::shift`), as
  undo, redo and `replace` do (a suggestion an undo, redo or `replace` touches is dropped). An edit
  that would change a suggestion's struck or ghost text, or indent a line with one on it, is put
  back, and the status bar says "Accept or reject the suggestion there first". The widget is
  brought up to date by finding the one changed range between the old and new display strings and
  pasting over it, so the view keeps its place. Tab accepts the suggestion the caret is on (in or
  touching its struck text, ghost text or buttons), Esc rejects it (Esc also does what it always
  does); with the caret anywhere else Tab indents and Esc decides nothing.
- **Accept** is `TextModel::accept`: one undo step (Accept all too), and the new text takes the
  suggestion's idea's paint. **Reject** drops it. Paint runs and selections go through
  `Pane::position` and `Pane::selection`, which map through the display string.
- Drawing: `PaintLook::Ghost` (a 12% fill and a dashed underline) and `PaintLook::Struck` (a line
  through) are drawn in the paint pass; the highlighter (`Colouring`, now always on) makes ghost
  text italic, struck text muted and the button room invisible. `PaintLook::Buttons` marks the
  room: the widget draws Accept and Reject there and hit-tests clicks (`TextEditor::on_decide`).
  In a project the paragraph also gets a dashed "pending" gutter stripe.

## Not built

A rope (a `String` is fine at prompt sizes), right-to-left text, multiple cursors, pixel-exact
scroll restore, per-character paint, and a diff crate. If scattered outside edits ever lose too much
paint, `relocate` is the one place to improve.

## Risks

- The copied widget must be re-copied at each iced upgrade, then the `// ours:` blocks re-applied.
- Recording depends on the text mirror staying equal to `Content`; the `debug_assert` and the tests
  guard it. Any new way of changing `Content` must go through `record` or `replay`.
