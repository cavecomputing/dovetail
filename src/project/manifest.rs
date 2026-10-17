//! The JSON manifest (`<name>.<PROJECT_EXT>`): the project's name, its versions and its ideas.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::save::atomic_write;

/// Bumped when the manifest layout changes in a way older builds can't read.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    /// Oldest first, so the last one is the newest version.
    pub versions: Vec<Version>,
    /// Ideas arrive in phase 3. Kept as raw JSON until then so nothing written here is lost.
    #[serde(default)]
    pub ideas: Vec<serde_json::Value>,
}

/// One committed version. Its text lives in git under the tag `v<number>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub number: u32,
    pub note: String,
    /// Seconds since 1970 (Unix time).
    pub committed_at: u64,
    /// Paint ranges arrive in phase 3; raw JSON until then.
    #[serde(default)]
    pub paint: Vec<serde_json::Value>,
}

impl Version {
    /// The day it was committed, such as "Oct 12" (in UTC, which needs no time zone data).
    pub fn committed_on(&self) -> String {
        // Days since 1970 to a calendar date, from Howard Hinnant's `civil_from_days`.
        let days = (self.committed_at / 86_400) as i64 + 719_468;
        let day_of_era = days.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153; // 0 is March
        let day = day_of_year - (153 * month_index + 2) / 5 + 1;
        const MONTHS: [&str; 12] = [
            "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec", "Jan", "Feb",
        ];
        format!("{} {day}", MONTHS[month_index as usize])
    }
}

impl Manifest {
    pub fn new(name: &str) -> Manifest {
        Manifest {
            format: FORMAT_VERSION,
            name: name.to_string(),
            versions: Vec::new(),
            ideas: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Manifest> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        atomic_write(path, text.as_bytes())?;
        Ok(())
    }

    /// The newest version's number, 0 before the first commit.
    pub fn latest_version(&self) -> u32 {
        self.versions.last().map_or(0, |version| version.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed_at(committed_at: u64) -> Version {
        Version {
            number: 1,
            note: String::new(),
            committed_at,
            paint: Vec::new(),
        }
    }

    #[test]
    fn commit_dates_read_as_month_and_day() {
        assert_eq!(committed_at(0).committed_on(), "Jan 1");
        assert_eq!(committed_at(951_782_400).committed_on(), "Feb 29"); // 2000, a leap year
        assert_eq!(committed_at(1_760_270_400).committed_on(), "Oct 12"); // 2025
        assert_eq!(committed_at(1_767_225_599).committed_on(), "Dec 31");
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Nyx.json");
        let mut manifest = Manifest::new("Nyx");
        manifest.versions.push(Version {
            number: 1,
            note: "first".to_string(),
            committed_at: 1_700_000_000,
            paint: vec![serde_json::json!({"idea": 1, "start": 0, "end": 4})],
        });
        manifest.ideas.push(serde_json::json!({"name": "Autonomy"}));
        manifest.save(&path).unwrap();
        assert_eq!(Manifest::load(&path).unwrap(), manifest);
        assert_eq!(manifest.latest_version(), 1);
    }

    #[test]
    fn missing_placeholders_default_to_empty() {
        let text =
            r#"{"format":1,"name":"A","versions":[{"number":1,"note":"","committed_at":0}]}"#;
        let manifest: Manifest = serde_json::from_str(text).unwrap();
        assert!(manifest.ideas.is_empty());
        assert!(manifest.versions[0].paint.is_empty());
    }

    #[test]
    fn no_versions_means_latest_is_zero() {
        assert_eq!(Manifest::new("A").latest_version(), 0);
    }
}
