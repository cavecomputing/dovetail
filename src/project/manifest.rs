//! The JSON manifest (`<name>.<PROJECT_EXT>`): the project's name, its versions and its ideas.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::save::atomic_write;
use crate::project::ideas::{Idea, Topic};
use crate::project::paint::Span;

/// Bumped when the manifest layout changes. 2 added ideas, topics and paint; every new field has
/// a default, so a format 1 manifest still opens (and is written back as format 2).
pub const FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    /// Oldest first, so the last one is the newest version.
    pub versions: Vec<Version>,
    #[serde(default)]
    pub ideas: Vec<Idea>,
    #[serde(default)]
    pub topics: Vec<Topic>,
    #[serde(default)]
    pub draft: Draft,
}

/// What the manifest keeps about the draft besides its text (which is `draft.txt`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// The draft's paint as of its last autosave, in byte offsets into `draft.txt`.
    #[serde(default)]
    pub paint: Vec<Span>,
}

/// One committed version. Its text lives in git under the tag `v<number>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub number: u32,
    pub note: String,
    /// Seconds since 1970 (Unix time).
    pub committed_at: u64,
    /// This version's paint, in byte offsets into its text. Fixed once committed, except when
    /// The writer paints the old version itself.
    #[serde(default)]
    pub paint: Vec<Span>,
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
            topics: Vec::new(),
            draft: Draft::default(),
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Manifest> {
        let text = std::fs::read_to_string(path)?;
        let mut manifest: Manifest = serde_json::from_str(&text)?;
        if manifest.format > FORMAT_VERSION {
            anyhow::bail!("this project was saved by a newer version of the app");
        }
        manifest.format = FORMAT_VERSION;
        Ok(manifest)
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

    /// The number the draft will get when it is committed. Notes and "cut on purpose" marks made
    /// while editing the draft are filed under it.
    pub fn draft_version(&self) -> u32 {
        self.latest_version() + 1
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
            paint: vec![Span {
                idea: 1,
                start: 0,
                end: 4,
                quote: "Nyx ".to_string(),
            }],
        });
        manifest.add_idea("Autonomy");
        manifest.draft.paint = manifest.versions[0].paint.clone();
        manifest.save(&path).unwrap();
        assert_eq!(Manifest::load(&path).unwrap(), manifest);
        assert_eq!(manifest.latest_version(), 1);
    }

    #[test]
    fn format_1_manifests_still_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("A.json");
        // Exactly what phase 2 wrote, and an even older one without the placeholders.
        for text in [
            r#"{"format":1,"name":"A","versions":[{"number":1,"note":"","committed_at":0,"paint":[]}],"ideas":[]}"#,
            r#"{"format":1,"name":"A","versions":[{"number":1,"note":"","committed_at":0}]}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            let manifest = Manifest::load(&path).unwrap();
            assert_eq!(manifest.format, FORMAT_VERSION);
            assert!(manifest.ideas.is_empty() && manifest.topics.is_empty());
            assert!(manifest.versions[0].paint.is_empty());
            assert!(manifest.draft.paint.is_empty());
            assert_eq!(manifest.draft_version(), 2);
        }
    }

    #[test]
    fn refuses_a_manifest_from_a_newer_app() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("A.json");
        std::fs::write(&path, r#"{"format":99,"name":"A","versions":[]}"#).unwrap();
        assert!(Manifest::load(&path).is_err());
    }

    #[test]
    fn no_versions_means_latest_is_zero() {
        assert_eq!(Manifest::new("A").latest_version(), 0);
    }
}
