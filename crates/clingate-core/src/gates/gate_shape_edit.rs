//! Edits to a gate's outline from the editor's menus: a side of a polygon
//! made level, a point added or taken away, a rectangle made a polygon.

use std::sync::Arc;

use anyhow::{anyhow, bail};
use flow_gates::{GateGeometry, GateNode};

use crate::gate_rules::autogate::{UNBOUNDED, rebuild};
use crate::gates::gate_contact::nearest_on_side;
use crate::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gates::gate_store::GateId;
use crate::gates::gate_traits::DrawableGate;

/// The fewest points a polygon keeps.
const FEWEST_POINTS: usize = 3;

fn polygon_nodes(gate: &Arc<dyn DrawableGate>, gate_id: &GateId) -> anyhow::Result<Vec<GateNode>> {
    let shape = gate
        .get_gate_ref(Some(gate_id))
        .ok_or_else(|| anyhow!("{gate_id} has no outline"))?;
    match &shape.geometry {
        GateGeometry::Polygon { nodes, .. } => Ok(nodes.clone()),
        _ => bail!("only a polygon's points can be edited"),
    }
}

/// `gate` with `nodes` for its points, numbered in order.
fn with_nodes(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    nodes: Vec<GateNode>,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let mut shape = gate
        .get_gate_ref(Some(gate_id))
        .ok_or_else(|| anyhow!("{gate_id} has no outline"))?
        .clone();
    let nodes = nodes
        .into_iter()
        .enumerate()
        .map(|(index, mut node)| {
            node.id = Arc::from(format!("polygon_node_{index}"));
            node
        })
        .collect();
    shape.geometry = GateGeometry::Polygon {
        nodes,
        closed: true,
    };
    rebuild(gate, shape).map_err(|e| anyhow!(e.to_string()))
}

fn coordinate(node: &GateNode, parameter: &str) -> anyhow::Result<f32> {
    node.get_coordinate(parameter)
        .ok_or_else(|| anyhow!("the gate is not drawn on {parameter}"))
}

/// The two ends of `side`: from point `side` to the next, round to the first.
fn ends(count: usize, side: usize) -> anyhow::Result<(usize, usize)> {
    if side >= count {
        bail!("the polygon has no side {side}");
    }
    Ok((side, (side + 1) % count))
}

/// `gate` with both ends of `side` moved to the average of their
/// `parameter`: level along the other axis.
pub fn level_side(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    side: usize,
    parameter: &str,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let mut nodes = polygon_nodes(gate, gate_id)?;
    let (from, to) = ends(nodes.len(), side)?;
    let middle = (coordinate(&nodes[from], parameter)? + coordinate(&nodes[to], parameter)?) / 2.0;
    for end in [from, to] {
        nodes[end].set_coordinate(parameter, middle);
    }
    with_nodes(gate, gate_id, nodes)
}

/// `gate` with a point on `side` where it comes nearest to `at`, a point on
/// the plot of `x` by `y`: the outline is unchanged, with one more point to
/// drag.
pub fn with_point_added(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    side: usize,
    (x, y): (&str, &str),
    at: (f32, f32),
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let mut nodes = polygon_nodes(gate, gate_id)?;
    let (from, to) = ends(nodes.len(), side)?;
    let a = (coordinate(&nodes[from], x)?, coordinate(&nodes[from], y)?);
    let b = (coordinate(&nodes[to], x)?, coordinate(&nodes[to], y)?);
    let wide = |p: (f32, f32)| (f64::from(p.0), f64::from(p.1));
    let on = nearest_on_side(wide(at), (wide(a), wide(b)));
    let mut point = nodes[from].clone();
    point.set_coordinate(x, on.0 as f32);
    point.set_coordinate(y, on.1 as f32);
    nodes.insert(from + 1, point);
    with_nodes(gate, gate_id, nodes)
}

/// `gate` without its point `index`. Refused where it would leave fewer
/// than three.
pub fn without_point(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    index: usize,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let mut nodes = polygon_nodes(gate, gate_id)?;
    if index >= nodes.len() {
        bail!("the polygon has no point {index}");
    }
    if nodes.len() <= FEWEST_POINTS {
        bail!("a polygon keeps at least {FEWEST_POINTS} points");
    }
    nodes.remove(index);
    with_nodes(gate, gate_id, nodes)
}

/// A rectangle as the polygon through its four corners, the same gate
/// otherwise. Refused for any other shape, and for a rectangle with an edge
/// left open to the end of an axis, which has no corner there to drag.
pub fn as_polygon(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    if gate.as_any().downcast_ref::<RectangleGate>().is_none() {
        bail!("only a rectangle can be made a polygon");
    }
    let shape = gate
        .get_gate_ref(Some(gate_id))
        .ok_or_else(|| anyhow!("{gate_id} has no outline"))?;
    let GateGeometry::Rectangle { min, max } = &shape.geometry else {
        bail!("only a rectangle can be made a polygon");
    };
    let (x, y) = (&shape.parameters.0, &shape.parameters.1);
    let corner = |at_x: &GateNode, at_y: &GateNode| -> anyhow::Result<GateNode> {
        let (cx, cy) = (coordinate(at_x, x)?, coordinate(at_y, y)?);
        if [cx, cy]
            .iter()
            .any(|v| !v.is_finite() || v.abs() > UNBOUNDED)
        {
            bail!(
                "{gate_id} has an edge open to the end of an axis, with no corner to make a point"
            );
        }
        Ok(GateNode::new("")
            .with_coordinate(x.clone(), cx)
            .with_coordinate(y.clone(), cy))
    };
    let corners = vec![
        corner(min, min)?,
        corner(max, min)?,
        corner(max, max)?,
        corner(min, max)?,
    ];
    with_nodes(gate, gate_id, corners)
}
