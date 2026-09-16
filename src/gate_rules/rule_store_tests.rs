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
        columns.insert(Arc::from("SampleID"), Arc::from(*specimen));
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
        ..SamplePairing::default()
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
    original.insert(RuleTarget::named("Ki67+"), gate_rule(Bound::Above, (0.002, 0.005)));
    original.insert(
        RuleTarget::under("CD38+", "CD19+CD33-"),
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
        ..SamplePairing::default()
    });

    let json = serde_json::to_string(&store).unwrap();
    let back: RuleStore = serde_json::from_str(&json).unwrap();

    assert_eq!(back.pairing.sample_id_column.as_ref(), "Specimen");
}

/// A sidecar written before the pairing existed still loads, on the defaults.
#[test]
fn a_sidecar_without_a_pairing_falls_back_to_the_default_columns() {
    let back: RuleStore = serde_json::from_str(r#"{"rules":[]}"#).unwrap();

    assert_eq!(back.pairing, SamplePairing::default());
    assert!(back.is_empty());
}

#[test]
fn a_rule_can_be_replaced_and_removed() {
    let mut s = store();
    let target = RuleTarget::named("Ki67+");
    assert!(
        s.insert(target.clone(), gate_rule(Bound::Above, (0.0, 0.1)))
            .is_none()
    );
    let previous = s.insert(target.clone(), gate_rule(Bound::Below, (0.0, 0.1)));
    assert_eq!(previous.map(|r| r.bound), Some(Bound::Above));
    assert_eq!(s.get(&target).map(|r| r.bound), Some(Bound::Below));
    assert_eq!(s.len(), 1, "replaced, not appended");
    assert!(s.remove(&target).is_some());
    assert!(s.is_empty());
}

// ─── choosing the reference by hand ───────────────────────────────────────────

#[test]
fn a_hand_set_reference_beats_the_pairing() {
    let meta = metadata(&[
        ("fs_a", "donor1_wk1", "FS"),
        ("fmx_a", "donor1_wk1", "FMX"),
        ("fmx_b", "donor2_wk1", "FMX"),
    ]);
    let mut s = store();
    s.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("fmx_b"));

    let found = s.reference_file(
        &Arc::from("fs_a"),
        &MeasuredOn::Partner(Arc::from("FMX")),
        &meta,
    );

    assert_eq!(
        found.as_deref(),
        Some("fmx_b"),
        "the chosen one, not the pairing's"
    );
}

/// The case it exists for: a specimen whose FMO was never run.
#[test]
fn a_hand_set_reference_rescues_a_specimen_with_no_partner() {
    let meta = metadata(&[("fs_a", "donor1_wk1", "FS"), ("fmx_b", "donor2_wk1", "FMX")]);
    let partner = MeasuredOn::Partner(Arc::from("FMX"));
    let mut s = store();
    assert_eq!(s.reference_file(&Arc::from("fs_a"), &partner, &meta), None);

    s.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("fmx_b"));

    assert_eq!(
        s.reference_file(&Arc::from("fs_a"), &partner, &meta)
            .as_deref(),
        Some("fmx_b")
    );
}

#[test]
fn a_reference_is_set_per_file_and_sample_type() {
    let meta = metadata(&[
        ("fs_a", "donor1_wk1", "FS"),
        ("fmx_a", "donor1_wk1", "FMX"),
        ("fs_b", "donor2_wk1", "FS"),
        ("fmx_b", "donor2_wk1", "FMX"),
    ]);
    let mut s = store();
    s.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("fmx_b"));

    // Another file of the same type is untouched.
    let found = s.reference_file(
        &Arc::from("fs_b"),
        &MeasuredOn::Partner(Arc::from("FMX")),
        &meta,
    );
    assert_eq!(
        found.as_deref(),
        Some("fmx_b"),
        "from the pairing, not the override"
    );

    // And another sample type for the same file is untouched.
    let unstained = s.reference_file(
        &Arc::from("fs_a"),
        &MeasuredOn::Partner(Arc::from("U")),
        &meta,
    );
    assert_eq!(unstained, None);
}

#[test]
fn setting_a_reference_twice_replaces_it() {
    let meta = metadata(&[
        ("fs_a", "donor1_wk1", "FS"),
        ("x", "other", "FMX"),
        ("y", "other", "FMX"),
    ]);
    let mut s = store();
    s.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("x"));
    s.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("y"));

    assert_eq!(s.references().len(), 1, "not two entries for one pair");
    assert_eq!(
        s.reference_file(
            &Arc::from("fs_a"),
            &MeasuredOn::Partner(Arc::from("FMX")),
            &meta
        )
        .as_deref(),
        Some("y")
    );
}

#[test]
fn clearing_a_reference_falls_back_to_the_pairing() {
    let meta = metadata(&[
        ("fs_a", "donor1_wk1", "FS"),
        ("fmx_a", "donor1_wk1", "FMX"),
        ("fmx_b", "donor2_wk1", "FMX"),
    ]);
    let partner = MeasuredOn::Partner(Arc::from("FMX"));
    let mut s = store();
    s.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("fmx_b"));

    assert!(s.clear_reference("fs_a", "FMX"));
    assert!(!s.clear_reference("fs_a", "FMX"), "already gone");

    assert_eq!(
        s.reference_file(&Arc::from("fs_a"), &partner, &meta)
            .as_deref(),
        Some("fmx_a")
    );
}

/// A rule measured on one named file - a QC or template run - ignores both the
/// pairing and the sample being gated.
#[test]
fn a_rule_measured_on_a_named_file_always_reads_it() {
    let meta = metadata(&[("fs_a", "donor1_wk1", "FS"), ("qc", "qc4", "FS")]);
    let on_qc = MeasuredOn::File(Arc::from("qc"));

    for gated in ["fs_a", "anything", "not even in the metadata"] {
        assert_eq!(
            store()
                .reference_file(&Arc::from(gated), &on_qc, &meta)
                .as_deref(),
            Some("qc")
        );
    }
}

#[test]
fn hand_set_references_survive_the_sidecar() {
    let mut original = store();
    original.insert(RuleTarget::named("Ki67+"), gate_rule(Bound::Above, (0.002, 0.005)));
    original.set_reference(Arc::from("fs_a"), Arc::from("FMX"), Arc::from("fmx_b"));

    let back: RuleStore = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();

    assert_eq!(back, original);
    assert_eq!(back.references().len(), 1);
}

#[test]
fn a_sidecar_without_overrides_still_loads() {
    let back: RuleStore = serde_json::from_str(r#"{"rules":[]}"#).unwrap();
    assert!(back.references().is_empty());
}

// ─── deriving the sample type when there is no column for it ──────────────────

/// Metadata with no `SampleType` column at all - the shape of an export made
/// before one was added, where the distinction lives in the file name.
fn metadata_without_type(rows: &[(&str, &str, &str)]) -> MetaDataFileMap {
    let mut map: MetaDataFileMap = im::HashMap::with_hasher(FxBuildHasher);
    for (file, specimen, filename) in rows {
        let mut columns: FxHashMap<MetaDataParameter, GroupId> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(*specimen));
        columns.insert(Arc::from("$FIL"), Arc::from(*filename));
        map.insert(Arc::from(*file) as FileId, columns);
    }
    map
}

fn from_file_name() -> SamplePairing {
    SamplePairing {
        derive_type: Some(DerivedSampleType {
            column: Arc::from("$FIL"),
            markers: vec![
                SampleTypeMarker {
                    contains: Arc::from("_FMX_"),
                    sample_type: Arc::from("FMX"),
                },
                SampleTypeMarker {
                    contains: Arc::from("_FS_"),
                    sample_type: Arc::from("FS"),
                },
            ],
        }),
        ..SamplePairing::default()
    }
}

#[test]
fn a_sample_type_is_derived_when_there_is_no_column_for_it() {
    let meta = metadata_without_type(&[
        ("fs_a", "donor1", "C6 donor1_FS_Plate_10.fcs"),
        ("fmx_a", "donor1", "B6 donor1_FMX_Plate_10.fcs"),
        ("fmx_b", "donor2", "B7 donor2_FMX_Plate_10.fcs"),
    ]);
    let store = RuleStore::with_pairing(from_file_name());

    let found = store.reference_file(
        &Arc::from("fs_a"),
        &MeasuredOn::Partner(Arc::from("FMX")),
        &meta,
    );

    assert_eq!(found.as_deref(), Some("fmx_a"), "not the other donor's");
}

#[test]
fn an_explicit_column_wins_over_a_derived_one() {
    // The name says FMX, the column says FS. The column is the authority.
    let mut meta = metadata_without_type(&[
        ("misnamed", "donor1", "B6 donor1_FMX_Plate_10.fcs"),
        ("real_fmx", "donor1", "B7 donor1_FMX_Plate_10.fcs"),
    ]);
    let columns = meta.get_mut(&(Arc::from("misnamed") as FileId)).unwrap();
    columns.insert(Arc::from("SampleType"), Arc::from("FS"));

    let store = RuleStore::with_pairing(from_file_name());
    let found = store.reference_file(
        &Arc::from("misnamed"),
        &MeasuredOn::Partner(Arc::from("FMX")),
        &meta,
    );

    assert_eq!(
        found.as_deref(),
        Some("real_fmx"),
        "the file whose column says FS must not match a request for FMX"
    );
}

#[test]
fn markers_are_tried_in_order() {
    let pairing = SamplePairing {
        derive_type: Some(DerivedSampleType {
            column: Arc::from("$FIL"),
            markers: vec![
                SampleTypeMarker {
                    contains: Arc::from("_FMX_"),
                    sample_type: Arc::from("FMX"),
                },
                // A substring of the one above: without ordering it would
                // swallow every FMX file too.
                SampleTypeMarker {
                    contains: Arc::from("_F"),
                    sample_type: Arc::from("something else"),
                },
            ],
        }),
        ..SamplePairing::default()
    };
    let mut columns: FxHashMap<MetaDataParameter, GroupId> = FxHashMap::default();
    columns.insert(Arc::from("$FIL"), Arc::from("a_FMX_b.fcs"));

    assert_eq!(pairing.sample_type_of(&columns).as_deref(), Some("FMX"));
}

#[test]
fn a_name_matching_no_marker_has_no_type() {
    let mut columns: FxHashMap<MetaDataParameter, GroupId> = FxHashMap::default();
    columns.insert(Arc::from("$FIL"), Arc::from("something_unexpected.fcs"));

    assert_eq!(from_file_name().sample_type_of(&columns), None);
}

#[test]
fn without_a_derivation_a_missing_column_is_simply_missing() {
    let mut columns: FxHashMap<MetaDataParameter, GroupId> = FxHashMap::default();
    columns.insert(Arc::from("$FIL"), Arc::from("a_FMX_b.fcs"));

    assert_eq!(SamplePairing::default().sample_type_of(&columns), None);
}

#[test]
fn a_derivation_survives_the_sidecar() {
    let store = RuleStore::with_pairing(from_file_name());

    let back: RuleStore = serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();

    assert_eq!(back.pairing, store.pairing);
}

// ─── naming the gates a rule applies to ───────────────────────────────────────

#[test]
fn one_rule_covers_every_gate_of_that_name() {
    // The case this exists for: "CD279+" occupied twenty-five containers in a
    // real export, and a rule per container is both miserable and wrong.
    let mut s = store();
    s.insert(RuleTarget::named("CD279+"), gate_rule(Bound::Above, (0.002, 0.005)));

    assert_eq!(s.len(), 1);
    for parent in [Some("CD4+"), Some("CD8+"), Some("anything at all"), None] {
        assert!(
            s.rule_for("CD279+", parent).is_some(),
            "should apply under {parent:?}"
        );
    }
}

#[test]
fn a_rule_naming_a_parent_wins_over_one_that_does_not() {
    let mut s = store();
    s.insert(RuleTarget::named("Ki67+"), gate_rule(Bound::Above, (0.002, 0.005)));
    s.insert(
        RuleTarget::under("Ki67+", "CD4+"),
        gate_rule(Bound::Above, (0.05, 0.06)),
    );

    let general = s.rule_for("Ki67+", Some("CD8+")).expect("the general rule");
    let specific = s.rule_for("Ki67+", Some("CD4+")).expect("the specific one");

    assert_ne!(general.rule, specific.rule, "the parent has to matter");
}

/// Order of insertion must not decide it - the specific one wins either way.
#[test]
fn the_specific_rule_wins_whichever_order_it_was_added() {
    for specific_first in [true, false] {
        let mut s = store();
        let general = (RuleTarget::named("Ki67+"), gate_rule(Bound::Above, (0.002, 0.005)));
        let specific = (
            RuleTarget::under("Ki67+", "CD4+"),
            gate_rule(Bound::Below, (0.002, 0.005)),
        );
        if specific_first {
            s.insert(specific.0.clone(), specific.1.clone());
            s.insert(general.0.clone(), general.1.clone());
        } else {
            s.insert(general.0.clone(), general.1.clone());
            s.insert(specific.0.clone(), specific.1.clone());
        }

        assert_eq!(
            s.rule_for("Ki67+", Some("CD4+")).map(|r| r.bound),
            Some(Bound::Below),
            "specific_first was {specific_first}"
        );
    }
}

#[test]
fn a_rule_for_one_parent_does_not_reach_another() {
    let mut s = store();
    s.insert(
        RuleTarget::under("Ki67+", "CD4+"),
        gate_rule(Bound::Above, (0.002, 0.005)),
    );

    assert!(s.rule_for("Ki67+", Some("CD4+")).is_some());
    assert!(s.rule_for("Ki67+", Some("CD8+")).is_none());
    assert!(s.rule_for("Ki67+", None).is_none());
    assert!(s.rule_for("CD38+", Some("CD4+")).is_none());
}

#[test]
fn a_target_reads_the_way_a_gate_is_spoken_about() {
    assert_eq!(RuleTarget::under("Ki67+", "CD4+").describe(), "Ki67+ of CD4+");
    assert_eq!(RuleTarget::named("Ki67+").describe(), "Ki67+");
}

#[test]
fn targets_survive_the_sidecar() {
    let mut original = store();
    original.insert(RuleTarget::named("CD279+"), gate_rule(Bound::Above, (0.002, 0.005)));
    original.insert(
        RuleTarget::under("Ki67+", "CD4+"),
        gate_rule(Bound::Below, (0.01, 0.02)),
    );

    let back: RuleStore = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();

    assert_eq!(back, original);
    assert_eq!(
        back.rule_for("Ki67+", Some("CD4+")).map(|r| r.bound),
        Some(Bound::Below)
    );
}
