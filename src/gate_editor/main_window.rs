use crate::gate_editor::gates::gate_buttons::NewGateButtons;
use crate::gate_editor::plots::axis_store::AxisStore;
use crate::gate_editor::plots::axis_store::AxisStoreImplExt;
use crate::gate_editor::plots::axis_store::AxisStoreStoreExt;
use crate::gate_editor::plots::axis_store::ScalingInfoSource;
use crate::gate_editor::plots::axis_store::{default_axis_params, index_of_fluoro};
use crate::gate_editor::plots::plot_window::PlotWindow;
use crate::omiq::metadata::MetaDataImplExt;
use crate::omiq::metadata::MetaDataOrigin;
use crate::omiq::metadata::MetaDataStore;

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
use dioxus::stores::use_store_sync;

use std::path::PathBuf;
use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/main_window.css");

#[component]
pub fn MainWindow() -> Element {
    let mut filehandler: Signal<Option<FcsFiles>> = use_signal(|| None);
    let mut message = use_signal(|| None::<String>);

    let mut metadata_store = use_store_sync(MetaDataStore::default);
    use_context_provider(|| metadata_store);

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

    let mut axis_store: Store<AxisStore, CopyValue<AxisStore, SyncStorage>> = use_store_sync(AxisStore::default);
    use_context_provider(|| axis_store);

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
            Err(e) => Err(anyhow::anyhow!("Failed to load axis settings from file {}", e)),
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
                
                let path_str = content.lines()
                    .filter(|l| !l.trim().is_empty())
                    .nth(2)
                    .ok_or_else(|| anyhow::anyhow!("File does not have a third line"))?;
                
                let path = PathBuf::from(path_str);

                gate_store.upload_gates_from_file(path, &metadata, axis_settings)
                    .map_err(|e| anyhow::anyhow!("Upload failed: {}", e))

            }).await;

            // 5. Handle the thread result and update UI signals
            match result {
                Ok(Ok(_)) => {
                    upload_succeded.set(true);
                    Ok(())
                }
                Ok(Err(e)) => {
                    println!("{e}");
                    Err(e)
                },
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

                div { class: "gate-window",

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
                        div { class: "file-info_button-panel",
                            button {
                                onclick: move |_| {
                                    if let Some(fcsfiles) = &*filehandler.read() {
                                        let count = fcsfiles.sample_count();
                                        let prev_index = (*sample_index.read() + count - 1) % count;
                                        sample_index.set(prev_index);
                                    }

                                },
                                "Prev"
                            }
                            button {
                                onclick: move |_| {
                                    if let Some(fcsfiles) = &*filehandler.read() {
                                        let next_index = (*sample_index.read() + 1) % fcsfiles.sample_count();
                                        sample_index.set(next_index);
                                    }

                                },
                                "Next"
                            }
                        }
                        match &*filehandler.read() {
                            Some(fh) => {
                                let list = fh.get_file_names();
                                rsx! {
                                    SearchableSelectList {
                                        items: list,
                                        on_select: move |(i, _)| { sample_index.set(i) },
                                        placeholder: "Select a file".to_string(),
                                        selected_index: Some(sample_index.into()),
                                    }
                                }
                            }
                            None => rsx! {},
                        }
                    
                    }
                }

                div {
                    NewGateButtons { callback: move |gate_type| current_gate_type.set(gate_type) }
                    {
                        let maybe_stubs = filehandler
                            .read()
                            .as_ref()
                            .map(|files| {
                                let list = files.file_list();
                                let idx = sample_index();
                                let idx2 = (idx + 1) % list.len();
                                (list[idx].clone(), list[idx2].clone())
                            });
                        if let Some((sample_stub, sample_stub2)) = maybe_stubs {
                            rsx! {
                                div { class: "gate-window-container",
                                    div { class: "gate-window",
                                        PlotWindow {
                                            sample_stub,
                                            x_axis_marker,
                                            y_axis_marker,
                                            parental_gate,
                                        }
                                    }
                                    div { class: "gate-window",
                                        PlotWindow {
                                            sample_stub: sample_stub2,
                                            x_axis_marker,
                                            y_axis_marker,
                                            parental_gate,
                                        }
                                    }
                                }
                            }
                        } else {
                            rsx! { "No directory selected" }
                        }
                    }
                }
            
            }
        }
    }
}
