//! `settings.toml`: the user's preferences, with defaults for anything missing.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::save::{atomic_write, failure_reason};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

/// Full width (the default) lets text use the whole window; Comfort caps lines at `chars`
/// characters. Either way the text starts at the top left, like any text editor.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ReadingMode {
    Comfort {
        #[serde(default = "default_comfort_chars")]
        chars: u16,
    },
    #[default]
    FullWidth,
}

fn default_comfort_chars() -> u16 {
    80
}

/// Whether the app draws its own minimise/maximise/close glyphs in the top strip.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowControls {
    /// Hide them under a tiling window manager, show them elsewhere.
    #[default]
    Auto,
    Show,
    Hide,
}

/// Which file's rules scratch follows: plain text, or Markdown's colours and list keys.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchSyntax {
    #[default]
    Plain,
    Markdown,
}

/// How requests to the assistant's server are shaped. Both are OpenAI's formats, which most
/// servers copy.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestFormat {
    /// `POST {base_url}/responses`.
    #[default]
    Responses,
    /// `POST {base_url}/chat/completions`, for servers that only speak that.
    ChatCompletions,
}

/// Where the assistant's requests go when the base URL is left blank.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Every setting from spec section 10. A key missing from the file takes its default and an
/// unknown key is ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub font_size: f32,
    pub reading_mode: ReadingMode,
    /// How many spaces the Tab key inserts. It never writes a tab character.
    pub tab_spaces: u8,
    pub word_wrap: bool,
    /// Each line's number left of the text, like vim's `set number`.
    pub line_numbers: bool,
    /// Colour XML tags and `{{macros}}`.
    pub colour_markup: bool,
    pub scratch_syntax: ScratchSyntax,
    /// How long typing must pause before an autosave starts.
    pub autosave_delay_ms: u64,
    /// The assistant's server: any OpenAI-compatible API. Blank means `DEFAULT_BASE_URL`.
    pub base_url: String,
    /// The model name the server knows. Empty until set; the assistant pane asks for one.
    pub model: String,
    pub request_format: RequestFormat,
    /// The API key, kept here on macOS, and on Linux when there is no system keyring (see
    /// `ai::key`). The file is then readable by its owner only.
    pub api_key_fallback: Option<String>,
    pub window_controls: WindowControls,
    /// Show line and column in the status bar.
    pub show_line_col: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: Theme::Dark,
            font_size: 14.0,
            reading_mode: ReadingMode::default(),
            tab_spaces: 4,
            word_wrap: true,
            line_numbers: false,
            colour_markup: true,
            scratch_syntax: ScratchSyntax::Plain,
            autosave_delay_ms: 500,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: String::new(),
            request_format: RequestFormat::Responses,
            api_key_fallback: None,
            window_controls: WindowControls::Auto,
            show_line_col: false,
        }
    }
}

impl Settings {
    /// Reads the settings file. Never fails: a missing file gives the defaults silently. A bad
    /// value takes its default on its own, so the other keys are kept, and the warning names it.
    /// A file that can't be read or isn't TOML at all gives the defaults plus a warning.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return (Settings::default(), None);
            }
            Err(error) => {
                let warning = format!("Couldn't read settings, using defaults: {error}");
                return (Settings::default(), Some(warning));
            }
        };
        let mut table: toml::Table = match toml::from_str(&text) {
            Ok(table) => table,
            Err(error) => {
                let warning = format!(
                    "Settings file has a mistake, using defaults: {}",
                    error.message()
                );
                return (Settings::default(), Some(warning));
            }
        };
        // Every key parses on its own (`#[serde(default)]` fills in the rest), so a key that
        // doesn't is a bad value: it is dropped and named, and the others still load.
        let mut bad_keys = Vec::new();
        table.retain(|name, value| {
            let alone = toml::Table::from_iter([(name.to_string(), value.clone())]);
            let parses = alone.try_into::<Settings>().is_ok();
            if !parses {
                bad_keys.push(name.to_string());
            }
            parses
        });
        let mut settings: Settings = table
            .try_into()
            .expect("keys that parse alone parse together");
        settings.clamp();
        let warning = if bad_keys.is_empty() {
            None
        } else {
            Some(format!(
                "Mistake in settings.toml: {} (using the default)",
                bad_keys.join(", ")
            ))
        };
        (settings, warning)
    }

    /// Keeps every number in a range the editor can draw, whether it came from the Settings
    /// window or was typed into the file by hand.
    pub fn clamp(&mut self) {
        if !self.font_size.is_finite() {
            self.font_size = Settings::default().font_size;
        }
        self.font_size = self.font_size.clamp(9.0, 40.0);
        if let ReadingMode::Comfort { chars } = self.reading_mode {
            self.reading_mode = ReadingMode::Comfort {
                chars: chars.clamp(30, 300),
            };
        }
        self.tab_spaces = self.tab_spaces.clamp(1, 16);
        self.autosave_delay_ms = self.autosave_delay_ms.min(10_000);
    }

    /// Writes the settings file atomically, creating its folder if needed. When it holds the
    /// API key, the file is made readable by its owner only before the key goes in.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        #[cfg(unix)]
        if self.api_key_fallback.is_some() {
            owner_only(path)?;
        }
        atomic_write(path, text.as_bytes())
    }

    /// Saves, unless the file on disk has a mistake or can't be read: then it is left alone for
    /// the user to fix (saving would replace their hand edits), and the error says so.
    pub fn save_unless_broken(&self, path: &Path) -> Result<(), String> {
        if let (_, Some(warning)) = Settings::load(path) {
            return Err(format!("{warning} · not saving until it's fixed"));
        }
        self.save(path).map_err(|error| failure_reason(&error))
    }
}

/// Creates `path` if needed and sets it to mode 0600 (read and write by its owner only).
/// `atomic_write` keeps a file's permissions, so the saved settings stay that way.
/// Unix only: on Windows the key always goes in the Credential Manager, never in this file.
#[cfg(unix)]
fn owner_only(path: &Path) -> io::Result<()> {
    use std::fs::{OpenOptions, Permissions};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(Permissions::from_mode(0o600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let settings = Settings::default();
        assert_eq!(settings.tab_spaces, 4);
        assert!(settings.word_wrap);
        assert!(!settings.line_numbers);
        assert!(settings.colour_markup);
        assert_eq!(settings.scratch_syntax, ScratchSyntax::Plain);
        assert!(!settings.show_line_col);
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.font_size, 14.0);
        assert_eq!(settings.reading_mode, ReadingMode::FullWidth);
        assert_eq!(settings.window_controls, WindowControls::Auto);
        assert_eq!(settings.base_url, DEFAULT_BASE_URL);
        assert_eq!(settings.model, "");
        assert_eq!(settings.request_format, RequestFormat::Responses);
        assert_eq!(settings.api_key_fallback, None);
    }

    #[test]
    fn missing_file_gives_defaults_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let (settings, warning) = Settings::load(&dir.path().join("settings.toml"));
        assert_eq!(settings, Settings::default());
        assert!(warning.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/settings.toml");
        let settings = Settings {
            theme: Theme::Light,
            reading_mode: ReadingMode::FullWidth,
            tab_spaces: 2,
            line_numbers: true,
            scratch_syntax: ScratchSyntax::Markdown,
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), (settings, None));
    }

    #[test]
    fn partial_file_fills_in_defaults_and_ignores_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "tab_spaces = 8\nsomething_new = 1\n[reading_mode]\nmode = \"comfort\"\n",
        )
        .unwrap();
        let (settings, warning) = Settings::load(&path);
        assert!(warning.is_none());
        assert_eq!(settings.tab_spaces, 8);
        assert_eq!(settings.reading_mode, ReadingMode::Comfort { chars: 80 });
        assert!(settings.word_wrap);
    }

    #[test]
    fn hand_written_values_parse() {
        let text = "theme = \"system\"\nwindow_controls = \"hide\"\nline_numbers = true\n[reading_mode]\nmode = \"comfort\"\nchars = 100\n";
        let settings: Settings = toml::from_str(text).unwrap();
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.window_controls, WindowControls::Hide);
        assert!(settings.line_numbers);
        assert_eq!(settings.reading_mode, ReadingMode::Comfort { chars: 100 });
    }

    #[test]
    fn one_bad_value_keeps_the_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "theme = \"purple\"\ntab_spaces = 2\nfont_size = 20\n",
        )
        .unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.tab_spaces, 2);
        assert_eq!(settings.font_size, 20.0);
        assert!(warning.unwrap().contains("settings.toml: theme"));
    }

    #[test]
    fn file_that_isnt_toml_gives_defaults_and_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "theme = ").unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(settings, Settings::default());
        assert!(warning.unwrap().contains("using defaults"));
    }

    #[test]
    fn hand_written_numbers_are_kept_in_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "font_size = 0\ntab_spaces = 255\nautosave_delay_ms = 99999999\n[reading_mode]\nmode = \"comfort\"\nchars = 0\n",
        )
        .unwrap();
        let (settings, warning) = Settings::load(&path);
        assert!(warning.is_none());
        assert_eq!(settings.font_size, 9.0);
        assert_eq!(settings.tab_spaces, 16);
        assert_eq!(settings.autosave_delay_ms, 10_000);
        assert_eq!(settings.reading_mode, ReadingMode::Comfort { chars: 30 });

        let mut odd = Settings {
            font_size: f32::NAN,
            tab_spaces: 0,
            ..Settings::default()
        };
        odd.clamp();
        assert_eq!(odd.font_size, Settings::default().font_size);
        assert_eq!(odd.tab_spaces, 1);
    }

    #[test]
    fn a_broken_file_is_not_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let hand_written = "theme = \"purple\"\nfont_size = 20\n";
        fs::write(&path, hand_written).unwrap();
        let error = Settings::default().save_unless_broken(&path).unwrap_err();
        assert!(error.contains("not saving"));
        assert_eq!(fs::read_to_string(&path).unwrap(), hand_written);

        fs::write(&path, "font_size = 20\n").unwrap();
        Settings::default().save_unless_broken(&path).unwrap();
        assert_eq!(Settings::load(&path).0, Settings::default());
    }

    #[test]
    fn the_old_claude_model_is_ignored() {
        // Every save before the assistant wrote the old default, which no server here knows, so
        // the pane asks for a model instead.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "claude_model = \"claude-opus-5-5\"\n").unwrap();
        let (settings, warning) = Settings::load(&path);
        assert!(warning.is_none());
        assert_eq!(settings.model, "");
        settings.save(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("claude_model"));
    }

    #[test]
    fn assistant_settings_parse_by_hand() {
        let text =
            "base_url = \"http://localhost:11434/v1/\"\nrequest_format = \"chat_completions\"\n";
        let settings: Settings = toml::from_str(text).unwrap();
        assert_eq!(settings.base_url, "http://localhost:11434/v1/");
        assert_eq!(settings.request_format, RequestFormat::ChatCompletions);
        let (_, warning) = {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            fs::write(&path, "request_format = \"carrier pigeon\"\n").unwrap();
            Settings::load(&path)
        };
        assert!(warning.unwrap().contains("request_format"));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_holding_the_key_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/settings.toml");
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

        Settings::default().save(&path).unwrap();
        let with_key = Settings {
            api_key_fallback: Some("sk-test".to_string()),
            ..Settings::default()
        };
        with_key.save(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(Settings::load(&path).0, with_key);

        // A new file with a key starts private too.
        let fresh = dir.path().join("other/settings.toml");
        with_key.save(&fresh).unwrap();
        assert_eq!(mode(&fresh), 0o600);
    }
}
