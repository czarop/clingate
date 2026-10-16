//! The Workspace tab's check of compensation and unmixing: spillover errors
//! told from autofluorescence left in - see [`clingate_core::spillover`]. The
//! tools for Claude check the same way with `check_spillover`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::prelude::*;

use clingate_core::gates::gate_paths::path_names;
use clingate_core::gates::gate_store::NodeId;
use clingate_core::session::SPILLOVER_SPECIMENS;
use clingate_core::spillover::{SpilloverCheck, SpilloverJob, job_for, run};

use crate::components::toast::{note, use_toast, warn};
use crate::gate_editor::gate_rules_window::{RulesRun, use_stop_run_on_change};
use crate::gate_editor::workspace_window::GateStore;

/// A check, read from the app's own state.
#[derive(Clone, Copy)]
pub(crate) struct SpilloverRun {
    run_with: RulesRun,
    gates: GateStore,
}

/// A job ready to run off the UI thread, and how many specimens it reads of
/// how many.
pub(crate) struct ReadyJob {
    pub job: SpilloverJob,
    pub specimens_read: usize,
    pub specimens: usize,
}

impl SpilloverRun {
    pub(crate) fn from_context() -> Self {
        Self {
            run_with: RulesRun::from_context(),
            gates: use_context(),
        }
    }

    /// Every population, by its path, in the order the paths read.
    pub(crate) fn populations(&self) -> Vec<(NodeId, String)> {
        let gates = self.gates.read();
        let mut found: Vec<(NodeId, String)> =
            clingate_core::gates::gate_paths::unique_names(&gates)
                .into_keys()
                .map(|node| {
                    let path = path_names(&gates, &node).join(" / ");
                    (node, path)
                })
                .collect();
        found.sort_by(|a, b| clingate_core::gate_rules::rule_store::human_order(&a.1, &b.1));
        found
    }

    /// The check on `node`, across `specimens` specimens spread through the
    /// workspace as it stands.
    pub(crate) fn job(&self, node: &NodeId, specimens: usize) -> Result<ReadyJob, String> {
        let (mut inputs, _) = self.run_with.inputs_now();
        if inputs.files.is_empty() {
            return Err("No FCS files are loaded".into());
        }
        let (specimens_read, specimens) = inputs.keep_specimens(specimens.max(1), &[]);
        Ok(ReadyJob {
            job: job_for(&self.gates.peek(), &inputs, node)?,
            specimens_read,
            specimens,
        })
    }
}

/// Choose a population, check it, and say what was found and what to do.
#[component]
pub fn SpilloverPanel() -> Element {
    let run_on = SpilloverRun::from_context();
    let toasts = use_toast();
    let mut population = use_signal(|| None::<NodeId>);
    let mut specimens = use_signal(|| SPILLOVER_SPECIMENS);
    let mut progress = use_signal(|| None::<(usize, usize)>);
    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
    let mut found = use_signal(|| None::<(usize, usize, SpilloverCheck)>);
    use_stop_run_on_change(cancel);
    let populations = use_memo(move || run_on.populations());

    let start = move |_| {
        spawn(async move {
            if progress.peek().is_some() {
                return;
            }
            let Some(node) = population.peek().clone() else {
                return warn(&toasts, "Choose the population to look in");
            };
            let ready = match run_on.job(&node, *specimens.peek()) {
                Ok(ready) => ready,
                Err(why) => return warn(&toasts, why),
            };
            let (read, of) = (ready.specimens_read, ready.specimens);
            progress.set(Some((0, 0)));
            let flag = Arc::new(AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(usize, usize)>();
            let worker = tokio::task::spawn_blocking(move || {
                run(&ready.job, &flag, |done, total| {
                    let _ = tx.send((done, total));
                })
            });
            while let Some(step) = rx.recv().await {
                progress.set(Some(step));
            }
            let ended = worker.await;
            progress.set(None);
            cancel.set(None);
            match ended {
                Ok(Ok(checked)) => found.set(Some((read, of, checked))),
                Ok(Err(why)) => note(&toasts, format!("Not checked: {why}")),
                Err(e) => warn(&toasts, format!("The check did not finish: {e}")),
            }
        });
    };

    rsx! {
        div { class: "workspace-row",
            h3 { "Check compensation and unmixing" }
            p { class: "workspace-hint",
                "Tells a spillover error from autofluorescence the unmixing left in. A channel rising with another alike on every sample is the matrix's, and it says which value to change by how much; on some samples only, their dye's. Large, granular cells negative for everything sitting higher in a channel are autofluorescence to extract, and it names the channel to look in on the unstained. Look in a clean population - single live cells or lymphocytes. Unstained files, marked U or Unstained in the sample type column, are read where there are any. Changes nothing."
            }
            if populations.read().is_empty() {
                p { class: "workspace-hint", "Load a gating file to choose a population." }
            } else {
                div { class: "workspace-path",
                    select {
                        onchange: move |e| {
                            let chosen = e.value();
                            population.set(
                                populations
                                    .read()
                                    .iter()
                                    .find(|(node, _)| node.as_str() == chosen)
                                    .map(|(node, _)| node.clone()),
                            );
                        },
                        option { value: "", selected: population().is_none(), "choose the population to look in" }
                        for (node , path) in populations.read().iter() {
                            option {
                                key: "{node.as_str()}",
                                value: "{node.as_str()}",
                                selected: population().as_ref() == Some(node),
                                "{path}"
                            }
                        }
                    }
                    label { class: "workspace-dim", "specimens" }
                    input {
                        class: "workspace-number",
                        r#type: "number",
                        min: "1",
                        value: "{specimens}",
                        oninput: move |e| {
                            if let Ok(n) = e.value().parse::<usize>() {
                                specimens.set(n.max(1));
                            }
                        },
                    }
                    button {
                        class: "workspace-primary",
                        disabled: progress().is_some() || population().is_none(),
                        onclick: start,
                        if progress().is_some() { "Checking..." } else { "Check" }
                    }
                }
            }
            if let Some((done, total)) = progress() {
                div { class: "workspace-progress",
                    div { class: "workspace-bar",
                        div {
                            class: "workspace-bar_fill",
                            style: "width: {done as f64 / total.max(1) as f64 * 100.0}%",
                        }
                    }
                    span { class: "workspace-dim", "read {done} of {total} files" }
                    button {
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
            if let Some((read, of, checked)) = found() {
                p { class: "workspace-hint",
                    "Read {read} of {of} specimens: {checked.stained} stained and {checked.unstained} unstained files."
                }
                if checked.findings.is_empty() {
                    p { "Nothing leans by a negative's width or more." }
                }
                ul { class: "workspace-findings",
                    for (at , finding) in checked.findings.iter().enumerate() {
                        li { key: "{at}", "{finding.says()}" }
                    }
                    for (at , problem) in checked.problems.iter().enumerate() {
                        li { key: "p{at}", class: "workspace-problem", "{problem}" }
                    }
                }
            }
        }
    }
}
