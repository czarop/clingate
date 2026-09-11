use crate::gate_editor::gates::gate_hierarchy::GateHierarchy;
use crate::gate_editor::gates::gate_single::boolean_gates::BooleanGate;
use anyhow::anyhow;
use dioxus::prelude::*;
use flow_fcs::TransformType;
use flow_gates::{BooleanOperation, Gate};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

use crate::gate_editor::{
    AxisInfo,
    gates::{
        gate_composite::{
            bisector_gate::BisectorGate, quadrant_gate::QuadrantGate,
            skewed_quadrant_gate::SkewedQuadrantGate,
        },
        gate_drag::GateDragData,
        gate_single::{
            ellipse_gate::{EllipseGate, create_default_ellipse},
            line_gate::{LineGate, create_default_line},
            polygon_gate::PolygonGate,
            rectangle_gate::{RectangleGate, create_default_rectangle},
        },
        gate_traits::DrawableGate,
        gate_types::PrimaryGateType,
    },
    plots::axis_store::PlotMapper,
};
use crate::omiq::deserialise::{
    BooleanOpType, CompositeType, FilterContainer, find_atomic_params,
    get_composite_gates_from_filter_container,
};
use crate::omiq::metadata::{MetaDataKey, MetaDataParameter};

pub type GateId = std::sync::Arc<str>;
pub type FileId = std::sync::Arc<str>;
pub type GroupId = std::sync::Arc<str>;

pub static ROOTGATE: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("root"));

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct GatesOnPlotKey {
    param_1: GateId,
    param_2: GateId,
    parental_gate_id: Option<GateId>,
}

impl GatesOnPlotKey {
    pub fn new(param_1: Arc<str>, param_2: Arc<str>, parental_gate_id: Option<GateId>) -> Self {
        if param_1 <= param_2 {
            Self {
                param_1,
                param_2,
                parental_gate_id,
            }
        } else {
            Self {
                param_1: param_2,
                param_2: param_1,
                parental_gate_id,
            }
        }
    }
}

#[derive(Default)]
pub struct GateMap(pub FxHashMap<GateId, Arc<dyn DrawableGate + 'static>>);

impl Deref for GateMap {
    type Target = FxHashMap<GateId, Arc<dyn DrawableGate + 'static>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for GateMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GateSource {
    Global,
    Group((GateId, MetaDataKey)),
    Sample((GateId, FileId)),
}

pub type GroupGateMap = FxHashMap<(GateId, MetaDataKey), Arc<dyn DrawableGate>>;
pub type SampleGateMap = FxHashMap<(GateId, FileId), Arc<dyn DrawableGate>>;

#[derive(Default, Store)]
pub struct GateSubStore {
    pub primary_and_subgate_registry: GateMap,
    pub sample_position_overrides: SampleGateMap,
    pub group_position_overrides: GroupGateMap,
}

#[derive(Clone, Default, PartialEq)]
pub struct GateOverrideResolver {
    pub active_gates: im::HashMap<GateId, ComparableGate, FxBuildHasher>,
    pub gate_origins: im::HashMap<GateId, GateSource, FxBuildHasher>,
}

#[derive(Clone)]
pub struct ComparableGate(pub Arc<dyn DrawableGate>);

impl PartialEq for ComparableGate {
    fn eq(&self, other: &Self) -> bool {
        // Fast pointer comparison: Are these the same allocation?
        Arc::ptr_eq(&self.0, &other.0)
    }
}

// This allows: my_comparable_gate.draw()
impl Deref for ComparableGate {
    type Target = Arc<dyn DrawableGate>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Arc<dyn DrawableGate>> for ComparableGate {
    fn from(arc: Arc<dyn DrawableGate>) -> Self {
        Self(arc)
    }
}

// for overides generate a clone of the drawable in new position with the same Uuid
impl GateOverrideResolver {
    // fn resolve(&self, id: &GateId) -> anyhow::Result<Gate> {
    //     self.active_gates
    //         .get(id)
    //         .ok_or_else(|| anyhow::anyhow!("Gate {} not found in active set", id))?
    //         .get_gate_ref(Some(id))
    //         .map(|g| g.clone())
    //         .ok_or_else(|| anyhow::anyhow!("Gate {} has no internal data", id))
    // }

    fn resolve_drawable(&self, id: &str) -> anyhow::Result<Arc<dyn DrawableGate + 'static>> {
        let drawable = self
            .active_gates
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Gate {} not found in active set", id))?;
        Ok(drawable.deref().clone())
    }
}

/// a plot is selected for a file,
/// The currently selected (parental) gate id is stored in a signal and accessed.
/// Create a GatesOnPlotKey with the current params and the parental gate id,
/// to retrieve a list of gate id's shown on the plot.
/// For each gate id, the actual gates can be retrieved from gate_registry.
/// Check for file-specific positioning before drawing

#[derive(Default, Store)]
pub struct GateState {
    // file_id: FileId,
    selected_gate: Option<Arc<str>>,
    // For the Renderer: "What gates do I draw on this Plot?"
    gate_ids_by_view: FxHashMap<GatesOnPlotKey, Vec<GateId>>,
    // For the Filtering: "How are these gates nested?" - this is the master hierarchy
    hierarchy: GateHierarchy,
    // when deleting a gate, do you need to delete any boolean gates that depend on it?
    boolean_gate_links: FxHashMap<GateId, Vec<GateId>>,
    gate_store: GateSubStore,
}

#[store(pub name = GateStateImplExt)]
impl<Lens> Store<GateState, Lens> {
    fn get_current_sample(
        &mut self,
        file_id: FileId,
        group_ids: &FxHashMap<MetaDataParameter, GroupId>,
    ) -> Result<GateOverrideResolver> {
        // construct the GateResolver for this file
        let mut active_gates: im::HashMap<Arc<str>, ComparableGate, FxBuildHasher> =
            im::HashMap::with_hasher(FxBuildHasher);
        let mut gate_origins = im::HashMap::with_hasher(FxBuildHasher);

        {
            let registry_binding = self.gate_store().primary_and_subgate_registry();
            let registry = registry_binding.read();
            let sample_ovr_binding = self.gate_store().sample_position_overrides();
            let sample_overrides = sample_ovr_binding.read();
            let group_ovr_binding = self.gate_store().group_position_overrides();
            let group_overrides = group_ovr_binding.read();

            for (default_id, base_arc) in &registry.0 {
                if let Some((key, s_ovr)) =
                    sample_overrides.get_key_value(&(default_id.clone(), file_id.clone()))
                {
                    active_gates.insert(default_id.clone(), s_ovr.clone().into());
                    gate_origins.insert(default_id.clone(), GateSource::Sample(key.clone()));
                } else if let Some((key, g_ovr)) = group_ids.iter().find_map(|gid| {
                    let key = MetaDataKey {
                        parameter: gid.0.clone(),
                        group: gid.1.clone(),
                    };
                    group_overrides.get_key_value(&(default_id.clone(), key))
                }) {
                    active_gates.insert(default_id.clone(), g_ovr.clone().into());
                    gate_origins.insert(default_id.clone(), GateSource::Group(key.clone()));
                } else {
                    active_gates.insert(default_id.clone(), base_arc.clone().into());
                    gate_origins.insert(default_id.clone(), GateSource::Global);
                }
            }
        }

        Ok(GateOverrideResolver {
            active_gates,
            gate_origins,
        })
    }

    fn get_gate_by_id(
        &self,
        id: GateId,
        resolver: &GateOverrideResolver,
    ) -> Option<Arc<dyn DrawableGate>> {
        resolver.resolve_drawable(&id).ok()
    }

    fn add_gate(
        &mut self,
        mapper: &PlotMapper,
        click_x: f32,
        click_y: f32,
        x_param: Arc<str>,
        y_param: Arc<str>,
        points: Option<Vec<(f32, f32)>>,
        parental_gate_id: Option<GateId>,
        gate_type: PrimaryGateType,
        name: Option<String>,
    ) -> Result<()> {
        let key = GatesOnPlotKey::new(x_param.clone(), y_param.clone(), parental_gate_id.clone());
        println!("{:?}", key);
        let parameters = (x_param.clone(), y_param.clone());

        let id = Uuid::new_v4().to_string();
        let id_arc: Arc<str> = Arc::from(id.as_ref() as &str);

        let g: Arc<dyn DrawableGate + 'static> = match gate_type {
            PrimaryGateType::Polygon => {
                let geo = flow_gates::geometry::create_polygon_geometry(
                    points.ok_or(anyhow!("points not provided for polygon gate"))?,
                    &x_param,
                    &y_param,
                )
                .map_err(|_| anyhow!("failed to create polygon geometry"))?;
                let gate = Gate {
                    id: id_arc,
                    name: name.unwrap_or(id.to_string()),
                    geometry: geo,
                    mode: flow_gates::GateMode::Global,
                    parameters,
                    label_position: None,
                };
                Arc::new(PolygonGate::try_new(gate, true)?)
            }
            PrimaryGateType::Ellipse => {
                let geo = create_default_ellipse(
                    mapper, click_x, click_y, 50f32, 30f32, &x_param, &y_param,
                )?;
                let gate = Gate {
                    id: id_arc,
                    name: name.unwrap_or(id.to_string()),
                    geometry: geo,
                    mode: flow_gates::GateMode::Global,
                    parameters,
                    label_position: None,
                };
                Arc::new(EllipseGate::try_new(gate, true)?)
            }
            PrimaryGateType::Rectangle => {
                let geo = create_default_rectangle(
                    mapper, click_x, click_y, 50f32, 50f32, &x_param, &y_param,
                )?;
                let gate = Gate {
                    id: id_arc,
                    name: name.unwrap_or(id.to_string()),
                    geometry: geo,
                    mode: flow_gates::GateMode::Global,
                    parameters,
                    label_position: None,
                };
                Arc::new(RectangleGate::try_new(gate, true)?)
            }
            PrimaryGateType::Line(y_coord) => {
                let geo = create_default_line(mapper, click_x, 50f32, &x_param, &y_param)?;
                if let Some(y_coord) = y_coord {
                    let gate = Gate {
                        id: id_arc,
                        name: name.unwrap_or(id.to_string()),
                        geometry: geo,
                        mode: flow_gates::GateMode::Global,
                        parameters,
                        label_position: None,
                    };
                    Arc::new(LineGate::try_new(gate, y_coord, true)?)
                } else {
                    Err(anyhow!(
                        "Line gate requires y coordinate for initialization"
                    ))?
                }
            }

            PrimaryGateType::Bisector => Arc::new(BisectorGate::try_new(
                mapper,
                id_arc,
                name.unwrap_or(id.to_string()),
                (click_x, click_y),
                x_param,
                y_param,
            )?),
            PrimaryGateType::Quadrant => Arc::new(QuadrantGate::try_new_from_raw_coord(
                mapper,
                id_arc,
                name.unwrap_or(id.to_string()),
                (click_x, click_y),
                x_param,
                y_param,
            )?),
            PrimaryGateType::SkewedQuadrant => {
                Arc::new(SkewedQuadrantGate::try_new_from_raw_coord(
                    mapper,
                    id_arc,
                    name.unwrap_or(id.to_string()),
                    (click_x, click_y),
                    x_param,
                    y_param,
                )?)
            }
            _ => panic!("add boolean gate with add_boolean_gate"),
        };

        let mut w = self.write();

        let gate_key = g.get_id();

        w.gate_ids_by_view
            .entry(key)
            .or_default()
            .push(gate_key.clone());

        if g.is_composite() {
            let gates = g.get_inner_gate_ids();
            for sg in gates {
                println!(
                    "Adding composite subgate gate {} with parent {}",
                    sg,
                    parental_gate_id.as_ref().unwrap_or(&ROOTGATE)
                );
                w.hierarchy.add_gate_child(
                    parental_gate_id.clone().unwrap_or(ROOTGATE.clone()),
                    sg.clone(),
                    None,
                )?;
                w.gate_store
                    .primary_and_subgate_registry
                    .insert(sg, g.clone());
            }
        } else {
            println!(
                "Adding gate {} with parent {}",
                g.get_id(),
                parental_gate_id.as_ref().unwrap_or(&ROOTGATE)
            );
            w.hierarchy.add_gate_child(
                parental_gate_id.unwrap_or(ROOTGATE.clone()),
                g.get_id(),
                None,
            )?;
        }

        w.gate_store
            .primary_and_subgate_registry
            .insert(gate_key.clone(), g.clone());

        Ok(())
    }

    fn add_boolean_gate(
        &mut self,
        name: Option<String>,
        operation: BooleanOperation,
        linked_gate_ids: Vec<GateId>,
        parental_gate_id: Option<GateId>,
        x_param: Arc<str>,
        y_param: Arc<str>,
    ) -> anyhow::Result<()> {
        let id = Uuid::new_v4().to_string();
        let gate_id: Arc<str> = Arc::from(id.as_ref() as &str);

        self.boolean_gate_links().with_mut(|w| {
            for link in linked_gate_ids.iter() {
                w.entry(link.clone())
                    .or_insert_with(Vec::new)
                    .push(gate_id.clone());
            }
        });

        let g = Arc::new(BooleanGate::new(
            gate_id.clone(),
            name.unwrap_or(id),
            linked_gate_ids,
            operation,
            x_param,
            y_param,
        )?);

        self.hierarchy().write().add_gate_child(
            parental_gate_id.unwrap_or(ROOTGATE.clone()),
            gate_id.clone(),
            None,
        )?;

        self.gate_store()
            .primary_and_subgate_registry()
            .write()
            .insert(g.get_id(), g.clone());

        Ok(())
    }

    fn remove_gate(&mut self, gate_id: GateId) -> anyhow::Result<()> {
        // build the collection of gates at the same level that need deleting
        // that's any composite 'brothers'
        let mut brothers = vec![];
        if let Some((_id, temp_g)) = self
            .gate_store()
            .primary_and_subgate_registry()
            .peek()
            .get_key_value(&gate_id)
        {
            if temp_g.is_composite() {
                brothers.extend_from_slice(&temp_g.get_inner_gate_ids());
            } else {
                brothers.push(gate_id.clone());
            }
        }
        let mut state = self.write();

        let mut roots: HashSet<Arc<str>> = HashSet::default();
        // and any boolean gates that depend on these gates - and any that depend on them etc
        while let Some(id) = brothers.pop() {
            if roots.insert(id.clone())
                && let Some(deps) = state.boolean_gate_links.remove(&id)
            {
                brothers.extend(deps);
            }
        }

        // Record each doomed gate's parent *before* touching the hierarchy.
        // delete_subtree unlinks every node it removes, so afterwards get_parent
        // returns None and the view key below would be built against the root -
        // leaving the deleted gate's id in gate_ids_by_view, still rendering.
        let mut gates_to_delete: HashSet<Arc<str>> = HashSet::default();
        let mut parents: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();

        for brother in &roots {
            let subtree = std::iter::once(brother.clone())
                .chain(state.hierarchy.get_descendants(brother))
                .collect::<Vec<_>>();

            for doomed in subtree {
                let parent = state
                    .hierarchy
                    .get_parent(&doomed)
                    .cloned()
                    .unwrap_or_else(|| ROOTGATE.clone());
                parents.insert(doomed.clone(), parent);
                gates_to_delete.insert(doomed);
            }
        }

        // A composite is registered under its own id as well as under each of its
        // subgate ids, but only the subgates live in the hierarchy. Pick the owning
        // composite up explicitly or its registry entry outlives the delete and keeps
        // appearing in every resolver built from the registry.
        let mut owners: Vec<(Arc<str>, Arc<str>)> = Vec::new();
        for id in &gates_to_delete {
            if let Some(gate) = state.gate_store.primary_and_subgate_registry.get(id) {
                let owner = gate.get_id();
                if owner != *id && !gates_to_delete.contains(&owner) {
                    let parent = parents.get(id).cloned().unwrap_or_else(|| ROOTGATE.clone());
                    owners.push((owner, parent));
                }
            }
        }
        for (owner, parent) in owners {
            parents.insert(owner.clone(), parent);
            gates_to_delete.insert(owner);
        }

        for brother in roots {
            state.hierarchy.delete_subtree(&brother);
        }

        for doomed_id in &gates_to_delete {
            if let Some((id, gate)) = state
                .gate_store
                .primary_and_subgate_registry
                .remove_entry(doomed_id)
            {
                let drawable_gate_id = gate.get_id();
                let params = gate.get_params();
                let parent = parents
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| ROOTGATE.clone());

                let key = GatesOnPlotKey::new(params.0, params.1, Some(parent));
                if let Some(gate_list) = state.gate_ids_by_view.get_mut(&key) {
                    gate_list.retain(|id| id != &drawable_gate_id);
                }
            }
        }

        // Drop the position overrides for every gate that went, not just the one
        // that was asked for.
        state
            .gate_store
            .sample_position_overrides
            .retain(|(gid, _file_id), _| !gates_to_delete.contains(gid));
        state
            .gate_store
            .group_position_overrides
            .retain(|(gid, _group_id), _| !gates_to_delete.contains(gid));

        // Stop deleted boolean gates lingering as dependents of surviving gates.
        for dependents in state.boolean_gate_links.values_mut() {
            dependents.retain(|id| !gates_to_delete.contains(id));
        }
        state
            .boolean_gate_links
            .retain(|id, dependents| !dependents.is_empty() && !gates_to_delete.contains(id));
        Ok(())
    }

    fn move_gate_point(
        &mut self,
        gate_id: GateId,
        point_idx: usize,
        new_point: (f32, f32),
        plot_map: &PlotMapper,
        resolver: &GateOverrideResolver,
    ) -> anyhow::Result<()> {
        let new_gate = resolver
            .resolve_drawable(&gate_id)?
            .replace_point(new_point, point_idx, plot_map)?;
        let new_gate_arc: Arc<dyn DrawableGate> = Arc::from(new_gate);
        let gate_origin = resolver
            .gate_origins
            .get(&gate_id)
            .ok_or_else(|| anyhow!("error finding gate source for {}", &gate_id))?
            .clone();

        let ids_to_update = if new_gate_arc.is_composite() {
            let mut ids = new_gate_arc.get_inner_gate_ids();
            ids.push(gate_id.clone());
            ids
        } else {
            vec![gate_id.clone()]
        };

        self.gate_store().with_mut(|state| {
            for id in ids_to_update {
                match &gate_origin {
                    GateSource::Global => {
                        state
                            .primary_and_subgate_registry
                            .insert(id.clone(), new_gate_arc.clone());
                    }
                    GateSource::Group(k) => {
                        state
                            .group_position_overrides
                            .insert(k.clone(), new_gate_arc.clone());
                    }
                    GateSource::Sample(k) => {
                        state
                            .sample_position_overrides
                            .insert(k.clone(), new_gate_arc.clone());
                    }
                }
            }
        });
        Ok(())
    }

    fn move_gate(
        &mut self,
        gate_drag_data: GateDragData,
        resolver: &GateOverrideResolver,
    ) -> Result<()> {
        let gate_id = gate_drag_data.gate_id();

        let new_gate = resolver
            .resolve_drawable(&gate_id)?
            .replace_points(gate_drag_data)?;

        let gate_origin = resolver
            .gate_origins
            .get(&gate_id)
            .ok_or_else(|| anyhow!("error finding gate source for {}", &gate_id))?
            .clone();

        if let Some(new_gate) = new_gate {
            let new_gate_arc: Arc<dyn DrawableGate> = Arc::from(new_gate);
            let ids_to_update = if new_gate_arc.is_composite() {
                let mut ids = new_gate_arc.get_inner_gate_ids();
                ids.push(gate_id.clone());
                ids
            } else {
                vec![gate_id.clone()]
            };

            self.gate_store().with_mut(|state| {
                for id in ids_to_update {
                    match &gate_origin {
                        GateSource::Global => {
                            state
                                .primary_and_subgate_registry
                                .insert(id.clone(), new_gate_arc.clone());
                        }
                        GateSource::Group(k) => {
                            state
                                .group_position_overrides
                                .insert(k.clone(), new_gate_arc.clone());
                        }
                        GateSource::Sample(k) => {
                            state
                                .sample_position_overrides
                                .insert(k.clone(), new_gate_arc.clone());
                        }
                    }
                }
            });
        }
        Ok(())
    }

    fn rotate_gate(
        &mut self,
        gate_id: GateId,
        current_position: (f32, f32),
        resolver: &GateOverrideResolver,
    ) -> anyhow::Result<()> {
        let new_gate = resolver
            .resolve_drawable(&gate_id)?
            .rotate_gate(current_position)?;

        let gate_origin = resolver
            .gate_origins
            .get(&gate_id)
            .ok_or_else(|| anyhow!("error finding gate source for {}", &gate_id))?
            .clone();

        if let Some(new_gate) = new_gate {
            let new_gate_arc: Arc<dyn DrawableGate> = Arc::from(new_gate);
            let ids_to_update = if new_gate_arc.is_composite() {
                let mut ids = new_gate_arc.get_inner_gate_ids();
                ids.push(gate_id.clone());
                ids
            } else {
                vec![gate_id.clone()]
            };

            self.gate_store().with_mut(|state| {
                for id in ids_to_update {
                    match &gate_origin {
                        GateSource::Global => {
                            state
                                .primary_and_subgate_registry
                                .insert(id.clone(), new_gate_arc.clone());
                        }
                        GateSource::Group(k) => {
                            state
                                .group_position_overrides
                                .insert(k.clone(), new_gate_arc.clone());
                        }
                        GateSource::Sample(k) => {
                            state
                                .sample_position_overrides
                                .insert(k.clone(), new_gate_arc.clone());
                        }
                    }
                }
            });
        }
        Ok(())
    }

    fn get_gates_for_plot<T>(
        &mut self,
        x_axis_title: T,
        y_axis_title: T,
        parental_gate_id: Option<T>,
        resolver: &GateOverrideResolver,
    ) -> Result<Vec<Arc<dyn DrawableGate>>>
    where
        T: Into<GateId> + Clone,
    {
        let key = GatesOnPlotKey::new(
            x_axis_title.into(),
            y_axis_title.into(),
            parental_gate_id.map(|id| id.into()),
        );
        let key_options = self.gate_ids_by_view().get(key);
        let mut gate_list = vec![];
        if let Some(key_store) = key_options {
            let ids = key_store.read().clone();

            for k in ids {
                if let Ok(gate_store_entry) = resolver.resolve_drawable(&k)
                    && gate_store_entry.is_primary()
                {
                    gate_list.push(gate_store_entry.clone());
                }
            }
        } else {
            return Err(anyhow::anyhow!("No keys found").into());
        }

        Ok(gate_list)
    }

    fn match_gates_to_plot<T>(
        &mut self,
        x_axis_title: T,
        y_axis_title: T,
        parental_gate_id: Option<T>,
        resolver: &GateOverrideResolver,
    ) -> anyhow::Result<()>
    where
        T: Into<GateId> + Clone,
    {
        let (x, y) = (x_axis_title.clone().into(), y_axis_title.clone().into());
        let key = GatesOnPlotKey::new(
            x_axis_title.clone().into(),
            y_axis_title.into(),
            parental_gate_id.map(|id| id.into()),
        );
        let mut updates = Vec::new();
        {
            let key_bind = self.gate_ids_by_view();
            let kbp = &*key_bind.peek();
            let Some(ids) = kbp.get(&key) else {
                return Err(anyhow::anyhow!("No keys found"));
            };

            for k in ids {
                let Some(new_gate) = resolver.resolve_drawable(k)?.match_to_plot_axis(&x, &y)?
                else {
                    continue;
                };
                let new_gate_arc: Arc<dyn DrawableGate> = Arc::from(new_gate);
                let gate_origin = resolver
                    .gate_origins
                    .get(k)
                    .ok_or_else(|| anyhow!("error finding gate source for {}", k))?
                    .clone();

                updates.push((
                    new_gate_arc.get_id(),
                    new_gate_arc.clone(),
                    gate_origin.clone(),
                ));

                if new_gate_arc.is_composite() {
                    for sub_id in new_gate_arc.get_inner_gate_ids() {
                        updates.push((sub_id, new_gate_arc.clone(), gate_origin.clone()));
                    }
                }
            }
        }
        self.gate_store().with_mut(|s| {
            for (k, v, o) in updates {
                match &o {
                    GateSource::Global => {
                        s.primary_and_subgate_registry.insert(k.clone(), v.clone());
                    }
                    GateSource::Group(k) => {
                        s.group_position_overrides.insert(k.clone(), v.clone());
                    }
                    GateSource::Sample(k) => {
                        s.sample_position_overrides.insert(k.clone(), v.clone());
                    }
                }
            }
        });

        Ok(())
    }

    // to do

    fn rescale_gates(
        &mut self,
        marker: &Arc<str>,
        old_axis_options: &AxisInfo,
        new_axis_options: &AxisInfo,
    ) -> Result<(), Vec<String>> {
        let mut errors = vec![];

        self.gate_store().with_mut(|s| {
            let mut memo: FxHashMap<usize, Arc<dyn DrawableGate>> = FxHashMap::default();
            let mut scale_gate = |gate: &Arc<dyn DrawableGate>| -> Arc<dyn DrawableGate> {
                // to avoid rescaling composite gates over and over (as they are also stored under subgate id's)
                // we load any scaled gates into a hashmap
                // we compare by heap memory address - anything pointing to the same address
                // will use the cached rescaled value
                let ptr = Arc::as_ptr(gate) as *const () as usize;
                if let Some(scaled) = memo.get(&ptr) {
                    return scaled.clone();
                }
                let (x_marker, y_marker) = gate.get_params();
                // let is_x = marker == &x_marker;
                // let data_range = if is_x {
                //     (*(plot_map.x_data_min_max().start()), *(plot_map.x_data_min_max().end()))
                // } else {
                //     (*(plot_map.y_data_min_max().start()), *(plot_map.y_data_min_max().end()))
                // };
                
                if marker == &x_marker || marker == &y_marker {
                    let new_gate = match gate.recalculate_gate_for_rescaled_axis(
                        marker.clone(),
                        &old_axis_options.transform,
                        &new_axis_options.transform,
                        // data_range,
                        (new_axis_options.axis_lower, new_axis_options.axis_upper),
                    ) {
                        Ok(new_gate) => Arc::from(new_gate),
                        Err(e) => {
                            errors.push(e.to_string());
                            gate.clone()
                        }
                    };
                    memo.insert(ptr, new_gate.clone());
                    new_gate
                } else {
                    gate.clone()
                }
            };

            s.primary_and_subgate_registry = GateMap(
                s.primary_and_subgate_registry
                    .iter()
                    .map(|(id, gate)| (id.clone(), scale_gate(gate)))
                    .collect(),
            );

            s.sample_position_overrides = s
                .sample_position_overrides
                .iter()
                .map(|(key, gate)| (key.clone(), scale_gate(gate)))
                .collect();

            s.group_position_overrides = s
                .group_position_overrides
                .iter()
                .map(|(key, gate)| (key.clone(), scale_gate(gate)))
                .collect();
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn set_current_axis_limits(
        &mut self,
        axis_name: Arc<str>,
        lower: f32,
        upper: f32,
        transform: TransformType,
    ) -> Result<(), Vec<String>> {
        let mut errors = vec![];

        self.gate_store().with_mut(|s| {
            let mut memo: FxHashMap<usize, Arc<dyn DrawableGate>> = FxHashMap::default();
            let mut scale_gate = |gate: &Arc<dyn DrawableGate>| -> Arc<dyn DrawableGate> {
                // to avoid rescaling composite gates over and over (as they are also stored under subgate id's)
                // we load any scaled gates into a hashmap
                // we compare by heap memory address - anything pointing to the same address
                // will use the cached rescaled value
                let ptr = Arc::as_ptr(gate) as *const () as usize;
                if let Some(scaled) = memo.get(&ptr) {
                    return scaled.clone();
                }
                let (x_marker, y_marker) = gate.get_params();
                if axis_name == x_marker || axis_name == y_marker {
                    let new_gate = match gate.recalculate_gate_for_new_axis_limits(
                        axis_name.clone(),
                        lower,
                        upper,
                        &transform,
                    ) {
                        Ok(Some(new_gate)) => Arc::from(new_gate),
                        Ok(None) => gate.clone(),
                        Err(e) => {
                            errors.push(e.to_string());
                            gate.clone()
                        }
                    };
                    memo.insert(ptr, new_gate.clone());
                    new_gate
                } else {
                    gate.clone()
                }
            };

            s.primary_and_subgate_registry = GateMap(
                s.primary_and_subgate_registry
                    .iter()
                    .map(|(id, gate)| (id.clone(), scale_gate(gate)))
                    .collect(),
            );

            s.sample_position_overrides = s
                .sample_position_overrides
                .iter()
                .map(|(key, gate)| (key.clone(), scale_gate(gate)))
                .collect();

            s.group_position_overrides = s
                .group_position_overrides
                .iter()
                .map(|(key, gate)| (key.clone(), scale_gate(gate)))
                .collect();
        });

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn get_gate_name(&self, id: GateId) -> Option<String> {
        if let Some(g) = self
            .gate_store()
            .peek()
            .primary_and_subgate_registry
            .get(&id)
        {
            return Some(g.get_name().to_string());
        }

        None
    }

    fn upload_gates_from_file(
        &mut self,
        path: PathBuf,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axis_settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    ) -> anyhow::Result<()> {
        // 1. Open the file
        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);

        // 2. Deserialize into your ExperimentJson struct
        let experiment: crate::omiq::deserialise::ExperimentJson = serde_json::from_reader(reader)?;

        let mut reachable: FxHashSet<Arc<str>> = FxHashSet::default();
        for node in experiment.tree.nodes.values() {
            collect_reachable(&node.filter_container_id, &experiment.tree.filter_containers, &mut reachable);
        }

        let mut composite_gates: std::collections::HashMap<
            CompositeType,
            Vec<(u32, crate::omiq::deserialise::AtomicContainer)>,
            FxBuildHasher,
        > = FxHashMap::default();
        let mut primary_gates = vec![];
        let mut boolean_gates = vec![];
        // step 1 is to separate the composite gates from the primary gates
        for (id, container) in &experiment.tree.filter_containers {
            if !reachable.contains(id){
                continue
            }
            let container = match container {
                FilterContainer::Atomic(atomic_container) => atomic_container,
                FilterContainer::Compound(compound_container) => {
                    boolean_gates.push(compound_container.clone());
                    continue;
                }
            };

            if container.group_id.is_none() {
                primary_gates.push(container.clone());
                continue;
            }
            let group_id_unprocessed = container.group_id.as_ref().unwrap();

            let group_id = group_id_unprocessed
                .split('_')
                .nth(0)
                .ok_or_else(|| anyhow::anyhow!("Error processing composite gate id"))?;
            let group_position = group_id_unprocessed
                .chars()
                .last()
                .and_then(|c| c.to_digit(10))
                .ok_or_else(|| anyhow::anyhow!("Error processing composite gate id"))?;

            let composite_type = if group_id_unprocessed.contains("SPLIT") {
                CompositeType::Bisector(group_id.to_string())
            } else if group_id_unprocessed.contains("SKEWEDQUAD") {
                CompositeType::SkewedQuadrant(group_id.to_string())
            } else if group_id_unprocessed.contains("QUAD") {
                CompositeType::Quadrant(group_id.to_string())
            } else {
                return Err(anyhow::anyhow!(
                    "Unknown composite gate type in id {}",
                    group_id_unprocessed
                ));
            };
            composite_gates
                .entry(composite_type)
                .or_default()
                .push((group_position, container.clone()));
        }

        // the parent id's are node id's rather than gate id's so need to initially map these
        let mut node_to_gate_id: FxHashMap<Arc<str>, GateId> = FxHashMap::default();

        for (node_id, node) in experiment.tree.nodes.iter() {
            node_to_gate_id.insert(node_id.clone(), node.filter_container_id.clone());
        }

        let mut w = self.write();

        let mut sorted_nodes: Vec<_> = experiment.tree.nodes.values().collect();

        // 2. Sort nodes by their depth in the tree
        // This ensures parents always exist before children
        sorted_nodes.sort_by_cached_key(|node| {
            let mut depth = 0;
            let mut current_parent: &str = &node.parent_id;
            
            // Walk up the tree to the root to find the depth
            while current_parent != "" {
                if let Some(parent) = experiment.tree.nodes.get(current_parent) {
                    current_parent = &parent.parent_id;
                    depth += 1;
                } else {
                    // Parent ID exists but isn't in the map (shouldn't happen with clean data)
                    break;
                }
            }
            depth
        });

        // build the hierarchy first.
        for node in sorted_nodes.into_iter() {
            // deal with composites - you need to add the sub-gates not the gates
            let parent_id = if *"" != *node.parent_id {
                node_to_gate_id
                    .get(&node.parent_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Could not find parent gate id for node {}", node.parent_id)
                    })?
                    .clone()
            } else {
                ROOTGATE.clone()
            };
            w.hierarchy
                .add_gate_child(parent_id, node.filter_container_id.clone(), Some(node.ord))?;
            node_to_gate_id.insert(node.id.clone(), node.filter_container_id.clone());
        }

        // 3. Iterate through the primary gates containers and process them
        for container in primary_gates {
            // Process the container using your logic
            let drawables = container.process_gates_to_drawable(metadata)?;

            for (source, gate) in drawables {
                // 4. Insert into your Store based on Source
                match source {
                    GateSource::Global => {
                        let gate_id = gate.get_id();
                        let parent =
                            w.hierarchy.get_parent(&gate_id).cloned().ok_or_else(|| {
                                anyhow::anyhow!("Could not locate parent of {} in hierarchy", gate_id)
                            })?;
                        let params = gate.get_params();
                        let key = GatesOnPlotKey::new(params.0, params.1, Some(parent));
                        w.gate_ids_by_view
                            .entry(key)
                            .or_default()
                            .push(gate.get_id());
                        w.gate_store
                            .primary_and_subgate_registry
                            .insert(gate.get_id(), gate);
                    }
                    GateSource::Group(key) => {
                        w.gate_store.group_position_overrides.insert(key, gate);
                    }
                    GateSource::Sample(key) => {
                        w.gate_store.sample_position_overrides.insert(key, gate);
                    }
                }
            }
        }

        for (composite_type, mut subgates) in composite_gates {
            subgates.sort_by_key(|(pos, _)| *pos);
            let to_add = get_composite_gates_from_filter_container(
                composite_type,
                &subgates,
                &axis_settings,
                metadata,
            )?;
            for ((id, source), gate) in to_add {
                let subgate_ids = gate.get_inner_gate_ids();
                match source {
                    GateSource::Global => {
                        let any_subgate = subgate_ids
                            .first()
                            .ok_or_else(|| anyhow::anyhow!("Composite gate has no subgates"))?;
                        let parent =
                            w.hierarchy
                                .get_parent(any_subgate)
                                .cloned()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Could not locate parent of subgate {} in hierarchy", any_subgate)
                                })?;
                        let params = gate.get_params();
                        let key = GatesOnPlotKey::new(params.0, params.1, Some(parent));

                        w.gate_ids_by_view.entry(key).or_default().push(id.clone());
                        w.gate_store
                            .primary_and_subgate_registry
                            .insert(id.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            w.gate_store
                                .primary_and_subgate_registry
                                .insert(sub_id, gate.clone());
                        }
                    }
                    GateSource::Group(key) => {
                        w.gate_store
                            .group_position_overrides
                            .insert(key.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            w.gate_store
                                .group_position_overrides
                                .insert((sub_id, key.1.clone()), gate.clone());
                        }
                    }
                    GateSource::Sample(key) => {
                        w.gate_store
                            .sample_position_overrides
                            .insert(key.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            w.gate_store
                                .sample_position_overrides
                                .insert((sub_id, key.1.clone()), gate.clone());
                        }
                    }
                }
            }
        }

        for boolean_gate in boolean_gates.iter() {
            let (x_param, y_param) =
                find_atomic_params(&boolean_gate.id, &experiment.tree.filter_containers)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "could not find operands for boolean gate {}",
                            boolean_gate.id.clone()
                        )
                    })?;
            let op = match boolean_gate.operation {
                BooleanOpType::And => BooleanOperation::And,
                BooleanOpType::Or => BooleanOperation::Or,
                BooleanOpType::Not => BooleanOperation::Not,
            };
            let bool_gate = BooleanGate::new(
                boolean_gate.id.clone(),
                boolean_gate.name.to_string(),
                boolean_gate.filter_container_ids.clone(),
                op,
                x_param,
                y_param,
            )?;

            let arc_gate: Arc<dyn DrawableGate> = Arc::new(bool_gate);
            for link_id in &boolean_gate.filter_container_ids {
                w.boolean_gate_links
                    .entry(link_id.clone())
                    .or_default()
                    .push(boolean_gate.id.clone());
            }
            w.gate_store
                .primary_and_subgate_registry
                .insert(arc_gate.get_id(), arc_gate);
        }

        Ok(())
    }
}

// Collect all reachable filterContainer IDs from the tree nodes,
// recursively following CompoundFilterContainer sub-ids.
fn collect_reachable(
    fc_id: &str,
    filter_containers: &std::collections::HashMap<Arc<str>, FilterContainer>,
    reachable: &mut rustc_hash::FxHashSet<Arc<str>>,
) {
    if !reachable.insert(fc_id.into()) {
        return; // already visited
    }

    if let Some(FilterContainer::Compound(c)) = filter_containers.get(fc_id) {
        for sub_id in &c.filter_container_ids {
            collect_reachable(sub_id, filter_containers, reachable);
        }
    }
    
}