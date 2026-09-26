use crate::components::toast::{use_toast, warn};
use crate::gate_editor::gates::gate_buttons::NewGateButtons;
use crate::gate_editor::pairing_controls::PairingColumns;
use crate::gate_editor::plots::axis_store::AxisStore;
use crate::gate_editor::plots::axis_store::AxisStoreImplExt;
use crate::gate_editor::plots::axis_store::AxisStoreStoreExt;
use crate::gate_editor::plots::axis_store::{index_of_fluoro, resolve_axes};
use crate::gate_editor::plots::plot_window::{PLOT_SIZE, PlotWindow};
use crate::gate_editor::plots::sample_pairs::{
    Pair, SecondChoice, landing, pair_files, pair_of, shown,
};
use crate::gate_editor::workspace_window::Generation;
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::MetaDataStore;

use crate::omiq::metadata::MetaDataStoreStoreExt;
use crate::searchable_select::SearchableSelectSet;
use crate::{
    file_load::FcsFiles,
    gate_editor::{
        AxisInfo,
        axis_info::AxisEdit,
        gate_sidebar::GateSidebar,
        gates::{
            GateState,
            gate_store::{GateStateImplExt, ROOTGATE},
            gate_types::PrimaryGateType,
        },
        plots::axis_store::Param,
    },
    searchable_select::SearchableSelectList,
};
use dioxus::prelude::*;

use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/main_window.css");

/// The drawing area inside the plot, inset by the axis labels. Taken from the
/// same helper the gate geometry uses, so the two agree, and from the plot's
/// own size so a change there cannot leave this behind.
static PLOT_AREA: std::sync::LazyLock<(u32, u32)> = std::sync::LazyLock::new(|| {
    let (x, _) = flow_gates::transforms::get_plotting_area(PLOT_SIZE, PLOT_SIZE);
    (x.start, x.end - x.start)
});

/// File indices in the order the pairs put their specimens, so the list and the
/// Next button agree about what comes next.
///
/// `total` caps it to the file list's length; files no pair mentions follow in
/// their own order rather than being dropped from the list.
fn listing_order(pairs: &[Pair], total: usize) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    for pair in pairs {
        for slot in &pair.slots {
            if let Some(i) = slot {
                order.push(*i);
            }
        }
        for i in &pair.files {
            if !order.contains(i) {
                order.push(*i);
            }
        }
    }
    if total != usize::MAX {
        for i in 0..total {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        order.retain(|i| *i < total);
    }
    order
}

#[component]
pub fn MainWindow() -> Element {
    // Created by the NavBar layout: which files are open describes the
    // document, and the gate rules tab counts them to say whether the pairing
    // column actually reaches them.
    let filehandler = use_context::<Signal<Option<FcsFiles>>>();
    // Reports of what just happened. This used to be a signal that nothing
    // rendered, so an axis rescale that failed said nothing at all.
    let toasts = use_toast();

    // Also created by the NavBar layout: the loaded metadata describes the
    // document, and the gate rules tab reads the same file list.
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();

    // Created by the NavBar layout and shared with every route under it: the
    // gates are the document, not a property of one screen.
    let mut gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();

    let mut current_gate_type = use_signal(|| PrimaryGateType::Polygon);
    use_context_provider(|| current_gate_type);

    // On the NavBar layout with the others: the scaling belongs to the loaded
    // document, and the gate rules tab has to read events the same way the
    // plots do or the coordinates would not agree.
    let mut axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    let mut sample_index = use_signal(|| 0);

    // The folder grouped into the pairs a person actually compares - the FMO
    // beside its full stain - rather than whatever order the files happen to
    // sit in. `sample_index` stays a file index so picking one off the list
    // still works; the buttons below step a specimen at a time.
    let rules = use_context::<Signal<RuleStore>>();
    let pairs = use_memo(move || {
        let Some(files) = filehandler.read().as_ref().map(|f| f.file_list().to_vec()) else {
            return Vec::<Pair>::new();
        };
        // The name plot_window looks a file up by, so the two agree.
        let keys: Vec<Arc<str>> = files.iter().map(|f| f.name.clone()).collect();
        pair_files(
            &keys,
            &metadata_store.file_name_to_gating_id().read(),
            &metadata_store.metadata().read(),
            &rules.read().pairing,
        )
    });

    // Which of a specimen's other files the second plot shows, as chosen
    // from the list or the selector above that plot. Kept as a type and a
    // place among that type's files rather than as a file, so it carries to
    // the next specimen: see `SecondChoice`.
    let mut second = use_signal(|| None::<SecondChoice>);

    // Select a file, and if it is not its specimen's first-plot file, make it
    // the second plot's choice from now on.
    let mut select_file = move |file: usize| {
        sample_index.set(file);
        let pairs = pairs.read();
        if let Some(pair) = pair_of(&pairs, file).map(|at| &pairs[at])
            && pair.left() != Some(file)
        {
            second.set(pair.choice_of(file));
        }
    };

    // Move `steps` specimens along, landing where the second plot's choice
    // can be shown beside the first. Stepping by file would show the same
    // pair twice.
    let mut step_specimen = move |steps: isize| {
        let at = landing(&pairs.read(), sample_index(), steps, second.read().as_ref());
        if let Some(first) = at {
            sample_index.set(first);
        }
    };

    let mut x_axis_marker: Signal<Param> = use_signal(|| {
        let p: Arc<str> = Arc::from("FSC-A");
        Param {
            marker: p.clone(),
            fluoro: p,
        }
    });
    let mut y_axis_marker = use_signal(|| {
        let p: Arc<str> = Arc::from("SSC-A");
        Param {
            marker: p.clone(),
            fluoro: p,
        }
    });

    // fetch the axis limits from the settings dict when axis changed
    let x_axis_limits = use_memo(move || {
        let param = x_axis_marker.read();
        match axis_store.settings().read().get(&param.fluoro) {
            Some(d) => d.clone(),
            None => AxisInfo::default(),
        }
    });

    let y_axis_limits = use_memo(move || {
        let param = y_axis_marker.read();
        match axis_store.settings().read().get(&param.fluoro) {
            Some(d) => d.clone(),
            None => AxisInfo::default(),
        }
    });

    // One of an axis's boxes was committed: check the number, apply it, and
    // carry the gates on the axis with it. Says whether it was taken; a box
    // whose number was not goes back to the axis as it is.
    //
    // Checked before anything is written. An upper limit below the lower one
    // used to go straight into the store and then into every quadrant on the
    // axis, whose layout clamps into the range - and `f32::clamp` panics on a
    // range the wrong way round, so the app went down (B-AX-1).
    let mut edit_axis = move |marker: Signal<Param>, edit: AxisEdit, value: f64| -> bool {
        let channel = marker.peek().fluoro.clone();
        let Some(current) = axis_store.settings().peek().get(&channel).cloned() else {
            warn(&toasts, format!("{channel} has no scaling loaded"));
            return false;
        };
        if let Err(why) = current.edited(edit, value) {
            warn(
                &toasts,
                format!(
                    "The {} was not changed - {why}. It stays at {}.",
                    edit.name(),
                    current.shown(edit)
                ),
            );
            return false;
        }
        let carried = match edit {
            AxisEdit::Cofactor => match axis_store.update_cofactor(&channel, value as f32) {
                Ok((old, new)) => gate_store.rescale_gates(&channel, &old, &new),
                Err(e) => {
                    warn(&toasts, e.to_string());
                    return false;
                }
            },
            AxisEdit::Lower | AxisEdit::Upper => {
                let updated = if edit == AxisEdit::Lower {
                    axis_store.update_lower(&channel, value as f32)
                } else {
                    axis_store.update_upper(&channel, value as f32)
                };
                match updated {
                    Ok((lower, upper, transform)) => {
                        gate_store.set_current_axis_limits(channel.clone(), lower, upper, transform)
                    }
                    Err(e) => {
                        warn(&toasts, e.to_string());
                        return false;
                    }
                }
            }
        };
        if let Err(errors) = carried {
            warn(
                &toasts,
                format!(
                    "Some gates could not follow the new {}: {}",
                    edit.name(),
                    errors.join("; ")
                ),
            );
        }
        true
    };

    // The scaling export arrives asynchronously, so on the first render the
    // store is still empty. These memos must therefore *subscribe* to it:
    // `peek` does not, so the index stayed pinned at its first value - 0 - and
    // both selectors displayed whichever channel happened to be listed first,
    // whatever the axes were actually set to.
    //
    // Matched on the channel, not the whole `Param`: `Param` compares on marker
    // and channel both, and the marker name comes from the scaling export, so a
    // key built from the channel alone can never match by equality.
    let x_axis_selected_index = use_memo(move || {
        let curr = x_axis_marker.read().fluoro.clone();
        index_of_fluoro(&axis_store.sorted_settings().read(), &curr).unwrap_or(0)
    });
    let y_axis_selected_index = use_memo(move || {
        let curr = y_axis_marker.read().fluoro.clone();
        index_of_fluoro(&axis_store.sorted_settings().read(), &curr).unwrap_or(0)
    });

    // Pick the opening axes once, when the scaling export lands. Only while the
    // user has not chosen any: switching files must keep the axes and the
    // selected gate where they are, so this deliberately never runs again.
    // Settle the axes whenever the scaling changes: keep each one's channel if
    // the new scaling has it, and fall back to the defaults if not. This used
    // to be a latch that picked the defaults once and never ran again, which
    // was right while the scaling could only load once. It can be replaced
    // now, and a replacement must neither throw away the axes a person chose
    // nor leave one pointing at a channel that is gone. See `resolve_axes`.
    //
    // Subscribes to the channel list only: the markers are peeked, so this
    // does not re-run on its own writes, or when a person picks an axis.
    use_effect(move || {
        let Some((x, y)) = resolve_axes(
            &axis_store.sorted_settings().read(),
            &x_axis_marker.peek(),
            &y_axis_marker.peek(),
        ) else {
            return;
        };
        if *x_axis_marker.peek() != x {
            x_axis_marker.set(x);
        }
        if *y_axis_marker.peek() != y {
            y_axis_marker.set(y);
        }
    });

    let mut parental_gate: Signal<Option<Arc<str>>> = use_signal(|| Some(ROOTGATE.clone()));

    // What this tab holds that names the old workspace. Memos first, so each
    // reset fires only on its own count - a file added to the list must not
    // throw away the gate a person is looking at.
    let generation = use_context::<Signal<Generation>>();
    let document = use_memo(move || generation.read().document);
    let file_list = use_memo(move || generation.read().files);
    // The selected gate is a node id from the document just discarded, and
    // nothing in the new one answers to it: left alone, every plot would
    // filter through a chain that no longer exists.
    use_effect(move || {
        document();
        parental_gate.set(Some(ROOTGATE.clone()));
    });
    // An index into a list that has changed may name another file, or none.
    use_effect(move || {
        file_list();
        sample_index.set(0);
    });

    // Nothing to edit until the workspace has files and the metadata that
    // says which sample each one is. Said, rather than left as a spinner that
    // never finishes.
    if filehandler
        .read()
        .as_ref()
        .is_none_or(|f| f.sample_count() == 0)
    {
        return rsx! {
            document::Stylesheet { href: CSS_STYLE }
            div { class: "spinner-container",
                "No FCS files are loaded - open a workspace on the first tab."
            }
        };
    }
    if metadata_store.metadata().read().is_empty() {
        return rsx! {
            document::Stylesheet { href: CSS_STYLE }
            div { class: "spinner-container",
                "No metadata is loaded - choose it on the first tab. The editor finds each file's sample through it."
            }
        };
    }

    rsx! {
        document::Stylesheet { href: CSS_STYLE }
        div { class: "sidebar-local",

            GateSidebar {
                selected_id: parental_gate,
                x_axis_param: x_axis_marker,
                y_axis_param: y_axis_marker,
            }

            main { class: "main-content",

                // The top bar: axis controls on the left, sample selection on
                // the right. Its own class - `gate-window` stacks a plot under
                // its title, which is the opposite of what this row wants.
                div { class: "controls-row",

                    div { class: "axis-controls-grid", style: "width: 600px;",
                        div { class: "grid-label", "X-Axis" }
                        SearchableSelectSet {
                            items: axis_store.sorted_settings()(),
                            on_select: move |(_, k): (_, Param)| {
                                x_axis_marker.set(k.clone());
                            },
                            placeholder: x_axis_marker.peek().to_string(),
                            selected_index: Some(x_axis_selected_index.into()),
                        }

                        div { class: "input-unit",
                            label { "Cofactor" }
                            CommittedNumber {
                                value: x_axis_limits.read().shown(AxisEdit::Cofactor),
                                disabled: x_axis_limits.read().is_linear(),
                                commit: move |v: f64| edit_axis(x_axis_marker, AxisEdit::Cofactor, v),
                            }
                        }
                        div { class: "input-unit",
                            label { "Lower" }
                            CommittedNumber {
                                value: x_axis_limits.read().shown(AxisEdit::Lower),
                                disabled: x_axis_limits.read().is_linear(),
                                commit: move |v: f64| edit_axis(x_axis_marker, AxisEdit::Lower, v),
                            }
                        }
                        div { class: "input-unit",
                            label { "Upper" }
                            CommittedNumber {
                                value: x_axis_limits.read().shown(AxisEdit::Upper),
                                commit: move |v: f64| edit_axis(x_axis_marker, AxisEdit::Upper, v),
                            }
                        }

                        div { class: "grid-label", "Y-Axis" }
                        SearchableSelectSet {
                            items: axis_store.sorted_settings()(),
                            on_select: move |(_, k): (_, Param)| {
                                // if let Some(axis) = axis_store.settings().peek().get(&k.clone()) {
                                y_axis_marker.set(k.clone());
                                // }
                            },
                            placeholder: y_axis_marker.peek().to_string(),
                            selected_index: Some(y_axis_selected_index.into()),
                        }

                        div { class: "input-unit",
                            label { "Cofactor" }
                            CommittedNumber {
                                value: y_axis_limits.read().shown(AxisEdit::Cofactor),
                                disabled: y_axis_limits.read().is_linear(),
                                commit: move |v: f64| edit_axis(y_axis_marker, AxisEdit::Cofactor, v),
                            }
                        }
                        div { class: "input-unit",
                            label { "Lower" }
                            CommittedNumber {
                                value: y_axis_limits.read().shown(AxisEdit::Lower),
                                disabled: y_axis_limits.read().is_linear(),
                                commit: move |v: f64| edit_axis(y_axis_marker, AxisEdit::Lower, v),
                            }
                        }
                        div { class: "input-unit",
                            label { "Upper" }
                            CommittedNumber {
                                value: y_axis_limits.read().shown(AxisEdit::Upper),
                                commit: move |v: f64| edit_axis(y_axis_marker, AxisEdit::Upper, v),
                            }
                        }
                    }
                    div { class: "file-info",
                        PairingColumns {}
                        div { class: "file-info_button-panel",
                            button { onclick: move |_| step_specimen(-1), "Prev" }
                            button { onclick: move |_| step_specimen(1), "Next" }
                        }
                        // Listed in the specimen order the plots step through,
                        // not the folder's. A list that disagrees with the Next
                        // button about what comes next is worse than either
                        // order on its own.
                        match &*filehandler.read() {
                            Some(fh) => {
                                let names = fh.get_file_names();
                                let order = listing_order(&pairs.read(), names.len());
                                let listed: Vec<String> = order
                                    .iter()
                                    .filter_map(|i| names.get(*i).cloned())
                                    .collect();
                                let to_file = order.clone();
                                let shown_at = use_memo(move || {
                                    listing_order(&pairs.read(), usize::MAX)
                                        .iter()
                                        .position(|i| *i == sample_index())
                                        .unwrap_or(0)
                                });
                                rsx! {
                                    SearchableSelectList {
                                        items: listed,
                                        on_select: move |(i, _)| {
                                            if let Some(file) = to_file.get(i) {
                                                select_file(*file);
                                            }
                                        },
                                        placeholder: "Select a file".to_string(),
                                        selected_index: Some(shown_at.into()),
                                    }
                                }
                            }
                            None => rsx! {},
                        }

                    }
                }

                div {
                    div { class: "new-gate-pane",
                        NewGateButtons { callback: move |gate_type| current_gate_type.set(gate_type) }
                    }
                    {
                        // The specimen holding the selected file, so both plots
                        // show the same donor and timepoint - and the selected
                        // file itself, whichever of the specimen's it is. See
                        // `sample_pairs::shown`.
                        let label = |stub: &crate::file_load::FcsSampleStub| {
                            stub.name().trim_end_matches(".fcs").to_string()
                        };
                        let shown = filehandler.read().as_ref().map(|files| {
                            let list = files.file_list();
                            let on = shown(&pairs.read(), sample_index(), second.read().as_ref());
                            let plots = [on.left, on.right]
                                .into_iter()
                                .map(|slot| {
                                    slot.and_then(|i| list.get(i))
                                        .map(|stub| (label(stub), stub.clone()))
                                })
                                .collect::<Vec<_>>();
                            // What the selector above the second plot offers:
                            // the specimen's other files, when there is more
                            // than one to choose between.
                            let choices: Vec<(usize, String)> = if on.choices.len() > 1 {
                                on.choices
                                    .iter()
                                    .filter_map(|i| list.get(*i).map(|stub| (*i, label(stub))))
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            (plots, choices, on.right)
                        });
                        match shown {
                            Some((shown, choices, right)) if shown.iter().any(Option::is_some) => rsx! {
                                div { class: "gate-window-container",
                                    for (slot , filled) in shown.into_iter().enumerate() {
                                        if let Some((name , sample_stub)) = filled {
                                        div { class: "gate-window", key: "{name}",
                                            if slot == 1 && !choices.is_empty() {
                                                // Which of the specimen's other
                                                // files this plot shows. The
                                                // choice is kept for the next
                                                // specimen too.
                                                select {
                                                    class: "gate-window_choice",
                                                    style: "margin-left: {PLOT_AREA.0}px; width: {PLOT_AREA.1}px;",
                                                    title: "Which of this specimen's other files to show here",
                                                    value: "{right.unwrap_or_default()}",
                                                    onchange: move |e| {
                                                        if let Ok(file) = e.value().parse::<usize>() {
                                                            select_file(file);
                                                        }
                                                    },
                                                    for (file , name) in choices.iter() {
                                                        option {
                                                            key: "{file}",
                                                            value: "{file}",
                                                            selected: Some(*file) == right,
                                                            "{name}"
                                                        }
                                                    }
                                                }
                                            }
                                            // Centred over the data area rather
                                            // than the image: the plot carries
                                            // a label gutter down its left
                                            // side, so centring on the whole
                                            // width would sit the name visibly
                                            // right of the cloud it names.
                                            div {
                                                class: "gate-window_title",
                                                style: "margin-left: {PLOT_AREA.0}px; width: {PLOT_AREA.1}px;",
                                                title: "{name}",
                                                "{name}"
                                            }
                                            PlotWindow {
                                                sample_stub,
                                                x_axis_marker,
                                                y_axis_marker,
                                                parental_gate,
                                            }
                                        }
                                        } else {
                                            // An empty slot, not a missing
                                            // plot: it holds its side of the
                                            // screen so the one beside it stays
                                            // where it belongs.
                                            div { class: "gate-window gate-window_empty", key: "empty-{slot}",
                                                div { class: "gate-window_title", "" }
                                                span { class: "gate-window_none", "no paired file" }
                                            }
                                        }
                                    }
                                }
                            },
                            _ => rsx! { "No directory selected" },
                        }
                    }
                }

            }
        }
    }
}

/// A number box that is applied when the person has finished with it - on
/// Enter, or on leaving the box - rather than at every keystroke.
///
/// Every keystroke used to be applied. Typing an upper limit of 400000 applied
/// 4, 40, 400 and on as it went, and each clamped the quadrants on the axis
/// into that range, where they stayed (B-AX-2). The box's change event fires
/// on Enter and on leaving the box, which is when a person means the number;
/// a delay was the other choice, and would still have applied a half-typed
/// number whenever someone paused mid-way.
///
/// `commit` says whether it took the number. When it did not, the box goes
/// back to `value`. That needs the box remounted: its `value` has not
/// changed, so it would not be written to the box again, which would go on
/// showing the number that was refused.
#[component]
fn CommittedNumber(
    value: f64,
    #[props(default = false)] disabled: bool,
    commit: Callback<f64, bool>,
) -> Element {
    let toasts = use_toast();
    let mut remounts = use_signal(|| 0u32);
    rsx! {
        for key in [remounts()] {
            input {
                key: "{key}",
                r#type: "number",
                step: "any",
                value: "{value}",
                disabled,
                onchange: move |e| {
                    let taken = match e.value().trim().parse::<f64>() {
                        Ok(typed) => commit.call(typed),
                        Err(_) => {
                            warn(&toasts, format!("That is not a number - it stays at {value}."));
                            false
                        }
                    };
                    if !taken {
                        remounts += 1;
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::listing_order;
    use crate::gate_editor::plots::sample_pairs::Pair;
    use std::sync::Arc;

    fn pair(files: &[usize], slots: &[Option<usize>]) -> Pair {
        Pair {
            specimen: Some(Arc::from("s")),
            files: files.to_vec(),
            kinds: vec![None; files.len()],
            slots: slots.to_vec(),
        }
    }

    #[test]
    fn the_list_follows_the_pairs_slot_by_slot() {
        // Specimens in pair order, each shown left file then right.
        let pairs = vec![
            pair(&[3, 0], &[Some(3), Some(0)]),
            pair(&[2, 1], &[Some(2), Some(1)]),
        ];
        assert_eq!(listing_order(&pairs, 4), vec![3, 0, 2, 1]);
    }

    #[test]
    fn a_file_in_no_slot_follows_its_specimen() {
        // A file the slots leave out is still listed, after its specimen's
        // slotted files - not dropped, and not moved to the end.
        let pairs = vec![
            pair(&[0, 1, 2], &[Some(0), Some(1)]),
            pair(&[3], &[None, Some(3)]),
        ];
        assert_eq!(listing_order(&pairs, 4), vec![0, 1, 2, 3]);
    }

    #[test]
    fn files_no_pair_mentions_are_listed_last_and_the_total_caps_it() {
        let pairs = vec![pair(&[1], &[Some(1), None])];
        assert_eq!(listing_order(&pairs, 3), vec![1, 0, 2]);
        // An index past the file list - pairs from a longer, older list - is
        // dropped rather than offered.
        let stale = vec![pair(&[5, 0], &[Some(5), Some(0)])];
        assert_eq!(listing_order(&stale, 2), vec![0, 1]);
        // Uncapped, nothing is added or removed.
        assert_eq!(listing_order(&stale, usize::MAX), vec![5, 0]);
    }

    #[test]
    fn every_file_is_listed_exactly_once() {
        use rand::prelude::*;
        for seed in 0..300 {
            let mut rng = StdRng::seed_from_u64(seed);
            let total = rng.random_range(0..12);
            let mut indices: Vec<usize> = (0..total).collect();
            indices.shuffle(&mut rng);
            // Split a random subset of the files into random pairs; the rest
            // no pair mentions.
            let kept = rng.random_range(0..=total);
            let mut pairs = Vec::new();
            let mut rest = &indices[..kept];
            while !rest.is_empty() {
                let take = rng.random_range(1..=rest.len().min(3));
                let (files, tail) = rest.split_at(take);
                let slots = files
                    .iter()
                    .map(|f| rng.random_bool(0.7).then_some(*f))
                    .collect::<Vec<_>>();
                pairs.push(pair(files, &slots));
                rest = tail;
            }
            let mut listed = listing_order(&pairs, total);
            listed.sort();
            assert_eq!(listed, (0..total).collect::<Vec<_>>(), "seed {seed}");
        }
    }
}
