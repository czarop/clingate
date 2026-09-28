//! clingate's tools for Claude, served over the Model Context Protocol.
//!
//! Each tool is a thin translation: its arguments into a call on
//! [`clingate_core::session::Session`], and the answer back as JSON. Everything
//! a tool knows - how a workspace loads, how a name is matched, what a count
//! means - is in the core, where it is tested as ordinary Rust.
//!
//! Every answer is JSON with an `outcome`:
//!
//! - `ok`, with the `result`;
//! - `needs_clarification` - a name matched nothing, or several things where
//!   one was needed. The person is to be asked; the `suggestions` are for them
//!   to choose from, not for the model to pick;
//! - `failed`, with the `reason`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clingate_core::session::{Refusal, Session};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

/// What the model is told about the tools as a whole, once.
pub const INSTRUCTIONS: &str = "\
clingate reads a flow cytometry workspace - FCS files, an Omiq metadata export, \
an Omiq scaling export and an Omiq gating file - and answers questions about \
the samples and gated populations in it.

Start with open_workspace. Always ask the user which folder to open; never \
guess a folder or reuse one from an earlier conversation without asking.

Samples, populations and parameters are named the way a person names them: \
samples by any word of their file name or metadata (e.g. 'fmx', 'wk1 fmx', \
'SampleType=FS', or 'all'); populations by their gate names, with '>' for \
steps down the tree (e.g. 'CD4+', 'CD4+ > CD279+'); parameters by marker or \
channel (e.g. 'CD4', 'BUV395-A').

Matching is strict. When an answer's outcome is 'needs_clarification', ask the \
user what they meant, showing them the suggestions. Never choose a suggestion \
yourself and never retry with a guess - even an obvious-looking one - without \
the user saying so.

If the overview says a compensation group needs an answer, those files cannot \
be read until the user says whether compensation was applied in Omiq (and, if \
it was, pastes the matrix exported from Omiq). Ask them; do not assume.

Distributions, gate edges and comparisons are in the units the plots are drawn \
in: arcsinh-scaled where the scaling says so.

To see whether a sample is distributed unlike the rest - the usual reason a \
rule puts a gate in the wrong place - use compare_samples: shift_in_iqrs far \
from 0, or spread_ratio well above 1, marks a sample worth a closer look.

Gates are edited in a working copy, exactly as in the clingate app. The \
rules change gates, so they run in steps, each only when the user asks: \
preview_rules says what would move and changes nothing; show the user the \
moves, above all those marked review. apply_rule_placements applies the last \
preview to the working copy - one step, which undo takes back - only once the \
user has said to. save_gating saves the working copy into the workspace folder \
(clingate_gating.omiqgt), where the app will open it; export_gating writes \
the saved copy under another name. Save and export only when the user says \
to, and never replace a file unless the user has said to replace that file.

If the overview says an earlier session left unsaved changes, ask the user \
whether to restore or discard them.

To review a rules run, start with assess_run: it lists the placements that look \
unlike their peers or that the rule was unsure of, with reasons, and costs \
little. compare_to_peers then shows one sample beside its peers in numbers. \
Present what you find to the user; the flags are for a person to judge.

When the user says a gate was placed badly, report_placement records it - with \
their reason - so the rules' confidence scores can be improved; do not report \
a gate on your own judgement. mark_run_reviewed records that the user has \
finished reviewing a run: only when they say so, since every placement they \
did not report then counts as accepted.";

/// The server, holding the one open workspace.
#[derive(Clone)]
pub struct Clingate {
    session: Arc<Mutex<Option<Session>>>,
    tool_router: ToolRouter<Self>,
}

impl Default for Clingate {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenWorkspace {
    /// The folder holding the workspace, as the user gave it.
    pub folder: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindSamples {
    /// Words from the samples' file names or metadata values, e.g. 'fmx', 'wk1 fmx',
    /// 'SampleType=FS'; 'all' for every sample.
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPopulations {
    /// Gate names to narrow the list, e.g. 'CD279+'. Leave out to list every population.
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PopulationStats {
    /// The population, by its gate names, e.g. 'CD4+' or 'CD4+ > CD279+'.
    pub population: String,
    /// The samples, by words of their file names or metadata, or 'all'.
    pub samples: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DistributionArgs {
    /// The population, by its gate names.
    pub population: String,
    /// Exactly one sample, by words of its file name or metadata.
    pub sample: String,
    /// The parameter, by marker or channel, e.g. 'CD4' or 'BUV395-A'.
    pub parameter: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnswerOmiq {
    /// The compensation group, by the name the overview lists it under.
    pub group: String,
    /// What the user said: was compensation applied in Omiq before these files were exported?
    pub compensation_applied_in_omiq: bool,
    /// If it was: the compensation matrix the user exported from Omiq and pasted, exactly as
    /// pasted.
    pub matrix: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListParameters {
    /// A marker or channel to look up, e.g. 'CD4' or 'BUV395-A'. Leave out to list every
    /// parameter.
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GateDetailsArgs {
    /// The population whose gate to describe, by its gate names.
    pub population: String,
    /// One sample, by words of its file name or metadata, to see the position that applies to
    /// it. Leave out for the gate as drawn.
    pub sample: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompareSamples {
    /// The population, by its gate names. Its parent is what is compared.
    pub population: String,
    /// The parameter, by marker or channel.
    pub parameter: String,
    /// The samples, by words of their file names or metadata, or 'all'.
    pub samples: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompareToPeers {
    /// The population whose gate to compare, by its gate names.
    pub population: String,
    /// Exactly one sample, by words of its file name or metadata.
    pub sample: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReportPlacement {
    /// The population whose gate was placed badly, by its gate names.
    pub population: String,
    /// Exactly one sample, by words of its file name or metadata.
    pub sample: String,
    /// What the user says is wrong: too_high, too_low, cuts_through_a_population,
    /// wrong_population, too_tight, too_loose, wrong_reference, should_not_have_moved, or
    /// other.
    pub problem: String,
    /// The user's own words about it, if they gave any.
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportGating {
    /// The file name the user chose - a name alone, no folder. '.omiqgt' is added if missing.
    pub file_name: String,
    /// Replace a file of that name. Only when the user has said to replace that file.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Serialize)]
struct Answer<T: Serialize> {
    outcome: &'static str,
    result: T,
}

fn ok<T: Serialize>(result: T) -> String {
    // Compact: every character is read by the model.
    serde_json::to_string(&Answer {
        outcome: "ok",
        result,
    })
    .unwrap_or_else(|e| failed(format!("the answer could not be written: {e}")))
}

fn refused(refusal: Refusal) -> String {
    serde_json::to_string(&refusal).unwrap_or_else(|e| failed(e))
}

fn failed(reason: impl std::fmt::Display) -> String {
    refused(Refusal::Failed {
        reason: reason.to_string(),
    })
}

fn answer<T: Serialize>(result: Result<T, Refusal>) -> String {
    match result {
        Ok(value) => ok(value),
        Err(refusal) => refused(refusal),
    }
}

const NO_WORKSPACE: &str =
    "no workspace is open: ask the user which folder to open, then call open_workspace";

impl Clingate {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// Run `work` on the open session, off the async runtime - reading FCS
    /// files is blocking work.
    async fn with_session<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Session) -> T + Send + 'static,
    ) -> Result<T, String> {
        let session = self.session.clone();
        tokio::task::spawn_blocking(move || {
            let mut held = session
                .lock()
                .map_err(|_| "the workspace is unavailable".to_string())?;
            match held.as_mut() {
                Some(session) => Ok(work(session)),
                None => Err(NO_WORKSPACE.to_string()),
            }
        })
        .await
        .map_err(|e| format!("the work stopped: {e}"))?
    }

    /// Run a question on the open session and answer it.
    async fn run<T: Serialize + Send + 'static>(
        &self,
        work: impl FnOnce(&mut Session) -> Result<T, Refusal> + Send + 'static,
    ) -> String {
        match self.with_session(work).await {
            Ok(result) => answer(result),
            Err(reason) => failed(reason),
        }
    }
}

#[tool_router]
impl Clingate {
    /// Open the workspace in a folder: its FCS files, and the metadata, scaling and gating
    /// exports beside them. Replaces any workspace already open. Answers with an overview:
    /// what loaded, the metadata columns and their values, and anything that has to be asked
    /// before the files can be read.
    #[tool(annotations(read_only_hint = true))]
    async fn open_workspace(&self, Parameters(args): Parameters<OpenWorkspace>) -> String {
        let folder = PathBuf::from(args.folder.trim());
        if !folder.is_dir() {
            return failed(format!("{} is not a folder", folder.display()));
        }
        let session = self.session.clone();
        let opened = tokio::task::spawn_blocking(move || {
            let opened = Session::open(&folder);
            match opened {
                Ok(new) => {
                    let overview = new.overview();
                    match session.lock() {
                        Ok(mut held) => {
                            *held = Some(new);
                            Ok(overview)
                        }
                        Err(_) => Err("the workspace is unavailable".to_string()),
                    }
                }
                Err(e) => Err(format!("{} could not be opened: {e}", folder.display())),
            }
        })
        .await;
        match opened {
            Ok(Ok(overview)) => ok(overview),
            Ok(Err(reason)) => failed(reason),
            Err(e) => failed(format!("the work stopped: {e}")),
        }
    }

    /// The open workspace at a glance: what loaded, the metadata columns and their values,
    /// the compensation groups, and anything that has to be asked before files can be read.
    #[tool(annotations(read_only_hint = true))]
    async fn workspace_overview(&self) -> String {
        match self.with_session(|s| s.overview()).await {
            Ok(overview) => ok(overview),
            Err(reason) => failed(reason),
        }
    }

    /// The samples a query names, with their metadata, and which file-name words or metadata
    /// columns each word matched.
    #[tool(annotations(read_only_hint = true))]
    async fn find_samples(&self, Parameters(args): Parameters<FindSamples>) -> String {
        self.run(move |s| s.find_samples(&args.query)).await
    }

    /// The populations in the gating tree - each with the shortest name that names only it,
    /// its full path, and the two parameters its gate is drawn on. A query narrows the list.
    #[tool(annotations(read_only_hint = true))]
    async fn list_populations(&self, Parameters(args): Parameters<ListPopulations>) -> String {
        self.run(move |s| s.populations(args.query.as_deref()))
            .await
    }

    /// Events in a population, in its parent and in the whole file, and the population's
    /// percent of parent and of total, for each sample named - with the minimum, median and
    /// maximum percent of parent across them.
    #[tool(annotations(read_only_hint = true))]
    async fn population_stats(&self, Parameters(args): Parameters<PopulationStats>) -> String {
        self.run(move |s| s.population_stats(&args.population, &args.samples))
            .await
    }

    /// How a population's parent is spread on one parameter in one sample - percentiles and
    /// a histogram - and where the population's gate edges sit on it, with the share of the
    /// parent between them. In the units the plots are drawn in.
    #[tool(annotations(read_only_hint = true))]
    async fn distribution(&self, Parameters(args): Parameters<DistributionArgs>) -> String {
        self.run(move |s| s.distribution(&args.population, &args.sample, &args.parameter))
            .await
    }

    /// Record what the user said about a compensation group of files exported from Omiq:
    /// whether compensation was applied in Omiq, and if so the matrix they pasted from Omiq.
    /// Only ever with the user's own answer. Changes how this session reads those files;
    /// writes nothing to disk.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn answer_omiq_compensation(&self, Parameters(args): Parameters<AnswerOmiq>) -> String {
        if args.compensation_applied_in_omiq && args.matrix.is_none() {
            return failed(
                "compensation was applied in Omiq, so the matrix is needed: ask the user to \
                 export it from Omiq and paste it",
            );
        }
        let matrix = if args.compensation_applied_in_omiq {
            args.matrix
        } else {
            None
        };
        self.run(move |s| s.answer_omiq(&args.group, matrix.as_deref()))
            .await
    }

    /// Every parameter - marker, channel, scale and axis range - or the one a query names.
    #[tool(annotations(read_only_hint = true))]
    async fn list_parameters(&self, Parameters(args): Parameters<ListParameters>) -> String {
        self.run(move |s| s.parameters(args.query.as_deref())).await
    }

    /// A population's gate: the parameters it is drawn on, its extent on each, and its shape.
    /// For one sample: the position that applies to that sample, and whether it is the gate as
    /// drawn or a position set for a group of samples or for that sample alone.
    #[tool(annotations(read_only_hint = true))]
    async fn gate_details(&self, Parameters(args): Parameters<GateDetailsArgs>) -> String {
        self.run(move |s| s.gate(&args.population, args.sample.as_deref()))
            .await
    }

    /// How a population's parent is spread on one parameter in each sample named, side by
    /// side: percentiles, each sample's distance from the others in typical interquartile
    /// ranges, its spread against theirs, and where the population's gate sits in it.
    #[tool(annotations(read_only_hint = true))]
    async fn compare_samples(&self, Parameters(args): Parameters<CompareSamples>) -> String {
        self.run(move |s| s.compare_samples(&args.population, &args.parameter, &args.samples))
            .await
    }

    /// The workspace's gate rules: for each, the gate, the parameter, which side it keeps,
    /// which sample it is measured on, and the rule in words.
    #[tool(annotations(read_only_hint = true))]
    async fn list_rules(&self) -> String {
        self.run(|s| s.rules_view()).await
    }

    /// Run every rule and say what it would do, changing nothing: the gates it would move
    /// (from where, to where, with what confidence, and which want review), those already in
    /// place, the reference specimens, and any it could not position.
    #[tool(annotations(read_only_hint = true))]
    async fn preview_rules(&self) -> String {
        self.run(|s| s.preview_rules()).await
    }

    /// Apply the placements of the last preview_rules to the working copy - one step, which
    /// undo takes back. Only when the user has said to. Nothing is saved; refused if the gates
    /// changed since the preview.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn apply_rule_placements(&self) -> String {
        self.run(|s| s.apply_previewed_rules()).await
    }

    /// Step the working copy back one edit - a rules run is one step. As the app's Undo.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn undo(&self) -> String {
        self.run(|s| s.undo()).await
    }

    /// Step forward again after an undo. As the app's Redo.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn redo(&self) -> String {
        self.run(|s| s.redo()).await
    }

    /// Put the working copy back to the last save, discarding unsaved changes - undo brings
    /// them back. As the app's Revert. Only when the user has said to.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn revert_to_saved(&self) -> String {
        self.run(|s| s.revert()).await
    }

    /// Save the working copy into the workspace folder as clingate_gating.omiqgt with its
    /// scaling, as the app's Save does; the workspace opens on it next time. Only when the user
    /// has said to save.
    #[tool(annotations(read_only_hint = false, destructive_hint = true))]
    async fn save_gating(&self) -> String {
        self.run(|s| s.save()).await
    }

    /// Write the saved copy - not unsaved changes - as an Omiq gating file in the workspace
    /// folder, under the name the user chose, as the app's Export does. Refuses to replace an
    /// existing file unless overwrite is set - which only the user may decide.
    #[tool(annotations(read_only_hint = false, destructive_hint = true))]
    async fn export_gating(&self, Parameters(args): Parameters<ExportGating>) -> String {
        self.run(move |s| s.export(&args.file_name, args.overwrite))
            .await
    }

    /// Take back the unsaved changes an earlier session - the app's or a previous one of these -
    /// left in the workspace folder. Only with the user's say-so.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn restore_unsaved_changes(&self) -> String {
        self.run(|s| s.restore_unsaved_changes()).await
    }

    /// Throw away the unsaved changes an earlier session left in the workspace folder. Only
    /// with the user's say-so.
    #[tool(annotations(read_only_hint = false, destructive_hint = true))]
    async fn discard_unsaved_changes(&self) -> String {
        self.run(|s| s.discard_unsaved_changes()).await
    }

    /// Assess the last applied rules run: which placements look unlike their peers - the
    /// other samples of the same kind that the rule placed confidently - or that the rule was
    /// unsure of, each with its reasons in words, worst first; and each gate across the run in
    /// a line. Reads no files: it works from the run the workspace keeps.
    #[tool(annotations(read_only_hint = true))]
    async fn assess_run(&self) -> String {
        self.run(|s| s.assess_run()).await
    }

    /// One sample's placement of a population's gate beside its peers', in numbers: its
    /// percentiles, peaks, line, where the line sits between the negative and positive peaks and
    /// what it lets through - each with the peers' 10th, 50th and 90th percentile. For finding
    /// out why assess_run flagged it.
    #[tool(annotations(read_only_hint = true))]
    async fn compare_to_peers(&self, Parameters(args): Parameters<CompareToPeers>) -> String {
        self.run(move |s| s.compare_to_peers(&args.population, &args.sample))
            .await
    }

    /// Report a gate the rules placed badly on one sample, as the app's Report dialog does:
    /// what the rule did and why, the population's distribution on this sample and the one the
    /// rule read, and the user's reason. These reports are how the confidence scores are
    /// improved. Only when the user has said this gate is wrong - never on your own judgement.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn report_placement(&self, Parameters(args): Parameters<ReportPlacement>) -> String {
        self.run(move |s| {
            s.report_placement(&args.population, &args.sample, &args.problem, &args.note)
        })
        .await
    }

    /// Mark the last applied rules run as reviewed, as the Gate Rules tab's button does: every
    /// placement not reported and still where the rule put it is recorded as accepted, and the
    /// review is copied into the review library. Only when the user says they have finished
    /// reviewing the run.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn mark_run_reviewed(&self) -> String {
        self.run(|s| s.mark_run_reviewed()).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Clingate {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("clingate", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
