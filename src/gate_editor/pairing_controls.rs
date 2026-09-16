//! Naming the metadata columns that say which files belong together.
//!
//! This lives beside the sample selector rather than only in the rules tab
//! because it decides what the main window shows: get the column wrong and
//! every file lands in its own specimen, so the second plot never appears and
//! nothing can be positioned. Being able to see and fix it where the symptom
//! shows is worth the small amount of space.

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
pub fn offered(columns: &[Arc<str>], current: &Arc<str>) -> Vec<Arc<str>> {
    let mut out = columns.to_vec();
    if !out.iter().any(|c| c == current) {
        out.insert(0, current.clone());
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

    // A column name that matches nothing is the quietest way for all of this to
    // go wrong: every file lands in its own specimen, no partner is ever found,
    // and the only symptom is one plot where there should be two.
    let reach = use_memo(move || {
        let wanted = rules.read().pairing.sample_id_column.clone();
        let lens = metadata_store.metadata();
        let metadata = lens.read();
        let matched = metadata
            .values()
            .filter(|c| c.contains_key(&wanted))
            .count();
        (matched, metadata.len())
    });

    rsx! {
        div { class: "pairing-columns",
            label { "Sample ID" }
            select {
                value: "{rules.read().pairing.sample_id_column}",
                onchange: move |e| {
                    rules.write().pairing.sample_id_column = Arc::from(e.value().as_str());
                },
                for name in offered(&columns.read(), &rules.read().pairing.sample_id_column) {
                    option { value: "{name}", "{name}" }
                }
            }

            label { "Sample type" }
            select {
                value: "{rules.read().pairing.sample_type_column}",
                onchange: move |e| {
                    rules.write().pairing.sample_type_column = Arc::from(e.value().as_str());
                },
                for name in offered(&columns.read(), &rules.read().pairing.sample_type_column) {
                    option { value: "{name}", "{name}" }
                }
            }

            {
                let (matched, total) = reach();
                let column = rules.read().pairing.sample_id_column.clone();
                if total == 0 {
                    rsx! {
                        span { class: "pairing-columns_note", "No metadata loaded." }
                    }
                } else if matched == total {
                    rsx! {
                        span { class: "pairing-columns_note", "Grouping {total} files by {column}." }
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
