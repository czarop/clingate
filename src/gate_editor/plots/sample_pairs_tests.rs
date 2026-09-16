//! Tests for grouping a folder into the pairs a person looks at.

#![cfg(test)]

use crate::gate_editor::plots::sample_pairs::{pair_files, pair_of};
use crate::gate_rules::rule_store::SamplePairing;
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::collections::HashMap;
use std::sync::Arc;

/// Files named `f0`, `f1`, ... each with the given specimen and sample type.
/// A `None` specimen stands for a file the metadata says nothing about.
fn fixture(
    files: &[(Option<&str>, Option<&str>)],
) -> (
    Vec<Arc<str>>,
    HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    crate::omiq::metadata::MetaDataFileMap,
) {
    let mut keys = Vec::new();
    let mut names: HashMap<Arc<str>, Arc<str>, FxBuildHasher> = HashMap::default();
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);

    for (i, (specimen, kind)) in files.iter().enumerate() {
        let key: Arc<str> = Arc::from(format!("f{i}").as_str());
        keys.push(key.clone());
        let Some(specimen) = specimen else { continue };
        let id: Arc<str> = Arc::from(format!("id{i}").as_str());
        names.insert(key, id.clone());
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(*specimen));
        if let Some(kind) = kind {
            columns.insert(Arc::from("SampleType"), Arc::from(*kind));
        }
        metadata.insert(id, columns);
    }
    (keys, names, metadata)
}

#[test]
fn a_specimens_files_are_shown_together() {
    // The folder interleaves two donors; the pairs must not.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FS")),
        (Some("QC-B"), Some("FS")),
        (Some("QC-A"), Some("FMX")),
        (Some("QC-B"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].specimen.as_deref(), Some("QC-A"));
    assert_eq!(pairs[0].files, vec![2, 0], "FMX first, then FS");
    assert_eq!(pairs[1].files, vec![3, 1]);
}

#[test]
fn the_fmo_goes_on_the_left() {
    // The line is set on the FMO and read off the full stain, so that is the
    // order a person reads them in.
    let (keys, names, metadata) =
        fixture(&[(Some("QC-A"), Some("FS")), (Some("QC-A"), Some("FMX"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs[0].left(), Some(1));
    assert_eq!(pairs[0].right(), Some(0));
}

#[test]
fn the_display_order_is_configurable() {
    let pairing = SamplePairing {
        display_order: vec![Arc::from("Unstained"), Arc::from("FS")],
        ..SamplePairing::default()
    };
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FS")),
        (Some("QC-A"), Some("Unstained")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &pairing);

    assert_eq!(pairs[0].files, vec![1, 0]);
}

#[test]
fn specimens_keep_the_order_their_first_file_appears_in() {
    // Stepping through pairs should still feel like stepping through the folder.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-B"), Some("FMX")),
        (Some("QC-A"), Some("FMX")),
        (Some("QC-B"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs[0].specimen.as_deref(), Some("QC-B"));
    assert_eq!(pairs[1].specimen.as_deref(), Some("QC-A"));
}

#[test]
fn a_specimen_with_only_one_file_still_shows_it() {
    // The FMO was never run. Better one plot than none.
    let (keys, names, metadata) = fixture(&[(Some("QC-A"), Some("FS"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].left(), Some(0));
    assert_eq!(pairs[0].right(), None);
}

#[test]
fn files_with_no_metadata_are_not_lumped_together() {
    // Grouping every unlabelled file into one nameless specimen would put
    // unrelated samples side by side and imply they were linked.
    let (keys, names, metadata) = fixture(&[(None, None), (None, None)]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].files, vec![0]);
    assert_eq!(pairs[1].files, vec![1]);
}

#[test]
fn a_type_the_order_does_not_name_sorts_last() {
    // A third file in the specimen should not displace the pair being compared.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("Unstained")),
        (Some("QC-A"), Some("FS")),
        (Some("QC-A"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs[0].files, vec![2, 1, 0]);
    assert_eq!(pairs[0].left(), Some(2));
    assert_eq!(pairs[0].right(), Some(1));
}

#[test]
fn a_file_selected_by_name_finds_its_pair() {
    // Picking a full stain off the list should show its specimen, not jump.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FMX")),
        (Some("QC-B"), Some("FMX")),
        (Some("QC-B"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pair_of(&pairs, 2), Some(1));
    assert_eq!(pair_of(&pairs, 0), Some(0));
    assert_eq!(pair_of(&pairs, 99), None);
}

#[test]
fn a_file_with_a_specimen_but_no_type_still_pairs() {
    // A missing SampleType is common in older exports; the specimen is what
    // groups them, and the type only decides which side.
    let (keys, names, metadata) = fixture(&[(Some("QC-A"), None), (Some("QC-A"), Some("FMX"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].files, vec![1, 0], "the typed file takes the left");
}

#[test]
fn the_defaults_match_an_omiq_metadata_export() {
    // Real exports carry `SampleID` and `SampleType`, and three files per
    // specimen: unstained, FMO, full stain. A default that misses the column -
    // "Sample ID", with a space - fails silently: every file lands in its own
    // specimen, no partner is ever found, and the only symptom is one plot
    // where there should be two.
    let pairing = SamplePairing::default();
    assert_eq!(&*pairing.sample_id_column, "SampleID");
    assert_eq!(&*pairing.sample_type_column, "SampleType");

    let (keys, names, metadata) = fixture(&[
        (Some("QC5_N/A"), Some("U")),
        (Some("QC5_N/A"), Some("FMX")),
        (Some("QC5_N/A"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &pairing);

    assert_eq!(pairs.len(), 1, "one specimen, not three");
    assert_eq!(pairs[0].left(), Some(1), "the FMO is shown first");
    assert_eq!(pairs[0].right(), Some(2), "the full stain beside it");
}
