use crate::components::context_menu::*;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateStateImplExt, GateStateStoreExt, NodeId, ROOTGATE};
use crate::gate_editor::plots::axis_store::{AxisStore, AxisStoreStoreExt, Param};
use dioxus::prelude::*;
use dioxus::stores::SyncStore;
use std::sync::Arc;
static SIDEBAR_STYLE: Asset = asset!("assets/gate_sidebar.css");

/// Where a link is up to, while one is being made.
///
/// Linking discards the source position's own gate, and nothing in the UI can
/// bring it back, so it is a two-step action: pick a target, then confirm.
#[derive(Clone, PartialEq)]
enum LinkStep {
    /// "Link to..." was chosen on this position; the next row clicked is the
    /// target rather than a new selection.
    Picking(Arc<str>),
    /// A target was chosen. Nothing has been written yet.
    Confirming { source: Arc<str>, target: Arc<str> },
}

/// Shared through context because the tree renders recursively.
#[derive(Clone, Copy)]
struct LinkPick(Signal<Option<LinkStep>>);

/// Whatever the store said when a link was refused, for showing to the user
/// rather than only printing it.
#[derive(Clone, Copy)]
struct LinkError(Signal<Option<String>>);

#[component]
pub fn GateSidebar(
    selected_id: Signal<Option<Arc<str>>>,
    x_axis_param: Signal<Param>,
    y_axis_param: Signal<Param>,
) -> Element {
    // let gate_store: Store<GateState> = use_context::<Store<GateState>>();
    let gate_store = use_context::<SyncStore<GateState>>();
    let hierarchy = gate_store.hierarchy();
    let roots = hierarchy.read().get_roots();
    let link_pick = use_context_provider(|| LinkPick(Signal::new(None)));
    let link_error = use_context_provider(|| LinkError(Signal::new(None)));
    let mut picking = link_pick.0;
    let mut error = link_error.0;
    let mut store_for_link = gate_store;

    // Names, so the confirmation says what it is about to do rather than
    // quoting ids.
    let name_of = move |node: &Arc<str>| -> String {
        store_for_link
            .read()
            .gate_for_node(&NodeId::from(node.clone()))
            .and_then(|g| store_for_link.read().registered_gate(g))
            .map(|g| g.get_name().to_string())
            .unwrap_or_else(|| node.to_string())
    };

    rsx! {
        document::Stylesheet { href: SIDEBAR_STYLE }
        div { class: "custom-sidebar",
            h3 { class: "sidebar-title", "Gate Hierarchy" }

            if let Some(step) = picking.read().clone() {
                match step {
                    LinkStep::Picking(_) => rsx! {
                        div { class: "link-prompt",
                            span { "Click a gate to link to" }
                            button {
                                class: "link-cancel",
                                onclick: move |_| picking.set(None),
                                "Cancel"
                            }
                        }
                    },
                    LinkStep::Confirming { source, target } => {
                        let (from, to) = (name_of(&source), name_of(&target));
                        let (s2, t2) = (source.clone(), target.clone());
                        rsx! {
                            div { class: "link-prompt link-confirm",
                                span {
                                    "Apply {to} at {from}? {from}'s own gate is discarded."
                                }
                                span { class: "link-actions",
                                    button {
                                        class: "link-go",
                                        onclick: move |_| {
                                            let result = store_for_link.write().link_node_to_gate(
                                                &NodeId::from(s2.clone()),
                                                &NodeId::from(t2.clone()),
                                            );
                                            match result {
                                                Ok(()) => error.set(None),
                                                Err(e) => error.set(Some(e.to_string())),
                                            }
                                            picking.set(None);
                                        },
                                        "Link"
                                    }
                                    button {
                                        class: "link-cancel",
                                        onclick: move |_| picking.set(None),
                                        "Cancel"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(message) = error.read().clone() {
                div { class: "link-error",
                    span { "{message}" }
                    button {
                        class: "link-cancel",
                        onclick: move |_| error.set(None),
                        "Dismiss"
                    }
                }
            }

            div { class: "sidebar-tree",
                // Wrapper sized to the widest row, so the tree can overflow
                // horizontally inside the scroller instead of widening the pane.
                div { class: "sidebar-tree-inner",

                    for root_id in roots {
                        for child_id in hierarchy.read().get_children(&root_id) {
                            GateNode {
                                key: "{child_id}",
                                node_id: child_id.clone(),
                                selected: selected_id,
                                level: 0,
                                x_axis_param,
                                y_axis_param,
                            }
                        }
                    }
                }
            }
        }
    }
}

// Keep your exact ChevronIcon, we'll just rotate it with CSS
#[component]
fn ChevronIcon() -> Element {
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "m9 18 6-6-6-6" }
        }
    }
}

#[component]
fn GateNode(
    // A position in the tree, not a gate: one gate can be applied at several
    // points, and each gets its own node. The gate is looked up from it below.
    node_id: Arc<str>,
    selected: Signal<Option<Arc<str>>>,
    level: usize,
    x_axis_param: Signal<Param>,
    y_axis_param: Signal<Param>,
) -> Element {
    let mut gate_store = use_context::<SyncStore<GateState>>();
    let mut picking = use_context::<LinkPick>().0;
    let mut link_error = use_context::<LinkError>().0;
    let axis_store: SyncStore<AxisStore> = use_context::<SyncStore<AxisStore>>();
    let mut is_expanded = use_signal(|| true);

    // Which gate this position shows. Several nodes may name the same gate -
    // that is what a linked gate is - so everything about the gate comes from
    // here, and everything about the tree from `node_id`.
    let Some(gate_id) = gate_store
        .read()
        .gate_for_node(&NodeId::from(node_id.clone()))
        .cloned()
    else {
        return rsx! {};
    };
    let is_linked = gate_store.read().is_linked(&gate_id);

    // Fetch children
    let hierarchy = gate_store.hierarchy();
    let children = hierarchy
        .read()
        .get_children(&node_id)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let has_children = !children.is_empty();
    let is_root = hierarchy.read().is_root(&node_id);

    // The plot this node's children are drawn on is keyed by the parent node,
    // so a linked gate's two placements open two different plots.
    let parent = {
        if is_root {
            node_id.clone()
        } else {
            hierarchy.read().get_parent(&node_id).unwrap().clone()
        }
    };

    let gate_name = {
        if let Some(gate) = gate_store
            .gate_store()
            .peek()
            .primary_and_subgate_registry
            .get(&gate_id)
        {
            if gate.is_composite() {
                if let Some(subgate) = gate.get_gate_ref(Some(&gate_id)) {
                    subgate.name.clone()
                } else {
                    gate.get_name().to_owned()
                }
            } else {
                gate.get_name().to_owned()
            }
        } else {
            gate_id.to_string()
        }
    };

    // Check if this node is the active one. Compared on the node: two
    // placements of a linked gate are different rows and only one is selected.
    let is_selected = selected.read().as_ref() == Some(&node_id);

    // Indent per level, halved to match the tree's font size.
    let padding = format!("{}px", level * 8 + 6);

    let gate_id_clone = gate_id.clone();
    let node_id_for_click = node_id.clone();
    let node_id_for_link = node_id.clone();
    let node_id_for_unlink = node_id.clone();
    let node_id_for_instance = node_id.clone();
    let gate_id_delete_clone = gate_id.clone();
    // let gate_id_rename_clone = gate_id.clone();
    let parent_for_delete = parent.clone();
    let parent_for_instance = parent.clone();
    let gate_id_for_not_gate = gate_id.clone();
    let parent_for_not_gate = parent.clone();
    let gate_id_for_and_gate = gate_id.clone();
    let parent_for_and_gate = parent.clone();
    let gate_id_for_or_gate = gate_id.clone();
    let parent_for_or_gate = parent.clone();
    rsx! {

        // 1. The Row (Clickable)
        ContextMenu {
            ContextMenuTrigger {
                div { class: "gate-node-container",
                    div {
                        class: format!("gate-node-row{}", if is_selected { " selected" } else { "" }),
                        style: "padding-left: {padding};",
                        onclick: move |e: Event<MouseData>| {

                            e.stop_propagation();

                            // A link is in progress: this row is the target, not
                            // a new selection.
                            // A link is in progress: this row is the target, not
                            // a new selection. Nothing is written until the
                            // confirmation below is accepted.
                            let in_progress = picking.peek().clone();
                            if let Some(LinkStep::Picking(source)) = in_progress {
                                picking.set(Some(LinkStep::Confirming {
                                    source,
                                    target: node_id_for_click.clone(),
                                }));
                                return;
                            }

                            let Some((x, y)) = gate_store
                                .gate_store()
                                .peek()
                                .primary_and_subgate_registry
                                .get(&gate_id_clone)
                                .map(|g| g.get_params()) else { return };
                            let (new_x, new_y);
                            if let Some(x_axis_settings) = axis_store.settings().peek().get(&x) {
                                new_x = Some(x_axis_settings.param.clone());
                            } else {
                                new_x = None;
                            }
                            if let Some(y_axis_settings) = axis_store.settings().peek().get(&y) {
                                new_y = Some(y_axis_settings.param.clone());
                            } else {
                                new_y = None;
                            }
                            if let (Some(new_x), Some(new_y)) = (new_x, new_y) {
                                x_axis_param.set(new_x);
                                y_axis_param.set(new_y);
                                selected.set(Some(parent.clone()));
                                *gate_store.selected_gate().write() = Some(gate_id_clone.clone());
                            }

                        },

                        if has_children {
                            div {
                                class: format!("toggle-icon{}", if is_expanded() { " expanded" } else { "" }),
                                onclick: move |e| {
                                    e.stop_propagation();
                                    is_expanded.toggle();
                                },
                                ChevronIcon {}
                            }
                        } else {
                            // Empty space so leaf nodes align perfectly with parent text
                            div { class: "toggle-icon-placeholder" }
                        }

                        // 3. The Label
                        span { class: "gate-name", "{gate_name}" }
                        if is_linked {
                            span {
                                class: "linked-badge",
                                title: "This gate is applied at more than one point in the tree",
                                "\u{1f517}"
                            }
                        }
                        button {
                            class: "activate-btn",
                            title: "Activate gate",
                            onclick: move |e| {
                                // IMPORTANT: Stop the row's onclick from firing
                                e.stop_propagation();
                                let Some((x, y)) = gate_store
                                    .gate_store()
                                    .peek()
                                    .primary_and_subgate_registry
                                    .get(&gate_id)
                                    .map(|g| g.get_params()) else { return };
                                let (new_x, new_y);
                                if let Some(x_axis_settings) = axis_store.settings().peek().get(&x) {
                                    new_x = Some(x_axis_settings.param.clone());
                                } else {
                                    new_x = None;
                                }
                                if let Some(y_axis_settings) = axis_store.settings().peek().get(&y) {
                                    new_y = Some(y_axis_settings.param.clone());
                                } else {
                                    new_y = None;
                                }
                                if let (Some(new_x), Some(new_y)) = (new_x, new_y) {
                                    x_axis_param.set(new_x);
                                    y_axis_param.set(new_y);
                                    // This placement, not the gate: the plot
                                    // below shows the population *this* node
                                    // sees.
                                    selected.set(Some(node_id.clone()));
                                }

                            },
                            "🎯"
                        }
                    
                    }

                    // 4. The Children (Recursive call)
                    if has_children && is_expanded() {
                        div { class: "gate-children",
                            for child_id in children {
                                GateNode {
                                    key: "{child_id}",
                                    node_id: child_id,
                                    selected,
                                    level: level + 1,
                                    x_axis_param,
                                    y_axis_param,
                                }
                            }
                        }
                    }
                }
            }
            ContextMenuContent {

                ContextMenuItem {
                    value: "delete".to_string(),
                    index: 0usize,
                    on_select: move |_| {
                        match gate_store.remove_gate(gate_id_delete_clone.clone()) {
                            Ok(_) => {
                                println!("deleted gate");
                                if is_root {
                                    selected.set(Some(ROOTGATE.clone()));
                                } else {
                                    selected.set(Some(parent_for_delete.clone()));
                                }

                            }
                            Err(_) => println!("failed to delete gate"),
                        }
                    },
                    "Delete"
                }
                ContextMenuItem {
                    value: "rename".to_string(),
                    index: 1usize,
                    on_select: move |_| {},
                    "Rename"
                }
                if is_linked {
                    ContextMenuItem {
                        value: "delete-instance".to_string(),
                        index: 6usize,
                        on_select: move |_| {
                            let result = gate_store
                                .write()
                                .delete_placement(&NodeId::from(node_id_for_instance.clone()));
                            match result {
                                Ok(()) => selected.set(Some(parent_for_instance.clone())),
                                Err(err) => link_error.set(Some(err.to_string())),
                            }
                        },
                        "Delete this instance"
                    }
                    ContextMenuItem {
                        value: "unlink".to_string(),
                        index: 7usize,
                        on_select: move |_| {
                            if let Err(err) = gate_store
                                .write()
                                .unlink_node(&NodeId::from(node_id_for_unlink.clone()))
                            {
                                link_error.set(Some(err.to_string()));
                            }
                        },
                        "Unlink"
                    }
                }
                ContextMenuItem {
                    value: "link".to_string(),
                    index: 8usize,
                    on_select: move |_| {
                        picking.set(Some(LinkStep::Picking(node_id_for_link.clone())));
                    },
                    "Link to..."
                }
                ContextMenuItem {
                    value: "not".to_string(),
                    index: 2usize,
                    on_select: move |_| {

                        let x_axis_param = x_axis_param.peek().fluoro.clone();
                        let y_axis_param = y_axis_param.peek().fluoro.clone();
                        match gate_store
                            .add_boolean_gate(
                                None,
                                flow_gates::BooleanOperation::Not,
                                vec![gate_id_for_not_gate.clone()],
                                Some(parent_for_not_gate.clone()),
                                x_axis_param,
                                y_axis_param,
                            )
                        {
                            Ok(_) => {}
                            Err(e) => println!("{e}"),
                        }
                    },
                    "Add NOT Gate"
                }
                ContextMenuItem {
                    value: "and".to_string(),
                    index: 3usize,
                    on_select: move |_| {
                        // let id = format!("AND_{}", gate_id_for_and_gate.clone());
                        let x_axis_param = x_axis_param.peek().fluoro.clone();
                        let y_axis_param = y_axis_param.peek().fluoro.clone();
                        match gate_store
                            .add_boolean_gate(
                                None,
                                flow_gates::BooleanOperation::And,
                                vec![gate_id_for_and_gate.clone()],
                                Some(parent_for_and_gate.clone()),
                                x_axis_param,
                                y_axis_param,
                            )
                        {
                            Ok(_) => {}
                            Err(e) => println!("{e}"),
                        }
                    },
                    "Add AND Gate"
                }
                ContextMenuItem {
                    value: "or".to_string(),
                    index: 4usize,
                    on_select: move |_| {

                        let x_axis_param = x_axis_param.peek().fluoro.clone();
                        let y_axis_param = y_axis_param.peek().fluoro.clone();
                        match gate_store
                            .add_boolean_gate(
                                None,
                                flow_gates::BooleanOperation::Or,
                                vec![gate_id_for_or_gate.clone()],
                                Some(parent_for_or_gate.clone()),
                                x_axis_param,
                                y_axis_param,
                            )
                        {
                            Ok(_) => {}
                            Err(e) => println!("{e}"),
                        }
                    },
                    "Add OR Gate"
                }
            }
        
        }
    }
}
