//! Tests for carrying gates through a change of scaling.
//!
//! Gate coordinates are held in the transformed space a plot is drawn in. A
//! new cofactor - typed into the editor, or arriving with a replacement scaling
//! file - would otherwise leave every gate at the same place on screen and
//! around different cells. The rescale sends each point back to raw data
//! through the old transform and out through the new one.
//!
//! The property that matters is the one a person relies on: **a gate admits
//! the same events after a rescale as before it.** That is tested directly -
//! raw events are put into display space through each transform, and the gate
//! is asked which it holds - for every gate type, across every tier a position
//! can live in, and for gates drawn after the rescale as well as before.
//!
//! Where a shape cannot keep that exactly, the tests say so and hold it to
//! what it can: a polygon's vertices and an ellipse's centre come across
//! exactly, but a straight edge drawn on one scale is a curve on another, so
//! the cells near a slanted edge can change sides. That is a property of
//! drawing on a transformed axis, not of the rescale.

#![cfg(test)]

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::gate_composite::bisector_gate::BisectorGate;
use crate::gate_editor::gates::gate_composite::quadrant_gate::QuadrantGate;
use crate::gate_editor::gates::gate_composite::skewed_quadrant_gate::SkewedQuadrantGate;
use crate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate;
use crate::gate_editor::gates::gate_single::line_gate::LineGate;
use crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_store::{GateSource, GateSubStore};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::plots::axis_store::{Param, PlotMapper};
use crate::omiq::metadata::MetaDataKey;
use flow_fcs::{TransformType, Transformable};
use flow_gates::{GateGeometry, create_polygon_geometry, create_rectangle_geometry};
use std::sync::Arc;

/// The channel being rescaled: a fluorescence channel, on arcsinh.
const X: &str = "CD3";
/// The other axis, which is linear throughout and must never move.
const Y: &str = "SSC-A";

const OLD: TransformType = TransformType::Arcsinh { cofactor: 150.0 };
const NEW: TransformType = TransformType::Arcsinh { cofactor: 1000.0 };

/// The raw range both axes cover, in both scalings.
const RAW_LOW: f32 = -500.0;
const RAW_HIGH: f32 = 200_000.0;

fn axis(transform: TransformType) -> AxisInfo {
    AxisInfo::new_from_raw(
        Param {
            marker: Arc::from(X),
            fluoro: Arc::from(X),
        },
        RAW_LOW,
        RAW_HIGH,
        transform,
    )
}

/// Where a raw value sits on the rescaled axis under `t`.
fn shown(t: &TransformType, raw: f32) -> f32 {
    t.transform(&raw)
}

fn raw_of(t: &TransformType, shown: f32) -> f32 {
    t.inverse_transform(&shown)
}

/// A mapper for a plot of X shown through `t` against a linear Y, built as
/// the plot window builds one. The composites read their constructors' click
/// in display space from it, and the skewed quadrant takes the reach of its
/// outer edges from its transforms.
fn mapper(t: &TransformType) -> PlotMapper {
    let (lo, hi) = (shown(t, RAW_LOW), shown(t, RAW_HIGH));
    PlotMapper::new(
        600.0,
        600.0,
        lo..=hi,
        0.0..=1000.0,
        lo..=hi,
        0.0..=1000.0,
        t.clone(),
        TransformType::Linear,
    )
}

fn inner(id: &str, geometry: GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from(id),
        name: format!("{id} name"),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

// ── the gates, each placed at raw positions and stored under OLD ─────────

fn rectangle(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    let (x1, x2) = (shown(t, 800.0), shown(t, 20_000.0));
    let geometry = create_rectangle_geometry(
        vec![(x1, 200.0), (x2, 200.0), (x2, 700.0), (x1, 700.0)],
        X,
        Y,
    )
    .unwrap();
    Arc::new(RectangleGate::try_new(inner(id, geometry), true).unwrap())
}

fn triangle(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    let geometry = create_polygon_geometry(
        vec![
            (shown(t, 500.0), 150.0),
            (shown(t, 60_000.0), 150.0),
            (shown(t, 5_000.0), 800.0),
        ],
        X,
        Y,
    )
    .unwrap();
    Arc::new(PolygonGate::try_new(inner(id, geometry), true).unwrap())
}

fn ellipse(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    let geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
        (f64::from(shown(t, 1_000.0)), 500.0),
        (f64::from(shown(t, 30_000.0)), 500.0),
        (f64::from(shown(t, 6_000.0)), 650.0),
        X,
        Y,
    )
    .unwrap();
    Arc::new(EllipseGate::try_new(inner(id, geometry), true).unwrap())
}

/// An ellipse leaning across the plot: neither of its axes lies along X.
fn tilted_ellipse(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    let geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
        (f64::from(shown(t, 1_000.0)), 300.0),
        (f64::from(shown(t, 30_000.0)), 700.0),
        (f64::from(shown(t, 3_000.0)), 800.0),
        X,
        Y,
    )
    .unwrap();
    Arc::new(EllipseGate::try_new(inner(id, geometry), true).unwrap())
}

/// An ellipse given directly in canonical form, centred at raw 5,000.
fn canonical_ellipse(id: &str, radius_x: f32, radius_y: f32, angle: f32) -> Arc<dyn DrawableGate> {
    let mut center = flow_gates::GateNode::new("ellipse_center");
    center.set_coordinate(Arc::from(X), shown(&OLD, 5_000.0));
    center.set_coordinate(Arc::from(Y), 500.0);
    let geometry = GateGeometry::Ellipse {
        center,
        radius_x,
        radius_y,
        angle,
    };
    Arc::new(EllipseGate::try_new(inner(id, geometry), true).unwrap())
}

fn ellipse_form(gate: &Arc<dyn DrawableGate>) -> (f32, f32, f32, f32) {
    match &gate.get_gate_ref(None).expect("a gate").geometry {
        GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } => (
            center.get_coordinate(X).unwrap(),
            *radius_x,
            *radius_y,
            *angle,
        ),
        other => panic!("not an ellipse: {other:?}"),
    }
}

fn line(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    let (x1, x2) = (shown(t, 2_000.0), shown(t, 90_000.0));
    let geometry =
        create_rectangle_geometry(vec![(x1, -1e16), (x2, -1e16), (x2, 1e16), (x1, 1e16)], X, Y)
            .unwrap();
    Arc::new(LineGate::try_new(inner(id, geometry), 500.0, true).unwrap())
}

/// A pixel position whose X is `raw` under `t`, for the composite
/// constructors, which take a click rather than a data coordinate.
fn pixel_at(t: &TransformType, raw: f32) -> (f32, f32) {
    let (lo, hi) = (shown(t, RAW_LOW), shown(t, RAW_HIGH));
    ((shown(t, raw) - lo) / (hi - lo) * 600.0, 300.0)
}

fn quadrant(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    Arc::new(
        QuadrantGate::try_new_from_raw_coord(
            &mapper(t),
            Arc::from(id),
            format!("{id} name"),
            pixel_at(t, 3_000.0),
            Arc::from(X),
            Arc::from(Y),
        )
        .unwrap(),
    )
}

fn skewed(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    let map = mapper(t);
    let square = SkewedQuadrantGate::try_new_from_raw_coord(
        &map,
        Arc::from(id),
        format!("{id} name"),
        pixel_at(t, 3_000.0),
        Arc::from(X),
        Arc::from(Y),
    )
    .unwrap();
    // A fresh one is a plain quadrant; tilt both arms, as a person would by
    // dragging the top and right handles, so its dividing lines are slanted.
    let tilted = square
        .replace_point((shown(t, 20_000.0), 0.0), 4, None, &map)
        .unwrap()
        .replace_point((0.0, 800.0), 3, None, &map)
        .unwrap();
    Arc::from(tilted)
}

fn bisector(id: &str, t: &TransformType) -> Arc<dyn DrawableGate> {
    Arc::new(
        BisectorGate::try_new(
            &mapper(t),
            Arc::from(id),
            format!("{id} name"),
            pixel_at(t, 3_000.0),
            Arc::from(X),
            Arc::from(Y),
        )
        .unwrap(),
    )
}

// ── asking a gate what it holds ──────────────────────────────────────────

/// Raw events across the range, spaced irregularly so none sits exactly on
/// a boundary by construction.
fn events() -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut x = 37.0_f32;
    while x < 150_000.0 {
        let mut y = 13.7_f32;
        while y < 1000.0 {
            out.push((x, y));
            y += 41.3;
        }
        x *= 1.061;
        x += 7.3;
    }
    out
}

/// Which region of `gate` each raw event lands in, with X shown through `t`.
/// `None` for an event outside every region. A composite's regions are its
/// subgates, in order.
fn membership(gate: &Arc<dyn DrawableGate>, t: &TransformType) -> Vec<Option<usize>> {
    let regions: Vec<Option<Arc<str>>> = if gate.is_composite() {
        gate.get_inner_gate_ids().into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    events()
        .iter()
        .map(|(x_raw, y)| {
            let x = shown(t, *x_raw);
            regions.iter().position(|region| {
                gate.get_gate_ref(region.as_deref())
                    .is_some_and(|g| g.geometry.contains_point(x, *y, X, Y).unwrap_or(false))
            })
        })
        .collect()
}

/// The share of events assigned the same region before and after.
fn agreement(before: &[Option<usize>], after: &[Option<usize>]) -> f64 {
    let same = before.iter().zip(after).filter(|(a, b)| a == b).count();
    same as f64 / before.len() as f64
}

fn admitted(members: &[Option<usize>]) -> usize {
    members.iter().filter(|m| m.is_some()).count()
}

/// Rescale one gate on its own, through the same code the store uses.
fn rescaled(gate: &Arc<dyn DrawableGate>) -> Arc<dyn DrawableGate> {
    let mut store = GateSubStore::default();
    let ids = GateSubStore::ids_for(gate, &gate.get_id());
    store.insert_for_source(&ids, gate, &GateSource::Global);
    store
        .rescale_channel(&Arc::from(X), &axis(OLD), &axis(NEW))
        .expect("the gate can be rescaled");
    store
        .primary_and_subgate_registry
        .get(&gate.get_id())
        .expect("still registered")
        .clone()
}

fn points(gate: &Arc<dyn DrawableGate>) -> Vec<(f32, f32)> {
    let inner = gate.get_gate_ref(None).expect("a gate");
    match &inner.geometry {
        GateGeometry::Rectangle { min, max } => vec![
            (
                min.get_coordinate(X).unwrap(),
                min.get_coordinate(Y).unwrap(),
            ),
            (
                max.get_coordinate(X).unwrap(),
                max.get_coordinate(Y).unwrap(),
            ),
        ],
        GateGeometry::Polygon { nodes, .. } => nodes
            .iter()
            .map(|n| (n.get_coordinate(X).unwrap(), n.get_coordinate(Y).unwrap()))
            .collect(),
        GateGeometry::Ellipse { center, .. } => vec![(
            center.get_coordinate(X).unwrap(),
            center.get_coordinate(Y).unwrap(),
        )],
        GateGeometry::Boolean { .. } => Vec::new(),
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.0)
}

// ── the gates whose boundaries run along the axes: exact ──────────────────

#[test]
fn a_rectangle_admits_exactly_the_same_events() {
    let before = rectangle("r", &OLD);
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert!(
        admitted(&b) > 50,
        "the fixture should hold something: {}",
        admitted(&b)
    );
    assert_eq!(
        b, a,
        "a rectangle's edges run along the axes, so nothing may change sides"
    );
}

#[test]
fn a_rectangles_edges_keep_their_raw_values() {
    let before = rectangle("r", &OLD);
    let after = rescaled(&before);
    for ((bx, by), (ax, ay)) in points(&before).into_iter().zip(points(&after)) {
        assert!(close(raw_of(&OLD, bx), raw_of(&NEW, ax)), "{bx} -> {ax}");
        assert_eq!(by, ay, "the other axis must not move");
    }
}

#[test]
fn a_line_gate_admits_exactly_the_same_events() {
    let before = line("l", &OLD);
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert!(admitted(&b) > 50);
    assert_eq!(b, a);
}

#[test]
fn a_quadrant_sorts_every_event_into_the_same_quarter() {
    let before = quadrant("q", &OLD);
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert!(admitted(&b) > 100);
    assert_eq!(b, a, "a quadrant's split runs along the axes");
}

#[test]
fn a_bisector_sorts_every_event_onto_the_same_side() {
    let before = bisector("b", &OLD);
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert!(admitted(&b) > 100);
    assert_eq!(b, a);
}

// ── the gates with slanted or curved edges: as close as the shape allows ──

#[test]
fn a_polygons_vertices_keep_their_raw_values() {
    let before = triangle("p", &OLD);
    let after = rescaled(&before);
    let (b, a) = (points(&before), points(&after));
    assert_eq!(b.len(), a.len());
    for ((bx, by), (ax, ay)) in b.into_iter().zip(a) {
        assert!(close(raw_of(&OLD, bx), raw_of(&NEW, ax)), "{bx} -> {ax}");
        assert_eq!(by, ay);
    }
}

#[test]
fn a_polygon_admits_nearly_the_same_events() {
    // The vertices come across exactly; the edges between them are straight
    // on whichever scale the gate is shown on, so the cells nearest a slanted
    // edge can change sides. Measured at 99.0% for this triangle across a
    // 150 to 1000 cofactor change.
    let before = triangle("p", &OLD);
    let after = rescaled(&before);
    let share = agreement(&membership(&before, &OLD), &membership(&after, &NEW));
    assert!(
        share > 0.98,
        "only {:.1}% of events kept their side",
        share * 100.0
    );
}

#[test]
fn an_ellipses_centre_keeps_its_raw_value() {
    let before = ellipse("e", &OLD);
    let after = rescaled(&before);
    let (b, a) = (points(&before)[0], points(&after)[0]);
    assert!(
        close(raw_of(&OLD, b.0), raw_of(&NEW, a.0)),
        "{} -> {}",
        b.0,
        a.0
    );
    assert_eq!(b.1, a.1);
}

#[test]
fn an_ellipse_admits_nearly_the_same_events() {
    // An ellipse on one scale is not an ellipse on another, so the rescale
    // keeps the centre and one end of each axis and fits the ellipse through
    // them. Measured at 99.5% for this one, which is taller in data units
    // than it is wide - the usual shape against a linear scatter channel.
    let before = ellipse("e", &OLD);
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert!(admitted(&b) > 20, "the fixture should hold something");
    let share = agreement(&b, &a);
    assert!(
        share > 0.98,
        "only {:.1}% of events kept their side",
        share * 100.0
    );
}

#[test]
fn a_tall_ellipse_keeps_finite_radii() {
    // Its long axis is the linear one, so its canonical angle is a quarter
    // turn. The rescale once read radius_x as lying along X regardless, sent
    // 150 SSC-A units through the CD3 transform, and came back with an
    // infinite radius - a gate admitting everything to one side.
    let before = ellipse("e", &OLD);
    let (_, _, _, angle) = ellipse_form(&before);
    assert!(
        (angle.abs() - std::f32::consts::FRAC_PI_2).abs() < 0.01,
        "the fixture should be standing on end: {angle}"
    );
    let (_, rx, ry, _) = ellipse_form(&rescaled(&before));
    assert!(rx.is_finite() && ry.is_finite(), "{rx} x {ry}");
    let (_, old_rx, _, _) = ellipse_form(&before);
    assert!(
        close(rx, old_rx),
        "the SSC-A extent must not change: {old_rx} -> {rx}"
    );
}

#[test]
fn a_tilted_ellipse_admits_nearly_the_same_events() {
    // Neither axis lies along X, so the carried axis ends are no longer
    // perpendicular and the new ellipse is fitted through them as a
    // conjugate pair. Measured at 98.4%.
    //
    // The lean is checked in X units, not as an angle: with CD3 spanning
    // about 8 units and SSC-A a thousand, any tilt reads as a hair off a
    // quarter turn, yet the long axis still reaches well across CD3.
    let before = tilted_ellipse("t", &OLD);
    let (_, radius_x, _, angle) = ellipse_form(&before);
    let reach = (radius_x * angle.cos()).abs();
    assert!(reach > 0.2, "the fixture should lean across X: {reach}");
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert!(admitted(&b) > 20, "the fixture should hold something");
    let share = agreement(&b, &a);
    assert!(
        share > 0.97,
        "only {:.1}% of events kept their side",
        share * 100.0
    );
}

#[test]
fn an_ellipse_rescales_the_same_whichever_way_its_angle_points() {
    // A canonical angle and the same angle plus a half turn describe one
    // ellipse; which one the import arrives at is down to float noise, so
    // the rescale must not depend on it.
    let one = rescaled(&canonical_ellipse("a", 1.2, 90.0, 0.4));
    let other = rescaled(&canonical_ellipse(
        "a",
        1.2,
        90.0,
        0.4 + std::f32::consts::PI,
    ));
    let (c1, rx1, ry1, a1) = ellipse_form(&one);
    let (c2, rx2, ry2, a2) = ellipse_form(&other);
    assert!(close(c1, c2), "{c1} vs {c2}");
    assert!(
        close(rx1, rx2) && close(ry1, ry2),
        "{rx1} x {ry1} vs {rx2} x {ry2}"
    );
    // The same axis, whichever way along it the angle points.
    assert!((a1 - a2).sin().abs() < 1e-3, "{a1} vs {a2}");
}

#[test]
fn a_skewed_quadrant_sorts_nearly_every_event_into_the_same_quarter() {
    // Its dividing lines are angled, and an angled line on one scale is a
    // curve on another. Measured at 99.7%.
    let before = skewed("s", &OLD);
    let after = rescaled(&before);
    let (b, a) = (membership(&before, &OLD), membership(&after, &NEW));
    assert_eq!(admitted(&b), b.len(), "the quarters should cover the plot");
    let share = agreement(&b, &a);
    assert!(
        share > 0.99,
        "only {:.1}% of events kept their quarter",
        share * 100.0
    );
}

// ── what else a rescale must and must not do ─────────────────────────────

#[test]
fn rescaling_there_and_back_returns_every_gate_to_where_it_was() {
    for gate in [rectangle("r", &OLD), triangle("p", &OLD), line("l", &OLD)] {
        let mut store = GateSubStore::default();
        store.insert_for_source(&[gate.get_id()], &gate, &GateSource::Global);
        let channel: Arc<str> = Arc::from(X);
        store
            .rescale_channel(&channel, &axis(OLD), &axis(NEW))
            .unwrap();
        store
            .rescale_channel(&channel, &axis(NEW), &axis(OLD))
            .unwrap();
        let back = store
            .primary_and_subgate_registry
            .get(&gate.get_id())
            .unwrap()
            .clone();
        for ((bx, by), (ax, ay)) in points(&gate).into_iter().zip(points(&back)) {
            assert!(close(bx, ax), "{} drifted from {bx} to {ax}", gate.get_id());
            assert_eq!(by, ay);
        }
    }
}

#[test]
fn a_linear_axis_becoming_arcsinh_keeps_the_raw_values() {
    let before = rectangle("r", &TransformType::Linear);
    let mut store = GateSubStore::default();
    store.insert_for_source(&[before.get_id()], &before, &GateSource::Global);
    store
        .rescale_channel(&Arc::from(X), &axis(TransformType::Linear), &axis(NEW))
        .unwrap();
    let after = store
        .primary_and_subgate_registry
        .get(&before.get_id())
        .unwrap()
        .clone();
    assert_eq!(
        membership(&before, &TransformType::Linear),
        membership(&after, &NEW)
    );
}

#[test]
fn a_gate_on_other_channels_is_left_as_the_same_gate() {
    // Not merely equal: the same allocation, so nothing downstream - the
    // gallery's cache among them - sees a change that did not happen.
    let other = {
        let geometry = create_rectangle_geometry(
            vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)],
            "CD8",
            "CD4",
        )
        .unwrap();
        let mut g = inner("other", geometry);
        g.parameters = (Arc::from("CD8"), Arc::from("CD4"));
        Arc::new(RectangleGate::try_new(g, true).unwrap()) as Arc<dyn DrawableGate>
    };
    let mut store = GateSubStore::default();
    store.insert_for_source(&[other.get_id()], &other, &GateSource::Global);
    store
        .rescale_channel(&Arc::from(X), &axis(OLD), &axis(NEW))
        .unwrap();
    let after = store
        .primary_and_subgate_registry
        .get(&other.get_id())
        .unwrap();
    assert!(Arc::ptr_eq(&other, after));
}

fn group(value: &str) -> MetaDataKey {
    MetaDataKey {
        parameter: Arc::from("SampleID"),
        group: Arc::from(value),
    }
}

#[test]
fn every_tier_is_carried_drawn_per_specimen_and_per_sample() {
    // The autogater's positions live in the specimen and sample tiers, so a
    // rescale that only reached the drawn gates would leave every placed gate
    // around the wrong cells.
    let drawn = rectangle("g", &OLD);
    let specimen = rectangle("g", &OLD);
    let sample = rectangle("g", &OLD);
    let mut store = GateSubStore::default();
    let id = drawn.get_id();
    store.insert_for_source(&[id.clone()], &drawn, &GateSource::Global);
    store.insert_for_source(
        &[id.clone()],
        &specimen,
        &GateSource::Group((id.clone(), group("D1"))),
    );
    store.insert_for_source(
        &[id.clone()],
        &sample,
        &GateSource::Sample((id.clone(), Arc::from("f1"))),
    );
    store
        .rescale_channel(&Arc::from(X), &axis(OLD), &axis(NEW))
        .unwrap();

    let carried = [
        store.primary_and_subgate_registry.get(&id).unwrap().clone(),
        store
            .group_position_overrides
            .get(&(id.clone(), group("D1")))
            .unwrap()
            .clone(),
        store
            .sample_position_overrides
            .get(&(id.clone(), Arc::from("f1")))
            .unwrap()
            .clone(),
    ];
    let expected = membership(&drawn, &OLD);
    for (tier, gate) in ["drawn", "per specimen", "per sample"].iter().zip(carried) {
        assert_eq!(
            membership(&gate, &NEW),
            expected,
            "the {tier} position was not carried"
        );
    }
}

#[test]
fn a_gate_shared_between_tiers_is_carried_once_not_twice() {
    // One allocation reachable from two tiers. Carried twice it would be
    // rescaled from the new scale as if it were the old one, and land
    // somewhere else entirely.
    let shared = rectangle("g", &OLD);
    let id = shared.get_id();
    let mut store = GateSubStore::default();
    store.insert_for_source(&[id.clone()], &shared, &GateSource::Global);
    store.insert_for_source(
        &[id.clone()],
        &shared,
        &GateSource::Group((id.clone(), group("D1"))),
    );
    store
        .rescale_channel(&Arc::from(X), &axis(OLD), &axis(NEW))
        .unwrap();
    let after = store
        .group_position_overrides
        .get(&(id.clone(), group("D1")))
        .unwrap()
        .clone();
    assert_eq!(membership(&after, &NEW), membership(&shared, &OLD));
}

#[test]
fn a_composite_registered_under_several_keys_is_carried_once() {
    // A quadrant is registered under its own id and each quarter's, all the
    // same allocation. Carried once per key it would compound.
    let q = quadrant("q", &OLD);
    let ids = GateSubStore::ids_for(&q, &q.get_id());
    assert!(ids.len() > 1);
    let mut store = GateSubStore::default();
    store.insert_for_source(&ids, &q, &GateSource::Global);
    store
        .rescale_channel(&Arc::from(X), &axis(OLD), &axis(NEW))
        .unwrap();
    for id in &ids {
        let under = store.primary_and_subgate_registry.get(id).unwrap().clone();
        assert_eq!(membership(&under, &NEW), membership(&q, &OLD), "under {id}");
    }
}

#[test]
fn rescaling_a_channel_no_gate_is_on_changes_nothing() {
    let gate = rectangle("r", &OLD);
    let mut store = GateSubStore::default();
    store.insert_for_source(&[gate.get_id()], &gate, &GateSource::Global);
    store
        .rescale_channel(&Arc::from("PE-A"), &axis(OLD), &axis(NEW))
        .unwrap();
    assert!(Arc::ptr_eq(
        &gate,
        store
            .primary_and_subgate_registry
            .get(&gate.get_id())
            .unwrap()
    ));
}

// ── gates drawn after the rescale ────────────────────────────────────────

#[test]
fn a_gate_drawn_after_a_rescale_lives_in_the_same_space_as_the_rescaled_ones() {
    // One gate drawn before the rescale and carried by it; another drawn
    // after it, around the same cells on the new scale. Both are then taken
    // back to the old scale. If the rescale had left the first anywhere other
    // than where a person would have drawn it, the two would now disagree.
    let early = rectangle("early", &OLD);
    let mut store = GateSubStore::default();
    store.insert_for_source(&[early.get_id()], &early, &GateSource::Global);
    let channel: Arc<str> = Arc::from(X);
    store
        .rescale_channel(&channel, &axis(OLD), &axis(NEW))
        .unwrap();

    let late = rectangle("late", &NEW);
    store.insert_for_source(&[late.get_id()], &late, &GateSource::Global);
    store
        .rescale_channel(&channel, &axis(NEW), &axis(OLD))
        .unwrap();

    let (early, late) = (
        store
            .primary_and_subgate_registry
            .get(&Arc::from("early"))
            .unwrap()
            .clone(),
        store
            .primary_and_subgate_registry
            .get(&Arc::from("late"))
            .unwrap()
            .clone(),
    );
    assert_eq!(membership(&early, &OLD), membership(&late, &OLD));
}

#[test]
fn a_gate_drawn_after_a_rescale_is_carried_by_the_next_one() {
    let late = rectangle("late", &NEW);
    let mut store = GateSubStore::default();
    store.insert_for_source(&[late.get_id()], &late, &GateSource::Global);
    let back_to_old = TransformType::Arcsinh { cofactor: 150.0 };
    store
        .rescale_channel(&Arc::from(X), &axis(NEW), &axis(back_to_old.clone()))
        .unwrap();
    let carried = store
        .primary_and_subgate_registry
        .get(&late.get_id())
        .unwrap()
        .clone();
    assert_eq!(membership(&late, &NEW), membership(&carried, &back_to_old));
}

// ── a new axis range ─────────────────────────────────────────────────────

#[test]
fn a_new_range_leaves_a_single_gate_as_the_same_gate() {
    // Only the quadrants take their extent from the axis range.
    let gate = rectangle("r", &OLD);
    let mut store = GateSubStore::default();
    store.insert_for_source(&[gate.get_id()], &gate, &GateSource::Global);
    store
        .relimit_channel(
            &Arc::from(X),
            shown(&OLD, -5000.0),
            shown(&OLD, RAW_HIGH),
            &OLD,
        )
        .unwrap();
    assert!(Arc::ptr_eq(
        &gate,
        store
            .primary_and_subgate_registry
            .get(&gate.get_id())
            .unwrap()
    ));
}

#[test]
fn a_new_range_keeps_a_quadrants_centre_and_sorts_events_the_same() {
    let q = quadrant("q", &OLD);
    let ids = GateSubStore::ids_for(&q, &q.get_id());
    let mut store = GateSubStore::default();
    store.insert_for_source(&ids, &q, &GateSource::Global);
    // Wider on both ends, so every event still falls inside the axis.
    store
        .relimit_channel(
            &Arc::from(X),
            shown(&OLD, -5000.0),
            shown(&OLD, 400_000.0),
            &OLD,
        )
        .unwrap();
    let after = store
        .primary_and_subgate_registry
        .get(&q.get_id())
        .unwrap()
        .clone();
    assert_eq!(membership(&after, &OLD), membership(&q, &OLD));
}
