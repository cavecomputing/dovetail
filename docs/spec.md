# Dovetail: implementation spec

A native desktop text editor, written in Rust, for Linux first (Matt's machines run Omarchy/Hyprland); it also builds and runs on macOS and Windows (Matt, 2026-10-18). By default it's a fast, minimal plain-text editor and scratchpad. On top of that, it has a project mode for iterating on roleplay system prompts, character cards and other writing across versions. That mode keeps track of which ideas from the old version made it into the new one, and how much space each one takes.

- Mockups: [mockups, drawn under the old working name Word Designer](https://claude.ai/artifact/VMzsZGMBzrTPNBbnd446Ln). The boards are plain doc, writing view, Claude pane, idea weights, commit check and settings. They're drawn in HTML only to show the native UI.
- Running brainstorm log: [notes.md](notes.md)
- Name: **Dovetail** (Matt, 2026-10-17). It replaces the working name Word Designer.

> **What `main` builds (Matt, 2026-10-18):** for now `main` is the plain editor and scratchpad (§2 to §6, §10, §11) plus the assistant pane (§9) without its project parts. Project mode (the Rewrite flow, project format and commit check in §7, and ideas, paint and weights in §8, plus the project parts of the other sections) lives on the `rewrite` branch until the basic text editing is solid. This spec keeps the whole design; the notes below mark what is only on `rewrite`.

---

## 1. The problem it solves

Matt writes prompts by putting two Typora windows side by side: the old version on the left, and a line-by-line rewrite on the right. Then he does several passes to condense it. The hard part is remembering which phrases and ideas map between versions. The biggest fear is that an idea's feeling or weight gets quietly written out without anyone noticing.

## 2. Look and feel

- **Vibe:** programmer-esque, like a simpler VS Code. Calm and minimal, the way Typora is.
- **Native desktop app**, not a web app. The content fills the window edge to edge, with no framed card or gap around it.
- **No branding** in the app: no logo or wordmark.
- **Colours:** the cavecomputing design system's Gruvbox palette, in dark (default) and light. Design system: https://claude.ai/artifact/TAYcpHgxU55sLKKU2sYeRv
  - Roles: `bg-surface` for the window, `bg-page` for the status bar, `bg-raised` for cards and chips, `fg` and `fg-muted` for text. Yellow `accent-text` for focus and active items, aqua for success, orange `accent-warm` for warnings. No red.
- **Font:** JetBrains Mono Nerd Font everywhere, in both the UI and the editor, in its NL build without ligatures (Matt, 2026-10-18: ligatures like `</` looked cramped). Default size 14. This deliberately overrides the design system's system-fonts rule. The Nerd Font glyphs can double as the app's icons.
- **Top strip:** about 32px tall, no border, same colour as the page. It holds **only the window controls**, which are plain glyphs, not GNOME circles. Matt dislikes GTK-style big header bars.
  - Auto-sense: hide the app-drawn controls under a tiling WM. Matt uses Omarchy/Hyprland, so check `HYPRLAND_INSTANCE_SIGNATURE` and/or the Wayland decoration support. Show them everywhere else, since not all of Matt's machines are tiling. Can be overridden in Settings.
  - Per OS (2026-10-18): on Linux and Windows the window is undecorated, so the strip holds the app's glyph controls and the window's edges, top included, resize it. Windows adds a drop shadow, and rounded corners on Windows 11. On macOS the window keeps the system's see-through, untitled title bar: its traffic lights sit in the strip, macOS resizes the window, and the strip is always there with nothing of ours in it, so Settings has no Window row there.
- **Status bar** at the bottom, VS Code style, 24px, mono 11px. See §6.

## 3. Opening behaviour

| Action | Result |
|---|---|
| Launch the app | Opens the one continuous **scratch space**, resuming exactly where it was left (text, cursor, scroll). |
| Open a project file (`.dovetail`) | Opens that project (`rewrite` branch only). |
| Open a `.txt`, `.md` or `.markdown` | Opens just that text file. Markdown is coloured (§5), never rendered. The Open, New file and Save as dialogs offer all three. |

- **No tabs, one window per launch.** Each launch (from the app menu, the terminal or a file manager) is its own window, which plays well with tiling WMs. Opening from inside the app (`Ctrl+O`, `Ctrl+P`, Recent in the menu, `Ctrl+N`'s new file, `Ctrl+0` for scratch) replaces the document in the same window instead (Matt, 2026-10-18: separate windows had become more annoying than useful). The document being left is saved first (scratch with its cursor); if that save fails, or the new document can't be opened (open in another window, not UTF-8), the window stays as it was and the status bar says why. A file whose save was refused because it changed on disk isn't left until Reload or Keep mine is chosen. Opening the document already shown does nothing. Scratch resumes where it was left, here or in the next window that opens it.
- `Ctrl+P` opens a quick switcher with fuzzy search over recent files (and projects, on the `rewrite` branch). Scratch is always its first row, whatever is typed, and the rows are numbered 0 (scratch) to 9 (Matt, 2026-10-18): with nothing typed, a row's number opens it, and `Ctrl+number` does at any time. `Ctrl+P` then Enter still opens the most recent file, which is highlighted first.
- `Ctrl+0` (or similar) jumps back to scratch from anywhere, in the same window.
- `Ctrl+,` opens Settings, a panel over the window (Matt, 2026-10-18: not a separate window). See §10.
- `Ctrl+L` opens and closes the assistant pane.
- The usual editor keys too (Matt, 2026-10-18): `Ctrl+N` new file (asks where, opens it like `Ctrl+O`), `Ctrl+O` open, `Ctrl+S` save (see §4), `Ctrl+Shift+S` save as, `Ctrl+W` close the window, `Ctrl+F` find, `Ctrl+=` / `Ctrl+-` font size.
- On macOS each `Ctrl` shortcut is `⌘` instead, and the menu and hints write shortcuts the macOS way (`⇧⌘S`). macOS also gets its usual `⌘↑` / `⌘↓` (start and end of the text), `⌘M` (minimize) and `⌘G` / `⇧⌘G` (next and previous match). `⌘Q` closes the window, so it waits for the save like any close (§4).
- **Find** (`Ctrl+F`): a bar just above the status bar. Matches ignore case and are lit up in the text, the current one outlined; Enter and Shift+Enter step through them, Esc closes the bar and leaves the match selected. On the `rewrite` branch, in a project it searches the draft.
- Startup has to be near-instant: launch, jot, close. This is a key reason to prefer pure Rust over a webview.
- Launch from the app menu or a short terminal command.
- Install for the current user from a checkout, one script per OS (2026-10-18). On Linux it registers a `.desktop` app that can open `.txt` and Markdown files (the `rewrite` branch also registers a MIME type for `.dovetail`). On Windows it adds a Start Menu entry and an "Open with" entry for `.txt`, `.md` and `.markdown` files. On macOS it makes an app bundle in `~/Applications` that registers no file types: Finder hands a file to an app in an Apple Event, which winit 0.30 can't receive, so for now files open from the terminal command or from inside the app.

## 4. Saving

- **Everything autosaves**, scratch included. There's never a "save changes?" prompt.
- `Ctrl+S` still does something useful (Matt, 2026-10-18). In a `.txt` (or a project, on the `rewrite` branch) it writes now instead of after the pause. Scratch has no file, so there it asks where to save: the window becomes that `.txt`, and scratch is cleared with its text kept for the next scratch window's undo (as after a Rewrite, on the `rewrite` branch).
- `Ctrl+Shift+S` (Save as) moves a `.txt` to a new file and keeps editing there; the old file keeps its last save. On the `rewrite` branch, projects live in their own folder, so they have no Save as.
- **Closing waits for the save.** The window only closes once the latest autosave has been written. If a save fails, the window stays open and says why. Known gap: logging out, shutting down or restarting the computer (every OS; on Windows that includes Windows Update's restarts), and Quit from the Dock on macOS, end the app without waiting (`⌘Q` and the app menu's Quit do wait).
- The scratch space persists between launches as a single document.
- Plain docs are ordinary `.txt` files, or Markdown `.md` files, saved exactly the same way.

## 5. Plain editor (the default)

- Text starts at the top left, like any text editor, with only a small margin (Matt, 2026-10-18: a centred, floating column didn't feel like an editor). Word wrap is on by default.
- Reading mode is either **Full width** (the default) or **Comfort**, which caps lines at a number of characters (80 by default, adjustable), still anchored left.
- Optional line numbers left of the text, like vim's `set number` (Matt, 2026-10-18), off by default. Ctrl+Shift+L (⇧⌘L on macOS) turns them on and off; it changes the setting, so every window follows and it's remembered. Each line's number is right-aligned against the text, in the text's font and size and on its baseline, and a wrapped line has a number only on its first row. Numbers are in `fg-subtle`, the caret's line in `fg` (vim's CursorLineNr). The gutter is as wide as the largest number needs plus a gap, and widens the column rather than narrowing the text, so Comfort still holds its number of characters. Clicking in it puts the cursor at the start of that row.
- The Tab key inserts **4 spaces** by default (configurable) and never writes a tab character; with several lines selected it indents each of them.
- The editing keys do the convenient thing, as in a code editor (Matt, 2026-10-18). Enter starts the new line indented like the one the cursor is on, as far as the cursor; on a line of only spaces the spaces go down with the cursor, so none are left trailing. Backspace in the spaces at the start of a line deletes back to the previous indent stop, so one press takes back a Tab. Ctrl+Backspace and Ctrl+Delete delete the word before or after the cursor (⌥⌫ and ⌥⌦ on macOS, where ⌘⌫ and ⌘⌦ delete to the start and end of the line). Alt+↑ and Alt+↓ (⌥↑ and ⌥↓ on macOS) move the line, or every selected line, up or down.
- Optional colouring for XML tags and `{{macros}}`. This matters for prompts like Cozy's NanoBear prompt, which uses `<system_instructions status="enabled">`, `{{char}}`, `{{user}}` and `{{#x}}…{{/x}}` blocks. Plain prose has to work just as well.
- Markdown files (`.md` and `.markdown`, Matt, 2026-10-18) are coloured as they're typed: `#` headings in the accent and bold, list bullets, numbers and task boxes, quotes muted, fenced code and `code spans` in aqua, bold and italic in the bundled bold and italic faces, `~~struck~~` text subtle, and links with their text in the cool accent and the `(url)` subtle. The markers stay visible and the text stays exactly as typed. Scratch and `.txt` files stay plain.
- In Markdown files the keys know about lists (Matt, 2026-10-18). Enter on a list item starts the next one with the same indentation and quote markers, the same bullet or the next number (later numbers aren't renumbered), and an empty `[ ]` task box if the item had one; on a `>` quote line it continues the quote. Enter on an empty item ends the list instead: an indented item unindents a step, any other loses its marker, and a quote line with nothing else loses its `>`. With the cursor in the line's marker, or in fenced code, Enter is the plain one above, and Shift+Enter always is. Tab on a list item nests it: the whole line moves in a step, wherever the cursor is on it, and Shift+Tab moves it back out. Ctrl+B and Ctrl+I (⌘B and ⌘I on macOS) put `**` or `_` (GitHub's italic, so it never runs into `**`; `*` against a letter or digit, where Markdown reads `_` as part of the word) around the selection and keep it selected, or with nothing selected put in a pair with the cursor between; when the markers are already there they come off.
- There's no markdown rendering. It isn't needed.

## 6. Status bar

Left to right:

- **Plain doc (`main`):** `☰ menu` · save dot · path (or `Scratch`) … words · tokens · assistant toggle
- **Plain doc (`rewrite` branch):** the same, then the **pen icon (Rewrite)**
- **Project (`rewrite` branch):** `☰ menu` · save dot · project path · `v3 → draft` … words · tokens · `−45% vs v3` · assistant toggle · **Commit**

Rules:
- On the `rewrite` branch the bottom-right corner always holds the **next step**: the pen icon on plain docs, Commit in projects. On `main` it holds the assistant toggle.
- **Save dot:** green means saved (quiet, with the word on hover). Yellow with a gentle pulse means saving. Orange with words means the save failed, for example "Save failed: disk full", and also blocks closing. There's no "unsaved" state, because everything autosaves.
- The tokenizer and encoding show on hover over the token count, not in the bar. Line and column are off by default and can be turned on.
- The menu holds new file, open, recent, save as (save to a file in scratch), find, show or hide line numbers, settings and scratch. On the `rewrite` branch a project's menu adds version history and idea weights.

## 7. Projects: the rewrite flow (`rewrite` branch)

1. In scratch or a `.txt`, paste or write the text, then click the **pen (Rewrite)**.
2. The text **slides left and becomes v1**, read-only. A copy lands on the right as the **draft**.
3. A project is created right then, with no save dialog. It goes under `~/Documents/Dovetail/<first few words>/` and can be renamed later.
4. If it came from scratch, scratch is cleared, with undo available. Matt was unsure about this, so it's the default and easy to change.
5. Work in the draft. Autosave keeps the work-in-progress across many open/close sessions.
6. **Commit** whenever Matt decides it's worth a new version. Commits are never automatic. The version number goes up and the draft becomes the new left-hand reference. Condensing is just another round of the same thing.

### Project format

```
Nyx/
  Nyx.dovetail ← JSON manifest: name, ideas, feeling notes, paint ranges per version. Double-clickable.
  draft.txt   ← current text, readable in anything
  .git/       ← hidden; one commit per version
```

Matt never has to touch git himself.

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
- **Idea weights** (Matt called this the key view): each idea's share of the whole prompt, as a % and in tokens, old vs new. It rolls up to topics.
  - An **Unpainted** row covers untagged text.
  - Text painted with two ideas counts toward both, so totals can go over 100%. That's useful for spotting packed lines.
  - An idea that drops to **0%** is flagged as missing.
  - It can be marked **cut on purpose** to quiet the flag.
- **Feeling notes:** each idea carries a short note in Matt's own words, like "warm but guarded". It's shown next to the cursor while that brush is active. The note can change between versions, and history keeps every version of it.
- **Merge and split ideas:** combine two ideas into one with a new note, keeping where it came from in history, like a git merge. Splitting works the other way round.

## 9. Assistant pane

- A pane that slides in from the right when called. Its toggle is in the status bar, and `Ctrl+L` opens and closes it.
- **Available everywhere** (Matt, 2026-10-17): scratch, plain `.txt` files and projects alike. It's a general assistant to chat with about whatever is being worked on, not a project-only feature.
- **Tool-capable:** it works through function calling rather than a fixed reply format. Tools let it read the document and the selection, search it, count tokens, propose edits and, in a project, read any version, the ideas, the feeling notes and the weights. The app runs each tool call and sends the result back until the model gives its answer.
- **Context:** in a project, it can always see everything: both versions, every idea, the feeling notes and the weights. In a plain doc, it sees the document. Small chips mark what Matt is focused on (the selection, the current idea or topic).
- **Suggest-only in v1.** Proposed edits (from its propose-edit tool) show up in the document as ghost text that can be accepted or rejected piece by piece. In a project, accepted text takes on the idea's colour, so tracking is never lost.
- It can answer questions like "does this still feel warm but guarded?" or "which idea shrank most since v3?"
- **Backend: any OpenAI-compatible API** (decided 2026-10-17), so Matt can point it at whatever he wants: Claude via a compatible endpoint, OpenRouter, Ollama, LM Studio, llama.cpp, vLLM. Settings hold the base URL, the API key (stored in the system keyring: the Secret Service on Linux, the Keychain on macOS, Credential Manager on Windows) and the model name. The default request format is the Responses API (`/v1/responses`), Matt's choice (2026-10-17); a setting switches to Chat Completions (`/v1/chat/completions`) for servers that only speak that. It's plain HTTP from Rust, with no SDK. Streaming replies. Keep the shared context (both versions, ideas, notes, weights) at the start of the prompt so providers that cache prompt prefixes keep repeat questions cheap.

## 10. Settings

A settings panel that opens over the app, from the menu or `Ctrl+,`, in whichever window it was asked for (scratch, a `.txt`, or a project on the `rewrite` branch), with the document dimmed behind it (Matt, 2026-10-18: not a separate window). It scrolls when taller than the window; Esc, its close glyph or a click on the dimmed document closes it. While it's open, typing goes only to its fields and the document's shortcuts are off. Changes apply instantly, and other open windows pick them up too. Settings are stored in `settings.toml` in the app's config folder, so they can be edited by hand: `~/.config/<app>/` on Linux, `~/Library/Application Support/<app>/` on macOS, `%APPDATA%\<app>\` on Windows. The panel shows the exact path.

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
- Before writing code, adapt Cozy's `CLAUDE.md` (github.com/cavecomputing/cozy) for this Rust project. Matt is new to Rust, so it should also explain cargo, crates and the build, test and run commands in plain terms.

## 13. Suggested build order

1. **Plain editor:** a window, the scratch space with resume, autosave with the close guard, `.txt` open and save, the status bar, settings, and 4-space tabs.
2. **Projects:** Rewrite creates a project, two panes (read-only v1 and the draft), Commit with git, and `Ctrl+P` recent files.
3. **Ideas:** brushes, paint that survives edits, gutter stripes, the idea strip, weights, and the missing-idea flags.
4. **Commit check and feeling notes.**
5. **Claude pane** with suggest-only edits.
6. **Later:** Hyprland auto-sense polish, the XML-aware view, Gemini, Cozy prompt export, and idea merge/split.

`main` holds step 1, `Ctrl+P` recent files from step 2, and step 5 without its project parts. Steps 2 to 4 and the project parts of step 5 are on the `rewrite` branch (Matt, 2026-10-18).

## 14. Still open

- Whether scratch should clear or keep its copy after a Rewrite. It clears, with undo, for now.
- How exactly paint should anchor to text so it survives heavy edits. This is a technical design question for the build.
- Whether Ln/Col and the other optional status items show up as Settings toggles.

## Parking lot

- Cozy prompt export, as JSON with `content` and `post_history_content`.
- A diff-on-paste fallback, so paint carries over when text is edited outside the app.
