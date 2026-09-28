//! The working copy, for the tools for Claude: undo, redo, revert, save,
//! export and the recovery copy.
//!
//! Every one of these is a call into [`crate::working_copy::WorkingCopy`],
//! the same type the app's Save, Undo, Redo and Revert buttons call, so the
//! tools and the app cannot come to behave differently. What is here is only
//! moving the session's gates and axes in and out, and saying what happened.

use std::path::PathBuf;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::working_copy::{self, WorkingState};

/// Where the working copy stands, after any of these.
#[derive(Debug, Clone, Serialize)]
pub struct EditState {
    pub unsaved_changes: bool,
    /// How many steps undo can go back.
    pub undo_steps: usize,
    pub can_redo: bool,
    /// Whether there is a saved copy - a gating file has been loaded.
    pub has_saved_copy: bool,
    /// Unsaved changes an earlier session - the app's or this one's - left
    /// in the folder, not yet restored or discarded.
    pub earlier_unsaved_changes: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Saved {
    pub gating_file: PathBuf,
    pub scaling_file: PathBuf,
    pub edits: EditState,
}

#[derive(Debug, Clone, Serialize)]
pub struct Exported {
    pub file: PathBuf,
    /// The export is the saved copy: unsaved changes are not in it.
    pub unsaved_changes_left_out: bool,
}

impl Session {
    pub(crate) fn working_state(&self) -> WorkingState {
        WorkingState {
            gates: self.gates.clone(),
            axes: self.axes.clone(),
        }
    }

    fn put_back(&mut self, state: WorkingState) {
        self.gates = state.gates;
        self.axes = state.axes;
        // A preview measured on the gates as they were.
        self.pending = None;
    }

    /// An edit is done: one step if it changed anything, and the recovery
    /// copy kept up to date - as the app does after every edit.
    pub(crate) fn edited(&mut self, before: WorkingState) {
        let now = self.working_state();
        if self.working.record(before, &now) {
            self.keep_recovery();
        }
    }

    fn keep_recovery(&self) {
        let wanted = self.working.recovery_wanted();
        if let Err(e) = working_copy::keep_recovery(
            &self.folder,
            wanted,
            &self.working_state(),
            self.metadata.metadata(),
        ) {
            tracing::warn!("the recovery copy could not be written: {e}");
        }
    }

    pub fn edit_state(&self) -> EditState {
        let standing = self.working.standing();
        EditState {
            unsaved_changes: standing.unsaved_changes,
            undo_steps: standing.undo_steps,
            can_redo: standing.can_redo,
            has_saved_copy: standing.has_saved_copy,
            earlier_unsaved_changes: working_copy::recovery_waiting(&self.folder).is_some()
                && !standing.unsaved_changes,
        }
    }

    /// The working copy's standing, as the app's edit bar reads it.
    pub fn standing(&self) -> working_copy::Standing {
        self.working.standing()
    }

    /// Step the working copy back one edit.
    pub fn undo(&mut self) -> Result<EditState, Refusal> {
        let current = self.working_state();
        let previous = self
            .working
            .undo(current)
            .ok_or_else(|| failed("there is nothing to undo"))?;
        self.put_back(previous);
        self.keep_recovery();
        Ok(self.edit_state())
    }

    /// Step forward again after an undo.
    pub fn redo(&mut self) -> Result<EditState, Refusal> {
        let current = self.working_state();
        let next = self
            .working
            .redo(current)
            .ok_or_else(|| failed("there is nothing to redo"))?;
        self.put_back(next);
        self.keep_recovery();
        Ok(self.edit_state())
    }

    /// Put the working copy back to the last save; undo brings the changes
    /// back.
    pub fn revert(&mut self) -> Result<EditState, Refusal> {
        let current = self.working_state();
        let saved = self.working.revert(current).map_err(failed)?;
        self.put_back(saved);
        self.keep_recovery();
        Ok(self.edit_state())
    }

    /// Save the working copy into the workspace folder, as the app's Save
    /// does: it becomes the saved copy, and the workspace opens on it.
    pub fn save(&mut self) -> Result<Saved, Refusal> {
        let current = self.working_state();
        let folder = self.folder.clone();
        let files = self
            .working
            .save(&folder, &current, self.metadata.metadata())
            .map_err(failed)?;
        self.parts.gating = super::PartState::Loaded {
            file: files.gating.clone(),
        };
        self.parts.scaling = super::PartState::Loaded {
            file: files.scaling.clone(),
        };
        Ok(Saved {
            gating_file: files.gating,
            scaling_file: files.scaling,
            edits: self.edit_state(),
        })
    }

    /// Export the saved copy as an Omiq gating file named `file_name`, in the
    /// workspace folder - as the app's Export does with a bare name. An
    /// existing file is never replaced unless `overwrite` says so.
    pub fn export(&self, file_name: &str, overwrite: bool) -> Result<Exported, Refusal> {
        let name = file_name.trim();
        let plain = std::path::Path::new(name)
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new(name));
        if name.is_empty() || !plain || name.starts_with('.') {
            return Err(failed(
                "give a file name alone, with no folder: it is written into the workspace folder",
            ));
        }
        let path = crate::workspace::export_target(Some(&self.folder), name);
        if path.exists() && !overwrite {
            return Err(failed(format!(
                "{} already exists: ask the user whether to replace it, or choose another name",
                path.display()
            )));
        }
        let left_out = self
            .working
            .export(&path, self.metadata.metadata())
            .map_err(failed)?;
        Ok(Exported {
            file: path,
            unsaved_changes_left_out: left_out,
        })
    }

    /// Take back the unsaved changes an earlier session left in the folder.
    pub fn restore_unsaved_changes(&mut self) -> Result<EditState, Refusal> {
        let folder = self.folder.clone();
        let state = self
            .working
            .restore_recovery(&folder, self.metadata.metadata())
            .map_err(failed)?;
        self.put_back(state);
        Ok(self.edit_state())
    }

    /// Throw away the unsaved changes an earlier session left in the folder.
    pub fn discard_unsaved_changes(&mut self) -> Result<EditState, Refusal> {
        if working_copy::recovery_waiting(&self.folder).is_none() {
            return Err(failed(
                "there are no unsaved changes from an earlier session",
            ));
        }
        working_copy::discard_recovery(&self.folder);
        Ok(self.edit_state())
    }
}
