# Dovetail: implementation spec

A native Linux desktop text editor, written in Rust. By default it's a fast, minimal plain-text editor and scratchpad. On top of that, it has a project mode for iterating on roleplay system prompts, character cards and other writing across versions. That mode keeps track of which ideas from the old version made it into the new one, and how much space each one takes.

- Mockups, drawn under the old working name Word Designer. The boards are plain doc, writing view, Claude pane, idea weights, commit check and settings. They're drawn in HTML only to show the native UI.
- Running brainstorm log: [notes.md](notes.md)
- Name: **Dovetail** (2026-10-17). It replaces the working name Word Designer.

---

## 1. The problem it solves

Rewriting a prompt usually means two windows side by side: the old version on the left, and a line-by-line rewrite on the right. Then come several passes to condense it. The hard part is remembering which phrases and ideas map between versions. The biggest fear is that an idea's feeling or weight gets quietly written out without anyone noticing.

## 2. Look and feel

- **Vibe:** programmer-esque, like a simpler VS Code. Calm and minimal, the way Typora is.
- **Native desktop app**, not a web app. The content fills the window edge to edge, with no framed card or gap around it.
- **No branding** in the app: no logo or wordmark.
- **Colours:** the cavecomputing design system's Gruvbox palette, in dark (default) and light.
  - Roles: `bg-surface` for the window, `bg-page` for the status bar, `bg-raised` for cards and chips, `fg` and `fg-muted` for text. Yellow `accent-text` for focus and active items, aqua for success, orange `accent-warm` for warnings. No red.
- **Font:** JetBrains Mono Nerd Font everywhere, in both the UI and the editor. This deliberately overrides the design system's system-fonts rule. The Nerd Font glyphs can double as the app's icons.
- **Top strip:** about 32px tall, no border, same colour as the page. It holds **only the window controls**, which are plain glyphs, not GNOME circles. No GTK-style big header bars.
  - Auto-sense: hide the app-drawn controls under a tiling WM. On Hyprland, check `HYPRLAND_INSTANCE_SIGNATURE` and/or the Wayland decoration support. Show them everywhere else, since not every machine is tiling. Can be overridden in Settings.
- **Status bar** at the bottom, VS Code style, 24px, mono 11px. See §6.

## 3. Opening behaviour

| Action | Result |
|---|---|
| Launch the app | Opens the one continuous **scratch space**, resuming exactly where it was left (text, cursor, scroll). |
| Open a project file (`.dovetail`) | Opens that project. |
| Open a `.txt` | Opens just that text file. |

- **No tabs.** Each opened file or project gets its own window, which plays well with tiling WMs.
- `Ctrl+P` opens a quick switcher with fuzzy search over recent files and projects.
- `Ctrl+0` (or similar) jumps back to scratch from anywhere.
- `Ctrl+,` opens Settings.
- Startup has to be near-instant: launch, jot, close. This is a key reason to prefer pure Rust over a webview.
- Launch from the app menu or a short terminal command.
- Register as a `.desktop` app with a MIME type for `.dovetail`. Optionally act as a `.txt` handler too.

## 4. Saving

- **Everything autosaves**, scratch included. There's never a "save changes?" prompt.
- **Closing waits for the save.** The window only closes once the latest autosave has been written. If a save fails, the window stays open and says why.
- The scratch space persists between launches as a single document.
- Plain docs are ordinary `.txt` files.

## 5. Plain editor (the default)

- One centred column of text. Word wrap is on by default.
- Reading mode is either **Comfort** (a centred column, 80 characters by default and adjustable) or **Full width**.
- The Tab key inserts **4 spaces** by default (configurable) and never writes a tab character.
- Optional colouring for XML tags and `{{macros}}`. This matters for prompts that use `<system_instructions status="enabled">`, `{{char}}`, `{{user}}` and `{{#x}}…{{/x}}` blocks. Plain prose has to work just as well.
- There's no markdown rendering. It isn't needed.

## 6. Status bar

Left to right:

- **Plain doc:** `☰ menu` · save dot · path (or `Scratch · not saved to a file`) … words · tokens · **pen icon (Rewrite)**
- **Project:** `☰ menu` · save dot · project path · `v3 → draft` … words · tokens · `−45% vs v3` · Claude toggle · **Commit**

Rules:
- The bottom-right corner always holds the **next step**: the pen icon on plain docs, Commit in projects.
- **Save dot:** green means saved (quiet, with the word on hover). Yellow with a gentle pulse means saving. Orange with words means the save failed, for example "Save failed: disk full", and also blocks closing. There's no "unsaved" state, because everything autosaves.
- The tokenizer and encoding show on hover over the token count, not in the bar. Line and column are off by default and can be turned on.
- The menu holds open, recent, settings and version history.

## 7. Projects: the rewrite flow

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

## 8. Ideas: tracking meaning across versions

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

- A pane that slides in from the right when called. Its toggle is in the status bar.
- **Available everywhere** (2026-10-17): scratch, plain `.txt` files and projects alike. It's a general assistant to chat with about whatever is being worked on, not a project-only feature.
- **Tool-capable:** it works through function calling rather than a fixed reply format. Tools let it read the document and the selection, search it, count tokens, propose edits and, in a project, read any version, the ideas, the feeling notes and the weights. The app runs each tool call and sends the result back until the model gives its answer.
- **Context:** in a project, it can always see everything: both versions, every idea, the feeling notes and the weights. In a plain doc, it sees the document. Small chips mark what the owner is focused on (the selection, the current idea or topic).
- **Suggest-only in v1.** Proposed edits (from its propose-edit tool) show up in the document as ghost text that can be accepted or rejected piece by piece. In a project, accepted text takes on the idea's colour, so tracking is never lost.
- It can answer questions like "does this still feel warm but guarded?" or "which idea shrank most since v3?"
- **Backend: any OpenAI-compatible API** (decided 2026-10-17), so it can point at any provider: Claude via a compatible endpoint, OpenRouter, Ollama, LM Studio, llama.cpp, vLLM. Settings hold the base URL, the API key (stored in the system keyring) and the model name. The default request format is the Responses API (`/v1/responses`), the owner's choice (2026-10-17); a setting switches to Chat Completions (`/v1/chat/completions`) for servers that only speak that. It's plain HTTP from Rust, with no SDK. Streaming replies. Keep the shared context (both versions, ideas, notes, weights) at the start of the prompt so providers that cache prompt prefixes keep repeat questions cheap.

## 10. Settings

A settings window opened from the menu or `Ctrl+,`. Changes apply instantly. Settings are stored in `~/.config/<app>/settings.toml`, so they can be edited by hand.

| Section | Settings |
|---|---|
| Appearance | Theme (Gruvbox dark / light / follow system), font, font size, reading mode (Comfort with a character width, or Full width) |
| Editor | Tab inserts N spaces (**default 4**, never tabs), word wrap (**on** by default), XML and macro colouring |
| Saving | Autosave timing, projects folder |
| Claude | Base URL, API key (keyring), model, request format (Responses, default / Chat Completions) |
| Window | Window controls: Auto / Always show / Always hide |

## 11. Counting

- Word and token counts update live in both panes.
- The tokenizer is OpenAI's o200k, via `tiktoken-rs`. That's a good enough estimate for Claude and Gemini.
- Later: count `{{macros}}` separately from real text, since they expand at runtime.

## 12. Tech direction (not locked)

- **Pure Rust is preferred.** Startup is fast, and there's no markdown or rich-text requirement, so plain text with colouring is well within reach.
  - UI candidates: **iced** (used by COSMIC, Linux-first) or **GPUI** (Zed's toolkit, excellent text, thinner docs). egui and Slint are possible but weaker for long text.
  - Tauri remains a fallback if a native editor gets too hard. It's slower to start.
- Likely crates: `git2` or `gix` for history, `tiktoken-rs` for counts, `serde` and `serde_json` for the manifest, `toml` for settings, `reqwest` for the OpenAI-compatible API, `keyring` for the API key, plus `notify`/`directories` as needed.
- **No name is hardcoded.** The app name (Dovetail), the project extension (`.dovetail`), the config folder (`~/.config/dovetail/`), the projects folder and the `.desktop`/MIME IDs all come from one constants module (`src/brand.rs`), so renaming later still means changing one file.
- Before writing code, write a `CLAUDE.md` for this Rust project. The owner is new to Rust, so it should also explain cargo, crates and the build, test and run commands in plain terms.

## 13. Suggested build order

1. **Plain editor:** a window, the scratch space with resume, autosave with the close guard, `.txt` open and save, the status bar, settings, and 4-space tabs.
2. **Projects:** Rewrite creates a project, two panes (read-only v1 and the draft), Commit with git, and `Ctrl+P` recent files.
3. **Ideas:** brushes, paint that survives edits, gutter stripes, the idea strip, weights, and the missing-idea flags.
4. **Commit check and feeling notes.**
5. **Claude pane** with suggest-only edits.
6. **Later:** Hyprland auto-sense polish, the XML-aware view, Gemini, prompt export, and idea merge/split.

## 14. Still open

- Whether scratch should clear or keep its copy after a Rewrite. It clears, with undo, for now.
- How exactly paint should anchor to text so it survives heavy edits. This is a technical design question for the build.
- Whether Ln/Col and the other optional status items show up as Settings toggles.

## Parking lot

- Prompt export, as JSON with `content` and `post_history_content`.
- A diff-on-paste fallback, so paint carries over when text is edited outside the app.
