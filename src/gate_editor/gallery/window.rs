//! The gallery: one gate, every sample in the run, as pictures.
//!
//! The editor answers "is this gate right on this sample?" The gallery answers
//! "is this gate right?", which is a different question and the one that
//! matters after an autogating run - a rule that works on the reference and
//! drifts on a third of the cohort looks perfect one sample at a time.
//!
//! Three things make that work:
//!
//! - **The same order as everywhere else.** Specimens come from `pair_files`,
//!   so the sort column chosen on the editor tab decides the order here too,
//!   and the FMX is always the left plot of a pair.
//! - **Pictures, not editors.** Every plot is a rendered bitmap with a static
//!   outline over it. Nothing here can move a gate, which is what makes it safe
//!   to put twenty of them on a page.
//! - **A page at a time.** Only the mounted page renders. Paging back is free,
//!   because the images are cached against the gates that produced them.

use std::path::PathBuf;
use std::sync::Arc;

use dioxus::prelude::*;
use dioxus::stores::SyncStore;

use crate::file_load::FcsFiles;
use crate::gate_editor::gate_sidebar::GateSidebar;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{NodeId, ROOTGATE};
use crate::gate_editor::plots::axis_store::{AxisStore, AxisStoreStoreExt, Param};
use crate::gate_editor::plots::sample_pairs::{Pair, pair_files};
use crate::gate_editor::route::Tab;
use crate::gate_editor::workspace_window::Generation;
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};

use super::cache::PlotCache;
use super::plot::{GalleryPlot, Permits};

static GALLERY_STYLE: Asset = asset!("assets/gallery.css");

/// How many specimens a page holds. Twenty plots: enough that a drift shows up
/// as a pattern rather than as a run of individually plausible pictures.
const PER_PAGE: usize = 10;

/// One specimen's place on a page.
#[derive(Clone, PartialEq)]
pub struct Card {
    pub title: String,
    /// One entry per display slot - FMX then FS - where the specimen has that
    /// file. `None` leaves the slot empty rather than sliding the other plot
    /// across, the same rule the editor follows.
    pub slots: Vec<Option<Slot>>,
}

/// One file's place in a card.
///
/// A struct rather than a `(name, path)` pair, because there are two names in
/// play and mixing them up fails silently. `name` is the file's name in the
/// program, which is what the metadata is searched for; `label` is the same
/// without its extension, for reading. The path is only ever used to open the
/// file - never to work out its name, which for a file from a sub-folder is
/// not the name on disk.
#[derive(Clone, PartialEq)]
pub struct Slot {
    pub name: Arc<str>,
    pub path: PathBuf,
}

impl Slot {
    pub fn label(&self) -> &str {
        self.name.trim_end_matches(".fcs")
    }
}

#[component]
pub fn GalleryWindow() -> Element {
    let gate_store = use_context::<SyncStore<GateState>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();
    let filehandler = use_context::<Signal<Option<FcsFiles>>>();
    let rules = use_context::<Signal<RuleStore>>();
    let active = use_context::<Signal<Tab>>();

    // This tab's own selection. Sharing the editor's would mean looking at a
    // gate here moved the editor's plots out from under the person, and would
    // make the back button between tabs mean something different each time.
    let mut selected_node: Signal<Option<Arc<str>>> = use_signal(|| Some(ROOTGATE.clone()));
    let x_axis_marker: Signal<Param> = use_signal(|| {
        let p: Arc<str> = Arc::from("FSC-A");
        Param {
            marker: p.clone(),
            fluoro: p,
        }
    });
    let y_axis_marker: Signal<Param> = use_signal(|| {
        let p: Arc<str> = Arc::from("SSC-A");
        Param {
            marker: p.clone(),
            fluoro: p,
        }
    });

    let mut page = use_signal(|| 0usize);
    let mut plot_size = use_signal(|| 260u32);

    // What names the old workspace. The selected gate is a node id from a
    // document that has gone, and a page number from a longer list may now be
    // past the end of it. The picture cache needs nothing: its key holds every
    // cofactor and the identity of every gate a picture depends on, and both
    // change when the workspace does.
    let generation = use_context::<Signal<Generation>>();
    let document = use_memo(move || generation.read().document);
    let file_list = use_memo(move || generation.read().files);
    use_effect(move || {
        document();
        selected_node.set(Some(ROOTGATE.clone()));
        page.set(0);
    });
    use_effect(move || {
        file_list();
        page.set(0);
    });

    // Shared by every plot on the page. The cache holds the pictures; the
    // permits stop twenty files being opened at once, which is a memory spike
    // rather than a speed-up - the work is mostly one core each anyway.
    let cache = use_signal_sync(PlotCache::default);
    use_context_provider(|| cache);
    use_context_provider(|| Permits::new(4));

    let pairs = use_memo(move || {
        let Some(files) = filehandler.read().as_ref().map(|f| f.file_list().to_vec()) else {
            return Vec::<Pair>::new();
        };
        // Each file's name in the program - what the metadata is searched for.
        let keys: Vec<Arc<str>> = files.iter().map(|f| f.name.clone()).collect();
        pair_files(
            &keys,
            &metadata_store.file_name_to_gating_id().read(),
            &metadata_store.metadata().read(),
            &rules.read().pairing,
        )
    });

    // The specimen's own name, from the sample id column, rather than the
    // OmiqID - the same choice the report makes, and for the same reason: an id
    // tells you which row it is, a name tells you which donor.
    let cards = use_memo(move || {
        let Some(files) = filehandler.read().as_ref().map(|f| f.file_list().to_vec()) else {
            return Vec::<Card>::new();
        };
        pairs
            .read()
            .iter()
            .map(|pair| {
                let slots = pair
                    .slots
                    .iter()
                    .take(2)
                    .map(|slot| {
                        slot.and_then(|at| files.get(at)).map(|stub| Slot {
                            name: stub.name.clone(),
                            path: stub.get_filepath().to_owned(),
                        })
                    })
                    .collect::<Vec<_>>();
                Card {
                    title: pair
                        .specimen
                        .as_ref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "unnamed specimen".to_string()),
                    slots,
                }
            })
            .collect()
    });

    let pages = use_memo(move || cards.read().len().div_ceil(PER_PAGE).max(1));

    // A selection change can leave the page past the end - a shorter run, or a
    // pairing column that groups differently. Clamping on read rather than
    // writing back avoids a signal write inside a render.
    let showing = use_memo(move || page().min(pages().saturating_sub(1)));

    // The selection, dropped if the document it named has been replaced.
    //
    // Loading a gating file on the editor tab swaps the whole tree, and a node
    // id from the old one answers to nothing in the new: the plots would filter
    // through an empty chain while the heading still named a gate that is gone.
    // Falling back to the root says what is actually being shown.
    let showing_node = use_memo(move || {
        let node = selected_node()?;
        if *node == **ROOTGATE {
            return Some(node);
        }
        let held = gate_store.read();
        held.gate_for_node(&NodeId::from(node.clone()))
            .is_some()
            .then_some(node)
    });

    // What the person is looking at, named rather than left as a node id.
    let gate_name = use_memo(move || {
        let Some(node) = showing_node() else {
            return "all events".to_string();
        };
        if *node == **ROOTGATE {
            return "all events".to_string();
        }
        gate_store
            .read()
            .gate_for_node(&NodeId::from(node.clone()))
            .and_then(|id| gate_store.read().registered_gate(id))
            .map(|g| g.get_name().to_string())
            .unwrap_or_else(|| node.to_string())
    });

    let x_axis = use_memo(move || {
        let param = x_axis_marker.read().fluoro.clone();
        axis_store
            .settings()
            .read()
            .get(&param)
            .cloned()
            .unwrap_or_default()
    });
    let y_axis = use_memo(move || {
        let param = y_axis_marker.read().fluoro.clone();
        axis_store
            .settings()
            .read()
            .get(&param)
            .cloned()
            .unwrap_or_default()
    });

    let first = showing() * PER_PAGE;
    let on_page: Vec<(usize, Card)> = cards
        .read()
        .iter()
        .enumerate()
        .skip(first)
        .take(PER_PAGE)
        .map(|(at, card)| (at, card.clone()))
        .collect();
    let total = cards.read().len();

    rsx! {
        document::Stylesheet { href: GALLERY_STYLE }
        div { class: "gallery-layout",

            GateSidebar {
                selected_id: selected_node,
                x_axis_param: x_axis_marker,
                y_axis_param: y_axis_marker,
            }

            main { class: "gallery-main",
                div { class: "gallery-bar",
                    div { class: "gallery-heading",
                        span { class: "gallery-gate", "{gate_name}" }
                        span { class: "gallery-axes",
                            "{x_axis.read().param} / {y_axis.read().param}"
                        }
                    }
                    div { class: "gallery-spacer" }
                    label { class: "gallery-size",
                        "Size"
                        select {
                            value: "{plot_size}",
                            onchange: move |e| {
                                if let Ok(v) = e.value().parse::<u32>() {
                                    plot_size.set(v);
                                }
                            },
                            option { value: "200", "small" }
                            option { value: "260", "medium" }
                            option { value: "340", "large" }
                        }
                    }
                    super::export::ExportPdf {
                        cards: cards(),
                        node: showing_node().unwrap_or_else(|| ROOTGATE.clone()),
                        x: x_axis_marker(),
                        y: y_axis_marker(),
                        gate_name: gate_name(),
                    }
                    div { class: "gallery-pager",
                        button {
                            disabled: showing() == 0,
                            onclick: move |_| page.set(showing().saturating_sub(1)),
                            "Prev"
                        }
                        span { class: "gallery-page_count",
                            "page {showing() + 1} of {pages()} · {total} specimens"
                        }
                        button {
                            disabled: showing() + 1 >= pages(),
                            onclick: move |_| page.set(showing() + 1),
                            "Next"
                        }
                    }
                }

                if total == 0 {
                    div { class: "gallery-empty",
                        "No samples loaded. Open a folder of FCS files on the editor tab."
                    }
                } else {
                    div { class: "gallery-grid",
                        for (at , card) in on_page {
                            div { class: "gallery-card", key: "{at}-{card.title}",
                                div { class: "gallery-card_title", title: "{card.title}", "{card.title}" }
                                div { class: "gallery-card_plots",
                                    for (slot , filled) in card.slots.iter().enumerate() {
                                        match filled {
                                            Some(file) => rsx! {
                                                div { class: "gallery-plot", key: "{slot}",
                                                    div { class: "gallery-plot_name", title: "{file.name}", "{file.label()}" }
                                                    // Only the page in front renders. A hidden
                                                    // tab is still mounted - that is how the
                                                    // shell keeps state - and twenty plots
                                                    // redrawing behind the editor on every gate
                                                    // edit is exactly the cost the shell's own
                                                    // comment warns about.
                                                    if active() == Tab::Gallery {
                                                        GalleryPlot {
                                                            name: file.name.clone(),
                                                            path: file.path.clone(),
                                                            node: showing_node().unwrap_or_else(|| ROOTGATE.clone()),
                                                            x: x_axis_marker(),
                                                            y: y_axis_marker(),
                                                            size: plot_size(),
                                                        }
                                                    }
                                                }
                                            },
                                            None => rsx! {
                                                div { class: "gallery-plot gallery-plot_empty", key: "{slot}",
                                                    div { class: "gallery-plot_name", "" }
                                                    span { "no paired file" }
                                                }
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
