//! A rule never places a gate over another gate on its plot: one no rule in
//! the run moves, or one a rule has already placed. A rule that moves a line
//! is held back until its gate just touches; anything else that would
//! overlap is left where it was, and the run says why.

use std::sync::Arc;

use flow_gates::GateGeometry;
use rustc_hash::FxHashMap;

use crate::gate_rules::autogate::{UNBOUNDED, anchor_gate, entry_at, rebuild};
use crate::gate_rules::rule_store::RuleStore;
use crate::gates::GateState;
use crate::gates::gate_contact::{
    Axis, Point, extent, nearly_overlaps, outline, overlaps, same_axes,
};
use crate::gates::gate_store::{FileId, GateId, NodeId};
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::MetaDataFileMap;

/// A gate to keep clear of: its name, and its outline on the plot.
pub type Neighbour = (Arc<str>, Vec<Point>);

fn rule_index(
    state: &GateState,
    store: &RuleStore,
    names: &FxHashMap<NodeId, Arc<str>>,
    node: &NodeId,
) -> Option<usize> {
    let entry = entry_at(state, store, names, node)?;
    store.entries().iter().position(|e| std::ptr::eq(e, entry))
}

/// Whether the rule at `node` reads its position from the gate `own`, and so
/// is placed after it.
fn reads_from(
    state: &GateState,
    store: &RuleStore,
    names: &FxHashMap<NodeId, Arc<str>>,
    node: &NodeId,
    own: &GateId,
) -> bool {
    entry_at(state, store, names, node).is_some_and(|entry| {
        entry.rule.rule.anchors().into_iter().any(|anchor| {
            anchor_gate(state, anchor).is_ok_and(|id| state.gate_identity(&id) == *own)
        })
    })
}

/// The other gates on the plot `node` is drawn on - beside it under the same
/// parent, on the same two parameters - that a rule in `store` listed after
/// `node`'s does not place, nor one reading its position from `node`'s gate.
/// Quadrants are left out: one covers its whole plot. The rules listed first
/// are placed first (see `rule_levels`).
pub(crate) fn settled_beside(
    state: &GateState,
    store: &RuleStore,
    names: &FxHashMap<NodeId, Arc<str>>,
    node: &NodeId,
) -> Vec<NodeId> {
    let Some(gate_id) = state.gate_for_node(node) else {
        return Vec::new();
    };
    let Some(gate) = state.registered_gate(gate_id) else {
        return Vec::new();
    };
    let Some(parent) = state.parent_node(node) else {
        return Vec::new();
    };
    let own = state.gate_identity(gate_id);
    let params = gate.get_params();
    let mine = rule_index(state, store, names, node);
    state
        .child_nodes(&parent)
        .iter()
        .filter(|beside| {
            let Some(id) = state.gate_for_node(beside) else {
                return false;
            };
            let Some(other) = state.registered_gate(id) else {
                return false;
            };
            let later = matches!(
                (mine, rule_index(state, store, names, beside)),
                (Some(mine), Some(theirs)) if theirs > mine
            );
            state.gate_identity(id) != own
                && !other.is_composite()
                && same_axes(&other.get_params(), &params)
                && !later
                && !reads_from(state, store, names, beside, &own)
        })
        .cloned()
        .collect()
}

/// The gates to keep `gate_id` clear of on `file`, at every place it is
/// drawn, as outlines on its own two parameters.
pub(crate) fn neighbours(
    state: &GateState,
    store: &RuleStore,
    names: &FxHashMap<NodeId, Arc<str>>,
    gate_id: &GateId,
    file: &FileId,
    metadata: &MetaDataFileMap,
) -> Vec<Neighbour> {
    let Some(gate) = state.registered_gate(gate_id) else {
        return Vec::new();
    };
    let (x, y) = gate.get_params();
    state
        .nodes_for_gate(gate_id)
        .iter()
        .flat_map(|node| settled_beside(state, store, names, node))
        .filter_map(|beside| {
            let id = state.gate_for_node(&beside)?;
            let shown = state.gate_for_file(id, file, metadata)?;
            let name = state
                .population_name(id)
                .unwrap_or_else(|| Arc::from(shown.get_name()));
            Some((name, outline(&shown, id, &x, &y)?))
        })
        .collect()
}

/// The first of `neighbours` that `gate` overlaps.
pub(crate) fn first_overlap<'n>(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    neighbours: &'n [Neighbour],
) -> Option<&'n Arc<str>> {
    first_meeting(gate, gate_id, neighbours, overlaps)
}

fn first_meeting<'n>(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    neighbours: &'n [Neighbour],
    meets: fn(&[Point], &[Point]) -> bool,
) -> Option<&'n Arc<str>> {
    let (x, y) = gate.get_params();
    let shape = outline(gate, gate_id, &x, &y)?;
    neighbours
        .iter()
        .find(|(_, other)| meets(&shape, other))
        .map(|(name, _)| name)
}

/// `from` moved `t` of the way to `to`: every point of a rectangle or a
/// polygon on the straight line between its two places. Past `to` beyond 1,
/// and back past `from` below 0. An open edge stays open.
fn between(
    from: &Arc<dyn DrawableGate>,
    to: &Arc<dyn DrawableGate>,
    t: f64,
) -> Option<Arc<dyn DrawableGate>> {
    let (a, b) = (from.get_gate_ref(None)?, to.get_gate_ref(None)?);
    let (x, y) = to.get_params();
    let mix = |p: &flow_gates::GateNode, q: &flow_gates::GateNode| {
        let mut node = q.clone();
        for param in [&x, &y] {
            if let (Some(u), Some(v)) = (p.get_coordinate(param), q.get_coordinate(param))
                && v.is_finite()
                && v.abs() <= UNBOUNDED
            {
                let at = f64::from(u) + t * (f64::from(v) - f64::from(u));
                node.set_coordinate(param.clone(), at as f32);
            }
        }
        node
    };
    let geometry = match (&a.geometry, &b.geometry) {
        (GateGeometry::Rectangle { min: m0, max: x0 }, GateGeometry::Rectangle { min, max }) => {
            GateGeometry::Rectangle {
                min: mix(m0, min),
                max: mix(x0, max),
            }
        }
        (GateGeometry::Polygon { nodes: n0, .. }, GateGeometry::Polygon { nodes, closed })
            if n0.len() == nodes.len() =>
        {
            GateGeometry::Polygon {
                nodes: n0.iter().zip(nodes).map(|(p, q)| mix(p, q)).collect(),
                closed: *closed,
            }
        }
        _ => return None,
    };
    let mut moved = b.clone();
    moved.geometry = geometry;
    rebuild(to, moved).ok()
}

/// How a rule's line placement came out against the gates beside it.
pub(crate) enum Clear {
    /// Clear as the rule placed it.
    AsPlaced,
    /// Held back until it just touched the gate named.
    HeldBack(Arc<dyn DrawableGate>, Arc<str>),
}

/// `moved` - where a rule moved `from` along one line - held back towards
/// `from`, and past it if `from` overlaps too, until it overlaps none of
/// `neighbours`. Refused when nowhere along that line is clear.
pub(crate) fn hold_clear(
    from: &Arc<dyn DrawableGate>,
    moved: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    neighbours: &[Neighbour],
) -> Result<Clear, String> {
    let Some(blocker) = first_overlap(moved, gate_id, neighbours).cloned() else {
        return Ok(Clear::AsPlaced);
    };
    let clear = |t: f64| {
        between(from, moved, t)
            .filter(|g| first_meeting(g, gate_id, neighbours, nearly_overlaps).is_none())
    };
    // Back from the rule's place in widening steps, to bracket the edge of
    // the clear stretch nearest to it.
    let mut inside = 1.0;
    let mut out = None;
    for step in (0..24).map(|k| f64::from(1u32 << k) / 1024.0) {
        let t = 1.0 - step;
        if clear(t).is_some() {
            out = Some(t);
            break;
        }
        inside = t;
    }
    let Some(mut outside) = out else {
        return Err(format!(
            "it would overlap {blocker} wherever along its line it went"
        ));
    };
    for _ in 0..60 {
        let mid = (inside + outside) / 2.0;
        if clear(mid).is_some() {
            outside = mid;
        } else {
            inside = mid;
        }
    }
    let held = clear(outside).ok_or_else(|| format!("it could not be kept clear of {blocker}"))?;
    Ok(Clear::HeldBack(held, blocker))
}

/// The fewest and the most steps an edit is tried at, from where the gate
/// was to where the edit takes it.
const FEWEST_EDIT_STEPS: usize = 16;
const MOST_EDIT_STEPS: usize = 4096;

/// How many steps from `was` to `to`, the same points moved, keep every
/// step under half the narrowest of `outlines`: no step can jump one.
fn edit_steps<'o>(
    was: &[Point],
    to: &[Point],
    outlines: impl Iterator<Item = &'o [Point]>,
) -> usize {
    let travel = was
        .iter()
        .zip(to)
        .map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1))
        .fold(0.0, f64::max);
    let narrowest = outlines
        .map(|o| {
            let (x0, x1) = extent(o, Axis::X);
            let (y0, y1) = extent(o, Axis::Y);
            (x1 - x0).min(y1 - y0)
        })
        .fold(f64::INFINITY, f64::min);
    let steps = (2.0 * travel / narrowest).ceil();
    if steps.is_finite() {
        (steps as usize).clamp(FEWEST_EDIT_STEPS, MOST_EDIT_STEPS)
    } else {
        MOST_EDIT_STEPS
    }
}

/// `moved` - an edit of `from` - stopped where it first touches one of
/// `others` that `from` was clear of: an edit in the editor never takes a
/// gate into another. A gate that cannot be moved part way, an ellipse,
/// keeps to `from` when `moved` would overlap.
pub fn kept_apart(
    from: &Arc<dyn DrawableGate>,
    moved: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    others: &[Arc<dyn DrawableGate>],
) -> Arc<dyn DrawableGate> {
    if moved.is_composite() {
        return moved.clone();
    }
    let (x, y) = moved.get_params();
    let Some(was) = outline(from, gate_id, &x, &y) else {
        return moved.clone();
    };
    let apart: Vec<Vec<Point>> = others
        .iter()
        .filter(|other| other.get_id() != *gate_id && !other.is_composite())
        .filter_map(|other| outline(other, &other.get_id(), &x, &y))
        .filter(|other| !overlaps(&was, other))
        .collect();
    let meets = |gate: &Arc<dyn DrawableGate>, test: fn(&[Point], &[Point]) -> bool| {
        outline(gate, gate_id, &x, &y).is_none_or(|shape| apart.iter().any(|o| test(&shape, o)))
    };
    if between(from, moved, 1.0).is_none() {
        return if meets(moved, overlaps) {
            from.clone()
        } else {
            moved.clone()
        };
    }
    let to = outline(moved, gate_id, &x, &y).unwrap_or_default();
    let steps = edit_steps(&was, &to, apart.iter().chain([&was]).map(Vec::as_slice));
    let step = 1.0 / steps as f64;
    let Some(first_meeting) = (1..=steps)
        .map(|k| k as f64 * step)
        .find(|t| between(from, moved, *t).is_none_or(|g| meets(&g, overlaps)))
    else {
        return moved.clone();
    };
    let (mut clear, mut meeting) = (first_meeting - step, first_meeting);
    for _ in 0..40 {
        let mid = (clear + meeting) / 2.0;
        if between(from, moved, mid).is_none_or(|g| meets(&g, nearly_overlaps)) {
            meeting = mid;
        } else {
            clear = mid;
        }
    }
    between(from, moved, clear).unwrap_or_else(|| from.clone())
}
