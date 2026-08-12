//! Durable expectation model (v1.5 §3–§5).
//!
//! v1.4 settled how changes are made safely: workers propose, evidence proves,
//! the parent integrates. It said nothing about whether the change was the one
//! the user wanted — and a coding agent fails that way far more often than it
//! fails to compile. Build passes, tests pass, lint passes, and the settings
//! page is still unusable, because "don't clutter the UI" lived only in a
//! message that was later summarised into "the user wants better settings".
//!
//! So what the user asked for becomes durable state, next to the work model in
//! [`crate::work`] and for the same reason: conversation is how a person steers
//! PurrCode, but it is not an adequate source of truth for a long-running
//! change. Conversation may be compacted freely. This may not.
//!
//! Three rules run through the whole module:
//!
//! - **Nothing is verified without evidence.** A status is a claim about the
//!   world, and a claim with nothing behind it is the false `Done` this release
//!   exists to stop.
//! - **Counts, not percentages.** "6 / 7 requirements verified" can be wrong,
//!   and therefore means something. "94% aligned" cannot be.
//! - **A correction revises the contract; it does not append to it.** When the
//!   user says "no, keep the sidebar", work done under "remove the sidebar"
//!   must stop counting as progress.

pub mod contract;
pub mod delivery;
pub mod evidence;
pub mod revision;

pub use contract::{
    Assumption, AssumptionId, EXPECTATION_SCHEMA_VERSION, ExpectationClause, ExpectationContract,
    ExpectationStrength, IntentSource, NonGoal, OpenQuestion, QuestionId, RequirementStatus,
    RequirementTally,
};
pub use delivery::{
    DeliveryAssessment, DeliveryBlocker, DeliveryInputs, DeliveryState, RequiredValidation,
};
pub use evidence::{
    AlignmentEvidence, AlignmentEvidenceKind, CitationFault, EvidenceLedger, check_citations,
};
pub use revision::{ContractChange, ContractRevision, RevisedContract};

use crate::work::{CriterionId, RequirementId};
use thiserror::Error;

/// Mirrors `work::durable_id!` so contract ids serialise identically to the
/// requirement ids they sit beside.
macro_rules! expectation_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            schemars::JsonSchema,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}
pub(crate) use expectation_id;

/// Everything that can be wrong with a contract or a correction to one.
#[derive(Debug, Error)]
pub enum ExpectationError {
    #[error("{0}")]
    Invalid(String),
    #[error("requirement {0:?} appears twice in the contract")]
    DuplicateClause(RequirementId),
    #[error("acceptance criterion {0:?} appears twice in the contract")]
    DuplicateCriterion(CriterionId),
    /// A hard requirement with no acceptance criterion cannot be checked, and a
    /// requirement nothing can check is one that will be declared satisfied by
    /// whoever is in a hurry.
    #[error("hard requirement {0:?} has no acceptance criterion, so nothing can check it")]
    UncheckableRequirement(RequirementId),
    #[error("requirement {0:?} is marked verified with no evidence behind it")]
    UnevidencedVerification(RequirementId),
    #[error("requirement {0:?} is waived without a reason")]
    UnreasonedWaiver(RequirementId),
    #[error("requirement {0:?} is not in this contract")]
    UnknownRequirement(RequirementId),
    #[error("assumption {0} is not in this contract")]
    UnknownAssumption(uuid::Uuid),
    #[error("open question {0} is not in this contract")]
    UnknownQuestion(uuid::Uuid),
    #[error("revision {found} does not follow revision {expected}")]
    RevisionOutOfOrder { expected: u64, found: u64 },
}
