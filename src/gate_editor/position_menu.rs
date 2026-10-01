//! The Position menu over each plot: where the selected gate's position on
//! the plot's sample comes from, and moving it - to this sample alone, to
//! every sample sharing one of its metadata values, off a column's positions,
//! or onto other samples. Each change is one step of the working copy.

use std::sync::Arc;

use clingate_core::gates::GateState;
use clingate_core::gates::gate_positions::{self, Release, Tier};
use clingate_core::gates::gate_store::{FileId, GateStateStoreExt, NodeId};
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

/// What the button says a sample shows.
pub(crate) fn tier_label(tier: &Tier) -> String {
    match tier {
        Tier::Drawn => "as drawn, for every sample".to_string(),
        Tier::Group(key) => format!("by {}, {}", key.parameter, key.group),
        Tier::Sample => "this sample's own".to_string(),
    }
}

/// The samples to offer for "apply to", by name, without `file` itself.
fn others(names: &[(Arc<str>, FileId)], file: &FileId, search: &str) -> Vec<(Arc<str>, FileId)> {
    let lowered = search.to_lowercase();
    names
        .iter()
        .filter(|(name, id)| id != file && matches_search(name, &lowered))
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

/// The selected gate's position on this plot's sample, and the ways to move
/// it. Disabled with no gate on the plot selected.
#[component]
pub fn PositionMenu(sample_name: Arc<str>, parental_gate: ReadSignal<Option<Arc<str>>>) -> Element {
    let gates = use_context::<GateStore>();
    let metadata = use_context::<MetadataStore>();
    let edits = use_context::<Edits>();
    let toasts = use_toast();
    let mut open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let mut chosen = use_signal(Vec::<FileId>::new);

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
                title: "Select a gate on the plot to see where its position on this sample comes from",
                "Position"
            }
        };
    };
    let file = target.sample.clone();
    let files: MetaDataFileMap = metadata.metadata().read().clone();
    let Some(tier) = gate_positions::tier(&gates.read(), &gate_id, &file, &files) else {
        return rsx! {};
    };
    let mut columns: Vec<(Arc<str>, Arc<str>)> = files
        .get(&file)
        .map(|row| row.iter().map(|(c, v)| (c.clone(), v.clone())).collect())
        .unwrap_or_default();
    columns.sort();
    let mut names: Vec<(Arc<str>, FileId)> = metadata
        .file_name_to_gating_id()
        .read()
        .iter()
        .map(|(name, id)| (name.clone(), id.clone()))
        .collect();
    names.sort();
    let offered = others(&names, &file, &search());
    let on = target.sample_name.clone();
    let gate = target.gate.clone();

    rsx! {
        div { class: "position-menu",
            button {
                class: "position-menu_button",
                title: "Where {gate}'s position on {on} comes from - click to change it",
                onclick: move |_| open.toggle(),
                "Position: {tier_label(&tier)}"
            }
            if open() {
                div { class: "position-menu_panel",
                    p { class: "position-menu_heading", "{gate} on {on}: {tier_label(&tier)}" }

                    if tier != Tier::Sample {
                        button {
                            onclick: {
                                let (gate_id, file, files, on) = (gate_id.clone(), file.clone(), files.clone(), on.clone());
                                move |_| {
                                    act(gates, edits, toasts, format!("{on} has a position of its own"), |state| {
                                        gate_positions::keep_for_sample(state, &gate_id, &file, &files)
                                    });
                                    open.set(false);
                                }
                            },
                            "Only this sample"
                        }
                    }
                    for (column , value) in columns.clone() {
                        if !matches!(&tier, Tier::Group(key) if key.parameter == column) {
                            button {
                                key: "{column}",
                                onclick: {
                                    let (gate_id, file, files) = (gate_id.clone(), file.clone(), files.clone());
                                    let said = format!("Every sample with {column} {value} has this position");
                                    move |_| {
                                        act(gates, edits, toasts, said.clone(), |state| {
                                            gate_positions::keep_for_group(state, &gate_id, &file, &column, &files)
                                        });
                                        open.set(false);
                                    }
                                },
                                "Every sample with {column} {value}"
                            }
                        }
                    }

                    if let Tier::Group(key) = tier.clone() {
                        p { class: "position-menu_heading", "Remove the positions by {key.parameter}:" }
                        for (how , label) in [
                            (Release::ToDrawn, "back to the gate as drawn"),
                            (Release::ToEachSample, "each sample keeps its own"),
                        ] {
                            button {
                                key: "{label}",
                                onclick: {
                                    let (gate_id, files, column) = (gate_id.clone(), files.clone(), key.parameter.clone());
                                    move |_| {
                                        act(gates, edits, toasts, format!("No positions by {column} - {label}"), |state| {
                                            gate_positions::release_column(state, &gate_id, &column, how, &files)
                                        });
                                        open.set(false);
                                    }
                                },
                                "{label}"
                            }
                        }
                    }
                    if tier == Tier::Sample {
                        button {
                            onclick: {
                                let (gate_id, file, on) = (gate_id.clone(), file.clone(), on.clone());
                                move |_| {
                                    act(gates, edits, toasts, format!("{on} shows its group's position, or the gate as drawn"), |state| {
                                        gate_positions::release_sample(state, &gate_id, &file);
                                        Ok(())
                                    });
                                    open.set(false);
                                }
                            },
                            "Remove this sample's own position"
                        }
                    }

                    p { class: "position-menu_heading", "Apply this position to:" }
                    div { class: "position-menu_picks",
                        button {
                            onclick: {
                                let all: Vec<FileId> = names.iter().map(|(_, id)| id.clone()).filter(|id| *id != file).collect();
                                move |_| chosen.set(all.clone())
                            },
                            "all"
                        }
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
                        button { onclick: move |_| chosen.set(Vec::new()), "none" }
                    }
                    input {
                        class: "position-menu_search",
                        placeholder: "Filter samples",
                        value: "{search}",
                        oninput: move |e| search.set(e.value()),
                    }
                    div { class: "position-menu_samples",
                        for (name , id) in offered {
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
                            let (gate_id, file, files) = (gate_id.clone(), file.clone(), files.clone());
                            move |_| {
                                let to = chosen.peek().clone();
                                act(gates, edits, toasts, format!("{} samples have this position as their own", to.len()), |state| {
                                    gate_positions::copy_to_samples(state, &gate_id, &file, &to, &files)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_samples_offered_leave_out_this_one_and_follow_the_search() {
        let names: Vec<(Arc<str>, FileId)> = ["DONOR-A_FS", "DONOR-A_FMX", "DONOR-B_FS"]
            .iter()
            .map(|n| (Arc::from(*n), Arc::from(n.to_lowercase())))
            .collect();
        let here: FileId = Arc::from("donor-a_fs");
        let named = |search: &str| -> Vec<String> {
            others(&names, &here, search)
                .iter()
                .map(|(name, _)| name.to_string())
                .collect()
        };
        assert_eq!(named(""), ["DONOR-A_FMX", "DONOR-B_FS"]);
        assert_eq!(named("fs"), ["DONOR-B_FS"]);
        assert_eq!(named("donor-b"), ["DONOR-B_FS"]);
    }

    #[test]
    fn the_button_says_where_the_position_comes_from() {
        use clingate_core::omiq::metadata::MetaDataKey;
        assert_eq!(tier_label(&Tier::Drawn), "as drawn, for every sample");
        assert_eq!(tier_label(&Tier::Sample), "this sample's own");
        assert_eq!(
            tier_label(&Tier::Group(MetaDataKey {
                parameter: Arc::from("SampleID"),
                group: Arc::from("DONOR-A"),
            })),
            "by SampleID, DONOR-A"
        );
    }
}
