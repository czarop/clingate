use crate::gate_editor::gates::gate_store::{GateOverrideResolver, GateStateStoreExt};
use crate::gate_editor::plots::axis_store::AxisStore;
use crate::gate_editor::plots::axis_store::AxisStoreStoreExt;
use crate::gate_editor::plots::plot_store::{PlotStore, PlotStoreStoreExt};
use crate::gate_editor::{
    gates::{
        GateState,
        gate_draft::GateDraft,
        gate_drag::{GateDragData, GateDragType, PointDragData, RotationData},
        gate_single::rectangle_gate,
        gate_store::GateStateImplExt,
        gate_traits::DrawableGate,
        gate_types::{Direction, GateRenderShape, GateStats, PrimaryGateType, ShapeType},
    },
    plots::axis_store::PlotMapper,
};
use dioxus::{prelude::*, stores::SyncStore};
use rustc_hash::FxHashMap;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct GateList(Vec<Arc<dyn DrawableGate>>);

impl PartialEq for GateList {
    fn eq(&self, other: &Self) -> bool {
        if self.0.len() != other.0.len() {
            return false;
        }
        // Check if every Arc pointer is identical to the previous one
        self.0
            .iter()
            .zip(other.0.iter())
            .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}
impl Deref for GateList {
    type Target = [Arc<dyn DrawableGate>];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[component]
pub fn GateLayer(
    x_channel: ReadSignal<Arc<str>>,
    y_channel: ReadSignal<Arc<str>>,
    parental_gate_id: ReadSignal<Option<Arc<str>>>,
) -> Element {
    let plot_map = use_context::<Signal<Option<Arc<PlotMapper>>>>();

    let resolver = use_context::<Signal<Option<Arc<GateOverrideResolver>>>>();

    let mut gate_store = use_context::<SyncStore<GateState>>();
    let mut draft_gate_coords = use_signal(Vec::<(f32, f32)>::new);

    let current_gate_type = use_context::<Signal<PrimaryGateType>>();

    let plot_store = use_context::<Store<PlotStore>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    let gates = use_memo(move || {
        let Some(resolver) = resolver() else {
            return GateList(vec![]);
        };
        let (x, y, parent) = (x_channel(), y_channel(), parental_gate_id());
        let g = gate_store
            .get_gates_for_plot(x, y, parent, &resolver)
            .unwrap_or_default();
        GateList(g)
    });

    use_effect(move || {
        println!("matching gates to plot");
        let (x, y, parent) = (x_channel(), y_channel(), parental_gate_id());
        // Peeked deliberately. This re-matches gates to the plot's axes, which
        // depends on their parameters, not their positions - so a gate moving
        // is not a reason to run it again, and tracking the resolver here would
        // re-run it on every drag. Contrast the filtered frame in plot_window,
        // which does have to track the resolver.
        let Some(resolver) = resolver.peek().clone() else {
            return;
        };
        let _ = plot_store.current_file_id();
        let _ = gate_store
            .match_gates_to_plot(x.clone(), y.clone(), parent.clone(), &resolver)
            .inspect_err(|e| println!("{}", e));
    });

    // convert clicked coords into a draft gate
    let draft_gate = use_memo(move || {
        if let PrimaryGateType::Polygon = &*current_gate_type.read() {
            let cur_coords = draft_gate_coords();
            if !cur_coords.is_empty() {
                let gate_draft = GateDraft::new_polygon(cur_coords, x_channel(), y_channel());
                Some(gate_draft)
            } else {
                None
            }
        } else {
            None
        }
    });

    // for editing a gate's points
    let mut drag_data = use_signal(|| Option::<GateDragType>::None);
    use_context_provider::<Signal<Option<GateDragType>>>(|| drag_data);

    // use_effect(move || {
    //     let x_param = x_channel();
    //     let y_param = y_channel();

    // });

    // the list of finalised gates
    // use_effect(move || {
    //     let g = match gate_store.get_gates_for_plot(x_channel(), y_channel(), parental_gate_id()) {
    //         Some(g) => g,
    //         None => vec![],
    //     };
    //     gates.set(g);
    // });

    // add a second part to this that modifies the % if the gate is being dragged or edited - perhaps a second use_resouce?
    let _ = use_resource(move || async move {
        let x_key = x_channel.read().clone();
        let y_key = y_channel.read().clone();
        let Some(resolver) = resolver() else {
            return;
        };
        let event_index_option = plot_store.event_index_map()();
        if let Ok(gates_on_plot) =
            gate_store.get_gates_for_plot(x_key, y_key, parental_gate_id(), &resolver)
        {
            if let Some(event_index_map) = event_index_option {
                let join_result = tokio::task::spawn_blocking(
                    move || -> anyhow::Result<FxHashMap<Arc<str>, GateStats>> {
                        let mut stat_map = FxHashMap::default();
                        let parental_events = event_index_map.event_index.len() as f32;
                        for gate in gates_on_plot {
                            let id = gate.get_id();
                            let stats =
                                crate::gate_editor::gates::gate_stats::get_percent_and_counts_gate(
                                    gate,
                                    &event_index_map,
                                    parental_events,
                                )?;
                            stat_map.insert(id, stats);
                        }

                        Ok(stat_map)
                    },
                )
                .await;

                match join_result {
                    Ok(Ok(index)) => *plot_store.gate_stats().write() = index,
                    Ok(Err(e)) => {
                        println!("{e}");
                        *plot_store.gate_stats().write() = FxHashMap::default();
                    }
                    Err(e) => {
                        println!("{e}");
                        *plot_store.gate_stats().write() = FxHashMap::default();
                    }
                }
            }
        } else {
            println!(
                "no gates to show for {:?}, {:?}, {:?}",
                x_channel(),
                y_channel(),
                parental_gate_id()
            );
        }
    });

    let mut dbl_click_lockout = use_signal(|| false);

    let mut last_processed_pos = use_signal(|| (0.0f64, 0.0f64));
    let mut last_update = use_signal(std::time::Instant::now);
    let mut cached_rect: Signal<
        Option<dioxus_elements::geometry::euclid::Rect<f64, dioxus_elements::geometry::Pixels>>,
    > = use_signal(|| None);

    let mut svg_data: Signal<Option<std::rc::Rc<MountedData>>> = use_signal(|| None);
    let Some(current_resolver_move) = resolver.peek().clone() else {
        return rsx! { "No Gate Resolver" };
    };
    let Some(current_resolver_up) = resolver.peek().clone() else {
        return rsx! { "No Gate Resolver" };
    };

    let Some(mapper) = plot_map.read().clone() else {
        return rsx! { "Loading mapper..." };
    };

    rsx! {

        div {
            style: "position: absolute; top: 0; left: 0; width: 100%; height: 100%;",
            onmounted: move |e| {
                let data = e.data();
                svg_data.set(Some(data.clone()));
            },
            svg {
                width: "100%",
                height: "100%",
                view_box: "0 0 {&mapper.width()} {&mapper.height()}",
                style: "position: absolute; top: 0; left: 0; z-index: 2; user-select: none; -webkit-user-select: none; cursor: crosshair;",
                oncontextmenu: move |evt| evt.prevent_default(),

                onclick: move |evt| {
                    if let Some(mapper) = plot_map() {
                        let local_coords = &evt.data.coordinates().element();
                        let norm_x = local_coords.x as f32;
                        let norm_y = local_coords.y as f32;
                        let (data_x, data_y) = mapper
                            .pixel_to_data(norm_x, norm_y, None, None);

                        let mut clicked_gate = None;
                        let selected_gate_op = gate_store.selected_gate().peek().cloned();

                        let gates = &*gates.read();

                        let current_gate_type = &*current_gate_type.peek();
                        let drag_data = &*drag_data.peek();
                        let draft_gate = &*draft_gate.peek();

                        if draft_gate.is_none() && drag_data.is_none() {
                            clicked_gate = was_gate_clicked((norm_x, norm_y), &mapper, gates);
                        }
                        if let Some(closest_gate) = clicked_gate {
                            let gate_id = closest_gate.get_id().clone();
                            gate_store.selected_gate().set(Some(gate_id.clone()));
                        } else if selected_gate_op.is_none() {
                            if &PrimaryGateType::Polygon == current_gate_type {
                                draft_gate_coords.write().push((data_x, data_y));
                            }
                        } else if drag_data.is_none() {
                            gate_store.selected_gate().set(None);
                            dbl_click_lockout.set(true);
                            spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                dbl_click_lockout.set(false);
                            });
                        }

                    }
                    drag_data.set(None);
                },
                ondoubleclick: move |evt| {
                    if gate_store.selected_gate().peek().is_some() || dbl_click_lockout() {
                        return;
                    }
                    let local_coords = evt.data.coordinates().element();
                    let px = local_coords.x as f32;
                    let py = local_coords.y as f32;
                    let x_param = &*x_channel.peek();
                    let y_param = &*y_channel.peek();
                    let (_, dy) = mapper.pixel_to_data(px, py, None, None);

                    let points = {
                        if let Some(curr_gate) = &*draft_gate.peek() {
                            let mut points = curr_gate.get_points();
                            draft_gate_coords.write().clear();
                            points.pop();
                            Some(points)
                        } else {
                            None
                        }
                    };

                    let geo = if let PrimaryGateType::Line(_) = &*current_gate_type.peek() {
                        PrimaryGateType::Line(Some(dy))
                    } else {
                        current_gate_type.peek().cloned()
                    };
                    let x_p = axis_store
                        .settings()
                        .peek()
                        .get(&x_param.clone())
                        .map(|x_param| x_param.param.marker.clone());

                    let y_p = axis_store
                        .settings()
                        .peek()
                        .get(&y_param.clone())
                        .map(|y_param| y_param.param.marker.clone());

                    let name = if let (Some(x_m), Some(y_m)) = (x_p, y_p) {
                        Some(format!("{x_m} v {y_m}"))
                    } else {
                        None
                    };
                    match gate_store
                        .add_gate(
                            &mapper,
                            px,
                            py,
                            x_param.clone(),
                            y_param.clone(),
                            points,
                            parental_gate_id(),
                            geo,
                            name,
                        )
                    {
                        Ok(_) => {}
                        Err(e) => {
                            draft_gate_coords.set(vec![]);
                            println!("{e}");
                        }
                    };
                },
                onmousemove: move |evt| {
                    evt.stop_propagation();

                    if let Some(data) = drag_data() {
                        let now = std::time::Instant::now();
                        let elapsed = now.duration_since(*last_update.peek());
                        if elapsed < Duration::from_millis(16) {
                            return;
                        }

                        let rect_option = cached_rect.read();
                        let Some(rect) = rect_option.as_ref() else { return };

                        let client = evt.data.client_coordinates();
                        let (last_px, last_py) = *last_processed_pos.peek();

                        // --- 2. COORDINATE CALCULATION ---
                        let rendered_w = rect.width();
                        let rendered_h = rect.height();
                        if rendered_w == 0.0 || rendered_h == 0.0 {
                            return;
                        }
                        let Some(map) = plot_map.as_ref() else { return };
                        let scale_x = map.width() as f64 / rendered_w;
                        let scale_y = map.height() as f64 / rendered_h;
                        let px = (client.x - rect.min_x()) * scale_x;
                        let py = (client.y - rect.min_y()) * scale_y;
                        let dx = (px - last_px).abs();
                        let dy = (py - last_py).abs();
                        if dx < 1.0 && dy < 1.0 {
                            return;
                        }

                        let selected_gate_op = gate_store.selected_gate().peek().cloned();
                        let current_resolver_move = current_resolver_move.clone();
                        let data_coords = map.pixel_to_data(px as f32, py as f32, None, None);
                        let mut new_data = data.clone_with_point(data_coords);
                        if let Some(selected_gate_id) = selected_gate_op {
                            match &mut new_data {
                                GateDragType::Point(point_drag_data) => {
                                    gate_store
                                        .move_gate_point(
                                            selected_gate_id.clone(),
                                            point_drag_data,
                                            data_coords,
                                            &map,
                                            &current_resolver_move,
                                        )
                                        .unwrap_or_else(|e| println!("gate move failed: {e:?}"));
                                }
                                GateDragType::Gate(gate_drag_data) => {
                                    gate_store
                                        .move_gate(gate_drag_data.clone(), &current_resolver_move)
                                        .unwrap_or_else(|e| println!("gate move failed: {e:?}"));
                                }
                                GateDragType::Rotation(rotation_data) => {
                                    gate_store
                                        .rotate_gate(
                                            selected_gate_id.clone(),
                                            rotation_data.current_loc(),
                                            &current_resolver_move,
                                        )
                                        .unwrap_or_else(|e| println!("gate move failed: {e:?}"));
                                }
                            }
                        }
                        drag_data.set(Some(new_data));
                        last_processed_pos.set((px, py));
                        last_update.set(std::time::Instant::now());
                    }
                },
                onmouseup: move |evt| {
                    if let Some(data) = drag_data() {
                        let local_coords = &evt.data.coordinates().element();
                        let px = local_coords.x as f32;
                        let py = local_coords.y as f32;
                        let mapper = &*plot_map.peek();
                        let Some(mapper_ref) = mapper.as_ref() else {
                            return;
                        };
                        let data_coords = mapper_ref.pixel_to_data(px, py, None, None);

                        let new_data = data.clone_with_point(data_coords);
                        let selected_gate_op = gate_store.selected_gate().peek().cloned();
                        if selected_gate_op.is_none() {
                            return;
                        }
                        let selected_gate_id = selected_gate_op.unwrap();
                        match new_data {
                            GateDragType::Point(mut point_drag_data) => {
                                if let Some(mapper) = mapper {
                                    gate_store
                                        .move_gate_point(
                                            selected_gate_id.clone(),
                                            &mut point_drag_data,
                                            data_coords,
                                            mapper,
                                            &current_resolver_up,
                                        )
                                        .unwrap_or_else(|e| println!("gate move failed: {e:?}"));
                                }

                            }
                            GateDragType::Gate(gate_drag_data) => {
                                gate_store
                                    .move_gate(gate_drag_data, &current_resolver_up)
                                    .unwrap_or_else(|e| println!("gate move failed: {e:?}"));
                            }
                            GateDragType::Rotation(rotation_data) => {
                                gate_store
                                    .rotate_gate(
                                        selected_gate_id.clone(),
                                        rotation_data.current_loc(),
                                        &current_resolver_up,
                                    )
                                    .unwrap_or_else(|e| println!("gate move failed: {e:?}"));
                            }
                        }
                    }

                },
                onmousedown: move |evt| {
                    evt.stop_propagation();

                    match evt.trigger_button() {
                        Some(dioxus_elements::input_data::MouseButton::Primary) => {
                            let div_data = svg_data.read().clone();
                            spawn(async move {
                                if let Some(mount) = div_data {
                                    // MEASURE ONCE at the start of the drag
                                    if let Ok(rect) = mount.get_client_rect().await {
                                        cached_rect.set(Some(rect));
                                    }
                                }
                            });
                            if let Some(mapper) = plot_map() {
                                let local_coords = &evt.data.coordinates().element();
                                let norm_x = local_coords.x as f32;
                                let norm_y = local_coords.y as f32;
                                let pixel_coords = (norm_x, norm_y);
                                let data_coords = mapper.pixel_to_data(norm_x, norm_y, None, None);
                                let selected_gate_id = gate_store.selected_gate().peek().cloned();
                                let gates = &*gates.read();

                                if selected_gate_id.is_some() && draft_gate.peek().is_none()
                                    && drag_data.peek().is_none()
                                {
                                    let clicked_gate = was_gate_clicked(
                                        pixel_coords,
                                        &mapper,
                                        gates,
                                    );
                                    if let Some(cg) = clicked_gate {
                                        if selected_gate_id.clone().unwrap() == cg.get_id() {
                                            let data = GateDragData::new(
                                                cg.get_id(),
                                                data_coords,
                                                data_coords,
                                            );
                                            drag_data.set(Some(GateDragType::Gate(data)));
                                        } else {
                                            println!("{} {}", selected_gate_id.unwrap(), cg.get_id());
                                        }
                                    }
                                }
                            }
                        }
                        Some(dioxus_elements::input_data::MouseButton::Secondary) => {
                            let _ = gate_store.selected_gate().take();
                            draft_gate_coords.write().clear();

                        }
                        _ => {}
                    }
                },
                // if let Some(gates) = gate_store
                //     .get_gates_for_plot(x_channel(), y_channel(), parental_gate_id())
                // {
                for (gate_index , gate) in gates.read().0.iter().enumerate() {

                    {
                        let is_selected = if let Some(id) = &*gate_store.selected_gate().read() {
                            id == &gate.get_id()
                        } else {
                            false
                        };

                        let dd = if let Some(dd) = &*drag_data.read() {
                            if is_selected { Some(dd.clone()) } else { None }
                        } else {
                            None
                        };

                        let gate_stats = plot_store.gate_stats().get(gate.get_id()).map(|stats| stats());

                        rsx! {
                            RenderGate {
                                gate: gate.clone(),
                                gate_index,
                                is_selected,
                                drag_data: dd,
                                mapper: mapper.clone(),
                                gate_stats,
                            }
                        }
                    }
                }
                // }
                match draft_gate() {
                    Some(draft) => {
                        let id = "draft".to_string();
                        rsx! {

                            RenderDraftGate { key: "{id}", gate: draft.clone() }
                        }
                    }
                    None => rsx! {},
                }
            }
        }
    }
}

#[derive(Props, Clone)]
pub struct RenderGateProps {
    gate: Arc<dyn DrawableGate>,
    gate_index: usize,
    is_selected: bool,
    drag_data: Option<GateDragType>,
    mapper: Arc<PlotMapper>,
    gate_stats: Option<GateStats>,
}

impl PartialEq for RenderGateProps {
    fn eq(&self, other: &Self) -> bool {
        self.is_selected == other.is_selected
            && self.gate_index == other.gate_index
            && Arc::ptr_eq(&self.gate, &other.gate)
            && self.gate_stats == other.gate_stats
            && self.drag_data == other.drag_data
            && Arc::ptr_eq(&self.mapper, &other.mapper)
    }
}

#[component]
fn RenderGate(props: RenderGateProps) -> Element {
    let g = props.gate;
    let gate_id = g.get_id().clone();

    let is_selected = props.is_selected;

    let drag_data = if let Some(GateDragType::Point(dd)) = &props.drag_data {
        if is_selected { Some(dd.clone()) } else { None }
    } else {
        None
    };

    let (is_point, idx) = if let Some(GateDragType::Point(dd)) = &props.drag_data {
        (true, dd.point_index())
    } else {
        (false, 0)
    };

    rsx! {

        for (shape_index , shape) in g.draw_self(is_selected, drag_data.clone(), &props.mapper, &props.gate_stats)
            .into_iter()
            .enumerate()
        {
            RenderShape {
                key: "{gate_id}-{shape_index}",
                shape,
                gate_id: gate_id.clone(),
                gate_index: props.gate_index,
                shape_index,
                drag_data: if is_point { if idx == shape_index { props.drag_data.clone() } else { None } } else { props.drag_data.clone() },
            }
        }

    }
}

fn was_gate_clicked(
    click_coords: (f32, f32),
    mapper: &PlotMapper,
    gates: &[Arc<dyn DrawableGate>],
) -> Option<Arc<dyn DrawableGate>> {
    let (data_x, data_y) = mapper.pixel_to_data(click_coords.0, click_coords.1, None, None);

    let mut closest_gate = None;

    let tolerance = mapper.get_data_tolerance(5.0);
    let mut closest_dist = f32::INFINITY;
    for gate in gates {
        if let Some(dist) = gate.is_point_on_perimeter((data_x, data_y), tolerance, mapper)
            && dist < closest_dist
        {
            closest_dist = dist;
            closest_gate = Some(gate.clone());
        }
    }

    closest_gate
}

#[component]
fn RenderDraftGate(gate: GateDraft) -> Element {
    rsx! {
        for (shape_index , shape) in gate.draw_self().into_iter().enumerate() {
            RenderShape {
                shape,
                gate_id: Arc::from("draft"),
                gate_index: 0,
                shape_index,
            }
        }
    }
}

#[component]
fn RenderShape(
    shape: GateRenderShape,
    gate_id: Arc<str>,
    gate_index: usize,
    shape_index: usize,
    drag_data: Option<GateDragType>,
) -> Element {
    let plot_map = use_context::<Signal<Option<Arc<PlotMapper>>>>();
    let mut drag_data_signal = use_context::<Signal<Option<GateDragType>>>();
    if let Some(mapper) = &*plot_map.read() {
        let transform = "none".to_string();
        match shape {
            GateRenderShape::PolyLine {
                points,
                style,
                shape_type: _,
            } => {
                let mapped = points
                    .iter()
                    .map(|(x, y)| {
                        let p = mapper.data_to_pixel(*x, *y, None, None);
                        format!("{},{}", p.0, p.1)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                rsx! {
                    g { transform,
                        polyline {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            points: "{mapped}",
                            stroke: style.stroke,
                            stroke_width: style.stroke_width,
                            stroke_dasharray: if style.dashed { "4" } else { "none" },
                            fill: style.fill,
                        }
                    }
                }
            }
            GateRenderShape::Circle {
                center,
                radius,
                fill,
                shape_type,
            } => {
                let p = mapper.data_to_pixel(center.0, center.1, None, None);
                rsx! {
                    g { transform,
                        circle {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            cx: "{p.0}",
                            cy: "{p.1}",
                            r: radius,
                            fill,
                            onmousedown: move |evt| {
                                match shape_type {
                                    ShapeType::Point(index)
                                    | ShapeType::CompositePoint(index, ..)
                                    | ShapeType::UndraggablePoint(index) => {
                                        match evt.trigger_button() {
                                            Some(dioxus_elements::input_data::MouseButton::Primary) => {
                                                let local_coords = &evt.data.coordinates().element();
                                                let px = local_coords.x as f32;
                                                let py = local_coords.y as f32;
                                                let data_coords = plot_map()
                                                    .unwrap()
                                                    .pixel_to_data(px, py, None, None);
                                                let point_drag_data = PointDragData::new(index, data_coords);
                                                drag_data_signal.set(Some(GateDragType::Point(point_drag_data)));
                                            }
                                            Some(dioxus_elements::input_data::MouseButton::Secondary) => {
                                                println!("make context menu to add or delete points");
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                }
                            },
                        }
                    }
                }
            }
            GateRenderShape::Polygon {
                points,
                style,
                shape_type: _,
            } => {
                let mapped_points = points
                    .iter()
                    .map(|(x, y)| {
                        let p = mapper.data_to_pixel(*x, *y, None, None);
                        format!("{},{}", p.0, p.1)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                rsx! {
                    g { transform,
                        polygon {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            points: mapped_points,
                            stroke: style.stroke,
                            stroke_width: style.stroke_width,
                            stroke_dasharray: if style.dashed { "4" } else { "none" },
                            fill: style.fill,
                        }
                    }

                }
            }
            GateRenderShape::Ellipse {
                center,
                radius_x,
                radius_y,
                degrees_rotation,
                style,
                shape_type: _,
            } => {
                let cp = mapper.data_to_pixel(center.0, center.1, None, None);
                let x_edge = mapper.data_to_pixel(center.0 + radius_x, center.1, None, None);
                let rx_px = (x_edge.0 - cp.0).abs();
                let y_edge = mapper.data_to_pixel(center.0, center.1 + radius_y, None, None);
                let ry_px = (y_edge.1 - cp.1).abs();

                rsx! {
                    g { transform,
                        ellipse {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            cx: cp.0,
                            cy: cp.1,
                            rx: rx_px,
                            ry: ry_px,
                            stroke: style.stroke,
                            stroke_width: style.stroke_width,
                            stroke_dasharray: if style.dashed { "4" } else { "none" },
                            fill: style.fill,
                            transform: "rotate({degrees_rotation} {cp.0} {cp.1})",
                        }
                    }

                }
            }
            GateRenderShape::Handle {
                center,
                size,
                shape_center,
                shape_type,
            } => {
                let c = mapper.data_to_pixel(center.0, center.1, None, None);
                let cp = mapper.data_to_pixel(shape_center.0, shape_center.1, None, None);

                let pixel_offset = 15.0;
                let handle_x = c.0;
                let handle_y = c.1 - (size + pixel_offset);

                // let translate = {
                //     if let Some(GateDragType::Gate(data)) = &drag_data {
                //         if *gate_id == *data.gate_id() {
                //             let offset = data.offset();
                //             let p_start = mapper.data_to_pixel(0.0, 0.0, None, None);
                //             let p_current = mapper.data_to_pixel(offset.0, offset.1, None, None);
                //             let dx = p_current.0 - p_start.0;
                //             let dy = p_current.1 - p_start.1;
                //             Some(format!("translate({} {})", -dx, -dy))
                //         } else {
                //             None
                //         }
                //     } else {
                //         None
                //     }
                // };
                let rotate = {
                    if let Some(GateDragType::Rotation(data)) = &drag_data {
                        if let ShapeType::Rotation(handle_angle_rad) = shape_type {
                            let angle = -(handle_angle_rad.to_degrees()) + data.rotation_deg();
                            Some(format!("rotate({} {} {})", angle, cp.0, cp.1))
                        } else {
                            None
                        }
                    } else if let ShapeType::Rotation(handle_angle_rad) = shape_type {
                        let r = -(handle_angle_rad.to_degrees());
                        Some(format!("rotate({} {} {})", r, cp.0, cp.1))
                    } else {
                        None
                    }
                };

                let transform = [rotate].into_iter().flatten().collect::<Vec<_>>().join(" ");

                rsx! {
                    g { transform,
                        line {
                            x1: "{c.0}",
                            y1: "{c.1}",
                            x2: "{handle_x}",
                            y2: "{handle_y}",
                            stroke: "orange",
                            stroke_width: "1.5",
                            stroke_dasharray: "4",
                            pointer_events: "none",
                        }
                        circle {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            cx: "{handle_x}",
                            cy: "{handle_y}",
                            stroke: "orange",
                            r: size,
                            onmousedown: move |evt| {
                                let local_coords = &evt.data.coordinates().element();
                                let px = local_coords.x as f32;
                                let py = local_coords.y as f32;
                                let data_coords = plot_map()
                                    .unwrap()
                                    .pixel_to_data(px, py, None, None);
                                drag_data_signal
                                    .set(
                                        Some(
                                            GateDragType::Rotation(
                                                RotationData::new(
                                                    gate_id.clone(),
                                                    shape_center,
                                                    data_coords,
                                                    data_coords,
                                                ),
                                            ),
                                        ),
                                    )
                            },
                        }
                    }
                }
            }
            GateRenderShape::Rectangle {
                x,
                y,
                width,
                height,
                style,
                shape_type: _,
            } => {
                let (mx, my, m_width, m_height) =
                    rectangle_gate::map_rect_to_pixels(x, y, width, height, mapper);
                rsx! {
                    g { transform,
                        rect {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            x: mx,
                            y: my,
                            width: m_width,
                            height: m_height,
                            stroke: style.stroke,
                            stroke_width: style.stroke_width,
                            stroke_dasharray: if style.dashed { "4" } else { "none" },
                            fill: style.fill,
                        }
                    }

                }
            }
            GateRenderShape::Line {
                x1,
                y1,
                x2,
                y2,
                style,
                shape_type: _,
            } => {
                let p1 = mapper.data_to_pixel(x1, y1, None, None);
                let p2 = mapper.data_to_pixel(x2, y2, None, None);
                rsx! {
                    g { transform,
                        line {
                            key: "{gate_id}-{gate_index}-{shape_index}",
                            x1: "{p1.0}",
                            y1: "{p1.1}",
                            x2: "{p2.0}",
                            y2: "{p2.1}",
                            stroke: style.stroke,
                            stroke_width: style.stroke_width,
                            stroke_dasharray: if style.dashed { "4" } else { "none" },
                        }
                    }

                }
            }
            GateRenderShape::Text {
                origin,
                offset,
                fontsize,
                text,
                text_anchor,
                shape_type,
            } => {
                let transform = match shape_type {
                    ShapeType::Text => transform,
                    ShapeType::UndraggableText(direction) => {
                        if let Some(GateDragType::Gate(data)) = drag_data {
                            if *gate_id == *data.gate_id() {
                                let offset = data.offset();
                                let p_start = mapper.data_to_pixel(0.0, 0.0, None, None);
                                let p_current =
                                    mapper.data_to_pixel(offset.0, offset.1, None, None);
                                let dx = p_current.0 - p_start.0;
                                let dy = p_current.1 - p_start.1;
                                match direction {
                                    Direction::X => format!("translate({} {})", 0, -dy),
                                    Direction::Y => format!("translate({} {})", -dx, 0),
                                    Direction::Both => "none".to_string(),
                                }
                            } else {
                                "none".to_string()
                            }
                        } else {
                            "none".to_string()
                        }
                    }
                    _ => unreachable!(),
                };

                let loc =
                    mapper.data_to_pixel(origin.0 + offset.0, origin.1 + offset.1, None, None);
                rsx! {
                    g { transform,
                        text {
                            x: loc.0,
                            y: loc.1,
                            text_anchor,
                            font_size: fontsize,
                            // pointer_events: "none",
                            "{text}"
                        }
                    }
                }
            }
        }
    } else {
        rsx! {}
    }
}
