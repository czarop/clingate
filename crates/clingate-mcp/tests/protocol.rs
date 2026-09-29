//! The server as Claude Desktop runs it: the real binary, spoken to over
//! stdin and stdout, one JSON-RPC message per line.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use clingate_core::file_load_tests::{scratch, write_fcs_rows};
use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};
use serde_json::{Value, json};

const FLUORESCENCE: [&str; 8] = [
    "BUV661-A",
    "BV785-A",
    "Alexa Fluor 700-A",
    "BUV737-A",
    "BUV805-A",
    "BUV563-A",
    "Alexa Fluor 647-A",
    "Vio Bright 423-A",
];

fn events(seed: u64) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let scatter = Uniform::new(1_000.0f32, 4_000_000.0).unwrap();
    let negative = Normal::new(0.0f32, 300.0).unwrap();
    let positive = Normal::new(40_000.0f32, 8_000.0).unwrap();
    (0..2_000)
        .map(|i| {
            let mut row = vec![scatter.sample(&mut rng), scatter.sample(&mut rng)];
            for c in 0..FLUORESCENCE.len() {
                row.push(if (i + c) % 3 == 0 {
                    positive.sample(&mut rng)
                } else {
                    negative.sample(&mut rng)
                });
            }
            row
        })
        .collect()
}

/// Two samples, metadata, scaling, and the core's gating fixture.
fn workspace(name: &str) -> PathBuf {
    // A folder of its own: tests run side by side, and one that writes into
    // a shared folder changes what the others open.
    let dir = scratch(&format!("mcp-protocol-{name}"));
    let mut channels: Vec<(&str, Option<&str>)> = vec![("FSC-A", None), ("SSC-A", None)];
    channels.extend(FLUORESCENCE.iter().map(|c| (*c, None)));
    write_fcs_rows(&dir.join("sample1_FMX.fcs"), &channels, &events(1), &[]);
    write_fcs_rows(&dir.join("sample2_FS.fcs"), &channels, &events(2), &[]);
    std::fs::write(
        dir.join("metadata.csv"),
        "OmiqID,Filename,test,Type,SampleType\n\
         sample1,sample1_FMX.fcs,one,one,FMX\n\
         sample2,sample2_FS.fcs,two,two,FS\n",
    )
    .unwrap();
    let mut scaling = String::from(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),0,0,4194304,0,0\n\
         SSC-A,,None (linear),0,0,4194304,0,0\n",
    );
    for c in FLUORESCENCE {
        scaling.push_str(&format!("{c},{c},Arcsinh,6000,-2000,200000,0,0\n"));
    }
    std::fs::write(dir.join("scaling.csv"), scaling).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../clingate-core/tests/fixtures/quadrant_with_boolean_child.omiqgt"),
        dir.join("gating.omiqgt"),
    )
    .unwrap();
    dir
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_clingate-mcp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut server = Self {
            child,
            stdin,
            stdout,
            next: 0,
        };
        let init = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "clingate");
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("ask the user")
        );
        server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Send a request and read up to its response. Every line on stdout has
    /// to be a JSON-RPC message: anything else there would break a client.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "the server closed"
            );
            let message: Value = serde_json::from_str(&line).unwrap_or_else(|e| {
                panic!("stdout carried something that is not JSON ({e}): {line}")
            });
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    /// Call a tool; its answer, parsed.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no answer from {tool}: {response}"));
        serde_json::from_str(text).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn claude_desktop_can_open_a_workspace_and_ask_about_it() {
    let folder = workspace("t1");
    let mut server = Server::start();

    let tools = server.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for wanted in [
        "open_workspace",
        "workspace_overview",
        "find_samples",
        "list_populations",
        "population_stats",
        "distribution",
        "answer_omiq_compensation",
        "list_parameters",
        "gate_details",
        "compare_samples",
        "list_rules",
        "preview_rules",
        "apply_rule_placements",
        "save_gating",
        "export_gating",
        "undo",
        "redo",
        "revert_to_saved",
        "restore_unsaved_changes",
        "discard_unsaved_changes",
        "report_placement",
        "mark_run_reviewed",
        "assess_run",
        "compare_to_peers",
        "mark_looks_right",
        "explain_gate_positioning",
        "rule_guide",
        "try_rules",
        "gate_profile",
        "gate_picture",
        "read_positioning_code",
        "replay_rules",
        "replay_case",
        "update_rule",
    ] {
        assert!(names.contains(&wanted), "{wanted} missing from {names:?}");
    }

    // Nothing open yet: the answer says to ask for a folder.
    let early = server.call(
        "population_stats",
        json!({"population": "Tmem", "samples": "all"}),
    );
    assert_eq!(early["outcome"], "failed");
    assert!(early["reason"].as_str().unwrap().contains("ask the user"));

    let opened = server.call(
        "open_workspace",
        json!({"folder": folder.to_str().unwrap()}),
    );
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["result"]["samples"], 2);
    assert_eq!(opened["result"]["parts"]["gating"]["state"], "loaded");

    let stats = server.call(
        "population_stats",
        json!({"population": "Tmem", "samples": "fmx"}),
    );
    assert_eq!(stats["outcome"], "ok", "{stats}");
    let rows = stats["result"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sample"], "sample1_FMX.fcs");
    assert_eq!(rows[0]["total_events"], 2_000);

    // A name that is nearly right is a question, with suggestions.
    let asked = server.call("find_samples", json!({"query": "fm"}));
    assert_eq!(asked["outcome"], "needs_clarification", "{asked}");
    assert!(!asked["suggestions"].as_array().unwrap().is_empty());
}

#[test]
fn gates_are_described_compared_and_saved_only_as_asked() {
    let folder = workspace("t2");
    let mut server = Server::start();
    let opened = server.call(
        "open_workspace",
        json!({"folder": folder.to_str().unwrap()}),
    );
    assert_eq!(opened["outcome"], "ok", "{opened}");

    let parameters = server.call("list_parameters", json!({}));
    assert_eq!(
        parameters["result"].as_array().unwrap().len(),
        2 + FLUORESCENCE.len()
    );

    let gate = server.call(
        "gate_details",
        json!({"population": "Tmem", "sample": "fs"}),
    );
    assert_eq!(gate["outcome"], "ok", "{gate}");
    assert_eq!(gate["result"]["sample"], "sample2_FS.fcs");
    let parameter = gate["result"]["parameters"][0]
        .as_str()
        .unwrap()
        .to_string();

    let compared = server.call(
        "compare_samples",
        json!({"population": "Tmem", "parameter": parameter, "samples": "all"}),
    );
    assert_eq!(compared["outcome"], "ok", "{compared}");
    assert_eq!(compared["result"]["rows"].as_array().unwrap().len(), 2);

    // No rules in this workspace, and nothing to apply.
    assert_eq!(server.call("list_rules", json!({}))["outcome"], "failed");
    assert_eq!(
        server.call("apply_rule_placements", json!({}))["outcome"],
        "failed"
    );

    // Nothing edited: nothing to undo.
    assert_eq!(server.call("undo", json!({}))["outcome"], "failed");

    // The gating file already there is not replaced unasked.
    let refused = server.call("export_gating", json!({"file_name": "gating"}));
    assert_eq!(refused["outcome"], "failed", "{refused}");
    let exported = server.call("export_gating", json!({"file_name": "from claude"}));
    assert_eq!(exported["outcome"], "ok", "{exported}");
    assert!(folder.join("from claude.omiqgt").is_file());

    // Save writes the working copy where the app saves it.
    let saved = server.call("save_gating", json!({}));
    assert_eq!(saved["outcome"], "ok", "{saved}");
    assert!(folder.join("clingate_gating.omiqgt").is_file());
    assert!(folder.join("clingate_scaling.csv").is_file());
}

#[test]
fn a_folder_that_is_not_there_is_said_so() {
    let mut server = Server::start();
    let answer = server.call("open_workspace", json!({"folder": "/no/such/folder"}));
    assert_eq!(answer["outcome"], "failed");
    assert!(answer["reason"].as_str().unwrap().contains("not a folder"));
}

/// The protocol workspace with a rule for Tmem, as a person would have
/// saved one on the Gate Rules tab.
fn workspace_with_rules(name: &str) -> PathBuf {
    use clingate_core::gate_rules::rule::{Rule, TailFractionRule};
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let dir = workspace(name);
    let parameter = clingate_core::session::Session::open(&dir)
        .unwrap()
        .gate("Tmem", None)
        .unwrap()
        .parameters[0]
        .clone();
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    store.insert(
        RuleTarget::named("Tmem"),
        GateRule {
            parameter: parameter.into(),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
        },
    );
    store
        .save(&clingate_core::workspace::rules_file(&dir))
        .unwrap();
    dir
}

#[test]
fn a_rules_run_is_reviewed_over_the_protocol_as_in_the_app() {
    let folder = workspace_with_rules("review");
    let mut server = Server::start();
    let opened = server.call(
        "open_workspace",
        json!({"folder": folder.to_str().unwrap()}),
    );
    assert_eq!(opened["outcome"], "ok", "{opened}");

    // Nothing to review until a run is applied.
    assert_eq!(server.call("assess_run", json!({}))["outcome"], "failed");
    assert_eq!(
        server.call("mark_run_reviewed", json!({}))["outcome"],
        "failed"
    );
    assert_eq!(server.call("preview_rules", json!({}))["outcome"], "ok");
    let applied = server.call("apply_rule_placements", json!({}));
    assert_eq!(applied["outcome"], "ok", "{applied}");
    assert!(folder.join("reviews").join("rules_run.json").is_file());

    // The piles, and every placement in one of them.
    let assessed = server.call("assess_run", json!({}));
    assert_eq!(assessed["outcome"], "ok", "{assessed}");
    let result = &assessed["result"];
    assert_eq!(result["placements"], 2);
    let piles: u64 = result["piles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p[1].as_u64().unwrap())
        .sum();
    assert_eq!(piles, 2);
    assert_eq!(result["gates"][0]["gate"], "Tmem");

    let compared = server.call(
        "compare_to_peers",
        json!({"population": "Tmem", "sample": "fmx"}),
    );
    assert_eq!(compared["outcome"], "ok", "{compared}");
    assert_eq!(
        compared["result"]["percentiles"].as_array().unwrap().len(),
        9
    );

    let looks = server.call(
        "mark_looks_right",
        json!({"population": "Tmem", "sample": "fmx", "looks_right": true}),
    );
    assert_eq!(looks["outcome"], "ok", "{looks}");
    assert!(folder.join("reviews").join("looks_right.json").is_file());
    let refused = server.call(
        "mark_looks_right",
        json!({"population": "Tmem", "sample": "sample9", "looks_right": true}),
    );
    // A sample it cannot find is a question, with what it could mean.
    assert_eq!(refused["outcome"], "needs_clarification", "{refused}");
    assert!(refused["suggestions"].as_array().unwrap().len() >= 2);

    // A report, in the user's words; a problem not on the list is refused.
    let wrong = server.call(
        "report_placement",
        json!({"population": "Tmem", "sample": "fs", "problem": "wonky"}),
    );
    assert_eq!(wrong["outcome"], "failed", "{wrong}");
    let reported = server.call(
        "report_placement",
        json!({"population": "Tmem", "sample": "fs", "problem": "too_high", "note": "misses the dim ones"}),
    );
    assert_eq!(reported["outcome"], "ok", "{reported}");
    assert_eq!(
        reported["result"]["problem"],
        "too high - positives left out"
    );
    let reports = clingate_core::review::report::reports_in(&folder);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].1.note, "misses the dim ones");

    let reviewed = server.call("mark_run_reviewed", json!({}));
    assert_eq!(reviewed["outcome"], "ok", "{reviewed}");
    assert_eq!(reviewed["result"]["reported"], 1);
    assert_eq!(reviewed["result"]["accepted"], 1);
    let review: clingate_core::review::RunReview = serde_json::from_str(
        &std::fs::read_to_string(folder.join("reviews").join("review.json")).unwrap(),
    )
    .unwrap();
    // The flag cleared over the protocol is in the review.
    assert!(
        review
            .placements
            .iter()
            .any(|p| p.placed.sample.id == "sample1" && p.flag.looked_right)
    );
}

#[test]
fn how_gates_are_positioned_is_read_and_a_reviewed_run_replayed_over_the_protocol() {
    let folder = workspace_with_rules("replay");
    let mut server = Server::start();

    // The guides need no workspace: how to choose, then one rule in full.
    let choosing = server.call("rule_guide", json!({}));
    assert_eq!(choosing["outcome"], "ok", "{choosing}");
    assert!(
        choosing["result"]["choosing"]
            .as_str()
            .unwrap()
            .starts_with("# Choosing a rule")
    );
    assert_eq!(choosing["result"]["rules"].as_array().unwrap().len(), 6);
    let follow = server.call("rule_guide", json!({"rule": "from another gate"}));
    assert_eq!(follow["result"]["kind"], "FromAnotherGate", "{follow}");
    assert!(
        follow["result"]["guide"]
            .as_str()
            .unwrap()
            .contains("`same_shape_as`")
    );
    let one = server.call("rule_guide", json!({"rule": "tail fraction"}));
    assert_eq!(one["outcome"], "ok", "{one}");
    assert_eq!(one["result"]["kind"], "TailFraction");
    assert!(one["result"]["guide"].as_str().unwrap().contains("`aim`"));
    let unknown = server.call("rule_guide", json!({"rule": "wobble"}));
    assert_eq!(unknown["outcome"], "failed", "{unknown}");
    assert!(
        unknown["reason"]
            .as_str()
            .unwrap()
            .contains("Above the negative")
    );

    // The description and the code need no workspace.
    let explained = server.call("explain_gate_positioning", json!({}));
    assert_eq!(explained["outcome"], "ok", "{explained}");
    assert!(
        explained["result"]["description"]
            .as_str()
            .unwrap()
            .contains("slide_to_capture")
    );
    let listed = explained["result"]["readable_source"].as_array().unwrap();
    assert!(listed.iter().any(|f| f["path"] == "gate_rules/autogate.rs"));
    let found = server.call(
        "read_positioning_code",
        json!({"search": "fn slide_to_capture"}),
    );
    assert_eq!(found["outcome"], "ok", "{found}");
    let at = found["result"][0]["line"].as_u64().unwrap();
    let read = server.call(
        "read_positioning_code",
        json!({"path": "gate_rules/autogate.rs", "from": at, "to": at + 2}),
    );
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["result"]["lines"].as_array().unwrap().len(), 3);
    assert!(
        read["result"]["lines"][0]
            .as_str()
            .unwrap()
            .contains("fn slide_to_capture(")
    );
    let refused = server.call("read_positioning_code", json!({"path": "session/mod.rs"}));
    assert_eq!(refused["outcome"], "failed", "{refused}");

    // A replay needs a run.
    let opened = server.call(
        "open_workspace",
        json!({"folder": folder.to_str().unwrap()}),
    );
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(server.call("replay_rules", json!({}))["outcome"], "failed");
    assert_eq!(server.call("preview_rules", json!({}))["outcome"], "ok");
    assert_eq!(
        server.call("apply_rule_placements", json!({}))["outcome"],
        "ok"
    );
    assert_eq!(server.call("mark_run_reviewed", json!({}))["outcome"], "ok");

    let rule = |low: f64, high: f64| {
        let parameter = clingate_core::session::Session::open(&folder)
            .unwrap()
            .rules()
            .unwrap()
            .entries()[0]
            .rule
            .parameter
            .to_string();
        json!({
            "parameter": parameter,
            "bound": "Above",
            "measured_on": "Itself",
            "rule": {"kind": "TailFraction", "band": [low, high]}
        })
    };
    let replayed = server.call(
        "replay_rules",
        json!({
            "scope": "workspace",
            "rule_changes": [{"target": {"gate": "Tmem"}, "rule": rule(0.05, 0.06)}]
        }),
    );
    assert_eq!(replayed["outcome"], "ok", "{replayed}");
    let result = &replayed["result"];
    assert_eq!(result["cases_total"], 2, "{result}");
    assert!(
        result["changes_tried"][0]
            .as_str()
            .unwrap()
            .contains("capture 5.000% to 6.000%")
    );
    let case = result["cases"][0]["case"].as_str().unwrap().to_string();

    let detail = server.call(
        "replay_case",
        json!({
            "case": case,
            "scope": "workspace",
            "rule_changes": [{"target": {"gate": "Tmem"}, "rule": rule(0.05, 0.06)}]
        }),
    );
    assert_eq!(detail["outcome"], "ok", "{detail}");
    assert!(
        detail["result"]["rule_replayed"]
            .as_str()
            .unwrap()
            .contains("capture 5.000% to 6.000%")
    );
    assert_eq!(
        detail["result"]["sample"]["histogram"]["counts"]
            .as_array()
            .unwrap()
            .len(),
        64
    );

    // Changes that do not read are refused with the form they take.
    let bad = server.call(
        "replay_rules",
        json!({"rule_changes": [{"target": "Tmem"}]}),
    );
    assert_eq!(bad["outcome"], "failed", "{bad}");
    assert!(
        bad["reason"].as_str().unwrap().contains("measured_on"),
        "{bad}"
    );
    let bad_scope = server.call("replay_rules", json!({"scope": "everywhere"}));
    assert_eq!(bad_scope["outcome"], "failed", "{bad_scope}");

    // Candidates tried on the files as they are, nothing moved.
    let tried = server.call(
        "try_rules",
        json!({"population": "Tmem", "candidates": [rule(0.05, 0.06), rule(0.01, 0.02)], "max_rows": 1}),
    );
    assert_eq!(tried["outcome"], "ok", "{tried}");
    assert_eq!(tried["result"]["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(tried["result"]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(tried["result"]["rows_total"], 2);
    let bad_candidates = server.call(
        "try_rules",
        json!({"population": "Tmem", "candidates": [{"kind": "TailFraction"}]}),
    );
    assert_eq!(bad_candidates["outcome"], "failed", "{bad_candidates}");

    // What the gate's populations look like, and a picture of it.
    let profiled = server.call("gate_profile", json!({"population": "Tmem"}));
    assert_eq!(profiled["outcome"], "ok", "{profiled}");
    assert!(
        profiled["result"]["profile"]["gate"]
            .as_str()
            .unwrap()
            .starts_with("Tmem of ")
    );
    assert!(!profiled["result"]["lines"].as_array().unwrap().is_empty());
    let every = server.call("gate_profile", json!({"specimens": 1}));
    assert_eq!(every["outcome"], "ok", "{every}");
    assert!(every["result"]["profile"].is_null());
    assert_eq!(every["result"]["specimens_read"], 1);
    let drawn = server.request(
        "tools/call",
        json!({"name": "gate_picture", "arguments": {"population": "Tmem", "tiles": 2}}),
    );
    let content = drawn["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image", "{drawn}");
    assert_eq!(content[0]["mimeType"], "image/png");
    // A PNG, base64: its signature.
    assert!(
        content[0]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBORw0KGgo")
    );
    let said: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(said["outcome"], "ok", "{said}");
    let plots = said["result"]["plots_left_to_right_top_to_bottom"]
        .as_array()
        .unwrap();
    assert!((1..=2).contains(&plots.len()), "{said}");
    let asked = server.request(
        "tools/call",
        json!({"name": "gate_picture", "arguments": {"population": "Tme"}}),
    );
    let said: Value =
        serde_json::from_str(asked["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(said["outcome"], "needs_clarification", "{said}");

    // Written only when asked, into the rules file.
    let updated = server.call(
        "update_rule",
        json!({"gate": "Tmem", "parent": "", "rule": rule(0.05, 0.06)}),
    );
    assert_eq!(updated["outcome"], "ok", "{updated}");
    let reopened = clingate_core::session::Session::open(&folder).unwrap();
    let view = reopened.rules_view().unwrap();
    assert_eq!(view.rules.len(), 1);
    assert!(
        view.rules[0].rule.contains("5.000% to 6.000%"),
        "{:?}",
        view.rules
    );
    let wrong = server.call(
        "update_rule",
        json!({"gate": "Tmem", "rule": {"kind": "TailFraction"}}),
    );
    assert_eq!(wrong["outcome"], "failed", "{wrong}");
}
