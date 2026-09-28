//! The app's end of the working copy - see [`clingate_core::working_copy`],
//! which holds every decision: what counts as an edit, undo, redo, revert,
//! save, export and the recovery copy. The tools for Claude use the same
//! type, so the two behave alike; this file only moves state between the
//! stores and it, and tells the tabs when their gates were put back.
//!
//! Views of a gate on axes the other way round turn it in the store, but that
//! is not an edit: it is not a step, and it does not make the copy unsaved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::GateStateStoreExt;
use clingate_core::omiq::metadata::MetaDataStoreStoreExt;
use clingate_core::working_copy::{self, WorkingCopy};
use dioxus::prelude::*;

pub use clingate_core::working_copy::WorkingState;

use crate::gate_editor::workspace_window::{
    AxesStore, GateStore, Generation, Loaded, MetadataStore, Part,
};

/// The working copy's controls, shared by every tab.
#[derive(Clone, Copy)]
pub struct Edits {
    gates: GateStore,
    axes: AxesStore,
    metadata: MetadataStore,
    loaded: Signal<Loaded>,
    generation: Signal<Generation>,
    working: Signal<WorkingCopy>,
    /// Unsaved changes an earlier session left in the folder, and when they
    /// were last written, while they are on offer.
    offer: Signal<Option<std::time::SystemTime>>,
}

/// The latest recovery write asked for; an older one still running skips
/// its write, so a slow write cannot land after a newer one.
static RECOVERY_WRITE: AtomicU64 = AtomicU64::new(0);

impl Edits {
    /// Made once, by the shell, for every tab to share.
    pub fn provide(
        gates: GateStore,
        axes: AxesStore,
        metadata: MetadataStore,
        loaded: Signal<Loaded>,
        generation: Signal<Generation>,
    ) -> Self {
        Self {
            gates,
            axes,
            metadata,
            loaded,
            generation,
            working: Signal::new(WorkingCopy::default()),
            offer: Signal::new(None),
        }
    }

    fn snapshot(&self) -> WorkingState {
        WorkingState {
            gates: self.gates.peek().clone(),
            axes: self.axes.peek().clone(),
        }
    }

    // ── recording edits ──────────────────────────────────────────────────

    /// The state before a one-off edit, to hand to [`Edits::after`].
    pub fn before(&self) -> WorkingState {
        self.snapshot()
    }

    /// A one-off edit is done: one step, if it changed anything.
    pub fn after(mut self, before: WorkingState) {
        let now = self.snapshot();
        if self.working.write().record(before, &now) {
            self.keep_recovery();
        }
    }

    /// An edit in progress, such as a drag: call on every move; the first
    /// call keeps the state before it.
    pub fn begin(mut self) {
        let this = self;
        self.working.write().begin(|| this.snapshot());
    }

    /// The edit in progress is finished: it becomes one step.
    pub fn commit(mut self) {
        if self.working.write().commit() {
            self.keep_recovery();
        }
    }

    // ── undo, redo, revert ───────────────────────────────────────────────

    pub fn undo(mut self) -> bool {
        let current = self.snapshot();
        let previous = self.working.write().undo(current);
        let done = previous.is_some();
        if let Some(previous) = previous {
            self.restore(previous);
            self.keep_recovery();
        }
        done
    }

    pub fn redo(mut self) -> bool {
        let current = self.snapshot();
        let next = self.working.write().redo(current);
        let done = next.is_some();
        if let Some(next) = next {
            self.restore(next);
            self.keep_recovery();
        }
        done
    }

    /// Put the working copy back to the saved copy - itself a step, so it
    /// can be undone.
    pub fn revert(mut self) -> Result<(), String> {
        let current = self.snapshot();
        let saved = self.working.write().revert(current)?;
        self.restore(saved);
        self.keep_recovery();
        Ok(())
    }

    /// Put a state back as the working copy.
    ///
    /// The selected gate stays selected if it is still there. Views re-match
    /// their gates to their axes (`Generation::restored`); and if gates came
    /// or went, the tabs are told the document changed, since a plot may be
    /// drawn through a gate that no longer exists.
    fn restore(mut self, state: WorkingState) {
        let nodes = |g: &GateState| -> HashSet<String> {
            g.placements()
                .map(|(n, _)| n.as_str().to_string())
                .collect()
        };
        let same_tree = nodes(&self.gates.peek()) == nodes(&state.gates);
        let selected = self.gates.selected_gate().peek().clone();
        let keep = selected.filter(|id| state.gates.is_registered(id));

        self.axes.set(state.axes);
        self.gates.set(state.gates);
        *self.gates.selected_gate().write() = keep;

        let mut generation = self.generation.write();
        generation.restored += 1;
        if !same_tree {
            generation.document += 1;
        }
    }

    // ── what the buttons read ────────────────────────────────────────────

    pub fn is_dirty(&self) -> bool {
        self.working.read().is_dirty()
    }

    pub fn can_undo(&self) -> bool {
        self.working.read().can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.working.read().can_redo()
    }

    /// Whether there is a saved copy to save over, revert to, or export.
    pub fn has_saved(&self) -> bool {
        self.working.read().has_saved()
    }

    /// Unsaved changes, read without subscribing - for a decision taken in a
    /// handler rather than something drawn.
    pub fn dirty_now(&self) -> bool {
        self.working.peek().is_dirty()
    }

    /// Where the working copy stands, read without subscribing - the same
    /// report the tools for Claude give.
    pub fn standing_now(&self) -> working_copy::Standing {
        self.working.peek().standing()
    }

    /// Whether a saved copy exists, read without subscribing.
    pub fn has_saved_now(&self) -> bool {
        self.working.peek().has_saved()
    }

    // ── loading and saving ───────────────────────────────────────────────

    /// A gating file has just been loaded: it is both the saved copy and the
    /// working copy, and there is nothing to undo.
    pub fn loaded_fresh(mut self) {
        let now = self.snapshot();
        self.working.write().loaded(&now);
    }

    /// Everything was cleared for a new workspace.
    pub fn cleared(mut self) {
        self.working.write().cleared();
        self.offer.set(None);
    }

    /// The folder the working copy is saved in: the workspace's, or the one
    /// the gating file came from when files were chosen one by one.
    pub(crate) fn folder(&self) -> Option<PathBuf> {
        let loaded = self.loaded.peek();
        working_copy::folder_of(loaded.folder.as_deref(), loaded.gating.path())
    }

    /// Write the working copy into the workspace folder; it becomes the saved
    /// copy, and the file the workspace opens next time.
    pub fn save(mut self) -> Result<PathBuf, String> {
        let folder = self
            .folder()
            .ok_or("there is no workspace folder to save into")?;
        let state = self.snapshot();
        let metadata = self.metadata.metadata().peek().clone();
        let files = self.working.write().save(&folder, &state, &metadata)?;
        let mut loaded = self.loaded.write();
        loaded.gating = Part::Loaded(files.gating.clone());
        loaded.scaling = Part::Loaded(files.scaling.clone());
        Ok(files.gating)
    }

    /// Write the saved copy - not the working copy - as an Omiq gating file.
    /// Says whether there were unsaved changes it left out.
    pub fn export(&self, target: &Path) -> Result<bool, String> {
        let metadata = self.metadata.metadata().peek().clone();
        self.working.peek().export(target, &metadata)
    }

    // ── the recovery copy ────────────────────────────────────────────────

    /// Keep the recovery copy up to date with the working copy, off the UI
    /// thread.
    fn keep_recovery(self) {
        let Some(folder) = self.folder() else {
            return;
        };
        let turn = RECOVERY_WRITE.fetch_add(1, Ordering::SeqCst) + 1;
        let wanted = self.working.peek().recovery_wanted();
        let state = self.snapshot();
        let metadata = self.metadata.metadata().peek().clone();
        spawn(async move {
            let written = tokio::task::spawn_blocking(move || {
                if RECOVERY_WRITE.load(Ordering::SeqCst) != turn {
                    return Ok(());
                }
                working_copy::keep_recovery(&folder, wanted, &state, &metadata)
            })
            .await;
            match written {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("the recovery copy could not be written: {e}"),
                Err(e) => tracing::warn!("the recovery copy could not be written: {e}"),
            }
        });
    }

    /// Look for unsaved changes an earlier session left in the workspace
    /// folder, and offer them if there are some. Called once a workspace has
    /// opened.
    pub fn check_recovery(mut self) {
        let found = self
            .folder()
            .and_then(|folder| working_copy::recovery_waiting(&folder));
        self.offer.set(found);
    }

    /// When the unsaved changes on offer were last written.
    pub fn recovery_offer(&self) -> Option<std::time::SystemTime> {
        *self.offer.read()
    }

    /// The same, read without subscribing.
    pub fn recovery_offer_now(&self) -> Option<std::time::SystemTime> {
        *self.offer.peek()
    }

    /// Take back the unsaved changes an earlier session left: they become the
    /// working copy, unsaved; the saved copy stays as it is.
    pub fn restore_recovery(mut self) -> Result<(), String> {
        let folder = self.folder().ok_or("there is no workspace folder")?;
        let metadata = self.metadata.metadata().peek().clone();
        let state = self.working.write().restore_recovery(&folder, &metadata)?;
        self.axes.set(state.axes);
        self.gates.set(state.gates);
        self.offer.set(None);
        let mut generation = self.generation.write();
        generation.document += 1;
        generation.restored += 1;
        Ok(())
    }

    /// Throw away the unsaved changes an earlier session left.
    pub fn discard_recovery(mut self) {
        if let Some(folder) = self.folder() {
            working_copy::discard_recovery(&folder);
        }
        self.offer.set(None);
    }
}

/// Save, undo, redo and revert, with whether the working copy is saved - and,
/// when an earlier session left unsaved changes, the offer to take them back.
/// The same bar on every tab that edits.
#[component]
pub fn EditBar() -> Element {
    use crate::components::toast::{say, use_toast, warn};
    let edits = use_context::<Edits>();
    let toasts = use_toast();
    let (dirty, saved) = (edits.is_dirty(), edits.has_saved());
    let offer = edits.recovery_offer();
    rsx! {
        div { class: "edit-bar",
            button {
                class: "edit-bar_save",
                disabled: !saved || !dirty,
                title: "Write the working copy into the workspace folder",
                onclick: move |_| match edits.save() {
                    Ok(path) => say(&toasts, format!("Saved to {}", path.display())),
                    Err(e) => warn(&toasts, format!("Not saved: {e}")),
                },
                "Save"
            }
            button {
                disabled: !edits.can_undo(),
                onclick: move |_| {
                    edits.undo();
                },
                "Undo"
            }
            button {
                disabled: !edits.can_redo(),
                onclick: move |_| {
                    edits.redo();
                },
                "Redo"
            }
            button {
                disabled: !saved || !dirty,
                title: "Go back to what was last saved - Undo brings the changes back",
                onclick: move |_| {
                    if let Err(e) = edits.revert() {
                        warn(&toasts, e);
                    }
                },
                "Revert"
            }
            span { class: if dirty { "edit-bar_state edit-bar_dirty" } else { "edit-bar_state" },
                if !saved {
                    ""
                } else if dirty {
                    "Unsaved changes"
                } else {
                    "Saved"
                }
            }
            if let Some(when) = offer {
                span { class: "edit-bar_offer",
                    "Unsaved changes from {ago(when)} were left in this workspace."
                    button {
                        onclick: move |_| match edits.restore_recovery() {
                            Ok(()) => say(&toasts, "The unsaved changes are back - Save to keep them"),
                            Err(e) => warn(&toasts, format!("They could not be restored: {e}")),
                        },
                        "Restore"
                    }
                    button { onclick: move |_| edits.discard_recovery(), "Discard" }
                }
            }
        }
    }
}

/// How long ago, in words.
fn ago(when: std::time::SystemTime) -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(when)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match seconds {
        0..60 => "moments ago".to_string(),
        60..3600 => format!("{} minutes ago", seconds / 60),
        3600..86_400 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}
