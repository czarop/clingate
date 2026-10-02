//! Turning a fitted population into a gate's geometry.
//!
//! The ways a phenotype rule can end, as geometry rather than as points: the
//! drawn shape carried edge by edge onto the sample, or slid there whole, or a
//! fresh polygon traced round the matched cells.
//!
//! Kept apart from [`autogate`](super::autogate) because it is arithmetic over
//! geometry and knows nothing about rules, reports or stores - the same split
//! that keeps `threshold` testable.

use std::sync::Arc;

use flow_gates::{GateGeometry, GateNode};

use super::phenotype::{Frame, Open};
use super::shape_fit::Outline;

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

/// How one of a gate's axes is carried from the reference to a sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Carry {
    /// The axis read on the reference's parent, and on the sample's.
    pub from: Frame,
    pub to: Frame,
    /// The gate's extent on the axis on the reference, and the sides of it
    /// the gate leaves open.
    pub extent: (f64, f64),
    pub open: Open,
}

impl Carry {
    /// Where `value` on the reference goes on the sample: the same reading in
    /// the sample's frame - see [`Carry::never_in`] for a side left open.
    pub fn value(&self, value: f64) -> f64 {
        self.never_in(value, self.to.value_at(self.from.read(value)))
    }

    /// Where `value` goes when the gate slides by [`Carry::shift`], keeping
    /// its size - see [`Carry::never_in`] for a side left open.
    pub fn slid(&self, value: f64) -> f64 {
        self.never_in(value, value + self.shift())
    }

    /// `moved`, unless `value` is an edge on a side the gate leaves open and
    /// `moved` pulls it in: nothing lay beyond it on the reference, so on a
    /// sample brighter or dimmer there is nothing it should cut off.
    fn never_in(&self, value: f64, moved: f64) -> f64 {
        if self.open.high && value >= self.extent.1 {
            moved.max(value)
        } else if self.open.low && value <= self.extent.0 {
            moved.min(value)
        } else {
            moved
        }
    }

    /// How far the gate's closed edges move, on average - how far a gate
    /// that keeps its size slides. Nothing where both sides are open.
    pub fn shift(&self) -> f64 {
        let moves: Vec<f64> = [
            (self.extent.0, self.open.low),
            (self.extent.1, self.open.high),
        ]
        .into_iter()
        .filter(|(_, open)| !open)
        .map(|(edge, _)| self.value(edge) - edge)
        .collect();
        if moves.is_empty() {
            0.0
        } else {
            moves.iter().sum::<f64>() / moves.len() as f64
        }
    }

    /// How much the gate stretches along the axis: the sample's unit against
    /// the reference's where both edges are carried, 1 where one stays.
    pub fn stretch(&self) -> f64 {
        if self.open.low || self.open.high {
            1.0
        } else {
            self.to.unit / self.from.unit
        }
    }
}

/// The same shape with every coordinate on the two plot axes carried as `x`
/// and `y` say.
pub fn carried(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    x: &Carry,
    y: &Carry,
) -> Result<GateGeometry, NoGeometry> {
    transformed(
        geometry,
        params,
        |v| x.value(v),
        |v| y.value(v),
        (x.to.unit / x.from.unit, y.to.unit / y.from.unit),
    )
}

/// The same shape slid along the two plot axes as `x` and `y` say, its size
/// unchanged.
pub fn slid(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    x: &Carry,
    y: &Carry,
) -> Result<GateGeometry, NoGeometry> {
    transformed(geometry, params, |v| x.slid(v), |v| y.slid(v), (1.0, 1.0))
}

/// `geometry` with each coordinate on the two plot axes put through `x` and
/// `y`, and an ellipse's radii scaled by `radii`.
///
/// Anything on a third channel is left exactly as it was. A gate can carry
/// coordinates for channels it is not drawn on - a rectangle imported from a
/// plot with more axes than this one - and rewriting those would move the gate
/// on a plot nobody asked about.
///
/// An unbounded edge stays unbounded. `1e16` is Omiq's way of saying "this
/// side does not close", and moving it would turn a half-open gate into one
/// with an arbitrary far edge that exports as a real coordinate.
fn transformed(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    x: impl Fn(f64) -> f64,
    y: impl Fn(f64) -> f64,
    radii: (f64, f64),
) -> Result<GateGeometry, NoGeometry> {
    let move_node = |node: &GateNode| -> GateNode {
        let mut out = node.clone();
        if let Some(v) = node.get_coordinate(&params.0) {
            out.set_coordinate(params.0.clone(), keep_unbounded(v, x(v as f64)));
        }
        if let Some(v) = node.get_coordinate(&params.1) {
            out.set_coordinate(params.1.clone(), keep_unbounded(v, y(v as f64)));
        }
        out
    };

    Ok(match geometry {
        GateGeometry::Rectangle { min, max } => {
            // Normalised: a rectangle whose min crept past its max is a gate
            // that admits nothing, and it would be silent.
            normalise(move_node(min), move_node(max), params)
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
            radius_x: (*radius_x as f64 * radii.0) as f32,
            radius_y: (*radius_y as f64 * radii.1) as f32,
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
