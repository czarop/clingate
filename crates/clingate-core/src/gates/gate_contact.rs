//! Where gates on one plot meet: whether two overlap, and how far one can
//! slide along an axis before it touches another. Worked on the plot as
//! drawn - a gate's coordinates are in the scaled units the plot draws, so
//! an edge is the straight line on screen.

use std::sync::Arc;

use flow_gates::GateGeometry;

use crate::gates::gate_store::GateId;
use crate::gates::gate_traits::DrawableGate;

/// A point on the plot, as (x, y).
pub type Point = (f64, f64);

/// The points an ellipse's outline is drawn through.
const ELLIPSE_POINTS: usize = 64;

/// Which axis of the plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    fn along(self, point: Point) -> f64 {
        match self {
            Axis::X => point.0,
            Axis::Y => point.1,
        }
    }

    fn across(self, point: Point) -> f64 {
        match self {
            Axis::X => point.1,
            Axis::Y => point.0,
        }
    }
}

/// Whether two gates are drawn on the same two parameters, either way round:
/// on the same plot, when they are under the same parent.
pub fn same_axes(a: &(Arc<str>, Arc<str>), b: &(Arc<str>, Arc<str>)) -> bool {
    (a.0 == b.0 && a.1 == b.1) || (a.0 == b.1 && a.1 == b.0)
}

/// The outline of the gate `gate_id` names, on a plot of `x` by `y`. `None`
/// for a gate with no outline of its own, such as a boolean gate, or one not
/// drawn on those two parameters.
pub fn outline(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    x: &str,
    y: &str,
) -> Option<Vec<Point>> {
    let shape = gate
        .get_gate_ref(Some(gate_id))
        .or_else(|| gate.get_gate_ref(None))?;
    let at = |node: &flow_gates::GateNode| {
        Some((
            f64::from(node.get_coordinate(x)?),
            f64::from(node.get_coordinate(y)?),
        ))
    };
    match &shape.geometry {
        GateGeometry::Polygon { nodes, .. } => nodes.iter().map(at).collect(),
        GateGeometry::Rectangle { min, max } => {
            let (low, high) = (at(min)?, at(max)?);
            Some(vec![
                (low.0, low.1),
                (high.0, low.1),
                (high.0, high.1),
                (low.0, high.1),
            ])
        }
        GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } => {
            // The radii are along the gate's own two parameters, in order.
            let (own_x, _) = &shape.parameters;
            let centre = at(center)?;
            let (rx, ry, turn) = (
                f64::from(*radius_x),
                f64::from(*radius_y),
                f64::from(*angle),
            );
            let swapped = **own_x != *x;
            Some(
                (0..ELLIPSE_POINTS)
                    .map(|i| {
                        let t = std::f64::consts::TAU * i as f64 / ELLIPSE_POINTS as f64;
                        let (u, v) = (rx * t.cos(), ry * t.sin());
                        let (a, b) = (
                            u * turn.cos() - v * turn.sin(),
                            u * turn.sin() + v * turn.cos(),
                        );
                        if swapped {
                            (centre.0 + b, centre.1 + a)
                        } else {
                            (centre.0 + a, centre.1 + b)
                        }
                    })
                    .collect(),
            )
        }
        GateGeometry::Boolean { .. } => None,
    }
}

/// Beyond this a coordinate is an edge left open to the end of the axis,
/// and says nothing about the size of the gate.
pub(crate) const OPEN: f64 = 1e8;

/// The smallest distance worth telling apart from none, for outlines of
/// this size: a gate's points are kept to about seven figures, so a gate
/// placed touching another may come back a hair over it.
fn tolerance(a: &[Point], b: &[Point]) -> f64 {
    let size = a
        .iter()
        .chain(b)
        .flat_map(|p| [p.0.abs(), p.1.abs()])
        .filter(|v| *v < OPEN)
        .fold(1.0_f64, f64::max);
    size * 1e-6
}

fn edges(outline: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    (0..outline.len()).map(move |i| (outline[i], outline[(i + 1) % outline.len()]))
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Whether two segments cross at a point inside both - touching at an end,
/// or running along each other, is not crossing.
fn cross_properly(p: (Point, Point), q: (Point, Point), eps: f64) -> bool {
    let d1 = cross(q.0, q.1, p.0);
    let d2 = cross(q.0, q.1, p.1);
    let d3 = cross(p.0, p.1, q.0);
    let d4 = cross(p.0, p.1, q.1);
    let scale = |a: Point, b: Point| ((b.0 - a.0).hypot(b.1 - a.1)).max(1.0);
    let (eq, ep) = (eps * scale(q.0, q.1), eps * scale(p.0, p.1));
    ((d1 > eq && d2 < -eq) || (d1 < -eq && d2 > eq))
        && ((d3 > ep && d4 < -ep) || (d3 < -ep && d4 > ep))
}

/// The point of the side from `a` to `b` nearest to `point`.
pub fn nearest_on_side(point: Point, (a, b): (Point, Point)) -> Point {
    let along = (b.0 - a.0, b.1 - a.1);
    let length = along.0 * along.0 + along.1 * along.1;
    let t = if length > 0.0 {
        (((point.0 - a.0) * along.0 + (point.1 - a.1) * along.1) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a.0 + t * along.0, a.1 + t * along.1)
}

/// Whether `point` is inside `outline`, counting by the edges a line out
/// from it crosses.
pub fn inside(point: Point, outline: &[Point]) -> bool {
    let mut inside = false;
    for (a, b) in edges(outline) {
        if (a.1 > point.1) != (b.1 > point.1) {
            let x = a.0 + (point.1 - a.1) / (b.1 - a.1) * (b.0 - a.0);
            if point.0 < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Whether `point` is inside `outline` and not on its edge.
fn strictly_inside(point: Point, outline: &[Point], eps: f64) -> bool {
    let on_edge = edges(outline).any(|(a, b)| {
        let length = (b.0 - a.0).hypot(b.1 - a.1).max(eps);
        let off = cross(a, b, point).abs() / length;
        let within = (point.0 - a.0) * (b.0 - a.0) + (point.1 - a.1) * (b.1 - a.1);
        off <= eps && within >= -eps && within <= length * length + eps
    });
    !on_edge && inside(point, outline)
}

fn centre(outline: &[Point]) -> Point {
    let n = outline.len() as f64;
    let (x, y) = outline
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p.0, y + p.1));
    (x / n, y / n)
}

/// Whether two outlines share any area. Touching - along an edge or at a
/// point - is not overlapping.
pub fn overlaps(a: &[Point], b: &[Point]) -> bool {
    overlaps_by_more_than(a, b, tolerance(a, b))
}

/// [`overlaps`], by a tenth of its tolerance: what a search for a place
/// clear of a gate asks, so that the place it finds is still clear once
/// its points are stored.
pub fn nearly_overlaps(a: &[Point], b: &[Point]) -> bool {
    overlaps_by_more_than(a, b, tolerance(a, b) / 10.0)
}

fn overlaps_by_more_than(a: &[Point], b: &[Point], eps: f64) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    let midpoints = |outline: &[Point]| -> Vec<Point> {
        edges(outline)
            .map(|(p, q)| ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0))
            .collect()
    };
    edges(a).any(|p| edges(b).any(|q| cross_properly(p, q, eps)))
        || a.iter()
            .chain(&midpoints(a))
            .chain(std::iter::once(&centre(a)))
            .any(|p| strictly_inside(*p, b, eps))
        || b.iter()
            .chain(&midpoints(b))
            .chain(std::iter::once(&centre(b)))
            .any(|p| strictly_inside(*p, a, eps))
}

/// How far along a ray from `from`, heading `along` the axis in `sign`'s
/// direction, it first meets the segment.
fn ray_hits(from: Point, axis: Axis, sign: f64, segment: (Point, Point)) -> Option<f64> {
    let (a, b) = segment;
    let (fa, fb) = (axis.across(a), axis.across(b));
    let level = axis.across(from);
    if (fa - level) * (fb - level) > 0.0 {
        return None;
    }
    let at = if fa == fb {
        // Lying along the ray: the nearer end.
        let (na, nb) = (axis.along(a), axis.along(b));
        if sign > 0.0 { na.min(nb) } else { na.max(nb) }
    } else {
        let t = (level - fa) / (fb - fa);
        axis.along(a) + t * (axis.along(b) - axis.along(a))
    };
    let distance = (at - axis.along(from)) * sign;
    (distance >= 0.0).then_some(distance)
}

/// How far `moving` can slide along `axis`, in `sign`'s direction, before it
/// touches `fixed`. `None` when it never would: nothing of `fixed` lies
/// across its path.
pub fn contact_distance(moving: &[Point], fixed: &[Point], axis: Axis, sign: f64) -> Option<f64> {
    let forward = moving
        .iter()
        .flat_map(|p| edges(fixed).filter_map(move |e| ray_hits(*p, axis, sign, e)));
    let backward = fixed
        .iter()
        .flat_map(|p| edges(moving).filter_map(move |e| ray_hits(*p, axis, -sign, e)));
    forward.chain(backward).min_by(f64::total_cmp)
}

/// The lowest and highest an outline reaches along `axis`.
pub fn extent(outline: &[Point], axis: Axis) -> (f64, f64) {
    outline.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
        (lo.min(axis.along(*p)), hi.max(axis.along(*p)))
    })
}
