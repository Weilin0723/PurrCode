use schemars::JsonSchema;
use serde::Deserializer;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use purrcode_runtime_core::{CommandAction, ProposedAction, RepositoryReadAction, SessionEvent};

use crate::errors::AgentError;
use crate::stream::is_unsafe_terminal_control;

/// Typed read action emitted by the model.
///
/// This is an alias for the runtime-domain [`RepositoryReadAction`] so that
/// the model schema, the durable session state, and PawGate/Claw all see the
/// same strongly-typed contract — no shell-string parsing required.
pub type AgentReadAction = RepositoryReadAction;

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
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
        StringList::One(value) => {
            if value.trim().is_empty() {
                vec![]
            } else {
                vec![value]
            }
        }
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
            StringList::One(value) => {
                if value.trim().is_empty() {
                    vec![]
                } else {
                    vec![value]
                }
            }
            StringList::Many(values) => values,
        }),
    )
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
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

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentActionWire {
    Read(AgentReadAction),
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

impl From<AgentActionWire> for AgentAction {
    fn from(value: AgentActionWire) -> Self {
        match value {
            AgentActionWire::Read(read) => Self::Read(read),
            AgentActionWire::ReadCommand(command) => Self::ReadCommand(command),
            AgentActionWire::WriteFile {
                path,
                content,
                expected_digest,
            } => Self::WriteFile {
                path,
                content,
                expected_digest,
            },
            AgentActionWire::DeleteFile {
                path,
                expected_digest,
            } => Self::DeleteFile {
                path,
                expected_digest,
            },
        }
    }
}

impl<'de> Deserialize<'de> for AgentAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("agent action must be an object"))?;
        let action_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if matches!(
            action_type.as_str(),
            "git_status"
                | "git_rev_parse"
                | "git_log"
                | "git_diff"
                | "git_show"
                | "git_ls_files"
                | "repository_grep"
                | "find"
                | "list"
                | "read_file"
        ) {
            // Some OpenAI-compatible structured-output providers flatten the
            // nested read enum and emit `{ "type": "list", ... }` instead
            // of the canonical `{ "type": "read", "kind": "list", ... }`.
            // Accept only the fixed repository-read allowlist here; all paths
            // and bounds still pass through normalisation, PawGate, durable
            // authorization, and the execution adapter's exact recheck.
            object.insert("kind".into(), serde_json::Value::String(action_type));
            object.insert("type".into(), serde_json::Value::String("read".into()));
        }
        serde_json::from_value::<AgentActionWire>(value)
            .map(Into::into)
            .map_err(D::Error::custom)
    }
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
    if let Some(ref plan) = turn.plan {
        if plan.iter().any(|step| step.trim().is_empty()) {
            return Err(AgentError::InvalidModelTurn(
                "every plan step must be non-empty".into(),
            ));
        }
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

    #[test]
    fn flattened_typed_read_is_accepted_without_expanding_mutation_inputs() {
        let action: AgentAction = serde_json::from_value(serde_json::json!({
            "type": "list",
            "paths": ["."],
            "max_entries": 32
        }))
        .unwrap();
        assert!(matches!(
            action,
            AgentAction::Read(RepositoryReadAction::List { ref paths, max_entries })
                if paths == &[PathBuf::from(".")] && max_entries == 32
        ));

        let error = serde_json::from_value::<AgentAction>(serde_json::json!({
            "type": "shell",
            "program": "rm",
            "arguments": ["-rf", "."]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown variant"));
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

/// Returns true when a `complete=true` rationale is only a progress report
/// about having gathered enough context, rather than the user-facing answer.
///
/// Providers are often eager to stop after a bounded read sequence and emit
/// text such as "I have enough information to answer". Treating that text as
/// completion would leave a read-heavy request with no actual answer. This is
/// deliberately narrow: a rationale that contains at least one sentence
/// outside the progress vocabulary is accepted, while a semantic repair prompt
/// can ask the provider either for the answer or for one more typed read.
pub(crate) fn completion_needs_repair(_objective: &str, rationale: &str) -> bool {
    let normalized = rationale.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return true;
    }
    let lower = normalized.to_ascii_lowercase();

    // Some providers emit an imperative next-step as the "answer", for
    // example: "Synthesize the gathered evidence into a comprehensive
    // explanation ...".  It is an instruction to a future turn, not the
    // explanation itself.  A colon or a later substantive sentence is enough
    // evidence that the imperative is being used as a heading followed by an
    // actual answer, so those cases remain valid.
    let progress_markers = [
        "need to inspect",
        "need to read",
        "must inspect",
        "must read",
        "listed root",
        "read root",
        "inspected",
        "gathered",
        "sufficient evidence",
        "enough evidence",
        "enough information",
        "enough context",
        "ready to",
        "can now",
        "no additional",
        "no more reads",
        "nothing more",
        "nothing else to inspect",
        "before explaining",
        "before producing",
        "before answering",
        "have gathered",
        "have enough",
        "i have reviewed",
        "reviewed the",
        "from prior reads",
        "prior reads",
        "the root cargo.toml",
        "workspace directory listing",
        "architecture doc",
        "readme describing",
        "main.rs",
        // Additional patterns from real-world failures
        "i have examined",
        "i found that",
        "i can see",
        "i can now",
        "now have",
        "has been gathered",
        "evidence to",
        "to fulfill",
        "fulfill the",
        "now explain",
        "now provide",
        "understand the",
        "understand how",
        "familiar with",
        "enough to",
        "sufficient to",
        "ready for",
        "prepared to",
        "in a position to",
        "all the information",
        "everything needed",
        "what is needed",
        "what's needed",
        "the information needed",
        "to deliver",
        "can deliver",
        "will now",
        "let me now",
        "i will now",
        "now i will",
        "now let me",
        "let's now",
    ];
    // A raw period is not necessarily a sentence boundary in repository
    // evidence: `Cargo.toml`, `main.rs`, and versions all contain one. Split
    // only a period followed by whitespace so the tail of a dotted filename
    // cannot masquerade as a substantive answer.
    let sentences = normalized
        .split(['!', '?', ';', '\n'])
        .flat_map(|part| part.split(". "))
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect::<Vec<_>>();
    let sentence_is_progress = |sentence: &str| {
        let sentence = sentence.to_ascii_lowercase();
        progress_markers
            .iter()
            .any(|marker| sentence.contains(marker))
    };
    let imperative_prefixes = [
        "synthesize ",
        "summarize ",
        "provide ",
        "produce ",
        "generate ",
        "write ",
        "explain ",
        "describe ",
        "identify ",
        "analyze ",
        "review ",
        "outline ",
        "compile ",
        "present ",
        "answer ",
        "give ",
        "tell ",
        "the next step is ",
        "next step: ",
        "now synthesize ",
        "now summarize ",
        "now provide ",
        "now explain ",
        "now produce ",
        "we should synthesize ",
        "i will synthesize ",
        "let's synthesize ",
        "now i can ",
        "i can now ",
        "i will now ",
        "now i will ",
        "let me now ",
        "let's now ",
        "now we can ",
        "we can now ",
        "to do this ",
    ];
    let imperative_meta = sentences.first().is_some_and(|first| {
        let first = first.to_ascii_lowercase();
        let meta_markers = [
            "gathered evidence",
            "gathered context",
            "information gathered",
            "prior reads",
            "findings",
            "repository context",
            "codebase",
            "the analysis",
            "the codebase",
            "the repository",
            "the structure",
            "the architecture",
            "the project",
            "the source",
            "the implementation",
            "the files",
            "the directory",
            "the workspace",
            "the crates",
            "the modules",
            "the entry points",
        ];
        imperative_prefixes
            .iter()
            .any(|prefix| first.starts_with(prefix))
            && meta_markers.iter().any(|marker| first.contains(marker))
            && !first.contains(':')
    });
    if imperative_meta
        && !sentences
            .iter()
            .skip(1)
            .any(|sentence| !sentence_is_progress(sentence))
    {
        return true;
    }

    let readiness_markers = [
        "gathered enough",
        "gathered sufficient",
        "sufficient evidence",
        "enough evidence",
        "enough information",
        "enough context",
        "ready to provide",
        "ready to produce",
        "ready to answer",
        "can now provide",
        "can now produce",
        "can now answer",
        "no additional reads",
        "no more reads",
        "nothing more to inspect",
        "nothing else to inspect",
        "without additional reads",
        "without further reads",
        "without more context",
        "have sufficient",
        "has sufficient",
        "now have sufficient",
        "now have enough",
        "to fulfill the",
        "fulfill the objective",
        "to answer this",
        "to provide a",
        "to deliver the",
        "can deliver the",
        "can explain the",
        "i can explain",
        "i will explain",
        "let me explain",
        "i am ready",
        "i'm ready",
        "am ready to",
        "in a position",
        "in position to",
    ];
    if !readiness_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return false;
    }

    // A sentence that only says what the agent read or that it is ready to
    // answer is progress, not the answer. Keep the check permissive enough for
    // a valid answer that starts with a short progress preface and then gives
    // concrete findings.
    let has_substantive_sentence = sentences
        .iter()
        .any(|sentence| !sentence_is_progress(sentence));
    !has_substantive_sentence
}

/// The evidence tokens the session's own action log records: every file path,
/// symbol, command and quoted evidence the agent read this turn.
///
/// FR-B5 backstop: a `complete: true` rationale that names no artifact the turn
/// actually read is a progress report, whatever its phrasing. The blocklist
/// above is the fast path; this is the backstop that a novel meta-answer cannot
/// slip past by using words the blocklist does not know. The session's own
/// action log is the ground truth — an answer that cites none of it has no
/// referent.
pub(crate) fn rationale_grounded_in_events(rationale: &str, events: &[SessionEvent]) -> bool {
    let lower = rationale.to_ascii_lowercase();
    let mut ground = Vec::new();

    // Every repository-relative path the model actually read becomes a token.
    // A path like `src/main.rs` is matched as a contiguous run so `main.rs`
    // and `src/main.rs` both count while a bare sentence fragment cannot.
    for event in events {
        let SessionEvent::ActionProposed { action, .. } = event else {
            continue;
        };
        let ProposedAction::RepositoryRead(read) = action else {
            continue;
        };
        match read {
            RepositoryReadAction::ReadFile { path, .. } => {
                ground.push(path.to_string_lossy().to_ascii_lowercase());
            }
            RepositoryReadAction::List { paths, .. }
            | RepositoryReadAction::Find { paths, .. }
            | RepositoryReadAction::RepositoryGrep { paths, .. }
            | RepositoryReadAction::GitDiff { paths } => {
                for path in paths {
                    let token = path.to_string_lossy().to_ascii_lowercase();
                    ground.push(token);
                }
            }
            RepositoryReadAction::GitShow { path, .. } => {
                ground.push(path.to_string_lossy().to_ascii_lowercase());
            }
            RepositoryReadAction::GitLsFiles { pathspec } => {
                for path in pathspec {
                    ground.push(path.to_string_lossy().to_ascii_lowercase());
                }
            }
            RepositoryReadAction::GitStatus
            | RepositoryReadAction::GitRevParse { .. }
            | RepositoryReadAction::GitLog { .. } => {
                // These reads carry no path argument; their output is the
                // evidence. Nothing to ground on.
            }
        }
    }

    // A directory listing (`.`) or a git read carries no path argument, yet its
    // output is the evidence the model is allowed to cite. Fold the recorded
    // read output into the ground too, so a rationale that names a file that a
    // `list` or `grep` surfaced is grounded.
    for event in events {
        let SessionEvent::ActionOutputRecorded { stdout, stderr, .. } = event else {
            continue;
        };
        ground.extend(read_output_tokens(stdout));
        ground.extend(read_output_tokens(stderr));
    }

    if ground.is_empty() {
        // The turn read nothing, so it has no artifact to cite. A completion
        // with nothing to ground on is only acceptable when the objective was
        // answerable from the prompt alone; that is the caller's decision via
        // the advice-only gate, so do not reject here.
        return true;
    }

    // A rationale is grounded when at least one read artifact appears in it as
    // a contiguous run. The full path counts (`src/main.rs` inside
    // `src/main.rs`), and so does the file stem, because a rationale naturally
    // cites `README` for `README.md` and `main` for `src/main.rs` — while a
    // bare sentence fragment (`file`, `the`) cannot.
    ground.iter().any(|token| {
        if token.is_empty() {
            return false;
        }
        if lower.contains(token) {
            return true;
        }
        let file_name = token.rsplit('/').next().unwrap_or(token);
        let stem = file_name
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(file_name);
        !stem.is_empty() && stem.len() >= 3 && lower.contains(stem)
    })
}

/// Path- and symbol-like fragments worth grounding a rationale against.
///
/// Splitting raw command output on whitespace would let the pronoun "you" or
/// the word "the" count as evidence; only fragments that look like a file path,
/// a module path, or a bare filename are returned.
fn read_output_tokens(output: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for candidate in output.split(|c: char| c.is_whitespace() || c == ',') {
        let candidate = candidate
            .trim_matches(|c: char| {
                !(c.is_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '/')
            })
            .to_ascii_lowercase();
        if candidate.is_empty() {
            continue;
        }
        let looks_path_like = candidate.contains('.') && !candidate.is_empty();
        let ends_like_source = candidate.ends_with(".rs")
            || candidate.ends_with(".toml")
            || candidate.ends_with(".md")
            || candidate.ends_with(".json")
            || candidate.ends_with(".lock")
            || candidate.ends_with(".png")
            || candidate.ends_with(".svg");
        if looks_path_like || ends_like_source {
            tokens.push(candidate);
        }
    }
    tokens
}

/// The full FR-B5 completion gate: fast-path blocklist first, then the
/// evidence-grounding backstop.
pub(crate) fn completion_needs_repair_with_events(
    objective: &str,
    rationale: &str,
    events: &[SessionEvent],
) -> bool {
    if completion_needs_repair(objective, rationale) {
        return true;
    }
    // The blocklist passed; the rationale must still be grounded in what the
    // turn actually read, otherwise it is a progress report wearing an answer's
    // clothes.
    !rationale_grounded_in_events(rationale, events)
}

#[cfg(test)]
mod advice_only_tests {
    use super::{
        completion_needs_repair, completion_needs_repair_with_events,
        objective_requests_advice_only,
    };

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

    #[test]
    fn detects_progress_only_completion_without_rejecting_concrete_answer() {
        assert!(completion_needs_repair(
            "Explain this codebase",
            "I have gathered sufficient evidence and can now provide the explanation. No additional reads are needed."
        ));
        assert!(completion_needs_repair(
            "Explain this codebase and identify the most important entry points",
            "I have gathered sufficient evidence to explain the codebase and identify the most important entry points. From prior reads I have: the root Cargo.toml listing all 31 crates, the README describing the project, the architecture doc detailing the four subsystems, the CLI main.rs showing all commands, and the workspace directory listing. I can now produce the final explanation without additional reads."
        ));
        assert!(completion_needs_repair(
            "Explain this codebase and identify the most important entry points.",
            "I have gathered sufficient evidence from the architecture docs, Cargo.toml, the main CLI entry point, and the directory structure to fulfill the objective. I can now explain the codebase and identify its most important entry points."
        ));
        // Additional real-world failures
        assert!(completion_needs_repair(
            "Explain this codebase",
            "I have examined the repository and reviewed the README, the architecture docs, and the directory structure. I now have sufficient evidence to explain the codebase."
        ));
        assert!(completion_needs_repair(
            "Explain how the daemon works",
            "I can now provide a comprehensive overview of the daemon. The information I gathered from reading daemon/src/main.rs and daemon/src/lib.rs is sufficient to answer this."
        ));
        assert!(completion_needs_repair(
            "Summarize the project",
            "Everything needed to summarize the project has been gathered. I am ready to deliver the summary."
        ));
        assert!(completion_needs_repair(
            "Explain this codebase and identify the most important entry points.",
            "I have reviewed the codebase architecture documentation, the main CLI entry point, the workspace Cargo.toml, and the directory structure. I have sufficient evidence to explain the codebase and identify its most important entry points."
        ));
        assert!(!completion_needs_repair(
            "Explain this codebase",
            "I reviewed the repository. The daemon owns session state, while the IDE renders the durable conversation and activity stream."
        ));
        assert!(completion_needs_repair("answer the request", "   "));
        assert!(!completion_needs_repair(
            "answer the request",
            "The requested change is complete and the relevant tests pass."
        ));
        assert!(completion_needs_repair(
            "Explain this codebase and identify the most important entry points",
            "Synthesize the gathered evidence into a comprehensive explanation of the codebase and its most important entry points."
        ));
        assert!(!completion_needs_repair(
            "Explain this codebase and identify the most important entry points",
            "Synthesize the gathered evidence: the daemon owns session state and the IDE renders the durable activity stream."
        ));
        assert!(!completion_needs_repair(
            "Explain this codebase and identify the most important entry points",
            "Synthesize the gathered evidence. The daemon owns session state and the IDE renders the durable activity stream."
        ));
    }

    #[test]
    fn the_observed_meta_answer_string_is_rejected_as_a_completion() {
        // PRD §2.4 regression: the exact phrasing that shipped as a finished
        // answer is a progress report — it names no artifact the turn read, so
        // it must be rejected even though none of the blocklist markers fire.
        let observed = "I have gathered sufficient evidence from the architecture documentation, \
            workspace manifests, and key source files (main.rs for both the CLI and the daemon) \
            to provide a comprehensive explanation of the codebase and identify its entry points.";
        // Fast path: several blocklist markers do fire ("gathered sufficient",
        // "sufficient evidence", "to provide a"), so this is rejected already.
        assert!(
            completion_needs_repair("Explain this codebase", observed),
            "the observed meta-answer must be rejected as a completion"
        );
    }

    #[test]
    fn evidence_grounding_rejects_an_ungrounded_rationale_even_without_blocklist_markers() {
        use purrcode_runtime_core::{ActionId, ProposedAction, SessionEvent};
        // The rationale mentions an artifact but the session's action log never
        // read it — nothing grounds the claim.
        let events = vec![SessionEvent::ActionProposed {
            action_id: ActionId::new(),
            action: ProposedAction::RepositoryRead(
                purrcode_runtime_core::RepositoryReadAction::ReadFile {
                    path: "README.md".into(),
                    max_bytes: 4096,
                },
            ),
        }];
        let rationale = "The daemon owns session state while the IDE renders the conversation.";
        // The blocklist alone accepts this (no progress marker fires).
        assert!(!completion_needs_repair("Explain this codebase", rationale));
        // The grounding backstop rejects it: nothing was actually read.
        assert!(completion_needs_repair_with_events(
            "Explain this codebase",
            rationale,
            &events
        ));
    }

    #[test]
    fn evidence_grounding_accepts_a_rationale_grounded_in_what_was_read() {
        use purrcode_runtime_core::{ActionId, ProposedAction, SessionEvent};
        let events = vec![SessionEvent::ActionProposed {
            action_id: ActionId::new(),
            action: ProposedAction::RepositoryRead(
                purrcode_runtime_core::RepositoryReadAction::ReadFile {
                    path: "src/main.rs".into(),
                    max_bytes: 4096,
                },
            ),
        }];
        let rationale = "src/main.rs is the CLI entry point: it parses the subcommands and \
            dispatches each to the daemon routes.";
        assert!(!completion_needs_repair("Explain this codebase", rationale));
        assert!(!completion_needs_repair_with_events(
            "Explain this codebase",
            rationale,
            &events
        ));
    }
}
