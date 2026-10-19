# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

The app is **Dovetail**, a native desktop text editor written in Rust. Linux (Matt's
Omarchy/Hyprland machines) comes first, and it also builds and runs on macOS and Windows: a change
for one OS must leave the others working, Linux above all. On `main` it is a fast, minimal
plain-text editor and scratchpad, with an assistant pane that reads the text and suggests edits.
The full design is in [docs/spec.md](docs/spec.md); read the section you are building before you
start.

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

The name (Dovetail), the app id, the config folder, the `.desktop` file, the macOS app bundle and
the Windows Start Menu entry all come from **[src/brand.rs](src/brand.rs)**. Renaming the app
again must mean editing that one file.

- Never write "Dovetail", `dovetail` or a path containing them anywhere else in `src/`. Use the constants. A grep for those strings outside `brand.rs` should find only docs.
- The binary name in `Cargo.toml` is the one unavoidable exception; it is listed next to the
  constants in `brand.rs` so a rename touches both.
- The install scripts in `scripts/` read every name from the binary (`<bin> --print-brand`), so
  they hardcode none either.

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
- **`#[cfg(...)]`**: compile the next item only on some platforms: `#[cfg(unix)]` is Linux and
  macOS, `#[cfg(windows)]` is Windows, `#[cfg(target_os = "macos")]` is one OS. `cfg!(...)` is the
  same test as a plain true or false inside a function, where both branches still have to compile.
  Code behind a cfg for another OS isn't even compiled on this one, which is why CI builds on all
  three.
- **Target**: the OS and processor a build is for, such as `x86_64-pc-windows-gnu`. Cargo builds
  for the machine it runs on unless given `--target`; `rustup target add <target>` downloads the
  standard library another target needs.

## Build / run / test

```bash
cargo run                         # build (debug) and open the app on the scratch space
cargo run -- notes.txt            # open a .txt file (everything after -- goes to the app)

cargo test                        # run every test
cargo test autosave               # run only tests whose name contains "autosave"
cargo clippy --all-targets        # the linter: catches common mistakes and non-idiomatic code
cargo fmt                         # reformat all code to the standard style (4-space indents)

cargo build --release             # the optimised binary, at target/release/<bin name> (.exe on Windows)
```

What each OS needs before the first build:

- **Linux**: Rust (rustup) and a C compiler (`base-devel` on Arch, `build-essential` on Ubuntu),
  for the TLS library under `reqwest`. To open a window it also needs a few system libraries
  (Ubuntu names): `libxkbcommon-x11-0 libwayland-client0 libvulkan1`. Omarchy/Arch already has
  them.
- **macOS**: Rust (rustup) and Xcode's command line tools (`xcode-select --install`), which bring
  the C compiler and linker.
- **Windows**: Rust (rustup-init.exe, which picks the MSVC toolchain) and Visual Studio Build Tools
  with the "Desktop development with C++" workload, for the linker and the Windows SDK;
  rustup-init offers to install them. No NASM or CMake is needed. On ARM64 Windows also add the
  "C++ Clang Compiler for Windows" component. A debug build writes its error messages to the
  terminal it was started from (`cargo run`), or opens a console window for them when started from
  Explorer; the release build has no console.

Installing for daily use, from a checkout. Each script builds the release binary and takes every
name from `brand.rs` through `<bin> --print-brand`:

- **Linux**: `scripts/install.sh` copies the binary to `~/.local/bin` and installs the app icon and
  the `.desktop` file (for plain text, Markdown and config files).
- **Arch**: `packaging/arch/PKGBUILD` builds a `dovetail-git` pacman package from GitHub's `main`
  (`cd packaging/arch && makepkg -si`), installing the same files under `/usr`. Its package name
  has to be written out (makepkg needs it literally); the installed names still come from
  `--print-brand`.
- **macOS**: `scripts/install-macos.sh` makes an app bundle in `~/Applications` (signed ad hoc, so
  no Apple account is needed) and a terminal command in `~/.local/bin`. The bundle can't be a
  file's "Open With" app yet (winit can't receive the Apple Event macOS opens files with), so open
  files from the terminal or from inside the app.
- **Windows**: `powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1` copies the
  exe to `%LOCALAPPDATA%\Programs\<app>` and adds a Start Menu entry and an "Open with" entry for
  `.txt`, Markdown and config files (`.json`, `.yaml`, `.toml`, `.ini`, ...); add `-Uninstall`
  to remove them.

### How much verification a change needs

- Every change: `cargo build`, `cargo test` and `cargo clippy --all-targets` with no warnings.
  CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the same three on Linux, macOS and
  Windows, plus `cargo fmt --check`, on every push. After pushing, check that run (the Actions tab
  on GitHub, or `gh run list`): a red OS is a red build, so fix it before moving on.
- Code that only one OS needs goes behind `#[cfg(unix)]`, `#[cfg(windows)]` or
  `#[cfg(target_os = "...")]` (or `cfg!(...)` inside a function), with a comment saying what the
  other OSes do, even when that is nothing. A test that uses one OS's API gets the same cfg.
- To check the Windows side from Linux or macOS before pushing: `rustup target add x86_64-pc-windows-gnu`,
  install MinGW (`mingw-w64-gcc` on Arch, `gcc-mingw-w64-x86-64` on Ubuntu, `brew install mingw-w64`
  on macOS), then `cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings`.
  `cargo build --release --target x86_64-pc-windows-gnu` makes a Windows exe that needs nothing
  else installed, at `target/x86_64-pc-windows-gnu/release/<bin name>.exe`. With wine installed,
  `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine cargo test --target x86_64-pc-windows-gnu` runs
  the tests too. Only CI builds and tests macOS, so write macOS-only code against the crate sources
  and treat its first CI run as its first real test.
- Logic (saving, counting, undo, finding, suggested edits, the assistant's tools): unit tests next
  to the code, in a `#[cfg(test)] mod tests` block at the bottom of the file. Keep that logic out of
  UI code so it can be tested without a window.
- Anything visual (layout, colours, the status bar, panes): run the app and look. In a headless
  container: `xvfb-run -a -s "-screen 0 1280x800x24" env ICED_BACKEND=tiny-skia cargo run`, then
  screenshot with `import -window root shot.png` (ImageMagick). Compare against the mockups
  (linked from the spec). Never claim a change was checked in the app when it wasn't.

## Architecture

One process per window: launching with no file opens scratch, with a file (`.txt`, `.md`, `.json`,
...) opens that file. Each launch gets its own window, which plays well with tiling window managers. Opening another
document from inside the app (Ctrl+O, Ctrl+P, Recent, Ctrl+N, Ctrl+0) replaces the one in the same
window, once the one being left is saved (`App::open_here`). A process has exactly one window:
Settings, the Ctrl+P switcher, the Ctrl+Shift+P palette and the menu open over it.

The UI is [iced](https://iced.rs) (Elm-style: `update` handles a `Message` and changes state, `view`
draws the state; nothing else mutates state). Logic modules do not depend on iced, so they can be
tested without a window.

### Folder layout

The code is split by *what it knows about*. The rule that keeps it organised: **`core/` and `ai/`
never import iced**, so all the logic can be tested without opening a window, and `ui/` holds only
drawing and event wiring.

```
.github/workflows/ci.yml build, test, clippy and fmt on Linux, macOS and Windows for every push
Cargo.toml, Cargo.lock   dependencies (see "Rust in plain terms")
LICENSE                  MIT, the app's licence (the font has its own, OFL)
assets/fonts/            the bundled JetBrains Mono NL Nerd Font (no ligatures) and its licence
assets/icon/             the app icon: SVGs for Linux, icon.icns for macOS, icon.ico for Windows,
                         and a 64px raw RGBA copy for the window (see its README)
docs/                    spec.md (the design), design-tokens.json (the Gruvbox palette)
packaging/arch/          PKGBUILD for a pacman package (dovetail-git) built from GitHub
scripts/                 per-OS installers: install.sh (Linux: binary, icon, .desktop file),
                         install-macos.sh (an app bundle in ~/Applications), install-windows.ps1
                         (Start Menu and Open with entries)
src/
  main.rs                entry point: reads the command line, decides what to open
  brand.rs               every name (see above)
  core/                  plain-editor logic, no UI
    paths.rs             where config and scratch live, and where save dialogs start
    settings.rs          settings.toml: load, defaults, save
    save.rs              atomic file writes and the save-state machine (saved/saving/failed)
    scratch.rs           the scratch space: text and cursor, resumed on launch; saving it to a
                         file empties it, keeping the text for the next scratch window's Ctrl+Z
    opened.rs            what a launch opens (scratch or a file): the text, the cursor, the lock,
                         and whether it's editable
    lock.rs              OS file locks so two windows never edit the same document
    config.rs            YAML, TOML and INI: which parts of a line to colour, the checks, and
                         YAML's indent step
    counts.rs            word and token counts (tokenizer loads in the background)
    find.rs              every match of a find query, ignoring case
    markup.rs            where XML tags and {{macros}} are, for colouring them
    markdown.rs          which parts of a Markdown (.md) line to colour, whether a code fence is
                         open, what Enter and Tab do in a list, and Ctrl+B and Ctrl+I's markers
    syntax.rs            which kind of text a file is (plain, Markdown, JSON, YAML, TOML, INI,
                         shell), by extension or `#!` line; the shared colour tokens; the
                         validity check for each; Ctrl+/'s comments
    shell.rs             shell scripts: which parts of a line to colour, whether a `#!` line
                         names sh or bash, and the `bash -n` check
    json.rs              which parts of a .json line to colour, whether the text is valid JSON,
                         and Format and Minify (whitespace only, so key order and digits are kept)
    text_file.rs         reading and saving text files: line endings, outside-change check
    recent.rs            the recently opened files (data folder), fuzzy matching
    text_model.rs        a pane's text and its undo/redo history, in byte offsets
    typing.rs            typing in shell scripts, JSON and YAML: pairs that close themselves, and
                         Enter that indents a block and lines up the line that ends one
    review.rs            suggested edits waiting in a pane, and the display text that shows
                         them inside the document (review mode)
  ui/                    everything iced: windows, widgets, styling
    app.rs               the App state, Message enum, update and view
    theme.rs             Gruvbox palette roles, the font and its icon glyphs, shared styles
    editor.rs            the editor pane (widget + text model), key bindings, and review mode
                         (suggestions coloured red and green in the text)
    highlighter.rs       how the editor colours its text: Markdown, JSON, config files and shell
                         scripts, tags and macros, suggested edits
    text_editor.rs       a copy of iced's text editor widget (draws find matches, suggested edits
                         and their buttons), changes marked `ours:`
    scroll_bar.rs        the editor's scroll bar, drawn like Settings' (the editor scrolls itself)
    key_guard.rs         keeps Ctrl+digit and Ctrl+punctuation (and ⌘ plus a letter on macOS) from
                         typing into text boxes; widgets still get the key for their shortcuts
    status_bar.rs        the bottom bar (save dot, path, counts, assistant toggle) and its menu
    switcher.rs          the Ctrl+P quick switcher: scratch and the recent files, numbered 0-9
    palette.rs           the Ctrl+Shift+P command palette: the open file's tools (Format JSON...)
    find_bar.rs          the Ctrl+F find bar: its matches, stepping through them, how they're lit
    file_actions.rs      App's handlers for opening a document in the window's place (Ctrl+O,
                         Ctrl+P, Ctrl+0), New file (Ctrl+N), Save (Ctrl+S) and Save as
    window_controls.rs   the top strip's window controls and the resize edges; on macOS, Quit
                         pointed at closing the window
    settings_panel.rs    the Settings panel over the text (Ctrl+,), laid out like settings.toml,
                         and how each change applies
    assistant_view.rs    the assistant pane (Ctrl+L, every window), drawn like a transcript: its
                         # header, the chat, suggested edits as diffs, the > prompt, and how it
                         sits beside or over the document
    assistant_actions.rs App's handlers for the assistant: the slide, the agent loop off the UI
                         thread, cards kept in step with the document, the key, the connection test
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
- **Shortcuts are written Ctrl+X but mean ⌘X on macOS.** Check a shortcut with iced's
  `modifiers.command()` (⌘ on macOS, Ctrl elsewhere), never `control()`; build test key presses
  with `Modifiers::COMMAND`, not `Modifiers::CTRL`; and show a shortcut through
  `status_bar::keys()`, which writes it the macOS way there (`⇧⌘S`).
- **Each OS frames the window its own way** (spec §2). Linux and Windows get an undecorated window
  with our glyph controls in the top strip and our resize edges, both left out under a tiling WM.
  macOS keeps its see-through title bar, with the traffic lights in our strip, and resizes the
  window itself.
- **Colours come from roles, not hex.** `ui/theme.rs` maps the cavecomputing Gruvbox tokens
  ([docs/design-tokens.json](docs/design-tokens.json)) to roles: `bg-surface` window, `bg-page`
  status bar, `bg-raised` cards and chips, `fg`/`fg-muted` text, `accent-text` (yellow) for focus
  and active, aqua for success, `accent-warm` (orange) for warnings. **No red**, except the see-through red behind text a suggested edit takes out (Matt asked
  for red and green there). Every status
  also carries a word, not colour alone.
- **One font**: JetBrains Mono Nerd Font, in its **NL** (no ligatures) Mono build (bundled in
  [assets/fonts/](assets/fonts/), OFL licensed). The ligature build merged pairs like `</` into one
  glyph, which Matt found cramped and not monospace-feeling, so never switch back to it. It is used
  in the UI and the editor. Nerd Font glyphs are the in-app icons; the only image is the app icon in
  [assets/icon/](assets/icon/).
- **No branding in the UI**: no logo, no app name in the window.
- **Settings** live in `settings.toml` (hand-editable) in the OS's config folder:
  `~/.config/<config dir>/` on Linux, `~/Library/Application Support/<config dir>/` on macOS,
  `%APPDATA%\<config dir>\` on Windows. Scratch, the recent list and the locks live in the data
  folder: `~/.local/share/<config dir>/` on Linux, the same folder as settings on macOS, and
  `%LOCALAPPDATA%\<config dir>\` on Windows (see `core/paths.rs`). Settings apply instantly, and
  unknown or missing keys fall back to defaults instead of failing.
