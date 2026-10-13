use std::sync::Arc;

use flow_gates::{BooleanOperation, create_rectangle_geometry};
use polars::prelude::*;

use crate::gates::GateId;
use crate::gates::gate_filtering::filter_events_by_hierarchy_to_mask;
use crate::gates::gate_filtering_tests::{centre_column, centre_row, events, resolver, selected};
use crate::gates::gate_single::boolean_gates::BooleanGate;
use crate::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gates::gate_traits::DrawableGate;
use crate::populations::Populations;

fn ids(names: &[&str]) -> Vec<GateId> {
    names.iter().map(|name| Arc::from(*name)).collect()
}

/// Three events as the file holds them: CD3 100, 500 and 1,000, which a
/// cofactor of 100 scales to asinh 1, 5 and 10 - 0.88, 2.31 and 3.00.
fn raw() -> DataFrame {
    df![
        "FSC-A" => [1.0f32, 2.0, 3.0],
        "SSC-A" => [1.0f32, 1.0, 1.0],
        "CD3" => [100.0f32, 500.0, 1000.0],
    ]
    .unwrap()
}

fn cofactors() -> Vec<(Arc<str>, f32)> {
    vec![(Arc::from("CD3"), 100.0), (Arc::from("FSC-A"), 100.0)]
}

/// A gate on CD3 from `low` to `high` in display space, any SSC-A.
fn on_cd3(id: &str, low: f32, high: f32) -> Arc<dyn DrawableGate> {
    let geometry = create_rectangle_geometry(
        vec![(low, -10.0), (high, -10.0), (high, 10.0), (low, 10.0)],
        "CD3",
        "SSC-A",
    )
    .unwrap();
    let gate = flow_gates::Gate {
        id: Arc::from(id),
        name: id.to_string(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from("CD3"), Arc::from("SSC-A")),
        label_position: None,
    };
    Arc::new(RectangleGate::try_new(gate, true).unwrap())
}

#[test]
fn a_population_holds_the_events_its_whole_chain_holds() {
    let frame = events();
    let gates = resolver(vec![centre_column("col"), centre_row("row")]);
    let chain = ids(&["col", "row"]);
    let by_chain = filter_events_by_hierarchy_to_mask(&frame, &chain, &gates).unwrap();

    let mut populations = Populations::new(frame, &[], gates);

    assert_eq!(
        selected(&populations.mask(&chain).unwrap()),
        selected(&by_chain)
    );
    assert_eq!(selected(&populations.mask(&chain).unwrap()), vec![1]);
}

#[test]
fn populations_with_a_parent_in_common_work_the_parent_out_once() {
    let gates = resolver(vec![
        centre_column("col"),
        centre_row("row"),
        centre_column("again"),
    ]);
    let mut populations = Populations::new(events(), &[], gates);

    populations.mask(&ids(&["col", "row"])).unwrap();
    populations.mask(&ids(&["col", "again"])).unwrap();
    populations.mask(&ids(&["col", "row"])).unwrap();

    assert_eq!(populations.worked_out(), 3, "col, col/row and col/again");
}

#[test]
fn the_root_holds_every_event_and_works_nothing_out() {
    let mut populations = Populations::new(events(), &[], resolver(vec![]));

    assert_eq!(
        selected(&populations.mask(&[]).unwrap()),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(populations.worked_out(), 0);
}

#[test]
fn a_chain_naming_a_gate_that_is_not_there_is_an_error() {
    let mut populations = Populations::new(events(), &[], resolver(vec![centre_column("col")]));

    assert!(populations.mask(&ids(&["col", "ghost"])).is_err());
}

#[test]
fn a_population_is_cut_on_the_columns_asked_for_that_the_file_has() {
    let mut populations = Populations::new(events(), &[], resolver(vec![centre_column("col")]));
    let columns: Vec<Arc<str>> = vec![Arc::from("SSC-A"), Arc::from("CD3")];

    let cut = populations.under(&ids(&["col"]), Some(&columns)).unwrap();

    assert_eq!(cut.get_column_names(), vec!["SSC-A"]);
    let values: Vec<f32> = cut
        .column("SSC-A")
        .unwrap()
        .f32()
        .unwrap()
        .into_no_null_iter()
        .collect();
    assert_eq!(values, vec![5.0, 1.0, 9.0], "events 1, 2 and 4");
}

#[test]
fn a_population_asked_for_on_every_column_keeps_them_all() {
    let mut populations = Populations::new(events(), &[], resolver(vec![centre_column("col")]));

    let cut = populations.under(&ids(&["col"]), None).unwrap();

    assert_eq!(cut.get_column_names(), vec!["FSC-A", "SSC-A"]);
    assert_eq!(cut.height(), 3);
}

#[test]
fn a_channel_is_scaled_once_it_is_read_and_not_before() {
    let mut populations = Populations::new(raw(), &cofactors(), resolver(vec![]));

    let cut = populations.under(&[], Some(&[Arc::from("CD3")])).unwrap();

    let values: Vec<f32> = cut
        .column("CD3")
        .unwrap()
        .f32()
        .unwrap()
        .into_no_null_iter()
        .collect();
    assert_eq!(
        values,
        vec![1.0f32.asinh(), 5.0f32.asinh(), 10.0f32.asinh()]
    );
    assert!(populations.is_unscaled("FSC-A"));
}

#[test]
fn every_channel_is_scaled_for_a_population_on_every_column() {
    let mut populations = Populations::new(raw(), &cofactors(), resolver(vec![]));

    let cut = populations.under(&[], None).unwrap();

    let values: Vec<f32> = cut
        .column("FSC-A")
        .unwrap()
        .f32()
        .unwrap()
        .into_no_null_iter()
        .collect();
    assert_eq!(
        values,
        vec![0.01f32.asinh(), 0.02f32.asinh(), 0.03f32.asinh()]
    );
}

#[test]
fn a_gate_holds_events_by_their_scaled_values() {
    let mut populations = Populations::new(
        raw(),
        &cofactors(),
        resolver(vec![on_cd3("band", 2.0, 2.5)]),
    );

    assert_eq!(
        selected(&populations.mask(&ids(&["band"])).unwrap()),
        vec![1]
    );
}

#[test]
fn a_boolean_gate_reads_its_operands_channels_scaled() {
    let not = Arc::new(
        BooleanGate::new(
            Arc::from("not"),
            "not".to_string(),
            vec![Arc::from("band")],
            BooleanOperation::Not,
            Arc::from("FSC-A"),
            Arc::from("SSC-A"),
        )
        .unwrap(),
    );
    let gates = resolver(vec![on_cd3("band", 2.0, 2.5), not]);
    let mut populations = Populations::new(raw(), &cofactors(), gates);

    assert_eq!(
        selected(&populations.mask(&ids(&["not"])).unwrap()),
        vec![0, 2]
    );
}
