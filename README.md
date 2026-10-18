# Dovetail

[![CI](https://github.com/cavecomputing/text/actions/workflows/ci.yml/badge.svg)](https://github.com/cavecomputing/text/actions/workflows/ci.yml)

Dovetail is a fast plain-text editor and scratchpad, written in Rust. Each file opens in its own
window, and everything saves itself as you type, so there is never a "save changes?" prompt. An
assistant pane can read the text and suggest edits, which show up in the text for you to accept or
reject. It works with any OpenAI-compatible server, from OpenAI to a model running on your own
machine. Linux comes first; it also runs on macOS and Windows.

![A scratch window with the assistant pane open and a suggested edit shown in the text](docs/screenshot.png)

## Install

Dovetail installs from a checkout of this repository
(`git clone https://github.com/cavecomputing/text.git`). Each OS has a script that builds the
release binary and installs it for the current user. What you need first:

- **Every OS**: Rust, installed with [rustup](https://rustup.rs).
- **Linux**: a C compiler (`base-devel` on Arch, `build-essential` on Ubuntu), and to open a
  window, `libxkbcommon-x11-0 libwayland-client0 libvulkan1` (Ubuntu names). Omarchy and Arch
  already have them.
- **macOS**: Xcode's command line tools: `xcode-select --install`.
- **Windows**: rustup-init.exe (it picks the MSVC toolchain) and Visual Studio Build Tools with the
  "Desktop development with C++" workload, which rustup-init offers to install. On ARM64 Windows,
  also add the "C++ Clang Compiler for Windows" component.

The first build downloads and compiles every dependency, so it takes a few minutes.

**Linux**: run `scripts/install.sh`. It puts the binary at `~/.local/bin/dovetail`, the icon under
`~/.local/share/icons/hicolor/`, and a `.desktop` entry in `~/.local/share/applications/`, so app
launchers find Dovetail and file managers offer it for text files. The script has no uninstall
option; to remove it, delete those files:

```sh
rm ~/.local/bin/dovetail ~/.local/share/applications/io.cavecomputing.Dovetail.desktop
rm ~/.local/share/icons/hicolor/*/apps/io.cavecomputing.Dovetail*.svg
```

**macOS**: run `scripts/install-macos.sh`. It makes `~/Applications/Dovetail.app` (signed ad hoc,
so no Apple developer account is needed) and a `dovetail` command in `~/.local/bin`. The app can't
be a file's "Open With" app yet, so open files from the terminal or from inside the app. After a
reinstall, the Keychain asks once before handing over the saved API key: choose Always Allow. To
uninstall: `rm -rf ~/Applications/Dovetail.app ~/.local/bin/dovetail`.

**Windows**: in PowerShell, from the checkout, run
`powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1`. It copies `dovetail.exe` to
`%LOCALAPPDATA%\Programs\Dovetail\`, adds a Start Menu entry, and adds Dovetail to the "Open with"
list for `.txt` files. Close Dovetail's windows before reinstalling, since Windows won't replace a
running program. To uninstall, run the same command with `-Uninstall` on the end.

On Linux and macOS, `~/.local/bin` must be on your `PATH` to run `dovetail` from a terminal; the
scripts tell you if it isn't. Uninstalling leaves your settings, scratch and any saved API key where
they are; to delete the key, clear its field in Settings first.

## Run

```sh
dovetail              # opens scratch (or start it from your app launcher)
dovetail notes.txt    # opens that file, creating it if it doesn't exist
```

**Scratch** is one continuous notepad that comes back exactly as you left it: text, cursor and
scroll position. It is a plain `scratch.txt`, readable without the app.

**One window per document.** Each window is its own process. Opening a file from inside the app
(Ctrl+O, Ctrl+P or the menu) starts a new window, which suits tiling window managers. A document is
edited in one window at a time: a second window on the same file, or on scratch, opens read-only
and says why.

Files are read as UTF-8. A file that isn't UTF-8 is refused rather than changed. A file keeps its
line endings (LF, CRLF or CR) and any byte order mark.

Where things live:

| | `settings.toml` | Scratch, the recent files list, locks |
|---|---|---|
| Linux | `~/.config/dovetail/` | `~/.local/share/dovetail/` |
| macOS | `~/Library/Application Support/dovetail/` | the same folder |
| Windows | `%APPDATA%\dovetail\` | `%LOCALAPPDATA%\dovetail\` |

## Keys

Shortcuts are written with Ctrl; on macOS use ⌘ (the app's menu and hints show them that way).

| Key | What it does |
|---|---|
| Ctrl+N | New file: asks where, then opens it in a new window |
| Ctrl+O | Open a file in a new window |
| Ctrl+P | Recent files: type part of a name, ↑ and ↓ to choose, Enter to open |
| Ctrl+0 | Open scratch in its own window |
| Ctrl+S | Save now; in scratch, save to a file |
| Ctrl+Shift+S | Save as |
| Ctrl+W | Close the window, once the latest save is written |
| Ctrl+F | Find. Enter and Shift+Enter step through the matches; Esc closes, leaving the match selected |
| Ctrl+= and Ctrl+- | Bigger or smaller text |
| Ctrl+, | Settings |
| Ctrl+L | Open or close the assistant pane |
| Esc | Close the menu, the switcher or Settings, or else the find bar |
| Tab | Indent with spaces, never a tab character; with several lines selected, indents each |
| Shift+Tab | Unindent the line, or each selected line |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z or Ctrl+Y | Redo |
| Ctrl+A, C, X, V | Select all, copy, cut, paste |
| Tab or Esc on a suggestion | Accept or reject the suggestion the caret is on |
| Enter, in the assistant's box | Send (Shift+Enter starts a new line) |

macOS also has ⌘↑ and ⌘↓ (start and end of the text, with ⇧ to select), ⌘G and ⇧⌘G (next and
previous match), ⌘M (minimise) and ⌘Q (closes the window, waiting for the save like any close).

## Saving

- Everything autosaves, scratch included, half a second after you stop typing (adjustable in
  Settings). The dot at the left of the status bar is aqua when saved, pulses yellow while saving,
  and turns orange with the reason if a save fails.
- Each save writes a temporary file in the same folder, then renames it over the old one, so a crash
  mid-save never leaves half a file.
- **Ctrl+S** writes now instead of after the pause. In scratch, which has no file, it asks where to
  save one: the window becomes that file and scratch is emptied. The next scratch window opens
  empty, and Ctrl+Z there brings the text back.
- **Save as** (Ctrl+Shift+S) moves the text to a new file and keeps editing there. The old file
  keeps its last save.
- **Closing waits for the save.** If it fails, the window stays open and the status bar says why.
- **A file changed outside Dovetail** (by another editor, or a `git checkout`) is not saved over.
  The status bar says the save failed and offers Reload (take the file on disk; Ctrl+Z brings your
  text back) or Keep mine (write your text over it). A file deleted outside is simply written again.
- **A file you can't write** opens read-only: the text is dimmed, the status bar says why, and you
  can still scroll, select and copy.

## The assistant

Ctrl+L, or the speech-bubble button at the right of the status bar, opens the assistant pane beside
the text (over it, in a narrow window). It works in every window, scratch or file.

**Setting it up.** Open Settings (Ctrl+,) and fill in the assistant section:

- **Base URL**: where the server's API starts, usually ending in `/v1`. Blank means
  `https://api.openai.com/v1`.
- **Model**: the model's name as the server knows it. The pane asks for one until it's set.
- **Request format**: `responses` (the default: `<base URL>/responses`, OpenAI's Responses API) or
  `chat completions` (`<base URL>/chat/completions`), for servers that only speak that.
- **API key**: only if the server needs one. It is kept in the system keyring: the Secret Service on
  Linux (gnome-keyring or KeePassXC provide it), the Keychain on macOS, Credential Manager on
  Windows. On Linux with no keyring running, it goes in `settings.toml` instead, readable only by
  you; the panel says which. Clearing the field deletes the key.
- **Test connection** asks the model for a one-word reply, with the assistant's tools on offer, and
  shows the answer or the error.

For example:

| | Base URL | Model | Request format | API key |
|---|---|---|---|---|
| OpenAI | `https://api.openai.com/v1` | a model your account can use | responses | your OpenAI key |
| Ollama, on this machine | `http://localhost:11434/v1` | a name from `ollama list` | chat completions | none |

Other servers that speak either format are set up the same way. Suggestions arrive as tool calls, so
pick a model that supports tool calling.

**What it does.** Ask about the text in the box at the bottom of the pane. The assistant can read
the document and the selection, search the text, count words and tokens, and suggest edits. It never
changes the text itself. Each suggestion shows in the text, the old words struck through and the new
ones beside them, with Accept and Reject buttons, and as a card in the chat with Accept, Reject and
Try shorter (Accept all and Reject all when several wait). You can keep typing elsewhere meanwhile.
Stop cancels a reply; New chat starts over and rejects the suggestions still waiting. The
conversation lasts as long as the window.

**What it sees.** Only this window's document. Each question sends the server the text (about the
first 48 KB; the model can read the rest by lines), its size, the selection and the caret's line.
It sees no other files or windows. The chips at the top of the pane show what it sees and what you
have selected. In a read-only window it can answer but not suggest edits.

## Settings

Ctrl+, opens Settings over the window. Changes apply at once and are saved to `settings.toml`.

| Section | Settings (default) |
|---|---|
| Appearance | Theme: dark, light or system (dark). Font size (14). Reading mode: full width, or comfort, which caps lines at a number of characters (full width; comfort is 80) |
| Editor | Spaces Tab inserts (4). Word wrap (on). Colour for XML tags and `{{macros}}` (on). Line and column in the status bar (off) |
| Saving | Pause in typing before an autosave (500 ms) |
| Assistant | Base URL, model, request format, API key, test connection |
| Window | Window controls: auto, show or hide (auto, which hides them under Hyprland, Sway, i3, niri and river). Not on macOS, which keeps its own |

`settings.toml` can be edited by hand too, and open windows pick up the change within a second.
Changing anything in the panel writes the file out with every key, which shows the format. A
missing key takes its default and an unknown one is ignored. A value Dovetail can't read takes its
default and the status bar shows "Settings problem" (hover for which key); until the file is fixed,
Settings won't save over it.

## Building from source

```sh
cargo run                         # build (debug) and open scratch
cargo run -- notes.txt            # open a file (everything after -- goes to the app)
cargo test                        # run every test
cargo clippy --all-targets        # the linter
cargo fmt                         # format the code
cargo build --release             # the optimised binary, in target/release/
```

Set `DOVETAIL_HOME` to a folder to keep a run's settings, scratch and recent files there instead of
in your real ones, for example `DOVETAIL_HOME=/tmp/dovetail-test cargo run`.

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) builds, tests and runs clippy (warnings
fail) on Linux, macOS and Windows for every push and pull request, and checks formatting.

Before changing code, read [CLAUDE.md](CLAUDE.md), the rules for working on it, and the section of
[docs/spec.md](docs/spec.md), the full design, that covers your change. Project mode, for rewriting
a text across versions while tracking its ideas, lives on the `rewrite` branch.

## Font

Dovetail bundles JetBrains Mono NL Nerd Font, under the SIL Open Font License: see
[assets/fonts/OFL.txt](assets/fonts/OFL.txt).
