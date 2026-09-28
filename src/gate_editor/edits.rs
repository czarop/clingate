//! The working copy: what the plots show, what every edit changes, and what
//! Save writes.
//!
//! Every edit - a gate drawn, dragged, deleted or linked, a label moved, a
//! whole rules run, a change of scaling - is one step in a
//! [`History`]: undo steps back through them, redo forward again. Save writes
//! the working copy to the workspace folder (`clingate_gating.omiqgt` and the
//! scaling beside it) and it becomes the saved copy, which is what an export
//! writes and what Revert goes back to. Loading a gating file replaces both.
//!
//! As the working copy is edited a recovery copy is kept in the folder, so a
//! crash or a close without saving loses nothing: the next time the
//! workspace opens, it offers the changes back.
//!
//! Views of a gate on axes the other way round turn it in the store, but that
//! is not an edit: it is not a step, and it does not make the copy unsaved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clingate_core::axis_store::{AxisStore, ScalingInfoSource, read_axis_configs};
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::GateStateStoreExt;
use clingate_core::history::History;
use clingate_core::omiq::metadata::MetaDataStoreStoreExt;
use clingate_core::workspace::GatingFiles;
use dioxus::prelude::*;

use crate::gate_editor::workspace_window::{
    AxesStore, GateStore, Generation, Loaded, MetadataStore, Part,
};

/// One state of the working copy: the gates, and the axes they are drawn in.
/// Together, because a change of scaling moves every gate on the channel.
#[derive(Clone)]
pub struct WorkingState {
    pub gates: GateState,
    pub axes: AxisStore,
}

/// The working copy's controls, shared by every tab.
#[derive(Clone, Copy)]
pub struct Edits {
    gates: GateStore,
    axes: AxesStore,
    metadata: MetadataStore,
    loaded: Signal<Loaded>,
    generation: Signal<Generation>,
    history: Signal<History<WorkingState>>,
    /// The last saved state, for an export and for Revert. `None` until a
    /// gating file is loaded.
    saved: Signal<Option<WorkingState>>,
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
            history: Signal::new(History::default()),
            saved: Signal::new(None),
            offer: Signal::new(None),
        }
    }

    fn snapshot(&self) -> WorkingState {
        WorkingState {
            gates: self.gates.peek().clone(),
            axes: self.axes.peek().clone(),
        }
    }

    fn changed_since(&self, before: &WorkingState) -> bool {
        !self.gates.peek().unchanged_since(&before.gates)
            || self.axes.peek().settings != before.axes.settings
    }

    // ── recording edits ──────────────────────────────────────────────────

    /// The state before a one-off edit, to hand to [`Edits::after`].
    pub fn before(&self) -> WorkingState {
        self.snapshot()
    }

    /// A one-off edit is done: one step, if it changed anything.
    pub fn after(mut self, before: WorkingState) {
        if self.changed_since(&before) {
            self.history.write().record(before);
            self.edited();
        }
    }

    /// An edit in progress, such as a drag: call on every move; the first
    /// call keeps the state before it.
    pub fn begin(mut self) {
        let this = self;
        self.history.write().begin(|| this.snapshot());
    }

    /// The edit in progress is finished: it becomes one step.
    pub fn commit(mut self) {
        let committed = self.history.write().commit();
        if committed {
            self.edited();
        }
    }

    fn edited(self) {
        self.keep_recovery();
    }

    // ── undo, redo, revert ───────────────────────────────────────────────

    pub fn undo(mut self) {
        let current = self.snapshot();
        let previous = self.history.write().undo(current);
        if let Some(previous) = previous {
            self.restore(previous);
            self.keep_recovery();
        }
    }

    pub fn redo(mut self) {
        let current = self.snapshot();
        let next = self.history.write().redo(current);
        if let Some(next) = next {
            self.restore(next);
            self.keep_recovery();
        }
    }

    /// Put the working copy back to the saved copy - itself a step, so it
    /// can be undone.
    pub fn revert(mut self) {
        let Some(saved) = self.saved.peek().clone() else {
            return;
        };
        if !self.is_dirty() {
            return;
        }
        let current = self.snapshot();
        self.history.write().reverted(current);
        self.restore(saved);
        self.keep_recovery();
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
        self.history.read().is_dirty()
    }

    pub fn can_undo(&self) -> bool {
        self.history.read().can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.read().can_redo()
    }

    /// Whether there is a saved copy to save over, revert to, or export.
    pub fn has_saved(&self) -> bool {
        self.saved.read().is_some()
    }

    /// Unsaved changes, read without subscribing - for a decision taken in a
    /// handler rather than something drawn.
    pub fn dirty_now(&self) -> bool {
        self.history.peek().is_dirty()
    }

    // ── loading and saving ───────────────────────────────────────────────

    /// A gating file has just been loaded: it is both the saved copy and the
    /// working copy, and there is nothing to undo.
    pub fn loaded_fresh(mut self) {
        self.history.write().reset();
        let now = self.snapshot();
        self.saved.set(Some(now));
    }

    /// Everything was cleared for a new workspace.
    pub fn cleared(mut self) {
        self.history.write().reset();
        self.saved.set(None);
        self.offer.set(None);
    }

    /// The folder the working copy is saved in: the workspace's, or the one
    /// the gating file came from when files were chosen one by one.
    fn folder(&self) -> Option<PathBuf> {
        let loaded = self.loaded.peek();
        loaded.folder.clone().or_else(|| {
            loaded
                .gating
                .path()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
    }

    /// Write the working copy into the workspace folder; it becomes the saved
    /// copy, and the file the workspace opens next time.
    pub async fn save(mut self) -> Result<PathBuf, String> {
        if self.saved.peek().is_none() {
            return Err("there is nothing to save: no gating file is loaded".into());
        }
        let folder = self
            .folder()
            .ok_or("there is no workspace folder to save into")?;
        let files = GatingFiles::saved(&folder);
        let state = self.snapshot();
        let metadata = self.metadata.metadata().peek().clone();
        let (gates, settings) = (state.gates.clone(), state.axes.settings.clone());
        let writing = files.clone();
        tokio::task::spawn_blocking(move || writing.write(&gates, &metadata, &settings))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

        {
            let mut loaded = self.loaded.write();
            loaded.gating = Part::Loaded(files.gating.clone());
            loaded.scaling = Part::Loaded(files.scaling.clone());
        }
        self.history.write().mark_saved();
        self.saved.set(Some(state));
        GatingFiles::recovery(&folder).remove();
        Ok(files.gating)
    }

    /// Write the saved copy - not the working copy - as an Omiq gating file.
    /// Says whether there were unsaved changes it left out.
    pub fn export(&self, target: &Path) -> Result<bool, String> {
        let saved = self.saved.peek();
        let Some(saved) = saved.as_ref() else {
            return Err("there is nothing to export: no gating file is loaded".into());
        };
        let document = clingate_core::omiq::serialise::to_omiq_document(
            &saved.gates,
            &self.metadata.metadata().peek(),
            &saved.axes.settings,
        )
        .map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
        std::fs::write(target, text).map_err(|e| e.to_string())?;
        Ok(self.dirty_now())
    }

    // ── the recovery copy ────────────────────────────────────────────────

    /// Keep the recovery copy up to date with the working copy: written
    /// while there are unsaved changes, removed when there are none.
    fn keep_recovery(self) {
        let Some(folder) = self.folder() else {
            return;
        };
        let files = GatingFiles::recovery(&folder);
        let turn = RECOVERY_WRITE.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.dirty_now() {
            files.remove();
            return;
        }
        let state = self.snapshot();
        let metadata = self.metadata.metadata().peek().clone();
        spawn(async move {
            let written = tokio::task::spawn_blocking(move || {
                if RECOVERY_WRITE.load(Ordering::SeqCst) != turn {
                    return Ok(());
                }
                files.write(&state.gates, &metadata, &state.axes.settings)
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
        let found = self.folder().and_then(|folder| {
            let files = GatingFiles::recovery(&folder);
            if files.exist() {
                files.modified()
            } else {
                None
            }
        });
        self.offer.set(found);
    }

    /// When the unsaved changes on offer were last written.
    pub fn recovery_offer(&self) -> Option<std::time::SystemTime> {
        *self.offer.read()
    }

    /// Take back the unsaved changes an earlier session left: they become the
    /// working copy, unsaved; the saved copy stays as it is.
    pub async fn restore_recovery(mut self) -> Result<(), String> {
        let folder = self.folder().ok_or("there is no workspace folder")?;
        let files = GatingFiles::recovery(&folder);
        let metadata = self.metadata.metadata().peek().clone();
        let (axes, gates) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let mut axes = AxisStore::default();
            axes.replace_axis_configs(read_axis_configs(
                files.scaling.clone(),
                ScalingInfoSource::Omiq,
            )?);
            let gates = GateState::from_gating_file(
                files.gating.clone(),
                &metadata,
                axes.settings.clone(),
            )?;
            Ok((axes, gates))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

        self.axes.set(axes);
        self.gates.set(gates);
        self.history.write().reset_unsaved();
        self.offer.set(None);
        let mut generation = self.generation.write();
        generation.document += 1;
        generation.restored += 1;
        Ok(())
    }

    /// Throw away the unsaved changes an earlier session left.
    pub fn discard_recovery(mut self) {
        if let Some(folder) = self.folder() {
            GatingFiles::recovery(&folder).remove();
        }
        self.offer.set(None);
    }

    /// Whether a saved copy exists, read without subscribing.
    pub fn has_saved_now(&self) -> bool {
        self.saved.peek().is_some()
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
                onclick: move |_| {
                    spawn(async move {
                        match edits.save().await {
                            Ok(path) => say(&toasts, format!("Saved to {}", path.display())),
                            Err(e) => warn(&toasts, format!("Not saved: {e}")),
                        }
                    });
                },
                "Save"
            }
            button {
                disabled: !edits.can_undo(),
                onclick: move |_| edits.undo(),
                "Undo"
            }
            button {
                disabled: !edits.can_redo(),
                onclick: move |_| edits.redo(),
                "Redo"
            }
            button {
                disabled: !saved || !dirty,
                title: "Go back to what was last saved - Undo brings the changes back",
                onclick: move |_| edits.revert(),
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
                        onclick: move |_| {
                            spawn(async move {
                                match edits.restore_recovery().await {
                                    Ok(()) => say(&toasts, "The unsaved changes are back - Save to keep them"),
                                    Err(e) => warn(&toasts, format!("They could not be restored: {e}")),
                                }
                            });
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
