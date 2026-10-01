//! One mode of positioning per gate, and moving a gate between modes without
//! moving it on any sample. Every position is a rectangle placed by hand at a
//! known left edge, so what a sample shows is read straight off that edge.

use std::sync::Arc;

use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::gates::GateState;
use crate::gates::gate_positions::{
    Mode, copy_to_samples, copy_to_values, mode, set_mode, settle_modes, sharing, values_of,
};
use crate::gates::gate_store::{FileId, GateSource};
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::{MetaDataFileMap, MetaDataKey};

const X: &str = "FSC-A";
const Y: &str = "SSC-A";
const BIG: f32 = 1e16;
const DRAWN: f32 = 0.0;

/// The gate "g", its left edge at `x0`.
fn at(x0: f32) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_rectangle_geometry(
        vec![(x0, 0.0), (BIG, 0.0), (BIG, BIG), (x0, BIG)],
        X,
        Y,
    )
    .unwrap();
    Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from("g"),
                name: "G".into(),
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

fn gate() -> Arc<str> {
    Arc::from("g")
}

fn file(name: &str) -> FileId {
    Arc::from(name)
}

fn column(name: &str) -> Arc<str> {
    Arc::from(name)
}

/// a1 and a2 of DONOR-A, b1 of DONOR-B; a1 and b1 are full stains, a2 an FMX.
fn metadata() -> MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (name, donor, kind) in [
        ("a1", "DONOR-A", "FS"),
        ("a2", "DONOR-A", "FMX"),
        ("b1", "DONOR-B", "FS"),
    ] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(column("SampleID"), Arc::from(donor));
        columns.insert(column("SampleType"), Arc::from(kind));
        map.insert(file(name), columns);
    }
    map
}

fn drawn() -> GateState {
    let mut state = GateState::default();
    state.place_gate(&[gate()], &at(DRAWN), &GateSource::Global);
    state.place_new_gate(None, gate()).unwrap();
    state
}

fn key(parameter: &str, group: &str) -> MetaDataKey {
    MetaDataKey {
        parameter: column(parameter),
        group: Arc::from(group),
    }
}

fn by(state: &mut GateState, parameter: &str, group: &str, x0: f32) {
    state.place_gate(
        &[gate()],
        &at(x0),
        &GateSource::Group((gate(), key(parameter, group))),
    );
}

fn own(state: &mut GateState, name: &str, x0: f32) {
    state.place_gate(
        &[gate()],
        &at(x0),
        &GateSource::Sample((gate(), file(name))),
    );
}

/// a1 at 50, a2 at 60, b1 at 70, each its own.
fn per_sample() -> GateState {
    let mut state = drawn();
    own(&mut state, "a1", 50.0);
    own(&mut state, "a2", 60.0);
    own(&mut state, "b1", 70.0);
    state
}

/// DONOR-A at 100, DONOR-B at 30.
fn by_donor() -> GateState {
    let mut state = drawn();
    by(&mut state, "SampleID", "DONOR-A", 100.0);
    by(&mut state, "SampleID", "DONOR-B", 30.0);
    state
}

/// The left edge `name` shows.
fn shows(state: &GateState, name: &str) -> f32 {
    let g = state
        .gate_for_file(&gate(), &file(name), &metadata())
        .unwrap();
    crate::gate_rules::autogate::extent_on(&g.get_gate_ref(None).unwrap().geometry, X)
        .unwrap()
        .0
}

fn all_show(state: &GateState) -> [f32; 3] {
    ["a1", "a2", "b1"].map(|name| shows(state, name))
}

fn to(state: &mut GateState, mode: Mode, in_view: Option<&str>) {
    let in_view = in_view.map(file);
    set_mode(state, &gate(), &mode, in_view.as_ref(), &metadata()).unwrap();
}

fn by_sample_id() -> Mode {
    Mode::ByColumn(column("SampleID"))
}

#[test]
fn a_gate_s_mode_is_read_from_the_positions_it_holds() {
    assert_eq!(mode(&drawn(), &gate()), Mode::Global);
    assert_eq!(mode(&by_donor(), &gate()), by_sample_id());
    assert_eq!(mode(&per_sample(), &gate()), Mode::PerSample);

    let mut both = by_donor();
    own(&mut both, "a1", 50.0);
    assert_eq!(mode(&both, &gate()), Mode::PerSample, "two kinds");
    let mut two_columns = by_donor();
    by(&mut two_columns, "SampleType", "FMX", 40.0);
    assert_eq!(mode(&two_columns, &gate()), Mode::PerSample, "two columns");
}

#[test]
fn made_per_sample_every_sample_holds_what_it_showed_as_its_own() {
    let mut state = by_donor();
    to(&mut state, Mode::PerSample, None);
    assert_eq!(mode(&state, &gate()), Mode::PerSample);
    assert_eq!(all_show(&state), [100.0, 100.0, 30.0]);
    for name in ["a1", "a2", "b1"] {
        assert!(state.has_sample_position(&gate(), &file(name)), "{name}");
    }
    assert!(state.group_columns_newest_first(&gate()).is_empty());
    // Copies, so the export writes them as each sample's.
    let mut written = state.files_with_own_position(&gate(), &metadata());
    written.sort();
    assert_eq!(written, [file("a1"), file("a2"), file("b1")]);
}

#[test]
fn made_per_sample_from_the_drawn_gate_every_sample_holds_it() {
    let mut state = drawn();
    to(&mut state, Mode::PerSample, None);
    assert_eq!(mode(&state, &gate()), Mode::PerSample);
    assert_eq!(all_show(&state), [DRAWN; 3]);
    assert_eq!(state.files_with_own_position(&gate(), &metadata()).len(), 3);
}

#[test]
fn made_by_a_column_the_group_in_view_takes_the_sample_in_view_s_position() {
    let mut state = per_sample();
    to(&mut state, by_sample_id(), Some("a2"));
    assert_eq!(mode(&state, &gate()), by_sample_id());
    assert_eq!(all_show(&state), [60.0, 60.0, 70.0]);
    assert!(!state.has_sample_positions(&gate()));

    // Nothing in view: each group its first sample's.
    let mut state = per_sample();
    to(&mut state, by_sample_id(), None);
    assert_eq!(all_show(&state), [50.0, 50.0, 70.0]);
}

#[test]
fn made_by_another_column_the_first_column_s_positions_go() {
    let mut state = by_donor();
    to(&mut state, Mode::ByColumn(column("SampleType")), Some("b1"));
    assert_eq!(mode(&state, &gate()), Mode::ByColumn(column("SampleType")));
    // FS takes b1's, FMX its only sample's.
    assert_eq!(all_show(&state), [30.0, 100.0, 30.0]);
    assert_eq!(
        state.group_columns_newest_first(&gate()),
        [column("SampleType")]
    );
}

#[test]
fn made_global_every_sample_shows_the_position_in_view() {
    let mut state = by_donor();
    to(&mut state, Mode::Global, Some("b1"));
    assert_eq!(mode(&state, &gate()), Mode::Global);
    assert_eq!(all_show(&state), [30.0; 3]);

    let mut state = per_sample();
    to(&mut state, Mode::Global, None);
    assert_eq!(all_show(&state), [DRAWN; 3], "nothing in view: as drawn");
}

#[test]
fn a_gate_already_in_its_mode_is_left_as_it_is() {
    let mut state = drawn();
    let before = state.clone();
    to(&mut state, Mode::Global, Some("a1"));
    assert!(state.unchanged_since(&before));

    let mut state = by_donor();
    let before = state.clone();
    to(&mut state, by_sample_id(), Some("b1"));
    assert!(state.unchanged_since(&before));
}

#[test]
fn a_value_or_sample_missing_a_position_of_its_own_is_given_what_it_shows() {
    let mut state = drawn();
    by(&mut state, "SampleID", "DONOR-A", 100.0);
    to(&mut state, by_sample_id(), None);
    assert!(state.has_group_position(&gate(), &key("SampleID", "DONOR-B")));
    assert_eq!(all_show(&state), [100.0, 100.0, DRAWN]);

    let mut state = drawn();
    own(&mut state, "a1", 50.0);
    to(&mut state, Mode::PerSample, None);
    assert!(state.has_sample_position(&gate(), &file("b1")));
    assert_eq!(all_show(&state), [50.0, DRAWN, DRAWN]);
}

#[test]
fn a_gate_holding_two_kinds_is_settled_per_sample_and_nothing_moves() {
    let mut state = drawn();
    own(&mut state, "a1", 50.0);
    by(&mut state, "SampleID", "DONOR-A", 100.0);
    own(&mut state, "b1", 70.0);
    let showed = all_show(&state);
    assert_eq!(showed, [100.0, 100.0, 70.0], "the newest position wins");

    settle_modes(&mut state, &metadata()).unwrap();
    assert_eq!(mode(&state, &gate()), Mode::PerSample);
    assert!(state.group_columns_newest_first(&gate()).is_empty());
    assert_eq!(all_show(&state), showed);
}

#[test]
fn a_gate_in_one_mode_is_settled_in_it() {
    let mut state = by_donor();
    settle_modes(&mut state, &metadata()).unwrap();
    assert_eq!(mode(&state, &gate()), by_sample_id());
    assert_eq!(all_show(&state), [100.0, 100.0, 30.0]);
}

#[test]
fn a_run_puts_a_gate_per_sample_into_positions_by_specimen() {
    let mut state = per_sample();
    let placement = crate::gate_rules::autogate::Placement {
        gate_id: gate(),
        specimen: key("SampleID", "DONOR-B"),
        gate: at(200.0),
    };
    crate::gate_rules::autogate::apply_placements(&mut state, &[placement], &metadata());
    assert_eq!(mode(&state, &gate()), by_sample_id());
    // DONOR-A was not placed: it keeps its first sample's.
    assert_eq!(all_show(&state), [50.0, 50.0, 200.0]);
}

#[test]
fn a_position_copied_to_samples_leaves_the_gate_per_sample() {
    let mut state = per_sample();
    copy_to_samples(
        &mut state,
        &gate(),
        &file("b1"),
        &[file("a1"), file("a2")],
        &metadata(),
    )
    .unwrap();
    assert_eq!(all_show(&state), [70.0; 3]);
    assert_eq!(mode(&state, &gate()), Mode::PerSample);
}

#[test]
fn a_position_copied_to_values_leaves_the_gate_by_its_column() {
    let mut state = by_donor();
    copy_to_values(
        &mut state,
        &gate(),
        &file("a1"),
        &column("SampleID"),
        &[Arc::from("DONOR-B")],
        &metadata(),
    )
    .unwrap();
    assert_eq!(all_show(&state), [100.0; 3]);
    assert_eq!(mode(&state, &gate()), by_sample_id());
}

#[test]
fn the_samples_sharing_a_value_include_the_sample_itself() {
    assert_eq!(
        sharing(&metadata(), &file("a1"), &column("SampleID")),
        [file("a1"), file("a2")]
    );
    assert_eq!(
        sharing(&metadata(), &file("a1"), &column("SampleType")),
        [file("a1"), file("b1")]
    );
    assert!(sharing(&metadata(), &file("a1"), &column("Donor")).is_empty());
    let donors: Vec<String> = values_of(&metadata(), &column("SampleID"))
        .iter()
        .map(|v| v.to_string())
        .collect();
    assert_eq!(donors, ["DONOR-A", "DONOR-B"]);
}

#[test]
fn a_column_no_sample_has_a_value_of_is_refused_and_nothing_changes() {
    let mut state = by_donor();
    let before = state.clone();
    let refused = set_mode(
        &mut state,
        &gate(),
        &Mode::ByColumn(column("Donor")),
        None,
        &metadata(),
    );
    assert!(refused.is_err());
    assert!(state.unchanged_since(&before));
}
