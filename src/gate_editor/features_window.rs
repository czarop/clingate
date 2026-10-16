//! The Test features tab: every feature waiting to be tried, how to try it
//! and what to send back, a check for those that can be tried on the open
//! workspace without the user, and what the user says of each - kept in the
//! workspace and copied as one Markdown note to send on. See
//! [`clingate_core::feature_checks`]; the tools for Claude read the same list
//! with `check_features`.

use dioxus::prelude::*;

use clingate_core::feature_checks::{
    Check, CheckInputs, Checked, FEATURES, Feature, Feedbacks, Tried, feedback_markdown,
    load_feedback, run_check, save_feedback,
};
use clingate_core::gate_rules::rule_store::RuleStore;

use crate::components::toast::{say, use_toast, warn};
use crate::gate_editor::gate_rules_window::RulesRun;
use crate::gate_editor::workspace_window::{GateStore, Loaded};

static CSS_STYLE: Asset = asset!("assets/features.css");

/// What the checks read and the feedback is kept in, from the app's state.
#[derive(Clone, Copy)]
pub(crate) struct FeatureRun {
    run_with: RulesRun,
    gates: GateStore,
    rules: Signal<RuleStore>,
    loaded: Signal<Loaded>,
}

impl FeatureRun {
    pub(crate) fn from_context() -> Self {
        Self {
            run_with: RulesRun::from_context(),
            gates: use_context(),
            rules: use_context(),
            loaded: use_context(),
        }
    }

    /// The workspace as it stands, for checks off the UI thread.
    pub(crate) fn snapshot(&self) -> Snapshot {
        let (inputs, axes) = self.run_with.inputs_now();
        Snapshot {
            gates: self.gates.peek().clone(),
            inputs,
            axes,
            rules: self.rules.peek().clone(),
        }
    }

    /// The feedback kept in the workspace; none without a workspace.
    pub(crate) fn feedback(&self) -> Feedbacks {
        self.loaded
            .peek()
            .folder
            .as_deref()
            .map(load_feedback)
            .unwrap_or_default()
    }

    /// Keep `feedback` in the workspace.
    pub(crate) fn keep(&self, feedback: &Feedbacks) -> Result<(), String> {
        let folder = self
            .loaded
            .peek()
            .folder
            .clone()
            .ok_or("Open a workspace folder first - feedback is kept in it")?;
        save_feedback(&folder, feedback).map_err(|e| e.to_string())
    }
}

/// The workspace, as the checks read it.
pub(crate) struct Snapshot {
    gates: clingate_core::gates::GateState,
    inputs: clingate_core::gate_rules::run::RunInputs,
    axes: clingate_core::omiq::serialise::AxisSettings,
    rules: RuleStore,
}

impl Snapshot {
    /// `check` on this workspace. Blocking.
    pub(crate) fn check(&self, check: Check) -> Checked {
        run_check(
            check,
            &CheckInputs {
                gates: &self.gates,
                inputs: &self.inputs,
                axes: &self.axes,
                rules: &self.rules,
            },
        )
    }

    /// Every feature's check, by feature id.
    pub(crate) fn check_every(&self) -> Vec<(&'static str, Checked)> {
        FEATURES
            .iter()
            .filter_map(|f| f.check.map(|check| (f.id, self.check(check))))
            .collect()
    }
}

#[component]
pub fn FeaturesWindow() -> Element {
    let run = FeatureRun::from_context();
    let loaded = use_context::<Signal<Loaded>>();
    let toasts = use_toast();
    let mut feedback = use_signal(Feedbacks::new);
    use_effect(move || {
        let _ = loaded.read().folder.clone();
        feedback.set(run.feedback());
    });
    let mut checking = use_signal(|| false);

    let mut change =
        move |id: &'static str, edit: &dyn Fn(&mut clingate_core::feature_checks::Feedback)| {
            let mut all = feedback.peek().clone();
            edit(all.entry(id.to_string()).or_default());
            if let Err(why) = run.keep(&all) {
                warn(&toasts, why);
            }
            feedback.set(all);
        };

    let check_every = move |_| {
        if checking() {
            return;
        }
        checking.set(true);
        spawn(async move {
            let snapshot = run.snapshot();
            let done = tokio::task::spawn_blocking(move || snapshot.check_every()).await;
            checking.set(false);
            match done {
                Ok(results) => {
                    let failed = results.iter().filter(|(_, c)| !c.passed).count();
                    let mut all = feedback.peek().clone();
                    for (id, checked) in results {
                        all.entry(id.to_string()).or_default().checked = Some(checked);
                    }
                    if let Err(why) = run.keep(&all) {
                        warn(&toasts, why);
                    }
                    feedback.set(all);
                    if failed == 0 {
                        say(&toasts, "Every check passed");
                    } else {
                        warn(&toasts, format!("{failed} check(s) failed - see below"));
                    }
                }
                Err(e) => warn(&toasts, format!("The checks did not finish: {e}")),
            }
        });
    };

    let copy_all = move |_| {
        let markdown = feedback_markdown(&feedback.read());
        match crate::clipboard::copy_text(&markdown) {
            Ok(()) => say(
                &toasts,
                "Feedback for Claude is on the clipboard - paste it into the chat",
            ),
            Err(e) => warn(&toasts, format!("The feedback could not be copied: {e}")),
        }
    };

    rsx! {
        document::Stylesheet { href: CSS_STYLE }
        div { class: "features",
            h2 { "Test features" }
            p { class: "features-hint",
                "New features waiting for you to try. Run the checks first: they try what they can on the open workspace and change nothing. For the rest, follow the steps, say how it went, and copy your feedback for Claude when you are done. What you write here is kept in the workspace's test_reports folder."
            }
            div { class: "features-bar",
                button {
                    class: "gate_rules-add",
                    disabled: checking(),
                    onclick: check_every,
                    if checking() { "Checking..." } else { "Run every check" }
                }
                button { class: "gate_rules-add", onclick: copy_all, "Copy feedback for Claude" }
            }
            for feature in FEATURES.iter() {
                FeatureCard {
                    key: "{feature.id}",
                    feature: *feature,
                    given: feedback.read().get(feature.id).cloned().unwrap_or_default(),
                    on_tried: move |(id, tried): (&'static str, Tried)| change(id, &|f| f.tried = tried),
                    on_note: move |(id, note): (&'static str, String)| change(id, &|f| f.note = note.clone()),
                }
            }
        }
    }
}

#[component]
fn FeatureCard(
    feature: Feature,
    given: clingate_core::feature_checks::Feedback,
    on_tried: EventHandler<(&'static str, Tried)>,
    on_note: EventHandler<(&'static str, String)>,
) -> Element {
    let id = feature.id;
    rsx! {
        div { class: "features-card",
            h3 { "{feature.title}" }
            p { class: "features-place", "{feature.place}" }
            ol {
                for (at , step) in feature.steps.iter().enumerate() {
                    li { key: "{at}", "{step}" }
                }
            }
            p { "Send back: {feature.send_back}" }
            if let Some(checked) = &given.checked {
                p { class: if checked.passed { "features-passed" } else { "features-failed" },
                    if checked.passed { "Check passed" } else { "Check FAILED" }
                }
                ul { class: "features-said",
                    for (at , line) in checked.said.iter().enumerate() {
                        li { key: "{at}", "{line}" }
                    }
                }
            } else if feature.check.is_some() {
                p { class: "features-place", "Checked by \"Run every check\"." }
            }
            div { class: "features-tried",
                for tried in Tried::ALL {
                    label { key: "{tried.label()}",
                        input {
                            r#type: "radio",
                            name: "tried-{id}",
                            checked: given.tried == tried,
                            onchange: move |_| on_tried.call((id, tried)),
                        }
                        " {tried.label()}"
                    }
                }
            }
            textarea {
                class: "features-note",
                placeholder: "What happened - what worked, what did not, anything odd",
                value: "{given.note}",
                oninput: move |e| on_note.call((id, e.value())),
            }
        }
    }
}
