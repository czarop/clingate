//! The menu a right click on a gate opens: on a polygon's side, make it
//! horizontal or vertical or add a point there; on a polygon's point, delete
//! it; on a rectangle, make it a polygon. A point edit changes the position
//! shown, as a drag does; a rectangle becomes a polygon at every position.
//! Each is one step of the working copy.

use std::sync::Arc;

use clingate_core::axis_store::PlotMapper;
use clingate_core::gates::gate_contact::{Point, outline};
use clingate_core::gates::gate_shape_edit::{level_side, with_point_added, without_point};
use clingate_core::gates::gate_single::rectangle_gate::RectangleGate;
use clingate_core::gates::gate_store::GateOverrideResolver;
use clingate_core::gates::gate_traits::DrawableGate;
use dioxus::prelude::*;
use flow_gates::GateGeometry;

use crate::gate_editor::edits::Edits;
use crate::gate_editor::position_menu::reposition;
use crate::gate_editor::workspace_window::GateStore;

/// How near, in pixels, a right click must be to a point or a side.
const POINT_REACH: f32 = 6.0;
const SIDE_REACH: f32 = 5.0;

/// What a right click landed on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ShapeTarget {
    Point(usize),
    Side(usize),
    Rectangle,
}

/// An open menu: the gate, what of it was clicked, and where - in the plot's
/// pixels, and on its two parameters.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapeMenu {
    pub gate_id: Arc<str>,
    pub target: ShapeTarget,
    pub at_pixel: (f32, f32),
    pub at: (f32, f32),
}

/// What the menu does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ShapeAction {
    MakeHorizontal,
    MakeVertical,
    AddPoint,
    DeletePoint,
    ConvertToPolygon,
}

impl ShapeAction {
    /// The actions offered on `target`, with what the menu calls them.
    pub(crate) fn offered(target: &ShapeTarget) -> &'static [(ShapeAction, &'static str)] {
        match target {
            ShapeTarget::Side(_) => &[
                (ShapeAction::MakeHorizontal, "Make horizontal"),
                (ShapeAction::MakeVertical, "Make vertical"),
                (ShapeAction::AddPoint, "Add point"),
            ],
            ShapeTarget::Point(_) => &[(ShapeAction::DeletePoint, "Delete point")],
            ShapeTarget::Rectangle => &[(ShapeAction::ConvertToPolygon, "Convert to polygon")],
        }
    }
}

fn is_polygon(gate: &Arc<dyn DrawableGate>) -> bool {
    gate.get_gate_ref(None)
        .is_some_and(|shape| matches!(shape.geometry, GateGeometry::Polygon { .. }))
}

fn is_rectangle(gate: &Arc<dyn DrawableGate>) -> bool {
    gate.as_any().downcast_ref::<RectangleGate>().is_some()
}

fn distance(a: Point, b: Point) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn distance_to_side(point: Point, (a, b): (Point, Point)) -> f64 {
    let along = (b.0 - a.0, b.1 - a.1);
    let length = along.0 * along.0 + along.1 * along.1;
    let t = if length > 0.0 {
        (((point.0 - a.0) * along.0 + (point.1 - a.1) * along.1) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance(point, (a.0 + t * along.0, a.1 + t * along.1))
}

fn inside(point: Point, outline: &[Point]) -> bool {
    let mut inside = false;
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        if (a.1 > point.1) != (b.1 > point.1)
            && point.0 < a.0 + (point.1 - a.1) / (b.1 - a.1) * (b.0 - a.0)
        {
            inside = !inside;
        }
    }
    inside
}

/// A gate, its id and its outline in the plot's pixels.
type DrawnGate<'g> = (Arc<str>, &'g Arc<dyn DrawableGate>, Vec<Point>);

/// What of `gates`, drawn on the plot of `x` by `y`, the right click at
/// `pixel` landed on: a polygon's point, then a polygon's side, then a
/// rectangle - the selected gate before the others.
pub(crate) fn target_at(
    gates: &[Arc<dyn DrawableGate>],
    selected: Option<&Arc<str>>,
    (x, y): (&str, &str),
    mapper: &PlotMapper,
    pixel: (f32, f32),
) -> Option<(Arc<str>, ShapeTarget)> {
    let click = (f64::from(pixel.0), f64::from(pixel.1));
    let mut ordered: Vec<&Arc<dyn DrawableGate>> = gates.iter().collect();
    ordered.sort_by_key(|gate| Some(&gate.get_id()) != selected);
    let drawn: Vec<DrawnGate> = ordered
        .into_iter()
        .filter_map(|gate| {
            let id = gate.get_id();
            let points = outline(gate, &id, x, y)?
                .into_iter()
                .map(|(dx, dy)| {
                    let (px, py) = mapper.data_to_pixel(dx as f32, dy as f32, None, None);
                    (f64::from(px), f64::from(py))
                })
                .collect();
            Some((id, gate, points))
        })
        .collect();
    let polygons = || drawn.iter().filter(|(_, gate, _)| is_polygon(gate));
    let point = polygons().find_map(|(id, _, points)| {
        let index = points
            .iter()
            .position(|p| distance(*p, click) <= f64::from(POINT_REACH))?;
        Some((id.clone(), ShapeTarget::Point(index)))
    });
    let side = || {
        polygons().find_map(|(id, _, points)| {
            let index = (0..points.len()).find(|i| {
                let side = (points[*i], points[(i + 1) % points.len()]);
                distance_to_side(click, side) <= f64::from(SIDE_REACH)
            })?;
            Some((id.clone(), ShapeTarget::Side(index)))
        })
    };
    let rectangle = || {
        drawn
            .iter()
            .filter(|(_, gate, _)| is_rectangle(gate))
            .find(|(_, _, points)| inside(click, points))
            .map(|(id, _, _)| (id.clone(), ShapeTarget::Rectangle))
    };
    point.or_else(side).or_else(rectangle)
}

/// `action` on the gate `menu` was opened on, as one step of the working
/// copy. A point edit is made to the position `resolver` shows, kept apart
/// from `apart_from`.
pub(crate) fn act(
    gates: GateStore,
    edits: Edits,
    resolver: &GateOverrideResolver,
    apart_from: &[Arc<dyn DrawableGate>],
    (x, y): (&str, &str),
    menu: &ShapeMenu,
    action: ShapeAction,
) -> Result<(), String> {
    let id = menu.gate_id.clone();
    let index = match menu.target {
        ShapeTarget::Side(index) | ShapeTarget::Point(index) => index,
        ShapeTarget::Rectangle => 0,
    };
    reposition(gates, edits, |state| {
        let mut reshape =
            |edit: &dyn Fn(&Arc<dyn DrawableGate>) -> anyhow::Result<Arc<dyn DrawableGate>>| {
                state.reshape_gate(&id, resolver, apart_from, edit)
            };
        match action {
            ShapeAction::ConvertToPolygon => state.convert_to_polygon(&id),
            ShapeAction::MakeHorizontal => reshape(&|g| level_side(g, &id, index, y)),
            ShapeAction::MakeVertical => reshape(&|g| level_side(g, &id, index, x)),
            ShapeAction::AddPoint => reshape(&|g| with_point_added(g, &id, index, (x, y), menu.at)),
            ShapeAction::DeletePoint => reshape(&|g| without_point(g, &id, index)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clingate_core::gates::gate_single::polygon_gate::PolygonGate;
    use flow_fcs::TransformType;

    const X: &str = "FSC-A";
    const Y: &str = "SSC-A";

    fn mapper() -> PlotMapper {
        PlotMapper::new(
            600.0,
            600.0,
            0.0..=1000.0,
            0.0..=1000.0,
            0.0..=1000.0,
            0.0..=1000.0,
            TransformType::Linear,
            TransformType::Linear,
        )
    }

    fn flow_gate(id: &str, geometry: GateGeometry) -> flow_gates::Gate {
        flow_gates::Gate {
            id: Arc::from(id),
            name: id.to_string(),
            geometry,
            mode: flow_gates::GateMode::Global,
            parameters: (Arc::from(X), Arc::from(Y)),
            label_position: None,
        }
    }

    fn polygon(id: &str, points: &[(f32, f32)]) -> Arc<dyn DrawableGate> {
        let geometry = flow_gates::create_polygon_geometry(points.to_vec(), X, Y).unwrap();
        Arc::new(PolygonGate::try_new(flow_gate(id, geometry), true).unwrap())
    }

    fn rectangle(id: &str, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> Arc<dyn DrawableGate> {
        let corners = vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        let geometry = flow_gates::create_rectangle_geometry(corners, X, Y).unwrap();
        Arc::new(RectangleGate::try_new(flow_gate(id, geometry), true).unwrap())
    }

    fn pixel(at: (f32, f32)) -> (f32, f32) {
        mapper().data_to_pixel(at.0, at.1, None, None)
    }

    fn clicked(
        gates: &[Arc<dyn DrawableGate>],
        selected: Option<&str>,
        at: (f32, f32),
    ) -> Option<(String, ShapeTarget)> {
        let selected = selected.map(Arc::<str>::from);
        target_at(gates, selected.as_ref(), (X, Y), &mapper(), at)
            .map(|(id, target)| (id.to_string(), target))
    }

    fn triangle() -> Arc<dyn DrawableGate> {
        polygon("t", &[(100.0, 100.0), (500.0, 100.0), (300.0, 500.0)])
    }

    #[test]
    fn a_click_on_a_point_opens_the_point_menu() {
        let near = pixel((500.0, 100.0));
        let at = (near.0 + 3.0, near.1 - 3.0);
        assert_eq!(
            clicked(&[triangle()], None, at),
            Some(("t".into(), ShapeTarget::Point(1)))
        );
    }

    #[test]
    fn a_click_by_a_side_opens_the_side_menu() {
        let middle = pixel((300.0, 100.0));
        let below = (middle.0, middle.1 + 4.0);
        assert_eq!(
            clicked(&[triangle()], None, below),
            Some(("t".into(), ShapeTarget::Side(0)))
        );
        let last = pixel((200.0, 300.0));
        assert_eq!(
            clicked(&[triangle()], None, last),
            Some(("t".into(), ShapeTarget::Side(2))),
            "the side back to the first point"
        );
    }

    #[test]
    fn a_click_inside_a_polygon_away_from_its_outline_opens_nothing() {
        assert_eq!(clicked(&[triangle()], None, pixel((300.0, 250.0))), None);
    }

    #[test]
    fn a_click_inside_a_rectangle_offers_to_make_it_a_polygon() {
        let rect = rectangle("r", (600.0, 600.0), (900.0, 900.0));
        assert_eq!(
            clicked(std::slice::from_ref(&rect), None, pixel((750.0, 750.0))),
            Some(("r".into(), ShapeTarget::Rectangle))
        );
        assert_eq!(clicked(&[rect], None, pixel((550.0, 750.0))), None);
    }

    /// Two polygons sharing a corner: the selected one's point is the one
    /// clicked, whichever is listed first.
    #[test]
    fn the_selected_gate_is_found_first() {
        let other = polygon("o", &[(500.0, 100.0), (900.0, 100.0), (700.0, 500.0)]);
        let gates = [other, triangle()];
        let corner = pixel((500.0, 100.0));
        assert_eq!(
            clicked(&gates, Some("t"), corner),
            Some(("t".into(), ShapeTarget::Point(1)))
        );
        assert_eq!(
            clicked(&gates, Some("o"), corner),
            Some(("o".into(), ShapeTarget::Point(0)))
        );
    }

    #[test]
    fn a_side_offers_both_levels_and_a_point_a_point_only_deletion() {
        let names = |target| {
            ShapeAction::offered(&target)
                .iter()
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(ShapeTarget::Side(0)),
            ["Make horizontal", "Make vertical", "Add point"]
        );
        assert_eq!(names(ShapeTarget::Point(0)), ["Delete point"]);
        assert_eq!(names(ShapeTarget::Rectangle), ["Convert to polygon"]);
    }
}
