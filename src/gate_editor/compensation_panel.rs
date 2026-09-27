//! The compensation part of the Workspace tab, laid out as Omiq lays out its
//! compensation task: a tab per group, each showing the group's files beside
//! its matrix, with everything that can be done to the group in one Actions
//! menu. Files move between groups through one dialog that can search and
//! pick many at once.
//!
//! What a group does is decided in [`crate::compensation::groups`]; this only
//! shows it and says what was asked for, as a [`CompensationAction`] the
//! Workspace tab carries out.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dioxus::prelude::*;

use crate::compensation::Spillover;
use crate::compensation::groups::{Applied, Compensation, FileFacts, Group, GroupId, Source};
use crate::components::toast::{use_toast, warn};
use crate::file_load::FcsFiles;
use crate::gate_editor::path_picker::{Chosen, Pick, UNAVAILABLE, choose};
use crate::gate_editor::workspace_window::Loaded;
use crate::searchable_select::matches_search;

/// Something done to the compensation groups from the tab.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum CompensationAction {
    /// Compensate the group with nothing, its files' own matrices, or a
    /// matrix held here.
    Source(GroupId, Source),
    LoadCsv(GroupId, PathBuf),
    /// Move files to a group, or to a new one.
    Assign(Vec<PathBuf>, Option<GroupId>),
    NewGroup,
    Rename(GroupId, String),
    Remove(GroupId),
    /// Nothing was applied to the group's Omiq exports: exported with no
    /// compensation task, or with one left at 0% throughout - the two export
    /// identically.
    AppliedNothing(GroupId),
    /// The group's Omiq exports were compensated in Omiq with this matrix.
    AppliedMatrix(GroupId, Arc<Spillover>),
    /// Ask again what Omiq applied.
    AppliedForget(GroupId),
    /// One entry of the wanted matrix, in percent: from, into, value.
    SetValue(GroupId, String, String, f64),
    /// A matrix over the files' fluorescence channels, compensating nothing.
    StartMatrix(GroupId),
    /// Want what Omiq applied again, dropping edits.
    ResetToApplied(GroupId),
    /// Write the matrix to `path`: for Omiq, or for the files as exported.
    Save(GroupId, PathBuf, ExportFor),
    /// Put the matrix for Omiq on the clipboard.
    Copy(GroupId),
}

/// Who an exported matrix is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExportFor {
    /// Omiq, which holds the files as recorded: the matrix wanted.
    Omiq,
    /// Software compensating the files as Omiq exported them: what takes
    /// them from what Omiq applied to what is wanted.
    TheseFiles,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Put the cursor in a box as it appears: `autofocus` only works for what is
/// on the page when it loads.
async fn focus(e: MountedEvent) {
    let _ = e.set_focus(true).await;
}

/// What the checks need of one of the workspace's files.
fn facts(files: &Option<FcsFiles>, path: &Path) -> FileFacts {
    files
        .as_ref()
        .and_then(|f| f.file_list().iter().find(|s| s.get_filepath() == path))
        .map(crate::compensation::facts_of)
        .unwrap_or_default()
}

/// What the tab shows of one group.
#[derive(Clone, PartialEq)]
struct GroupView {
    group: Group,
    /// Its files: path, and the name the program shows.
    members: Vec<(PathBuf, String)>,
    notes: Vec<String>,
    /// Whether any of its files were exported from Omiq.
    holds_omiq: bool,
    /// Whether it holds Omiq exports and has not said what Omiq applied.
    unanswered: bool,
    /// The one matrix it wants, if it has one.
    wanted: Option<Arc<Spillover>>,
    /// What Omiq applied to its exports, if a matrix.
    applied: Option<Arc<Spillover>>,
}

impl GroupView {
    /// Whether its tab should draw the eye: a question to answer, or a
    /// problem.
    fn needs_attention(&self) -> bool {
        self.unanswered || !self.notes.is_empty()
    }
}

/// A file as the assign dialog lists it.
#[derive(Clone, PartialEq)]
struct AssignRow {
    path: PathBuf,
    name: String,
    group: GroupId,
    group_name: String,
}

/// The compensation groups: which matrix each file's events are compensated
/// with.
#[component]
pub(crate) fn CompensationPanel(
    busy: bool,
    on_action: EventHandler<CompensationAction>,
) -> Element {
    let files = use_context::<Signal<Option<FcsFiles>>>();
    let compensation = use_context::<Signal<Compensation>>();
    let loaded = use_context::<Signal<Loaded>>();
    let mut selected = use_signal(|| None::<GroupId>);
    let mut assigning = use_signal(|| false);

    let comp = compensation.read();
    let held = files.read();
    let names: std::collections::BTreeMap<&Path, String> = held
        .as_ref()
        .map(|f| {
            f.file_list()
                .iter()
                .map(|s| (s.get_filepath(), s.name().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let name_of = |p: &Path| names.get(p).cloned().unwrap_or_else(|| file_name(p));
    let views: Vec<GroupView> = comp
        .groups()
        .iter()
        .map(|g| GroupView {
            group: g.clone(),
            members: comp
                .files_in(g.id)
                .map(|p| (p.to_path_buf(), name_of(p)))
                .collect(),
            notes: comp.check(g.id, |p| facts(&held, p)),
            holds_omiq: comp.holds_omiq_exports(g.id),
            unanswered: comp.unanswered(g.id),
            wanted: comp.wanted(g.id),
            applied: comp.applied(g.id),
        })
        .collect();
    let rows: Vec<AssignRow> = views
        .iter()
        .flat_map(|v| {
            v.members.iter().map(|(path, name)| AssignRow {
                path: path.clone(),
                name: name.clone(),
                group: v.group.id,
                group_name: v.group.name.clone(),
            })
        })
        .collect();
    drop(comp);
    drop(held);
    let folder = loaded.read().folder.clone();

    if views.is_empty() {
        return rsx! {};
    }
    let current = selected()
        .filter(|id| views.iter().any(|v| v.group.id == *id))
        .unwrap_or(views[0].group.id);
    let groups: Vec<(GroupId, String)> = views
        .iter()
        .map(|v| (v.group.id, v.group.name.clone()))
        .collect();
    let shown = views
        .iter()
        .find(|v| v.group.id == current)
        .cloned()
        .expect("current is one of them");

    rsx! {
        div { class: "workspace-row",
            h3 {
                "Compensation"
                span { class: "workspace-status",
                    if views.len() == 1 { "1 group" } else { "{views.len()} groups" }
                }
                span { class: "workspace-comp-spacer" }
                button {
                    disabled: busy,
                    onclick: move |_| assigning.set(true),
                    "Assign files…"
                }
            }
            div { class: "workspace-comp-tabs",
                for view in views.iter() {
                    button {
                        key: "{view.group.id}",
                        class: if view.group.id == current { "workspace-comp-tab workspace-comp-tab-on" } else { "workspace-comp-tab" },
                        onclick: {
                            let id = view.group.id;
                            move |_| selected.set(Some(id))
                        },
                        "{view.group.name}"
                        span { class: "workspace-comp-count", " ({view.members.len()})" }
                        if view.needs_attention() {
                            span { class: "workspace-comp-flag", title: "Needs attention", " !" }
                        }
                    }
                }
                button {
                    class: "workspace-comp-tab workspace-comp-tab-add",
                    disabled: busy,
                    title: "New group",
                    onclick: move |_| {
                        on_action.call(CompensationAction::NewGroup);
                        let last = compensation.peek().groups().last().map(|g| g.id);
                        selected.set(last);
                    },
                    "+"
                }
            }
            GroupPanel {
                key: "{current}",
                view: shown,
                folder,
                busy,
                on_action,
            }
            if assigning() {
                AssignDialog {
                    rows,
                    groups,
                    to: current,
                    on_assign: move |(paths, to): (Vec<PathBuf>, Option<GroupId>)| {
                        let first = paths.first().cloned();
                        on_action.call(CompensationAction::Assign(paths, to));
                        if let Some(first) = first {
                            let now = compensation.peek().group_of(&first);
                            selected.set(now);
                        }
                        assigning.set(false);
                    },
                    on_close: move |_| assigning.set(false),
                }
            }
        }
    }
}

/// One group: what it is compensated with, its files beside its matrix, and
/// its Actions menu.
#[component]
fn GroupPanel(
    view: GroupView,
    folder: Option<PathBuf>,
    busy: bool,
    on_action: EventHandler<CompensationAction>,
) -> Element {
    let files = use_context::<Signal<Option<FcsFiles>>>();
    let compensation = use_context::<Signal<Compensation>>();
    let mut renaming = use_signal(|| None::<String>);
    let mut pasting = use_signal(|| false);
    let mut paste_error = use_signal(|| None::<String>);
    let mut spilling_only = use_signal(|| false);
    let GroupView {
        group,
        members,
        notes,
        holds_omiq,
        unanswered,
        wanted,
        applied,
    } = view;
    let id = group.id;
    let count = members.len();

    let placeholder = if unanswered {
        "Say whether compensation was applied in Omiq first.".to_string()
    } else {
        match &group.source {
            Source::None => "No compensation. Choose a matrix from Actions.".to_string(),
            Source::FilesOwn => "Each file uses the matrix in its own FCS header.".to_string(),
            Source::Unreadable { path, why } => format!("{} can't be read: {why}", path.display()),
            Source::Loaded { .. } | Source::Edited { .. } => String::new(),
        }
    };

    rsx! {
        div { class: "workspace-comp-panel",
            div { class: "workspace-comp-head",
                if let Some(name) = renaming() {
                    input {
                        class: "workspace-comp-name",
                        value: "{name}",
                        onmounted: focus,
                        oninput: move |e| renaming.set(Some(e.value())),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                if let Some(name) = renaming.peek().clone() {
                                    on_action.call(CompensationAction::Rename(id, name));
                                }
                                renaming.set(None);
                            } else if e.key() == Key::Escape {
                                renaming.set(None);
                            }
                        },
                    }
                    button {
                        class: "workspace-primary",
                        onclick: move |_| {
                            if let Some(name) = renaming.peek().clone() {
                                on_action.call(CompensationAction::Rename(id, name));
                            }
                            renaming.set(None);
                        },
                        "OK"
                    }
                    button { onclick: move |_| renaming.set(None), "Cancel" }
                } else {
                    strong { "{group.name}" }
                }
                span { class: "workspace-status", "Compensation: {group.source.describe()}" }
                span { class: "workspace-comp-spacer" }
                ActionsMenu {
                    group: group.clone(),
                    count,
                    holds_omiq,
                    unanswered,
                    has_wanted: wanted.is_some(),
                    has_applied: applied.is_some(),
                    can_reset: applied
                        .as_ref()
                        .is_some_and(|a| wanted.as_ref().is_none_or(|w| !w.same_as(a))),
                    folder,
                    busy,
                    on_action,
                    on_paste: move |_| {
                        paste_error.set(None);
                        pasting.set(true);
                    },
                    on_rename: {
                        let name = group.name.clone();
                        move |_| renaming.set(Some(name.clone()))
                    },
                }
            }

            if holds_omiq {
                OmiqBanner {
                    group: id,
                    applied: group.applied.clone(),
                    busy,
                    on_action,
                }
            }

            if pasting() {
                PasteBox {
                    prompt: "Paste a compensation matrix copied from Omiq or a spreadsheet.",
                    error: paste_error(),
                    busy,
                    on_ok: move |text: String| {
                        let read = compensation
                            .peek()
                            .read_pasted(id, &text, |p| facts(&files.peek(), p));
                        match read {
                            Ok(matrix) => {
                                on_action.call(CompensationAction::Source(
                                    id,
                                    Source::Edited {
                                        matrix: Arc::new(matrix),
                                        from: Some("Pasted matrix".to_string()),
                                        changed: false,
                                    },
                                ));
                                pasting.set(false);
                            }
                            Err(why) => paste_error.set(Some(why)),
                        }
                    },
                    on_cancel: move |_| pasting.set(false),
                }
            }

            for note in notes {
                p { class: "workspace-problem", "{note}" }
            }

            div { class: "workspace-comp-body",
                div { class: "workspace-comp-files",
                    div { class: "workspace-comp-subhead", "Files in group ({count})" }
                    if count == 0 {
                        p { class: "workspace-dim", "No files. Use Assign files… to move some here." }
                    } else {
                        ul {
                            for (path, name) in members {
                                li { key: "{path.display()}", title: "{path.display()}", "{name}" }
                            }
                        }
                    }
                }
                div { class: "workspace-comp-matrix",
                    div { class: "workspace-comp-subhead",
                        "Compensation matrix"
                        if let Some(m) = wanted.as_ref().filter(|_| !unanswered) {
                            if m.involved().is_some_and(|i| i.channels().len() < m.channels().len()) {
                                label { class: "workspace-comp-toggle",
                                    input {
                                        r#type: "checkbox",
                                        checked: spilling_only(),
                                        onchange: move |e| spilling_only.set(e.checked()),
                                    }
                                    " Only channels that spill"
                                }
                            }
                        }
                    }
                    match wanted.clone().filter(|_| !unanswered) {
                        Some(matrix) => rsx! {
                            MatrixGrid {
                                matrix,
                                reference: applied.clone(),
                                spilling_only: spilling_only(),
                                editable: !busy,
                                on_set: move |(from, into, percent): (String, String, f64)| {
                                    on_action.call(CompensationAction::SetValue(id, from, into, percent))
                                },
                            }
                            p { class: "workspace-hint",
                                "Rows spill into columns, in percent, as in Omiq."
                                if applied.is_some() {
                                    " Highlighted values differ from Omiq's matrix; only the difference is applied here."
                                }
                            }
                        },
                        None => rsx! {
                            p { class: "workspace-dim", "{placeholder}" }
                        },
                    }
                }
            }
        }
    }
}

/// Everything that can be done to a group, in one menu.
#[component]
fn ActionsMenu(
    group: Group,
    count: usize,
    holds_omiq: bool,
    unanswered: bool,
    has_wanted: bool,
    has_applied: bool,
    can_reset: bool,
    folder: Option<PathBuf>,
    busy: bool,
    on_action: EventHandler<CompensationAction>,
    on_paste: EventHandler<()>,
    on_rename: EventHandler<()>,
) -> Element {
    let toasts = use_toast();
    let mut open = use_signal(|| false);
    let id = group.id;
    let file_stem = group.name.replace(['/', '\\'], "-");
    let place = move |file: String| {
        folder
            .as_ref()
            .map(|f| f.join(&file))
            .unwrap_or_else(|| PathBuf::from(file))
    };
    let omiq_start = place(format!("{file_stem} compensation.csv"));
    let files_start = place(format!("{file_stem} compensation for exported files.csv"));
    // Choose where to save, then save there.
    let mut save_to = move |start: PathBuf, export_for: ExportFor| {
        open.set(false);
        spawn(async move {
            let filter = ["csv".to_string()];
            match choose(Pick::SaveFile, &start, "CSV", &filter, false).await {
                Chosen::Picked(picked) => {
                    if let Some(path) = picked.into_iter().next() {
                        on_action.call(CompensationAction::Save(id, path, export_for));
                    }
                }
                Chosen::Cancelled => {}
                Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
            }
        });
    };
    let load = move |_| {
        open.set(false);
        spawn(async move {
            let filter = ["csv".to_string()];
            let start = PathBuf::new();
            match choose(Pick::OpenFile, &start, "CSV", &filter, false).await {
                Chosen::Picked(picked) => {
                    if let Some(path) = picked.into_iter().next() {
                        on_action.call(CompensationAction::LoadCsv(id, path));
                    }
                }
                Chosen::Cancelled => {}
                Chosen::Unavailable => warn(&toasts, UNAVAILABLE),
            }
        });
    };
    // Do it, and close the menu.
    let act = move |action: CompensationAction| {
        move |_| {
            open.set(false);
            on_action.call(action.clone());
        }
    };
    let own_chosen = matches!(group.source, Source::FilesOwn);
    let none_chosen = matches!(group.source, Source::None);

    rsx! {
        div { class: "workspace-comp-menu",
            button {
                disabled: busy,
                onclick: move |_| open.toggle(),
                "Actions ▾"
            }
            if open() {
                // Clicking anywhere else closes the menu.
                div { class: "workspace-comp-backdrop", onclick: move |_| open.set(false) }
                div { class: "workspace-comp-menu-list",
                    button {
                        disabled: unanswered,
                        onclick: move |_| {
                            open.set(false);
                            on_paste.call(());
                        },
                        "Paste matrix…"
                    }
                    button { disabled: unanswered, onclick: load, "Load matrix from CSV…" }
                    button {
                        disabled: unanswered || count == 0,
                        onclick: act(CompensationAction::StartMatrix(id)),
                        "New matrix"
                    }
                    button {
                        disabled: unanswered || own_chosen,
                        onclick: act(CompensationAction::Source(id, Source::FilesOwn)),
                        "Use each file's own matrix"
                    }
                    button {
                        disabled: none_chosen,
                        onclick: act(CompensationAction::Source(id, Source::None)),
                        "No compensation"
                    }
                    if can_reset {
                        button {
                            onclick: act(CompensationAction::ResetToApplied(id)),
                            "Reset to Omiq's matrix"
                        }
                    }
                    if has_wanted && !unanswered {
                        hr {}
                        button {
                            onclick: act(CompensationAction::Copy(id)),
                            "Copy matrix for Omiq"
                        }
                        button {
                            onclick: {
                                let start = omiq_start.clone();
                                move |_| save_to(start.clone(), ExportFor::Omiq)
                            },
                            "Save matrix for Omiq…"
                        }
                        if has_applied {
                            button {
                                title: "The matrix that takes these files, as Omiq exported them, to this one - for other software that compensates the exported files.",
                                onclick: {
                                    let start = files_start.clone();
                                    move |_| save_to(start.clone(), ExportFor::TheseFiles)
                                },
                                "Save matrix for the exported files…"
                            }
                        }
                    }
                    hr {}
                    if holds_omiq && !unanswered {
                        button {
                            onclick: act(CompensationAction::AppliedForget(id)),
                            "Change Omiq answer"
                        }
                    }
                    button {
                        onclick: move |_| {
                            open.set(false);
                            on_rename.call(());
                        },
                        "Rename group…"
                    }
                    button {
                        disabled: count > 0,
                        title: if count > 0 { "Move its files to another group first" } else { "" },
                        onclick: act(CompensationAction::Remove(id)),
                        "Delete group"
                    }
                }
            }
        }
    }
}

/// For a group holding files Omiq exported: whether Omiq applied
/// compensation to them, which the files don't record.
#[component]
fn OmiqBanner(
    group: GroupId,
    applied: Applied,
    busy: bool,
    on_action: EventHandler<CompensationAction>,
) -> Element {
    let files = use_context::<Signal<Option<FcsFiles>>>();
    let compensation = use_context::<Signal<Compensation>>();
    let mut pasting = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let answer = match &applied {
        Applied::Unknown => None,
        Applied::Nothing => Some("no compensation applied in Omiq".to_string()),
        Applied::Matrix { path: None, .. } => Some("compensation applied in Omiq".to_string()),
        Applied::Matrix {
            path: Some(path), ..
        } => Some(format!(
            "compensation applied in Omiq ({})",
            file_name(path)
        )),
        Applied::Unreadable { path, why } => Some(format!(
            "compensation applied in Omiq, but {} can't be read: {why}",
            file_name(path)
        )),
    };

    rsx! {
        if let Some(answer) = answer {
            div { class: "workspace-comp-answer",
                span { "Omiq exported FCS: {answer}." }
                button {
                    disabled: busy,
                    onclick: move |_| on_action.call(CompensationAction::AppliedForget(group)),
                    "Change"
                }
            }
        } else {
            div { class: "workspace-confirm",
                p { strong { "Omiq exported FCS detected." } }
                div { class: "workspace-buttons",
                    button {
                        disabled: busy,
                        onclick: move |_| {
                            pasting.set(false);
                            on_action.call(CompensationAction::AppliedNothing(group));
                        },
                        "No compensation applied in Omiq"
                    }
                    button {
                        class: if pasting() { "workspace-primary" } else { "" },
                        disabled: busy,
                        onclick: move |_| {
                            error.set(None);
                            pasting.set(true);
                        },
                        "Compensation applied in Omiq"
                    }
                }
                if pasting() {
                    PasteBox {
                        prompt: "Export the compensation matrix from Omiq and paste it here.",
                        error: error(),
                        busy,
                        on_ok: move |text: String| {
                            let read = compensation
                                .peek()
                                .read_pasted(group, &text, |p| facts(&files.peek(), p));
                            match read {
                                Ok(matrix) => {
                                    on_action.call(CompensationAction::AppliedMatrix(group, Arc::new(matrix)));
                                    pasting.set(false);
                                }
                                Err(why) => error.set(Some(why)),
                            }
                        },
                        on_cancel: move |_| pasting.set(false),
                    }
                }
            }
        }
    }
}

/// A box to paste a matrix into, checked when OK is clicked.
#[component]
fn PasteBox(
    prompt: String,
    error: Option<String>,
    busy: bool,
    on_ok: EventHandler<String>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut text = use_signal(String::new);
    // The text last checked: its error is shown until the text changes.
    let mut checked = use_signal(|| None::<String>);
    let error = error.filter(|_| checked.read().as_deref() == Some(text.read().as_str()));
    rsx! {
        div { class: "workspace-comp-paste",
            p { "{prompt}" }
            textarea {
                rows: 8,
                onmounted: focus,
                spellcheck: false,
                placeholder: "Channel names across the top, values in percent",
                value: "{text}",
                oninput: move |e| text.set(e.value()),
            }
            if let Some(why) = error {
                p { class: "workspace-problem", "{why}" }
            }
            div { class: "workspace-buttons",
                button {
                    class: "workspace-primary",
                    disabled: busy || text().trim().is_empty(),
                    onclick: move |_| {
                        let pasted = text.peek().clone();
                        checked.set(Some(pasted.clone()));
                        on_ok.call(pasted);
                    },
                    "OK"
                }
                button { onclick: move |_| on_cancel.call(()), "Cancel" }
            }
        }
    }
}

/// Pick files - searching, ticking, shift-clicking a run - and move them to
/// a group.
#[component]
fn AssignDialog(
    rows: Vec<AssignRow>,
    groups: Vec<(GroupId, String)>,
    to: GroupId,
    on_assign: EventHandler<(Vec<PathBuf>, Option<GroupId>)>,
    on_close: EventHandler<()>,
) -> Element {
    let mut search = use_signal(String::new);
    let mut picked = use_signal(BTreeSet::<PathBuf>::new);
    // Where a shift-click runs from: the last row clicked, among those shown.
    let mut anchor = use_signal(|| None::<PathBuf>);
    // A group's id, or "new".
    let mut destination = use_signal(move || to.to_string());

    let lowered = search().to_lowercase();
    let shown: Vec<AssignRow> = rows
        .iter()
        .filter(|r| matches_search(&r.name, &lowered) || matches_search(&r.group_name, &lowered))
        .cloned()
        .collect();
    let shown_paths: Vec<PathBuf> = shown.iter().map(|r| r.path.clone()).collect();
    let count = picked.read().len();
    let all_shown_picked =
        !shown.is_empty() && shown.iter().all(|r| picked.read().contains(&r.path));

    rsx! {
        div { class: "workspace-comp-modal-backdrop", onclick: move |_| on_close.call(()),
            div {
                class: "workspace-comp-modal",
                onclick: move |e| e.stop_propagation(),
                h3 { "Assign files to a group" }
                input {
                    class: "workspace-comp-search",
                    r#type: "search",
                    placeholder: "Search files",
                    onmounted: focus,
                    value: "{search}",
                    oninput: move |e| search.set(e.value()),
                }
                div { class: "workspace-buttons",
                    button {
                        disabled: shown.is_empty(),
                        onclick: {
                            let shown_paths = shown_paths.clone();
                            move |_| {
                                let mut p = picked.write();
                                if all_shown_picked {
                                    shown_paths.iter().for_each(|s| {
                                        p.remove(s);
                                    });
                                } else {
                                    p.extend(shown_paths.iter().cloned());
                                }
                            }
                        },
                        if all_shown_picked { "Unselect shown" } else { "Select shown ({shown.len()})" }
                    }
                    button {
                        disabled: count == 0,
                        onclick: move |_| picked.write().clear(),
                        "Clear"
                    }
                    span { class: "workspace-status", "{count} selected · shift-click selects a run" }
                }
                div { class: "workspace-comp-pick",
                    table { class: "workspace-table",
                        thead {
                            tr {
                                th {}
                                th { "File" }
                                th { "Group" }
                            }
                        }
                        tbody {
                            for row in shown.iter() {
                                tr {
                                    key: "{row.path.display()}",
                                    class: if picked.read().contains(&row.path) { "workspace-comp-picked" } else { "" },
                                    onclick: {
                                        let path = row.path.clone();
                                        let shown_paths = shown_paths.clone();
                                        move |e: MouseEvent| {
                                            let from = anchor
                                                .peek()
                                                .as_ref()
                                                .and_then(|a| shown_paths.iter().position(|p| p == a));
                                            let at = shown_paths.iter().position(|p| *p == path);
                                            match (e.modifiers().shift(), from, at) {
                                                (true, Some(a), Some(b)) => {
                                                    let (lo, hi) = (a.min(b), a.max(b));
                                                    picked.write().extend(shown_paths[lo..=hi].iter().cloned());
                                                }
                                                _ => {
                                                    let mut p = picked.write();
                                                    if !p.remove(&path) {
                                                        p.insert(path.clone());
                                                    }
                                                }
                                            }
                                            anchor.set(Some(path.clone()));
                                        }
                                    },
                                    td {
                                        input {
                                            r#type: "checkbox",
                                            tabindex: "-1",
                                            checked: picked.read().contains(&row.path),
                                        }
                                    }
                                    td { title: "{row.path.display()}", "{row.name}" }
                                    td { class: if row.group == to { "" } else { "workspace-dim" }, "{row.group_name}" }
                                }
                            }
                        }
                    }
                    if shown.is_empty() {
                        p { class: "workspace-dim", "No files match." }
                    }
                }
                div { class: "workspace-path",
                    span { "Move to" }
                    select {
                        onchange: move |e| destination.set(e.value()),
                        for (id, name) in groups.iter() {
                            option { value: "{id}", selected: destination() == id.to_string(), "{name}" }
                        }
                        option { value: "new", selected: destination() == "new", "New group" }
                    }
                    button {
                        class: "workspace-primary",
                        disabled: count == 0,
                        onclick: move |_| {
                            let paths: Vec<PathBuf> = picked.peek().iter().cloned().collect();
                            let to = destination.peek().parse::<GroupId>().ok();
                            on_assign.call((paths, to));
                        },
                        if count == 1 { "Assign 1 file" } else { "Assign {count} files" }
                    }
                    button { onclick: move |_| on_close.call(()), "Cancel" }
                }
            }
        }
    }
}

/// A matrix laid out as Omiq shows it - each row the fluorochrome that
/// spills, each column the detector it spills into, in percent - with every
/// entry off the diagonal editable. Entries that differ from `reference` are
/// highlighted.
#[component]
fn MatrixGrid(
    matrix: Arc<Spillover>,
    reference: Option<Arc<Spillover>>,
    spilling_only: bool,
    editable: bool,
    on_set: EventHandler<(String, String, f64)>,
) -> Element {
    let n = matrix.channels().len();
    let mixing: Vec<usize> = match matrix.involved() {
        Some(m) if spilling_only => (0..n)
            .filter(|&i| m.channels().contains(&matrix.channels()[i]))
            .collect(),
        _ => (0..n).collect(),
    };
    let differs = |i: usize, j: usize| -> bool {
        let Some(r) = &reference else {
            return false;
        };
        let (a, b) = (&matrix.channels()[i], &matrix.channels()[j]);
        let at = |c: &Arc<str>| r.channels().iter().position(|x| x == c);
        let was = match (at(a), at(b)) {
            (Some(x), Some(y)) => r.value(x, y),
            _ if i == j => 1.0,
            _ => 0.0,
        };
        (was - matrix.value(i, j)).abs() > 1e-9
    };
    let percent = |v: f64| {
        // To four places: what a file stores as a 32-bit float shows as
        // 30, not 30.0000001.
        let p = (v * 100.0 * 1e4).round() / 1e4;
        let text = format!("{p:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    };

    rsx! {
        div { class: "workspace-comp-grid",
            table {
                thead {
                    tr {
                        th { class: "workspace-comp-corner" }
                        for &j in mixing.iter() {
                            th { class: "workspace-comp-col", title: "{matrix.channels()[j]}",
                                span { "{matrix.channels()[j]}" }
                            }
                        }
                    }
                }
                tbody {
                    for &i in mixing.iter() {
                        tr { key: "{matrix.channels()[i]}",
                            th { class: "workspace-comp-row", "{matrix.channels()[i]}" }
                            for &j in mixing.iter() {
                                td {
                                    class: if differs(i, j) { "workspace-comp-changed" } else { "" },
                                    if i == j {
                                        span { class: "workspace-dim", "100" }
                                    } else {
                                        input {
                                            r#type: "number",
                                            step: "0.1",
                                            disabled: !editable,
                                            title: "{matrix.channels()[i]} into {matrix.channels()[j]}",
                                            value: "{percent(matrix.value(i, j))}",
                                            onchange: {
                                                let from = matrix.channels()[i].to_string();
                                                let into = matrix.channels()[j].to_string();
                                                move |e: FormEvent| {
                                                    if let Ok(v) = e.value().trim().parse::<f64>() {
                                                        on_set.call((from.clone(), into.clone(), v));
                                                    }
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
