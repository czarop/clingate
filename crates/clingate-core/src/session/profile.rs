//! What a gate's populations look like across the dataset, and pictures of
//! it - for choosing a rule with Claude. See [`crate::gate_rules::profile`]
//! and [`crate::picture`]. Nothing here moves or writes anything.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::gate_rules::profile::{GateProfile, ProfileTarget, SampleReading};
use crate::gate_rules::rule_store::{RuleStore, RuleTarget};
use crate::gate_rules::run::RunInputs;
use crate::gates::gate_store::{GateId, NodeId};

/// Specimens a profile reads by default, and at most.
pub const PROFILE_SPECIMENS: usize = 12;
pub const PROFILE_SPECIMENS_MAX: usize = 60;
/// Tiles a picture has by default.
pub const PICTURE_TILES: usize = 6;

/// A profile, for the tools.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileAnswer {
    /// How many specimens were read, of how many.
    pub specimens_read: usize,
    pub specimens: usize,
    /// Each gate on each of its markers, in a line.
    pub lines: Vec<String>,
    /// In full, when one gate was asked for.
    pub profile: Option<GateProfile>,
    pub problems: Vec<String>,
    pub classes: &'static str,
    pub next: &'static str,
}

/// A picture of a gate, and what each tile is.
#[derive(Debug, Clone)]
pub struct Picture {
    pub png: Vec<u8>,
    pub tiles: Vec<String>,
}

const CLASSES: &str = "Each sample's parent population on a marker is classed: separate (a dip at \
    least half as deep as the lower peak beside it), shoulder (a shallower dip), smear (no dip, \
    the negative's right side at least 1.4 times its left, measured down to a quarter of the \
    peak), merged (no dip, and the right side never falls to a quarter of the peak before the \
    data ends), negative only (one even peak), several peaks (three or more), too few events \
    (under 200). negative R/L is the negative's right side against its left. 'gate N widths up' \
    is where the gate's lower side sits now, in right-side widths above the negative's peak. \
    'above the FMX's top' is, on each specimen's full stain, the fraction above its control's \
    99.5th percentile - the signal a band rule on the control has to find.";

impl Session {
    /// A population's gate as the rules name it: its name, and its parent's
    /// name made unique the way a rule's target reads it.
    pub(crate) fn target_of(
        &self,
        node: &NodeId,
    ) -> Option<(RuleTarget, GateId, (String, String))> {
        let gate_id = self.gates.gate_for_node(node)?.clone();
        let gate = self.gates.registered_gate(&gate_id)?;
        let names = crate::gates::gate_paths::unique_names(&self.gates);
        let parent = self
            .gates
            .parent_node(node)
            .and_then(|p| names.get(&p).cloned());
        let target = RuleTarget {
            gate: self
                .gates
                .population_name(&gate_id)
                .unwrap_or_else(|| Arc::from(gate.get_name())),
            parent,
        };
        let (x, y) = gate.get_params();
        Some((target, gate_id, (x.to_string(), y.to_string())))
    }

    /// The workspace's files, cut down to `wanted` specimens spread evenly
    /// through them - each with all its files - and any file `also` names.
    fn some_specimens(
        &self,
        rules: &RuleStore,
        wanted: usize,
        also: &[String],
    ) -> (RunInputs, usize, usize) {
        let mut inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            rules,
        );
        let column = &rules.pairing.sample_id_column;
        let specimen_of = |name: &Arc<str>| -> Option<String> {
            let id = inputs.names.get(name)?;
            inputs
                .metadata
                .get(id)
                .and_then(|row| row.get(column))
                .map(|s| s.to_string())
        };
        let mut specimens: Vec<String> = inputs
            .files
            .iter()
            .filter_map(|(name, _)| specimen_of(name))
            .collect();
        specimens.sort_by(|a, b| crate::gate_rules::rule_store::human_order(a, b));
        specimens.dedup();
        let total = specimens.len();
        let chosen: std::collections::HashSet<String> = if total <= wanted {
            specimens.into_iter().collect()
        } else {
            (0..wanted)
                .map(|i| specimens[i * total / wanted].clone())
                .collect()
        };
        let mut without = 0;
        let names = inputs.names.clone();
        inputs.files.retain(|(name, _)| {
            let id = names.get(name).map(|i| i.to_string());
            if id.as_ref().is_some_and(|i| also.contains(i)) {
                return true;
            }
            match specimen_of(name) {
                Some(s) => chosen.contains(&s),
                None => {
                    without += 1;
                    without <= wanted
                }
            }
        });
        (inputs, chosen.len().min(total), total)
    }

    /// Every population's gate, or those `query` names, profiled across up
    /// to `specimens` specimens spread through the dataset.
    pub fn gate_profile(
        &self,
        query: Option<&str>,
        specimens: Option<usize>,
    ) -> Result<ProfileAnswer, Refusal> {
        let rules = self.rules.clone().unwrap_or_default();
        let nodes: Vec<NodeId> = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => {
                let (nodes, facts) = self.population_facts();
                super::lookup::match_populations(q, &facts)?
                    .into_iter()
                    .map(|i| nodes[i].clone())
                    .collect()
            }
            None => self.population_facts().0,
        };
        let targets: Vec<ProfileTarget> = nodes
            .iter()
            .filter_map(|n| self.target_of(n))
            .map(|(target, _, markers)| ProfileTarget { target, markers })
            .collect();
        if targets.is_empty() {
            return Err(failed("no gate to profile"));
        }
        let references: Vec<String> = targets
            .iter()
            .filter_map(|t| rules.rule_for(&t.target.gate, t.target.parent.as_deref()))
            .filter_map(|r| match &r.measured_on {
                crate::gate_rules::rule_store::MeasuredOn::File(f) => Some(f.to_string()),
                _ => None,
            })
            .collect();
        let wanted = specimens
            .unwrap_or(PROFILE_SPECIMENS)
            .clamp(1, PROFILE_SPECIMENS_MAX);
        let (inputs, read, total) = self.some_specimens(&rules, wanted, &references);
        let (profiles, problems) = crate::gate_rules::profile::profile(
            &self.gates,
            &inputs,
            &targets,
            &AtomicBool::new(false),
        );
        let lines = profiles.iter().flat_map(|p| p.lines()).collect();
        let one = (profiles.len() == 1)
            .then(|| profiles.into_iter().next())
            .flatten();
        Ok(ProfileAnswer {
            specimens_read: read,
            specimens: total,
            lines,
            profile: one.map(|mut p| {
                for m in &mut p.markers {
                    for s in m.reference.iter_mut() {
                        s.file = self.sample_name(&Arc::from(s.file.as_str()));
                    }
                }
                p
            }),
            problems,
            classes: CLASSES,
            next: "gate_picture shows the gate on typical and extreme samples; rule_guide says \
                   which rule suits which shape; try_rules tries candidates on the data",
        })
    }

    /// The gate of `population` drawn on several samples, as one picture:
    /// those `samples` names, or - left out - the reference its rule reads,
    /// and for each sample type the samples where the gate holds least, most
    /// and most typically.
    pub fn gate_picture(
        &self,
        population: &str,
        samples: Option<&str>,
        tiles: Option<usize>,
    ) -> Result<Picture, Refusal> {
        let (node, _) = self.one_population(population)?;
        let (target, gate_id, (x, y)) = self
            .target_of(&node)
            .ok_or_else(|| failed("that population has no gate"))?;
        let parent = self.gates.parent_node(&node);
        let chain = parent
            .map(|p| self.gates.gate_chain_for_node(&p))
            .unwrap_or_default();
        let most = tiles
            .unwrap_or(PICTURE_TILES)
            .clamp(1, crate::picture::MOST_TILES);
        let ids = self.metadata.file_name_to_gating_id();

        // Which samples, as gating ids with a word on why each was chosen.
        let chosen: Vec<(Arc<str>, String)> = match samples.map(str::trim).filter(|s| !s.is_empty())
        {
            Some(query) => self
                .find_samples(query)?
                .samples
                .iter()
                .filter_map(|row| ids.get(row.name.as_str()).cloned())
                .take(most)
                .map(|id| (id, String::new()))
                .collect(),
            None => self.telling_samples(&target, &x, &y, most),
        };
        if chosen.is_empty() {
            return Err(failed("no sample to draw"));
        }
        let names: BTreeMap<&Arc<str>, &Arc<str>> = ids.iter().map(|(n, i)| (i, n)).collect();
        let cofactors = crate::gate_rules::run::RunInputs::cofactors_of(&self.axes.settings);
        let axis = |p: &str| self.axes.settings.get(p).cloned().unwrap_or_default();
        let metadata = self.metadata.metadata();
        let pairing = self.rules.clone().unwrap_or_default().pairing;
        let requests: Vec<(crate::picture::PlotRequest, String)> = chosen
            .iter()
            .filter_map(|(id, why)| {
                let name = names.get(id)?;
                let stub = self.files.file_list().iter().find(|s| *s.name == ***name)?;
                let groups = metadata.get(id).cloned().unwrap_or_default();
                let kind = metadata
                    .get(id)
                    .and_then(|row| pairing.sample_type_of(row))
                    .map(|t| format!(" ({t})"))
                    .unwrap_or_default();
                let caption = format!("{name}{kind}{why}");
                Some((
                    crate::picture::PlotRequest {
                        path: stub.filepath.clone(),
                        compensation: self.compensation.matrix_for(&stub.filepath),
                        cofactors: cofactors.clone(),
                        chain: chain.clone(),
                        resolver: self.gates.get_current_sample(id.clone(), &groups),
                        x: Arc::from(x.as_str()),
                        y: Arc::from(y.as_str()),
                        x_axis: axis(&x),
                        y_axis: axis(&y),
                        gate: self.gates.gate_for_file(&gate_id, id, metadata),
                    },
                    caption,
                ))
            })
            .collect();
        use rayon::prelude::*;
        let drawn: Vec<(Result<crate::picture::Drawn, String>, String)> = requests
            .par_iter()
            .map(|(r, caption)| {
                (
                    crate::picture::render(r).map_err(|e| e.to_string()),
                    caption.clone(),
                )
            })
            .collect();
        let mut images = Vec::new();
        let mut captions = Vec::new();
        for (result, caption) in drawn {
            match result {
                Ok(d) => {
                    captions.push(match d.holds {
                        Some(h) => format!(
                            "{caption}: the gate holds {:.3}% of {} events",
                            h * 100.0,
                            d.parent_events
                        ),
                        None => format!("{caption}: {} events", d.parent_events),
                    });
                    images.push(d.image);
                }
                Err(e) => captions.push(format!("{caption}: could not be drawn - {e}")),
            }
        }
        let columns = if images.len() <= 4 {
            images.len().min(2)
        } else {
            3
        };
        let png = crate::picture::grid(&images, columns.max(1)).map_err(failed)?;
        Ok(Picture {
            png,
            tiles: captions,
        })
    }

    /// The samples most worth seeing: the file the gate's rule reads, then for
    /// each sample type the ones where the gate holds least, most, and most
    /// typically - read from a profile of a spread of specimens.
    fn telling_samples(
        &self,
        target: &RuleTarget,
        x: &str,
        y: &str,
        most: usize,
    ) -> Vec<(Arc<str>, String)> {
        let rules = self.rules.clone().unwrap_or_default();
        let reference = rules
            .rule_for(&target.gate, target.parent.as_deref())
            .and_then(|r| match &r.measured_on {
                crate::gate_rules::rule_store::MeasuredOn::File(f) => Some(f.to_string()),
                _ => None,
            });
        let (inputs, _, _) =
            self.some_specimens(&rules, PROFILE_SPECIMENS_MAX, reference.as_slice());
        let (profiles, _) = crate::gate_rules::profile::profile(
            &self.gates,
            &inputs,
            &[ProfileTarget {
                target: target.clone(),
                markers: (x.to_string(), y.to_string()),
            }],
            &AtomicBool::new(false),
        );
        let readings: Vec<SampleReading> = profiles
            .into_iter()
            .flat_map(|p| p.markers)
            .flat_map(|m| m.samples)
            .collect();
        let mut out: Vec<(Arc<str>, String)> = Vec::new();
        let add = |file: &str, why: &str, out: &mut Vec<(Arc<str>, String)>| {
            if out.len() < most && !out.iter().any(|(f, _)| &**f == file) {
                out.push((Arc::from(file), why.to_string()));
            }
        };
        if let Some(r) = &reference {
            add(r, " - the reference", &mut out);
        }
        // The gated sample type first: the last in the pairing's order.
        let mut kinds: Vec<Option<String>> =
            readings.iter().map(|r| r.sample_type.clone()).collect();
        kinds.sort();
        kinds.dedup();
        let order = &rules.pairing.display_order;
        kinds.sort_by_key(|k| {
            std::cmp::Reverse(
                k.as_ref()
                    .and_then(|k| order.iter().position(|o| **o == **k))
                    .map_or(0, |p| p + 1),
            )
        });
        for kind in kinds {
            let mut of: Vec<&SampleReading> = readings
                .iter()
                .filter(|r| r.sample_type == kind && r.holds.is_some())
                .collect();
            if of.is_empty() {
                continue;
            }
            of.sort_by(|a, b| a.holds.unwrap().total_cmp(&b.holds.unwrap()));
            add(&of[of.len() / 2].file, " - typical", &mut out);
            add(&of[0].file, " - holds least", &mut out);
            add(&of[of.len() - 1].file, " - holds most", &mut out);
        }
        out
    }
}
