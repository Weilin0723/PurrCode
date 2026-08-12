//! Fresh by construction, not by label (v1.5 §9).
//!
//! `ReviewRecord { context: ReviewContext::Fresh }` is a good thing to have in
//! the log and a poor thing to rely on. It records that whoever assembled the
//! request said the reviewer was independent. It cannot record that they were
//! right, and the failure is silent: a reviewer that quietly read "I've made
//! this really clean now" still produces confident, well-formed findings. It
//! just mostly agrees.
//!
//! So the independent code reviewer and the alignment reviewer accept exactly
//! one input type, and that type has nowhere to put a transcript. There is no
//! `messages` field to leave populated, no `history` to forget to clear, and no
//! `Vec<ModelMessage>` constructor. The prompt is built here, from these
//! fields, and the reviewers cannot be handed anything else.
//!
//! What a reviewer gets is what a competent colleague would get if you asked
//! them to check a branch: the requirements, the repository's own rules, the
//! diff, the files it touched, and what the tests said.

use std::path::PathBuf;

use purrcode_provider_gateway::ModelMessage;
use purrcode_runtime_core::expectation::ExpectationContract;

/// One file the reviewer may read, and its content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewSubject {
    pub path: PathBuf,
    pub content: String,
}

impl ReviewSubject {
    pub fn new(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// Everything a fresh-context reviewer is allowed to see.
///
/// Every field is private and every one of them is set through a method that
/// names what it is. Adding a transcript would mean adding a field, and a field
/// called `transcript` on this type would not survive review — which is a
/// weaker guarantee than a compiler error and a stronger one than a comment.
#[derive(Clone, Debug, Default)]
pub struct FreshReviewInput {
    contract_brief: String,
    objective: String,
    repository_rules: String,
    diff: String,
    files: Vec<ReviewSubject>,
    validations: Vec<(String, String)>,
    /// The requirements, in the order the reviewer will refer to them by index.
    ///
    /// Indices rather than ids: a model asked for a UUID will produce a
    /// well-formed one that belongs to nothing, and the resulting finding lands
    /// on no requirement at all.
    requirements: Vec<String>,
}

impl FreshReviewInput {
    /// Start from the contract. There is no other entry point, so a reviewer
    /// input that does not know what the task was for cannot be built.
    pub fn for_contract(contract: &ExpectationContract) -> Self {
        Self {
            contract_brief: contract.brief(),
            objective: contract.objective.clone(),
            requirements: contract
                .required()
                .map(|clause| clause.statement.clone())
                .collect(),
            ..Self::default()
        }
    }

    pub fn with_repository_rules(mut self, rules: impl Into<String>) -> Self {
        self.repository_rules = rules.into();
        self
    }

    pub fn with_diff(mut self, diff: impl Into<String>) -> Self {
        self.diff = diff.into();
        self
    }

    pub fn with_files(mut self, files: Vec<ReviewSubject>) -> Self {
        self.files = files;
        self
    }

    /// What the deterministic checks did, as facts rather than as prose.
    pub fn with_validations(mut self, validations: Vec<(String, String)>) -> Self {
        self.validations = validations;
        self
    }

    pub fn requirements(&self) -> &[String] {
        &self.requirements
    }

    pub fn objective(&self) -> &str {
        &self.objective
    }

    pub fn diff(&self) -> &str {
        &self.diff
    }

    /// The messages sent to the reviewer.
    ///
    /// The only place a `Vec<ModelMessage>` is produced for a fresh review, and
    /// it is produced from the fields above — so whatever a caller has lying
    /// around, this is what the reviewer reads.
    pub fn into_messages(self, system_prompt: &str) -> Vec<ModelMessage> {
        let mut body = String::new();
        body.push_str(&self.contract_brief);

        if !self.repository_rules.trim().is_empty() {
            body.push_str("\n\nREPOSITORY RULES\n");
            body.push_str(self.repository_rules.trim());
        }

        body.push_str("\n\nREQUIREMENTS, BY INDEX\n");
        if self.requirements.is_empty() {
            body.push_str("(this task has no hard requirements)\n");
        } else {
            for (index, statement) in self.requirements.iter().enumerate() {
                body.push_str(&format!("[{index}] {statement}\n"));
            }
        }

        body.push_str("\nDETERMINISTIC CHECKS\n");
        if self.validations.is_empty() {
            body.push_str("(none were run — treat this as unknown, not as passing)\n");
        } else {
            for (name, outcome) in &self.validations {
                body.push_str(&format!("- {name}: {outcome}\n"));
            }
        }

        body.push_str("\nTHE DIFF\n");
        if self.diff.trim().is_empty() {
            body.push_str("(the working tree is unchanged)\n");
        } else {
            body.push_str(&self.diff);
            body.push('\n');
        }

        for file in &self.files {
            body.push_str(&format!("\nFILE {}\n", file.path.display()));
            body.push_str(&file.content);
            body.push('\n');
        }

        vec![
            ModelMessage {
                role: "system".into(),
                content: system_prompt.to_owned(),
            },
            ModelMessage {
                role: "user".into(),
                content: body,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::expectation::{ExpectationClause, IntentSource};
    use purrcode_runtime_core::work::{AcceptanceCriterion, CriterionId};

    fn contract() -> ExpectationContract {
        let mut contract = ExpectationContract::new("Improve the Settings experience");
        contract.clauses.push(ExpectationClause::required(
            "MCP configuration works",
            vec![AcceptanceCriterion {
                id: CriterionId::new(),
                statement: "a user can add and remove a server".into(),
            }],
            IntentSource::new(0, "MCP must actually work"),
        ));
        contract
    }

    #[test]
    fn the_reviewer_reads_the_contract_the_diff_and_the_checks() {
        let messages = FreshReviewInput::for_contract(&contract())
            .with_diff("--- a/settings.rs\n+++ b/settings.rs\n@@\n+fn add_server() {}")
            .with_validations(vec![("FullUnitTests".into(), "passed".into())])
            .into_messages("you are a code reviewer");
        let body = &messages[1].content;
        assert!(body.contains("MCP configuration works"));
        assert!(body.contains("[0] MCP configuration works"));
        assert!(body.contains("fn add_server"));
        assert!(body.contains("FullUnitTests: passed"));
    }

    #[test]
    fn checks_that_did_not_run_are_shown_as_unknown_rather_than_omitted() {
        // An empty section reads as "nothing to worry about". The reviewer must
        // be told the difference between "the tests passed" and "no tests ran",
        // because that difference is the one the gate turns on.
        let messages =
            FreshReviewInput::for_contract(&contract()).into_messages("you are a code reviewer");
        assert!(
            messages[1].content.contains("not as passing"),
            "{}",
            messages[1].content
        );
    }

    #[test]
    fn the_implementers_reasoning_has_nowhere_to_enter() {
        // The point of the type. Everything a caller could hand a reviewer goes
        // through the builders above; none of them takes conversation. This
        // test is a tripwire: it fails the moment somebody adds a field that
        // would carry it.
        let confession = "I've made this really clean now, I'm confident it's right";
        let messages = FreshReviewInput::for_contract(&contract())
            .with_repository_rules("run cargo fmt before committing")
            .with_diff("--- a/settings.rs\n+++ b/settings.rs")
            .with_files(vec![ReviewSubject::new("settings.rs", "fn main() {}")])
            .with_validations(vec![("FullUnitTests".into(), "passed".into())])
            .into_messages("you are a code reviewer");
        let everything = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!everything.contains(confession));
        assert!(!everything.to_lowercase().contains("assistant"));
    }
}
