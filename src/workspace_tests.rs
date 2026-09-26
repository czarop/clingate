//! Tests for finding a workspace in a folder and naming its files.

#![cfg(test)]

use crate::file_load::FcsFiles;
use crate::file_load_tests::{scratch, write_fcs};
use crate::workspace::*;
use std::path::{Path, PathBuf};

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"").unwrap();
}

// ── finding the parts ────────────────────────────────────────────────────

#[test]
fn a_tidy_folder_is_recognised_whole() {
    let dir = scratch("tidy");
    touch(&dir.join("a.fcs"));
    touch(&dir.join("gates.omiqgt"));
    touch(&dir.join("run_metadata.csv"));
    touch(&dir.join("run_scaling.csv"));

    let found = detect(&dir).unwrap();
    assert_eq!(found.fcs, vec![dir.join("a.fcs")]);
    assert_eq!(found.gating, Found::One(dir.join("gates.omiqgt")));
    assert_eq!(found.metadata, Found::One(dir.join("run_metadata.csv")));
    assert_eq!(found.scaling, Found::One(dir.join("run_scaling.csv")));
}

#[test]
fn fcs_files_are_found_in_sub_folders() {
    let dir = scratch("nested");
    touch(&dir.join("top.fcs"));
    touch(&dir.join("Plate_9").join("a.fcs"));
    touch(&dir.join("Plate_10").join("WK1").join("b.fcs"));

    let found = detect(&dir).unwrap();
    assert_eq!(found.fcs.len(), 3);
    assert!(
        found
            .fcs
            .contains(&dir.join("Plate_10").join("WK1").join("b.fcs"))
    );
}

#[test]
fn everything_else_has_to_be_at_the_top() {
    // A sub-folder of old exports is common; a scaling file three folders down
    // is more likely last month's than the one wanted.
    let dir = scratch("toponly");
    touch(&dir.join("old").join("gates.omiqgt"));
    touch(&dir.join("old").join("metadata.csv"));
    touch(&dir.join("old").join("scaling.csv"));

    let found = detect(&dir).unwrap();
    assert_eq!(found.gating, Found::Missing);
    assert_eq!(found.metadata, Found::Missing);
    assert_eq!(found.scaling, Found::Missing);
}

#[test]
fn two_candidates_are_reported_rather_than_one_being_picked() {
    let dir = scratch("ambiguous");
    touch(&dir.join("metadata_v1.csv"));
    touch(&dir.join("metadata_v2.csv"));
    touch(&dir.join("a.omiqgt"));
    touch(&dir.join("b.omiqgt"));

    let found = detect(&dir).unwrap();
    assert!(matches!(found.metadata, Found::Several(ref v) if v.len() == 2));
    assert!(matches!(found.gating, Found::Several(ref v) if v.len() == 2));
}

#[test]
fn a_csv_naming_neither_is_left_alone() {
    let dir = scratch("othercsv");
    touch(&dir.join("counts.csv"));
    let found = detect(&dir).unwrap();
    assert_eq!(found.metadata, Found::Missing);
    assert_eq!(found.scaling, Found::Missing);
}

#[test]
fn names_and_extensions_are_matched_whatever_their_case() {
    let dir = scratch("case");
    touch(&dir.join("A.FCS"));
    touch(&dir.join("Gates.OMIQGT"));
    touch(&dir.join("Run_MetaData.CSV"));
    touch(&dir.join("SCALING.csv"));

    let found = detect(&dir).unwrap();
    assert_eq!(found.fcs.len(), 1);
    assert!(found.gating.one().is_some());
    assert!(found.metadata.one().is_some());
    assert!(found.scaling.one().is_some());
}

#[test]
fn a_csv_naming_both_is_ambiguous_for_both() {
    // Rather than letting the order of two lines of code decide which it is.
    let dir = scratch("both");
    touch(&dir.join("metadata_and_scaling.csv"));
    touch(&dir.join("metadata.csv"));
    let found = detect(&dir).unwrap();
    assert!(matches!(found.metadata, Found::Several(_)));
    assert_eq!(
        found.scaling,
        Found::One(dir.join("metadata_and_scaling.csv"))
    );
}

#[test]
fn hidden_files_are_skipped() {
    // macOS leaves `._name.fcs` beside every file it copies to a shared drive.
    // They end in .fcs and are not FCS files.
    let dir = scratch("hidden");
    touch(&dir.join("a.fcs"));
    touch(&dir.join("._a.fcs"));
    touch(&dir.join(".cache").join("b.fcs"));
    let found = detect(&dir).unwrap();
    assert_eq!(found.fcs, vec![dir.join("a.fcs")]);
}

#[cfg(unix)]
#[test]
fn a_link_back_up_the_tree_does_not_recurse_forever() {
    let dir = scratch("loop");
    touch(&dir.join("sub").join("a.fcs"));
    std::os::unix::fs::symlink(&dir, dir.join("sub").join("up")).unwrap();
    let found = fcs_under(&dir).unwrap();
    assert_eq!(found, vec![dir.join("sub").join("a.fcs")]);
}

#[test]
fn a_folder_that_is_not_there_is_an_error() {
    assert!(detect(Path::new("/no/such/folder/anywhere")).is_err());
}

// ── naming files ─────────────────────────────────────────────────────────

#[test]
fn a_top_level_file_keeps_its_name() {
    let root = Path::new("/w");
    assert_eq!(program_name(Some(root), Path::new("/w/A1.fcs")), "A1.fcs");
}

#[test]
fn a_file_in_a_sub_folder_takes_the_folder_into_its_name() {
    let root = Path::new("/w");
    assert_eq!(
        program_name(Some(root), Path::new("/w/Plate_10/A1.fcs")),
        "Plate_10_A1.fcs"
    );
}

#[test]
fn every_folder_below_the_workspace_is_in_the_name() {
    // The immediate folder alone would give both of these "WK1_A1.fcs".
    let root = Path::new("/w");
    let a = program_name(Some(root), Path::new("/w/Plate_9/WK1/A1.fcs"));
    let b = program_name(Some(root), Path::new("/w/Plate_10/WK1/A1.fcs"));
    assert_eq!(a, "Plate_9_WK1_A1.fcs");
    assert_eq!(b, "Plate_10_WK1_A1.fcs");
    assert_ne!(a, b);
}

#[test]
fn a_file_from_outside_the_workspace_keeps_its_name() {
    let root = Path::new("/w");
    assert_eq!(
        program_name(Some(root), Path::new("/elsewhere/Plate_3/A1.fcs")),
        "A1.fcs"
    );
    assert_eq!(program_name(None, Path::new("/x/A1.fcs")), "A1.fcs");
}

// ── the file list ────────────────────────────────────────────────────────

#[test]
fn files_from_sub_folders_are_loaded_under_their_program_names() {
    let dir = scratch("programnames");
    write_fcs(&dir.join("top.fcs"), 3);
    std::fs::create_dir_all(dir.join("Plate_10")).unwrap();
    write_fcs(&dir.join("Plate_10").join("A1.fcs"), 3);
    let paths = fcs_under(&dir).unwrap();
    let files = FcsFiles::open(Some(&dir), &paths);

    let names = files.get_file_names();
    assert!(names.contains(&"top.fcs".to_string()), "{names:?}");
    assert!(names.contains(&"Plate_10_A1.fcs".to_string()), "{names:?}");
    // The file is still opened from where it is.
    let stub = files
        .file_list()
        .iter()
        .find(|f| f.name() == "Plate_10_A1.fcs")
        .unwrap();
    assert_eq!(stub.get_filepath(), dir.join("Plate_10").join("A1.fcs"));
}

#[test]
fn a_bad_file_is_listed_with_its_reason_and_the_rest_still_load() {
    let dir = scratch("onebad");
    write_fcs(&dir.join("good.fcs"), 3);
    std::fs::write(dir.join("bad.fcs"), b"not an fcs file at all").unwrap();
    let files = FcsFiles::open(Some(&dir), &fcs_under(&dir).unwrap());

    assert_eq!(files.sample_count(), 1);
    assert_eq!(files.unread().len(), 1);
    assert_eq!(files.unread()[0].path, dir.join("bad.fcs"));
    assert!(!files.unread()[0].reason.is_empty());
}

#[test]
fn a_file_whose_program_name_is_taken_is_refused() {
    // The metadata matches by that name, so the second would silently be
    // given the first one's sample.
    let dir = scratch("clash");
    write_fcs(&dir.join("Plate_10_A1.fcs"), 3);
    std::fs::create_dir_all(dir.join("Plate_10")).unwrap();
    write_fcs(&dir.join("Plate_10").join("A1.fcs"), 3);
    let files = FcsFiles::open(Some(&dir), &fcs_under(&dir).unwrap());

    assert_eq!(files.sample_count(), 1);
    assert_eq!(files.unread().len(), 1);
    assert!(
        files.unread()[0].reason.contains("already used"),
        "{:?}",
        files.unread()
    );
}

#[test]
fn adding_the_same_file_twice_loads_it_once() {
    let dir = scratch("twice");
    write_fcs(&dir.join("a.fcs"), 3);
    let mut files = FcsFiles::open(Some(&dir), &[dir.join("a.fcs")]);
    files.add(&[dir.join("a.fcs")]);
    assert_eq!(files.sample_count(), 1);
    assert!(files.unread().is_empty());
}

#[test]
fn files_can_be_added_from_outside_the_workspace_and_removed() {
    let dir = scratch("addremove");
    let elsewhere = scratch("addremove-elsewhere");
    write_fcs(&dir.join("a.fcs"), 3);
    write_fcs(&elsewhere.join("b.fcs"), 3);

    let mut files = FcsFiles::open(Some(&dir), &[dir.join("a.fcs")]);
    files.add(&[elsewhere.join("b.fcs")]);
    assert_eq!(files.get_file_names(), vec!["a.fcs", "b.fcs"]);

    assert!(files.remove(&dir.join("a.fcs")));
    assert_eq!(files.get_file_names(), vec!["b.fcs"]);
    assert!(!files.remove(&dir.join("a.fcs")), "already gone");
}

#[test]
fn removing_an_unreadable_file_clears_its_report() {
    let dir = scratch("removebad");
    std::fs::write(dir.join("bad.fcs"), b"nope").unwrap();
    let mut files = FcsFiles::open(Some(&dir), &[dir.join("bad.fcs")]);
    assert_eq!(files.unread().len(), 1);
    assert!(files.remove(&dir.join("bad.fcs")));
    assert!(files.unread().is_empty());
}

#[test]
fn the_list_is_in_name_order_whatever_order_it_was_given() {
    let dir = scratch("order");
    for name in ["c.fcs", "a.fcs", "b.fcs"] {
        write_fcs(&dir.join(name), 2);
    }
    let files = FcsFiles::open(
        Some(&dir),
        &[dir.join("c.fcs"), dir.join("a.fcs"), dir.join("b.fcs")],
    );
    assert_eq!(files.get_file_names(), vec!["a.fcs", "b.fcs", "c.fcs"]);
}

// ── remembering ──────────────────────────────────────────────────────────

#[test]
fn a_remembered_workspace_survives_a_save_and_a_load() {
    let dir = scratch("remember");
    let at = dir.join("nested").join("workspace.json");
    let saved = Remembered {
        folder: Some(PathBuf::from("/data/run1")),
        fcs: vec![PathBuf::from("/data/run1/a.fcs")],
        metadata: Some(PathBuf::from("/data/run1/metadata.csv")),
        scaling: Some(PathBuf::from("/data/run1/scaling.csv")),
        gating: Some(PathBuf::from("/data/run1/gates.omiqgt")),
    };
    saved.save_to(&at).unwrap();
    assert_eq!(Remembered::load_from(&at).unwrap(), saved);
    assert!(
        !at.with_extension("json.tmp").exists(),
        "the temporary file is renamed away"
    );
}

#[test]
fn a_remembered_workspace_says_which_files_have_gone() {
    let dir = scratch("gone");
    write_fcs(&dir.join("here.fcs"), 2);
    let remembered = Remembered {
        fcs: vec![dir.join("here.fcs"), dir.join("gone.fcs")],
        scaling: Some(dir.join("scaling.csv")),
        ..Default::default()
    };
    let missing = remembered.missing();
    assert_eq!(missing.len(), 2);
    assert!(missing.contains(&dir.join("gone.fcs")));
    assert!(missing.contains(&dir.join("scaling.csv")));
}

#[test]
fn a_remembered_workspace_carries_no_rules() {
    // Rules are exported and imported on purpose; a new workspace starts
    // without any. Asserted on the serialised form so a field added later
    // cannot quietly start carrying them.
    let text = serde_json::to_string(&Remembered::default()).unwrap();
    assert!(!text.contains("rule"), "{text}");
}

#[cfg(unix)]
#[test]
fn a_linked_file_is_found_but_a_linked_folder_is_not_searched() {
    // Data on shared drives is often linked into a working folder rather than
    // copied; those files count. A linked folder is not searched.
    let dir = scratch("links");
    let elsewhere = scratch("links-elsewhere");
    touch(&elsewhere.join("real.fcs"));
    touch(&elsewhere.join("metadata.csv"));
    touch(&elsewhere.join("deeper").join("hidden_by_link.fcs"));
    std::os::unix::fs::symlink(elsewhere.join("real.fcs"), dir.join("linked.fcs")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("metadata.csv"), dir.join("metadata.csv")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("deeper"), dir.join("linked_folder")).unwrap();

    let found = detect(&dir).unwrap();
    assert_eq!(found.fcs, vec![dir.join("linked.fcs")]);
    assert_eq!(found.metadata, Found::One(dir.join("metadata.csv")));
}

#[cfg(unix)]
#[test]
fn a_link_that_points_nowhere_is_skipped_not_fatal() {
    let dir = scratch("dangling");
    touch(&dir.join("a.fcs"));
    std::os::unix::fs::symlink(dir.join("gone.fcs"), dir.join("dangling.fcs")).unwrap();
    let found = detect(&dir).unwrap();
    assert_eq!(found.fcs, vec![dir.join("a.fcs")]);
}

// ── what is left ─────────────────────────────────────────────────────────

#[test]
fn several_candidates_are_not_one() {
    let several = Found::Several(vec![PathBuf::from("a"), PathBuf::from("b")]);
    assert_eq!(several.one(), None);
    assert_eq!(Found::Missing.one(), None);
    assert_eq!(Found::One(PathBuf::from("a")).one(), Some(Path::new("a")));
}

#[test]
fn a_remembered_workspace_with_nothing_in_it_is_empty() {
    assert!(Remembered::default().is_empty());
    // A folder alone does not make a workspace worth offering to reopen.
    let folder_only = Remembered {
        folder: Some(PathBuf::from("/data")),
        ..Default::default()
    };
    assert!(folder_only.is_empty());
    for part in [
        Remembered {
            fcs: vec![PathBuf::from("a.fcs")],
            ..Default::default()
        },
        Remembered {
            metadata: Some(PathBuf::from("m.csv")),
            ..Default::default()
        },
        Remembered {
            scaling: Some(PathBuf::from("s.csv")),
            ..Default::default()
        },
        Remembered {
            gating: Some(PathBuf::from("g.omiqgt")),
            ..Default::default()
        },
    ] {
        assert!(!part.is_empty(), "{part:?}");
    }
}

#[test]
fn a_corrupt_remembered_workspace_is_an_error_not_a_default() {
    let dir = scratch("corrupt");
    let at = dir.join("workspace.json");
    std::fs::write(&at, "{ not json").unwrap();
    assert!(Remembered::load_from(&at).is_err());
    assert!(Remembered::load_from(&dir.join("absent.json")).is_err());
}

#[test]
fn a_remembered_workspace_from_an_older_version_still_loads() {
    // Every field defaults, so a file written before a field existed - or
    // one that names only the folder - is read rather than refused.
    let dir = scratch("older");
    let at = dir.join("workspace.json");
    std::fs::write(&at, r#"{"folder": "/data/run1"}"#).unwrap();
    let loaded = Remembered::load_from(&at).unwrap();
    assert_eq!(loaded.folder, Some(PathBuf::from("/data/run1")));
    assert!(loaded.fcs.is_empty());
}

/// Was B-WS-1: "outside the workspace" was decided by `strip_prefix`, which
/// compares components without resolving `..`, so a path that climbed out
/// through the workspace folder counted as inside it and was named
/// `.._elsewhere_A1.fcs`.
#[test]
fn a_path_that_climbs_out_of_the_workspace_keeps_its_own_name() {
    let root = Path::new("/w");
    assert_eq!(
        program_name(Some(root), Path::new("/w/../elsewhere/A1.fcs")),
        "A1.fcs"
    );
}

#[test]
fn dots_in_a_path_inside_the_workspace_name_the_file_by_where_it_really_is() {
    let root = Path::new("/w");
    for (path, name) in [
        ("/w/Plate_1/../Plate_2/A1.fcs", "Plate_2_A1.fcs"),
        ("/w/./Plate_1/./A1.fcs", "Plate_1_A1.fcs"),
        ("/w/Plate_1/WK1/../../A1.fcs", "A1.fcs"),
    ] {
        assert_eq!(program_name(Some(root), Path::new(path)), name, "{path}");
    }
    // A root written with dots of its own is the same folder.
    assert_eq!(
        program_name(
            Some(Path::new("/data/../w/.")),
            Path::new("/w/Plate_1/A1.fcs")
        ),
        "Plate_1_A1.fcs"
    );
    // Climbing above the filesystem root stays at it.
    assert_eq!(
        program_name(Some(root), Path::new("/../../w/Plate_1/A1.fcs")),
        "Plate_1_A1.fcs"
    );
}

#[test]
fn a_file_that_failed_is_tried_again_when_added_again() {
    let dir = scratch("retry");
    let path = dir.join("late.fcs");
    std::fs::write(&path, b"still copying").unwrap();
    let mut files = FcsFiles::open(Some(&dir), &[path.clone()]);
    assert_eq!(files.unread().len(), 1);

    write_fcs(&path, 3);
    files.add(&[path.clone()]);
    assert_eq!(files.sample_count(), 1);
    assert!(files.unread().is_empty(), "the old failure is not kept");
    assert_eq!(files.paths(), vec![path]);
    assert_eq!(files.root(), Some(dir.as_path()));
}

#[test]
fn a_refused_name_clash_is_reported_once_however_often_it_is_added() {
    let dir = scratch("clashtwice");
    write_fcs(&dir.join("Plate_1_A1.fcs"), 3);
    std::fs::create_dir_all(dir.join("Plate_1")).unwrap();
    write_fcs(&dir.join("Plate_1").join("A1.fcs"), 3);
    let mut files = FcsFiles::open(Some(&dir), &[dir.join("Plate_1_A1.fcs")]);
    files.add(&[dir.join("Plate_1").join("A1.fcs")]);
    files.add(&[dir.join("Plate_1").join("A1.fcs")]);
    assert_eq!(files.unread().len(), 1);
}

#[test]
fn only_fcs_files_are_collected_from_sub_folders() {
    let dir = scratch("onlyfcs");
    touch(&dir.join("sub").join("a.fcs"));
    touch(&dir.join("sub").join("notes.txt"));
    touch(&dir.join("sub").join("metadata.csv"));
    let found = fcs_under(&dir).unwrap();
    assert_eq!(found, vec![dir.join("sub").join("a.fcs")]);
}
