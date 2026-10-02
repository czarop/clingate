//! The Gate Rules tab: say where each gate belongs, once.
//!
//! A rule names a *population* - "Ki67+ of CD4+" - rather than a container, so
//! one rule covers every place that gate appears. In a real export "CD279+"
//! occupied twenty-five containers; four rules covered the whole panel.

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::pairing_controls::PairingColumns;
use crate::gate_editor::path_picker::{Pick, PickPath};
use clingate_core::axis_store::{AxisStore, AxisStoreStoreExt};
use clingate_core::gate_rules::autogate::{PhenotypeRead, Report, describe};
use clingate_core::gate_rules::choices::{
    EdgeForm, beside, carry_over, choices, describe_phenotype, every_target, fallback_targets,
    follow_from_form, follow_to_form, marker_label, side_from, side_to,
};
use clingate_core::gate_rules::phenotype::MarkerRead;
use clingate_core::gate_rules::rule::{
    AboveTheNegativeRule, BandAim, Meet, NegativeFinder, NextToRule, PercentileOffsetRule,
    PhenotypeRule, Rule, ShapeFit, Side, TailFractionRule, ValleyOrSmearRule, ValleyRule,
};
use clingate_core::gate_rules::rule_store::{
    Bound, GateRule, MeasuredOn, RuleEntry, RuleStore, RuleTarget,
};
use clingate_core::gate_rules::run::{Progress, RunInputs, run_rules_pausing};
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::GateStateImplExt;
use clingate_core::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};
use dioxus::prelude::*;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/gate_rules.css");

/// Below this, a placement is worth opening by hand. Measured against the
/// hand-gated export: everything at or above it landed within 0.18 arcsinh
/// units of where a person had put it, which is a typical manual nudge.
const REVIEW_FLOOR: f64 = 0.30;

/// Below this, the verification table marks a gate's purity as worth a look.
///
/// Not a failure: a population that overlaps its neighbours on the two axes
/// the gate is drawn on cannot be gated cleanly there by anything, and the
/// honest response is to gate it somewhere else. The mark says "this number is
/// the reason to doubt the gate", which is what a person scanning a run needs.
const PURE_ENOUGH: f64 = 0.70;

/// How a phenotype rule fitted the gate, for the verification table.
fn fitted(read: &PhenotypeRead) -> String {
    let limited = if read.clamped { " (size limited)" } else { "" };
    match (read.reshaped, read.refused_outline) {
        (Some((dx, dy)), Some(area)) => format!(
            "shape kept: the polygon was {area:.1}x the area; moved {dx:+.0}, {dy:+.0}{limited}"
        ),
        (Some((dx, dy)), None) => format!("moved {dx:+.0}, {dy:+.0}{limited}"),
        (None, _) => "new polygon".to_string(),
    }
}

/// The frame a marker was read in, for its tooltip.
fn frame_of(marker: &MarkerRead) -> &'static str {
    if marker.by_landmarks {
        "0 at the negative's peak, 1 at the valley above it"
    } else {
        "in spreads from the parent's middle"
    }
}

/// A count as a percentage of its whole, for the report's tables.
fn fraction(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "-".to_string();
    }
    format!("{:.2}%", part as f64 / whole as f64 * 100.0)
}

/// A file id as the name a person knows it by, falling back to the id itself
/// when the metadata has not been loaded.
fn name_of(files: &[(Arc<str>, Arc<str>)], id: &str) -> String {
    files
        .iter()
        .find(|(_, file_id)| &**file_id == id)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// The loaded files, by the name a person recognises them by.
///
/// Rules address files by the id the gating document uses, which is opaque -
/// nobody picks `1TBQ` off a list. The metadata carries the mapping back to
/// the file name, so that is what the form shows.
fn loaded_files(map: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>) -> Vec<(Arc<str>, Arc<str>)> {
    let mut out: Vec<(Arc<str>, Arc<str>)> = map
        .iter()
        .map(|(name, id)| (name.clone(), id.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Raise a running solve's stop flag the moment anything it reads changes:
/// the gates, the files, their compensation, the metadata, the scaling or the
/// rules.
///
/// A run works on a snapshot and writes its answers when it finishes. The tabs
/// are hidden rather than unmounted, so it carries on while the Workspace tab
/// loads another document or the editor moves a gate, and its answers would
/// then land on a document it never measured. Raising the flag stops it at
/// the next file instead of finishing work that will be thrown away.
///
/// This is only the early stop. Whether to write is decided when the run
/// ends, by comparing what it started from with what is there now - an
/// effect runs after the change that fires it, and a run can finish in
/// between.
///
/// Subscribes to the parts of the gate store that hold the gating, not to its
/// root: selecting a gate writes the store too, and that is not a change.
/// ([`GateStateImplExt::subscribe_to_gating`] is where that line is drawn.)
pub(crate) fn use_stop_run_on_change(cancel: Signal<Option<Arc<std::sync::atomic::AtomicBool>>>) {
    let gates = use_context::<crate::gate_editor::workspace_window::GateStore>();
    let metadata = use_context::<crate::gate_editor::workspace_window::MetadataStore>();
    let axes = use_context::<crate::gate_editor::workspace_window::AxesStore>();
    let rules = use_context::<Signal<RuleStore>>();
    let files = use_context::<Signal<Option<clingate_core::file_load::FcsFiles>>>();
    let compensation = use_context::<Signal<clingate_core::compensation::groups::Compensation>>();
    use_effect(move || {
        gates.subscribe_to_gating();
        let _ = compensation.read();
        let _ = metadata.metadata().read();
        let _ = metadata.file_name_to_gating_id().read();
        let _ = axes.settings().read();
        let _ = rules.read();
        let _ = files.read();
        if let Some(flag) = cancel.peek().as_ref() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

/// What a rules run reads and writes, from the document every tab shares.
///
/// The tab's Run button goes through this, and so do the parity tests that
/// hold the app to the tools for Claude: whatever the button does, they do.
#[derive(Clone, Copy)]
pub(crate) struct RulesRun {
    gates: crate::gate_editor::workspace_window::GateStore,
    metadata: crate::gate_editor::workspace_window::MetadataStore,
    axes: crate::gate_editor::workspace_window::AxesStore,
    rules: Signal<RuleStore>,
    files: Signal<Option<clingate_core::file_load::FcsFiles>>,
    compensation: Signal<clingate_core::compensation::groups::Compensation>,
    edits: crate::gate_editor::edits::Edits,
}

impl RulesRun {
    pub(crate) fn from_context() -> Self {
        Self {
            gates: use_context(),
            metadata: use_context(),
            axes: use_context(),
            rules: use_context(),
            files: use_context(),
            compensation: use_context(),
            edits: use_context(),
        }
    }

    /// Everything the run reads, as it stands now - assembled as the tools
    /// for Claude assemble it - and the axis settings, compared whole: any
    /// new scaling is a new workspace as far as the answers are concerned.
    pub(crate) fn inputs_now(&self) -> (RunInputs, clingate_core::omiq::serialise::AxisSettings) {
        let axes = self.axes.settings().read().clone();
        let inputs = RunInputs::assemble(
            self.files.read().as_ref(),
            &self.compensation.read(),
            &self.metadata.read(),
            &axes,
            &self.rules.read(),
        );
        (inputs, axes)
    }

    /// A report of a badly placed gate, ready to gather off the UI thread:
    /// the workspace as it stands now, and the one function the tools for
    /// Claude report through. `None` with no workspace folder to keep it in.
    pub(crate) fn report_job(
        &self,
        request: clingate_core::review::ReportRequest,
    ) -> Option<
        impl FnOnce() -> Result<(std::path::PathBuf, clingate_core::review::PlacementReport), String>
        + Send
        + 'static
        + use<>,
    > {
        let folder = self.edits.folder()?;
        let (inputs, axes) = self.inputs_now();
        let gates = self.gates.peek().clone();
        Some(move || {
            let report =
                clingate_core::review::report::gather(&folder, &request, &gates, &inputs, &axes)?;
            let path = report.save(&folder).map_err(|e| e.to_string())?;
            Ok((path, report))
        })
    }

    /// Mark the workspace's last applied run reviewed, copying it into
    /// `library` - as the tools for Claude do.
    pub(crate) fn mark_reviewed(
        &self,
        library: Option<&std::path::Path>,
    ) -> Result<(clingate_core::review::RunReview, Option<std::path::PathBuf>), String> {
        let folder = self.edits.folder().ok_or("there is no workspace folder")?;
        clingate_core::review::report::mark_reviewed(
            &folder,
            &self.gates.peek(),
            &self.metadata.peek().metadata().clone(),
            library,
        )
    }

    /// A run's placements written as one step of the working copy, and the
    /// gates a paused run needs placed by hand each given a position of their
    /// own on their specimen, so moving one moves nobody else's.
    pub(crate) fn place(
        mut self,
        placements: &[clingate_core::gate_rules::autogate::Placement],
        hold: &[clingate_core::gate_rules::run::NeedsPlacing],
    ) {
        let metadata = self.metadata.peek().metadata().clone();
        let before = self.edits.before();
        {
            let mut gates = self.gates.write();
            clingate_core::gate_rules::autogate::apply_placements(
                &mut gates, placements, &metadata,
            );
            clingate_core::gate_rules::run::hold_for_placing(&mut gates, hold, &metadata);
        }
        self.edits.after(before);
    }

    /// A finished run kept in the workspace's `reviews` folder for reviewing.
    pub(crate) fn keep(
        &self,
        outcome: &clingate_core::gate_rules::run::RunOutcome,
        rules: &RuleStore,
    ) -> Result<(), String> {
        let record = clingate_core::review::RunRecord::of_run(
            &outcome.report,
            &outcome.placements,
            rules,
            &self.metadata.peek(),
        );
        match self.edits.folder() {
            Some(folder) => record
                .applied(&folder, &outcome.events)
                .map(|_| ())
                .map_err(|e| format!("the run could not be kept for review: {e}")),
            None => Ok(()),
        }
    }
}

#[component]
pub fn GateRulesWindow() -> Element {
    let run_with = RulesRun::from_context();
    let gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();
    let mut rules = use_context::<Signal<RuleStore>>();

    let choices = use_memo(move || choices(&gate_store.read()));
    let files = use_memo(move || loaded_files(&metadata_store.file_name_to_gating_id().read()));

    // The form.
    let mut gate = use_signal(String::new);
    let mut parent = use_signal(String::new);
    let mut parameter = use_signal(String::new);
    let mut bound = use_signal(|| "Above".to_string());
    let mut measured_on = use_signal(|| "FMX".to_string());
    let mut kind = use_signal(|| "TailFraction".to_string());
    let mut low = use_signal(|| "0.2".to_string());
    let mut high = use_signal(|| "0.5".to_string());
    let mut aim = use_signal(|| BandAim::default().key().to_string());
    let mut pool = use_signal(|| {
        clingate_core::gate_rules::rule::Pool::default()
            .key()
            .to_string()
    });
    let mut percentile = use_signal(|| "99".to_string());
    let mut offset = use_signal(|| "0.5".to_string());
    let mut calibrate_on = use_signal(String::new);
    let mut finder = use_signal(|| NegativeFinder::default().key().to_string());
    let mut scale = use_signal(|| "1.0".to_string());
    let mut smoothing = use_signal(|| "1.0".to_string());
    // A valley rule's fallback, as `RuleTarget::describe` writes it; empty
    // for none.
    let mut valley_fallback = use_signal(String::new);
    // The sample a valley-or-smear rule places smears from; empty for none.
    let mut smear_example = use_signal(String::new);
    let mut nudge = use_signal(|| "0.0".to_string());
    // The phenotype rule's own fields. `outline_smoothing` is separate from
    // `smoothing` above even though the two are never on screen together: one
    // scales a density's bandwidth along one axis and the other a boundary's
    // in two, and a shared signal would carry a number tuned for one rule into
    // the other.
    let mut fit = use_signal(|| ShapeFit::default().key().to_string());
    let mut markers = use_signal(Vec::<String>::new);
    let mut keep = use_signal(|| "95".to_string());
    let mut outline_smoothing = use_signal(|| "1.0".to_string());
    let mut vertices = use_signal(|| "24".to_string());
    // A rule from another gate: the whole shape of one gate ("shape"), or
    // edges set against others ("edges").
    let mut follow_mode = use_signal(|| "shape".to_string());
    let mut follow_anchor = use_signal(String::new);
    let mut follow_edges = use_signal(Vec::<EdgeForm>::new);
    // A rule next to another gate: that gate, as `RuleTarget::describe`
    // writes it, which side of it, and how it is met.
    let mut next_anchor = use_signal(String::new);
    let mut next_side = use_signal(|| "Lower".to_string());
    let mut next_meet = use_signal(|| Meet::default().key().to_string());
    let mut next_gap = use_signal(|| "0".to_string());
    let mut gated_file = use_signal(String::new);
    let mut reference_type = use_signal(|| "FMX".to_string());
    let mut reference_file = use_signal(String::new);
    // The workspace's files: what a run measures. The same list the editor and
    // the gallery show, so the three cannot be looking at different
    // experiments.
    let filehandler = use_context::<Signal<Option<clingate_core::file_load::FcsFiles>>>();
    let mut running = use_signal(|| false);
    let mut report = use_signal(|| None::<Report>);
    // A bare name is kept in the workspace folder's `rules` folder; the
    // default is the rules file the workspace opens with.
    let mut sidecar = use_signal(|| clingate_core::workspace::RULES_FILE.to_string());
    let loaded = use_context::<Signal<crate::gate_editor::workspace_window::Loaded>>();
    let sidecar_path =
        move || clingate_core::workspace::rules_path(loaded.peek().folder.as_deref(), &sidecar());
    let toasts = use_toast();
    // Not a message: it says what the form in front of you is currently doing,
    // and has to stay readable while you fill it in. A toast that faded after
    // five seconds would take away the one thing that distinguishes editing a
    // rule from adding one.
    let mut editing_note = use_signal(|| None::<String>);
    let mut progress = use_signal(|| None::<Progress>);
    // Set while a run is in flight, so the Stop button has something to raise.
    let mut cancel = use_signal(|| None::<Arc<std::sync::atomic::AtomicBool>>);
    use_stop_run_on_change(cancel);

    // A run stopped for gates to be placed by hand, and the editor's ask to go
    // on with it.
    let mut paused_run = use_context::<Signal<Option<crate::gate_editor::paused_run::PausedRun>>>();
    let go_on = use_context::<Signal<crate::gate_editor::paused_run::GoOn>>();
    let mut active = use_context::<Signal<crate::gate_editor::route::Tab>>();
    let generation = use_context::<Signal<crate::gate_editor::workspace_window::Generation>>();
    // A paused run whose next part did not go ahead is still waiting - unless
    // another workspace was opened meanwhile.
    let mut give_back = move |paused: Option<crate::gate_editor::paused_run::PausedRun>| {
        if let Some(paused) = paused
            && paused.is_on(generation.peek().document)
        {
            paused_run.set(Some(paused));
        }
    };

    // Runs the rules - or, handed a paused run, goes on with it from the
    // level it stopped before, measuring on the gates as they are now.
    let start_run = move |from: Option<crate::gate_editor::paused_run::PausedRun>| {
        let mut from = from;
        spawn(async move {
            if running() {
                give_back(from.take());
                return;
            }
            running.set(true);
            report.set(None);
            progress.set(Some(Progress::Measuring { done: 0, total: 0 }));

            // Everything the run reads, as it stands now. Read
            // again before anything is written: see `RunInputs`.
            //
            // The axis settings are compared whole, not only the
            // cofactors the run reads: any new scaling is a new
            // workspace as far as the answers are concerned.
            let inputs_now = move || run_with.inputs_now();
            let started = inputs_now();
            if started.0.files.is_empty() {
                warn(
                    &toasts,
                    "No FCS files are loaded - open a workspace on the first tab",
                );
                running.set(false);
                progress.set(None);
                give_back(from.take());
                return;
            }

            // A snapshot, not a lock. Every gate is behind an Arc,
            // so this is a refcount bump rather than a copy, and
            // the store is free for the rest of the editor the
            // moment it is taken.
            let snapshot = gate_store.read().clone();
            let started_gates = snapshot.clone();

            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            let stopped = flag.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();

            let from_level = from.as_ref().map_or(0, |paused| paused.next_level());
            let worker = {
                let inputs = started.0.clone();
                tokio::task::spawn_blocking(move || {
                    run_rules_pausing(
                        &snapshot,
                        &inputs,
                        from_level,
                        |step| {
                            let _ = tx.send(step);
                        },
                        &flag,
                    )
                })
            };

            // The worker's sender drops when it returns, which ends
            // this loop - no sentinel message to get wrong.
            while let Some(step) = rx.recv().await {
                progress.set(Some(step));
            }

            let outcome = worker.await;
            progress.set(None);
            cancel.set(None);
            running.set(false);

            let outcome = match outcome {
                Ok(o) => o,
                Err(e) => {
                    warn(&toasts, format!("The run did not finish: {e}"));
                    give_back(from.take());
                    return;
                }
            };
            // Answers measured on one workspace mean nothing in
            // another, whatever stopped or did not stop the run.
            // Checked here rather than trusted to the stop flag:
            // the flag is raised by an effect, which runs after the
            // change that fires it, and the run can finish first.
            if !gate_store.peek().unchanged_since(&started_gates) || inputs_now() != started {
                warn(
                    &toasts,
                    crate::gate_editor::paused_run::changed_while_running(from.is_some()),
                );
                give_back(from.take());
                return;
            }
            if outcome.cancelled || stopped.load(std::sync::atomic::Ordering::Relaxed) {
                note(&toasts, "Stopped - no gates were moved");
                give_back(from.take());
                return;
            }

            // Writing happens here, on the one thread that owns the
            // store, and only once the document is known to be the
            // one the run measured: each part of the run is one step
            // of the working copy.
            let needs = outcome
                .paused
                .as_ref()
                .map(|paused| paused.needs.clone())
                .unwrap_or_default();
            run_with.place(&outcome.placements, &needs);
            let whole = match from.take() {
                Some(paused) => paused.then(outcome),
                None => outcome,
            };
            if whole.paused.is_some() {
                let paused = crate::gate_editor::paused_run::PausedRun::new(
                    whole,
                    generation.peek().document,
                    started.0.rules.clone(),
                    &gate_store.peek(),
                );
                paused.adopt_rules(rules);
                warn(
                    &toasts,
                    crate::gate_editor::paused_run::paused_message(paused.needs()),
                );
                paused_run.set(Some(paused));
                active.set(crate::gate_editor::route::Tab::Editor);
                crate::gate_editor::paused_run::show_need(0);
                return;
            }
            if let Err(e) = run_with.keep(&whole, &started.0.rules) {
                warn(&toasts, e);
            }
            crate::gate_editor::review::reviews_changed();

            let run = whole.report;
            say(
                &toasts,
                format!(
                    "Moved {} gates, left {} already in band and {} reference; {} need review",
                    run.positioned.len(),
                    run.unchanged.len(),
                    run.reference.len(),
                    run.needs_review(REVIEW_FLOOR).count()
                ),
            );
            report.set(Some(run));
        });
    };
    use_effect(move || {
        if go_on().0 == 0 {
            return;
        }
        let Some(why_not) = paused_run
            .peek()
            .as_ref()
            .map(|paused| paused.why_not_go_on(&gate_store.peek(), &rules.peek()))
        else {
            return;
        };
        match why_not {
            Some(why_not) => warn(&toasts, why_not),
            None => start_run(paused_run.write().take()),
        }
    });

    // Picking a gate offers only the parents and parameters that gate is drawn
    // with, so the form cannot name a combination the document does not have.
    // Picking a parent narrows the gates, and picking a gate narrows the
    // parameters, so the form cannot name a combination the document lacks.
    let selected_children = use_memo(move || choices.read().children_of(&parent()).to_vec());
    let selected_parameters = use_memo(move || choices.read().parameters_of(&gate()).to_vec());
    // Every channel the panel carries, for the phenotype rule's marker picker.
    // The gate's own two parameters are not enough: a population is identified
    // by markers the plot it is drawn on says nothing about, which is the
    // entire reason that rule exists.
    let panel = use_memo(move || {
        axis_store
            .sorted_settings()
            .read()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    });

    // The rule the form is standing in for, when it was opened by Edit. The
    // next Add replaces it, so a rule can be moved to another population rather
    // than only removed and retyped. Duplicate leaves this empty, which is the
    // quick way to cover a second population with the same rule.
    let mut editing = use_signal(|| None::<RuleTarget>);

    // The last run's report names gate positions in a document that has gone
    // once the gates are replaced, and is cleared with it.
    let document = use_memo(move || generation.read().document);
    use_effect(move || {
        document();
        report.set(None);
    });
    // A rule being edited that is no longer in the store - a new workspace
    // starts without rules - leaves the form claiming to edit nothing.
    use_effect(move || {
        let gone = editing
            .read()
            .as_ref()
            .is_some_and(|target| rules.read().get(target).is_none());
        if gone {
            editing.set(None);
            editing_note.set(None);
        }
    });

    let mut load = move |entry: RuleEntry, replacing: bool| {
        parent.set(
            entry
                .target
                .parent
                .clone()
                .map(|p| p.to_string())
                .unwrap_or_default(),
        );
        gate.set(entry.target.gate.to_string());
        parameter.set(entry.rule.parameter.to_string());
        bound.set(
            match entry.rule.bound {
                Bound::Above => "Above",
                Bound::Below => "Below",
            }
            .to_string(),
        );
        match &entry.rule.measured_on {
            MeasuredOn::Itself => measured_on.set("Itself".to_string()),
            MeasuredOn::Partner(t) => measured_on.set(t.to_string()),
            MeasuredOn::File(f) => calibrate_on.set(f.to_string()),
        }
        smear_example.set(String::new());
        match &entry.rule.rule {
            Rule::TailFraction(r) => {
                kind.set("TailFraction".to_string());
                pool.set(r.pool.key().to_string());
                low.set(format!("{}", r.band.0 * 100.0));
                high.set(format!("{}", r.band.1 * 100.0));
                aim.set(r.aim.key().to_string());
            }
            Rule::PercentileOffset(r) => {
                kind.set("PercentileOffset".to_string());
                percentile.set(format!("{}", r.percentile));
                offset.set(format!("{}", r.offset));
            }
            Rule::AboveTheNegative(r) => {
                kind.set("AboveTheNegative".to_string());
                finder.set(r.find.key().to_string());
                scale.set(format!("{}", r.scale));
                nudge.set(format!("{}", r.nudge));
            }
            Rule::MatchThePhenotype(r) => {
                kind.set("MatchThePhenotype".to_string());
                fit.set(r.fit.key().to_string());
                markers.set(r.markers.iter().map(|m| m.to_string()).collect());
                keep.set(format!("{}", r.keep * 100.0));
                outline_smoothing.set(format!("{}", r.smoothing));
                vertices.set(format!("{}", r.vertices));
            }
            Rule::FromAnotherGate(r) => {
                kind.set("FromAnotherGate".to_string());
                let (same, edges) = follow_to_form(r);
                follow_mode.set(if same.is_some() { "shape" } else { "edges" }.to_string());
                follow_anchor.set(same.unwrap_or_default());
                follow_edges.set(edges);
            }
            Rule::NextToGate(r) => {
                kind.set("NextToGate".to_string());
                parameter.set(r.parameter.to_string());
                next_anchor.set(r.anchor.describe());
                next_side.set(side_to(r.side));
                next_meet.set(r.meet.key().to_string());
                next_gap.set(format!("{}", r.gap));
            }
            Rule::InTheValley(r) => {
                kind.set("InTheValley".to_string());
                smoothing.set(format!("{}", r.smoothing));
                valley_fallback.set(
                    r.fallback
                        .as_ref()
                        .map(RuleTarget::describe)
                        .unwrap_or_default(),
                );
            }
            Rule::ValleyOrSmear(r) => {
                kind.set("ValleyOrSmear".to_string());
                smoothing.set(format!("{}", r.smoothing));
                valley_fallback.set(
                    r.fallback
                        .as_ref()
                        .map(RuleTarget::describe)
                        .unwrap_or_default(),
                );
                smear_example.set(r.smear_example.as_deref().unwrap_or_default().to_string());
            }
        }
        editing.set(replacing.then(|| entry.target.clone()));
        editing_note.set(Some(if replacing {
            format!("Editing {} - Add rule saves it", entry.target.describe())
        } else {
            format!(
                "Copied {} - change it and Add rule",
                entry.target.describe()
            )
        }));
    };

    let mut add = move || {
        let name = gate();
        if name.is_empty() {
            warn(&toasts, "Choose a gate first");
            return;
        }
        let param = parameter();
        // A phenotype rule positions nothing along an axis, so there is no
        // parameter to name. Every other rule needs one.
        // Neither does a rule from another gate: its position is the other
        // gate's.
        let positions = !matches!(kind().as_str(), "MatchThePhenotype" | "FromAnotherGate");
        if param.is_empty() && positions {
            warn(&toasts, "Choose the parameter the rule positions");
            return;
        }
        let target = match parent().as_str() {
            "" => RuleTarget::named(name.as_str()),
            p => RuleTarget::under(name.as_str(), p),
        };
        let rule = match kind().as_str() {
            "MatchThePhenotype" => {
                let (Ok(k), Ok(sm), Ok(v)) = (
                    keep().parse::<f64>(),
                    outline_smoothing().parse::<f64>(),
                    vertices().parse::<usize>(),
                ) else {
                    warn(
                        &toasts,
                        "The percentage, smoothing and point count must be numbers",
                    );
                    return;
                };
                if !(0.0..=100.0).contains(&k) {
                    warn(&toasts, "The percentage to hold must be between 0 and 100");
                    return;
                }
                if fit() == ShapeFit::DrawPolygon.key() && v < 3 {
                    warn(&toasts, "A polygon needs at least three points");
                    return;
                }
                Rule::MatchThePhenotype(PhenotypeRule {
                    markers: markers().iter().map(|m| Arc::from(m.as_str())).collect(),
                    fit: match fit().as_str() {
                        "DrawPolygon" => ShapeFit::DrawPolygon,
                        _ => ShapeFit::KeepShape,
                    },
                    // Typed as a percentage, stored as a fraction.
                    keep: k / 100.0,
                    smoothing: sm,
                    vertices: v,
                })
            }
            "FromAnotherGate" => {
                let targets = every_target(&choices.read());
                let same = (follow_mode() == "shape").then(|| follow_anchor());
                match follow_from_form(same.as_deref(), &follow_edges(), &targets) {
                    Ok(rule) => Rule::FromAnotherGate(rule),
                    Err(e) => {
                        warn(&toasts, e);
                        return;
                    }
                }
            }
            "NextToGate" => {
                let Some(anchor) = beside(&choices.read(), &name, &parent())
                    .into_iter()
                    .find(|t| t.describe() == next_anchor())
                else {
                    warn(&toasts, "Choose the gate it sits next to");
                    return;
                };
                let gap = next_gap().trim().parse::<f64>().unwrap_or(f64::NAN);
                if let Some(problem) = clingate_core::gate_rules::rule::gap_problem(gap) {
                    warn(&toasts, problem);
                    return;
                }
                Rule::NextToGate(NextToRule {
                    anchor,
                    parameter: Arc::from(param.as_str()),
                    side: side_from(&next_side()).unwrap_or(Side::Lower),
                    meet: Meet::from_key(&next_meet()).unwrap_or_default(),
                    gap,
                })
            }
            "InTheValley" => {
                let Ok(sm) = smoothing().parse::<f64>() else {
                    warn(&toasts, "The smoothing must be a number");
                    return;
                };
                let fallback = fallback_targets(&choices.read(), &name, &parent())
                    .into_iter()
                    .find(|t| t.describe() == valley_fallback());
                Rule::InTheValley(ValleyRule {
                    smoothing: sm,
                    fallback,
                    ..ValleyRule::default()
                })
            }
            "ValleyOrSmear" => {
                let Ok(sm) = smoothing().parse::<f64>() else {
                    warn(&toasts, "The smoothing must be a number");
                    return;
                };
                let fallback = fallback_targets(&choices.read(), &name, &parent())
                    .into_iter()
                    .find(|t| t.describe() == valley_fallback());
                // An example was gated by hand for one gate: a rule moved to
                // another starts without.
                let example = smear_example();
                let same_gate = editing.peek().as_ref() == Some(&target);
                Rule::ValleyOrSmear(ValleyOrSmearRule {
                    smoothing: sm,
                    fallback,
                    smear_example: (same_gate && !example.is_empty())
                        .then(|| Arc::from(example.as_str())),
                    ..ValleyOrSmearRule::default()
                })
            }
            "AboveTheNegative" => {
                let (Ok(s), Ok(n)) = (scale().parse::<f64>(), nudge().parse::<f64>()) else {
                    warn(&toasts, "The scale and nudge must be numbers");
                    return;
                };
                Rule::AboveTheNegative(AboveTheNegativeRule {
                    scale: s,
                    nudge: n,
                    find: match finder().as_str() {
                        "NegativePeak" => NegativeFinder::NegativePeak,
                        _ => NegativeFinder::BelowTheGate,
                    },
                    ..AboveTheNegativeRule::default()
                })
            }
            "PercentileOffset" => {
                let (Ok(p), Ok(o)) = (percentile().parse::<f64>(), offset().parse::<f64>()) else {
                    warn(&toasts, "The percentile and offset must be numbers");
                    return;
                };
                Rule::PercentileOffset(PercentileOffsetRule::new(p, o))
            }
            _ => {
                let (Ok(l), Ok(h)) = (low().parse::<f64>(), high().parse::<f64>()) else {
                    warn(&toasts, "The band must be two numbers");
                    return;
                };
                if l > h {
                    warn(&toasts, "The band's lower bound is above its upper");
                    return;
                }
                // Typed as percentages, stored as fractions.
                Rule::TailFraction(TailFractionRule {
                    pool: clingate_core::gate_rules::rule::Pool::from_key(&pool())
                        .unwrap_or_default(),
                    ..TailFractionRule::aimed(
                        (l / 100.0, h / 100.0),
                        BandAim::from_key(&aim()).unwrap_or_default(),
                    )
                })
            }
        };
        // These read a named reference sample rather than a partner of each
        // specimen.
        let calibrated = matches!(
            kind().as_str(),
            "AboveTheNegative" | "InTheValley" | "ValleyOrSmear" | "MatchThePhenotype"
        );
        if calibrated && calibrate_on().is_empty() {
            warn(&toasts, "Choose the sample to calibrate against");
            return;
        }
        let described = target.describe();
        let follows = matches!(kind().as_str(), "FromAnotherGate" | "NextToGate");
        let rule = GateRule {
            // A rule from another gate reads its own sample, and positions
            // along no parameter of its own.
            parameter: Arc::from(if follows { "" } else { param.as_str() }),
            bound: if bound() == "Below" {
                Bound::Below
            } else {
                Bound::Above
            },
            measured_on: if follows {
                MeasuredOn::Itself
            } else if calibrated {
                // This rule calibrates against one named sample - the QC -
                // rather than a partner of each specimen.
                MeasuredOn::File(Arc::from(calibrate_on().as_str()))
            } else {
                match measured_on().as_str() {
                    "" | "Itself" => MeasuredOn::Itself,
                    t => MeasuredOn::Partner(Arc::from(t)),
                }
            },
            rule,
        };
        // An edit is saved where the rule stood, even when it moved the rule to
        // another population: the list's order is the user's, and a rule that
        // jumped to the bottom on every edit was a rule they could lose track
        // of.
        match editing.take() {
            Some(was) => rules.write().replace(&was, target, rule),
            None => {
                rules.write().insert(target, rule);
            }
        }
        say(&toasts, format!("Rule set for {described}"));
    };

    rsx! {
        document::Link { rel: "stylesheet", href: CSS_STYLE }
        div { class: "gate_rules",
            h2 { "Gate rules" }
            crate::gate_editor::edits::EditBar {}
            p { class: "gate_rules-hint",
                "A rule names a population, not a gate on one plot. Leaving the parent as "
                em { "any" }
                " applies it wherever that gate appears."
            }

            // ── the rules that exist ──────────────────────────────────────
            if rules.read().is_empty() {
                p { class: "gate_rules-empty", "No rules yet." }
            } else {
                table { class: "gate_rules-table",
                    thead {
                        tr {
                            th { "Applies to" }
                            th { "Parameter" }
                            th { "Keeps" }
                            th { "Measured on" }
                            th { "Rule" }
                            th { }
                        }
                    }
                    tbody {
                        for entry in rules.read().entries().to_vec() {
                            tr { key: "{entry.target.describe()}",
                                td { "{entry.target.describe()}" }
                                // A phenotype rule positions nothing along an
                                // axis, so showing the parameter and the side
                                // it keeps would be showing two fields it does
                                // not read.
                                if matches!(entry.rule.rule, Rule::MatchThePhenotype(_)) {
                                    td { class: "gate_rules-hint", "the whole shape" }
                                    td { }
                                } else if matches!(entry.rule.rule, Rule::FromAnotherGate(_)) {
                                    td { class: "gate_rules-hint", "from another gate" }
                                    td { }
                                } else if matches!(entry.rule.rule, Rule::NextToGate(_)) {
                                    td { class: "gate_rules-hint", "next to another gate" }
                                    td { }
                                } else {
                                    td { "{entry.rule.parameter}" }
                                    td {
                                        match entry.rule.bound {
                                            Bound::Above => "above",
                                            Bound::Below => "below",
                                        }
                                    }
                                }
                                td {
                                    match &entry.rule.measured_on {
                                        MeasuredOn::Itself => "the sample itself".to_string(),
                                        MeasuredOn::Partner(t) => format!("its {t}"),
                                        MeasuredOn::File(f) => name_of(&files.read(), f),
                                    }
                                }
                                // A phenotype rule names its markers by the
                                // column it reads, which is what it has to
                                // store and not what anybody ticked. The
                                // panel is the only place the two are
                                // connected, and it lives here rather than in
                                // the rule.
                                match &entry.rule.rule {
                                    Rule::MatchThePhenotype(r) => rsx! {
                                        td { "{describe_phenotype(r, &panel.read())}" }
                                    },
                                    other => rsx! {
                                        td { "{other.describe()}" }
                                    },
                                }
                                td { class: "gate_rules-actions",
                                    button {
                                        class: "gate_rules-secondary",
                                        title: "Open this rule in the form below. Saving replaces it, so changing the population moves the rule rather than copying it.",
                                        onclick: {
                                            let entry = entry.clone();
                                            move |_| load(entry.clone(), true)
                                        },
                                        "edit"
                                    }
                                    button {
                                        class: "gate_rules-secondary",
                                        title: "Open a copy in the form below, leaving this one alone - the quick way to cover a second population with the same rule.",
                                        onclick: {
                                            let entry = entry.clone();
                                            move |_| load(entry.clone(), false)
                                        },
                                        "duplicate"
                                    }
                                    button {
                                        class: "gate_rules-remove",
                                        onclick: {
                                            let target = entry.target.clone();
                                            move |_| {
                                                rules.write().remove(&target);
                                            }
                                        },
                                        "remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── adding one ────────────────────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Add a rule" }

                // Population first, then the gate drawn on it - the order a
                // person says it in, and the order that makes the second list
                // short enough to read.
                label { "Population" }
                select {
                    value: "{parent}",
                    onchange: move |e| {
                        let chosen = e.value();
                        let (keep_gate, keep_parameter) = carry_over(
                            &choices.read(),
                            &chosen,
                            &gate(),
                            &parameter(),
                        );
                        parent.set(chosen);
                        gate.set(keep_gate);
                        parameter.set(keep_parameter);
                    },
                    option { value: "", "choose a population" }
                    for name in choices.read().parents.clone() {
                        option { value: "{name}", "{name}" }
                    }
                }

                label { "Gate" }
                select {
                    value: "{gate}",
                    onchange: move |e| {
                        let chosen = e.value();
                        // A new phenotype rule starts from the markers the gate
                        // is drawn on; a different gate is drawn on different ones.
                        if kind() == "MatchThePhenotype" {
                            markers.set(clingate_core::gate_rules::choices::plot_markers(
                                choices.read().parameters_of(&chosen),
                                &panel.read(),
                            ));
                        }
                        gate.set(chosen);
                        parameter.set(String::new());
                    },
                    option { value: "", "choose a gate" }
                    // `selected` on the option as well as `value` on the
                    // select. Edit sets the population and the gate in one go,
                    // and this list is derived from the population - so the
                    // value can reach the select before the matching option
                    // exists, and a select given a value it has no option for
                    // falls back to the first one. Opening a rule then showed
                    // "choose a gate" for a rule that plainly named one.
                    // Marking the option is order-independent: the browser
                    // honours it whenever the option is appended.
                    //
                    // The population's own select needs none of this, because
                    // its options do not depend on anything the same update
                    // sets.
                    for name in selected_children.read().clone() {
                        option {
                            value: "{name}",
                            selected: gate() == *name,
                            "{name}"
                        }
                    }
                }

                // A phenotype rule fits a whole shape to a population, so it
                // has no parameter it positions along and no leading side.
                // Leaving the fields on screen would invite a person to set
                // something the rule then ignores.
                if !matches!(kind().as_str(), "MatchThePhenotype" | "FromAnotherGate") {
                    label { "Positions on" }
                    select {
                        value: "{parameter}",
                        onchange: move |e| parameter.set(e.value()),
                        option { value: "", "choose a parameter" }
                        // Derived from the gate, which Edit sets in the same
                        // update - see the note on the gate's own list.
                        for name in selected_parameters.read().clone() {
                            option {
                                value: "{name}",
                                selected: parameter() == *name,
                                "{clingate_core::gate_rules::choices::marker_label(&name, &panel.read())}"
                            }
                        }
                    }

                    // A gate next to another moves along a parameter, but no
                    // line of its own decides which events it keeps.
                    if kind() != "NextToGate" {
                        label { "Gate keeps events" }
                        select {
                            value: "{bound}",
                            onchange: move |e| bound.set(e.value()),
                            option { value: "Above", "above the line" }
                            option { value: "Below", "below the line" }
                        }
                    }
                }

                // The calibrated rules name one reference file rather than a
                // partner of each specimen, so the partner field means nothing.
                if !matches!(kind().as_str(), "AboveTheNegative" | "InTheValley" | "ValleyOrSmear" | "MatchThePhenotype" | "FromAnotherGate" | "NextToGate") {
                    label { "Measured on" }
                    input {
                        value: "{measured_on}",
                        oninput: move |e| measured_on.set(e.value()),
                        placeholder: "FMX, or Itself",
                    }
                }

                label { "Rule" }
                select {
                    value: "{kind}",
                    onchange: move |e| {
                        let chosen = e.value();
                        if chosen == "MatchThePhenotype" && markers().is_empty() {
                            markers.set(clingate_core::gate_rules::choices::plot_markers(
                                &selected_parameters.read(),
                                &panel.read(),
                            ));
                        }
                        kind.set(chosen);
                    },
                    option { value: "TailFraction", "capture a percentage of the parent" }
                    option { value: "PercentileOffset", "step above a percentile" }
                    option { value: "ValleyOrSmear", "in the valley, or on a smear as on one gated by hand" }
                    // Replaced by the one above; offered only to a rule that
                    // already is one, so it can still be edited.
                    if kind() == "AboveTheNegative" {
                        option { value: "AboveTheNegative", "above the negative, as on a reference sample" }
                    }
                    if kind() == "InTheValley" {
                        option { value: "InTheValley", "in the valley between the negative and the positive" }
                    }
                    option { value: "MatchThePhenotype", "find the cells that match the reference population" }
                    option { value: "FromAnotherGate", "from another gate: its position, or against its edge" }
                    option { value: "NextToGate", "next to another gate: up against it, touching but not over it" }
                }

                if kind() == "NextToGate" {
                    p { class: "gate_rules-hint gate_rules-span",
                        "Brings this gate up against another on the same plot, as close as it can be without overlapping it, on each sample. A run places that gate first, so settle it before relying on this one. Rectangles and polygons only."
                    }
                    label { "Next to" }
                    select {
                        value: "{next_anchor}",
                        onchange: move |e| next_anchor.set(e.value()),
                        option { value: "", "choose the gate beside it" }
                        for target in beside(&choices.read(), &gate(), &parent()) {
                            option {
                                value: "{target.describe()}",
                                selected: next_anchor() == target.describe(),
                                "{target.describe()}"
                            }
                        }
                    }
                    label { "On its" }
                    select {
                        value: "{next_side}",
                        onchange: move |e| next_side.set(e.value()),
                        option { value: "Lower", "lower side - to its left, or below it" }
                        option { value: "Upper", "upper side - to its right, or above it" }
                    }
                    label { "By" }
                    select {
                        value: "{next_meet}",
                        onchange: move |e| next_meet.set(e.value()),
                        for meet in Meet::ALL {
                            option {
                                value: "{meet.key()}",
                                selected: next_meet() == meet.key(),
                                "{meet.label()}"
                            }
                        }
                    }
                    label { "Gap" }
                    input {
                        r#type: "number",
                        step: "any",
                        value: "{next_gap}",
                        oninput: move |e| next_gap.set(e.value()),
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Growing moves the side facing the other gate, every point alike, and keeps the far side where it is; following its outline makes the facing side take the shape of the other's side facing it, not its top or bottom, where the two lie alongside (polygons); sliding moves the gate whole. The gap is left between them, in the plot's units."
                    }
                }

                if kind() == "FromAnotherGate" {
                    p { class: "gate_rules-hint gate_rules-span",
                        "Puts this gate where another is on the same sample - its whole shape, or one edge against another gate's edge. A run places the gate it follows first, so settle that gate before relying on this one."
                    }
                    label { "Follows" }
                    select {
                        value: "{follow_mode}",
                        onchange: move |e| follow_mode.set(e.value()),
                        option { value: "shape", "the same shape as" }
                        option { value: "edges", "edges set against other gates" }
                    }
                    if follow_mode() == "shape" {
                        label { "Gate" }
                        select {
                            value: "{follow_anchor}",
                            onchange: move |e| follow_anchor.set(e.value()),
                            option { value: "", "choose the gate it follows" }
                            for target in every_target(&choices.read()) {
                                option {
                                    value: "{target.describe()}",
                                    selected: follow_anchor() == target.describe(),
                                    "{target.describe()}"
                                }
                            }
                        }
                    } else {
                        for (at , row) in follow_edges().into_iter().enumerate() {
                            label { "Edge {at + 1}" }
                            div { class: "gate_rules-edge",
                                select {
                                    value: "{row.side}",
                                    onchange: move |e| follow_edges.with_mut(|v| v[at].side = e.value()),
                                    option { value: "Lower", "its lower edge" }
                                    option { value: "Upper", "its upper edge" }
                                }
                                " on "
                                select {
                                    value: "{row.parameter}",
                                    onchange: move |e| follow_edges.with_mut(|v| v[at].parameter = e.value()),
                                    option { value: "", "parameter" }
                                    for name in selected_parameters.read().clone() {
                                        option {
                                            value: "{name}",
                                            selected: row.parameter == *name,
                                            "{marker_label(&name, &panel.read())}"
                                        }
                                    }
                                }
                                " at the "
                                select {
                                    value: "{row.anchor_side}",
                                    onchange: move |e| follow_edges.with_mut(|v| v[at].anchor_side = e.value()),
                                    option { value: "Lower", "lower edge" }
                                    option { value: "Upper", "upper edge" }
                                }
                                " of "
                                select {
                                    value: "{row.anchor}",
                                    onchange: move |e| follow_edges.with_mut(|v| v[at].anchor = e.value()),
                                    option { value: "", "choose a gate" }
                                    for target in every_target(&choices.read()) {
                                        option {
                                            value: "{target.describe()}",
                                            selected: row.anchor == target.describe(),
                                            "{target.describe()}"
                                        }
                                    }
                                }
                                " plus "
                                input {
                                    value: "{row.gap}",
                                    placeholder: "0",
                                    oninput: move |e| follow_edges.with_mut(|v| v[at].gap = e.value()),
                                }
                                button {
                                    class: "gate_rules-secondary",
                                    onclick: move |_| {
                                        follow_edges.with_mut(|v| {
                                            v.remove(at);
                                        })
                                    },
                                    "remove"
                                }
                            }
                        }
                        button {
                            class: "gate_rules-secondary gate_rules-span",
                            onclick: move |_| {
                                follow_edges
                                    .with_mut(|v| {
                                        v.push(EdgeForm {
                                            side: "Upper".into(),
                                            anchor_side: "Lower".into(),
                                            ..EdgeForm::default()
                                        })
                                    })
                            },
                            "add an edge"
                        }
                    }
                }

                if kind() == "MatchThePhenotype" {
                    p { class: "gate_rules-hint gate_rules-span",
                        "Describes the cells inside the gate on the reference sample by where they sit across the markers below, then finds the same cells in every other sample and fits the gate to wherever they turn out to be. For populations the other rules cannot reach: a smear with no dip, several clusters near each other, anything that moves in both axes at once. Nothing is normalised between samples - each one's markers are read against its own parent - so donor differences are carried rather than flattened."
                    }

                    {calibrate_picker(calibrate_on, files)}

                    label { "Identified by" }
                    div { class: "gate_rules-markers",
                        if panel.read().is_empty() {
                            span { class: "gate_rules-hint",
                                "No panel loaded yet - open a file on the plots tab first."
                            }
                        }
                        for param in clingate_core::gate_rules::choices::marker_panel(&panel.read()) {
                            label { class: "gate_rules-marker",
                                input {
                                    r#type: "checkbox",
                                    checked: markers().iter().any(|m| *m == *param.fluoro),
                                    onchange: {
                                        let fluoro = param.fluoro.clone();
                                        move |e: FormEvent| {
                                            let name = fluoro.to_string();
                                            let mut chosen = markers();
                                            if e.checked() {
                                                if !chosen.contains(&name) {
                                                    chosen.push(name);
                                                }
                                            } else {
                                                chosen.retain(|m| *m != name);
                                            }
                                            markers.set(chosen);
                                        }
                                    },
                                }
                                "{param}"
                            }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        if markers().is_empty() {
                            "Nothing ticked means the whole panel. Narrowing it is usually better: a marker that says nothing about this population still contributes noise to the distance, so ticking the four or five that define it beats ticking thirty."
                        } else {
                            "{markers().len()} ticked, and only these are read - the gate's own two markers are not added behind the scenes, so they start ticked here. A marker that says nothing about this population still contributes noise to the distance, so fewer and more relevant beats more."
                        }
                    }

                    label { "Then" }
                    select {
                        value: "{fit}",
                        onchange: move |e| fit.set(e.value()),
                        for choice in ShapeFit::ALL {
                            option { value: "{choice.key()}", "{choice.choice()}" }
                        }
                    }

                    if fit() == ShapeFit::DrawPolygon.key() {
                        p { class: "gate_rules-hint gate_rules-span",
                            "The gate becomes a polygon whatever it is now, because no other shape can follow a traced boundary. A rectangle or an ellipse will stop being one."
                        }

                        label { "Hold this much (%)" }
                        input {
                            r#type: "number",
                            step: "1",
                            value: "{keep}",
                            oninput: move |e| keep.set(e.value()),
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "Of the matched cells. Not all of them: the last few percent are the ones the match is least sure about, and a boundary drawn to include them is drawn around the doubt."
                        }

                        label { "Boundary smoothing" }
                        input {
                            r#type: "number",
                            step: "0.1",
                            value: "{outline_smoothing}",
                            oninput: move |e| outline_smoothing.set(e.value()),
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "Below 1 follows the cells more closely and picks up their noise; above 1 gives a smoother outline that may cut corners off a genuinely angular population."
                        }

                        label { "About this many points" }
                        input {
                            r#type: "number",
                            step: "1",
                            value: "{vertices}",
                            oninput: move |e| vertices.set(e.value()),
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "A gate with two hundred points is a different kind of object from one drawn by hand, however well it fits."
                        }
                    } else {
                        p { class: "gate_rules-hint gate_rules-span",
                            "The gate is moved and resized onto the matched cells and keeps its shape and its kind - a rectangle stays a rectangle. Use this where the outline means something the data does not: a quadrant, a shape agreed with somebody else, a gate that has to stay comparable with how it was drawn before."
                        }
                    }
                }

                if kind() == "ValleyOrSmear" {
                    {calibrate_picker(calibrate_on, files)}
                    p { class: "gate_rules-hint gate_rules-span",
                        "Reads each sample for a dip between its negative and its positive. Where there is one, the gate goes in it, offset as on the reference. Where there is none - a smear - the gate goes as far above the negative, in widths of the negative, as on a smear gated by hand."
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "A reference that is a smear is that example. A reference with a dip says nothing about where to cut a smear, so the run stops at the first smear for you to gate it in the editor; Continue the run, and that sample is the example every other smear is placed from. Save the rules to keep it."
                    }

                    label { "Smoothing" }
                    input {
                        r#type: "number",
                        step: "0.1",
                        value: "{smoothing}",
                        oninput: move |e| smoothing.set(e.value()),
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Scales the density's bandwidth. Below 1 finds shallower dips and more noise; above 1 smooths shallow ones away."
                    }

                    label { "On a smear" }
                    select {
                        value: "{valley_fallback}",
                        onchange: move |e| valley_fallback.set(e.value()),
                        option { value: "", "as on a smear gated by hand" }
                        for target in fallback_targets(&choices.read(), &gate(), &parent()) {
                            option {
                                value: "{target.describe()}",
                                selected: valley_fallback() == target.describe(),
                                "where {target.describe()} is"
                            }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Or its edge goes where the same gate's is under another parent, on the same sample: a run places that gate first, and every placement made this way comes up for review."
                    }

                    if !smear_example().is_empty() {
                        label { "Smear example" }
                        div {
                            span { "{file_name(&files.read(), &smear_example())} " }
                            button {
                                onclick: move |_| smear_example.set(String::new()),
                                "Forget"
                            }
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "Smears are placed from this sample, as it is gated now. Forget it, and the next run stops at a smear for you to gate another."
                        }
                    }
                }

                if kind() == "InTheValley" {
                    {calibrate_picker(calibrate_on, files)}
                    p { class: "gate_rules-hint gate_rules-span",
                        "Finds the dip between the negative and the positive on each sample and puts the gate at its lowest point, offset by however far from the bottom the gate sits on the reference. It reads the boundary rather than pacing out from the negative's centre, so nothing is multiplied and a shallower dip still places correctly. It needs two populations: where the positives are a smear with no peak of their own, use above-the-negative instead."
                    }

                    p { class: "gate_rules-hint gate_rules-span",
                        "A shallower dip than the reference's still gets a gate - refusing hid the answer exactly where it was most wanted - but it is scored on how deep it is against the reference's, and a shallow one rises to the top for review. Only a density with no dip at all is left unplaced, because then there is nothing to place."
                    }

                    label { "Smoothing" }
                    input {
                        r#type: "number",
                        step: "0.1",
                        value: "{smoothing}",
                        oninput: move |e| smoothing.set(e.value()),
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Scales the density's bandwidth. Below 1 finds shallower dips and more noise; above 1 smooths shallow ones away."
                    }

                    label { "With no dip" }
                    select {
                        value: "{valley_fallback}",
                        onchange: move |e| valley_fallback.set(e.value()),
                        option { value: "", "leave the gate unplaced" }
                        for target in fallback_targets(&choices.read(), &gate(), &parent()) {
                            option {
                                value: "{target.describe()}",
                                selected: valley_fallback() == target.describe(),
                                "where {target.describe()} is"
                            }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "For a sample whose positives smear with no dip: its edge goes where the same gate's is under another parent, on the same sample. A run places that gate first, and every placement made this way comes up for review."
                    }
                }

                if kind() == "AboveTheNegative" {
                    {calibrate_picker(calibrate_on, files)}
                    p { class: "gate_rules-hint gate_rules-span",
                        "Reads how far above that sample's negative its gate sits, in widths of that negative, and puts every other gate the same number of widths above its own. No FMO needed - the negative is read from the sample being gated."
                    }

                    label { "Find the negative" }
                    select {
                        value: "{finder}",
                        onchange: move |e| finder.set(e.value()),
                        for option_ in NegativeFinder::ALL {
                            option { value: "{option_.key()}", "{option_.choice()}" }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Pick by what the plot looks like. A real valley between negative and positive - use the events below the gate: the gate already sits in that valley, so everything under it is the negative, and it measured about five times the sharper. Dim cells rising out of the negative with no valley - use the negative's own peak: below-the-gate swallows the smear, and its centre drifts about 0.27 of a width as the smear grows from 4% to 35% of the population, where the peak finder drifts 0.09. Neither can tell a spillover shoulder from another channel apart from real dim expression, so place those by hand."
                    }

                    label { "Scale" }
                    input {
                        r#type: "number",
                        step: "0.05",
                        value: "{scale}",
                        oninput: move |e| scale.set(e.value()),
                    }
                    label { "Nudge" }
                    input {
                        r#type: "number",
                        step: "0.01",
                        value: "{nudge}",
                        oninput: move |e| nudge.set(e.value()),
                    }
                } else if kind() == "PercentileOffset" {
                    label { "Percentile" }
                    input {
                        r#type: "number",
                        value: "{percentile}",
                        oninput: move |e| percentile.set(e.value()),
                    }
                    label { "Offset" }
                    input {
                        r#type: "number",
                        step: "0.1",
                        value: "{offset}",
                        oninput: move |e| offset.set(e.value()),
                    }
                } else if kind() == "TailFraction" {
                    label { "Capture between (%)" }
                    div { class: "gate_rules-band",
                        input {
                            r#type: "number",
                            step: "0.01",
                            value: "{low}",
                            oninput: move |e| low.set(e.value()),
                        }
                        span { "to" }
                        input {
                            r#type: "number",
                            step: "0.01",
                            value: "{high}",
                            oninput: move |e| high.set(e.value()),
                        }
                    }
                    label { "Where in the band" }
                    select {
                        value: "{aim}",
                        onchange: move |e| aim.set(e.value()),
                        for option_ in BandAim::ALL {
                            option { value: "{option_.key()}", "{option_.choice()}" }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "The search halves its range each step. Anywhere stops at the first position inside the band, so where it lands depends on the population's most extreme events - two alike samples can land at opposite edges. The middle carries on until the gate holds the band's middle fraction, the same on every sample."
                    }
                    label { "Counted on" }
                    select {
                        value: "{pool}",
                        onchange: move |e| pool.set(e.value()),
                        for option_ in clingate_core::gate_rules::rule::Pool::ALL {
                            option {
                                value: "{option_.key()}",
                                selected: pool() == option_.key(),
                                "{option_.choice()}"
                            }
                        }
                    }
                }

                button { class: "gate_rules-add", onclick: move |_| add(), "Add rule" }
            }

            // ── which files belong together ───────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Sample pairing" }
                p { class: "gate_rules-hint gate_rules-span",
                    "Naming conventions differ between datasets, so the columns that group a specimen's files are named here rather than guessed."
                }

                PairingColumns {}
            }

            // ── references chosen by hand ─────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Reference files" }
                p { class: "gate_rules-hint gate_rules-span",
                    "The pairing finds the right partner in the ordinary case. Where it cannot - an FMO that was never run, a stand-in from another specimen - name the file to measure here."
                }

                if !rules.read().references().is_empty() {
                    div { class: "gate_rules-span",
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Gating" }
                                    th { "Asking for" }
                                    th { "Measures" }
                                    th { }
                                }
                            }
                            tbody {
                                for reference in rules.read().references().to_vec() {
                                    tr { key: "{reference.gated}/{reference.sample_type}",
                                        td { "{name_of(&files.read(), &reference.gated)}" }
                                        td { "{reference.sample_type}" }
                                        td { "{name_of(&files.read(), &reference.reference)}" }
                                        td {
                                            button {
                                                class: "gate_rules-remove",
                                                onclick: {
                                                    let gated = reference.gated.clone();
                                                    let sample_type = reference.sample_type.clone();
                                                    move |_| {
                                                        rules.write().clear_reference(&gated, &sample_type);
                                                    }
                                                },
                                                "clear"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                label { "When gating" }
                select {
                    value: "{gated_file}",
                    onchange: move |e| gated_file.set(e.value()),
                    option { value: "", "choose a file" }
                    // Marked, not only valued: see the gate's list.
                    for (name , id) in files.read().clone() {
                        option { value: "{id}", selected: gated_file() == *id, "{name}" }
                    }
                }

                label { "And the rule asks for" }
                input {
                    value: "{reference_type}",
                    oninput: move |e| reference_type.set(e.value()),
                    placeholder: "FMX",
                }

                label { "Measure instead" }
                select {
                    value: "{reference_file}",
                    onchange: move |e| reference_file.set(e.value()),
                    option { value: "", "choose a file" }
                    for (name , id) in files.read().clone() {
                        option { value: "{id}", selected: reference_file() == *id, "{name}" }
                    }
                }

                button {
                    class: "gate_rules-add",
                    onclick: move |_| {
                        let (gated, reference) = (gated_file(), reference_file());
                        if gated.is_empty() || reference.is_empty() {
                            warn(&toasts, "Choose both files");
                            return;
                        }
                        rules
                            .write()
                            .set_reference(
                                Arc::from(gated.as_str()),
                                Arc::from(reference_type().as_str()),
                                Arc::from(reference.as_str()),
                            );
                        say(&toasts, "Reference set");
                    },
                    "Set reference"
                }
            }

            // ── running the rules ─────────────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Autogate" }
                p { class: "gate_rules-hint gate_rules-span",
                    "Solves every rule above against the loaded workflow and gives each specimen its own gate. The position drawn by hand stays put underneath, so this can be re-run or ignored. If a gate with other ruled gates under it cannot be placed on a sample, or is placed with a confidence under 0.2, the run stops and the editor shows each one to place by hand before the gates under it are measured."
                }

                p { class: "gate_rules-hint gate_rules-span",
                    match filehandler.read().as_ref().map(|f| f.sample_count()) {
                        Some(n) if n > 0 => format!("Runs over the {n} FCS files in the workspace."),
                        _ => "No FCS files are loaded - open a workspace on the first tab.".to_string(),
                    }
                }

                if paused_run.read().is_some() {
                    p { class: "gate_rules-hint gate_rules-span",
                        "A run is paused for gates to be placed by hand - continue or stop it from the banner in the editor."
                    }
                }
                button {
                    class: "gate_rules-add",
                    disabled: running() || paused_run.read().is_some(),
                    onclick: move |_| start_run(None),
                    if running() { "Working..." } else { "Solve and apply" }
                }

                if let Some(step) = progress() {
                    div { class: "gate_rules-progress gate_rules-span",
                        div { class: "gate_rules-bar",
                            div {
                                class: "gate_rules-bar_fill",
                                style: "width: {step.fraction() * 100.0}%",
                            }
                        }
                        span { class: "gate_rules-progress_text", "{step.describe()}" }
                        button {
                            class: "gate_rules-cancel",
                            onclick: move |_| {
                                if let Some(flag) = cancel() {
                                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                    note(&toasts, "Stopping...");
                                }
                            },
                            "Stop"
                        }
                    }
                }
            }

            // ── the sidecar ───────────────────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Rules file" }
                label { "File" }
                div { class: "gate_rules-path",
                    input {
                        value: "{sidecar}",
                        oninput: move |e| sidecar.set(e.value()),
                    }
                    // Choosing one that exists, for Load. Naming one to write
                    // is the button beside Save; they are different dialogs,
                    // and an open dialog cannot name a file that is not there.
                    PickPath {
                        path: sidecar,
                        mode: Pick::OpenFile,
                        label: "Rules",
                        extensions: vec!["json".to_string()],
                    }
                }
                div { class: "gate_rules-band gate_rules-actions_row",
                    button {
                        onclick: move |_| {
                            let path = sidecar_path();
                            match rules.read().save(&path) {
                                Ok(()) => say(&toasts, format!("Saved to {}", path.display())),
                                Err(e) => warn(&toasts, format!("Could not save: {e}")),
                            }
                        },
                        "Save"
                    }
                    // Joined to Save, not floating between the two actions:
                    // this dialog names where to write, which is Save's
                    // question and not Load's.
                    PickPath {
                        path: sidecar,
                        mode: Pick::SaveFile,
                        label: "Rules",
                        extensions: vec!["json".to_string()],
                    }
                    span { class: "gate_rules-gap" }
                    button {
                        onclick: move |_| {
                            let path = sidecar_path();
                            match RuleStore::load(&path) {
                                Ok(loaded) => {
                                    let n = loaded.len();
                                    rules.set(loaded);
                                    say(&toasts, format!("Loaded {n} rules"));
                                }
                                Err(e) => warn(&toasts, format!("Could not load: {e}")),
                            }
                        },
                        "Load"
                    }
                }
                if let Some(folder) = loaded.read().folder.clone() {
                    p { class: "gate_rules-hint",
                        "A name alone is kept in {folder.join(clingate_core::workspace::RULES_DIR).display()}, and the workspace opens with {clingate_core::workspace::RULES_FILE} there."
                    }
                }
            }

            crate::gate_editor::review::ReviewPanel {}

            if let Some(text) = editing_note() {
                p { class: "gate_rules-message", "{text}" }
            }

            // What a run did, last, in a box that scrolls: thousands of lines
            // would otherwise push the sections above out of reach.
            if let Some(run) = report.read().as_ref() {
                div { class: "gate_rules-report",
                    // Two kinds of placement, two tables. A phenotype rule's
                    // "from" and "to" are fractions of the parent and every
                    // other rule's are coordinates on an axis, so one table
                    // would put an axis value in the same column as a
                    // percentage and label them both From.
                    if run.positioned.iter().any(|p| p.phenotype.is_none()) {
                        h3 { "Positioned" }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Measured on" }
                                    th { "From" }
                                    th { "To" }
                                    th { "Captured" }
                                    th { "Line only" }
                                    th { "Confidence" }
                                    th { "Weakest" }
                                }
                            }
                            tbody {
                                for placed in run.positioned.iter().filter(|p| p.phenotype.is_none()) {
                                    tr {
                                        class: if placed.needs_review(REVIEW_FLOOR) { "gate_rules-weak" } else { "" },
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{name_of(&files.read(), &placed.measured_on)}" }
                                        td { "{placed.from:.3}" }
                                        td { "{placed.to:.3}" }
                                        td {
                                            title: "of {placed.reference_events} events on {name_of(&files.read(), &placed.captured_on)}",
                                            "{placed.achieved * 100.0:.3}%"
                                            if !placed.in_band {
                                                " (outside the band - nearest achievable)"
                                            }
                                        }
                                        td {
                                            class: if placed.above_the_line > 0.0
                                                && placed.achieved < placed.above_the_line * 0.5 { "gate_rules-weak" } else { "" },
                                            title: "what a bare threshold on this parameter would take, the gate's other sides ignored - far above the captured figure means the gate's other axis is discarding the events",
                                            "{placed.above_the_line * 100.0:.3}%"
                                        }
                                        td { "{placed.confidence:.2}" }
                                        td { "{placed.weakest.unwrap_or(\"-\")}" }
                                    }
                                }
                            }
                        }
                    }
                    if run.positioned.iter().any(|p| p.phenotype.is_some()) {
                        h3 { "Matched by phenotype" }
                        p { class: "gate_rules-hint",
                            "A gate drawn round the wrong cells looks exactly like one drawn round the right cells until these are read. Each marker shows where the matched cells sat on the reference and where they sit here, read the same way on both: 0 at the negative's peak and 1 at the valley above it, or in spreads from the parent's middle where a sample has no valley. A marker the population is positive or negative on only has to stay on the same side of the valley, however much brighter or dimmer; a dim one has to stay close. A sample where it does not, or where the cells are too few, too rare or scattered, is left where it was and listed with the reason."
                        }
                        div { class: "gate_rules-verify",
                            table { class: "gate_rules-table",
                                thead {
                                    tr {
                                        th { "Specimen" }
                                        th { "Gate" }
                                        th { "Matched" }
                                        th { "On the reference" }
                                        th { "Only this population" }
                                        th { "Of the population" }
                                        th { "Clouds" }
                                        th { "Fitted" }
                                        th { "Marker, reference to here" }
                                        th { "Confidence" }
                                        th { "Weakest" }
                                    }
                                }
                                tbody {
                                    for placed in run.positioned.iter() {
                                        if let Some(read) = placed.phenotype.as_ref() {
                                            tr {
                                                class: if placed.confidence < REVIEW_FLOOR { "gate_rules-weak" } else { "" },
                                                td { "{placed.specimen}" }
                                                td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                                td {
                                                    title: "of {read.parent} events in the parent population",
                                                    "{read.matched} ({fraction(read.matched, read.parent)})"
                                                }
                                                td {
                                                    title: "what the hand-drawn gate held on {name_of(&files.read(), &placed.measured_on)}",
                                                    "{read.reference_matched} ({fraction(read.reference_matched, read.reference_parent)})"
                                                }
                                                td {
                                                    class: if read.purity < PURE_ENOUGH { "gate_rules-doubt" } else { "" },
                                                    title: "of what the fitted gate holds. Low means the population is not separated on these two axes, so no boundary round it can exclude its neighbours - which is a fact about the plot, not a fault in the fit",
                                                    "{read.purity * 100.0:.0}%"
                                                }
                                                td {
                                                    title: "how much of the matched population the fitted gate holds",
                                                    "{read.caught * 100.0:.0}%"
                                                }
                                                td {
                                                    class: if read.pieces > 1 { "gate_rules-doubt" } else { "" },
                                                    title: if read.pieces > 1 { "the matched cells sit in more than one place on this plot, so one outline is not the whole story" } else { "the matched cells form a single cloud" },
                                                    "{read.pieces}"
                                                }
                                                td {
                                                    {fitted(read)}
                                                }
                                                td {
                                                    for marker in read.centres.iter() {
                                                        span {
                                                            class: if marker.drifted() { "gate_rules-doubt" } else { "" },
                                                            title: "{marker.marker}: {marker.reference_middle:+.2} on the reference, {marker.middle:+.2} here, {frame_of(marker)}",
                                                            // The marker as it was ticked, not the
                                                            // column it was read from.
                                                            "{marker_label(&marker.marker, &panel.read())} {marker.reference_middle:+.1}→{marker.middle:+.1}  "
                                                        }
                                                    }
                                                }
                                                td { "{placed.confidence:.2}" }
                                                td { "{placed.weakest.unwrap_or(\"-\")}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !run.unchanged.is_empty() {
                        h3 { "Already in band - left alone" }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Captures" }
                                    th { "Line only" }
                                }
                            }
                            tbody {
                                for kept in run.unchanged.iter() {
                                    tr {
                                        td { "{kept.specimen}" }
                                        td { "{describe(&kept.gate, kept.parent_gate.as_deref())}" }
                                        td { "{kept.achieved * 100.0:.3}%" }
                                        td { "{kept.above_the_line * 100.0:.3}%" }
                                    }
                                }
                            }
                        }
                    }
                    if run.positioned.iter().any(|p| p.valley.is_some()) {
                        h3 { "How the valley was read" }
                        p { class: "gate_rules-note",
                            "The gate sits at the lowest point of the dip, offset by however far from the bottom it sits on the reference. Depth is how far the dip falls below the lower of the two peaks either side: a shallower dip than the reference's still places correctly, but one near zero means the two populations have merged."
                        }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Ref peak" }
                                    th { "Ref bottom" }
                                    th { "Ref depth" }
                                    th { "Peak" }
                                    th { "Bottom" }
                                    th { "Depth" }
                                    th { "Offset" }
                                    th { "Placed at" }
                                }
                            }
                            tbody {
                                for (placed , (reference , here)) in run
                                    .positioned
                                    .iter()
                                    .filter_map(|p| p.valley.map(|v| (p, v)))
                                {
                                    tr {
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{reference.peak:.3}" }
                                        td { "{reference.bottom:.3}" }
                                        td { "{reference.depth:.3}" }
                                        td { "{here.peak:.3}" }
                                        td { "{here.bottom:.3}" }
                                        td {
                                            class: if here.depth < reference.depth * 0.5 { "gate_rules-weak" } else { "" },
                                            title: "against the reference's dip",
                                            "{here.depth:.3}"
                                        }
                                        td { "{here.offset:+.3}" }
                                        td { "{here.at:.3}" }
                                    }
                                }
                            }
                        }
                    }
                    if run.positioned.iter().any(|p| p.negative.is_some()) {
                        h3 { "How the negative was read" }
                        p { class: "gate_rules-note",
                            "The gate sits a fixed number of the negative's own widths above its centre, so a sample whose negative reads tighter gets a gate nearer to it. The ratio is that comparison made directly: below 1.00 this sample's negative measured narrower than the reference's."
                        }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Ref centre" }
                                    th { "Ref width" }
                                    th { "Centre" }
                                    th { "Width" }
                                    th { "Ratio" }
                                    th { "Widths" }
                                    th { "Placed at" }
                                    th { "Flank n" }
                                }
                            }
                            tbody {
                                for (placed , (reference , here)) in run
                                    .positioned
                                    .iter()
                                    .filter_map(|p| p.negative.map(|n| (p, n)))
                                {
                                    tr {
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{reference.centre:.3}" }
                                        td { "{reference.spread:.3}" }
                                        td { "{here.centre:.3}" }
                                        td { "{here.spread:.3}" }
                                        td {
                                            class: if (here.spread / reference.spread - 1.0).abs() > 0.15 { "gate_rules-weak" } else { "" },
                                            title: "this sample's negative width over the reference's",
                                            "{here.spread / reference.spread:.2}"
                                        }
                                        td { "{here.widths:.2}" }
                                        td { "{here.at:.3}" }
                                        td {
                                            title: "events the width was measured from",
                                            "{here.flank_events}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !run.reference.is_empty() {
                        h3 { "Reference - left as drawn" }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Captures" }
                                    th { "Line only" }
                                }
                            }
                            tbody {
                                for kept in run.reference.iter() {
                                    tr {
                                        td { "{kept.specimen}" }
                                        td { "{describe(&kept.gate, kept.parent_gate.as_deref())}" }
                                        td { "{kept.achieved * 100.0:.3}%" }
                                        td {
                                            class: if kept.above_the_line > 0.0
                                                && kept.achieved < kept.above_the_line * 0.5 { "gate_rules-weak" } else { "" },
                                            "{kept.above_the_line * 100.0:.3}%"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !run.skipped.is_empty() {
                        h3 { "Not positioned" }
                        ul { class: "gate_rules-skipped",
                            for missed in run.skipped.iter() {
                                li {
                                    // The file's own name: an opaque gating id
                                    // names a file nobody can look up.
                                    if missed.file.is_empty() {
                                        "{describe(&missed.gate, missed.parent_gate.as_deref())}: {missed.reason}"
                                    } else {
                                        "{describe(&missed.gate, missed.parent_gate.as_deref())} on {name_of(&files.read(), &missed.file)}: {missed.reason}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(reshaped: Option<(f64, f64)>, clamped: bool, refused: Option<f64>) -> PhenotypeRead {
        PhenotypeRead {
            markers: Vec::new(),
            matched: 0,
            parent: 0,
            reference_matched: 0,
            reference_parent: 0,
            purity: 1.0,
            caught: 1.0,
            pieces: 1,
            centres: Vec::new(),
            reshaped,
            clamped,
            refused_outline: refused,
        }
    }

    #[test]
    fn the_table_says_how_the_phenotype_rule_fitted_the_gate() {
        assert_eq!(fitted(&read(None, false, None)), "new polygon");
        assert_eq!(
            fitted(&read(Some((12.0, -3.0)), false, None)),
            "moved +12, -3"
        );
        assert_eq!(
            fitted(&read(Some((12.0, -3.0)), true, None)),
            "moved +12, -3 (size limited)"
        );
        assert_eq!(
            fitted(&read(Some((12.0, -3.0)), true, Some(2.44))),
            "shape kept: the polygon was 2.4x the area; moved +12, -3 (size limited)"
        );
    }

    #[test]
    fn a_marker_says_which_frame_it_was_read_in() {
        let marker = |by_landmarks| MarkerRead {
            marker: Arc::from("CD4"),
            by_landmarks,
            identity: clingate_core::gate_rules::phenotype::Identity::Between(-0.6, 0.6),
            reference_middle: 0.0,
            reference_spread: 0.2,
            middle: 0.1,
        };
        assert_eq!(
            frame_of(&marker(true)),
            "0 at the negative's peak, 1 at the valley above it"
        );
        assert_eq!(
            frame_of(&marker(false)),
            "in spreads from the parent's middle"
        );
    }

    /// B-RUN-1: a run must stop when anything it read changes. Driven through
    /// the real hook, in a headless `VirtualDom` holding the same stores and
    /// signals the app provides.
    mod a_run_stops_when_anything_it_read_changes {
        use super::*;
        use crate::gate_editor::workspace_window::{AxesStore, GateStore, MetadataStore};
        use clingate_core::axis_store::PlotMapper;
        use clingate_core::gates::gate_store::{GateSource, GateStateStoreExt};
        use clingate_core::gates::gate_types::PrimaryGateType;
        use dioxus::stores::use_store_sync;
        use dioxus_core::{NoOpMutations, generation};
        use std::sync::atomic::{AtomicBool, Ordering};

        /// Everything the hook watches, handed to the change under test.
        #[derive(Clone, Copy)]
        struct Held {
            gates: GateStore,
            metadata: MetadataStore,
            axes: AxesStore,
            rules: Signal<RuleStore>,
            files: Signal<Option<clingate_core::file_load::FcsFiles>>,
        }

        #[derive(Clone)]
        struct Setup {
            change: Rc<dyn Fn(Held)>,
            flag: Arc<AtomicBool>,
        }
        impl PartialEq for Setup {
            fn eq(&self, _: &Self) -> bool {
                true
            }
        }
        use std::rc::Rc;

        fn mapper() -> PlotMapper {
            PlotMapper::new(
                600.0,
                600.0,
                0.0..=1000.0,
                0.0..=1000.0,
                0.0..=1000.0,
                0.0..=1000.0,
                flow_fcs::TransformType::Linear,
                flow_fcs::TransformType::Linear,
            )
        }

        /// A document with one rectangle at the root, as the editor draws it.
        fn document() -> GateState {
            let mut state = GateState::default();
            state
                .add_gate(
                    &mapper(),
                    300.0,
                    300.0,
                    Arc::from("FSC-A"),
                    Arc::from("SSC-A"),
                    None,
                    None,
                    PrimaryGateType::Rectangle,
                    Some("r".to_string()),
                )
                .unwrap();
            state
        }

        /// Mount the hook with nothing running, start a run, make `change`,
        /// and say whether the run's stop flag went up.
        fn stops_for(change: impl Fn(Held) + 'static) -> bool {
            let flag = Arc::new(AtomicBool::new(false));
            let mut dom = VirtualDom::new_with_props(
                |setup: Setup| {
                    let gates = use_store_sync(document);
                    let metadata = use_store_sync(MetaDataStore::default);
                    let axes = use_store_sync(AxisStore::default);
                    let rules = use_signal(RuleStore::default);
                    let files = use_signal(|| None::<clingate_core::file_load::FcsFiles>);
                    use_context_provider(|| gates);
                    use_context_provider(|| metadata);
                    use_context_provider(|| axes);
                    use_context_provider(|| rules);
                    use_context_provider(|| files);
                    let compensation =
                        use_signal(clingate_core::compensation::groups::Compensation::default);
                    use_context_provider(|| compensation);
                    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
                    use_stop_run_on_change(cancel);
                    // Pass 1 starts the run; pass 2 makes the change.
                    if generation() == 1 {
                        cancel.set(Some(setup.flag.clone()));
                    }
                    if generation() == 2 {
                        (setup.change)(Held {
                            gates,
                            metadata,
                            axes,
                            rules,
                            files,
                        });
                    }
                    rsx! {}
                },
                Setup {
                    change: Rc::new(change),
                    flag: flag.clone(),
                },
            );
            // Effects run from `wait_for_work`, not from a render, so each pass
            // renders and then lets the queued effects run.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            let settle = |dom: &mut VirtualDom| {
                runtime.block_on(async {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(20),
                        dom.wait_for_work(),
                    )
                    .await;
                });
                dom.render_immediate(&mut NoOpMutations);
            };
            dom.rebuild_in_place();
            settle(&mut dom);
            for _ in 0..3 {
                dom.mark_dirty(ScopeId::APP);
                dom.render_immediate(&mut NoOpMutations);
                settle(&mut dom);
            }
            flag.load(Ordering::Relaxed)
        }

        #[test]
        fn nothing_changing_does_not_stop_it() {
            // Guard for every test below: mounting, starting the run and
            // re-rendering are not changes, or they would all pass for free.
            assert!(!stops_for(|_| {}));
        }

        #[test]
        fn a_gate_moved_in_the_editor_stops_it() {
            assert!(stops_for(|held| {
                let mut gates = held.gates;
                let id = gates.peek().registered_ids()[0].clone();
                let moved = gates.peek().registered_gate(&id).unwrap();
                gates.write().place_gate(
                    std::slice::from_ref(&id),
                    &moved,
                    &GateSource::Sample((id.clone(), Arc::from("s1"))),
                );
            }));
        }

        #[test]
        fn a_gating_file_loaded_over_the_document_stops_it() {
            assert!(stops_for(|held| {
                let mut gates = held.gates;
                *gates.write() = GateState::default();
            }));
        }

        #[test]
        fn new_metadata_stops_it() {
            assert!(stops_for(|held| {
                let metadata = held.metadata;
                metadata
                    .metadata()
                    .write()
                    .insert(Arc::from("f1"), Default::default());
            }));
        }

        #[test]
        fn a_new_scaling_stops_it() {
            assert!(stops_for(|held| {
                let mut axes = held.axes;
                axes.with_mut(|a| a.replace_axis_configs(vec![clingate_core::AxisInfo::default()]));
            }));
        }

        #[test]
        fn an_edited_rule_stops_it() {
            assert!(stops_for(|held| {
                let mut rules = held.rules;
                rules.write().pairing.sample_id_column = Arc::from("Donor");
            }));
        }

        #[test]
        fn a_changed_file_list_stops_it() {
            assert!(stops_for(|held| {
                let mut files = held.files;
                files.set(Some(clingate_core::file_load::FcsFiles::default()));
            }));
        }

        #[test]
        fn selecting_a_gate_does_not_stop_it() {
            assert!(!stops_for(|held| {
                let gates = held.gates;
                let id = gates.peek().registered_ids()[0].clone();
                *gates.selected_gate().write() = Some(id);
            }));
        }

        #[test]
        fn showing_another_file_in_the_editor_does_not_stop_it() {
            // The editor re-matches its gates to the plot whenever the file
            // changes. With nothing to transpose that must not write.
            assert!(!stops_for(|held| {
                use clingate_core::gates::gate_store::GateStateImplExt as _;
                let mut gates = held.gates;
                let resolver = gates
                    .peek()
                    .get_current_sample(Arc::from("f2"), &Default::default());
                let parent =
                    gates
                        .peek()
                        .parent_node(&clingate_core::gates::gate_store::NodeId::from(
                            gates.peek().registered_ids()[0].clone(),
                        ));
                gates
                    .match_gates_to_plot(
                        Arc::<str>::from("FSC-A"),
                        Arc::<str>::from("SSC-A"),
                        parent.map(|p| p.as_arc().clone()),
                        &resolver,
                    )
                    .expect("the gate is on this plot");
            }));
        }
    }
}

/// The one sample a calibrated rule reads its reference from.
/// The name a gating id is loaded under, or the id where none is.
fn file_name(files: &[(Arc<str>, Arc<str>)], id: &str) -> String {
    files
        .iter()
        .find(|(_, gating_id)| &**gating_id == id)
        .map_or(id, |(name, _)| &**name)
        .to_string()
}

fn calibrate_picker(
    mut calibrate_on: Signal<String>,
    files: Memo<Vec<(Arc<str>, Arc<str>)>>,
) -> Element {
    rsx! {
        label { "Calibrate on" }
        select {
            value: "{calibrate_on}",
            onchange: move |e| calibrate_on.set(e.value()),
            option { value: "", "choose the reference sample" }
            // Marked, not only valued: see the gate's list.
            for (name , id) in files.read().clone() {
                option {
                    value: "{id}",
                    selected: calibrate_on() == *id,
                    "{name}"
                }
            }
        }
    }
}
