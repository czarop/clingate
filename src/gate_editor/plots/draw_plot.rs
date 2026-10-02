#![allow(non_snake_case)]
use std::{ops::RangeInclusive, sync::Arc};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use dioxus::prelude::*;

use flow_plots::{
    BasePlotOptions, ColorMaps, DensityPlot, DensityPlotOptions, Plot, ScatterPlotData,
    render::RenderConfig,
};

use crate::gate_editor::gates::draw_gates::GateLayer;
use clingate_core::AxisInfo;
use clingate_core::axis_store::PlotMapper;

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
        let points = data();
        async move {
            let (x_axis_info, y_axis_info) = (x_axis_info(), y_axis_info());
            let size = size();
            match tokio::task::spawn_blocking(move || {
                render_plot(points, size, x_axis_info, y_axis_info)
            })
            .await
            {
                Ok(r) => r,
                Err(e) => Err(anyhow::anyhow!("Failed to generate plot {}", e)),
            }
        }
    });

    rsx! {
        match &*render_result.read() {
            Some(Ok((data, map))) => {
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
            None => {
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

/// `points` drawn on these axes, as a PNG data URL, and the mapping the gates
/// are drawn with. No points is a plot like any other: the axes, empty, for
/// the gates to be drawn on - a parent gate holding nothing is an answer, not
/// a plot still on its way.
pub(crate) fn render_plot(
    points: Vec<(f32, f32)>,
    (width, height): (u32, u32),
    x_axis_info: AxisInfo,
    y_axis_info: AxisInfo,
) -> anyhow::Result<(String, Arc<PlotMapper>)> {
    let bounds = get_bounds(&points).unwrap_or((
        (x_axis_info.axis_lower, x_axis_info.axis_upper),
        (y_axis_info.axis_lower, y_axis_info.axis_upper),
    ));
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

    // Built from the options the image is drawn with, so the gates map to
    // the plotting area the events are in.
    let mapper = PlotMapper::for_plot(
        &base_options,
        x_axis_options.range.clone(),
        y_axis_options.range.clone(),
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
        .point_size(0.5_f32)
        .build()?;
    let data = ScatterPlotData {
        points,
        gate_ids: None,
        z_values: None,
    };
    let png = plot.render(data, &options, &mut RenderConfig::default())?;
    Ok((
        format!("data:image/png;base64,{}", BASE64_STANDARD.encode(&png)),
        Arc::new(mapper),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use clingate_core::axis_store::Param;
    use flow_fcs::TransformType;

    fn axis(name: &str) -> AxisInfo {
        AxisInfo {
            param: Param {
                marker: Arc::from(name),
                fluoro: Arc::from(name),
            },
            axis_lower: 0.0,
            axis_upper: 1000.0,
            transform: TransformType::Linear,
        }
    }

    #[test]
    fn a_population_with_no_events_is_drawn_as_empty_axes_the_gates_map_onto() {
        let (image, mapper) =
            render_plot(Vec::new(), (300, 300), axis("FSC-A"), axis("SSC-A")).unwrap();
        assert!(image.starts_with("data:image/png;base64,"));
        let with_events = render_plot(
            vec![(100.0, 100.0), (900.0, 900.0)],
            (300, 300),
            axis("FSC-A"),
            axis("SSC-A"),
        )
        .unwrap()
        .1;
        // Gates sit where they would on a plot with events in it.
        for point in [(0.0, 0.0), (500.0, 250.0), (1000.0, 1000.0)] {
            assert_eq!(
                mapper.data_to_pixel(point.0, point.1, None, None),
                with_events.data_to_pixel(point.0, point.1, None, None),
                "{point:?}"
            );
        }
    }
}
