//! A rules run waiting for a person: the gates it could not place, or placed
//! too doubtfully to measure anything under, shown one at a time in the editor
//! on the sample that needs them, until the run goes on or is stopped.

use std::collections::HashMap;
use std::sync::Arc;

use dioxus::prelude::*;
use rustc_hash::FxBuildHasher;

use clingate_core::axis_store::{AxisStoreStoreExt, Param};
use clingate_core::gate_rules::autogate::describe;
use clingate_core::gate_rules::rule_store::RuleStore;
use clingate_core::gate_rules::run::{NeedsPlacing, RunOutcome};
use clingate_core::gates::GateState;
use clingate_core::omiq::metadata::MetaDataStoreStoreExt;

use crate::components::toast::{note, use_toast, warn};
use crate::gate_editor::gate_rules_window::RulesRun;
use crate::gate_editor::review_window::{EditorFocus, where_drawn};
use crate::gate_editor::workspace_window::{AxesStore, GateStore, MetadataStore};

/// A run stopped for a person, and everything it needs to go on.
pub(crate) struct PausedRun {
    /// What the run has done so far, already written into the working copy.
    pub so_far: RunOutcome,
    pub next_level: usize,
    pub needs: Vec<NeedsPlacing>,
    /// Which of `needs` the editor is showing.
    pub at: usize,
}

/// Asks the Gate Rules tab to go on with the paused run. A count, so each ask
/// is a change the tab sees.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct GoOn(pub u64);

/// What the editor should show for one gate a person has to place: the
/// population it is drawn on, its axes, and the sample.
pub(crate) fn focus_for(
    need: &NeedsPlacing,
    gates: &GateState,
    params: &[Param],
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
) -> Option<EditorFocus> {
    let drawn = where_drawn(gates, params, &need.gate_id, need.parent_gate.as_deref())?;
    let sample_name = names
        .iter()
        .find(|(_, gating_id)| **gating_id == need.file)
        .map(|(name, _)| name.clone())?;
    Some(EditorFocus {
        parent: drawn.parent,
        x: drawn.x.fluoro,
        y: drawn.y.fluoro,
        sample_name,
    })
}

/// "Lymph of Live on DONOR-B: no FMO for this specimen".
pub(crate) fn describe_need(need: &NeedsPlacing) -> String {
    let on = need
        .specimen
        .as_ref()
        .map(|specimen| specimen.group.clone())
        .unwrap_or_else(|| need.file.clone());
    format!(
        "{} on {on}: {}",
        describe(&need.gate, need.parent_gate.as_deref()),
        need.why
    )
}

/// The editor showing the gate a paused run needs placed at `at`. Called from
/// a handler, so it reads the document through the context rather than hooks.
pub(crate) fn show_need(at: usize) {
    let mut paused = consume_context::<Signal<Option<PausedRun>>>();
    let mut focus = consume_context::<Signal<Option<EditorFocus>>>();
    let gates = consume_context::<GateStore>();
    let axes = consume_context::<AxesStore>();
    let metadata = consume_context::<MetadataStore>();
    let want = {
        let mut held = paused.write();
        let Some(run) = held.as_mut() else {
            return;
        };
        run.at = at.min(run.needs.len().saturating_sub(1));
        run.needs.get(run.at).cloned()
    };
    let Some(need) = want else {
        return;
    };
    let params: Vec<Param> = axes.sorted_settings().peek().iter().cloned().collect();
    focus.set(focus_for(
        &need,
        &gates.peek(),
        &params,
        &metadata.file_name_to_gating_id().peek(),
    ));
}

/// Across the top of the editor while a run is paused: which gate to place
/// on which sample, and the way on.
#[component]
pub(crate) fn PausedBanner() -> Element {
    let mut paused = use_context::<Signal<Option<PausedRun>>>();
    let mut go_on = use_context::<Signal<GoOn>>();
    let rules = use_context::<Signal<RuleStore>>();
    let run_with = RulesRun::from_context();
    let toasts = use_toast();

    let Some((at, count, line)) = paused.read().as_ref().map(|run| {
        (
            run.at,
            run.needs.len(),
            run.needs.get(run.at).map(describe_need).unwrap_or_default(),
        )
    }) else {
        return rsx! {};
    };

    let stop = move |_| {
        let Some(run) = paused.write().take() else {
            return;
        };
        match run_with.keep(&run.so_far, &rules.peek()) {
            Ok(()) => note(
                &toasts,
                "The run was stopped. What it placed before it paused stays, and is on the Review tab",
            ),
            Err(e) => warn(&toasts, e),
        }
        crate::gate_editor::review::reviews_changed();
    };

    rsx! {
        div { class: "paused-run",
            span { class: "paused-run-text",
                "Rules run paused - place by hand ({at + 1} of {count}): {line}"
            }
            button {
                disabled: at == 0,
                onclick: move |_| show_need(at.saturating_sub(1)),
                "Previous"
            }
            button {
                disabled: at + 1 >= count,
                onclick: move |_| show_need(at + 1),
                "Next"
            }
            button {
                class: "paused-run-go-on",
                title: "Measure the gates under these as they are placed now",
                onclick: move |_| {
                    let next = go_on.peek().0 + 1;
                    go_on.set(GoOn(next));
                },
                "Continue the run"
            }
            button { onclick: stop, "Stop the run" }
        }
    }
}
