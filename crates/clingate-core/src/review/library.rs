//! The review library: one folder, chosen once, that every reviewed run is
//! copied into - its review and its reports - so what reviewers found adds up
//! across runs rather than staying scattered through run folders. Typically a
//! shared drive, so colleagues' reviews add up too.
//!
//! Where it is is a setting of this computer's clingate, not of a workspace,
//! kept in clingate's settings file; the app sets it, and the app and the
//! tools for Claude both read it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// clingate's settings on this computer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The review library, if one has been chosen.
    #[serde(default)]
    pub review_library: Option<PathBuf>,
    /// Whatever else the file holds - settings a newer clingate keeps there -
    /// written back as it was rather than dropped when this one saves.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Where the settings are kept: clingate's folder in the user's config
/// directory, beside the last workspace.
pub fn settings_file() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("clingate").join("settings.json"))
}

impl Settings {
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        crate::workspace::make_parent(path)?;
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// The review library this computer's clingate is set to, if any.
pub fn configured() -> Option<PathBuf> {
    let file = settings_file()?;
    match Settings::load_from(&file) {
        Ok(s) => s.review_library,
        Err(e) => {
            tracing::warn!("{} could not be read: {e}", file.display());
            None
        }
    }
}

/// Set - or with `None`, forget - the review library.
pub fn set(library: Option<PathBuf>) -> anyhow::Result<()> {
    let file = settings_file().ok_or_else(|| anyhow::anyhow!("no config folder on this system"))?;
    let mut settings = Settings::load_from(&file).unwrap_or_default();
    settings.review_library = library;
    settings.save_to(&file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_settings_file_is_no_library() {
        let folder = crate::file_load_tests::scratch("settings-missing");
        let settings = Settings::load_from(&folder.join("settings.json")).unwrap();
        assert_eq!(settings.review_library, None);
    }

    #[test]
    fn the_library_is_kept_and_read_back() {
        let folder = crate::file_load_tests::scratch("settings-round-trip");
        let file = folder.join("clingate").join("settings.json");
        let settings = Settings {
            review_library: Some(PathBuf::from("/shared/reviews")),
            ..Settings::default()
        };
        settings.save_to(&file).unwrap();
        assert_eq!(Settings::load_from(&file).unwrap(), settings);
        // And forgotten.
        Settings::default().save_to(&file).unwrap();
        assert_eq!(Settings::load_from(&file).unwrap().review_library, None);
    }

    #[test]
    fn settings_this_version_does_not_know_survive_a_save() {
        let folder = crate::file_load_tests::scratch("settings-unknown");
        let file = folder.join("settings.json");
        std::fs::write(&file, r#"{"review_library": "/old", "theme": "dark"}"#).unwrap();
        let mut settings = Settings::load_from(&file).unwrap();
        settings.review_library = Some(PathBuf::from("/new"));
        settings.save_to(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["theme"], "dark", "{text}");
        assert_eq!(back["review_library"], "/new");
    }

    #[test]
    fn a_damaged_settings_file_is_an_error_not_a_silent_default() {
        let folder = crate::file_load_tests::scratch("settings-damaged");
        let file = folder.join("settings.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(Settings::load_from(&file).is_err());
    }
}
