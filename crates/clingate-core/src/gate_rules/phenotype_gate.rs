//! Turning a fitted population into a gate's geometry.
//!
//! The two ways a phenotype rule can end, as geometry rather than as points:
//! the drawn shape moved and resized onto the matched cells, or a fresh
//! polygon traced round them.
//!
//! Kept apart from [`autogate`](super::autogate) because it is arithmetic over
//! geometry and knows nothing about rules, reports or stores - the same split
//! that keeps `threshold` testable.

use std::sync::Arc;

use flow_gates::{GateGeometry, GateNode};

use super::shape_fit::{Outline, Reshape};

/// Why a geometry could not be made.
#[derive(Clone, Debug, PartialEq)]
pub enum NoGeometry {
    /// A boolean gate has no shape of its own to move or replace.
    NotAShape,
    /// The outline came back with too few points to close.
    TooFewPoints(usize),
}

impl std::fmt::Display for NoGeometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoGeometry::NotAShape => write!(
                f,
                "this gate is a statement about other gates, so it has no shape to fit"
            ),
            NoGeometry::TooFewPoints(n) => {
                write!(f, "the boundary came back with only {n} points")
            }
        }
    }
}

/// The same shape, moved and resized onto a population.
///
/// Every coordinate on the two plot axes goes through the reshape; anything on
/// a third channel is left exactly as it was. A gate can carry coordinates for
/// channels it is not drawn on - a rectangle imported from a plot with more
/// axes than this one - and rewriting those would move the gate on a plot
/// nobody asked about.
///
/// An unbounded edge stays unbounded. `1e16` is Omiq's way of saying "this
/// side does not close", and scaling it would turn a half-open gate into one
/// with an arbitrary far edge that exports as a real coordinate.
pub fn reshaped(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    reshape: &Reshape,
) -> Result<GateGeometry, NoGeometry> {
    let move_node = |node: &GateNode| -> GateNode {
        let mut out = node.clone();
        let x = node.get_coordinate(&params.0);
        let y = node.get_coordinate(&params.1);
        // Both axes at once, because the reshape is defined on a point.
        // Missing coordinates stand in as the population's own centre, so the
        // axis that is present moves exactly as it should and the absent one
        // contributes nothing.
        let from = (
            x.map(|v| v as f64).unwrap_or(reshape.from.centre.0),
            y.map(|v| v as f64).unwrap_or(reshape.from.centre.1),
        );
        let to = reshape.moved(from);
        if let Some(v) = x {
            out.set_coordinate(params.0.clone(), keep_unbounded(v, to.0));
        }
        if let Some(v) = y {
            out.set_coordinate(params.1.clone(), keep_unbounded(v, to.1));
        }
        out
    };

    Ok(match geometry {
        GateGeometry::Rectangle { min, max } => {
            let (a, b) = (move_node(min), move_node(max));
            // A negative scale cannot happen - spreads are positive - so min
            // stays min, but the pair is normalised anyway: a rectangle whose
            // min crept past its max is a gate that admits nothing, and it
            // would be silent.
            normalise(a, b, params)
        }
        GateGeometry::Polygon { nodes, closed } => GateGeometry::Polygon {
            nodes: nodes.iter().map(move_node).collect(),
            closed: *closed,
        },
        GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } => GateGeometry::Ellipse {
            center: move_node(center),
            // The radii are lengths on each axis, so they take the scale
            // without the shift.
            radius_x: (*radius_x as f64 * reshape.scale.0) as f32,
            radius_y: (*radius_y as f64 * reshape.scale.1) as f32,
            angle: *angle,
        },
        GateGeometry::Boolean { .. } => return Err(NoGeometry::NotAShape),
    })
}

/// Omiq's sentinel for an edge that does not close.
const UNBOUNDED: f32 = 1e16;

fn keep_unbounded(was: f32, now: f64) -> f32 {
    if !was.is_finite() || was.abs() >= UNBOUNDED {
        was
    } else {
        now as f32
    }
}

fn normalise(a: GateNode, b: GateNode, params: &(Arc<str>, Arc<str>)) -> GateGeometry {
    let (mut min, mut max) = (a, b);
    for axis in [&params.0, &params.1] {
        let (Some(lo), Some(hi)) = (min.get_coordinate(axis), max.get_coordinate(axis)) else {
            continue;
        };
        if lo > hi {
            min.set_coordinate(axis.clone(), hi);
            max.set_coordinate(axis.clone(), lo);
        }
    }
    GateGeometry::Rectangle { min, max }
}

/// A fresh polygon from a traced boundary.
///
/// `named` is the gate's id, which the node ids are derived from so a gate
/// written twice has the same node names both times - an export that renamed
/// every vertex on every run would show a diff for a gate that had not moved.
pub fn polygon(
    outline: &Outline,
    params: &(Arc<str>, Arc<str>),
    named: &str,
) -> Result<GateGeometry, NoGeometry> {
    if outline.0.len() < 3 {
        return Err(NoGeometry::TooFewPoints(outline.0.len()));
    }
    let nodes = outline
        .0
        .iter()
        .enumerate()
        .map(|(at, (x, y))| {
            GateNode::new(format!("{named}_{at}"))
                .with_coordinate(params.0.clone(), *x as f32)
                .with_coordinate(params.1.clone(), *y as f32)
        })
        .collect();
    Ok(GateGeometry::Polygon {
        nodes,
        closed: true,
    })
}
