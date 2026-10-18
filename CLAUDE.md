# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

The app is **Dovetail**, a native Linux text editor written in Rust. On `main` it is a fast,
minimal plain-text editor and scratchpad, with an assistant pane that reads the text and suggests
edits. The full design is in [docs/spec.md](docs/spec.md); read the section you are building
before you start.

Project mode (Rewrite, ideas, paint, versions) lives on the `rewrite` branch, and project work
happens there.

The owner, Matt, is new to Rust. When you report back, explain Rust-specific things in plain words
(what a crate is, why the borrow checker complained, what a command does) rather than assuming he
knows them. [Rust in plain terms](#rust-in-plain-terms) below is the shared vocabulary.

## Git workflow

Matt has given standing permission to **commit and push to `main` as needed**, without asking. This
is the opposite of Cozy's rule, on purpose: there are no outside users yet, and Matt wants the work
to land where he can see it.

- Commit when a piece of work is finished and verified (see [Verification](#how-much-verification-a-change-needs)),
  then `git push`. Don't leave finished work sitting only in the working tree.
- **One commit, one concern.** A commit should be reviewable and revertable on its own, so split
  unrelated work: a bug fix and a docs correction are two commits. Size is not the test; twenty files
  for one concern is still one commit.
- Say in the message what broke and why the fix works, not just what you typed.
- Never commit a red build. `cargo build`, `cargo test` and `cargo clippy` must all pass first.
- Only include files that belong to the task.

[AGENTS.md](AGENTS.md) is a short pointer to this file, not a copy. Edit this file directly.

## No hardcoded app name

The name (Dovetail), the app id, the config folder and the `.desktop` file all come from
**[src/brand.rs](src/brand.rs)**. Renaming the app again must mean editing that one file.

- Never write "Dovetail", `dovetail` or a path containing them anywhere else in `src/`. Use the constants. A grep for those strings outside `brand.rs` should find only docs.
- The binary name in `Cargo.toml` is the one unavoidable exception; it is listed next to the
  constants in `brand.rs` so a rename touches both.

## Least code that does the job

Aim for the smallest diff that completes the task in full. The app stays approachable for someone
learning Rust only while the code stays small and plain.

- Prefer editing existing code over adding new code, and a few lines at the right call site over a
  new helper, module or trait. Add a module when a concern genuinely needs its own home.
- Reuse what is already here: the palette roles in `ui/theme.rs`, the save path in `core/save.rs`, the
  constants in `brand.rs`. A second implementation of something the repo already does is the most
  expensive kind of code.
- **No new crate without a reason you can state in one line.** The spec's list (iced, serde,
  serde_json, toml, directories, tiktoken-rs, git2/gix, reqwest, keyring, notify) is pre-approved;
  anything else, say why in the commit message.
- Leave out what nothing needs yet: config knobs, generic traits with one implementation, `Box<dyn>`
  where a plain enum works, error handling around code that cannot fail.
- Plain Rust over clever Rust. Prefer a `match` to a chain of combinators when it reads better,
  owned `String`s over lifetimes when performance doesn't care, and no macros of our own.
- Deleting code is a legitimate way to finish a task.

This governs the amount of code, not the amount of work. Deliver everything asked for.

## Naming

A name is the cheapest documentation. Make it a concise nameplate for what the thing is or does.

- Prefer the specific noun to the category: `draft`, `save_state`, `brush` over `data`, `value`,
  `item`.
- Name a function for what it returns or does: `word_count()`, `close_allowed()`.
- Rust's conventions are not optional: `snake_case` for functions, variables and modules,
  `CamelCase` for types and enum variants, `SCREAMING_SNAKE_CASE` for constants. `cargo clippy`
  will complain otherwise.
- Don't abbreviate past recognition.

Renaming existing code is its own task. Don't fold a rename into an unrelated change.

## Rust in plain terms

- **Crate**: a Rust package. Our app is one crate; the libraries it uses (iced, serde, ...) are
  crates too, downloaded from crates.io.
- **Cargo**: Rust's build tool and package manager in one. It reads `Cargo.toml` (the list of
  dependencies and settings) and `Cargo.lock` (the exact versions it picked, committed so every build
  is the same).
- **Module**: one `.rs` file (or folder) inside the crate. `mod settings;` in `main.rs` pulls in
  `src/settings.rs`.
- **`target/`**: where compiled output goes. It is large, ignored by git, and safe to delete
  (`cargo clean`); the next build just takes longer.
- The first build downloads and compiles every dependency, which takes a few minutes. Later builds
  only recompile what changed.
- **Debug vs release**: `cargo run` builds a debug binary, quick to compile but slower to run.
  `cargo run --release` takes longer to build and gives the fast binary you'd actually install.
- **`Result` and `?`**: a function that can fail returns `Result<T, E>`; `?` means "if this failed,
  return the error to my caller".
- **`unwrap()` / `expect()`**: crash if the value is an error or missing. Fine in tests; in app code
  only where failure is genuinely impossible, with `expect("why this can't fail")`.

## Build / run / test

```bash
cargo run                         # build (debug) and open the app on the scratch space
cargo run -- notes.txt            # open a .txt file (everything after -- goes to the app)

cargo test                        # run every test
cargo test autosave               # run only tests whose name contains "autosave"
cargo clippy --all-targets        # the linter: catches common mistakes and non-idiomatic code
cargo fmt                         # reformat all code to the standard style (4-space indents)

cargo build --release             # the optimised binary, at target/release/<bin name>
```

Installing for daily use (Linux): `scripts/install.sh` builds the release binary, copies it to
`~/.local/bin`, and installs the app icon and the `.desktop` file, all generated from the constants
in `brand.rs`.

Linux needs a few system libraries to open a window (Ubuntu/Debian package names):
`libxkbcommon-x11-0 libwayland-client0 libvulkan1`. Omarchy/Arch already has them.

### How much verification a change needs

- Every change: `cargo build`, `cargo test` and `cargo clippy --all-targets` with no warnings.
- Logic (saving, counting, undo, finding, suggested edits, the assistant's tools): unit tests next
  to the code, in a `#[cfg(test)] mod tests` block at the bottom of the file. Keep that logic out of
  UI code so it can be tested without a window.
- Anything visual (layout, colours, the status bar, panes): run the app and look. In a headless
  container: `xvfb-run -a -s "-screen 0 1280x800x24" env ICED_BACKEND=tiny-skia cargo run`, then
  screenshot with `import -window root shot.png` (ImageMagick). Compare against the mockups
  (linked from the spec). Never claim a change was checked in the app when it wasn't.

## Architecture

One process per window: launching with no file opens scratch, with a `.txt` opens that file.
Opening another file from inside the app starts a new process of the same binary, so every file
gets its own window and plays well with tiling window managers. A process has exactly one window:
Settings, the Ctrl+P switcher and the menu open over it.

The UI is [iced](https://iced.rs) (Elm-style: `update` handles a `Message` and changes state, `view`
draws the state; nothing else mutates state). Logic modules do not depend on iced, so they can be
tested without a window.

### Folder layout

The code is split by *what it knows about*. The rule that keeps it organised: **`core/` and `ai/`
never import iced**, so all the logic can be tested without opening a window, and `ui/` holds only
drawing and event wiring.

```
Cargo.toml, Cargo.lock   dependencies (see "Rust in plain terms")
assets/fonts/            the bundled JetBrains Mono NL Nerd Font (no ligatures) and its licence
assets/icon/             the app icon (SVGs for the installer, a 64px raw RGBA copy for the window)
docs/                    spec.md (the design), design-tokens.json (the Gruvbox palette)
scripts/install.sh       installs the binary, app icon and .desktop file
src/
  main.rs                entry point: reads the command line, decides what to open
  brand.rs               every name (see above)
  core/                  plain-editor logic, no UI
    paths.rs             where config and scratch live, and where save dialogs start
    settings.rs          settings.toml: load, defaults, save
    save.rs              atomic file writes and the save-state machine (saved/saving/failed)
    scratch.rs           the scratch space: text and cursor, resumed on launch; saving it to a
                         file empties it, keeping the text for the next scratch window's Ctrl+Z
    opened.rs            what a launch opens (scratch or a .txt): the text, the cursor, the lock,
                         and whether it's editable
    lock.rs              OS file locks so two windows never edit the same document
    counts.rs            word and token counts (tokenizer loads in the background)
    find.rs              every match of a find query, ignoring case
    markup.rs            where XML tags and {{macros}} are, for colouring them
    text_file.rs         reading and saving .txt files: line endings, outside-change check
    recent.rs            the recently opened files (data folder), fuzzy matching
    text_model.rs        a pane's text and its undo/redo history, in byte offsets
    review.rs            suggested edits waiting in a pane, and the display text that shows
                         them inside the document (review mode)
  ui/                    everything iced: windows, widgets, styling
    app.rs               the App state, Message enum, update and view
    theme.rs             Gruvbox palette roles, the font and its icon glyphs, shared styles
    editor.rs            the editor pane (widget + text model), key bindings, tag/macro colouring,
                         and review mode (suggestions as ghost text, Accept and Reject)
    text_editor.rs       a copy of iced's text editor widget (draws find matches, suggested edits
                         and their buttons), changes marked `ours:`
    status_bar.rs        the bottom bar (save dot, path, counts, assistant toggle) and its menu
    switcher.rs          the Ctrl+P quick switcher over recent files
    find_bar.rs          the Ctrl+F find bar: its matches, stepping through them, how they're lit
    file_actions.rs      App's handlers for New file (Ctrl+N), Save (Ctrl+S) and Save as
    window_controls.rs   the top strip's window controls and the resize edges
    settings_panel.rs    the Settings panel over the window (Ctrl+,) and how each change applies
    assistant_view.rs    the assistant pane (Ctrl+L, every window): chips, chat, suggestion cards,
                         composer, and how it sits beside or over the document
    assistant_actions.rs App's handlers for the assistant: the slide, the agent loop off the UI
                         thread, cards kept in step with the ghost text, the key, the connection test
  ai/                    the assistant pane's logic, no UI
    client.rs            requests to any OpenAI-compatible server (Responses or Chat Completions)
                         and the streamed reply
    sse.rs               splitting a stream of server-sent events, the format replies come in
    tools.rs             the tools the model may call, run against a snapshot; suggested edits
    agent.rs             a conversation and its agent loop: ask, stream, run tools, repeat
    context.rs           the document snapshot, the standing instructions, the note per question
    key.rs               the API key: the system keyring, or settings.toml when there is none
```

One file per concern, named for that concern. When a file passes a few hundred lines, or starts
doing two jobs, split it inside its folder. In Rust a folder module has a `mod.rs` that lists its
files (`pub mod save;`); keep that list current. Tests sit at the bottom of the file they test, in
`#[cfg(test)] mod tests`.

### Rules the code can't tell you

- **Saving is never lost.** Every write goes to a temp file in the same folder, then is renamed over
  the target (an atomic replace), so a crash mid-save never leaves half a file. Closing a window waits
  for the latest save; if it failed, the window stays open and the status bar says why. There is no
  "unsaved" state and no "save changes?" prompt. Ctrl+S only writes sooner, or in scratch (which has
  no file) asks where to save one.
- **Startup must stay near-instant.** Anything slow (loading the tokenizer, network) happens
  off the UI thread after the first frame. The window shows text first and fills in counts later.
- **Tab never writes a tab character.** It inserts N spaces (default 4, from settings).
- **Colours come from roles, not hex.** `ui/theme.rs` maps the cavecomputing Gruvbox tokens
  ([docs/design-tokens.json](docs/design-tokens.json)) to roles: `bg-surface` window, `bg-page`
  status bar, `bg-raised` cards and chips, `fg`/`fg-muted` text, `accent-text` (yellow) for focus
  and active, aqua for success, `accent-warm` (orange) for warnings. **No red, ever.** Every status
  also carries a word, not colour alone.
- **One font**: JetBrains Mono Nerd Font, in its **NL** (no ligatures) Mono build (bundled in
  [assets/fonts/](assets/fonts/), OFL licensed). The ligature build merged pairs like `</` into one
  glyph, which Matt found cramped and not monospace-feeling, so never switch back to it. It is used
  in the UI and the editor. Nerd Font glyphs are the in-app icons; the only image is the app icon in
  [assets/icon/](assets/icon/).
- **No branding in the UI**: no logo, no app name in the window.
- **Settings** live in `~/.config/<config dir>/settings.toml` (hand-editable), apply instantly, and
  unknown or missing keys fall back to defaults instead of failing.
