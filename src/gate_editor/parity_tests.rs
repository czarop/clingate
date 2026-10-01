//! The app and the tools for Claude, held to each other.
//!
//! The two share their decisions - [`clingate_core::working_copy`] for every
//! edit, undo, save and export, [`clingate_core::workspace`] for where things
//! are kept, [`RunInputs::assemble`] for what a rules run reads - but each has
//! its own wiring around them, and wiring is where they could drift apart.
//! So these tests do the same things both ways, on two identical workspace
//! folders, and compare what comes out: the working copy's gates, what the
//! edit bar would show, and every file written.
//!
//! The app side is the app's own code, put together as the shell puts it
//! together ([`provide_document`]), in a headless `VirtualDom`: the Workspace
//! tab's handles open the folder, the Rules tab's [`RulesRun`] assembles and
//! applies a run, and the edit bar's [`Edits`] saves, undoes and exports. The
//! tools' side is [`Session`], which is all the MCP server calls.
//!
//! Deliberate differences, not tested as the same: the tools take a bare file
//! name for an export and never write over a file unless told to, where the
//! app takes any path from its box or a save dialog (which asks itself).
//!
//! [`RunInputs::assemble`]: clingate_core::gate_rules::run::RunInputs::assemble

#![cfg(test)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use clingate_core::gate_rules::rule_store::RuleStore;
use clingate_core::gate_rules::run::run_rules;
use clingate_core::omiq::metadata::MetaDataStoreStoreExt;
use clingate_core::session::Session;
use clingate_core::test_workspace::two_samples_with_a_rule;
use clingate_core::workspace::{RECOVERY_GATING, SAVED_GATING, SAVED_SCALING};
use dioxus::prelude::*;
use dioxus_core::NoOpMutations;

use crate::components::toast::ToastProvider;
use crate::gate_editor::edits::Edits;
use crate::gate_editor::gate_rules_window::RulesRun;
use crate::gate_editor::route::provide_document;
use crate::gate_editor::workspace_window::{AxesStore, GateStore, Handles, MetadataStore};

// ── the app, headless ────────────────────────────────────────────────────

/// Every handle the tests reach the app through.
#[derive(Clone, Copy)]
struct Held {
    workspace: Handles,
    rules_run: RulesRun,
    edits: Edits,
    gates: GateStore,
    axes: AxesStore,
    metadata: MetadataStore,
    rules: Signal<RuleStore>,
    paused: Signal<Option<crate::gate_editor::paused_run::PausedRun>>,
}

/// Where the probe leaves the handles, for the test to pick up.
#[derive(Clone, Default)]
struct Holder(Rc<RefCell<Option<Held>>>);

impl PartialEq for Holder {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

fn root(holder: Holder) -> Element {
    provide_document();
    rsx! {
        ToastProvider { Probe { holder } }
    }
}

#[component]
fn Probe(holder: Holder) -> Element {
    let held = Held {
        workspace: Handles::from_context(),
        rules_run: RulesRun::from_context(),
        edits: use_context(),
        gates: use_context(),
        axes: use_context(),
        metadata: use_context(),
        rules: use_context(),
        paused: use_context(),
    };
    *holder.0.borrow_mut() = Some(held);
    rsx! {}
}

struct App {
    dom: VirtualDom,
    runtime: tokio::runtime::Runtime,
    holder: Holder,
}

impl App {
    fn new() -> Self {
        let holder = Holder::default();
        let mut dom = VirtualDom::new_with_props(root, holder.clone());
        dom.rebuild_in_place();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut app = Self {
            dom,
            runtime,
            holder,
        };
        app.settle();
        app
    }

    fn held(&self) -> Held {
        self.holder.0.borrow().expect("the probe has rendered")
    }

    /// Do something with the app's handles, as a click would, and let
    /// whatever it started - a recovery write - finish.
    fn with<T>(&mut self, f: impl FnOnce(Held) -> T) -> T {
        let held = self.held();
        let out = self.dom.in_scope(ScopeId::APP, || f(held));
        self.settle();
        out
    }

    /// Run an async action of the app's to the end, as a button's task would.
    fn run<F>(&mut self, f: impl FnOnce(Held) -> F)
    where
        F: std::future::Future<Output = ()> + 'static,
    {
        let held = self.held();
        let done = Rc::new(RefCell::new(false));
        let flag = done.clone();
        self.dom.in_scope(ScopeId::APP, || {
            let task = f(held);
            spawn(async move {
                task.await;
                *flag.borrow_mut() = true;
            });
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        while !*done.borrow() {
            assert!(Instant::now() < deadline, "the app's action never finished");
            self.step(Duration::from_millis(50));
        }
        self.settle();
    }

    /// One turn of the app: wait for work, then render - inside the runtime,
    /// as rendering can poll a task that woke after the wait.
    fn step(&mut self, wait: Duration) -> bool {
        let _inside = self.runtime.enter();
        let dom = &mut self.dom;
        let woke = self
            .runtime
            .block_on(async { tokio::time::timeout(wait, dom.wait_for_work()).await })
            .is_ok();
        self.dom.render_immediate(&mut NoOpMutations);
        woke
    }

    /// Until nothing is left to do.
    fn settle(&mut self) {
        let mut idle = 0;
        while idle < 3 {
            if self.step(Duration::from_millis(100)) {
                idle = 0;
            } else {
                idle += 1;
            }
        }
    }

    fn open(&mut self, folder: &Path) {
        let folder = folder.to_path_buf();
        self.run(move |held| held.workspace.open_folder(folder));
    }

    /// The working copy's gates, as Omiq's document - what Save would write.
    fn working(&self) -> serde_json::Value {
        let held = self.held();
        self.dom.in_scope(ScopeId::APP, || {
            clingate_core::omiq::serialise::to_omiq_document(
                &held.gates.peek(),
                &held.metadata.metadata().peek(),
                &held.axes.peek().settings,
            )
            .unwrap()
        })
    }

    fn standing(&self) -> clingate_core::working_copy::Standing {
        let held = self.held();
        self.dom
            .in_scope(ScopeId::APP, || held.edits.standing_now())
    }

    fn offers_earlier_changes(&self) -> bool {
        let held = self.held();
        self.dom
            .in_scope(ScopeId::APP, || held.edits.recovery_offer_now().is_some())
    }

    fn rules(&self) -> serde_json::Value {
        let held = self.held();
        self.dom.in_scope(ScopeId::APP, || {
            serde_json::to_value(&*held.rules.peek()).unwrap()
        })
    }

    /// The Rules tab's Run, to the end: what it reads, the run, and the
    /// placements written as one step.
    fn run_rules(&mut self) {
        self.with(|held| {
            let (inputs, _) = held.rules_run.inputs_now();
            assert!(!inputs.files.is_empty(), "the app has its files");
            let snapshot = held.gates.peek().clone();
            let outcome = run_rules(&snapshot, &inputs, |_| {}, &AtomicBool::new(false));
            held.rules_run.place(&outcome.placements, &[]);
            held.rules_run.keep(&outcome, &inputs.rules).unwrap();
        });
    }
}

// ── the tools ────────────────────────────────────────────────────────────

fn working(session: &Session) -> serde_json::Value {
    clingate_core::omiq::serialise::to_omiq_document(
        session.gates(),
        session.metadata().metadata(),
        &session.axes().settings,
    )
    .unwrap()
}

// ── comparing ────────────────────────────────────────────────────────────

/// Two identical workspaces: one for the tools, one for the app.
fn twins(name: &str) -> (PathBuf, PathBuf) {
    (
        two_samples_with_a_rule(&format!("{name}-tools")),
        two_samples_with_a_rule(&format!("{name}-app")),
    )
}

fn bytes(path: PathBuf) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// The same working copy, the same standing, the same recovery copy on disk.
fn same(step: &str, session: &Session, app: &App, tools: &Path, ours: &Path) {
    assert!(
        working(session) == app.working(),
        "{step}: the working copies differ"
    );
    assert_eq!(
        session.standing(),
        app.standing(),
        "{step}: the edit states differ"
    );
    assert_eq!(
        bytes(tools.join(RECOVERY_GATING)),
        bytes(ours.join(RECOVERY_GATING)),
        "{step}: the recovery copies differ"
    );
}

// ── the tests ────────────────────────────────────────────────────────────

#[test]
fn a_workspace_opens_the_same_for_the_tools_and_the_app() {
    let (tools, ours) = twins("parity-open");
    let session = Session::open(&tools).unwrap();
    let mut app = App::new();
    app.open(&ours);

    same("opened", &session, &app, &tools, &ours);
    assert!(
        session.standing().has_saved_copy,
        "the gating file is the saved copy"
    );
    // The rules from the rules folder, with the same pairing.
    assert_eq!(
        serde_json::to_value(session.rules().expect("the tools found the rules")).unwrap(),
        app.rules(),
        "the rules differ"
    );
    assert!(!session.edit_state().earlier_unsaved_changes);
    assert!(!app.offers_earlier_changes());
}

#[test]
fn a_rules_run_undo_redo_export_and_save_come_out_the_same() {
    let (tools, ours) = twins("parity-edits");
    let mut session = Session::open(&tools).unwrap();
    let mut app = App::new();
    app.open(&ours);
    let drawn = working(&session);

    // Nothing to undo either way.
    assert!(session.undo().is_err());
    assert!(!app.with(|h| h.edits.undo()));
    same("nothing to undo", &session, &app, &tools, &ours);

    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    app.run_rules();
    assert!(
        working(&session) != drawn,
        "the run moved nothing - the test proves nothing"
    );
    same("a rules run", &session, &app, &tools, &ours);
    assert!(tools.join(RECOVERY_GATING).is_file());
    // Both keep the run for review, and keep the same thing - all but the
    // moment it was applied.
    let kept = |folder: &Path| {
        let mut record = clingate_core::review::RunRecord::load(folder)
            .unwrap()
            .expect("the run is kept");
        record.applied_at.clear();
        record
    };
    assert!(!kept(&tools).placed.is_empty());
    assert_eq!(kept(&tools), kept(&ours), "the kept runs differ");
    // And the same events kept with each.
    let events = |folder: &Path| {
        let run = clingate_core::review::RunRecord::load(folder)
            .unwrap()
            .unwrap();
        let mut e = clingate_core::review::events::load(folder, &run.applied_at)
            .unwrap()
            .expect("events kept with the run")
            .samples;
        e.sort_by(|a, b| (&a.gate_id, &a.file).cmp(&(&b.gate_id, &b.file)));
        e
    };
    let (from_tools, from_app) = (events(&tools), events(&ours));
    assert!(!from_tools.is_empty());
    assert_eq!(from_tools, from_app, "the kept events differ");
    // And assess the same: the app's review list is `assess` over the gates
    // it holds, the tools' assess_run over the session's.
    let from_tools = session.assess_run().unwrap();
    let from_app = app.with(|h| {
        let run = clingate_core::review::RunRecord::load(&ours)
            .unwrap()
            .unwrap();
        let state = h.gates.peek();
        let files = h.metadata.metadata().peek().clone();
        clingate_core::review::assess::assess(&run, Some((&state, &files)))
    });
    assert_eq!(from_tools.gates, from_app.gates);
    // And sort into the same piles: the Review tab's board, the tools'
    // assess_run.
    let board = app.with(|h| {
        let state = h.gates.peek();
        let files = h.metadata.metadata().peek().clone();
        clingate_core::review::board::board_in(&ours, &state, &files)
            .unwrap()
            .unwrap()
    });
    use clingate_core::review::board::Pile;
    assert_eq!(from_tools.placements, board.entries.len());
    let app_piles: Vec<(Pile, usize)> = Pile::ALL.iter().map(|p| (*p, board.count(*p))).collect();
    assert_eq!(from_tools.piles, app_piles, "the piles differ");
    let app_flags: Vec<_> = board
        .pile(Pile::NeedsALook)
        .filter_map(|e| e.flag.clone())
        .collect();
    assert_eq!(
        from_tools.flags, app_flags,
        "the flags needing a look differ"
    );
    // The raw assessment agrees too.
    let tools_raw = clingate_core::review::assess::assess(
        &clingate_core::review::RunRecord::load(&tools)
            .unwrap()
            .unwrap(),
        None,
    );
    let app_raw = clingate_core::review::assess::assess(
        &clingate_core::review::RunRecord::load(&ours)
            .unwrap()
            .unwrap(),
        None,
    );
    assert_eq!(tools_raw.flags.len(), app_raw.flags.len());
    assert_eq!(from_app.placements, app_raw.placements);

    session.undo().unwrap();
    assert!(app.with(|h| h.edits.undo()));
    same("undone", &session, &app, &tools, &ours);

    session.redo().unwrap();
    assert!(app.with(|h| h.edits.redo()));
    same("redone", &session, &app, &tools, &ours);

    // An export is the saved copy, by the same name in the same place.
    let exported = session.export("shared", false).unwrap();
    app.with(|h| h.workspace.write_gating(Path::new("shared")));
    assert_eq!(exported.file, tools.join("shared.omiqgt"));
    assert_eq!(
        bytes(exported.file),
        bytes(ours.join("shared.omiqgt")),
        "the exports differ"
    );

    let saved = session.save().unwrap();
    app.with(|h| h.edits.save()).unwrap();
    same("saved", &session, &app, &tools, &ours);
    for file in [SAVED_GATING, SAVED_SCALING] {
        assert!(tools.join(file).is_file(), "{file}");
        assert_eq!(
            bytes(tools.join(file)),
            bytes(ours.join(file)),
            "the saved {file} differ"
        );
    }
    assert_eq!(saved.gating_file, tools.join(SAVED_GATING));

    // Nothing unsaved to revert, either way.
    assert!(session.revert().is_err());
    assert!(app.with(|h| h.edits.revert()).is_err());

    // Back before the save, then revert to it, then undo the revert.
    session.undo().unwrap();
    assert!(app.with(|h| h.edits.undo()));
    same("undone past the save", &session, &app, &tools, &ours);
    session.revert().unwrap();
    app.with(|h| h.edits.revert()).unwrap();
    same("reverted", &session, &app, &tools, &ours);
    session.undo().unwrap();
    assert!(app.with(|h| h.edits.undo()));
    same("revert undone", &session, &app, &tools, &ours);

    // Opened again, both open on the save.
    drop(session);
    let reopened = Session::open(&tools).unwrap();
    let mut app = App::new();
    app.open(&ours);
    assert!(
        working(&reopened) == app.working(),
        "reopened on different gates"
    );
}

#[test]
fn unsaved_changes_left_behind_come_back_the_same() {
    let (tools, ours) = twins("parity-recovery");
    {
        let mut session = Session::open(&tools).unwrap();
        session.preview_rules().unwrap();
        session.apply_previewed_rules().unwrap();
        let mut app = App::new();
        app.open(&ours);
        app.run_rules();
        same("before closing", &session, &app, &tools, &ours);
        // Both closed without saving.
    }

    let mut session = Session::open(&tools).unwrap();
    let mut app = App::new();
    app.open(&ours);
    same("reopened", &session, &app, &tools, &ours);
    assert!(session.edit_state().earlier_unsaved_changes);
    assert!(app.offers_earlier_changes());

    session.restore_unsaved_changes().unwrap();
    app.with(|h| h.edits.restore_recovery()).unwrap();
    same("restored", &session, &app, &tools, &ours);
    assert!(!session.edit_state().earlier_unsaved_changes);
    assert!(!app.offers_earlier_changes());

    session.revert().unwrap();
    app.with(|h| h.edits.revert()).unwrap();
    same("reverted", &session, &app, &tools, &ours);
    session.undo().unwrap();
    assert!(app.with(|h| h.edits.undo()));
    same("revert undone", &session, &app, &tools, &ours);
}

#[test]
fn unsaved_changes_left_behind_are_discarded_the_same() {
    let (tools, ours) = twins("parity-discard");
    {
        let mut session = Session::open(&tools).unwrap();
        session.preview_rules().unwrap();
        session.apply_previewed_rules().unwrap();
        let mut app = App::new();
        app.open(&ours);
        app.run_rules();
    }
    let mut session = Session::open(&tools).unwrap();
    let mut app = App::new();
    app.open(&ours);
    session.discard_unsaved_changes().unwrap();
    app.with(|h| h.edits.discard_recovery());
    same("discarded", &session, &app, &tools, &ours);
    assert!(!tools.join(RECOVERY_GATING).exists());
    assert!(!app.offers_earlier_changes());
}

#[test]
fn rules_saved_on_the_rules_tab_are_the_ones_the_tools_open() {
    let (tools, ours) = twins("parity-rules-file");
    let mut app = App::new();
    app.open(&ours);
    // The Rules tab's Save with its box as it starts: the default name.
    let path =
        clingate_core::workspace::rules_path(Some(&ours), clingate_core::workspace::RULES_FILE);
    assert_eq!(path, ours.join("rules").join("gate_rules.json"));
    app.with(|h| h.rules.peek().save(&path)).unwrap();
    let from_app = Session::open(&ours).unwrap();
    let from_tools = Session::open(&tools).unwrap();
    assert_eq!(
        serde_json::to_value(from_app.rules().unwrap()).unwrap(),
        serde_json::to_value(from_tools.rules().unwrap()).unwrap(),
    );
}

#[test]
fn a_report_and_a_review_come_out_the_same() {
    use clingate_core::review::Problem;
    use clingate_core::review::report::{PlacementReport, reports_in};
    let (tools, ours) = twins("parity-review");
    let mut session = Session::open(&tools).unwrap();
    session.set_review_library(None);
    let mut app = App::new();
    app.open(&ours);
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    app.run_rules();

    // Tmem on the FMX sample, reported by each.
    session
        .report_placement("Tmem", "sample1_FMX", "too_low", "let in negatives")
        .unwrap();
    let job = app.with(|h| {
        let state = h.gates.peek().clone();
        let node = state
            .placements()
            .find(|(_, p)| {
                state
                    .registered_gate(&p.gate_id)
                    .is_some_and(|g| g.get_name() == "Tmem")
            })
            .map(|(n, _)| n.clone())
            .expect("Tmem is in the tree");
        let sample = h
            .metadata
            .file_name_to_gating_id()
            .peek()
            .get("sample1_FMX.fcs")
            .cloned()
            .unwrap();
        h.rules_run
            .report_job(clingate_core::review::ReportRequest {
                node,
                sample,
                problem: Problem::TooLow,
                note: "let in negatives".into(),
            })
            .expect("the app has a folder")
    });
    job().unwrap();

    let one = |folder: &Path| -> PlacementReport {
        let mut found = reports_in(folder);
        assert_eq!(found.len(), 1);
        let mut r = found.remove(0).1;
        r.id.clear();
        r.reported_at.clear();
        r.workspace.clear();
        // Each side applied its own run, stamped to the second: the two can
        // straddle a second boundary.
        r.run_applied_at = None;
        r
    };
    let (from_tools, from_app) = (one(&tools), one(&ours));
    assert!(from_tools.placed().is_some(), "the rule placed it");
    assert!(from_tools == from_app, "the reports differ");

    // Reviewed by each: the same outcomes for every placement.
    session.mark_run_reviewed().unwrap();
    app.with(|h| h.rules_run.mark_reviewed(None)).unwrap();
    let review = |folder: &Path| {
        let text = std::fs::read_to_string(folder.join("reviews").join("review.json")).unwrap();
        let mut r: clingate_core::review::RunReview = serde_json::from_str(&text).unwrap();
        r.reviewed_at.clear();
        r.workspace.clear();
        r.run_applied_at.clear();
        for p in &mut r.placements {
            if let clingate_core::review::report::Outcome::Reported { reports } = &mut p.outcome {
                reports.clear();
            }
        }
        r
    };
    let (a, b) = (review(&tools), review(&ours));
    assert_eq!(a.reported(), 1);
    assert!(a == b, "the reviews differ");
}

// ── a run that pauses for a person ───────────────────────────────────────

/// A gate with a shape of its own and a gate with one under it: the parent
/// gets a rule it cannot meet on any specimen - read on an FMO no file is -
/// and the child a rule, so the run has to stop for the parent first.
fn rules_that_stop_at_a_parent(gates: &clingate_core::gates::GateState) -> RuleStore {
    use clingate_core::gate_rules::rule::{Rule, TailFractionRule};
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleTarget, SamplePairing,
    };
    let shaped = |node: &clingate_core::gates::gate_store::NodeId| {
        let id = gates.gate_for_node(node)?.clone();
        let gate = gates.registered_gate(&id)?;
        gate.get_gate_ref(None)?;
        Some((gates.population_name(&id)?, gate.get_params().0))
    };
    let (parent, child) = gates
        .placements()
        .find_map(|(node, _)| {
            let parent = shaped(node)?;
            let child = gates.child_nodes(node).iter().find_map(&shaped)?;
            Some((parent, child))
        })
        .expect("a shaped gate with a shaped gate under it");
    let rule = |parameter: Arc<str>, measured_on| GateRule {
        parameter,
        bound: Bound::Above,
        measured_on,
        rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
    };
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    store.insert(
        RuleTarget::named(parent.0.clone()),
        rule(parent.1, MeasuredOn::Partner("FMO".into())),
    );
    store.insert(
        RuleTarget::under(child.0, parent.0),
        rule(child.1, MeasuredOn::Itself),
    );
    store
}

#[test]
fn a_paused_run_holds_each_gate_on_its_sample_and_the_editor_opens_on_it() {
    use clingate_core::axis_store::{AxisStoreStoreExt, Param};
    use clingate_core::gate_rules::run::run_rules_pausing;
    use clingate_core::gates::gate_store::GateSource;
    use clingate_core::omiq::metadata::MetaDataStoreStoreExt;

    let folder = two_samples_with_a_rule("paused-run-app");
    let mut app = App::new();
    app.open(&folder);
    let needs = app.with(|held| {
        let rules = rules_that_stop_at_a_parent(&held.gates.peek());
        held.rules.clone().set(rules);
        let (inputs, _) = held.rules_run.inputs_now();
        let snapshot = held.gates.peek().clone();
        let outcome = run_rules_pausing(&snapshot, &inputs, 0, |_| {}, &AtomicBool::new(false));
        let paused = outcome
            .paused
            .as_ref()
            .expect("the parent has a ruled gate under it");
        held.rules_run.place(&outcome.placements, &paused.needs);
        paused.needs.clone()
    });
    assert_eq!(needs.len(), 2, "the parent on both specimens: {needs:?}");

    app.with(|held| {
        let gates = held.gates.peek();
        let metadata = held.metadata.metadata().peek().clone();
        let params: Vec<Param> = held.axes.sorted_settings().peek().iter().cloned().collect();
        let names = held.metadata.file_name_to_gating_id().peek().clone();
        let files: Vec<Arc<str>> = names.keys().cloned().collect();
        let mut specimens = Vec::new();
        for need in &needs {
            let specimen = need.specimen.clone().expect("each file has a test value");
            match gates.gate_and_source_for_file(&need.gate_id, &need.file, &metadata) {
                Some((GateSource::Group((_, key)), _)) => assert_eq!(key, specimen),
                other => panic!("not held on its specimen: {:?}", other.map(|o| o.0)),
            }
            specimens.push(specimen.group.clone());

            let focus = crate::gate_editor::paused_run::focus_for(need, &gates, &params, &names)
                .expect("the editor can show it");
            let node = gates.nodes_for_gate(&need.gate_id)[0].clone();
            assert_eq!(&*focus.parent, gates.parent_node(&node).unwrap().as_str());
            let (x, y) = gates.registered_gate(&need.gate_id).unwrap().get_params();
            assert_eq!((focus.x, focus.y), (x, y));
            assert!(files.contains(&focus.sample_name));
            assert_eq!(
                names[&focus.sample_name], need.file,
                "the file the run named"
            );
        }
        specimens.sort();
        specimens.dedup();
        assert_eq!(specimens.len(), 2, "one position per specimen");
    });
}

#[test]
fn the_banner_names_the_gate_the_sample_and_why() {
    use clingate_core::gate_rules::run::NeedsPlacing;
    use clingate_core::omiq::metadata::MetaDataKey;
    let mut need = NeedsPlacing {
        gate_id: Arc::from("g1"),
        gate: Arc::from("Lymph"),
        parent_gate: Some(Arc::from("Live")),
        file: Arc::from("file_b"),
        specimen: Some(MetaDataKey {
            parameter: Arc::from("SampleID"),
            group: Arc::from("DONOR-B"),
        }),
        why: "no FMO for this specimen".to_string(),
    };
    assert_eq!(
        crate::gate_editor::paused_run::describe_need(&need),
        "Lymph of Live on DONOR-B: no FMO for this specimen"
    );
    need.specimen = None;
    assert!(crate::gate_editor::paused_run::describe_need(&need).contains("on file_b:"));
}

/// A paused run of `rules_that_stop_at_a_parent` on the open workspace, its
/// placements written as the Run button writes them.
fn pause_the_rules(app: &mut App) -> crate::gate_editor::paused_run::PausedRun {
    use clingate_core::gate_rules::run::run_rules_pausing;
    app.with(|held| {
        let rules = rules_that_stop_at_a_parent(&held.gates.peek());
        held.rules.clone().set(rules.clone());
        let (inputs, _) = held.rules_run.inputs_now();
        let snapshot = held.gates.peek().clone();
        let outcome = run_rules_pausing(&snapshot, &inputs, 0, |_| {}, &AtomicBool::new(false));
        let needs = outcome.paused.as_ref().expect("it pauses").needs.clone();
        held.rules_run.place(&outcome.placements, &needs);
        crate::gate_editor::paused_run::PausedRun::new(outcome, 1, rules, &held.gates.peek())
    })
}

#[test]
fn a_paused_run_is_dropped_when_another_workspace_is_opened() {
    let mut app = App::new();
    app.open(&two_samples_with_a_rule("paused-run-first"));
    let paused = pause_the_rules(&mut app);
    app.with(|held| held.paused.clone().set(Some(paused)));
    app.open(&two_samples_with_a_rule("paused-run-second"));
    assert!(
        app.with(|held| held.paused.peek().is_none()),
        "its gates and samples belong to the workspace that was closed"
    );
}

#[test]
fn a_paused_run_belongs_only_to_the_document_it_was_run_on() {
    let mut app = App::new();
    app.open(&two_samples_with_a_rule("paused-run-document"));
    let paused = pause_the_rules(&mut app);
    assert!(paused.is_on(1));
    assert!(!paused.is_on(2));
}

#[test]
fn a_paused_run_goes_on_only_while_its_rules_and_their_levels_stand() {
    use clingate_core::gate_rules::autogate::rule_levels;
    let mut app = App::new();
    app.open(&two_samples_with_a_rule("paused-run-go-on"));
    let paused = pause_the_rules(&mut app);
    app.with(|held| {
        let rules = held.rules.peek().clone();
        assert_eq!(paused.why_not_go_on(&held.gates.peek(), &rules), None);

        let mut changed = rules.clone();
        let mut entry = changed.entries()[0].clone();
        entry.rule.bound = clingate_core::gate_rules::rule_store::Bound::Below;
        changed.insert(entry.target, entry.rule);
        assert_eq!(
            rule_levels(&held.gates.peek(), &changed),
            rule_levels(&held.gates.peek(), &rules)
        );
        assert!(
            paused.why_not_go_on(&held.gates.peek(), &changed).is_some(),
            "a rule was changed"
        );

        let mut gates = held.gates.peek().clone();
        let child = rule_levels(&gates, &rules)
            .last()
            .and_then(|level| level.iter().next().cloned())
            .expect("a gate on the last level");
        gates.delete_placement(&child).unwrap();
        assert!(
            paused.why_not_go_on(&gates, &rules).is_some(),
            "a gate the run would place next is gone"
        );
    });
}

#[test]
fn stopping_a_paused_run_keeps_it_with_the_rules_it_ran() {
    let folder = two_samples_with_a_rule("paused-run-stop");
    let mut app = App::new();
    app.open(&folder);
    let paused = pause_the_rules(&mut app);
    let ran = app.with(|held| {
        let ran = held.rules.peek().clone();
        held.rules.clone().set(RuleStore::default());
        paused.keep(&held.rules_run).unwrap();
        ran
    });
    let record = clingate_core::review::RunRecord::load(&folder)
        .unwrap()
        .expect("the run is kept");
    assert_eq!(record.rules, ran);
}

#[test]
fn a_run_going_on_from_a_pause_is_told_to_continue_not_to_run_again() {
    use crate::gate_editor::paused_run::changed_while_running;
    assert!(changed_while_running(true).contains("Continue the run"));
    assert!(!changed_while_running(true).contains("run it again"));
    assert!(changed_while_running(false).contains("run it again"));
}

// ── the Position menu ────────────────────────────────────────────────────

#[test]
fn a_change_of_mode_from_the_menu_is_one_step_that_undo_takes_back() {
    use clingate_core::gates::gate_positions::{self, Mode};
    use clingate_core::omiq::metadata::MetaDataStoreStoreExt;
    let mut app = App::new();
    app.open(&two_samples_with_a_rule("position-menu-step"));
    let (gate_id, file, files) = app.with(|held| {
        let gates = held.gates.peek();
        let gate_id = gates
            .placements()
            .find_map(|(node, _)| {
                let id = gates.gate_for_node(node)?;
                gates.registered_gate(id)?.get_gate_ref(None)?;
                Some(id.clone())
            })
            .expect("a gate with a shape");
        let files = held.metadata.metadata().peek().clone();
        let file = files.keys().min().unwrap().clone();
        (gate_id, file, files)
    });
    let mode = |app: &mut App| app.with(|held| gate_positions::mode(&held.gates.peek(), &gate_id));
    let was = mode(&mut app);
    assert_ne!(was, Mode::PerSample);
    assert_eq!(app.standing().undo_steps, 0);

    app.with(|held| {
        crate::gate_editor::position_menu::reposition(held.gates, held.edits, |state| {
            gate_positions::set_mode(state, &gate_id, &Mode::PerSample, Some(&file), &files)
        })
    })
    .unwrap();
    assert_eq!(mode(&mut app), Mode::PerSample);
    assert_eq!(app.standing().undo_steps, 1, "one step");

    assert!(app.with(|held| held.edits.undo()));
    assert_eq!(mode(&mut app), was);

    let refused = app.with(|held| {
        crate::gate_editor::position_menu::reposition(held.gates, held.edits, |state| {
            let by_nothing = Mode::ByColumn(std::sync::Arc::from("Nothing"));
            gate_positions::set_mode(state, &gate_id, &by_nothing, Some(&file), &files)
        })
    });
    assert!(
        refused
            .unwrap_err()
            .contains("no sample has a value of Nothing")
    );
    assert_eq!(mode(&mut app), was);
    assert_eq!(app.standing().undo_steps, 0, "a refused change is no step");
}

/// A gate dragged into another on its plot stops touching it when the
/// editor keeps gates apart, and goes all the way when it does not.
#[test]
fn a_drag_with_gates_kept_apart_stops_at_the_gate_beside() {
    use clingate_core::axis_store::PlotMapper;
    use clingate_core::gates::GateState;
    use clingate_core::gates::gate_drag::GateDragData;
    use clingate_core::gates::gate_store::{GateStateImplExt, ROOTGATE};
    use clingate_core::gates::gate_types::PrimaryGateType;
    let square = |x: f32| vec![(x, 0.0), (x + 100.0, 0.0), (x + 100.0, 100.0), (x, 100.0)];
    let mut app = App::new();
    let (moving, beside) = app.with(|held| {
        let mut state = GateState::default();
        let mapper = PlotMapper::new(
            600.0,
            600.0,
            0.0..=1000.0,
            0.0..=1000.0,
            0.0..=1000.0,
            0.0..=1000.0,
            flow_fcs::TransformType::Linear,
            flow_fcs::TransformType::Linear,
        );
        for (name, x) in [("moving", 0.0), ("beside", 200.0)] {
            state
                .add_gate(
                    &mapper,
                    0.0,
                    0.0,
                    Arc::from("FSC-A"),
                    Arc::from("SSC-A"),
                    Some(square(x)),
                    Some(ROOTGATE.clone()),
                    PrimaryGateType::Polygon,
                    Some(name.to_string()),
                )
                .unwrap();
        }
        let id = |name: &str| {
            state
                .registered_ids()
                .into_iter()
                .find(|id| state.registered_gate(id).unwrap().get_name() == name)
                .unwrap()
        };
        let ids = (id("moving"), id("beside"));
        held.gates.clone().set(state);
        ids
    });
    let drag_right = |app: &mut App, keep_apart: bool| {
        app.with(|held| {
            let mut gates = held.gates;
            let resolver = gates
                .peek()
                .get_current_sample(Arc::from("no-such-file"), &Default::default());
            let apart_from: Vec<_> = if keep_apart {
                [&moving, &beside]
                    .map(|id| resolver.resolve_drawable(id).unwrap())
                    .to_vec()
            } else {
                Vec::new()
            };
            let drag = GateDragData::new(moving.clone(), (50.0, 50.0), (200.0, 50.0));
            gates.move_gate(drag, &resolver, &apart_from).unwrap();
            let gate = gates.peek().registered_gate(&moving).unwrap();
            let shape = gate.get_gate_ref(None).unwrap();
            match &shape.geometry {
                flow_gates::GateGeometry::Polygon { nodes, .. } => {
                    f64::from(nodes[0].get_coordinate("FSC-A").unwrap())
                }
                other => panic!("a polygon, got {other:?}"),
            }
        })
    };

    let left_edge = drag_right(&mut app, true);
    assert!(
        (left_edge - 100.0).abs() < 1e-2,
        "stops touching, at {left_edge}"
    );

    let left_edge = drag_right(&mut app, false);
    assert!(
        (left_edge - 250.0).abs() < 1e-2,
        "goes on through, at {left_edge}"
    );
}

/// The gate "t" drawn globally at `drawn` and held for the sample "s1" at
/// `own`, put in place of an opened workspace's gates.
fn a_gate_with_a_position_for_s1(
    app: &mut App,
    drawn: Arc<dyn clingate_core::gates::gate_traits::DrawableGate>,
    own: Arc<dyn clingate_core::gates::gate_traits::DrawableGate>,
) {
    use clingate_core::gates::GateState;
    use clingate_core::gates::gate_store::GateSource;
    app.open(&two_samples_with_a_rule("shape-menu"));
    app.with(|held| {
        let mut state = GateState::default();
        let id: Arc<str> = Arc::from("t");
        state.place_gate(std::slice::from_ref(&id), &drawn, &GateSource::Global);
        state.place_new_gate(None, id.clone()).unwrap();
        state.place_gate(
            std::slice::from_ref(&id),
            &own,
            &GateSource::Sample((id.clone(), Arc::from("s1"))),
        );
        held.gates.clone().set(state);
    });
}

fn shape_t(
    points: &[(f32, f32)],
    rectangle: bool,
) -> Arc<dyn clingate_core::gates::gate_traits::DrawableGate> {
    use clingate_core::gates::gate_single::{
        polygon_gate::PolygonGate, rectangle_gate::RectangleGate,
    };
    let parameters = (Arc::from("FSC-A"), Arc::from("SSC-A"));
    let geometry = if rectangle {
        flow_gates::create_rectangle_geometry(points.to_vec(), "FSC-A", "SSC-A").unwrap()
    } else {
        flow_gates::create_polygon_geometry(points.to_vec(), "FSC-A", "SSC-A").unwrap()
    };
    let gate = flow_gates::Gate {
        id: Arc::from("t"),
        name: "T".into(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters,
        label_position: None,
    };
    if rectangle {
        Arc::new(RectangleGate::try_new(gate, true).unwrap())
    } else {
        Arc::new(PolygonGate::try_new(gate, true).unwrap())
    }
}

/// What `file` shows of "t", as its points and whether it is a polygon.
fn shown_t(app: &mut App, file: &str) -> (Vec<(f64, f64)>, bool) {
    app.with(|held| {
        let gates = held.gates.peek();
        let resolver = gates.get_current_sample(Arc::from(file), &Default::default());
        let gate = resolver.resolve_drawable("t").unwrap();
        let polygon = gate
            .as_any()
            .downcast_ref::<clingate_core::gates::gate_single::polygon_gate::PolygonGate>()
            .is_some();
        let points =
            clingate_core::gates::gate_contact::outline(&gate, &Arc::from("t"), "FSC-A", "SSC-A")
                .unwrap();
        (points, polygon)
    })
}

fn from_the_menu(
    app: &mut App,
    file: &str,
    target: crate::gate_editor::gates::shape_menu::ShapeTarget,
    action: crate::gate_editor::gates::shape_menu::ShapeAction,
) -> Result<(), String> {
    use crate::gate_editor::gates::shape_menu::{ShapeMenu, act};
    app.with(|held| {
        let resolver = held
            .gates
            .peek()
            .get_current_sample(Arc::from(file), &Default::default());
        let menu = ShapeMenu {
            gate_id: Arc::from("t"),
            target,
            at_pixel: (0.0, 0.0),
            at: (40.0, 3.0),
        };
        act(
            held.gates,
            held.edits,
            &resolver,
            &[],
            ("FSC-A", "SSC-A"),
            &menu,
            action,
        )
    })
}

/// A side levelled on the sample shown changes that sample's position only,
/// as a drag does, in one step undo takes back.
#[test]
fn a_side_levelled_from_the_menu_changes_the_position_shown_in_one_step() {
    use crate::gate_editor::gates::shape_menu::{ShapeAction, ShapeTarget};
    let triangle = [(0.0, 0.0), (100.0, 10.0), (50.0, 100.0)];
    let mut app = App::new();
    a_gate_with_a_position_for_s1(
        &mut app,
        shape_t(&triangle, false),
        shape_t(&triangle, false),
    );
    let steps = app.standing().undo_steps;

    from_the_menu(
        &mut app,
        "s1",
        ShapeTarget::Side(0),
        ShapeAction::MakeHorizontal,
    )
    .unwrap();

    assert_eq!(
        shown_t(&mut app, "s1").0,
        [(0.0, 5.0), (100.0, 5.0), (50.0, 100.0)]
    );
    assert_eq!(
        shown_t(&mut app, "s2").0,
        [(0.0, 0.0), (100.0, 10.0), (50.0, 100.0)],
        "the drawn position, unchanged"
    );
    assert_eq!(app.standing().undo_steps, steps + 1);
    assert!(app.with(|held| held.edits.undo()));
    assert_eq!(
        shown_t(&mut app, "s1").0,
        [(0.0, 0.0), (100.0, 10.0), (50.0, 100.0)]
    );
}

#[test]
fn a_point_added_then_deleted_from_the_menu_gives_back_the_outline() {
    use crate::gate_editor::gates::shape_menu::{ShapeAction, ShapeTarget};
    let square = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    let mut app = App::new();
    a_gate_with_a_position_for_s1(&mut app, shape_t(&square, false), shape_t(&square, false));

    from_the_menu(&mut app, "s1", ShapeTarget::Side(0), ShapeAction::AddPoint).unwrap();
    assert_eq!(
        shown_t(&mut app, "s1").0,
        [
            (0.0, 0.0),
            (40.0, 0.0),
            (100.0, 0.0),
            (100.0, 100.0),
            (0.0, 100.0)
        ]
    );
    from_the_menu(
        &mut app,
        "s1",
        ShapeTarget::Point(1),
        ShapeAction::DeletePoint,
    )
    .unwrap();
    assert_eq!(
        shown_t(&mut app, "s1").0,
        [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
}

/// A refused edit says why and is no step.
#[test]
fn a_triangle_keeps_its_points_when_one_is_deleted_from_the_menu() {
    use crate::gate_editor::gates::shape_menu::{ShapeAction, ShapeTarget};
    let triangle = [(0.0, 0.0), (100.0, 10.0), (50.0, 100.0)];
    let mut app = App::new();
    a_gate_with_a_position_for_s1(
        &mut app,
        shape_t(&triangle, false),
        shape_t(&triangle, false),
    );
    let steps = app.standing().undo_steps;

    let refused = from_the_menu(
        &mut app,
        "s1",
        ShapeTarget::Point(0),
        ShapeAction::DeletePoint,
    );

    assert!(refused.unwrap_err().contains("at least 3"));
    assert_eq!(shown_t(&mut app, "s1").0.len(), 3);
    assert_eq!(app.standing().undo_steps, steps);
}

/// Made a polygon from one sample's plot, the rectangle is a polygon for
/// every sample, each where it was.
#[test]
fn a_rectangle_made_a_polygon_from_the_menu_is_one_for_every_sample() {
    use crate::gate_editor::gates::shape_menu::{ShapeAction, ShapeTarget};
    let drawn = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let own = [(50.0, 0.0), (60.0, 0.0), (60.0, 10.0), (50.0, 10.0)];
    let mut app = App::new();
    a_gate_with_a_position_for_s1(&mut app, shape_t(&drawn, true), shape_t(&own, true));
    let steps = app.standing().undo_steps;

    from_the_menu(
        &mut app,
        "s1",
        ShapeTarget::Rectangle,
        ShapeAction::ConvertToPolygon,
    )
    .unwrap();

    let as_points = |corners: &[(f32, f32)]| -> Vec<(f64, f64)> {
        corners
            .iter()
            .map(|(x, y)| (f64::from(*x), f64::from(*y)))
            .collect()
    };
    assert_eq!(shown_t(&mut app, "s1"), (as_points(&own), true));
    assert_eq!(shown_t(&mut app, "s2"), (as_points(&drawn), true));
    assert_eq!(app.standing().undo_steps, steps + 1);
    assert!(app.with(|held| held.edits.undo()));
    assert!(!shown_t(&mut app, "s1").1, "a rectangle again");
}
