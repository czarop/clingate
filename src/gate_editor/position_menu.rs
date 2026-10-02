//! The Position menu over each plot: how the selected gate is positioned -
//! the same for every sample, by a metadata column, or per sample - and
//! putting this sample's position onto other samples, or other values of the
//! column, without changing that. Each change is one step of the working
//! copy.

use std::sync::Arc;

use clingate_core::gates::GateState;
use clingate_core::gates::gate_positions::{self, Mode};
use clingate_core::gates::gate_store::{GateStateStoreExt, NodeId};
use clingate_core::omiq::metadata::{MetaDataFileMap, MetaDataStoreStoreExt};
use dioxus::prelude::*;

use crate::components::toast::{Toasts, note, use_toast, warn};
use crate::gate_editor::edits::Edits;
use crate::gate_editor::review::selected_target;
use crate::gate_editor::workspace_window::{GateStore, MetadataStore};
use crate::searchable_select::matches_search;

/// `change` made to the gates as one step of the working copy.
pub(crate) fn reposition(
    mut gates: GateStore,
    edits: Edits,
    change: impl FnOnce(&mut GateState) -> anyhow::Result<()>,
) -> Result<(), String> {
    let before = edits.before();
    let done = change(&mut gates.write());
    edits.after(before);
    done.map_err(|e| e.to_string())
}

/// How a gate is positioned, as the menu says it.
pub(crate) fn mode_label(mode: &Mode) -> String {
    match mode {
        Mode::Global => "the same for every sample".to_string(),
        Mode::ByColumn(column) => format!("by {column}"),
        Mode::PerSample => "per sample".to_string(),
    }
}

/// What "apply this position to" offers, as `(shown, id)`: without `here`,
/// and only those the search finds.
fn offered(
    entries: &[(Arc<str>, Arc<str>)],
    here: &Arc<str>,
    search: &str,
) -> Vec<(Arc<str>, Arc<str>)> {
    let lowered = search.to_lowercase();
    entries
        .iter()
        .filter(|(shown, id)| id != here && matches_search(shown, &lowered))
        .cloned()
        .collect()
}

fn act(
    gates: GateStore,
    edits: Edits,
    toasts: Toasts,
    said: String,
    change: impl FnOnce(&mut GateState) -> anyhow::Result<()>,
) {
    match reposition(gates, edits, change) {
        Ok(()) => note(&toasts, said),
        Err(e) => warn(&toasts, e),
    }
}

/// How the gate selected on this plot is positioned, and the ways to change
/// it. Disabled with no gate on the plot selected.
#[component]
pub fn PositionMenu(sample_name: Arc<str>, parental_gate: ReadSignal<Option<Arc<str>>>) -> Element {
    let gates = use_context::<GateStore>();
    let metadata = use_context::<MetadataStore>();
    let edits = use_context::<Edits>();
    let toasts = use_toast();
    let mut open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let mut chosen = use_signal(Vec::<Arc<str>>::new);

    let selected = gates.selected_gate().read().clone();
    let file = metadata
        .file_name_to_gating_id()
        .read()
        .get(&sample_name)
        .cloned();
    let parent = parental_gate().map(NodeId::from);
    let target = selected_target(
        &gates.read(),
        selected.as_ref(),
        parent.as_ref(),
        file,
        &sample_name,
    );
    let (Some(gate_id), Some(target)) = (selected, target) else {
        return rsx! {
            button {
                class: "position-menu_button",
                disabled: true,
                title: "Select a gate on the plot to see how it is positioned",
                "Position"
            }
        };
    };
    let file = target.sample.clone();
    let files: MetaDataFileMap = metadata.metadata().read().clone();
    let mode = gate_positions::mode(&gates.read(), &gate_id);
    let mut columns: Vec<(Arc<str>, Arc<str>)> = files
        .get(&file)
        .map(|row| row.iter().map(|(c, v)| (c.clone(), v.clone())).collect())
        .unwrap_or_default();
    columns.sort();
    let mut modes = vec![Mode::Global];
    modes.extend(
        columns
            .iter()
            .map(|(column, _)| Mode::ByColumn(column.clone())),
    );
    modes.push(Mode::PerSample);

    // What "apply this position to" offers: samples, or the column's values.
    let (entries, here): (Vec<(Arc<str>, Arc<str>)>, Arc<str>) = match &mode {
        Mode::ByColumn(column) => (
            gate_positions::values_of(&files, column)
                .into_iter()
                .map(|value| (value.clone(), value))
                .collect(),
            files
                .get(&file)
                .and_then(|row| row.get(column))
                .cloned()
                .unwrap_or_default(),
        ),
        _ => {
            let mut names: Vec<(Arc<str>, Arc<str>)> = metadata
                .file_name_to_gating_id()
                .read()
                .iter()
                .map(|(name, id)| (name.clone(), id.clone()))
                .collect();
            names.sort();
            (names, file.clone())
        }
    };
    let shown = offered(&entries, &here, &search());
    let on = target.sample_name.clone();
    let gate = target.gate.clone();

    rsx! {
        div { class: "position-menu",
            button {
                class: "position-menu_button",
                title: "How {gate} is positioned - click to change it",
                onclick: move |_| open.toggle(),
                "Position: {mode_label(&mode)}"
            }
            if open() {
                div { class: "position-menu_panel",
                    p { class: "position-menu_heading", "Position {gate}:" }
                    for choice in modes {
                        button {
                            key: "{mode_label(&choice)}",
                            disabled: choice == mode,
                            title: if choice == Mode::Global { "Every sample takes the position on {on}" } else { "Every sample keeps the position it has now" },
                            onclick: {
                                let (gate_id, file, files, gate, choice) = (gate_id.clone(), file.clone(), files.clone(), gate.clone(), choice.clone());
                                move |_| {
                                    let said = format!("{gate} is positioned {}", mode_label(&choice));
                                    act(gates, edits, toasts, said, |state| {
                                        gate_positions::set_mode(state, &gate_id, &choice, Some(&file), &files)
                                    });
                                    chosen.set(Vec::new());
                                    open.set(false);
                                }
                            },
                            "{mode_label(&choice)}"
                        }
                    }

                    if mode != Mode::Global {
                        p { class: "position-menu_heading", "Apply the position on {on} to:" }
                        div { class: "position-menu_picks",
                            button {
                                onclick: {
                                    let all: Vec<Arc<str>> = entries.iter().map(|(_, id)| id.clone()).filter(|id| *id != here).collect();
                                    move |_| chosen.set(all.clone())
                                },
                                "all"
                            }
                            if mode == Mode::PerSample {
                                for (column , value) in columns.clone() {
                                    button {
                                        key: "same-{column}",
                                        onclick: {
                                            let (file, files) = (file.clone(), files.clone());
                                            move |_| {
                                                let same = gate_positions::sharing(&files, &file, &column);
                                                chosen.set(same.into_iter().filter(|id| *id != file).collect());
                                            }
                                        },
                                        "same {column} ({value})"
                                    }
                                }
                            }
                            button { onclick: move |_| chosen.set(Vec::new()), "none" }
                        }
                        input {
                            class: "position-menu_search",
                            placeholder: "Filter",
                            value: "{search}",
                            oninput: move |e| search.set(e.value()),
                        }
                        div { class: "position-menu_samples",
                            for (name , id) in shown {
                                label { key: "{id}",
                                    input {
                                        r#type: "checkbox",
                                        checked: chosen.read().contains(&id),
                                        onchange: {
                                            let id = id.clone();
                                            move |_| {
                                                chosen.with_mut(|picked| {
                                                    match picked.iter().position(|p| *p == id) {
                                                        Some(at) => {
                                                            picked.remove(at);
                                                        }
                                                        None => picked.push(id.clone()),
                                                    }
                                                })
                                            }
                                        },
                                    }
                                    "{name}"
                                }
                            }
                        }
                        button {
                            disabled: chosen.read().is_empty(),
                            onclick: {
                                let (gate_id, file, files, mode) = (gate_id.clone(), file.clone(), files.clone(), mode.clone());
                                move |_| {
                                    let to = chosen.peek().clone();
                                    let said = format!("{} now have the position on {on}", to.len());
                                    act(gates, edits, toasts, said, |state| match &mode {
                                        Mode::ByColumn(column) => gate_positions::copy_to_values(
                                            state, &gate_id, &file, column, &to, &files,
                                        ),
                                        _ => gate_positions::copy_to_samples(state, &gate_id, &file, &to, &files),
                                    });
                                    chosen.set(Vec::new());
                                    open.set(false);
                                }
                            },
                            "Apply to {chosen.read().len()}"
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

    #[test]
    fn what_is_offered_leaves_out_this_one_and_follows_the_search() {
        let entries: Vec<(Arc<str>, Arc<str>)> = ["DONOR-A_FS", "DONOR-A_FMX", "DONOR-B_FS"]
            .iter()
            .map(|n| (Arc::from(*n), Arc::from(n.to_lowercase())))
            .collect();
        let here: Arc<str> = Arc::from("donor-a_fs");
        let named = |search: &str| -> Vec<String> {
            offered(&entries, &here, search)
                .iter()
                .map(|(name, _)| name.to_string())
                .collect()
        };
        assert_eq!(named(""), ["DONOR-A_FMX", "DONOR-B_FS"]);
        assert_eq!(named("fs"), ["DONOR-B_FS"]);
        assert_eq!(named("donor-b"), ["DONOR-B_FS"]);
    }

    #[test]
    fn the_button_says_how_the_gate_is_positioned() {
        assert_eq!(mode_label(&Mode::Global), "the same for every sample");
        assert_eq!(mode_label(&Mode::PerSample), "per sample");
        assert_eq!(
            mode_label(&Mode::ByColumn(Arc::from("SampleID"))),
            "by SampleID"
        );
    }
}
