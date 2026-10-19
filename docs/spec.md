# Dovetail: implementation spec

A native desktop text editor, written in Rust, for Linux first; it also builds and runs on macOS and Windows (2026-10-18). By default it's a fast, minimal plain-text editor and scratchpad. On top of that, it has a project mode for iterating on roleplay system prompts, character cards and other writing across versions. That mode keeps track of which ideas from the old version made it into the new one, and how much space each one takes.

- Mockups, drawn under the old working name Word Designer. The boards are plain doc, writing view, Claude pane, idea weights, commit check and settings; the settings board is superseded by §10's layout like `settings.toml` (2026-10-18). They're drawn in HTML only to show the native UI.
- Running brainstorm log: [notes.md](notes.md)
- Name: **Dovetail** (2026-10-17). It replaces the working name Word Designer.

> **What `main` builds (2026-10-18):** for now `main` is the plain editor and scratchpad (§2 to §6, §10, §11) plus the assistant pane (§9) without its project parts. Project mode (the Rewrite flow, project format and commit check in §7, and ideas, paint and weights in §8, plus the project parts of the other sections) lives on the `rewrite` branch until the basic text editing is solid. This spec keeps the whole design; the notes below mark what is only on `rewrite`.

---

## 1. The problem it solves

Rewriting a prompt usually means two windows side by side: the old version on the left, and a line-by-line rewrite on the right. Then come several passes to condense it. The hard part is remembering which phrases and ideas map between versions. The biggest fear is that an idea's feeling or weight gets quietly written out without anyone noticing.

## 2. Look and feel

- **Vibe:** programmer-esque, like a simpler VS Code. Calm and minimal, the way Typora is.
- **Native desktop app**, not a web app. The content fills the window edge to edge, with no framed card or gap around it.
- **No branding** in the app: no logo or wordmark.
- **Colours:** the cavecomputing design system's Gruvbox palette, in dark (default) and light.
  - Roles: `bg-surface` for the window, `bg-page` for the status bar, `bg-raised` for cards and chips, `fg` and `fg-muted` for text. Yellow `accent-text` for focus and active items, aqua for success, orange `accent-warm` for warnings. No red, except the see-through red behind text a suggested edit takes out.
- **Font:** JetBrains Mono Nerd Font everywhere, in both the UI and the editor, in its NL build without ligatures (2026-10-18: ligatures like `</` looked cramped). Default size 14. This deliberately overrides the design system's system-fonts rule. The Nerd Font glyphs can double as the app's icons.
- **Top strip:** about 32px tall, no border, same colour as the page. It holds **only the window controls**, which are plain glyphs, not GNOME circles. No GTK-style big header bars.
  - Auto-sense: hide the app-drawn controls under a tiling WM. On Hyprland, check `HYPRLAND_INSTANCE_SIGNATURE` and/or the Wayland decoration support. Show them everywhere else, since not every machine is tiling. Can be overridden in Settings.
  - Per OS (2026-10-18): on Linux and Windows the window is undecorated, so the strip holds the app's glyph controls and the window's edges, top included, resize it. Windows adds a drop shadow, and rounded corners on Windows 11. On macOS the window keeps the system's see-through, untitled title bar: its traffic lights sit in the strip, macOS resizes the window, and the strip is always there with nothing of ours in it, so Settings has no Window row there.
- **Status bar** at the bottom, VS Code style, 24px, mono 11px. See §6.

## 3. Opening behaviour

| Action | Result |
|---|---|
| Launch the app | Opens the one continuous **scratch space**, resuming exactly where it was left (text, cursor, scroll). |
| Open a project file (`.dovetail`) | Opens that project (`rewrite` branch only). |
| Open a folder | Shows it in the folder pane, beside whatever is open (scratch when launched on a folder). |
| Open a `.txt`, `.md`, `.markdown`, a config file (`.json`, `.yaml`, `.yml`, `.toml`, `.ini`, `.cfg`, `.conf`) or a shell script (`.sh`, `.bash`, or no extension and a `#!` line naming sh, bash or dash) | Opens just that text file. Markdown and config files are coloured (§5), never rendered. New file and Save as keep a typed name as it is, any extension included (2026-10-18). |

- **No tabs, one window per launch.** Each launch (from the app menu, the terminal or a file manager) is its own window, which plays well with tiling WMs. Opening from inside the app (`Ctrl+O`, `Ctrl+P`, Recent in the menu, `Ctrl+N`'s new file, `Ctrl+0` for scratch) replaces the document in the same window instead (2026-10-18: separate windows had become more annoying than useful). The document being left is saved first (scratch with its cursor); if that save fails, or the new document can't be opened (open in another window, not UTF-8), the window stays as it was and the status bar says why. A file whose save was refused because it changed on disk isn't left until Reload or Keep mine is chosen. Opening the document already shown does nothing. Scratch resumes where it was left, here or in the next window that opens it.
- `Ctrl+P` opens a quick switcher with fuzzy search over recent files (and projects, on the `rewrite` branch). Scratch is always its first row, whatever is typed, and the rows are numbered 0 (scratch) to 9 (2026-10-18): with nothing typed, a row's number opens it, and `Ctrl+number` does at any time. `Ctrl+P` then Enter still opens the most recent file, which is highlighted first.
- `Ctrl+Shift+P` opens a command palette that looks like the switcher and lists only the tools for the kind of file open (2026-10-18): Format JSON and Minify JSON in a `.json` file, and a line saying there are none elsewhere. Typing filters them, Enter runs the highlighted one.
- `Ctrl+O` is the app's own picker, not the system dialog (2026-10-19: one picker for files and folders, which the Linux portal and Windows dialogs can't do): a path box starting at the open file's folder, the folder's entries below, filtered by the name typed after the last `/`. Tab goes into the highlighted folder, Enter opens the highlighted row (a file in the window, a folder in the folder pane), and with nothing typed after the `/` the first row is `./`, the folder itself.
- **Folder pane** (`Ctrl+Shift+E`, the owner picked mockup A with filtering, 2026-10-19): left of the text, drawn like the assistant pane: `# ~/path` with a close glyph, a `>` filter, then the folder as a tree (folders `accent-cool` with ▸/▾, the open file `accent-text`). Typing in the filter lists every file under the folder whose path fuzzy-matches (up to 20,000 files, read off the UI thread). `.git` and `node_modules` are left out. Folders go in the recent list and show in `Ctrl+P` and Recent with a trailing `/`.
- `Ctrl+0` (or similar) jumps back to scratch from anywhere, in the same window.
- `Ctrl+,` opens Settings, a panel over the window (2026-10-18: not a separate window). See §10.
- `Ctrl+L` opens and closes the assistant pane.
- The usual editor keys too (2026-10-18): `Ctrl+N` new file (asks where, opens it like `Ctrl+O`), `Ctrl+O` open (a file or a folder), `Ctrl+S` save (see §4), `Ctrl+Shift+S` save as, `Ctrl+W` close the window, `Ctrl+F` find, `Ctrl+=` / `Ctrl+-` font size.
- On macOS each `Ctrl` shortcut is `⌘` instead, and the menu and hints write shortcuts the macOS way (`⇧⌘S`). macOS also gets its usual `⌘↑` / `⌘↓` (start and end of the text), `⌘M` (minimize) and `⌘G` / `⇧⌘G` (next and previous match). `⌘Q` closes the window, so it waits for the save like any close (§4).
- **Find** (`Ctrl+F`): a bar just above the status bar. Matches ignore case (unless Match case is on; Whole word is the other switch) and are lit up in the text, the current one outlined; Enter and Shift+Enter step through them, Esc closes the bar and leaves the match selected. On the `rewrite` branch, in a project it searches the draft.
- **Find and replace** (`Ctrl+Shift+F` everywhere, 2026-10-18; `⇧⌘F` on macOS): the find bar with a Replace box under it. Enter in it replaces the selected match and moves to the next, Ctrl+Enter replaces every match as one undo step.
- **Go to line** (`Ctrl+G`; `⌃G` on macOS, where `⌘G` is the next match): a box in the find bar's place; the caret jumps to the typed line as it's typed, Enter goes back to the text.
- Startup has to be near-instant: launch, jot, close. This is a key reason to prefer pure Rust over a webview.
- Launch from the app menu or a short terminal command.
- Install for the current user from a checkout, one script per OS (2026-10-18). On Linux it registers a `.desktop` app that can open `.txt`, Markdown and config files (the `rewrite` branch also registers a MIME type for `.dovetail`). On Windows it adds a Start Menu entry and an "Open with" entry for `.txt`, Markdown and config files. On macOS it makes an app bundle in `~/Applications` that registers no file types: Finder hands a file to an app in an Apple Event, which winit 0.30 can't receive, so for now files open from the terminal command or from inside the app.

## 4. Saving

- **Everything autosaves**, scratch included. There's never a "save changes?" prompt.
- `Ctrl+S` still does something useful (2026-10-18). In a `.txt` (or a project, on the `rewrite` branch) it writes now instead of after the pause. Scratch has no file, so there it asks where to save: the window becomes that `.txt`, and scratch is cleared with its text kept for the next scratch window's undo (as after a Rewrite, on the `rewrite` branch).
- `Ctrl+Shift+S` (Save as) moves a `.txt` to a new file and keeps editing there; the old file keeps its last save. On the `rewrite` branch, projects live in their own folder, so they have no Save as.
- **Closing waits for the save.** The window only closes once the latest autosave has been written. If a save fails, the window stays open and says why. Known gap: logging out, shutting down or restarting the computer (every OS; on Windows that includes Windows Update's restarts), and Quit from the Dock on macOS, end the app without waiting (`⌘Q` and the app menu's Quit do wait).
- The scratch space persists between launches as a single document.
- Plain docs are ordinary `.txt` files, or Markdown `.md` files, saved exactly the same way.

## 5. Plain editor (the default)

- Text starts at the top left, like any text editor, with only a small margin (2026-10-18: a centred, floating column didn't feel like an editor). Word wrap is on by default.
- Reading mode is either **Full width** (the default) or **Comfort**, which caps lines at a number of characters (80 by default, adjustable), still anchored left.
- Optional line numbers left of the text, like vim's `set number` (2026-10-18), off by default. Ctrl+Shift+L (⇧⌘L on macOS) turns them on and off; it changes the setting, so every window follows and it's remembered. Each line's number is right-aligned against the text, in the text's font and size and on its baseline, and a wrapped line has a number only on its first row. Numbers are in `fg-subtle` and the caret's line in `fg`, with a two-character gap and no divider, as in vim (2026-10-18: a full-height divider read as a table border, and a yellow number vied with headings). The gutter is as wide as the largest number needs plus a gap, and widens the column rather than narrowing the text, so Comfort still holds its number of characters. Clicking in it puts the cursor at the start of that row.
- A scroll bar at the editor's right edge, the same rail and handle as Settings' (2026-10-18), shown only when the text is taller than the view. Dragging the handle or pressing the rail scrolls the text. cosmic-text lays out only lines it has shown, so until then a long wrapped line counts as one row and the handle's size is close rather than exact.
- The Tab key inserts **4 spaces** by default (configurable) and never writes a tab character; with several lines selected it indents each of them.
- The editing keys do the convenient thing, as in a code editor (2026-10-18). Enter starts the new line indented like the one the cursor is on, as far as the cursor; on a line of only spaces the spaces go down with the cursor, so none are left trailing. Backspace in the spaces at the start of a line deletes back to the previous indent stop, so one press takes back a Tab. Ctrl+Backspace and Ctrl+Delete delete the word before or after the cursor (⌥⌫ and ⌥⌦ on macOS, where ⌘⌫ and ⌘⌦ delete to the start and end of the line). Alt+↑ and Alt+↓ (⌥↑ and ⌥↓ on macOS) move the line, or every selected line, up or down. Ctrl+Shift+D duplicates them below and Ctrl+Shift+K deletes them. Ctrl+D selects the word at the caret, then each next place it appears as well (matching case, whole words for a word), and typing changes them all at once, as in VS Code; moving the caret or Esc lets them go.
- In shell scripts and JSON files typing helps with blocks (2026-10-18). Brackets and quotes close themselves (scripts `( [ { " '` and the backtick, JSON `[ { "`): before a space, a closer or the end of the line the pair goes in with the caret between, around a selection it wraps it, and typing a closer right before the same one steps over it; a quote after a letter, digit or backslash, and an opener before a word, are typed alone. Backspace in an empty pair deletes both. Enter indents a step after a line that opens a block (`{ [ (`, and in scripts `then`, `do`, `else`, `case … in`, a `pattern)`), a step less after a script's `;;`, and between a bracket pair puts the closer on its own line. A line that ends a block (`} ] )`, and `fi`, `done`, `esac`, `else`, `elif`) is lined up with its block's start when Enter is pressed on it. Shift+Enter stays the plain new line.
- In YAML files indenting follows the file (2026-10-18): Tab, Shift+Tab, Backspace and Enter use the file's smallest indent as their step (2 when nothing is indented yet) instead of the `tab_spaces` setting, whose comment in Settings says so. Enter indents a step after `key:`, a block scalar (`key: |`, `>-`, ...) or a bare `-`, and after `- key: value` lines the next line up with the key.
- Ctrl+/ (⌘/ on macOS) comments out the caret's line, or every line the selection is on, in a shell script or a YAML, TOML or INI file (2026-10-18): `# ` goes in at the smallest indent among them, so the block keeps its shape, and blank lines are left alone. When every one is already a comment, the `#` (and a space after it) comes off instead. One undo step, and the selection stays on the same text. JSON, Markdown and plain text have no comments, so it does nothing there.
- Optional colouring for XML tags and `{{macros}}`. This matters for prompts that use `<system_instructions status="enabled">`, `{{char}}`, `{{user}}` and `{{#x}}…{{/x}}` blocks. Plain prose has to work just as well.
- Markdown files (`.md` and `.markdown`, 2026-10-18) are coloured as they're typed: `#` headings in the accent and bold, list bullets, numbers and task boxes, quotes muted, fenced code and `code spans` in aqua, bold and italic in the bundled bold and italic faces, `~~struck~~` text subtle, and links with their text in the cool accent and the `(url)` subtle. The markers stay visible and the text stays exactly as typed. Scratch and `.txt` files stay plain.
- In Markdown files the keys know about lists (2026-10-18). Enter on a list item starts the next one with the same indentation and quote markers, the same bullet or the next number (later numbers aren't renumbered), and an empty `[ ]` task box if the item had one; on a `>` quote line it continues the quote. Enter on an empty item ends the list instead: an indented item unindents a step, any other loses its marker, and a quote line with nothing else loses its `>`. Enter at the end of a fence that opens a code block nothing below closes puts the closing fence (the same characters and length, with the line's indent and `>` markers) two lines down, with the cursor on the empty line between (2026-10-18). With the cursor in the line's marker, or in fenced code, Enter is the plain one above, and Shift+Enter always is. Tab on a list item nests it: the whole line moves in a step, wherever the cursor is on it, and Shift+Tab moves it back out. Ctrl+B and Ctrl+I (⌘B and ⌘I on macOS) put `**` or `_` (GitHub's italic, so it never runs into `**`; `*` against a letter or digit, where Markdown reads `_` as part of the word) around the selection and keep it selected, or with nothing selected put in a pair with the cursor between; when the markers are already there they come off.
- There's no markdown rendering. It isn't needed.
- JSON files (`.json`, 2026-10-18) are coloured as they're typed: keys `accent-cool`, strings `accent-aqua`, numbers `accent-text`, `true`/`false`/`null` `accent-warm`, with tags and macros still coloured inside strings. After the same pause as the token count, the text is checked in the background; invalid JSON puts the first problem, with its line and column, in the status bar in orange. The command palette (`Ctrl+Shift+P`, 2026-10-18) has Format JSON (one value per line, indented by the Tab setting) and Minify JSON (one line); they are not in the menu. Both only change the whitespace between values, so key order and number digits are kept, are one undo step, and refuse invalid JSON with a notice beside the problem.
- Config files are coloured and checked the same way (2026-10-18): YAML (`.yaml`, `.yml`), TOML (`.toml`) and INI (`.ini`, `.cfg`, `.conf`). The same roles as JSON, plus comments and YAML's markers (`---`, `-`, `|`, anchors, tags) in `fg-subtle`, and INI sections and TOML tables in `accent-text` bold. A YAML block scalar's lines and a TOML multi-line string are coloured as text. YAML is checked with saphyr-parser, TOML with the `toml` crate; INI, which has no one standard, only needs each line to be a section, a key and value, a comment, blank, or an indented carry-on of a value (as Python's configparser reads it). No Format for these, since rewriting would lose comments.
- Shell scripts (2026-10-18): `.sh` and `.bash` files, and a file with no extension whose `#!` line runs sh, bash or dash (directly or through `env`). Keywords where a command could start in `accent-warm`, `$variables` and `${…}` (inside `"…"` strings and unquoted here-documents too), assignments and function names in `accent-cool`, strings in `accent-aqua`, comments and here-document markers in `fg-subtle`. Strings and here-documents carry on over lines. After the usual pause the script is checked with `bash -n` (it parses without running), and its first error goes in the status bar like a data file's; no check when bash isn't installed, nor on Windows, where `bash` is usually WSL's and slow to start.

## 6. Status bar

Left to right:

- **Plain doc (`main`):** `☰ menu` · save dot · path (or `Scratch`) … words · tokens · assistant toggle
- **Plain doc (`rewrite` branch):** the same, then the **pen icon (Rewrite)**
- **Project (`rewrite` branch):** `☰ menu` · save dot · project path · `v3 → draft` … words · tokens · `−45% vs v3` · assistant toggle · **Commit**

Rules:
- On the `rewrite` branch the bottom-right corner always holds the **next step**: the pen icon on plain docs, Commit in projects. On `main` it holds the assistant toggle.
- **Save dot:** green means saved (quiet, with the word on hover). Yellow with a gentle pulse means saving. Orange with words means the save failed, for example "Save failed: disk full", and also blocks closing. There's no "unsaved" state, because everything autosaves.
- The tokenizer and encoding show on hover over the token count, not in the bar. Line and column are off by default and can be turned on.
- The menu holds new file, open, open recent, save as (save to a file in scratch), find, replace, go to line, show or hide line numbers, settings and scratch, then the recent files last, so the commands stay put however many there are (2026-10-18). File-type tools (Format and Minify JSON) are in the `Ctrl+Shift+P` palette instead (2026-10-18). On the `rewrite` branch a project's menu adds version history and idea weights.

## 7. Projects: the rewrite flow (`rewrite` branch)

1. In scratch or a `.txt`, paste or write the text, then click the **pen (Rewrite)**.
2. The text **slides left and becomes v1**, read-only. A copy lands on the right as the **draft**.
3. A project is created right then, with no save dialog. It goes under `~/Documents/Dovetail/<first few words>/` and can be renamed later.
4. If it came from scratch, scratch is cleared, with undo available. This is the default and easy to change.
5. Work in the draft. Autosave keeps the work-in-progress across many open/close sessions.
6. **Commit** whenever the owner decides it's worth a new version. Commits are never automatic. The version number goes up and the draft becomes the new left-hand reference. Condensing is just another round of the same thing.

### Project format

```
Nyx/
  Nyx.dovetail ← JSON manifest: name, ideas, feeling notes, paint ranges per version. Double-clickable.
  draft.txt   ← current text, readable in anything
  .git/       ← hidden; one commit per version
```

Nobody has to touch git by hand.

### Commit check

Before committing, a small dialog shows:
- Token and word changes.
- Ideas carried, cut on purpose, and **missing**.
- A "Heads up" when an idea has no text left. The options are Go back, Mark as cut, or Commit anyway.

## 8. Ideas: tracking meaning across versions (`rewrite` branch)

This is what makes the app different. It's the agreed direction, with details to be refined while building.

- **Ideas** are named concepts like "Autonomy" or "Slow burn". Each one has a colour.
- **Two levels:** topics (paragraph-level, such as "Dialogue rules", and possibly mapped to XML sections) hold ideas.
- **Brushes:** pick an idea and paint its text in the old version. While that brush is active, anything typed in the draft gets painted with it automatically. Switching brushes mid-sentence splits the sentence between ideas. Text can also be selected and turned into an idea on the fly.
- **Paint sticks to the text, not the line.** It survives edits, merges and moves, and carries across commits.
- **Gutter stripes:** a thin coloured stripe beside each paragraph shows which ideas live there.
- **Idea strip:** a slim row of chips along the bottom of a project. Each chip shows the idea's colour, its name, a small old/new bar and `old% → new%`. The active brush is highlighted there.
- **Idea weights** (the key view): each idea's share of the whole prompt, as a % and in tokens, old vs new. It rolls up to topics.
  - An **Unpainted** row covers untagged text.
  - Text painted with two ideas counts toward both, so totals can go over 100%. That's useful for spotting packed lines.
  - An idea that drops to **0%** is flagged as missing.
  - It can be marked **cut on purpose** to quiet the flag.
- **Feeling notes:** each idea carries a short note in the writer's own words, like "warm but guarded". It's shown next to the cursor while that brush is active. The note can change between versions, and history keeps every version of it.
- **Merge and split ideas:** combine two ideas into one with a new note, keeping where it came from in history, like a git merge. Splitting works the other way round.

## 9. Assistant pane

- A pane that slides in from the right when called. Its toggle is in the status bar, and `Ctrl+L` opens and closes it.
- **Drawn like a terminal transcript** (mockup A, 2026-10-18), in keeping with the settings.toml-style Settings: one 13px mono size, no bubbles, cards or chips. `#` comment lines at the top give the model, "suggests only", what it sees and any focus (in `accent-text`), with a `new chat` word on the right. Questions follow a `>` in `accent-text`, replies are plain `fg` text, tool use is `fg-subtle` lines, and each waiting edit is `propose_edit(line N)` in `accent-cool`, the reason as a `#` comment, and `accept`, `reject` and `try shorter` as words that frame on hover, like Settings' values. The change itself shows only in the document (2026-10-19). `# Ctrl+Enter accepts` and `# Ctrl+Backspace rejects` sit above the composer while edits wait. The composer is a `>` prompt with no box; Enter sends, and `stop` shows while a reply streams.
- **Available everywhere** (2026-10-17): scratch, plain `.txt` files and projects alike. It's a general assistant to chat with about whatever is being worked on, not a project-only feature.
- **Tool-capable:** it works through function calling rather than a fixed reply format. Tools let it read the document and the selection, search it, count tokens, propose edits and, in a project, read any version, the ideas, the feeling notes and the weights. The app runs each tool call and sends the result back until the model gives its answer.
- **Context:** in a project, it can always see everything: both versions, every idea, the feeling notes and the weights. In a plain doc, it sees the document. A `# focus:` line marks what the owner is focused on (the selection, the current idea or topic).
- **Code and config too** (2026-10-18): the instructions name the kind of file open (a shell script, a YAML file, ...) and ask for its indentation and style to be kept. A proposed edit is narrowed to the lines that change, dropping whole lines the model copied around it, and within one line to the words that change, so the red and green in the document show only those. In a checked file (JSON, YAML, TOML, INI, shell) that is valid, an edit that alone would make it invalid still shows, but the tool's result tells the model what breaks.
- **Suggest-only in v1.** Proposed edits (from its propose-edit tool) show up in the document, the text taken out on a see-through red and struck through, the text put in on a see-through green, and are accepted or rejected piece by piece in the chat or with Ctrl+Enter and Ctrl+Backspace. The next question's note tells the model which were accepted and rejected. In a project, accepted text takes on the idea's colour, so tracking is never lost.
- It can answer questions like "does this still feel warm but guarded?" or "which idea shrank most since v3?"
- **Backend: any OpenAI-compatible API** (decided 2026-10-17), so it can point at any provider: Claude via a compatible endpoint, OpenRouter, Ollama, LM Studio, llama.cpp, vLLM. Settings hold the base URL, the API key (stored in the system keyring: the Secret Service on Linux, the Keychain on macOS, Credential Manager on Windows) and the model name. The default request format is the Responses API (`/v1/responses`), the owner's choice (2026-10-17); a setting switches to Chat Completions (`/v1/chat/completions`) for servers that only speak that. It's plain HTTP from Rust, with no SDK. Streaming replies. Keep the shared context (both versions, ideas, notes, weights) at the start of the prompt so providers that cache prompt prefixes keep repeat questions cheap.

## 10. Settings

A settings panel that opens over the app, from the menu or `Ctrl+,`, in whichever window it was asked for (scratch, a `.txt`, or a project on the `rewrite` branch), covering the whole text area edge to edge, between the window controls and the status bar (2026-10-18: not a separate window, and not a card floating over the dimmed text). It is laid out like `settings.toml` in the editor's own type (2026-10-18, so it reads as part of the editor rather than a form): a `[section]` line per group, then one setting a line, its key name in the TOML key colour, `=`, the current value in its TOML colour (strings aqua, numbers yellow, on and off orange) with the other choices dimmed beside it, and a `# comment` in a column after, starting where the text does. The status bar stays, showing the settings file's path and a dot for whether it saved. It scrolls when taller than the window, with the bar at the right edge like the editor's; Esc or the close glyph at the end of its first line closes it (2026-10-18: he tried Esc alone and wanted the button back). While it's open, typing goes only to its fields and the document's shortcuts are off. Changes apply instantly, and other open windows pick them up too. Settings are stored in `settings.toml` in the app's config folder, so they can be edited by hand: `~/.config/<app>/` on Linux, `~/Library/Application Support/<app>/` on macOS, `%APPDATA%\<app>\` on Windows. The status bar shows the exact path while the panel is open.

| Section | Settings |
|---|---|
| Appearance | Theme (Gruvbox dark / light / follow system), font, font size (default 14), reading mode (Full width, default, or Comfort with a character width) |
| Editor | Tab inserts N spaces (**default 4**, never tabs), word wrap (**on** by default), line numbers (**off** by default, Ctrl+Shift+L), XML and macro colouring |
| Saving | Autosave timing (the `rewrite` branch adds the projects folder) |
| Assistant | Base URL, API key (keyring), model, request format (Responses, default / Chat Completions), test connection |
| Window | Window controls: Auto / Always show / Always hide (Linux and Windows; macOS always shows its own) |

## 11. Counting

- Word and token counts update live (in both panes of a project, on the `rewrite` branch).
- The tokenizer is OpenAI's o200k, via `tiktoken-rs`. That's a good enough estimate for Claude and Gemini.
- Later: count `{{macros}}` separately from real text, since they expand at runtime.

## 12. Tech direction (not locked)

- **Pure Rust is preferred.** Startup is fast, and there's no markdown or rich-text requirement, so plain text with colouring is well within reach.
  - UI candidates: **iced** (used by COSMIC, Linux-first) or **GPUI** (Zed's toolkit, excellent text, thinner docs). egui and Slint are possible but weaker for long text.
  - Tauri remains a fallback if a native editor gets too hard. It's slower to start.
- Likely crates: `git2` or `gix` for history, `tiktoken-rs` for counts, `serde` and `serde_json` for the manifest, `toml` for settings, `reqwest` for the OpenAI-compatible API, `keyring` for the API key, plus `notify`/`directories` as needed.
- **No name is hardcoded.** The app name (Dovetail), the project extension (`.dovetail`), the config folder name (`dovetail`), the projects folder and the `.desktop`/MIME IDs all come from one constants module (`src/brand.rs`), so renaming later still means changing one file.
- Before writing code, write a `CLAUDE.md` for this Rust project. The owner is new to Rust, so it should also explain cargo, crates and the build, test and run commands in plain terms.

## 13. Suggested build order

1. **Plain editor:** a window, the scratch space with resume, autosave with the close guard, `.txt` open and save, the status bar, settings, and 4-space tabs.
2. **Projects:** Rewrite creates a project, two panes (read-only v1 and the draft), Commit with git, and `Ctrl+P` recent files.
3. **Ideas:** brushes, paint that survives edits, gutter stripes, the idea strip, weights, and the missing-idea flags.
4. **Commit check and feeling notes.**
5. **Claude pane** with suggest-only edits.
6. **Later:** Hyprland auto-sense polish, the XML-aware view, Gemini, prompt export, and idea merge/split.

`main` holds step 1, `Ctrl+P` recent files from step 2, and step 5 without its project parts. Steps 2 to 4 and the project parts of step 5 are on the `rewrite` branch (2026-10-18).

## 14. Still open

- Whether scratch should clear or keep its copy after a Rewrite. It clears, with undo, for now.
- How exactly paint should anchor to text so it survives heavy edits. This is a technical design question for the build.
- Whether Ln/Col and the other optional status items show up as Settings toggles.

## Parking lot

- Prompt export, as JSON with `content` and `post_history_content`.
- A diff-on-paste fallback, so paint carries over when text is edited outside the app.
