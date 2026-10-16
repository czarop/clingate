//! The features waiting for the user to try: how to try each, what to send
//! back, and - where it can be done without them - a check that tries it on
//! the open workspace and changes nothing. The app's Test features tab and
//! the tools for Claude (`check_features`) read the same list and run the
//! same checks; what the user says of each is kept in the workspace.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::gate_rules::rule_store::{Named, RuleStore};
use crate::gate_rules::run::RunInputs;
use crate::gates::GateState;
use crate::omiq::serialise::AxisSettings;

/// A feature to try.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Feature {
    pub id: &'static str,
    pub title: &'static str,
    /// Where it is: a tab, or the tools for Claude.
    pub place: &'static str,
    /// What to do, in order.
    pub steps: &'static [&'static str],
    /// What to send back.
    pub send_back: &'static str,
    /// The check that tries it on the open workspace, if one can.
    pub check: Option<Check>,
}

/// A check that tries a feature on the open workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Every rule copied to each sibling population, as a dry run.
    SiblingCopies,
    /// A rectangle made a polygon, saved and read back.
    PolygonSaved,
    /// Every name the rules hold, as a run reads it.
    RuleNames,
    /// The gating saved and read back, then saved again: the same file.
    GatingSaved,
}

/// What the features are, newest first.
pub const FEATURES: &[Feature] = &[
    Feature {
        id: "copy_rules",
        title: "Copying rules to sibling populations",
        place: "Gate Rules tab - \"Copy rules to sibling populations\"; copy_rules for Claude",
        steps: &[
            "Choose a population whose gates have rules (e.g. CD4+CD8-).",
            "Tick the populations to copy them to (e.g. CD4-CD8+, TCRgd+).",
            "Read the table: what would be copied, what is left out and why.",
            "Copy, then Save the rules, and run them.",
        ],
        send_back: "Anything copied that should not have been, anything left out that should \
                    have been copied (and the reason it gave), and how the copied rules placed \
                    their gates.",
        check: Some(Check::SiblingCopies),
    },
    Feature {
        id: "polygon_saved",
        title: "A rectangle made a polygon stays a polygon",
        place: "Gate editor - right-click a rectangle, \"Convert to polygon\"",
        steps: &[
            "Convert a rectangle gate to a polygon and Save.",
            "Close and reopen the workspace: it should still be a polygon.",
            "Export to Omiq and upload: it should be a polygon there too.",
            "Undo a conversion and Save: it should be a rectangle again.",
        ],
        send_back: "Which gate, and whether it was a polygon after reopening and in Omiq.",
        check: Some(Check::PolygonSaved),
    },
    Feature {
        id: "rule_names",
        title: "Rules naming a marker or a file name run as meant",
        place: "Gate Rules tab - Run; list_rules for Claude",
        steps: &[
            "Run the rules of a file that names a parameter by its marker (CD3) or a \
             reference by its file name.",
            "No gate should pause or be listed as placed on no sample for that reason.",
        ],
        send_back: "Any gate still refused with \"the rule positions ...\" or \"not one of this \
                    workspace's files\".",
        check: Some(Check::RuleNames),
    },
    Feature {
        id: "gating_saved",
        title: "Saving keeps every gate as it is",
        place: "Gate editor - Save, and Export to Omiq",
        steps: &["Save, close and reopen: every gate should be where and what it was."],
        send_back: "Any gate that changed shape, type, position or label after a save.",
        check: Some(Check::GatingSaved),
    },
    Feature {
        id: "rule_report",
        title: "A report on one rule's run, with every sample's plot",
        place: "Review tab - \"Report on this rule\"; rule_report for Claude",
        steps: &[
            "Run and apply the rules, then open the Review tab and choose a gate.",
            "Press \"Report on this rule\" and open the folder it names.",
            "Check the numbers in each plot's corner match the table, and that the flagged \
             and not-flagged pictures hold the right samples.",
        ],
        send_back: "The report's folder (zip it), and the sample numbers that look wrong.",
        check: None,
    },
    Feature {
        id: "test_report",
        title: "The test report, with the pick settings typed",
        place: "Gate Rules tab - \"Make a test report\"; test_report for Claude",
        steps: &[
            "Type pick settings in \"Pick the best rule\" without picking.",
            "Press \"Make a test report\": its Pick settings section should show what you \
             typed.",
        ],
        send_back: "The report file from test_reports.",
        check: None,
    },
    Feature {
        id: "pick_rule",
        title: "Picking the best rule for every gate",
        place: "Gate Rules tab - \"Pick the best rule\"; pick_rule for Claude",
        steps: &[
            "On a workspace gated by hand, set the FMX band and the cut-off, and pick.",
            "Step through each gate's closest picks on the Gallery tab, over your gate.",
            "Use the picks you agree with, and run them.",
        ],
        send_back: "A test report made after the pick, and the gates where the pick was wrong \
                    by eye.",
        check: None,
    },
    Feature {
        id: "claude_tools",
        title: "The tools for Claude in Claude Desktop",
        place: "Claude Desktop, with clingate-mcp added",
        steps: &[
            "Ask Claude to open the workspace, list the rules, and copy one population's \
             rules to another.",
            "Ask it for a rule report on one gate, and to explain a flagged sample.",
            "Check it writes rules only through update_rule or copy_rules, and asks before \
             writing.",
        ],
        send_back: "The conversation, and anything it did without asking.",
        check: None,
    },
    Feature {
        id: "compensation",
        title: "Compensation, round trip to Omiq",
        place: "Workspace tab - Compensation",
        steps: &[
            "Check the files land in the compensation groups expected.",
            "Answer the Omiq question; edit a value; Copy matrix for Omiq, paste into Omiq, \
             export again, load the new export.",
            "The plots should look as they did before the round trip.",
        ],
        send_back: "Any step where the plots changed when they should not have.",
        check: None,
    },
];

/// The feature `id` names.
pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.id == id)
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checked {
    pub passed: bool,
    /// What it found, a line each.
    pub said: Vec<String>,
}

/// The open workspace, as a check reads it.
pub struct CheckInputs<'a> {
    pub gates: &'a GateState,
    pub inputs: &'a RunInputs,
    pub axes: &'a AxisSettings,
    /// The rules as the rules file holds them.
    pub rules: &'a RuleStore,
}

/// Run `check` on the workspace. Changes nothing.
pub fn run_check(check: Check, on: &CheckInputs) -> Checked {
    match check {
        Check::SiblingCopies => sibling_copies(on),
        Check::PolygonSaved => polygon_saved(on),
        Check::RuleNames => rule_names(on),
        Check::GatingSaved => gating_saved(on),
    }
}

fn sibling_copies(on: &CheckInputs) -> Checked {
    use crate::gate_rules::siblings::{copy_rules, populations_with_rules, siblings_of};
    let mut said = Vec::new();
    for (from, from_name) in populations_with_rules(on.gates, on.rules) {
        for (to, to_name) in siblings_of(on.gates, &from) {
            let copy = copy_rules(on.gates, on.rules, &from, &to, false);
            said.push(format!(
                "{from_name} to {to_name}: {} would be copied, {} left out",
                copy.copied.len(),
                copy.not_copied.len()
            ));
            said.extend(
                copy.not_copied
                    .iter()
                    .map(|n| format!("  - {}: {}", n.target.describe(), n.reason)),
            );
        }
    }
    if said.is_empty() {
        said.push("no population with rules has a sibling with gates of the same names".into());
    }
    Checked { passed: true, said }
}

/// The workspace files' gating ids.
fn held(on: &CheckInputs) -> rustc_hash::FxHashSet<Arc<str>> {
    on.inputs
        .files
        .iter()
        .filter_map(|(name, _)| on.inputs.names.get(name).cloned())
        .collect()
}

/// `state` written as the gating file and read back.
fn saved_and_read(
    state: &GateState,
    on: &CheckInputs,
) -> Result<(serde_json::Value, GateState), String> {
    let written =
        crate::omiq::serialise::to_omiq_document(state, &on.inputs.metadata, &held(on), on.axes)
            .map_err(|e| format!("could not be written: {e}"))?;
    let path = std::env::temp_dir().join(format!(
        "clingate-check-{}-{:?}.omiqgt",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, written.to_string()).map_err(|e| e.to_string())?;
    let mut read = GateState::default();
    let loaded = read.upload_gates_from_file(path.clone(), &on.inputs.metadata, on.axes.clone());
    let _ = std::fs::remove_file(path);
    loaded.map_err(|e| format!("could not be read back: {e}"))?;
    Ok((written, read))
}

/// The first rectangle with four corners - one not open to the end of an
/// axis - made a polygon in a copy of the gating: its id and the copy.
fn first_rectangle_made_a_polygon(gates: &GateState) -> Option<(Arc<str>, GateState)> {
    use crate::gates::gate_single::rectangle_gate::RectangleGate;
    gates
        .placements()
        .filter_map(|(node, _)| gates.gate_for_node(node).cloned())
        .filter(|id| {
            gates
                .registered_gate(id)
                .is_some_and(|g| g.as_any().downcast_ref::<RectangleGate>().is_some())
        })
        .find_map(|id| {
            let mut copy = gates.clone();
            copy.convert_to_polygon(&id).ok()?;
            Some((id, copy))
        })
}

fn polygon_saved(on: &CheckInputs) -> Checked {
    let Some((id, converted)) = first_rectangle_made_a_polygon(on.gates) else {
        return Checked {
            passed: true,
            said: vec!["no rectangle gate with four corners to try it on".into()],
        };
    };
    let name = on
        .gates
        .population_name(&id)
        .map_or(id.to_string(), |n| n.to_string());
    let (written, read) = match saved_and_read(&converted, on) {
        Ok(saved) => saved,
        Err(why) => {
            return Checked {
                passed: false,
                said: vec![format!("{name}: the gating {why}")],
            };
        }
    };
    let kind = written["tree"]["filterContainers"][&*id]["defaultFilter"]["type"]
        .as_str()
        .unwrap_or("nothing")
        .to_string();
    let read_back_a_polygon = read
        .registered_gate(&id)
        .and_then(|g| g.get_gate_ref(Some(&id)).map(|g| g.geometry.clone()))
        .is_some_and(|g| matches!(g, flow_gates::GateGeometry::Polygon { .. }));
    Checked {
        passed: kind == "PolygonGate" && read_back_a_polygon,
        said: vec![format!(
            "{name}: made a polygon, saved as {kind} and read back {}",
            if read_back_a_polygon {
                "a polygon"
            } else {
                "not a polygon"
            }
        )],
    }
}

fn rule_names(on: &CheckInputs) -> Checked {
    let panel: Vec<&Arc<str>> = on.axes.values().map(|axis| &axis.param.fluoro).collect();
    let mut said = Vec::new();
    let mut passed = true;
    for (written, read) in on.rules.entries().iter().zip(on.inputs.rules.entries()) {
        let mut as_written = Vec::new();
        let mut written_rule = written.rule.clone();
        let _ = written_rule.rename(|named, name| {
            as_written.push((named, name.clone()));
            Ok::<_, ()>(name.clone())
        });
        let mut as_read = Vec::new();
        let mut read_rule = read.rule.clone();
        let _ = read_rule.rename(|_, name| {
            as_read.push(name.clone());
            Ok::<_, ()>(name.clone())
        });
        let gate = written.target.describe();
        for ((named, given), read) in as_written.into_iter().zip(as_read) {
            let (what, found) = match named {
                Named::Channel(what) => (what, panel.contains(&&read)),
                Named::File(what) => (what, on.inputs.metadata.contains_key(&read)),
            };
            if !found {
                passed = false;
                said.push(format!(
                    "{gate}: {what} {given} is not in this workspace - the run refuses it"
                ));
            } else if given != read {
                said.push(format!("{gate}: {what} {given} is read as {read}"));
            }
        }
    }
    if said.is_empty() {
        said.push("every name the rules hold is as a run reads it".into());
    }
    Checked { passed, said }
}

fn gating_saved(on: &CheckInputs) -> Checked {
    let once = saved_and_read(on.gates, on);
    let twice = once
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|(_, read)| saved_and_read(read, on));
    match (once, twice) {
        (Ok((first, _)), Ok((second, _))) if first == second => Checked {
            passed: true,
            said: vec!["saved, read back and saved again: the same file".into()],
        },
        (Ok((first, _)), Ok((second, _))) => {
            let differs = |document: &serde_json::Value| {
                document["tree"]["filterContainers"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            };
            let (a, b) = (differs(&first), differs(&second));
            let changed: Vec<String> = a
                .iter()
                .filter(|(id, container)| b.get(*id) != Some(container))
                .map(|(id, container)| container["name"].as_str().unwrap_or(id).to_string())
                .collect();
            Checked {
                passed: false,
                said: vec![format!(
                    "saved again after reading back, these changed: {}",
                    if changed.is_empty() {
                        "the tree".to_string()
                    } else {
                        changed.join(", ")
                    }
                )],
            }
        }
        (Err(why), _) | (_, Err(why)) => Checked {
            passed: false,
            said: vec![format!("the gating {why}")],
        },
    }
}

/// How a feature went for the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tried {
    #[default]
    NotYet,
    Works,
    Problem,
}

impl Tried {
    pub const ALL: [Tried; 3] = [Tried::NotYet, Tried::Works, Tried::Problem];

    pub fn label(self) -> &'static str {
        match self {
            Tried::NotYet => "Not tried",
            Tried::Works => "Works",
            Tried::Problem => "Problem",
        }
    }
}

/// What the user says of one feature, and what its check last found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub tried: Tried,
    pub note: String,
    pub checked: Option<Checked>,
}

/// Every feature's feedback, by id.
pub type Feedbacks = BTreeMap<String, Feedback>;

/// Where the feedback is kept in a workspace.
pub fn feedback_file(folder: &Path) -> PathBuf {
    folder
        .join(crate::test_report::FOLDER)
        .join("feature_feedback.json")
}

/// The workspace's feedback; none where none is kept, or it cannot be read.
pub fn load_feedback(folder: &Path) -> Feedbacks {
    std::fs::read_to_string(feedback_file(folder))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_feedback(folder: &Path, feedback: &Feedbacks) -> std::io::Result<()> {
    let path = feedback_file(folder);
    crate::workspace::make_parent(&path)?;
    std::fs::write(path, serde_json::to_string_pretty(feedback)?)
}

/// Every feature with what the user said and what its check found, as
/// Markdown to send on.
pub fn feedback_markdown(feedback: &Feedbacks) -> String {
    let mut out = String::from("# clingate feature feedback\n");
    for feature in FEATURES {
        let given = feedback.get(feature.id).cloned().unwrap_or_default();
        out.push_str(&format!(
            "\n## {}\n\n- Where: {}\n- Tried: {}\n",
            feature.title,
            feature.place,
            given.tried.label()
        ));
        if !given.note.trim().is_empty() {
            out.push_str(&format!("- Said: {}\n", given.note.trim()));
        }
        if let Some(checked) = &given.checked {
            out.push_str(&format!(
                "- Check: {}\n",
                if checked.passed { "passed" } else { "FAILED" }
            ));
            for line in &checked.said {
                out.push_str(&format!("  - {line}\n"));
            }
        }
    }
    out
}
