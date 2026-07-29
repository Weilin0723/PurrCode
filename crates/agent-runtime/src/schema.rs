use schemars::JsonSchema;
use serde::Deserializer;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use purrcode_runtime_core::{CommandAction, RepositoryReadAction};

use crate::errors::AgentError;
use crate::stream::is_unsafe_terminal_control;

/// Typed read action emitted by the model.
///
/// This is an alias for the runtime-domain [`RepositoryReadAction`] so that
/// the model schema, the durable session state, and PawGate/Claw all see the
/// same strongly-typed contract — no shell-string parsing required.
pub type AgentReadAction = RepositoryReadAction;

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTurn {
    #[serde(default, deserialize_with = "deserialize_optional_string_list")]
    pub plan: Option<Vec<String>>,
    pub current_step_index: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub expected_postconditions: Vec<String>,
    pub rationale: String,
    pub action: Option<AgentAction>,
    pub complete: bool,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPlan {
    #[serde(deserialize_with = "deserialize_string_list")]
    pub steps: Vec<String>,
    #[serde(deserialize_with = "deserialize_string_list")]
    pub assumptions: Vec<String>,
    #[serde(deserialize_with = "deserialize_string_list")]
    pub risks: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringList {
    One(String),
    Many(Vec<String>),
}

fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match StringList::deserialize(deserializer)? {
        StringList::One(value) => vec![value],
        StringList::Many(values) => values,
    })
}

fn deserialize_optional_string_list<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        Option::<StringList>::deserialize(deserializer)?.map(|value| match value {
            StringList::One(value) => vec![value],
            StringList::Many(values) => values,
        }),
    )
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
    Read(AgentReadAction),
    /// Transitional compatibility input. It is never sent to PawGate or Claw
    /// directly; normalization must convert it to a canonical typed read.
    ReadCommand(CommandAction),
    WriteFile {
        path: PathBuf,
        content: String,
        expected_digest: Option<String>,
    },
    DeleteFile {
        path: PathBuf,
        expected_digest: String,
    },
}

pub(crate) fn validate_turn(turn: &AgentTurn) -> Result<(), AgentError> {
    if turn
        .rationale
        .chars()
        .chain(turn.plan.iter().flatten().flat_map(|step| step.chars()))
        .chain(
            turn.expected_postconditions
                .iter()
                .flat_map(|postcondition| postcondition.chars()),
        )
        .any(is_unsafe_terminal_control)
    {
        return Err(AgentError::InvalidModelTurn(
            "model-visible text contains unsafe terminal control characters".into(),
        ));
    }
    if turn.complete == turn.action.is_some() {
        return Err(AgentError::InvalidModelTurn(
            "exactly one of complete=true or action must be supplied".into(),
        ));
    }
    if turn
        .plan
        .as_ref()
        .is_some_and(|steps| steps.is_empty() || steps.len() > 64)
    {
        return Err(AgentError::InvalidModelTurn(
            "plan must contain between 1 and 64 steps".into(),
        ));
    }
    if turn
        .current_step_index
        .as_ref()
        .is_some_and(|index| match &turn.plan {
            Some(steps) => *index >= steps.len(),
            None => true,
        })
    {
        return Err(AgentError::InvalidModelTurn(
            "current_step_index must reference the supplied plan".into(),
        ));
    }
    if turn.expected_postconditions.len() > 16 {
        return Err(AgentError::InvalidModelTurn(
            "expected_postconditions exceeds 16 entries".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_plan(plan: &AgentPlan) -> Result<(), AgentError> {
    if plan.steps.is_empty()
        || plan.steps.len() > 64
        || plan.steps.iter().any(|step| step.trim().is_empty())
    {
        return Err(AgentError::InvalidModelTurn(
            "plan must contain 1 to 64 non-empty steps".into(),
        ));
    }
    if plan
        .steps
        .iter()
        .chain(plan.assumptions.iter())
        .chain(plan.risks.iter())
        .any(|text| text.chars().any(is_unsafe_terminal_control))
    {
        return Err(AgentError::InvalidModelTurn(
            "plan contains unsafe terminal control characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn scalar_string_lists_are_normalized_without_weakening_validation() {
        let plan: AgentPlan = serde_json::from_value(serde_json::json!({
            "steps": "inspect the repository",
            "assumptions": ".",
            "risks": []
        }))
        .unwrap();
        assert_eq!(plan.steps, ["inspect the repository"]);
        assert_eq!(plan.assumptions, ["."]);
        validate_plan(&plan).unwrap();

        let turn: AgentTurn = serde_json::from_value(serde_json::json!({
            "plan": "inspect",
            "current_step_index": 0,
            "expected_postconditions": "tests pass",
            "rationale": "start safely",
            "action": null,
            "complete": true
        }))
        .unwrap();
        assert_eq!(turn.plan.unwrap(), ["inspect"]);
        assert_eq!(turn.expected_postconditions, ["tests pass"]);
    }
}

pub(crate) fn objective_requests_advice_only(objective: &str) -> bool {
    let objective = objective.to_ascii_lowercase();
    let explicitly_non_mutating = [
        "plan only",
        "only a plan",
        "review only",
        "analysis only",
        "do not modify",
        "don't modify",
        "do not change",
        "don't change",
    ]
    .iter()
    .any(|marker| objective.contains(marker));
    let explicitly_forbids_execution = [
        "do not execute",
        "don't execute",
        "or execute",
        "do not run",
        "don't run",
        "no tools",
        "without tools",
    ]
    .iter()
    .any(|marker| objective.contains(marker));

    let requests_plan_or_review = [
        "propose a plan",
        "provide a plan",
        "give me a plan",
        "improvement plan",
        "plan for improvement",
        "plan on how to improve",
    ]
    .iter()
    .any(|marker| objective.contains(marker));
    let requests_implementation = [
        "implement",
        "apply the",
        "make the change",
        "make changes",
        "edit ",
        "modify ",
        "fix ",
        "execute ",
        "run the test",
        "run tests",
        "build it",
    ]
    .iter()
    .any(|marker| objective.contains(marker));

    (explicitly_non_mutating && explicitly_forbids_execution)
        || (requests_plan_or_review && !requests_implementation)
}

#[cfg(test)]
mod advice_only_tests {
    use super::objective_requests_advice_only;

    #[test]
    fn recognizes_explicit_plan_only_without_execution() {
        assert!(objective_requests_advice_only(
            "Inspect README.md and return a concise improvement plan only. \
             Do not modify files or execute tools."
        ));
    }

    #[test]
    fn does_not_skip_validation_for_implementation_requests() {
        assert!(!objective_requests_advice_only(
            "Plan the fix, implement it, and run the tests."
        ));
        assert!(!objective_requests_advice_only(
            "Do not modify unrelated files; run the relevant tests."
        ));
    }

    #[test]
    fn recognizes_natural_review_and_plan_request_without_implementation() {
        assert!(objective_requests_advice_only(
            "Please review the repo and propose a plan for improvement"
        ));
        assert!(!objective_requests_advice_only(
            "Review the repo, propose a plan, implement it, and run the tests"
        ));
    }
}
