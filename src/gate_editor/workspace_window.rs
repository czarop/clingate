//! The Workspace tab: what is loaded, and loading it.
//!
//! A workspace is the FCS files, the metadata that says which file is which
//! sample, the scaling that says how each channel is displayed, and the gating
//! file that holds the gates - see [`clingate_core::workspace`]. They were named by line
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
//!   see [`clingate_core::workspace::carry_to_scaling`].
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
//! Opening a folder discards everything, rules included, and then opens what
//! the folder holds - its rules too, from `rules/gate_rules.json`, where the
//! rules tab saves them (see [`clingate_core::workspace::workspace_rules`],
//! which the tools for Claude open a workspace's rules with as well). Rules
//! are not carried from one workspace to the next.

use std::path::{Path, PathBuf};

use dioxus::prelude::*;

use crate::components::toast::{Toasts, note, say, use_toast, warn};
use crate::gate_editor::compensation_panel::{CompensationAction, CompensationPanel, ExportFor};
use crate::gate_editor::path_picker::{Chosen, Pick, UNAVAILABLE, choose};
use clingate_core::axis_store::{
    AxisStore, AxisStoreStoreExt, ScalingInfoSource, read_axis_configs,
};
use clingate_core::compensation::groups::{Compensation, GroupId, Source};
use clingate_core::compensation::{Spillover, own_matrices};
use clingate_core::file_load::FcsFiles;
use clingate_core::gate_rules::rule_store::{RuleStore, SamplePairing};
use clingate_core::gates::GateState;
use clingate_core::omiq::metadata::{
    MetaDataImplExt, MetaDataOrigin, MetaDataStore, MetaDataStoreStoreExt, OMIQ_FILE_NAME_COLUMN,
    OMIQ_ID_COLUMN,
};
use clingate_core::workspace::{
    Found, Remembered, RulesRead, ScalingCarried, WORKSPACE_FILE, carry_to_scaling, detect,
    gating_needs,
};

pub type GateStore = Store<GateState, CopyValue<GateState, SyncStorage>>;
pub type MetadataStore = Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>;
pub type AxesStore = Store<AxisStore, CopyValue<AxisStore, SyncStorage>>;

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
    /// Set when the folder was opened from the workspace saved in it: how
    /// many FCS files in the folder that workspace leaves out.
    pub restored: Option<usize>,
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
    /// The working copy was put back to another state - an undo, a redo, a
    /// revert. Plots match their gates to their axes again.
    pub restored: u64,
}

/// Every handle the loading needs, so each operation is one value to move
/// into a task. All of them are cheap copies of handles to shared state.
#[derive(Clone, Copy)]
pub(crate) struct Handles {
    gates: GateStore,
    metadata: MetadataStore,
    axes: AxesStore,
    rules: Signal<RuleStore>,
    files: Signal<Option<FcsFiles>>,
    compensation: Signal<Compensation>,
    loaded: Signal<Loaded>,
    generation: Signal<Generation>,
    toasts: Toasts,
    edits: crate::gate_editor::edits::Edits,
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

    /// Record what is open: in the workspace's own folder, so opening the
    /// folder again - here, on another machine, or from the tools for Claude -
    /// finds it as it was left, and in this program's configuration folder,
    /// so it can be offered again next launch.
    ///
    /// A failure is said rather than swallowed: it means the workspace will
    /// not open as it was left, which is worth knowing now.
    fn remember(self) {
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
            pairing: Some(self.rules.peek().pairing.clone()),
        };
        if remembered.is_empty() {
            return;
        }
        if let Err(e) = remembered.save_into_folder() {
            warn(
                &self.toasts,
                format!("Could not save the workspace into its folder: {e}"),
            );
        }
        // Not from tests: they would overwrite the real last workspace.
        if !cfg!(test)
            && let Some(location) = Remembered::location()
            && let Err(e) = remembered.save_to(&location)
        {
            warn(
                &self.toasts,
                format!("Could not remember this workspace for next time: {e}"),
            );
        }
    }

    /// Discard everything, for a new workspace.
    fn clear_all(mut self, folder: Option<PathBuf>) {
        self.edits.cleared();
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
            store.set_metadata_from_file(
                reading,
                OMIQ_ID_COLUMN,
                OMIQ_FILE_NAME_COLUMN,
                MetaDataOrigin::Omiq,
            )
        })
        .await;
        match flatten(result) {
            Ok(warnings) => {
                let rows = self.metadata.metadata().peek().len();
                say(&self.toasts, metadata_loaded(&path, rows));
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

        // Written whole: a new scaling redraws everything anyway.
        let mut axes = self.axes;
        let mut gates = self.gates;
        let carried = carry_to_scaling(&mut axes.write(), &mut gates.write(), configs);
        let ScalingCarried { diff, problems } = match carried {
            Ok(carried) => carried,
            Err(e) => {
                self.set_part(Which::Scaling, Part::Failed(path, e));
                return false;
            }
        };
        say(
            &self.toasts,
            format!("Loaded the scaling from {}", file_name(&path)),
        );
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
    async fn load_gating(self, path: PathBuf, why: GatingLoad) -> bool {
        let metadata = self.metadata.metadata().peek().clone();
        let axes = self.axes.settings().peek().clone();
        let needs = gating_needs(&metadata, &axes);
        if let Some(needs) = needs {
            if why == GatingLoad::Chosen {
                note(&self.toasts, gating_waiting(&path, needs));
            }
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
                // Both the saved copy and the working copy.
                self.edits.loaded_fresh();
                self.document_changed();
                self.set_part(Which::Gating, Part::Loaded(path.clone()));
                say(&self.toasts, gating_loaded(&path, count, why));
                true
            }
            Err(e) => {
                self.set_part(Which::Gating, Part::Failed(path, e));
                false
            }
        }
    }

    /// Record that the folder opened as it was saved, and how many FCS files
    /// in it the saved workspace does not include - added since, or left out
    /// on purpose - so none is missed. Shown on the tab rather than said: a
    /// message would be buried under the loads' own.
    async fn note_unlisted(mut self, folder: &Path, named: &[PathBuf]) {
        let looking = folder.to_path_buf();
        let unlisted = match tokio::task::spawn_blocking(move || detect(&looking)).await {
            Ok(Ok(found)) => found.fcs.iter().filter(|f| !named.contains(f)).count(),
            _ => 0,
        };
        self.loaded.write().restored = Some(unlisted);
    }

    /// Import the chosen gating file if it was waiting for this.
    async fn gating_if_waiting(self) {
        if let Part::Waiting(path, _) = self.loaded.peek().gating.clone() {
            self.load_gating(path, GatingLoad::Waited).await;
        }
    }

    // ── what the buttons do ──────────────────────────────────────────────

    pub(crate) async fn open_folder(self, folder: PathBuf) {
        if !self.begin("Opening the folder") {
            return;
        }
        match Remembered::load_from_folder(&folder) {
            Ok(Some(saved)) => {
                let named = saved.fcs.clone();
                self.restore(saved).await;
                self.note_unlisted(&folder, &named).await;
                self.edits.check_recovery();
                self.end();
                return;
            }
            Ok(None) => {}
            Err(e) => warn(
                &self.toasts,
                format!(
                    "The workspace saved in {} could not be read, so the folder is opened afresh: {e}",
                    folder.display()
                ),
            ),
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
        self.open_rules(&folder, None);
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
            self.load_gating(path.clone(), GatingLoad::Chosen).await;
        }
        self.edits.check_recovery();
        self.end();
    }

    async fn reopen(self, remembered: Remembered) {
        if !self.begin("Reopening the last workspace") {
            return;
        }
        // The folder's own copy where there is one: it is the one kept with
        // the data, and may have been saved since on another machine.
        let saved = remembered
            .folder
            .as_deref()
            .and_then(|folder| Remembered::load_from_folder(folder).ok().flatten());
        match saved {
            Some(saved) => {
                let named = saved.fcs.clone();
                let folder = saved.folder.clone().unwrap_or_default();
                self.restore(saved).await;
                self.note_unlisted(&folder, &named).await;
            }
            None => self.restore(remembered).await,
        }
        self.edits.check_recovery();
        self.end();
    }

    /// Open everything a remembered workspace names, as it was left.
    async fn restore(mut self, remembered: Remembered) {
        let gone = remembered.missing();
        self.clear_all(remembered.folder.clone());
        match remembered.folder.as_deref() {
            Some(folder) => self.open_rules(folder, remembered.pairing.clone()),
            None => {
                if let Some(pairing) = remembered.pairing.clone() {
                    self.rules.write().pairing = pairing;
                }
            }
        }
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
            self.load_gating(path, GatingLoad::Chosen).await;
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
    }

    /// The folder's rules, with the pairing the workspace was last left with
    /// over them.
    fn open_rules(mut self, folder: &Path, pairing: Option<SamplePairing>) {
        let opened = clingate_core::workspace::workspace_rules(folder, pairing);
        if let RulesRead::Failed(e) = &opened.read {
            warn(
                &self.toasts,
                format!(
                    "The rules in {} could not be read: {e}",
                    opened.file.display()
                ),
            );
        }
        self.rules.set(opened.store);
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
    async fn restore_compensation(mut self, saved: clingate_core::compensation::groups::Saved) {
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

    fn assign_to_group(mut self, paths: &[PathBuf], group: GroupId) {
        let moved = self.compensation.write().move_files(paths, group);
        match moved {
            Ok(()) => self.remember(),
            Err(why) => warn(&self.toasts, why),
        }
    }

    fn new_compensation_group(mut self) -> GroupId {
        let name = self.compensation.peek().next_group_name();
        let id = self.compensation.write().new_group(name);
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
            CompensationAction::Assign(paths, Some(group)) => self.assign_to_group(&paths, group),
            CompensationAction::Assign(paths, None) => {
                let group = self.new_compensation_group();
                self.assign_to_group(&paths, group);
            }
            CompensationAction::NewGroup => {
                self.new_compensation_group();
            }
            CompensationAction::Rename(group, name) => self.rename_compensation_group(group, name),
            CompensationAction::Remove(group) => self.remove_compensation_group(group),
            CompensationAction::AppliedNothing(group) => {
                self.set_applied(group, clingate_core::compensation::groups::Applied::Nothing)
            }
            CompensationAction::AppliedMatrix(group, matrix) => self.set_applied(
                group,
                clingate_core::compensation::groups::Applied::Matrix { path: None, matrix },
            ),
            // The matrix wanted is kept: it is what the files should end up
            // compensated with whatever the answer, and nothing is drawn
            // until the question is answered again.
            CompensationAction::AppliedForget(group) => {
                self.set_applied(group, clingate_core::compensation::groups::Applied::Unknown)
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
                            changed: false,
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
                if let Some(clingate_core::compensation::groups::Applied::Matrix { path, matrix }) =
                    applied
                {
                    let source = match path {
                        Some(path) => Source::Loaded { path, matrix },
                        None => Source::Edited {
                            matrix,
                            from: Some("Omiq's matrix".into()),
                            changed: false,
                        },
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
                    .map(clingate_core::compensation::fluorescence_channels)
            })
            .unwrap_or_default()
    }

    fn set_applied(
        mut self,
        group: GroupId,
        applied: clingate_core::compensation::groups::Applied,
    ) {
        let set = self.compensation.write().set_applied(group, applied);
        match set {
            Ok(()) => self.remember(),
            Err(why) => warn(&self.toasts, why),
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
                Some(applied) => clingate_core::compensation::residual(&wanted, &applied)
                    .map(|(channels, values)| {
                        clingate_core::compensation::write_grid(&channels, &values, ',')
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
                        self.load_gating(gating, GatingLoad::NewMetadata).await;
                    }
                }
            }
            Which::Scaling => {
                // Carrying the gates to a new scaling is an edit of the
                // working copy, when there is one.
                let before = self.edits.before();
                if self.load_scaling(path).await {
                    if self.edits.has_saved_now() {
                        self.edits.after(before);
                    }
                    self.gating_if_waiting().await;
                }
            }
            Which::Gating => {
                self.load_gating(path, GatingLoad::Chosen).await;
            }
        }
        self.end();
    }

    /// Export the saved copy - not unsaved changes - as an Omiq gating file.
    /// A bare name goes into the workspace folder, as the tools' export does.
    pub(crate) fn write_gating(self, typed: &Path) {
        let folder = self.loaded.peek().folder.clone();
        let target =
            clingate_core::workspace::export_target(folder.as_deref(), &typed.to_string_lossy());
        let target = target.as_path();
        match self.edits.export(target) {
            Ok(false) => say(&self.toasts, format!("Exported to {}", target.display())),
            Ok(true) => say(
                &self.toasts,
                format!(
                    "Exported the last save to {} - the unsaved changes are not in it",
                    target.display()
                ),
            ),
            Err(e) => warn(&self.toasts, format!("Could not export: {e}")),
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

/// Why a gating file is being imported, which is what its message says.
#[derive(Clone, Copy, PartialEq, Debug)]
enum GatingLoad {
    /// Chosen, or found in the folder.
    Chosen,
    /// Imported again so its per-specimen positions follow new metadata.
    NewMetadata,
    /// Chosen earlier, and waiting for the metadata or the scaling.
    Waited,
}

fn metadata_loaded(path: &Path, rows: usize) -> String {
    format!(
        "Loaded the metadata from {}: {rows} file{}",
        file_name(path),
        if rows == 1 { "" } else { "s" }
    )
}

fn gating_loaded(path: &Path, gates: usize, why: GatingLoad) -> String {
    let name = file_name(path);
    match why {
        GatingLoad::Chosen => format!("Loaded {gates} gates from {name}"),
        GatingLoad::NewMetadata => format!(
            "Imported {name} again so its per-specimen positions follow the new metadata: {gates} gates"
        ),
        GatingLoad::Waited => {
            format!(
                "Loaded {gates} gates from {name}, which was waiting for the metadata and scaling"
            )
        }
    }
}

fn gating_waiting(path: &Path, needs: &str) -> String {
    format!(
        "{} is not imported yet: it waits for {needs}, and is imported as soon as that is loaded",
        file_name(path)
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
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
        let consequence = "The working copy has unsaved changes - by hand or by the rules - and they will be lost. Save first if you need them.";
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
    if action.discards_gates() && handles.gates_loaded() && handles.edits.dirty_now() {
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

impl Handles {
    /// The Workspace tab's handles, from the document every tab shares. The
    /// tab takes its own through this, and so do the parity tests.
    pub(crate) fn from_context() -> Self {
        Handles {
            gates: use_context::<GateStore>(),
            metadata: use_context::<MetadataStore>(),
            axes: use_context::<AxesStore>(),
            rules: use_context::<Signal<RuleStore>>(),
            files: use_context::<Signal<Option<FcsFiles>>>(),
            compensation: use_context::<Signal<Compensation>>(),
            loaded: use_context::<Signal<Loaded>>(),
            generation: use_context::<Signal<Generation>>(),
            toasts: use_toast(),
            edits: use_context::<crate::gate_editor::edits::Edits>(),
        }
    }
}

#[component]
pub fn WorkspaceWindow() -> Element {
    let handles = Handles::from_context();
    let loaded = handles.loaded;
    let files = handles.files;
    let toasts = handles.toasts;

    // Which columns group and order the samples is chosen beside the sample
    // selector and on the rules tab, not here; it is kept with the workspace
    // whenever it changes. A memo, so an edit to a rule is not a save. Not
    // while something is loading: a reopen clears the grouping before it
    // puts the saved one back, and the load saves when it ends.
    // Save points the workspace at the saved files; remembered at once, so
    // the folder opens on them next time.
    let parts = use_memo(move || {
        let l = loaded.read();
        (
            l.gating.path().map(Path::to_path_buf),
            l.scaling.path().map(Path::to_path_buf),
        )
    });
    use_effect(move || {
        parts.read();
        if loaded.peek().busy.is_none() {
            handles.remember();
        }
    });

    let pairing = use_memo(move || handles.rules.read().pairing.clone());
    use_effect(move || {
        pairing.read();
        if loaded.peek().busy.is_none() {
            handles.remember();
        }
    });

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
            crate::gate_editor::edits::EditBar {}

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
                "Opening a folder opens the workspace saved in it as it was left - its files, compensation, and which columns group and sort the samples, kept in {WORKSPACE_FILE} and updated as you work. A folder without one starts a new workspace: FCS files are found anywhere under it, sub-folders included. A file in a sub-folder is known in the program by its folders and its name joined with underscores - Plate_10/A1.fcs is Plate_10_A1.fcs - and that is the name the metadata has to use for it. Nothing on disk is renamed. The gating file, the metadata and the scaling have to be at the top of the folder: an .omiqgt, and CSVs with \"metadata\" and \"scaling\" in their names."
            }

            if let Some(doing) = busy {
                p { class: "workspace-busy", "{doing}..." }
            }
            if let Some(unlisted) = loaded.read().restored {
                p { class: "workspace-restored",
                    "Opened as it was last saved."
                    if unlisted == 1 {
                        " 1 FCS file in the folder is not in this workspace: add it with Add files if it belongs."
                    } else if unlisted > 1 {
                        " {unlisted} FCS files in the folder are not in this workspace: add them with Add files if they belong."
                    }
                }
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

            CompensationPanel {
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
                h3 { "Export gating file" }
                p { class: "workspace-hint",
                    "Writes the saved copy - the gates as last saved, drawn, per specimen and per sample - as an Omiq gating file. Unsaved changes are not in it: Save first. A gating file has to have been loaded: its header carries the dataset and workflow ids Omiq checks on import."
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
                        "Export"
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

#[cfg(test)]
mod messages {
    use super::*;

    #[test]
    fn loading_metadata_says_metadata_and_how_many_files() {
        let path = PathBuf::from("/w/metadata_plate10.csv");
        assert_eq!(
            metadata_loaded(&path, 36),
            "Loaded the metadata from metadata_plate10.csv: 36 files"
        );
        assert!(metadata_loaded(&path, 1).ends_with(": 1 file"));
    }

    #[test]
    fn a_gating_file_says_why_it_was_imported() {
        let path = PathBuf::from("/w/gating.omiqgt");
        assert_eq!(
            gating_loaded(&path, 40, GatingLoad::Chosen),
            "Loaded 40 gates from gating.omiqgt"
        );
        let again = gating_loaded(&path, 40, GatingLoad::NewMetadata);
        assert!(
            again.contains("again") && again.contains("new metadata"),
            "{again}"
        );
        let waited = gating_loaded(&path, 40, GatingLoad::Waited);
        assert!(waited.contains("waiting"), "{waited}");
    }

    #[test]
    fn a_gating_file_that_waits_says_what_for() {
        let said = gating_waiting(&PathBuf::from("/w/gating.omiqgt"), "the scaling");
        assert!(
            said.starts_with("gating.omiqgt is not imported yet"),
            "{said}"
        );
        assert!(said.contains("waits for the scaling"), "{said}");
    }
}
