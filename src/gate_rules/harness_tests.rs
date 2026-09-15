//! Scoring the rules against a real, hand-gated workflow.
//!
//! The constants in the confidence model and the bands a rule should carry are
//! not things that can be reasoned out - they have to be measured against gates
//! a person actually placed. This runs the whole stack over a real export and
//! prints what it finds, so the numbers can be read rather than guessed at.
//!
//! It skips silently unless three paths are given, because the files it needs
//! carry sample identifiers and a workflow url and must never be committed:
//!
//! ```text
//! OMIQ_GATING_FILE=/path/to/export.omiqgt \
//! OMIQ_METADATA_FILE=/path/to/metadata.csv \
//! OMIQ_SCALING_FILE=/path/to/scaling.csv \
//! OMIQ_FCS_DIR=/path/to/fcs \
//! cargo test --lib --no-default-features a_real_ -- --nocapture
//! ```
//!
//! The FCS files are expected to be **already compensated and scaled** - Omiq
//! can export them that way, which puts them in the same space as the gating
//! file's own coordinates. Nothing here transforms them.

#![cfg(test)]

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;
use crate::gate_editor::gates::gate_store::{GateId, NodeId};
use crate::gate_editor::plots::axis_store::{ScalingInfoSource, read_axis_configs};
use crate::gate_rules::confidence::{ConfidenceModel, CountAndSeparation};
use crate::gate_rules::rule::{PositioningRule, TailFractionRule};
use crate::gate_rules::threshold::interquartile_spread;
use crate::omiq::metadata::{MetaDataOrigin, parse_metadata_csv};
use flow_fcs::Fcs;
use flow_gates::GateGeometry;
use polars::prelude::*;
use rustc_hash::FxBuildHasher;
use std::path::PathBuf;
use std::sync::Arc;

fn path_from(var: &str) -> Option<PathBuf> {
    std::env::var(var).ok().map(PathBuf::from).filter(|p| {
        let ok = p.exists();
        if !ok {
            println!("{var} points at {p:?}, which does not exist - skipping");
        }
        ok
    })
}

/// Which edge of a rectangle actually discriminates on x.
///
/// A "positive" gate bounds x from below and runs off the top of the plot; a
/// "negative" one bounds it from above. The unused edge is dragged off-scale,
/// so it is not a threshold at all - reading it as one reports that the gate
/// captures the whole parent population, which is how this was found.
#[derive(PartialEq, Clone, Copy)]
enum Edge {
    Lower,
    Upper,
    /// Both edges are inside the data - a window rather than a threshold.
    Window,
    /// Neither is: the gate does not discriminate on x at all, and whatever it
    /// selects, it selects on the other axis.
    Neither,
}

impl Edge {
    fn label(self) -> &'static str {
        match self {
            Edge::Lower => "lower",
            Edge::Upper => "upper",
            Edge::Window => "window",
            Edge::Neither => "-",
        }
    }
}

/// What one gate looks like on one sample.
struct Row {
    sample: String,
    gate: String,
    channel: String,
    parent_events: usize,
    edge: Edge,
    values: Vec<f64>,
    manual_x: f64,
    manual_fraction: f64,
    spread: f64,
    y_full_span: bool,
}

#[test]
fn a_real_workflow_shows_what_the_manual_gates_capture() {
    let (Some(gating), Some(metadata_path), Some(scaling), Some(fcs_dir)) = (
        path_from("OMIQ_GATING_FILE"),
        path_from("OMIQ_METADATA_FILE"),
        path_from("OMIQ_SCALING_FILE"),
        path_from("OMIQ_FCS_DIR"),
    ) else {
        println!(
            "real-file harness skipped: set OMIQ_GATING_FILE, OMIQ_METADATA_FILE, OMIQ_SCALING_FILE and OMIQ_FCS_DIR"
        );
        return;
    };

    let parsed = parse_metadata_csv(metadata_path, "OmiqID", "Filename", MetaDataOrigin::Omiq)
        .expect("the metadata csv should parse");

    let mut axes: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher> =
        im::HashMap::with_hasher(FxBuildHasher);
    for config in read_axis_configs(scaling, ScalingInfoSource::Omiq).expect("scaling csv") {
        axes.insert(config.param.fluoro.clone(), config);
    }

    let mut state = GateState::default();
    state
        .upload_gates_from_file(gating, &parsed.metadata, axes)
        .expect("the gating file should import");
    println!(
        "imported {} gates over {} positions",
        state.gate_count(),
        state.placements().count()
    );

    let mut files: Vec<PathBuf> = std::fs::read_dir(&fcs_dir)
        .expect("fcs dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fcs")))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .fcs files in {fcs_dir:?}");

    let mut rows: Vec<Row> = Vec::new();
    for file in &files {
        match rows_for_file(&state, &parsed, file) {
            Ok(mut r) => rows.append(&mut r),
            Err(e) => println!("{}: {e}", file.display()),
        }
    }

    report(&rows);
    score_against_manual(&rows, (0.002, 0.005));
    score_with_each_gates_own_band(&rows);
}

fn rows_for_file(
    state: &GateState,
    parsed: &crate::omiq::metadata::ParsedMetaData,
    file: &std::path::Path,
) -> anyhow::Result<Vec<Row>> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("unreadable file name"))?;

    // The harness is handed bare file names; the metadata keys on Omiq's
    // decorated ones, so match on the stem.
    let file_id = parsed
        .file_name_to_gating_id
        .iter()
        .find(|(k, _)| {
            let stem = name.trim_end_matches(".fcs");
            k.contains(stem) || stem.contains(k.trim_end_matches(".fcs"))
        })
        .map(|(_, v)| v.clone())
        .ok_or_else(|| anyhow::anyhow!("no metadata row matches {name}"))?;

    let groups = parsed
        .metadata
        .get(&file_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no metadata for {file_id}"))?;

    // Already compensated and scaled, so no transforms are applied here.
    let fcs = Fcs::open(file.to_str().unwrap_or_default())?;
    let df = (*fcs.apply_arcsinh_transforms(&[])?).clone();
    let df = df.with_row_index("original_index".into(), None)?;

    let resolver = state.get_current_sample(file_id.clone(), &groups);
    let sample = name.trim_end_matches(".fcs").to_string();

    let mut out = Vec::new();
    for (node, placement) in state.placements() {
        let gate_id = &placement.gate_id;
        let Some(gate) = state.gate_for_file(gate_id, &file_id, &parsed.metadata) else {
            continue;
        };
        let Some(inner) = gate.get_gate_ref(None) else {
            continue;
        };
        let GateGeometry::Rectangle { min, max } = &inner.geometry else {
            continue;
        };
        let (x_param, y_param) = gate.get_params();
        let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)): (
            Option<f32>,
            Option<f32>,
            Option<f32>,
            Option<f32>,
        ) = (
            min.get_coordinate(&x_param),
            max.get_coordinate(&x_param),
            min.get_coordinate(&y_param),
            max.get_coordinate(&y_param),
        ) else {
            continue;
        };
        if df.column(x_param.as_ref()).is_err() {
            continue;
        }

        let Some(parent) = state.parent_node(node) else {
            continue;
        };
        let chain = state.gate_chain_for_node(&parent);
        let values = parent_values(&df, &chain, &resolver, &x_param)?;
        if values.len() < 2 {
            continue;
        }

        let mut sorted = values.clone();
        sorted.sort_by(|a, b| b.total_cmp(a));
        let lo = *sorted.last().expect("checked non-empty");
        let hi = sorted[0];

        // An edge outside the data it is drawn over is not a threshold: it was
        // dragged off the plot to leave that side open.
        let lower_bounds = (min_x as f64) > lo;
        let upper_bounds = (max_x as f64) < hi && (max_x as f64).abs() < 1e9;
        let (edge, manual_x) = match (lower_bounds, upper_bounds) {
            (true, false) => (Edge::Lower, min_x as f64),
            (false, true) => (Edge::Upper, max_x as f64),
            (true, true) => (Edge::Window, min_x as f64),
            (false, false) => (Edge::Neither, min_x as f64),
        };
        let admitted = match edge {
            Edge::Upper => values.iter().filter(|v| **v < manual_x).count(),
            _ => values.iter().filter(|v| **v > manual_x).count(),
        };

        out.push(Row {
            sample: sample.clone(),
            gate: gate.get_name().to_string(),
            channel: x_param.to_string(),
            parent_events: values.len(),
            edge,
            values: values.clone(),
            manual_x,
            manual_fraction: admitted as f64 / values.len() as f64,
            spread: interquartile_spread(&sorted),
            // A gate whose y edges span the plot is the one-dimensional case.
            y_full_span: (max_y - min_y).abs() > 1e6,
        });
    }
    Ok(out)
}

fn parent_values(
    df: &DataFrame,
    chain: &[GateId],
    resolver: &crate::gate_editor::gates::gate_store::GateOverrideResolver,
    channel: &str,
) -> anyhow::Result<Vec<f64>> {
    let frame = if chain.is_empty() {
        df.clone()
    } else {
        let mask = filter_events_by_hierarchy_to_mask(df, chain, resolver)?;
        df.filter(&mask)?
    };
    Ok(frame
        .column(channel)?
        .f32()?
        .into_no_null_iter()
        .map(|v| v as f64)
        .collect())
}

fn report(rows: &[Row]) {
    println!("\n{}", "=".repeat(108));
    println!("What the hand-placed gates actually capture");
    println!("{}", "=".repeat(108));
    println!(
        "{:<22} {:<22} {:<17} {:>7} {:>7} {:>9} {:>9} {:>4}",
        "sample", "gate", "channel", "parent", "edge", "manual x", "% parent", "1D"
    );
    for r in rows {
        if !r.y_full_span || r.edge == Edge::Neither {
            continue;
        }
        println!(
            "{:<22} {:<22} {:<17} {:>7} {:>7} {:>9.3} {:>8.3}% {:>4}",
            truncate(&r.sample, 22),
            truncate(&r.gate, 22),
            truncate(&r.channel, 17),
            r.parent_events,
            r.edge.label(),
            r.manual_x,
            r.manual_fraction * 100.0,
            if r.y_full_span { "y" } else { "n" }
        );
    }

    let skipped = rows.iter().filter(|r| r.edge == Edge::Neither).count();
    println!(
        "\n{skipped} of {} rows bound x on neither side - those gates select on the other axis",
        rows.len()
    );

    let one_d: Vec<&Row> = rows
        .iter()
        .filter(|r| r.y_full_span && r.edge != Edge::Neither)
        .collect();
    println!(
        "\n{} rows, {} of them one-dimensional",
        rows.len(),
        one_d.len()
    );
    if one_d.is_empty() {
        return;
    }
    let mut fractions: Vec<f64> = one_d.iter().map(|r| r.manual_fraction).collect();
    fractions.sort_by(f64::total_cmp);
    let q =
        |p: f64| fractions[((p * (fractions.len() - 1) as f64) as usize).min(fractions.len() - 1)];
    println!(
        "fraction captured by a one-dimensional gate: p10 {:.3}%  median {:.3}%  p90 {:.3}%",
        q(0.1) * 100.0,
        q(0.5) * 100.0,
        q(0.9) * 100.0
    );
    let mut spreads: Vec<f64> = one_d
        .iter()
        .map(|r| r.spread)
        .filter(|s| *s > 0.0)
        .collect();
    spreads.sort_by(f64::total_cmp);
    if !spreads.is_empty() {
        println!(
            "interquartile spread of those parent populations: median {:.3}",
            spreads[spreads.len() / 2]
        );
    }
}

/// Solve each gate the way a rule would and set it beside what a person did.
///
/// Only the lower-edge gates on an FMO sample: that is where the band rule is
/// applied by hand, and it is the case the solvers were written for.
fn score_against_manual(rows: &[Row], band: (f64, f64)) {
    let rule = TailFractionRule::new(band);
    let model = CountAndSeparation::default();
    println!("\n{}", "=".repeat(108));
    println!(
        "Solved against hand-placed, band {:.1}% to {:.1}%, FMO samples only",
        band.0 * 100.0,
        band.1 * 100.0
    );
    println!("{}", "=".repeat(108));
    println!(
        "{:<22} {:<20} {:>7} {:>9} {:>9} {:>8} {:>7} {:>6}  {}",
        "sample",
        "gate",
        "parent",
        "manual x",
        "solved x",
        "diff",
        "manual%",
        "conf",
        "limiting factor"
    );

    let mut diffs: Vec<f64> = Vec::new();
    for r in rows {
        if !r.y_full_span || r.edge != Edge::Lower || !r.sample.contains("FMX") {
            continue;
        }
        let Ok(solved) = rule.solve(&r.values) else {
            continue;
        };
        let confidence = model.assess(&solved, Some(r.manual_x));
        let diff = solved.x - r.manual_x;
        diffs.push(diff.abs());
        println!(
            "{:<22} {:<20} {:>7} {:>9.3} {:>9.3} {:>8.3} {:>6.3}% {:>6.2}  {}",
            truncate(&r.sample, 22),
            truncate(&r.gate, 20),
            r.parent_events,
            r.manual_x,
            solved.x,
            diff,
            r.manual_fraction * 100.0,
            confidence.score,
            confidence.weakest().map(|c| c.name).unwrap_or("-")
        );
    }
    if diffs.is_empty() {
        return;
    }
    diffs.sort_by(f64::total_cmp);
    println!(
        "\n|solved - manual| over {} gates: median {:.3}  p90 {:.3}  max {:.3}  (arcsinh units)",
        diffs.len(),
        diffs[diffs.len() / 2],
        diffs[(diffs.len() * 9 / 10).min(diffs.len() - 1)],
        diffs[diffs.len() - 1]
    );
}

/// The same comparison with each gate given the band it was evidently placed
/// under, rather than one band for all of them.
///
/// A band is specified per gate and differs between them, so scoring every gate
/// against a single band measures the band, not the solver. Taking the fraction
/// the hand-placed gate actually captured and asking the solver to hit it
/// isolates the question that matters: given the right rule, does it put the
/// line where a person did?
fn score_with_each_gates_own_band(rows: &[Row]) {
    println!("\n{}", "=".repeat(108));
    println!("Solved against hand-placed, each gate given the band it was evidently placed under");
    println!("{}", "=".repeat(108));
    println!(
        "{:<22} {:<20} {:>7} {:>8} {:>9} {:>9} {:>8}",
        "sample", "gate", "parent", "band %", "manual x", "solved x", "diff"
    );
    let mut by_gate: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for r in rows {
        if !r.y_full_span || r.edge != Edge::Lower || !r.sample.contains("FMX") {
            continue;
        }
        // A band of plus or minus a fifth around what the gate captured, which
        // is the width of a rule like "0.2% to 0.5%" around its own midpoint.
        let f = r.manual_fraction;
        let band = ((f * 0.8).max(0.0), (f * 1.2).min(1.0));
        let Ok(solved) = TailFractionRule::new(band).solve(&r.values) else {
            continue;
        };
        let diff = solved.x - r.manual_x;
        by_gate.entry(r.gate.clone()).or_default().push(diff.abs());
        println!(
            "{:<22} {:<20} {:>7} {:>7.3}% {:>9.3} {:>9.3} {:>8.3}",
            truncate(&r.sample, 22),
            truncate(&r.gate, 20),
            r.parent_events,
            f * 100.0,
            r.manual_x,
            solved.x,
            diff
        );
    }
    println!("\n{:<20} {:>6} {:>9} {:>9}", "gate", "n", "median", "max");
    for (gate, mut ds) in by_gate {
        ds.sort_by(f64::total_cmp);
        println!(
            "{:<20} {:>6} {:>9.3} {:>9.3}",
            truncate(&gate, 20),
            ds.len(),
            ds[ds.len() / 2],
            ds[ds.len() - 1]
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}
