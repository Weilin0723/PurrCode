//! What a verification is allowed to point at (v1.5 §8).
//!
//! The contract already refuses `Verified` with an empty evidence list, and
//! that closes the crude version of the failure. It does not close the honest
//! one. `EvidenceId::new()` produces a perfectly non-empty list of perfectly
//! valid identifiers that establish nothing at all, and a reviewer under
//! pressure to close a requirement will reach for exactly that shape without
//! ever intending to deceive anyone.
//!
//! So evidence is a durable record before it is a citation. An id must resolve
//! to something the log actually contains, and that something must be about the
//! requirement it is cited for:
//!
//! > **Verified means the evidence exists and is about this requirement — not
//! > that the evidence list is non-empty.**
//!
//! The second half matters as much as the first. Citing the settings test for
//! "MCP configuration works" passes any check that only asks whether the id
//! resolves, and it is the more likely mistake of the two: the ids are opaque,
//! the model is holding several, and nothing about a UUID says which
//! requirement it belongs to.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ExpectationError;
use super::contract::require_text;
use crate::work::{EvidenceId, RequirementId};

/// What produced a piece of evidence.
///
/// Kept on the record because the three carry different weight and the user is
/// entitled to see which one closed a requirement. A requirement verified only
/// by `Review` was settled by a model reading a diff; one verified by
/// `Validation` was settled by a command that exited zero. Both are legitimate,
/// and collapsing them would hide which one happened.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentEvidenceKind {
    /// The agent ran something and the log holds what happened.
    Execution,
    /// A deterministic check: a build, a test, a lint.
    Validation,
    /// A reviewer looked at the work and said what it saw.
    Review,
}

impl AlignmentEvidenceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Execution => "execution",
            Self::Validation => "validation",
            Self::Review => "review",
        }
    }
}

/// One durable thing a requirement's status may be built on.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AlignmentEvidence {
    pub id: EvidenceId,
    pub kind: AlignmentEvidenceKind,
    /// The requirement this bears on.
    ///
    /// Recorded at the point the evidence is gathered, not at the point it is
    /// cited, so the citation can be checked against something written down
    /// before anyone needed the requirement to pass.
    pub requirement_id: RequirementId,
    /// Where it came from, in terms a user can follow back: `cargo test`,
    /// `settings.rs:221`, `alignment review 3`.
    pub source: String,
    /// What it showed.
    pub detail: String,
    pub recorded_at: DateTime<Utc>,
}

impl AlignmentEvidence {
    pub fn new(
        kind: AlignmentEvidenceKind,
        requirement_id: RequirementId,
        source: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: EvidenceId::new(),
            kind,
            requirement_id,
            source: source.into(),
            detail: detail.into(),
            recorded_at: Utc::now(),
        }
    }

    pub fn validate(&self) -> Result<(), ExpectationError> {
        require_text("evidence source", &self.source)?;
        require_text("evidence detail", &self.detail)?;
        Ok(())
    }
}

/// Everything the session has recorded, indexed by id.
///
/// A `BTreeMap` in the session state rather than a list, because the question
/// asked of it — "does this citation resolve, and is it about this
/// requirement?" — is asked once per cited id on every status change.
pub type EvidenceLedger = std::collections::BTreeMap<EvidenceId, AlignmentEvidence>;

/// Why a citation was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CitationFault {
    /// The id resolves to nothing. A freshly minted UUID looks exactly like
    /// this, which is the point.
    Unresolved(EvidenceId),
    /// The evidence exists and is about a different requirement.
    Misattributed {
        evidence: EvidenceId,
        recorded_for: RequirementId,
        cited_for: RequirementId,
    },
}

impl std::fmt::Display for CitationFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unresolved(id) => write!(
                formatter,
                "evidence {id:?} was cited but never recorded, so it establishes nothing"
            ),
            Self::Misattributed {
                evidence,
                recorded_for,
                cited_for,
            } => write!(
                formatter,
                "evidence {evidence:?} was recorded for requirement {recorded_for:?} \
                 and cannot settle {cited_for:?}"
            ),
        }
    }
}

/// Check that every cited id resolves to evidence about `requirement`.
///
/// Returns the first fault rather than all of them: a status change carries a
/// handful of citations, and a caller that got one wrong has a bug to fix
/// before the rest of the list is interesting.
pub fn check_citations(
    ledger: &EvidenceLedger,
    requirement: RequirementId,
    cited: &[EvidenceId],
) -> Result<(), CitationFault> {
    for id in cited {
        let Some(evidence) = ledger.get(id) else {
            return Err(CitationFault::Unresolved(*id));
        };
        if evidence.requirement_id != requirement {
            return Err(CitationFault::Misattributed {
                evidence: *id,
                recorded_for: evidence.requirement_id,
                cited_for: requirement,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_with(requirement: RequirementId) -> (EvidenceLedger, EvidenceId) {
        let evidence = AlignmentEvidence::new(
            AlignmentEvidenceKind::Validation,
            requirement,
            "cargo test",
            "mcp_config_roundtrip passed",
        );
        let id = evidence.id;
        let mut ledger = EvidenceLedger::new();
        ledger.insert(id, evidence);
        (ledger, id)
    }

    #[test]
    fn a_freshly_minted_id_is_not_evidence() {
        // The whole reason this module exists: `EvidenceId::new()` satisfies
        // "the list is non-empty" and establishes nothing.
        let requirement = RequirementId::new();
        let (ledger, _) = ledger_with(requirement);
        let fault =
            check_citations(&ledger, requirement, &[EvidenceId::new()]).expect_err("must refuse");
        assert!(matches!(fault, CitationFault::Unresolved(_)), "{fault:?}");
    }

    #[test]
    fn evidence_for_another_requirement_does_not_settle_this_one() {
        // The likelier mistake. The ids are opaque, the agent is holding
        // several, and nothing about a UUID says which requirement it is for.
        let mcp = RequirementId::new();
        let settings = RequirementId::new();
        let (ledger, id) = ledger_with(settings);
        let fault = check_citations(&ledger, mcp, &[id]).expect_err("must refuse");
        assert!(
            matches!(fault, CitationFault::Misattributed { .. }),
            "{fault:?}"
        );
        assert!(fault.to_string().contains("cannot settle"));
    }

    #[test]
    fn evidence_recorded_for_this_requirement_settles_it() {
        let requirement = RequirementId::new();
        let (ledger, id) = ledger_with(requirement);
        check_citations(&ledger, requirement, &[id]).expect("this is what evidence is for");
    }

    #[test]
    fn evidence_must_say_where_it_came_from_and_what_it_showed() {
        let mut evidence = AlignmentEvidence::new(
            AlignmentEvidenceKind::Review,
            RequirementId::new(),
            "   ",
            "the panel now hides advanced controls",
        );
        assert!(evidence.validate().is_err());
        evidence.source = "alignment review 2".into();
        evidence.detail = "  ".into();
        assert!(
            evidence.validate().is_err(),
            "evidence with no finding behind it is a citation to nothing"
        );
    }
}
