//! Turning what the user said into something that can be checked (v1.5 §3).
//!
//! This is the step v1.5 was missing. The contract, the reviewers and the gate
//! were all built to hold a task to what the user asked for, and none of them
//! ran, because nothing turned a message into a contract. A user typing
//!
//! > make the Settings page simpler, but don't remove functionality
//!
//! got a session with an objective string and no requirements — so the gate had
//! nothing to gate, and the agent finished when it felt finished.
//!
//! ## The rule this module enforces
//!
//! A compiled clause must quote the user. Not "be faithful to" them —
//! *quote* them, with the words checked against the messages they actually
//! sent. The reason is that intent does not drift in one step. It drifts like
//! this, and every step looks reasonable next to the one before it:
//!
//! > don't clutter the UI → simplify the UI → reduce the number of options →
//! > remove the advanced settings
//!
//! A model asked to produce a requirement plus a quotation will happily produce
//! the fourth line with the first line's meaning attached. So the quotation is
//! looked up in the transcript, the runtime decides which message it came from
//! rather than believing the model's claim, and a clause that quotes nobody is
//! refused.

use purrcode_provider_gateway::ModelMessage;
use purrcode_runtime_core::expectation::{
    Assumption, AssumptionId, ContractChange, ContractRevision, ExpectationClause,
    ExpectationContract, ExpectationStrength, IntentSource, NonGoal, OpenQuestion,
};
use purrcode_runtime_core::work::{AcceptanceCriterion, CriterionId, RequirementId};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::{AlignmentError, ModelRoute, normalise};

/// The shortest run of words that counts as quoting somebody.
///
/// Below this a "quotation" matches by accident — "the", "it should" — and the
/// provenance check becomes a formality that always passes.
const MINIMUM_QUOTATION_CHARACTERS: usize = 8;

const SYSTEM_PROMPT: &str = "\
You compile a user's request into a checkable task contract. You are not \
planning the work and not writing code.

Rules, in order of importance:

1. Every requirement and every non-goal must carry `quotation`: the user's own \
   words, copied verbatim from their message. Do not paraphrase, tidy, expand \
   or summarise inside `quotation`. If you cannot point at words the user \
   actually wrote, the requirement does not belong in the contract.
2. Separate what the task is not done without (`required`) from what the user \
   would like (`preferred`). Only `required` can hold up delivery, so a \
   contract that marks everything required produces an agent that never \
   finishes, and one that marks nothing required produces an agent that ships \
   anything.
3. Record what the user ruled out as non-goals. \"Don't remove functionality\" \
   is a non-goal, not a requirement — scope drift is doing something extra, \
   and a list of things that must be true cannot detect it.
4. Every required clause needs at least one acceptance criterion: a concrete \
   observation that would settle whether it holds. \"Settings are better\" \
   cannot be checked and will be declared done by whoever is in a hurry.
5. Write requirements in the product's terms, not the implementation's. \
   \"The user can add, edit and remove an MCP server\" — not \"wire McpConfig \
   into settings.rs\", which is a plan, and plans change without the \
   requirement changing.
6. Raise an open question only when proceeding might do the wrong work. Mark \
   it blocking only when every reading is a risk. A competent colleague picks \
   the obvious reading and says which one they picked.";

/// One requirement as the model returns it, before it has an identity.
#[derive(Debug, Deserialize, JsonSchema)]
struct DraftClause {
    /// The requirement in one sentence, in the product's terms.
    statement: String,
    /// `required` or `preferred`.
    strength: String,
    /// Concrete observations that would settle it.
    #[serde(default)]
    acceptance_criteria: Vec<String>,
    /// The user's own words. Checked against the transcript.
    quotation: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftNonGoal {
    statement: String,
    quotation: String,
    /// The part of the tree this rules out, when the user named one.
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftQuestion {
    question: String,
    /// Whether proceeding without an answer risks doing the wrong work.
    blocking: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftContract {
    /// The task in one line, as the user would describe it.
    objective: String,
    /// One paragraph the user reads to check they were understood.
    understanding: String,
    #[serde(default)]
    clauses: Vec<DraftClause>,
    #[serde(default)]
    non_goals: Vec<DraftNonGoal>,
    /// Beliefs filled in where the request was silent.
    #[serde(default)]
    assumptions: Vec<String>,
    #[serde(default)]
    open_questions: Vec<DraftQuestion>,
}

/// What the compiler produced.
#[derive(Clone, Debug)]
pub struct CompiledContract {
    pub contract: ExpectationContract,
    /// "What PurrCode understood", in one paragraph (§19).
    pub understanding: String,
    /// What compiling it cost, when the provider said.
    pub usage: crate::Usage,
}

/// What the compiler reads.
#[derive(Clone, Debug, Default)]
pub struct IntentRequest {
    /// Every user message so far, in order. Provenance indexes into this.
    pub user_messages: Vec<String>,
    /// What the repository is, in a sentence or two — enough for the model to
    /// phrase requirements in the product's terms rather than in generic ones.
    pub repository_summary: String,
}

impl IntentRequest {
    pub fn from_messages(messages: Vec<String>) -> Self {
        Self {
            user_messages: messages,
            repository_summary: String::new(),
        }
    }

    /// Which message these words came from, if any.
    ///
    /// The runtime decides this rather than trusting the model's `message_index`
    /// — an index is exactly the kind of field a model fills in plausibly and
    /// wrongly, and a wrong one makes the provenance trail point at the wrong
    /// sentence, which is worse than having none.
    fn locate(&self, quotation: &str) -> Option<u64> {
        let needle = normalise(quotation);
        if needle.chars().count() < MINIMUM_QUOTATION_CHARACTERS {
            return None;
        }
        self.user_messages
            .iter()
            .position(|message| normalise(message).contains(&needle))
            .map(|index| index as u64)
    }
}

/// Compiles a user's request into a contract.
#[derive(Debug)]
pub struct IntentCompiler {
    route: ModelRoute,
}

impl IntentCompiler {
    pub fn new(route: ModelRoute) -> Self {
        Self { route }
    }

    /// Compile the request, with one bounded correction attempt.
    ///
    /// The retry exists because a paraphrase is a *recoverable* mistake and a
    /// silent one is not: told exactly which clause quoted nobody, a model
    /// usually returns the verbatim words on the second attempt. Told nothing,
    /// it never learns it did anything wrong. Two failures end the compilation
    /// rather than degrading into "accept the paraphrase" — a contract built on
    /// words the user did not say is the drift this module exists to stop.
    pub async fn compile(
        &self,
        request: &IntentRequest,
    ) -> Result<CompiledContract, AlignmentError> {
        if request.user_messages.iter().all(|m| m.trim().is_empty()) {
            return Err(AlignmentError::Invalid(
                "there is no user request to compile".into(),
            ));
        }
        let mut messages = self.prompt(request);
        let mut last_complaint = None;
        for attempt in 0..2 {
            if let Some(complaint) = last_complaint.take() {
                messages.push(ModelMessage {
                    role: "user".into(),
                    content: complaint,
                });
            }
            let (draft, usage): (DraftContract, crate::Usage) = self
                .route
                .structured(messages.clone(), schema_for!(DraftContract))
                .await?;
            match assemble(draft, request, usage) {
                Ok(compiled) => return Ok(compiled),
                Err(AlignmentError::Unfaithful(complaint)) if attempt == 0 => {
                    last_complaint = Some(format!(
                        "That contract was refused: {complaint}\n\n\
                         Return it again with `quotation` copied verbatim from the user's \
                         messages. If no words of theirs support a clause, drop the clause \
                         rather than inventing words for it."
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Err(AlignmentError::Unfaithful(
            "the compiler could not produce a contract whose clauses quote the user".into(),
        ))
    }

    fn prompt(&self, request: &IntentRequest) -> Vec<ModelMessage> {
        let mut body = String::from("THE USER'S MESSAGES, IN ORDER\n\n");
        for (index, message) in request.user_messages.iter().enumerate() {
            body.push_str(&format!("[{index}] {message}\n\n"));
        }
        if !request.repository_summary.trim().is_empty() {
            body.push_str("THE REPOSITORY\n");
            body.push_str(request.repository_summary.trim());
            body.push('\n');
        }
        vec![
            ModelMessage {
                role: "system".into(),
                content: SYSTEM_PROMPT.into(),
            },
            ModelMessage {
                role: "user".into(),
                content: body,
            },
        ]
    }

    /// Compile a user correction into a revision of the contract they already
    /// have (v1.5 §5).
    ///
    /// Returned as a [`ContractRevision`] rather than a replacement contract,
    /// because the difference is what makes a correction cheap: restating a
    /// clause resets the work done under the old wording and leaves everything
    /// else alone, while a replacement quietly discards the evidence for the
    /// parts that did not change.
    pub async fn revise(
        &self,
        contract: &ExpectationContract,
        request: &IntentRequest,
    ) -> Result<ContractRevision, AlignmentError> {
        let correction = request
            .user_messages
            .last()
            .cloned()
            .filter(|message| !message.trim().is_empty())
            .ok_or_else(|| AlignmentError::Invalid("there is no correction to apply".into()))?;

        let mut body = String::from("THE CONTRACT AS IT STANDS\n\n");
        body.push_str(&contract.brief());
        body.push_str("\n\nREQUIREMENTS, BY INDEX\n");
        for (index, clause) in contract.clauses.iter().enumerate() {
            body.push_str(&format!(
                "[{index}] ({}) {}\n",
                if clause.is_required() {
                    "required"
                } else {
                    "preferred"
                },
                clause.statement
            ));
        }
        body.push_str(&format!("\nWHAT THE USER JUST SAID\n\n{correction}\n"));

        let messages = vec![
            ModelMessage {
                role: "system".into(),
                content: REVISION_PROMPT.into(),
            },
            ModelMessage {
                role: "user".into(),
                content: body,
            },
        ];
        let (draft, _usage): (DraftRevision, crate::Usage) = self
            .route
            .structured(messages, schema_for!(DraftRevision))
            .await?;
        assemble_revision(draft, contract, request, &correction)
    }
}

const REVISION_PROMPT: &str = "\
The user has corrected a task that is already running. Turn what they said into \
a revision of the existing contract.

A correction is not an addition. If the user says \"no, I didn't mean delete the \
sidebar, it's just cramped\", the requirement to delete it is restated — not \
left standing beside a new one. Restating resets the work done under the old \
wording so it stops counting as progress; that is the point of saying so.

Refer to requirements by the index shown. Quote the user verbatim in \
`quotation`. Say in `reason` what you understood them to mean — that sentence \
is what they read to check you took the right lesson from what they said.";

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftRevisionChange {
    /// One of: restate, withdraw, add, strengthen, soften, non_goal, answer.
    kind: String,
    /// The index of the requirement this changes, for restate/withdraw/
    /// strengthen/soften.
    #[serde(default)]
    requirement: Option<usize>,
    /// The new wording, the added requirement, or the non-goal.
    #[serde(default)]
    statement: Option<String>,
    #[serde(default)]
    acceptance_criteria: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftRevision {
    /// What you understood the user to mean.
    reason: String,
    /// The user's own words, verbatim.
    quotation: String,
    #[serde(default)]
    changes: Vec<DraftRevisionChange>,
}

fn assemble(
    draft: DraftContract,
    request: &IntentRequest,
    usage: crate::Usage,
) -> Result<CompiledContract, AlignmentError> {
    if draft.objective.trim().is_empty() {
        return Err(AlignmentError::Invalid(
            "the contract has no objective".into(),
        ));
    }
    let mut contract = ExpectationContract::new(draft.objective.trim());

    for clause in draft.clauses {
        let statement = clause.statement.trim().to_owned();
        if statement.is_empty() {
            continue;
        }
        let Some(message_index) = request.locate(&clause.quotation) else {
            return Err(AlignmentError::Unfaithful(format!(
                "the requirement \"{statement}\" cites \"{}\", which the user never wrote",
                clause.quotation.trim()
            )));
        };
        let required = !clause.strength.eq_ignore_ascii_case("preferred");
        let criteria: Vec<AcceptanceCriterion> = clause
            .acceptance_criteria
            .into_iter()
            .filter(|criterion| !criterion.trim().is_empty())
            .map(|criterion| AcceptanceCriterion {
                id: CriterionId::new(),
                statement: criterion.trim().to_owned(),
            })
            .collect();
        // A hard requirement with nothing that could check it is one that gets
        // declared done. Rather than refuse the whole contract, it becomes the
        // preference it actually is — recorded and reported, and not able to
        // hold delivery on an unfalsifiable claim.
        let strength = if required && criteria.is_empty() {
            ExpectationStrength::Preferred
        } else if required {
            ExpectationStrength::Required
        } else {
            ExpectationStrength::Preferred
        };
        contract.clauses.push(ExpectationClause {
            id: RequirementId::new(),
            statement,
            strength,
            acceptance_criteria: criteria,
            status: purrcode_runtime_core::expectation::RequirementStatus::Unverified,
            source: IntentSource::new(message_index, clause.quotation.trim()),
        });
    }

    for non_goal in draft.non_goals {
        let statement = non_goal.statement.trim().to_owned();
        if statement.is_empty() {
            continue;
        }
        let Some(message_index) = request.locate(&non_goal.quotation) else {
            return Err(AlignmentError::Unfaithful(format!(
                "the non-goal \"{statement}\" cites \"{}\", which the user never wrote",
                non_goal.quotation.trim()
            )));
        };
        let mut recorded = NonGoal::new(
            statement,
            IntentSource::new(message_index, non_goal.quotation.trim()),
        );
        if let Some(prefix) = non_goal
            .path_prefix
            .as_deref()
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty())
        {
            recorded = recorded.within(prefix);
        }
        contract.non_goals.push(recorded);
    }

    contract.assumptions = draft
        .assumptions
        .into_iter()
        .filter(|assumption| !assumption.trim().is_empty())
        .map(|assumption| Assumption {
            id: AssumptionId::new(),
            statement: assumption.trim().to_owned(),
            invalidated_by: None,
        })
        .collect();

    contract.open_questions = draft
        .open_questions
        .into_iter()
        .filter(|question| !question.question.trim().is_empty())
        .map(|question| OpenQuestion::new(question.question.trim(), question.blocking))
        .collect();

    contract
        .validate()
        .map_err(|error| AlignmentError::Invalid(error.to_string()))?;

    Ok(CompiledContract {
        understanding: if draft.understanding.trim().is_empty() {
            contract.objective.clone()
        } else {
            draft.understanding.trim().to_owned()
        },
        contract,
        usage,
    })
}

fn assemble_revision(
    draft: DraftRevision,
    contract: &ExpectationContract,
    request: &IntentRequest,
    correction: &str,
) -> Result<ContractRevision, AlignmentError> {
    if draft.reason.trim().is_empty() {
        return Err(AlignmentError::Invalid(
            "a revision must say what the agent understood the user to mean".into(),
        ));
    }
    // The correction is the last message, so its index is known without asking.
    let message_index = request.user_messages.len().saturating_sub(1) as u64;
    let quotation = if request.locate(&draft.quotation).is_some() {
        draft.quotation.trim().to_owned()
    } else {
        // The user's correction is right there; falling back to it whole is
        // honest, where inventing a tidier quotation would not be.
        correction.trim().to_owned()
    };
    let source = IntentSource::new(message_index, quotation);

    let clause_at =
        |index: Option<usize>| -> Option<&ExpectationClause> { contract.clauses.get(index?) };

    let mut changes = Vec::new();
    for change in draft.changes {
        match change.kind.to_ascii_lowercase().as_str() {
            "restate" => {
                let Some(clause) = clause_at(change.requirement) else {
                    continue;
                };
                let Some(to) = change.statement.as_deref().map(str::trim) else {
                    continue;
                };
                if to.is_empty() {
                    continue;
                }
                changes.push(ContractChange::ClauseRestated {
                    id: clause.id,
                    from: clause.statement.clone(),
                    to: to.to_owned(),
                });
            }
            "withdraw" => {
                let Some(clause) = clause_at(change.requirement) else {
                    continue;
                };
                changes.push(ContractChange::ClauseWithdrawn {
                    id: clause.id,
                    statement: clause.statement.clone(),
                    reason: change
                        .reason
                        .unwrap_or_else(|| draft.reason.trim().to_owned()),
                });
            }
            "add" => {
                let Some(statement) = change.statement.as_deref().map(str::trim) else {
                    continue;
                };
                if statement.is_empty() {
                    continue;
                }
                let criteria: Vec<AcceptanceCriterion> = change
                    .acceptance_criteria
                    .iter()
                    .filter(|criterion| !criterion.trim().is_empty())
                    .map(|criterion| AcceptanceCriterion {
                        id: CriterionId::new(),
                        statement: criterion.trim().to_owned(),
                    })
                    .collect();
                let clause = if criteria.is_empty() {
                    ExpectationClause::preferred(statement, source.clone())
                } else {
                    ExpectationClause::required(statement, criteria, source.clone())
                };
                changes.push(ContractChange::ClauseAdded {
                    clause: Box::new(clause),
                });
            }
            "strengthen" | "soften" => {
                let Some(clause) = clause_at(change.requirement) else {
                    continue;
                };
                let to = if change.kind.eq_ignore_ascii_case("strengthen") {
                    ExpectationStrength::Required
                } else {
                    ExpectationStrength::Preferred
                };
                if clause.strength == to {
                    continue;
                }
                // Promotion needs something that can check the clause, or the
                // resulting contract would not validate. Dropping the change
                // here rather than erroring would make it silent: a
                // strengthen with nowhere else to go in this revision leaves
                // `changes` empty, `assemble_revision` reads as a no-op, and
                // `record_correction` treats that identically to "thanks" —
                // the correction simply vanishes with no trace anywhere. That
                // is the exact failure this crate exists to stop, so it is
                // refused loudly instead, the same way an unquoted clause is.
                if to == ExpectationStrength::Required && clause.acceptance_criteria.is_empty() {
                    let criteria: Vec<AcceptanceCriterion> = change
                        .acceptance_criteria
                        .iter()
                        .filter(|criterion| !criterion.trim().is_empty())
                        .map(|criterion| AcceptanceCriterion {
                            id: CriterionId::new(),
                            statement: criterion.trim().to_owned(),
                        })
                        .collect();
                    if criteria.is_empty() {
                        return Err(AlignmentError::Unfaithful(format!(
                            "\"{}\" would need to become required, but nothing was given \
                             that could check it — say one concrete thing that would prove \
                             it's done, and it can become required",
                            clause.statement
                        )));
                    }
                    changes.push(ContractChange::AcceptanceCriteriaSet {
                        id: clause.id,
                        criteria,
                    });
                }
                changes.push(ContractChange::StrengthChanged {
                    id: clause.id,
                    from: clause.strength,
                    to,
                });
            }
            "non_goal" => {
                let Some(statement) = change.statement.as_deref().map(str::trim) else {
                    continue;
                };
                if statement.is_empty() {
                    continue;
                }
                changes.push(ContractChange::NonGoalAdded {
                    non_goal: NonGoal::new(statement, source.clone()),
                });
            }
            "answer" => {
                let Some(question) = contract
                    .open_questions
                    .iter()
                    .find(|question| question.answer.is_none())
                else {
                    continue;
                };
                let Some(answer) = change.statement.as_deref().map(str::trim) else {
                    continue;
                };
                if answer.is_empty() {
                    continue;
                }
                changes.push(ContractChange::QuestionAnswered {
                    id: question.id,
                    answer: answer.to_owned(),
                });
            }
            _ => {}
        }
    }

    if changes.is_empty() {
        return Err(AlignmentError::Invalid(
            "the correction did not change the contract".into(),
        ));
    }

    let revision = ContractRevision {
        revision: contract.revision + 1,
        source,
        reason: draft.reason.trim().to_owned(),
        changes,
    };
    // Rehearsed here so a revision that cannot be applied never reaches the
    // event log, where it would be refused with less to say about why.
    contract
        .revise(revision.clone())
        .map_err(|error| AlignmentError::Invalid(error.to_string()))?;
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ScriptedProvider, route};
    use serde_json::json;

    const REQUEST: &str =
        "Make the Settings page simpler, but don't remove functionality. MCP must actually work.";

    fn request() -> IntentRequest {
        IntentRequest::from_messages(vec![REQUEST.into()])
    }

    fn faithful() -> serde_json::Value {
        json!({
            "objective": "Simplify the Settings page without losing capability",
            "understanding": "You want Settings to feel simpler without anything disappearing, and MCP configuration to genuinely work.",
            "clauses": [
                {
                    "statement": "The user can add, edit and remove an MCP server from Settings",
                    "strength": "required",
                    "acceptance_criteria": ["adding a server and reopening Settings shows it"],
                    "quotation": "MCP must actually work"
                },
                {
                    "statement": "The Settings page reads as less busy",
                    "strength": "preferred",
                    "acceptance_criteria": [],
                    "quotation": "Make the Settings page simpler"
                }
            ],
            "non_goals": [
                {
                    "statement": "Removing existing functionality",
                    "quotation": "don't remove functionality",
                    "path_prefix": null
                }
            ],
            "assumptions": ["Settings means the desktop Settings window"],
            "open_questions": []
        })
    }

    #[tokio::test]
    async fn a_request_becomes_a_contract_with_the_users_words_attached() {
        let provider = ScriptedProvider::new(vec![faithful()]);
        let compiled = IntentCompiler::new(route(provider))
            .compile(&request())
            .await
            .expect("a faithful contract compiles");
        assert_eq!(compiled.contract.tally().total(), 1, "one hard requirement");
        assert_eq!(compiled.contract.non_goals.len(), 1);
        let clause = compiled.contract.required().next().unwrap();
        assert_eq!(clause.source.quotation, "MCP must actually work");
        assert_eq!(clause.source.message_index, 0);
        assert!(compiled.understanding.contains("MCP"));
    }

    #[tokio::test]
    async fn a_requirement_the_user_never_asked_for_is_refused() {
        // The drift this module exists to stop, in one step: the user said
        // "don't clutter the UI" and the contract says "remove the advanced
        // settings", with a quotation nobody wrote.
        let invented = json!({
            "objective": "Simplify Settings",
            "understanding": "…",
            "clauses": [{
                "statement": "Remove the advanced settings section",
                "strength": "required",
                "acceptance_criteria": ["the advanced section is gone"],
                "quotation": "remove the advanced settings"
            }],
            "non_goals": [], "assumptions": [], "open_questions": []
        });
        let provider = ScriptedProvider::new(vec![invented.clone(), invented]);
        let error = IntentCompiler::new(route(provider))
            .compile(&request())
            .await
            .expect_err("a clause quoting nobody must not compile");
        assert!(
            matches!(error, AlignmentError::Unfaithful(_)),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn the_compiler_says_which_clause_was_invented_and_accepts_the_correction() {
        // A paraphrase is recoverable; told exactly what failed, the model
        // usually returns the verbatim words. Told nothing, it never learns it
        // did anything wrong.
        let invented = json!({
            "objective": "Simplify Settings",
            "understanding": "…",
            "clauses": [{
                "statement": "Remove the advanced settings section",
                "strength": "required",
                "acceptance_criteria": ["the advanced section is gone"],
                "quotation": "remove the advanced settings"
            }],
            "non_goals": [], "assumptions": [], "open_questions": []
        });
        let provider = ScriptedProvider::new(vec![invented, faithful()]);
        let compiled = IntentCompiler::new(route(provider.clone()))
            .compile(&request())
            .await
            .expect("the second attempt quotes the user");
        assert_eq!(compiled.contract.tally().total(), 1);
        assert!(
            provider.transcript().contains("which the user never wrote"),
            "the complaint must name the problem: {}",
            provider.transcript()
        );
    }

    #[tokio::test]
    async fn an_unfalsifiable_hard_requirement_becomes_the_preference_it_is() {
        // "Settings are better" with nothing that could check it would be
        // declared done by whoever is in a hurry. It is real, it is recorded,
        // and it does not hold delivery.
        let vague = json!({
            "objective": "Simplify Settings",
            "understanding": "…",
            "clauses": [{
                "statement": "Settings feel simpler",
                "strength": "required",
                "acceptance_criteria": [],
                "quotation": "Make the Settings page simpler"
            }],
            "non_goals": [], "assumptions": [], "open_questions": []
        });
        let provider = ScriptedProvider::new(vec![vague]);
        let compiled = IntentCompiler::new(route(provider))
            .compile(&request())
            .await
            .unwrap();
        assert_eq!(compiled.contract.tally().total(), 0);
        assert_eq!(compiled.contract.preferred().count(), 1);
    }

    #[tokio::test]
    async fn the_provenance_index_is_the_runtimes_finding_not_the_models_claim() {
        let messages = IntentRequest::from_messages(vec![
            "have a look at the settings page".into(),
            "MCP must actually work".into(),
        ]);
        let provider = ScriptedProvider::new(vec![json!({
            "objective": "Make MCP configuration work",
            "understanding": "…",
            "clauses": [{
                "statement": "The user can add, edit and remove an MCP server",
                "strength": "required",
                "acceptance_criteria": ["adding a server and reopening Settings shows it"],
                "quotation": "MCP must actually work"
            }],
            "non_goals": [], "assumptions": [], "open_questions": []
        })]);
        let compiled = IntentCompiler::new(route(provider))
            .compile(&messages)
            .await
            .unwrap();
        assert_eq!(
            compiled
                .contract
                .required()
                .next()
                .unwrap()
                .source
                .message_index,
            1,
            "the quotation is in the second message, whatever the model would have said"
        );
    }

    #[tokio::test]
    async fn a_correction_restates_rather_than_appends() {
        // The sidebar case. Appending would leave "remove the sidebar" standing
        // beside the correction, and the work done under it still counting.
        let provider = ScriptedProvider::new(vec![faithful()]);
        let compiled = IntentCompiler::new(route(provider))
            .compile(&request())
            .await
            .unwrap();
        let contract = compiled.contract;
        let correction = "no, I didn't mean remove MCP from Settings, just make it less cramped";
        let revising = ScriptedProvider::new(vec![json!({
            "reason": "the user wants MCP configuration kept in Settings and made less dense",
            "quotation": "just make it less cramped",
            "changes": [{
                "kind": "restate",
                "requirement": 0,
                "statement": "MCP configuration stays in Settings and is less densely laid out",
                "acceptance_criteria": [],
                "reason": null
            }]
        })]);
        let revision = IntentCompiler::new(route(revising))
            .revise(
                &contract,
                &IntentRequest::from_messages(vec![REQUEST.into(), correction.into()]),
            )
            .await
            .expect("a correction compiles into a revision");
        assert_eq!(revision.revision, 2);
        let revised = contract.revise(revision).unwrap();
        assert_eq!(
            revised.contract.clauses.len(),
            2,
            "the clause was restated, not duplicated"
        );
        assert!(
            revised
                .contract
                .required()
                .next()
                .unwrap()
                .statement
                .contains("less densely")
        );
    }

    #[tokio::test]
    async fn a_correction_that_changes_nothing_is_refused_rather_than_recorded() {
        let provider = ScriptedProvider::new(vec![faithful()]);
        let contract = IntentCompiler::new(route(provider))
            .compile(&request())
            .await
            .unwrap()
            .contract;
        let revising = ScriptedProvider::new(vec![json!({
            "reason": "the user is happy",
            "quotation": "looks good",
            "changes": []
        })]);
        let error = IntentCompiler::new(route(revising))
            .revise(
                &contract,
                &IntentRequest::from_messages(vec![REQUEST.into(), "looks good".into()]),
            )
            .await
            .expect_err("an empty revision is not a revision");
        assert!(matches!(error, AlignmentError::Invalid(_)), "{error:?}");
    }

    #[tokio::test]
    async fn strengthening_a_clause_with_nothing_that_could_check_it_is_refused_not_dropped() {
        // The bug this guards: "strengthen" with no acceptance_criteria used
        // to `continue` past both pushes, so a revision containing only this
        // change ended up with an empty `changes` list — indistinguishable
        // from a correction that changed nothing, and silently discarded by
        // `record_correction` without a trace anywhere. A user who says
        // "actually that's not optional" deserves better than the correction
        // vanishing.
        let provider = ScriptedProvider::new(vec![faithful()]);
        let contract = IntentCompiler::new(route(provider))
            .compile(&request())
            .await
            .unwrap()
            .contract;
        // Index 1 is `faithful()`'s preferred clause with no acceptance
        // criteria: "The Settings page reads as less busy".
        assert!(!contract.clauses[1].is_required());
        assert!(contract.clauses[1].acceptance_criteria.is_empty());

        let revising = ScriptedProvider::new(vec![json!({
            "reason": "the user says decluttering settings is not optional",
            "quotation": "it's not optional",
            "changes": [{
                "kind": "strengthen",
                "requirement": 1,
                "statement": null,
                "acceptance_criteria": [],
                "reason": null
            }]
        })]);
        let error = IntentCompiler::new(route(revising))
            .revise(
                &contract,
                &IntentRequest::from_messages(vec![REQUEST.into(), "it's not optional".into()]),
            )
            .await
            .expect_err("a strengthen with nothing to check it must not silently vanish");
        assert!(
            matches!(error, AlignmentError::Unfaithful(_)),
            "must be surfaced to the user rather than swallowed as a no-op: {error:?}"
        );
        assert!(
            error.to_string().contains("less busy"),
            "the message should name the clause that could not be strengthened: {error}"
        );
    }
}
