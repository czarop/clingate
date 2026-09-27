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

Distributions are in the units the plots are drawn in: arcsinh-scaled where \
the scaling says so.";

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
        match self
            .with_session(move |s| s.find_samples(&args.query))
            .await
        {
            Ok(result) => answer(result),
            Err(reason) => failed(reason),
        }
    }

    /// The populations in the gating tree - each with the shortest name that names only it,
    /// its full path, and the two parameters its gate is drawn on. A query narrows the list.
    #[tool(annotations(read_only_hint = true))]
    async fn list_populations(&self, Parameters(args): Parameters<ListPopulations>) -> String {
        match self
            .with_session(move |s| s.populations(args.query.as_deref()))
            .await
        {
            Ok(result) => answer(result),
            Err(reason) => failed(reason),
        }
    }

    /// Events in a population, in its parent and in the whole file, and the population's
    /// percent of parent and of total, for each sample named - with the minimum, median and
    /// maximum percent of parent across them.
    #[tool(annotations(read_only_hint = true))]
    async fn population_stats(&self, Parameters(args): Parameters<PopulationStats>) -> String {
        match self
            .with_session(move |s| s.population_stats(&args.population, &args.samples))
            .await
        {
            Ok(result) => answer(result),
            Err(reason) => failed(reason),
        }
    }

    /// How a population's parent is spread on one parameter in one sample - percentiles and
    /// a histogram - and where the population's gate edges sit on it, with the share of the
    /// parent between them. In the units the plots are drawn in.
    #[tool(annotations(read_only_hint = true))]
    async fn distribution(&self, Parameters(args): Parameters<DistributionArgs>) -> String {
        match self
            .with_session(move |s| s.distribution(&args.population, &args.sample, &args.parameter))
            .await
        {
            Ok(result) => answer(result),
            Err(reason) => failed(reason),
        }
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
        match self
            .with_session(move |s| s.answer_omiq(&args.group, matrix.as_deref()))
            .await
        {
            Ok(result) => answer(result),
            Err(reason) => failed(reason),
        }
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
