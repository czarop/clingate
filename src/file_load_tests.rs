//! Tests for reading FCS headers without taking the application down.

#![cfg(test)]

use crate::file_load::FcsSampleStub;
use std::path::{Path, PathBuf};

/// A minimal valid FCS 3.1 file: two float parameters, `events` events.
///
/// Built by hand rather than checked in, so the test says exactly what a valid
/// file is, and so one can be written anywhere a test needs a folder of them.
pub fn write_fcs(path: &Path, events: usize) {
    let data: Vec<u8> = (0..events * 2)
        .flat_map(|at| (at as f32).to_le_bytes())
        .collect();

    // The text segment's own offsets depend on its length, which depends on
    // the offsets; fixed-width numbers break the circle.
    let text_for = |text_start: usize, data_start: usize, data_end: usize| -> String {
        let pairs = [
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
            ("$PAR", "2".to_string()),
            ("$TOT", events.to_string()),
            (
                "$FIL",
                path.file_name().unwrap().to_string_lossy().to_string(),
            ),
            ("$P1N", "FSC-A".to_string()),
            ("$P1B", "32".to_string()),
            ("$P1E", "0,0".to_string()),
            ("$P1R", "262144".to_string()),
            ("$P2N", "SSC-A".to_string()),
            ("$P2B", "32".to_string()),
            ("$P2E", "0,0".to_string()),
            ("$P2R", "262144".to_string()),
        ];
        let _ = text_start;
        let mut text = String::from("/");
        for (k, v) in pairs {
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
    let _ = a == b;
    assert!(a == a.clone());
}
