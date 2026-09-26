//! What one gallery plot shows.
//!
//! The editor answers this with `gate_ids_by_view`, a cache the UI fills in as
//! it draws: `match_gates_to_plot` writes the view's gate list, then
//! `get_gates_for_plot` reads it back. That works for a tab with one plot and
//! one selection, and it is the wrong shape here - the gallery draws twenty
//! views at once, off threads, and a shared `&mut` cache between them would be
//! either a bottleneck or a race.
//!
//! These are plain functions of the state instead. Nothing is cached, nothing
//! is written, and the same arguments always give the same answer, which is
//! also what makes them testable without a renderer.

use std::sync::Arc;

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateId, GateOverrideResolver, NodeId, ROOTGATE};
use crate::gate_editor::gates::gate_traits::DrawableGate;

/// The gates drawn on the plot of `node`, resolved for one file.
///
/// The plot of a node shows the events that node's chain admits, with the
/// node's *children* drawn on top - so this is the children, not the node. That
/// is the same relation the editor has, and it is why clicking a gate in the
/// hierarchy selects its parent: the plot a gate is visible on is its parent's.
///
/// Resolved but not yet matched to the plot's axes, deliberately. Resolving
/// hands back the very `Arc` the store holds, so these can be fingerprinted by
/// address; matching mints a new `Arc` on every call, which cannot be. See
/// [`super::cache`].
pub fn drawn_on(
    state: &GateState,
    node: &Arc<str>,
    resolver: &GateOverrideResolver,
) -> Vec<Arc<dyn DrawableGate>> {
    let mut out = Vec::new();
    for child in state.child_nodes(&NodeId::from(node.clone())) {
        let Some(gate_id) = state.gate_for_node(&child) else {
            continue;
        };
        let Ok(gate) = resolver.resolve_drawable(gate_id) else {
            continue;
        };
        // A composite is registered under its group id and under each corner's.
        // Drawing the corners as well as the group would draw every line twice.
        if !gate.is_primary() {
            continue;
        }
        if out.iter().any(|held| Arc::ptr_eq(held, &gate)) {
            continue;
        }
        out.push(gate);
    }
    out
}

/// Those of `gates` that are drawn on this pair of axes, rewritten for them.
///
/// [`DrawableGate::match_to_plot_axis`] has three answers, and the middle one
/// is easy to misread:
///
/// - `Ok(None)` - the gate is already on these axes, so there is nothing to
///   rewrite. This is the *usual* case, and the gate is kept exactly as it is.
/// - `Ok(Some(g))` - the plot has the gate's two parameters the other way
///   round, and `g` is the gate transposed onto them.
/// - `Err(_)` - the gate is drawn on some other pair and is not on this plot.
///   Not a failure: a population usually carries gates on several pairs.
///
/// Reading `Ok(None)` as "not on this plot" drops every gate that did not need
/// transposing, which is nearly all of them - the gallery drew no outlines at
/// all until this was fixed. The editor's `match_gates_to_plot` takes the same
/// three answers but *writes* the rewritten gates back to its view index and
/// skips the `None`s, because for it `None` means "the copy I hold is still
/// right"; here there is no stored copy to leave alone, so `None` has to
/// produce the original.
pub fn matched_to_axes(
    gates: &[Arc<dyn DrawableGate>],
    x: &str,
    y: &str,
) -> Vec<Arc<dyn DrawableGate>> {
    gates
        .iter()
        .filter_map(|gate| match gate.match_to_plot_axis(x, y) {
            Ok(None) => Some(gate.clone()),
            Ok(Some(transposed)) => Some(Arc::from(transposed)),
            Err(_) => None,
        })
        .collect()
}

/// Every gate a plot's pixels depend on: what filters it, then what is drawn.
///
/// One list rather than two, because the cache only ever asks "has anything
/// changed", and a gate moving from the chain into the drawing - which happens
/// when the selection moves up a level - should read as a change either way.
pub fn dependencies(
    state: &GateState,
    node: &Arc<str>,
    resolver: &GateOverrideResolver,
) -> Vec<Arc<dyn DrawableGate>> {
    let mut out: Vec<Arc<dyn DrawableGate>> = chain_of(state, node)
        .iter()
        .filter_map(|id| resolver.resolve_drawable(id).ok())
        .collect();
    out.extend(drawn_on(state, node, resolver));
    out
}

/// The gates that filter the plot of `node`, outermost first.
pub fn chain_of(state: &GateState, node: &Arc<str>) -> Vec<GateId> {
    if **node == **ROOTGATE {
        return Vec::new();
    }
    state.gate_chain_for_node(&NodeId::from(node.clone()))
}
