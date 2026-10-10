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

use clingate_core::gate_rules::fit::{FitSettings, RankBy};
use clingate_core::gate_rules::pick::PickSettings;
use clingate_core::gate_rules::rule_store::GateRule;
use clingate_core::gate_rules::score::ScoreSettings;
use clingate_core::session::{FitAsk, Refusal, Session};
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

Read the samples' events - population_stats, distribution, compare_samples, \
compare_to_peers, gate_profile, gate_picture, try_rules, score_rules, \
fit_rule, pick_rule, preview_rules - only \
when the user asks you to, or asks for something that cannot be done without \
them. Writing, changing or explaining a rule does not need them: say what the \
rule does, and offer to look at the data rather than looking. Never read the \
data to predict which samples a rule will refuse, stop at or flag - a run \
finds that out, and says so.

When the user asks whether a sample is distributed unlike the rest - the usual \
reason a rule puts a gate in the wrong place - use compare_samples: shift_in_iqrs far \
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

To review a rules run, start with assess_run: it lists the placements whose \
gate sits 2 or more of its parent's IQRs from where its peers put theirs, or \
that the rule was unsure of, with reasons, and costs little, and the gates the \
run could not place. compare_to_peers then shows one \
sample beside its peers in numbers. Present what you find to the user; the \
flags are for a person to judge. A gate the run could not place on any sample \
needs its rule changed: tell the user what is wrong with it, and change it only \
when they agree.

When the user says a gate was placed badly, report_placement records it - with \
their reason - so the rules' confidence scores can be improved; do not report \
a gate on your own judgement. mark_run_reviewed records that the user has \
finished reviewing a run: only when they say so, since every placement they \
did not report then counts as accepted.

When the user asks you to build rules for a panel from the data with them - \
done once per panel, so be thorough but lean: \
1. gate_profile with no population: every gate in a line - what its \
populations look like by sample type. Agree with the user which gates need \
rules. \
2. For each of those: rule_guide (once, and a rule's full guide when you \
need it), gate_picture on that gate (one picture, not one per sample), and \
gate_profile on that gate for the numbers. \
3. Shortlist two or three candidate rules from the shape of the data and \
the guides, and try_rules them together - the results decide, not the \
shortlist. Show the user what each would do, especially where they disagree \
and what they flag. \
4. update_rule only on the user's word. \
Do not picture every gate or try every setting: a few well-chosen \
candidates per gate is the point.

When the user has gated a workspace by hand and asks how close the rules come \
to it, score_rules runs every rule - or one population's - on the files, each \
gate under its parent as drawn, and compares the rule's gate with theirs by the \
events both hold, whatever their shape: their agreement (1 only for exactly the \
same events), how much of their gate the rule's catches, and how much it holds \
beyond it. A sample is off below a line the user can set (off_below), which a \
gate of few cells may fall further below (noise_widths); a sample the rule \
cannot place agrees 0. Per gate, the typical \
and lowest agreement and the samples that are off tell a few samples far off \
from every sample a little off. Show the user the gates and samples least in \
agreement; it moves nothing.

To find the settings for one gate's rule that come closest to the user's \
gating, fit_rule tries the rule as it stands, any candidates you give, and - \
by default when you give none - every combination of a few values of each \
setting that matters for its kind, each scored as score_rules scores it. With \
eight specimens or more it ranks on half and checks on the other half: a \
candidate first on its fit half but far down on its check half was tuned to a \
few samples. Rank by typical agreement (rank_by 'typical') or by fewest \
samples off ('off'); each candidate says where it stands both ways. Narrow or \
extend the defaults from the gate's profile by giving candidates. Show the \
user the best and those tied with it (among_best), and the samples off under \
each - some may be easier gated by hand than fitted. They are kept for the \
app: on the Gallery tab, the gate's population shows each one's gate over \
the user's, one candidate at a time, so the user can compare them by eye. \
Change the rule only on their word.

To pick a rule for a gate - its kind as well as its settings - or for every \
gate, pick_rule tries the kinds in the user's order of preference and takes \
the first that passes their cut-off: a band read on the FMX at the range \
they accept, above the negative, the valley or a smear, a band on each \
sample, and last the phenotype. pick_settings says what the user last set \
in the app; ask them for the FMX range and the cut-off if they have not set \
them (0.95 agreement, no more than a tenth of the samples off, unless they \
say otherwise). Per gate it gives the \
rule beside the rule as it stands, whether it passed, and how each kind did. \
A gate where nothing passed shows only the closest rule: say it is flagged, \
and that the gate may be better gated by hand. For many gates it takes a \
while - say so. Show the user the flagged gates first, then where the rule \
picked does most better, and update_rule only with the rules they choose; \
the closest are on the Gallery tab to compare by eye.

Writing a rule: name its parameter and markers by marker or channel, and a \
reference file by any words that pick out one sample - they are stored as the \
channel and the metadata row a run reads, and update_rule says what it \
changed. A rule is refused, with what would work, if its gate is not drawn \
under the parent named, its parameter is not one the gate is drawn on, or the \
sample type or file it reads is not in the workspace. list_populations gives \
each population's rule_target - the gate and parent exactly as a rule names \
them, e.g. 'IFNy+ of CD161+Va7.2+ / CD4+CD8-' - and, for a linked gate, \
linked_with: the other places the same gate is drawn. list_rules shows any rule already written that \
cannot run, under problems; fix those before previewing.

A run never places a gate over another gate on the same plot: a rule that \
moves a line is held back until its gate touches the other, and anything else \
that would overlap is left where it was. Ruled gates on one plot are placed in \
the order their rules are listed. A run places gates down the tree: a gate under another gate a rule moves is \
measured only after that gate is placed, whatever order the rules are listed \
in - that order counts only between ruled gates on one plot. So settle the rules for the gates higher up before those under them. \
preview_rules runs every level; try_rules and gate_profile read the gates as \
they stand, so a child is only judged on its parent's new position once the \
parent's placements are applied (with the user's say-so) - until then, say \
that the child's numbers are on the parent as it was. A replay keeps each \
gate's events as its run read them, so a changed rule for a parent does not \
change what the gates under it are replayed on. A linked gate (one gate drawn at several places) must be reached by one \
rule at one place, the population that should decide it; the copies follow. \
Two rules for it, or one rule that reaches it at two parents, is refused.

A gate the guide places by another gate - 'the same position as the main \
CD4-CD8+ gate', 'aligned to the left edge of CD19+CD14-' - takes the rule \
kind FromAnotherGate (rule_guide 'From another gate'): same_shape_as copies \
a gate's whole shape, edges sets an edge against another gate's edge. An \
edge's gap is added to the other gate's edge in the plot's units, so its sign \
follows the axis: a gate below or left of the other (its Upper edge at the \
other's Lower) leaves a space with a negative gap, and a positive one puts \
it over the other, where a run never places it; above or right of it, the \
reverse. update_rule lists under problems a rule whose edges put its gate \
over the gate it follows on the gates as drawn: tell the user. A run \
places the gate it follows first, on each sample. Settle the gate it follows \
before writing this rule - its rule written and reviewed, or placed by hand - \
since this gate copies wherever that one ends up. update_rule refuses a rule \
that follows itself, a gate that is not there, or a loop. A gate the guide \
places 'adjacent to' another on the same plot - grown or slid up to it, \
touching but not overlapping - takes the kind NextToGate (rule_guide 'Next to \
another gate'). A marker that separates on some samples and smears on others \
takes the kind ValleyOrSmear (rule_guide 'Valley or smear'), measured on a \
hand-gated File: in the dip where a sample has one, and on a smear as far \
above the negative as on its smear_example, a smear gated by hand - the \
reference itself when the reference is a smear. With no example, a smear is \
left unplaced: ask the user to gate one smear by hand, then name its file as \
smear_example. Which samples are smears is found when the rules run; do not \
read the data beforehand to predict it. A ValleyOrSmear rule can name a \
fallback instead - usually the same gate under another parent - for samples \
where the positives smear with no dip: there its edge goes where the \
fallback's is, placed first, and the placement comes up for review. \
A ValleyOrSmear rule that puts some gates too high inside a thin positive \
population - a few percent of the cells spread wide, the gate at a ripple \
where they pile up rather than in the dip below them - can take \
'lowest_before': true: the lowest point between the negative and the dip it \
found. Off by default; suggest it, and set it only on the user's word, and \
only on the rules whose placements it fixes. \
A ValleyOrSmear rule that gates a shallow wobble inside positives that run straight \
off the negative - a dip a few percent deep where the real ones are tens of \
percent - can take 'smallest_dip': a fraction below which a dip is read as \
none, so the sample is a smear or goes to the fallback. Each placement's \
confidence gives its dip's depth; propose a value between the wobbles and \
the real dips, show the user which samples it changes, and set it only on \
their word. It applies to the reference too. \
A MatchThePhenotype edge drawn against a marker's negative - CD8 just above \
its negatives, where the positives smear - can be pinned there ('pinned': \
[marker]): it then stays as many of the negative's widths above the \
negative's peak as on the reference, wherever the positives go. Nothing in \
the data says whether an edge was meant to sit in a gap or against the \
negative, so suggest it, do not decide it: preview_rules lists under \
could_pin the edges that lie within their negative on the reference; ask the \
user, and pin only on their word.

Runs. The files analysed together - this workspace - are one run. A gating \
guide that says 'per run in the first instance' means one line for all of \
them: a band rule with 'pool': 'Run' reads every FMX file in the workspace \
together and puts the same line on every specimen. For small populations, \
where 0.2% of one FMX is a couple of events, that is the difference between \
a line set by stray events and one set by dozens. Offer to check with \
gate_profile that the specimens are alike enough to share a line. Write a pooled rule \
only when the user asks for one: it is not how this lab gates, and each \
specimen's own FMX no longer holds the band under it.

To work out with the user how the rules could \
place gates better: explain_gate_positioning says exactly how every rule \
decides, and \
read_positioning_code shows the code itself - read what you need rather than \
assuming. replay_rules replays reviewed runs - the workspace's last run and \
those in the review library - on the events each run kept, with the rules they \
ran with and with changes you propose, and says for each placement whether a \
change fixed it, broke it or left it wrong, against where the reviewer said \
the gate belongs. replay_case shows one placement in full. A case that is \
not_reproduced cannot judge a change: say so rather than counting it. Propose \
changes, replay them, and discuss what they fix and break with the user; a \
change that fixes some samples and breaks others is a finding, not a result. \
update_rule writes a rule to the workspace's rules file - only when the user \
says to, and after they have seen the replay. Changing the code itself is for \
the user and their developers: describe the change and the evidence for it.";

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
pub struct MarkLooksRight {
    /// The population whose gate it is, by its gate names.
    pub population: String,
    /// Exactly one sample, by words of its file name or metadata.
    pub sample: String,
    /// True to mark it as looking right; false to take the mark back.
    pub looks_right: bool,
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
pub struct TryRules {
    /// The population whose gate the rules are for, by its gate names.
    pub population: String,
    /// One to four candidate rules, each a whole rule: {"parameter", "bound",
    /// "measured_on", "rule": {"kind", ...}} - see rule_guide.
    pub candidates: serde_json::Value,
    /// How many samples to list, most telling first (default 30, at most 300). The summaries
    /// always count every sample.
    pub max_rows: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScoreRulesArgs {
    /// The population whose rule to score, by its gate names. Leave out for every rule.
    pub population: Option<String>,
    /// How many samples to list, least in agreement with the hand gating first (default 40,
    /// at most 400). The gates' lines always count every sample.
    pub max_rows: Option<usize>,
    /// The agreement below which a sample is off (default 0.8).
    pub off_below: Option<f64>,
    /// How far below off_below a sample of few events may fall and not be off, in
    /// counting-noise widths of 1 / sqrt(events in both gates) (default 1; 0 holds every sample
    /// to the same line).
    pub noise_widths: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FitRuleArgs {
    /// The population whose rule to fit, by its gate names.
    pub population: String,
    /// Whole rules to try beside the rule as it stands, each {"parameter", "bound",
    /// "measured_on", "rule": {"kind", ...}} - see rule_guide. Leave out to try the default
    /// settings for the rule's kind.
    pub candidates: Option<serde_json::Value>,
    /// Also try the default settings: every combination of a few values of each setting that
    /// matters for the rule's kind, around the rule as it stands - or, with no rule, around
    /// each candidate (default: true when no candidates are given, false when they are).
    pub defaults: Option<bool>,
    /// How many candidates to show, best first (default 10, at most 64).
    pub shown: Option<usize>,
    /// "typical" (default): the highest typical agreement first, a tie going to the fewest
    /// samples off; or "off": the fewest samples off first, then the highest typical.
    pub rank_by: Option<String>,
    /// Typical agreements no further apart than this are a tie (default 0.02).
    pub tie_within: Option<f64>,
    /// Rank on half the specimens and check on the other half, when there are eight or more
    /// (default true).
    pub split: Option<bool>,
    /// As score_rules: the agreement below which a sample is off (default 0.8).
    pub off_below: Option<f64>,
    /// As score_rules: how far below off_below a sample of few events may fall (default 1).
    pub noise_widths: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PickRuleArgs {
    /// The population whose gate to pick a rule for, by its gate names. Leave out for every
    /// gate a rule places, other than from another gate.
    pub population: Option<String>,
    /// As fit_rule: "typical" (default) or "off".
    pub rank_by: Option<String>,
    /// As fit_rule: typical agreements no further apart than this are a tie (default 0.02).
    pub tie_within: Option<f64>,
    /// As fit_rule: rank on half the specimens and check on the other half (default true).
    pub split: Option<bool>,
    /// The lowest and highest share of each specimen's FMX events above the line the user
    /// accepts, as two shares - [0.005, 0.01] for 0.5% to 1%: a band read on the FMX at it is
    /// tried first, as it is. [] tries none; leave out for what the user last set in the app,
    /// if anything (pick_settings says).
    pub fmx_band: Option<Vec<f64>>,
    /// A sample agreeing less than this with its hand gate is off. Leave out for what the
    /// user last set in the app - 0.95 unless they changed it.
    pub off_below: Option<f64>,
    /// As score_rules: how far below off_below a sample of few events may fall. Leave out for
    /// what the user last set - 1 unless they changed it.
    pub noise_widths: Option<f64>,
    /// A rule passes with no more than this share of its samples off. Leave out for what the
    /// user last set - 0.1 unless they changed it.
    pub most_off: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GateProfileArgs {
    /// The population whose gate to profile, by its gate names. Leave out for every gate, a
    /// line each.
    pub population: Option<String>,
    /// How many specimens to read, spread evenly through the dataset (default 12, at most 60).
    pub specimens: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GatePictureArgs {
    /// The population whose gate to draw, by its gate names.
    pub population: String,
    /// Samples to draw, by words of their file names or metadata. Leave out to draw the ones
    /// most worth seeing: the reference the gate's rule reads, and for each sample type where
    /// the gate holds least, most and most typically.
    pub samples: Option<String>,
    /// How many plots at most (default 6, at most 9).
    pub tiles: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RuleGuide {
    /// One rule, by its kind or name, e.g. 'TailFraction' or 'above the negative'. Leave out
    /// for how to choose between them and each in a line.
    pub rule: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadPositioningCode {
    /// A file from the list explain_gate_positioning gives, e.g. 'gate_rules/autogate.rs'.
    /// Leave out, with no search, for the list.
    pub path: Option<String>,
    /// The first line to read, from 1.
    pub from: Option<usize>,
    /// The last line to read. At most 400 lines come back at once.
    pub to: Option<usize>,
    /// Text to find in every readable file, e.g. 'fn slide_to_capture'. Answers with each
    /// matching line's file and number.
    pub search: Option<String>,
}

/// Rule changes, as the tools take them.
/// The score settings asked for, the defaults where none are.
fn score_settings(off_below: Option<f64>, noise_widths: Option<f64>) -> ScoreSettings {
    let defaults = ScoreSettings::default();
    ScoreSettings {
        off_below: off_below.unwrap_or(defaults.off_below),
        noise_widths: noise_widths.unwrap_or(defaults.noise_widths),
    }
}

/// Candidate rules as the tools give them.
fn candidate_rules(value: serde_json::Value) -> Result<Vec<GateRule>, Refusal> {
    serde_json::from_value(value).map_err(|e| Refusal::Failed {
        reason: format!(
            "the candidates could not be read ({e}): give a list of whole rules, each the rule \
             part of {RULE_CHANGES}"
        ),
    })
}

/// The ranking asked for, the defaults where none is.
fn fit_settings(
    rank_by_name: Option<&str>,
    tie_within: Option<f64>,
    split: Option<bool>,
) -> Result<FitSettings, Refusal> {
    let defaults = FitSettings::default();
    Ok(FitSettings {
        rank_by: rank_by(rank_by_name)?,
        tie_within: tie_within.unwrap_or(defaults.tie_within),
        split: split.unwrap_or(defaults.split),
    })
}

/// The ordering asked for by name, typical agreement where none is.
fn rank_by(name: Option<&str>) -> Result<RankBy, Refusal> {
    match name.map(str::trim) {
        None | Some("typical") => Ok(RankBy::Typical),
        Some("off") => Ok(RankBy::Off),
        Some(other) => Err(Refusal::Failed {
            reason: format!("rank_by is \"typical\" or \"off\" - not \"{other}\""),
        }),
    }
}

/// The band on the FMX asked for: `given`, none for an empty one, or `kept`
/// when none is given.
fn fmx_band(
    given: Option<&[f64]>,
    kept: Option<(f64, f64)>,
) -> Result<Option<(f64, f64)>, Refusal> {
    match given {
        None => Ok(kept),
        Some([]) => Ok(None),
        Some(&[lowest, highest]) => Ok(Some((lowest, highest))),
        Some(other) => Err(Refusal::Failed {
            reason: format!(
                "fmx_band is two shares, or [] for no band on the FMX - not {other:?}"
            ),
        }),
    }
}

const RULE_CHANGES: &str = "a list of {\"target\": {\"gate\": \"CD69+\", \"parent\": \"CD4+\" or      null}, \"rule\": {\"parameter\": \"CD69\", \"bound\": \"Above\" or \"Below\",      \"measured_on\": \"Itself\" or {\"Partner\": \"FMX\"} or {\"File\": \"<file id>\"},      \"rule\": {\"kind\": \"TailFraction\", \"band\": [0.002, 0.005]}}} - the rule kinds and their      fields are in explain_gate_positioning, section 8, and rule_guide";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplayRules {
    /// Rules to try in place of the ones the runs used: a list of {"target": {"gate", "parent"},
    /// "rule": {"parameter", "bound", "measured_on", "rule": {"kind", ...}}}. Leave out to
    /// replay the rules as they ran.
    pub rule_changes: Option<serde_json::Value>,
    /// Which reviewed runs: 'workspace' (its last run), 'library' (every run in the review
    /// library) or 'both' (the default).
    pub scope: Option<String>,
    /// One gate's cases only, by its name or 'gate of parent', e.g. 'CD69+'.
    pub gate: Option<String>,
    /// How many cases to list, most telling first (default 60, at most 500). The totals always
    /// count every case.
    pub max_cases: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplayCase {
    /// The case, exactly as replay_rules names it.
    pub case: String,
    /// The same rule changes given to replay_rules, if any.
    pub rule_changes: Option<serde_json::Value>,
    /// The same scope given to replay_rules, if any.
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateRule {
    /// The gate's name, e.g. 'CD69+'.
    pub gate: String,
    /// The parent, when the rule is for the gate under that parent only - as the part after
    /// 'of' in list_populations' rule_target: the shortest path that names only that parent,
    /// e.g. 'CD4+' or 'CD161+Va7.2+ / CD4+CD8-'.
    pub parent: Option<String>,
    /// The whole rule: {"parameter", "bound", "measured_on", "rule": {"kind", ...}}, as in a
    /// replay's rule_changes.
    pub rule: serde_json::Value,
}

fn rule_changes(
    given: Option<serde_json::Value>,
) -> Result<Vec<clingate_core::review::replay::RuleChange>, Refusal> {
    match given {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value).map_err(|e| Refusal::Failed {
            reason: format!("the rule changes could not be read ({e}): give {RULE_CHANGES}"),
        }),
    }
}

fn scope(given: Option<String>) -> Result<clingate_core::session::ReplayScope, Refusal> {
    let key = given.unwrap_or_default();
    clingate_core::session::ReplayScope::from_key(&key).ok_or_else(|| Refusal::Failed {
        reason: format!("{key} is not a scope: 'workspace', 'library' or 'both'"),
    })
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
    /// Reads the samples' events: only when the user asks for it.
    #[tool(annotations(read_only_hint = true))]
    async fn population_stats(&self, Parameters(args): Parameters<PopulationStats>) -> String {
        self.run(move |s| s.population_stats(&args.population, &args.samples))
            .await
    }

    /// How a population's parent is spread on one parameter in one sample - percentiles and
    /// a histogram - and where the population's gate edges sit on it, with the share of the
    /// parent between them. In the units the plots are drawn in.
    /// Reads the samples' events: only when the user asks for it.
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
    /// Reads the samples' events: only when the user asks for it.
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

    /// Assess the last applied rules run: which placements have their gate 2 or more of the
    /// parent's IQRs from where their peers - the other samples of the same kind that the rule
    /// placed confidently - put theirs, or that the rule was unsure of, each with its reasons in
    /// words, worst first; each gate across the run in a
    /// line; and the placements the run could not make, those refused on every sample first.
    /// Reads no files: it works from the run the workspace keeps.
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

    /// Mark a flagged placement of the last run as looking right - the flag was wrong - or take
    /// the mark back, as the app's Looks right button does. It moves the placement from "needs a
    /// look" to "passed", and the review records that the flag was cleared. Only when the user
    /// says so.
    #[tool]
    async fn mark_looks_right(&self, Parameters(args): Parameters<MarkLooksRight>) -> String {
        self.run(move |s| s.mark_looks_right(&args.population, &args.sample, args.looks_right))
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

    /// Exactly how the rules position gates, in words: which file of a specimen is gated and
    /// which is read, each rule kind step by step with its constants, how confidence is scored,
    /// how a run is reviewed and replayed, known behaviours worth discussing, and the rules
    /// file's format. Also lists the source files read_positioning_code can show. Needs no
    /// workspace.
    #[tool(annotations(read_only_hint = true))]
    async fn explain_gate_positioning(&self) -> String {
        use clingate_core::review::explain;
        ok(serde_json::json!({
            "description": explain::HOW_RULES_POSITION_GATES,
            "readable_source": explain::contents()
                .into_iter()
                .map(|(path, holds, lines)| serde_json::json!({
                    "path": path, "holds": holds, "lines": lines
                }))
                .collect::<Vec<_>>(),
        }))
    }

    /// A guide to the gate rules, for choosing one for a gate or working out why one misplaced
    /// a gate. With no rule: how to choose between them by what the data looks like, which
    /// file a rule reads, the settings they share, and each rule in a line. With a rule: what
    /// it is for and not for, how it works step by step, every setting, its traps, and what
    /// its confidence says. Needs no workspace.
    #[tool(annotations(read_only_hint = true))]
    async fn rule_guide(&self, Parameters(args): Parameters<RuleGuide>) -> String {
        use clingate_core::gate_rules::guide;
        match args.rule.filter(|r| !r.trim().is_empty()) {
            None => ok(serde_json::json!({
                "choosing": guide::CHOOSING,
                "rules": guide::GUIDES
                    .iter()
                    .map(|g| serde_json::json!({"kind": g.key, "name": g.name, "in_a_line": g.summary}))
                    .collect::<Vec<_>>(),
            })),
            Some(asked) => match guide::find(&asked) {
                Some(g) => ok(serde_json::json!({"kind": g.key, "name": g.name, "guide": g.text})),
                None => failed(format!(
                    "{asked} is not a rule; the rules are: {}",
                    guide::GUIDES
                        .iter()
                        .map(|g| format!("{} ({})", g.name, g.key))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            },
        }
    }

    /// Try one to four candidate rules for one population's gate on the workspace's files as
    /// they are, moving nothing: for each, what the gate would hold on every sample, summed up
    /// by sample type, beside what it holds as the gates stand now, with what each rule was
    /// unsure of or could not place. Each file is read once for all the candidates.
    /// Reads the samples' events: only when the user asks for it.
    #[tool(annotations(read_only_hint = true))]
    async fn try_rules(&self, Parameters(args): Parameters<TryRules>) -> String {
        self.run(move |s| {
            s.try_rules(&args.population, &candidate_rules(args.candidates)?, args.max_rows)
        })
        .await
    }

    /// Score the rules against the gating drawn by hand: every rule, or one population's, run
    /// on the files with each gate under its parent as drawn, and per gate and sample the
    /// events the rule's gate and the hand gate both hold - their agreement, how much of the
    /// hand gate the rule catches and how much it holds beyond it, and whether the sample is
    /// off, below a line a sample of few events may fall further below -
    /// with how far the rule's gate sits from the hand gate and, for a rule that moves one
    /// edge, how far it moves it. Each gate in a line, the most samples off first, then the
    /// samples least in agreement. Moves nothing. Reads the samples' events: only when the
    /// user asks for it.
    #[tool(annotations(read_only_hint = true))]
    async fn score_rules(&self, Parameters(args): Parameters<ScoreRulesArgs>) -> String {
        let settings = score_settings(args.off_below, args.noise_widths);
        self.run(move |s| s.score_rules(args.population.as_deref(), args.max_rows, settings))
            .await
    }

    /// Search one population's rule settings for those closest to the gating drawn by hand:
    /// the rule as it stands, the candidates given and - by default when none are given -
    /// every combination of a few values of each setting that matters for its kind, each
    /// scored as score_rules scores a rule and ranked. With eight specimens or more, ranked
    /// on half of them and checked on the other half. Each candidate's score on both halves,
    /// its place ranked by typical agreement and by samples off, and whether it is the best
    /// or tied with it. The best, those tied with it and the rule as it stands are kept in the
    /// workspace, for the app's Gallery tab to show over the user's gate. Moves no gate.
    /// Reads the samples' events: only when the user asks for it.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn fit_rule(&self, Parameters(args): Parameters<FitRuleArgs>) -> String {
        let settings = score_settings(args.off_below, args.noise_widths);
        self.run(move |s| {
            let candidates = match args.candidates {
                Some(value) => candidate_rules(value)?,
                None => Vec::new(),
            };
            let fit = fit_settings(args.rank_by.as_deref(), args.tie_within, args.split)?;
            let ask = FitAsk {
                defaults: args.defaults.unwrap_or(candidates.is_empty()),
                candidates,
                shown: args.shown,
            };
            s.fit_rule(&args.population, ask, settings, fit)
        })
        .await
    }

    /// The settings pick_rule uses for anything left out: those the user last chose in the
    /// app - the band on the FMX it tries first (none if not set), the agreement below which a
    /// sample is off, and the share of samples that may be off - or the defaults. Reads no
    /// events.
    #[tool(annotations(read_only_hint = true))]
    async fn pick_settings(&self) -> String {
        self.run(|s| Ok(s.pick_settings())).await
    }

    /// Pick a rule for one population's gate, or every gate a rule places other than from
    /// another gate, trying the kinds of rule in the user's order of preference and taking
    /// the first that passes: a band read on each specimen's FMX at the range the user
    /// accepts, as it is; above the negative; the valley or a smear; a band read on each
    /// sample; and last the phenotype on the plot's two axes - the negative, the valley and
    /// the phenotype calibrated on one sample gated by hand. A rule passes when no more than
    /// most_off of its samples agree less than off_below with the hand gate. Every kind but the
    /// phenotype is tried as the hand gating starts it, and the first in order that passes has
    /// its settings searched; when none passes, each kind's settings are searched in turn; when still none
    /// passes, the closest of everything tried is shown, not passed - flag it to the user. Per
    /// gate: the rule beside the rule as it stands, whether it passed, how each kind did, and
    /// the others tied with it - kept for the app's Gallery tab. The settings the user last
    /// chose in the app are used for anything left out. Changes no rule and moves no gate.
    /// Reads the samples' events: only when the user asks for it.
    #[tool(annotations(read_only_hint = false, destructive_hint = false))]
    async fn pick_rule(&self, Parameters(args): Parameters<PickRuleArgs>) -> String {
        self.run(move |s| {
            let fit = fit_settings(args.rank_by.as_deref(), args.tie_within, args.split)?;
            let kept = s.pick_settings();
            let settings = PickSettings {
                fmx_band: fmx_band(args.fmx_band.as_deref(), kept.fmx_band)?,
                score: ScoreSettings {
                    off_below: args.off_below.unwrap_or(kept.score.off_below),
                    noise_widths: args.noise_widths.unwrap_or(kept.score.noise_widths),
                },
                most_off: args.most_off.unwrap_or(kept.most_off),
            };
            s.pick_rules(args.population.as_deref(), settings, fit)
        })
        .await
    }

    /// What a gate's populations look like across the dataset, on each of its two markers, by
    /// sample type: each sample's shape class (separate, shoulder, smear, merged, negative
    /// only, several peaks), how the negative shifts and changes shape, where the gate sits now
    /// against the negative and what it holds, and on the full stain how much lies above its
    /// FMX's top. Reads a spread of specimens; moves nothing. For choosing a rule with the
    /// user, only when they ask for the data to be read.
    #[tool(annotations(read_only_hint = true))]
    async fn gate_profile(&self, Parameters(args): Parameters<GateProfileArgs>) -> String {
        self.run(move |s| s.gate_profile(args.population.as_deref(), args.specimens))
            .await
    }

    /// A picture of a gate drawn on several samples, as the app draws them, with the gate's
    /// outline in: the samples named, or the most telling ones. Read the plots as a person
    /// would - where the negative ends, whether the positive separates or smears, whether the
    /// gate sits alike on every sample. The captions say which plot is which, left to right,
    /// top to bottom, and what the gate holds on each.
    /// Reads the samples' events: only when the user asks for it.
    #[tool(annotations(read_only_hint = true))]
    async fn gate_picture(
        &self,
        Parameters(args): Parameters<GatePictureArgs>,
    ) -> rmcp::model::CallToolResult {
        use base64::Engine as _;
        use rmcp::model::{CallToolResult, ContentBlock};
        let drawn = self
            .with_session(move |s| {
                s.gate_picture(&args.population, args.samples.as_deref(), args.tiles)
            })
            .await;
        match drawn {
            Ok(Ok(picture)) => CallToolResult::success(vec![
                ContentBlock::image(
                    base64::engine::general_purpose::STANDARD.encode(&picture.png),
                    "image/png",
                ),
                ContentBlock::text(ok(serde_json::json!({
                    "plots_left_to_right_top_to_bottom": picture.tiles,
                }))),
            ]),
            Ok(Err(refusal)) => CallToolResult::success(vec![ContentBlock::text(refused(refusal))]),
            Err(reason) => CallToolResult::success(vec![ContentBlock::text(failed(reason))]),
        }
    }

    /// The source code that positions gates, scores them and replays them - exactly what the
    /// program runs. Give a path and a line range to read (400 lines at most at once), or a
    /// search to find where something is. Needs no workspace.
    #[tool(annotations(read_only_hint = true))]
    async fn read_positioning_code(
        &self,
        Parameters(args): Parameters<ReadPositioningCode>,
    ) -> String {
        use clingate_core::review::explain;
        if let Some(text) = args.search.filter(|t| !t.trim().is_empty()) {
            return ok(explain::search(&text, 60));
        }
        match args.path {
            Some(path) => match explain::read(&path, args.from, args.to) {
                Ok(part) => ok(part),
                Err(reason) => failed(reason),
            },
            None => ok(explain::contents()
                .into_iter()
                .map(|(path, holds, lines)| {
                    serde_json::json!({"path": path, "holds": holds, "lines": lines})
                })
                .collect::<Vec<_>>()),
        }
    }

    /// Replay reviewed rules runs on the events each kept: with the rules they ran with, and
    /// with any rule changes given. Says per gate and per placement whether a change fixed,
    /// broke or left wrong where the gate went, against where the reviewer said it belongs.
    /// Changes nothing.
    #[tool(annotations(read_only_hint = true))]
    async fn replay_rules(&self, Parameters(args): Parameters<ReplayRules>) -> String {
        self.run(move |s| {
            let changes = rule_changes(args.rule_changes)?;
            let scope = scope(args.scope)?;
            s.replay_rules(&changes, scope, args.gate.as_deref(), args.max_cases)
        })
        .await
    }

    /// One case of a replay in full: the sample's population - and the file the rule read,
    /// when it read another - as histograms on the rule's parameter, where the gate started,
    /// where the run, the replay and the reviewer put the line, and every confidence component.
    #[tool(annotations(read_only_hint = true))]
    async fn replay_case(&self, Parameters(args): Parameters<ReplayCase>) -> String {
        self.run(move |s| {
            let changes = rule_changes(args.rule_changes)?;
            let scope = scope(args.scope)?;
            s.replay_case(&changes, scope, &args.case)
        })
        .await
    }

    /// Write one rule into the workspace's rules file, replacing the rule for the same gate
    /// and parent: the file the Gate Rules tab edits. Only when the user says to, after they
    /// have seen what replay_rules says it fixes and breaks.
    #[tool(annotations(read_only_hint = false, destructive_hint = true))]
    async fn update_rule(&self, Parameters(args): Parameters<UpdateRule>) -> String {
        self.run(move |s| {
            let rule = serde_json::from_value(args.rule).map_err(|e| Refusal::Failed {
                reason: format!(
                    "the rule could not be read ({e}): give the rule part of {RULE_CHANGES}"
                ),
            })?;
            let target = clingate_core::gate_rules::rule_store::RuleTarget {
                gate: args.gate.trim().into(),
                parent: args
                    .parent
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .map(Into::into),
            };
            s.update_rule(clingate_core::review::replay::RuleChange { target, rule })
        })
        .await
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
