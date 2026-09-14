//! Tests for the metadata and scaling imports.
//!
//! Both read a CSV Omiq exported and turn it into the lookup tables everything
//! else depends on: which group a sample belongs to, which axis settings a
//! channel uses, and - crucially for the export path - how to get from a gating
//! id back to the id Omiq itself uses.
//!
//! cargo test metadata -- --nocapture

#![cfg(test)]

use crate::gate_editor::plots::axis_store::{AxisStore, Param, ScalingInfoSource, read_axis_configs};
use crate::omiq::metadata::{MetaDataOrigin, parse_metadata_csv};
use flow_fcs::TransformType;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Write a CSV to a uniquely named temp file and hand back the path.
fn temp_csv(name: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "clingate-{name}-{}-{:?}.csv",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut file = std::fs::File::create(&path).expect("temp file");
    file.write_all(contents.as_bytes()).expect("write");
    path
}

// ─── Metadata export ──────────────────────────────────────────────────────────

const METADATA: &str = "\
OmiqID,Filename,$VOL,Panel
F12345,SampleA.fcs,high,MyPanel
F67890,SampleB.fcs,low,MyPanel
";

fn parse_metadata(contents: &str, name: &str) -> crate::omiq::metadata::ParsedMetaData {
    let path = temp_csv(name, contents);
    let parsed = parse_metadata_csv(path.clone(), "OmiqID", "Filename", MetaDataOrigin::Omiq)
        .expect("metadata parses");
    let _ = std::fs::remove_file(path);
    parsed
}

#[test]
fn every_sample_gets_a_metadata_entry() {
    let parsed = parse_metadata(METADATA, "meta-basic");
    assert_eq!(parsed.metadata.len(), 2);
}

/// Omiq's metadata export prefixes ids with `F`, but the gating JSON does not.
/// The import strips it so the two line up.
#[test]
fn the_omiq_file_prefix_is_stripped_from_the_gating_id() {
    let parsed = parse_metadata(METADATA, "meta-prefix");

    assert!(
        parsed.metadata.contains_key(&Arc::from("12345") as &Arc<str>),
        "expected the unprefixed id, got {:?}",
        parsed.metadata.keys().collect::<Vec<_>>()
    );
    assert!(!parsed.metadata.contains_key(&Arc::from("F12345") as &Arc<str>));
}

/// The export path has to put the prefix back, so the reverse mapping is kept.
#[test]
fn the_original_omiq_id_is_recoverable_for_export() {
    let parsed = parse_metadata(METADATA, "meta-reverse");

    assert_eq!(
        parsed.gating_id_to_actual_id.get(&Arc::from("12345") as &Arc<str>),
        Some(&"F12345".to_string())
    );
    assert_eq!(
        parsed.gating_id_to_actual_id.get(&Arc::from("67890") as &Arc<str>),
        Some(&"F67890".to_string())
    );
}

#[test]
fn file_names_map_to_their_gating_ids() {
    let parsed = parse_metadata(METADATA, "meta-names");

    assert_eq!(
        parsed
            .file_name_to_gating_id
            .get(&Arc::from("SampleA.fcs") as &Arc<str>)
            .map(|id| id.to_string()),
        Some("12345".to_string())
    );
}

#[test]
fn each_sample_carries_its_metadata_columns() {
    let parsed = parse_metadata(METADATA, "meta-columns");
    let sample_a = parsed
        .metadata
        .get(&Arc::from("12345") as &Arc<str>)
        .expect("sample present");

    assert_eq!(
        sample_a.get(&Arc::from("$VOL") as &Arc<str>).map(|g| g.to_string()),
        Some("high".to_string())
    );
    assert_eq!(
        sample_a.get(&Arc::from("Panel") as &Arc<str>).map(|g| g.to_string()),
        Some("MyPanel".to_string())
    );
}

/// The id and name columns identify the sample; they are not themselves
/// metadata to group by.
#[test]
fn the_id_and_name_columns_are_not_treated_as_metadata() {
    let parsed = parse_metadata(METADATA, "meta-exclude");
    let sample_a = parsed.metadata.get(&Arc::from("12345") as &Arc<str>).unwrap();

    assert!(!sample_a.contains_key(&Arc::from("OmiqID") as &Arc<str>));
    assert!(!sample_a.contains_key(&Arc::from("Filename") as &Arc<str>));
    assert_eq!(sample_a.len(), 2, "just $VOL and Panel");
}

#[test]
fn samples_in_different_groups_are_kept_apart() {
    let parsed = parse_metadata(METADATA, "meta-groups");

    let a = parsed.metadata.get(&Arc::from("12345") as &Arc<str>).unwrap();
    let b = parsed.metadata.get(&Arc::from("67890") as &Arc<str>).unwrap();

    assert_ne!(
        a.get(&Arc::from("$VOL") as &Arc<str>),
        b.get(&Arc::from("$VOL") as &Arc<str>)
    );
}

#[test]
fn an_unprefixed_id_is_left_alone() {
    let parsed = parse_metadata(
        "OmiqID,Filename,$VOL\n999,Sample.fcs,high\n",
        "meta-noprefix",
    );

    assert!(parsed.metadata.contains_key(&Arc::from("999") as &Arc<str>));
}

#[test]
fn a_metadata_export_with_no_rows_parses_to_nothing() {
    let parsed = parse_metadata("OmiqID,Filename,$VOL\n", "meta-empty");
    assert!(parsed.metadata.is_empty());
}

// ─── Scaling export ───────────────────────────────────────────────────────────

const SCALING: &str = "\
Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z
FSC-A,,None (linear),1,0,4194304,0,0
BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0
";

fn parse_scaling(contents: &str, name: &str) -> Vec<crate::gate_editor::AxisInfo> {
    let path = temp_csv(name, contents);
    let configs = read_axis_configs(path.clone(), ScalingInfoSource::Omiq).expect("scaling parses");
    let _ = std::fs::remove_file(path);
    configs
}

#[test]
fn every_channel_in_the_export_becomes_an_axis() {
    assert_eq!(parse_scaling(SCALING, "scale-basic").len(), 2);
}

#[test]
fn a_linear_channel_keeps_its_raw_bounds() {
    let configs = parse_scaling(SCALING, "scale-linear");
    let fsc = configs
        .iter()
        .find(|a| &*a.param.fluoro == "FSC-A")
        .expect("FSC-A present");

    assert!(fsc.is_linear());
    assert_eq!(fsc.axis_lower, 0.0);
    assert_eq!(fsc.axis_upper, 4_194_304.0);
}

/// Omiq's Min/Max are raw values, but gates live in transformed space, so the
/// bounds are transformed on the way in.
#[test]
fn an_arcsinh_channel_stores_transformed_bounds() {
    let configs = parse_scaling(SCALING, "scale-arcsinh");
    let cd3 = configs
        .iter()
        .find(|a| &*a.param.fluoro == "BV421-A")
        .expect("BV421-A present");

    assert_eq!(cd3.get_cofactor(), Some(6000.0));
    assert!(
        cd3.axis_upper < 100.0,
        "bounds should be transformed, got {}",
        cd3.axis_upper
    );
    // And they convert back to what Omiq wrote.
    assert!((cd3.get_untransformed_upper() - 4_194_304.0).abs() / 4_194_304.0 < 1e-3);
}

/// The secondary column is the marker; when it is blank the channel names
/// itself, which is what scatter parameters do.
#[test]
fn a_channel_with_no_marker_names_itself() {
    let configs = parse_scaling(SCALING, "scale-marker");
    let fsc = configs.iter().find(|a| &*a.param.fluoro == "FSC-A").unwrap();
    let cd3 = configs.iter().find(|a| &*a.param.fluoro == "BV421-A").unwrap();

    assert_eq!(&*fsc.param.marker, "FSC-A");
    assert_eq!(&*cd3.param.marker, "CD3", "the marker comes from the secondary column");
}

/// Regression: an unrecognised scaling type was `unreachable!()`, so a single
/// unsupported row took every other axis in the file down with it.
#[test]
fn an_unsupported_scaling_type_is_skipped_not_fatal() {
    let configs = parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n\
         Odd-A,,Logicle,1,0,4194304,0,0\n\
         SSC-A,,None (linear),1,0,4194304,0,0\n",
        "scale-unsupported",
    );

    assert_eq!(configs.len(), 2, "the two supported channels survive");
    assert!(configs.iter().all(|a| &*a.param.fluoro != "Odd-A"));
}

#[test]
fn a_scaling_export_with_no_rows_parses_to_nothing() {
    let configs = parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n",
        "scale-empty",
    );
    assert!(configs.is_empty());
}

// ─── Applying scaling to the store ────────────────────────────────────────────

#[test]
fn applying_configs_registers_them_by_channel() {
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(SCALING, "scale-apply"));

    assert_eq!(store.settings.len(), 2);
    assert!(store.settings.contains_key(&Arc::from("FSC-A") as &Arc<str>));
    assert_eq!(store.sorted_settings.len(), 2, "display order is recorded");
}

#[test]
fn applying_configs_twice_replaces_rather_than_duplicates() {
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(SCALING, "scale-once"));
    store.apply_axis_configs(parse_scaling(SCALING, "scale-twice"));

    assert_eq!(store.settings.len(), 2);
    assert_eq!(store.sorted_settings.len(), 2);
}

#[test]
fn a_later_config_wins_for_the_same_channel() {
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(SCALING, "scale-first"));
    store.apply_axis_configs(parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         BV421-A,CD3,Arcsinh,250,-1000,4194304,0,0\n",
        "scale-second",
    ));

    let cd3 = store.settings.get(&Arc::from("BV421-A") as &Arc<str>).unwrap();
    assert_eq!(cd3.get_cofactor(), Some(250.0), "the newer cofactor applies");
    assert!(matches!(cd3.transform, TransformType::Arcsinh { .. }));
}

// ─── Opening axes ─────────────────────────────────────────────────────────────
//
// Regression: the selectors showed "Time" on both axes after loading a file.
// Two causes, both covered here. The index memo in main_window read the store
// with `peek`, which does not subscribe, so it kept the value computed before
// the scaling export had loaded - 0 - and displayed whichever channel came
// first. And it matched on the whole `Param`, which carries the marker name
// from the export, so the hardcoded FSC-A default could not match by equality.

/// Time first, and a marker name on the scatter channel, so a lookup that
/// matches whole `Param`s or falls back to index 0 lands on Time.
const SCALING_TIME_FIRST: &str = "\
Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z
Time,,None (linear),1,0,4194304,0,0
BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0
FSC-A,Forward Scatter,None (linear),1,0,4194304,0,0
SSC-A,Side Scatter,None (linear),1,0,4194304,0,0
";

fn store_with(contents: &str, name: &str) -> AxisStore {
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(contents, name));
    store
}

#[test]
fn a_channel_is_found_by_name_whatever_its_marker() {
    let store = store_with(SCALING_TIME_FIRST, "idx-marker");

    // The hardcoded default knows the channel but not the marker name, so an
    // equality match on Param fails where this succeeds.
    assert_eq!(store.index_of_fluoro("FSC-A"), Some(2));
    assert_eq!(store.index_of_fluoro("SSC-A"), Some(3));
    assert_eq!(
        store.sorted_settings.get_index_of(&Param {
            marker: Arc::from("FSC-A"),
            fluoro: Arc::from("FSC-A"),
        }),
        None,
        "matching the whole Param is what used to fail"
    );
}

#[test]
fn a_missing_channel_is_reported_rather_than_guessed() {
    let store = store_with(SCALING_TIME_FIRST, "idx-missing");
    assert_eq!(store.index_of_fluoro("CD4-A"), None);
}

#[test]
fn a_loaded_file_opens_on_the_scatter_pair() {
    let store = store_with(SCALING_TIME_FIRST, "open-scatter");
    let (x, y) = store.default_axis_params().expect("settings are loaded");

    assert_eq!(&*x.fluoro, "FSC-A", "not Time, whatever the file order");
    assert_eq!(&*y.fluoro, "SSC-A");
    // The marker name comes from the export, not from the hardcoded default.
    assert_eq!(&*x.marker, "Forward Scatter");
    assert_eq!(&*y.marker, "Side Scatter");
}

#[test]
fn a_file_without_scatter_opens_on_its_first_two_channels() {
    let store = store_with(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0\n\
         BV510-A,CD4,Arcsinh,6000,-1000,4194304,0,0\n",
        "open-no-scatter",
    );
    let (x, y) = store.default_axis_params().expect("settings are loaded");

    assert_eq!(&*x.fluoro, "BV421-A");
    assert_eq!(&*y.fluoro, "BV510-A");
}

#[test]
fn an_empty_store_has_no_opening_axes() {
    // Distinguishes "no scaling export yet" from "loaded"; the caller must not
    // commit to an axis before the export lands, which is the bug above.
    assert!(AxisStore::default().default_axis_params().is_none());
}

#[test]
fn a_single_channel_file_opens_on_that_channel_for_both_axes() {
    let store = store_with(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0\n",
        "open-single",
    );
    let (x, y) = store.default_axis_params().expect("settings are loaded");
    assert_eq!(&*x.fluoro, "BV421-A");
    assert_eq!(&*y.fluoro, "BV421-A", "falls back rather than failing");
}
