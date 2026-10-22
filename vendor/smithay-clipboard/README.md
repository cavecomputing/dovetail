# smithay-clipboard 0.7.3, patched for Dovetail

A copy of [smithay-clipboard](https://github.com/Smithay/smithay-clipboard) 0.7.3 (MIT, see
LICENSE), the Wayland clipboard iced uses, put in place of the crates.io release by
`[patch.crates-io]` in the app's Cargo.toml.

Why: Hyprland tells only an app's first Wayland data device about a drag, and the first one is
this clipboard's, which ignored drags. So drops on the window never arrived. The changes, marked
`ours:`, accept a drag of files as a copy and hand its `text/uri-list` to the handler the app sets
with `smithay_clipboard::on_drop` (`src/ui/wayland_drop.rs`). Everything else is as released.

To update: copy the new release's `src/`, `Cargo.toml` (without its `[[example]]`) and `LICENSE`
here, then carry over each `ours:` change.
