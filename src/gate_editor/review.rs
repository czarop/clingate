//! Reviewing a rules run in the app: reporting a gate the rules placed badly,
//! and marking a run reviewed. Everything decided here is decided in
//! [`clingate_core::review`], which the tools for Claude call as well; this
//! file is the dialog and the buttons.

use std::path::PathBuf;
use std::sync::Arc;

use clingate_core::gates::gate_store::{FileId, GateStateStoreExt, NodeId};
use clingate_core::omiq::metadata::MetaDataStoreStoreExt;
use clingate_core::review::report::Decision;
use clingate_core::review::{Problem, ReportRequest, RunRecord};
use dioxus::prelude::*;

use crate::components::toast::{say, use_toast, warn};
use crate::gate_editor::gate_rules_window::RulesRun;
use crate::gate_editor::path_picker::{Pick, PickPath};
use crate::gate_editor::workspace_window::{GateStore, Loaded, MetadataStore};

/// Something in the workspace's reviews changed - a run applied, a report
/// made, a run marked reviewed - so what shows them should read them again.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct ReviewsChanged(pub u64);

/// Tell whatever shows the reviews to read them again.
pub fn reviews_changed() {
    if let Some(mut changed) = try_consume_context::<Signal<ReviewsChanged>>() {
        changed.write().0 += 1;
    }
}

/// The gate and sample a report is about.
#[derive(Clone, PartialEq, Debug)]
pub struct ReportTarget {
    pub node: NodeId,
    pub sample: FileId,
    /// For the dialog's heading.
    pub gate: String,
    pub sample_name: String,
}

/// What a plot's Report button reports: the gate selected on it, at its
/// place under the plot's population, on the plot's sample.
pub fn selected_target(
    state: &clingate_core::gates::GateState,
    selected: Option<&clingate_core::gates::gate_store::GateId>,
    parent: Option<&NodeId>,
    sample: Option<FileId>,
    sample_name: &str,
) -> Option<ReportTarget> {
    let gate_id = selected?;
    let node = clingate_core::review::report::node_under(state, gate_id, parent)?;
    let gate = state.registered_gate(gate_id)?.get_name().to_string();
    Some(ReportTarget {
        node,
        sample: sample?,
        gate,
        sample_name: sample_name.trim_end_matches(".fcs").to_string(),
    })
}

/// Report the gate selected on a plot, on that plot's sample. Disabled with
/// no gate selected, or none that can be found under the plot's population.
#[component]
pub fn ReportButton(sample_name: Arc<str>, parental_gate: ReadSignal<Option<Arc<str>>>) -> Element {
    let gates = use_context::<GateStore>();
    let metadata = use_context::<MetadataStore>();
    let mut open = use_context::<Signal<Option<ReportTarget>>>();
    let target = {
        let selected = gates.selected_gate().read().clone();
        let sample = metadata
            .file_name_to_gating_id()
            .read()
            .get(&sample_name)
            .cloned();
        let parent = parental_gate().map(NodeId::from);
        selected_target(
            &gates.read(),
            selected.as_ref(),
            parent.as_ref(),
            sample,
            &sample_name,
        )
    };
    let enabled = target.is_some();
    rsx! {
        button {
            class: "review-report_button",
            disabled: !enabled,
            title: if enabled { "Report the selected gate as badly placed on this sample" } else { "Select a gate on the plot to report it" },
            onclick: move |_| open.set(target.clone()),
            "Report..."
        }
    }
}

/// The report dialog, shown while a [`ReportTarget`] is set. Mounted once in
/// the gate editor; the gallery's tiles open it too.
#[component]
pub fn ReportDialog() -> Element {
    let mut open = use_context::<Signal<Option<ReportTarget>>>();
    let loaded = use_context::<Signal<Loaded>>();
    let gates = use_context::<GateStore>();
    let run_with = RulesRun::from_context();
    let toasts = use_toast();
    let mut problem = use_signal(|| None::<Problem>);
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let Some(target) = open() else {
        return rsx! {};
    };

    // What the rules did for this gate on this sample, from the run kept in
    // the workspace - read when the dialog opens.
    let did = loaded
        .peek()
        .folder
        .as_deref()
        .and_then(|folder| RunRecord::load(folder).ok().flatten())
        .map(|run| {
            let gate_id = gates
                .peek()
                .gate_for_node(&target.node)
                .map(|g| g.to_string())
                .unwrap_or_default();
            match run
                .placed
                .iter()
                .find(|p| p.gate_id == gate_id && p.sample.id == *target.sample)
            {
                Some(p) => format!(
                    "The rules run of {} moved this gate from {} to {}, with confidence {:.2} - weakest: {}.",
                    p_time(&run.applied_at),
                    p.from.map_or("-".into(), |v| format!("{v:.3}")),
                    p.to.map_or("-".into(), |v| format!("{v:.3}")),
                    p.confidence,
                    p.weakest.as_deref().unwrap_or("-"),
                ),
                None if run
                    .kept
                    .iter()
                    .any(|k| k.gate_id == gate_id && k.sample.id == *target.sample) =>
                {
                    "The last rules run left this gate where it was.".to_string()
                }
                None => "The last rules run did not place this gate for this sample.".to_string(),
            }
        })
        .unwrap_or_else(|| "No rules run has been applied in this workspace.".to_string());

    let submit = {
        let target = target.clone();
        move |_| {
            let Some(chosen) = problem() else {
                return;
            };
            let request = ReportRequest {
                node: target.node.clone(),
                sample: target.sample.clone(),
                problem: chosen,
                note: note(),
            };
            let Some(job) = run_with.report_job(request) else {
                warn(
                    &toasts,
                    "Open a workspace folder first: reports are kept in it",
                );
                return;
            };
            busy.set(true);
            spawn(async move {
                let done = tokio::task::spawn_blocking(job).await;
                busy.set(false);
                match done {
                    Ok(Ok((path, _))) => {
                        say(
                            &toasts,
                            format!(
                                "Reported - kept in {}. Fix the gate and Save: the fix is recorded with it.",
                                path.parent()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default()
                            ),
                        );
                        open.set(None);
                        problem.set(None);
                        note.set(String::new());
                        reviews_changed();
                    }
                    Ok(Err(e)) => warn(&toasts, format!("Not reported: {e}")),
                    Err(e) => warn(&toasts, format!("Not reported: {e}")),
                }
            });
        }
    };

    rsx! {
        document::Stylesheet { href: asset!("/assets/review.css") }
        div { class: "review-dialog_backdrop",
            div { class: "review-dialog",
                h3 { "Report a badly placed gate" }
                p { class: "review-dialog_what",
                    strong { "{target.gate}" }
                    " on "
                    strong { "{target.sample_name}" }
                }
                p { class: "review-dialog_did", "{did}" }
                fieldset { class: "review-dialog_problems",
                    legend { "What is wrong with it?" }
                    for p in Problem::ALL {
                        label { key: "{p.key()}",
                            input {
                                r#type: "radio",
                                name: "review-problem",
                                checked: problem() == Some(p),
                                onchange: move |_| problem.set(Some(p)),
                            }
                            " {p.describe()}"
                        }
                    }
                }
                label { class: "review-dialog_note",
                    "Note (optional)"
                    textarea {
                        value: "{note}",
                        rows: 3,
                        oninput: move |e| note.set(e.value()),
                    }
                }
                p { class: "review-dialog_hint",
                    "The report keeps what the rule did, how sure it was and why, and the population on this sample and the one the rule read - with a sample of its events - so the confidence scores can be improved. Fix the gate afterwards; Save records the fix with the report."
                }
                div { class: "review-dialog_buttons",
                    button {
                        disabled: problem().is_none() || busy(),
                        onclick: submit,
                        if busy() { "Reading the files..." } else { "Report" }
                    }
                    button {
                        disabled: busy(),
                        onclick: move |_| {
                            open.set(None);
                            problem.set(None);
                            note.set(String::new());
                        },
                        "Cancel"
                    }
                }
            }
        }
    }
}

/// "28 Sep 2026, 12:03" from a record's timestamp.
pub(crate) fn p_time(stamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(stamp)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| stamp.to_string())
}

/// The Gate Rules tab's review section: the last applied run, its reports,
/// the review library, and marking the run reviewed.
#[component]
pub fn ReviewPanel() -> Element {
    let loaded = use_context::<Signal<Loaded>>();
    let changed = use_context::<Signal<ReviewsChanged>>();
    let run_with = RulesRun::from_context();
    let toasts = use_toast();
    let mut library = use_signal(|| {
        clingate_core::review::library::configured()
            .map_or(String::new(), |p| p.display().to_string())
    });

    // Read again whenever the reviews change or another workspace opens.
    let state = use_memo(move || {
        let _ = changed.read();
        let folder = loaded.read().folder.clone()?;
        let run = RunRecord::load(&folder);
        // This run's reports, and whether this run - not an earlier one -
        // has been marked reviewed.
        let (reports, reviewed) = match &run {
            Ok(Some(run)) => (
                clingate_core::review::report::reports_of_run(&folder, &run.applied_at),
                clingate_core::review::report::is_reviewed(&folder, &run.applied_at),
            ),
            _ => (Vec::new(), false),
        };
        Some((run.map_err(|e| e.to_string()), reports, reviewed))
    });

    let summary = match &*state.read() {
        None => "Open a workspace folder to review its rules runs.".to_string(),
        Some((Err(e), _, _)) => format!("The kept run could not be read: {e}"),
        Some((Ok(None), _, _)) => {
            "No rules run has been applied in this workspace yet.".to_string()
        }
        Some((Ok(Some(run)), reports, reviewed)) => {
            let placed_reports = reports
                .iter()
                .filter(|(_, r)| matches!(r.decision, Decision::Placed(_)))
                .count();
            format!(
                "Last run applied {}: {} gates moved, {} left alone, {} could not be placed. {} report{} ({} on gates the run moved).{}",
                p_time(&run.applied_at),
                run.placed.len(),
                run.kept.len(),
                run.skipped.len(),
                reports.len(),
                if reports.len() == 1 { "" } else { "s" },
                placed_reports,
                if *reviewed { " Marked reviewed." } else { "" },
            )
        }
    };
    let can_mark = matches!(&*state.read(), Some((Ok(Some(_)), _, _)));

    // What needs a look, counted as the Review tab sorts it.
    let (sorted, _) =
        crate::gate_editor::review_window::use_board(crate::gate_editor::route::Tab::Rules);
    let mut active = use_context::<Signal<crate::gate_editor::route::Tab>>();

    rsx! {
        document::Stylesheet { href: asset!("/assets/review.css") }
        fieldset { class: "gate_rules-form",
            legend { "Review" }
            label { "Last run" }
            p { class: "gate_rules-hint", "{summary}" }
            label { "" }
            p { class: "gate_rules-hint",
                "Report a gate the rules placed badly from the gate editor (Report... above each plot) or from a tile on the Review tab. When the run has been checked, mark it reviewed: every placement not reported and still where the rule put it is recorded as accepted, which is what the confidence scores are measured against."
            }
            if let Some(board) = sorted() {
                label { "Needs a look" }
                div { class: "review-flags",
                    p { class: "gate_rules-hint",
                        {
                            use clingate_core::review::board::Pile;
                            format!(
                                "{} need a look, {} passed, {} reported, {} changed since. Work through them on the Review tab, where each is drawn beside the file its rule read and, when flagged, a typical peer, and can be opened in the editor.",
                                board.count(Pile::NeedsALook),
                                board.count(Pile::Passed),
                                board.count(Pile::Reported),
                                board.count(Pile::Changed),
                            )
                        }
                    }
                    div {
                        button {
                            onclick: move |_| active.set(crate::gate_editor::route::Tab::Review),
                            "Open the Review tab"
                        }
                    }
                }
            }
            label { "Review library" }
            div { class: "gate_rules-path",
                input {
                    value: "{library}",
                    placeholder: "A folder reviews from every run are copied into - a shared drive, say",
                    oninput: move |e| library.set(e.value()),
                }
                PickPath { path: library, mode: Pick::Folder, label: "Review library" }
                button {
                    onclick: move |_| {
                        let chosen = library().trim().to_string();
                        let setting = (!chosen.is_empty()).then(|| PathBuf::from(&chosen));
                        match clingate_core::review::library::set(setting.clone()) {
                            Ok(()) => say(
                                &toasts,
                                match setting {
                                    Some(p) => format!("Reviews will be copied into {}", p.display()),
                                    None => "No review library: reviews stay in each run's folder".to_string(),
                                },
                            ),
                            Err(e) => warn(&toasts, format!("Not set: {e}")),
                        }
                    },
                    "Set"
                }
            }
            label { "" }
            div { class: "gate_rules-actions_row",
                button {
                    disabled: !can_mark,
                    onclick: move |_| {
                        let library = clingate_core::review::library::configured();
                        match run_with.mark_reviewed(library.as_deref()) {
                            Ok((review, copied)) => {
                                say(
                                    &toasts,
                                    format!(
                                        "Reviewed: {} accepted, {} reported, {} moved without a report{}",
                                        review.accepted(),
                                        review.reported(),
                                        review.moved_unreported(),
                                        match copied {
                                            Some(p) => format!(" - copied to {}", p.display()),
                                            None => " - no review library set, so kept in this folder only".to_string(),
                                        }
                                    ),
                                );
                                reviews_changed();
                            }
                            Err(e) => warn(&toasts, format!("Not marked: {e}")),
                        }
                    },
                    "Mark run reviewed"
                }
            }
        }
    }
}
