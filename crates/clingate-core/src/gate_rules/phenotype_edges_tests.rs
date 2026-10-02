//! A phenotype gate's edges on the axes its rule reads follow that marker's
//! negative and valley - not the matched cells' middle, which moves with how
//! many there are and how bright - and the sides the reference gate leaves
//! open ask nothing of a cell.
//!
//! The plot is CD56 against CD3, as for CD3+CD56+ cells; the rule reads CD56.
//! Every sample's parent is CD3+ (Y about 500) with a CD56 negative at 300.

use std::sync::Arc;

use flow_gates::{GateGeometry, create_rectangle_geometry};
use rand::SeedableRng;
use rand_distr::{Distribution, Exp, Normal};
use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::gate_rules::autogate::{Report, extent_on, measure_file, position_all};
use crate::gate_rules::rule::{PhenotypeRule, Rule, ShapeFit};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gates::GateState;
use crate::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gates::gate_store::GateSource;
use crate::gates::gate_traits::DrawableGate;

const X: &str = "CD56";
const Y: &str = "CD3";

/// How the CD56 positives are spread: a smear trailing off the negative with
/// this mean, or a population at this centre and width.
#[derive(Clone, Copy)]
enum Positives {
    Smear(f64),
    Population(f64, f64),
}

/// 10,000 CD56-negative cells, N(300, `width`), and `count` positives.
fn sample(seed: u64, width: f64, count: usize, positives: Positives) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let negative = Normal::new(300.0, width).unwrap();
    let cd3 = Normal::new(500.0, 40.0).unwrap();
    let mut xs: Vec<f32> = (0..10_000)
        .map(|_| negative.sample(&mut rng) as f32)
        .collect();
    xs.extend((0..count).map(|_| match positives {
        Positives::Smear(mean) => (300.0 + Exp::new(1.0 / mean).unwrap().sample(&mut rng)) as f32,
        Positives::Population(centre, spread) => {
            Normal::new(centre, spread).unwrap().sample(&mut rng) as f32
        }
    }));
    let ys: Vec<f32> = (0..xs.len()).map(|_| cd3.sample(&mut rng) as f32).collect();
    df![X => xs, Y => ys].unwrap()
}

/// A state with CD3+CD56+ drawn from `x` on CD56 and from 300 to 700 on CD3.
fn gated(x: (f32, f32)) -> (GateState, Arc<str>) {
    let id: Arc<str> = Arc::from("nkt");
    let geometry = create_rectangle_geometry(
        vec![(x.0, 300.0), (x.1, 300.0), (x.1, 700.0), (x.0, 700.0)],
        X,
        Y,
    )
    .unwrap();
    let gate: Arc<dyn DrawableGate> = Arc::new(
        RectangleGate::try_new(
            flow_gates::Gate {
                id: id.clone(),
                name: "CD3+CD56+".into(),
                geometry,
                mode: flow_gates::GateMode::Global,
                parameters: (Arc::from(X), Arc::from(Y)),
                label_position: None,
            },
            true,
        )
        .unwrap(),
    );
    let mut state = GateState::default();
    state.place_gate(&[id.clone()], &gate, &GateSource::Global);
    state.place_new_gate(None, id.clone()).unwrap();
    (state, id)
}

fn specimens() -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, id) in [("reference", "QC"), ("sample", "DONOR")] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(id));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    map
}

fn rule(fit: ShapeFit) -> RuleStore {
    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD3+CD56+"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from("reference")),
            rule: Rule::MatchThePhenotype(PhenotypeRule {
                markers: vec![Arc::from(X)],
                fit,
                ..Default::default()
            }),
        },
    );
    store
}

/// The rule run on `reference` and `sample` with the gate drawn from `x`;
/// the report, and the sample's gate on CD56.
fn run(
    fit: ShapeFit,
    x: (f32, f32),
    reference: &polars::prelude::DataFrame,
    sample: &polars::prelude::DataFrame,
) -> (Report, (f32, f32)) {
    let (mut state, id) = gated(x);
    let (map, rules) = (specimens(), rule(fit));
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in [("reference", reference), ("sample", sample)] {
        let (m, u) = measure_file(&state, &Arc::from(file), frame, &map, &rules).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, &rules, &measured, &unmeasured, &map);
    let gate = state
        .gate_for_file(&id, &Arc::from("sample"), &map)
        .expect("the sample has a gate");
    let GateGeometry::Rectangle { .. } = &gate.get_gate_ref(None).unwrap().geometry else {
        panic!("a rectangle must stay a rectangle");
    };
    let on_cd56 = extent_on(&gate.get_gate_ref(None).unwrap().geometry, X).unwrap();
    (report, on_cd56)
}

/// The reference smears to a mean of 200 above its negative, the sample only
/// to 140: fewer and dimmer positives. The negatives are the same, so the gate
/// starts at 600 on both - where the old fit, moving the gate by the matched
/// cells' middle, took it down some 40 towards the negative.
#[test]
fn a_dimmer_smear_leaves_the_gate_where_it_sits_against_the_negative() {
    let reference = sample(1, 50.0, 2_000, Positives::Smear(200.0));
    let dimmer = sample(2, 50.0, 2_000, Positives::Smear(140.0));
    let (report, (lower, _)) = run(ShapeFit::KeepShape, (600.0, 3_000.0), &reference, &dimmer);
    let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(report.positioned.len(), 1, "{why:?}");
    assert!((lower - 600.0).abs() < 15.0, "{lower}");
}

/// The reference's CD56 positives, at 700, reach down past the valley, where
/// the gate starts at 400. The sample has the same cells and 1,000 more
/// trailing on from 1050 to 1800, brighter than any the reference gate held. The gate runs to 3000,
/// past every cell, so it says "everything above here": those cells are the
/// same population, not cut off at the reference's brightest.
#[test]
fn brighter_cells_beyond_a_gate_drawn_past_every_cell_still_match() {
    let reference = sample(3, 50.0, 3_000, Positives::Population(700.0, 120.0));
    let brighter = with_a_bright_tail(&reference, 4, 1_000);
    let (report, _) = run(ShapeFit::KeepShape, (400.0, 3_000.0), &reference, &brighter);
    let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(report.positioned.len(), 1, "{why:?}");
    let read = report.positioned[0].phenotype.as_ref().unwrap();
    // The rest match as on the reference, give or take the few the sample's
    // own valley moves across.
    assert!(
        read.matched > read.reference_matched + 800,
        "{} matched against {} on the reference: the bright cells were not",
        read.matched,
        read.reference_matched
    );
}

/// The reference with `count` more cells trailing from 1050 to 1800.
fn with_a_bright_tail(
    of: &polars::prelude::DataFrame,
    seed: u64,
    count: usize,
) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let (bright, cd3) = (
        rand_distr::Uniform::new(1_050.0, 1_800.0).unwrap(),
        Normal::new(500.0, 40.0).unwrap(),
    );
    let xs: Vec<f32> = (0..count).map(|_| bright.sample(&mut rng) as f32).collect();
    let ys: Vec<f32> = (0..count).map(|_| cd3.sample(&mut rng) as f32).collect();
    of.vstack(&df![X => xs, Y => ys].unwrap()).unwrap()
}

/// The gate stops at 1100, and beyond it on the reference are about 10 stray
/// events - dust, under 1% of the gate's 3,000 - so it still says
/// "everything above here", and the sample's 1,000 brighter cells match. With
/// 300 cells beyond it, the edge means something, and they do not.
#[test]
fn a_few_stray_events_beyond_the_gate_do_not_close_it_and_a_population_does() {
    let positives = sample(3, 50.0, 3_000, Positives::Population(700.0, 120.0));
    let brighter = with_a_bright_tail(&positives, 4, 1_000);
    let matched = |reference: &polars::prelude::DataFrame| {
        let (report, _) = run(ShapeFit::KeepShape, (400.0, 1_100.0), reference, &brighter);
        let read = report.positioned[0].phenotype.clone().unwrap();
        (read.matched, read.reference_matched)
    };
    let (dusty, held) = matched(&with_a_bright_tail(&positives, 5, 10));
    assert!(dusty > held + 800, "{dusty} matched against {held}");
    let (closed, held) = matched(&with_a_bright_tail(&positives, 5, 300));
    assert!(closed < held + 200, "{closed} matched against {held}");
}

/// The sample's negative is twice as wide. The gate's CD56 edges, 600 and
/// 900, both have cells beyond them, so both are carried, and the gate would
/// grow to twice its area: past the limit, so it slides, its size kept, and
/// says so. Moving only, it slides without saying anything.
#[test]
fn a_gate_that_would_grow_past_the_area_limit_slides_instead() {
    let reference = sample(5, 50.0, 2_000, Positives::Smear(200.0));
    let wider = sample(6, 100.0, 2_000, Positives::Smear(200.0));
    let (kept, (lower, upper)) = run(ShapeFit::KeepShape, (600.0, 900.0), &reference, &wider);
    assert!(((upper - lower) - 300.0).abs() < 0.5, "{lower}..{upper}");
    assert!(kept.positioned[0].phenotype.as_ref().unwrap().clamped);
    let (moved, (lower, upper)) = run(ShapeFit::MoveOnly, (600.0, 900.0), &reference, &wider);
    assert!(((upper - lower) - 300.0).abs() < 0.5, "{lower}..{upper}");
    assert!(!moved.positioned[0].phenotype.as_ref().unwrap().clamped);
}

/// A tenth wider: the gate is carried edge by edge and grows with it, to
/// about 330 of CD56, within the limit.
#[test]
fn a_kept_shape_grows_with_its_axis_within_the_area_limit() {
    let reference = sample(5, 50.0, 2_000, Positives::Smear(200.0));
    let wider = sample(7, 55.0, 2_000, Positives::Smear(220.0));
    let (report, (lower, upper)) = run(ShapeFit::KeepShape, (600.0, 900.0), &reference, &wider);
    assert!(((upper - lower) - 330.0).abs() < 15.0, "{lower}..{upper}");
    assert!(!report.positioned[0].phenotype.as_ref().unwrap().clamped);
    let (_, (lower, upper)) = run(ShapeFit::MoveOnly, (600.0, 900.0), &reference, &wider);
    assert!(((upper - lower) - 300.0).abs() < 0.5, "{lower}..{upper}");
}
