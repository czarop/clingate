//! A report on how one rule did in the last applied run, written as Markdown
//! with pictures of every sample it placed, to be sent on whole.
//!
//! One gate at a time, so a rule that went wrong on some samples can be sent
//! on its own: what the rule was, what the run did on each sample and why the
//! review flagged it, and each sample's plot - the gate's parent population on
//! the gate's two parameters, with the gate as it stands now. The flagged
//! samples' plots and the rest go in separate pictures, numbered as the table
//! numbers them. Nothing in the workspace changes. A change to what the report
//! says raises [`SUITE_VERSION`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use image::{Rgba, RgbaImage};
use rayon::prelude::*;

use crate::gate_rules::autogate::describe;
use crate::gate_rules::rule_store::{GateRule, human_order};
use crate::gate_rules::run::RunInputs;
use crate::gates::GateState;
use crate::omiq::serialise::AxisSettings;
use crate::picture::{Drawn, PlotRequest, TILE, grid, number_tile, render};
use crate::review::board::{Board, Entry, Pile, board_in};
use crate::review::run_record::{PlacementStatus, RunRecord};
use crate::test_report::{Report, cell, measured_on, share_or_dash, stopped};

/// What the report tells, and the version of what it says.
pub const SUITE: &str = "rule run";
pub const SUITE_VERSION: u32 = 1;
/// Plots in one picture, in rows of [`COLUMNS`].
pub const TILES_PER_PICTURE: usize = crate::picture::MOST_TILES;
const COLUMNS: usize = 3;

/// Everything a report reads: the workspace as it stands, and the gate.
pub struct RuleReportJob {
    /// The workspace folder's name.
    pub workspace: String,
    /// When the report is made, as it is printed.
    pub made: String,
    /// The workspace folder, where the run and its review are kept.
    pub folder: PathBuf,
    pub gates: GateState,
    pub inputs: RunInputs,
    pub axes: AxisSettings,
    /// The gate reported on, by its id.
    pub gate_id: String,
}

/// A report: its Markdown and its pictures.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleReport {
    /// The gate, as a person reads it.
    pub gate: String,
    pub markdown: String,
    pub pictures: Vec<Picture>,
}

/// One picture of numbered plots, and the name it is kept under.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub name: String,
    pub png: Vec<u8>,
}

/// One sample's placement, numbered as its plot is.
struct Row<'a> {
    number: usize,
    entry: &'a Entry,
    drawn: Result<Drawn, String>,
}

/// The report on the gate `job` names, or why there is none - no run kept,
/// the run did not place that gate, or asked to stop. `progress` hears how
/// many plots are drawn of how many.
pub fn rule_report(
    job: &RuleReportJob,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<RuleReport, String> {
    let metadata = &job.inputs.metadata;
    let run = RunRecord::load(&job.folder)
        .map_err(|e| e.to_string())?
        .ok_or("No rules run has been applied in this workspace")?;
    let board = board_in(&job.folder, &job.gates, metadata)
        .map_err(|e| e.to_string())?
        .ok_or("No rules run has been applied in this workspace")?;
    let entries = in_report_order(&board, &job.gate_id);
    let first = entries
        .first()
        .ok_or("The last applied run did not place this gate on any sample")?;
    let gate = describe(&first.gate, first.parent_gate.as_deref());

    let mut report = Report::default();
    report.push(header(job, &run, &gate));
    report.push(rule(&run, &job.inputs, first));
    report.push(summary(&board, &entries, &job.gate_id));

    let drawn = report.timed("Drawing", || Ok(draw(job, &entries, cancel, &progress)))?;
    stopped(cancel)?;
    let rows: Vec<Row> = entries
        .iter()
        .zip(drawn)
        .enumerate()
        .map(|(at, (entry, drawn))| Row {
            number: at + 1,
            entry,
            drawn,
        })
        .collect();
    for row in &rows {
        if let Err(why) = &row.drawn {
            report.problem(format!(
                "Sample {} ({}) could not be drawn: {why}",
                row.number,
                sample_name(row.entry)
            ));
        }
    }
    report.push(sample_by_sample(&run, &rows));
    let (pictures, said) = pictures(&rows, &mut report);
    report.push(said);
    Ok(RuleReport {
        gate,
        markdown: report.finish(),
        pictures,
    })
}

/// The gate's placements: the flagged worst first, then the rest by sample.
fn in_report_order<'a>(board: &'a Board, gate_id: &str) -> Vec<&'a Entry> {
    let (mut flagged, mut rest): (Vec<&Entry>, Vec<&Entry>) = board
        .entries
        .iter()
        .filter(|e| e.gate_id == gate_id)
        .partition(|e| e.flag.is_some());
    let severity = |e: &Entry| e.flag.as_ref().map_or(0.0, |f| f.severity);
    flagged.sort_by(|a, b| {
        severity(b)
            .total_cmp(&severity(a))
            .then_with(|| human_order(&sample_name(a), &sample_name(b)))
    });
    rest.sort_by(|a, b| human_order(&sample_name(a), &sample_name(b)));
    flagged.extend(rest);
    flagged
}

fn sample_name(entry: &Entry) -> String {
    entry
        .sample
        .name
        .clone()
        .unwrap_or_else(|| entry.sample.id.clone())
}

/// Every placement's plot, in order; one that cannot be drawn says why.
fn draw(
    job: &RuleReportJob,
    entries: &[&Entry],
    cancel: &AtomicBool,
    progress: &(impl Fn(usize, usize) + Sync),
) -> Vec<Result<Drawn, String>> {
    let done = AtomicUsize::new(0);
    progress(0, entries.len());
    entries
        .par_iter()
        .map(|entry| {
            if cancel.load(Ordering::Relaxed) {
                return Err("stopped".to_string());
            }
            let drawn = request(job, entry).and_then(|r| render(&r).map_err(|e| e.to_string()));
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, entries.len());
            drawn
        })
        .collect()
}

/// The plot of `entry`'s sample: its gate's parent population on the gate's
/// parameters, with the gate as it stands for that sample.
fn request(job: &RuleReportJob, entry: &Entry) -> Result<PlotRequest, String> {
    let state = &job.gates;
    let inputs = &job.inputs;
    let node =
        crate::review::report::node_named(state, &entry.gate_id, entry.parent_gate.as_deref())
            .ok_or("the gate is no longer drawn")?;
    let chain = state
        .parent_node(&node)
        .map(|parent| state.gate_chain_for_node(&parent))
        .unwrap_or_default();
    let gate_id: Arc<str> = Arc::from(entry.gate_id.as_str());
    let file: Arc<str> = Arc::from(entry.sample.id.as_str());
    let gate = state
        .gate_for_file(&gate_id, &file, &inputs.metadata)
        .ok_or("the gate is not drawn for this sample")?;
    let (x, y) = gate.get_params();
    let name = entry
        .sample
        .name
        .as_deref()
        .ok_or("the sample has no file name")?;
    let (_, path) = inputs
        .files
        .iter()
        .find(|(n, _)| &**n == name)
        .ok_or("its file is not in the workspace")?;
    let groups = inputs.metadata.get(&file).cloned().unwrap_or_default();
    let axis = |p: &str| job.axes.get(p).cloned().unwrap_or_default();
    Ok(PlotRequest {
        path: path.clone(),
        compensation: inputs.compensation.matrix_for(path),
        cofactors: inputs.cofactors.clone(),
        chain,
        resolver: state.get_current_sample(file, &groups),
        x_axis: axis(&x),
        y_axis: axis(&y),
        x,
        y,
        gate: Some(gate),
    })
}

fn header(job: &RuleReportJob, run: &RunRecord, gate: &str) -> String {
    format!(
        "# clingate rule report: {}\n\n\
         | | |\n|---|---|\n\
         | Tells | {SUITE}, version {SUITE_VERSION} |\n\
         | clingate | {} |\n\
         | Made | {} |\n\
         | Workspace | {} |\n\
         | Run applied | {} |\n\
         | Gate | {} |\n",
        cell(gate),
        env!("CARGO_PKG_VERSION"),
        job.made,
        cell(&job.workspace),
        run.applied_at,
        cell(gate),
    )
}

fn rule_said(rule: &GateRule) -> String {
    format!(
        "{} - {} on {}, keeping {} the line, {}",
        rule.rule.kind(),
        rule.rule.describe(),
        rule.parameter,
        format!("{:?}", rule.bound).to_lowercase(),
        measured_on(&rule.measured_on)
    )
}

/// The rule the run placed the gate by, and whether it has changed since.
fn rule(run: &RunRecord, inputs: &RunInputs, entry: &Entry) -> String {
    let (gate, parent) = (entry.gate.as_str(), entry.parent_gate.as_deref());
    let then = run.rules.rule_for(gate, parent);
    let now = inputs.rules.rule_for(gate, parent);
    let then_said = then.map_or("no rule kept with the run".to_string(), rule_said);
    let now_said = match now {
        _ if now == then => "the same".to_string(),
        Some(now) => format!("changed to {}", rule_said(now)),
        None => "removed".to_string(),
    };
    format!("\n## The rule\n\n- In the run: {then_said}\n- Now: {now_said}\n")
}

fn summary(board: &Board, entries: &[&Entry], gate_id: &str) -> String {
    let count = |keep: &dyn Fn(&Entry) -> bool| entries.iter().filter(|e| keep(e)).count();
    let piles: Vec<String> = Pile::ALL
        .iter()
        .map(|pile| {
            format!(
                "{} {}",
                pile.title().to_lowercase(),
                board.count_for(*pile, Some(gate_id))
            )
        })
        .collect();
    let mut out = format!(
        "\n## Summary\n\n\
         - {} placements: {} moved by the rule, {} already met it\n\
         - {} flagged by the review, {} of them judged to look right\n\
         - On the Review tab: {}\n",
        entries.len(),
        count(&|e| e.moved),
        count(&|e| !e.moved),
        count(&|e| e.flag.is_some()),
        count(&|e| e.flag.is_some() && e.looks_right),
        piles.join(", "),
    );
    let first = entries[0];
    for unplaced in board
        .unplaced
        .iter()
        .filter(|u| u.gate == first.gate && u.parent_gate == first.parent_gate)
    {
        out.push_str(&format!("- Not placed: {}\n", unplaced.says()));
    }
    out
}

fn sample_by_sample(run: &RunRecord, rows: &[Row]) -> String {
    let mut out = "\n## Sample by sample\n\n\
         Numbered as the plots are. The line is on the rule's parameter; \
         for a phenotype rule, the share of the parent the gate held. The rule admitted is what \
         the placed gate held of the population it was judged on, when it was placed; the gate \
         holds now is what it holds of this sample's parent as drawn now.\n\n\
         | # | Sample | Type | Flagged | Review | Line | Confidence | Weakest | In band | Rule admitted | Gate holds now | Rule read | Why flagged |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|\n"
        .to_string();
    for row in rows {
        let entry = row.entry;
        let placed = run
            .placed
            .iter()
            .find(|p| p.gate_id == entry.gate_id && p.sample.id == entry.sample.id);
        let kept = run
            .kept
            .iter()
            .find(|k| k.gate_id == entry.gate_id && k.sample.id == entry.sample.id);
        let line = match (placed, kept) {
            (Some(p), _) => format!("{} → {}", number(p.from), number(p.to)),
            (None, Some(k)) => format!("{} (left)", number(k.line)),
            (None, None) => "-".into(),
        };
        let admitted = placed
            .and_then(|p| p.achieved)
            .or_else(|| kept.and_then(|k| k.achieved));
        let holds = row.drawn.as_ref().ok().map_or("-".to_string(), |d| {
            format!("{} of {}", share_or_dash(d.holds), d.parent_events)
        });
        let why = entry.flag.as_ref().map_or(String::new(), |f| {
            f.reasons
                .iter()
                .map(|r| r.says.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        });
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.number,
            cell(&sample_name(entry)),
            cell(entry.sample.sample_type.as_deref().unwrap_or("-")),
            if entry.flag.is_some() { "yes" } else { "" },
            review(entry),
            line,
            entry.confidence.map_or("-".into(), |c| format!("{c:.2}")),
            cell(entry.weakest.as_deref().unwrap_or("")),
            placed.map_or("", |p| if p.in_band { "yes" } else { "no" }),
            share_or_dash(admitted),
            holds,
            cell(
                entry
                    .rule_read
                    .as_ref()
                    .map_or("", |r| r.name.as_deref().unwrap_or(&r.id))
            ),
            cell(&why),
        ));
    }
    out
}

/// Where the placement stands in the review, in a few words.
fn review(entry: &Entry) -> String {
    let mut said = entry.pile.title().to_string();
    if entry.looks_right {
        said.push_str(", looks right");
    }
    if entry.status == Some(PlacementStatus::Moved) {
        said.push_str(", moved since - drawn as it is now");
    }
    said
}

fn number(value: Option<f64>) -> String {
    value.map_or("-".into(), |v| format!("{v:.4}"))
}

/// The plots, flagged and not, in pictures of [`TILES_PER_PICTURE`], and
/// which is which.
fn pictures(rows: &[Row], report: &mut Report) -> (Vec<Picture>, String) {
    let (flagged, rest): (Vec<&Row>, Vec<&Row>) =
        rows.iter().partition(|row| row.entry.flag.is_some());
    let mut pictures = Vec::new();
    let mut said = "\n## Pictures\n\n\
         Each plot is the gate's parent population on the gate's two parameters, with the gate \
         as it stands now, numbered in its corner as in the table. Left to right, top to bottom.\n"
        .to_string();
    for (stem, rows) in [("flagged", flagged), ("not-flagged", rest)] {
        if rows.is_empty() {
            said.push_str(&format!("\n- {stem}: none\n"));
            continue;
        }
        for (at, chunk) in rows.chunks(TILES_PER_PICTURE).enumerate() {
            let name = format!("{stem}-{}.png", at + 1);
            let tiles: Vec<RgbaImage> = chunk.iter().map(|row| tile(row)).collect();
            match grid(&tiles, COLUMNS) {
                Ok(png) => {
                    let numbers: Vec<String> =
                        chunk.iter().map(|row| row.number.to_string()).collect();
                    said.push_str(&format!("\n- {name}: {}\n", numbers.join(", ")));
                    pictures.push(Picture { name, png });
                }
                Err(e) => report.problem(format!("{name} could not be made: {e}")),
            }
        }
    }
    (pictures, said)
}

/// A row's plot numbered, or a grey tile with its number where it could not
/// be drawn, so the rest keep their places.
fn tile(row: &Row) -> RgbaImage {
    let mut image = match &row.drawn {
        Ok(drawn) => drawn.image.clone(),
        Err(_) => RgbaImage::from_pixel(TILE, TILE, Rgba([200, 200, 200, 255])),
    };
    number_tile(&mut image, row.number);
    image
}

/// Keep `report` in the workspace `folder`: a folder of its own, named by its
/// gate and `stamp`, holding the Markdown and the pictures. The Markdown's
/// path.
pub fn keep(folder: &Path, report: &RuleReport, stamp: &str) -> std::io::Result<PathBuf> {
    let gate: String = report
        .gate
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let at = folder
        .join(crate::test_report::FOLDER)
        .join(format!("rule-report-{gate}-{stamp}"));
    std::fs::create_dir_all(&at)?;
    for picture in &report.pictures {
        std::fs::write(at.join(&picture.name), &picture.png)?;
    }
    let path = at.join("report.md");
    std::fs::write(&path, &report.markdown)?;
    Ok(path)
}
