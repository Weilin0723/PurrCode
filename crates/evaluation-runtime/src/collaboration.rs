//! Collaborative dogfood and benchmark (v1.4 §PR15).
//!
//! The question this module exists to answer is uncomfortable and worth asking
//! plainly: **does delegation actually help, or does it only look
//! sophisticated?** The PRD's own framing is that delegation must improve the
//! outcome enough to justify the coordination cost, and that the classifier
//! should choose a single agent when it does not.
//!
//! So the unit of measurement here is not a run — it is a *pair* of runs of the
//! same task, one single-agent and one collaborative, compared on outcome
//! first and cost second. A collaborative run that succeeds while burning three
//! times the tokens for the same result is recorded as a loss, because it is
//! one.
//!
//! Every metric is **derived from the durable session log**, never
//! self-reported. A benchmark that asked the agent how it did would measure the
//! agent's opinion of itself.

use purrcode_runtime_core::{SessionEvent, SessionState, ValidationStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const COLLABORATION_SCHEMA_VERSION: u32 = 1;

/// The task categories the PRD asks the benchmark to cover (§PR15).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationCategory {
    SingleFileFix,
    MultiFileFeature,
    FrontendAndBackend,
    FeatureWithTests,
    FeatureWithMigration,
    SecuritySensitiveFeature,
    LargeRefactor,
    FailingCiDebug,
    DocumentationAndImplementation,
    MultiModuleBug,
}

impl CollaborationCategory {
    pub const ALL: &'static [Self] = &[
        Self::SingleFileFix,
        Self::MultiFileFeature,
        Self::FrontendAndBackend,
        Self::FeatureWithTests,
        Self::FeatureWithMigration,
        Self::SecuritySensitiveFeature,
        Self::LargeRefactor,
        Self::FailingCiDebug,
        Self::DocumentationAndImplementation,
        Self::MultiModuleBug,
    ];

    /// Whether delegation is *expected* to help for this shape of task.
    ///
    /// This is the benchmark's prior, and the classifier disagreeing with it is
    /// a finding rather than an error — but a run where the classifier
    /// delegates a single-file fix is a straightforward failure of §PR2.
    pub fn delegation_expected(self) -> bool {
        match self {
            Self::SingleFileFix | Self::FailingCiDebug => false,
            Self::MultiFileFeature
            | Self::FrontendAndBackend
            | Self::FeatureWithTests
            | Self::FeatureWithMigration
            | Self::SecuritySensitiveFeature
            | Self::LargeRefactor
            | Self::DocumentationAndImplementation
            | Self::MultiModuleBug => true,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SingleFileFix => "single-file fix",
            Self::MultiFileFeature => "multi-file feature",
            Self::FrontendAndBackend => "frontend + backend",
            Self::FeatureWithTests => "feature + tests",
            Self::FeatureWithMigration => "feature + migration",
            Self::SecuritySensitiveFeature => "security-sensitive feature",
            Self::LargeRefactor => "large refactor",
            Self::FailingCiDebug => "failing CI debugging",
            Self::DocumentationAndImplementation => "documentation + implementation",
            Self::MultiModuleBug => "multi-module bug",
        }
    }
}

/// One task, run twice.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollaborationTask {
    pub schema_version: u32,
    pub id: String,
    pub category: CollaborationCategory,
    pub objective: String,
    /// Paths the task is expected to touch. Used to score whether the work was
    /// actually done, independently of what the agent claimed.
    #[serde(default)]
    pub expected_changed_paths: Vec<String>,
    /// Paths no arm may touch. A run that writes here fails regardless of
    /// whether it also satisfied the objective.
    #[serde(default)]
    pub forbidden_paths: Vec<String>,
    pub maximum_seconds: u64,
}

/// Which arm of the comparison a run belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationArm {
    Single,
    Collaborative,
}

impl CollaborationArm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Collaborative => "collaborative",
        }
    }
}

/// What one arm of one task actually did, measured from its durable log.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CollaborationRun {
    pub arm_completed: bool,
    /// Validations that passed / failed, from `ValidationRecorded`.
    pub validations_passed: u32,
    pub validations_failed: u32,
    pub model_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub elapsed_seconds: u64,
    /// Workers started, from the delegation ledger. Zero for a single-agent arm
    /// — and a non-zero value there is itself a finding.
    pub workers: u32,
    pub conflicts: u32,
    /// Approvals a human had to give: action approvals plus integration
    /// decisions. The number a developer feels.
    pub human_interventions: u32,
    /// Actions PawGate denied. A run that repeatedly proposed forbidden work
    /// was not "safe", it was *stopped*, and the distinction matters.
    pub policy_denials: u32,
    /// Paths the run changed, so the objective can be scored independently.
    pub changed_paths: Vec<String>,
}

impl CollaborationRun {
    /// Derive every metric from a finished session's durable state.
    ///
    /// `elapsed_seconds` is the only value the caller supplies, because wall
    /// time is the one thing the log does not record end to end.
    pub fn from_session(
        state: &SessionState,
        events: &[SessionEvent],
        elapsed_seconds: u64,
    ) -> Self {
        let mut run = CollaborationRun {
            elapsed_seconds,
            workers: state.delegation_ledger.workers_started,
            input_tokens: state.delegation_ledger.total_worker_usage.input_tokens,
            output_tokens: state.delegation_ledger.total_worker_usage.output_tokens,
            ..CollaborationRun::default()
        };
        for event in events {
            match event {
                SessionEvent::SessionCompleted => run.arm_completed = true,
                SessionEvent::ModelRequestFinished {
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    run.model_calls += 1;
                    run.input_tokens += input_tokens.unwrap_or(0);
                    run.output_tokens += output_tokens.unwrap_or(0);
                }
                SessionEvent::ValidationRecorded { status, .. } => match status {
                    ValidationStatus::Passed => run.validations_passed += 1,
                    ValidationStatus::Failed | ValidationStatus::TimedOut => {
                        run.validations_failed += 1
                    }
                    _ => {}
                },
                SessionEvent::JudgmentRecorded { decision, .. } => {
                    if matches!(
                        decision,
                        purrcode_runtime_core::JudgmentDecision::Deny { .. }
                    ) {
                        run.policy_denials += 1;
                    }
                }
                SessionEvent::ApprovalRecorded { .. }
                | SessionEvent::IntegrationApproved { .. }
                | SessionEvent::IntegrationRejected { .. } => run.human_interventions += 1,
                SessionEvent::IntegrationConflictDetected { conflicts, .. } => {
                    run.conflicts += conflicts.len() as u32
                }
                SessionEvent::ActionOutputRecorded { .. } => {}
                _ => {}
            }
        }
        // Changed paths come from what was integrated and what the session's
        // own actions touched, not from any summary the model wrote.
        let mut changed: std::collections::BTreeSet<String> = state
            .delegations
            .values()
            .filter_map(|record| record.result.as_ref())
            .flat_map(|result| result.changed_paths.iter())
            .map(|path| path.display().to_string())
            .collect();
        for event in events {
            if let SessionEvent::IntegrationApplied { changed_paths, .. } = event {
                changed.extend(changed_paths.iter().map(|path| path.display().to_string()));
            }
        }
        run.changed_paths = changed.into_iter().collect();
        run
    }

    /// Total tokens, the cost axis the comparison ranks on.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Whether this run did the task: it completed, nothing failed validation,
    /// it touched what the task expected, and it touched nothing forbidden.
    pub fn succeeded_at(&self, task: &CollaborationTask) -> bool {
        if !self.arm_completed || self.validations_failed > 0 {
            return false;
        }
        if task.forbidden_paths.iter().any(|forbidden| {
            self.changed_paths
                .iter()
                .any(|changed| changed.starts_with(forbidden))
        }) {
            return false;
        }
        task.expected_changed_paths.iter().all(|expected| {
            self.changed_paths
                .iter()
                .any(|changed| changed.starts_with(expected))
        })
    }
}

/// Did delegation earn its keep on this task?
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationVerdict {
    /// Collaboration succeeded where the single agent did not.
    CollaborationWon,
    /// Both succeeded and collaboration did not cost materially more.
    Comparable,
    /// Both succeeded but collaboration cost materially more for no gain.
    NotWorthIt,
    /// Collaboration failed where the single agent succeeded.
    CollaborationRegressed,
    /// Neither arm did the task.
    BothFailed,
    /// The runtime chose not to delegate — the right answer for simple tasks,
    /// and scored as such rather than as a missing measurement.
    DeclinedToDelegate,
}

impl CollaborationVerdict {
    /// True when this verdict counts in delegation's favour, *including* a
    /// correct refusal to delegate. Refusing cheaply is a win for the product
    /// even though no workers ran.
    pub fn is_favourable(self) -> bool {
        matches!(
            self,
            Self::CollaborationWon | Self::Comparable | Self::DeclinedToDelegate
        )
    }
}

/// The cost multiple beyond which "it worked too" stops being good enough.
pub const MATERIAL_COST_MULTIPLE: f64 = 1.5;

/// One task's two runs, scored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollaborationComparison {
    pub schema_version: u32,
    pub task_id: String,
    pub category: CollaborationCategory,
    pub single: CollaborationRun,
    pub collaborative: CollaborationRun,
    pub verdict: CollaborationVerdict,
    /// Collaborative tokens ÷ single tokens. `None` when the single arm spent
    /// nothing measurable.
    pub cost_multiple: Option<f64>,
    /// True when the classifier's choice matched the category's prior.
    pub classification_as_expected: bool,
}

impl CollaborationComparison {
    pub fn score(
        task: &CollaborationTask,
        single: CollaborationRun,
        collaborative: CollaborationRun,
    ) -> Self {
        let single_ok = single.succeeded_at(task);
        let collaborative_ok = collaborative.succeeded_at(task);
        let cost_multiple = (single.total_tokens() > 0)
            .then(|| collaborative.total_tokens() as f64 / single.total_tokens() as f64);

        let verdict = if collaborative.workers == 0 {
            // The runtime declined. Correct for a simple task; a finding for a
            // task the category says should have split.
            CollaborationVerdict::DeclinedToDelegate
        } else {
            match (single_ok, collaborative_ok) {
                (false, true) => CollaborationVerdict::CollaborationWon,
                (true, false) => CollaborationVerdict::CollaborationRegressed,
                (false, false) => CollaborationVerdict::BothFailed,
                (true, true) => {
                    if cost_multiple.is_some_and(|multiple| multiple > MATERIAL_COST_MULTIPLE) {
                        CollaborationVerdict::NotWorthIt
                    } else {
                        CollaborationVerdict::Comparable
                    }
                }
            }
        };
        let classification_as_expected =
            (collaborative.workers > 0) == task.category.delegation_expected();

        Self {
            schema_version: COLLABORATION_SCHEMA_VERSION,
            task_id: task.id.clone(),
            category: task.category,
            single,
            collaborative,
            verdict,
            cost_multiple,
            classification_as_expected,
        }
    }
}

/// The whole benchmark, aggregated.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollaborationReport {
    pub schema_version: u32,
    pub comparisons: Vec<CollaborationComparison>,
}

impl CollaborationReport {
    pub fn new(comparisons: Vec<CollaborationComparison>) -> Self {
        Self {
            schema_version: COLLABORATION_SCHEMA_VERSION,
            comparisons,
        }
    }

    pub fn tasks(&self) -> usize {
        self.comparisons.len()
    }

    /// How often the classifier agreed with the category's prior. This is the
    /// §PR15 "critical metric" in one number: a runtime that delegates
    /// everything scores badly here even if every run happens to pass.
    pub fn classification_accuracy(&self) -> f64 {
        if self.comparisons.is_empty() {
            return 0.0;
        }
        let correct = self
            .comparisons
            .iter()
            .filter(|comparison| comparison.classification_as_expected)
            .count();
        correct as f64 / self.comparisons.len() as f64
    }

    /// Share of tasks where delegation (or a correct refusal) was the right
    /// call.
    pub fn favourable_rate(&self) -> f64 {
        if self.comparisons.is_empty() {
            return 0.0;
        }
        let favourable = self
            .comparisons
            .iter()
            .filter(|comparison| comparison.verdict.is_favourable())
            .count();
        favourable as f64 / self.comparisons.len() as f64
    }

    /// Regressions: tasks the single agent did and the collaborative one broke.
    /// The number that would block a release on its own.
    pub fn regressions(&self) -> usize {
        self.comparisons
            .iter()
            .filter(|comparison| comparison.verdict == CollaborationVerdict::CollaborationRegressed)
            .count()
    }

    pub fn conflicts(&self) -> u32 {
        self.comparisons
            .iter()
            .map(|comparison| comparison.collaborative.conflicts)
            .sum()
    }

    pub fn by_category(&self) -> BTreeMap<CollaborationCategory, usize> {
        let mut counts = BTreeMap::new();
        for comparison in &self.comparisons {
            *counts.entry(comparison.category).or_insert(0) += 1;
        }
        counts
    }

    /// Tasks the category says should NOT have been split, that were split
    /// anyway.
    ///
    /// Separate from [`Self::classification_accuracy`] because §12 lists
    /// "simple tasks stay single-agent" as its own release gate, and a
    /// percentage hides the difference between one borderline call and a
    /// runtime that splits everything. This is binary on purpose.
    pub fn simple_tasks_delegated(&self) -> Vec<&str> {
        self.comparisons
            .iter()
            .filter(|comparison| {
                !comparison.category.delegation_expected() && comparison.collaborative.workers > 0
            })
            .map(|comparison| comparison.task_id.as_str())
            .collect()
    }

    /// Does this run clear the §PR15 bar?
    ///
    /// Four conditions, and the third is the one a mediocre runtime fails: a
    /// high pass rate bought by delegating everything does not clear it,
    /// because splitting even one task that should have stayed single-agent is
    /// a failure of the release's central claim.
    pub fn meets_release_bar(&self) -> bool {
        self.tasks() >= 10
            && self.regressions() == 0
            && self.simple_tasks_delegated().is_empty()
            && self.favourable_rate() >= 0.8
    }

    /// A human-readable summary for the release record.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# v1.4 collaborative benchmark\n\n");
        out.push_str(&format!(
            "{} task(s) · classification accuracy {:.0}% · favourable {:.0}% · \
             {} regression(s) · {} conflict(s)\n\n",
            self.tasks(),
            self.classification_accuracy() * 100.0,
            self.favourable_rate() * 100.0,
            self.regressions(),
            self.conflicts(),
        ));
        let over_delegated = self.simple_tasks_delegated();
        if !over_delegated.is_empty() {
            out.push_str(&format!(
                "**Simple tasks that were split anyway: {}.** This fails the \
                 \"simple tasks stay single-agent\" gate on its own.\n\n",
                over_delegated.join(", ")
            ));
        }
        out.push_str(
            "| task | category | verdict | workers | cost× | single | collaborative |\n\
             | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for comparison in &self.comparisons {
            out.push_str(&format!(
                "| {} | {} | {:?} | {} | {} | {} tok | {} tok |\n",
                comparison.task_id,
                comparison.category.label(),
                comparison.verdict,
                comparison.collaborative.workers,
                comparison
                    .cost_multiple
                    .map(|multiple| format!("{multiple:.2}"))
                    .unwrap_or_else(|| "—".into()),
                comparison.single.total_tokens(),
                comparison.collaborative.total_tokens(),
            ));
        }
        out.push_str(&format!(
            "\nRelease bar: **{}**\n",
            if self.meets_release_bar() {
                "met"
            } else {
                "not met"
            }
        ));
        out
    }
}

/// The v1.4 task catalog: one task per PRD category, ten in total.
///
/// The objectives are written against a PurrCode-shaped Rust workspace because
/// that is the repository the team dogfoods on. They are deliberately concrete
/// — an objective like "improve the code" cannot be scored.
pub fn default_catalog() -> Vec<CollaborationTask> {
    let task = |id: &str,
                category: CollaborationCategory,
                objective: &str,
                expected: &[&str],
                forbidden: &[&str],
                seconds: u64| CollaborationTask {
        schema_version: COLLABORATION_SCHEMA_VERSION,
        id: id.to_owned(),
        category,
        objective: objective.to_owned(),
        expected_changed_paths: expected.iter().map(|p| (*p).to_owned()).collect(),
        forbidden_paths: forbidden.iter().map(|p| (*p).to_owned()).collect(),
        maximum_seconds: seconds,
    };
    vec![
        task(
            "rename-constant",
            CollaborationCategory::SingleFileFix,
            "Rename the MAX_RETRIES constant to MAXIMUM_RETRY_ATTEMPTS in src/retry.rs \
             and update its uses in that file.",
            &["src/retry.rs"],
            &["migrations/"],
            300,
        ),
        task(
            "paginate-list-endpoint",
            CollaborationCategory::MultiFileFeature,
            "Add offset/limit pagination to the item list endpoint: parse the query \
             parameters, bound them, and return the total count alongside the page.",
            &["src/"],
            &[],
            900,
        ),
        task(
            "settings-toggle",
            CollaborationCategory::FrontendAndBackend,
            "Add a per-user 'compact view' preference: persist it in the backend and \
             read it in the web settings panel.",
            &["src/", "web/"],
            &[],
            900,
        ),
        task(
            "retry-backoff-with-tests",
            CollaborationCategory::FeatureWithTests,
            "Add exponential backoff with jitter to the retry helper, and cover the \
             bounds and the jitter range with tests.",
            &["src/retry.rs", "tests/"],
            &[],
            900,
        ),
        task(
            "account-link-table",
            CollaborationCategory::FeatureWithMigration,
            "Add persistent account linking: a migration for the account_links table \
             and the repository code that reads and writes it.",
            &["migrations/", "src/"],
            &[],
            900,
        ),
        task(
            "oauth-token-exchange",
            CollaborationCategory::SecuritySensitiveFeature,
            "Add OAuth authorization-code token exchange with persistent account \
             linking, tests, and an independent security review.",
            &["src/auth/", "migrations/", "tests/"],
            &[],
            1200,
        ),
        task(
            "extract-http-client",
            CollaborationCategory::LargeRefactor,
            "Extract the duplicated HTTP client setup into one module and update every \
             call site to use it.",
            &["src/"],
            &[],
            1200,
        ),
        task(
            "fix-flaky-timeout-test",
            CollaborationCategory::FailingCiDebug,
            "The timeout test fails intermittently in CI. Find the cause and fix it \
             without weakening what the test asserts.",
            &["src/"],
            &[],
            600,
        ),
        task(
            "document-and-add-health-endpoint",
            CollaborationCategory::DocumentationAndImplementation,
            "Add a /health endpoint that reports dependency status, and document it in \
             the API reference.",
            &["src/", "docs/"],
            &[],
            900,
        ),
        task(
            "fix-cross-module-id-mismatch",
            CollaborationCategory::MultiModuleBug,
            "Ids created in the scheduler do not match the ids the reporter looks up, \
             so completed jobs never appear. Fix the mismatch in both modules.",
            &["src/"],
            &[],
            900,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::{ActionId, SessionId};

    fn task(category: CollaborationCategory, expected: &[&str]) -> CollaborationTask {
        CollaborationTask {
            schema_version: COLLABORATION_SCHEMA_VERSION,
            id: "t".into(),
            category,
            objective: "do the thing".into(),
            expected_changed_paths: expected.iter().map(|p| (*p).to_owned()).collect(),
            forbidden_paths: vec!["secrets/".into()],
            maximum_seconds: 600,
        }
    }

    fn run(tokens: u64, workers: u32, changed: &[&str], completed: bool) -> CollaborationRun {
        CollaborationRun {
            arm_completed: completed,
            input_tokens: tokens,
            workers,
            changed_paths: changed.iter().map(|p| (*p).to_owned()).collect(),
            ..CollaborationRun::default()
        }
    }

    #[test]
    fn metrics_are_derived_from_the_log_not_from_a_summary() {
        let session = SessionId::new();
        let mut state = SessionState::empty(session);
        let events = vec![
            SessionEvent::SessionCreated {
                objective: "add oauth".into(),
                repository: "/repo".into(),
                authority_mode: Default::default(),
            },
            SessionEvent::ModelRequestFinished {
                role: "coding_worker".into(),
                input_tokens: Some(1_200),
                output_tokens: Some(300),
            },
            SessionEvent::ModelRequestFinished {
                role: "coding_worker".into(),
                input_tokens: Some(800),
                output_tokens: Some(200),
            },
            SessionEvent::ValidationRecorded {
                action_id: ActionId::new(),
                status: ValidationStatus::Passed,
                evidence: "cargo test".into(),
            },
            SessionEvent::ValidationRecorded {
                action_id: ActionId::new(),
                status: ValidationStatus::Failed,
                evidence: "cargo clippy".into(),
            },
            SessionEvent::SessionCompleted,
        ];
        for event in &events {
            // Not every fixture event is a legal transition in isolation; the
            // metric derivation reads the event list, and the state supplies
            // the delegation ledger.
            let _ = state.reduce_event(event);
        }
        let measured = CollaborationRun::from_session(&state, &events, 42);
        assert_eq!(measured.model_calls, 2);
        assert_eq!(measured.input_tokens, 2_000);
        assert_eq!(measured.output_tokens, 500);
        assert_eq!(measured.validations_passed, 1);
        assert_eq!(measured.validations_failed, 1);
        assert_eq!(measured.elapsed_seconds, 42);
        assert!(measured.arm_completed);
    }

    #[test]
    fn a_run_that_failed_validation_did_not_succeed_however_it_finished() {
        let task = task(CollaborationCategory::MultiFileFeature, &["src/"]);
        let mut measured = run(1_000, 2, &["src/lib.rs"], true);
        assert!(measured.succeeded_at(&task));
        measured.validations_failed = 1;
        assert!(!measured.succeeded_at(&task));
    }

    #[test]
    fn touching_a_forbidden_path_fails_the_task_even_when_the_objective_was_met() {
        let task = task(CollaborationCategory::MultiFileFeature, &["src/"]);
        let measured = run(1_000, 2, &["src/lib.rs", "secrets/key.pem"], true);
        assert!(!measured.succeeded_at(&task));
    }

    #[test]
    fn work_the_task_expected_must_actually_have_happened() {
        let task = task(
            CollaborationCategory::FeatureWithMigration,
            &["migrations/"],
        );
        // Completed, validated, and did not touch the migration it was for.
        let measured = run(1_000, 2, &["src/lib.rs"], true);
        assert!(!measured.succeeded_at(&task));
    }

    #[test]
    fn succeeding_more_expensively_for_the_same_result_is_not_a_win() {
        // The §PR15 question. Both arms did the task; the collaborative one
        // spent 3× the tokens. That is a loss, and the report says so.
        let task = task(CollaborationCategory::MultiFileFeature, &["src/"]);
        let comparison = CollaborationComparison::score(
            &task,
            run(1_000, 0, &["src/lib.rs"], true),
            run(3_000, 3, &["src/lib.rs"], true),
        );
        assert_eq!(comparison.verdict, CollaborationVerdict::NotWorthIt);
        assert!(!comparison.verdict.is_favourable());
        assert_eq!(comparison.cost_multiple, Some(3.0));
    }

    #[test]
    fn a_comparable_cost_for_the_same_result_is_acceptable() {
        let task = task(CollaborationCategory::MultiFileFeature, &["src/"]);
        let comparison = CollaborationComparison::score(
            &task,
            run(1_000, 0, &["src/lib.rs"], true),
            run(1_200, 3, &["src/lib.rs"], true),
        );
        assert_eq!(comparison.verdict, CollaborationVerdict::Comparable);
        assert!(comparison.verdict.is_favourable());
    }

    #[test]
    fn delegation_winning_and_regressing_are_both_recorded() {
        let task = task(CollaborationCategory::MultiFileFeature, &["src/"]);
        let won = CollaborationComparison::score(
            &task,
            run(1_000, 0, &["src/lib.rs"], false),
            run(2_000, 3, &["src/lib.rs"], true),
        );
        assert_eq!(won.verdict, CollaborationVerdict::CollaborationWon);

        let regressed = CollaborationComparison::score(
            &task,
            run(1_000, 0, &["src/lib.rs"], true),
            run(2_000, 3, &["src/lib.rs"], false),
        );
        assert_eq!(
            regressed.verdict,
            CollaborationVerdict::CollaborationRegressed
        );
        assert!(!regressed.verdict.is_favourable());
    }

    #[test]
    fn declining_to_delegate_a_simple_task_is_the_right_answer() {
        let task = task(CollaborationCategory::SingleFileFix, &["src/retry.rs"]);
        let comparison = CollaborationComparison::score(
            &task,
            run(500, 0, &["src/retry.rs"], true),
            // The classifier refused: no workers ran.
            run(500, 0, &["src/retry.rs"], true),
        );
        assert_eq!(comparison.verdict, CollaborationVerdict::DeclinedToDelegate);
        assert!(comparison.verdict.is_favourable());
        assert!(
            comparison.classification_as_expected,
            "refusing to split a single-file fix is the expected classification"
        );
    }

    #[test]
    fn delegating_a_single_file_fix_is_flagged_as_a_misclassification() {
        let task = task(CollaborationCategory::SingleFileFix, &["src/retry.rs"]);
        let comparison = CollaborationComparison::score(
            &task,
            run(500, 0, &["src/retry.rs"], true),
            run(2_000, 3, &["src/retry.rs"], true),
        );
        assert!(!comparison.classification_as_expected);
    }

    #[test]
    fn the_release_bar_is_not_cleared_by_delegating_everything() {
        // Ten tasks, every one passing, but the classifier split the simple
        // ones too. High pass rate, wrong product.
        let comparisons: Vec<_> = CollaborationCategory::ALL
            .iter()
            .map(|category| {
                let task = task(*category, &["src/"]);
                CollaborationComparison::score(
                    &task,
                    run(1_000, 0, &["src/lib.rs"], true),
                    run(1_100, 3, &["src/lib.rs"], true),
                )
            })
            .collect();
        let report = CollaborationReport::new(comparisons);
        assert_eq!(report.tasks(), 10);
        assert_eq!(report.regressions(), 0);
        assert_eq!(
            report.simple_tasks_delegated().len(),
            2,
            "both no-delegation categories were split"
        );
        assert!(
            !report.meets_release_bar(),
            "a run that delegates everything must not clear the bar, whatever its pass rate"
        );
        assert!(
            report.to_markdown().contains("stay single-agent"),
            "the report must name the gate it failed"
        );
    }

    #[test]
    fn one_regression_blocks_the_release_bar_on_its_own() {
        let mut comparisons: Vec<_> = CollaborationCategory::ALL
            .iter()
            .map(|category| {
                let task = task(*category, &["src/"]);
                let delegated = if category.delegation_expected() { 3 } else { 0 };
                CollaborationComparison::score(
                    &task,
                    run(1_000, 0, &["src/lib.rs"], true),
                    run(1_100, delegated, &["src/lib.rs"], true),
                )
            })
            .collect();
        let report = CollaborationReport::new(comparisons.clone());
        assert!(report.meets_release_bar(), "the clean run clears the bar");

        let task = task(CollaborationCategory::LargeRefactor, &["src/"]);
        comparisons[6] = CollaborationComparison::score(
            &task,
            run(1_000, 0, &["src/lib.rs"], true),
            run(1_100, 3, &["src/lib.rs"], false),
        );
        let regressed = CollaborationReport::new(comparisons);
        assert_eq!(regressed.regressions(), 1);
        assert!(!regressed.meets_release_bar());
    }

    #[test]
    fn the_catalog_covers_every_prd_category() {
        let catalog = default_catalog();
        assert_eq!(catalog.len(), 10);
        for category in CollaborationCategory::ALL {
            assert!(
                catalog.iter().any(|task| task.category == *category),
                "category {category:?} is missing from the catalog"
            );
        }
        for task in &catalog {
            assert!(!task.objective.trim().is_empty());
            assert!(
                !task.expected_changed_paths.is_empty(),
                "task `{}` has nothing to score against",
                task.id
            );
        }
    }

    #[test]
    fn the_markdown_report_states_the_bar_plainly() {
        let task = task(CollaborationCategory::MultiFileFeature, &["src/"]);
        let report = CollaborationReport::new(vec![CollaborationComparison::score(
            &task,
            run(1_000, 0, &["src/lib.rs"], true),
            run(3_000, 3, &["src/lib.rs"], true),
        )]);
        let markdown = report.to_markdown();
        assert!(markdown.contains("NotWorthIt"));
        assert!(markdown.contains("Release bar: **not met**"));
        assert!(markdown.contains("3.00"));
    }
}
