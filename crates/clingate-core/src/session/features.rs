//! The features waiting to be tried, and their checks on the open workspace -
//! see [`crate::feature_checks`].

use serde::Serialize;

use super::Session;
use crate::feature_checks::{
    CheckInputs, Checked, FEATURES, Feature, Feedback, load_feedback, run_check,
};
use crate::gate_rules::rule_store::RuleStore;
use crate::gate_rules::run::RunInputs;

/// A feature, what its check found here, and what the user has said of it.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureState {
    #[serde(flatten)]
    pub feature: Feature,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<Checked>,
    pub feedback: Feedback,
}

/// Every feature waiting to be tried.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureChecks {
    pub features: Vec<FeatureState>,
    pub next: &'static str,
}

impl Session {
    /// Every feature waiting to be tried, each check run on the workspace as
    /// it stands. Changes nothing.
    pub fn check_features(&self) -> FeatureChecks {
        let rules = self.rules.clone().unwrap_or_else(RuleStore::default);
        let inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &rules,
        );
        let on = CheckInputs {
            gates: &self.gates,
            inputs: &inputs,
            axes: &self.axes.settings,
            rules: &rules,
        };
        let feedback = load_feedback(&self.folder);
        FeatureChecks {
            features: FEATURES
                .iter()
                .map(|feature| FeatureState {
                    feature: *feature,
                    checked: feature.check.map(|check| run_check(check, &on)),
                    feedback: feedback.get(feature.id).cloned().unwrap_or_default(),
                })
                .collect(),
            next: "tell the user which checks failed and what they found, then which features \
                   they have not tried yet, with the steps and what to send back; their \
                   feedback is kept by the app's Test features tab. checked is each check run \
                   now; feedback.checked is what the tab last found, at checked_at, on the \
                   workspace as it was then",
        }
    }
}
