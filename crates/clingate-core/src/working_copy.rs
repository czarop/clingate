//! The working copy: what the plots show, what every edit changes, and what
//! Save writes - one implementation for the app and for the tools for Claude,
//! so the two cannot come to behave differently.
//!
//! Every edit - a gate drawn, dragged, deleted or linked, a label moved, a
//! whole rules run, a change of scaling - is one step of a [`History`]: undo
//! steps back through them, redo forward again. Save writes the working copy
//! to the workspace folder (`clingate_gating.omiqgt` and the scaling beside
//! it) and it becomes the saved copy: what an export writes and what Revert
//! goes back to. Loading a gating file replaces both.
//!
//! While there are unsaved changes a recovery copy is kept in the folder, so
//! a crash or a close without saving loses nothing: the workspace offers the
//! changes back the next time it opens.
//!
//! A [`WorkingCopy`] holds the history and the saved copy; the state itself
//! stays where its owner keeps it - the app's stores, the session's fields -
//! and is handed in and out as a [`WorkingState`].

use std::path::{Path, PathBuf};

use crate::axis_store::{AxisStore, ScalingInfoSource, read_axis_configs};
use crate::gates::GateState;
use crate::history::History;
use crate::omiq::metadata::MetaDataFileMap;
use crate::workspace::{GatingFiles, Remembered};

/// One state of the working copy: the gates, and the axes they are drawn in.
/// Together, because a change of scaling moves every gate on the channel.
#[derive(Clone)]
pub struct WorkingState {
    pub gates: GateState,
    pub axes: AxisStore,
}

impl WorkingState {
    /// Whether `self` differs from `earlier` in anything an edit changes.
    ///
    /// By identity of the gates, not their contents: every edit replaces the
    /// gates it touches. Turning a gate to a plot's axes also replaces it, so
    /// a view counts as a change here - which is why callers compare around
    /// an edit, never around a view.
    pub fn changed_since(&self, earlier: &WorkingState) -> bool {
        !self.gates.unchanged_since(&earlier.gates) || self.axes.settings != earlier.axes.settings
    }
}

/// Why a working-copy action did nothing, said so a person - or a model - can
/// act on it.
pub type Refused = String;

/// Where the working copy stands: what the app's edit bar shows and what the
/// tools for Claude report, from the one function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Standing {
    pub unsaved_changes: bool,
    /// How many steps undo can go back.
    pub undo_steps: usize,
    pub can_redo: bool,
    /// Whether there is a saved copy - a gating file has been loaded.
    pub has_saved_copy: bool,
}

/// The working copy's history and saved copy.
#[derive(Default)]
pub struct WorkingCopy {
    history: History<WorkingState>,
    saved: Option<WorkingState>,
}

impl WorkingCopy {
    // ── starting ─────────────────────────────────────────────────────────

    /// A gating file has just been loaded - or a workspace opened on its
    /// saved copy: it is both the saved copy and the working copy, and there
    /// is nothing to undo.
    pub fn loaded(&mut self, current: &WorkingState) {
        self.history.reset();
        self.saved = Some(current.clone());
    }

    /// Everything was cleared for a new workspace.
    pub fn cleared(&mut self) {
        self.history.reset();
        self.saved = None;
    }

    // ── recording edits ──────────────────────────────────────────────────

    /// A one-off edit is done: one step, if it changed anything. Returns
    /// whether it was.
    pub fn record(&mut self, before: WorkingState, now: &WorkingState) -> bool {
        if now.changed_since(&before) {
            self.history.record(before);
            true
        } else {
            false
        }
    }

    /// An edit in progress, such as a drag: call on every move; only the
    /// first call's state is kept.
    pub fn begin(&mut self, before: impl FnOnce() -> WorkingState) {
        self.history.begin(before);
    }

    /// The edit in progress is finished: one step. Returns whether there was
    /// one.
    pub fn commit(&mut self) -> bool {
        self.history.commit()
    }

    // ── undo, redo, revert ───────────────────────────────────────────────

    /// The state to put back for an undo, given the current one; `None` with
    /// nothing to undo.
    pub fn undo(&mut self, current: WorkingState) -> Option<WorkingState> {
        self.history.undo(current)
    }

    /// The state to put back for a redo; `None` with nothing to redo.
    pub fn redo(&mut self, current: WorkingState) -> Option<WorkingState> {
        self.history.redo(current)
    }

    /// The saved copy, to put back as the working copy - itself a step, so it
    /// can be undone. Refused with nothing saved, or nothing unsaved.
    pub fn revert(&mut self, current: WorkingState) -> Result<WorkingState, Refused> {
        let Some(saved) = self.saved.clone() else {
            return Err("there is no saved copy to go back to: no gating file is loaded".into());
        };
        if !self.is_dirty() {
            return Err("there are no unsaved changes to go back from".into());
        }
        self.history.reverted(current);
        Ok(saved)
    }

    // ── what is shown ────────────────────────────────────────────────────

    pub fn standing(&self) -> Standing {
        Standing {
            unsaved_changes: self.is_dirty(),
            undo_steps: self.undo_steps(),
            can_redo: self.can_redo(),
            has_saved_copy: self.has_saved(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo_steps(&self) -> usize {
        self.history.undo_steps()
    }

    /// Whether there is a saved copy to save over, revert to, or export.
    pub fn has_saved(&self) -> bool {
        self.saved.is_some()
    }

    // ── saving and exporting ─────────────────────────────────────────────

    /// Write the working copy into `folder`; it becomes the saved copy, the
    /// recovery copy goes, and the workspace file there - where there is one
    /// - opens on it next time.
    pub fn save(
        &mut self,
        folder: &Path,
        current: &WorkingState,
        metadata: &MetaDataFileMap,
    ) -> Result<GatingFiles, Refused> {
        if self.saved.is_none() {
            return Err("there is nothing to save: no gating file is loaded".into());
        }
        let files = GatingFiles::saved(folder);
        files
            .write(&current.gates, metadata, &current.axes.settings)
            .map_err(|e| format!("{}: {e}", files.gating.display()))?;
        self.history.mark_saved();
        self.saved = Some(current.clone());
        GatingFiles::recovery(folder).remove();
        record_save(folder, &files);
        // A reported gate is where the reviewer means it to be once saved.
        crate::review::report::record_corrections(folder, &current.gates, metadata);
        Ok(files)
    }

    /// Write the saved copy - not the working copy - as an Omiq gating file
    /// at `target`. Says whether there were unsaved changes it left out.
    pub fn export(&self, target: &Path, metadata: &MetaDataFileMap) -> Result<bool, Refused> {
        let Some(saved) = &self.saved else {
            return Err("there is nothing to export: no gating file is loaded".into());
        };
        let own = [
            crate::workspace::SAVED_GATING,
            crate::workspace::RECOVERY_GATING,
        ];
        if target
            .file_name()
            .is_some_and(|n| own.iter().any(|o| n == std::ffi::OsStr::new(o)))
        {
            return Err(format!(
                "{} is where the working copy is kept - Save writes it; export to another name",
                target.display()
            ));
        }
        let document =
            crate::omiq::serialise::to_omiq_document(&saved.gates, metadata, &saved.axes.settings)
                .map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
        crate::workspace::make_parent(target).map_err(|e| e.to_string())?;
        std::fs::write(target, text).map_err(|e| format!("{}: {e}", target.display()))?;
        Ok(self.is_dirty())
    }

    // ── the recovery copy ────────────────────────────────────────────────

    /// What the recovery copy should hold now: the working copy while there
    /// are unsaved changes, nothing otherwise. Pass the answer to
    /// [`keep_recovery`], on whatever thread suits.
    pub fn recovery_wanted(&self) -> bool {
        self.is_dirty()
    }

    /// Take back the unsaved changes an earlier session left in `folder`:
    /// they are returned to become the working copy, unsaved, with nothing to
    /// undo. The saved copy stays as it is.
    pub fn restore_recovery(
        &mut self,
        folder: &Path,
        metadata: &MetaDataFileMap,
    ) -> Result<WorkingState, Refused> {
        let state = read_gating(&GatingFiles::recovery(folder), metadata)?;
        self.history.reset_unsaved();
        Ok(state)
    }
}

/// Write the recovery copy of `current` into `folder`, or remove it when
/// `wanted` is false - see [`WorkingCopy::recovery_wanted`].
pub fn keep_recovery(
    folder: &Path,
    wanted: bool,
    current: &WorkingState,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    let files = GatingFiles::recovery(folder);
    if wanted {
        files.write(&current.gates, metadata, &current.axes.settings)
    } else {
        files.remove();
        Ok(())
    }
}

/// When unsaved changes an earlier session left in `folder` were last
/// written, if there are some.
pub fn recovery_waiting(folder: &Path) -> Option<std::time::SystemTime> {
    let files = GatingFiles::recovery(folder);
    if files.exist() {
        files.modified()
    } else {
        None
    }
}

/// Throw away the unsaved changes an earlier session left in `folder`.
pub fn discard_recovery(folder: &Path) {
    GatingFiles::recovery(folder).remove();
}

/// Read a gating file and the scaling saved with it.
pub fn read_gating(
    files: &GatingFiles,
    metadata: &MetaDataFileMap,
) -> Result<WorkingState, Refused> {
    let configs = read_axis_configs(files.scaling.clone(), ScalingInfoSource::Omiq)
        .map_err(|e| format!("{}: {e}", files.scaling.display()))?;
    let mut axes = AxisStore::default();
    axes.replace_axis_configs(configs);
    let gates = GateState::from_gating_file(files.gating.clone(), metadata, axes.settings.clone())
        .map_err(|e| format!("{}: {e}", files.gating.display()))?;
    Ok(WorkingState { gates, axes })
}

/// Point the workspace file in `folder`, where there is one, at the files
/// just saved, so the folder opens on them. Without one there is nothing to
/// point: a folder opened afresh finds the saved copy by its name.
fn record_save(folder: &Path, files: &GatingFiles) {
    if let Ok(Some(mut remembered)) = Remembered::load_from_folder(folder) {
        remembered.gating = Some(files.gating.clone());
        remembered.scaling = Some(files.scaling.clone());
        if let Err(e) = remembered.save_into_folder() {
            tracing::warn!("the workspace file could not be pointed at the save: {e}");
        }
    }
}

/// Where a working copy's folder is, for callers that know its parts rather
/// than its folder: the workspace folder, or the gating file's own.
pub fn folder_of(workspace: Option<&Path>, gating: Option<&Path>) -> Option<PathBuf> {
    workspace
        .map(Path::to_path_buf)
        .or_else(|| gating.and_then(Path::parent).map(Path::to_path_buf))
}
