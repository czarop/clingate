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

/// The plain-data half of the gate store.
///
/// These take `&mut GateSubStore` rather than a `Store` lens so they can be
/// exercised without a Dioxus runtime; the store methods are thin wrappers that
/// keep the same write granularity.
impl GateSubStore {
    /// Every key a gate occupies. A composite is registered under its own id
    /// *and* under each of its subgate ids, all aliased to the same `Arc`.
    pub fn ids_for(gate: &Arc<dyn DrawableGate>, gate_id: &GateId) -> Vec<GateId> {
        if gate.is_composite() {
            let mut ids = gate.get_inner_gate_ids();
            ids.push(gate_id.clone());
            ids
        } else {
            vec![gate_id.clone()]
        }
    }

    /// Write an edited gate back into the tier it was resolved from.
    ///
    /// Overrides are keyed by `(gate id, sample or group)`, so each of a
    /// composite's ids needs its own entry. Reusing the resolved gate's key for
    /// all of them leaves the subgate overrides pointing at the pre-edit gate -
    /// and the subgate entries are exactly what filtering and statistics read,
    /// so the gate would appear to move while still gating its old position.
    pub fn insert_for_source(
        &mut self,
        ids: &[GateId],
        gate: &Arc<dyn DrawableGate>,
        origin: &GateSource,
    ) {
        for id in ids {
            match origin {
                GateSource::Global => {
                    self.primary_and_subgate_registry
                        .insert(id.clone(), gate.clone());
                }
                GateSource::Group((_, group_key)) => {
                    self.group_position_overrides
                        .insert((id.clone(), group_key.clone()), gate.clone());
                }
                GateSource::Sample((_, file_id)) => {
                    self.sample_position_overrides
                        .insert((id.clone(), file_id.clone()), gate.clone());
                }
            }
        }
    }

    /// Apply a transform to every stored gate across all three tiers.
    ///
    /// Results are memoised by heap address: a composite is aliased under
    /// several keys, so without this the transform would be applied once per
    /// subgate id and compound on itself.
    pub fn map_gates<F>(&mut self, mut transform: F)
    where
        F: FnMut(&Arc<dyn DrawableGate>) -> Arc<dyn DrawableGate>,
    {
        let mut memo: FxHashMap<usize, Arc<dyn DrawableGate>> = FxHashMap::default();
        let mut apply = |gate: &Arc<dyn DrawableGate>| -> Arc<dyn DrawableGate> {
            let ptr = Arc::as_ptr(gate) as *const () as usize;
            if let Some(done) = memo.get(&ptr) {
                return done.clone();
            }
            let mapped = transform(gate);
            memo.insert(ptr, mapped.clone());
            mapped
        };

        let registry: FxHashMap<GateId, Arc<dyn DrawableGate>> = self
            .primary_and_subgate_registry
            .iter()
            .map(|(id, gate)| (id.clone(), apply(gate)))
            .collect();
        let samples: SampleGateMap = self
            .sample_position_overrides
            .iter()
            .map(|(key, gate)| (key.clone(), apply(gate)))
            .collect();
        let groups: GroupGateMap = self
            .group_position_overrides
            .iter()
            .map(|(key, gate)| (key.clone(), apply(gate)))
            .collect();

        self.primary_and_subgate_registry = GateMap(registry);
        self.sample_position_overrides = samples;
        self.group_position_overrides = groups;
    }
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

/// One appearance of a gate in the gating tree.
///
/// Omiq keys nodes separately from filter containers, so one gate can be
/// applied at several points in the tree - 38 of 146 containers in a real
/// export, one of them at nine points. `GateId` answers "which gate is this"
/// (geometry, name, per-file positions); `NodeId` answers "where in the tree",
/// and several nodes may name the same gate. Conflating the two is what
/// collapsed 250 placements to 146 on import.
///
/// A newtype rather than an alias so the two cannot be passed for one another.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NodeId(Arc<str>);

impl NodeId {
    pub fn as_arc(&self) -> &Arc<str> {
        &self.0
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Arc<str>> for NodeId {
    fn from(id: Arc<str>) -> Self {
        Self(id)
    }
}

impl From<&str> for NodeId {
    fn from(id: &str) -> Self {
        Self(Arc::from(id))
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What one node in the tree holds: which gate it shows, and its own display
/// state. The tree position itself (parent, sibling order) lives in the
/// hierarchy, keyed by this node's id.
#[derive(Clone, Debug, PartialEq)]
pub struct GatePlacement {
    /// The gate this node displays. Shared between linked nodes.
    pub gate_id: GateId,
    /// Omiq's per-node collapsed flag.
    pub collapsed: bool,
}

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
    // What the imported Omiq file carried that the editor does not itself need,
    // kept so a new file can be written from scratch.
    omiq_rebuild: crate::omiq::rebuild::OmiqRebuildStore,
    // Which gate each node in the hierarchy shows. Every id the hierarchy holds
    // is a NodeId and has an entry here.
    //
    // Stage 1 mints node ids equal to the gate id, so the mapping is one-to-one
    // and nothing changes behaviourally; the table is what lets a later stage
    // mint a node per Omiq GatingNode without touching anything that reads it.
    placements: FxHashMap<NodeId, GatePlacement>,
    // Reverse index of `placements`, so "where does this gate appear?" does not
    // scan. Rebuilt through the same two methods that write `placements`, never
    // separately, so the two cannot drift.
    nodes_by_gate: FxHashMap<GateId, Vec<NodeId>>,
}

impl GateState {
    /// What the imported Omiq file carried, for writing a new one.
    pub fn omiq_rebuild(&self) -> &crate::omiq::rebuild::OmiqRebuildStore {
        &self.omiq_rebuild
    }

    /// Record that `node` shows `gate`. Call whenever an id is put into the
    /// hierarchy, so the two never disagree about what is in the tree.
    fn record_placement(&mut self, node: NodeId, gate_id: GateId, collapsed: bool) {
        if let Some(previous) = self.placements.insert(
            node.clone(),
            GatePlacement {
                gate_id: gate_id.clone(),
                collapsed,
            },
        ) && previous.gate_id != gate_id
        {
            // Re-pointed at a different gate: drop it from the old gate's list.
            Self::detach_node(&mut self.nodes_by_gate, &previous.gate_id, &node);
        }
        let nodes = self.nodes_by_gate.entry(gate_id).or_default();
        if !nodes.contains(&node) {
            nodes.push(node);
        }
    }

    /// Forget a node. The gate itself is untouched: it survives while any other
    /// node still shows it, which is what makes deleting one instance of a
    /// linked gate different from deleting the gate.
    fn forget_placement(&mut self, node: &NodeId) -> Option<GatePlacement> {
        let placement = self.placements.remove(node)?;
        Self::detach_node(&mut self.nodes_by_gate, &placement.gate_id, node);
        Some(placement)
    }

    fn detach_node(
        nodes_by_gate: &mut FxHashMap<GateId, Vec<NodeId>>,
        gate_id: &GateId,
        node: &NodeId,
    ) {
        if let Some(nodes) = nodes_by_gate.get_mut(gate_id) {
            nodes.retain(|n| n != node);
            if nodes.is_empty() {
                nodes_by_gate.remove(gate_id);
            }
        }
    }

    /// The gate a node shows.
    pub fn gate_for_node(&self, node: &NodeId) -> Option<&GateId> {
        self.placements.get(node).map(|p| &p.gate_id)
    }

    /// Every point in the tree where this gate appears, in insertion order.
    pub fn nodes_for_gate(&self, gate_id: &GateId) -> &[NodeId] {
        self.nodes_by_gate
            .get(gate_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// How many points in the tree show this gate.
    pub fn placement_count(&self, gate_id: &GateId) -> usize {
        self.nodes_for_gate(gate_id).len()
    }

    /// Whether this gate is applied at more than one point - Omiq calls this a
    /// linked gate. Derived from the node table rather than stored, so it
    /// cannot fall out of step with the tree.
    pub fn is_linked(&self, gate_id: &GateId) -> bool {
        self.placement_count(gate_id) > 1
    }

    /// A gate with no node. Registered and evaluable - a live boolean may still
    /// reference it - but drawn nowhere. Omiq leaves these behind too.
    pub fn is_ghost(&self, gate_id: &GateId) -> bool {
        self.is_registered(gate_id) && self.placement_count(gate_id) == 0
    }

    /// How many gates are registered, counting a composite once per key it
    /// occupies.
    pub fn gate_count(&self) -> usize {
        self.gate_store.primary_and_subgate_registry.len()
    }

    /// Whether a gate id resolves to anything - which is what a boolean gate's
    /// operand lookup needs at filter time.
    pub fn is_registered(&self, gate_id: &GateId) -> bool {
        self.gate_store
            .primary_and_subgate_registry
            .contains_key(gate_id)
    }

    /// The gate that applies to one sample: a per-sample override wins, then a
    /// per-group override, then the global position.
    ///
    /// The same precedence `get_current_sample` uses, for one gate rather than
    /// all of them - the export needs it per file when writing `perFileFilters`.
    pub fn gate_for_file(
        &self,
        gate_id: &GateId,
        file_id: &FileId,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
    ) -> Option<Arc<dyn DrawableGate>> {
        if let Some(gate) = self
            .gate_store
            .sample_position_overrides
            .get(&(gate_id.clone(), file_id.clone()))
        {
            return Some(gate.clone());
        }

        if let Some(groups) = metadata.get(file_id) {
            for (parameter, group) in groups {
                let key = MetaDataKey {
                    parameter: parameter.clone(),
                    group: group.clone(),
                };
                if let Some(gate) = self
                    .gate_store
                    .group_position_overrides
                    .get(&(gate_id.clone(), key))
                {
                    return Some(gate.clone());
                }
            }
        }

        self.registered_gate(gate_id)
    }

    /// Every gate id in the registry.
    pub fn registered_ids(&self) -> Vec<GateId> {
        self.gate_store
            .primary_and_subgate_registry
            .keys()
            .cloned()
            .collect()
    }

    /// This gate's sort order among its siblings, for writing Omiq's `ord`.
    pub fn gate_order(&self, gate_id: &GateId) -> Option<u64> {
        self.hierarchy.get_order(gate_id)
    }

    /// The gate registered under an id, if any.
    pub fn registered_gate(&self, gate_id: &GateId) -> Option<Arc<dyn DrawableGate>> {
        self.gate_store
            .primary_and_subgate_registry
            .get(gate_id)
            .cloned()
    }

    /// This gate's parent in the gating tree, or `None` if it has no node.
    pub fn hierarchy_parent(&self, gate_id: &GateId) -> Option<GateId> {
        self.hierarchy.get_parent(gate_id).cloned()
    }

    /// Whether the gate is listed on any plot. A nodeless container is
    /// registered but drawn nowhere.
    pub fn is_on_any_plot(&self, gate_id: &GateId) -> bool {
        self.gate_ids_by_view
            .values()
            .any(|ids| ids.contains(gate_id))
    }

    /// Delete a gate, its subtree, and anything that depended on it.
    pub fn remove_gate(&mut self, gate_id: GateId) -> anyhow::Result<()> {
        // build the collection of gates at the same level that need deleting
        // that's any composite 'brothers'
        let mut brothers = vec![];
        if let Some((_id, temp_g)) = self
            .gate_store
            .primary_and_subgate_registry
            .get_key_value(&gate_id)
        {
            if temp_g.is_composite() {
                brothers.extend_from_slice(&temp_g.get_inner_gate_ids());
            } else {
                brothers.push(gate_id.clone());
            }
        }

        let mut roots: HashSet<Arc<str>> = HashSet::default();
        // and any boolean gates that depend on these gates - and any that depend on them etc
        while let Some(id) = brothers.pop() {
            if roots.insert(id.clone())
                && let Some(deps) = self.boolean_gate_links.remove(&id)
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
                .chain(self.hierarchy.get_descendants(brother))
                .collect::<Vec<_>>();

            for doomed in subtree {
                let parent = self
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
            if let Some(gate) = self.gate_store.primary_and_subgate_registry.get(id) {
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
            for removed in self.hierarchy.delete_subtree(&brother) {
                self.forget_placement(&NodeId::from(removed));
            }
        }

        for doomed_id in &gates_to_delete {
            if let Some((id, gate)) = self
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
                if let Some(gate_list) = self.gate_ids_by_view.get_mut(&key) {
                    gate_list.retain(|id| id != &drawable_gate_id);
                }
            }
        }

        // Drop the position overrides for every gate that went, not just the one
        // that was asked for.
        self
            .gate_store
            .sample_position_overrides
            .retain(|(gid, _file_id), _| !gates_to_delete.contains(gid));
        self
            .gate_store
            .group_position_overrides
            .retain(|(gid, _group_id), _| !gates_to_delete.contains(gid));

        // A deleted gate must not be written back into an Omiq file, so its
        // rebuild entry goes with it. Ghost containers are untouched: they are
        // keyed separately and were never gates in this editor.
        self.omiq_rebuild
            .gates
            .retain(|id, _| !gates_to_delete.contains(id));

        // Stop deleted boolean gates lingering as dependents of surviving gates.
        for dependents in self.boolean_gate_links.values_mut() {
            dependents.retain(|id| !gates_to_delete.contains(id));
        }
        self
            .boolean_gate_links
            .retain(|id, dependents| !dependents.is_empty() && !gates_to_delete.contains(id));
        Ok(())
    }
}

impl GateState {
    /// Create a gate of the given type and register it on its plot and in the
    /// hierarchy.
    pub fn add_gate(
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
        // Normalise "no parent" to the root here as well as in the hierarchy
        // below. Keying the view index on a bare None would file the gate under
        // a key that remove_gate and get_gates_for_plot - which both ask for
        // Some(ROOTGATE) - would never look under, so it could never be found
        // again to redraw or delete.
        let parental_gate_id = Some(parental_gate_id.unwrap_or_else(|| ROOTGATE.clone()));
        let key = GatesOnPlotKey::new(x_param.clone(), y_param.clone(), parental_gate_id.clone());
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


        let gate_key = g.get_id();

        self.gate_ids_by_view
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
                self.hierarchy.add_gate_child(
                    parental_gate_id.clone().unwrap_or(ROOTGATE.clone()),
                    sg.clone(),
                    None,
                )?;
                self.record_placement(NodeId::from(sg.clone()), sg.clone(), false);
                self.gate_store
                    .primary_and_subgate_registry
                    .insert(sg, g.clone());
            }
        } else {
            println!(
                "Adding gate {} with parent {}",
                g.get_id(),
                parental_gate_id.as_ref().unwrap_or(&ROOTGATE)
            );
            self.hierarchy.add_gate_child(
                parental_gate_id.unwrap_or(ROOTGATE.clone()),
                g.get_id(),
                None,
            )?;
            self.record_placement(NodeId::from(g.get_id()), g.get_id(), false);
        }

        self.gate_store
            .primary_and_subgate_registry
            .insert(gate_key.clone(), g.clone());

        Ok(())
    }
}

impl GateState {
    /// Resolve every gate for one sample: a per-sample override wins, then a
    /// per-group override, then the global position.
    pub fn get_current_sample(
        &self,
        file_id: FileId,
        group_ids: &FxHashMap<MetaDataParameter, GroupId>,
    ) -> GateOverrideResolver {
        // construct the GateResolver for this file
        let mut active_gates: im::HashMap<Arc<str>, ComparableGate, FxBuildHasher> =
            im::HashMap::with_hasher(FxBuildHasher);
        let mut gate_origins: im::HashMap<Arc<str>, GateSource, FxBuildHasher> =
            im::HashMap::with_hasher(FxBuildHasher);

        {
            let registry = &self.gate_store.primary_and_subgate_registry;
            let sample_overrides = &self.gate_store.sample_position_overrides;
            let group_overrides = &self.gate_store.group_position_overrides;

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

        GateOverrideResolver {
            active_gates,
            gate_origins,
        }
    }
}

impl GateState {
    /// Build the gate tree from an Omiq experiment export.
    pub fn upload_gates_from_file(
        &mut self,
        path: PathBuf,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axis_settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    ) -> anyhow::Result<()> {
        // 1. Open the file
        let text = std::fs::read_to_string(&path)?;

        // 2. Deserialize into your ExperimentJson struct, and keep an untyped
        // view alongside it for the fields the typed one does not model.
        let experiment: crate::omiq::deserialise::ExperimentJson = serde_json::from_str(&text)?;
        let raw: serde_json::Value = serde_json::from_str(&text)?;

        let mut reachable: FxHashSet<Arc<str>> = FxHashSet::default();
        for node in experiment.tree.nodes.values() {
            collect_reachable(&node.filter_container_id, &experiment.tree.filter_containers, &mut reachable);
        }

        // Capture what the file carries before any of it is turned into gates.
        let mut rebuild = crate::omiq::rebuild::OmiqRebuildStore::capture(&experiment, &raw);
        rebuild.capture_ghosts(&raw, &reachable);
        self.omiq_rebuild = rebuild;

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
            // Stage 1 still keys the tree on the container, so two nodes sharing
            // one container collapse to a single position - this is the line
            // that loses a linked gate's other placements. The node table below
            // is recorded through the same call so that a later stage can key
            // on `node.id` here and everything that reads placements follows.
            self.hierarchy
                .add_gate_child(parent_id, node.filter_container_id.clone(), Some(node.ord))?;
            self.record_placement(
                NodeId::from(node.filter_container_id.clone()),
                node.filter_container_id.clone(),
                node.collapsed,
            );
            node_to_gate_id.insert(node.id.clone(), node.filter_container_id.clone());
        }

        // A composite is all-or-nothing in Omiq, so a group that arrives
        // incomplete is not a composite any more. Deleting one strips every node
        // and every container in the group *except* any member a live boolean
        // gate still references - that survivor stays behind as a nodeless
        // container so the boolean can still be evaluated.
        //
        // Such a survivor is fully self-describing on its own (a quadrant corner
        // is just a rectangle), so import it as a standalone gate rather than
        // failing the entire import on the arity check below. Its original
        // groupId is recovered from the raw container for export, not from here.
        let mut orphaned_subgates = Vec::new();
        composite_gates.retain(|composite_type, members| {
            let expected = match composite_type {
                CompositeType::Bisector(_) => 2,
                CompositeType::Quadrant(_) | CompositeType::SkewedQuadrant(_) => 4,
            };
            if members.len() == expected {
                return true;
            }
            orphaned_subgates.extend(members.drain(..).map(|(_, container)| container));
            false
        });
        for mut container in orphaned_subgates {
            container.group_id = None;
            primary_gates.push(container);
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
                        // Every container that has a node is in the hierarchy, so
                        // a missing parent means this one has no node: a ghost
                        // kept alive only by a boolean gate that references it.
                        // It belongs on no plot, but it must still be registered
                        // or that boolean cannot resolve its operand.
                        if let Some(parent) = self.hierarchy.get_parent(&gate_id).cloned() {
                            let params = gate.get_params();
                            let key = GatesOnPlotKey::new(params.0, params.1, Some(parent));
                            self.gate_ids_by_view
                                .entry(key)
                                .or_default()
                                .push(gate_id.clone());
                        }
                        self.gate_store
                            .primary_and_subgate_registry
                            .insert(gate_id, gate);
                    }
                    GateSource::Group(key) => {
                        self.gate_store.group_position_overrides.insert(key, gate);
                    }
                    GateSource::Sample(key) => {
                        self.gate_store.sample_position_overrides.insert(key, gate);
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
                            self.hierarchy
                                .get_parent(any_subgate)
                                .cloned()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Could not locate parent of subgate {} in hierarchy", any_subgate)
                                })?;
                        let params = gate.get_params();
                        let key = GatesOnPlotKey::new(params.0, params.1, Some(parent));

                        self.gate_ids_by_view.entry(key).or_default().push(id.clone());
                        self.gate_store
                            .primary_and_subgate_registry
                            .insert(id.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            self.gate_store
                                .primary_and_subgate_registry
                                .insert(sub_id, gate.clone());
                        }
                    }
                    GateSource::Group(key) => {
                        self.gate_store
                            .group_position_overrides
                            .insert(key.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            self.gate_store
                                .group_position_overrides
                                .insert((sub_id, key.1.clone()), gate.clone());
                        }
                    }
                    GateSource::Sample(key) => {
                        self.gate_store
                            .sample_position_overrides
                            .insert(key.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            self.gate_store
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
                self.boolean_gate_links
                    .entry(link_id.clone())
                    .or_default()
                    .push(boolean_gate.id.clone());
            }
            self.gate_store
                .primary_and_subgate_registry
                .insert(arc_gate.get_id(), arc_gate);
        }

        Ok(())
    }
}

#[store(pub name = GateStateImplExt)]
impl<Lens> Store<GateState, Lens> {
    fn get_current_sample(
        &mut self,
        file_id: FileId,
        group_ids: &FxHashMap<MetaDataParameter, GroupId>,
    ) -> Result<GateOverrideResolver> {
        Ok(self.peek().get_current_sample(file_id, group_ids))
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
        self.write().add_gate(
            mapper,
            click_x,
            click_y,
            x_param,
            y_param,
            points,
            parental_gate_id,
            gate_type,
            name,
        )?;
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
        self.write().remove_gate(gate_id)
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

        let ids_to_update = GateSubStore::ids_for(&new_gate_arc, &gate_id);

        self.gate_store().with_mut(|state| {
            state.insert_for_source(&ids_to_update, &new_gate_arc, &gate_origin);
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
            let ids_to_update = GateSubStore::ids_for(&new_gate_arc, &gate_id);

            self.gate_store().with_mut(|state| {
                state.insert_for_source(&ids_to_update, &new_gate_arc, &gate_origin);
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
            let ids_to_update = GateSubStore::ids_for(&new_gate_arc, &gate_id);

            self.gate_store().with_mut(|state| {
                state.insert_for_source(&ids_to_update, &new_gate_arc, &gate_origin);
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
            for (id, gate, origin) in updates {
                s.insert_for_source(&[id], &gate, &origin);
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
            s.map_gates(|gate| {
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
                    new_gate
                } else {
                    gate.clone()
                }
            });
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
            s.map_gates(|gate| {
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
                    new_gate
                } else {
                    gate.clone()
                }
            });
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
        self.write()
            .upload_gates_from_file(path, metadata, axis_settings)
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
//cargo test gate_store_tests -- --nocapture
// ─── Tests ────────────────────────────────────────────────────────────────────
//
// These exercise the plain-data half of the store. They live in this file rather
// than a sibling module so they can reach GateState's private fields, which is
// what lets them assert on the registry, the view index and the override maps
// directly rather than through the Dioxus lenses.

#[cfg(test)]
mod gate_store_tests {
    use super::*;
    use crate::gate_editor::gates::gate_types::PrimaryGateType;
    use flow_gates::create_rectangle_geometry;

    const X: &str = "FSC-A";
    const Y: &str = "SSC-A";

    fn mapper() -> PlotMapper {
        PlotMapper::new(
            600.0,
            600.0,
            0.0..=1000.0,
            0.0..=1000.0,
            0.0..=1000.0,
            0.0..=1000.0,
            TransformType::Linear,
            TransformType::Linear,
        )
    }

    fn rectangle(id: &str) -> Arc<dyn DrawableGate> {
        let geometry = create_rectangle_geometry(
            vec![(10.0, 10.0), (90.0, 10.0), (90.0, 90.0), (10.0, 90.0)],
            X,
            Y,
        )
        .unwrap();
        let gate = Gate {
            id: Arc::from(id),
            name: id.to_string(),
            geometry,
            mode: flow_gates::GateMode::Global,
            parameters: (Arc::from(X), Arc::from(Y)),
            label_position: None,
        };
        Arc::new(RectangleGate::try_new(gate, true).unwrap())
    }

    fn quadrant(id: &str) -> Arc<dyn DrawableGate> {
        Arc::new(
            QuadrantGate::try_new_from_raw_coord(
                &mapper(),
                Arc::from(id),
                id.to_string(),
                (300.0, 300.0),
                Arc::from(X),
                Arc::from(Y),
            )
            .unwrap(),
        )
    }

    fn file(id: &str) -> FileId {
        Arc::from(id)
    }

    fn group_key(parameter: &str, group: &str) -> MetaDataKey {
        MetaDataKey {
            parameter: Arc::from(parameter),
            group: Arc::from(group),
        }
    }

    // ── GateSubStore::ids_for ─────────────────────────────────────────────────

    #[test]
    fn a_single_gate_occupies_one_key() {
        let g = rectangle("r");
        assert_eq!(GateSubStore::ids_for(&g, &g.get_id()), vec![g.get_id()]);
    }

    #[test]
    fn a_composite_occupies_its_own_key_and_each_subgate_key() {
        let g = quadrant("q");
        let ids = GateSubStore::ids_for(&g, &g.get_id());

        assert_eq!(ids.len(), 5, "four quadrants plus the composite itself");
        assert!(ids.contains(&g.get_id()));
        for sub in g.get_inner_gate_ids() {
            assert!(ids.contains(&sub), "missing subgate {sub}");
        }
    }

    // ── GateSubStore::insert_for_source ───────────────────────────────────────

    #[test]
    fn a_global_gate_is_written_to_the_registry() {
        let mut store = GateSubStore::default();
        let g = rectangle("r");

        store.insert_for_source(&[g.get_id()], &g, &GateSource::Global);

        assert!(store.primary_and_subgate_registry.contains_key(&g.get_id()));
        assert!(store.sample_position_overrides.is_empty());
    }

    /// Regression: the write loop used the resolved gate's own key for every id,
    /// so a composite's subgate overrides were never updated. Filtering and
    /// statistics resolve subgates by id, so the gate moved on screen while
    /// still gating its old position.
    #[test]
    fn every_subgate_gets_its_own_sample_override_entry() {
        let mut store = GateSubStore::default();
        let g = quadrant("q");
        let ids = GateSubStore::ids_for(&g, &g.get_id());
        let origin = GateSource::Sample((g.get_id(), file("sample-1")));

        store.insert_for_source(&ids, &g, &origin);

        assert_eq!(store.sample_position_overrides.len(), 5);
        for id in ids {
            assert!(
                store
                    .sample_position_overrides
                    .contains_key(&(id.clone(), file("sample-1"))),
                "no sample override written for {id}"
            );
        }
    }

    #[test]
    fn every_subgate_gets_its_own_group_override_entry() {
        let mut store = GateSubStore::default();
        let g = quadrant("q");
        let ids = GateSubStore::ids_for(&g, &g.get_id());
        let key = group_key("$VOL", "high");
        let origin = GateSource::Group((g.get_id(), key.clone()));

        store.insert_for_source(&ids, &g, &origin);

        assert_eq!(store.group_position_overrides.len(), 5);
        for id in ids {
            assert!(
                store
                    .group_position_overrides
                    .contains_key(&(id.clone(), key.clone())),
                "no group override written for {id}"
            );
        }
    }

    #[test]
    fn writing_an_override_leaves_the_global_position_alone() {
        let mut store = GateSubStore::default();
        let g = rectangle("r");
        store.insert_for_source(&[g.get_id()], &g, &GateSource::Global);

        let moved = rectangle("r");
        store.insert_for_source(
            &[moved.get_id()],
            &moved,
            &GateSource::Sample((moved.get_id(), file("s1"))),
        );

        assert_eq!(store.primary_and_subgate_registry.len(), 1);
        assert!(Arc::ptr_eq(
            store.primary_and_subgate_registry.get(&g.get_id()).unwrap(),
            &g
        ));
    }

    #[test]
    fn overrides_for_different_samples_do_not_collide() {
        let mut store = GateSubStore::default();
        let g = rectangle("r");

        for sample in ["s1", "s2", "s3"] {
            store.insert_for_source(
                &[g.get_id()],
                &g,
                &GateSource::Sample((g.get_id(), file(sample))),
            );
        }

        assert_eq!(store.sample_position_overrides.len(), 3);
    }

    // ── GateSubStore::map_gates ───────────────────────────────────────────────

    /// A composite is aliased under five keys. Without memoisation by heap
    /// address the transform would run once per key and compound on itself.
    #[test]
    fn a_composite_is_transformed_once_however_many_keys_it_holds() {
        let mut store = GateSubStore::default();
        let g = quadrant("q");
        let ids = GateSubStore::ids_for(&g, &g.get_id());
        store.insert_for_source(&ids, &g, &GateSource::Global);

        let mut calls = 0;
        store.map_gates(|gate| {
            calls += 1;
            gate.clone()
        });

        assert_eq!(store.primary_and_subgate_registry.len(), 5);
        assert_eq!(calls, 1, "the transform ran {calls} times, expected once");
    }

    #[test]
    fn map_gates_visits_all_three_tiers() {
        let mut store = GateSubStore::default();
        let a = rectangle("a");
        let b = rectangle("b");
        let c = rectangle("c");
        store.insert_for_source(&[a.get_id()], &a, &GateSource::Global);
        store.insert_for_source(
            &[b.get_id()],
            &b,
            &GateSource::Sample((b.get_id(), file("s1"))),
        );
        store.insert_for_source(
            &[c.get_id()],
            &c,
            &GateSource::Group((c.get_id(), group_key("$VOL", "high"))),
        );

        let mut seen = 0;
        store.map_gates(|gate| {
            seen += 1;
            gate.clone()
        });

        assert_eq!(seen, 3, "one visit per distinct gate across the tiers");
    }

    #[test]
    fn map_gates_replaces_what_is_stored() {
        let mut store = GateSubStore::default();
        let original = rectangle("r");
        store.insert_for_source(&[original.get_id()], &original, &GateSource::Global);

        let replacement = rectangle("r");
        store.map_gates(|_| replacement.clone());

        assert!(Arc::ptr_eq(
            store.primary_and_subgate_registry.get(&original.get_id()).unwrap(),
            &replacement
        ));
    }

    // ── GateState::add_gate ───────────────────────────────────────────────────

    fn add_rect(state: &mut GateState, parent: Option<GateId>) -> GateId {
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                parent,
                PrimaryGateType::Rectangle,
                Some("a gate".to_string()),
            )
            .unwrap();

        // The most recently registered primary gate.
        state
            .gate_store
            .primary_and_subgate_registry
            .iter()
            .find(|(_, g)| g.get_name() == "a gate")
            .map(|(id, _)| id.clone())
            .expect("the new gate is in the registry")
    }

    #[test]
    fn adding_a_gate_registers_it_and_parents_it_to_the_root() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        assert!(state.gate_store.primary_and_subgate_registry.contains_key(&id));
        assert_eq!(
            state.hierarchy.get_parent(&id).map(|p| p.to_string()),
            Some(ROOTGATE.to_string())
        );
    }

    #[test]
    fn adding_a_gate_puts_it_on_its_plot() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        let key = GatesOnPlotKey::new(Arc::from(X), Arc::from(Y), Some(ROOTGATE.clone()));
        assert_eq!(
            state.gate_ids_by_view.get(&key).map(|v| v.as_slice()),
            Some([id].as_slice())
        );
    }

    #[test]
    fn a_child_gate_hangs_off_its_parent() {
        let mut state = GateState::default();
        let parent = add_rect(&mut state, None);
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                Some(parent.clone()),
                PrimaryGateType::Rectangle,
                Some("child".to_string()),
            )
            .unwrap();

        let child = state
            .gate_store
            .primary_and_subgate_registry
            .iter()
            .find(|(_, g)| g.get_name() == "child")
            .map(|(id, _)| id.clone())
            .unwrap();

        assert_eq!(
            state.hierarchy.get_parent(&child).map(|p| p.to_string()),
            Some(parent.to_string())
        );
    }

    #[test]
    fn a_composite_registers_every_subgate_in_the_hierarchy() {
        let mut state = GateState::default();
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                None,
                PrimaryGateType::Quadrant,
                Some("quad".to_string()),
            )
            .unwrap();

        // Four subgates hang off the root, and every registry key resolves.
        assert_eq!(state.hierarchy.get_children(&ROOTGATE).len(), 4);
        assert!(state.gate_store.primary_and_subgate_registry.len() >= 5);
    }

    // ── GateState::remove_gate ────────────────────────────────────────────────

    /// Regression: the view index was cleaned using a key built from the root
    /// rather than the gate's real parent, because delete_subtree had already
    /// unlinked it - so deleted gates kept rendering.
    #[test]
    fn removing_a_gate_takes_it_off_its_plot() {
        let mut state = GateState::default();
        let parent = add_rect(&mut state, None);
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                Some(parent.clone()),
                PrimaryGateType::Rectangle,
                Some("child".to_string()),
            )
            .unwrap();
        let child = state
            .gate_store
            .primary_and_subgate_registry
            .iter()
            .find(|(_, g)| g.get_name() == "child")
            .map(|(id, _)| id.clone())
            .unwrap();

        state.remove_gate(child.clone()).unwrap();

        let key = GatesOnPlotKey::new(Arc::from(X), Arc::from(Y), Some(parent));
        let on_plot = state.gate_ids_by_view.get(&key).cloned().unwrap_or_default();
        assert!(
            !on_plot.contains(&child),
            "the deleted gate is still listed on its plot: {on_plot:?}"
        );
    }

    #[test]
    fn removing_a_gate_unregisters_it() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        state.remove_gate(id.clone()).unwrap();

        assert!(!state.gate_store.primary_and_subgate_registry.contains_key(&id));
        assert!(state.hierarchy.get_parent(&id).is_none());
    }

    #[test]
    fn removing_a_gate_takes_its_descendants_with_it() {
        let mut state = GateState::default();
        let parent = add_rect(&mut state, None);
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                Some(parent.clone()),
                PrimaryGateType::Rectangle,
                Some("child".to_string()),
            )
            .unwrap();
        let child = state
            .gate_store
            .primary_and_subgate_registry
            .iter()
            .find(|(_, g)| g.get_name() == "child")
            .map(|(id, _)| id.clone())
            .unwrap();

        state.remove_gate(parent.clone()).unwrap();

        assert!(!state.gate_store.primary_and_subgate_registry.contains_key(&parent));
        assert!(
            !state.gate_store.primary_and_subgate_registry.contains_key(&child),
            "the child outlived its parent"
        );
    }

    /// Regression: overrides were dropped for the requested gate only, so a
    /// descendant's per-sample position lingered after its parent was deleted.
    #[test]
    fn removing_a_gate_drops_the_overrides_of_its_descendants() {
        let mut state = GateState::default();
        let parent = add_rect(&mut state, None);
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                Some(parent.clone()),
                PrimaryGateType::Rectangle,
                Some("child".to_string()),
            )
            .unwrap();
        let child = state
            .gate_store
            .primary_and_subgate_registry
            .iter()
            .find(|(_, g)| g.get_name() == "child")
            .map(|(id, _)| id.clone())
            .unwrap();

        // Give both a per-sample override.
        let moved = rectangle("moved");
        state.gate_store.sample_position_overrides
            .insert((parent.clone(), file("s1")), moved.clone());
        state.gate_store.sample_position_overrides
            .insert((child.clone(), file("s1")), moved.clone());

        state.remove_gate(parent.clone()).unwrap();

        assert!(state.gate_store.sample_position_overrides.is_empty(),
            "overrides left behind: {:?}",
            state.gate_store.sample_position_overrides.keys().collect::<Vec<_>>());
    }

    /// Regression: only the subgate ids were removed, so the composite's own
    /// registry entry outlived the delete and kept appearing in every resolver.
    #[test]
    fn removing_a_composite_unregisters_the_composite_itself() {
        let mut state = GateState::default();
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                None,
                PrimaryGateType::Quadrant,
                Some("quad".to_string()),
            )
            .unwrap();

        let any_subgate = state.hierarchy.get_children(&ROOTGATE)[0].clone();
        state.remove_gate(any_subgate).unwrap();

        assert!(
            state.gate_store.primary_and_subgate_registry.is_empty(),
            "registry still holds: {:?}",
            state.gate_store.primary_and_subgate_registry.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn removing_an_unknown_gate_is_harmless() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        state.remove_gate(Arc::from("does-not-exist")).unwrap();

        assert!(state.gate_store.primary_and_subgate_registry.contains_key(&id));
    }

    // ── GateState::get_current_sample ─────────────────────────────────────────

    fn groups(pairs: &[(&str, &str)]) -> FxHashMap<MetaDataParameter, GroupId> {
        pairs
            .iter()
            .map(|(p, g)| (Arc::from(*p) as Arc<str>, Arc::from(*g) as Arc<str>))
            .collect()
    }

    #[test]
    fn a_gate_with_no_override_resolves_to_its_global_position() {
        let mut state = GateState::default();
        let g = rectangle("r");
        state.gate_store.insert_for_source(&[g.get_id()], &g, &GateSource::Global);

        let resolver = state.get_current_sample(file("s1"), &groups(&[]));

        assert!(Arc::ptr_eq(&resolver.active_gates.get(&g.get_id()).unwrap().0, &g));
        assert_eq!(
            resolver.gate_origins.get(&g.get_id()),
            Some(&GateSource::Global)
        );
    }

    #[test]
    fn a_sample_override_wins_for_that_sample_only() {
        let mut state = GateState::default();
        let global = rectangle("r");
        let override_gate = rectangle("r");
        state.gate_store.insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        state.gate_store.sample_position_overrides
            .insert((global.get_id(), file("s1")), override_gate.clone());

        let overridden = state.get_current_sample(file("s1"), &groups(&[]));
        let untouched = state.get_current_sample(file("s2"), &groups(&[]));

        assert!(Arc::ptr_eq(
            &overridden.active_gates.get(&global.get_id()).unwrap().0,
            &override_gate
        ));
        assert!(Arc::ptr_eq(
            &untouched.active_gates.get(&global.get_id()).unwrap().0,
            &global
        ));
    }

    #[test]
    fn a_group_override_applies_to_every_sample_in_that_group() {
        let mut state = GateState::default();
        let global = rectangle("r");
        let group_gate = rectangle("r");
        let key = group_key("$VOL", "high");
        state.gate_store.insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        state.gate_store.group_position_overrides
            .insert((global.get_id(), key.clone()), group_gate.clone());

        let in_group = state.get_current_sample(file("s1"), &groups(&[("$VOL", "high")]));
        let out_of_group = state.get_current_sample(file("s2"), &groups(&[("$VOL", "low")]));

        assert!(Arc::ptr_eq(
            &in_group.active_gates.get(&global.get_id()).unwrap().0,
            &group_gate
        ));
        assert!(Arc::ptr_eq(
            &out_of_group.active_gates.get(&global.get_id()).unwrap().0,
            &global
        ));
    }

    /// Precedence is sample, then group, then global.
    #[test]
    fn a_sample_override_beats_a_group_override() {
        let mut state = GateState::default();
        let global = rectangle("r");
        let group_gate = rectangle("r");
        let sample_gate = rectangle("r");
        let key = group_key("$VOL", "high");

        state.gate_store.insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        state.gate_store.group_position_overrides
            .insert((global.get_id(), key), group_gate);
        state.gate_store.sample_position_overrides
            .insert((global.get_id(), file("s1")), sample_gate.clone());

        let resolver = state.get_current_sample(file("s1"), &groups(&[("$VOL", "high")]));

        assert!(Arc::ptr_eq(
            &resolver.active_gates.get(&global.get_id()).unwrap().0,
            &sample_gate
        ));
        assert!(matches!(
            resolver.gate_origins.get(&global.get_id()),
            Some(GateSource::Sample(_))
        ));
    }

    #[test]
    fn the_resolver_covers_every_registered_gate() {
        let mut state = GateState::default();
        for id in ["a", "b", "c"] {
            let g = rectangle(id);
            state.gate_store.insert_for_source(&[g.get_id()], &g, &GateSource::Global);
        }

        let resolver = state.get_current_sample(file("s1"), &groups(&[]));

        assert_eq!(resolver.active_gates.len(), 3);
        assert_eq!(resolver.gate_origins.len(), 3);
    }

    #[test]
    fn an_empty_store_resolves_to_an_empty_resolver() {
        let state = GateState::default();
        let resolver = state.get_current_sample(file("s1"), &groups(&[]));

        assert!(resolver.active_gates.is_empty());
    }

    // ── The node table ────────────────────────────────────────────────────────
    //
    // Stage 1 of splitting placement from gate. Node ids are still equal to
    // gate ids here, so these pin the table's own invariants - not linked gates,
    // which the tree cannot yet hold.

    #[test]
    fn adding_a_gate_records_one_placement() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        assert_eq!(state.placement_count(&id), 1);
        assert_eq!(state.nodes_for_gate(&id), &[NodeId::from(id.clone())]);
        assert_eq!(state.gate_for_node(&NodeId::from(id.clone())), Some(&id));
        assert!(!state.is_linked(&id), "one placement is not a link");
        assert!(!state.is_ghost(&id), "it has a node");
    }

    #[test]
    fn every_corner_of_a_composite_gets_its_own_placement() {
        let mut state = GateState::default();
        state
            .add_gate(
                &mapper(), 300.0, 300.0, Arc::from(X), Arc::from(Y),
                None, None, PrimaryGateType::Quadrant, Some("q".to_string()),
            )
            .unwrap();

        // Four corners in the tree; the composite's own key is registered but
        // is not a tree position.
        assert_eq!(state.placements.len(), 4);
        for node in state.placements.keys() {
            assert_eq!(state.placement_count(state.gate_for_node(node).unwrap()), 1);
        }
    }

    #[test]
    fn deleting_a_gate_forgets_its_placement() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);
        state.remove_gate(id.clone()).unwrap();

        assert_eq!(state.placement_count(&id), 0);
        assert!(state.gate_for_node(&NodeId::from(id.clone())).is_none());
        assert!(state.nodes_by_gate.get(&id).is_none(), "no empty vec left behind");
    }

    #[test]
    fn deleting_a_parent_forgets_its_descendants_placements() {
        let mut state = GateState::default();
        let parent = add_rect(&mut state, None);
        let child = add_rect(&mut state, Some(parent.clone()));

        state.remove_gate(parent.clone()).unwrap();

        assert_eq!(state.placement_count(&child), 0, "the subtree went with it");
        assert!(state.placements.is_empty());
    }

    /// The table has to survive an import, or the export path built on it would
    /// see an empty tree.
    #[test]
    fn an_imported_tree_records_a_placement_per_node() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);
        let child = add_rect(&mut state, Some(id.clone()));

        assert_eq!(state.placements.len(), 2);
        assert_eq!(state.gate_for_node(&NodeId::from(child.clone())), Some(&child));
    }

    /// Re-pointing a node at another gate must leave the first gate's list
    /// clean - this is what the link operation will do in a later stage.
    #[test]
    fn repointing_a_node_moves_it_between_gates() {
        let mut state = GateState::default();
        let a = add_rect(&mut state, None);
        // add_rect finds a gate by name, and both are called "a gate", so take
        // the second id as the one that is not the first.
        add_rect(&mut state, None);
        let b = state
            .registered_ids()
            .into_iter()
            .find(|id| id != &a)
            .expect("a second gate was added");
        let node = NodeId::from(a.clone());

        state.record_placement(node.clone(), b.clone(), false);

        assert_eq!(state.placement_count(&a), 0, "no stale entry on the old gate");
        assert_eq!(state.placement_count(&b), 2);
        assert!(state.is_linked(&b));
        assert_eq!(state.gate_for_node(&node), Some(&b));
    }

    #[test]
    fn recording_the_same_placement_twice_is_idempotent() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        state.record_placement(NodeId::from(id.clone()), id.clone(), false);

        assert_eq!(state.placement_count(&id), 1, "no duplicate node entry");
    }

    #[test]
    fn a_registered_gate_with_no_node_is_a_ghost() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);
        state.forget_placement(&NodeId::from(id.clone()));

        assert!(state.is_ghost(&id), "registered, but nowhere in the tree");
        assert!(!state.is_ghost(&Arc::from("never-existed")));
    }
}
