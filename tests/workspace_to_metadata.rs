//! From a folder on disk to each FCS file's metadata.
//!
//! Three modules meet here and each only knows its own half: `workspace`
//! finds the files and names them, `file_load` opens them under those names,
//! and `omiq::metadata` joins the metadata export to them by name. A file
//! whose name in the program is not what the metadata's file-name column
//! says has no specimen, no group and no gates - and nothing on its own side
//! of the seam can see that.

mod common;

use clingate::file_load::FcsFiles;
use clingate::omiq::metadata::{MetaDataOrigin, parse_metadata_csv};
use clingate::workspace::{Found, Remembered, detect};
use common::*;
use std::sync::Arc;

/// A run as Omiq users lay it out: plates in sub-folders, the same well
/// names on each plate, metadata and scaling at the top.
fn plate_folder(name: &str) -> std::path::PathBuf {
    let dir = scratch(name);
    write_small_fcs(&dir.join("QC.fcs"), 5);
    write_small_fcs(&dir.join("Plate_1").join("A1.fcs"), 5);
    write_small_fcs(&dir.join("Plate_2").join("A1.fcs"), 5);
    write_metadata(
        &dir.join("run_metadata.csv"),
        &["Plate", "SampleID"],
        &[
            ("F001", "QC.fcs", &["QC", "QC-A"]),
            ("F101", "Plate_1_A1.fcs", &["P1", "D-1"]),
            ("F201", "Plate_2_A1.fcs", &["P2", "D-2"]),
        ],
    );
    write_scaling(
        &dir.join("run_scaling.csv"),
        &[
            ("FSC-A", "", "None (linear)", 1, 0, 262_144),
            ("SSC-A", "", "None (linear)", 1, 0, 262_144),
        ],
    );
    dir
}

#[test]
fn every_file_in_a_plate_folder_finds_its_own_metadata() {
    let dir = plate_folder("plates");
    let found = detect(&dir).unwrap();
    let Found::One(metadata_path) = found.metadata else {
        panic!("one metadata file: {:?}", found.metadata);
    };
    assert!(matches!(found.scaling, Found::One(_)));

    let files = FcsFiles::open(Some(&dir), &found.fcs);
    assert!(files.unread().is_empty(), "{:?}", files.unread());
    let parsed =
        parse_metadata_csv(metadata_path, "OmiqID", "Filename", MetaDataOrigin::Omiq).unwrap();

    // Each file, under the name the program gives it, reaches the row the
    // export wrote for it - and the two wells called A1 are not confused.
    for (name, plate) in [
        ("QC.fcs", "QC"),
        ("Plate_1_A1.fcs", "P1"),
        ("Plate_2_A1.fcs", "P2"),
    ] {
        assert!(
            files.get_file_names().contains(&name.to_string()),
            "{name} is loaded"
        );
        let id = parsed
            .file_name_to_gating_id
            .get(&Arc::<str>::from(name))
            .unwrap_or_else(|| panic!("{name} has no metadata row"));
        let row = &parsed.metadata[id];
        assert_eq!(
            row.get(&Arc::<str>::from("Plate")).map(|p| &**p),
            Some(plate),
            "{name}"
        );
    }
}

#[test]
fn a_metadata_export_naming_wells_by_their_bare_names_reaches_neither() {
    // What an export from a workspace where the files were not in plate
    // folders says. The program names cannot match it, and the files are
    // left without metadata rather than quietly given one another's.
    let dir = plate_folder("barenames");
    write_metadata(
        &dir.join("run_metadata.csv"),
        &["Plate"],
        &[("F101", "A1.fcs", &["P1"]), ("F201", "A1.fcs", &["P2"])],
    );
    let files = FcsFiles::open(Some(&dir), &detect(&dir).unwrap().fcs);
    let parsed = parse_metadata_csv(
        dir.join("run_metadata.csv"),
        "OmiqID",
        "Filename",
        MetaDataOrigin::Omiq,
    )
    .unwrap();

    let reached: Vec<String> = files
        .get_file_names()
        .into_iter()
        .filter(|name| parsed.file_name_to_gating_id.contains_key(name.as_str()))
        .collect();
    assert!(reached.is_empty(), "{reached:?}");
    // Neither row is tied to a file called A1.fcs either, and that is said:
    // nothing tells which of the two it would be.
    assert!(parsed.file_name_to_gating_id.is_empty());
    assert_eq!(parsed.metadata.len(), 2, "both rows are kept, by id");
    assert_eq!(
        parsed
            .shared_names
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>(),
        ["A1.fcs (rows 2 and 3)"]
    );
}

/// Was B-META-1, end to end: a metadata row the export left without a file
/// name shifted every later file onto the row before it, so a file in plate 2
/// was put in plate 1's group. The row is now left out, and said so.
#[test]
fn an_incomplete_metadata_row_does_not_move_a_file_into_another_plate() {
    let dir = plate_folder("shifted");
    write_metadata(
        &dir.join("run_metadata.csv"),
        &["Plate"],
        &[
            ("F001", "QC.fcs", &["QC"]),
            ("F150", "", &["P1"]),
            ("F201", "Plate_2_A1.fcs", &["P2"]),
        ],
    );
    let parsed = parse_metadata_csv(
        dir.join("run_metadata.csv"),
        "OmiqID",
        "Filename",
        MetaDataOrigin::Omiq,
    )
    .unwrap();
    let id = &parsed.file_name_to_gating_id[&Arc::<str>::from("Plate_2_A1.fcs")];
    assert_eq!(
        parsed.metadata[id]
            .get(&Arc::<str>::from("Plate"))
            .map(|p| &**p),
        Some("P2")
    );
    assert_eq!(
        parsed
            .skipped
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>(),
        ["row 3 (F150) has no Filename"]
    );
}

#[test]
fn a_remembered_workspace_reopens_the_same_files_under_the_same_names() {
    let dir = plate_folder("reopen");
    let found = detect(&dir).unwrap();
    let mut files = FcsFiles::open(Some(&dir), &found.fcs);
    // One file removed by hand before closing: reopening must not bring it
    // back just because it is still in the folder.
    assert!(files.remove(&dir.join("Plate_2").join("A1.fcs")));

    let remembered = Remembered {
        folder: Some(dir.clone()),
        fcs: files.paths(),
        metadata: found.metadata.one().map(|p| p.to_path_buf()),
        scaling: found.scaling.one().map(|p| p.to_path_buf()),
        gating: None,
    };
    let at = dir.join("state").join("workspace.json");
    remembered.save_to(&at).unwrap();

    let back = Remembered::load_from(&at).unwrap();
    assert!(back.missing().is_empty());
    let reopened = FcsFiles::open(back.folder.as_deref(), &back.fcs);
    assert_eq!(reopened.get_file_names(), files.get_file_names());
    assert_eq!(reopened.get_file_names(), vec!["Plate_1_A1.fcs", "QC.fcs"]);
}
