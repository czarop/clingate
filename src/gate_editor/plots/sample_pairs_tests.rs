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
fn a_specimen_with_only_one_file_shows_it_on_its_own_side() {
    // The FMO was never run, so the left slot stays empty and the full stain
    // keeps the right. It used to slide over to fill the gap, which meant the
    // same side of the screen showed a control for one specimen and a stain for
    // the next - and comparing those two is exactly the mistake the paired
    // layout exists to prevent.
    let (keys, names, metadata) = fixture(&[(Some("QC-A"), Some("FS"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].left(), None);
    assert_eq!(pairs[0].right(), Some(0));
    // The file is still listed - it is shown, just not on the FMO's side.
    assert_eq!(pairs[0].files, vec![0]);
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

// ─── the slots each plot is drawn into ───────────────────────────────────────

#[test]
fn the_fmo_holds_its_side_even_when_the_specimen_has_none() {
    // The full stain must not slide over to fill the empty slot: the same side
    // of the screen showing the control for one specimen and the stain for the
    // next is how a person comes to compare the wrong two plots.
    let keys: Vec<Arc<str>> = vec![Arc::from("only_fs.fcs")];
    let mut names = HashMap::with_hasher(FxBuildHasher);
    names.insert(
        Arc::from("only_fs.fcs") as Arc<str>,
        Arc::from("f1") as Arc<str>,
    );
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
    columns.insert(Arc::from("SampleID"), Arc::from("A"));
    columns.insert(Arc::from("SampleType"), Arc::from("FS"));
    metadata.insert(Arc::from("f1") as Arc<str>, columns);

    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].left(),
        None,
        "no FMO, so the left slot stays empty"
    );
    assert_eq!(pairs[0].right(), Some(0), "and the stain keeps the right");
}

#[test]
fn the_fmo_is_on_the_left_whichever_order_the_folder_holds_them() {
    // Ranking alone let two files of unnamed types keep the folder's order,
    // which is how the two plots came to swap sides between specimens.
    for (first, second) in [("FMX", "FS"), ("FS", "FMX")] {
        let keys: Vec<Arc<str>> = vec![Arc::from("a.fcs"), Arc::from("b.fcs")];
        let mut names = HashMap::with_hasher(FxBuildHasher);
        names.insert(Arc::from("a.fcs") as Arc<str>, Arc::from("f1") as Arc<str>);
        names.insert(Arc::from("b.fcs") as Arc<str>, Arc::from("f2") as Arc<str>);
        let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
        for (id, kind) in [("f1", first), ("f2", second)] {
            let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
            columns.insert(Arc::from("SampleID"), Arc::from("A"));
            columns.insert(Arc::from("SampleType"), Arc::from(kind));
            metadata.insert(Arc::from(id) as Arc<str>, columns);
        }
        let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
        let left = pairs[0].left().unwrap();
        let right = pairs[0].right().unwrap();
        assert_ne!(left, right);
        // Whichever file it is, the left one is the FMO.
        let kind_of = |i: usize| {
            let id: Arc<str> = Arc::from(if i == 0 { "f1" } else { "f2" });
            metadata
                .get(&id)
                .unwrap()
                .get(&Arc::from("SampleType"))
                .unwrap()
                .clone()
        };
        assert_eq!(&*kind_of(left), "FMX", "folder order {first} then {second}");
        assert_eq!(&*kind_of(right), "FS");
    }
}

#[test]
fn a_specimen_the_display_order_does_not_name_is_still_shown() {
    // A misconfigured order must not blank the screen. It falls back to the
    // files in order; the pairing controls are where the mismatch is reported.
    let keys: Vec<Arc<str>> = vec![Arc::from("a.fcs"), Arc::from("b.fcs")];
    let mut names = HashMap::with_hasher(FxBuildHasher);
    names.insert(Arc::from("a.fcs") as Arc<str>, Arc::from("f1") as Arc<str>);
    names.insert(Arc::from("b.fcs") as Arc<str>, Arc::from("f2") as Arc<str>);
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for id in ["f1", "f2"] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from("A"));
        columns.insert(Arc::from("SampleType"), Arc::from("something else"));
        metadata.insert(Arc::from(id) as Arc<str>, columns);
    }
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(pairs[0].left(), Some(0));
    assert_eq!(pairs[0].right(), Some(1));
}

#[test]
fn specimens_follow_the_sort_column() {
    let keys: Vec<Arc<str>> = vec![Arc::from("a.fcs"), Arc::from("b.fcs"), Arc::from("c.fcs")];
    let mut names = HashMap::with_hasher(FxBuildHasher);
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for (key, id, day) in [
        ("a.fcs", "f1", "D85"),
        ("b.fcs", "f2", "D4"),
        ("c.fcs", "f3", "D29"),
    ] {
        names.insert(Arc::from(key) as Arc<str>, Arc::from(id) as Arc<str>);
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(day));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        columns.insert(Arc::from("Day"), Arc::from(day));
        metadata.insert(Arc::from(id) as Arc<str>, columns);
    }
    let pairing = SamplePairing {
        sort_column: Some(Arc::from("Day")),
        ..SamplePairing::default()
    };
    let pairs = pair_files(&keys, &names, &metadata, &pairing);
    let order: Vec<&str> = pairs
        .iter()
        .map(|p| p.specimen.as_deref().unwrap())
        .collect();
    assert_eq!(order, ["D4", "D29", "D85"], "not the folder's order");
}

/// The editor and the gallery both show a pair's first two slots
/// (`take(2)` in `main_window` and in `gallery::window`), and nothing else of
/// it: picking any file of a specimen from the list shows that specimen's
/// slots. So a file reaches the screen only through one of those two.
fn on_screen(pairs: &[crate::gate_editor::plots::sample_pairs::Pair], file: usize) -> bool {
    pairs
        .iter()
        .any(|p| p.slots.iter().take(2).any(|s| *s == Some(file)))
}

#[test]
#[ignore = "known bug B-PAIR-1: a second file of the same type is never shown"]
fn a_second_file_of_the_same_type_is_shown_somewhere() {
    // A tube acquired twice - a re-run after a clog - is two full stains for
    // one specimen. The list offers both; choosing the re-run showed the first
    // run beside the FMO, and the gallery and its PDF never drew it at all.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FMX")),
        (Some("QC-A"), Some("FS")),
        (Some("QC-A"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    for file in 0..keys.len() {
        assert!(
            on_screen(&pairs, file),
            "f{file} is never on screen: {pairs:?}"
        );
    }
}

#[test]
#[ignore = "known bug B-PAIR-1: past two files, a specimen with no named type loses the rest"]
fn every_file_of_an_untyped_specimen_is_shown_somewhere() {
    // With no type the display order names, a specimen falls back to its
    // files in order - all of them in its slots, of which two are shown.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("other")),
        (Some("QC-A"), Some("other")),
        (Some("QC-A"), Some("other")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    for file in 0..keys.len() {
        assert!(
            on_screen(&pairs, file),
            "f{file} is never on screen: {pairs:?}"
        );
    }
}

#[test]
fn every_file_of_a_well_formed_folder_is_shown() {
    // What the two tests above hold the pairing to, on the folders it was
    // built for: one FMX and one FS per specimen, some missing one, some files
    // the metadata does not know. Every file is on screen.
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FS")),
        (Some("B"), Some("FMX")),
        (None, None),
        (Some("A"), Some("FMX")),
        (Some("C"), Some("FS")),
        (Some("B"), Some("FS")),
        (None, None),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    for file in 0..keys.len() {
        assert!(
            on_screen(&pairs, file),
            "f{file} is never on screen: {pairs:?}"
        );
    }
}

// ── stepping through specimens: the editor's Previous and Next ───────────

use crate::gate_editor::plots::sample_pairs::step_from;

#[test]
fn next_lands_on_the_next_specimens_fmo_and_wraps() {
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FS")),
        (Some("A"), Some("FMX")),
        (Some("B"), Some("FS")),
        (Some("B"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    // From either of A's files, Next is B's FMX; from B, it wraps to A's.
    assert_eq!(step_from(&pairs, 0, 1), Some(3));
    assert_eq!(step_from(&pairs, 1, 1), Some(3));
    assert_eq!(step_from(&pairs, 2, 1), Some(1));
    // Previous wraps the other way, and a whole lap comes back.
    assert_eq!(step_from(&pairs, 0, -1), Some(3));
    assert_eq!(step_from(&pairs, 0, 2), Some(1));
}

#[test]
fn a_file_no_pair_holds_steps_from_the_first_specimen() {
    let (keys, names, metadata) = fixture(&[(Some("A"), Some("FMX")), (Some("B"), Some("FMX"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(step_from(&pairs, 99, 1), Some(1));
    assert_eq!(step_from(&[], 0, 1), None, "nothing to step through");
}

#[test]
#[ignore = "known bug B-NAV-1: Next stops dead at a specimen with no FMO"]
fn next_steps_onto_a_specimen_with_no_fmo() {
    // B has only its full stain, which keeps the right-hand side (the FMO's
    // side stays empty - see `the_fmo_holds_its_side_even_...`). Next lands
    // on a specimen's left file, finds none, and selects nothing: pressing it
    // again computes the same step from the same place, so the buttons can
    // never get past B.
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FMX")),
        (Some("A"), Some("FS")),
        (Some("B"), Some("FS")),
        (Some("C"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(pairs[1].left(), None, "B has no FMO, the premise");
    assert_eq!(step_from(&pairs, 0, 1), Some(2), "Next from A shows B");

    // And whatever the folder, pressing Next as many times as there are
    // specimens visits every one of them.
    let mut at = 0;
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..pairs.len() {
        at = step_from(&pairs, at, 1).unwrap_or(at);
        seen.insert(pair_of(&pairs, at));
    }
    assert_eq!(seen.len(), pairs.len(), "Next visits every specimen");
}
