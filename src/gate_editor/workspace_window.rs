//! The Workspace tab: what is loaded, and loading it.
//!
//! A workspace is the FCS files, the metadata that says which file is which
//! sample, the scaling that says how each channel is displayed, and the gating
//! file that holds the gates - see [`crate::workspace`]. They were named by line
//! number in a `file_paths.txt` beside the binary and read once at startup, so
//! changing any of them meant editing a text file and restarting. Everything
//! about them now happens here, and the other tabs only read the result.
//!
//! ## The order things load in
//!
//! The gates are imported *through* the metadata and the scaling: the metadata
//! maps each Omiq file to the specimen its per-sample positions are keyed by,
//! and the scaling puts every coordinate into the space it is drawn in. So a
//! gating file chosen before either has loaded waits for them, and says so,
//! rather than failing.
//!
//! ## Replacing a part under loaded gates
//!
//! - **Scaling** carries the gates across. Each channel whose transform or
//!   range changed goes through the same `rescale_gates` and
//!   `set_current_axis_limits` calls the editor's cofactor and range boxes
//!   make, so drawn, per-specimen and per-sample positions all come through -
//!   see [`scaling_diff`].
//! - **Metadata** re-imports the gating file. It defines the specimen groups
//!   the per-specimen positions are keyed by, so new metadata can re-key every
//!   one of them, and only the import knows how.
//! - **Gating** replaces the gates, which is what it is for.
//!
//! The last two discard every gate position changed since the gating file was
//! loaded - hand edits and autogate results alike - so they ask first. They
//! ask every time gates are loaded, not only when something changed: the gate
//! store keeps no record of what has changed, and asking needlessly is a far
//! smaller fault than not asking when it mattered.
//!
//! ## A new workspace starts empty
//!
//! Opening a folder discards everything, rules included. Rules - and the
//! sample pairing that travels with them - are exported to a file on the rules
//! tab and imported on purpose; they are not carried from one workspace to the
//! next.

use std::path::{Path, PathBuf};

use dioxus::prelude::*;

use crate::compensation::groups::{Compensation, GroupId, Source};
use crate::compensation::{Spillover, own_matrices};
use crate::components::toast::{Toasts, note, say, use_toast, warn};
use crate::file_load::FcsFiles;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::GateStateImplExt;
use crate::gate_editor::path_picker::{Chosen, Pick, UNAVAILABLE, choose};
use crate::gate_editor::plots::axis_store::{
    AxisStore, AxisStoreStoreExt, ScalingDiff, ScalingInfoSource, read_axis_configs, scaling_diff,
};
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::{
    MetaDataImplExt, MetaDataOrigin, MetaDataStore, MetaDataStoreStoreExt,
};
use crate::omiq::serialise::to_omiq_document;
use crate::workspace::{Found, Remembered, detect};

pub type GateStore = Store<GateState, CopyValue<GateState, SyncStorage>>;
pub type MetadataStore = Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>;
pub type AxesStore = Store<AxisStore, CopyValue<AxisStore, SyncStorage>>;

/// What replacing the scaling did to the gates.
#[derive(Debug, Clone, PartialEq)]
pub struct ScalingCarried {
    /// Which channels changed, and which the new file does not have.
    pub diff: ScalingDiff,
    /// Gates that could not be carried, and why. They are left as they were.
    pub problems: Vec<String>,
}

/// Replace the scaling, carrying every gate through each channel's change.
///
/// Channel by channel, the edits a person could make by hand in the editor -
/// a new cofactor is a rescale, a new range a relimit - so whatever the gates
/// hold comes through. Separate from the loading so the whole of it can be run
/// on real stores without the tab around it.
///
/// Refused, with nothing changed, if any channel in `configs` could not be
/// drawn on (see [`crate::gate_editor::AxisInfo::problem`]): `read_axis_configs` refuses such a
/// file already, and this is the second line, before either store is touched.
pub fn carry_to_scaling(
    mut axes: AxesStore,
    mut gates: GateStore,
    configs: Vec<crate::gate_editor::AxisInfo>,
) -> Result<ScalingCarried, String> {
    let unusable: Vec<String> = configs
        .iter()
        .filter_map(crate::gate_editor::AxisInfo::problem)
        .collect();
    if !unusable.is_empty() {
        return Err(format!(
            "the scaling cannot be used: {}",
            unusable.join("; ")
        ));
    }
    let diff = scaling_diff(&axes.peek().settings, &configs);
    axes.with_mut(|s| s.replace_axis_configs(configs));
    let mut problems: Vec<String> = Vec::new();
    for change in &diff.changed {
        if change.transform_changed()
            && let Err(errors) = gates.rescale_gates(&change.channel, &change.old, &change.new)
        {
            problems.extend(errors);
        }
        if change.range_changed()
            && let Err(errors) = gates.set_current_axis_limits(
                change.channel.clone(),
                change.new.axis_lower,
                change.new.axis_upper,
                change.new.transform.clone(),
            )
        {
            problems.extend(errors);
        }
    }
    Ok(ScalingCarried { diff, problems })
}

/// Where one part of the workspace stands.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum Part {
    #[default]
    Absent,
    /// The folder held several files that could be this one, so none was
    /// picked.
    Candidates(Vec<PathBuf>),
    Loading(PathBuf),
    /// Chosen, and waiting for something else to load first.
    Waiting(PathBuf, &'static str),
    Loaded(PathBuf),
    Failed(PathBuf, String),
}

impl Part {
    /// The file this part is, or is about to be.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Part::Loading(p) | Part::Waiting(p, _) | Part::Loaded(p) | Part::Failed(p, _) => {
                Some(p)
            }
            Part::Absent | Part::Candidates(_) => None,
        }
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self, Part::Loaded(_))
    }
}

/// What the workspace holds, apart from the FCS files themselves.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Loaded {
    /// The folder it was opened from, if it was.
    pub folder: Option<PathBuf>,
    pub metadata: Part,
    pub scaling: Part,
    pub gating: Part,
    /// What is running, while something is. One thing at a time: two loads
    /// interleaving would leave the stores holding half of each.
    pub busy: Option<&'static str>,
}

/// Counts that go up when the workspace changes under the other tabs.
///
/// They hold state that names things in the old workspace - the gate a plot
/// is filtered through, the file the editor is showing - and each tab resets
/// its own when these move. A count rather than a flag, so a reset cannot be
/// missed by a tab that was not looking when it was set.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Generation {
    /// The gates were replaced or re-imported: node ids from before mean
    /// nothing now.
    pub document: u64,
    /// The file list changed: indices into it from before may name another
    /// file, or none.
    pub files: u64,
}

/// Every handle the loading needs, so each operation is one value to move
/// into a task. All of them are cheap copies of handles to shared state.
#[derive(Clone, Copy)]
struct Handles {
    gates: GateStore,
    metadata: MetadataStore,
    axes: AxesStore,
    rules: Signal<RuleStore>,
    files: Signal<Option<FcsFiles>>,
    compensation: Signal<Compensation>,
    loaded: Signal<Loaded>,
    generation: Signal<Generation>,
    toasts: Toasts,
}

/// Which part of the workspace an action is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Metadata,
    Scaling,
    Gating,
}

impl Which {
    fn title(self) -> &'static str {
        match self {
            Which::Metadata => "Metadata",
            Which::Scaling => "Scaling",
            Which::Gating => "Gating file",
        }
    }

    fn filter(self) -> (&'static str, &'static str) {
        match self {
            Which::Metadata | Which::Scaling => ("CSV", "csv"),
            Which::Gating => ("Omiq gating file", "omiqgt"),
        }
    }

    fn part(self, loaded: &Loaded) -> &Part {
        match self {
            Which::Metadata => &loaded.metadata,
            Which::Scaling => &loaded.scaling,
            Which::Gating => &loaded.gating,
        }
    }
}

impl Handles {
    fn set_part(mut self, which: Which, part: Part) {
        // Said as well as shown beside the part: a file refused as damaged -
        // a scaling with an unusable axis, a gating file whose tree loops - is
        // otherwise a status line further down a tab the person may not be
        // looking at. Every loader parses before it touches its store, so
        // what was loaded before is still what is loaded.
        if let Part::Failed(path, why) = &part {
            warn(
                &self.toasts,
                format!(
                    "{} {} was not loaded, and nothing was changed: {why}",
                    which.title(),
                    file_name(path)
                ),
            );
        }
        let mut loaded = self.loaded.write();
        match which {
            Which::Metadata => loaded.metadata = part,
            Which::Scaling => loaded.scaling = part,
            Which::Gating => loaded.gating = part,
        }
    }

    fn begin(mut self, doing: &'static str) -> bool {
        if self.loaded.peek().busy.is_some() {
            return false;
        }
        self.loaded.write().busy = Some(doing);
        true
    }

    fn end(mut self) {
        self.loaded.write().busy = None;
        self.remember();
    }

    fn document_changed(mut self) {
        self.generation.write().document += 1;
    }

    fn files_changed(mut self) {
        self.generation.write().files += 1;
    }

    /// Whether anything would be lost by replacing the gates.
    fn gates_loaded(self) -> bool {
        self.gates.read().gate_count() > 0
    }

    /// Record what is open, so it can be offered again next launch.
    ///
    /// A failure is said once rather than swallowed: it means the next launch
    /// will not offer this workspace, which is worth knowing now.
    fn remember(self) {
        let Some(location) = Remembered::location() else {
            return;
        };
        let loaded = self.loaded.peek().clone();
        let remembered = Remembered {
            folder: loaded.folder.clone(),
            fcs: self
                .files
                .peek()
                .as_ref()
                .map(FcsFiles::paths)
                .unwrap_or_default(),
            metadata: loaded.metadata.path().map(Path::to_path_buf),
            scaling: loaded.scaling.path().map(Path::to_path_buf),
            gating: loaded.gating.path().map(Path::to_path_buf),
            compensation: Some(self.compensation.peek().saved()),
        };
        if let Err(e) = remembered.save_to(&location) {
            warn(
                &self.toasts,
                format!("Could not remember this workspace for next time: {e}"),
            );
        }
    }

    /// Discard everything, for a new workspace.
    fn clear_all(mut self, folder: Option<PathBuf>) {
        self.gates.set(GateState::default());
        self.metadata.set(MetaDataStore::default());
        self.axes.set(AxisStore::default());
        self.rules.set(RuleStore::default());
        self.files.set(None);
        self.compensation.set(Compensation::default());
        let busy = self.loaded.peek().busy;
        self.loaded.set(Loaded {
            folder,
            busy,
            ..Loaded::default()
        });
        self.document_changed();
        self.files_changed();
    }

    // ── the four parts ───────────────────────────────────────────────────

    async fn load_metadata(self, path: PathBuf) -> bool {
        self.set_part(Which::Metadata, Part::Loading(path.clone()));
        let mut store = self.metadata;
        let reading = path.clone();
        let result = tokio::task::spawn_blocking(move || {
            store.set_metadata_from_file(reading, "OmiqID", "Filename", MetaDataOrigin::Omiq)
        })
        .await;
        match flatten(result) {
            Ok(warnings) => {
                self.set_part(Which::Metadata, Part::Loaded(path));
                for warning in warnings {
                    warn(&self.toasts, warning);
                }
                true
            }
            Err(e) => {
                self.set_part(Which::Metadata, Part::Failed(path, e));
                false
            }
        }
    }

    /// Load a scaling file, carrying any loaded gates across to it.
    async fn load_scaling(self, path: PathBuf) -> bool {
        self.set_part(Which::Scaling, Part::Loading(path.clone()));
        let reading = path.clone();
        let configs = match flatten(
            tokio::task::spawn_blocking(move || {
                read_axis_configs(reading, ScalingInfoSource::Omiq)
            })
            .await,
        ) {
            Ok(configs) => configs,
            Err(e) => {
                self.set_part(Which::Scaling, Part::Failed(path, e));
                return false;
            }
        };

        let ScalingCarried { diff, problems } =
            match carry_to_scaling(self.axes, self.gates, configs) {
                Ok(carried) => carried,
                Err(e) => {
                    self.set_part(Which::Scaling, Part::Failed(path, e));
                    return false;
                }
            };
        self.set_part(Which::Scaling, Part::Loaded(path));

        if self.gates_loaded() && !diff.changed.is_empty() {
            note(
                &self.toasts,
                format!(
                    "Gates carried to the new scaling on {} channel{}",
                    diff.changed.len(),
                    if diff.changed.len() == 1 { "" } else { "s" }
                ),
            );
        }
        if self.gates_loaded() && !diff.dropped.is_empty() {
            warn(
                &self.toasts,
                format!(
                    "The new scaling has no entry for {} - gates drawn on them keep the previous scaling",
                    diff.dropped
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        if !problems.is_empty() {
            warn(
                &self.toasts,
                format!(
                    "Some gates could not be carried across: {}",
                    problems.join("; ")
                ),
            );
        }
        true
    }

    /// Import a gating file, or wait for what it needs.
    async fn load_gating(self, path: PathBuf) -> bool {
        let metadata = self.metadata.metadata().peek().clone();
        let axes = self.axes.settings().peek().clone();
        let needs = match (metadata.is_empty(), axes.is_empty()) {
            (true, true) => Some("the metadata and the scaling"),
            (true, false) => Some("the metadata"),
            (false, true) => Some("the scaling"),
            (false, false) => None,
        };
        if let Some(needs) = needs {
            self.set_part(Which::Gating, Part::Waiting(path, needs));
            return false;
        }

        self.set_part(Which::Gating, Part::Loading(path.clone()));
        let reading = path.clone();
        let parsed = flatten(
            tokio::task::spawn_blocking(move || {
                GateState::from_gating_file(reading, &metadata, axes)
            })
            .await,
        );
        match parsed {
            Ok(fresh) => {
                let count = fresh.gate_count();
                let mut gates = self.gates;
                gates.set(fresh);
                self.document_changed();
                self.set_part(Which::Gating, Part::Loaded(path.clone()));
                say(
                    &self.toasts,
                    format!("Loaded {count} gates from {}", file_name(&path)),
                );
                true
            }
            Err(e) => {
                self.set_part(Which::Gating, Part::Failed(path, e));
                false
            }
        }
    }

    /// Import the chosen gating file if it was waiting for this.
    async fn gating_if_waiting(self) {
        if let Part::Waiting(path, _) = self.loaded.peek().gating.clone() {
            self.load_gating(path).await;
        }
    }

    // ── what the buttons do ──────────────────────────────────────────────

    async fn open_folder(self, folder: PathBuf) {
        if !self.begin("Opening the folder") {
            return;
        }
        let looking = folder.clone();
        let detected = match flatten(
            tokio::task::spawn_blocking(move || detect(&looking).map_err(anyhow::Error::from))
                .await,
        ) {
            Ok(d) => d,
            Err(e) => {
                warn(
                    &self.toasts,
                    format!("Could not read {}: {e}", folder.display()),
                );
                self.loaded_end_without_saving();
                return;
            }
        };

        self.clear_all(Some(folder.clone()));
        self.open_fcs(Some(folder.clone()), detected.fcs).await;

        let mut chosen: Vec<(Which, PathBuf)> = Vec::new();
        for (which, found) in [
            (Which::Metadata, detected.metadata),
            (Which::Scaling, detected.scaling),
            (Which::Gating, detected.gating),
        ] {
            match found {
                Found::One(path) => chosen.push((which, path)),
                Found::Several(candidates) => self.set_part(which, Part::Candidates(candidates)),
                Found::Missing => self.set_part(which, Part::Absent),
            }
        }
        // Metadata and scaling first: the gates are imported through both.
        for (which, path) in &chosen {
            match which {
                Which::Metadata => {
                    self.load_metadata(path.clone()).await;
                }
                Which::Scaling => {
                    self.load_scaling(path.clone()).await;
                }
                Which::Gating => {}
            }
        }
        if let Some((_, path)) = chosen.iter().find(|(w, _)| *w == Which::Gating) {
            self.load_gating(path.clone()).await;
        }
        self.end();
    }

    async fn reopen(self, remembered: Remembered) {
        if !self.begin("Reopening the last workspace") {
            return;
        }
        let gone = remembered.missing();
        self.clear_all(remembered.folder.clone());
        self.open_fcs(remembered.folder.clone(), remembered.fcs.clone())
            .await;
        if let Some(saved) = remembered.compensation.clone() {
            self.restore_compensation(saved).await;
        }
        if let Some(path) = remembered.metadata.clone() {
            self.load_metadata(path).await;
        }
        if let Some(path) = remembered.scaling.clone() {
            self.load_scaling(path).await;
        }
        if let Some(path) = remembered.gating.clone() {
            self.load_gating(path).await;
        }
        if !gone.is_empty() {
            warn(
                &self.toasts,
                format!(
                    "{} file{} from the last workspace no longer exist{}",
                    gone.len(),
                    if gone.len() == 1 { "" } else { "s" },
                    if gone.len() == 1 { "s" } else { "" }
                ),
            );
        }
        self.end();
    }

    async fn open_fcs(mut self, root: Option<PathBuf>, paths: Vec<PathBuf>) {
        let files = tokio::task::spawn_blocking(move || FcsFiles::open(root.as_deref(), &paths))
            .await
            .unwrap_or_default();
        self.files.set(Some(files));
        self.files_changed();
        self.sync_compensation();
    }

    /// Group any file new to the workspace by its own matrix, and forget any
    /// that has gone. Files already grouped stay where they were put.
    fn sync_compensation(mut self) {
        let owns = self
            .files
            .peek()
            .as_ref()
            .map(|f| own_matrices(f.file_list()))
            .unwrap_or_default();
        self.compensation.write().sync(owns);
    }

    /// Put the remembered groups back, reading each group's CSV again. A CSV
    /// that can no longer be read leaves its group unable to compensate, and
    /// says so, rather than quietly drawing its files uncompensated.
    async fn restore_compensation(mut self, saved: crate::compensation::groups::Saved) {
        let owns = self
            .files
            .peek()
            .as_ref()
            .map(|f| own_matrices(f.file_list()))
            .unwrap_or_default();
        let restored = tokio::task::spawn_blocking(move || {
            Compensation::restore(&saved, owns, |path| {
                Spillover::read_omiq_csv(path).map_err(|e| e.to_string())
            })
        })
        .await;
        match restored {
            Ok(restored) => {
                let unreadable: Vec<String> = restored
                    .groups()
                    .iter()
                    .filter_map(|g| match &g.source {
                        Source::Unreadable { path, why } => {
                            Some(format!("{} ({}): {why}", g.name, file_name(path)))
                        }
                        _ => None,
                    })
                    .collect();
                if !unreadable.is_empty() {
                    warn(
                        &self.toasts,
                        format!(
                            "A compensation matrix could not be read again, and its files will not be drawn until one is chosen: {}",
                            unreadable.join("; ")
                        ),
                    );
                }
                self.compensation.set(restored);
            }
            Err(e) => warn(
                &self.toasts,
                format!("Could not restore the compensation groups: {e}"),
            ),
        }
    }

    /// Give a group a matrix from a CSV. One that cannot be read changes
    /// nothing and says why.
    async fn load_matrix(self, group: GroupId, path: PathBuf) {
        let reading = path.clone();
        let read =
            flatten(tokio::task::spawn_blocking(move || Spillover::read_omiq_csv(&reading)).await);
        match read {
            Ok(matrix) => self.set_compensation(
                group,
                Source::Loaded {
                    path,
                    matrix: std::sync::Arc::new(matrix),
                },
            ),
            Err(why) => warn(
                &self.toasts,
                format!("The matrix {} was not loaded: {why}", file_name(&path)),
            ),
        }
    }

    fn set_compensation(mut self, group: GroupId, source: Source) {
        let set = self.compensation.write().set_source(group, source);
        match set {
            Ok(()) => self.remember(),
            Err(why) => warn(&self.toasts, why),
        }
    }

    fn move_to_group(mut self, path: &Path, group: GroupId) {
        let moved = self.compensation.write().move_file(path, group);
        match moved {
            Ok(()) => self.remember(),
            Err(why) => warn(&self.toasts, why),
        }
    }

    fn new_compensation_group(mut self) -> GroupId {
        let n = self.compensation.peek().groups().len() + 1;
        let id = self.compensation.write().new_group(format!("Group {n}"));
        self.remember();
        id
    }

    fn rename_compensation_group(mut self, group: GroupId, name: String) {
        self.compensation.write().rename(group, name);
        self.remember();
    }

    fn compensation_action(mut self, action: CompensationAction) {
        match action {
            CompensationAction::Source(group, source) => self.set_compensation(group, source),
            CompensationAction::LoadCsv(group, path) => {
                spawn(self.load_matrix(group, path));
            }
            CompensationAction::Move(path, Some(group)) => self.move_to_group(&path, group),
            CompensationAction::Move(path, None) => {
                let group = self.new_compensation_group();
                self.move_to_group(&path, group);
            }
            CompensationAction::NewGroup => {
                self.new_compensation_group();
            }
            CompensationAction::Rename(group, name) => self.rename_compensation_group(group, name),
            CompensationAction::Remove(group) => self.remove_compensation_group(group),
            CompensationAction::AppliedNothing(group) => {
                self.set_applied(group, crate::compensation::groups::Applied::Nothing)
            }
            CompensationAction::AppliedCsv(group, path) => {
                spawn(self.load_applied(group, path));
            }
            CompensationAction::AppliedForget(group) => {
                // Only with nothing wanted: otherwise the matrix wanted would
                // be relative to an answer no longer given.
                self.set_compensation(group, Source::None);
                self.set_applied(group, crate::compensation::groups::Applied::Unknown);
            }
            CompensationAction::SetValue(group, from, into, percent) => {
                let channels = self.first_files_channels(group);
                let set = self
                    .compensation
                    .write()
                    .set_value(group, &from, &into, percent, &channels);
                match set {
                    Ok(()) => self.remember(),
                    Err(why) => warn(&self.toasts, format!("Not changed: {why}")),
                }
            }
            CompensationAction::StartMatrix(group) => {
                let channels = self.first_files_channels(group);
                match Spillover::identity(&channels) {
                    Ok(matrix) => self.set_compensation(
                        group,
                        Source::Edited {
                            matrix: std::sync::Arc::new(matrix),
                            from: None,
                        },
                    ),
                    Err(e) => warn(
                        &self.toasts,
                        format!("No matrix could be made for these files: {e}"),
                    ),
                }
            }
            CompensationAction::ResetToApplied(group) => {
                let applied = self
                    .compensation
                    .peek()
                    .group(group)
                    .map(|g| g.applied.clone());
                if let Some(crate::compensation::groups::Applied::Matrix { path, matrix }) = applied
                {
                    let source = match path {
                        Some(path) => Source::Loaded { path, matrix },
                        None => Source::Edited { matrix, from: None },
                    };
                    self.set_compensation(group, source);
                }
            }
            CompensationAction::Save(group, path, export_for) => {
                self.save_matrix(group, &path, export_for)
            }
            CompensationAction::Copy(group) => {
                let Some(wanted) = self.compensation.peek().wanted(group) else {
                    warn(&self.toasts, "This group has no matrix to copy");
                    return;
                };
                match crate::clipboard::copy_text(&wanted.to_omiq_paste()) {
                    Ok(()) => say(
                        &self.toasts,
                        "Copied - paste it into Omiq's compensation matrix",
                    ),
                    Err(why) => warn(&self.toasts, why),
                }
            }
        }
    }

    /// The fluorescence channels of the group's first file: what a matrix
    /// made here from nothing is over.
    fn first_files_channels(self, group: GroupId) -> Vec<std::sync::Arc<str>> {
        let comp = self.compensation.peek();
        let files = self.files.peek();
        comp.files_in(group)
            .find_map(|path| {
                files
                    .as_ref()?
                    .file_list()
                    .iter()
                    .find(|s| s.get_filepath() == path)
                    .map(crate::compensation::fluorescence_channels)
            })
            .unwrap_or_default()
    }

    fn set_applied(mut self, group: GroupId, applied: crate::compensation::groups::Applied) {
        let set = self.compensation.write().set_applied(group, applied);
        match set {
            Ok(()) => self.remember(),
            Err(why) => warn(&self.toasts, why),
        }
    }

    /// Say the group's Omiq exports were compensated in Omiq with the matrix
    /// in this CSV. One that cannot be read changes nothing.
    async fn load_applied(self, group: GroupId, path: PathBuf) {
        let reading = path.clone();
        let read =
            flatten(tokio::task::spawn_blocking(move || Spillover::read_omiq_csv(&reading)).await);
        match read {
            Ok(matrix) => self.set_applied(
                group,
                crate::compensation::groups::Applied::Matrix {
                    path: Some(path),
                    matrix: std::sync::Arc::new(matrix),
                },
            ),
            Err(why) => warn(
                &self.toasts,
                format!("The matrix {} was not loaded: {why}", file_name(&path)),
            ),
        }
    }

    /// Write a group's matrix as Omiq lays it out.
    fn save_matrix(self, group: GroupId, path: &Path, export_for: ExportFor) {
        let comp = self.compensation.peek();
        let Some(wanted) = comp.wanted(group) else {
            warn(&self.toasts, "This group has no matrix to save");
            return;
        };
        let text = match export_for {
            ExportFor::Omiq => Ok(wanted.to_omiq_csv()),
            ExportFor::TheseFiles => match comp.applied(group) {
                Some(applied) => crate::compensation::residual(&wanted, &applied)
                    .map(|(channels, values)| {
                        crate::compensation::write_grid(&channels, &values, ',')
                    })
                    .map_err(|e| e.to_string()),
                // Nothing applied: the files are as recorded, and what they
                // need is the matrix itself.
                None => Ok(wanted.to_omiq_csv()),
            },
        };
        drop(comp);
        match text.and_then(|t| std::fs::write(path, t).map_err(|e| e.to_string())) {
            Ok(()) => say(&self.toasts, format!("Written to {}", path.display())),
            Err(why) => warn(&self.toasts, format!("Could not write the matrix: {why}")),
        }
    }

    fn remove_compensation_group(mut self, group: GroupId) {
        let removed = self.compensation.write().remove_group(group);
        match removed {
            Ok(()) => self.remember(),
            Err(why) => warn(&self.toasts, format!("The group was not removed: {why}")),
        }
    }

    async fn add_fcs(mut self, paths: Vec<PathBuf>) {
        if !self.begin("Adding FCS files") {
            return;
        }
        let mut files = self
            .files
            .peek()
            .clone()
            .unwrap_or_else(|| FcsFiles::open(self.loaded.peek().folder.as_deref(), &[]));
        let before = files.sample_count();
        let files = tokio::task::spawn_blocking(move || {
            files.add(&paths);
            files
        })
        .await;
        match files {
            Ok(files) => {
                let added = files.sample_count() - before;
                self.files.set(Some(files));
                self.files_changed();
                self.sync_compensation();
                say(
                    &self.toasts,
                    format!("Added {added} file{}", if added == 1 { "" } else { "s" }),
                );
            }
            Err(e) => warn(&self.toasts, format!("Could not add the files: {e}")),
        }
        self.end();
    }

    fn remove_fcs(mut self, path: &Path) {
        if self.loaded.peek().busy.is_some() {
            return;
        }
        let removed = self
            .files
            .write()
            .as_mut()
            .is_some_and(|files| files.remove(path));
        if removed {
            self.files_changed();
            self.sync_compensation();
            self.remember();
        }
    }

    async fn replace(self, which: Which, path: PathBuf) {
        if !self.begin(match which {
            Which::Metadata => "Loading the metadata",
            Which::Scaling => "Loading the scaling",
            Which::Gating => "Loading the gating file",
        }) {
            return;
        }
        match which {
            Which::Metadata => {
                if self.load_metadata(path).await {
                    // The specimen groups may have changed under every
                    // per-specimen position, and only the import knows how to
                    // key them again.
                    let gating = self.loaded.peek().gating.path().map(Path::to_path_buf);
                    if let Some(gating) = gating {
                        self.load_gating(gating).await;
                    }
                }
            }
            Which::Scaling => {
                if self.load_scaling(path).await {
                    self.gating_if_waiting().await;
                }
            }
            Which::Gating => {
                self.load_gating(path).await;
            }
        }
        self.end();
    }

    fn write_gating(self, target: &Path) {
        let written = (|| -> anyhow::Result<()> {
            let document = to_omiq_document(
                &self.gates.read(),
                &self.metadata.metadata().read(),
                &self.axes.settings().read(),
            )?;
            // Pretty-printed: the first thing anyone does with a file Omiq
            // rejects is open it and look.
            std::fs::write(target, serde_json::to_string_pretty(&document)?)?;
            Ok(())
        })();
        match written {
            Ok(()) => say(&self.toasts, format!("Written to {}", target.display())),
            Err(e) => warn(&self.toasts, format!("Could not write: {e}")),
        }
    }

    /// Stop being busy without recording the workspace - for a failure that
    /// changed nothing.
    fn loaded_end_without_saving(mut self) {
        self.loaded.write().busy = None;
    }
}

fn flatten<T>(result: Result<anyhow::Result<T>, tokio::task::JoinError>) -> Result<T, String> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("the worker thread failed: {e}")),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Something done to the compensation groups from the tab.
#[derive(Clone, PartialEq, Debug)]
enum CompensationAction {
    /// Compensate the group with nothing, or with its files' own matrices.
    Source(GroupId, Source),
    LoadCsv(GroupId, PathBuf),
    /// Move a file to a group, or to a new one.
    Move(PathBuf, Option<GroupId>),
    NewGroup,
    Rename(GroupId, String),
    Remove(GroupId),
    /// Nothing was applied to the group's Omiq exports: exported with no
    /// compensation task, or with one left at 0% throughout - the two export
    /// identically.
    AppliedNothing(GroupId),
    /// The group's Omiq exports were compensated in Omiq with this CSV.
    AppliedCsv(GroupId, PathBuf),
    /// Ask again what Omiq applied.
    AppliedForget(GroupId),
    /// One entry of the wanted matrix, in percent: from, into, value.
    SetValue(GroupId, String, String, f64),
    /// A matrix over the files' fluorescence channels, compensating nothing.
    StartMatrix(GroupId),
    /// Want what Omiq applied again, dropping edits.
    ResetToApplied(GroupId),
    /// Write the matrix to `path`: for Omiq, or for the files as exported.
    Save(GroupId, PathBuf, ExportFor),
    /// Put the matrix for Omiq on the clipboard.
    Copy(GroupId),
}

/// Who an exported matrix is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExportFor {
    /// Omiq, which holds the files as recorded: the matrix wanted.
    Omiq,
    /// Software compensating the files as Omiq exported them: what takes
    /// them from what Omiq applied to what is wanted.
    TheseFiles,
}

/// An action that discards the loaded gates, held until it is confirmed.
#[derive(Clone, PartialEq, Debug)]
enum Pending {
    OpenFolder(PathBuf),
    Reopen(Remembered),
    Replace(Which, PathBuf),
}

impl Pending {
    /// Whether this action throws away the gates now loaded, and so has to be
    /// confirmed first. Replacing the scaling carries the gates to the new
    /// transforms rather than discarding them.
    fn discards_gates(&self) -> bool {
        !matches!(self, Pending::Replace(Which::Scaling, _))
    }

    fn describe(&self) -> String {
        let consequence = "Every gate position changed since the gating file was loaded - by hand or by the autogater - will be lost. Write the gating file first if you need them.";
        match self {
            Pending::OpenFolder(folder) => format!(
                "Opening {} starts a new workspace: the gates, the metadata, the scaling and the rules all go. {consequence}",
                folder.display()
            ),
            Pending::Reopen(_) => format!(
                "Reopening the last workspace replaces this one, rules included. {consequence}"
            ),
            Pending::Replace(Which::Metadata, _) => format!(
                "New metadata can re-key every per-specimen gate position, so the gating file is imported again after it loads. {consequence}"
            ),
            Pending::Replace(Which::Gating, path) => format!(
                "Loading {} replaces every gate. {consequence}",
                file_name(path)
            ),
            // Carries the gates across; never asks.
            Pending::Replace(Which::Scaling, _) => String::new(),
        }
    }
}

/// Run an action now, or hold it for confirmation if it would discard gates.
fn act(handles: Handles, mut pending: Signal<Option<Pending>>, action: Pending) {
    if action.discards_gates() && handles.gates_loaded() {
        pending.set(Some(action));
        return;
    }
    run(handles, action);
}

fn run(handles: Handles, action: Pending) {
    spawn(async move {
        match action {
            Pending::OpenFolder(folder) => handles.open_folder(folder).await,
            Pending::Reopen(remembered) => handles.reopen(remembered).await,
            Pending::Replace(which, path) => handles.replace(which, path).await,
        }
    });
}

#[component]
pub fn WorkspaceWindow() -> Element {
    let handles = Handles {
        gates: use_context::<GateStore>(),
        metadata: use_context::<MetadataStore>(),
        axes: use_context::<AxesStore>(),
        rules: use_context::<Signal<RuleStore>>(),
        files: use_context::<Signal<Option<FcsFiles>>>(),
        compensation: use_context::<Signal<Compensation>>(),
        loaded: use_context::<Signal<Loaded>>(),
        generation: use_context::<Signal<Generation>>(),
        toasts: use_toast(),
    };
    let loaded = handles.loaded;
    let files = handles.files;
    let toasts = handles.toasts;

    let mut pending = use_signal(|| None::<Pending>);
    // What was typed into the folder box, if anything; `None` shows the open
    // folder, so the box follows a folder opened any other way.
    let mut folder_typed = use_signal(|| None::<String>);
    let folder_shown = folder_typed().unwrap_or_else(|| {
        loaded
            .read()
            .folder
            .as_ref()
            .map(|f| f.display().to_string())
            .unwrap_or_default()
    });
    let mut export_to = use_signal(|| "gating_export.omiqgt".to_string());

    // The last workspace, offered rather than opened: it may be large, and it
    // may not be the one wanted today.
    let last = use_signal(|| {
        Remembered::location()
            .and_then(|at| Remembered::load_from(&at).ok())
            .filter(|r| !r.is_empty())
    });

    let busy = loaded.read().busy;
    let nothing_open = loaded.read().folder.is_none()
        && files.read().as_ref().is_none_or(|f| f.sample_count() == 0);

    rsx! {
        document::Stylesheet { href: asset!("/assets/workspace.css") }
        div { class: "workspace",
            h2 { "Workspace" }

            // A box beside the dialog, as everywhere else. The dialog is a
            // separate service a machine may not be running, and without the
            // box a workspace could not be opened at all there.
            div { class: "workspace-path",
                input {
                    value: "{folder_shown}",
                    disabled: busy.is_some(),
                    placeholder: "the folder holding the FCS files",
                    oninput: move |e| folder_typed.set(Some(e.value())),
                }
                button {
                    disabled: busy.is_some(),
                    onclick: {
                        let start = PathBuf::from(folder_shown.trim());
                        move |_| {
                            let start = start.clone();
                            spawn(async move {
                                match choose(Pick::Folder, &start, "", &[], false).await {
                                    Chosen::Picked(picked) => {
                                        if let Some(folder) = picked.into_iter().next() {
                                            folder_typed.set(None);
                                            act(handles, pending, Pending::OpenFolder(folder));
                                        }
                                    }
                                    Chosen::Cancelled => {}
                                    Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                                }
                            });
                        }
                    },
                    "Choose..."
                }
                button {
                    class: "workspace-primary",
                    disabled: busy.is_some() || folder_shown.trim().is_empty(),
                    onclick: {
                        let folder = PathBuf::from(folder_shown.trim());
                        move |_| {
                            folder_typed.set(None);
                            act(handles, pending, Pending::OpenFolder(folder.clone()));
                        }
                    },
                    "Open folder"
                }
            }
            p { class: "workspace-hint",
                "Opening a folder starts a new workspace. FCS files are found anywhere under it, sub-folders included. A file in a sub-folder is known in the program by its folders and its name joined with underscores - Plate_10/A1.fcs is Plate_10_A1.fcs - and that is the name the metadata has to use for it. Nothing on disk is renamed. The gating file, the metadata and the scaling have to be at the top of the folder: an .omiqgt, and CSVs with \"metadata\" and \"scaling\" in their names."
            }

            if let Some(doing) = busy {
                p { class: "workspace-busy", "{doing}..." }
            }

            if let Some(action) = pending() {
                div { class: "workspace-confirm",
                    p { "{action.describe()}" }
                    div { class: "workspace-buttons",
                        button {
                            class: "workspace-primary",
                            onclick: move |_| {
                                if let Some(action) = pending.take() {
                                    run(handles, action);
                                }
                            },
                            "Continue"
                        }
                        button { onclick: move |_| pending.set(None), "Cancel" }
                    }
                }
            }

            if nothing_open && busy.is_none() {
                if let Some(remembered) = last() {
                    div { class: "workspace-last",
                        span {
                            "Last workspace: "
                            {
                                remembered
                                    .folder
                                    .as_ref()
                                    .map(|f| f.display().to_string())
                                    .unwrap_or_else(|| format!("{} FCS files", remembered.fcs.len()))
                            }
                        }
                        button {
                            class: "workspace-primary",
                            onclick: move |_| {
                                if let Some(remembered) = last.peek().clone() {
                                    act(handles, pending, Pending::Reopen(remembered));
                                }
                            },
                            "Reopen"
                        }
                    }
                }
            }

            FcsSection {
                busy: busy.is_some(),
                on_add: move |paths: Vec<PathBuf>| {
                    spawn(handles.add_fcs(paths));
                },
                on_remove: move |path: PathBuf| handles.remove_fcs(&path),
            }

            CompensationSection {
                busy: busy.is_some(),
                on_action: move |action: CompensationAction| handles.compensation_action(action),
            }

            for which in [Which::Metadata, Which::Scaling, Which::Gating] {
                PartRow {
                    key: "{which.title()}",
                    which,
                    part: which.part(&loaded.read()).clone(),
                    busy: busy.is_some(),
                    on_choose: move |path: PathBuf| act(handles, pending, Pending::Replace(which, path)),
                }
            }

            div { class: "workspace-row",
                h3 { "Write gating file" }
                p { class: "workspace-hint",
                    "Writes the gates as they stand - drawn, per specimen and per sample - as an Omiq gating file. A gating file has to have been loaded: its header carries the dataset and workflow ids Omiq checks on import."
                }
                div { class: "workspace-path",
                    input {
                        value: "{export_to}",
                        oninput: move |e| export_to.set(e.value()),
                    }
                    button {
                        disabled: busy.is_some(),
                        onclick: move |_| {
                            let start = PathBuf::from(export_to.peek().trim());
                            spawn(async move {
                                let filter = ["omiqgt".to_string()];
                                match choose(Pick::SaveFile, &start, "Omiq gating file", &filter, false).await {
                                    Chosen::Picked(picked) => {
                                        if let Some(target) = picked.into_iter().next() {
                                            export_to.set(target.display().to_string());
                                            handles.write_gating(&target);
                                        }
                                    }
                                    Chosen::Cancelled => {}
                                    Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                                }
                            });
                        },
                        "Choose..."
                    }
                    button {
                        class: "workspace-primary",
                        disabled: busy.is_some(),
                        onclick: move |_| handles.write_gating(&PathBuf::from(export_to.peek().trim())),
                        "Write"
                    }
                }
            }
        }
    }
}

/// The FCS files: what loaded, what did not and why, and adding or removing.
#[component]
fn FcsSection(
    busy: bool,
    on_add: EventHandler<Vec<PathBuf>>,
    on_remove: EventHandler<PathBuf>,
) -> Element {
    let files = use_context::<Signal<Option<FcsFiles>>>();
    let metadata = use_context::<MetadataStore>();
    let toasts = use_toast();
    // Files the metadata has no row for, under their name in the program.
    // Only counted once some metadata has loaded: before that every file is
    // unmatched, and saying so would be noise.
    let unmatched: Vec<std::sync::Arc<str>> = {
        let names = metadata.file_name_to_gating_id();
        let known = names.read();
        if known.is_empty() {
            Vec::new()
        } else {
            files
                .read()
                .as_ref()
                .map(|f| {
                    f.file_list()
                        .iter()
                        .filter(|stub| !known.contains_key(&stub.name))
                        .map(|stub| stub.name.clone())
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    let (loaded, unread) = files
        .read()
        .as_ref()
        .map(|f| (f.file_list().to_vec(), f.unread().to_vec()))
        .unwrap_or_default();

    rsx! {
        div { class: "workspace-row",
            h3 {
                "FCS files"
                span { class: "workspace-status",
                    match (loaded.len(), unread.len()) {
                        (0, 0) => "none loaded".to_string(),
                        (n, 0) => format!("{n} loaded"),
                        (n, m) => format!("{n} loaded, {m} could not be used"),
                    }
                }
            }
            div { class: "workspace-buttons",
                button {
                    disabled: busy,
                    onclick: move |_| {
                        let start = files
                            .peek()
                            .as_ref()
                            .and_then(|f| f.root().map(|r| r.join("x")))
                            .unwrap_or_default();
                        spawn(async move {
                            let filter = ["fcs".to_string()];
                            match choose(Pick::OpenFile, &start, "FCS", &filter, true).await {
                                Chosen::Picked(picked) => on_add.call(picked),
                                Chosen::Cancelled => {}
                                Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                            }
                        });
                    },
                    "Add files..."
                }
            }
            if !unmatched.is_empty() {
                p { class: "workspace-problem",
                    "{unmatched.len()} of these have no row in the metadata under their name in the program, so they cannot be matched to a sample and will not be drawn."
                    if unmatched.iter().any(|name| name.contains('_')) {
                        " A file from a sub-folder is named with its folder in front - the metadata has to use that name too."
                    }
                }
            }
            if !unread.is_empty() {
                table { class: "workspace-table workspace-unread",
                    thead {
                        tr {
                            th { "Could not use" }
                            th { "Why" }
                            th {}
                        }
                    }
                    tbody {
                        for problem in unread {
                            tr { key: "{problem.path.display()}",
                                td { title: "{problem.path.display()}", "{file_name(&problem.path)}" }
                                td { "{problem.reason}" }
                                td {
                                    button {
                                        disabled: busy,
                                        onclick: {
                                            let path = problem.path.clone();
                                            move |_| on_remove.call(path.clone())
                                        },
                                        "remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !loaded.is_empty() {
                // Scrolls inside its own box: a plate is ninety-six files, and
                // the parts below should not sit a page away.
                div { class: "workspace-files",
                    table { class: "workspace-table",
                        thead {
                            tr {
                                th { "Name in the program" }
                                th { "File" }
                                th {}
                            }
                        }
                        tbody {
                            for stub in loaded {
                                tr { key: "{stub.get_filepath().display()}",
                                    td {
                                        class: if unmatched.contains(&stub.name) { "workspace-problem" } else { "" },
                                        title: if unmatched.contains(&stub.name) { "no row in the metadata under this name" } else { "" },
                                        "{stub.name()}"
                                    }
                                    td { class: "workspace-dim", "{stub.get_filepath().display()}" }
                                    td {
                                        button {
                                            disabled: busy,
                                            onclick: {
                                                let path = stub.get_filepath().to_path_buf();
                                                move |_| on_remove.call(path.clone())
                                            },
                                            "remove"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The compensation groups: which matrix each file's events are compensated
/// with. See [`crate::compensation::groups`].
#[component]
fn CompensationSection(busy: bool, on_action: EventHandler<CompensationAction>) -> Element {
    let files = use_context::<Signal<Option<FcsFiles>>>();
    let compensation = use_context::<Signal<Compensation>>();
    let loaded = use_context::<Signal<Loaded>>();
    let comp = compensation.read();
    // Each file's name in the program and its channels, by path.
    let held = files.read();
    let stubs: std::collections::BTreeMap<PathBuf, &crate::file_load::FcsSampleStub> = held
        .as_ref()
        .map(|f| {
            f.file_list()
                .iter()
                .map(|s| (s.get_filepath().to_path_buf(), s))
                .collect()
        })
        .unwrap_or_default();
    let groups: Vec<GroupView> = comp
        .groups()
        .iter()
        .map(|g| {
            let members: Vec<(PathBuf, String)> = comp
                .files_in(g.id)
                .map(|p| {
                    let name = stubs
                        .get(p)
                        .map(|s| s.name().to_string())
                        .unwrap_or_else(|| file_name(p));
                    (p.to_path_buf(), name)
                })
                .collect();
            let notes = comp.check(g.id, |p| {
                stubs
                    .get(p)
                    .map(|s| crate::compensation::facts_of(s))
                    .unwrap_or_default()
            });
            GroupView {
                group: g.clone(),
                members,
                notes,
                holds_omiq: comp.holds_omiq_exports(g.id),
                unanswered: comp.unanswered(g.id),
                wanted: comp.wanted(g.id),
                applied: comp.applied(g.id),
            }
        })
        .collect();
    let choices: Vec<(GroupId, String)> = comp
        .groups()
        .iter()
        .map(|g| (g.id, g.name.clone()))
        .collect();
    drop(comp);
    drop(held);
    let folder = loaded.read().folder.clone();

    if groups.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "workspace-row",
            h3 {
                "Compensation"
                span { class: "workspace-status",
                    if groups.len() == 1 { "1 group" } else { "{groups.len()} groups" }
                }
            }
            p { class: "workspace-hint",
                "As in Omiq, the files are grouped by the spillover matrix they carry, and each group is compensated with its files' own matrices, a matrix loaded from a CSV copied out of Omiq (channel names across the top, values in percent), one edited here, or not at all. Move a file to another group to compensate it differently. Omiq applies its compensation to the events when it exports a file and records nothing of it, so for files exported from Omiq you are asked what was applied: an edit here then applies only the change, and the matrix exported for Omiq is the whole of it. The editor, the gallery and a rules run all read files compensated this way; gates stay where they are, so a new matrix moves the events under them."
            }
            for view in groups {
                CompensationGroup {
                    key: "{view.group.id}",
                    view,
                    choices: choices.clone(),
                    folder: folder.clone(),
                    busy,
                    on_action,
                }
            }
            div { class: "workspace-buttons",
                button {
                    disabled: busy,
                    onclick: move |_| on_action.call(CompensationAction::NewGroup),
                    "New group"
                }
            }
        }
    }
}

/// What the tab shows of one compensation group.
#[derive(Clone, PartialEq)]
struct GroupView {
    group: crate::compensation::groups::Group,
    members: Vec<(PathBuf, String)>,
    notes: Vec<String>,
    /// Whether any of its files were exported from Omiq.
    holds_omiq: bool,
    /// Whether it holds Omiq exports and has not said what Omiq applied.
    unanswered: bool,
    /// The one matrix it wants, if it has one.
    wanted: Option<std::sync::Arc<Spillover>>,
    /// What Omiq applied to its exports, if a matrix.
    applied: Option<std::sync::Arc<Spillover>>,
}

#[component]
fn CompensationGroup(
    view: GroupView,
    choices: Vec<(GroupId, String)>,
    folder: Option<PathBuf>,
    busy: bool,
    on_action: EventHandler<CompensationAction>,
) -> Element {
    let toasts = use_toast();
    let GroupView {
        group,
        members,
        notes,
        holds_omiq,
        unanswered,
        wanted,
        applied,
    } = view;
    let id = group.id;
    let mut typed = use_signal(|| None::<String>);
    let loaded_path = match &group.source {
        Source::Loaded { path, .. } | Source::Unreadable { path, .. } => path.display().to_string(),
        _ => String::new(),
    };
    let shown = typed().unwrap_or(loaded_path);
    let selected = match &group.source {
        Source::None => "none",
        Source::FilesOwn => "own",
        Source::Loaded { .. } | Source::Unreadable { .. } | Source::Edited { .. } => "matrix",
    };
    let count = members.len();
    // Where an export goes unless another place is typed.
    let mut export_typed = use_signal(|| None::<String>);
    let export_shown = export_typed().unwrap_or_else(|| {
        let file = format!("{} compensation.csv", group.name.replace(['/', '\\'], "-"));
        folder
            .as_ref()
            .map(|f| f.join(&file).display().to_string())
            .unwrap_or(file)
    });

    rsx! {
        div { class: "workspace-comp-group",
            div { class: "workspace-path",
                input {
                    class: "workspace-comp-name",
                    value: "{group.name}",
                    disabled: busy,
                    onchange: move |e| on_action.call(CompensationAction::Rename(id, e.value())),
                }
                span { class: "workspace-status",
                    if count == 1 { "1 file" } else { "{count} files" }
                }
                // Each option says whether it is the one chosen, rather than
                // the select being given a value: the matrix option only
                // exists once there is one, and a value set before its option
                // exists is not applied, which left the list showing the
                // group's old source.
                select {
                    disabled: busy,
                    onchange: move |e| match e.value().as_str() {
                        "none" => on_action.call(CompensationAction::Source(id, Source::None)),
                        "own" => on_action.call(CompensationAction::Source(id, Source::FilesOwn)),
                        _ => {}
                    },
                    option {
                        value: "own",
                        selected: selected == "own",
                        disabled: unanswered,
                        "each file's own matrix"
                    }
                    option { value: "none", selected: selected == "none", "no compensation" }
                    if selected == "matrix" {
                        option { value: "matrix", selected: true, "matrix from {group.source.describe()}" }
                    }
                }
                if count == 0 {
                    button {
                        disabled: busy,
                        onclick: move |_| on_action.call(CompensationAction::Remove(id)),
                        "remove group"
                    }
                }
            }

            if holds_omiq {
                AppliedQuestion {
                    group: id,
                    applied: group.applied.clone(),
                    busy,
                    on_action,
                }
            }

            div { class: "workspace-path",
                input {
                    value: "{shown}",
                    disabled: busy || unanswered,
                    placeholder: "a compensation matrix CSV copied out of Omiq",
                    oninput: move |e| typed.set(Some(e.value())),
                }
                button {
                    disabled: busy || unanswered,
                    onclick: {
                        let start = PathBuf::from(shown.trim());
                        move |_| {
                            let start = start.clone();
                            spawn(async move {
                                let filter = ["csv".to_string()];
                                match choose(Pick::OpenFile, &start, "CSV", &filter, false).await {
                                    Chosen::Picked(picked) => {
                                        if let Some(path) = picked.into_iter().next() {
                                            typed.set(None);
                                            on_action.call(CompensationAction::LoadCsv(id, path));
                                        }
                                    }
                                    Chosen::Cancelled => {}
                                    Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                                }
                            });
                        }
                    },
                    "Choose..."
                }
                button {
                    class: "workspace-primary",
                    disabled: busy || unanswered || shown.trim().is_empty(),
                    onclick: {
                        let path = PathBuf::from(shown.trim());
                        move |_| {
                            typed.set(None);
                            on_action.call(CompensationAction::LoadCsv(id, path.clone()));
                        }
                    },
                    "Load matrix"
                }
            }
            for note in notes {
                p { class: "workspace-problem", "{note}" }
            }

            details { class: "workspace-comp-matrix",
                summary { class: "workspace-dim",
                    match &wanted {
                        Some(m) => format!("Matrix - {} channels", m.channels().len()),
                        None => "Matrix".to_string(),
                    }
                }
                if unanswered {
                    p { class: "workspace-hint", "Say what Omiq applied to these files first: an edit is made against it." }
                } else {
                    div { class: "workspace-buttons",
                        if wanted.is_none() {
                            button {
                                disabled: busy || count == 0,
                                onclick: move |_| on_action.call(CompensationAction::StartMatrix(id)),
                                "Start a matrix"
                            }
                        }
                        if let Some(a) = &applied {
                            if wanted.as_ref().is_none_or(|w| !w.same_as(a)) {
                                button {
                                    disabled: busy,
                                    onclick: move |_| on_action.call(CompensationAction::ResetToApplied(id)),
                                    "Back to what Omiq applied"
                                }
                            }
                        }
                    }
                    if let Some(matrix) = wanted.clone() {
                        MatrixGrid {
                            matrix,
                            reference: applied.clone(),
                            editable: !busy,
                            on_set: move |(from, into, percent): (String, String, f64)| {
                                on_action.call(CompensationAction::SetValue(id, from, into, percent))
                            },
                        }
                        p { class: "workspace-hint",
                            "Each row is the fluorochrome that spills, each column the detector it spills into, in percent - as Omiq shows it. "
                            if applied.is_some() {
                                "Entries that differ from what Omiq applied are marked; only the difference is applied to these files."
                            }
                        }
                        div { class: "workspace-path",
                            input {
                                value: "{export_shown}",
                                disabled: busy,
                                oninput: move |e| export_typed.set(Some(e.value())),
                            }
                            button {
                                disabled: busy,
                                onclick: {
                                    let start = PathBuf::from(export_shown.trim());
                                    move |_| {
                                        let start = start.clone();
                                        spawn(async move {
                                            let filter = ["csv".to_string()];
                                            match choose(Pick::SaveFile, &start, "CSV", &filter, false).await {
                                                Chosen::Picked(picked) => {
                                                    if let Some(path) = picked.into_iter().next() {
                                                        export_typed.set(Some(path.display().to_string()));
                                                    }
                                                }
                                                Chosen::Cancelled => {}
                                                Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                                            }
                                        });
                                    }
                                },
                                "Choose..."
                            }
                        }
                        div { class: "workspace-buttons",
                            button {
                                class: "workspace-primary",
                                disabled: busy || export_shown.trim().is_empty(),
                                title: "The whole matrix, for the files as Omiq holds them: paste it into Omiq's compensation.",
                                onclick: {
                                    let path = PathBuf::from(export_shown.trim());
                                    move |_| on_action.call(CompensationAction::Save(id, path.clone(), ExportFor::Omiq))
                                },
                                "Save for Omiq"
                            }
                            button {
                                disabled: busy,
                                title: "The same, on the clipboard, to paste straight into Omiq.",
                                onclick: move |_| on_action.call(CompensationAction::Copy(id)),
                                "Copy for Omiq"
                            }
                            if applied.is_some() {
                                button {
                                    disabled: busy || export_shown.trim().is_empty(),
                                    title: "What takes these files, as Omiq exported them, to this matrix: for software that will compensate the exported files themselves. Not needed here or in Omiq.",
                                    onclick: {
                                        let path = PathBuf::from(export_shown.trim());
                                        move |_| on_action.call(CompensationAction::Save(id, path.clone(), ExportFor::TheseFiles))
                                    },
                                    "Save for these exported files"
                                }
                            }
                        }
                    }
                }
            }

            if count > 0 {
                details {
                    summary { class: "workspace-dim", "Files in this group" }
                    div { class: "workspace-files",
                        table { class: "workspace-table",
                            tbody {
                                for (path, name) in members {
                                    tr { key: "{path.display()}",
                                        td { title: "{path.display()}", "{name}" }
                                        td {
                                            select {
                                                disabled: busy,
                                                onchange: {
                                                    let path = path.clone();
                                                    move |e: FormEvent| {
                                                        let to = e.value().parse::<GroupId>().ok();
                                                        on_action.call(CompensationAction::Move(path.clone(), to));
                                                    }
                                                },
                                                for (other, other_name) in choices.iter() {
                                                    option {
                                                        value: "{other}",
                                                        selected: *other == id,
                                                        "{other_name}"
                                                    }
                                                }
                                                option { value: "new", "a new group" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The question a group of Omiq exports has to answer: what Omiq applied to
/// them when they were exported, which the files do not record.
#[component]
fn AppliedQuestion(
    group: GroupId,
    applied: crate::compensation::groups::Applied,
    busy: bool,
    on_action: EventHandler<CompensationAction>,
) -> Element {
    use crate::compensation::groups::Applied;
    let toasts = use_toast();
    let mut typed = use_signal(String::new);
    let answered = match &applied {
        Applied::Unknown => None,
        Applied::Nothing => Some(
            "Exported from Omiq with no compensation applied (no compensation task, or all 0%)."
                .to_string(),
        ),
        Applied::Matrix {
            path: Some(path), ..
        } => Some(format!("Compensated in Omiq with {}.", file_name(path))),
        Applied::Matrix { path: None, .. } => {
            Some("Compensated in Omiq with a matrix given earlier.".to_string())
        }
        Applied::Unreadable { path, why } => Some(format!(
            "Compensated in Omiq with {}, which can no longer be read ({why}).",
            file_name(path)
        )),
    };

    let class = if answered.is_none() {
        "workspace-confirm"
    } else {
        "workspace-comp-answer"
    };
    rsx! {
        div { class,
            if let Some(answer) = answered {
                span { "{answer}" }
                button {
                    disabled: busy,
                    onclick: move |_| on_action.call(CompensationAction::AppliedForget(group)),
                    "change"
                }
            } else {
                p {
                    "Some of these files were exported from Omiq, which applies its compensation to the events and records nothing of it in the file. Was any compensation applied to them in Omiq when they were exported? A compensation task left at 0% applies none."
                }
                div { class: "workspace-buttons",
                    button {
                        disabled: busy,
                        onclick: move |_| on_action.call(CompensationAction::AppliedNothing(group)),
                        "None - no compensation task, or all 0%"
                    }
                }
                div { class: "workspace-path",
                    input {
                        value: "{typed}",
                        disabled: busy,
                        placeholder: "some - the matrix they were exported with, copied out of Omiq as CSV",
                        oninput: move |e| typed.set(e.value()),
                    }
                    button {
                        disabled: busy,
                        onclick: move |_| {
                            let start = PathBuf::from(typed.peek().trim());
                            spawn(async move {
                                let filter = ["csv".to_string()];
                                match choose(Pick::OpenFile, &start, "CSV", &filter, false).await {
                                    Chosen::Picked(picked) => {
                                        if let Some(path) = picked.into_iter().next() {
                                            on_action.call(CompensationAction::AppliedCsv(group, path));
                                        }
                                    }
                                    Chosen::Cancelled => {}
                                    Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                                }
                            });
                        },
                        "Choose..."
                    }
                    button {
                        class: "workspace-primary",
                        disabled: busy || typed().trim().is_empty(),
                        onclick: move |_| {
                            let path = PathBuf::from(typed.peek().trim());
                            on_action.call(CompensationAction::AppliedCsv(group, path));
                        },
                        "Some - this matrix"
                    }
                }
            }
        }
    }
}

/// A matrix laid out as Omiq shows it - each row the fluorochrome that
/// spills, each column the detector it spills into, in percent - with every
/// entry off the diagonal editable. Entries that differ from `reference` are
/// marked.
#[component]
fn MatrixGrid(
    matrix: std::sync::Arc<Spillover>,
    reference: Option<std::sync::Arc<Spillover>>,
    editable: bool,
    on_set: EventHandler<(String, String, f64)>,
) -> Element {
    // Only the channels that take part, unless asked for all: a panel-wide
    // matrix is mostly identity, and forty columns of zeros hide the few
    // entries that matter.
    let mut all = use_signal(|| false);
    let n = matrix.channels().len();
    let mixing: Vec<usize> = match matrix.involved() {
        Some(m) => (0..n)
            .filter(|&i| m.channels().contains(&matrix.channels()[i]))
            .collect(),
        None => Vec::new(),
    };
    let shown: Vec<usize> = if all() || mixing.is_empty() {
        (0..n).collect()
    } else {
        mixing.clone()
    };
    let differs = |i: usize, j: usize| -> bool {
        let Some(r) = &reference else {
            return false;
        };
        let (a, b) = (&matrix.channels()[i], &matrix.channels()[j]);
        let at = |c: &std::sync::Arc<str>| r.channels().iter().position(|x| x == c);
        let was = match (at(a), at(b)) {
            (Some(x), Some(y)) => r.value(x, y),
            _ if i == j => 1.0,
            _ => 0.0,
        };
        (was - matrix.value(i, j)).abs() > 1e-9
    };
    let percent = |v: f64| {
        let p = (v * 100.0 * 1e6).round() / 1e6;
        let text = format!("{p:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    // What to add to the grid to reach a channel not shown yet.
    let mut add_from = use_signal(String::new);
    let mut add_into = use_signal(String::new);
    let mut add_value = use_signal(String::new);
    let channels: Vec<String> = matrix.channels().iter().map(|c| c.to_string()).collect();

    rsx! {
        div { class: "workspace-buttons",
            label {
                input {
                    r#type: "checkbox",
                    checked: all(),
                    onchange: move |e| all.set(e.checked()),
                }
                if mixing.is_empty() {
                    " every channel (none spills into another)"
                } else {
                    " every channel, not only the {mixing.len()} that spill"
                }
            }
        }
        div { class: "workspace-comp-grid",
            table {
                thead {
                    tr {
                        th {}
                        for &j in shown.iter() {
                            th { class: "workspace-comp-col", title: "{matrix.channels()[j]}",
                                span { "{matrix.channels()[j]}" }
                            }
                        }
                    }
                }
                tbody {
                    for &i in shown.iter() {
                        tr { key: "{matrix.channels()[i]}",
                            th { class: "workspace-comp-row", "{matrix.channels()[i]}" }
                            for &j in shown.iter() {
                                td {
                                    class: if differs(i, j) { "workspace-comp-changed" } else { "" },
                                    if i == j {
                                        span { class: "workspace-dim", "100" }
                                    } else {
                                        input {
                                            r#type: "number",
                                            step: "0.1",
                                            disabled: !editable,
                                            title: "{matrix.channels()[i]} into {matrix.channels()[j]}",
                                            value: "{percent(matrix.value(i, j))}",
                                            onchange: {
                                                let from = matrix.channels()[i].to_string();
                                                let into = matrix.channels()[j].to_string();
                                                move |e: FormEvent| {
                                                    if let Ok(v) = e.value().trim().parse::<f64>() {
                                                        on_set.call((from.clone(), into.clone(), v));
                                                    }
                                                }
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if !all() {
            // Setting a value between two channels that do not spill yet
            // brings them into the grid.
            div { class: "workspace-path",
                span { class: "workspace-dim", "Set" }
                select {
                    disabled: !editable,
                    onchange: move |e| add_from.set(e.value()),
                    option { value: "", selected: add_from().is_empty(), "from..." }
                    for c in channels.iter() {
                        option { value: "{c}", selected: add_from() == *c, "{c}" }
                    }
                }
                span { class: "workspace-dim", "into" }
                select {
                    disabled: !editable,
                    onchange: move |e| add_into.set(e.value()),
                    option { value: "", selected: add_into().is_empty(), "into..." }
                    for c in channels.iter() {
                        option { value: "{c}", selected: add_into() == *c, "{c}" }
                    }
                }
                input {
                    r#type: "number",
                    step: "0.1",
                    class: "workspace-comp-value",
                    placeholder: "%",
                    value: "{add_value}",
                    oninput: move |e| add_value.set(e.value()),
                }
                button {
                    disabled: !editable || add_from().is_empty() || add_into().is_empty() || add_value().trim().parse::<f64>().is_err(),
                    onclick: move |_| {
                        let value = add_value.peek().trim().parse::<f64>();
                        if let Ok(v) = value {
                            on_set.call((add_from.peek().clone(), add_into.peek().clone(), v));
                            add_value.set(String::new());
                        }
                    },
                    "Set"
                }
            }
        }
    }
}

/// One of the metadata, the scaling or the gating file.
#[component]
fn PartRow(which: Which, part: Part, busy: bool, on_choose: EventHandler<PathBuf>) -> Element {
    let toasts = use_toast();
    // What was typed into the box, if anything. `None` shows the loaded
    // path, so the box follows a load made any other way - opening a folder,
    // reopening the last workspace - without being overwritten mid-typing.
    let mut typed = use_signal(|| None::<String>);
    let shown = typed().unwrap_or_else(|| {
        part.path()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });
    let (label, extension) = which.filter();
    let mut candidate = use_signal(String::new);

    let status = match &part {
        Part::Absent => "not chosen".to_string(),
        Part::Candidates(list) => format!(
            "{} files in the folder could be this - choose one",
            list.len()
        ),
        Part::Loading(_) => "loading...".to_string(),
        Part::Waiting(_, needs) => format!("waiting for {needs}"),
        Part::Loaded(_) => "loaded".to_string(),
        Part::Failed(_, why) => format!("could not load: {why}"),
    };
    let status_class = match &part {
        Part::Failed(..) | Part::Candidates(_) => "workspace-status workspace-problem",
        Part::Waiting(..) | Part::Absent => "workspace-status workspace-waiting",
        _ => "workspace-status",
    };

    rsx! {
        div { class: "workspace-row",
            h3 {
                "{which.title()}"
                span { class: "{status_class}", "{status}" }
            }
            if which == Which::Scaling {
                p { class: "workspace-hint",
                    "Replacing the scaling carries the gates across: every channel whose transform or range changed is rescaled, exactly as changing it in the editor would."
                }
            }
            if let Part::Candidates(list) = &part {
                div { class: "workspace-path",
                    select {
                        value: "{candidate}",
                        onchange: move |e| candidate.set(e.value()),
                        option { value: "", "choose one" }
                        for path in list.clone() {
                            option {
                                value: "{path.display()}",
                                selected: *candidate.read() == path.display().to_string(),
                                "{file_name(&path)}"
                            }
                        }
                    }
                    button {
                        class: "workspace-primary",
                        disabled: busy || candidate.read().is_empty(),
                        onclick: move |_| on_choose.call(PathBuf::from(candidate())),
                        "Use this"
                    }
                }
            }
            div { class: "workspace-path",
                input {
                    value: "{shown}",
                    disabled: busy,
                    oninput: move |e| typed.set(Some(e.value())),
                }
                button {
                    disabled: busy,
                    onclick: {
                        let start = PathBuf::from(shown.trim());
                        move |_| {
                            let start = start.clone();
                            spawn(async move {
                                let filter = [extension.to_string()];
                                match choose(Pick::OpenFile, &start, label, &filter, false).await {
                                    Chosen::Picked(picked) => {
                                        if let Some(path) = picked.into_iter().next() {
                                            typed.set(None);
                                            on_choose.call(path);
                                        }
                                    }
                                    Chosen::Cancelled => {}
                                    Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
                                }
                            });
                        }
                    },
                    "Choose..."
                }
                button {
                    class: "workspace-primary",
                    disabled: busy || shown.trim().is_empty(),
                    onclick: {
                        let path = PathBuf::from(shown.trim());
                        move |_| {
                            typed.set(None);
                            on_choose.call(path.clone());
                        }
                    },
                    "Load"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_replacing_the_scaling_goes_ahead_without_asking() {
        let path = PathBuf::from("/data/run1/new.csv");
        assert!(!Pending::Replace(Which::Scaling, path.clone()).discards_gates());
        for action in [
            Pending::OpenFolder(PathBuf::from("/data/run2")),
            Pending::Reopen(Remembered::default()),
            Pending::Replace(Which::Metadata, path.clone()),
            Pending::Replace(Which::Gating, PathBuf::from("/data/run1/gates.omiqgt")),
        ] {
            assert!(action.discards_gates(), "{action:?}");
            let said = action.describe();
            assert!(
                said.contains("will be lost"),
                "{action:?} does not say what is lost: {said}"
            );
        }
    }

    #[test]
    fn a_confirmation_names_what_is_being_opened() {
        let open = Pending::OpenFolder(PathBuf::from("/data/run2")).describe();
        assert!(
            open.contains("/data/run2") && open.contains("rules"),
            "{open}"
        );
        let gating =
            Pending::Replace(Which::Gating, PathBuf::from("/data/run1/gates_v2.omiqgt")).describe();
        assert!(gating.contains("gates_v2.omiqgt"), "{gating}");
        let metadata = Pending::Replace(Which::Metadata, PathBuf::from("m.csv")).describe();
        assert!(metadata.contains("imported again"), "{metadata}");
    }

    #[test]
    fn a_part_knows_its_file_whatever_state_it_is_in() {
        let p = PathBuf::from("/data/metadata.csv");
        for part in [
            Part::Loading(p.clone()),
            Part::Waiting(p.clone(), "the scaling"),
            Part::Loaded(p.clone()),
            Part::Failed(p.clone(), "bad".into()),
        ] {
            assert_eq!(part.path(), Some(p.as_path()), "{part:?}");
        }
        assert_eq!(Part::Absent.path(), None);
        assert_eq!(
            Part::Candidates(vec![p.clone()]).path(),
            None,
            "none was picked"
        );
        assert!(Part::Loaded(p.clone()).is_loaded());
        assert!(!Part::Loading(p).is_loaded());
    }

    #[test]
    fn each_part_is_read_from_its_own_slot_and_offers_its_own_file_type() {
        let loaded = Loaded {
            metadata: Part::Loaded(PathBuf::from("m.csv")),
            scaling: Part::Loaded(PathBuf::from("s.csv")),
            gating: Part::Loaded(PathBuf::from("g.omiqgt")),
            ..Loaded::default()
        };
        assert_eq!(
            Which::Metadata.part(&loaded).path(),
            Some(Path::new("m.csv"))
        );
        assert_eq!(
            Which::Scaling.part(&loaded).path(),
            Some(Path::new("s.csv"))
        );
        assert_eq!(
            Which::Gating.part(&loaded).path(),
            Some(Path::new("g.omiqgt"))
        );
        assert_eq!(Which::Metadata.filter().1, "csv");
        assert_eq!(Which::Scaling.filter().1, "csv");
        assert_eq!(Which::Gating.filter().1, "omiqgt");
    }

    #[test]
    fn a_failed_load_says_why_and_a_crashed_worker_says_so() {
        assert_eq!(flatten(Ok(Ok(3))), Ok(3));
        assert_eq!(
            flatten::<()>(Ok(Err(anyhow::anyhow!("no such column")))),
            Err("no such column".to_string())
        );
    }

    #[test]
    fn a_file_is_named_by_its_file_name() {
        assert_eq!(file_name(Path::new("/a/b/gates.omiqgt")), "gates.omiqgt");
        assert_eq!(file_name(Path::new("/")), "/");
    }
}
