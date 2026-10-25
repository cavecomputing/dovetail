<p align="center">
  <img src="assets/icon/icon.svg" alt="Dovetail" width="128">
</p>

<p align="center">
  A fast, quiet plain-text editor and scratchpad
</p>

Dovetail is a plain-text editor for notes and config files. Open it and you're straight into a
scratchpad that remembers exactly where you left off. Each launch gets its own window, and
opening another file from inside one swaps it in, once the one you're leaving is saved. Everything
saves itself as you type, so there's no "save changes?" prompt, ever.

Markdown, JSON, YAML, TOML, INI, SSH config and shell script files are coloured as you type (the
text stays exactly as you wrote it), and the data and config files are checked too: the status bar
says where one stops being valid. JSON files can be formatted or minified from the command palette,
and an SSH config gets a new `Host` block from it.

There's also an optional assistant pane (`Ctrl+L`) that can suggest edits for you to accept or
reject, using any OpenAI-compatible service you point it at. The editor works fine without it; the
[guide](docs/guide.md#the-assistant) covers setting it up.

<p align="center">
  <img src="docs/screenshot.png" alt="A scratch window with the assistant pane open and a suggested edit shown in the text" width="80%">
</p>

> [!WARNING]
> Dovetail is a work in progress. It's usable day to day, but things still change, break and get
> moved around. Keep anything important backed up somewhere else too.

## Quick start

Dovetail is built on your own computer from this repository. You need Git and
[Rust](https://rustup.rs) installed first (the [guide](docs/guide.md#install) lists the few extra
bits each OS needs).

```sh
git clone https://github.com/cavecomputing/text.git
cd text
scripts/install.sh            # Linux
scripts/install-macos.sh      # macOS
```

On Windows, in PowerShell inside the `text` folder:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1
```

The first build takes a few minutes. After that, start Dovetail from your app launcher (or Start
Menu), or from a terminal:

```sh
dovetail              # opens the scratchpad
dovetail notes.txt    # opens a file
```

Linux comes first. macOS and Windows work too, but get less use.

## Handy keys

| Key | What it does |
|---|---|
| Ctrl+N | New file |
| Ctrl+O | Open a file |
| Ctrl+P | Scratch and the recent files |
| Ctrl+Shift+P | The open file's tools (Format and Minify JSON) |
| Ctrl+F | Find |
| Ctrl+Shift+F | Find and replace |
| Ctrl+S | Save now (in the scratchpad, save it to a file) |
| Ctrl+L | Show or hide the assistant |
| Ctrl+, | Settings |

On a Mac, use ⌘ instead of Ctrl. The full list is in the [guide](docs/guide.md#keys).

## Updating

Run `git pull` in the `text` folder, then the same install script again. Your settings, scratchpad
and API key are kept.

## More

- [The guide](docs/guide.md): installing on each OS, every key, how saving works, the assistant,
  every setting, and working on the code.
- [The design](docs/spec.md): what Dovetail is meant to become.

Dovetail is MIT licensed: see [LICENSE](LICENSE). It bundles JetBrains Mono NL Nerd Font, under
the SIL Open Font License: see [assets/fonts/OFL.txt](assets/fonts/OFL.txt).
