//! One rule for a gate that is a clear population on some samples and a smear
//! on others: in the valley where there is a dip, and on a smear as far above
//! the negative as on a smear gated by hand.
//!
//! Five samples, each its own specimen. On X, `ref` and `dip` have a negative
//! at 300 and a positive at 900 with a dip between; `smear1` and `smear2`
//! have the same negative with a tail falling away from it and no dip, and
//! `smear_heavy` three times the tail. `thin` has the negative, a gap to 700,
//! 300 positives spread thin from 700 to 1400 and 900 more piled up at 1650;
//! `shallow` the negative and 5,000 positives at 480, so close the dip
//! between them is shallow.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rand::SeedableRng;
use rand_distr::{Distribution, Exp, Normal};
use rustc_hash::FxBuildHasher;

use crate::file_load_tests::{scratch, write_fcs_rows};
use crate::gate_rules::autogate::{NO_SMEAR_EXAMPLE, apply_placements, extent_on};
use crate::gate_rules::rule::{Rule, ValleyOrSmearRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::{
    RunInputs, RunOutcome, run_rules, run_rules_pausing, with_smear_examples,
    without_smear_examples,
};
use crate::gates::GateState;
use crate::gates::gate_store::GateSource;
use crate::gates::gate_traits::DrawableGate;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";
const BIG: f32 = 1e16;
const SAMPLES: [&str; 7] = [
    "ref",
    "dip",
    "smear1",
    "smear2",
    "smear_heavy",
    "thin",
    "shallow",
];

fn rect(id: &str, name: &str, x0: f32) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_rectangle_geometry(
        vec![(x0, -BIG), (BIG, -BIG), (BIG, BIG), (x0, BIG)],
        X,
        Y,
    )
    .unwrap();
    Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from(id),
                name: name.into(),
                geometry,
                mode: flow_gates::GateMode::Global,
                parameters: (Arc::from(X), Arc::from(Y)),
                label_position: None,
            },
            true,
        )
        .unwrap(),
    )
}

fn add(state: &mut GateState, gate: Arc<dyn DrawableGate>, parent: Option<&str>) {
    let id: Arc<str> = Arc::from(gate.get_id());
    state.place_gate(&[id.clone()], &gate, &GateSource::Global);
    state.place_new_gate(parent.map(Arc::from), id).unwrap();
}

/// A and B holding everything; CD69+ under A drawn from 600 - in the dip -
/// and CD69+ under B drawn from 700.
fn gates() -> GateState {
    let mut state = GateState::default();
    add(&mut state, rect("a", "A", -BIG), None);
    add(&mut state, rect("b", "B", -BIG), None);
    add(&mut state, rect("cd69_a", "CD69+", 600.0), Some("a"));
    add(&mut state, rect("cd69_b", "CD69+", 700.0), Some("b"));
    state
}

/// 15,000 negatives at 300 and 5,000 positives - 15,000 on the heavy smear:
/// at 900 where there is a dip, otherwise trailing off the negative.
fn events(sample: &str, seed: u64) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let negative = Normal::new(300.0f32, 50.0).unwrap();
    let positive = Normal::new(900.0f32, 50.0).unwrap();
    let tail = Exp::new(1.0f32 / 150.0).unwrap();
    let mut rows: Vec<Vec<f32>> = (0..15_000)
        .map(|_| vec![negative.sample(&mut rng), 0.0])
        .collect();
    if sample == "thin" {
        let spread = rand_distr::Uniform::new(700.0f32, 1400.0).unwrap();
        let piled = Normal::new(1650.0f32, 40.0).unwrap();
        rows.extend((0..300).map(|_| vec![spread.sample(&mut rng), 0.0]));
        rows.extend((0..900).map(|_| vec![piled.sample(&mut rng), 0.0]));
        return rows;
    }
    if sample == "shallow" {
        let near = Normal::new(480.0f32, 70.0).unwrap();
        rows.extend((0..5_000).map(|_| vec![near.sample(&mut rng), 0.0]));
        return rows;
    }
    let smear = sample.starts_with("smear");
    let positives = if sample == "smear_heavy" {
        15_000
    } else {
        5_000
    };
    rows.extend((0..positives).map(|_| {
        let x = if smear {
            300.0 + tail.sample(&mut rng)
        } else {
            positive.sample(&mut rng)
        };
        vec![x, 0.0]
    }));
    rows
}

fn metadata() -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for sample in SAMPLES {
        let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
        columns.insert(Arc::from("SampleID"), Arc::from(sample));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        map.insert(Arc::from(sample) as Arc<str>, columns);
    }
    map
}

fn write(name: &str) -> Vec<(Arc<str>, PathBuf)> {
    let dir = scratch(name);
    SAMPLES
        .iter()
        .enumerate()
        .map(|(seed, sample)| {
            let path = dir.join(format!("{sample}.fcs"));
            write_fcs_rows(
                &path,
                &[(X, None), (Y, None)],
                &events(sample, seed as u64 + 1),
                &[],
            );
            (Arc::from(format!("{sample}.fcs").as_str()), path)
        })
        .collect()
}

fn target() -> RuleTarget {
    RuleTarget::under("CD69+", "A")
}

/// The rule on CD69+ of A, read against `reference`.
fn rules(reference: &str, either: ValleyOrSmearRule) -> RuleStore {
    let mut store = RuleStore::default();
    store.insert(
        target(),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from(reference)),
            rule: Rule::ValleyOrSmear(either),
        },
    );
    store
}

fn inputs(files: &[(Arc<str>, PathBuf)], rules: RuleStore) -> RunInputs {
    RunInputs {
        files: files.to_vec(),
        compensation: crate::compensation::groups::Compensation::default(),
        names: SAMPLES
            .iter()
            .map(|s| (Arc::from(format!("{s}.fcs").as_str()), Arc::from(*s)))
            .collect::<std::collections::HashMap<_, _, FxBuildHasher>>(),
        cofactors: Vec::new(),
        metadata: metadata(),
        rules,
    }
}

fn run(state: &GateState, files: &[(Arc<str>, PathBuf)], rules: RuleStore) -> RunOutcome {
    run_rules(
        state,
        &inputs(files, rules),
        |_| {},
        &AtomicBool::new(false),
    )
}

/// Where CD69+ of A starts on X for `sample` once `outcome` is applied.
fn edge(state: &GateState, outcome: &RunOutcome, sample: &str) -> f32 {
    let mut after = state.clone();
    apply_placements(&mut after, &outcome.placements, &metadata());
    let gate = after
        .gate_for_file(&Arc::from("cd69_a"), &Arc::from(sample), &metadata())
        .unwrap();
    extent_on(&gate.get_gate_ref(None).unwrap().geometry, X)
        .unwrap()
        .0
}

fn placed<'a>(
    outcome: &'a RunOutcome,
    sample: &str,
) -> Option<&'a crate::gate_rules::autogate::Positioned> {
    outcome
        .report
        .positioned
        .iter()
        .find(|p| &*p.gate_id == "cd69_a" && &*p.file == sample)
}

/// `state` with CD69+ of A put by hand on `sample` from `x0`.
fn gated_by_hand(mut state: GateState, sample: &str, x0: f32) -> GateState {
    state.place_gate(
        &[Arc::from("cd69_a")],
        &rect("cd69_a", "CD69+", x0),
        &GateSource::Sample((Arc::from("cd69_a"), Arc::from(sample))),
    );
    state
}

/// The dip is where the reference was gated, so `dip` - the same two
/// populations - is gated in its own dip, near 600; the smears have nothing
/// to be placed from.
#[test]
fn a_sample_with_a_dip_is_placed_in_it_and_a_smear_needs_an_example() {
    let files = write("valley-smear-none");
    let outcome = run(&gates(), &files, rules("ref", ValleyOrSmearRule::default()));
    let dip = placed(&outcome, "dip").expect("placed");
    assert!(dip.valley.is_some() && dip.negative.is_none());
    assert!((edge(&gates(), &outcome, "dip") - 600.0).abs() < 40.0);
    for smear in ["smear1", "smear2"] {
        assert!(placed(&outcome, smear).is_none());
        assert!(
            outcome
                .report
                .unplaced
                .iter()
                .any(|u| &*u.file == smear && u.reason == NO_SMEAR_EXAMPLE),
            "{smear}"
        );
    }
}

/// A pausing run stops at the first smear, before keeping anything of the
/// level, to go on from the same level once it is gated by hand.
#[test]
fn a_pausing_run_stops_at_the_first_smear_for_it_to_be_gated_by_hand() {
    let files = write("valley-smear-pause");
    let outcome = run_rules_pausing(
        &gates(),
        &inputs(&files, rules("ref", ValleyOrSmearRule::default())),
        0,
        |_| {},
        &AtomicBool::new(false),
    );
    let paused = outcome.paused.as_ref().expect("paused");
    assert_eq!(paused.next_level, 0, "the same level again");
    assert_eq!(paused.needs.len(), 1, "one smear per rule");
    let need = &paused.needs[0];
    assert!(need.file.starts_with("smear"));
    assert_eq!(need.smear_example_for, Some(target()));
    assert!(outcome.placements.is_empty() && outcome.report.positioned.is_empty());

    let taken = with_smear_examples(&rules("ref", ValleyOrSmearRule::default()), &paused.needs);
    match &taken.get(&target()).unwrap().rule {
        Rule::ValleyOrSmear(either) => assert_eq!(either.smear_example, Some(need.file.clone())),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        without_smear_examples(&taken, &paused.needs),
        rules("ref", ValleyOrSmearRule::default()),
        "a stopped run takes the example out again"
    );
}

/// smear1 gated by hand from 500 and saved as the example: it is left where
/// it was put, and smear2 - the same negative and tail - is placed as far
/// above its negative, near 500 too. The dip is still placed in its dip.
#[test]
fn the_smear_gated_by_hand_places_the_other_smears() {
    let files = write("valley-smear-example");
    let state = gated_by_hand(gates(), "smear1", 500.0);
    let example = ValleyOrSmearRule {
        smear_example: Some(Arc::from("smear1")),
        ..Default::default()
    };
    let outcome = run(&state, &files, rules("ref", example));
    assert!(
        outcome
            .report
            .reference
            .iter()
            .any(|r| &*r.gate_id == "cd69_a" && &*r.file == "smear1")
    );
    assert_eq!(edge(&state, &outcome, "smear1"), 500.0);
    let smear2 = placed(&outcome, "smear2").expect("placed");
    assert!(smear2.negative.is_some() && smear2.valley.is_none());
    assert_eq!(&*smear2.measured_on, "smear1");
    assert!((edge(&state, &outcome, "smear2") - 500.0).abs() < 25.0);
    assert!(placed(&outcome, "dip").unwrap().valley.is_some());
}

/// The heavy smear has the same negative, so it is cut at 500 too: its
/// negative is read from its peak, not from everything below the gate, which
/// would take in the extra dim cells and put the cut near 521.
#[test]
fn a_heavier_smear_is_cut_as_far_above_the_same_negative() {
    let files = write("valley-smear-heavy");
    let state = gated_by_hand(gates(), "smear1", 500.0);
    let example = ValleyOrSmearRule {
        smear_example: Some(Arc::from("smear1")),
        ..Default::default()
    };
    let outcome = run(&state, &files, rules("ref", example));
    assert!((edge(&state, &outcome, "smear_heavy") - 500.0).abs() < 12.0);
}

/// A reference that is a smear is its own example: no run stops, and every
/// sample - the one with a dip too, there being no dip on the reference to
/// gate it from - is placed as far above its negative as on the reference.
#[test]
fn a_reference_that_is_a_smear_places_every_sample_from_itself() {
    let files = write("valley-smear-smeary-reference");
    let state = gated_by_hand(gates(), "smear1", 500.0);
    let outcome = run_rules_pausing(
        &state,
        &inputs(&files, rules("smear1", ValleyOrSmearRule::default())),
        0,
        |_| {},
        &AtomicBool::new(false),
    );
    assert!(outcome.paused.is_none());
    for sample in ["smear2", "dip", "ref"] {
        let p = placed(&outcome, sample).unwrap_or_else(|| panic!("{sample}"));
        assert!(p.negative.is_some(), "{sample}");
        assert_eq!(&*p.measured_on, "smear1");
    }
    assert!((edge(&state, &outcome, "smear2") - 500.0).abs() < 25.0);
}

/// With a fallback, a smear takes CD69+ of B's edge, 700, instead of waiting
/// for an example; the dip is still placed in its dip.
#[test]
fn a_smear_takes_the_fallback_gate_s_edge_when_there_is_one() {
    let files = write("valley-smear-fallback");
    let either = ValleyOrSmearRule {
        fallback: Some(RuleTarget::under("CD69+", "B")),
        ..Default::default()
    };
    let outcome = run(&gates(), &files, rules("ref", either));
    for smear in ["smear1", "smear2"] {
        assert!(placed(&outcome, smear).is_some(), "{smear}");
        assert_eq!(edge(&gates(), &outcome, smear), 700.0);
    }
    assert!(placed(&outcome, "dip").unwrap().valley.is_some());
}

/// `thin`'s positives spread from 700 stand far lower than 5% of the
/// negative's peak, so the rule walks past the gap below them and gates in
/// the dip below the 900 piled up at 1650. Taking the lowest point it walked
/// past puts the gate in the gap, between the negative's last cells near 500
/// and the positives' first at 700; the dip sample, whose first dip counted,
/// is placed as before.
#[test]
fn thin_positives_are_gated_below_them_when_the_rule_takes_the_lowest_point() {
    let files = write("valley-smear-thin");
    let first = run(&gates(), &files, rules("ref", ValleyOrSmearRule::default()));
    assert!(
        edge(&gates(), &first, "thin") > 1400.0,
        "{}",
        edge(&gates(), &first, "thin")
    );

    let lowest = ValleyOrSmearRule {
        lowest_before: true,
        ..Default::default()
    };
    let outcome = run(&gates(), &files, rules("ref", lowest));
    let thin = edge(&gates(), &outcome, "thin");
    assert!((500.0..700.0).contains(&thin), "{thin}");
    assert!(placed(&outcome, "thin").unwrap().valley.is_some());
    assert_eq!(
        edge(&gates(), &outcome, "dip"),
        edge(&gates(), &first, "dip")
    );
    assert!(
        placed(&outcome, "smear1").is_none(),
        "a smear is still a smear"
    );
}

/// How deep the dip `sample` was placed in under `outcome`.
fn depth(outcome: &RunOutcome, sample: &str) -> f64 {
    placed(outcome, sample)
        .and_then(|p| p.valley.as_ref())
        .map(|(_, here)| here.depth)
        .unwrap_or_else(|| panic!("{sample} was not placed in a dip"))
}

/// `shallow`'s positives sit so close to the negative that the dip between
/// is shallower than `dip`'s. With the smallest dip set between the two, it
/// reads as a smear - here, one with no example - while `dip` is placed in
/// its dip as before.
#[test]
fn a_dip_shallower_than_the_rule_asks_for_is_a_smear() {
    let files = write("valley-smear-shallow");
    let any = run(&gates(), &files, rules("ref", ValleyOrSmearRule::default()));
    let (shallow, deep) = (depth(&any, "shallow"), depth(&any, "dip"));
    assert!(shallow < deep / 2.0, "{shallow} against {deep}");

    let asking = ValleyOrSmearRule {
        smallest_dip: Some((shallow + deep) / 2.0),
        ..Default::default()
    };
    let outcome = run(&gates(), &files, rules("ref", asking));
    assert!(placed(&outcome, "shallow").is_none());
    assert!(
        outcome
            .report
            .unplaced
            .iter()
            .any(|u| &*u.file == "shallow" && u.reason == NO_SMEAR_EXAMPLE),
        "{:?}",
        outcome.report.unplaced
    );
    assert_eq!(edge(&gates(), &outcome, "dip"), edge(&gates(), &any, "dip"));
}
