//! Compensation groups: which matrix each file is compensated with.
//!
//! As in Omiq's compensation task, files are grouped by the matrix they carry:
//! every file whose own `$SPILLOVER` is the same matrix joins one group, and
//! files with none (or an identity matrix, which changes nothing) share
//! another. Each group then says what its files are compensated with - their
//! own matrix, a matrix loaded from a CSV, or nothing - and a file can be
//! moved to another group, or to a new one, to be compensated differently
//! from the files it came in with.
//!
//! A file is never left uncompensated by accident: a group using the files'
//! own matrices, holding a file that has none, is an error for that file, as
//! is a group whose CSV could not be read. Only a group set to no
//! compensation compensates nothing.
//!
//! ## Files Omiq has already compensated
//!
//! Omiq applies its compensation to the events when it exports a file and
//! records nothing of it (see [`crate::compensation`]). So each group has two
//! matrices, both measured against the events as the cytometer recorded them:
//!
//! - what is **wanted** - its [`Source`], the matrix a person sees, edits and
//!   takes back to Omiq; and
//! - what is **already applied** to the Omiq exports among its files - its
//!   [`Applied`], which only a person can say, since the file does not.
//!
//! A file is read as its events with what was applied taken back out, then
//! what is wanted put in: `events · A · T⁻¹`. When the two are the same that
//! is nothing at all - Omiq's export shown as Omiq showed it - and an edit
//! applies only the difference. Files the cytometer wrote have nothing
//! applied. Until a group holding Omiq exports says what was applied to
//! them, they are shown as exported and cannot be given a matrix: nothing
//! could say what it would be relative to.

use super::Spillover;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type GroupId = u32;

/// What [`Compensation::check`] needs to know of a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFacts {
    /// Its channels, as (`$PnN`, `$PnS`).
    pub channels: Vec<(String, Option<String>)>,
}

/// A file as the workspace found it: its own matrix, and whether Omiq wrote
/// it - see [`crate::compensation::own_matrices`].
#[derive(Clone, Debug, PartialEq)]
pub struct FileMatrix {
    pub path: PathBuf,
    /// `Ok(None)` for no matrix, `Err` for one that could not be read.
    pub own: Result<Option<Spillover>, String>,
    pub written_by_omiq: bool,
}

impl FileMatrix {
    /// A file the cytometer wrote.
    pub fn new(path: PathBuf, own: Result<Option<Spillover>, String>) -> Self {
        Self {
            path,
            own,
            written_by_omiq: false,
        }
    }

    /// A file Omiq exported.
    pub fn from_omiq(path: PathBuf) -> Self {
        Self {
            path,
            own: Ok(None),
            written_by_omiq: true,
        }
    }
}

/// What reading a file does to its events: take out what was already
/// applied, then put in what is wanted - see the module documentation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Correction {
    /// The matrix already applied to the events, to take back out.
    pub undo: Option<Arc<Spillover>>,
    /// The matrix wanted, to apply.
    pub apply: Option<Arc<Spillover>>,
}

impl Correction {
    /// Nothing to do: the events as they are.
    pub fn none() -> Self {
        Self::default()
    }

    /// Apply `matrix` to events nothing has been applied to.
    pub fn apply(matrix: Arc<Spillover>) -> Self {
        Self {
            undo: None,
            apply: Some(matrix),
        }
    }

    /// Take out `undo` and put in `apply`. Nothing at all when they are the
    /// same matrix, or when both change nothing.
    pub fn between(undo: Option<Arc<Spillover>>, apply: Option<Arc<Spillover>>) -> Self {
        let undo = undo.filter(|m| !m.is_identity());
        let apply = apply.filter(|m| !m.is_identity());
        match (&undo, &apply) {
            (Some(a), Some(t)) if a.same_as(t) => Self::none(),
            _ => Self { undo, apply },
        }
    }

    pub fn is_none(&self) -> bool {
        self.undo.is_none() && self.apply.is_none()
    }
}

/// What was already applied to the Omiq exports in a group.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    /// Not said yet. The files are shown as exported, and the group cannot
    /// be given a matrix until it is.
    Unknown,
    /// Nothing: exported from Omiq with no compensation task, or with one
    /// left at 0% throughout - which export identically.
    Nothing,
    /// This matrix, the one they were compensated with in Omiq.
    Matrix {
        path: Option<PathBuf>,
        matrix: Arc<Spillover>,
    },
    /// A CSV said to be the matrix applied, that could not be read.
    Unreadable { path: PathBuf, why: String },
}

/// How far, in percentage points, a loaded matrix may be from a file's own
/// before [`Compensation::check`] mentions it.
pub const FAR_FROM_OWN: f64 = 10.0;

/// What a group's files are compensated with.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Nothing: the events as they are in the file.
    None,
    /// Each file's own `$SPILLOVER`.
    FilesOwn,
    /// A matrix loaded from a CSV.
    Loaded {
        path: PathBuf,
        matrix: Arc<Spillover>,
    },
    /// A CSV that was chosen and could not be read. Its files are not drawn
    /// until it is replaced or the source changed: drawn uncompensated they
    /// would look like a result.
    Unreadable { path: PathBuf, why: String },
    /// A matrix edited here. `from` names what it was edited from.
    Edited {
        matrix: Arc<Spillover>,
        from: Option<String>,
    },
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::None => "no compensation".to_string(),
            Source::FilesOwn => "each file's own matrix".to_string(),
            Source::Loaded { path, .. } | Source::Unreadable { path, .. } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Source::Edited {
                from: Some(from), ..
            } => format!("{from}, edited here"),
            Source::Edited { from: None, .. } => "a matrix made here".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub source: Source,
    /// What was already applied to the Omiq exports among its files.
    pub applied: Applied,
    /// The file matrix this group was formed around, if any: a file added
    /// later with the same matrix joins it. `None` for the group of files
    /// without one, and for groups made by hand.
    formed_around: Option<Arc<Spillover>>,
    /// Whether this is the group files without a matrix of their own join.
    for_files_without: bool,
    /// Whether this is the group Omiq exports join. Kept apart from files
    /// the cytometer wrote without a matrix: those have nothing applied,
    /// and Omiq exports have to say what they have.
    for_omiq_exports: bool,
}

/// What one file carries.
#[derive(Clone, Debug, PartialEq)]
struct Member {
    group: GroupId,
    /// Its own matrix: `Ok(None)` for none (or an identity), `Err` for one
    /// that could not be read.
    own: Result<Option<Arc<Spillover>>, String>,
    /// Whether Omiq wrote it, so that the group's [`Applied`] is what was
    /// done to its events.
    omiq: bool,
}

/// Every group, and which group each file is in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compensation {
    groups: Vec<Group>,
    files: BTreeMap<PathBuf, Member>,
    next_id: GroupId,
}

impl Compensation {
    /// Bring the groups in step with the workspace's files: each file not
    /// here yet is added as [`Compensation::add_file`] does, and each file no
    /// longer in the workspace taken out. Files already here stay where they
    /// are.
    pub fn sync(&mut self, files: Vec<FileMatrix>) {
        let present: std::collections::BTreeSet<PathBuf> =
            files.iter().map(|f| f.path.clone()).collect();
        let gone: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| !present.contains(*p))
            .cloned()
            .collect();
        for path in gone {
            self.remove_file(&path);
        }
        for file in files {
            self.add(file);
        }
    }

    /// What is wrong, or worth knowing, about how a group's files will be
    /// compensated - for showing beside the group. `facts_of` gives what is
    /// needed of each file.
    ///
    /// - A file that cannot be compensated as the group says, and why.
    /// - A loaded matrix that does not fit a file's channels.
    /// - A loaded matrix far from a file's own, or that fits it better read
    ///   the other way round - usually the wrong matrix, or rows and columns
    ///   swapped.
    pub fn check(&self, group: GroupId, facts_of: impl Fn(&Path) -> FileFacts) -> Vec<String> {
        let mut notes = Vec::new();
        let Some(g) = self.group(group) else {
            return notes;
        };
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        };
        let mut far = 0usize;
        let mut swapped = 0usize;
        for path in self.files_in(group) {
            let correction = match self.matrix_for(path) {
                Ok(c) => c,
                Err(why) => {
                    notes.push(format!("{}: {why}", name(path)));
                    continue;
                }
            };
            let facts = facts_of(path);
            let lookup: Vec<(&str, Option<&str>)> = facts
                .channels
                .iter()
                .map(|(n, l)| (n.as_str(), l.as_deref()))
                .collect();
            // As compensating matches it: only the channels that take part.
            let unfit = [&correction.undo, &correction.apply]
                .into_iter()
                .flatten()
                .filter_map(|m| m.involved())
                .find_map(|m| m.resolve(&lookup).err());
            if let Some(why) = unfit {
                notes.push(format!("{}: the matrix does not fit it: {why}", name(path)));
                continue;
            }
            let Some(matrix) = correction.apply else {
                continue;
            };
            if let (Source::Loaded { .. }, Some(Ok(Some(own)))) = (&g.source, self.own_matrix(path))
                && let Some(c) = own.compare(&matrix)
            {
                if c.transposed_fits_better {
                    swapped += 1;
                } else if c.largest_difference > FAR_FROM_OWN {
                    far += 1;
                }
            }
        }
        if swapped > 0 {
            notes.push(format!(
                "The loaded matrix is closer to {swapped} of these files' own matrices read the other way round. \
                 A spillover matrix has each fluorochrome in a row and each detector in a column; check how it was exported."
            ));
        }
        if far > 0 {
            notes.push(format!(
                "The loaded matrix differs from {far} of these files' own matrices by more than {FAR_FROM_OWN} percentage points somewhere. \
                 That is expected after recomputing compensation, but check it is the matrix meant for these files."
            ));
        }
        notes
    }

    /// Add a file the cytometer wrote - see [`Compensation::add`].
    pub fn add_file(&mut self, path: PathBuf, own: Result<Option<Spillover>, String>) {
        self.add(FileMatrix::new(path, own));
    }

    /// Add a file, into the group formed around the same matrix as its own,
    /// or a new group if there is none. A file already here keeps its group.
    pub fn add(&mut self, file: FileMatrix) {
        let FileMatrix {
            path,
            own,
            written_by_omiq,
        } = file;
        if self.files.contains_key(&path) {
            return;
        }
        let own = own.map(|m| m.filter(|m| !m.is_identity()).map(Arc::new));
        let group = match &own {
            Ok(Some(matrix)) => self
                .groups
                .iter()
                .find(|g| g.formed_around.as_ref().is_some_and(|f| f.same_as(matrix)))
                .map(|g| g.id)
                .unwrap_or_else(|| {
                    let name = format!("{} channels, from the files", matrix.channels().len());
                    self.push_group(name, Source::FilesOwn, Some(matrix.clone()), false)
                }),
            Ok(None) if written_by_omiq => self
                .groups
                .iter()
                .find(|g| g.for_omiq_exports)
                .map(|g| g.id)
                .unwrap_or_else(|| {
                    let id =
                        self.push_group("Exported from Omiq".into(), Source::None, None, false);
                    self.group_mut(id).expect("just made").for_omiq_exports = true;
                    id
                }),
            Ok(None) => self
                .groups
                .iter()
                .find(|g| g.for_files_without)
                .map(|g| g.id)
                .unwrap_or_else(|| {
                    self.push_group("No matrix in the files".into(), Source::None, None, true)
                }),
            // One group per unreadable file: nothing says two of them are
            // alike, and each needs a matrix chosen for it.
            Err(_) => self.push_group("Matrix unreadable".into(), Source::FilesOwn, None, false),
        };
        self.files.insert(
            path,
            Member {
                group,
                own,
                omiq: written_by_omiq,
            },
        );
    }

    fn push_group(
        &mut self,
        name: String,
        source: Source,
        formed_around: Option<Arc<Spillover>>,
        for_files_without: bool,
    ) -> GroupId {
        let id = self.next_id;
        self.next_id += 1;
        let name = format!("Group {} - {name}", self.groups.len() + 1);
        self.groups.push(Group {
            id,
            name,
            source,
            applied: Applied::Unknown,
            formed_around,
            for_files_without,
            for_omiq_exports: false,
        });
        id
    }

    /// Take a file out. A group left empty that was formed around files'
    /// matrices goes with it; one given a CSV, or made by hand, stays.
    pub fn remove_file(&mut self, path: &Path) {
        let Some(member) = self.files.remove(path) else {
            return;
        };
        let empty = !self.files.values().any(|m| m.group == member.group);
        if empty {
            self.groups.retain(|g| {
                g.id != member.group
                    || !(matches!(g.source, Source::FilesOwn | Source::None)
                        && (g.formed_around.is_some() || g.for_files_without || g.for_omiq_exports))
            });
        }
    }

    /// A new, empty group, compensating with nothing until given a source.
    pub fn new_group(&mut self, name: impl Into<String>) -> GroupId {
        let id = self.next_id;
        self.next_id += 1;
        self.groups.push(Group {
            id,
            name: name.into(),
            source: Source::None,
            applied: Applied::Unknown,
            formed_around: None,
            for_files_without: false,
            for_omiq_exports: false,
        });
        id
    }

    pub fn move_file(&mut self, path: &Path, to: GroupId) -> Result<(), String> {
        if !self.groups.iter().any(|g| g.id == to) {
            return Err(format!("there is no group {to}"));
        }
        let member = self
            .files
            .get_mut(path)
            .ok_or_else(|| format!("{} is not in the workspace", path.display()))?;
        member.group = to;
        Ok(())
    }

    /// What the group's files are to be compensated with.
    ///
    /// Refused for a group holding Omiq exports until it has said what was
    /// already applied to them - see [`Compensation::set_applied`]. Setting
    /// it back to no compensation is always allowed.
    pub fn set_source(&mut self, group: GroupId, source: Source) -> Result<(), String> {
        if source != Source::None && self.unanswered(group) {
            return Err(Self::ASK.to_string());
        }
        self.group_mut(group)?.source = source;
        Ok(())
    }

    /// Say what was already applied to the group's Omiq exports.
    ///
    /// Answering with a matrix also makes it what is wanted, unless something
    /// else already is: files compensated in Omiq with a matrix are wanted
    /// compensated with that matrix until someone edits it, and that is no
    /// change at all to their events.
    pub fn set_applied(&mut self, group: GroupId, applied: Applied) -> Result<(), String> {
        let g = self.group_mut(group)?;
        if let Applied::Matrix { path, matrix } = &applied
            && matches!(g.source, Source::None)
        {
            g.source = match path {
                Some(path) => Source::Loaded {
                    path: path.clone(),
                    matrix: matrix.clone(),
                },
                None => Source::Edited {
                    matrix: matrix.clone(),
                    from: None,
                },
            };
        }
        g.applied = applied;
        Ok(())
    }

    /// What to say to a group holding Omiq exports that has not said what was
    /// applied to them.
    pub const ASK: &'static str = "these files were exported from Omiq, which applies its compensation \
        to the events and records nothing of it in the file: say whether they were compensated in Omiq \
        before giving them a matrix here";

    /// Whether the group holds a file Omiq wrote.
    pub fn holds_omiq_exports(&self, group: GroupId) -> bool {
        self.files.values().any(|m| m.group == group && m.omiq)
    }

    /// Whether the group holds Omiq exports and has not said what was
    /// already applied to them.
    pub fn unanswered(&self, group: GroupId) -> bool {
        self.holds_omiq_exports(group)
            && self
                .group(group)
                .is_some_and(|g| matches!(g.applied, Applied::Unknown))
    }

    /// The matrix the group wants, where it is one matrix for all its files:
    /// a loaded or edited one, or the matrix its files share when each file
    /// uses its own. What an edit starts from, and what is exported.
    pub fn wanted(&self, group: GroupId) -> Option<Arc<Spillover>> {
        let g = self.group(group)?;
        match &g.source {
            Source::Loaded { matrix, .. } | Source::Edited { matrix, .. } => Some(matrix.clone()),
            Source::FilesOwn => g.formed_around.clone().or_else(|| {
                self.files
                    .values()
                    .filter(|m| m.group == group)
                    .find_map(|m| m.own.as_ref().ok().cloned().flatten())
            }),
            Source::None | Source::Unreadable { .. } => None,
        }
    }

    /// The matrix already applied to the group's Omiq exports, if one was.
    pub fn applied(&self, group: GroupId) -> Option<Arc<Spillover>> {
        match &self.group(group)?.applied {
            Applied::Matrix { matrix, .. } => Some(matrix.clone()),
            _ => None,
        }
    }

    /// Set one entry of the matrix the group wants: the percentage of
    /// `from`'s signal that shows up in `into`. The group's matrix becomes
    /// one edited here, starting from what it wanted - see
    /// [`Compensation::wanted`] - or, with nothing wanted, from what was
    /// applied, or from no compensation over `channels`.
    pub fn set_value(
        &mut self,
        group: GroupId,
        from: &str,
        into: &str,
        percent: f64,
        channels: &[Arc<str>],
    ) -> Result<(), String> {
        if self.unanswered(group) {
            return Err(Self::ASK.to_string());
        }
        if from == into {
            return Err("a channel's spillover into itself is 100%".to_string());
        }
        if !percent.is_finite() {
            return Err(format!("{percent} is not a percentage"));
        }
        let base = self
            .wanted(group)
            .or_else(|| self.applied(group))
            .map(|m| (*m).clone())
            .map_or_else(|| Spillover::identity(channels), Ok)
            .map_err(|e| e.to_string())?;
        let edited = base
            .with_value(from, into, percent / 100.0)
            .map_err(|e| e.to_string())?;
        let from_name = match &self.group(group).map(|g| &g.source) {
            Some(Source::Edited { from, .. }) => from.clone(),
            Some(source @ (Source::Loaded { .. } | Source::FilesOwn)) => Some(source.describe()),
            _ => None,
        };
        self.group_mut(group)?.source = Source::Edited {
            matrix: Arc::new(edited),
            from: from_name,
        };
        Ok(())
    }

    fn group_mut(&mut self, group: GroupId) -> Result<&mut Group, String> {
        self.groups
            .iter_mut()
            .find(|g| g.id == group)
            .ok_or_else(|| format!("there is no group {group}"))
    }

    pub fn rename(&mut self, group: GroupId, name: impl Into<String>) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group) {
            g.name = name.into();
        }
    }

    /// Remove a group, if no file is in it.
    pub fn remove_group(&mut self, group: GroupId) -> Result<(), String> {
        let held = self.files.values().filter(|m| m.group == group).count();
        if held > 0 {
            return Err(format!("it still holds {held} files"));
        }
        self.groups.retain(|g| g.id != group);
        Ok(())
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_of(&self, path: &Path) -> Option<GroupId> {
        self.files.get(path).map(|m| m.group)
    }

    pub fn files_in(&self, group: GroupId) -> impl Iterator<Item = &Path> + '_ {
        self.files
            .iter()
            .filter(move |(_, m)| m.group == group)
            .map(|(p, _)| p.as_path())
    }

    /// The file's own matrix, as read.
    pub fn own_matrix(&self, path: &Path) -> Option<&Result<Option<Arc<Spillover>>, String>> {
        self.files.get(path).map(|m| &m.own)
    }

    /// What reading `path` does to its events - see [`Correction`] - or why
    /// it cannot be compensated as its group says.
    ///
    /// A file the workspace does not know is not compensated: it is not one
    /// of the files, so nothing has been decided about it.
    pub fn matrix_for(&self, path: &Path) -> Result<Correction, String> {
        let Some(member) = self.files.get(path) else {
            return Ok(Correction::none());
        };
        let group = self
            .group(member.group)
            .ok_or_else(|| "its compensation group is gone".to_string())?;
        let wanted = match &group.source {
            Source::None => None,
            Source::Loaded { matrix, .. } | Source::Edited { matrix, .. } => Some(matrix.clone()),
            Source::Unreadable { path, why } => {
                return Err(format!(
                    "its group's matrix {} could not be read: {why}",
                    path.display()
                ));
            }
            Source::FilesOwn => match &member.own {
                Ok(Some(own)) => Some(own.clone()),
                Ok(None) => {
                    return Err(format!(
                        "{} compensates with each file's own matrix, and this file has none",
                        group.name
                    ));
                }
                Err(why) => return Err(format!("its own matrix could not be read: {why}")),
            },
        };
        let applied = if member.omiq {
            match &group.applied {
                Applied::Nothing => None,
                Applied::Matrix { matrix, .. } => Some(matrix.clone()),
                // Shown as exported, which is right with nothing wanted.
                Applied::Unknown if wanted.is_none() => None,
                Applied::Unknown => return Err(Self::ASK.to_string()),
                Applied::Unreadable { path, why } => {
                    return Err(format!(
                        "the matrix it was compensated with in Omiq, {}, could not be read: {why}",
                        path.display()
                    ));
                }
            }
        } else {
            None
        };
        Ok(Correction::between(applied, wanted))
    }

    /// A number that changes whenever what `path` is compensated with does:
    /// for telling a plot drawn before a change from one drawn after.
    pub fn digest(&self, path: &Path) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self.matrix_for(path) {
            Ok(c) => {
                for (tag, m) in [(1u8, &c.undo), (2u8, &c.apply)] {
                    let Some(m) = m else {
                        continue;
                    };
                    tag.hash(&mut h);
                    for c in m.channels() {
                        c.hash(&mut h);
                    }
                    for i in 0..m.channels().len() {
                        for j in 0..m.channels().len() {
                            m.value(i, j).to_bits().hash(&mut h);
                        }
                    }
                }
            }
            Err(why) => {
                2u8.hash(&mut h);
                why.hash(&mut h);
            }
        }
        h.finish()
    }

    /// What to remember of this: the groups, their sources, and which file
    /// is in which. The matrices themselves are read again from the files
    /// and the CSVs.
    pub fn saved(&self) -> Saved {
        let index = |id: GroupId| self.groups.iter().position(|g| g.id == id);
        Saved {
            groups: self
                .groups
                .iter()
                .map(|g| SavedGroup {
                    name: g.name.clone(),
                    source: match &g.source {
                        Source::None => SavedSource::None,
                        Source::FilesOwn => SavedSource::FilesOwn,
                        Source::Loaded { path, .. } | Source::Unreadable { path, .. } => {
                            SavedSource::Csv(path.clone())
                        }
                        Source::Edited { matrix, from } => SavedSource::Edited {
                            matrix: SavedMatrix::of(matrix),
                            from: from.clone(),
                        },
                    },
                    applied: match &g.applied {
                        Applied::Unknown => SavedApplied::Unknown,
                        Applied::Nothing => SavedApplied::Nothing,
                        Applied::Matrix {
                            path: Some(path), ..
                        }
                        | Applied::Unreadable { path, .. } => SavedApplied::Csv(path.clone()),
                        Applied::Matrix { path: None, matrix } => {
                            SavedApplied::Matrix(SavedMatrix::of(matrix))
                        }
                    },
                })
                .collect(),
            files: self
                .files
                .iter()
                .filter_map(|(p, m)| index(m.group).map(|i| (p.clone(), i)))
                .collect(),
        }
    }

    /// Put back what [`Compensation::saved`] gave, for `files` and their own
    /// matrices. `read_csv` reads a group's CSV; one that fails leaves its
    /// group [`Source::Unreadable`]. A file the saved groups do not mention
    /// is grouped as a new one would be.
    pub fn restore(
        saved: &Saved,
        files: Vec<FileMatrix>,
        read_csv: impl Fn(&Path) -> Result<Spillover, String>,
    ) -> Self {
        let mut this = Self::default();
        let ids: Vec<GroupId> = saved
            .groups
            .iter()
            .map(|g| {
                let id = this.new_group(g.name.clone());
                let source = match &g.source {
                    SavedSource::None => Source::None,
                    SavedSource::FilesOwn => Source::FilesOwn,
                    SavedSource::Csv(path) => match read_csv(path) {
                        Ok(matrix) => Source::Loaded {
                            path: path.clone(),
                            matrix: Arc::new(matrix),
                        },
                        Err(why) => Source::Unreadable {
                            path: path.clone(),
                            why,
                        },
                    },
                    SavedSource::Edited { matrix, from } => match matrix.read() {
                        Ok(matrix) => Source::Edited {
                            matrix: Arc::new(matrix),
                            from: from.clone(),
                        },
                        Err(why) => Source::Unreadable {
                            path: PathBuf::from("(the matrix edited here)"),
                            why,
                        },
                    },
                };
                let applied = match &g.applied {
                    SavedApplied::Unknown => Applied::Unknown,
                    SavedApplied::Nothing => Applied::Nothing,
                    SavedApplied::Csv(path) => match read_csv(path) {
                        Ok(matrix) => Applied::Matrix {
                            path: Some(path.clone()),
                            matrix: Arc::new(matrix),
                        },
                        Err(why) => Applied::Unreadable {
                            path: path.clone(),
                            why,
                        },
                    },
                    SavedApplied::Matrix(matrix) => match matrix.read() {
                        Ok(matrix) => Applied::Matrix {
                            path: None,
                            matrix: Arc::new(matrix),
                        },
                        Err(why) => Applied::Unreadable {
                            path: PathBuf::from("(the matrix applied in Omiq)"),
                            why,
                        },
                    },
                };
                let group = this.group_mut(id).expect("just made");
                group.source = source;
                group.applied = applied;
                id
            })
            .collect();
        let placed: BTreeMap<&Path, usize> =
            saved.files.iter().map(|(p, i)| (p.as_path(), *i)).collect();
        let mut unplaced = Vec::new();
        for file in files {
            match placed.get(file.path.as_path()).and_then(|&i| ids.get(i)) {
                Some(&group) => {
                    let own = file
                        .own
                        .map(|m| m.filter(|m| !m.is_identity()).map(Arc::new));
                    this.files.insert(
                        file.path,
                        Member {
                            group,
                            own,
                            omiq: file.written_by_omiq,
                        },
                    );
                }
                None => unplaced.push(file),
            }
        }
        // Groups keep joining files with their matrix: each is formed around
        // the matrix its files carry, if they all carry the same one.
        for g in &mut this.groups {
            let members: Vec<&Member> = this.files.values().filter(|m| m.group == g.id).collect();
            if !members.is_empty() && members.iter().all(|m| m.omiq && matches!(m.own, Ok(None))) {
                g.for_omiq_exports = true;
                continue;
            }
            let owns: Vec<&Result<Option<Arc<Spillover>>, String>> =
                members.iter().map(|m| &m.own).collect();
            match owns.first() {
                Some(Ok(Some(first)))
                    if owns
                        .iter()
                        .all(|o| matches!(o, Ok(Some(m)) if m.same_as(first))) =>
                {
                    g.formed_around = Some(first.clone());
                }
                Some(Ok(None)) if owns.iter().all(|o| matches!(o, Ok(None))) => {
                    g.for_files_without = true;
                }
                _ => {}
            }
        }
        for file in unplaced {
            this.add(file);
        }
        this
    }
}

/// See [`Compensation::saved`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub groups: Vec<SavedGroup>,
    /// Each file, and the index of its group in `groups`.
    pub files: Vec<(PathBuf, usize)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedGroup {
    pub name: String,
    pub source: SavedSource,
    /// Absent in a workspace remembered before it was asked.
    #[serde(default)]
    pub applied: SavedApplied,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SavedSource {
    None,
    FilesOwn,
    Csv(PathBuf),
    /// A matrix edited here, kept with the workspace rather than in a file.
    Edited {
        matrix: SavedMatrix,
        from: Option<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum SavedApplied {
    #[default]
    Unknown,
    Nothing,
    Csv(PathBuf),
    Matrix(SavedMatrix),
}

/// A matrix as it is remembered: its channels, and its values row by row in
/// percent, as Omiq shows them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedMatrix {
    pub channels: Vec<String>,
    pub percent: Vec<f64>,
}

impl SavedMatrix {
    fn of(matrix: &Spillover) -> Self {
        let n = matrix.channels().len();
        Self {
            channels: matrix.channels().iter().map(|c| c.to_string()).collect(),
            percent: (0..n * n)
                .map(|k| matrix.value(k / n, k % n) * 100.0)
                .collect(),
        }
    }

    fn read(&self) -> Result<Spillover, String> {
        Spillover::new(
            self.channels
                .iter()
                .map(|c| Arc::from(c.as_str()))
                .collect(),
            self.percent.iter().map(|v| v / 100.0).collect(),
        )
        .map_err(|e| e.to_string())
    }
}
