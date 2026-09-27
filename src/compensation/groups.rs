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

use super::Spillover;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type GroupId = u32;

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
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub source: Source,
    /// The file matrix this group was formed around, if any: a file added
    /// later with the same matrix joins it. `None` for the group of files
    /// without one, and for groups made by hand.
    formed_around: Option<Arc<Spillover>>,
    /// Whether this is the group files without a matrix of their own join.
    for_files_without: bool,
}

/// What one file carries.
#[derive(Clone, Debug, PartialEq)]
struct Member {
    group: GroupId,
    /// Its own matrix: `Ok(None)` for none (or an identity), `Err` for one
    /// that could not be read.
    own: Result<Option<Arc<Spillover>>, String>,
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
    pub fn sync(&mut self, files: Vec<(PathBuf, Result<Option<Spillover>, String>)>) {
        let present: std::collections::BTreeSet<PathBuf> =
            files.iter().map(|(p, _)| p.clone()).collect();
        let gone: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| !present.contains(*p))
            .cloned()
            .collect();
        for path in gone {
            self.remove_file(&path);
        }
        for (path, own) in files {
            self.add_file(path, own);
        }
    }

    /// What is wrong, or worth knowing, about how a group's files will be
    /// compensated - for showing beside the group. `channels_of` gives a
    /// file's channels, as (`$PnN`, `$PnS`).
    ///
    /// - A file that cannot be compensated as the group says, and why.
    /// - A loaded matrix that does not fit a file's channels.
    /// - A loaded matrix far from a file's own, or that fits it better read
    ///   the other way round - usually the wrong matrix, or rows and columns
    ///   swapped.
    pub fn check(
        &self,
        group: GroupId,
        channels_of: impl Fn(&Path) -> Vec<(String, Option<String>)>,
    ) -> Vec<String> {
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
            let matrix = match self.matrix_for(path) {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(why) => {
                    notes.push(format!("{}: {why}", name(path)));
                    continue;
                }
            };
            let channels = channels_of(path);
            let lookup: Vec<(&str, Option<&str>)> = channels
                .iter()
                .map(|(n, l)| (n.as_str(), l.as_deref()))
                .collect();
            if let Err(why) = matrix.resolve(&lookup) {
                notes.push(format!("{}: the matrix does not fit it: {why}", name(path)));
                continue;
            }
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

    /// Add a file, into the group formed around the same matrix as its own,
    /// or a new group if there is none. A file already here keeps its group.
    pub fn add_file(&mut self, path: PathBuf, own: Result<Option<Spillover>, String>) {
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
        self.files.insert(path, Member { group, own });
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
            formed_around,
            for_files_without,
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
                        && (g.formed_around.is_some() || g.for_files_without))
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
            formed_around: None,
            for_files_without: false,
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

    pub fn set_source(&mut self, group: GroupId, source: Source) -> Result<(), String> {
        let group = self
            .groups
            .iter_mut()
            .find(|g| g.id == group)
            .ok_or_else(|| format!("there is no group {group}"))?;
        group.source = source;
        Ok(())
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

    /// What `path` is compensated with: `Ok(None)` for nothing, an error for
    /// a file that cannot be compensated as its group says.
    ///
    /// A file the workspace does not know is not compensated: it is not one
    /// of the files, so nothing has been decided about it.
    pub fn matrix_for(&self, path: &Path) -> Result<Option<Arc<Spillover>>, String> {
        let Some(member) = self.files.get(path) else {
            return Ok(None);
        };
        let group = self
            .group(member.group)
            .ok_or_else(|| "its compensation group is gone".to_string())?;
        match &group.source {
            Source::None => Ok(None),
            Source::Loaded { matrix, .. } => Ok(Some(matrix.clone())),
            Source::Unreadable { path, why } => Err(format!(
                "its group's matrix {} could not be read: {why}",
                path.display()
            )),
            Source::FilesOwn => match &member.own {
                Ok(Some(own)) => Ok(Some(own.clone())),
                Ok(None) => Err(format!(
                    "{} compensates with each file's own matrix, and this file has none",
                    group.name
                )),
                Err(why) => Err(format!("its own matrix could not be read: {why}")),
            },
        }
    }

    /// A number that changes whenever what `path` is compensated with does:
    /// for telling a plot drawn before a change from one drawn after.
    pub fn digest(&self, path: &Path) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self.matrix_for(path) {
            Ok(None) => 0u8.hash(&mut h),
            Ok(Some(m)) => {
                1u8.hash(&mut h);
                for c in m.channels() {
                    c.hash(&mut h);
                }
                for i in 0..m.channels().len() {
                    for j in 0..m.channels().len() {
                        m.value(i, j).to_bits().hash(&mut h);
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
        files: Vec<(PathBuf, Result<Option<Spillover>, String>)>,
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
                };
                this.set_source(id, source).expect("just made");
                id
            })
            .collect();
        let placed: BTreeMap<&Path, usize> =
            saved.files.iter().map(|(p, i)| (p.as_path(), *i)).collect();
        let mut unplaced = Vec::new();
        for (path, own) in files {
            match placed.get(path.as_path()).and_then(|&i| ids.get(i)) {
                Some(&group) => {
                    let own = own.map(|m| m.filter(|m| !m.is_identity()).map(Arc::new));
                    this.files.insert(path, Member { group, own });
                }
                None => unplaced.push((path, own)),
            }
        }
        // Groups keep joining files with their matrix: each is formed around
        // the matrix its files carry, if they all carry the same one.
        for g in &mut this.groups {
            let owns: Vec<&Result<Option<Arc<Spillover>>, String>> = this
                .files
                .values()
                .filter(|m| m.group == g.id)
                .map(|m| &m.own)
                .collect();
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
        for (path, own) in unplaced {
            this.add_file(path, own);
        }
        this
    }
}

/// See [`Compensation::saved`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub groups: Vec<SavedGroup>,
    /// Each file, and the index of its group in `groups`.
    pub files: Vec<(PathBuf, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGroup {
    pub name: String,
    pub source: SavedSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedSource {
    None,
    FilesOwn,
    Csv(PathBuf),
}
