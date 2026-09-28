//! The Review tab: the last rules run's placements as pictures, sorted into
//! what a reviewer works through.
//!
//! The gallery shows one gate on every sample; this shows every placement
//! the run made, each on its own sample, in the piles of
//! [`clingate_core::review::board`]: needs a look, passed, reported, and
//! changed since. A flagged placement is drawn beside the peer that placed
//! the same gate most typically, so what the flag is about can be seen
//! rather than read. From each tile it can be opened in the editor to fix,
//! reported, or judged to look right.
//!
//! The plots are the gallery's: the population above the gate, on the gate's
//! own axes, with the gate as it now stands on that sample. Fixing a gate in
//! the editor and coming back moves its tile to "changed since".

use std::path::PathBuf;
use std::sync::Arc;

use dioxus::prelude::*;
use dioxus::stores::SyncStore;

use clingate_core::axis_store::{AxisStore, AxisStoreStoreExt, Param};
use clingate_core::file_load::FcsFiles;
use clingate_core::gate_rules::autogate::describe;
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::{GateId, NodeId};
use clingate_core::omiq::metadata::MetaDataStoreStoreExt;
use clingate_core::review::RunRecord;
use clingate_core::review::assess::assess;
use clingate_core::review::board::{Board, Entry, LooksRight, Pile, board};
use clingate_core::review::run_record::SampleRef;

use crate::components::toast::{use_toast, warn};
use crate::gate_editor::gallery::cache::PlotCache;
use crate::gate_editor::gallery::plot::{GalleryPlot, Permits};
use crate::gate_editor::review::{ReportTarget, ReviewsChanged, reviews_changed};
use crate::gate_editor::route::Tab;
use crate::gate_editor::workspace_window::{Loaded, MetadataStore};

static GALLERY_STYLE: Asset = asset!("assets/gallery.css");
static REVIEW_STYLE: Asset = asset!("assets/review.css");

/// Tiles on a page.
const PER_PAGE: usize = 12;

/// Where the editor should go: the population a gate is drawn on, the
/// gate's axes, and the sample. Set by "Open in editor"; the editor takes it
/// and clears it.
#[derive(Clone, PartialEq, Debug)]
pub struct EditorFocus {
    pub parent: Arc<str>,
    pub x: Arc<str>,
    pub y: Arc<str>,
    pub sample_name: Arc<str>,
}

/// What the tab reads from the workspace's files: the run, what the reports
/// name, and which flags have been cleared. Read again when a review changes.
#[derive(PartialEq)]
struct Held {
    folder: PathBuf,
    run: RunRecord,
    reported: Vec<(String, String)>,
    looks: LooksRight,
}

/// The run's board, while `tab` is in front - with the gates as they stand.
///
/// Reading the reports means parsing every report file, so that happens only
/// when a review changes; assessing and sorting is cheap and follows the
/// gates. Neither is done behind another tab.
pub fn use_board(tab: Tab) -> (Memo<Option<Arc<Board>>>, Memo<Option<Arc<HeldRun>>>) {
    let loaded = use_context::<Signal<Loaded>>();
    let changed = use_context::<Signal<ReviewsChanged>>();
    let active = use_context::<Signal<Tab>>();
    let gates = use_context::<SyncStore<GateState>>();
    let metadata = use_context::<MetadataStore>();
    let held = use_memo(move || {
        let _ = changed.read();
        if active() != tab {
            return None;
        }
        let folder = loaded.read().folder.clone()?;
        let run = RunRecord::load(&folder).ok()??;
        let reported = clingate_core::review::report::reports_in(&folder)
            .into_iter()
            .map(|(_, r)| (r.gate_id, r.sample.id))
            .collect();
        let looks = LooksRight::load(&folder, &run);
        Some(Arc::new(HeldRun(Held {
            folder,
            run,
            reported,
            looks,
        })))
    });
    let sorted = use_memo(move || {
        let held = held.read().clone()?;
        let state = gates.read();
        let files = metadata.metadata();
        let files = files.read();
        let assessment = assess(&held.0.run, Some((&state, &files)));
        Some(Arc::new(board(
            &held.0.run,
            &assessment,
            &held.0.reported,
            &held.0.looks,
            &state,
            &files,
        )))
    });
    (sorted, held)
}

/// [`Held`], opaque outside this module.
#[derive(PartialEq)]
pub struct HeldRun(Held);

#[component]
pub fn ReviewWindow() -> Element {
    let active = use_context::<Signal<Tab>>();
    let (sorted, held) = use_board(Tab::Review);

    let mut pile = use_signal(|| Pile::NeedsALook);
    let mut gate_filter = use_signal(String::new);
    let mut page = use_signal(|| 0usize);
    let mut plot_size = use_signal(|| 220u32);
    let mut beside_peer = use_signal(|| true);

    let cache = use_signal_sync(PlotCache::default);
    use_context_provider(|| cache);
    use_context_provider(|| Permits::new(4));

    let Some(board) = sorted() else {
        return rsx! {
            document::Stylesheet { href: GALLERY_STYLE }
            document::Stylesheet { href: REVIEW_STYLE }
            div { class: "gallery-empty review-tab_empty",
                "No rules run has been applied in this workspace. Run the rules on the Gate Rules tab; their placements are reviewed here."
            }
        };
    };

    // The gates on the board, for the filter.
    let mut gates: Vec<(String, String)> = Vec::new();
    for e in &board.entries {
        if !gates.iter().any(|(id, _)| *id == e.gate_id) {
            gates.push((
                e.gate_id.clone(),
                describe(&e.gate, e.parent_gate.as_deref()),
            ));
        }
    }
    gates.sort_by(|a, b| clingate_core::gate_rules::rule_store::human_order(&a.1, &b.1));

    // The gate chosen in the filter, or every gate: the piles' counts and
    // their tiles both follow it.
    let only = gate_filter.read().clone();
    let only = (!only.is_empty()).then_some(only);
    let shown: Vec<Entry> = board.pile_for(pile(), only.as_deref()).cloned().collect();
    let pages = shown.len().div_ceil(PER_PAGE).max(1);
    let at_page = page().min(pages - 1);
    let on_page: Vec<Entry> = shown
        .iter()
        .skip(at_page * PER_PAGE)
        .take(PER_PAGE)
        .cloned()
        .collect();
    let applied = crate::gate_editor::review::p_time(&board.run_applied_at);
    let showing = active() == Tab::Review;

    rsx! {
        document::Stylesheet { href: GALLERY_STYLE }
        document::Stylesheet { href: REVIEW_STYLE }
        main { class: "gallery-main review-tab",
            div { class: "gallery-bar",
                div { class: "gallery-heading",
                    span { class: "gallery-gate", "Review" }
                    span { class: "gallery-axes",
                        "The rules run of {applied}: {board.entries.len()} placements"
                    }
                }
                div { class: "review-tab_piles",
                    for p in Pile::ALL {
                        button {
                            key: "{p:?}",
                            class: if pile() == p { "review-tab_pile selected" } else { "review-tab_pile" },
                            onclick: move |_| {
                                pile.set(p);
                                page.set(0);
                            },
                            "{p.title()} ({board.count_for(p, only.as_deref())})"
                        }
                    }
                }
                div { class: "gallery-spacer" }
                label { class: "gallery-size",
                    "Gate"
                    select {
                        value: "{gate_filter}",
                        onchange: move |e| {
                            gate_filter.set(e.value());
                            page.set(0);
                        },
                        option { value: "", "every gate" }
                        for (id , name) in gates.iter() {
                            option { key: "{id}", value: "{id}", "{name}" }
                        }
                    }
                }
                label { class: "gallery-size",
                    input {
                        r#type: "checkbox",
                        checked: beside_peer(),
                        onchange: move |e| beside_peer.set(e.checked()),
                    }
                    "beside a typical peer"
                }
                label { class: "gallery-size",
                    "Size"
                    select {
                        value: "{plot_size}",
                        onchange: move |e| {
                            if let Ok(v) = e.value().parse::<u32>() {
                                plot_size.set(v);
                            }
                        },
                        option { value: "180", "small" }
                        option { value: "220", "medium" }
                        option { value: "300", "large" }
                    }
                }
                div { class: "gallery-pager",
                    button {
                        disabled: at_page == 0,
                        onclick: move |_| page.set(at_page.saturating_sub(1)),
                        "Prev"
                    }
                    span { class: "gallery-page_count",
                        "page {at_page + 1} of {pages} · {shown.len()} shown"
                    }
                    button {
                        disabled: at_page + 1 >= pages,
                        onclick: move |_| page.set(at_page + 1),
                        "Next"
                    }
                }
            }
            p { class: "review-tab_hint", {pile_hint(pile())} }
            if on_page.is_empty() {
                div { class: "gallery-empty", "Nothing here." }
            } else {
                div { class: "gallery-grid",
                    for entry in on_page {
                        ReviewTile {
                            key: "{entry.gate_id}-{entry.sample.id}",
                            entry: entry.clone(),
                            held: held().expect("the board is read from it"),
                            size: plot_size(),
                            beside_peer: beside_peer(),
                            showing,
                        }
                    }
                }
            }
        }
    }
}

fn pile_hint(pile: Pile) -> &'static str {
    match pile {
        Pile::NeedsALook => {
            "Placements that look unlike their peers - the other samples of the same kind the rule placed confidently - or that the rule was unsure of, worst first. Open one in the editor to fix it, report it, or mark it as looking right."
        }
        Pile::Passed => {
            "Placements like their peers', and flagged ones judged to look right. Worth a skim: the flags are a guide, not a guarantee."
        }
        Pile::Reported => {
            "Placements reported as badly placed. Saving records the fix with each report."
        }
        Pile::Changed => {
            "Placements no longer where the rule put them, without a report - moved by hand, undone, or not yet saved. Report one to say what was wrong."
        }
    }
}

/// Where one entry is drawn: the population above its gate, its axes, and
/// the file.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Drawn {
    pub gate_node: NodeId,
    pub parent: Arc<str>,
    pub x: Param,
    pub y: Param,
}

/// Where a tile draws a placement: the population above its gate - the one
/// the gate is drawn on - on the gate's own two parameters. `None` for a gate
/// the document no longer has.
pub(crate) fn where_drawn(
    state: &GateState,
    params: &[Param],
    gate_id: &str,
    parent_gate: Option<&str>,
) -> Option<Drawn> {
    let gate_node = clingate_core::review::report::node_named(state, gate_id, parent_gate)?;
    let parent = state.parent_node(&gate_node)?;
    let id: GateId = Arc::from(gate_id);
    let (x, y) = state.registered_gate(&id)?.get_params();
    let find = |channel: &Arc<str>| {
        params
            .iter()
            .find(|p| p.fluoro == *channel)
            .cloned()
            .unwrap_or(Param {
                marker: channel.clone(),
                fluoro: channel.clone(),
            })
    };
    Some(Drawn {
        gate_node,
        parent: Arc::from(parent.as_str()),
        x: find(&x),
        y: find(&y),
    })
}

/// A sample's file in the workspace, by the name the run recorded.
pub(crate) fn file_of(
    files: &[(Arc<str>, PathBuf)],
    sample: &SampleRef,
) -> Option<(Arc<str>, PathBuf)> {
    let name = sample.name.as_deref()?;
    files.iter().find(|(n, _)| &**n == name).cloned()
}

/// What "Open in editor" sets the editor to: the population, each axis if
/// the scaling has its channel, and the sample's place in the file list.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Focused {
    pub parent: Arc<str>,
    pub x: Option<Param>,
    pub y: Option<Param>,
    pub file: Option<usize>,
}

pub(crate) fn focus_on(focus: &EditorFocus, params: &[Param], files: &[Arc<str>]) -> Focused {
    let param = |channel: &Arc<str>| params.iter().find(|p| p.fluoro == *channel).cloned();
    Focused {
        parent: focus.parent.clone(),
        x: param(&focus.x),
        y: param(&focus.y),
        file: files.iter().position(|n| *n == focus.sample_name),
    }
}

#[component]
fn ReviewTile(
    entry: Entry,
    held: Arc<HeldRun>,
    size: u32,
    beside_peer: bool,
    showing: bool,
) -> Element {
    let gates = use_context::<SyncStore<GateState>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();
    let filehandler = use_context::<Signal<Option<FcsFiles>>>();
    let mut report_target = use_context::<Signal<Option<ReportTarget>>>();
    let mut focus = use_context::<Signal<Option<EditorFocus>>>();
    let mut active = use_context::<Signal<Tab>>();
    let toasts = use_toast();

    let drawn = {
        let params: Vec<Param> = axis_store
            .sorted_settings()
            .read()
            .iter()
            .cloned()
            .collect();
        where_drawn(
            &gates.read(),
            &params,
            &entry.gate_id,
            entry.parent_gate.as_deref(),
        )
    };
    let files: Vec<(Arc<str>, PathBuf)> = filehandler
        .read()
        .as_ref()
        .map(|f| {
            f.file_list()
                .iter()
                .map(|s| (s.name.clone(), s.get_filepath().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let path_of = |sample: &SampleRef| file_of(&files, sample);
    let mine = path_of(&entry.sample);
    let peer = entry
        .flag
        .as_ref()
        .and_then(|f| f.typical_peer.clone())
        .filter(|_| beside_peer);
    let peer_file = peer.as_ref().and_then(|p| path_of(p));

    let label = |s: &SampleRef| {
        s.name
            .clone()
            .unwrap_or_else(|| s.id.clone())
            .trim_end_matches(".fcs")
            .to_string()
    };
    let title = label(&entry.sample);
    let gate = describe(&entry.gate, entry.parent_gate.as_deref());
    let confidence = match entry.confidence {
        Some(c) => format!("confidence {c:.2}"),
        None => "already met its rule".to_string(),
    };
    let severity_class = match entry.flag.as_ref().map(|f| f.severity) {
        Some(s) if s >= 5.0 => "review-tile review-tile_severe",
        Some(_) if entry.pile == Pile::NeedsALook => "review-tile review-tile_notable",
        _ => "review-tile",
    };

    let open_in_editor = {
        let drawn = drawn.clone();
        let mine = mine.clone();
        move |_| {
            let (Some(drawn), Some((name, _))) = (drawn.clone(), mine.clone()) else {
                warn(&toasts, "That gate or sample is no longer in the workspace");
                return;
            };
            focus.set(Some(EditorFocus {
                parent: drawn.parent.clone(),
                x: drawn.x.fluoro.clone(),
                y: drawn.y.fluoro.clone(),
                sample_name: name,
            }));
            active.set(Tab::Editor);
        }
    };
    let report = {
        let drawn = drawn.clone();
        let entry = entry.clone();
        let title = title.clone();
        move |_| {
            let Some(drawn) = drawn.clone() else {
                warn(&toasts, "That gate is no longer in the document");
                return;
            };
            report_target.set(Some(ReportTarget {
                node: drawn.gate_node.clone(),
                sample: Arc::from(entry.sample.id.as_str()),
                gate: entry.gate.clone(),
                sample_name: title.clone(),
                choices: Vec::new(),
            }));
        }
    };
    let looks_right = {
        let held = held.clone();
        let entry = entry.clone();
        move |mark: bool| {
            let held = &held.0;
            match LooksRight::set(
                &held.folder,
                &held.run,
                &entry.gate_id,
                &entry.sample.id,
                mark,
            ) {
                Ok(_) => reviews_changed(),
                Err(e) => warn(&toasts, format!("Could not keep that: {e}")),
            }
        }
    };
    let mark = looks_right.clone();
    let unmark = looks_right;

    rsx! {
        div { class: "{severity_class}",
            div { class: "review-tile_head",
                span { class: "review-tile_sample", title: "{title}", "{title}" }
                span { class: "review-tile_gate", "{gate} · {confidence}" }
            }
            if let Some(flag) = entry.flag.as_ref() {
                ul { class: "review-tile_reasons",
                    for (at , reason) in flag.reasons.iter().enumerate() {
                        li { key: "{at}", "{reason.says}" }
                    }
                }
            }
            if entry.looks_right {
                div { class: "review-tile_note", "Flagged, and judged to look right." }
            }
            if entry.reports > 0 {
                div { class: "review-tile_note", "Reported ({entry.reports})." }
            }
            if entry.pile == Pile::Changed {
                div { class: "review-tile_note",
                    "No longer where the rule put it - moved, undone or not yet saved."
                }
            }
            div { class: "review-tile_plots",
                match (drawn.clone(), mine.clone()) {
                    (Some(drawn), Some((name, path))) => rsx! {
                        div { class: "gallery-plot",
                            div { class: "gallery-plot_name", "this sample" }
                            if showing {
                                GalleryPlot {
                                    name,
                                    path,
                                    node: drawn.parent.clone(),
                                    x: drawn.x.clone(),
                                    y: drawn.y.clone(),
                                    size,
                                }
                            }
                        }
                        if let (Some(peer), Some((peer_name, peer_path))) = (peer.as_ref(), peer_file.clone()) {
                            div { class: "gallery-plot",
                                div { class: "gallery-plot_name", title: "{label(peer)}",
                                    "a typical peer: {label(peer)}"
                                }
                                if showing {
                                    GalleryPlot {
                                        name: peer_name,
                                        path: peer_path,
                                        node: drawn.parent.clone(),
                                        x: drawn.x.clone(),
                                        y: drawn.y.clone(),
                                        size,
                                    }
                                }
                            }
                        }
                    },
                    _ => rsx! {
                        div { class: "gallery-plot gallery-plot_empty",
                            span { "The gate or the sample is no longer in the workspace." }
                        }
                    },
                }
            }
            div { class: "review-tile_actions",
                button {
                    disabled: drawn.is_none() || mine.is_none(),
                    title: "Show this sample and gate in the gate editor, to reposition it",
                    onclick: open_in_editor,
                    "Open in editor"
                }
                if entry.pile == Pile::NeedsALook {
                    button {
                        title: "The flag was wrong: move this to Passed. Recorded with the review.",
                        onclick: move |_| mark(true),
                        "Looks right"
                    }
                }
                if entry.looks_right {
                    button {
                        title: "Put it back under Needs a look",
                        onclick: move |_| unmark(false),
                        "Undo looks right"
                    }
                }
                button {
                    class: "review-report_button",
                    disabled: drawn.is_none(),
                    onclick: report,
                    "Report..."
                }
            }
        }
    }
}
