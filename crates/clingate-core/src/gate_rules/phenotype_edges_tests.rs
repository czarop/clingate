//! A phenotype gate's edges keep their place in the gap between the
//! population and the cells beyond it - not the matched cells' middle, which
//! moves with how many there are and how bright - and the sides the reference
//! gate leaves open ask nothing of a cell.
//!
//! The plot is CD56 against CD3, as for CD3+CD56+ cells; the rule reads CD56.
//! Every sample's parent is CD3+ (Y about 500) with a CD56 negative.

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

/// 10,000 CD56-negative cells, N(300, `width`), and `count` positives, all
/// N(500, 40) on CD3.
fn sample(seed: u64, width: f64, count: usize, positives: Positives) -> polars::prelude::DataFrame {
    sample_of(seed, (300.0, width), 40.0, count, positives)
}

/// 10,000 CD56-negative cells, N(`negative`), and `count` positives, all
/// N(500, `cd3_width`) on CD3.
fn sample_of(
    seed: u64,
    negative: (f64, f64),
    cd3_width: f64,
    count: usize,
    positives: Positives,
) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let cd3 = Normal::new(500.0, cd3_width).unwrap();
    let negative = Normal::new(negative.0, negative.1).unwrap();
    let mut xs: Vec<f32> = (0..10_000)
        .map(|_| negative.sample(&mut rng) as f32)
        .collect();
    xs.extend((0..count).map(|_| match positives {
        Positives::Smear(mean) => {
            (negative.mean() + Exp::new(1.0 / mean).unwrap().sample(&mut rng)) as f32
        }
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
    rule_reading(fit, &[X])
}

/// The phenotype rule for CD3+CD56+, reading `markers`.
fn rule_reading(fit: ShapeFit, markers: &[&str]) -> RuleStore {
    rule_of(PhenotypeRule {
        markers: markers.iter().map(|m| Arc::from(*m)).collect(),
        fit,
        ..Default::default()
    })
}

/// The rule for CD3+CD56+ reading CD56, its edge on CD56 pinned to the
/// negative.
fn pinned(fit: ShapeFit) -> RuleStore {
    rule_of(PhenotypeRule {
        markers: vec![Arc::from(X)],
        pinned: vec![Arc::from(X)],
        fit,
        ..Default::default()
    })
}

/// `phenotype` as the rule for CD3+CD56+.
fn rule_of(phenotype: PhenotypeRule) -> RuleStore {
    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD3+CD56+"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from("reference")),
            rule: Rule::MatchThePhenotype(phenotype),
        },
    );
    store
}

/// The rule run on `reference` and `sample` with the gate drawn from `x`;
/// the report, and the sample's gate on CD56 and on CD3.
fn run(
    fit: ShapeFit,
    x: (f32, f32),
    reference: &polars::prelude::DataFrame,
    sample: &polars::prelude::DataFrame,
) -> (Report, (f32, f32), (f32, f32)) {
    run_rules(&rule(fit), x, reference, sample)
}

/// [`run`], with `rules`.
fn run_rules(
    rules: &RuleStore,
    x: (f32, f32),
    reference: &polars::prelude::DataFrame,
    sample: &polars::prelude::DataFrame,
) -> (Report, (f32, f32), (f32, f32)) {
    let (mut state, id) = gated(x);
    let map = specimens();
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in [("reference", reference), ("sample", sample)] {
        let (m, u) = measure_file(&state, &Arc::from(file), frame, &map, rules).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, rules, &measured, &unmeasured, &map);
    let gate = state
        .gate_for_file(&id, &Arc::from("sample"), &map)
        .expect("the sample has a gate");
    let GateGeometry::Rectangle { .. } = &gate.get_gate_ref(None).unwrap().geometry else {
        panic!("a rectangle must stay a rectangle");
    };
    let on = |axis| extent_on(&gate.get_gate_ref(None).unwrap().geometry, axis).unwrap();
    (report, on(X), on(Y))
}

/// The reference smears to a mean of 200 above its negative, the sample only
/// to 140: fewer and dimmer positives. The negatives are the same, so the gate
/// starts at 600 on both - where the old fit, moving the gate by the matched
/// cells' middle, took it down some 40 towards the negative.
#[test]
fn a_dimmer_smear_leaves_the_gate_where_it_sits_against_the_negative() {
    let reference = sample(1, 50.0, 2_000, Positives::Smear(200.0));
    let dimmer = sample(2, 50.0, 2_000, Positives::Smear(140.0));
    let (report, (lower, _), _) = run(ShapeFit::KeepShape, (600.0, 3_000.0), &reference, &dimmer);
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
    let (report, _, _) = run(ShapeFit::KeepShape, (400.0, 3_000.0), &reference, &brighter);
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
        let (report, _, _) = run(ShapeFit::KeepShape, (400.0, 1_100.0), reference, &brighter);
        let read = report.positioned[0].phenotype.clone().unwrap();
        (read.matched, read.reference_matched)
    };
    let (dusty, held) = matched(&with_a_bright_tail(&positives, 5, 10));
    assert!(dusty > held + 800, "{dusty} matched against {held}");
    let (closed, held) = matched(&with_a_bright_tail(&positives, 5, 300));
    assert!(closed < held + 200, "{closed} matched against {held}");
}

/// The reference's positives, N(900, 50), sit apart from its negative,
/// N(300, 50), and the gate's lower edge halfway between them: from the
/// negative's 95th percentile, 300 + 1.645 x 50 = 382, to the positives' 5th,
/// 900 - 1.645 x 50 = 818. On the sample the negative is at 400 and the
/// positives at N(1300, 80), the gap from 482 to 1300 - 1.645 x 80 = 1168,
/// and halfway is 825 - not 1000, where following the positives alone would
/// put it, nor 600, where it was.
#[test]
fn an_edge_keeps_its_place_in_the_gap_between_the_negative_and_the_positives() {
    let reference = sample_of(
        8,
        (300.0, 50.0),
        40.0,
        3_000,
        Positives::Population(900.0, 50.0),
    );
    let moved = sample_of(
        9,
        (400.0, 50.0),
        40.0,
        3_000,
        Positives::Population(1_300.0, 80.0),
    );
    let (report, (lower, _), _) = run(ShapeFit::KeepShape, (600.0, 3_000.0), &reference, &moved);
    let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(report.positioned.len(), 1, "{why:?}");
    assert!((lower - 825.0).abs() < 15.0, "{lower}");
}

/// Every cell lies within the gate's 300 to 700 on CD3, so its edges there
/// follow the population's own: 1.645 widths either side of 500. On a sample
/// two and a half times as wide on CD3, N(500, 100) against N(500, 40), they
/// move out 99 each, and the gate would grow by half: past the limit, so it slides,
/// its size kept, and says so. Moving only, it slides without saying anything.
#[test]
fn a_gate_that_would_grow_past_the_area_limit_slides_instead() {
    let reference = sample(5, 50.0, 2_000, Positives::Smear(200.0));
    let wider = sample_of(6, (300.0, 50.0), 100.0, 2_000, Positives::Smear(200.0));
    let (kept, _, (lower, upper)) = run(ShapeFit::KeepShape, (600.0, 3_000.0), &reference, &wider);
    assert!(((upper - lower) - 400.0).abs() < 0.5, "{lower}..{upper}");
    assert!(kept.positioned[0].phenotype.as_ref().unwrap().clamped);
    let (moved, _, (lower, upper)) = run(ShapeFit::MoveOnly, (600.0, 3_000.0), &reference, &wider);
    assert!(((upper - lower) - 400.0).abs() < 0.5, "{lower}..{upper}");
    assert!(!moved.positioned[0].phenotype.as_ref().unwrap().clamped);
}

/// A quarter wider on CD3, N(500, 50): the edges move out 1.645 x 10 = 16
/// each, and the kept shape grows with them to about 433, within the limit.
#[test]
fn a_kept_shape_grows_with_its_axis_within_the_area_limit() {
    let reference = sample(5, 50.0, 2_000, Positives::Smear(200.0));
    let wider = sample_of(7, (300.0, 50.0), 50.0, 2_000, Positives::Smear(200.0));
    let (report, _, (lower, upper)) =
        run(ShapeFit::KeepShape, (600.0, 3_000.0), &reference, &wider);
    assert!(((upper - lower) - 433.0).abs() < 10.0, "{lower}..{upper}");
    assert!(!report.positioned[0].phenotype.as_ref().unwrap().clamped);
    let (_, _, (lower, upper)) = run(ShapeFit::MoveOnly, (600.0, 3_000.0), &reference, &wider);
    assert!(((upper - lower) - 400.0).abs() < 0.5, "{lower}..{upper}");
}

/// MAIT cells, CD161+Va7.2+, with CD161 on X and Va7.2 on Y, and a rule that
/// reads both: 10,000 Va7.2- cells, N(100, 30) on Y, negative on X at
/// N(300, 50) with 300 more trailing off it to a mean of 300 above; and the
/// Va7.2+ cells, N(500, 40) on Y - 400 CD161-, N(300, 50) on X, and 1,000
/// MAIT cells, N(1000, 80).
fn mait(seed: u64) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let negative = Normal::new(300.0, 50.0).unwrap();
    let trailing = Exp::new(1.0 / 300.0).unwrap();
    let (va72_negative, va72_positive) = (
        Normal::new(100.0, 30.0).unwrap(),
        Normal::new(500.0, 40.0).unwrap(),
    );
    let mut cells: Vec<(f64, f64)> = Vec::new();
    for _ in 0..10_000 {
        cells.push((negative.sample(&mut rng), va72_negative.sample(&mut rng)));
    }
    for _ in 0..300 {
        cells.push((
            300.0 + trailing.sample(&mut rng),
            va72_negative.sample(&mut rng),
        ));
    }
    for _ in 0..400 {
        cells.push((negative.sample(&mut rng), va72_positive.sample(&mut rng)));
    }
    let positive = Normal::new(1_000.0, 80.0).unwrap();
    for _ in 0..1_000 {
        cells.push((positive.sample(&mut rng), va72_positive.sample(&mut rng)));
    }
    let xs: Vec<f32> = cells.iter().map(|c| c.0 as f32).collect();
    let ys: Vec<f32> = cells.iter().map(|c| c.1 as f32).collect();
    df![X => xs, Y => ys].unwrap()
}

/// The gate's CD161 edge at 650 sits between the Va7.2+ CD161- cells, whose
/// 95th percentile is 300 + 1.645 x 50 = 382, and the MAIT cells, whose 5th is
/// 1000 - 1.645 x 80 = 868. The sample is another draw of the same cells.
/// Across the whole parent CD161 splits where the Va7.2- cells thin out, which
/// moves from draw to draw, and on this one falls among the MAIT cells: placed
/// by that split, the edge went to 881, through them. Placed against the
/// cells beside it, it stays at 650.
#[test]
fn an_edge_is_placed_against_the_cells_beside_it_not_the_whole_parent_s_split() {
    let rules = rule_reading(ShapeFit::KeepShape, &[X, Y]);
    let (report, (lower, _), _) = run_rules(&rules, (650.0, 1_600.0), &mait(10), &mait(11));
    let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(report.positioned.len(), 1, "{why:?}");
    assert!((lower - 650.0).abs() < 20.0, "{lower}");
}

/// The sample's negative sits at 400 and is 80 wide against the reference's
/// 300 and 50, its positives at 1500 against 700. The gate, drawn from 400 -
/// two of the reference negative's widths above its peak - starts two of the
/// sample's above its own, at 560, however it is fitted; kept in the gap
/// instead, it would start near 630.
#[test]
fn a_pinned_edge_keeps_its_widths_above_the_negative() {
    let reference = sample(5, 50.0, 3_000, Positives::Population(700.0, 100.0));
    let sample = sample_of(
        6,
        (400.0, 80.0),
        40.0,
        3_000,
        Positives::Population(1_500.0, 100.0),
    );
    for fit in [ShapeFit::KeepShape, ShapeFit::MoveOnly] {
        let (report, (lower, _), _) =
            run_rules(&pinned(fit), (400.0, 3_000.0), &reference, &sample);
        let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(report.positioned.len(), 1, "{why:?}");
        assert!((lower - 560.0).abs() < 30.0, "{fit:?}: {lower}");
        let read = report.positioned[0].phenotype.as_ref().unwrap();
        assert_eq!(read.pinned, [Arc::from(X)]);
        assert!(read.could_pin.is_empty());
    }
}

/// Drawn from 360, the gate's edge cuts the reference negative 1.2 widths
/// above its peak, so the run says it could be pinned there; drawn from 600,
/// six widths up in the gap, it does not.
#[test]
fn an_edge_drawn_inside_the_negative_could_be_pinned() {
    let reference = sample(7, 50.0, 3_000, Positives::Population(700.0, 100.0));
    let sample = sample(8, 50.0, 3_000, Positives::Population(700.0, 100.0));
    for (from, could) in [(360.0, true), (600.0, false)] {
        let (report, _, _) = run(ShapeFit::KeepShape, (from, 3_000.0), &reference, &sample);
        let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(report.positioned.len(), 1, "{why:?}");
        let read = report.positioned[0].phenotype.as_ref().unwrap();
        assert_eq!(
            read.could_pin == [Arc::from(X)],
            could,
            "from {from}: {:?}",
            read.could_pin
        );
        assert!(read.pinned.is_empty());
    }
}
/// How far the edges placed from either half of the sample's events agree,
/// with the gate drawn from 450 and placed by `fit`; `None` where the run
/// does not score it.
fn steady(fit: ShapeFit, sample: &polars::prelude::DataFrame) -> Option<f64> {
    let reference = sample_of(
        31,
        (300.0, 50.0),
        40.0,
        3_000,
        Positives::Population(800.0, 100.0),
    );
    let (report, _, _) = run(fit, (450.0, 3_000.0), &reference, sample);
    let why: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(report.positioned.len(), 1, "{why:?}");
    report.positioned[0]
        .components
        .iter()
        .find(|c| c.name == crate::gate_rules::confidence::STEADY)
        .map(|c| c.score)
}

/// Both halves of a sample like the reference put the edge in the same gap.
/// Halves that hold different positives - one dim, reaching down towards the
/// negative, one bright - put it in different places, and the dim cells
/// between are held by one placement only.
#[test]
fn edges_that_agree_between_halves_score_and_those_that_do_not_are_doubted() {
    let alike = sample_of(
        33,
        (300.0, 50.0),
        40.0,
        3_000,
        Positives::Population(800.0, 100.0),
    );
    let agreeing = steady(ShapeFit::KeepShape, &alike).unwrap();
    assert!(agreeing > 0.99, "{agreeing}");
    let apart = steady(ShapeFit::KeepShape, &halves_apart(32)).unwrap();
    assert!(apart < 0.95, "{apart}");
    let moved_only = steady(ShapeFit::MoveOnly, &halves_apart(32)).unwrap();
    assert!(moved_only < 0.95, "{moved_only}");
}

/// 10,000 CD56-negative cells, N(300, 50), and 3,000 positives, every other
/// one at N(550, 60) and the rest at N(850, 60) - so each half of its events
/// holds only one of the two.
fn halves_apart(seed: u64) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let negative = Normal::new(300.0, 50.0).unwrap();
    let dim = Normal::new(550.0, 60.0).unwrap();
    let bright = Normal::new(850.0, 60.0).unwrap();
    let cd3 = Normal::new(500.0, 40.0).unwrap();
    let xs: Vec<f32> = (0..13_000)
        .map(|at| match (at < 10_000, at % 2) {
            (true, _) => negative.sample(&mut rng),
            (false, 0) => dim.sample(&mut rng),
            (false, _) => bright.sample(&mut rng),
        } as f32)
        .collect();
    let ys: Vec<f32> = (0..xs.len()).map(|_| cd3.sample(&mut rng) as f32).collect();
    df![X => xs, Y => ys].unwrap()
}

/// A phenotype rule draws no single line, so it is scored on the events its
/// gate shares with the hand gate: counted here inside the drawn rectangle
/// and the one a run moves it to, on the same samples written as files.
#[test]
fn a_phenotype_rule_is_scored_on_the_events_it_shares_with_the_hand_gate() {
    use crate::file_load_tests::{scratch, write_fcs_rows};
    let reference = sample(1, 50.0, 2_000, Positives::Smear(200.0));
    let dimmer = sample(2, 50.0, 2_000, Positives::Smear(140.0));
    let drawn = (600.0, 3_000.0);
    let (report, moved_x, moved_y) = run(ShapeFit::KeepShape, drawn, &reference, &dimmer);
    let placed = &report.positioned[0];

    let dir = scratch("score-phenotype");
    let files: Vec<(Arc<str>, std::path::PathBuf)> = [("reference", &reference), ("sample", &dimmer)]
        .into_iter()
        .map(|(name, frame)| {
            let column = |c: &str| frame.column(c).unwrap().f32().unwrap().to_vec();
            let rows: Vec<Vec<f32>> = column(X)
                .into_iter()
                .zip(column(Y))
                .map(|(x, y)| vec![x.unwrap(), y.unwrap()])
                .collect();
            let path = dir.join(format!("{name}.fcs"));
            write_fcs_rows(&path, &[(X, None), (Y, None)], &rows, &[]);
            (Arc::from(format!("{name}.fcs").as_str()), path)
        })
        .collect();
    let inputs = crate::gate_rules::run::RunInputs {
        names: files
            .iter()
            .map(|(id, _)| (id.clone(), Arc::from(id.trim_end_matches(".fcs"))))
            .collect(),
        files,
        compensation: Default::default(),
        cofactors: Vec::new(),
        metadata: specimens(),
        rules: rule(ShapeFit::KeepShape),
    };
    let (state, _) = gated((600.0, 3_000.0));
    let scored = crate::gate_rules::score::score_rules(
        &state,
        &inputs,
        |_| true,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();

    let row = scored
        .rows
        .iter()
        .find(|r| r.what == "moved")
        .unwrap_or_else(|| panic!("{scored:#?}"));
    assert_eq!(row.file, "sample");
    assert_eq!((row.hand_edge, row.rule_edge, row.edge_off_iqrs), (None, None, None));

    // Rectangle edges count as inside, as the plots count them.
    let within = |range: (f32, f32), v: f32| range.0.min(range.1) <= v && v <= range.0.max(range.1);
    let column = |c: &str| dimmer.column(c).unwrap().f32().unwrap().to_vec();
    let (mut hand, mut rule, mut both) = (0, 0, 0);
    for (x, y) in column(X).into_iter().zip(column(Y)) {
        let (x, y) = (x.unwrap(), y.unwrap());
        let in_hand = within(drawn, x) && within((300.0, 700.0), y);
        let in_rule = within(moved_x, x) && within(moved_y, y);
        hand += usize::from(in_hand);
        rule += usize::from(in_rule);
        both += usize::from(in_hand && in_rule);
    }
    let events = row.events.unwrap();
    assert_eq!((events.hand, events.rule, events.both), (hand, rule, both), "{row:?}");
    assert!(hand != both || rule != both, "the gate moved, or this proves little");
    assert_eq!(row.caught, Some(both as f64 / hand as f64));
    assert_eq!(row.extra, Some((rule - both) as f64 / rule as f64));
    assert!((row.hand_holds.unwrap() - placed.from).abs() < 1e-9, "{row:?} {}", placed.from);
    assert!((row.rule_holds.unwrap() - placed.to).abs() < 1e-9, "{row:?} {}", placed.to);
    assert_eq!(scored.gates[0].median_caught, row.caught);
}
