//! What the integration tests share: files on disk that look like the ones a
//! workspace is made of.
//!
//! The unit tests build their inputs in memory. These write real files - FCS,
//! metadata CSV, scaling CSV - because the seams under test are the ones where
//! a file written by one part of the program, or by Omiq, is read by another.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A fresh, empty folder for one test.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "clingate-it-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    dir
}

/// A minimal valid FCS 3.1 file with the given channels and events.
///
/// `events` is row-major: one `Vec` per event, one value per channel. Float
/// data, little-endian, list mode - the layout flow cytometers write.
pub fn write_fcs(path: &Path, channels: &[&str], events: &[Vec<f32>]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let data: Vec<u8> = events
        .iter()
        .flat_map(|row| {
            assert_eq!(row.len(), channels.len(), "one value per channel");
            row.iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()
        })
        .collect();

    // The text segment's own offsets depend on its length, which depends on
    // the offsets; fixed-width numbers break the circle.
    let text_for = |data_start: usize, data_end: usize| -> String {
        let mut text = String::from("/");
        let mut put = |k: &str, v: String| text.push_str(&format!("{k}/{v}/"));
        put("$BEGINANALYSIS", "0".into());
        put("$ENDANALYSIS", "0".into());
        put("$BEGINSTEXT", "0".into());
        put("$ENDSTEXT", "0".into());
        put("$BEGINDATA", format!("{data_start:08}"));
        put("$ENDDATA", format!("{data_end:08}"));
        put("$BYTEORD", "1,2,3,4".into());
        put("$DATATYPE", "F".into());
        put("$MODE", "L".into());
        put("$NEXTDATA", "0".into());
        put("$PAR", channels.len().to_string());
        put("$TOT", events.len().to_string());
        put(
            "$FIL",
            path.file_name().unwrap().to_string_lossy().to_string(),
        );
        for (n, name) in channels.iter().enumerate() {
            let n = n + 1;
            put(&format!("$P{n}N"), name.to_string());
            put(&format!("$P{n}B"), "32".into());
            put(&format!("$P{n}E"), "0,0".into());
            put(&format!("$P{n}R"), "262144".into());
        }
        text
    };

    let text_start = 58;
    let probe = text_for(0, 0);
    let text_end = text_start + probe.len() - 1;
    let data_start = text_end + 1;
    let data_end = data_start + data.len().saturating_sub(1);
    let text = text_for(data_start, data_end);
    assert_eq!(
        text.len(),
        probe.len(),
        "fixed-width offsets keep the length"
    );

    let mut header = String::from("FCS3.1    ");
    for offset in [text_start, text_end, data_start, data_end, 0, 0] {
        header.push_str(&format!("{offset:>8}"));
    }
    let mut bytes = header.into_bytes();
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(&data);
    std::fs::write(path, bytes).expect("the FCS file is written");
}

/// A small FCS file with scatter channels and `n` events.
pub fn write_small_fcs(path: &Path, n: usize) {
    let events: Vec<Vec<f32>> = (0..n)
        .map(|i| vec![1_000.0 + i as f32, 2_000.0 + i as f32])
        .collect();
    write_fcs(path, &["FSC-A", "SSC-A"], &events);
}

/// An Omiq metadata export: `OmiqID`, `Filename`, then the columns given.
pub fn write_metadata(path: &Path, columns: &[&str], rows: &[(&str, &str, &[&str])]) {
    let mut text = String::from("OmiqID,Filename");
    for c in columns {
        text.push(',');
        text.push_str(c);
    }
    text.push('\n');
    for (id, file, values) in rows {
        text.push_str(&format!("{id},{file}"));
        for v in *values {
            text.push(',');
            text.push_str(v);
        }
        text.push('\n');
    }
    std::fs::write(path, text).expect("the metadata is written");
}

/// An Omiq scaling export, one row per channel:
/// (channel, marker, "Arcsinh" or "None (linear)", cofactor, min, max).
pub fn write_scaling(path: &Path, rows: &[(&str, &str, &str, i64, i64, i64)]) {
    let mut text = String::from(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n",
    );
    for (channel, marker, kind, cofactor, min, max) in rows {
        text.push_str(&format!(
            "{channel},{marker},{kind},{cofactor},{min},{max},0,0\n"
        ));
    }
    std::fs::write(path, text).expect("the scaling is written");
}

/// The repository's fixtures folder.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The axis settings the checked-in gating fixtures were drawn on: scatter
/// linear, fluorescence arcsinh.
pub fn fixture_axes() -> clingate::omiq::serialise::AxisSettings {
    use clingate::gate_editor::AxisInfo;
    use clingate::gate_editor::plots::axis_store::Param;
    use flow_fcs::TransformType;
    use std::sync::Arc;

    let mut settings = im::HashMap::with_hasher(rustc_hash::FxBuildHasher);
    let mut put = |channel: &str, lower: f32, upper: f32, transform: TransformType| {
        settings.insert(
            Arc::from(channel) as Arc<str>,
            AxisInfo {
                param: Param {
                    marker: Arc::from(channel),
                    fluoro: Arc::from(channel),
                },
                axis_lower: lower,
                axis_upper: upper,
                transform,
            },
        );
    };
    for channel in [
        "BUV661-A",
        "BV785-A",
        "Alexa Fluor 700-A",
        "BUV737-A",
        "BUV805-A",
        "BUV563-A",
        "Alexa Fluor 647-A",
        "Vio Bright 423-A",
    ] {
        put(
            channel,
            -1.0,
            6.0,
            TransformType::Arcsinh { cofactor: 6000.0 },
        );
    }
    for channel in ["FSC-A", "SSC-A"] {
        put(channel, 0.0, 4_194_304.0, TransformType::Linear);
    }
    settings
}

/// Metadata for the fixtures' two files, one per group.
pub fn fixture_metadata() -> clingate::omiq::metadata::MetaDataFileMap {
    use std::sync::Arc;
    let mut map = im::HashMap::with_hasher(rustc_hash::FxBuildHasher);
    for (file, group) in [("sample1", "one"), ("sample2", "two")] {
        let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
        columns.insert(Arc::from("test"), Arc::from(group));
        columns.insert(Arc::from("Type"), Arc::from(group));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    map
}
