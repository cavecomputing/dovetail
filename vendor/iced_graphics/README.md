# iced_graphics 0.14.0, patched for Dovetail

A copy of [iced_graphics](https://github.com/iced-rs/iced/tree/0.14.0/graphics) 0.14.0 (MIT, see
LICENSE), put in place of the crates.io release by `[patch.crates-io]` in the app's Cargo.toml.

Why: iced's text editor colours only the lines in view, and works out which those are by counting
rows from the line the view starts in. When the view starts partway into a wrapped line, the rows
of that line scrolled off the top were not counted, so the last lines in view were never coloured:
a line typed under a `#` heading kept the heading's look (it is split off the heading and keeps its
colours), and a new `## heading` stayed plain. The change, marked `ours:`, is in
`src/text/editor.rs` (`highlight`). Everything else is as released.

To update: copy the new release's `src/`, `fonts/` and `Cargo.toml` here and carry over the
`ours:` change, or drop this copy if iced has fixed it.
