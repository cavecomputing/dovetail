//! The assistant's API key. It lives in the system keyring (the Secret Service on Linux, which
//! gnome-keyring and KeePassXC provide). Only when no keyring is running does it go in
//! settings.toml, which is then readable by its owner only; the Settings window says which.
//!
//! These calls talk to the keyring over D-Bus and can block (it may ask to unlock), so call them
//! off the UI thread.

use keyring::{Entry, Error};

use crate::brand::APP_ID;
use crate::core::settings::Settings;

/// The keyring entry's user name; the service is the app id.
const KEY_USER: &str = "api-key";

/// Where the key is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyStore {
    Keyring,
    /// settings.toml, mode 0600, because no keyring is running.
    SettingsFile,
}

/// Where a key is kept on this machine. Connects to the keyring the first time.
pub fn key_store() -> KeyStore {
    match Entry::store_status() {
        Ok(()) => KeyStore::Keyring,
        Err(_) => KeyStore::SettingsFile,
    }
}

/// The saved key, or `None` when there is none.
pub fn api_key(settings: &Settings) -> Result<Option<String>, String> {
    read_key(settings, key_store())
}

/// Saves `key` (an empty one deletes it). With a keyring, any copy in `settings` is removed; with
/// none, the key goes in `settings`. Either way, save the settings afterwards.
pub fn set_api_key(key: &str, settings: &mut Settings) -> Result<KeyStore, String> {
    let store = key_store();
    write_key(key, settings, store)?;
    Ok(store)
}

fn read_key(settings: &Settings, store: KeyStore) -> Result<Option<String>, String> {
    // A key saved while no keyring was running still counts.
    let in_settings = settings
        .api_key_fallback
        .clone()
        .filter(|key| !key.trim().is_empty());
    if store == KeyStore::SettingsFile {
        return Ok(in_settings);
    }
    match Entry::new(APP_ID, KEY_USER).and_then(|entry| entry.get_password()) {
        Ok(key) => Ok(Some(key)),
        Err(Error::NoEntry) => Ok(in_settings),
        Err(error) => Err(format!(
            "Couldn't read the API key from the keyring: {error}"
        )),
    }
}

fn write_key(key: &str, settings: &mut Settings, store: KeyStore) -> Result<(), String> {
    let key = key.trim();
    if store == KeyStore::SettingsFile {
        settings.api_key_fallback = (!key.is_empty()).then(|| key.to_string());
        return Ok(());
    }
    let entry = Entry::new(APP_ID, KEY_USER).map_err(keyring_words)?;
    match key.is_empty() {
        true => match entry.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => {}
            Err(error) => return Err(keyring_words(error)),
        },
        false => entry.set_password(key).map_err(keyring_words)?,
    }
    settings.api_key_fallback = None;
    Ok(())
}

fn keyring_words(error: Error) -> String {
    format!("Couldn't save the API key in the keyring: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the settings-file path is tested: the keyring path would touch the real keyring of
    // whoever runs the tests.
    #[test]
    fn without_a_keyring_the_key_lives_in_settings() {
        let mut settings = Settings::default();
        assert_eq!(read_key(&settings, KeyStore::SettingsFile), Ok(None));

        write_key("  sk-test  ", &mut settings, KeyStore::SettingsFile).unwrap();
        assert_eq!(settings.api_key_fallback.as_deref(), Some("sk-test"));
        assert_eq!(
            read_key(&settings, KeyStore::SettingsFile),
            Ok(Some("sk-test".to_string()))
        );

        write_key("", &mut settings, KeyStore::SettingsFile).unwrap();
        assert_eq!(settings.api_key_fallback, None);

        settings.api_key_fallback = Some("   ".to_string());
        assert_eq!(read_key(&settings, KeyStore::SettingsFile), Ok(None));
    }
}
