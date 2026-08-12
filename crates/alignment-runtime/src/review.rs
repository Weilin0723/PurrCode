//! The reviewers that actually run (v1.5 §8–§10).
//!
//! Three of the four v1.5 reviews need a model. The fourth — the deterministic
//! one — is the validation runtime and needs nothing from here; its outcomes
//! arrive as facts and are carried into these reviews as facts.
//!
//! What every reviewer here has in common is the shape of what it may return:
//! findings and verdicts. There is no field for a patch, no field for an edit,
//! and no way to express one. That is §28 held by the type system rather than
//! by instruction — **reviewers judge, repair agents repair** — and the reason
//! is that a reviewer that fixes what it finds has destroyed its own evidence.
//! The finding and the fix become one act, and nothing independent ever
//! confirmed the problem was real.
//!
//! The other thing they share is that they cannot see the implementer's
//! reasoning. Not because they are asked not to look — [`FreshReviewInput`] has
//! nowhere to put it.

use purrcode_runtime_core::expectation::RequirementStatus;
use purrcode_runtime_core::review::{
    FindingCategory, FindingId, ReviewFinding, ReviewId, ReviewKind, Severity,
};
use purrcode_runtime_core::work::RequirementId;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::fresh::FreshReviewInput;
use crate::{AlignmentError, ModelRoute};

const CODE_REVIEW_PROMPT: &str = "\
You are reviewing a branch you did not write. You have the requirements, the \
repository's rules, the diff, the files it touched and what the deterministic \
checks did. You do not have the author's account of their work, and you are not \
going to get it — judge the change, not the story around it.

Report correctness, regressions, security and architecture problems. For each \
one, say where it is and what you looked at; a finding with no evidence is an \
assertion, and something downstream will spend a repair cycle on it.

Severity means what it says. `critical` and `high` stop delivery, so use them \
for things that are wrong, not for things you would have done differently. \
`medium` and `low` are recorded and reported and do not hold the work.

You may not propose edits, and there is nowhere to put one. Say what is wrong \
and what should be done about it; somebody else does it.

Finding nothing is a legitimate outcome. A reviewer that always finds something \
is a reviewer nobody reads.";

const ALIGNMENT_REVIEW_PROMPT: &str = "\
You are checking whether this change is what the user asked for. Not whether \
the code is good — somebody else is doing that — whether it is the *right \
work*.

Go requirement by requirement, by index, and return one verdict for each:

- `satisfied`: the change does this, and you can say what in the diff does it.
- `violated`: the change contradicts it, or satisfies its letter and not its \
  point. \"Simplify the settings\" answered by deleting half the settings is \
  violated, not satisfied.
- `undetermined`: you looked and could not tell. This is a real answer and the \
  right one when the diff does not settle it. Do not round it up.

`undetermined` is not a failure and does not embarrass you. Rounding it up to \
`satisfied` is how a task gets reported as done when nobody established that it \
was.

Also report work the user did not ask for. A change that serves no requirement \
is scope drift even when it is an improvement, and the user cannot see it in a \
list of files.

You may not propose edits. Say what is wrong; somebody else fixes it.";

const COVERAGE_REVIEW_PROMPT: &str = "\
For each requirement, by index, decide whether the diff actually covers it and \
point at what does. You are checking coverage, not quality.

A requirement that nothing in the diff addresses is `undetermined` at best — \
never `satisfied` on the grounds that it looks straightforward. The whole \
purpose of this pass is to catch the requirement everybody forgot.";

/// What a reviewer decided about one requirement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Satisfied,
    Violated,
    /// Checked, and the check could not settle it.
    Undetermined,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftVerdict {
    /// The requirement's index, as listed in the prompt.
    requirement: usize,
    verdict: VerdictKind,
    /// Why. For `satisfied`, what in the diff does it; for `violated`, what
    /// contradicts it; for `undetermined`, what you could not establish.
    detail: String,
    /// What you looked at: file and line, a test name, a command's output.
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftFinding {
    /// One of: info, low, medium, high, critical.
    severity: String,
    /// One of: correctness, regression, requirement_gap, ux_mismatch, security,
    /// architecture, testing, scope_drift.
    category: String,
    /// The requirement this bears on, by index, when it bears on one.
    #[serde(default)]
    requirement: Option<usize>,
    description: String,
    /// What you looked at. A finding with none is an assertion.
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    affected_paths: Vec<String>,
    /// What to do about it — advice, never a patch.
    recommendation: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DraftReview {
    #[serde(default)]
    findings: Vec<DraftFinding>,
    #[serde(default)]
    verdicts: Vec<DraftVerdict>,
}

/// A reviewer's verdict on one requirement, resolved back to its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequirementVerdict {
    pub requirement_id: RequirementId,
    pub kind: VerdictKind,
    pub detail: String,
    /// What the reviewer looked at, which becomes the durable evidence a
    /// `Verified` status may then cite.
    pub evidence: Vec<String>,
}

impl RequirementVerdict {
    /// The status this verdict implies, given ids for the evidence it cited.
    ///
    /// `Satisfied` with nothing behind it does not become `Verified`. It
    /// becomes `Unknown`, which is the honest reading of a reviewer that said
    /// yes and could not say why — and, unlike `Verified`, does not clear the
    /// gate.
    pub fn into_status(
        self,
        evidence: Vec<purrcode_runtime_core::work::EvidenceId>,
    ) -> RequirementStatus {
        match self.kind {
            VerdictKind::Satisfied if evidence.is_empty() => RequirementStatus::Unknown {
                detail: format!(
                    "a reviewer said this holds and cited nothing: {}",
                    self.detail
                ),
            },
            VerdictKind::Satisfied => RequirementStatus::Verified { evidence },
            VerdictKind::Violated => RequirementStatus::Violated {
                evidence,
                detail: self.detail,
            },
            VerdictKind::Undetermined => RequirementStatus::Unknown {
                detail: self.detail,
            },
        }
    }
}

/// One review, as it came back.
#[derive(Clone, Debug)]
pub struct ReviewOutcome {
    pub review: ReviewId,
    pub kind: ReviewKind,
    pub findings: Vec<ReviewFinding>,
    pub verdicts: Vec<RequirementVerdict>,
    /// What this review cost, when the provider said. `None` is reported as
    /// unmeasured rather than as free.
    pub usage: crate::Usage,
}

impl ReviewOutcome {
    pub fn blocking(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.blocks_delivery())
            .count()
    }
}

/// Correctness, regressions, security and architecture, read without the
/// implementer's transcript (§9).
#[derive(Debug)]
pub struct IndependentCodeReviewer {
    route: ModelRoute,
}

impl IndependentCodeReviewer {
    pub fn new(route: ModelRoute) -> Self {
        Self { route }
    }

    pub async fn review(
        &self,
        input: FreshReviewInput,
        requirement_ids: &[RequirementId],
    ) -> Result<ReviewOutcome, AlignmentError> {
        run(
            &self.route,
            ReviewKind::IndependentCode,
            CODE_REVIEW_PROMPT,
            input,
            requirement_ids,
        )
        .await
    }
}

/// Whether the change is what the user asked for (§10).
#[derive(Debug)]
pub struct AlignmentReviewer {
    route: ModelRoute,
    kind: ReviewKind,
}

impl AlignmentReviewer {
    /// The reviewer that reads the change against the user's intent.
    pub fn user_alignment(route: ModelRoute) -> Self {
        Self {
            route,
            kind: ReviewKind::UserAlignment,
        }
    }

    /// The narrower pass: does the diff cover each requirement at all (§8)?
    pub fn requirement_coverage(route: ModelRoute) -> Self {
        Self {
            route,
            kind: ReviewKind::RequirementCoverage,
        }
    }

    pub async fn review(
        &self,
        input: FreshReviewInput,
        requirement_ids: &[RequirementId],
    ) -> Result<ReviewOutcome, AlignmentError> {
        let prompt = match self.kind {
            ReviewKind::RequirementCoverage => COVERAGE_REVIEW_PROMPT,
            _ => ALIGNMENT_REVIEW_PROMPT,
        };
        run(&self.route, self.kind, prompt, input, requirement_ids).await
    }
}

async fn run(
    route: &ModelRoute,
    kind: ReviewKind,
    prompt: &str,
    input: FreshReviewInput,
    requirement_ids: &[RequirementId],
) -> Result<ReviewOutcome, AlignmentError> {
    if input.requirements().len() != requirement_ids.len() {
        return Err(AlignmentError::Invalid(
            "the reviewer's requirement list and the contract's have drifted apart".into(),
        ));
    }
    let review = ReviewId::new();
    let messages = input.into_messages(prompt);
    let (draft, usage): (DraftReview, crate::Usage) =
        route.structured(messages, schema_for!(DraftReview)).await?;

    let mut findings = Vec::new();
    for raw in draft.findings {
        let description = raw.description.trim().to_owned();
        let recommendation = raw.recommendation.trim().to_owned();
        if description.is_empty() || recommendation.is_empty() {
            continue;
        }
        let evidence: Vec<String> = raw
            .evidence
            .into_iter()
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect();
        // A finding nothing established would cost a repair cycle. Dropped
        // rather than downgraded: there is nothing here to report to the user
        // either.
        if evidence.is_empty() {
            continue;
        }
        let requirement_id = raw
            .requirement
            .and_then(|index| requirement_ids.get(index))
            .copied();
        let category = parse_category(&raw.category);
        // A requirement gap that names no requirement is a complaint with
        // nowhere to land, and the log refuses it. Re-categorising is more
        // useful than dropping: the observation may still be worth reporting.
        let category = if category == FindingCategory::RequirementGap && requirement_id.is_none() {
            FindingCategory::ScopeDrift
        } else {
            category
        };
        findings.push(ReviewFinding {
            id: FindingId::new(),
            review,
            kind,
            severity: parse_severity(&raw.severity),
            category,
            requirement_id,
            description,
            evidence,
            affected_paths: raw
                .affected_paths
                .into_iter()
                .filter(|path| !path.trim().is_empty())
                .map(std::path::PathBuf::from)
                .collect(),
            recommendation,
        });
    }

    let mut verdicts = Vec::new();
    for raw in draft.verdicts {
        let Some(requirement_id) = requirement_ids.get(raw.requirement).copied() else {
            continue;
        };
        let detail = raw.detail.trim().to_owned();
        verdicts.push(RequirementVerdict {
            requirement_id,
            kind: raw.verdict,
            detail: if detail.is_empty() {
                "the reviewer gave no reason".to_owned()
            } else {
                detail
            },
            evidence: raw
                .evidence
                .into_iter()
                .map(|item| item.trim().to_owned())
                .filter(|item| !item.is_empty())
                .collect(),
        });
    }

    Ok(ReviewOutcome {
        review,
        kind,
        findings,
        verdicts,
        usage,
    })
}

fn parse_severity(raw: &str) -> Severity {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" => Severity::Critical,
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        // An unrecognised severity is not treated as blocking. A typo should
        // not be able to stop delivery, and the finding is still recorded.
        _ => Severity::Info,
    }
}

fn parse_category(raw: &str) -> FindingCategory {
    match raw.trim().to_ascii_lowercase().as_str() {
        "regression" => FindingCategory::Regression,
        "requirement_gap" | "requirementgap" => FindingCategory::RequirementGap,
        "ux_mismatch" | "uxmismatch" => FindingCategory::UxMismatch,
        "security" => FindingCategory::Security,
        "architecture" => FindingCategory::Architecture,
        "testing" => FindingCategory::Testing,
        "scope_drift" | "scopedrift" => FindingCategory::ScopeDrift,
        _ => FindingCategory::Correctness,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ScriptedProvider, route};
    use purrcode_runtime_core::expectation::{
        ExpectationClause, ExpectationContract, IntentSource,
    };
    use purrcode_runtime_core::work::{AcceptanceCriterion, CriterionId, EvidenceId};
    use serde_json::json;

    fn contract() -> ExpectationContract {
        let mut contract = ExpectationContract::new("Simplify Settings without losing capability");
        for statement in [
            "The user can add, edit and remove an MCP server",
            "Every setting that existed before is still reachable",
        ] {
            contract.clauses.push(ExpectationClause::required(
                statement,
                vec![AcceptanceCriterion {
                    id: CriterionId::new(),
                    statement: "observable from the Settings window".into(),
                }],
                IntentSource::new(0, "MCP must actually work"),
            ));
        }
        contract
    }

    fn ids(contract: &ExpectationContract) -> Vec<RequirementId> {
        contract.required().map(|clause| clause.id).collect()
    }

    #[tokio::test]
    async fn a_reviewer_returns_findings_that_land_on_real_requirements() {
        let contract = contract();
        let provider = ScriptedProvider::new(vec![json!({
            "findings": [{
                "severity": "high",
                "category": "requirement_gap",
                "requirement": 1,
                "description": "the advanced section was deleted rather than collapsed",
                "evidence": ["settings.rs:221"],
                "affected_paths": ["crates/purrcode-ide/src/app/settings.rs"],
                "recommendation": "put the controls behind a disclosure instead"
            }],
            "verdicts": [
                {"requirement": 0, "verdict": "satisfied", "detail": "add_server and remove_server are wired", "evidence": ["settings.rs:88"]},
                {"requirement": 1, "verdict": "violated", "detail": "eleven settings no longer exist", "evidence": ["settings.rs:221"]}
            ]
        })]);
        let outcome = AlignmentReviewer::user_alignment(route(provider))
            .review(FreshReviewInput::for_contract(&contract), &ids(&contract))
            .await
            .unwrap();
        assert_eq!(outcome.findings.len(), 1);
        assert_eq!(outcome.blocking(), 1);
        assert_eq!(
            outcome.findings[0].requirement_id,
            Some(ids(&contract)[1]),
            "the finding lands on the requirement the index named"
        );
        assert_eq!(outcome.verdicts.len(), 2);
        assert_eq!(outcome.verdicts[1].kind, VerdictKind::Violated);
    }

    #[tokio::test]
    async fn a_finding_with_no_evidence_never_becomes_a_repair_cycle() {
        let contract = contract();
        let provider = ScriptedProvider::new(vec![json!({
            "findings": [{
                "severity": "critical",
                "category": "correctness",
                "requirement": null,
                "description": "this feels wrong",
                "evidence": [],
                "affected_paths": [],
                "recommendation": "have another look"
            }],
            "verdicts": []
        })]);
        let outcome = IndependentCodeReviewer::new(route(provider))
            .review(FreshReviewInput::for_contract(&contract), &ids(&contract))
            .await
            .unwrap();
        assert!(
            outcome.findings.is_empty(),
            "an assertion is not a finding, and the correction loop would have spent a cycle on it"
        );
    }

    #[tokio::test]
    async fn a_satisfied_verdict_that_cites_nothing_does_not_clear_the_gate() {
        // The reviewer's version of the false Done: it said yes and could not
        // say why. `Unknown` is the honest reading, and it does not deliver.
        let verdict = RequirementVerdict {
            requirement_id: RequirementId::new(),
            kind: VerdictKind::Satisfied,
            detail: "looks right to me".into(),
            evidence: vec![],
        };
        let status = verdict.into_status(vec![]);
        assert!(!status.clears_delivery());
        assert_eq!(status.label(), "unknown");

        let cited = RequirementVerdict {
            requirement_id: RequirementId::new(),
            kind: VerdictKind::Satisfied,
            detail: "add_server and remove_server are wired".into(),
            evidence: vec!["settings.rs:88".into()],
        };
        assert!(cited.into_status(vec![EvidenceId::new()]).clears_delivery());
    }

    #[tokio::test]
    async fn an_unrecognised_severity_cannot_stop_delivery_by_accident() {
        let contract = contract();
        let provider = ScriptedProvider::new(vec![json!({
            "findings": [{
                "severity": "blocker",
                "category": "correctness",
                "requirement": null,
                "description": "the registry is never reloaded",
                "evidence": ["mcp_host.rs:83"],
                "affected_paths": [],
                "recommendation": "reload after a config write"
            }],
            "verdicts": []
        })]);
        let outcome = IndependentCodeReviewer::new(route(provider))
            .review(FreshReviewInput::for_contract(&contract), &ids(&contract))
            .await
            .unwrap();
        assert_eq!(outcome.findings.len(), 1, "it is still recorded");
        assert_eq!(outcome.blocking(), 0, "a typo does not become a blocker");
    }

    #[tokio::test]
    async fn a_finding_that_names_a_requirement_outside_the_list_is_not_attributed_to_one() {
        // The log refuses a finding naming a requirement the contract does not
        // have. Resolving by index rather than by id is what makes that
        // impossible to hit by accident.
        let contract = contract();
        let provider = ScriptedProvider::new(vec![json!({
            "findings": [{
                "severity": "medium",
                "category": "requirement_gap",
                "requirement": 9,
                "description": "something about a requirement that does not exist",
                "evidence": ["settings.rs:1"],
                "affected_paths": [],
                "recommendation": "n/a"
            }],
            "verdicts": [{"requirement": 9, "verdict": "satisfied", "detail": "…", "evidence": []}]
        })]);
        let outcome = AlignmentReviewer::requirement_coverage(route(provider))
            .review(FreshReviewInput::for_contract(&contract), &ids(&contract))
            .await
            .unwrap();
        assert_eq!(outcome.findings.len(), 1);
        assert!(outcome.findings[0].requirement_id.is_none());
        assert_eq!(
            outcome.findings[0].category,
            FindingCategory::ScopeDrift,
            "a requirement gap with nowhere to land is refused by the log; this one is reported"
        );
        assert!(
            outcome.verdicts.is_empty(),
            "a verdict on a requirement that does not exist is dropped"
        );
    }

    #[tokio::test]
    async fn every_finding_this_crate_produces_is_acceptable_to_the_log() {
        let contract = contract();
        let provider = ScriptedProvider::new(vec![json!({
            "findings": [{
                "severity": "high",
                "category": "ux_mismatch",
                "requirement": 0,
                "description": "the picker is three clicks deep",
                "evidence": ["settings.rs:140"],
                "affected_paths": [],
                "recommendation": "surface it on the first panel"
            }],
            "verdicts": []
        })]);
        let outcome = IndependentCodeReviewer::new(route(provider))
            .review(FreshReviewInput::for_contract(&contract), &ids(&contract))
            .await
            .unwrap();
        for finding in &outcome.findings {
            finding
                .validate()
                .expect("a finding this crate emits must be one the session log will hold");
        }
    }
}
