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
        gate_drag::{GateDragData, PointDragData},
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

#[derive(Clone, Default)]
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

#[derive(Clone, Default, Store)]
pub struct GateSubStore {
    pub primary_and_subgate_registry: GateMap,
    pub sample_position_overrides: SampleGateMap,
    pub group_position_overrides: GroupGateMap,
    /// When each per-group and per-sample position was written, as a count
    /// of writes: the larger, the more recent. See
    /// [`GateSubStore::position_for`].
    ///
    /// Kept beside the maps rather than in them so everything that reads a
    /// position - filtering, drawing, the export - keeps reading the maps it
    /// always has. Written only by [`GateSubStore::set_group_position`] and
    /// [`GateSubStore::set_sample_position`], trimmed only by the matching
    /// `retain_` methods, so they cannot drift from the maps.
    group_written: FxHashMap<(GateId, MetaDataKey), u64>,
    sample_written: FxHashMap<(GateId, FileId), u64>,
    /// One count for both tiers, so a per-sample position and a per-group
    /// one can be told apart by age.
    writes: u64,
}

/// The plain-data half of the gate store.
///
/// These take `&mut GateSubStore` rather than a `Store` lens so they can be
/// exercised without a Dioxus runtime; the store methods are thin wrappers that
/// keep the same write granularity.
impl GateSubStore {
    /// Write one per-group position, as the newest.
    pub fn set_group_position(&mut self, key: (GateId, MetaDataKey), gate: Arc<dyn DrawableGate>) {
        self.writes += 1;
        self.group_written.insert(key.clone(), self.writes);
        self.group_position_overrides.insert(key, gate);
    }

    /// Write one per-sample position, as the newest.
    pub fn set_sample_position(&mut self, key: (GateId, FileId), gate: Arc<dyn DrawableGate>) {
        self.writes += 1;
        self.sample_written.insert(key.clone(), self.writes);
        self.sample_position_overrides.insert(key, gate);
    }

    /// Drop every per-sample position `keep` says no to.
    pub fn retain_sample_positions(&mut self, mut keep: impl FnMut(&(GateId, FileId)) -> bool) {
        self.sample_position_overrides.retain(|key, _| keep(key));
        self.sample_written.retain(|key, _| keep(key));
    }

    /// Where `gate_id` sits for one file: of the file's own position and the
    /// newest position of any group it is in, whichever was written last -
    /// or `None`, for the gate's global position.
    ///
    /// The one rule every reader follows: the plots, the filtering and the
    /// statistics through [`GateState::get_current_sample`], the export through
    /// [`GateState::gate_for_file`]. A file's own position used to win
    /// whatever its age, so a rules run - which positions a whole specimen -
    /// was hidden from any file of it that had been given a position of its
    /// own, while the report said it had been positioned (B-GRP-2). Now the
    /// last position written applies, whichever kind it is: a run beats an
    /// older adjustment on one sample, and a later adjustment beats the run.
    ///
    /// Positions of equal age - only possible when they were put straight
    /// into the maps rather than through the `set_` methods - go to the
    /// file's own, as they always did.
    pub fn position_for<'a>(
        &self,
        gate_id: &GateId,
        file_id: &FileId,
        groups: impl IntoIterator<Item = (&'a MetaDataParameter, &'a GroupId)>,
    ) -> Option<(GateSource, &Arc<dyn DrawableGate>)> {
        let own_key = (gate_id.clone(), file_id.clone());
        let own = self.sample_position_overrides.get(&own_key).map(|gate| {
            let written = self.sample_written.get(&own_key).copied().unwrap_or(0);
            (written, gate)
        });
        let group = self
            .newest_group_position(gate_id, groups)
            .map(|(key, gate)| {
                let written = self
                    .group_written
                    .get(&(gate_id.clone(), key.clone()))
                    .copied()
                    .unwrap_or(0);
                (written, key, gate)
            });
        match (own, group) {
            (Some((own_at, own)), Some((group_at, key, group))) => {
                if group_at > own_at {
                    Some((GateSource::Group((gate_id.clone(), key)), group))
                } else {
                    Some((GateSource::Sample(own_key), own))
                }
            }
            (Some((_, own)), None) => Some((GateSource::Sample(own_key), own)),
            (None, Some((_, key, group))) => {
                Some((GateSource::Group((gate_id.clone(), key)), group))
            }
            (None, None) => None,
        }
    }

    /// Drop every per-group position `keep` says no to.
    pub fn retain_group_positions(&mut self, mut keep: impl FnMut(&(GateId, MetaDataKey)) -> bool) {
        self.group_position_overrides.retain(|key, _| keep(key));
        self.group_written.retain(|key, _| keep(key));
    }

    /// Of the per-group positions `gate_id` holds for a file in `groups` - one
    /// `(column, value)` pair per metadata column the file has - the one
    /// written last.
    ///
    /// A file is in a group under every column it has: its SampleID, its
    /// Type, its Donor. A gate can hold positions under more than one of them -
    /// the gating file groups it by one column, a rules run by the pairing's
    /// sample id column, and a run after that column is changed by another.
    /// The newest applies, so a file shows the last position anything gave
    /// it, and an older position still holds for the files nothing newer
    /// covers. This used to be whichever column the file's metadata hash map
    /// happened to yield first (B-GRP-1).
    ///
    /// A tie is only possible between positions put straight into the map
    /// rather than through [`GateSubStore::set_group_position`]; it goes to the
    /// column whose name sorts first, so the answer never depends on hashing.
    pub fn newest_group_position<'a>(
        &self,
        gate_id: &GateId,
        groups: impl IntoIterator<Item = (&'a MetaDataParameter, &'a GroupId)>,
    ) -> Option<(MetaDataKey, &Arc<dyn DrawableGate>)> {
        groups
            .into_iter()
            .filter_map(|(parameter, group)| {
                let key = (
                    gate_id.clone(),
                    MetaDataKey {
                        parameter: parameter.clone(),
                        group: group.clone(),
                    },
                );
                let gate = self.group_position_overrides.get(&key)?;
                let written = self.group_written.get(&key).copied().unwrap_or(0);
                Some((written, key.1, gate))
            })
            .max_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| b.1.parameter.cmp(&a.1.parameter))
            })
            .map(|(_, key, gate)| (key, gate))
    }

    /// The metadata columns `gate_id` holds per-group positions under, the
    /// most recently written first.
    pub fn group_columns_newest_first(&self, gate_id: &GateId) -> Vec<MetaDataParameter> {
        let mut newest: FxHashMap<MetaDataParameter, u64> = FxHashMap::default();
        for (id, key) in self.group_position_overrides.keys() {
            if id != gate_id {
                continue;
            }
            let written = self
                .group_written
                .get(&(id.clone(), key.clone()))
                .copied()
                .unwrap_or(0);
            let at = newest.entry(key.parameter.clone()).or_insert(0);
            *at = (*at).max(written);
        }
        let mut columns: Vec<(MetaDataParameter, u64)> = newest.into_iter().collect();
        columns.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        columns.into_iter().map(|(column, _)| column).collect()
    }

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
                    self.set_group_position((id.clone(), group_key.clone()), gate.clone());
                }
                GateSource::Sample((_, file_id)) => {
                    self.set_sample_position((id.clone(), file_id.clone()), gate.clone());
                }
            }
        }
    }

    /// Write the gates [`oriented_to_plot`] turned, each into the tier it
    /// was resolved from.
    pub fn apply_orientation(&mut self, updates: Vec<OrientedGate>) {
        for (id, gate, origin) in updates {
            self.insert_for_source(&[id], &gate, &origin);
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

    /// Carry every gate drawn on `marker` from one transform to another.
    ///
    /// Gate coordinates are held in the transformed space the plot is drawn
    /// in, so a new cofactor would otherwise leave each gate at the same place
    /// on screen and around different cells. Each point goes back to raw data
    /// through the old transform and out through the new one, so a gate keeps
    /// admitting the same events.
    ///
    /// Every tier - drawn, per specimen, per sample - through [`map_gates`],
    /// which also makes sure a gate shared between tiers or aliased under a
    /// composite's several keys is carried once and not compounded.
    ///
    /// A gate that cannot be carried keeps its old geometry and its error is
    /// returned; the others are carried regardless.
    ///
    /// [`map_gates`]: GateSubStore::map_gates
    pub fn rescale_channel(
        &mut self,
        marker: &Arc<str>,
        old: &AxisInfo,
        new: &AxisInfo,
    ) -> Result<(), Vec<String>> {
        let mut errors = vec![];
        self.map_gates(|gate| {
            let (x_marker, y_marker) = gate.get_params();
            if marker != &x_marker && marker != &y_marker {
                return gate.clone();
            }
            match gate.recalculate_gate_for_rescaled_axis(
                marker.clone(),
                &old.transform,
                &new.transform,
                (new.axis_lower, new.axis_upper),
            ) {
                Ok(new_gate) => Arc::from(new_gate),
                Err(e) => {
                    errors.push(e.to_string());
                    gate.clone()
                }
            }
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Carry every gate drawn on `axis` to a new axis range.
    ///
    /// Only the gates whose extent comes from the axis range change - the
    /// quadrants, whose outer edges run to the ends of the axes. Everything
    /// else answers `None` and is kept as the same gate.
    pub fn relimit_channel(
        &mut self,
        axis: &Arc<str>,
        lower: f32,
        upper: f32,
        transform: &TransformType,
    ) -> Result<(), Vec<String>> {
        let mut errors = vec![];
        self.map_gates(|gate| {
            let (x_marker, y_marker) = gate.get_params();
            if axis != &x_marker && axis != &y_marker {
                return gate.clone();
            }
            match gate.recalculate_gate_for_new_axis_limits(axis.clone(), lower, upper, transform) {
                Ok(Some(new_gate)) => Arc::from(new_gate),
                Ok(None) => gate.clone(),
                Err(e) => {
                    errors.push(e.to_string());
                    gate.clone()
                }
            }
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
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

    /// The gate this resolver holds for `id`.
    ///
    /// Public because the gallery resolves gates outside the component that
    /// draws them - twenty plots at once, off the UI thread - rather than
    /// through the editor's `gate_ids_by_view` cache, which is written as a
    /// side effect of drawing.
    pub fn resolve_drawable(&self, id: &str) -> anyhow::Result<Arc<dyn DrawableGate + 'static>> {
        let drawable = self
            .active_gates
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Gate {} not found in active set", id))?;
        Ok(drawable.deref().clone())
    }
}

/// A gate turned to a plot's axes: the id to write it under, the gate, and
/// the tier it was resolved from.
pub type OrientedGate = (GateId, Arc<dyn DrawableGate>, GateSource);

/// The gates among `ids` held the other way round from a plot of `x` by `y`,
/// turned to it.
///
/// Only the position `resolver` shows is turned - the global one, or the
/// group's or sample's that overrides it - and it is written back into that
/// tier, so the same gate can be held on its axes one way globally and the
/// other for one sample. The export turns each back to the file's axes; see
/// `omiq::serialise::as_imported`.
///
/// A composite is written under its own id and each of its parts'.
pub fn oriented_to_plot(
    ids: &[GateId],
    x: &str,
    y: &str,
    resolver: &GateOverrideResolver,
) -> anyhow::Result<Vec<OrientedGate>> {
    let mut updates = Vec::new();
    for k in ids {
        let Some(new_gate) = resolver.resolve_drawable(k)?.match_to_plot_axis(x, y)? else {
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
    Ok(updates)
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

/// `Clone` is a snapshot, not a deep copy: every gate is behind an `Arc`, so
/// cloning bumps refcounts. That is what lets a long solve run on a worker
/// thread against a consistent view while the editor stays live.
#[derive(Clone, Default, Store)]
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
    /// Whether the document is still the one `earlier` was taken from: the
    /// same gates at the same positions - global, per group and per sample -
    /// in the same tree.
    ///
    /// What a rules run needs to know before it writes its answers. It solves
    /// against a snapshot, and answers worked out on one document mean nothing
    /// in another - a moved gate, a new file's gating, a whole document loaded
    /// over it. Every edit replaces the `Arc` of the gate it touches, so gates
    /// are compared by identity: cheap, and a gate moved away and back again
    /// still counts as changed, which is the safe answer. Which gate is
    /// selected, and which tree nodes are folded, say nothing about the
    /// gating and are not compared.
    pub fn unchanged_since(&self, earlier: &GateState) -> bool {
        fn same<K: Eq + std::hash::Hash>(
            a: &FxHashMap<K, Arc<dyn DrawableGate>>,
            b: &FxHashMap<K, Arc<dyn DrawableGate>>,
        ) -> bool {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, g)| b.get(k).is_some_and(|h| Arc::ptr_eq(g, h)))
        }
        let (now, then) = (&self.gate_store, &earlier.gate_store);
        same(
            &now.primary_and_subgate_registry.0,
            &then.primary_and_subgate_registry.0,
        ) && same(
            &now.sample_position_overrides,
            &then.sample_position_overrides,
        ) && same(
            &now.group_position_overrides,
            &then.group_position_overrides,
        ) && self.placements.len() == earlier.placements.len()
            && self.placements.iter().all(|(node, placed)| {
                earlier
                    .placements
                    .get(node)
                    .is_some_and(|was| was.gate_id == placed.gate_id)
                    && self.parent_node(node) == earlier.parent_node(node)
            })
    }

    /// Turn the gates among `ids` to a plot of `x` by `y`, as viewing that
    /// plot does. See [`oriented_to_plot`].
    pub fn orient_to_plot(
        &mut self,
        ids: &[GateId],
        x: &str,
        y: &str,
        resolver: &GateOverrideResolver,
    ) -> anyhow::Result<()> {
        let updates = oriented_to_plot(ids, x, y, resolver)?;
        self.gate_store.apply_orientation(updates);
        Ok(())
    }

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

    /// Every position in the tree, with the gate it shows. This is the tree:
    /// the exporter writes one Omiq node per entry.
    pub fn placements(&self) -> impl Iterator<Item = (&NodeId, &GatePlacement)> {
        self.placements.iter()
    }

    #[cfg(test)]
    pub fn plot_of_for_probe(&self, node: &NodeId) -> String {
        self.plot_of(node).to_string()
    }

    #[cfg(test)]
    pub fn view_ids_for_probe(&self, parent: &str) -> Vec<String> {
        self.gate_ids_by_view
            .iter()
            .filter(|(k, _)| k.parental_gate_id.as_deref() == Some(parent))
            .flat_map(|(_, v)| v.iter().map(|g| g.to_string()))
            .collect()
    }

    #[cfg(test)]
    pub fn view_keys_for_probe(&self) -> Vec<(String, String, Option<String>, usize)> {
        self.gate_ids_by_view
            .iter()
            .map(|(k, v)| {
                (
                    k.param_1.to_string(),
                    k.param_2.to_string(),
                    k.parental_gate_id.as_ref().map(|p| p.to_string()),
                    v.len(),
                )
            })
            .collect()
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
    ///
    /// A composite is never a ghost while any of its corners is on the tree. It
    /// has no node of its own - only its corners do - so its own key always has
    /// a placement count of zero, and it is registered under that key as well as
    /// under each corner's. Asking the count alone therefore called every
    /// composite a ghost, which would have had the sweep collect all of them.
    pub fn is_ghost(&self, gate_id: &GateId) -> bool {
        let Some(gate) = self.gate_store.primary_and_subgate_registry.get(gate_id) else {
            return false;
        };
        if gate.is_composite() {
            return gate
                .get_inner_gate_ids()
                .iter()
                .all(|corner| self.placement_count(corner) == 0);
        }
        self.placement_count(gate_id) == 0
    }

    // ── Ghost collection ──────────────────────────────────────────────────────

    /// Drop every registered gate that nothing live can reach any more.
    ///
    /// A ghost - a gate with no node - is kept deliberately, because a live
    /// boolean may still evaluate against it. Deleting the last boolean that
    /// referenced one leaves it stranded: registered, drawn nowhere, reachable
    /// by nothing, and still written out on export. A real file imported with
    /// six ghosts, so they accumulate for the life of a session.
    ///
    /// This is the same reachability idea the importer uses, run over the live
    /// store instead of the file's containers: start from every gate that holds
    /// a position in the tree and follow booleans to their operands. Anything
    /// the walk does not reach is collected.
    ///
    /// Returns the ids it dropped, in no particular order - the registry is a
    /// hash map - for the caller to log or assert on.
    pub fn collect_stranded_ghosts(&mut self) -> Vec<GateId> {
        let mut reachable: HashSet<GateId> = HashSet::default();
        let mut frontier: Vec<GateId> = self
            .placements
            .values()
            .map(|placement| placement.gate_id.clone())
            .collect();

        while let Some(id) = frontier.pop() {
            if !reachable.insert(id.clone()) {
                continue;
            }
            let Some(gate) = self.gate_store.primary_and_subgate_registry.get(&id) else {
                continue;
            };
            // A composite is registered under its own id and under each corner's,
            // all aliased to one Arc. Reaching any key keeps every key: the
            // corner ids are how a filter resolves to the whole gate, and Omiq
            // treats the group as all-or-nothing anyway.
            if gate.is_composite() {
                frontier.push(gate.get_id());
                frontier.extend(gate.get_inner_gate_ids());
            }
            // A boolean keeps its operands alive, and an operand may itself be a
            // boolean, so this has to run to a fixed point rather than one deep.
            if let Some(inner) = gate.get_gate_ref(None)
                && let flow_gates::GateGeometry::Boolean { operands, .. } = &inner.geometry
            {
                frontier.extend(operands.iter().cloned());
            }
        }

        let stranded: Vec<GateId> = self
            .gate_store
            .primary_and_subgate_registry
            .keys()
            .filter(|id| !reachable.contains(*id))
            .cloned()
            .collect();

        if stranded.is_empty() {
            return stranded;
        }

        let dropped: HashSet<GateId> = stranded.iter().cloned().collect();
        self.gate_store
            .primary_and_subgate_registry
            .retain(|id, _| !dropped.contains(id));
        self.gate_store
            .retain_sample_positions(|(id, _file)| !dropped.contains(id));
        self.gate_store
            .retain_group_positions(|(id, _group)| !dropped.contains(id));
        self.omiq_rebuild
            .gates
            .retain(|id, _| !dropped.contains(id));
        for dependents in self.boolean_gate_links.values_mut() {
            dependents.retain(|id| !dropped.contains(id));
        }
        self.boolean_gate_links
            .retain(|id, dependents| !dependents.is_empty() && !dropped.contains(id));

        // `omiq_rebuild.ghost_containers` is deliberately left alone. Those are
        // containers that were already unreachable in the file Omiq wrote, kept
        // verbatim so a round trip returns the document it was given - they were
        // never gates in this editor, and sweeping them on this rule would drop
        // every one of them on the first delete. Collecting what *this* session
        // stranded is a different thing from discarding what Omiq shipped.
        stranded
    }

    // ── Linking ───────────────────────────────────────────────────────────────

    /// Drop one position of a gate, leaving the gate itself alone if it is
    /// applied elsewhere.
    ///
    /// The subtree under this position goes with it: those children belong to
    /// this placement, not to the gate, so the sibling placement keeps its own.
    ///
    /// When the last position goes the gate becomes a ghost, and whether it is
    /// then kept comes down to the one question that matters: does anything
    /// still reach it? A boolean built on it keeps it registered and evaluable.
    /// Nothing reaching it means it is invisible, unreferenced and written out
    /// on export for no reason, so `collect_stranded_ghosts` takes it. Use
    /// `remove_gate` to delete a gate and everything built on it outright.
    pub fn delete_placement(&mut self, node: &NodeId) -> anyhow::Result<()> {
        if !self.placements.contains_key(node) {
            return Err(anyhow!("no such position in the tree: {node}"));
        }

        // A composite is all-or-nothing in Omiq, so dropping one position of one
        // corner would leave a three-cornered quadrant behind. Take the whole
        // group at this plot instead.
        let group = self.composite_group_at(node);
        if group.len() > 1 {
            for corner_node in group {
                self.delete_one_placement(&corner_node);
            }
            // After the whole group, not inside the loop: a composite is still
            // reachable while any corner holds a position, so a sweep run
            // between corners would see a half-deleted group and do nothing.
            self.collect_stranded_ghosts();
            return Ok(());
        }

        self.delete_one_placement(node);
        self.collect_stranded_ghosts();
        Ok(())
    }

    /// Every corner node of the composite at this position's plot, or just this
    /// node when it is not part of one.
    fn composite_group_at(&self, node: &NodeId) -> Vec<NodeId> {
        let Some(gate) = self
            .gate_for_node(node)
            .and_then(|id| self.registered_gate(id))
        else {
            return vec![node.clone()];
        };
        if !gate.is_composite() {
            return vec![node.clone()];
        }
        let plot = self.plot_of(node);
        gate.get_inner_gate_ids()
            .iter()
            .filter_map(|corner| {
                self.nodes_for_gate(corner)
                    .iter()
                    .find(|n| self.plot_of(n) == plot)
                    .cloned()
            })
            .collect()
    }

    fn delete_one_placement(&mut self, node: &NodeId) {
        // What each doomed position showed, and which plot it was on, before the
        // tree forgets where any of them were.
        let doomed: Vec<(Arc<str>, GateId)> = std::iter::once(node.as_arc().clone())
            .chain(self.hierarchy.get_descendants(node.as_str()))
            .map(NodeId::from)
            .filter_map(|n| Some((self.plot_of(&n), self.gate_for_node(&n)?.clone())))
            .collect();

        for removed in self.hierarchy.delete_subtree(node.as_str()) {
            self.forget_placement(&NodeId::from(removed));
        }
        for (plot, gate_id) in doomed {
            self.unindex_view_at(&plot, &gate_id);
        }
    }

    /// Apply the gate `target` shows at the position `node`, so the two share
    /// one gate - Omiq's linked gate.
    ///
    /// The gate `node` used to show is discarded at this position. If that was
    /// its only position, what happens next is what happens when a last
    /// position is deleted: a boolean built on it keeps it registered and
    /// evaluable, and with nothing reaching it `collect_stranded_ghosts` takes
    /// it. It used to be kept whether anything reached it or not, and such
    /// gates were written out on export as containers on no plot (B-DOC-1).
    ///
    /// Refused when the two are on different parameters - the result would be a
    /// gate drawn on axes it was not measured against - and for composites,
    /// which Omiq treats as all-or-nothing across their corners.
    pub fn link_node_to_gate(&mut self, node: &NodeId, target: &NodeId) -> anyhow::Result<()> {
        let Some(from) = self.gate_for_node(node).cloned() else {
            return Err(anyhow!("no such position in the tree: {node}"));
        };
        let Some(to) = self.gate_for_node(target).cloned() else {
            return Err(anyhow!("no such position in the tree: {target}"));
        };
        if node == target {
            return Err(anyhow!("a gate cannot be linked to itself"));
        }
        if from == to {
            return Err(anyhow!("these positions already share a gate"));
        }

        let (Some(source_gate), Some(target_gate)) =
            (self.registered_gate(&from), self.registered_gate(&to))
        else {
            return Err(anyhow!("one of the gates is not registered"));
        };

        // Omiq treats a composite as one gate spread over its corners, and
        // links it by placing the whole group under each parent - in a real
        // export, a skewed quadrant with all four corners under the same three
        // parents. So a composite link is one action over every corner.
        if source_gate.is_composite() || target_gate.is_composite() {
            if !(source_gate.is_composite() && target_gate.is_composite()) {
                return Err(anyhow!(
                    "a composite gate can only be linked to another composite gate"
                ));
            }
            return self.link_composite(node, &source_gate, &target_gate);
        }

        if source_gate.get_params() != target_gate.get_params() {
            let (tx, ty) = target_gate.get_params();
            let (sx, sy) = source_gate.get_params();
            return Err(anyhow!(
                "cannot link a gate on {sx}/{sy} to one on {tx}/{ty}: they are drawn on different axes"
            ));
        }

        let collapsed = self.placements.get(node).is_some_and(|p| p.collapsed);
        let plot = self.plot_of(node);
        self.record_placement(node.clone(), to, collapsed);
        self.unindex_view_at(&plot, &from);
        self.reindex_view(node);
        self.collect_stranded_ghosts();
        Ok(())
    }

    /// Link every corner of one composite to the matching corner of another.
    ///
    /// Corners correspond by position: `get_inner_gate_ids` is built in a fixed
    /// geometric order - bottom-left, bottom-right, top-right, top-left - so
    /// index `i` is the same corner of any composite of the same kind. That is
    /// the same correspondence the import and export already rely on.
    ///
    /// All or nothing. Half a linked quadrant is a corrupt document, so every
    /// corner is resolved and checked before a single one is re-pointed.
    fn link_composite(
        &mut self,
        node: &NodeId,
        source: &Arc<dyn DrawableGate>,
        target: &Arc<dyn DrawableGate>,
    ) -> anyhow::Result<()> {
        let source_corners = source.get_inner_gate_ids();
        let target_corners = target.get_inner_gate_ids();

        if source_corners.len() != target_corners.len() {
            return Err(anyhow!(
                "these composites have different numbers of parts ({} and {}), so their corners do not correspond",
                source_corners.len(),
                target_corners.len()
            ));
        }
        if source.get_params() != target.get_params() {
            let ((sx, sy), (tx, ty)) = (source.get_params(), target.get_params());
            return Err(anyhow!(
                "cannot link a gate on {sx}/{sy} to one on {tx}/{ty}: they are drawn on different axes"
            ));
        }

        // Every corner of the source composite at *this* plot. A composite
        // applied at several points has a set of corner nodes under each.
        let plot = self.plot_of(node);
        let mut moves: Vec<(NodeId, GateId, GateId)> = Vec::new();
        for (corner, replacement) in source_corners.iter().zip(target_corners.iter()) {
            let at_this_plot = self
                .nodes_for_gate(corner)
                .iter()
                .find(|n| self.plot_of(n) == plot)
                .cloned();
            let Some(corner_node) = at_this_plot else {
                return Err(anyhow!(
                    "corner {corner} of this composite is not placed here, so the group cannot be linked as a whole"
                ));
            };
            moves.push((corner_node, corner.clone(), replacement.clone()));
        }

        for (corner_node, old_gate, new_gate) in moves {
            let collapsed = self
                .placements
                .get(&corner_node)
                .is_some_and(|p| p.collapsed);
            self.record_placement(corner_node.clone(), new_gate, collapsed);
            self.unindex_view_at(&plot, &old_gate);
            self.reindex_view(&corner_node);
        }
        // After every corner, not inside the loop: a composite is reachable
        // while any corner holds a position, so a sweep between corners would
        // see a half-linked group and keep it.
        self.collect_stranded_ghosts();
        Ok(())
    }

    /// Give this position a gate of its own again, copying the geometry it
    /// currently shares. The other positions keep the original.
    pub fn unlink_node(&mut self, node: &NodeId) -> anyhow::Result<GateId> {
        let Some(shared) = self.gate_for_node(node).cloned() else {
            return Err(anyhow!("no such position in the tree: {node}"));
        };
        if !self.is_linked(&shared) {
            return Err(anyhow!("this gate is only applied at one point"));
        }
        let Some(gate) = self.registered_gate(&shared) else {
            return Err(anyhow!("gate {shared} is not registered"));
        };

        // A composite is one gate spread over its corners, so a copy needs an id
        // for the group and one for every corner, and all of them re-pointed
        // together.
        if gate.is_composite() {
            return self.unlink_composite(node, &gate);
        }

        let new_id: GateId = Arc::from(Uuid::new_v4().to_string().as_str());
        let copy: Arc<dyn DrawableGate> = gate
            .with_new_id(new_id.clone())
            .ok_or_else(|| anyhow!("this kind of gate cannot be copied, so it cannot be unlinked"))?
            .into();

        self.gate_store
            .primary_and_subgate_registry
            .insert(new_id.clone(), copy);

        let collapsed = self.placements.get(node).is_some_and(|p| p.collapsed);
        let plot = self.plot_of(node);
        self.record_placement(node.clone(), new_id.clone(), collapsed);
        self.unindex_view_at(&plot, &shared);
        self.reindex_view(node);
        Ok(new_id)
    }

    /// Give this composite's position a group of its own, copying the geometry
    /// it currently shares. The other positions keep the original.
    ///
    /// Returns the new composite's id.
    fn unlink_composite(
        &mut self,
        node: &NodeId,
        gate: &Arc<dyn DrawableGate>,
    ) -> anyhow::Result<GateId> {
        let plot = self.plot_of(node);
        let old_corners = gate.get_inner_gate_ids();

        // Resolve every corner at this plot before copying anything: an
        // incomplete group must fail without leaving a half-built composite
        // registered.
        let mut corner_nodes = Vec::with_capacity(old_corners.len());
        for corner in &old_corners {
            let Some(corner_node) = self
                .nodes_for_gate(corner)
                .iter()
                .find(|n| self.plot_of(n) == plot)
                .cloned()
            else {
                return Err(anyhow!(
                    "corner {corner} of this composite is not placed here, so the group cannot be unlinked as a whole"
                ));
            };
            corner_nodes.push(corner_node);
        }

        let new_id: GateId = Arc::from(Uuid::new_v4().to_string().as_str());
        let copy: Arc<dyn DrawableGate> = gate
            .with_new_group_id(new_id.clone())
            .ok_or_else(|| {
                anyhow!("this kind of composite cannot be copied, so it cannot be unlinked")
            })?
            .into();
        let new_corners = copy.get_inner_gate_ids();
        if new_corners.len() != old_corners.len() {
            return Err(anyhow!("the copy has a different number of corners"));
        }

        // A composite is registered under its own id as well as each corner's,
        // all aliased to one Arc, which is what lets a corner id resolve to the
        // whole gate at filter time.
        self.gate_store
            .primary_and_subgate_registry
            .insert(new_id.clone(), copy.clone());
        for corner in &new_corners {
            self.gate_store
                .primary_and_subgate_registry
                .insert(corner.clone(), copy.clone());
        }

        for ((corner_node, old_corner), new_corner) in corner_nodes
            .iter()
            .zip(old_corners.iter())
            .zip(new_corners.iter())
        {
            let collapsed = self
                .placements
                .get(corner_node)
                .is_some_and(|p| p.collapsed);
            self.record_placement(corner_node.clone(), new_corner.clone(), collapsed);
            self.unindex_view_at(&plot, old_corner);
            self.reindex_view(corner_node);
        }

        Ok(new_id)
    }

    /// Stop drawing a gate on one plot.
    ///
    /// Scoped to the plot, not to the gate: the renderer lists gates per plot,
    /// and a plot is a parent position. A gate that is no longer shown at any
    /// position under `parent` comes off that plot even when it is still applied
    /// elsewhere - and one that another sibling position still shows has to
    /// stay, however many positions it has lost.
    fn unindex_view_at(&mut self, parent: &Arc<str>, gate_id: &GateId) {
        let still_shown_here = self.nodes_for_gate(gate_id).iter().any(|node| {
            self.parent_node(node)
                .map(|p| p.as_arc().clone())
                .unwrap_or_else(|| ROOTGATE.clone())
                == *parent
        });
        if still_shown_here {
            return;
        }

        for (key, ids) in self.gate_ids_by_view.iter_mut() {
            if key.parental_gate_id.as_ref() == Some(parent) {
                ids.retain(|id| id != gate_id);
            }
        }
        self.gate_ids_by_view.retain(|_, ids| !ids.is_empty());
    }

    /// The plot a position sits on.
    fn plot_of(&self, node: &NodeId) -> Arc<str> {
        self.parent_node(node)
            .map(|p| p.as_arc().clone())
            .unwrap_or_else(|| ROOTGATE.clone())
    }

    /// File the gate at this position under the plot its parent defines.
    fn reindex_view(&mut self, node: &NodeId) {
        let Some(gate_id) = self.gate_for_node(node).cloned() else {
            return;
        };
        let Some(gate) = self.registered_gate(&gate_id) else {
            return;
        };
        let parent = self.plot_of(node);
        let (x, y) = gate.get_params();
        let key = GatesOnPlotKey::new(x, y, Some(parent));
        let ids = self.gate_ids_by_view.entry(key).or_default();
        if !ids.contains(&gate_id) {
            ids.push(gate_id);
        }
    }

    /// Put a newly created gate into the tree at its own node.
    ///
    /// A gate created here uses its own id as its node id - unique, and one
    /// placement. Every path that creates a gate goes through this, so none can
    /// add to the hierarchy without recording the placement that goes with it.
    pub fn place_new_gate(
        &mut self,
        parent: Option<GateId>,
        gate_id: GateId,
    ) -> anyhow::Result<NodeId> {
        let parent_node = self.as_parent_node(&parent.unwrap_or_else(|| ROOTGATE.clone()));
        self.hierarchy
            .add_gate_child(parent_node.as_arc().clone(), gate_id.clone(), None)?;
        let node = NodeId::from(gate_id.clone());
        self.record_placement(node.clone(), gate_id, false);
        Ok(node)
    }

    /// Interpret an id coming from the UI as a tree position.
    ///
    /// The sidebar now hands out node ids, and a gate created here uses its own
    /// id as its node id, so both are already nodes. An id that is only a gate -
    /// an imported gate named by a caller that has not been converted yet -
    /// resolves to its first placement. Ambiguous for a linked gate, which is
    /// why the UI passes nodes.
    pub fn as_parent_node(&self, id: &Arc<str>) -> NodeId {
        let node = NodeId::from(id.clone());
        if **id == **ROOTGATE || self.placements.contains_key(&node) {
            return node;
        }
        self.primary_node_for_gate(id)
            .unwrap_or_else(|| NodeId::from(ROOTGATE.clone()))
    }

    /// The first place this gate appears, for callers that hold a gate and need
    /// *a* tree position. Ambiguous for a linked gate by construction: prefer a
    /// caller that already knows which node it means.
    pub fn primary_node_for_gate(&self, gate_id: &GateId) -> Option<NodeId> {
        self.nodes_for_gate(gate_id).first().cloned()
    }

    /// The node above this one in the tree.
    pub fn parent_node(&self, node: &NodeId) -> Option<NodeId> {
        self.hierarchy
            .get_parent(node.as_str())
            .cloned()
            .map(NodeId::from)
    }

    /// Child nodes of a node, in sibling order.
    pub fn child_nodes(&self, node: &NodeId) -> Vec<NodeId> {
        self.hierarchy
            .get_children(node.as_str())
            .into_iter()
            .cloned()
            .map(NodeId::from)
            .collect()
    }

    /// Root nodes of the tree.
    pub fn root_nodes(&self) -> Vec<NodeId> {
        self.hierarchy
            .get_roots()
            .into_iter()
            .map(NodeId::from)
            .collect()
    }

    /// The gates to apply, root first, to reach the population this node sees -
    /// the node itself included.
    ///
    /// Node-scoped rather than gate-scoped: a linked gate's ancestors depend on
    /// which placement is being viewed, so a chain taken from the gate alone was
    /// whichever placement won the import.
    pub fn gate_chain_for_node(&self, node: &NodeId) -> Vec<GateId> {
        self.hierarchy
            .get_chain_to_root(node.as_str())
            .into_iter()
            .filter(|id| **id != **ROOTGATE)
            .filter_map(|id| self.gate_for_node(&NodeId::from(id)).cloned())
            .collect()
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

    /// The gate that applies to one sample: the more recently written of its
    /// own position and its groups' (see [`GateSubStore::position_for`]), else
    /// the global position.
    ///
    /// The same rule `get_current_sample` uses, for one gate rather than
    /// all of them - the export needs it per file when writing `perFileFilters`.
    pub fn gate_for_file(
        &self,
        gate_id: &GateId,
        file_id: &FileId,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
    ) -> Option<Arc<dyn DrawableGate>> {
        let groups = metadata.get(file_id).into_iter().flatten();
        if let Some((_, gate)) = self.gate_store.position_for(gate_id, file_id, groups) {
            return Some(gate.clone());
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
    /// Taken from its first placement; see `node_order` for a specific one.
    pub fn gate_order(&self, gate_id: &GateId) -> Option<u64> {
        self.node_order(&self.primary_node_for_gate(gate_id)?)
    }

    /// A node's sort order among its siblings.
    pub fn node_order(&self, node: &NodeId) -> Option<u64> {
        self.hierarchy.get_order(node.as_str())
    }

    /// Which gate ids carry a per-group and a per-sample override.
    ///
    /// Returned as sets rather than answered per gate: the sidebar asks for
    /// every row it draws, and the override maps hold an entry per specimen or
    /// file, so a scan each time would be a scan of thousands per row.
    /// The metadata column this gate's positions are grouped by, if any.
    ///
    /// Omiq stores one filter per file whatever drives it, and names the
    /// grouping column separately. Without that name a set of per-file
    /// positions reads as per-sample even when every file of a specimen holds
    /// the same one, so the same run came back group-specific for containers
    /// that already carried the name and sample-specific for the rest.
    ///
    /// A gate can hold positions under more than one column (see
    /// [`GateSubStore::newest_group_position`]); this is the one written most
    /// recently, and [`GateState::group_columns_newest_first`] lists them all.
    pub fn group_override_column(
        &self,
        gate_id: &GateId,
    ) -> Option<crate::omiq::metadata::MetaDataParameter> {
        self.group_columns_newest_first(gate_id).into_iter().next()
    }

    /// Every column this gate's per-group positions are held under, the most
    /// recently written first.
    pub fn group_columns_newest_first(
        &self,
        gate_id: &GateId,
    ) -> Vec<crate::omiq::metadata::MetaDataParameter> {
        self.gate_store.group_columns_newest_first(gate_id)
    }

    pub fn overridden_ids(&self) -> (FxHashSet<GateId>, FxHashSet<GateId>) {
        let groups = self
            .gate_store
            .group_position_overrides
            .keys()
            .map(|(id, _)| id.clone())
            .collect();
        let samples = self
            .gate_store
            .sample_position_overrides
            .keys()
            .map(|(id, _)| id.clone())
            .collect();
        (groups, samples)
    }

    /// Every file this gate resolves differently for than its global position.
    ///
    /// The export needs this because a position written in this session - by
    /// the autogater, or by dragging a gate on one sample - belongs to no list
    /// captured at import. A container that arrived global has no per-file ids
    /// at all, so without asking, an override written afterwards reaches the
    /// screen and never reaches the file.
    pub fn files_with_own_position(
        &self,
        gate_id: &GateId,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
    ) -> Vec<FileId> {
        let global = self.registered_gate(gate_id);
        metadata
            .keys()
            .filter(|file| {
                match (self.gate_for_file(gate_id, file, metadata), &global) {
                    // Resolved to something other than the registry entry.
                    (Some(resolved), Some(global)) => !Arc::ptr_eq(&resolved, global),
                    (Some(_), None) => true,
                    _ => false,
                }
            })
            .cloned()
            .collect()
    }

    /// Write a gate into one of the three tiers.
    ///
    /// The store methods write back into the tier a gate was *resolved* from,
    /// which is right for an edit - dragging a global gate should move the
    /// global gate. Positioning by rule is the other case: it takes a gate that
    /// resolved globally and gives one specimen its own copy, so the tier is
    /// named rather than inherited.
    pub fn place_gate(
        &mut self,
        ids: &[GateId],
        gate: &Arc<dyn DrawableGate>,
        source: &GateSource,
    ) {
        self.gate_store.insert_for_source(ids, gate, source);
    }

    /// The gate registered under an id, if any.
    pub fn registered_gate(&self, gate_id: &GateId) -> Option<Arc<dyn DrawableGate>> {
        self.gate_store
            .primary_and_subgate_registry
            .get(gate_id)
            .cloned()
    }

    /// The gate above this one in the tree, or `None` if it has no node.
    ///
    /// Resolved through this gate's first placement, so it is ambiguous for a
    /// linked gate - `parent_node` is the unambiguous form. The root has no
    /// gate, so a top-level gate reports `ROOTGATE`.
    pub fn hierarchy_parent(&self, gate_id: &GateId) -> Option<GateId> {
        let parent = self.parent_node(&self.primary_node_for_gate(gate_id)?)?;
        if *parent.as_str() == **ROOTGATE {
            return Some(ROOTGATE.clone());
        }
        self.gate_for_node(&parent).cloned()
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
        // delete_subtree unlinks every node it removes, so afterwards the parent
        // lookup returns None and the view key below would be built against the
        // root - leaving the deleted gate's id in gate_ids_by_view, still
        // rendering.
        //
        // The tree is keyed by node, so this walks every placement of every
        // doomed gate. Deleting the gate deletes it everywhere it appears;
        // removing a single instance of a linked gate is a different operation
        // on one node.
        let mut gates_to_delete: HashSet<Arc<str>> = HashSet::default();
        let mut parents: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        let mut doomed_roots: Vec<NodeId> = Vec::new();

        for brother in &roots {
            let nodes = self.nodes_for_gate(brother).to_vec();
            // A gate with no node - a ghost, or a composite, which is registered
            // under its own id but puts only its corners in the tree - still has
            // to leave the registry.
            if nodes.is_empty() {
                gates_to_delete.insert(brother.clone());
                parents
                    .entry(brother.clone())
                    .or_insert_with(|| ROOTGATE.clone());
                continue;
            }

            for node in nodes {
                let subtree = std::iter::once(node.as_arc().clone())
                    .chain(self.hierarchy.get_descendants(node.as_str()))
                    .collect::<Vec<_>>();

                for doomed in subtree {
                    let doomed = NodeId::from(doomed);
                    let parent = self
                        .parent_node(&doomed)
                        .map(|p| p.as_arc().clone())
                        .unwrap_or_else(|| ROOTGATE.clone());
                    if let Some(gate) = self.gate_for_node(&doomed).cloned() {
                        parents.insert(gate.clone(), parent);
                        gates_to_delete.insert(gate);
                    }
                }
                doomed_roots.push(node);
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

        for node in doomed_roots {
            for removed in self.hierarchy.delete_subtree(node.as_str()) {
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
        self.gate_store
            .retain_sample_positions(|(gid, _file_id)| !gates_to_delete.contains(gid));
        self.gate_store
            .retain_group_positions(|(gid, _group_id)| !gates_to_delete.contains(gid));

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
        self.boolean_gate_links
            .retain(|id, dependents| !dependents.is_empty() && !gates_to_delete.contains(id));

        // Deleting the last boolean that referenced a ghost strands it. Sweep
        // once the delete has settled, so the walk sees the tree as it now is.
        self.collect_stranded_ghosts();
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
        // The tree is keyed by node, and so is the view index: a linked gate at
        // two points in the tree sits on two different populations, so it is the
        // placement, not the gate, that says which plot this belongs to.
        let parent_node =
            self.as_parent_node(&parental_gate_id.unwrap_or_else(|| ROOTGATE.clone()));
        let key = GatesOnPlotKey::new(
            x_param.clone(),
            y_param.clone(),
            Some(parent_node.as_arc().clone()),
        );
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
                println!("Adding composite subgate gate {sg} with parent {parent_node}");
                self.hierarchy
                    .add_gate_child(parent_node.as_arc().clone(), sg.clone(), None)?;
                self.record_placement(NodeId::from(sg.clone()), sg.clone(), false);
                self.gate_store
                    .primary_and_subgate_registry
                    .insert(sg, g.clone());
            }
        } else {
            println!("Adding gate {} with parent {parent_node}", g.get_id());
            self.hierarchy
                .add_gate_child(parent_node.as_arc().clone(), g.get_id(), None)?;
            self.record_placement(NodeId::from(g.get_id()), g.get_id(), false);
        }

        self.gate_store
            .primary_and_subgate_registry
            .insert(gate_key.clone(), g.clone());

        Ok(())
    }
}

impl GateState {
    /// Resolve every gate for one sample: the more recently written of its own
    /// position and its groups' (see [`GateSubStore::position_for`]), else the
    /// global position.
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

            for (default_id, base_arc) in &registry.0 {
                if let Some((source, gate)) = self
                    .gate_store
                    .position_for(default_id, &file_id, group_ids)
                {
                    active_gates.insert(default_id.clone(), gate.clone().into());
                    gate_origins.insert(default_id.clone(), source);
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
    /// Replace every gate with the ones in an Omiq export.
    ///
    /// A second import is a *replacement*, not an addition.
    /// [`upload_gates_from_file`](Self::upload_gates_from_file) registers what
    /// the file holds alongside whatever is already there, which is right for
    /// filling an empty document and wrong for loading a different one: the
    /// previous document's gates would stay in the registry, unreachable from
    /// the new tree but still resolved into every sample and still written back
    /// out on export.
    ///
    /// The new document is built in a state of its own and swapped in only once
    /// it has parsed, so a malformed file costs nothing - what is on screen
    /// afterwards is what was there before, rather than half of a document that
    /// failed to load.
    pub fn replace_gates_from_file(
        &mut self,
        path: PathBuf,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axis_settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    ) -> anyhow::Result<()> {
        *self = Self::from_gating_file(path, metadata, axis_settings)?;
        Ok(())
    }

    /// A whole document, read from an Omiq export into a state of its own.
    ///
    /// Separate from [`replace_gates_from_file`](Self::replace_gates_from_file)
    /// because reading one is slow enough to want a worker thread - a real file
    /// is hundreds of kilobytes over a few hundred containers - and a `Store`
    /// cannot be written from one. The editor parses through this, then swaps
    /// the result in on the thread that owns the store; the two paths share this
    /// one definition of what loading a file means.
    pub fn from_gating_file(
        path: PathBuf,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axis_settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    ) -> anyhow::Result<Self> {
        let mut fresh = GateState::default();
        fresh.upload_gates_from_file(path, metadata, axis_settings)?;
        Ok(fresh)
    }

    /// Build the gate tree from an Omiq experiment export.
    pub fn upload_gates_from_file(
        &mut self,
        path: PathBuf,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axis_settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    ) -> anyhow::Result<()> {
        // Quadrants are built against the axes, by clamping into their range:
        // an unusable axis would panic partway through the import (B-AX-3).
        // The scaling reader refuses such a file, so this is the second line.
        let mut problems: Vec<String> = axis_settings
            .values()
            .filter_map(AxisInfo::problem)
            .collect();
        if !problems.is_empty() {
            problems.sort();
            return Err(anyhow!(
                "the scaling cannot be used to lay out the gates: {}",
                problems.join("; ")
            ));
        }

        // 1. Open the file
        let text = std::fs::read_to_string(&path)?;

        // 2. Deserialize into your ExperimentJson struct, and keep an untyped
        // view alongside it for the fields the typed one does not model.
        let experiment: crate::omiq::deserialise::ExperimentJson = serde_json::from_str(&text)?;
        let raw: serde_json::Value = serde_json::from_str(&text)?;

        let mut reachable: FxHashSet<Arc<str>> = FxHashSet::default();
        for node in experiment.tree.nodes.values() {
            collect_reachable(
                &node.filter_container_id,
                &experiment.tree.filter_containers,
                &mut reachable,
            );
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
            if !reachable.contains(id) {
                continue;
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

        // 2. Sort nodes by their depth in the tree, so parents always exist
        // before their children.
        //
        // Depth is found by walking each node's `parentId` to the root. A tree
        // has no loops, and Omiq never writes one, so a node that is its own
        // parent - or two that are each other's - is a damaged file: refused
        // by name, rather than walked round forever. That walk used to hang
        // the Workspace tab on "Loading" (B-OMIQ-2).
        let mut depths: FxHashMap<&str, usize> = FxHashMap::default();
        for node in experiment.tree.nodes.values() {
            let mut depth = 0;
            let mut seen: FxHashSet<&str> = FxHashSet::default();
            seen.insert(&node.id);
            let mut current_parent: &str = &node.parent_id;
            while !current_parent.is_empty() {
                if !seen.insert(current_parent) {
                    return Err(anyhow!(
                        "the file is damaged: its gate tree loops - node {} is its own ancestor, by way of node {current_parent}",
                        node.id
                    ));
                }
                match experiment.tree.nodes.get(current_parent) {
                    Some(parent) => {
                        current_parent = &parent.parent_id;
                        depth += 1;
                    }
                    // A parent the file does not contain. It opens anyway, the
                    // node under the root; see the test of that case.
                    None => break,
                }
            }
            depths.insert(&node.id, depth);
        }
        let mut sorted_nodes: Vec<_> = experiment.tree.nodes.values().collect();
        sorted_nodes.sort_by_key(|node| depths[&*node.id]);

        // Build the tree. The hierarchy is keyed by node, so Omiq's own node ids
        // go in directly and a parent is just `node.parent_id` - no mapping from
        // node to container, and no collapsing of a gate that appears at several
        // points into whichever node happened to be processed last.
        for node in sorted_nodes.into_iter() {
            let parent_id = if *"" != *node.parent_id {
                node.parent_id.clone()
            } else {
                ROOTGATE.clone()
            };
            self.hierarchy
                .add_gate_child(parent_id, node.id.clone(), Some(node.ord))?;
            self.record_placement(
                NodeId::from(node.id.clone()),
                node.filter_container_id.clone(),
                node.collapsed,
            );
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
                        self.gate_store
                            .primary_and_subgate_registry
                            .insert(gate_id.clone(), gate);

                        // Drawn on the plot of every position it occupies, not
                        // just the first: a gate Omiq applies at several points
                        // belongs on each of those plots.
                        //
                        // A gate with no position is a ghost - kept alive only
                        // by a boolean that references it - and belongs on no
                        // plot, which falls out of this loop being empty. It is
                        // registered above regardless, or that boolean cannot
                        // resolve its operand.
                        for node in self.nodes_for_gate(&gate_id).to_vec() {
                            self.reindex_view(&node);
                        }
                    }
                    GateSource::Group(key) => {
                        self.gate_store.set_group_position(key, gate);
                    }
                    GateSource::Sample(key) => {
                        self.gate_store.set_sample_position(key, gate);
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
                        let parent = self
                            .primary_node_for_gate(any_subgate)
                            .and_then(|n| self.parent_node(&n))
                            .map(|n| n.as_arc().clone())
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Could not locate parent of subgate {} in hierarchy",
                                    any_subgate
                                )
                            })?;
                        let params = gate.get_params();
                        let key = GatesOnPlotKey::new(params.0, params.1, Some(parent));

                        self.gate_ids_by_view
                            .entry(key)
                            .or_default()
                            .push(id.clone());
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
                            .set_group_position(key.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            self.gate_store
                                .set_group_position((sub_id, key.1.clone()), gate.clone());
                        }
                    }
                    GateSource::Sample(key) => {
                        self.gate_store
                            .set_sample_position(key.clone(), gate.clone());
                        for sub_id in subgate_ids {
                            self.gate_store
                                .set_sample_position((sub_id, key.1.clone()), gate.clone());
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
    /// Subscribe the caller to every change in the gating - the gates at
    /// every tier, and the tree - and to nothing else. Selecting a gate writes
    /// the store as well, and a caller asking whether the gating changed should
    /// not wake for that.
    fn subscribe_to_gating(&self) {
        let _ = self.gate_store().read();
        let _ = self.hierarchy().read();
        let _ = self.placements().read();
    }

    fn get_current_sample(
        &mut self,
        file_id: FileId,
        group_ids: &FxHashMap<MetaDataParameter, GroupId>,
    ) -> Result<GateOverrideResolver> {
        // Subscribe to the three tiers the resolver is built from.
        //
        // The plain-data function below reads through `peek`, which does not
        // subscribe. Without these reads the memo that builds the resolver has
        // no dependency on the gate store at all: it runs once and never again,
        // so every edit is written to the store and never seen. A dragged gate
        // snaps back to the geometry the stale resolver still holds, and a newly
        // created gate is filed on its plot but dropped by `get_gates_for_plot`,
        // which resolves each id through the resolver before drawing it.
        {
            let registry = self.gate_store().primary_and_subgate_registry();
            let by_sample = self.gate_store().sample_position_overrides();
            let by_group = self.gate_store().group_position_overrides();
            let _ = registry.read();
            let _ = by_sample.read();
            let _ = by_group.read();
        }

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

        self.write()
            .place_new_gate(parental_gate_id, gate_id.clone())?;

        self.gate_store()
            .primary_and_subgate_registry()
            .write()
            .insert(g.get_id(), g.clone());

        Ok(())
    }

    fn remove_gate(&mut self, gate_id: GateId) -> anyhow::Result<()> {
        self.write().remove_gate(gate_id)
    }

    /// Move one point of a gate, for a drag in progress.
    ///
    /// `drag` is taken by reference so the anchor can be filled in here, on the
    /// first move of the drag, from the gate as it stands before anything has
    /// been written. Every later move of the same drag reuses it, which is what
    /// keeps the gate pinned once the pointer crosses it - see
    /// [`PointDragData::anchor`].
    fn move_gate_point(
        &mut self,
        gate_id: GateId,
        drag: &mut PointDragData,
        new_point: (f32, f32),
        plot_map: &PlotMapper,
        resolver: &GateOverrideResolver,
    ) -> anyhow::Result<()> {
        let current = resolver.resolve_drawable(&gate_id)?;
        let point_idx = drag.point_index();
        if let Some(anchor) = current.drag_anchor(point_idx) {
            drag.set_anchor_once(anchor);
        }
        let new_gate = current.replace_point(new_point, point_idx, drag.anchor(), plot_map)?;
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
        let updates = {
            let key_bind = self.gate_ids_by_view();
            let kbp = &*key_bind.peek();
            let Some(ids) = kbp.get(&key) else {
                return Err(anyhow::anyhow!("No keys found"));
            };
            oriented_to_plot(ids, &x, &y, resolver)?
        };
        // Almost always nothing to do: a gate needs writing only when the plot
        // shows its axes the other way round. Writing regardless notified
        // everything subscribed to the gates on every change of file or plot -
        // re-rendering what had not changed, and stopping a rules run as if
        // the gating had been edited.
        if updates.is_empty() {
            return Ok(());
        }
        self.gate_store().with_mut(|s| s.apply_orientation(updates));

        Ok(())
    }

    /// See [`GateSubStore::rescale_channel`].
    fn rescale_gates(
        &mut self,
        marker: &Arc<str>,
        old_axis_options: &AxisInfo,
        new_axis_options: &AxisInfo,
    ) -> Result<(), Vec<String>> {
        let mut result = Ok(());
        self.gate_store().with_mut(|s| {
            result = s.rescale_channel(marker, old_axis_options, new_axis_options);
        });
        result
    }

    /// See [`GateSubStore::relimit_channel`].
    fn set_current_axis_limits(
        &mut self,
        axis_name: Arc<str>,
        lower: f32,
        upper: f32,
        transform: TransformType,
    ) -> Result<(), Vec<String>> {
        let mut result = Ok(());
        self.gate_store().with_mut(|s| {
            result = s.relimit_channel(&axis_name, lower, upper, &transform);
        });
        result
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

    /// See [`GateState::replace_gates_from_file`].
    fn replace_gates_from_file(
        &mut self,
        path: PathBuf,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
        axis_settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    ) -> anyhow::Result<()> {
        self.write()
            .replace_gates_from_file(path, metadata, axis_settings)
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
            store
                .primary_and_subgate_registry
                .get(&original.get_id())
                .unwrap(),
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

        assert!(
            state
                .gate_store
                .primary_and_subgate_registry
                .contains_key(&id)
        );
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
        let on_plot = state
            .gate_ids_by_view
            .get(&key)
            .cloned()
            .unwrap_or_default();
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

        assert!(
            !state
                .gate_store
                .primary_and_subgate_registry
                .contains_key(&id)
        );
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

        assert!(
            !state
                .gate_store
                .primary_and_subgate_registry
                .contains_key(&parent)
        );
        assert!(
            !state
                .gate_store
                .primary_and_subgate_registry
                .contains_key(&child),
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
        state
            .gate_store
            .sample_position_overrides
            .insert((parent.clone(), file("s1")), moved.clone());
        state
            .gate_store
            .sample_position_overrides
            .insert((child.clone(), file("s1")), moved.clone());

        state.remove_gate(parent.clone()).unwrap();

        assert!(
            state.gate_store.sample_position_overrides.is_empty(),
            "overrides left behind: {:?}",
            state
                .gate_store
                .sample_position_overrides
                .keys()
                .collect::<Vec<_>>()
        );
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
            state
                .gate_store
                .primary_and_subgate_registry
                .keys()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn removing_an_unknown_gate_is_harmless() {
        let mut state = GateState::default();
        let id = add_rect(&mut state, None);

        state.remove_gate(Arc::from("does-not-exist")).unwrap();

        assert!(
            state
                .gate_store
                .primary_and_subgate_registry
                .contains_key(&id)
        );
    }

    // ── GateState::get_current_sample ─────────────────────────────────────────

    fn groups(pairs: &[(&str, &str)]) -> FxHashMap<MetaDataParameter, GroupId> {
        pairs
            .iter()
            .map(|(p, g)| (Arc::from(*p) as Arc<str>, Arc::from(*g) as Arc<str>))
            .collect()
    }

    // ── per-group positions under more than one column: the newest applies ──

    /// A registered gate `r`, and a file in group `one` under both `Type` and
    /// `SampleID`.
    fn grouped_twice() -> (GateState, GateId, FxHashMap<MetaDataParameter, GroupId>) {
        let mut state = GateState::default();
        let global = rectangle("r");
        state
            .gate_store
            .insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        let groups = groups(&[("Type", "one"), ("SampleID", "one")]);
        (state, global.get_id(), groups)
    }

    fn place(
        state: &mut GateState,
        id: &GateId,
        column: &str,
        group: &str,
    ) -> Arc<dyn DrawableGate> {
        let gate = rectangle("r");
        state.place_gate(
            std::slice::from_ref(id),
            &gate,
            &GateSource::Group((id.clone(), group_key(column, group))),
        );
        gate
    }

    fn resolved(
        state: &GateState,
        id: &GateId,
        groups: &FxHashMap<MetaDataParameter, GroupId>,
    ) -> Arc<dyn DrawableGate> {
        state
            .get_current_sample(file("f"), groups)
            .active_gates
            .get(id)
            .unwrap()
            .0
            .clone()
    }

    #[test]
    fn the_newest_group_position_applies_whichever_column_it_is_under() {
        // Both orders, so no hash order can make this pass by luck.
        for (first, second) in [("Type", "SampleID"), ("SampleID", "Type")] {
            let (mut state, id, groups) = grouped_twice();
            place(&mut state, &id, first, "one");
            let newest = place(&mut state, &id, second, "one");
            assert!(
                Arc::ptr_eq(&resolved(&state, &id, &groups), &newest),
                "{first} then {second}"
            );
        }
    }

    #[test]
    fn an_older_group_position_still_holds_where_nothing_newer_applies() {
        let (mut state, id, _) = grouped_twice();
        let by_type = place(&mut state, &id, "Type", "one");
        place(&mut state, &id, "SampleID", "one");
        // Another specimen of the same type: the newer position is not its.
        let other = groups(&[("Type", "one"), ("SampleID", "two")]);
        assert!(Arc::ptr_eq(&resolved(&state, &id, &other), &by_type));
    }

    #[test]
    fn writing_a_column_again_makes_it_the_newest() {
        let (mut state, id, groups) = grouped_twice();
        place(&mut state, &id, "Type", "one");
        place(&mut state, &id, "SampleID", "one");
        let again = place(&mut state, &id, "Type", "one");
        assert!(Arc::ptr_eq(&resolved(&state, &id, &groups), &again));
        assert_eq!(
            state.group_columns_newest_first(&id),
            vec![Arc::from("Type"), Arc::from("SampleID")] as Vec<MetaDataParameter>
        );
    }

    #[test]
    fn the_export_and_the_screen_resolve_a_file_the_same_way() {
        let (mut state, id, groups) = grouped_twice();
        place(&mut state, &id, "SampleID", "one");
        place(&mut state, &id, "Type", "one");
        let mut metadata: crate::omiq::metadata::MetaDataFileMap =
            im::HashMap::with_hasher(FxBuildHasher);
        metadata.insert(file("f"), groups.clone());
        assert!(Arc::ptr_eq(
            &state.gate_for_file(&id, &file("f"), &metadata).unwrap(),
            &resolved(&state, &id, &groups)
        ));
    }

    #[test]
    fn positions_written_straight_into_the_map_tie_by_column_name() {
        // Only tests write the map directly; the rule still must not hash.
        let (mut state, id, groups) = grouped_twice();
        let by_sample = rectangle("r");
        state
            .gate_store
            .group_position_overrides
            .insert((id.clone(), group_key("Type", "one")), rectangle("r"));
        state.gate_store.group_position_overrides.insert(
            (id.clone(), group_key("SampleID", "one")),
            by_sample.clone(),
        );
        assert!(Arc::ptr_eq(&resolved(&state, &id, &groups), &by_sample));
    }

    #[test]
    fn deleting_a_gate_forgets_when_its_positions_were_written() {
        let (mut state, id, _) = grouped_twice();
        state.place_new_gate(None, id.clone()).unwrap();
        place(&mut state, &id, "Type", "one");
        state.remove_gate(id.clone()).unwrap();
        assert!(state.group_columns_newest_first(&id).is_empty());
        assert!(state.gate_store.group_written.is_empty());
        assert!(state.gate_store.sample_written.is_empty());
    }

    // ── a sample's own position against its group's: the newer applies ─────

    #[test]
    fn a_group_position_written_after_a_samples_own_applies_to_it() {
        // A rules run positions the specimen after someone adjusted one of
        // its samples by hand: the run's answer is what that sample shows.
        let (mut state, id, groups) = grouped_twice();
        let own = rectangle("r");
        state.place_gate(
            std::slice::from_ref(&id),
            &own,
            &GateSource::Sample((id.clone(), file("f"))),
        );
        let run = place(&mut state, &id, "SampleID", "one");
        assert!(Arc::ptr_eq(&resolved(&state, &id, &groups), &run));
    }

    #[test]
    fn a_samples_own_position_written_after_its_groups_applies_to_it() {
        // And an adjustment made after the run beats the run.
        let (mut state, id, groups) = grouped_twice();
        place(&mut state, &id, "SampleID", "one");
        let own = rectangle("r");
        state.place_gate(
            std::slice::from_ref(&id),
            &own,
            &GateSource::Sample((id.clone(), file("f"))),
        );
        assert!(Arc::ptr_eq(&resolved(&state, &id, &groups), &own));
        // The other samples of the group keep the group's position.
        let other = state
            .get_current_sample(file("g"), &groups)
            .active_gates
            .get(&id)
            .unwrap()
            .0
            .clone();
        assert!(!Arc::ptr_eq(&other, &own));
    }

    #[test]
    fn the_resolver_names_the_tier_it_resolved_from() {
        // The editor writes a drag back into this tier, so it must be the one
        // the position actually came from.
        let (mut state, id, groups) = grouped_twice();
        let own = rectangle("r");
        state.place_gate(
            std::slice::from_ref(&id),
            &own,
            &GateSource::Sample((id.clone(), file("f"))),
        );
        place(&mut state, &id, "SampleID", "one");
        let resolver = state.get_current_sample(file("f"), &groups);
        assert_eq!(
            resolver.gate_origins.get(&id),
            Some(&GateSource::Group((
                id.clone(),
                group_key("SampleID", "one")
            )))
        );
    }

    // ── GateState::unchanged_since: whether a run's answers still apply ──────

    /// A document with one gate drawn at the root, and that gate's id.
    fn drawn() -> (GateState, GateId) {
        let mut state = GateState::default();
        let g = rectangle("r");
        state
            .gate_store
            .insert_for_source(&[g.get_id()], &g, &GateSource::Global);
        state.place_new_gate(None, g.get_id()).unwrap();
        (state, g.get_id())
    }

    #[test]
    fn a_snapshot_is_unchanged_since_itself() {
        let (state, _) = drawn();
        let snapshot = state.clone();
        assert!(state.unchanged_since(&snapshot));
    }

    #[test]
    fn any_edit_to_the_gating_is_a_change() {
        let (before, id) = drawn();
        let edits: Vec<(&str, Box<dyn Fn(&mut GateState)>)> = vec![
            (
                "the gate moved",
                Box::new(|s: &mut GateState| {
                    let moved = rectangle("r");
                    s.gate_store
                        .insert_for_source(&[moved.get_id()], &moved, &GateSource::Global);
                }),
            ),
            (
                "a sample given its own position",
                Box::new(|s: &mut GateState| {
                    let own = rectangle("r");
                    s.place_gate(
                        &[own.get_id()],
                        &own,
                        &GateSource::Sample((own.get_id(), file("s1"))),
                    );
                }),
            ),
            (
                "a specimen given its own position",
                Box::new(|s: &mut GateState| {
                    let own = rectangle("r");
                    s.place_gate(
                        &[own.get_id()],
                        &own,
                        &GateSource::Group((own.get_id(), group_key("SampleID", "A"))),
                    );
                }),
            ),
            (
                "a gate added",
                Box::new(|s: &mut GateState| {
                    let other = rectangle("q");
                    s.gate_store
                        .insert_for_source(&[other.get_id()], &other, &GateSource::Global);
                    s.place_new_gate(None, other.get_id()).unwrap();
                }),
            ),
            (
                "the gate deleted",
                Box::new({
                    let id = id.clone();
                    move |s: &mut GateState| s.remove_gate(id.clone()).unwrap()
                }),
            ),
            (
                "the whole document replaced",
                Box::new(|s: &mut GateState| *s = drawn().0),
            ),
        ];
        for (what, edit) in edits {
            let mut after = before.clone();
            edit(&mut after);
            assert!(!after.unchanged_since(&before), "{what}");
        }
    }

    #[test]
    fn selecting_a_gate_is_not_a_change() {
        let (before, id) = drawn();
        let mut after = before.clone();
        after.selected_gate = Some(id);
        assert!(after.unchanged_since(&before));
    }

    #[test]
    fn a_gate_with_no_override_resolves_to_its_global_position() {
        let mut state = GateState::default();
        let g = rectangle("r");
        state
            .gate_store
            .insert_for_source(&[g.get_id()], &g, &GateSource::Global);

        let resolver = state.get_current_sample(file("s1"), &groups(&[]));

        assert!(Arc::ptr_eq(
            &resolver.active_gates.get(&g.get_id()).unwrap().0,
            &g
        ));
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
        state
            .gate_store
            .insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        state
            .gate_store
            .sample_position_overrides
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
        state
            .gate_store
            .insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        state
            .gate_store
            .group_position_overrides
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

    /// Written straight into the maps - so of no known age - a sample's own
    /// position beats its group's, as it always has. Written through the
    /// store, the newer wins; see the tests after this one.
    #[test]
    fn a_sample_override_beats_a_group_override() {
        let mut state = GateState::default();
        let global = rectangle("r");
        let group_gate = rectangle("r");
        let sample_gate = rectangle("r");
        let key = group_key("$VOL", "high");

        state
            .gate_store
            .insert_for_source(&[global.get_id()], &global, &GateSource::Global);
        state
            .gate_store
            .group_position_overrides
            .insert((global.get_id(), key), group_gate);
        state
            .gate_store
            .sample_position_overrides
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
            state
                .gate_store
                .insert_for_source(&[g.get_id()], &g, &GateSource::Global);
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
                &mapper(),
                300.0,
                300.0,
                Arc::from(X),
                Arc::from(Y),
                None,
                None,
                PrimaryGateType::Quadrant,
                Some("q".to_string()),
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
        assert!(
            state.nodes_by_gate.get(&id).is_none(),
            "no empty vec left behind"
        );
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
        assert_eq!(
            state.gate_for_node(&NodeId::from(child.clone())),
            Some(&child)
        );
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

        assert_eq!(
            state.placement_count(&a),
            0,
            "no stale entry on the old gate"
        );
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

    // ── which files have a position of their own ──────────────────────────────

    fn metadata_for(files: &[(&str, &[(&str, &str)])]) -> crate::omiq::metadata::MetaDataFileMap {
        let mut map = im::HashMap::with_hasher(FxBuildHasher);
        for (f, columns) in files {
            map.insert(file(f), groups(columns));
        }
        map
    }

    #[test]
    fn only_files_whose_gate_resolves_differently_have_their_own_position() {
        // The export writes a per-file position only for these, so a file
        // listed here that has none - or one missed that has one - is a gate
        // written to the wrong place.
        let mut state = GateState::default();
        let global = rectangle("r");
        state.place_gate(&[global.get_id()], &global, &GateSource::Global);
        state.place_gate(
            &[global.get_id()],
            &rectangle("r"),
            &GateSource::Sample((global.get_id(), file("s1"))),
        );
        state.place_gate(
            &[global.get_id()],
            &rectangle("r"),
            &GateSource::Group((global.get_id(), group_key("Plate", "P2"))),
        );
        let metadata = metadata_for(&[
            ("s1", &[("Plate", "P1")]),
            ("s2", &[("Plate", "P1")]),
            ("s3", &[("Plate", "P2")]),
        ]);

        let mut own = state.files_with_own_position(&global.get_id(), &metadata);
        own.sort();
        assert_eq!(own, vec![file("s1"), file("s3")]);
    }

    #[test]
    fn a_gate_with_no_overrides_has_no_file_of_its_own() {
        let mut state = GateState::default();
        let global = rectangle("r");
        state.place_gate(&[global.get_id()], &global, &GateSource::Global);
        let metadata = metadata_for(&[("s1", &[]), ("s2", &[])]);
        assert!(
            state
                .files_with_own_position(&global.get_id(), &metadata)
                .is_empty()
        );
    }

    // ── ghosts ────────────────────────────────────────────────────────────────

    #[test]
    fn a_gate_nothing_reaches_is_collected_and_one_on_a_plot_is_kept() {
        let mut state = GateState::default();
        let placed = add_rect(&mut state, None);
        let ghost = rectangle("ghost");
        state.place_gate(&[ghost.get_id()], &ghost, &GateSource::Global);

        let dropped = state.collect_stranded_ghosts();
        assert_eq!(dropped, vec![ghost.get_id()]);
        assert!(!state.is_registered(&ghost.get_id()));
        assert!(state.is_registered(&placed));
        assert!(
            state.collect_stranded_ghosts().is_empty(),
            "nothing left to collect"
        );
    }

    // ── walking the tree ──────────────────────────────────────────────────────

    #[test]
    fn the_tree_can_be_walked_from_the_root_down_and_back_up() {
        let mut state = GateState::default();
        let parent = add_rect(&mut state, None);
        let parent_node = state.primary_node_for_gate(&parent).expect("placed");
        let child = {
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
                    Some("the child".to_string()),
                )
                .unwrap();
            state
                .registered_ids()
                .into_iter()
                .find(|id| {
                    state
                        .registered_gate(id)
                        .is_some_and(|g| g.get_name() == "the child")
                })
                .unwrap()
        };
        let child_node = state.primary_node_for_gate(&child).unwrap();

        assert_eq!(state.root_nodes(), vec![NodeId::from(ROOTGATE.clone())]);
        assert!(
            state
                .child_nodes(&NodeId::from(ROOTGATE.clone()))
                .contains(&parent_node)
        );
        assert_eq!(state.child_nodes(&parent_node), vec![child_node.clone()]);
        assert_eq!(state.parent_node(&child_node), Some(parent_node.clone()));
        assert_eq!(
            state.gate_chain_for_node(&child_node),
            vec![parent.clone(), child]
        );
        assert!(state.node_order(&child_node).is_some());
    }

    #[test]
    fn an_id_from_the_ui_is_read_as_a_tree_position() {
        let mut state = GateState::default();
        let gate = add_rect(&mut state, None);
        let node = state.primary_node_for_gate(&gate).unwrap();

        // A node id, a gate id, and the root all resolve; an unknown id
        // falls back to the root rather than inventing a position.
        assert_eq!(state.as_parent_node(node.as_arc()), node);
        assert_eq!(state.as_parent_node(&gate), node);
        assert_eq!(
            state.as_parent_node(&ROOTGATE),
            NodeId::from(ROOTGATE.clone())
        );
        assert_eq!(
            state.as_parent_node(&Arc::from("nowhere")),
            NodeId::from(ROOTGATE.clone())
        );
    }

    #[test]
    fn a_new_gate_is_placed_at_its_own_node() {
        let mut state = GateState::default();
        let g = rectangle("fresh");
        state.place_gate(&[g.get_id()], &g, &GateSource::Global);
        let node = state.place_new_gate(None, g.get_id()).unwrap();

        assert_eq!(node, NodeId::from(g.get_id()));
        assert_eq!(state.gate_for_node(&node), Some(&g.get_id()));
        assert_eq!(state.placement_count(&g.get_id()), 1);
    }
}
