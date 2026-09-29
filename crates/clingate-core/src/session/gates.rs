//! What a session can say about the parameters, one gate, and how a
//! population's parent compares across samples.

use rayon::prelude::*;
use serde::Serialize;

use super::{GateEdges, Refusal, Session, describe_param, failed, finite, quantile, round};
use crate::gates::gate_store::GateSource;

#[derive(Debug, Clone, Serialize)]
pub struct ParameterRow {
    pub marker: String,
    pub channel: String,
    pub scale: String,
    /// The axis range, in the units the plots are drawn in.
    pub axis_from: f64,
    pub axis_to: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateDetails {
    pub population: String,
    pub population_path: String,
    /// The sample the position is for, if one was asked about.
    pub sample: Option<String>,
    /// Where this position comes from: the gate as drawn, a position set for
    /// a group of samples (which metadata column and value), or one set for
    /// this sample alone.
    pub position: String,
    /// The two parameters it is drawn on.
    pub parameters: Vec<String>,
    /// Its extent on each of them - `None` for an edge open to that side.
    pub extent: Vec<Extent>,
    /// How a rule names it - see `PopulationRow::rule_target`.
    pub rule_target: String,
    /// The other places this same gate is drawn, when it is linked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linked_with: Vec<String>,
    /// The shape itself, in the units the plots are drawn in.
    pub geometry: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Extent {
    pub parameter: String,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub population: String,
    pub population_path: String,
    pub of: String,
    pub parameter: String,
    pub scale: String,
    /// The median across samples of each sample's median.
    pub plate_median: Option<f64>,
    /// The median across samples of each sample's interquartile range - the
    /// unit `shift_in_iqrs` is measured in.
    pub typical_iqr: Option<f64>,
    pub rows: Vec<CompareRow>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CompareRow {
    pub sample: String,
    pub events: Option<usize>,
    pub p5: Option<f64>,
    pub p25: Option<f64>,
    pub median: Option<f64>,
    pub p75: Option<f64>,
    pub p95: Option<f64>,
    /// How far this sample's median is from the plate's, in typical
    /// interquartile ranges. Large either way is a sample distributed unlike
    /// the others on this parameter.
    pub shift_in_iqrs: Option<f64>,
    /// This sample's interquartile range over the typical one: well above 1
    /// is a broader population than the others'.
    pub spread_ratio: Option<f64>,
    /// The population's gate on this parameter in this sample, and the share
    /// of the parent between its edges.
    pub gate: Option<GateEdges>,
    pub problem: Option<String>,
}

impl Session {
    /// Every parameter the scaling describes, or the one `query` names.
    pub fn parameters(&self, query: Option<&str>) -> Result<Vec<ParameterRow>, Refusal> {
        let (params, _) = self.parameter_facts();
        let chosen = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => vec![self.one_parameter(q)?],
            None => params,
        };
        Ok(chosen
            .iter()
            .map(|p| {
                let axis = self.axes.settings.get(&p.fluoro);
                ParameterRow {
                    marker: p.marker.to_string(),
                    channel: p.fluoro.to_string(),
                    scale: self.scale_of(p),
                    axis_from: axis.map(|a| round(a.axis_lower as f64, 4)).unwrap_or(0.0),
                    axis_to: axis.map(|a| round(a.axis_upper as f64, 4)).unwrap_or(0.0),
                }
            })
            .collect())
    }

    /// A population's gate: the shape as drawn, or - for one sample - the
    /// position that applies to it and where that position comes from.
    pub fn gate(&self, population: &str, sample: Option<&str>) -> Result<GateDetails, Refusal> {
        let (node, facts) = self.one_population(population)?;
        let gate_id = self
            .gates
            .gate_for_node(&node)
            .cloned()
            .ok_or_else(|| failed("that population has no gate"))?;
        let sample = sample.map(str::trim).filter(|s| !s.is_empty());
        let (sample_name, source, gate) = match sample {
            None => (
                None,
                GateSource::Global,
                self.gates
                    .registered_gate(&gate_id)
                    .ok_or_else(|| failed("that population's gate is not registered"))?,
            ),
            Some(query) => {
                let stub = self.one_sample(query)?;
                let file = self
                    .gating_id(stub)
                    .ok_or_else(|| failed(format!("{} has no row in the metadata", stub.name)))?;
                let (source, gate) = self
                    .gates
                    .gate_and_source_for_file(&gate_id, &file, self.metadata.metadata())
                    .ok_or_else(|| failed("that population's gate is not registered"))?;
                (Some(stub.name.to_string()), source, gate)
            }
        };
        let position = match &source {
            GateSource::Global => "as drawn - not positioned for any sample or group".to_string(),
            GateSource::Group((_, key)) => {
                format!("positioned for the group {}={}", key.parameter, key.group)
            }
            GateSource::Sample(_) => "positioned for this sample alone".to_string(),
        };
        let (rule_target, linked_with) = self.rule_names_of(&node);
        let (x, y) = gate.get_params();
        let geometry = gate.get_gate_ref(None).map(|g| g.geometry.clone());
        let extent = [x.clone(), y.clone()]
            .iter()
            .map(|p| {
                let found = geometry
                    .as_ref()
                    .and_then(|g| crate::gate_rules::autogate::extent_on(g, p));
                Extent {
                    parameter: p.to_string(),
                    lower: found.and_then(|(lo, _)| finite(lo)),
                    upper: found.and_then(|(_, hi)| finite(hi)),
                }
            })
            .collect();
        Ok(GateDetails {
            population: facts.label.clone(),
            population_path: facts.full_path(),
            sample: sample_name,
            position,
            parameters: vec![x.to_string(), y.to_string()],
            extent,
            geometry: geometry.and_then(|g| serde_json::to_value(g).ok()),
            rule_target,
            linked_with,
        })
    }

    /// How `population`'s parent is spread on `parameter` in each sample
    /// named, side by side, with each sample's distance from the rest.
    pub fn compare_samples(
        &self,
        population: &str,
        parameter: &str,
        samples: &str,
    ) -> Result<Comparison, Refusal> {
        let (node, facts) = self.one_population(population)?;
        let param = self.one_parameter(parameter)?;
        let chosen = self.find_samples(samples)?;
        let stubs = self.stubs_named(&chosen);

        let mut rows: Vec<CompareRow> = stubs
            .par_iter()
            .map(|stub| match self.parent_on(stub, &node, &param) {
                Ok((values, gate)) if !values.is_empty() => CompareRow {
                    sample: stub.name.to_string(),
                    events: Some(values.len()),
                    p5: Some(round(quantile(&values, 0.05), 4)),
                    p25: Some(round(quantile(&values, 0.25), 4)),
                    median: Some(round(quantile(&values, 0.5), 4)),
                    p75: Some(round(quantile(&values, 0.75), 4)),
                    p95: Some(round(quantile(&values, 0.95), 4)),
                    gate,
                    ..CompareRow::default()
                },
                Ok((_, gate)) => CompareRow {
                    sample: stub.name.to_string(),
                    events: Some(0),
                    gate,
                    ..CompareRow::default()
                },
                Err(why) => CompareRow {
                    sample: stub.name.to_string(),
                    problem: Some(why),
                    ..CompareRow::default()
                },
            })
            .collect();
        rows.sort_by(|a, b| a.sample.cmp(&b.sample));

        let sorted = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v
        };
        let medians = sorted(rows.iter().filter_map(|r| r.median).collect());
        let iqrs = sorted(rows.iter().filter_map(|r| Some(r.p75? - r.p25?)).collect());
        let plate_median = (!medians.is_empty()).then(|| quantile(&medians, 0.5));
        let typical_iqr = (!iqrs.is_empty()).then(|| quantile(&iqrs, 0.5));
        if let (Some(centre), Some(unit)) = (plate_median, typical_iqr)
            && unit > 0.0
        {
            for row in &mut rows {
                row.shift_in_iqrs = row.median.map(|m| round((m - centre) / unit, 2));
                row.spread_ratio = row
                    .p75
                    .zip(row.p25)
                    .map(|(hi, lo)| round((hi - lo) / unit, 2));
            }
        }
        Ok(Comparison {
            population: facts.label.clone(),
            population_path: facts.full_path(),
            of: "the population's parent".to_string(),
            parameter: describe_param(&param),
            scale: self.scale_of(&param),
            plate_median: plate_median.map(|m| round(m, 4)),
            typical_iqr: typical_iqr.map(|u| round(u, 4)),
            rows,
        })
    }
}
