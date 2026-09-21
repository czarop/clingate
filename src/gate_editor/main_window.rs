use crate::gate_editor::gates::gate_buttons::NewGateButtons;
use crate::gate_editor::pairing_controls::PairingColumns;
use crate::gate_editor::plots::axis_store::AxisStore;
use crate::gate_editor::plots::axis_store::AxisStoreImplExt;
use crate::gate_editor::plots::axis_store::AxisStoreStoreExt;
use crate::gate_editor::plots::axis_store::ScalingInfoSource;
use crate::gate_editor::plots::axis_store::{default_axis_params, index_of_fluoro};
use crate::gate_editor::plots::plot_window::PlotWindow;
use crate::gate_editor::plots::sample_pairs::{Pair, pair_files, pair_of};
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::MetaDataImplExt;
use crate::omiq::metadata::MetaDataOrigin;
use crate::omiq::metadata::MetaDataStore;
use crate::omiq::serialise::to_omiq_document;

use crate::omiq::metadata::MetaDataStoreStoreExt;
use crate::searchable_select::SearchableSelectSet;
use crate::{
    file_load::FcsFiles,
    gate_editor::{
        AxisInfo,
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

use std::path::PathBuf;
use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/main_window.css");

/// The plot is 600x600, and the drawing area inside it is inset by the axis
/// labels. Taken from the same helper the gate geometry uses, so the two agree.
const PLOT_SIZE: u32 = 600;
static PLOT_AREA: std::sync::LazyLock<(u32, u32)> = std::sync::LazyLock::new(|| {
    let (x, _) = flow_gates::transforms::get_plotting_area(PLOT_SIZE, PLOT_SIZE);
    (x.start, x.end - x.start)
});

/// The gating file the session started with, so the Load box opens showing
/// what is currently loaded rather than empty.
///
/// The same third line of `file_paths.txt` the startup import reads. A failure
/// here is not worth reporting - it only means the box starts empty, and the
/// startup import will have said so already.
fn loaded_gating_file() -> String {
    std::fs::read_to_string("file_paths.txt")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .nth(2)
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_default()
}

/// Loading and writing gating files, side by side.
///
/// One row rather than two stacked boxes: they are the same kind of action on
/// the same kind of file, and the sample list below needs the height more than
/// either of them does.
#[component]
fn GatingFiles(parental_gate: Signal<Option<Arc<str>>>) -> Element {
    rsx! {
        div { class: "gating-files",
            LoadGatingFile { parental_gate }
            ExportGatingFile {}
        }
    }
}

/// Replace every gate with the ones in another Omiq gating file.
///
/// A replacement, not an addition - see
/// [`GateState::replace_gates_from_file`](crate::gate_editor::gates::gate_store::GateState::replace_gates_from_file).
/// The store builds the new document separately and swaps it in only once it
/// has parsed, so a mistyped path leaves what is on screen alone.
///
/// The selected position is sent back to the root afterwards. It is a node id
/// from the document being discarded, and nothing in the new one answers to it:
/// left alone it would leave every plot filtering through a chain that no longer
/// exists.
#[component]
fn LoadGatingFile(parental_gate: Signal<Option<Arc<str>>>) -> Element {
    let mut gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    let mut path = use_signal(loaded_gating_file);
    let mut result = use_signal(|| None::<Result<String, String>>);
    let mut busy = use_signal(|| false);

    let load = move |_| {
        if busy() {
            return;
        }
        let target = PathBuf::from(path().trim().to_string());
        let metadata = metadata_store.metadata().peek().clone();
        let axes = axis_store.settings().peek().clone();
        if metadata.is_empty() || axes.is_empty() {
            result.set(Some(Err(
                "Load the metadata and scaling files first - gates cannot be placed without them"
                    .to_string(),
            )));
            return;
        }
        busy.set(true);
        result.set(None);

        // Parsed on a worker thread and applied here. A real gating file is
        // hundreds of kilobytes over a few hundred containers, which is long
        // enough to freeze the window if it runs on the renderer.
        spawn(async move {
            let parsed = tokio::task::spawn_blocking(move || {
                GateState::from_gating_file(target.clone(), &metadata, axes)
                    .map(|fresh| (fresh, target))
            })
            .await;

            match parsed {
                Ok(Ok((fresh, target))) => {
                    let count = fresh.gate_count();
                    // The selection has to go before the gates it names do,
                    // so no plot renders against a chain from the old document.
                    parental_gate.set(Some(ROOTGATE.clone()));
                    gate_store.set(fresh);
                    result.set(Some(Ok(format!(
                        "{count} gates from {}",
                        target
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("the file")
                    ))));
                }
                Ok(Err(e)) => result.set(Some(Err(e.to_string()))),
                Err(e) => result.set(Some(Err(format!("load thread failed: {e}")))),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "export-gating",
            label { "Load from" }
            input {
                value: "{path}",
                disabled: busy(),
                oninput: move |e| path.set(e.value()),
            }
            button {
                class: "export-gating_go",
                disabled: busy(),
                title: "Replace every gate with the ones in this file. The positions on screen are discarded.",
                onclick: load,
                if busy() {
                    "Loading..."
                } else {
                    "Replace gates"
                }
            }
            if let Some(outcome) = result() {
                match outcome {
                    Ok(what) => rsx! {
                        span { class: "export-gating_note", "Loaded {what}" }
                    },
                    Err(why) => rsx! {
                        span { class: "export-gating_note export-gating_warn", "{why}" }
                    },
                }
            }
        }
    }
}

/// Write the current gates back out as an Omiq gating file.
///
/// Everything the document needs is already held: the geometry comes from the
/// gates as they stand - per-specimen and per-sample positions included, since
/// the exporter resolves each file through `gate_for_file` - and the dataset,
/// workflow and task ids from the header captured on import. A session that
/// never imported a file has no header, and the error says so rather than
/// writing a document Omiq would reject.
#[component]
fn ExportGatingFile() -> Element {
    let gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    let mut path = use_signal(|| "gating_export.omiqgt".to_string());
    let mut result = use_signal(|| None::<Result<String, String>>);

    rsx! {
        div { class: "export-gating",
            label { "Export to" }
            input {
                value: "{path}",
                oninput: move |e| path.set(e.value()),
            }
            button {
                class: "export-gating_go",
                onclick: move |_| {
                    let target = PathBuf::from(path());
                    let written = (|| -> anyhow::Result<String> {
                        let document = to_omiq_document(
                            &gate_store.read(),
                            &metadata_store.metadata().read(),
                            &axis_store.settings().read(),
                        )?;
                        // Pretty-printed: the first thing anyone does with a
                        // file Omiq rejects is open it and look.
                        std::fs::write(&target, serde_json::to_string_pretty(&document)?)?;
                        Ok(target.display().to_string())
                    })();
                    result
                        .set(
                            Some(match written {
                                Ok(where_to) => Ok(where_to),
                                Err(e) => Err(e.to_string()),
                            }),
                        );
                },
                "Write gating file"
            }
            if let Some(outcome) = result() {
                match outcome {
                    Ok(where_to) => rsx! {
                        span { class: "export-gating_note", "Written to {where_to}" }
                    },
                    Err(why) => rsx! {
                        span { class: "export-gating_note export-gating_warn", "{why}" }
                    },
                }
            }
        }
    }
}

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
    let mut filehandler = use_context::<Signal<Option<FcsFiles>>>();
    let mut message = use_signal(|| None::<String>);

    // Also created by the NavBar layout: the loaded metadata describes the
    // document, and the gate rules tab reads the same file list.
    let mut metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();

    let meta_result = use_resource(move || async move {
        let result = tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let content = std::fs::read_to_string("file_paths.txt")?;
            let second_line = content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .nth(1)
                .ok_or_else(|| anyhow::anyhow!("File does not have a second non-empty line"))?;
            let path = PathBuf::from(second_line);
            metadata_store.set_metadata_from_file(path, "OmiqID", "Filename", MetaDataOrigin::Omiq)
        })
        .await;

        match result {
            Ok(r) => r,
            Err(e) => Err(anyhow::anyhow!("Failed to load metadata from file {}", e)),
        }
    });

    // Created by the NavBar layout and shared with every route under it: the
    // gates are the document, not a property of one screen.
    let mut gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();

    let mut current_gate_type = use_signal(|| PrimaryGateType::Polygon);
    use_context_provider(|| current_gate_type);

    // On the NavBar layout with the others: the scaling belongs to the loaded
    // document, and the gate rules tab has to read events the same way the
    // plots do or the coordinates would not agree.
    let mut axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    let axis_result = use_resource(move || async move {
        let result = tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let content = std::fs::read_to_string("file_paths.txt")?;
            let forth_line = content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .nth(3)
                .ok_or_else(|| anyhow::anyhow!("File does not have a forth non-empty line"))?;
            let path = PathBuf::from(forth_line);
            axis_store.set_axes_from_file(path, ScalingInfoSource::Omiq)
        })
        .await;

        match result {
            Ok(r) => r,
            Err(e) => Err(anyhow::anyhow!(
                "Failed to load axis settings from file {}",
                e
            )),
        }
    });

    let file_result = use_resource(move || async move {
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<FcsFiles> {
            let content = std::fs::read_to_string("file_paths.txt")?;
            let path = content
                .lines()
                .find(|l| !l.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("No path found"))?;

            FcsFiles::create(path.trim())
        })
        .await;

        match result {
            Ok(Ok(files)) => {
                message.set(None);
                filehandler.set(Some(files));
                Ok(())
            }
            Ok(Err(e)) => {
                message.set(Some(e.to_string()));
                Err(e)
            }
            Err(e) => {
                message.set(Some(e.to_string()));
                Err(anyhow::anyhow!("Failed to load files from path {}", e))
            }
        }
    });

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
        let keys: Vec<Arc<str>> = files
            .iter()
            .map(|f| {
                Arc::from(
                    f.get_filepath()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default(),
                )
            })
            .collect();
        pair_files(
            &keys,
            &metadata_store.file_name_to_gating_id().read(),
            &metadata_store.metadata().read(),
            &rules.read().pairing,
        )
    });

    // Move `steps` specimens along, landing on the first file of the one
    // arrived at. Stepping by file would show the same pair twice.
    let mut step_specimen = move |steps: isize| {
        let pairs = pairs.read();
        if pairs.is_empty() {
            return;
        }
        let at = pair_of(&pairs, sample_index()).unwrap_or(0) as isize;
        let count = pairs.len() as isize;
        let next = (at + steps).rem_euclid(count) as usize;
        if let Some(first) = pairs[next].left() {
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
    let mut axes_initialised = use_signal(|| false);
    use_effect(move || {
        // `peek` on the latch deliberately: this must not re-fire on its own
        // write, only when the channel list changes.
        if *axes_initialised.peek() {
            return;
        }
        let Some((x, y)) = default_axis_params(&axis_store.sorted_settings().read()) else {
            return;
        };
        x_axis_marker.set(x);
        y_axis_marker.set(y);
        axes_initialised.set(true);
    });

    let mut upload_succeded = use_signal(|| false);
    let gate_resource = use_resource(move || {
        // cheap im clones
        let metadata = metadata_store.metadata().read().clone();
        let axis_settings = axis_store.settings().read().clone();
        async move {
            if *upload_succeded.peek() {
                return Ok(());
            }

            if metadata.is_empty() || axis_settings.is_empty() {
                return Err(anyhow::anyhow!("Metadata or Axis settings are empty"));
            }

            let result = tokio::task::spawn_blocking(move || {
                let content = std::fs::read_to_string("file_paths.txt")
                    .map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))?;

                let path_str = content
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .nth(2)
                    .ok_or_else(|| anyhow::anyhow!("File does not have a third line"))?;

                let path = PathBuf::from(path_str);

                gate_store
                    .upload_gates_from_file(path, &metadata, axis_settings)
                    .map_err(|e| anyhow::anyhow!("Upload failed: {}", e))
            })
            .await;

            // 5. Handle the thread result and update UI signals
            match result {
                Ok(Ok(_)) => {
                    upload_succeded.set(true);
                    Ok(())
                }
                Ok(Err(e)) => {
                    println!("{e}");
                    Err(e)
                }
                Err(e) => Err(anyhow::anyhow!("Thread joined with error: {}", e)),
            }
        }
    });

    let parental_gate: Signal<Option<Arc<str>>> = use_signal(|| Some(ROOTGATE.clone()));

    rsx! {
        document::Stylesheet { href: CSS_STYLE }
        div { class: "sidebar-local",

            match &*meta_result.read() {
                Some(Ok(())) => {}
                Some(Err(e)) => return rsx! {
                    div { class: "spinner-container", "{e}" }
                },
                None => return rsx! {
                    div { class: "spinner-container",
                        div { class: "spinner" }
                    }
                },
            }

            match &*file_result.read() {
                Some(Ok(())) => {}
                Some(Err(e)) => return rsx! {
                    div { class: "spinner-container", "{e}" }
                },
                None => return rsx! {
                    div { class: "spinner-container",
                        div { class: "spinner" }
                    }
                },
            }

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
                            input {
                                r#type: "number",
                                value: "{x_axis_limits.read().get_cofactor().unwrap_or_default().round()}",
                                disabled: x_axis_limits.read().is_linear(),
                                oninput: move |evt| {
                                    if let Ok(val) = evt.value().parse::<i32>() {
                                        if val >= 1 {
                                            let param = x_axis_marker.peek();
                                            let res = axis_store.update_cofactor(&param.fluoro, val as f32);
                                            match res {
                                                Ok((old, new)) => {
                                                    match gate_store.rescale_gates(&param.fluoro, &old, &new) {
                                                        Ok(_) => message.set(None),
                                                        Err(e) => {
                                                            message.set(Some(e.join("\n")));
                                                        }
                                                    };
                                                }
                                                Err(e) => println!("{e}"),

                                            }
                                        } else {
                                            message
                                                .set(
                                                    Some("Arcsinh cofactor should be a positive integer".to_string()),
                                                );
                                        }
                                    }
                                },
                                step: "any",
                            }
                        }
                        div { class: "input-unit",
                            label { "Lower" }
                            input {
                                r#type: "number",
                                value: "{x_axis_limits.read().get_untransformed_lower().round()}",
                                disabled: x_axis_limits.read().is_linear(),
                                oninput: move |e| {
                                    if let Ok(lower) = e.value().parse::<i32>() {
                                        let param = x_axis_marker.peek();
                                        match axis_store.update_lower(&param.fluoro, lower as f32) {
                                            Ok(l_u_t) => {
                                                match gate_store
                                                    .set_current_axis_limits(
                                                        param.fluoro.clone(),
                                                        l_u_t.0,
                                                        l_u_t.1,
                                                        l_u_t.2,
                                                    )
                                                {
                                                    Ok(_) => {}
                                                    Err(e) => println!("{:#?}", e),
                                                };
                                            }
                                            Err(e) => println!("{e}"),
                                        };
                                    }
                                },
                            }
                        }
                        div { class: "input-unit",
                            label { "Upper" }
                            input {
                                r#type: "number",
                                value: "{x_axis_limits.read().get_untransformed_upper().round()}",
                                oninput: move |e| {
                                    if let Ok(upper) = e.value().parse::<i32>() {
                                        let param = x_axis_marker.peek();
                                        match axis_store.update_upper(&param.fluoro, upper as f32) {
                                            Ok(l_u_t) => {
                                                match gate_store
                                                    .set_current_axis_limits(
                                                        param.fluoro.clone(),
                                                        l_u_t.0,
                                                        l_u_t.1,
                                                        l_u_t.2,
                                                    )
                                                {
                                                    Ok(_) => {}
                                                    Err(e) => println!("{:#?}", e),
                                                };
                                            }
                                            Err(e) => println!("{e}"),
                                        };
                                    }
                                },
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
                            input {
                                r#type: "number",
                                value: "{y_axis_limits.read().get_cofactor().unwrap_or_default().round()}",
                                disabled: y_axis_limits.read().is_linear(),
                                oninput: move |evt| {
                                    if let Ok(val) = evt.value().parse::<i32>() {
                                        if val >= 1 {
                                            message.set(None);
                                            let param = y_axis_marker.peek();
                                            let res = axis_store.update_cofactor(&param.fluoro, val as f32);
                                            match res {
                                                Ok((old, new)) => {
                                                    match gate_store.rescale_gates(&param.fluoro, &old, &new) {
                                                        Ok(_) => message.set(None),
                                                        Err(e) => {
                                                            message.set(Some(e.join("\n")));
                                                        }
                                                    };
                                                }
                                                Err(e) => println!("{e}"),
                                            }
                                        } else {
                                            message
                                                .set(
                                                    Some("Arcsinh cofactor should be a positive integer".to_string()),
                                                );
                                        }
                                    }
                                },
                                step: "any",
                            }
                        }
                        div { class: "input-unit",
                            label { "Lower" }
                            input {
                                r#type: "number",
                                value: "{y_axis_limits.read().get_untransformed_lower().round()}",
                                disabled: y_axis_limits.read().is_linear(),
                                oninput: move |e| {
                                    if let Ok(lower) = e.value().parse::<i32>() {
                                        let param = y_axis_marker.peek();
                                        match axis_store.update_lower(&param.fluoro, lower as f32) {
                                            Ok(l_u_t) => {
                                                match gate_store
                                                    .set_current_axis_limits(
                                                        param.fluoro.clone(),
                                                        l_u_t.0,
                                                        l_u_t.1,
                                                        l_u_t.2,
                                                    )
                                                {
                                                    Ok(_) => {}
                                                    Err(e) => println!("{:#?}", e),
                                                };
                                            }
                                            Err(e) => println!("{e}"),
                                        };
                                    }
                                },
                            }
                        }
                        div { class: "input-unit",
                            label { "Upper" }
                            input {
                                r#type: "number",
                                value: "{y_axis_limits.read().get_untransformed_upper().round()}",
                                oninput: move |e| {
                                    if let Ok(upper) = e.value().parse::<i32>() {
                                        let param = y_axis_marker.peek();
                                        match axis_store.update_upper(&param.fluoro, upper as f32) {
                                            Ok(l_u_t) => {
                                                match gate_store
                                                    .set_current_axis_limits(
                                                        param.fluoro.clone(),
                                                        l_u_t.0,
                                                        l_u_t.1,
                                                        l_u_t.2,
                                                    )
                                                {
                                                    Ok(_) => {}
                                                    Err(e) => println!("{:#?}", e),
                                                };
                                            }
                                            Err(e) => println!("{e}"),
                                        };
                                    }
                                },
                            }
                        }
                    }
                    div { class: "file-info",
                        PairingColumns {}
                        GatingFiles { parental_gate }
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
                                                sample_index.set(*file);
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
                        // show the same donor and timepoint.
                        let shown = filehandler
                            .read()
                            .as_ref()
                            .map(|files| {
                                let list = files.file_list();
                                let pairs = pairs.read();
                                // By slot, not by position: the FMO's side of
                                // the screen stays the FMO's even for a
                                // specimen that has none, rather than its full
                                // stain sliding over to fill the gap.
                                let slots = pair_of(&pairs, sample_index())
                                    .map(|at| pairs[at].slots.clone())
                                    .unwrap_or_else(|| vec![Some(sample_index())]);
                                slots
                                    .into_iter()
                                    .take(2)
                                    .map(|slot| {
                                        slot.and_then(|i| list.get(i)).map(|stub| {
                                            let name = stub
                                                .get_filepath()
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or_default()
                                                .trim_end_matches(".fcs")
                                                .to_string();
                                            (name, stub.clone())
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            });
                        match shown {
                            Some(shown) if shown.iter().any(Option::is_some) => rsx! {
                                div { class: "gate-window-container",
                                    for (slot , filled) in shown.into_iter().enumerate() {
                                        if let Some((name , sample_stub)) = filled {
                                        div { class: "gate-window", key: "{name}",
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
