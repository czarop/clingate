//! The working copy's history: undo, redo, and whether it has been saved.
//!
//! The gates on the plots are a working copy. Every edit - a drag, a gate
//! added or deleted, a whole rules run, a change of scaling - is one step:
//! the state before it is kept, up to [`UNDO_LIMIT`] of them, and undoing
//! puts the working copy back to the last. Saving writes the working copy to
//! the workspace folder; the saved copy is what an export writes.
//!
//! The states are cheap to keep: a gate is shared between states, not copied,
//! so a step holds only the maps that say which gate is where.
//!
//! Generic over the state so it is testable on its own; the app keeps its
//! gates and axes in it.

use std::collections::VecDeque;

/// How many steps back undo can go.
pub const UNDO_LIMIT: usize = 20;

/// A state of the working copy, with the version it was when it was taken.
#[derive(Clone, Debug)]
struct Step<T> {
    state: T,
    version: u64,
}

/// The undo and redo stacks, and which version of the working copy is saved.
///
/// Each edit gives the working copy a new version; undo and redo return it to
/// the version it had. So "unsaved changes" is a comparison of versions, and
/// undoing back to the saved state is correctly no change at all.
#[derive(Debug)]
pub struct History<T> {
    undo: VecDeque<Step<T>>,
    redo: Vec<Step<T>>,
    /// The state before an edit still in progress - a drag, from press to
    /// release - so the whole of it is one step.
    pending: Option<T>,
    version: u64,
    next_version: u64,
    saved: Option<u64>,
    limit: usize,
}

impl<T: Clone> Default for History<T> {
    fn default() -> Self {
        Self::new(UNDO_LIMIT)
    }
}

impl<T: Clone> History<T> {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            pending: None,
            version: 0,
            next_version: 1,
            saved: Some(0),
            limit: limit.max(1),
        }
    }

    /// A fresh start, as saved: after a workspace or a gating file is opened.
    pub fn reset(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.version = self.fresh_version();
        self.saved = Some(self.version);
    }

    /// A fresh start that is *not* saved: unsaved changes recovered after the
    /// program last closed without saving them.
    pub fn reset_unsaved(&mut self) {
        self.reset();
        self.saved = None;
    }

    fn fresh_version(&mut self) -> u64 {
        let v = self.next_version;
        self.next_version += 1;
        v
    }

    /// Start an edit that will be finished by [`commit`](Self::commit):
    /// `before` is the state as it stands. A second call before the commit
    /// is ignored, so a drag can call this on every move.
    pub fn begin(&mut self, before: impl FnOnce() -> T) {
        if self.pending.is_none() {
            self.pending = Some(before());
        }
    }

    pub fn is_editing(&self) -> bool {
        self.pending.is_some()
    }

    /// Finish the edit begun: it becomes one undo step, redo is cleared, and
    /// the working copy has a new version. Nothing happens if none was begun.
    pub fn commit(&mut self) -> bool {
        let Some(before) = self.pending.take() else {
            return false;
        };
        self.undo.push_back(Step {
            state: before,
            version: self.version,
        });
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.version = self.fresh_version();
        true
    }

    /// Forget the edit begun, as if it had not happened - one that turned
    /// out to change nothing.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// A one-off edit: `before` is the state before it.
    pub fn record(&mut self, before: T) {
        self.pending = Some(before);
        self.commit();
    }

    /// Step back: the state to put the working copy back to, given the one it
    /// is in now, which redo can return to. `None` with nothing to undo.
    pub fn undo(&mut self, current: T) -> Option<T> {
        self.pending = None;
        let step = self.undo.pop_back()?;
        self.redo.push(Step {
            state: current,
            version: self.version,
        });
        self.version = step.version;
        Some(step.state)
    }

    /// Step forward again after an undo. `None` with nothing to redo.
    pub fn redo(&mut self, current: T) -> Option<T> {
        self.pending = None;
        let step = self.redo.pop()?;
        self.undo.push_back(Step {
            state: current,
            version: self.version,
        });
        self.version = step.version;
        Some(step.state)
    }

    /// The working copy has just been saved.
    pub fn mark_saved(&mut self) {
        self.saved = Some(self.version);
    }

    /// The working copy has just been put back to the saved copy: an undoable
    /// step like any other, landing on the saved version.
    pub fn reverted(&mut self, before: T) {
        self.record(before);
        match self.saved {
            Some(saved) => self.version = saved,
            // Recovered changes never had a saved version in this session.
            None => self.saved = Some(self.version),
        }
    }

    /// Whether the working copy differs from what was last saved.
    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.version)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_steps(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A working copy that is just a number, edited by setting it.
    struct Doc {
        value: i32,
        history: History<i32>,
    }

    impl Doc {
        fn new() -> Self {
            Self {
                value: 0,
                history: History::new(3),
            }
        }
        fn edit(&mut self, to: i32) {
            self.history.record(self.value);
            self.value = to;
        }
        fn undo(&mut self) -> bool {
            match self.history.undo(self.value) {
                Some(v) => {
                    self.value = v;
                    true
                }
                None => false,
            }
        }
        fn redo(&mut self) -> bool {
            match self.history.redo(self.value) {
                Some(v) => {
                    self.value = v;
                    true
                }
                None => false,
            }
        }
    }

    #[test]
    fn undo_and_redo_walk_the_edits_back_and_forth() {
        let mut d = Doc::new();
        d.edit(1);
        d.edit(2);
        assert!(d.undo());
        assert_eq!(d.value, 1);
        assert!(d.undo());
        assert_eq!(d.value, 0);
        assert!(!d.undo(), "nothing left to undo");
        assert!(d.redo());
        assert!(d.redo());
        assert_eq!(d.value, 2);
        assert!(!d.redo(), "nothing left to redo");
    }

    #[test]
    fn a_new_edit_after_an_undo_clears_redo() {
        let mut d = Doc::new();
        d.edit(1);
        d.edit(2);
        d.undo();
        d.edit(5);
        assert!(!d.history.can_redo());
        d.undo();
        assert_eq!(d.value, 1);
    }

    #[test]
    fn only_the_last_steps_are_kept() {
        let mut d = Doc::new();
        for v in 1..=5 {
            d.edit(v);
        }
        assert_eq!(d.history.undo_steps(), 3);
        while d.undo() {}
        assert_eq!(d.value, 2, "back three steps from 5");
    }

    #[test]
    fn an_edit_in_progress_is_one_step_however_many_moves_it_takes() {
        let mut d = Doc::new();
        for v in 1..=10 {
            let before = d.value;
            d.history.begin(|| before);
            d.value = v;
        }
        assert!(d.history.commit());
        assert!(!d.history.commit(), "committed once");
        assert_eq!(d.history.undo_steps(), 1);
        d.undo();
        assert_eq!(d.value, 0, "the state before the first move");

        d.history.begin(|| 0);
        d.history.cancel();
        assert!(!d.history.commit(), "a cancelled edit is no step");
    }

    #[test]
    fn unsaved_is_a_matter_of_versions_so_undoing_to_the_save_is_clean() {
        let mut d = Doc::new();
        assert!(!d.history.is_dirty(), "opened as saved");
        d.edit(1);
        assert!(d.history.is_dirty());
        d.history.mark_saved();
        assert!(!d.history.is_dirty());
        d.edit(2);
        assert!(d.history.is_dirty());
        d.undo();
        assert!(!d.history.is_dirty(), "back where it was saved");
        d.undo();
        assert!(d.history.is_dirty(), "before the save");
        d.redo();
        assert!(!d.history.is_dirty());
    }

    #[test]
    fn reverting_to_the_saved_copy_is_clean_and_can_be_undone() {
        let mut d = Doc::new();
        d.edit(1);
        d.history.mark_saved();
        d.edit(2);
        d.edit(3);
        // Revert: the saved copy's value comes back.
        let before = d.value;
        d.history.reverted(before);
        d.value = 1;
        assert!(!d.history.is_dirty());
        assert!(d.undo());
        assert_eq!(d.value, 3, "the revert itself undoes");
        assert!(d.history.is_dirty());
    }

    #[test]
    fn a_reset_starts_clean_and_a_recovered_start_does_not() {
        let mut d = Doc::new();
        d.edit(1);
        d.history.reset();
        assert!(!d.history.is_dirty() && !d.history.can_undo());
        d.history.reset_unsaved();
        assert!(d.history.is_dirty() && !d.history.can_undo());
        d.history.mark_saved();
        assert!(!d.history.is_dirty());
    }
}
