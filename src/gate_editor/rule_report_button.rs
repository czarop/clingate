//! The Review tab's report on the rule of the gate shown: every sample the
//! last run placed it on, with its plot, kept in the workspace for the user
//! to send on - see [`clingate_core::rule_report`]. The tools for Claude make
//! the same report with `rule_report`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::prelude::*;

use clingate_core::rule_report::{RuleReportJob, keep, rule_report};
use clingate_core::test_report::stamped_now;

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::gate_rules_window::{RulesRun, use_stop_run_on_change};
use crate::gate_editor::workspace_window::{GateStore, Loaded};

/// What a rule's report reads, from the app's own state.
#[derive(Clone, Copy)]
pub(crate) struct RuleReportRun {
    run_with: RulesRun,
    gates: GateStore,
    loaded: Signal<Loaded>,
}

impl RuleReportRun {
    pub(crate) fn from_context() -> Self {
        Self {
            run_with: RulesRun::from_context(),
            gates: use_context(),
            loaded: use_context(),
        }
    }

    /// The workspace as it stands now - the gating as drawn, saved or not -
    /// for a report on the gate `gate_id`, made `made`.
    pub(crate) fn job(&self, made: String, gate_id: &str) -> Result<RuleReportJob, String> {
        let folder = self
            .loaded
            .peek()
            .folder
            .clone()
            .ok_or("Open a workspace folder first - the report is kept in it")?;
        let (inputs, axes) = self.run_with.inputs_now();
        Ok(RuleReportJob {
            workspace: folder.file_name().map_or_else(
                || folder.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            made,
            folder,
            gates: self.gates.peek().clone(),
            inputs,
            axes,
            gate_id: gate_id.to_string(),
        })
    }
}

/// The button that reports on the rule of `gate_id`, and the report last made.
#[component]
pub fn RuleReportButton(gate_id: String) -> Element {
    let run = RuleReportRun::from_context();
    let toasts = use_toast();
    let mut progress = use_signal(|| None::<(usize, usize)>);
    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
    let mut made = use_signal(|| None::<(PathBuf, String)>);
    use_stop_run_on_change(cancel);

    let start = move |_| {
        let gate_id = gate_id.clone();
        spawn(async move {
            if progress.peek().is_some() {
                return;
            }
            let (when, stamp) = stamped_now();
            let job = match run.job(when, &gate_id) {
                Ok(job) => job,
                Err(why) => return warn(&toasts, why),
            };
            progress.set(Some((0, 0)));
            let flag = Arc::new(AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(usize, usize)>();
            let worker = tokio::task::spawn_blocking(move || {
                let report = rule_report(&job, &flag, |done, total| {
                    let _ = tx.send((done, total));
                })?;
                keep(&job.folder, &report, &stamp)
                    .map(|path| (path, report.markdown))
                    .map_err(|e| format!("The report could not be kept: {e}"))
            });
            while let Some(step) = rx.recv().await {
                progress.set(Some(step));
            }
            let ended = worker.await;
            let stopped = cancel
                .peek()
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Relaxed));
            progress.set(None);
            cancel.set(None);
            match ended {
                Ok(Ok((path, report))) => {
                    say(&toasts, format!("Rule report kept at {}", path.display()));
                    made.set(Some((path, report)));
                }
                Ok(Err(_)) if stopped => note(&toasts, "Stopped - no report was kept"),
                Ok(Err(why)) => warn(&toasts, why),
                Err(e) => warn(&toasts, format!("The report did not finish: {e}")),
            }
        });
    };

    rsx! {
        div { class: "review-tab_report",
            button {
                class: "review-tab_pile",
                disabled: progress().is_some(),
                title: "Every sample the last run placed this gate on - the numbers, why each was flagged, and each one's plot, flagged and not - in one report kept in the workspace's test_reports folder, to send on.",
                onclick: start,
                if progress().is_some() { "Making the report..." } else { "Report on this rule" }
            }
            if let Some((done, total)) = progress() {
                span { "Drawing {done} of {total}" }
                button {
                    class: "review-tab_pile",
                    onclick: move |_| {
                        if let Some(flag) = cancel() {
                            flag.store(true, Ordering::Relaxed);
                            note(&toasts, "Stopping...");
                        }
                    },
                    "Stop"
                }
            }
            if let Some((path, report)) = made() {
                span { "Kept with its pictures at {path.display()}" }
                button {
                    class: "review-tab_pile",
                    onclick: move |_| match crate::clipboard::copy_text(&report) {
                        Ok(()) => say(&toasts, "The report is on the clipboard"),
                        Err(e) => warn(&toasts, format!("The report could not be copied: {e}")),
                    },
                    "Copy the report"
                }
            }
        }
    }
}
