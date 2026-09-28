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
//!
//! ## Loading them
//!
//! The steps that need care - carrying the gates through a new scaling, and
//! what a gating file waits for - are here as plain functions, so the app and
//! anything driving clingate without a window load a workspace the same way.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What replacing the scaling did to the gates.
#[derive(Debug, Clone, PartialEq)]
pub struct ScalingCarried {
    /// Which channels changed, and which the new file does not have.
    pub diff: crate::axis_store::ScalingDiff,
    /// Gates that could not be carried, and why. They are left as they were.
    pub problems: Vec<String>,
}

/// Replace the scaling, carrying every gate through each channel's change.
///
/// Channel by channel, the edits a person could make by hand in the editor -
/// a new cofactor is a rescale, a new range a relimit - so whatever the gates
/// hold comes through.
///
/// Refused, with nothing changed, if any channel in `configs` could not be
/// drawn on (see [`crate::AxisInfo::problem`]): `read_axis_configs`
/// refuses such a file already, and this is the second line, before either is
/// touched.
pub fn carry_to_scaling(
    axes: &mut crate::axis_store::AxisStore,
    gates: &mut crate::gates::GateState,
    configs: Vec<crate::AxisInfo>,
) -> Result<ScalingCarried, String> {
    let unusable: Vec<String> = configs
        .iter()
        .filter_map(crate::AxisInfo::problem)
        .collect();
    if !unusable.is_empty() {
        return Err(format!(
            "the scaling cannot be used: {}",
            unusable.join("; ")
        ));
    }
    let diff = crate::axis_store::scaling_diff(&axes.settings, &configs);
    axes.replace_axis_configs(configs);
    let mut problems: Vec<String> = Vec::new();
    for change in &diff.changed {
        if change.transform_changed()
            && let Err(errors) = gates.rescale_channel(&change.channel, &change.old, &change.new)
        {
            problems.extend(errors);
        }
        if change.range_changed()
            && let Err(errors) = gates.relimit_channel(
                &change.channel,
                change.new.axis_lower,
                change.new.axis_upper,
                &change.new.transform,
            )
        {
            problems.extend(errors);
        }
    }
    Ok(ScalingCarried { diff, problems })
}

/// What a gating file has to wait for before it can be imported: the gates
/// are imported *through* the metadata, which keys the per-specimen positions,
/// and the scaling, which puts every coordinate where it is drawn. `None` when
/// both are loaded.
pub fn gating_needs(
    metadata: &crate::omiq::metadata::MetaDataFileMap,
    axes: &crate::omiq::serialise::AxisSettings,
) -> Option<&'static str> {
    match (metadata.is_empty(), axes.is_empty()) {
        (true, true) => Some("the metadata and the scaling"),
        (true, false) => Some("the metadata"),
        (false, true) => Some("the scaling"),
        (false, false) => None,
    }
}

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
        gating: saved_first(gating, SAVED_GATING),
        metadata: Found::from(metadata),
        scaling: saved_first(scaling, SAVED_SCALING),
    })
}

/// Among several candidates, the one this program saved, which is the one
/// worked on last: after a first save a folder holds both the Omiq export and
/// the saved copy, and without this opening it afresh would ask which.
fn saved_first(candidates: Vec<PathBuf>, saved: &str) -> Found {
    if candidates.len() > 1
        && let Some(ours) = candidates
            .iter()
            .find(|p| p.file_name().is_some_and(|n| n == saved))
    {
        return Found::One(ours.clone());
    }
    Found::from(candidates)
}

// ── saving the working copy ──────────────────────────────────────────────

/// The file Save writes the gates to, in the workspace folder. Omiq's format,
/// so it can go back to Omiq; never the file that was loaded, which is left
/// as it came.
pub const SAVED_GATING: &str = "clingate_gating.omiqgt";
/// The scaling saved with them. A gate's coordinates are in the units its
/// axes are drawn in, so gates saved after a cofactor changed are only right
/// read against the scaling they were saved with.
pub const SAVED_SCALING: &str = "clingate_scaling.csv";
/// The working copy as it stands, kept as it is edited so a crash or a close
/// without saving loses nothing. Hidden, so opening the folder afresh does not
/// mistake it for a part of the workspace.
pub const RECOVERY_GATING: &str = ".clingate_recovery.omiqgt";
pub const RECOVERY_SCALING: &str = ".clingate_recovery_scaling.csv";

/// A gating file and the scaling that goes with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatingFiles {
    pub gating: PathBuf,
    pub scaling: PathBuf,
}

impl GatingFiles {
    /// Where Save writes, in `folder`.
    pub fn saved(folder: &Path) -> Self {
        Self {
            gating: folder.join(SAVED_GATING),
            scaling: folder.join(SAVED_SCALING),
        }
    }

    /// Where the recovery copy is kept, in `folder`.
    pub fn recovery(folder: &Path) -> Self {
        Self {
            gating: folder.join(RECOVERY_GATING),
            scaling: folder.join(RECOVERY_SCALING),
        }
    }

    /// Whether both are there.
    pub fn exist(&self) -> bool {
        self.gating.is_file() && self.scaling.is_file()
    }

    /// When the gating file was last written, for saying how old a recovery
    /// copy is.
    pub fn modified(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.gating)
            .and_then(|m| m.modified())
            .ok()
    }

    /// Write `gates` and the scaling `axes` describes.
    ///
    /// The gating document is built before anything is written, so a gate
    /// that cannot be written leaves both files as they were. Each is written
    /// beside itself and renamed over, so a crash part-way leaves the old file
    /// rather than half a new one.
    pub fn write(
        &self,
        gates: &crate::gates::GateState,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axes: &crate::omiq::serialise::AxisSettings,
    ) -> anyhow::Result<()> {
        let document = crate::omiq::serialise::to_omiq_document(gates, metadata, axes)?;
        let text = serde_json::to_string_pretty(&document)?;
        crate::axis_store::write_axis_configs(axes.values(), &self.scaling)?;
        let temporary = self.gating.with_extension("omiqgt.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.gating)?;
        Ok(())
    }

    /// Remove both, where they are there.
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.gating);
        let _ = std::fs::remove_file(&self.scaling);
    }
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

/// The name of the workspace file kept in a workspace's folder.
pub const WORKSPACE_FILE: &str = "clingate_workspace.json";

/// The files a workspace was last opened with, and the choices made about
/// them, for opening it again.
///
/// The files themselves rather than the folder. Any part may have been
/// replaced with a file from elsewhere, and FCS files added or removed one at
/// a time, so re-reading the folder would open something other than what was
/// closed.
///
/// Kept in two places. In the workspace's own folder, as
/// [`WORKSPACE_FILE`], with the paths inside the folder written relative to
/// it - so the folder can be moved, copied to another machine, or opened by
/// the tools for Claude, and still open as it was left. And in this
/// program's configuration folder, with every path in full, so the last
/// workspace can be offered at launch.
///
/// The rules are deliberately absent. They are exported to their own file and
/// imported on purpose; a new workspace starts without any. Which metadata
/// columns group and order the samples is not a rule, and is kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// The compensation groups, their sources and which file is in which.
    /// `None` for a workspace remembered before there were any: its files
    /// are grouped afresh.
    #[serde(default)]
    pub compensation: Option<crate::compensation::groups::Saved>,
    /// The metadata columns that say which files are one specimen and what
    /// each file is, the order a specimen's files are shown in, and the
    /// column samples are sorted by. `None` for a workspace remembered before
    /// they were kept: it opens with the defaults.
    #[serde(default)]
    pub pairing: Option<crate::gate_rules::rule_store::SamplePairing>,
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

    /// Where a folder's own workspace file is.
    pub fn in_folder(folder: &Path) -> PathBuf {
        folder.join(WORKSPACE_FILE)
    }

    /// The workspace saved in `folder`, if there is one, with its paths put
    /// back in full against the folder as it is now - wherever it has moved.
    pub fn load_from_folder(folder: &Path) -> anyhow::Result<Option<Self>> {
        let at = Self::in_folder(folder);
        if !at.is_file() {
            return Ok(None);
        }
        let mut loaded = Self::load_from(&at)?;
        loaded.map_paths(|path| from_folder(folder, path));
        loaded.folder = Some(folder.to_path_buf());
        Ok(Some(loaded))
    }

    /// Save into the workspace's own folder, with the paths inside it written
    /// relative to it. Nothing is written for a workspace with no folder.
    pub fn save_into_folder(&self) -> anyhow::Result<Option<PathBuf>> {
        let Some(folder) = self.folder.clone() else {
            return Ok(None);
        };
        let mut portable = self.clone();
        portable.map_paths(|path| in_folder(&folder, path));
        portable.folder = None;
        let at = Self::in_folder(&folder);
        portable.save_to(&at)?;
        Ok(Some(at))
    }

    /// Every path this names: the parts, the FCS files, and the files and
    /// matrices the compensation groups hold.
    fn map_paths(&mut self, f: impl Fn(&Path) -> PathBuf) {
        use crate::compensation::groups::{SavedApplied, SavedSource};
        for path in self
            .fcs
            .iter_mut()
            .chain(self.metadata.iter_mut())
            .chain(self.scaling.iter_mut())
            .chain(self.gating.iter_mut())
        {
            *path = f(path);
        }
        if let Some(compensation) = &mut self.compensation {
            for (path, _) in &mut compensation.files {
                *path = f(path);
            }
            for group in &mut compensation.groups {
                if let SavedSource::Csv(path) = &mut group.source {
                    *path = f(path);
                }
                if let SavedApplied::Csv(path) = &mut group.applied {
                    *path = f(path);
                }
            }
        }
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
        use crate::compensation::groups::SavedSource;
        let matrices = self.compensation.iter().flat_map(|c| {
            c.groups.iter().filter_map(|g| match &g.source {
                SavedSource::Csv(path) => Some(path),
                _ => None,
            })
        });
        self.fcs
            .iter()
            .chain(self.metadata.iter())
            .chain(self.scaling.iter())
            .chain(self.gating.iter())
            .chain(matrices)
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

/// A path inside `folder` as the steps down from it, joined with `/` whatever
/// the platform, so the file reads the same on macOS and Windows. A path
/// outside the folder is kept in full.
fn in_folder(folder: &Path, path: &Path) -> PathBuf {
    match path.strip_prefix(folder) {
        Ok(inside) => PathBuf::from(
            inside
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        ),
        Err(_) => path.to_path_buf(),
    }
}

/// The inverse of [`in_folder`]: a relative path is put back under `folder`,
/// one step at a time so it carries this platform's separators.
fn from_folder(folder: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let mut full = folder.to_path_buf();
    for step in path.to_string_lossy().split('/') {
        if !step.is_empty() {
            full.push(step);
        }
    }
    full
}
