# Dovetail guide

Everything the [README](../README.md) leaves out: installing on each OS, every key, how saving works, setting up the assistant, and every setting.

## Install

Dovetail is built from this repository on your machine. First install what it needs:

- **Every OS**: Rust 1.91 or newer, through [rustup](https://rustup.rs) (Rust's installer, which
  also keeps it up to date). On Omarchy and Arch: `sudo pacman -S rustup && rustup default stable`;
  elsewhere, follow rustup.rs. Then open a new terminal so `cargo` is found. If you already have
  Rust, run `rustup update` first; a distro's packaged Rust is often too old.
- **Linux**: a C compiler, because the libraries for HTTPS include C code:
  `sudo pacman -S --needed base-devel` on Arch and Omarchy, `sudo apt install build-essential` on
  Ubuntu. On Ubuntu, also the libraries that open a window:
  `sudo apt install libxkbcommon-x11-0 libwayland-client0 libvulkan1` (Arch and Omarchy have them).
  The Open and Save dialogs need a file-chooser portal (xdg-desktop-portal with its -gtk, -gnome or
  -kde backend) or `zenity`. GNOME and KDE come with one; on Hyprland, Sway and other tiling setups,
  install `xdg-desktop-portal-gtk` next to the compositor's own portal. Without one, Ctrl+O, Ctrl+N
  and Save as do nothing.
- **macOS**: Xcode's command line tools: `xcode-select --install`.
- **Windows**: [Git](https://git-scm.com/download/win), which Windows doesn't come with. Then
  rustup-init.exe from rustup.rs: when it says the C++ build tools are missing, take its quick
  install (Visual Studio Community with those tools), or install Visual Studio Build Tools with the
  "Desktop development with C++" workload yourself. On ARM64 Windows, also add the "C++ Clang
  Compiler for Windows" component in the Visual Studio Installer.

Then, in a terminal:

```sh
git clone https://github.com/cavecomputing/text.git
cd text               # the repository's folder is called text
scripts/install.sh    # Linux; on macOS, scripts/install-macos.sh
```

On Windows, run the last step in PowerShell, in the `text` folder:
`powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1`.

The first build downloads and compiles every library the app uses, so it takes a few minutes. The
scripts look for the build in the repository's own `target/` folder, so unset `CARGO_TARGET_DIR`
(or a `build.target-dir` in your Cargo config) before running them.

- **Linux**: the binary goes to `~/.local/bin/dovetail`, the icon under
  `~/.local/share/icons/hicolor/`, and a `.desktop` entry in `~/.local/share/applications/`, so app
  launchers find Dovetail and file managers offer it for text and Markdown files.
- **macOS**: you get `~/Applications/Dovetail.app` (signed ad hoc, so no Apple developer account is
  needed) and a `dovetail` command in `~/.local/bin`. The app can't be a file's "Open With" app
  yet, so open files from the terminal or from inside the app.
- **Windows**: `dovetail.exe` goes to `%LOCALAPPDATA%\Programs\Dovetail\`, with a Start Menu entry,
  and Dovetail joins the "Open with" list for `.txt`, Markdown and config files (`.json`, `.yaml`,
  `.yml`, `.toml`, `.ini`, `.cfg`, `.conf`).

To run `dovetail` from a terminal, its folder must be on your `PATH`: `~/.local/bin` on Linux and
macOS, `%LOCALAPPDATA%\Programs\Dovetail` on Windows. The scripts tell you if it isn't. On Linux
and macOS, add `export PATH="$HOME/.local/bin:$PATH"` to the end of `~/.bashrc` (or `~/.zshrc`)
and open a new terminal.

### Arch and Omarchy: a pacman package

Instead of `scripts/install.sh`, you can build a pacman package, so Dovetail is installed, updated
and removed like any other package. It lives in `packaging/arch/` and builds the latest `main` from
GitHub:

```sh
cd text/packaging/arch
makepkg -si           # builds, then asks to install it and any missing dependencies
```

That puts `dovetail` in `/usr/bin`, with its icon and launcher entry under `/usr/share`. To update,
run `makepkg -si` there again (it fetches the newest commit); to remove it,
`sudo pacman -R dovetail-git`. Don't mix it with `scripts/install.sh`: if you used the script
before, delete `~/.local/bin/dovetail` and
`~/.local/share/applications/io.cavecomputing.Dovetail.desktop` first, or the old copy wins.

### Updating and uninstalling

To update, run `git pull` in the `text` folder, then the same install script again. Your settings,
scratch and API key are kept. On Windows, close Dovetail's windows first, since Windows won't
replace or delete a running program. On macOS, the Keychain then asks once before handing over the
saved API key: choose Always Allow.

To uninstall on Linux, delete what the script installed:

```sh
rm ~/.local/bin/dovetail ~/.local/share/applications/io.cavecomputing.Dovetail.desktop
rm ~/.local/share/icons/hicolor/*/apps/io.cavecomputing.Dovetail*.svg
```

On macOS: `rm -rf ~/Applications/Dovetail.app ~/.local/bin/dovetail`. On Windows, close Dovetail's
windows and run the install command with `-Uninstall` on the end. It builds first to learn the
names, so it needs the repository and Rust still in place.

Uninstalling leaves your settings, scratch and any saved API key where they are; to delete the key,
clear its field in Settings first.

## Run

```sh
dovetail              # opens scratch (or start it from your app launcher)
dovetail notes.txt    # opens that file, creating it if it doesn't exist
```

**Scratch** is one continuous notepad that comes back exactly as you left it: text, cursor and
scroll position. It is a plain `scratch.txt` in the data folder (on Linux,
`~/.local/share/dovetail/scratch.txt`), readable without the app.

**One document per window.** Each launch is its own window and process, which suits tiling window
managers. Opening from inside the app (Ctrl+O, Ctrl+P, the ☰ menu, Ctrl+N, or Ctrl+0 for scratch)
swaps the document in the same window: the one you're leaving is saved first, and if that save
fails, or the new one can't be opened, the window stays as it was and the status bar says why. A
document is edited in one window at a time: a second window on the same file, or on scratch, opens
read-only and says why, and opening one that another window has open is refused.

**The status bar**, left to right: the ☰ menu (New file, Open, Recent, Save as, Find, Line
numbers, Settings, Scratch), the save dot, and the file's path (or Scratch); on the right, words
and tokens, then the assistant button. Tokens are counted with OpenAI's o200k tokenizer (hover to
see it), a close enough guide for other models.

**Markdown** files (`.md` and `.markdown`) open like `.txt` files, and the Open, New file and Save
as dialogs offer them alongside `.txt`: type `notes.md` as the name to make one. They are coloured
as you type: headings, list markers and task boxes, quotes, code, bold, italic, struck text and
links. Nothing is rendered: the text stays exactly as typed, markers and all. Enter continues a
list or a quote, Tab nests a list item, and Ctrl+B and Ctrl+I put in bold and italic markers (see
[Keys](#keys)). Scratch and `.txt` files stay plain.

**JSON** files (`.json`), such as Cozy exports, open the same way and are coloured as you type:
keys in blue, strings in green, numbers in yellow, and `true`, `false` and `null` in orange. Tags
and `{{macros}}` inside strings are still coloured. A moment after you stop typing, the file is
checked: if it isn't valid JSON, the status bar says where and why, such as `Invalid JSON at line
12, column 5: expected , or }`. The command palette (Ctrl+Shift+P) has **Format JSON** (each value
on its own line, indented by your Tab setting) and **Minify JSON** (all on one line). Both change only the spaces and line
breaks between values, so keys keep their order and numbers their digits, and Ctrl+Z undoes either
in one step. Neither runs on invalid JSON: the status bar says it can't, next to the problem.

**Config files** are coloured and checked the same way: YAML (`.yaml`, `.yml`), TOML (`.toml`)
and INI (`.ini`, `.cfg`, `.conf`). Keys are blue, strings green, numbers yellow, `true`, `false`,
`null` (and YAML's `yes`, `no`, `on`, `off`) orange, comments grey, and INI sections and TOML
tables yellow and bold. The text of a YAML block (`prompt: |`) and of a TOML `"""` string is
coloured as text, so a line inside one that looks like a key isn't. If the file stops being valid,
the status bar says where and why, as for JSON. INI has no single standard, so the check only asks
that each line is a `[section]`, a `key = value` (or `key: value`), a `;` or `#` comment, or
indented to carry on the value above it. There's no Format for these: rewriting them would lose
your comments and layout.

Files must be UTF-8, which almost every editor writes today. Any other file opens empty and
read-only, and the status bar says why; the file itself is never touched. A file keeps its line
endings (Unix, Windows or old Mac style; a file that mixes them is saved with whichever it uses
most) and any byte order mark, the invisible character some Windows programs put at the start.

Where things live:

| | `settings.toml` | Scratch, recent files, and locks (small files that stop two windows editing one document) |
|---|---|---|
| Linux | `~/.config/dovetail/` | `~/.local/share/dovetail/` |
| macOS | `~/Library/Application Support/dovetail/` | the same folder |
| Windows | `%APPDATA%\dovetail\` | `%LOCALAPPDATA%\dovetail\` |

## Keys

Shortcuts are written with Ctrl; on macOS use ⌘ (the app's menu and hints show them that way).

| Key | What it does |
|---|---|
| Ctrl+N | New file: asks where, then opens it, empty, as Ctrl+O would. Choosing an existing file empties it (the dialog asks first); a file open in any window is refused |
| Ctrl+O | Open a file in this window |
| Ctrl+P | Scratch and the recent files, numbered 0 (scratch) to 9: type part of a name, ↑ and ↓ to choose, Enter to open. With nothing typed a row's number opens it; Ctrl+number does at any time |
| Ctrl+Shift+P | The command palette: the tools for the kind of file open (Format JSON and Minify JSON in a `.json` file). Type part of a name, ↑ and ↓ to choose, Enter to run |
| Ctrl+0 | Back to scratch in this window (if another window has it, the status bar says so instead) |
| Ctrl+S | Save now; in scratch, save to a file |
| Ctrl+Shift+S | Save as |
| Ctrl+W | Close the window, once the latest save is written |
| Ctrl+F | Find (text selected on one line becomes the search). Case is ignored unless you switch on Match case (the Aa button); the button next to it finds whole words only. In the find box, Enter and Shift+Enter step through the matches; Esc closes, leaving the match selected |
| Ctrl+Shift+F | Find and replace: the find bar with a Replace box under it (Tab moves between the two). In the Replace box, Enter replaces the selected match and goes to the next one, and Ctrl+Enter replaces them all, as one Ctrl+Z |
| Ctrl+G | Go to line: type a line number and the text jumps there as you type; Enter goes back to the text with the caret at the start of that line, Esc closes the box. On macOS it's ⌃G, since ⌘G is the next find match |
| Ctrl+= and Ctrl+- | Bigger or smaller text. This is the Font size setting (9 to 40), so every window follows and it is remembered |
| Ctrl+, | Settings |
| Ctrl+L | Open or close the assistant pane |
| Ctrl+Shift+L | Line numbers on or off. This is the Line numbers setting, so every window follows and it is remembered |
| Esc | Close the menu, the switcher or Settings, or else the go to line box, the find bar, or Ctrl+D's extra selections, and put the keyboard back in the text (from the assistant's box, say) |
| Tab | Insert spaces at the cursor, never a tab character (text selected within one line is replaced); with several lines selected, indent each. In a Markdown file, on a list item it indents the whole line instead, nesting the item |
| Shift+Tab | Unindent the line, or each selected line |
| Enter | New line, indented like the line the cursor is on (as far as the cursor). On a line of only spaces, the spaces move down too, so the line left behind is empty. In a Markdown file, on a list item or quote it starts the next one: the same bullet, the next number, an empty `[ ]` task box, the same `>`. On an empty item it ends the list instead: an indented item moves out a level, any other loses its marker. In fenced code it's a plain new line |
| Shift+Enter | New line, indented like Enter's, but never a new list item |
| Backspace | In the spaces at the start of a line, delete back to the previous indent, so one press takes back a Tab; anywhere else, one character |
| Ctrl+Backspace and Ctrl+Delete | Delete the word before or after the cursor (or just the selection). On macOS these are ⌥⌫ and ⌥⌦, and ⌘⌫ and ⌘⌦ delete to the start and end of the line |
| Alt+↑ and Alt+↓ | Move the line, or every selected line, up or down past the next one (⌥↑ and ⌥↓ on macOS) |
| Ctrl+Shift+D | Duplicate the line, or every selected line: the copy goes just below, and the cursor moves onto it |
| Ctrl+Shift+K | Delete the line, or every selected line |
| Ctrl+D | Select the word at the cursor; press again to also select the next place it appears (matching case and whole words; with part of a word selected, anywhere), going round to the top after the end. Typing, pasting, Backspace, Delete and Enter then change every selected place at once, each press its own Ctrl+Z. Moving the cursor or Esc lets the extra places go |
| Ctrl+B and Ctrl+I | In a Markdown file, bold (`**`) or italic (`_`, or `*` inside a word): put the markers around the selection, or with nothing selected a pair with the cursor between. Pressed again, they come off |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z or Ctrl+Y | Redo |
| Ctrl+A, C, X, V | Select all, copy, cut, paste |
| Tab or Esc on a suggestion | Accept or reject the suggestion the cursor is on |
| Enter, in the assistant's box | Send (Shift+Enter starts a new line) |

While Settings is open, only Esc, Ctrl+W, Ctrl+=, Ctrl+- and Ctrl+Shift+L (and ⌘M on macOS) work.

macOS also has ⌘↑ and ⌘↓ (start and end of the text, with ⇧ to select), ⌘G and ⇧⌘G (next and
previous match), ⌘M (minimise) and ⌘Q (closes the window, waiting for the save like any close).

## Saving

- Everything autosaves, scratch included, half a second after you stop typing (adjustable in
  Settings). The dot at the left of the status bar is aqua when saved, pulses yellow while saving,
  and turns orange with the reason if a save fails. Dovetail tries again after each pause, so the
  dot turns aqua once the cause is fixed.
- A save is written whole or not at all, so a crash mid-save never leaves half a file.
- **Ctrl+S** writes now instead of after the pause. In scratch, which has no file, it asks where to
  save one: the window becomes that file and scratch is emptied. The next scratch window opens
  empty, and Ctrl+Z there brings the text back.
- **Save as** (Ctrl+Shift+S) moves the text to a new file and keeps editing there, undo history and
  all. Typing that hasn't autosaved yet is written to the old file first, so the old file keeps the
  text as it was at that moment; later edits go only to the new file.
- **Closing waits for the save.** If it fails, the window stays open and the status bar says why.
  Fix the cause, or use Save as to put the text somewhere that works (or Reload or Keep mine, for a
  file changed outside), then close again.
- **A file changed outside Dovetail** (by another editor, or a `git checkout`) is not saved over.
  Dovetail checks just before each save, so it notices the next time you type; it never reloads a
  file by itself. The status bar then says the save failed and offers Reload (take the file on
  disk; Ctrl+Z brings your text back) or Keep mine (write your text over it). Until you choose,
  Ctrl+S does nothing and the window won't close. A file deleted outside is written again at the
  next save. Scratch has no such check, so don't edit `scratch.txt` elsewhere while its window is
  open.
- **A file you can't write** opens read-only: the text is dimmed, the status bar says why, and you
  can still scroll, select and copy. Save as writes it to a file you can write and carries on
  editing there.

## The assistant

Ctrl+L, or the speech-bubble button at the right of the status bar, opens the assistant pane beside
the text (over it, in a narrow window). It works in every window, scratch or file.

**Setting it up.** Open Settings (Ctrl+,), copy a row into the assistant section, then press Test
connection:

| | Base URL | Model | Request format | API key |
|---|---|---|---|---|
| OpenAI | `https://api.openai.com/v1` | a model your account can use | responses | your OpenAI key |
| OpenRouter | `https://openrouter.ai/api/v1` | a name from openrouter.ai/models | chat completions | your OpenRouter key |
| Ollama | `http://localhost:11434/v1` | a name from `ollama list` | chat completions | none |
| LM Studio | `http://localhost:1234/v1` | the model's name in LM Studio | chat completions | none |
| llama.cpp (`llama-server`) | `http://localhost:8080/v1` | any name | chat completions | none |

These are starting points. A server on another computer works the same way, at its address, such as
`http://192.168.1.20:11434/v1`. Other servers work too if they speak one of the two formats, stream
their replies (server-sent events), and take the key as `Authorization: Bearer <key>` or need none.

Ollama gives a model 4k tokens of context by default on machines with under 24 GB of video memory.
That is less than a long document plus the assistant's instructions, so the model misses part of
the text. Raise it with the context length slider in the Ollama app's settings, or start the server
with `OLLAMA_CONTEXT_LENGTH=32768 ollama serve`.

**The model must support tools.** The assistant reads, searches and suggests edits by calling tools:
functions the app offers the model (also called function calling). Every request offers them, so
with a model that can't use tools every question fails, plain ones included. In Ollama,
`ollama show <model>` lists `tools` under Capabilities if it can. Test connection sends a one-line
question along with the tools and shows the reply or the error, so such a model fails there rather
than in the middle of a chat.

The fields:

- **Base URL**: where the server's API starts, usually ending in `/v1`. Blank means
  `https://api.openai.com/v1`.
- **Model**: the model's name as the server knows it. The pane asks for one until it's set.
- **Request format**: responses (the default: OpenAI's Responses API, `<base URL>/responses`) or
  chat completions (`<base URL>/chat/completions`), the older format every OpenAI-compatible server
  understands. If Test connection says 404 Not Found, try the other one. In `settings.toml` it is
  `request_format = "responses"` or `"chat_completions"`.
- **API key**: only if the server needs one. It is kept in the system's password store: a keyring
  such as gnome-keyring or KeePassXC on Linux, the Keychain on macOS, Credential Manager on Windows.
  On Linux with no keyring running, it goes in `settings.toml` instead, readable only by you. The
  line under the field says which. Clearing the field deletes the key.

**What it does.** Ask about the text in the box at the bottom of the pane. The assistant can read
the document and the selection, search the text, count words and tokens, and suggest edits. It never
changes the text itself. Each suggestion shows in the text, the old words struck through and the new
ones beside them, with Accept and Reject buttons, and as a card in the chat with Accept, Reject and
Try shorter, which asks for a shorter version (Accept all and Reject all when several wait). You can
keep typing elsewhere meanwhile. While it answers, Send turns into Stop. New chat starts over and
rejects the suggestions still waiting. Closing the window forgets the conversation.

**What it sees.** Only this window's document. Each question sends the server the file's name, the
text (about the first 48 KB; the model can read the rest a few lines at a time), its size, and the
selection, or the cursor's line when nothing is selected. Every request also resends the
conversation so far, including whatever its tools read. It sees no other files or windows. The
chips at the top of the pane show what it sees and what you have selected. In a read-only window it
can answer but not suggest edits.

## Settings

Ctrl+, opens Settings in place of the text; Esc goes back. It is laid out like
`settings.toml`: one setting a line under its key name, the current value coloured and the other
choices dimmed (click one to pick it, or − and + to change a number), with a comment after.
Changes apply at once and are saved to `settings.toml`, whose path the status bar shows meanwhile.

| Section | Settings (default) |
|---|---|
| Appearance | Theme: dark (default), light or system. Font size (14). Reading mode: full width (default) or comfort, which caps lines at 80 characters (adjustable) |
| Editor | Tab inserts 4 spaces (adjustable). Word wrap (on). Line numbers left of the text (off; Ctrl+Shift+L). Colour for XML tags and `{{macros}}` (on). Line and column in the status bar (off) |
| Saving | Pause in typing before an autosave (500 ms) |
| Assistant | Base URL (`https://api.openai.com/v1`). Model (none: the pane asks for one). Request format (responses). API key (none). Test connection |
| Window | Window controls, the minimise, maximise and close buttons: auto (default) hides them under a tiling window manager (Hyprland, Sway, i3, niri, river) and shows them elsewhere; or show, or hide. Not on macOS, which keeps its own |

`settings.toml` can be edited by hand too, and open windows pick up the change within a second. To
get a file listing every key, change any setting in the panel once. A missing key takes its default
and an unknown one is ignored. A value Dovetail can't read takes its default and the status bar
shows "Settings problem" (hover for which key). Until you fix the file, changes in the panel still
apply but aren't saved, so your hand edits are never overwritten.

## Working on the code

This needs the same tools as [Install](#install).

```sh
cargo run                                   # build (debug) and open scratch
cargo run -- notes.txt                      # open a file (everything after -- goes to the app)
cargo test                                  # run every test
cargo clippy --all-targets -- -D warnings   # the linter; any warning fails, as in CI
cargo fmt --check                           # what CI checks; cargo fmt fixes it
cargo build --release                       # the optimised binary, in target/release/
```

Set `DOVETAIL_HOME` to a folder to keep a run's settings, scratch and recent files there instead of
in your real ones, for example `DOVETAIL_HOME=/tmp/dovetail-test cargo run`. The API key is the
exception: it still lives in the system keyring, so setting or clearing it in such a run changes
your real one.

CI ([.github/workflows/ci.yml](../.github/workflows/ci.yml)) builds, tests and runs clippy (warnings
fail) on Linux, macOS and Windows for every push and pull request, and checks formatting.

Before changing code, read [CLAUDE.md](../CLAUDE.md) (the rules for working on it) and the part of
[docs/spec.md](spec.md) (the full design) that covers your change.
