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
    let rows: Vec<Vec<f32>> = (0..events)
        .map(|e| (0..width).map(|c| (e * width + c) as f32).collect())
        .collect();
    write_fcs_rows(path, channels, &rows, keywords);
}

/// A minimal valid FCS 3.1 file holding exactly these events, one row per
/// event and one value per channel.
pub fn write_fcs_rows(
    path: &Path,
    channels: &[(&str, Option<&str>)],
    rows: &[Vec<f32>],
    keywords: &[(&str, &str)],
) {
    let width = channels.len();
    let events = rows.len();
    let data: Vec<u8> = rows
        .iter()
        .flat_map(|row| {
            assert_eq!(row.len(), width, "one value per channel");
            row.iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()
        })
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
    // Neither file carries a $GUID, so opening gives each a random one of
    // its own.
    assert!(a != b, "two different files compared equal");
    assert!(a == a.clone());
}

/// Was B-FCS-1, fixed in flow_fcs: `validate_guid` looked the GUID up as
/// `GUID`, but keywords are stored as `$GUID`, so it never found one and
/// inserted a random UUID - over the file's own `$GUID`. Every open gave a
/// file a new identity, so equality "by `$GUID`" compared two random
/// numbers: two copies of one acquisition, or one file opened twice, were
/// never equal.
#[test]
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

/// A marker label holding the file's delimiter - written doubled, as the
/// standard escapes it - is read whole, and the channels after it keep
/// their own labels. It used to be cut at the delimiter: `CD45RA/RO` read as
/// `CD45RA`, and the axis was named, and matched to its scaling, wrongly.
#[test]
fn a_marker_label_holding_the_delimiter_is_read_whole() {
    let dir = scratch("escaped-label");
    let path = dir.join("panel.fcs");
    write_fcs_with(
        &path,
        3,
        &[
            ("FSC-A", None),
            ("BV421-A", Some("CD45RA//RO")),
            ("PE-A", Some("CD3")),
        ],
        &[],
    );
    let stub = open(&path);
    assert_eq!(
        &*stub.find_parameter("BV421-A").unwrap().label_name,
        "CD45RA/RO"
    );
    assert_eq!(&*stub.find_parameter("PE-A").unwrap().label_name, "CD3");
    assert_eq!(*stub.get_number_of_parameters().unwrap(), 3);
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

/// A file whose header places its data segment somewhere other than where
/// its events are - the leading digit of the data-start offset damaged to a
/// form feed, which reads as whitespace, so `313` reads as `13` - with every
/// event byte still present.
pub fn with_data_start_damaged(path: &Path) {
    write_fcs(path, 50);
    let mut bytes = std::fs::read(path).unwrap();
    // The header's third offset, the data start, is bytes 26..34.
    let lead = (26..34)
        .find(|at| bytes[*at] != b' ')
        .expect("the data start has digits");
    assert!(
        lead < 33,
        "the data start has more than one digit to damage"
    );
    bytes[lead] = 0x0c;
    std::fs::write(path, bytes).unwrap();
}

/// As [`with_data_start_damaged`], and `$BEGINDATA` damaged too - to
/// another number of the same width, so nothing else moves. Neither the
/// header nor the keywords now place the 50 events, and nothing shows where
/// they are.
pub fn with_data_offsets_damaged(path: &Path) {
    with_data_start_damaged(path);
    let mut bytes = std::fs::read(path).unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let at = text
        .find("$BEGINDATA/")
        .expect("the file states its data start")
        + "$BEGINDATA/".len();
    bytes[at..at + 8].copy_from_slice(b"00000100");
    std::fs::write(path, bytes).unwrap();
}

/// Was B-FCS-2: the workspace checked a file's header and keywords but not
/// that its events could be read, so a file whose header's data offset was
/// damaged was accepted - and reading its events tripped an assertion inside
/// flow_fcs ("Parameter 1 should have 50 events, got 88"). The workspace now
/// asks flow_fcs where the events are, as reading them does; where nothing
/// places them, the file is refused when the workspace opens.
#[test]
fn a_file_whose_events_cannot_be_located_is_refused() {
    let dir = scratch("datamismatch");
    let path = dir.join("damaged.fcs");
    with_data_offsets_damaged(&path);
    let why = FcsSampleStub::open(path.to_str().unwrap())
        .err()
        .expect("refused")
        .to_string();
    assert!(why.contains("its events cannot be read"), "{why}");
}

/// The header's data offset damaged, but `$BEGINDATA`/`$ENDDATA` intact and
/// placing exactly the 50 events: flow_fcs reads them from there, so the
/// workspace takes the file and its events come back as they were written.
#[test]
fn a_damaged_header_offset_the_keywords_overrule_is_read_correctly() {
    let dir = scratch("overruled");
    let path = dir.join("damaged.fcs");
    with_data_start_damaged(&path);
    assert!(FcsSampleStub::open(path.to_str().unwrap()).is_ok());
    let fcs = flow_fcs::Fcs::open(path.to_str().unwrap()).expect("read from the keywords");
    assert_eq!(fcs.data_frame.height(), 50);
}

/// A file cut short in its events is accepted by the workspace today, but
/// reading its events is refused cleanly ("Data end offset ... is beyond
/// mmap length"), so it costs an error on that file and nothing more.
#[test]
fn reading_the_events_of_a_file_cut_short_is_an_error() {
    let dir = scratch("cutdata");
    let path = dir.join("cut.fcs");
    write_fcs(&path, 100);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 200]).unwrap();
    let read = std::panic::catch_unwind(|| flow_fcs::Fcs::open(path.to_str().unwrap()).is_err());
    assert_eq!(
        read.ok(),
        Some(true),
        "a panic, or events read from a file cut short"
    );
}
