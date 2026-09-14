use crate::file_load::FcsSampleStub;
use crate::gate_editor::gates::gate_store::{
    ComparableGate, GateOverrideResolver, GateStateStoreExt, NodeId,
};
use crate::gate_editor::plots::data_helpers::{
    get_event_mask_from_scaled_df, get_filtered_dataframe, get_flow_data, zip_cols_from_filtered_df,
};
use crate::gate_editor::plots::draw_plot::PseudoColourPlot;
use crate::omiq::metadata::MetaDataStoreStoreExt;

use crate::gate_editor::plots::plot_store::{EventIndexMapped, PlotStore, PlotStoreStoreExt};
use crate::gate_editor::{
    AxisInfo,
    gates::{GateState, gate_store::GateStateImplExt},
    plots::axis_store::{AxisStore, AxisStoreImplExt, AxisStoreStoreExt, Param},
};
use crate::omiq::metadata::MetaDataStore;
use dioxus::{CapturedError, prelude::*};
use polars::frame::DataFrame;

use std::sync::Arc;

#[component]
pub fn PlotWindow(
    sample_stub: ReadSignal<FcsSampleStub>,
    x_axis_marker: ReadSignal<Param>,
    y_axis_marker: ReadSignal<Param>,
    parental_gate: ReadSignal<Option<Arc<str>>>,
) -> Element {
    let mut gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let mut gate_resolver_store: Signal<Option<Arc<GateOverrideResolver>>> = use_signal(|| None);
    use_context_provider(|| gate_resolver_store);

    let plot_store = use_store(PlotStore::default);
    use_context_provider(|| plot_store);

    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();

    let mut axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    // RESOURCE 1: Load FCS File
    let mut fcs_file: SyncSignal<Option<flow_fcs::Fcs>> = use_signal_sync(|| None);
    let _ = use_resource(move || async move {
        let sample_path = sample_stub.read().get_filepath().to_owned();

        let Some(file_name) = sample_path.file_name() else {
            return;
        };
        let Some(file_name) = file_name.to_str() else {
            return;
        };
        let Some(id) = metadata_store
            .file_name_to_gating_id()
            .read()
            .get(file_name)
            .cloned()
        else {
            return;
        };

        match get_flow_data(sample_path).await {
            Ok(f) => {
                *plot_store.current_file_id().write() = id.clone();
                fcs_file.set(Some(f))
            }
            Err(e) => {
                fcs_file.set(None);
                println!("error generating fcs file {}", e);
            }
        }
    });

    // not currently used - but rejig if it is needed!
    // use_effect(move || {
    //     // if let Some(fcs_file) = &*fcs_file.read() {
    //     if let Some(fcs_file) = fcs_file() {
    //         let mut sorted_settings = indexmap::IndexSet::with_hasher(rustc_hash::FxBuildHasher);

    //         // 1. Get the parameters and sort them by their internal FCS parameter number once
    //         let mut params_to_add: Vec<_> = fcs_file.parameters.values().collect();
    //         params_to_add.sort_by_key(|p| p.parameter_number);

    //         // 2. Iterate and update the store
    //         for fcs_param in params_to_add {
    //             let p = Param {
    //                 marker: fcs_param.label_name.clone(),
    //                 fluoro: fcs_param.channel_name.clone(),
    //             };

    //             // Add settings to the FxHashMap if not present
    //             axis_store.add_new_default_axis_settings(&p, &fcs_file);

    //             // Insert into the IndexSet (Order is preserved automatically)
    //             sorted_settings.insert(p.clone());
    //         }

    //         // 3. Update the store's set
    //         *axis_store.sorted_settings().write() = sorted_settings;
    //     }
    // });

    // this is currently scaling the data but filtering is done elsewhere!
    let scaled_data = use_resource(move || async move {
        let mut params: Vec<(Arc<str>, f32)> = Vec::new();
        for (k, v) in axis_store.settings().read().iter() {
            if v.is_arcsinh() {
                params.push((k.clone(), v.get_cofactor().unwrap()))
            }
        }

        if fcs_file.read().is_none() {return Err(anyhow::anyhow!("No data to scale"))};

        let result =
            tokio::task::spawn_blocking(move || -> Result<Arc<DataFrame>, anyhow::Error> {
                let param_refs: Vec<(&str, f32)> =
                    params.iter().map(|(k, v)| (k.as_ref(), *v)).collect();
                let scaled_df = &*fcs_file.read().as_ref().unwrap().apply_arcsinh_transforms(param_refs.as_slice())?;
                let df_with_index = scaled_df.with_row_index("original_index".into(), None)?;

                Ok(Arc::new(df_with_index))
            })
            .await;

        match result {
            Ok(d) => d,
            Err(_) => Err(anyhow::anyhow!("error scaling data")),
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

    let resolver = use_memo(move || {
        let id: Arc<str> = plot_store.current_file_id()();
        let Some(groups) = metadata_store.metadata()().get(&id).cloned() else {
            return Err(CapturedError::from_display(format!(
                "no metadata for file {}",
                id
            )));
        };
        match gate_store.get_current_sample(id.clone(), &groups) {
            Ok(resolver) => {
                gate_resolver_store.set(Some(Arc::new(resolver.clone())));
                Ok(resolver)
            }
            Err(e) => Err(e),
        }
    });

    // The events this plot shows depend on its gating chain and on nothing
    // else. Rebuilding the resolver on every gate write is right - it is how a
    // moved gate reaches the draw layer - but making the frame depend on the
    // whole resolver meant that editing any gate anywhere re-filtered the
    // dataframe and rebuilt the event index, which is the expensive half of
    // drawing a plot. A gate is a gate: editing one must not reload the plot
    // underneath it.
    //
    // This memo is the narrow dependency instead - the resolved gates of this
    // plot's own chain. It re-runs on every gate write, but a memo only
    // notifies its subscribers when the value changes, and `ComparableGate`
    // compares by pointer, so a gate edited off this chain - which includes
    // every gate drawn on this plot - leaves the frame alone. A plot with no
    // parent shows every event, so its chain is empty and no gate edit can
    // touch it.
    //
    // The hierarchy is read to register the dependency, since it decides what
    // the chain is; the chain itself is then computed through `peek`, which
    // would otherwise widen the subscription back to every gate write.
    let chain_gates: Memo<Vec<ComparableGate>> = use_memo(move || {
        // Registers the dependency and releases the guard; the value is not
        // needed here, only the subscription.
        drop(gate_store.hierarchy().read());
        let Some(parent) = parental_gate() else {
            return Vec::new();
        };
        let chain = gate_store.peek().gate_chain_for_node(&NodeId::from(parent));
        let resolved = resolver.read();
        let Ok(active) = resolved.as_ref() else {
            return Vec::new();
        };
        chain
            .iter()
            .filter_map(|id| active.active_gates.get(id).cloned())
            .collect()
    });

    let mut plot_data_signal = use_signal(Vec::new);

    let filtered_dataframe: Resource<std::result::Result<Arc<DataFrame>, anyhow::Error>> =
        use_resource(move || {
            let x_fluoro = x_axis_marker.read().fluoro.clone();
            let y_fluoro = y_axis_marker.read().fluoro.clone();
            let parental = parental_gate();
            plot_store.current_file_id()();
            // Depend on the chain, not on the whole resolver. Moving a gate in
            // this plot's chain still re-filters it - that is what `chain_gates`
            // tracks, and the read has to be here, in the synchronous half of
            // the closure, because a read inside the async block registers no
            // dependency. The resolver is then peeked: `chain_gates` has already
            // registered the only dependency this frame has on it, and reading
            // it here would put every gate edit back on the critical path.
            chain_gates.read();
            let current_resolver = resolver.peek().clone();
            async move {
                let Ok(resolver) = current_resolver else {
                    return Err(anyhow::anyhow!("No resolver"));
                };

                let d = scaled_data.read().as_ref()
                    .and_then(|res| res.as_ref().ok())
                    .cloned();

                let Some(d) = d else { 
                    plot_data_signal.set(vec![]);
                    return Err(anyhow::anyhow!("No data yet"))
                };
                    let filtered_data =
                        match get_filtered_dataframe(d.clone(), parental, resolver).await {
                            Ok(d) => d.clone(),
                            Err(e) => {
                                plot_data_signal.set(vec![]);
                                return Err(anyhow::anyhow!("No data to display {}", e));
                            }
                        };

                    match zip_cols_from_filtered_df(filtered_data.clone(), x_fluoro, y_fluoro).await
                    {
                        Ok(d) => plot_data_signal.set(d),
                        Err(_) => plot_data_signal.set(vec![]),
                    };

                    Ok(filtered_data)
                
            }
        });

    let event_index = use_resource(move || {
        let df_arc = match &*filtered_dataframe.read() {
            Some(Ok(df)) => Some(df.clone()),
            _ => None,
        };
        // Tracked, not peeked. This happens to re-run anyway - the filtered
        // frame above depends on both markers - but relying on that is one
        // refactor away from a stale index built against the wrong columns.
        let x_name = x_axis_marker.read().fluoro.clone();
        let y_name = y_axis_marker.read().fluoro.clone();
        async move {
            let df = match df_arc {
                Some(d) => d,
                None => return Ok(None),
            };

            let join_result =
                tokio::task::spawn_blocking(move || -> anyhow::Result<EventIndexMapped> {
                    // std::thread::sleep(std::time::Duration::from_secs(3));
                    // Build the R-Tree
                    let ei = get_event_mask_from_scaled_df(df.clone(), x_name, y_name)
                        .map_err(|e| anyhow::anyhow!("R-Tree build failed: {e}"))?;
                    // Extract the mapping
                    let map: Vec<usize> = df
                        .column("original_index")?
                        .u32()?
                        .into_iter()
                        .flatten()
                        .map(|v| v as usize)
                        .collect();
                    Ok(EventIndexMapped {
                        event_index: ei,
                        index_map: Arc::new(map),
                    })
                })
                .await;

            match join_result {
                Ok(Ok(index)) => Ok(Some(index)),
                Ok(Err(e)) => {
                    println!("{e}");
                    Err(e)
                }
                Err(join_err) => {
                    println!("{join_err}");
                    Err(anyhow::anyhow!("Task panicked: {join_err}"))
                }
            }
        }
    });

    // for the actual gating, we just use df filtering. however for the currently displayed events we use an EventIndex
    // to get real time % for child gates
    use_effect(move || {
        let data = event_index
            .read()
            .as_ref()
            .and_then(|res| res.as_ref().ok())
            .and_then(|opt| opt.clone());

        *plot_store.event_index_map().write() = data;
    });

    match &*event_index.read() {
        Some(Ok(_)) => {}
        Some(Err(e)) => {
            return rsx! {
                div { class: "spinner-container", "{e}" }
            };
        }
        None => {
            return rsx! {
                div { class: "spinner-container",
                    div { class: "spinner" }
                }
            };
        }
    }

    rsx! {

        div { style: "position: relative; width: 100%; height: 100%;",

            {
                match event_index.state().cloned() {
                    UseResourceState::Pending | UseResourceState::Stopped => rsx! {
                        div { style: "position: absolute; top: 0; left: 0; right: 0; bottom: 0; display: flex; align-items: center; justify-content: center; background-color: rgba(255, 255, 255, 0.6); z-index: 10;",
                            div { class: "spinner" }
                        }
                    },
                    _ => rsx! {},
                }
            }
            {
                let show_plot = resolver.read().is_ok();
                if show_plot {
                    rsx! {
                        PseudoColourPlot {
                            size: (600, 600),
                            data: plot_data_signal,
                            x_axis_info: x_axis_limits.read().clone(),
                            y_axis_info: y_axis_limits.read().clone(),
                            parental_gate_id: parental_gate,
                        }
                    }
                } else {
                    rsx! {}
                }
            }
        
        }
    }

}
