# iced_graphics 0.14.0, patched for Dovetail

A copy of [iced_graphics](https://github.com/iced-rs/iced/tree/0.14.0/graphics) 0.14.0 (MIT, see
LICENSE), put in place of the crates.io release by `[patch.crates-io]` in the app's Cargo.toml.

Why: iced's text editor colours each line once, from where it last stopped down to the bottom of
the view, and never looks at a line again unless told it changed. Two things it trusted went wrong,
and the lines missed then stayed wrong until the app was restarted:

- Its count of the lines in view. It counted rows from the line the view starts in, without the
  rows of that line scrolled off the top, so with the view starting partway into a wrapped line the
  last lines in view were never coloured: a line typed under a `#` heading kept the heading's look
  (it is split off the heading and keeps its colours), and a new `## heading` stayed plain.
- The highlighter's note of how far it had got. The widget keeps its highlighter when the app puts
  another document in the editor, so the top of the new document, down to where the old one had
  been coloured, stayed plain.

The change, marked `ours:`, is the body of `highlight` in `src/text/editor.rs`: every line in view
is coloured on every draw. The lines in view are the ones cosmic-text will draw (its
`layout_runs`), the highlighter is told to start again at the top of the view, and the editor is
only touched when a colour actually differs, so a draw that changes nothing costs no repaint.
Everything else is as released.

To update: copy the new release's `src/`, `fonts/` and `Cargo.toml` here and carry over the
`ours:` change. The tests in the app's `src/ui/text_editor.rs` fail without it.
