//! A workspace held whole, with no window: what a tool server, a script or a
//! test drives clingate through.
//!
//! The app keeps the files, metadata, scaling, gates, compensation and rules
//! in separate stores and joins them up in its tabs. A [`Session`] holds the
//! same things as plain values, loads them the way the Workspace tab does, and
//! answers questions about them by the names a person uses - samples by their
//! file name or metadata, populations by their gate path, parameters by marker
//! or channel (see [`lookup`]). Every answer is a plain, serialisable value.
//!
//! A name that does not match exactly is never guessed at: the answer is a
//! [`lookup::Clarification`] saying what did not match and what might have been
//! meant, for whoever asked to choose.
//!
//! Nothing here changes a file on disk. It reads events per question, as the
//! editor does per plot; nothing is kept between questions but the workspace
//! itself.

mod edits;
mod gates;
pub mod lookup;
mod replay;
mod review;
mod rules;

pub use edits::{EditState, Exported, Saved};
pub use gates::{CompareRow, Comparison, GateDetails, ParameterRow};
pub use replay::{
    CASES_MAX, CASES_SHOWN, CaseDetail, CaseLine, CasePopulation, ReplayAnswer, ReplayScope,
    RuleUpdated, describe_rule,
};
pub use review::{FLAGS_SHOWN, Reported, Reviewed, RunAssessment};
pub use rules::{
    RulesPreview, RulesView, TRIAL_ROWS, TRIAL_ROWS_MAX, TrialAnswer, TrialCandidate, TrialRow,
};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde::Serialize;

use crate::axis_store::{AxisStore, ScalingInfoSource, read_axis_configs};
use crate::compensation::groups::{Applied, Compensation};
use crate::file_load::{FcsFiles, FcsSampleStub};
use crate::gate_rules::rule_store::RuleStore;
use crate::gates::GateState;
use crate::gates::gate_store::NodeId;
use crate::omiq::metadata::MetaDataStore;
use crate::workspace::{Found, Remembered, detect, gating_needs};
use lookup::{Clarification, ParameterFacts, PopulationFacts, SampleFacts, SampleMatch};

/// The rules sidecar a session picks up from the folder, as the Gate Rules
/// tab saves it by default.
pub use crate::workspace::RULES_FILE;

/// Why a question could not be answered.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Refusal {
    /// A name matched nothing, or several things where one was needed: ask.
    NeedsClarification(Clarification),
    /// Something else went wrong, said in words.
    Failed { reason: String },
}

impl From<Clarification> for Refusal {
    fn from(c: Clarification) -> Self {
        Refusal::NeedsClarification(c)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NeedsClarification(c) => write!(f, "{} ({}): {}", c.about, c.asked, c.problem),
            Refusal::Failed { reason } => write!(f, "{reason}"),
        }
    }
}

fn failed(reason: impl std::fmt::Display) -> Refusal {
    Refusal::Failed {
        reason: reason.to_string(),
    }
}

/// A workspace, loaded.
pub struct Session {
    folder: PathBuf,
    files: FcsFiles,
    metadata: MetaDataStore,
    axes: AxisStore,
    gates: GateState,
    compensation: Compensation,
    rules: Option<RuleStore>,
    parts: Parts,
    warnings: Vec<String>,
    /// The placements a rules preview proposed, until they are applied or
    /// the gates change under them.
    pending: Option<rules::Pending>,
    /// The working copy's history and saved copy - the same type the app
    /// uses, so undo, save and the rest behave alike. See `working_copy`.
    working: crate::working_copy::WorkingCopy,
    /// Where reviewed runs are copied - see `crate::review::library`.
    review_library: Option<PathBuf>,
}

/// Which file each part of the workspace was read from.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Parts {
    /// The workspace file the app keeps in the folder. When it is there, the
    /// files, parts, compensation groups and sample grouping are the ones it
    /// names, as the app last left them; otherwise the folder is searched.
    pub workspace: PartState,
    pub metadata: PartState,
    pub scaling: PartState,
    pub gating: PartState,
    pub rules: PartState,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PartState {
    #[default]
    Missing,
    Loaded {
        file: PathBuf,
    },
    /// More than one file could be it, so none was read.
    Ambiguous {
        candidates: Vec<PathBuf>,
    },
    Failed {
        file: PathBuf,
        reason: String,
    },
    /// Found, but waiting for something else - a gating file for the metadata
    /// and scaling it is imported through.
    Waiting {
        file: PathBuf,
        needs: String,
    },
}

impl Session {
    /// Open the workspace in `folder` the way the Workspace tab does. Where
    /// the app has saved the workspace in the folder, as it was left: its FCS
    /// files, compensation groups and answers, parts and sample grouping.
    /// Otherwise found afresh: the FCS files anywhere under it, compensation
    /// grouped by each file's own matrix, then the metadata, the scaling, and
    /// the gating file through both. A part that cannot be read is recorded as such rather than
    /// failing the whole; only a folder that cannot be read at all is an error.
    pub fn open(folder: &Path) -> anyhow::Result<Self> {
        let mut parts = Parts::default();
        let saved_at = Remembered::in_folder(folder);
        let saved = match Remembered::load_from_folder(folder) {
            Ok(Some(saved)) => {
                parts.workspace = PartState::Loaded {
                    file: saved_at.clone(),
                };
                Some(saved)
            }
            Ok(None) => None,
            Err(e) => {
                parts.workspace = PartState::Failed {
                    file: saved_at.clone(),
                    reason: format!("{e}; the folder was searched instead"),
                };
                None
            }
        };
        let named = |path: &Option<PathBuf>| path.clone().map_or(Found::Missing, Found::One);
        let detected = match &saved {
            Some(saved) => crate::workspace::Detected {
                fcs: saved.fcs.clone(),
                metadata: named(&saved.metadata),
                scaling: named(&saved.scaling),
                gating: named(&saved.gating),
            },
            None => detect(folder)?,
        };
        let files = FcsFiles::open(Some(folder), &detected.fcs);
        let owns = crate::compensation::own_matrices(files.file_list());
        let compensation = match saved.as_ref().and_then(|s| s.compensation.as_ref()) {
            Some(groups) => Compensation::restore(groups, owns, |path| {
                crate::compensation::Spillover::read_omiq_csv(path).map_err(|e| e.to_string())
            }),
            None => {
                let mut fresh = Compensation::default();
                fresh.sync(owns);
                fresh
            }
        };

        let mut warnings = Vec::new();

        let mut metadata = MetaDataStore::default();
        parts.metadata = match detected.metadata {
            Found::One(file) => match MetaDataStore::read_omiq(file.clone()) {
                Ok((store, told)) => {
                    metadata = store;
                    warnings.extend(told);
                    PartState::Loaded { file }
                }
                Err(e) => PartState::Failed {
                    file,
                    reason: e.to_string(),
                },
            },
            Found::Several(candidates) => PartState::Ambiguous { candidates },
            Found::Missing => PartState::Missing,
        };

        let mut axes = AxisStore::default();
        parts.scaling = match detected.scaling {
            Found::One(file) => match read_axis_configs(file.clone(), ScalingInfoSource::Omiq) {
                Ok(configs) => {
                    axes.replace_axis_configs(configs);
                    PartState::Loaded { file }
                }
                Err(e) => PartState::Failed {
                    file,
                    reason: e.to_string(),
                },
            },
            Found::Several(candidates) => PartState::Ambiguous { candidates },
            Found::Missing => PartState::Missing,
        };

        let mut gates = GateState::default();
        parts.gating = match detected.gating {
            Found::One(file) => match gating_needs(metadata.metadata(), &axes.settings) {
                Some(needs) => PartState::Waiting {
                    file,
                    needs: needs.to_string(),
                },
                None => match GateState::from_gating_file(
                    file.clone(),
                    metadata.metadata(),
                    axes.settings.clone(),
                ) {
                    Ok(state) => {
                        gates = state;
                        PartState::Loaded { file }
                    }
                    Err(e) => PartState::Failed {
                        file,
                        reason: e.to_string(),
                    },
                },
            },
            Found::Several(candidates) => PartState::Ambiguous { candidates },
            Found::Missing => PartState::Missing,
        };

        let opened = crate::workspace::workspace_rules(
            folder,
            saved.as_ref().and_then(|s| s.pairing.clone()),
        );
        let mut rules = None;
        parts.rules = match opened.read {
            crate::workspace::RulesRead::Loaded => {
                rules = Some(opened.store);
                PartState::Loaded { file: opened.file }
            }
            crate::workspace::RulesRead::Failed(reason) => PartState::Failed {
                file: opened.file,
                reason,
            },
            crate::workspace::RulesRead::Missing => PartState::Missing,
        };

        let mut session = Self {
            folder: folder.to_path_buf(),
            files,
            metadata,
            axes,
            gates,
            compensation,
            rules,
            parts,
            warnings,
            pending: None,
            working: crate::working_copy::WorkingCopy::default(),
            review_library: crate::review::library::configured(),
        };
        // As the app does when its gating file loads: both copies.
        if matches!(session.parts.gating, PartState::Loaded { .. }) {
            let now = session.working_state();
            session.working.loaded(&now);
        }
        Ok(session)
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn gates(&self) -> &GateState {
        &self.gates
    }

    pub fn axes(&self) -> &AxisStore {
        &self.axes
    }

    pub fn metadata(&self) -> &MetaDataStore {
        &self.metadata
    }

    pub fn rules(&self) -> Option<&RuleStore> {
        self.rules.as_ref()
    }

    // ─── What is here ─────────────────────────────────────────────────────

    /// The workspace at a glance: what was loaded, what could not be, the
    /// metadata columns and their values, and what compensation still needs
    /// to be told.
    pub fn overview(&self) -> Overview {
        let mut columns: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for sample in self.sample_facts() {
            for (column, value) in sample.metadata {
                columns.entry(column).or_default().insert(value);
            }
        }
        let without_metadata: Vec<String> = self
            .files
            .file_list()
            .iter()
            .filter(|s| self.gating_id(s).is_none())
            .map(|s| s.name.to_string())
            .collect();
        Overview {
            folder: self.folder.clone(),
            samples: self.files.sample_count(),
            unreadable_files: self
                .files
                .unread()
                .iter()
                .map(|u| format!("{}: {}", u.path.display(), u.reason))
                .collect(),
            samples_without_metadata: without_metadata,
            parts: self.parts.clone(),
            populations: self.gates.placements().count(),
            parameters: self.axes.sorted_settings.len(),
            metadata_columns: columns
                .into_iter()
                .map(|(column, values)| MetadataColumn {
                    distinct_values: values.len(),
                    values: values.into_iter().take(12).collect(),
                    column,
                })
                .collect(),
            compensation: self.compensation_groups(),
            edits: self.edit_state(),
            warnings: self.warnings.clone(),
        }
    }

    fn compensation_groups(&self) -> Vec<CompensationGroup> {
        self.compensation
            .groups()
            .iter()
            .map(|g| CompensationGroup {
                name: g.name.clone(),
                files: self.compensation.files_in(g.id).count(),
                compensated_with: g.source.describe(),
                needs_answer: self.compensation.unanswered(g.id).then(|| {
                    "these files were exported from Omiq, which applies its compensation to the \
                     events without recording it: ask whether compensation was applied in Omiq, \
                     and if so for the matrix"
                        .to_string()
                }),
            })
            .collect()
    }

    /// The samples `query` names - see [`lookup::match_samples`] - with their
    /// metadata.
    pub fn find_samples(&self, query: &str) -> Result<Samples, Refusal> {
        let facts = self.sample_facts();
        let matched = lookup::match_samples(query, &facts)?;
        Ok(Samples {
            samples: matched
                .indices
                .iter()
                .map(|&i| SampleRow {
                    name: facts[i].name.clone(),
                    metadata: facts[i].metadata.iter().cloned().collect(),
                })
                .collect(),
            matched_by: matched,
        })
    }

    /// Every population, or those `query` names - see
    /// [`lookup::match_populations`].
    pub fn populations(&self, query: Option<&str>) -> Result<Vec<PopulationRow>, Refusal> {
        let (nodes, facts) = self.population_facts();
        let chosen: Vec<usize> = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => lookup::match_populations(q, &facts)?,
            None => (0..nodes.len()).collect(),
        };
        Ok(chosen
            .into_iter()
            .map(|i| {
                let gate = self
                    .gates
                    .gate_for_node(&nodes[i])
                    .and_then(|id| self.gates.registered_gate(id));
                PopulationRow {
                    name: facts[i].label.clone(),
                    path: facts[i].full_path(),
                    parameters: gate
                        .map(|g| {
                            let (x, y) = g.get_params();
                            vec![x.to_string(), y.to_string()]
                        })
                        .unwrap_or_default(),
                }
            })
            .collect())
    }

    // ─── Numbers ──────────────────────────────────────────────────────────

    /// How many events each named sample has in `population`, and what share
    /// of its parent that is.
    pub fn population_stats(
        &self,
        population: &str,
        samples: &str,
    ) -> Result<PopulationStats, Refusal> {
        let (node, facts) = self.one_population(population)?;
        let chosen = self.find_samples(samples)?;
        let chain = self.gates.gate_chain_for_node(&node);
        if chain.is_empty() {
            return Err(failed("that population has no gates above it to count"));
        }
        let stubs = self.stubs_named(&chosen);

        let mut rows: Vec<StatsRow> = stubs
            .par_iter()
            .map(|stub| match self.count(stub, &chain) {
                Ok((total, parent, events)) => StatsRow {
                    sample: stub.name.to_string(),
                    total_events: Some(total),
                    parent_events: Some(parent),
                    events: Some(events),
                    percent_of_parent: percent(events, parent),
                    percent_of_total: percent(events, total),
                    problem: None,
                },
                Err(why) => StatsRow {
                    sample: stub.name.to_string(),
                    problem: Some(why),
                    ..StatsRow::default()
                },
            })
            .collect();
        rows.sort_by(|a, b| a.sample.cmp(&b.sample));

        let mut shares: Vec<f64> = rows.iter().filter_map(|r| r.percent_of_parent).collect();
        shares.sort_by(f64::total_cmp);
        Ok(PopulationStats {
            population: facts.label.clone(),
            population_path: facts.full_path(),
            samples_matched_by: chosen.matched_by.matched_by,
            summary: (!shares.is_empty()).then(|| Summary {
                samples: shares.len(),
                min_percent_of_parent: shares[0],
                median_percent_of_parent: quantile(&shares, 0.5),
                max_percent_of_parent: shares[shares.len() - 1],
            }),
            rows,
        })
    }

    /// How `population`'s parent is spread on `parameter` in one sample, and
    /// where the population's gate sits on it - in the units the plots are
    /// drawn in (arcsinh-scaled where the scaling says so).
    pub fn distribution(
        &self,
        population: &str,
        sample: &str,
        parameter: &str,
    ) -> Result<Distribution, Refusal> {
        let (node, facts) = self.one_population(population)?;
        let stub = self.one_sample(sample)?;
        let param = self.one_parameter(parameter)?;
        let (values, gate) = self.parent_on(stub, &node, &param).map_err(failed)?;
        Ok(Distribution {
            sample: stub.name.to_string(),
            population: facts.label.clone(),
            population_path: facts.full_path(),
            of: "the population's parent".to_string(),
            parameter: describe_param(&param),
            scale: self.scale_of(&param),
            events: values.len(),
            percentiles: Percentiles::of(&values),
            histogram: Histogram::of(&values, 24),
            gate_on_this_parameter: gate,
        })
    }

    /// The one sample `query` names, or a question listing those it matched.
    pub(crate) fn one_sample(&self, query: &str) -> Result<&FcsSampleStub, Refusal> {
        let chosen = self.find_samples(query)?;
        let stubs = self.stubs_named(&chosen);
        match stubs.as_slice() {
            [one] => Ok(*one),
            several => Err(Refusal::NeedsClarification(Clarification {
                about: "samples",
                asked: query.to_string(),
                problem: format!("{} samples match; this needs exactly one", several.len()),
                suggestions: several
                    .iter()
                    .map(|s| s.name.to_string())
                    .take(20)
                    .collect(),
            })),
        }
    }

    /// The one parameter `query` names.
    pub(crate) fn one_parameter(&self, query: &str) -> Result<crate::axis_store::Param, Refusal> {
        let (params, facts) = self.parameter_facts();
        let which = lookup::one_parameter(query, &facts)?;
        Ok(params[which].clone())
    }

    /// How a parameter is drawn: "linear", or "arcsinh, cofactor 150".
    pub(crate) fn scale_of(&self, param: &crate::axis_store::Param) -> String {
        match self
            .axes
            .settings
            .get(&param.fluoro)
            .map(|a| a.transform.clone())
        {
            Some(flow_fcs::TransformType::Arcsinh { cofactor }) => {
                format!("arcsinh, cofactor {cofactor}")
            }
            Some(flow_fcs::TransformType::Linear) | None => "linear".to_string(),
            Some(other) => format!("{other:?}"),
        }
    }

    /// The values, sorted, of `node`'s parent population on `param` in one
    /// sample, and where `node`'s own gate sits on that parameter there.
    pub(crate) fn parent_on(
        &self,
        stub: &FcsSampleStub,
        node: &NodeId,
        param: &crate::axis_store::Param,
    ) -> Result<(Vec<f64>, Option<GateEdges>), String> {
        let chain = self.gates.gate_chain_for_node(node);
        let Some((own, above)) = chain.split_last() else {
            return Err("that population has no gate".to_string());
        };
        let (frame, resolver) = self.read(stub).map_err(|e| e.to_string())?;
        let parent =
            crate::events::under_chain(&frame, above, &resolver).map_err(|e| e.to_string())?;
        let mut values: Vec<f64> = parent
            .column(&param.fluoro)
            .and_then(|c| {
                c.f32()
                    .map(|v| v.into_no_null_iter().map(f64::from).collect())
            })
            .map_err(|e| format!("{}: {e}", param.fluoro))?;
        values.retain(|v| v.is_finite());
        values.sort_by(f64::total_cmp);

        let gate = resolver
            .resolve_drawable(own)
            .ok()
            .and_then(|g| {
                g.get_gate_ref(None).and_then(|g| {
                    crate::gate_rules::autogate::extent_on(&g.geometry, &param.fluoro)
                })
            })
            .map(|(lower, upper)| GateEdges {
                lower: finite(lower),
                upper: finite(upper),
                parent_inside_fraction: if values.is_empty() {
                    None
                } else {
                    Some(round(
                        values
                            .iter()
                            .filter(|v| **v >= lower as f64 && **v <= upper as f64)
                            .count() as f64
                            / values.len() as f64,
                        4,
                    ))
                },
            });
        Ok((values, gate))
    }

    // ─── Compensation ─────────────────────────────────────────────────────

    /// Say what Omiq applied to a compensation group's exports: nothing, or
    /// the matrix pasted from Omiq (checked against the group's files).
    pub fn answer_omiq(&mut self, group: &str, matrix: Option<&str>) -> Result<String, Refusal> {
        let wanted = group.trim().to_lowercase();
        let found: Vec<_> = self
            .compensation
            .groups()
            .iter()
            .filter(|g| g.name.to_lowercase() == wanted)
            .map(|g| g.id)
            .collect();
        let [id] = found.as_slice() else {
            return Err(Refusal::NeedsClarification(Clarification {
                about: "compensation group",
                asked: group.to_string(),
                problem: "no compensation group has this name".to_string(),
                suggestions: self
                    .compensation
                    .groups()
                    .iter()
                    .map(|g| g.name.clone())
                    .collect(),
            }));
        };
        let applied = match matrix {
            None => Applied::Nothing,
            Some(text) => {
                let stubs = self.files.file_list();
                let read = self.compensation.read_pasted(*id, text, |path| {
                    stubs
                        .iter()
                        .find(|s| s.get_filepath() == path)
                        .map(crate::compensation::facts_of)
                        .unwrap_or_default()
                });
                Applied::Matrix {
                    path: None,
                    matrix: Arc::new(read.map_err(failed)?),
                }
            }
        };
        self.compensation
            .set_applied(*id, applied)
            .map_err(failed)?;
        let group = self
            .compensation
            .group(*id)
            .map(|g| format!("{} is now compensated with {}", g.name, g.source.describe()))
            .unwrap_or_default();
        Ok(group)
    }

    // ─── Plumbing ─────────────────────────────────────────────────────────

    fn gating_id(&self, stub: &FcsSampleStub) -> Option<Arc<str>> {
        self.metadata
            .file_name_to_gating_id()
            .get(&stub.name)
            .cloned()
    }

    fn sample_facts(&self) -> Vec<SampleFacts> {
        self.files
            .file_list()
            .iter()
            .map(|stub| {
                let mut metadata: Vec<(String, String)> = self
                    .gating_id(stub)
                    .and_then(|id| self.metadata.metadata().get(&id))
                    .map(|columns| {
                        columns
                            .iter()
                            .map(|(c, v)| (c.to_string(), v.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                metadata.sort();
                SampleFacts {
                    name: stub.name.to_string(),
                    metadata,
                }
            })
            .collect()
    }

    fn stubs_named(&self, samples: &Samples) -> Vec<&FcsSampleStub> {
        samples
            .samples
            .iter()
            .filter_map(|row| self.files.file_list().iter().find(|s| *s.name == *row.name))
            .collect()
    }

    /// Every node with its path of gate names, root first, and its label.
    fn population_facts(&self) -> (Vec<NodeId>, Vec<PopulationFacts>) {
        let mut nodes: Vec<(NodeId, Vec<String>)> = self
            .gates
            .placements()
            .map(|(node, _)| {
                let path: Vec<String> = self
                    .gates
                    .gate_chain_for_node(node)
                    .iter()
                    .filter_map(|id| self.gates.registered_gate(id))
                    .map(|g| g.get_name().to_string())
                    .collect();
                (node.clone(), path)
            })
            .filter(|(_, path)| !path.is_empty())
            .collect();
        nodes.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.as_str().cmp(b.0.as_str())));
        let (nodes, mut facts): (Vec<NodeId>, Vec<PopulationFacts>) = nodes
            .into_iter()
            .map(|(node, path)| (node, PopulationFacts::new(path)))
            .unzip();
        lookup::label_populations(&mut facts, |i| format!("id:{}", nodes[i].as_str()));
        (nodes, facts)
    }

    /// The one population `query` names. `id:` and a node id picks one that
    /// cannot be told from another by name.
    fn one_population(&self, query: &str) -> Result<(NodeId, PopulationFacts), Refusal> {
        let (nodes, facts) = self.population_facts();
        if let Some(id) = query.trim().strip_prefix("id:") {
            return nodes
                .iter()
                .position(|n| n.as_str() == id.trim())
                .map(|i| (nodes[i].clone(), facts[i].clone()))
                .ok_or_else(|| failed(format!("there is no population with id {id}")));
        }
        let at = lookup::one_population(query, &facts)?;
        Ok((nodes[at].clone(), facts[at].clone()))
    }

    fn parameter_facts(&self) -> (Vec<crate::axis_store::Param>, Vec<ParameterFacts>) {
        let params: Vec<crate::axis_store::Param> =
            self.axes.sorted_settings.iter().cloned().collect();
        let facts = params
            .iter()
            .map(|p| ParameterFacts {
                marker: p.marker.to_string(),
                channel: p.fluoro.to_string(),
            })
            .collect();
        (params, facts)
    }

    /// A sample's events as the plots read them, and the gates as they apply
    /// to it.
    fn read(
        &self,
        stub: &FcsSampleStub,
    ) -> anyhow::Result<(
        polars::prelude::DataFrame,
        crate::gates::gate_store::GateOverrideResolver,
    )> {
        let id = self
            .gating_id(stub)
            .ok_or_else(|| anyhow::anyhow!("it has no row in the metadata"))?;
        let groups = self
            .metadata
            .metadata()
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let resolver = self.gates.get_current_sample(id, &groups);
        let path = stub.get_filepath();
        let cofactors = crate::gate_rules::run::RunInputs::cofactors_of(&self.axes.settings);
        let frame =
            crate::events::read_scaled(path, &self.compensation.matrix_for(path), &cofactors)?;
        Ok((frame, resolver))
    }

    /// Events in the whole file, under the parent, and in the population.
    fn count(
        &self,
        stub: &FcsSampleStub,
        chain: &[crate::gates::GateId],
    ) -> Result<(usize, usize, usize), String> {
        let (frame, resolver) = self.read(stub).map_err(|e| e.to_string())?;
        let total = frame.height();
        let (_, above) = chain.split_last().expect("checked not empty");
        let parent =
            crate::events::under_chain(&frame, above, &resolver).map_err(|e| e.to_string())?;
        let events = crate::events::under_chain(&frame, chain, &resolver)
            .map_err(|e| e.to_string())?
            .height();
        Ok((total, parent.height(), events))
    }
}

/// "CD4 (BUV395-A)", or the channel alone where it has no marker.
fn describe_param(param: &crate::axis_store::Param) -> String {
    if param.marker == param.fluoro {
        param.fluoro.to_string()
    } else {
        format!("{} ({})", param.marker, param.fluoro)
    }
}

fn percent(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| round(100.0 * part as f64 / whole as f64, 3))
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

fn finite(value: f32) -> Option<f64> {
    value.is_finite().then(|| round(value as f64, 4))
}

/// The value at `q` of sorted `values`, nearest rank.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let at = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[at.min(sorted.len() - 1)]
}

// ─── Answers ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub folder: PathBuf,
    pub samples: usize,
    pub unreadable_files: Vec<String>,
    /// Samples with no metadata row: they cannot be gated or counted.
    pub samples_without_metadata: Vec<String>,
    pub parts: Parts,
    pub populations: usize,
    pub parameters: usize,
    pub metadata_columns: Vec<MetadataColumn>,
    pub compensation: Vec<CompensationGroup>,
    /// Whether the working copy has unsaved changes, what can be undone,
    /// and whether an earlier session left unsaved changes behind.
    pub edits: EditState,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetadataColumn {
    pub column: String,
    pub distinct_values: usize,
    /// Up to twelve of them.
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompensationGroup {
    pub name: String,
    pub files: usize,
    pub compensated_with: String,
    /// What has to be asked before these files can be read, if anything.
    pub needs_answer: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Samples {
    pub samples: Vec<SampleRow>,
    pub matched_by: SampleMatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct SampleRow {
    pub name: String,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationRow {
    /// As short as it can be while naming only this population - what to
    /// ask for it by.
    pub name: String,
    /// Every gate from the root down to it.
    pub path: String,
    /// The two parameters its gate is drawn on.
    pub parameters: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationStats {
    pub population: String,
    pub population_path: String,
    pub samples_matched_by: Vec<lookup::MatchedWord>,
    pub summary: Option<Summary>,
    pub rows: Vec<StatsRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub samples: usize,
    pub min_percent_of_parent: f64,
    pub median_percent_of_parent: f64,
    pub max_percent_of_parent: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StatsRow {
    pub sample: String,
    pub total_events: Option<usize>,
    pub parent_events: Option<usize>,
    pub events: Option<usize>,
    pub percent_of_parent: Option<f64>,
    pub percent_of_total: Option<f64>,
    /// Why this sample could not be counted.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Distribution {
    pub sample: String,
    pub population: String,
    pub population_path: String,
    /// Which events are described.
    pub of: String,
    pub parameter: String,
    pub scale: String,
    pub events: usize,
    pub percentiles: Option<Percentiles>,
    pub histogram: Option<Histogram>,
    /// The population's gate edges on this parameter, if its gate is drawn
    /// on it, and the share of the parent between them.
    pub gate_on_this_parameter: Option<GateEdges>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateEdges {
    /// `None` for an edge open to that side.
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub parent_inside_fraction: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Percentiles {
    pub p1: f64,
    pub p5: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p95: f64,
    pub p99: f64,
}

impl Percentiles {
    fn of(sorted: &[f64]) -> Option<Self> {
        if sorted.is_empty() {
            return None;
        }
        let q = |p: f64| round(quantile(sorted, p), 4);
        Some(Self {
            p1: q(0.01),
            p5: q(0.05),
            p25: q(0.25),
            p50: q(0.5),
            p75: q(0.75),
            p95: q(0.95),
            p99: q(0.99),
        })
    }
}

/// Counts in equal bins between the 0.5th and 99.5th percentiles, so a few
/// outliers do not squash the shape into one bin.
#[derive(Debug, Clone, Serialize)]
pub struct Histogram {
    pub from: f64,
    pub to: f64,
    pub bin_width: f64,
    pub counts: Vec<usize>,
    /// Events outside the range, below and above.
    pub below: usize,
    pub above: usize,
}

impl Histogram {
    fn of(sorted: &[f64], bins: usize) -> Option<Self> {
        if sorted.len() < 2 {
            return None;
        }
        let from = quantile(sorted, 0.005);
        let to = quantile(sorted, 0.995);
        if to <= from {
            return None;
        }
        let width = (to - from) / bins as f64;
        let mut counts = vec![0usize; bins];
        let (mut below, mut above) = (0, 0);
        for v in sorted {
            if *v < from {
                below += 1;
            } else if *v > to {
                above += 1;
            } else {
                let bin = (((v - from) / width) as usize).min(bins - 1);
                counts[bin] += 1;
            }
        }
        Some(Self {
            from: round(from, 4),
            to: round(to, 4),
            bin_width: round(width, 4),
            counts,
            below,
            above,
        })
    }
}
