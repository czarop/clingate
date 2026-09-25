//! Naming the metadata columns that say which files belong together.
//!
//! This lives beside the sample selector rather than only in the rules tab
//! because it decides what the main window shows: get the column wrong and
//! every file lands in its own specimen, so the second plot never appears and
//! nothing can be positioned. Being able to see and fix it where the symptom
//! shows is worth the small amount of space.

use crate::file_load::FcsFiles;
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::{MetaDataFileMap, MetaDataStore, MetaDataStoreStoreExt};
use dioxus::prelude::*;
use std::sync::Arc;

/// Every column the loaded metadata carries, sorted.
pub fn metadata_columns(metadata: &MetaDataFileMap) -> Vec<Arc<str>> {
    let mut out: Vec<Arc<str>> = Vec::new();
    for columns in metadata.values() {
        for name in columns.keys() {
            if !out.iter().any(|existing| existing == name) {
                out.push(name.clone());
            }
        }
    }
    out.sort();
    out
}

/// The columns to offer, with the current choice always among them.
///
/// A select whose value is not in its options shows the wrong thing, and the
/// stored column can legitimately name something this metadata does not have -
/// a sidecar written for another export, or a file not loaded yet.
///
/// The stray choice goes on the **end**, never the front. Put at the front it
/// shifted every real column down one while it was there, and choosing one made
/// it vanish and shift them all back - so the first pick from a freshly loaded
/// export landed on the neighbour of the row that had been clicked. Appending
/// leaves every real column at the same index whether the stray is there or
/// not, so nothing moves under the pointer.
pub fn offered(columns: &[Arc<str>], current: &Arc<str>) -> Vec<Arc<str>> {
    let mut out = columns.to_vec();
    if !out.iter().any(|c| c == current) {
        out.push(current.clone());
    }
    out
}

/// The two column pickers, plus how many files the sample id column reaches.
#[component]
pub fn PairingColumns() -> Element {
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let mut rules = use_context::<Signal<RuleStore>>();

    let columns = use_memo(move || metadata_columns(&metadata_store.metadata().read()));

    let filehandler = use_context::<Signal<Option<FcsFiles>>>();

    // How many of the *open files* the column reaches - not how many rows the
    // metadata export happens to carry. A metadata file usually describes a
    // whole experiment while a folder holds part of one, so counting its rows
    // would report on files nobody has loaded and stay quiet about the ones on
    // screen. A column that matches nothing is the quietest way for all of this
    // to go wrong: every file lands in its own specimen, no partner is ever
    // found, and the only symptom is one plot where there should be two.
    let reach = use_memo(move || {
        let wanted = rules.read().pairing.sample_id_column.clone();
        let Some(files) = filehandler.read().as_ref().map(|f| f.file_list().to_vec()) else {
            return (0, 0);
        };
        let ids = metadata_store.file_name_to_gating_id();
        let ids = ids.read();
        let lens = metadata_store.metadata();
        let metadata = lens.read();

        let matched = files
            .iter()
            .filter(|stub| {
                // The same name plot_window and the pairing look a file up by.
                ids.get(&stub.name)
                    .and_then(|id| metadata.get(id))
                    .is_some_and(|columns| columns.contains_key(&wanted))
            })
            .count();
        (matched, files.len())
    });

    // Which sample types the loaded files actually resolve to, and how many
    // resolve to none at all.
    //
    // Nothing used to say when this failed, and it fails quietly and totally:
    // with no type, the two plots cannot be told apart so they keep the
    // folder's order and swap sides between specimens, and the autogater
    // cannot tell a full stain from its FMO so it reads whichever file sorts
    // first. A whole run came back unpositioned because every rule had been
    // measured against an FMO, and not one message said so.
    let types = use_memo(move || {
        let pairing = rules.read().pairing.clone();
        let Some(files) = filehandler.read().as_ref().map(|f| f.file_list().to_vec()) else {
            return (Vec::<(Arc<str>, usize)>::new(), 0usize);
        };
        let ids = metadata_store.file_name_to_gating_id();
        let ids = ids.read();
        let lens = metadata_store.metadata();
        let metadata = lens.read();

        let mut counts: Vec<(Arc<str>, usize)> = Vec::new();
        let mut untyped = 0usize;
        for stub in &files {
            let found = ids
                .get(&stub.name)
                .and_then(|id| metadata.get(id))
                .and_then(|columns| pairing.sample_type_of(columns));
            match found {
                Some(kind) => match counts.iter_mut().find(|(k, _)| *k == kind) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((kind, 1)),
                },
                None => untyped += 1,
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        (counts, untyped)
    });

    rsx! {
        div { class: "pairing-columns",
            label { "Sample ID" }
            select {
                value: "{rules.read().pairing.sample_id_column}",
                onchange: move |e| {
                    rules.write().pairing.sample_id_column = Arc::from(e.value().as_str());
                },
                // `selected` as well as the select's `value`: the options
                // arrive with the metadata, after the value was set, and a
                // select given a value before its option exists shows the
                // first option instead - here, the Sample type box read
                // "SampleID" while pairing by SampleType. See the gate picker
                // in `gate_rules_window` for the same fix.
                for name in offered(&columns.read(), &rules.read().pairing.sample_id_column) {
                    option {
                        key: "{name}",
                        value: "{name}",
                        selected: *name == *rules.read().pairing.sample_id_column,
                        "{name}"
                    }
                }
            }

            label { "Sample type" }
            select {
                value: "{rules.read().pairing.sample_type_column}",
                onchange: move |e| {
                    rules.write().pairing.sample_type_column = Arc::from(e.value().as_str());
                },
                for name in offered(&columns.read(), &rules.read().pairing.sample_type_column) {
                    option {
                        key: "{name}",
                        value: "{name}",
                        selected: *name == *rules.read().pairing.sample_type_column,
                        "{name}"
                    }
                }
            }

            {
                let (found, untyped) = types();
                let order = rules.read().pairing.display_order.clone();
                let named: Vec<Arc<str>> = found
                    .iter()
                    .map(|(k, _)| k.clone())
                    .filter(|k| !order.contains(k))
                    .collect();
                if found.is_empty() {
                    rsx! {
                        span { class: "pairing-columns_note pairing-columns_warn",
                            "No loaded file has a value in this column. Without a sample type the two plots cannot be told apart, so they keep the folder's order, and a rule reads whichever file of a specimen sorts first rather than its full stain."
                        }
                    }
                } else {
                    let listed = found
                        .iter()
                        .map(|(k, n)| format!("{k} ({n})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if untyped > 0 || !named.is_empty() {
                        let mut trouble = Vec::new();
                        if untyped > 0 {
                            trouble.push(format!("{untyped} files have no type"));
                        }
                        if !named.is_empty() {
                            trouble.push(format!(
                                "{} is not in the plot order, so it is shown only when picked above the second plot, and a rule never gates it",
                                named
                                    .iter()
                                    .map(|k| k.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                        rsx! {
                            span { class: "pairing-columns_note pairing-columns_warn",
                                "Found {listed} - but {trouble.join(\"; \")}."
                            }
                        }
                    } else {
                        rsx! {
                            span { class: "pairing-columns_note", "{listed}." }
                        }
                    }
                }
            }

            label { "Plot order" }
            input {
                title: "The sample types, left plot first. A specimen with no file of the first type leaves that plot empty rather than sliding its other one across. This is also what tells a rule which file of a specimen is the full stain to gate.",
                value: "{rules.read().pairing.display_order.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(\", \")}",
                onchange: move |e| {
                    let order: Vec<Arc<str>> = e
                        .value()
                        .split(',')
                        .map(|t| t.trim())
                        .filter(|t| !t.is_empty())
                        .map(Arc::from)
                        .collect();
                    if !order.is_empty() {
                        rules.write().pairing.display_order = order;
                    }
                },
            }
            label { "Sort by" }
            select {
                value: "{rules.read().pairing.sort_column.clone().unwrap_or_default()}",
                onchange: move |e| {
                    let picked = e.value();
                    rules.write().pairing.sort_column = if picked.is_empty() {
                        None
                    } else {
                        Some(Arc::from(picked.as_str()))
                    };
                },
                option { key: "", value: "", "the folder's own order" }
                for name in columns.read().iter() {
                    option {
                        key: "{name}",
                        value: "{name}",
                        selected: rules.read().pairing.sort_column.as_ref() == Some(name),
                        "{name}"
                    }
                }
            }

            {
                let (matched, total) = reach();
                let column = rules.read().pairing.sample_id_column.clone();
                if total == 0 {
                    rsx! {
                        span { class: "pairing-columns_note", "No files loaded." }
                    }
                } else if matched == total {
                    rsx! {
                        span { class: "pairing-columns_note", "{total} files grouped." }
                    }
                } else {
                    rsx! {
                        span { class: "pairing-columns_note pairing-columns_warn",
                            "Only {matched} of {total} files carry {column} - the rest cannot be paired or positioned."
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<Arc<str>> {
        names.iter().map(|n| Arc::from(*n)).collect()
    }

    #[test]
    fn a_column_the_metadata_has_is_offered_once_and_in_place() {
        let columns = cols(&["Donor_Day", "Plate", "Type"]);
        let offer = offered(&columns, &Arc::from("Type"));
        assert_eq!(offer, columns, "nothing added, nothing moved");
    }

    #[test]
    fn a_column_the_metadata_lacks_is_offered_without_moving_the_others() {
        // The bug this guards: prepended, the stray pushed every real column
        // down one index. Choosing one then removed it and shifted them all
        // back, so the first pick from a freshly loaded export landed on the
        // neighbour of the row that had been clicked.
        let columns = cols(&["Donor_Day", "Plate", "Type"]);
        let stray = offered(&columns, &Arc::from("SampleType"));

        assert_eq!(stray.len(), columns.len() + 1);
        assert_eq!(
            &stray[..columns.len()],
            &columns[..],
            "every real column keeps the index it has without the stray"
        );
        assert_eq!(&*stray[columns.len()], "SampleType", "the stray goes last");
    }

    #[test]
    fn the_real_columns_sit_at_the_same_index_either_way() {
        // Stated as the property rather than the arrangement: what matters is
        // that nothing moves under the pointer when the stray disappears.
        let columns = cols(&["Donor_Day", "Plate", "Type"]);
        let before = offered(&columns, &Arc::from("SampleType"));
        let after = offered(&columns, &Arc::from("Type"));
        for (i, name) in columns.iter().enumerate() {
            assert_eq!(&before[i], name);
            assert_eq!(&after[i], name);
        }
    }

    #[test]
    fn columns_are_gathered_across_files_and_sorted() {
        use rustc_hash::{FxBuildHasher, FxHashMap};
        let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
        for (id, extra) in [("f1", "Plate"), ("f2", "Donor_Day")] {
            let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
            columns.insert(Arc::from("Type"), Arc::from("FS"));
            columns.insert(Arc::from(extra), Arc::from("x"));
            metadata.insert(Arc::from(id) as Arc<str>, columns);
        }
        let found = metadata_columns(&metadata);
        assert_eq!(found, cols(&["Donor_Day", "Plate", "Type"]));
    }
}
