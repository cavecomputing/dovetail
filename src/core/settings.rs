//! `settings.toml`: the user's preferences, with defaults for anything missing.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
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

/// Comfort is a centred column `chars` characters wide; full width uses the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ReadingMode {
    Comfort {
        #[serde(default = "default_comfort_chars")]
        chars: u16,
    },
    FullWidth,
}

impl Default for ReadingMode {
    fn default() -> ReadingMode {
        ReadingMode::Comfort {
            chars: default_comfort_chars(),
        }
    }
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
    /// Colour XML tags and `{{macros}}`.
    pub colour_markup: bool,
    /// How long typing must pause before an autosave starts.
    pub autosave_delay_ms: u64,
    /// Where new projects go. `None` means the default from `Paths::projects_dir`.
    pub projects_folder: Option<PathBuf>,
    pub claude_model: String,
    pub window_controls: WindowControls,
    /// Show line and column in the status bar.
    pub show_line_col: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: Theme::Dark,
            font_size: 16.0,
            reading_mode: ReadingMode::default(),
            tab_spaces: 4,
            word_wrap: true,
            colour_markup: true,
            autosave_delay_ms: 500,
            projects_folder: None,
            claude_model: "claude-opus-5-5".to_string(),
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
        let table: toml::Table = match toml::from_str(&text) {
            Ok(table) => table,
            Err(error) => {
                let warning = format!(
                    "Settings file has a mistake, using defaults: {}",
                    error.message()
                );
                return (Settings::default(), Some(warning));
            }
        };
        let defaults = Settings::default();
        let mut bad_keys = Vec::new();
        let mut settings = Settings {
            theme: key(&table, "theme", defaults.theme, &mut bad_keys),
            font_size: key(&table, "font_size", defaults.font_size, &mut bad_keys),
            reading_mode: key(&table, "reading_mode", defaults.reading_mode, &mut bad_keys),
            tab_spaces: key(&table, "tab_spaces", defaults.tab_spaces, &mut bad_keys),
            word_wrap: key(&table, "word_wrap", defaults.word_wrap, &mut bad_keys),
            colour_markup: key(
                &table,
                "colour_markup",
                defaults.colour_markup,
                &mut bad_keys,
            ),
            autosave_delay_ms: key(
                &table,
                "autosave_delay_ms",
                defaults.autosave_delay_ms,
                &mut bad_keys,
            ),
            projects_folder: key(
                &table,
                "projects_folder",
                defaults.projects_folder,
                &mut bad_keys,
            ),
            claude_model: key(&table, "claude_model", defaults.claude_model, &mut bad_keys),
            window_controls: key(
                &table,
                "window_controls",
                defaults.window_controls,
                &mut bad_keys,
            ),
            show_line_col: key(
                &table,
                "show_line_col",
                defaults.show_line_col,
                &mut bad_keys,
            ),
        };
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

    /// Writes the settings file atomically, creating its folder if needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
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

/// The value of `name` in the file, or `fallback` if it is missing or wrong (then its name is
/// added to `bad_keys`, except when it's just missing).
fn key<T: DeserializeOwned>(
    table: &toml::Table,
    name: &str,
    fallback: T,
    bad_keys: &mut Vec<String>,
) -> T {
    let Some(value) = table.get(name) else {
        return fallback;
    };
    match value.clone().try_into() {
        Ok(value) => value,
        Err(_) => {
            bad_keys.push(name.to_string());
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let settings = Settings::default();
        assert_eq!(settings.tab_spaces, 4);
        assert!(settings.word_wrap);
        assert!(settings.colour_markup);
        assert!(!settings.show_line_col);
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.reading_mode, ReadingMode::Comfort { chars: 80 });
        assert_eq!(settings.window_controls, WindowControls::Auto);
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
            projects_folder: Some(PathBuf::from("/home/me/stuff")),
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
        let text = "theme = \"system\"\nwindow_controls = \"hide\"\n[reading_mode]\nmode = \"comfort\"\nchars = 100\n";
        let settings: Settings = toml::from_str(text).unwrap();
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.window_controls, WindowControls::Hide);
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
        assert_eq!(odd.font_size, 16.0);
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
}
