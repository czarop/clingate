//! Turning a fitted population into a gate's geometry.
//!
//! The ways a phenotype rule can end, as geometry rather than as points: the
//! drawn shape carried edge by edge onto the sample - each edge kept at the
//! same point of the gap between the population and the cells beyond it - or
//! slid there whole, or a fresh polygon traced round the matched cells.
//!
//! Kept apart from [`autogate`](super::autogate) because it is arithmetic over
//! geometry and knows nothing about rules, reports or stores - the same split
//! that keeps `threshold` testable.

use std::sync::Arc;

use flow_gates::{GateGeometry, GateNode};

use super::phenotype::{Open, STRAY_EVENTS, STRAY_SHARE};
use super::rule::Side;
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

/// The share of a population at each end of an axis that lies past its
/// boundary there: its last few cells are the ones the match is least sure of.
pub const BOUNDARY: f64 = 0.05;

/// Where a population ends on one side of an axis, and where the cells beyond
/// it begin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
    pub inside: f64,
    /// `None` where no more than dust lies beyond - see [`Open`].
    pub beyond: Option<f64>,
}

impl Gap {
    /// On `side`, from the population's values on the axis and the rest of
    /// the parent's within the gate's span on the other axis. `None` for a
    /// population with no cells.
    pub fn of(inside: &[f64], rest: &[f64], side: Side) -> Option<Self> {
        let low = side == Side::Lower;
        let edge = quantile(inside, if low { BOUNDARY } else { 1.0 - BOUNDARY })?;
        let beyond: Vec<f64> = rest
            .iter()
            .copied()
            .filter(|v| if low { *v < edge } else { *v > edge })
            .collect();
        let enough =
            beyond.len() > STRAY_EVENTS && beyond.len() as f64 >= STRAY_SHARE * inside.len() as f64;
        Some(Self {
            inside: edge,
            beyond: enough
                .then(|| {
                    near_boundary(&beyond, side)
                        .or_else(|| quantile(&beyond, if low { 1.0 - BOUNDARY } else { BOUNDARY }))
                })
                .flatten(),
        })
    }
}

/// How many widths from its peak a symmetric population's 95th percentile
/// sits.
const NEAR_BOUNDARY_WIDTHS: f64 = 1.645;

/// Where the cells `beyond` a population on `side` of it end towards it:
/// their peak, as a negative's is found, plus 1.645 widths of their far side -
/// where their 95th percentile would be were they as wide towards the
/// population. Not their percentile itself: towards the population is where
/// its dim tail trails into them, and a percentile moves with the tail.
fn near_boundary(beyond: &[f64], side: Side) -> Option<f64> {
    // Turned so the population lies above them, their far side below.
    let toward = if side == Side::Lower { 1.0 } else { -1.0 };
    let turned: Vec<f64> = beyond.iter().map(|value| value * toward).collect();
    let peak = crate::gate_rules::threshold::negative_peak(&turned)?;
    Some(toward * (peak.centre + NEAR_BOUNDARY_WIDTHS * peak.spread))
}

/// Where `edge`, drawn between a population and the cells beyond it on the
/// reference (`there`), goes on the sample (`here`): at the same point of the
/// gap between them, as a person would put it. An edge drawn outside the gap
/// - into the top of the cells beyond, or into the population - goes as far as
/// the boundary it sits past moved: scaled by the gap, a wider one would
/// carry it further in. Where either has nothing beyond, as far as the
/// population's own boundary moved.
pub fn edge_in_gap(edge: f64, there: Gap, here: Gap) -> f64 {
    match (there.beyond, here.beyond) {
        (Some(beyond), Some(beyond_here)) if (there.inside - beyond).abs() > f64::EPSILON => {
            let at = (edge - beyond) / (there.inside - beyond);
            if at < 0.0 {
                edge + beyond_here - beyond
            } else if at > 1.0 {
                edge + here.inside - there.inside
            } else {
                beyond_here + at * (here.inside - beyond_here)
            }
        }
        _ => edge + here.inside - there.inside,
    }
}

/// The value `share` of the way through `values`, or `None` for none.
fn quantile(values: &[f64], share: f64) -> Option<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(f64::total_cmp);
    Some(sorted[((sorted.len() - 1) as f64 * share).round() as usize])
}

/// How one of a gate's axes is carried from the reference to a sample: its
/// two edges moved, and everything between them kept in proportion, so a
/// polygon keeps its shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Carry {
    /// The gate's extent on the axis on the reference, and the sides of it
    /// the gate leaves open.
    pub extent: (f64, f64),
    pub open: Open,
    /// Where its edges go on the sample.
    pub to: (f64, f64),
}

impl Carry {
    /// The extent its edges are carried to - see [`Carry::never_in`].
    pub fn carried(&self) -> (f64, f64) {
        self.never_in(self.to)
    }

    /// The extent slid by [`Carry::shift`], its size kept.
    pub fn slid(&self) -> (f64, f64) {
        let by = self.shift();
        (self.extent.0 + by, self.extent.1 + by)
    }

    /// `edges`, with a side the gate leaves open never pulled in: nothing lay
    /// beyond it on the reference, so on a sample brighter or dimmer there is
    /// nothing it should cut off.
    fn never_in(&self, (low, high): (f64, f64)) -> (f64, f64) {
        (
            if self.open.low {
                low.min(self.extent.0)
            } else {
                low
            },
            if self.open.high {
                high.max(self.extent.1)
            } else {
                high
            },
        )
    }

    /// How far the gate's closed edges move, on average - how far a gate
    /// that keeps its size slides. Nothing where both sides are open.
    pub fn shift(&self) -> f64 {
        let moves: Vec<f64> = [
            (self.to.0 - self.extent.0, self.open.low),
            (self.to.1 - self.extent.1, self.open.high),
        ]
        .into_iter()
        .filter(|(_, open)| !open)
        .map(|(by, _)| by)
        .collect();
        if moves.is_empty() {
            0.0
        } else {
            moves.iter().sum::<f64>() / moves.len() as f64
        }
    }

    /// How much the gate stretches along the axis: its carried extent against
    /// its own where both edges are carried, 1 where a side is open.
    pub fn stretch(&self) -> f64 {
        if self.open.low || self.open.high {
            1.0
        } else {
            ratio(self.carried(), self.extent)
        }
    }

    /// `value` put as far between the ends of `onto` as it lies between the
    /// gate's ends on the reference.
    fn within(&self, onto: (f64, f64), value: f64) -> f64 {
        let width = self.extent.1 - self.extent.0;
        if width.abs() < f64::EPSILON {
            value + ((onto.0 - self.extent.0) + (onto.1 - self.extent.1)) / 2.0
        } else {
            onto.0 + (value - self.extent.0) / width * (onto.1 - onto.0)
        }
    }
}

/// How much wider `to` is than `from`; 1 for a point.
fn ratio(to: (f64, f64), from: (f64, f64)) -> f64 {
    let width = from.1 - from.0;
    if width.abs() < f64::EPSILON {
        1.0
    } else {
        (to.1 - to.0) / width
    }
}

/// The same shape with its edges on the two plot axes carried as `x` and `y`
/// say, and everything between them kept in proportion.
pub fn carried(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    x: &Carry,
    y: &Carry,
) -> Result<GateGeometry, NoGeometry> {
    onto(geometry, params, (x, x.carried()), (y, y.carried()))
}

/// The same shape slid along the two plot axes as `x` and `y` say, its size
/// unchanged.
pub fn slid(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    x: &Carry,
    y: &Carry,
) -> Result<GateGeometry, NoGeometry> {
    onto(geometry, params, (x, x.slid()), (y, y.slid()))
}

/// `geometry` with each axis's extent put onto the one given beside it.
fn onto(
    geometry: &GateGeometry,
    params: &(Arc<str>, Arc<str>),
    (x, to_x): (&Carry, (f64, f64)),
    (y, to_y): (&Carry, (f64, f64)),
) -> Result<GateGeometry, NoGeometry> {
    transformed(
        geometry,
        params,
        |v| x.within(to_x, v),
        |v| y.within(to_y, v),
        (ratio(to_x, x.extent), ratio(to_y, y.extent)),
    )
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
