//! Where a gate placed next to another goes: against it along one axis, as
//! close as it can be without overlapping it.
//!
//! Worked in coordinates turned so the other gate always lies ahead: `u`
//! runs along the chosen axis towards it, `v` across. The side away from it
//! is then the low `u` side, whichever side of it the gate is on.

use std::sync::Arc;

use flow_gates::GateGeometry;

use crate::gate_rules::autogate::{UNBOUNDED, rebuild, translate_by};
use crate::gate_rules::rule::{Meet, NextToRule, Side};
use crate::gates::gate_contact::{
    Axis, OPEN, Point, contact_distance, extent, nearly_overlaps, outline,
};
use crate::gates::gate_store::GateId;
use crate::gates::gate_traits::DrawableGate;

/// The turn into `u`, `v` coordinates, and back.
struct Frame {
    axis: Axis,
    sign: f64,
}

impl Frame {
    fn to_uv(&self, p: Point) -> Point {
        match self.axis {
            Axis::X => (self.sign * p.0, p.1),
            Axis::Y => (self.sign * p.1, p.0),
        }
    }

    fn from_uv(&self, (u, v): Point) -> Point {
        match self.axis {
            Axis::X => (self.sign * u, v),
            Axis::Y => (v, self.sign * u),
        }
    }

    fn all_to_uv(&self, points: &[Point]) -> Vec<Point> {
        points.iter().map(|p| self.to_uv(*p)).collect()
    }
}

/// `gate` placed next to the gate whose outline is `against`, as `rule` says.
pub(crate) fn placed_next_to(
    gate: &Arc<dyn DrawableGate>,
    gate_id: &GateId,
    against: &[Point],
    rule: &NextToRule,
) -> Result<Arc<dyn DrawableGate>, String> {
    let other = rule.anchor.describe();
    let inner = gate
        .get_gate_ref(Some(gate_id))
        .or_else(|| gate.get_gate_ref(None))
        .ok_or_else(|| "this gate has no shape".to_string())?;
    let rectangle = match &inner.geometry {
        GateGeometry::Rectangle { .. } => true,
        GateGeometry::Polygon { .. } => false,
        _ => {
            return Err(
                "a gate is placed next to another only if it is a rectangle or a polygon".into(),
            );
        }
    };
    let (x, y) = gate.get_params();
    let axis = if *rule.parameter == *x {
        Axis::X
    } else if *rule.parameter == *y {
        Axis::Y
    } else {
        return Err(format!(
            "this gate is drawn on {x} and {y}, so it does not move along {}",
            rule.parameter
        ));
    };
    let frame = Frame {
        axis,
        sign: match rule.side {
            Side::Lower => 1.0,
            Side::Upper => -1.0,
        },
    };
    let shape =
        outline(gate, gate_id, &x, &y).ok_or_else(|| "this gate has no shape".to_string())?;
    if shape
        .iter()
        .any(|p| p.0.abs() > f64::from(UNBOUNDED) || p.1.abs() > f64::from(UNBOUNDED))
    {
        return Err("this gate runs to the end of an axis, so it has no side to bring up".into());
    }
    let mine = frame.all_to_uv(&shape);
    let theirs = frame.all_to_uv(against);
    match (rule.meet, rectangle) {
        (Meet::Slide, _) => {
            let by = slide_by(&mine, &theirs, rule.gap, &other)?;
            translate_by(gate, &rule.parameter, frame.sign * by).map_err(|e| e.to_string())
        }
        (Meet::FollowOutline, false) => {
            let followed = follow_outline(&mine, &theirs, rule.gap, &other)?;
            let points: Vec<(f32, f32)> = followed
                .iter()
                .map(|p| {
                    let (a, b) = frame.from_uv(*p);
                    (a as f32, b as f32)
                })
                .collect();
            let geometry =
                flow_gates::create_polygon_geometry(points, &x, &y).map_err(|e| e.to_string())?;
            with_geometry(gate, inner, geometry)
        }
        (Meet::GrowSide | Meet::FollowOutline, _) => {
            let by = grow_by(&mine, &theirs, rule.gap, rectangle, &other)?;
            let geometry = grown(
                &inner.geometry,
                &rule.parameter,
                &mine,
                frame.sign,
                by,
                rectangle,
            );
            with_geometry(gate, inner, geometry)
        }
    }
}

fn with_geometry(
    gate: &Arc<dyn DrawableGate>,
    inner: &flow_gates::Gate,
    geometry: GateGeometry,
) -> Result<Arc<dyn DrawableGate>, String> {
    let mut moved = inner.clone();
    moved.geometry = geometry;
    rebuild(gate, moved).map_err(|e| e.to_string())
}

fn never_level(other: &str) -> String {
    format!("it never comes level with {other} along that axis, so it cannot meet it")
}

/// How far along `u` the whole gate slides to touch the other, `gap` short.
fn slide_by(mine: &[Point], theirs: &[Point], gap: f64, other: &str) -> Result<f64, String> {
    let (lo, hi) = extent(mine, Axis::X);
    let (their_lo, their_hi) = extent(theirs, Axis::X);
    // Back to where it is wholly behind the other, then forward to touch.
    let behind = their_lo - hi - (hi - lo + their_hi - their_lo + 1.0);
    let start: Vec<Point> = mine.iter().map(|p| (p.0 + behind, p.1)).collect();
    let reach = contact_distance(&start, theirs, Axis::X, 1.0).ok_or_else(|| never_level(other))?;
    Ok(behind + reach - gap)
}

/// The points of the side facing the other gate: past the middle of the
/// gate along `u` - for a rectangle, its facing edge.
fn facing(mine: &[Point]) -> impl Fn(f64) -> bool {
    let (lo, hi) = extent(mine, Axis::X);
    let middle = (lo + hi) / 2.0;
    move |u| u > middle
}

/// `mine` with its facing points moved `by` along `u`.
fn grown_outline(mine: &[Point], by: f64) -> Vec<Point> {
    let ahead = facing(mine);
    mine.iter()
        .map(|p| if ahead(p.0) { (p.0 + by, p.1) } else { *p })
        .collect()
}

/// How far the facing side moves along `u` to touch the other, `gap` short:
/// out to it, or back from it where the gate overlaps it.
fn grow_by(
    mine: &[Point],
    theirs: &[Point],
    gap: f64,
    rectangle: bool,
    other: &str,
) -> Result<f64, String> {
    let (lo, hi) = extent(mine, Axis::X);
    // An edge open to the end of an axis is not somewhere to search to.
    let their_hi = theirs
        .iter()
        .map(|p| p.0)
        .filter(|u| u.abs() < OPEN)
        .fold(f64::MIN, f64::max);
    let ahead = facing(mine);
    let nearest_facing = mine
        .iter()
        .map(|p| p.0)
        .filter(|u| ahead(*u))
        .fold(f64::MAX, f64::min);
    let room = if rectangle {
        hi - lo
    } else {
        nearest_facing - (lo + hi) / 2.0
    };
    let clear = |by: f64| !nearly_overlaps(&grown_outline(mine, by), theirs);
    let mut back = -room * 0.99;
    let mut on = their_hi - nearest_facing + (hi - lo) + 1.0;
    if !clear(back) {
        return Err(format!(
            "it overlaps {other} however far its side comes back"
        ));
    }
    if clear(on) {
        return Err(never_level(other));
    }
    for _ in 0..60 {
        let middle = (back + on) / 2.0;
        if clear(middle) {
            back = middle;
        } else {
            on = middle;
        }
    }
    Ok(back - gap)
}

/// `geometry` with its facing side moved `by` along `u`, in data units.
fn grown(
    geometry: &GateGeometry,
    parameter: &str,
    mine: &[Point],
    sign: f64,
    by: f64,
    rectangle: bool,
) -> GateGeometry {
    let ahead = facing(mine);
    let shift = |node: &mut flow_gates::GateNode| {
        if let Some(at) = node.get_coordinate(parameter) {
            node.set_coordinate(parameter, (f64::from(at) + sign * by) as f32);
        }
    };
    let mut moved = geometry.clone();
    match &mut moved {
        GateGeometry::Rectangle { min, max } if rectangle => {
            shift(if sign > 0.0 { max } else { min });
        }
        GateGeometry::Polygon { nodes, .. } => {
            for node in nodes.iter_mut() {
                let u = node
                    .get_coordinate(parameter)
                    .map(|at| sign * f64::from(at));
                if u.is_some_and(&ahead) {
                    shift(node);
                }
            }
        }
        _ => {}
    }
    moved
}

/// The least `u` of `outline` along the line across at `v`.
fn nearest_at(outline: &[Point], v: f64) -> Option<f64> {
    (0..outline.len())
        .filter_map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
            if (a.1 - v) * (b.1 - v) > 0.0 {
                return None;
            }
            Some(if a.1 == b.1 {
                a.0.min(b.0)
            } else {
                a.0 + (v - a.1) / (b.1 - a.1) * (b.0 - a.0)
            })
        })
        .min_by(f64::total_cmp)
}

/// The furthest `u` of `chain` at `v`, where it reaches that level.
fn furthest_at(chain: &[Point], v: f64) -> Option<f64> {
    chain
        .windows(2)
        .filter_map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            if (a.1 - v) * (b.1 - v) > 0.0 {
                return None;
            }
            Some(if a.1 == b.1 {
                a.0.max(b.0)
            } else {
                a.0 + (v - a.1) / (b.1 - a.1) * (b.0 - a.0)
            })
        })
        .max_by(f64::total_cmp)
}

/// `outline`'s two sides between its lowest and highest points: the side
/// towards the other gate from bottom to top, and the side away from top to
/// bottom.
fn sides(outline: &[Point]) -> (Vec<Point>, Vec<Point>) {
    let n = outline.len();
    let lowest = (0..n)
        .min_by(|a, b| {
            outline[*a]
                .1
                .total_cmp(&outline[*b].1)
                .then(outline[*a].0.total_cmp(&outline[*b].0))
        })
        .unwrap_or(0);
    let highest = (0..n)
        .max_by(|a, b| {
            outline[*a]
                .1
                .total_cmp(&outline[*b].1)
                .then(outline[*b].0.total_cmp(&outline[*a].0))
        })
        .unwrap_or(0);
    let walk = |from: usize, to: usize| -> Vec<Point> {
        let steps = (to + n - from) % n;
        (0..=steps).map(|k| outline[(from + k) % n]).collect()
    };
    let (up, down) = (walk(lowest, highest), walk(highest, lowest));
    let mean = |side: &[Point]| side.iter().map(|p| p.0).sum::<f64>() / side.len() as f64;
    if mean(&up) >= mean(&down) {
        (up, down)
    } else {
        let mut near: Vec<Point> = down;
        near.reverse();
        let mut far = up;
        far.reverse();
        (near, far)
    }
}

/// `mine` with its facing side, where it lies alongside the other gate,
/// replaced by the other's outline `gap` short of it: steps where that
/// stretch begins and ends, and the rest of the gate as it was.
fn follow_outline(
    mine: &[Point],
    theirs: &[Point],
    gap: f64,
    other: &str,
) -> Result<Vec<Point>, String> {
    let (my_lo, my_hi) = extent(mine, Axis::Y);
    let (their_lo, their_hi) = extent(theirs, Axis::Y);
    let (lo, hi) = (my_lo.max(their_lo), my_hi.min(their_hi));
    if hi <= lo {
        return Err(never_level(other));
    }
    let mut levels: Vec<f64> = theirs
        .iter()
        .map(|p| p.1)
        .filter(|v| *v > lo && *v < hi)
        .chain([lo, hi])
        .collect();
    levels.sort_by(f64::total_cmp);
    levels.dedup();
    let along: Vec<Point> = levels
        .iter()
        .filter_map(|v| Some((nearest_at(theirs, *v)? - gap, *v)))
        .collect();

    let (near, far) = sides(mine);
    let at = |v: f64| {
        furthest_at(&near, v)
            .map(|u| (u, v))
            .ok_or_else(|| never_level(other))
    };
    let mut followed: Vec<Point> = near.iter().copied().filter(|p| p.1 < lo).collect();
    followed.push(at(lo)?);
    followed.extend(along);
    followed.push(at(hi)?);
    followed.extend(near.iter().copied().filter(|p| p.1 > hi));
    followed.extend(far);
    followed.dedup();
    if followed.len() > 1 && followed.first() == followed.last() {
        followed.pop();
    }
    Ok(followed)
}
