//! What a workspace is made of, and finding it in a folder.
//!
//! A workspace is four things: the FCS files, the metadata that says which
//! file is which sample, the scaling that says how each channel is displayed,
//! and the gating file that holds the gates. They used to be named by line
//! number in a `file_paths.txt` beside the binary; they are now chosen in the
//! app, and this module is the part of that which does not need a window.
//!
//! ## Finding them in a folder
//!
//! Opening a folder is the quick way in, so it tries to recognise each part:
//!
//! - **FCS files** anywhere under the folder, sub-folders included. Runs are
//!   routinely kept one folder per plate.
//! - **The gating file**: an `.omiqgt` at the top level.
//! - **The metadata and the scaling**: a `.csv` at the top level with
//!   "metadata" or "scaling" in its name.
//!
//! Only the FCS files are searched for recursively. A sub-folder full of old
//! exports is common, and a second `scaling.csv` three folders down is far
//! more likely to be last month's than the one wanted.
//!
//! Nothing is guessed. Where a part is missing, or more than one file could be
//! it, that is reported as such and the person chooses.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What a search turned up for one part of the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Missing,
    One(PathBuf),
    /// More than one file could be it, so none is chosen.
    Several(Vec<PathBuf>),
}

impl Found {
    fn from(mut candidates: Vec<PathBuf>) -> Self {
        candidates.sort();
        match candidates.len() {
            0 => Found::Missing,
            1 => Found::One(candidates.remove(0)),
            _ => Found::Several(candidates),
        }
    }

    /// The one file, when there is exactly one.
    pub fn one(&self) -> Option<&Path> {
        match self {
            Found::One(path) => Some(path),
            _ => None,
        }
    }
}

/// Everything a folder turned up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub fcs: Vec<PathBuf>,
    pub gating: Found,
    pub metadata: Found,
    pub scaling: Found,
}

/// Look through a folder for the parts of a workspace.
pub fn detect(folder: &Path) -> std::io::Result<Detected> {
    let mut gating = Vec::new();
    let mut metadata = Vec::new();
    let mut scaling = Vec::new();

    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if hidden(&entry.file_name()) || entry_kind(&entry)? != Kind::File {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if has_extension(&path, "omiqgt") {
            gating.push(path);
        } else if has_extension(&path, "csv") {
            // A name carrying both words is a candidate for both, which makes
            // each ambiguous rather than letting the order of these two lines
            // decide.
            if name.contains("metadata") {
                metadata.push(path.clone());
            }
            if name.contains("scaling") {
                scaling.push(path);
            }
        }
    }

    Ok(Detected {
        fcs: fcs_under(folder)?,
        gating: Found::from(gating),
        metadata: Found::from(metadata),
        scaling: Found::from(scaling),
    })
}

/// Every FCS file under `folder`, sub-folders included, sorted.
///
/// Symbolic links to folders are not followed. A link back up the tree would
/// otherwise recurse until the stack ran out, and following links is also how
/// a search wanders onto a network drive nobody pointed it at.
pub fn fcs_under(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if hidden(&entry.file_name()) {
                continue;
            }
            let path = entry.path();
            match entry_kind(&entry)? {
                Kind::Folder => pending.push(path),
                Kind::File if has_extension(&path, "fcs") => found.push(path),
                _ => {}
            }
        }
    }
    found.sort();
    Ok(found)
}

#[derive(PartialEq, Eq)]
enum Kind {
    File,
    /// A real folder, to search. Never a link to one.
    Folder,
    Other,
}

/// What an entry is, following a link to a file but never a link to a folder.
///
/// Links to files are followed: data on shared drives is often linked into a
/// working folder rather than copied, and those files are exactly as wanted
/// as any other. Links to folders are not, for the reasons at [`fcs_under`].
/// A link that points nowhere is skipped rather than failing the whole search.
fn entry_kind(entry: &std::fs::DirEntry) -> std::io::Result<Kind> {
    let kind = entry.file_type()?;
    if kind.is_dir() {
        return Ok(Kind::Folder);
    }
    if kind.is_file() {
        return Ok(Kind::File);
    }
    if kind.is_symlink() {
        return Ok(match std::fs::metadata(entry.path()) {
            Ok(target) if target.is_file() => Kind::File,
            _ => Kind::Other,
        });
    }
    Ok(Kind::Other)
}

/// Dot-files and dot-folders are skipped.
///
/// Mostly for `._name.fcs`: the resource-fork files macOS leaves beside every
/// file it copies to a shared drive. They end in `.fcs`, they are not FCS
/// files, and a folder copied from a Mac has one for every real file.
fn hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn has_extension(path: &Path, wanted: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(wanted))
}

/// The name a file goes by inside the program.
///
/// Files are matched to their metadata by name, so two called `A1.fcs` in two
/// plate folders could not be told apart. A file in a sub-folder of the
/// workspace is therefore known by its path below the workspace, with the
/// folders joined by underscores: `Plate_10/A1.fcs` is `Plate_10_A1.fcs`.
///
/// The whole path rather than only the folder the file sits in, because the
/// immediate folder is not unique either - `Plate_9/WK1/A1.fcs` and
/// `Plate_10/WK1/A1.fcs` would both be `WK1_A1.fcs`.
///
/// Nothing on disk is renamed. This is only what the program calls the file,
/// and so what the metadata's file name column has to say for it.
///
/// A file at the top level, or one added from outside the workspace folder,
/// keeps its own name.
///
/// Whether a file is inside is decided on the paths with `.` and `..`
/// resolved, so `/w/../elsewhere/A1.fcs` is outside `/w` - it used to count as
/// inside and be named `.._elsewhere_A1.fcs` (B-WS-1) - and
/// `/w/Plate_1/../Plate_2/A1.fcs` is `Plate_2_A1.fcs`.
pub fn program_name(root: Option<&Path>, path: &Path) -> String {
    let path = &lexically_normal(path);
    let root = root.map(lexically_normal);
    if let Some(below) = root.and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf)) {
        let parts: Vec<String> = below
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        if !parts.is_empty() {
            return parts.join("_");
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// `path` with `.` dropped and each `..` taking the folder before it, as
/// written - the disk is not consulted, so a symbolic link is not followed.
/// A `..` with nothing before it to take is kept.
fn lexically_normal(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    // Nothing to climb out of; keep it. Above the root there
                    // is only the root, so there it is dropped.
                    out.push(part);
                }
            }
            other => out.push(other),
        }
    }
    out
}

// ── remembering the last one ─────────────────────────────────────────────

/// The files a workspace was last opened with, for opening it again.
///
/// The files themselves rather than the folder. Any part may have been
/// replaced with a file from elsewhere, and FCS files added or removed one at
/// a time, so re-reading the folder would open something other than what was
/// closed.
///
/// The rules are deliberately absent. They are exported to their own file and
/// imported on purpose; a new workspace starts without any.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remembered {
    /// The folder it was opened from, if it was - shown so a person knows
    /// which run they are looking at.
    #[serde(default)]
    pub folder: Option<PathBuf>,
    #[serde(default)]
    pub fcs: Vec<PathBuf>,
    #[serde(default)]
    pub metadata: Option<PathBuf>,
    #[serde(default)]
    pub scaling: Option<PathBuf>,
    #[serde(default)]
    pub gating: Option<PathBuf>,
}

impl Remembered {
    /// Where it is kept: this program's folder in the user's configuration
    /// directory, not beside the binary. A path beside the binary was what
    /// `file_paths.txt` did, and it meant a rebuild or a second copy of the
    /// app started with whatever that copy had last been pointed at.
    pub fn location() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("clingate").join("workspace.json"))
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Written beside and renamed over, so a crash mid-write leaves the
        // previous workspace rather than half a file that fails to parse.
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    /// The parts that no longer exist on disk, for saying so before opening.
    pub fn missing(&self) -> Vec<PathBuf> {
        self.fcs
            .iter()
            .chain(self.metadata.iter())
            .chain(self.scaling.iter())
            .chain(self.gating.iter())
            .filter(|path| !path.exists())
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.fcs.is_empty()
            && self.metadata.is_none()
            && self.scaling.is_none()
            && self.gating.is_none()
    }
}
