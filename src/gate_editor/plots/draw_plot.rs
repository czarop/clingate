#![allow(non_snake_case)]
use std::{ops::RangeInclusive, sync::Arc};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use dioxus::prelude::*;

use flow_plots::{
    BasePlotOptions, ColorMaps, DensityPlot, DensityPlotOptions, Plot, ScatterPlotData,
    render::RenderConfig,
};

use crate::gate_editor::{AxisInfo, gates::draw_gates::GateLayer, plots::axis_store::PlotMapper};

#[component]
pub fn PseudoColourPlot(
    data: ReadSignal<Vec<(f32, f32)>>,
    size: ReadSignal<(u32, u32)>,
    x_axis_info: ReadSignal<AxisInfo>,
    y_axis_info: ReadSignal<AxisInfo>,
    parental_gate_id: ReadSignal<Option<Arc<str>>>,
) -> Element {
    // let mut plot_image_src = use_signal(|| String::new());
    let mut plot_map = use_signal(|| None::<Arc<PlotMapper>>);
    use_context_provider::<Signal<Option<Arc<PlotMapper>>>>(|| plot_map);

    let render_result = use_resource(move || {
        let data_final: flow_plots::ScatterPlotData = ScatterPlotData {
            points: data(),
            gate_ids: None,
            z_values: None,
        };
        async move {
            let x_axis_info = x_axis_info();
            let y_axis_info = y_axis_info();
            let (width, height) = size();

            // No points is not a failure. The frame can legitimately be empty -
            // the file is still opening, or a parent gate selects nothing - and
            // showing a person the words "could not get bounds" for either is
            // just a rendering step's internals leaking into the plot.
            if data_final.points.is_empty() {
                return Ok(None);
            }

            let result = tokio::task::spawn_blocking(
                move || -> Result<(String, Arc<PlotMapper>), anyhow::Error> {
                    let bounds = get_bounds(&data_final.points)
                        .ok_or_else(|| anyhow::anyhow!("Could not get bounds"))?;
                    let plot = DensityPlot::new();
                    let base_options = BasePlotOptions::new()
                        .width(width)
                        .height(height)
                        .title("My Density Plot")
                        .show_colorbar(false)
                        .build()?;

                    let x_axis_options = flow_plots::AxisOptions::new()
                        .range(x_axis_info.axis_lower..=x_axis_info.axis_upper)
                        .transform(x_axis_info.transform.clone())
                        .label(x_axis_info.param.to_string())
                        .build()?;
                    let y_axis_options = flow_plots::AxisOptions::new()
                        .range(y_axis_info.axis_lower..=y_axis_info.axis_upper)
                        .transform(y_axis_info.transform.clone())
                        .label(y_axis_info.param.to_string())
                        .build()?;

                    let (inc_x, inc_y) = {
                        (
                            *(x_axis_options.range.start())..=*(x_axis_options.range.end()),
                            *(y_axis_options.range.start())..=*(y_axis_options.range.end()),
                        )
                    };

                    let mapper = PlotMapper::new(
                        width as f32,
                        height as f32,
                        inc_x,
                        inc_y,
                        RangeInclusive::new(bounds.0.0, bounds.0.1),
                        RangeInclusive::new(bounds.1.0, bounds.1.1),
                        x_axis_info.transform.clone(),
                        y_axis_info.transform.clone(),
                    );
                    let options = DensityPlotOptions::new()
                        .base(base_options)
                        .plot_type(flow_plots::PlotType::Density)
                        .colormap(ColorMaps::Jet)
                        .x_axis(x_axis_options)
                        .y_axis(y_axis_options)
                        .point_size(0.5)
                        .build()?;

                    let mut render_config = RenderConfig::default();

                    let plot_data = plot.render(data_final, &options, &mut render_config)?;

                    let base64_str = BASE64_STANDARD.encode(&plot_data);
                    Ok((
                        format!("data:image/jpeg;base64,{}", base64_str),
                        Arc::new(mapper),
                    ))
                },
            )
            .await;

            match result {
                Ok(r) => r.map(Some),
                Err(e) => Err(anyhow::anyhow!("Failed to generate plot {}", e)),
            }
        }
    });

    rsx! {
        match &*render_result.read() {
            Some(Ok(Some((data, map)))) => {
                plot_map.set(Some(map.clone()));
                let size = size();
                rsx! {
                    div { style: "position: relative; width: {size.0}px; height: {size.1}px;",
                        img {
                            style: "user-select: none; -webkit-user-select: none;",
                            src: "{data}",
                            width: "{size.0}",
                            height: "{size.1}",
                        }
                        GateLayer {
                            x_channel: x_axis_info().param.fluoro.clone(),
                            y_channel: y_axis_info().param.fluoro.clone(),
                            parental_gate_id,

                        }
                    }
                }

            }
            Some(Err(e)) => {
                rsx! {
                    {e.to_string()}
                }
            }
            // Nothing to draw yet, which looks the same to a person as a plot
            // still being rendered.
            Some(Ok(None)) | None => {
                let size = size();
                let style = format!("width: {}px; height: {}px;", size.0, size.1);
                rsx! {
                    div { style, class: "spinner-container",
                        div { class: "spinner" }
                        span { style: "margin-top: 10px; font-size: 12px; color: #666;", "Rendering Plot..." }
                    }
                }
            }
        }

    }
}

fn get_bounds(data: &[(f32, f32)]) -> Option<((f32, f32), (f32, f32))> {
    if data.is_empty() {
        return None;
    }

    let initial = (
        (data[0].0, data[0].0), // (min_x, max_x)
        (data[0].1, data[0].1), // (min_y, max_y)
    );

    let bounds = data.iter().skip(1).fold(initial, |mut acc, &(x, y)| {
        acc.0.0 = acc.0.0.min(x);
        acc.0.1 = acc.0.1.max(x);
        acc.1.0 = acc.1.0.min(y);
        acc.1.1 = acc.1.1.max(y);
        acc
    });

    Some(bounds)
}
