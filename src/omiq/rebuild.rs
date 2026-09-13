//! Everything an Omiq gating file carries that the editor does not itself need,
//! captured on import so a new one can be written from scratch.
//!
//! This is deliberately *beside* the gate registry rather than on the gates.
//! None of it changes when a gate is dragged - it describes the gate's identity
//! in the Omiq document, not its shape - so keying it by gate id means it
//! survives every edit for free. Geometry-derived provenance is the exception
//! and lives on the gate: see `EllipseHandles`.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::gate_editor::gates::gate_store::{FileId, GateId};
use crate::omiq::deserialise::{ExperimentJson, FilterContainer, GateSerialized};

/// Which Omiq gate type a gate arrived as.
///
/// The shape alone cannot tell us what to write back: a quadrant's corners are
/// rectangles in the file but are held as polygons once imported, and a
/// bisector's halves are `RangeGate`s that become line gates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum OmiqGateType {
    Rectangle,
    Polygon,
    Ellipse,
    Range,
    Angle,
}

impl OmiqGateType {
    pub fn of(gate: &GateSerialized) -> Option<Self> {
        Some(match gate {
            GateSerialized::Rectangle { .. } => Self::Rectangle,
            GateSerialized::Polygon { .. } => Self::Polygon,
            GateSerialized::Ellipse { .. } => Self::Ellipse,
            GateSerialized::Line { .. } => Self::Range,
            GateSerialized::Angle { .. } => Self::Angle,
            GateSerialized::Unknown => return None,
        })
    }

    /// The string Omiq writes in the filter's `type` field.
    pub fn wire_name(&self) -> &'static str {
        match self {
            Self::Rectangle => "RectangleGate",
            Self::Polygon => "PolygonGate",
            Self::Ellipse => "EllipseGate",
            Self::Range => "RangeGate",
            Self::Angle => "AngleGate",
        }
    }
}

/// The document-level fields, which sit outside `tree` and are otherwise lost.
///
/// `extra` catches anything a future Omiq version adds that we do not model, so
/// a round trip does not quietly drop it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmiqDocumentHeader {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub dataset_id: i64,
    #[serde(default)]
    pub inverted: bool,
    #[serde(default)]
    pub task_id: i64,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub workflow_id: i64,
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// What one gate needs in order to be written back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OmiqRebuildData {
    /// The tree node that pointed at this gate. Omiq keys nodes separately from
    /// filter containers; the node carries the position in the tree.
    pub node_id: Option<Arc<str>>,
    /// Empty string at the root; `None` for a nodeless ghost.
    pub parent_node_id: Option<Arc<str>>,
    pub ord: u64,
    pub collapsed: bool,
    /// `"DEFAULT"` on every atomic container seen so far. Kept verbatim.
    pub container_type: Arc<str>,
    /// e.g. `"IinB_QUAD3"` - the composite group and this gate's position in it.
    /// Retained even when the group was broken up, so the gate exports as the
    /// corner it was.
    pub group_id: Option<String>,
    /// The metadata column driving per-group positions, e.g. `"Donor_Day"`.
    pub md: Option<Arc<str>>,
    /// Which Omiq gate type this arrived as.
    pub source_type: Option<OmiqGateType>,
    /// Exactly the files Omiq listed a per-file position for, so a group
    /// override fans back out to the same set rather than one derived from the
    /// metadata - which could differ.
    pub per_file_ids: Vec<FileId>,
}

/// Everything captured from one gating file.
#[derive(Clone, Debug, Default)]
pub struct OmiqRebuildStore {
    pub header: OmiqDocumentHeader,
    pub gates: FxHashMap<GateId, OmiqRebuildData>,
    /// Containers with no node that no live gate reaches. They are inert for
    /// evaluation but are kept verbatim and written back untouched: a composite
    /// member that a live boolean references looks exactly like these until the
    /// reachability walk says otherwise, and dropping one would break that
    /// boolean in Omiq.
    pub ghost_containers: FxHashMap<GateId, serde_json::Value>,
}

impl OmiqRebuildStore {
    pub fn get(&self, gate_id: &GateId) -> Option<&OmiqRebuildData> {
        self.gates.get(gate_id)
    }

    pub fn is_empty(&self) -> bool {
        self.gates.is_empty() && self.ghost_containers.is_empty()
    }

    /// Capture a parsed experiment.
    ///
    /// `raw` is the same document as untyped JSON, used for the header and for
    /// the verbatim ghost containers. Taking both avoids re-parsing and keeps
    /// the typed and untyped views of the file in step.
    pub fn capture(experiment: &ExperimentJson, raw: &serde_json::Value) -> Self {
        let header = serde_json::from_value(raw.clone()).unwrap_or_default();

        // A node names the container it draws; index the other way round.
        let mut node_for_container: FxHashMap<Arc<str>, &crate::omiq::deserialise::GatingNode> =
            FxHashMap::default();
        for node in experiment.tree.nodes.values() {
            node_for_container.insert(node.filter_container_id.clone(), node);
        }

        let mut gates = FxHashMap::default();
        for (id, container) in &experiment.tree.filter_containers {
            let node = node_for_container.get(id);

            let (container_type, group_id, md, source_type, per_file_ids) = match container {
                FilterContainer::Atomic(atomic) => {
                    let mut files: Vec<FileId> = atomic.per_file_filters.keys().cloned().collect();
                    // Hash order is not stable; sort so exports are reproducible.
                    files.sort();
                    (
                        Arc::from("DEFAULT"),
                        atomic.group_id.clone(),
                        atomic.md.clone(),
                        OmiqGateType::of(&atomic.default_filter),
                        files,
                    )
                }
                FilterContainer::Compound(compound) => (
                    Arc::from(boolean_wire_name(&compound.operation)),
                    None,
                    None,
                    None,
                    Vec::new(),
                ),
            };

            gates.insert(
                id.clone(),
                OmiqRebuildData {
                    node_id: node.map(|n| n.id.clone()),
                    parent_node_id: node.map(|n| n.parent_id.clone()),
                    ord: node.map(|n| n.ord).unwrap_or(0),
                    collapsed: node.map(|n| n.collapsed).unwrap_or(false),
                    container_type,
                    group_id,
                    md,
                    source_type,
                    per_file_ids,
                },
            );
        }

        Self {
            header,
            gates,
            ghost_containers: FxHashMap::default(),
        }
    }

    /// Record the containers that no live gate reaches, keeping their raw JSON.
    pub fn capture_ghosts(
        &mut self,
        raw: &serde_json::Value,
        reachable: &rustc_hash::FxHashSet<Arc<str>>,
    ) {
        let Some(containers) = raw
            .get("tree")
            .and_then(|t| t.get("filterContainers"))
            .and_then(|c| c.as_object())
        else {
            return;
        };

        for (id, value) in containers {
            let id: Arc<str> = Arc::from(id.as_str());
            if !reachable.contains(&id) {
                self.ghost_containers.insert(id, value.clone());
            }
        }
    }
}

fn boolean_wire_name(op: &crate::omiq::deserialise::BooleanOpType) -> &'static str {
    match op {
        crate::omiq::deserialise::BooleanOpType::And => "AND",
        crate::omiq::deserialise::BooleanOpType::Or => "OR",
        crate::omiq::deserialise::BooleanOpType::Not => "NOT",
    }
}
