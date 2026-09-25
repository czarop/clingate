//! Tests for reading FCS headers without taking the application down.

#![cfg(test)]

use crate::file_load::FcsSampleStub;
use flow_fcs::TransformType;
use std::path::{Path, PathBuf};

/// A minimal valid FCS 3.1 file: two float parameters, `events` events.
///
/// Built by hand rather than checked in, so the test says exactly what a valid
/// file is, and so one can be written anywhere a test needs a folder of them.
pub fn write_fcs(path: &Path, events: usize) {
    write_fcs_with(path, events, &[("FSC-A", None), ("SSC-A", None)], &[]);
}

/// A minimal valid FCS 3.1 file with the channels given - each a `$PnN` and,
/// if there is one, a `$PnS` label - and any further keywords.
pub fn write_fcs_with(
    path: &Path,
    events: usize,
    channels: &[(&str, Option<&str>)],
    keywords: &[(&str, &str)],
) {
    let width = channels.len();
    let data: Vec<u8> = (0..events * width)
        .flat_map(|at| (at as f32).to_le_bytes())
        .collect();

    // The text segment's own offsets depend on its length, which depends on
    // the offsets; fixed-width numbers break the circle.
    let text_for = |text_start: usize, data_start: usize, data_end: usize| -> String {
        let mut pairs = vec![
            ("$BEGINANALYSIS", "0".to_string()),
            ("$ENDANALYSIS", "0".to_string()),
            ("$BEGINSTEXT", "0".to_string()),
            ("$ENDSTEXT", "0".to_string()),
            ("$BEGINDATA", format!("{data_start:08}")),
            ("$ENDDATA", format!("{data_end:08}")),
            ("$BYTEORD", "1,2,3,4".to_string()),
            ("$DATATYPE", "F".to_string()),
            ("$MODE", "L".to_string()),
            ("$NEXTDATA", "0".to_string()),
            ("$PAR", width.to_string()),
            ("$TOT", events.to_string()),
            (
                "$FIL",
                path.file_name().unwrap().to_string_lossy().to_string(),
            ),
        ];
        let _ = text_start;
        let mut text = String::from("/");
        for (k, v) in pairs.drain(..) {
            text.push_str(&format!("{k}/{v}/"));
        }
        for (n, (name, label)) in channels.iter().enumerate() {
            let n = n + 1;
            text.push_str(&format!(
                "$P{n}N/{name}/$P{n}B/32/$P{n}E/0,0/$P{n}R/262144/"
            ));
            if let Some(label) = label {
                text.push_str(&format!("$P{n}S/{label}/"));
            }
        }
        for (k, v) in keywords {
            text.push_str(&format!("{k}/{v}/"));
        }
        text
    };

    let text_start = 58;
    let probe = text_for(text_start, 0, 0);
    let text_end = text_start + probe.len() - 1;
    let data_start = text_end + 1;
    let data_end = data_start + data.len().saturating_sub(1);
    let text = text_for(text_start, data_start, data_end);
    assert_eq!(
        text.len(),
        probe.len(),
        "fixed-width offsets keep the length"
    );

    let mut header = String::from("FCS3.1    ");
    for offset in [text_start, text_end, data_start, data_end, 0, 0] {
        header.push_str(&format!("{offset:>8}"));
    }
    assert_eq!(header.len(), 58);

    let mut bytes = header.into_bytes();
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(&data);
    std::fs::write(path, bytes).expect("the fixture is written");
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("clingate-fcs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    dir
}

#[test]
fn a_valid_file_opens() {
    let dir = scratch("valid");
    let path = dir.join("good.fcs");
    write_fcs(&path, 10);
    let stub = FcsSampleStub::open(path.to_str().unwrap()).expect("a valid file opens");
    assert_eq!(*stub.get_number_of_events().unwrap(), 10);
    assert!(stub.parameters.contains_key("FSC-A"));
}

#[test]
fn an_empty_file_is_an_error_not_a_crash() {
    // flow_fcs slices the header without checking the file has one.
    let dir = scratch("empty");
    let path = dir.join("empty.fcs");
    std::fs::write(&path, b"").unwrap();
    let error = FcsSampleStub::open(path.to_str().unwrap()).expect_err("an empty file");
    assert!(
        error.to_string().contains("shorter than an FCS header"),
        "{error}"
    );
}

#[test]
fn a_tiny_file_is_an_error_not_a_crash() {
    let dir = scratch("tiny");
    let path = dir.join("tiny.fcs");
    std::fs::write(&path, b"FCS3.1").unwrap();
    assert!(FcsSampleStub::open(path.to_str().unwrap()).is_err());
}

#[test]
fn a_truncated_file_is_an_error_not_a_crash() {
    // The header is intact and points past the end of the file - the case
    // that panicked inside flow_fcs's text-segment reader.
    let dir = scratch("truncated");
    let path = dir.join("cut.fcs");
    write_fcs(&path, 10);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..80]).unwrap();
    let error = FcsSampleStub::open(path.to_str().unwrap()).expect_err("a truncated file");
    assert!(error.to_string().contains("truncated"), "{error}");
}

#[test]
fn a_file_that_is_not_fcs_is_an_error_not_a_crash() {
    let dir = scratch("notfcs");
    let path = dir.join("actually_a_csv.fcs");
    std::fs::write(&path, "a,b,c\n1,2,3\n".repeat(20)).unwrap();
    assert!(FcsSampleStub::open(path.to_str().unwrap()).is_err());
}

#[test]
fn a_file_missing_a_required_keyword_is_an_error_not_a_crash() {
    let dir = scratch("keyword");
    let path = dir.join("nokeyword.fcs");
    write_fcs(&path, 10);
    // Rename $TOT out of existence, keeping the length so the offsets hold.
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes
        .windows(4)
        .position(|w| w == b"$TOT")
        .expect("the fixture carries $TOT");
    bytes[at + 1..at + 4].copy_from_slice(b"XXX");
    std::fs::write(&path, bytes).unwrap();
    let error = FcsSampleStub::open(path.to_str().unwrap()).expect_err("no $TOT");
    assert!(error.to_string().contains("keywords"), "{error}");
}

#[test]
fn comparing_two_files_does_not_need_a_guid() {
    // Equality used to expect a GUID on both sides and panic without one.
    let dir = scratch("guid");
    let (a, b) = (dir.join("a.fcs"), dir.join("b.fcs"));
    write_fcs(&a, 5);
    write_fcs(&b, 5);
    let (a, b) = (
        FcsSampleStub::open(a.to_str().unwrap()).unwrap(),
        FcsSampleStub::open(b.to_str().unwrap()).unwrap(),
    );
    // Neither file carries a $GUID. (Opening gives each a random one - see
    // B-FCS-1 - so this holds whichever way they are told apart.)
    assert!(a != b, "two different files compared equal");
    assert!(a == a.clone());
}

/// BUG (docs/test-audit.md, B-FCS-1): flow_fcs's `validate_guid` looks the
/// GUID up as `GUID`, but keywords are stored as `$GUID`, so it never finds
/// one and inserts a random UUID - over the file's own `$GUID`. Every open
/// gives a file a new identity, so equality "by `$GUID`" compares two random
/// numbers: two copies of one acquisition, or one file opened twice, are
/// never equal.
#[test]
#[ignore = "known bug B-FCS-1: opening a file replaces its $GUID with a random one"]
fn two_copies_of_one_acquisition_are_equal_by_guid() {
    let dir = scratch("sameguid");
    let (a, b) = (dir.join("a.fcs"), dir.join("copy_of_a.fcs"));
    for path in [&a, &b] {
        write_fcs_with(
            path,
            5,
            &[("FSC-A", None)],
            &[("$GUID", "same-acquisition")],
        );
    }
    let (a, b) = (open(&a), open(&b));
    assert_eq!(a.get_guid().unwrap(), "same-acquisition");
    assert!(
        a == b,
        "the same $GUID is the same acquisition, wherever it is"
    );
}

#[test]
fn files_are_compared_by_guid_not_by_where_they_are() {
    let dir = scratch("setguid");
    let (a, b) = (dir.join("a.fcs"), dir.join("b.fcs"));
    write_fcs(&a, 3);
    write_fcs(&b, 3);
    let (mut a, mut b) = (open(&a), open(&b));
    a.set_guid("one".into());
    b.set_guid("one".into());
    assert_eq!(a.get_guid().unwrap(), "one");
    assert!(a == b, "the same GUID at two paths");

    b.set_guid("two".into());
    assert!(a != b, "different GUIDs");
}

fn open(path: &Path) -> FcsSampleStub {
    FcsSampleStub::open(path.to_str().unwrap()).expect("the fixture opens")
}

#[test]
fn the_extension_must_be_fcs_in_any_case() {
    let dir = scratch("extension");
    let upper = dir.join("UPPER.FCS");
    write_fcs(&upper, 3);
    assert!(FcsSampleStub::open(upper.to_str().unwrap()).is_ok());

    let wrong = dir.join("renamed.txt");
    write_fcs(&wrong, 3);
    let error = FcsSampleStub::open(wrong.to_str().unwrap()).expect_err("a .txt file");
    assert!(error.to_string().contains("extension"), "{error}");

    let none = dir.join("no_extension");
    write_fcs(&none, 3);
    assert!(FcsSampleStub::open(none.to_str().unwrap()).is_err());
}

#[test]
fn a_missing_file_is_an_error() {
    let error = FcsSampleStub::open("/no/such/file.fcs").expect_err("nothing there");
    assert!(error.to_string().contains("could not open"), "{error}");
}

#[test]
fn a_file_is_named_after_itself_until_renamed_for_the_program() {
    let dir = scratch("named");
    let path = dir.join("A1.fcs");
    write_fcs(&path, 3);
    let stub = open(&path);
    assert_eq!(stub.name(), "A1.fcs");
    assert_eq!(stub.get_filepath(), path);
    assert_eq!(stub.named("Plate_1_A1.fcs").name(), "Plate_1_A1.fcs");
}

#[test]
fn scatter_and_time_are_linear_and_everything_else_is_not() {
    let dir = scratch("transforms");
    let path = dir.join("panel.fcs");
    write_fcs_with(
        &path,
        3,
        &[
            ("FSC-A", None),
            ("SSC-H", None),
            ("Time", None),
            ("BV421-A", Some("CD3")),
        ],
        &[],
    );
    let stub = open(&path);
    for linear in ["FSC-A", "SSC-H", "Time"] {
        assert_eq!(
            stub.find_parameter(linear).unwrap().transform,
            TransformType::Linear,
            "{linear}"
        );
    }
    assert_eq!(
        stub.find_parameter("BV421-A").unwrap().transform,
        TransformType::default()
    );
}

#[test]
fn a_channel_is_labelled_with_its_marker_or_else_its_own_name() {
    let dir = scratch("labels");
    let path = dir.join("panel.fcs");
    write_fcs_with(&path, 3, &[("FSC-A", None), ("BV421-A", Some("CD3"))], &[]);
    let stub = open(&path);
    assert_eq!(&*stub.find_parameter("BV421-A").unwrap().label_name, "CD3");
    assert_eq!(&*stub.find_parameter("FSC-A").unwrap().label_name, "FSC-A");
    assert_eq!(*stub.get_number_of_parameters().unwrap(), 2);
}

#[test]
fn a_parameter_is_found_whatever_the_case_of_its_name() {
    let dir = scratch("findparam");
    let path = dir.join("a.fcs");
    write_fcs(&path, 3);
    let mut stub = open(&path);
    assert_eq!(stub.find_parameter("fsc-a").unwrap().parameter_number, 1);
    assert_eq!(stub.find_parameter("SSC-A").unwrap().parameter_number, 2);
    assert!(stub.find_parameter("CD3").is_err());

    stub.find_mutable_parameter("ssc-a").unwrap().transform =
        TransformType::Arcsinh { cofactor: 5.0 };
    assert_eq!(
        stub.find_parameter("SSC-A").unwrap().transform,
        TransformType::Arcsinh { cofactor: 5.0 }
    );
    assert!(stub.find_mutable_parameter("CD3").is_err());
}

#[test]
fn a_keyword_is_read_as_text_whatever_its_type() {
    let dir = scratch("keywords");
    let path = dir.join("a.fcs");
    write_fcs(&path, 7);
    let stub = open(&path);
    assert_eq!(stub.get_keyword_string_value("$FIL").unwrap(), "a.fcs");
    assert_eq!(stub.get_keyword_string_value("$TOT").unwrap(), "7");
    assert_eq!(stub.get_fil_keyword().unwrap(), "a.fcs");
    assert!(stub.get_keyword_string_value("$NOSUCH").is_err());
}
