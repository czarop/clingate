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

    /// One turn of the app: wait for work, then render.
    fn step(&mut self, wait: Duration) -> bool {
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
            held.rules_run.apply(&outcome, &inputs.rules).unwrap();
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
