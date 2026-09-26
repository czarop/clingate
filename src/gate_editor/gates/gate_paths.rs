//! Naming a population unambiguously.
//!
//! A gate's name is not unique. `CD4+CD8-` can be drawn under `CD3+CD14-` and
//! again under a MAIT population, and those are two different populations that
//! happen to share a label. A rule that names one by its bare name names both,
//! and would position a gate somewhere nobody asked for.
//!
//! Omiq's answer is to say as much of the path as it takes and no more, so the
//! common case stays short: `CD4+CD8-` while that is unambiguous, and
//! `CD3+CD14- / CD4+CD8-` only where it is not. That is what this computes.

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::NodeId;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

/// How a path reads when more than one step is needed.
pub const SEPARATOR: &str = " / ";

/// A name for every node, each as short as it can be while still naming only
/// itself.
///
/// Built for the whole tree at once because shortness is a property of the
/// tree: whether `CD4+CD8-` is enough depends on what else is in the document.
pub fn unique_names(state: &GateState) -> FxHashMap<NodeId, Arc<str>> {
    // Root-first names for every node, so a suffix of one is a path ending
    // there.
    let mut chains: Vec<(NodeId, Vec<Arc<str>>)> = Vec::new();
    for (node, _) in state.placements() {
        let names: Vec<Arc<str>> = state
            .gate_chain_for_node(node)
            .iter()
            .filter_map(|id| state.registered_gate(id))
            .map(|g| Arc::from(g.get_name()) as Arc<str>)
            .collect();
        // `gate_chain_for_node` already reads root-first, which is the order a
        // path is spoken in - so the last few entries are a path ending here.
        if names.is_empty() {
            continue;
        }
        chains.push((node.clone(), names));
    }

    let mut out = FxHashMap::default();
    for (node, names) in &chains {
        let mut depth = 1;
        // Lengthen until no other node ends the same way. The full path always
        // terminates this: two nodes cannot share every ancestor and still be
        // two nodes.
        while depth < names.len() && !is_unique(&chains, node, names, depth) {
            depth += 1;
        }
        out.insert(node.clone(), Arc::from(suffix(names, depth).as_str()));
    }
    out
}

/// Does any *other* node's path end the same way?
fn is_unique(
    chains: &[(NodeId, Vec<Arc<str>>)],
    node: &NodeId,
    names: &[Arc<str>],
    depth: usize,
) -> bool {
    let mine = &names[names.len() - depth..];
    !chains.iter().any(|(other, other_names)| {
        other != node
            && other_names.len() >= depth
            && &other_names[other_names.len() - depth..] == mine
    })
}

fn suffix(names: &[Arc<str>], depth: usize) -> String {
    names[names.len() - depth.min(names.len())..]
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

/// The names in the order a person would read them, deduplicated.
pub fn sorted_unique(names: &FxHashMap<NodeId, Arc<str>>) -> Vec<Arc<str>> {
    let seen: FxHashSet<Arc<str>> = names.values().cloned().collect();
    let mut out: Vec<Arc<str>> = seen.into_iter().collect();
    out.sort();
    out
}
