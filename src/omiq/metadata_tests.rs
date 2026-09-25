//! Tests for the metadata and scaling imports.
//!
//! Both read a CSV Omiq exported and turn it into the lookup tables everything
//! else depends on: which group a sample belongs to, which axis settings a
//! channel uses, and - crucially for the export path - how to get from a gating
//! id back to the id Omiq itself uses.
//!
//! cargo test metadata -- --nocapture

#![cfg(test)]

use crate::gate_editor::plots::axis_store::{
    AxisStore, Param, ScalingInfoSource, read_axis_configs,
};
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
        parsed
            .metadata
            .contains_key(&Arc::from("12345") as &Arc<str>),
        "expected the unprefixed id, got {:?}",
        parsed.metadata.keys().collect::<Vec<_>>()
    );
    assert!(
        !parsed
            .metadata
            .contains_key(&Arc::from("F12345") as &Arc<str>)
    );
}

/// The export path has to put the prefix back, so the reverse mapping is kept.
#[test]
fn the_original_omiq_id_is_recoverable_for_export() {
    let parsed = parse_metadata(METADATA, "meta-reverse");

    assert_eq!(
        parsed
            .gating_id_to_actual_id
            .get(&Arc::from("12345") as &Arc<str>),
        Some(&"F12345".to_string())
    );
    assert_eq!(
        parsed
            .gating_id_to_actual_id
            .get(&Arc::from("67890") as &Arc<str>),
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
        sample_a
            .get(&Arc::from("$VOL") as &Arc<str>)
            .map(|g| g.to_string()),
        Some("high".to_string())
    );
    assert_eq!(
        sample_a
            .get(&Arc::from("Panel") as &Arc<str>)
            .map(|g| g.to_string()),
        Some("MyPanel".to_string())
    );
}

/// The id and name columns identify the sample; they are not themselves
/// metadata to group by.
#[test]
fn the_id_and_name_columns_are_not_treated_as_metadata() {
    let parsed = parse_metadata(METADATA, "meta-exclude");
    let sample_a = parsed
        .metadata
        .get(&Arc::from("12345") as &Arc<str>)
        .unwrap();

    assert!(!sample_a.contains_key(&Arc::from("OmiqID") as &Arc<str>));
    assert!(!sample_a.contains_key(&Arc::from("Filename") as &Arc<str>));
    assert_eq!(sample_a.len(), 2, "just $VOL and Panel");
}

#[test]
fn samples_in_different_groups_are_kept_apart() {
    let parsed = parse_metadata(METADATA, "meta-groups");

    let a = parsed
        .metadata
        .get(&Arc::from("12345") as &Arc<str>)
        .unwrap();
    let b = parsed
        .metadata
        .get(&Arc::from("67890") as &Arc<str>)
        .unwrap();

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
    let fsc = configs
        .iter()
        .find(|a| &*a.param.fluoro == "FSC-A")
        .unwrap();
    let cd3 = configs
        .iter()
        .find(|a| &*a.param.fluoro == "BV421-A")
        .unwrap();

    assert_eq!(&*fsc.param.marker, "FSC-A");
    assert_eq!(
        &*cd3.param.marker, "CD3",
        "the marker comes from the secondary column"
    );
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

/// Nothing downstream is ready for a biexponential axis: the rescale helpers
/// treat it as linear, and the skewed quadrant and parts of `AxisInfo` stop
/// at `todo!()`. The importer is what keeps it out, so this holds it to that
/// - whatever the export calls a biexponential scale, the axis that comes
/// back is linear or arcsinh, or there is no axis at all.
#[test]
fn the_import_never_produces_a_biexponential_axis() {
    let configs = parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n\
         BV421-A,CD3,Arcsinh,150,-500,200000,0,0\n\
         PE-A,CD4,Logicle,1,0,262144,0,0\n\
         APC-A,CD8,Biexponential,1,0,262144,0,0\n\
         FITC-A,CD19,Biex,1,0,262144,0,0\n",
        "scale-biexponential",
    );

    let kept: Vec<&str> = configs.iter().map(|a| &*a.param.fluoro).collect();
    assert_eq!(kept, ["FSC-A", "BV421-A"]);
    assert!(configs.iter().all(|a| matches!(
        a.transform,
        TransformType::Linear | TransformType::Arcsinh { .. }
    )));
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
    assert!(
        store
            .settings
            .contains_key(&Arc::from("FSC-A") as &Arc<str>)
    );
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

    let cd3 = store
        .settings
        .get(&Arc::from("BV421-A") as &Arc<str>)
        .unwrap();
    assert_eq!(
        cd3.get_cofactor(),
        Some(250.0),
        "the newer cofactor applies"
    );
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

// ─── Replacing the scaling ────────────────────────────────────────────────────

#[test]
fn replacing_the_scaling_drops_channels_the_new_file_does_not_carry() {
    // Merging would carry the old file's settings for these into a workspace
    // that never had them.
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(SCALING_TIME_FIRST, "replace-first"));
    assert!(store.settings.contains_key(&Arc::from("Time") as &Arc<str>));

    store.replace_axis_configs(parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n",
        "replace-second",
    ));
    assert_eq!(store.settings.len(), 1);
    assert_eq!(store.sorted_settings.len(), 1);
    assert!(!store.settings.contains_key(&Arc::from("Time") as &Arc<str>));
}

#[test]
fn replacing_the_scaling_takes_the_new_files_order() {
    // Merging kept the old order for every channel the two files shared.
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(SCALING_TIME_FIRST, "order-first"));
    store.replace_axis_configs(parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         SSC-A,,None (linear),1,0,4194304,0,0\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n",
        "order-second",
    ));
    assert_eq!(store.index_of_fluoro("SSC-A"), Some(0));
    assert_eq!(store.index_of_fluoro("FSC-A"), Some(1));
}

#[test]
fn merging_still_merges() {
    // The replace is new; the merge keeps its meaning for anything that feeds
    // the store from more than one source.
    let mut store = AxisStore::default();
    store.apply_axis_configs(parse_scaling(SCALING_TIME_FIRST, "merge-first"));
    store.apply_axis_configs(parse_scaling(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         PE-A,CD8,Arcsinh,6000,-1000,4194304,0,0\n",
        "merge-second",
    ));
    assert!(store.settings.contains_key(&Arc::from("Time") as &Arc<str>));
    assert!(store.settings.contains_key(&Arc::from("PE-A") as &Arc<str>));
}

// ─── What replacing the scaling changes ───────────────────────────────────────

const SCALING_A: &str = "\
Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z
FSC-A,,None (linear),1,0,4194304,0,0
BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0
PE-A,CD8,Arcsinh,6000,-1000,4194304,0,0
";

fn diff_to(new: &str) -> crate::gate_editor::plots::axis_store::ScalingDiff {
    let loaded = store_with(SCALING_A, "diff-loaded");
    crate::gate_editor::plots::axis_store::scaling_diff(
        &loaded.settings,
        &parse_scaling(new, "diff-new"),
    )
}

#[test]
fn identical_scaling_changes_nothing() {
    let diff = diff_to(SCALING_A);
    assert!(diff.changed.is_empty());
    assert!(diff.dropped.is_empty());
}

#[test]
fn a_new_cofactor_is_a_transform_change() {
    let diff = diff_to(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n\
         BV421-A,CD3,Arcsinh,250,-1000,4194304,0,0\n\
         PE-A,CD8,Arcsinh,6000,-1000,4194304,0,0\n",
    );
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(&*diff.changed[0].channel, "BV421-A");
    assert!(diff.changed[0].transform_changed());
}

#[test]
fn a_new_range_is_a_range_change_only() {
    let diff = diff_to(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n\
         BV421-A,CD3,Arcsinh,6000,-5000,4194304,0,0\n\
         PE-A,CD8,Arcsinh,6000,-1000,4194304,0,0\n",
    );
    assert_eq!(diff.changed.len(), 1);
    assert!(diff.changed[0].range_changed());
    assert!(!diff.changed[0].transform_changed());
}

#[test]
fn a_channel_the_new_file_drops_is_reported_not_changed() {
    let diff = diff_to(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n\
         BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0\n",
    );
    assert!(diff.changed.is_empty());
    assert_eq!(diff.dropped, vec![Arc::<str>::from("PE-A")]);
}

#[test]
fn a_channel_only_the_new_file_has_changes_nothing() {
    // No gate can be on a channel the old scaling did not carry - the import
    // would have refused it - so there is nothing to carry across.
    let diff = diff_to(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),1,0,4194304,0,0\n\
         BV421-A,CD3,Arcsinh,6000,-1000,4194304,0,0\n\
         PE-A,CD8,Arcsinh,6000,-1000,4194304,0,0\n\
         APC-A,CD4,Arcsinh,6000,-1000,4194304,0,0\n",
    );
    assert!(diff.changed.is_empty());
    assert!(diff.dropped.is_empty());
}

// ─── Keeping the axes across a scaling replace ────────────────────────────────

fn param(marker: &str, fluoro: &str) -> crate::gate_editor::plots::axis_store::Param {
    crate::gate_editor::plots::axis_store::Param {
        marker: Arc::from(marker),
        fluoro: Arc::from(fluoro),
    }
}

#[test]
fn the_chosen_axes_are_kept_when_the_scaling_still_has_them() {
    use crate::gate_editor::plots::axis_store::resolve_axes;
    let store = store_with(SCALING_A, "resolve-keep");
    let (x, y) = resolve_axes(
        &store.sorted_settings,
        &param("CD3", "BV421-A"),
        &param("CD8", "PE-A"),
    )
    .expect("a scaling is loaded");
    assert_eq!(&*x.fluoro, "BV421-A");
    assert_eq!(&*y.fluoro, "PE-A");
}

#[test]
fn a_placeholder_marker_is_replaced_by_the_scalings_own() {
    // The editor starts on "FSC-A" with no marker name; the scaling export
    // carries the real one, and the axis should show it.
    use crate::gate_editor::plots::axis_store::resolve_axes;
    let store = store_with(SCALING_TIME_FIRST, "resolve-marker");
    let (x, _) = resolve_axes(
        &store.sorted_settings,
        &param("FSC-A", "FSC-A"),
        &param("SSC-A", "SSC-A"),
    )
    .unwrap();
    assert_eq!(&*x.marker, "Forward Scatter");
}

#[test]
fn an_axis_on_a_dropped_channel_falls_back_to_the_default() {
    use crate::gate_editor::plots::axis_store::{default_axis_params, resolve_axes};
    let store = store_with(SCALING_A, "resolve-dropped");
    let (default_x, _) = default_axis_params(&store.sorted_settings).unwrap();
    let (x, y) = resolve_axes(
        &store.sorted_settings,
        &param("CD4", "APC-A"),
        &param("CD8", "PE-A"),
    )
    .unwrap();
    assert_eq!(x, default_x, "APC-A is not in this scaling");
    assert_eq!(&*y.fluoro, "PE-A", "the other axis is kept");
}

#[test]
fn nothing_is_resolved_before_a_scaling_has_loaded() {
    use crate::gate_editor::plots::axis_store::resolve_axes;
    let empty = AxisStore::default();
    assert!(resolve_axes(&empty.sorted_settings, &param("a", "a"), &param("b", "b")).is_none());
}

// ─── Rows the export left incomplete ──────────────────────────────────────────

/// BUG (docs/test-audit.md, B-META-1): a row with no file name (or no id) is
/// skipped when the ids are collected, but the metadata columns are then
/// read by position in that shortened list - so every file after the
/// skipped row is given the row before it. Here SampleC would be put in
/// SampleA's group.
#[test]
#[ignore = "known bug B-META-1: a skipped row shifts every later file's metadata by one"]
fn a_row_without_a_file_name_does_not_shift_the_rows_after_it() {
    let parsed = parse_metadata(
        "\
OmiqID,Filename,Group
F1,SampleA.fcs,first
F2,,second
F3,SampleC.fcs,third
",
        "meta-shift",
    );
    let group = |id: &str| {
        parsed.metadata[&Arc::<str>::from(id)]
            .get(&Arc::<str>::from("Group"))
            .map(|g| g.to_string())
    };
    assert_eq!(group("1").as_deref(), Some("first"));
    assert_eq!(group("3").as_deref(), Some("third"));
}

#[test]
fn a_row_without_a_file_name_is_left_out() {
    let parsed = parse_metadata(
        "\
OmiqID,Filename,Group
F1,SampleA.fcs,first
F2,,second
",
        "meta-noname",
    );
    assert!(!parsed.metadata.contains_key(&Arc::<str>::from("2")));
    assert_eq!(parsed.file_name_to_gating_id.len(), 1);
}

#[test]
fn a_blank_metadata_value_is_absent_rather_than_empty() {
    let parsed = parse_metadata(
        "\
OmiqID,Filename,Group,Plate
F1,SampleA.fcs,,P1
",
        "meta-blank",
    );
    let row = &parsed.metadata[&Arc::<str>::from("1")];
    assert_eq!(
        row.get(&Arc::<str>::from("Plate")).map(|p| &**p),
        Some("P1")
    );
    assert_eq!(row.get(&Arc::<str>::from("Group")), None);
}

#[test]
fn a_numeric_looking_column_is_read_as_text() {
    // Every column is read as a string: a plate numbered 007 must stay 007,
    // since it is matched against the gating file's group names as text.
    let parsed = parse_metadata(
        "\
OmiqID,Filename,Plate
F1,SampleA.fcs,007
",
        "meta-text",
    );
    assert_eq!(
        parsed.metadata[&Arc::<str>::from("1")]
            .get(&Arc::<str>::from("Plate"))
            .map(|p| &**p),
        Some("007")
    );
}

#[test]
fn a_metadata_file_missing_its_id_column_is_an_error() {
    let path = temp_csv("meta-noid", "Filename,Group\nSampleA.fcs,g\n");
    let result = parse_metadata_csv(path.clone(), "OmiqID", "Filename", MetaDataOrigin::Omiq);
    let _ = std::fs::remove_file(path);
    let error = result.err().expect("no OmiqID column");
    assert!(error.to_string().contains("OmiqID"), "{error}");
}

#[test]
fn a_metadata_file_that_is_not_there_is_an_error() {
    assert!(
        parse_metadata_csv(
            PathBuf::from("/no/such/metadata.csv"),
            "OmiqID",
            "Filename",
            MetaDataOrigin::Omiq
        )
        .is_err()
    );
}
