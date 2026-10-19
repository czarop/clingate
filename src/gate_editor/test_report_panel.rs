//! The Gate Rules tab's test report: the pick made for every gate and scored
//! sample by sample, kept in the workspace for the user to send on - see
//! [`clingate_core::test_report`]. The tools for Claude make the same report
//! with `test_report`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::prelude::*;

use clingate_core::gate_rules::pick::kept_settings;
use clingate_core::test_report::{TestJob, TestStep, keep, stamped_now, test_report};

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::gate_rules_window::{RulesRun, use_stop_run_on_change};
use crate::gate_editor::pick_panel::Typed;
use crate::gate_editor::workspace_window::{GateStore, Loaded};

/// What a test report reads, from the app's own state.
#[derive(Clone, Copy)]
pub(crate) struct TestReportRun {
    run_with: RulesRun,
    gates: GateStore,
    loaded: Signal<Loaded>,
}

impl TestReportRun {
    pub(crate) fn from_context() -> Self {
        Self {
            run_with: RulesRun::from_context(),
            gates: use_context(),
            loaded: use_context(),
        }
    }

    /// The workspace as it stands now - the gating as drawn, saved or not,
    /// and the pick settings as `typed`, the kept ones filling in the rest -
    /// made `made`, with the folder the report is kept in.
    pub(crate) fn job(&self, made: String, typed: &Typed) -> Result<(PathBuf, TestJob), String> {
        let folder = self
            .loaded
            .peek()
            .folder
            .clone()
            .ok_or("Open a workspace folder first - the report is kept in it")?;
        let (inputs, _) = self.run_with.inputs_now();
        if inputs.files.is_empty() {
            return Err("No FCS files are loaded - open a workspace on the first tab".into());
        }
        let settings = typed.settings(&kept_settings(&folder))?;
        let job = TestJob {
            workspace: clingate_core::workspace::name_of(&folder),
            made,
            gates: self.gates.peek().clone(),
            settings,
            inputs,
        };
        Ok((folder, job))
    }
}

/// The button that makes a test report with the pick settings `typed`, and
/// the report last made.
#[component]
pub(crate) fn TestReportPanel(typed: Signal<Typed>) -> Element {
    let run = TestReportRun::from_context();
    let toasts = use_toast();
    let mut progress = use_signal(|| None::<TestStep>);
    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
    let mut made = use_signal(|| None::<(PathBuf, String)>);
    use_stop_run_on_change(cancel);

    let start = move |_| {
        spawn(async move {
            if progress.peek().is_some() {
                return;
            }
            let (when, stamp) = stamped_now();
            let (folder, job) = match run.job(when, &typed.peek()) {
                Ok(ready) => ready,
                Err(why) => return warn(&toasts, why),
            };
            progress.set(Some(TestStep::CheckingSpillover { done: 0, total: 0 }));
            let flag = Arc::new(AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TestStep>();
            let worker = tokio::task::spawn_blocking(move || {
                test_report(&job, &flag, |step| {
                    let _ = tx.send(step);
                })
            });
            while let Some(step) = rx.recv().await {
                progress.set(Some(step));
            }
            let ended = worker.await;
            progress.set(None);
            cancel.set(None);
            match ended {
                Ok(Ok(report)) => match keep(&folder, &report, &stamp) {
                    Ok(path) => {
                        say(&toasts, format!("Test report kept at {}", path.display()));
                        made.set(Some((path, report)));
                    }
                    Err(e) => warn(&toasts, format!("The report could not be kept: {e}")),
                },
                Ok(Err(_)) => note(&toasts, "Stopped - no report was kept"),
                Err(e) => warn(&toasts, format!("The report did not finish: {e}")),
            }
        });
    };

    rsx! {
        fieldset { class: "gate_rules-form",
            legend { "Test report" }
            p { class: "gate_rules-hint gate_rules-span",
                "Picks a rule for every gate a rule places, with the pick settings above as typed, and scores the picks and your rules as they stand against your gates, sample by sample. Writes it all to one report in the workspace's test_reports folder, to send on. Changes nothing else, and takes as long as a pick."
            }
            button {
                class: "gate_rules-add",
                disabled: progress().is_some(),
                onclick: start,
                if progress().is_some() { "Making the report..." } else { "Make a test report" }
            }
            if let Some(step) = progress() {
                div { class: "gate_rules-progress gate_rules-span",
                    div { class: "gate_rules-bar",
                        div {
                            class: "gate_rules-bar_fill",
                            style: "width: {step.fraction() * 100.0}%",
                        }
                    }
                    span { class: "gate_rules-progress_text", "{step.describe()}" }
                    button {
                        class: "gate_rules-cancel",
                        onclick: move |_| {
                            if let Some(flag) = cancel() {
                                flag.store(true, Ordering::Relaxed);
                                note(&toasts, "Stopping...");
                            }
                        },
                        "Stop"
                    }
                }
            }
            if let Some((path, report)) = made() {
                p { class: "gate_rules-hint gate_rules-span", "Kept at {path.display()}" }
                button {
                    class: "gate_rules-add",
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
