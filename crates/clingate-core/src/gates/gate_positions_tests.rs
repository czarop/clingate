//! Moving a sample's position for a gate between the gate as drawn, a group's
//! and its own. Every position is a rectangle placed by hand at a known left
//! edge, so what a sample shows is read straight off that edge.

use std::sync::Arc;

use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::gates::GateState;
use crate::gates::gate_positions::{
    Release, Tier, copy_to_samples, keep_for_group, keep_for_sample, release_column,
    release_sample, tier,
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

/// a1 and a2 of DONOR-A, b1 of DONOR-B; a1 and b1 are full stains, a2 an FMX.
fn metadata() -> MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (name, donor, kind) in [
        ("a1", "DONOR-A", "FS"),
        ("a2", "DONOR-A", "FMX"),
        ("b1", "DONOR-B", "FS"),
    ] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(donor));
        columns.insert(Arc::from("SampleType"), Arc::from(kind));
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

fn donor(group: &str) -> MetaDataKey {
    MetaDataKey {
        parameter: Arc::from("SampleID"),
        group: Arc::from(group),
    }
}

fn by_donor(state: &mut GateState, group: &str, x0: f32) {
    state.place_gate(
        &[gate()],
        &at(x0),
        &GateSource::Group((gate(), donor(group))),
    );
}

fn own(state: &mut GateState, name: &str, x0: f32) {
    state.place_gate(
        &[gate()],
        &at(x0),
        &GateSource::Sample((gate(), file(name))),
    );
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

fn tier_of(state: &GateState, name: &str) -> Tier {
    tier(state, &gate(), &file(name), &metadata()).unwrap()
}

#[test]
fn a_sample_s_tier_is_where_its_position_comes_from() {
    let mut state = drawn();
    assert_eq!(tier_of(&state, "a1"), Tier::Drawn);
    by_donor(&mut state, "DONOR-A", 100.0);
    own(&mut state, "b1", 70.0);
    assert_eq!(tier_of(&state, "a1"), Tier::Group(donor("DONOR-A")));
    assert_eq!(tier_of(&state, "b1"), Tier::Sample);
}

#[test]
fn kept_for_a_sample_its_position_is_its_own_and_unchanged() {
    let mut state = drawn();
    by_donor(&mut state, "DONOR-A", 100.0);
    keep_for_sample(&mut state, &gate(), &file("a1"), &metadata()).unwrap();
    assert_eq!(tier_of(&state, "a1"), Tier::Sample);
    assert_eq!(shows(&state, "a1"), 100.0);
    assert_eq!(tier_of(&state, "a2"), Tier::Group(donor("DONOR-A")));
}

#[test]
fn a_drawn_gate_kept_for_a_sample_is_written_out_as_that_sample_s() {
    let mut state = drawn();
    keep_for_sample(&mut state, &gate(), &file("b1"), &metadata()).unwrap();
    keep_for_group(
        &mut state,
        &gate(),
        &file("a1"),
        &Arc::from("SampleID"),
        &metadata(),
    )
    .unwrap();
    let mut written = state.files_with_own_position(&gate(), &metadata());
    written.sort();
    assert_eq!(written, [file("a1"), file("a2"), file("b1")]);
}

#[test]
fn kept_for_a_group_every_sample_of_it_shows_that_sample_s_position() {
    let mut state = drawn();
    own(&mut state, "a1", 50.0);
    keep_for_group(
        &mut state,
        &gate(),
        &file("a1"),
        &Arc::from("SampleID"),
        &metadata(),
    )
    .unwrap();
    assert_eq!(tier_of(&state, "a1"), Tier::Group(donor("DONOR-A")));
    assert_eq!(shows(&state, "a2"), 50.0, "the FMX of the same donor");
    assert_eq!(shows(&state, "b1"), DRAWN, "another donor");

    // By another column: every full stain, whichever donor.
    keep_for_group(
        &mut state,
        &gate(),
        &file("a1"),
        &Arc::from("SampleType"),
        &metadata(),
    )
    .unwrap();
    assert_eq!(shows(&state, "b1"), 50.0);
    assert!(
        keep_for_group(
            &mut state,
            &gate(),
            &file("a1"),
            &Arc::from("Donor"),
            &metadata()
        )
        .is_err(),
        "a column the sample has no value in"
    );
}

#[test]
fn a_position_is_given_to_the_samples_chosen_and_no_others() {
    let mut state = drawn();
    own(&mut state, "a1", 100.0);
    copy_to_samples(&mut state, &gate(), &file("a1"), &[file("b1")], &metadata()).unwrap();
    assert_eq!(shows(&state, "b1"), 100.0);
    assert_eq!(tier_of(&state, "b1"), Tier::Sample);
    assert_eq!(shows(&state, "a2"), DRAWN);
}

#[test]
fn without_its_own_position_a_sample_shows_its_group_s_or_the_gate_as_drawn() {
    let mut state = drawn();
    by_donor(&mut state, "DONOR-A", 100.0);
    own(&mut state, "a1", 50.0);
    own(&mut state, "b1", 70.0);
    release_sample(&mut state, &gate(), &file("a1"));
    assert_eq!(shows(&state, "a1"), 100.0);
    assert_eq!(shows(&state, "b1"), 70.0, "only the one sample's");
    release_sample(&mut state, &gate(), &file("b1"));
    assert_eq!(shows(&state, "b1"), DRAWN);
}

/// DONOR-A by its group at 100, over an older position of a1's own at 50;
/// b1's own position at 70 is newer than DONOR-B's at 30.
fn grouped_over_older_and_under_newer() -> GateState {
    let mut state = drawn();
    own(&mut state, "a1", 50.0);
    by_donor(&mut state, "DONOR-A", 100.0);
    by_donor(&mut state, "DONOR-B", 30.0);
    own(&mut state, "b1", 70.0);
    state
}

#[test]
fn a_column_s_positions_removed_back_to_the_drawn_gate() {
    let mut state = grouped_over_older_and_under_newer();
    release_column(
        &mut state,
        &gate(),
        &Arc::from("SampleID"),
        Release::ToDrawn,
        &metadata(),
    )
    .unwrap();
    assert_eq!(shows(&state, "a1"), DRAWN, "not the old position it hid");
    assert_eq!(shows(&state, "a2"), DRAWN);
    assert_eq!(
        shows(&state, "b1"),
        70.0,
        "it showed its own, and still does"
    );
    assert!(state.group_columns_newest_first(&gate()).is_empty());
}

#[test]
fn a_column_s_positions_removed_each_sample_keeping_what_it_showed() {
    let mut state = grouped_over_older_and_under_newer();
    release_column(
        &mut state,
        &gate(),
        &Arc::from("SampleID"),
        Release::ToEachSample,
        &metadata(),
    )
    .unwrap();
    for (name, x0) in [("a1", 100.0), ("a2", 100.0), ("b1", 70.0)] {
        assert_eq!(shows(&state, name), x0, "{name}");
        assert_eq!(tier_of(&state, name), Tier::Sample, "{name}");
    }
    assert!(state.group_columns_newest_first(&gate()).is_empty());
}

#[test]
fn removing_one_column_s_positions_leaves_another_s() {
    let mut state = drawn();
    state.place_gate(
        &[gate()],
        &at(40.0),
        &GateSource::Group((
            gate(),
            MetaDataKey {
                parameter: Arc::from("SampleType"),
                group: Arc::from("FMX"),
            },
        )),
    );
    by_donor(&mut state, "DONOR-B", 30.0);
    release_column(
        &mut state,
        &gate(),
        &Arc::from("SampleID"),
        Release::ToDrawn,
        &metadata(),
    )
    .unwrap();
    assert_eq!(shows(&state, "a2"), 40.0);
    assert_eq!(shows(&state, "b1"), DRAWN);
}

#[test]
fn the_samples_sharing_a_value_include_the_sample_itself() {
    use crate::gates::gate_positions::sharing;
    let column = |name: &str| Arc::from(name);
    assert_eq!(
        sharing(&metadata(), &file("a1"), &column("SampleID")),
        [file("a1"), file("a2")]
    );
    assert_eq!(
        sharing(&metadata(), &file("a1"), &column("SampleType")),
        [file("a1"), file("b1")]
    );
    assert!(sharing(&metadata(), &file("a1"), &column("Donor")).is_empty());
}
