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
