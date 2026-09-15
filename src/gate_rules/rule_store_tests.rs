//! The store is bookkeeping, so what is tested is that the bookkeeping cannot
//! quietly answer the wrong question: the wrong side of a threshold, the wrong
//! partner file, or a rule that did not survive being written down.

#![cfg(test)]

use super::rule::{PercentileOffsetRule, Rule, TailFractionRule};
use super::rule_store::*;
use crate::gate_editor::gates::gate_store::{FileId, GroupId};
use crate::omiq::metadata::{MetaDataFileMap, MetaDataParameter};
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::sync::Arc;

fn gate_rule(bound: Bound, band: (f64, f64)) -> GateRule {
    GateRule {
        parameter: Arc::from("eFluor 660-A"),
        bound,
        measured_on: MeasuredOn::Partner(Arc::from("FMX")),
        rule: Rule::TailFraction(TailFractionRule::new(band)),
    }
}

/// A negative population with a clean tail on each side.
fn two_tailed() -> Vec<f64> {
    let mut v: Vec<f64> = (0..9_800).map(|i| (i % 100) as f64 / 100.0).collect();
    v.extend(std::iter::repeat_n(8.0, 100)); // a high tail
    v.extend(std::iter::repeat_n(-8.0, 100)); // and a low one
    v
}

// ─── which side the gate keeps ────────────────────────────────────────────────

#[test]
fn a_gate_that_keeps_the_top_solves_on_the_top() {
    let solved = gate_rule(Bound::Above, (0.009, 0.011))
        .solve(&two_tailed(), None)
        .unwrap();

    assert_eq!(solved.threshold.events_admitted, 100);
    assert!(solved.threshold.x > 1.0, "x was {}", solved.threshold.x);
}

/// A rule that keeps the bottom is the same rule on the reflection, so the two
/// must be mirror images - not merely "also plausible".
#[test]
fn a_gate_that_keeps_the_bottom_is_the_mirror_of_one_that_keeps_the_top() {
    let values = two_tailed();
    let band = (0.009, 0.011);

    let above = gate_rule(Bound::Above, band).solve(&values, None).unwrap();
    let mirrored: Vec<f64> = values.iter().map(|v| -v).collect();
    let below = gate_rule(Bound::Below, band)
        .solve(&mirrored, None)
        .unwrap();

    assert!(
        (below.threshold.x + above.threshold.x).abs() < 1e-9,
        "{} should be the negation of {}",
        below.threshold.x,
        above.threshold.x
    );
    assert_eq!(
        below.threshold.events_admitted,
        above.threshold.events_admitted
    );
}

#[test]
fn a_gate_that_keeps_the_bottom_admits_the_low_tail() {
    let values = two_tailed();
    let rule = gate_rule(Bound::Below, (0.009, 0.011));

    let solved = rule.solve(&values, None).unwrap();

    assert!(solved.threshold.x < -1.0, "x was {}", solved.threshold.x);
    assert_eq!(
        rule.admitted(&values, solved.threshold.x),
        100,
        "the hundred events at -8 are what it should hold"
    );
}

#[test]
fn counting_follows_the_side_the_gate_keeps() {
    let values = vec![-2.0, -1.0, 1.0, 2.0];
    assert_eq!(
        gate_rule(Bound::Above, (0.0, 1.0)).admitted(&values, 0.0),
        2
    );
    assert_eq!(
        gate_rule(Bound::Below, (0.0, 1.0)).admitted(&values, 0.0),
        2
    );
    assert_eq!(
        gate_rule(Bound::Above, (0.0, 1.0)).admitted(&values, 1.5),
        1
    );
    assert_eq!(
        gate_rule(Bound::Below, (0.0, 1.0)).admitted(&values, -1.5),
        1
    );
}

/// The displacement component compares against a reference, which has to be
/// mirrored with everything else or a gate that had not moved would read as
/// having moved twice its own position.
#[test]
fn a_reference_is_mirrored_with_the_values() {
    let values = two_tailed();
    let rule = gate_rule(Bound::Below, (0.009, 0.011));

    let unmoved = rule.solve(&values, None).unwrap();
    let scored = rule.solve(&values, Some(unmoved.threshold.x)).unwrap();

    let displacement = scored
        .confidence
        .get("distance moved from the reference")
        .expect("scored against a reference");
    assert_eq!(displacement.score, 1.0, "{}", displacement.detail);
}

// ─── finding the sample a rule is measured on ─────────────────────────────────

fn metadata(rows: &[(&str, &str, &str)]) -> MetaDataFileMap {
    let mut map: MetaDataFileMap = im::HashMap::with_hasher(FxBuildHasher);
    for (file, specimen, kind) in rows {
        let mut columns: FxHashMap<MetaDataParameter, GroupId> = FxHashMap::default();
        columns.insert(Arc::from("Sample ID"), Arc::from(*specimen));
        columns.insert(Arc::from("SampleType"), Arc::from(*kind));
        map.insert(Arc::from(*file) as FileId, columns);
    }
    map
}

fn store() -> RuleStore {
    RuleStore::default()
}

#[test]
fn a_rule_measured_on_a_partner_finds_the_fmo_of_the_same_specimen() {
    let meta = metadata(&[
        ("fs_a", "donor1_wk1", "FS"),
        ("fmx_a", "donor1_wk1", "FMX"),
        ("fs_b", "donor2_wk1", "FS"),
        ("fmx_b", "donor2_wk1", "FMX"),
    ]);
    let partner = MeasuredOn::Partner(Arc::from("FMX"));

    let found = store().reference_file(&Arc::from("fs_a"), &partner, &meta);

    assert_eq!(found.as_deref(), Some("fmx_a"), "not the other donor's FMO");
}

#[test]
fn a_rule_measured_on_the_sample_itself_returns_it() {
    let meta = metadata(&[("fs_a", "donor1_wk1", "FS")]);

    let found = store().reference_file(&Arc::from("fs_a"), &MeasuredOn::Itself, &meta);

    assert_eq!(found.as_deref(), Some("fs_a"));
}

/// An FMO is not always run. Answering with the sample itself would look like a
/// result and be a quite different measurement, so it answers with nothing.
#[test]
fn a_missing_partner_is_reported_not_substituted() {
    let meta = metadata(&[("fs_a", "donor1_wk1", "FS"), ("fmx_b", "donor2_wk1", "FMX")]);

    let found = store().reference_file(
        &Arc::from("fs_a"),
        &MeasuredOn::Partner(Arc::from("FMX")),
        &meta,
    );

    assert_eq!(found, None);
}

#[test]
fn a_file_with_no_metadata_has_no_partner() {
    let meta = metadata(&[("fs_a", "donor1_wk1", "FS")]);

    let found = store().reference_file(
        &Arc::from("unknown"),
        &MeasuredOn::Partner(Arc::from("FMX")),
        &meta,
    );

    assert_eq!(found, None);
}

/// The columns are configuration, so another dataset is a matter of naming its
/// own - not of teaching the code a new filename convention.
#[test]
fn the_pairing_columns_are_configurable() {
    let mut map: MetaDataFileMap = im::HashMap::with_hasher(FxBuildHasher);
    for (file, specimen, kind) in [("a", "s1", "control"), ("b", "s1", "stained")] {
        let mut columns: FxHashMap<MetaDataParameter, GroupId> = FxHashMap::default();
        columns.insert(Arc::from("Specimen"), Arc::from(specimen));
        columns.insert(Arc::from("Panel role"), Arc::from(kind));
        map.insert(Arc::from(file) as FileId, columns);
    }
    let store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: Arc::from("Specimen"),
        sample_type_column: Arc::from("Panel role"),
    });

    let found = store.reference_file(
        &Arc::from("b"),
        &MeasuredOn::Partner(Arc::from("control")),
        &map,
    );

    assert_eq!(found.as_deref(), Some("a"));
}

// ─── the sidecar ──────────────────────────────────────────────────────────────

#[test]
fn a_store_round_trips_through_its_sidecar() {
    let mut original = store();
    original.insert(Arc::from("gate-1"), gate_rule(Bound::Above, (0.002, 0.005)));
    original.insert(
        Arc::from("gate-2"),
        GateRule {
            parameter: Arc::from("RB613-A"),
            bound: Bound::Below,
            measured_on: MeasuredOn::Itself,
            rule: Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.4)),
        },
    );

    let path = std::env::temp_dir().join(format!(
        "clingate-rules-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    original.save(&path).unwrap();
    let back = RuleStore::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(original, back);
    assert_eq!(back.len(), 2);
}

#[test]
fn a_sidecar_keeps_the_pairing_it_was_written_with() {
    let store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: Arc::from("Specimen"),
        sample_type_column: Arc::from("Panel role"),
    });

    let json = serde_json::to_string(&store).unwrap();
    let back: RuleStore = serde_json::from_str(&json).unwrap();

    assert_eq!(back.pairing.sample_id_column.as_ref(), "Specimen");
}

/// A sidecar written before the pairing existed still loads, on the defaults.
#[test]
fn a_sidecar_without_a_pairing_falls_back_to_the_default_columns() {
    let back: RuleStore = serde_json::from_str(r#"{"rules":{}}"#).unwrap();

    assert_eq!(back.pairing, SamplePairing::default());
    assert!(back.is_empty());
}

#[test]
fn a_rule_can_be_replaced_and_removed() {
    let mut s = store();
    assert!(
        s.insert(Arc::from("g"), gate_rule(Bound::Above, (0.0, 0.1)))
            .is_none()
    );
    let previous = s.insert(Arc::from("g"), gate_rule(Bound::Below, (0.0, 0.1)));
    assert_eq!(previous.map(|r| r.bound), Some(Bound::Above));
    assert_eq!(s.get("g").map(|r| r.bound), Some(Bound::Below));
    assert!(s.remove("g").is_some());
    assert!(s.is_empty());
}
