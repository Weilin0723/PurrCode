//! The agent workspace and integration review (v1.4 §PR11, §PR12).
//!
//! Not a swarm visualization — a worker tree you can act on. Every row is a
//! delegation the daemon knows about, every field comes from the daemon's
//! projection of the durable log, and every action (accept, reject, cancel) is
//! a daemon command. There is deliberately no client-side delegation state: if
//! this panel and the daemon ever disagreed, the panel would be the one lying,
//! and the user would be approving something other than what lands.

use serde_json::Value;

/// One worker in the tree.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DelegationRow {
    pub delegation_id: String,
    pub objective: String,
    pub capability: String,
    pub status: String,
    pub integration_state: String,
    pub access: String,
    pub agent_profile: Option<String>,
    pub model_role: Option<String>,
    pub routing_reason: Option<String>,
    pub allowed_paths: Vec<String>,
    pub changed_paths: Vec<String>,
    pub tool_calls: u32,
    pub input_tokens: u64,
    pub maximum_input_tokens: u64,
    pub elapsed_seconds: u64,
    pub validations: Vec<String>,
    pub findings: Vec<String>,
    pub conflicts: Vec<String>,
    pub evidence_count: usize,
    pub blocked_by: Option<String>,
    pub repair_cycles: u8,
    pub summary: Option<String>,
    pub paused_reason: Option<String>,
}

impl DelegationRow {
    /// True when this worker's changes are waiting on a human.
    pub fn awaits_decision(&self) -> bool {
        matches!(self.integration_state.as_str(), "proposed" | "conflicted")
    }

    /// The one-line status the tree shows, in the same words the daemon uses.
    pub fn status_label(&self) -> String {
        if let Some(blocking) = &self.blocked_by {
            return format!("Blocked by {}", &blocking[..blocking.len().min(8)]);
        }
        if self.paused_reason.is_some() {
            return "Running (reconciling)".into();
        }
        match (self.status.as_str(), self.integration_state.as_str()) {
            (_, "conflicted") => "Conflict".into(),
            (_, "applied") => "Integrated".into(),
            (_, "rejected") => "Rejected".into(),
            (_, "proposed") => "Awaiting review".into(),
            ("running", _) => "Running".into(),
            ("ready", _) => "Ready".into(),
            ("planned", _) => "Planned".into(),
            ("completed", _) => "Completed".into(),
            ("failed", _) => "Failed".into(),
            ("cancelled", _) => "Cancelled".into(),
            (other, _) => other.to_owned(),
        }
    }

    /// Budget usage as a short `used/limit` string, or `—` when unknown.
    pub fn budget_label(&self) -> String {
        if self.maximum_input_tokens == 0 {
            return "—".into();
        }
        format!(
            "{}k/{}k tokens",
            self.input_tokens / 1_000,
            self.maximum_input_tokens / 1_000
        )
    }
}

/// The diff and evidence for one proposal (§PR12).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IntegrationReview {
    pub delegation_id: String,
    pub patch_digest: String,
    pub effective_patch_digest: String,
    pub base_drifted: bool,
    pub decision: String,
    pub patch: Option<String>,
    pub hunks: Vec<HunkRow>,
    /// Hunk indices the user selected. Empty means "the whole patch".
    pub selected: Vec<usize>,
    pub cursor: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HunkRow {
    pub index: usize,
    pub path: String,
    pub old_start: u32,
    pub old_lines: u32,
}

impl IntegrationReview {
    pub fn toggle_selected(&mut self) {
        let Some(hunk) = self.hunks.get(self.cursor) else {
            return;
        };
        match self.selected.iter().position(|index| *index == hunk.index) {
            Some(position) => {
                self.selected.remove(position);
            }
            None => self.selected.push(hunk.index),
        }
        self.selected.sort_unstable();
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    /// The label for the accept action, which has to be honest about what will
    /// land: a partial accept produces a different patch than the one reviewed.
    pub fn accept_label(&self) -> String {
        if self.selected.is_empty() {
            "Accept all".into()
        } else {
            format!("Accept {} selected hunk(s)", self.selected.len())
        }
    }
}

/// The panel state.
#[derive(Clone, Debug, Default)]
pub struct AgentWorkspace {
    pub session_id: String,
    pub classification: Option<String>,
    pub plan_reason: Option<String>,
    pub running: u32,
    pub awaiting_decision: u32,
    pub workers_started: u32,
    pub workers_completed: u32,
    pub total_input_tokens: u64,
    pub maximum_parallel_workers: u32,
    pub rows: Vec<DelegationRow>,
    pub selected: usize,
    pub review: Option<IntegrationReview>,
    pub loading: bool,
    pub error: Option<String>,
    /// Last action outcome, shown so a click that did something says so.
    pub notice: Option<String>,
}

impl AgentWorkspace {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            ..Self::default()
        }
    }

    pub fn selected_row(&self) -> Option<&DelegationRow> {
        self.rows.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = match delta {
            delta if delta < 0 => self.selected.saturating_sub(delta.unsigned_abs()),
            delta => (self.selected + delta as usize).min(last),
        };
    }

    /// Refresh the tree from the daemon.
    pub async fn load(&mut self, client: &reqwest::Client, daemon_url: &str, token: &str) {
        self.loading = true;
        let url = format!(
            "{}/v1/sessions/{}/delegations",
            daemon_url.trim_end_matches('/'),
            self.session_id
        );
        match client.get(&url).bearer_auth(token).send().await {
            Ok(response) => match response.json::<Value>().await {
                Ok(value) => {
                    self.apply(&value);
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            },
            Err(error) => self.error = Some(error.to_string()),
        }
        self.loading = false;
    }

    /// Open the integration review for the selected worker.
    pub async fn open_review(&mut self, client: &reqwest::Client, daemon_url: &str, token: &str) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let url = format!(
            "{}/v1/sessions/{}/delegations/{}",
            daemon_url.trim_end_matches('/'),
            self.session_id,
            row.delegation_id
        );
        match client.get(&url).bearer_auth(token).send().await {
            Ok(response) => match response.json::<Value>().await {
                Ok(value) => {
                    self.review = Some(parse_review(&value));
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            },
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Accept the open review — the whole patch, or the selected hunks.
    ///
    /// Returns whether the daemon accepted. The caller uses this to decide
    /// whether to refresh: a refusal means nothing changed, and refreshing
    /// would overwrite the refusal with a tree that looks fine.
    pub async fn accept(
        &mut self,
        client: &reqwest::Client,
        daemon_url: &str,
        token: &str,
    ) -> bool {
        let Some(review) = self.review.clone() else {
            return false;
        };
        let url = format!(
            "{}/v1/sessions/{}/delegations/{}/accept",
            daemon_url.trim_end_matches('/'),
            self.session_id,
            review.delegation_id
        );
        let body = serde_json::json!({ "hunks": review.selected });
        self.post(client, &url, token, body, "accepted").await
    }

    pub async fn reject(
        &mut self,
        client: &reqwest::Client,
        daemon_url: &str,
        token: &str,
        reason: &str,
    ) -> bool {
        let Some(row) = self.selected_row().cloned() else {
            return false;
        };
        let url = format!(
            "{}/v1/sessions/{}/delegations/{}/reject",
            daemon_url.trim_end_matches('/'),
            self.session_id,
            row.delegation_id
        );
        let body = serde_json::json!({ "reason": reason });
        self.post(client, &url, token, body, "rejected").await
    }

    pub async fn cancel(
        &mut self,
        client: &reqwest::Client,
        daemon_url: &str,
        token: &str,
        reason: &str,
    ) -> bool {
        let Some(row) = self.selected_row().cloned() else {
            return false;
        };
        let url = format!(
            "{}/v1/sessions/{}/delegations/{}/cancel",
            daemon_url.trim_end_matches('/'),
            self.session_id,
            row.delegation_id
        );
        let body = serde_json::json!({ "reason": reason });
        self.post(client, &url, token, body, "cancelled").await
    }

    /// POST one action to the daemon. Returns whether it was accepted.
    ///
    /// A refusal is kept on screen verbatim. Telling the user their patch
    /// landed when the daemon refused it — because an unresolved conflict, a
    /// drifted base or a scope violation — would be the worst bug this panel
    /// could have.
    async fn post(
        &mut self,
        client: &reqwest::Client,
        url: &str,
        token: &str,
        body: Value,
        verb: &str,
    ) -> bool {
        match client.post(url).bearer_auth(token).json(&body).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    self.notice = Some(format!("Worker changes {verb}."));
                    self.review = None;
                    self.error = None;
                    true
                } else {
                    let detail = response.text().await.unwrap_or_default();
                    self.error = Some(format!("The daemon refused: {status} {detail}"));
                    false
                }
            }
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        }
    }

    /// Apply a workspace payload. Public so the panel can be tested without a
    /// daemon, and so the parser has one home.
    pub fn apply(&mut self, value: &Value) {
        self.classification = value["classification"].as_str().map(str::to_owned);
        self.plan_reason = value["plan_reason"].as_str().map(str::to_owned);
        self.running = value["running"].as_u64().unwrap_or(0) as u32;
        self.awaiting_decision = value["awaiting_decision"].as_u64().unwrap_or(0) as u32;
        self.workers_started = value["workers_started"].as_u64().unwrap_or(0) as u32;
        self.workers_completed = value["workers_completed"].as_u64().unwrap_or(0) as u32;
        self.total_input_tokens = value["total_worker_usage"]["input_tokens"]
            .as_u64()
            .unwrap_or(0);
        self.maximum_parallel_workers = value["governance"]["maximum_parallel_workers"]
            .as_u64()
            .unwrap_or(0) as u32;
        self.rows = value["delegations"]
            .as_array()
            .map(|rows| rows.iter().map(parse_row).collect())
            .unwrap_or_default();
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_row(value: &Value) -> DelegationRow {
    DelegationRow {
        delegation_id: value["delegation_id"].as_str().unwrap_or_default().into(),
        objective: value["objective"].as_str().unwrap_or_default().into(),
        capability: value["capability"].as_str().unwrap_or_default().into(),
        status: value["status"].as_str().unwrap_or("unknown").into(),
        integration_state: value["integration_state"]
            .as_str()
            .unwrap_or("not_proposed")
            .into(),
        access: value["access"].as_str().unwrap_or("read_only").into(),
        agent_profile: value["agent_profile"].as_str().map(str::to_owned),
        model_role: value["model_role"].as_str().map(str::to_owned),
        routing_reason: value["routing_reason"].as_str().map(str::to_owned),
        allowed_paths: strings(&value["allowed_paths"]),
        changed_paths: strings(&value["changed_paths"]),
        tool_calls: value["tool_calls"].as_u64().unwrap_or(0) as u32,
        input_tokens: value["usage"]["input_tokens"].as_u64().unwrap_or(0),
        maximum_input_tokens: value["budget"]["maximum_input_tokens"]
            .as_u64()
            .unwrap_or(0),
        elapsed_seconds: value["usage"]["duration_seconds"].as_u64().unwrap_or(0),
        validations: value["validations"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{}: {}",
                            item["name"].as_str().unwrap_or("validation"),
                            item["status"].as_str().unwrap_or("unknown")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        findings: value["findings"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "[{}] {}{}",
                            item["severity"].as_str().unwrap_or("info"),
                            item["title"].as_str().unwrap_or_default(),
                            item["path"]
                                .as_str()
                                .map(|path| format!(" ({path})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        conflicts: value["conflicts"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{} — {}",
                            item["path"].as_str().unwrap_or_default(),
                            item["detail"].as_str().unwrap_or_default()
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        evidence_count: value["evidence_ids"].as_array().map(Vec::len).unwrap_or(0),
        blocked_by: value["blocked_by"].as_str().map(str::to_owned),
        repair_cycles: value["repair_cycles"].as_u64().unwrap_or(0) as u8,
        summary: value["summary"].as_str().map(str::to_owned),
        paused_reason: value["paused_reason"].as_str().map(str::to_owned),
    }
}

fn parse_review(value: &Value) -> IntegrationReview {
    IntegrationReview {
        delegation_id: value["delegation"]["delegation_id"]
            .as_str()
            .unwrap_or_default()
            .into(),
        patch_digest: value["patch_digest"].as_str().unwrap_or_default().into(),
        effective_patch_digest: value["effective_patch_digest"]
            .as_str()
            .unwrap_or_default()
            .into(),
        base_drifted: value["base_drifted"].as_bool().unwrap_or(false),
        decision: value["decision"].as_str().unwrap_or("unknown").into(),
        patch: value["patch"].as_str().map(str::to_owned),
        hunks: value["hunks"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| HunkRow {
                        index: item["index"].as_u64().unwrap_or(0) as usize,
                        path: item["path"].as_str().unwrap_or_default().into(),
                        old_start: item["old_start"].as_u64().unwrap_or(0) as u32,
                        old_lines: item["old_lines"].as_u64().unwrap_or(0) as u32,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        selected: Vec::new(),
        cursor: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Value {
        serde_json::json!({
            "session_id": "11111111-1111-1111-1111-111111111111",
            "objective": "add oauth",
            "classification": "parallel",
            "plan_reason": "3 independent components can run in isolated worktrees",
            "running": 1,
            "awaiting_decision": 1,
            "workers_started": 3,
            "workers_completed": 2,
            "total_worker_usage": { "input_tokens": 12_000 },
            "governance": { "maximum_parallel_workers": 3 },
            "delegations": [
                {
                    "delegation_id": "aaaaaaaa-0000-0000-0000-000000000001",
                    "objective": "implement the token exchange",
                    "capability": "implement_backend",
                    "status": "completed",
                    "integration_state": "proposed",
                    "access": "writable",
                    "agent_profile": "backend-specialist",
                    "model_role": "coding_worker",
                    "routing_reason": "highest-ranked Project provider",
                    "allowed_paths": ["src/auth/**"],
                    "changed_paths": ["src/auth/token.rs"],
                    "tool_calls": 6,
                    "usage": { "input_tokens": 8_000, "duration_seconds": 45 },
                    "budget": { "maximum_input_tokens": 120_000 },
                    "validations": [{ "name": "cargo test", "status": "passed" }],
                    "findings": [],
                    "conflicts": [],
                    "evidence_ids": ["e1", "e2"],
                    "blocked_by": null,
                    "repair_cycles": 0,
                    "summary": "implemented",
                    "paused_reason": null
                },
                {
                    "delegation_id": "aaaaaaaa-0000-0000-0000-000000000002",
                    "objective": "review the auth change",
                    "capability": "security_review",
                    "status": "blocked",
                    "integration_state": "not_proposed",
                    "access": "read_only",
                    "allowed_paths": ["src/**"],
                    "changed_paths": [],
                    "tool_calls": 0,
                    "usage": {},
                    "budget": { "maximum_input_tokens": 80_000 },
                    "validations": [],
                    "findings": [],
                    "conflicts": [],
                    "evidence_ids": [],
                    "blocked_by": "aaaaaaaa-0000-0000-0000-000000000003",
                    "repair_cycles": 0
                }
            ]
        })
    }

    #[test]
    fn the_tree_shows_what_the_daemon_reported() {
        let mut workspace = AgentWorkspace::new("11111111-1111-1111-1111-111111111111");
        workspace.apply(&payload());
        assert_eq!(workspace.rows.len(), 2);
        assert_eq!(workspace.classification.as_deref(), Some("parallel"));
        assert_eq!(workspace.running, 1);
        assert_eq!(workspace.maximum_parallel_workers, 3);

        let backend = &workspace.rows[0];
        assert_eq!(backend.status_label(), "Awaiting review");
        assert!(backend.awaits_decision());
        assert_eq!(backend.agent_profile.as_deref(), Some("backend-specialist"));
        assert_eq!(backend.budget_label(), "8k/120k tokens");
        assert_eq!(backend.validations, ["cargo test: passed"]);
        assert_eq!(backend.evidence_count, 2);

        // A blocked worker says what it is blocked on rather than looking idle.
        let reviewer = &workspace.rows[1];
        assert!(reviewer.status_label().starts_with("Blocked by"));
        assert!(!reviewer.awaits_decision());
    }

    #[test]
    fn selection_stays_inside_the_tree() {
        let mut workspace = AgentWorkspace::new("s");
        workspace.apply(&payload());
        workspace.move_selection(-1);
        assert_eq!(workspace.selected, 0);
        workspace.move_selection(5);
        assert_eq!(workspace.selected, 1);
        // A refresh that returns fewer rows must not leave the cursor past the
        // end — the next action would target nothing.
        workspace.apply(&serde_json::json!({ "delegations": [] }));
        assert_eq!(workspace.selected, 0);
        assert!(workspace.selected_row().is_none());
    }

    #[test]
    fn a_paused_worker_reads_as_reconciling_not_finished() {
        let mut workspace = AgentWorkspace::new("s");
        workspace.apply(&serde_json::json!({
            "delegations": [{
                "delegation_id": "d1",
                "status": "running",
                "integration_state": "not_proposed",
                "paused_reason": "daemon restarted"
            }]
        }));
        assert_eq!(workspace.rows[0].status_label(), "Running (reconciling)");
    }

    #[test]
    fn a_conflict_is_labelled_as_one() {
        let mut workspace = AgentWorkspace::new("s");
        workspace.apply(&serde_json::json!({
            "delegations": [{
                "delegation_id": "d1",
                "status": "completed",
                "integration_state": "conflicted",
                "conflicts": [{ "path": "src/lib.rs", "detail": "base lines 10-14 overlap" }]
            }]
        }));
        let row = &workspace.rows[0];
        assert_eq!(row.status_label(), "Conflict");
        assert_eq!(row.conflicts.len(), 1);
        assert!(row.conflicts[0].contains("overlap"));
    }

    #[test]
    fn hunk_selection_changes_what_accept_will_do() {
        let mut review = parse_review(&serde_json::json!({
            "delegation": { "delegation_id": "d1" },
            "patch_digest": "worker-digest",
            "effective_patch_digest": "worker-digest",
            "base_drifted": false,
            "decision": "ready_for_approval",
            "patch": "diff --git a/x b/x\n",
            "hunks": [
                { "index": 0, "path": "src/auth/token.rs", "old_start": 10, "old_lines": 5 },
                { "index": 1, "path": "src/auth/store.rs", "old_start": 1, "old_lines": 2 }
            ]
        }));
        assert_eq!(review.accept_label(), "Accept all");
        review.toggle_selected();
        assert!(review.is_selected(0));
        assert_eq!(review.accept_label(), "Accept 1 selected hunk(s)");
        review.cursor = 1;
        review.toggle_selected();
        assert_eq!(review.selected, [0, 1]);
        // Toggling off returns to the whole-patch default.
        review.toggle_selected();
        review.cursor = 0;
        review.toggle_selected();
        assert_eq!(review.accept_label(), "Accept all");
    }

    #[test]
    fn base_drift_is_surfaced_in_the_review() {
        let review = parse_review(&serde_json::json!({
            "delegation": { "delegation_id": "d1" },
            "base_drifted": true,
            "decision": "conflict",
            "hunks": []
        }));
        assert!(review.base_drifted);
        assert_eq!(review.decision, "conflict");
    }
}
