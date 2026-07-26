//! Structured, human-readable cards derived from durable runtime events.

use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CardKind {
    Conversation,
    Plan,
    Action,
    PawGate,
    Claw,
    Output,
    Validation,
    Checkpoint,
    Recovery,
    Completion,
    Skill,
    Context,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineCard {
    pub kind: CardKind,
    pub title: String,
    pub summary: String,
    pub details: Vec<String>,
}

impl TimelineCard {
    fn new(kind: CardKind, title: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            summary: summary.into(),
            details: Vec::new(),
        }
    }

    fn details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

pub fn cards_from_events(events: &[Value]) -> Vec<TimelineCard> {
    events.iter().filter_map(card_from_event).collect()
}

pub fn pending_action_from_events(events: &[Value]) -> Option<Value> {
    let mut pending: Option<(String, Value)> = None;
    for event in events {
        let name = event.get("event").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").unwrap_or(&Value::Null);
        if name == "action_proposed" {
            if let (Some(action_id), Some(action)) = (
                data.get("action_id").and_then(Value::as_str),
                data.get("action"),
            ) {
                pending = Some((action_id.to_owned(), action.clone()));
            }
        } else if matches!(
            name,
            "approval_rejected" | "authorization_persisted" | "execution_started"
        ) {
            let completed = data
                .get("action_id")
                .or_else(|| data.pointer("/authorization/action_id"))
                .and_then(Value::as_str);
            if pending
                .as_ref()
                .is_some_and(|(id, _)| Some(id.as_str()) == completed)
            {
                pending = None;
            }
        }
    }
    pending.map(|(_, action)| action)
}

pub fn action_summary(action: &Value) -> String {
    summarize_action(action)
}

fn card_from_event(value: &Value) -> Option<TimelineCard> {
    let event = value.get("event")?.as_str()?;
    let data = value.get("data").unwrap_or(&Value::Null);
    let text = |name: &str| data.get(name).and_then(Value::as_str).unwrap_or("");
    let id = || short_id(text("action_id"));
    let card = match event {
        "session_created" => {
            TimelineCard::new(CardKind::Context, "Session started", text("objective"))
                .details(vec![format!("Repository: {}", text("repository"))])
        }
        "conversation_message_added" => {
            let message = data.get("message").unwrap_or(&Value::Null);
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("system");
            let title = if role == "user" {
                "You"
            } else if role == "assistant" {
                "PurrCode"
            } else {
                "System"
            };
            TimelineCard::new(
                CardKind::Conversation,
                title,
                message.get("content").and_then(Value::as_str).unwrap_or(""),
            )
        }
        "plan_created" | "plan_revised" => {
            let steps = strings(data.get("steps"));
            let revision = data
                .get("revision")
                .and_then(Value::as_u64)
                .map(|n| format!(" revision {n}"))
                .unwrap_or_default();
            TimelineCard::new(
                CardKind::Plan,
                format!("Plan{revision}"),
                format!("{} step(s)", steps.len()),
            )
            .details(numbered(steps))
        }
        "action_proposed" => action_card(data),
        "judgment_recorded" => {
            let decision = data.get("decision").unwrap_or(&Value::Null);
            let name = decision
                .get("decision")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let reason = decision
                .pointer("/details/reason")
                .and_then(Value::as_str)
                .unwrap_or("");
            TimelineCard::new(
                CardKind::PawGate,
                "PawGate decision",
                format!("{} · action {}", humanize(name), id()),
            )
            .details(nonempty(vec![reason.to_owned()]))
        }
        "contextual_judgment_recorded" | "outcome_judgment_recorded" => TimelineCard::new(
            CardKind::PawGate,
            "PawGate review",
            "Contextual safety judgment recorded",
        ),
        "approval_recorded" => TimelineCard::new(
            CardKind::PawGate,
            "Approval recorded",
            format!("Exact action {} authorized", id()),
        )
        .details(vec![format!("Digest: {}", text("action_digest"))]),
        "approval_rejected" => TimelineCard::new(
            CardKind::PawGate,
            "Action rejected",
            format!("Action {} was denied", id()),
        )
        .details(nonempty(vec![text("reason").to_owned()])),
        "authorization_persisted" => TimelineCard::new(
            CardKind::PawGate,
            "Authorization persisted",
            "Claw may execute the exact authorized action",
        ),
        "execution_started" => {
            TimelineCard::new(CardKind::Claw, "Claw executing", format!("Action {}", id()))
        }
        "execution_finished" => TimelineCard::new(
            CardKind::Claw,
            "Claw finished",
            format!(
                "Action {} · exit {}",
                id(),
                data.get("exit_code")
                    .map(display_scalar)
                    .unwrap_or_else(|| "signal".into())
            ),
        )
        .details(nonempty(vec![
            optional_detail(data, "sandbox_backend", "Sandbox"),
            optional_detail(data, "sandbox_level", "Level"),
        ])),
        "action_output_recorded" => {
            let stdout = text("stdout");
            let stderr = text("stderr");
            TimelineCard::new(
                CardKind::Output,
                "Tool output",
                format!(
                    "Action {} · {} stdout / {} stderr chars",
                    id(),
                    stdout.chars().count(),
                    stderr.chars().count()
                ),
            )
            .details(nonempty(vec![
                preview("stdout", stdout),
                preview("stderr", stderr),
            ]))
        }
        "validation_recorded" => TimelineCard::new(
            CardKind::Validation,
            format!("Validation {}", humanize(text("status"))),
            text("evidence"),
        )
        .details(vec![format!("Action: {}", id())]),
        "checkpoint_created" => {
            TimelineCard::new(CardKind::Checkpoint, "Checkpoint created", text("label")).details(
                vec![
                    format!("Head: {}", text("head")),
                    format!("Patch digest: {}", text("patch_digest")),
                ],
            )
        }
        "worktree_disposition_recorded" => TimelineCard::new(
            CardKind::Checkpoint,
            "Worktree update",
            humanize(text("strategy")),
        )
        .details(nonempty(vec![text("detail").to_owned()])),
        "recovery_required" => {
            TimelineCard::new(CardKind::Recovery, "Recovery required", text("reason"))
        }
        "session_failed" => TimelineCard::new(CardKind::Recovery, "Session failed", text("reason")),
        "session_cancelled" => {
            TimelineCard::new(CardKind::Recovery, "Session cancelled", text("reason"))
        }
        "session_completed" => TimelineCard::new(
            CardKind::Completion,
            "Session completed",
            "All recorded work is complete",
        ),
        "context_compacted" => {
            TimelineCard::new(CardKind::Context, "Context compacted", text("summary"))
        }
        "context_indexed" => TimelineCard::new(
            CardKind::Context,
            "Repository indexed",
            format!(
                "{} files · {} symbols · {} sensitive paths",
                number(data, "files"),
                number(data, "symbols"),
                number(data, "sensitive_files")
            ),
        ),
        "model_request_started" => TimelineCard::new(
            CardKind::Context,
            "Model request",
            format!("{} · {}/{}", text("role"), text("provider"), text("model")),
        ),
        "model_request_finished" => TimelineCard::new(
            CardKind::Context,
            "Model response",
            format!(
                "{} · {} input / {} output tokens",
                text("role"),
                number(data, "input_tokens"),
                number(data, "output_tokens")
            ),
        ),
        name if name.starts_with("skill_")
            || name == "installed_skill_reused"
            || name == "installed_skill_matched"
            || name == "external_search_avoided"
            || name == "capability_gap_detected" =>
        {
            skill_card(name, data)
        }
        _ => return None,
    };
    Some(card)
}

fn action_card(data: &Value) -> TimelineCard {
    let action = data.get("action").unwrap_or(&Value::Null);
    let summary = summarize_action(action);
    TimelineCard::new(CardKind::Action, "Action proposed", summary).details(vec![
        format!(
            "Action: {}",
            short_id(data.get("action_id").and_then(Value::as_str).unwrap_or(""))
        ),
        "Ctrl+D opens the complete daemon-backed diff".into(),
    ])
}

fn summarize_action(action: &Value) -> String {
    let kind = action
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("action");
    match kind {
        "command" => format!(
            "{} {}",
            action
                .get("program")
                .and_then(Value::as_str)
                .unwrap_or("command"),
            strings(action.get("arguments")).join(" ")
        ),
        "write_file" => format!(
            "Write {}",
            action.get("path").and_then(Value::as_str).unwrap_or("file")
        ),
        "delete_file" => format!(
            "Delete {}",
            action.get("path").and_then(Value::as_str).unwrap_or("file")
        ),
        "external_tool" => format!(
            "{} / {}",
            action
                .get("server_id")
                .and_then(Value::as_str)
                .unwrap_or("server"),
            action
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
        ),
        _ => "Proposed runtime action".into(),
    }
}

fn skill_card(event: &str, data: &Value) -> TimelineCard {
    let skill = data
        .get("skill_id")
        .or_else(|| data.get("candidate_id"))
        .and_then(Value::as_str)
        .unwrap_or("skill");
    TimelineCard::new(CardKind::Skill, humanize(event), skill)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn numbered(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| format!("{}. {value}", i + 1))
        .collect()
}
fn nonempty(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}
fn number(data: &Value, key: &str) -> String {
    data.get(key)
        .map(display_scalar)
        .unwrap_or_else(|| "?".into())
}
fn display_scalar(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}
fn humanize(value: &str) -> String {
    let mut text = value.replace('_', " ");
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    text
}
fn optional_detail(data: &Value, key: &str, label: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .map(|v| format!("{label}: {v}"))
        .unwrap_or_default()
}
fn preview(label: &str, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let mut shown: String = value.chars().take(800).collect();
    if value.chars().count() > 800 {
        shown.push('…');
    }
    format!("{label}: {shown}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_plan_action_gate_execution_validation_and_completion() {
        let events = vec![
            json!({"event":"plan_created","data":{"steps":["Inspect", "Test"]}}),
            json!({"event":"action_proposed","data":{"action_id":"12345678-aaaa","action":{"type":"command","program":"cargo","arguments":["test"]}}}),
            json!({"event":"judgment_recorded","data":{"action_id":"12345678-aaaa","decision":{"decision":"require_approval","details":{"reason":"network","constraints":{}}}}}),
            json!({"event":"execution_finished","data":{"action_id":"12345678-aaaa","exit_code":0,"sandbox_backend":"seatbelt"}}),
            json!({"event":"validation_recorded","data":{"action_id":"12345678-aaaa","status":"passed","evidence":"all tests passed"}}),
            json!({"event":"session_completed"}),
        ];
        let cards = cards_from_events(&events);
        assert_eq!(cards.len(), 6);
        assert_eq!(cards[0].kind, CardKind::Plan);
        assert_eq!(cards[2].kind, CardKind::PawGate);
        assert!(cards[2].summary.contains("Require approval"));
        assert_eq!(cards[5].kind, CardKind::Completion);
    }

    #[test]
    fn tool_output_is_bounded_and_never_renders_json() {
        let output = "x".repeat(2_000);
        let cards = cards_from_events(&[
            json!({"event":"action_output_recorded","data":{"action_id":"a","stdout":output,"stderr":"","truncated":false}}),
        ]);
        assert!(cards[0].details[0].len() < 820);
        assert!(!cards[0].summary.contains("{\""));
    }

    #[test]
    fn approval_lifecycle_clears_only_the_matching_pending_action() {
        let events = vec![
            json!({"event":"action_proposed","data":{"action_id":"one","action":{"type":"write_file","path":"a.rs","content":"x"}}}),
            json!({"event":"approval_rejected","data":{"action_id":"other","reason":"no"}}),
        ];
        assert!(pending_action_from_events(&events).is_some());
        let mut resolved = events;
        resolved
            .push(json!({"event":"approval_rejected","data":{"action_id":"one","reason":"no"}}));
        assert!(pending_action_from_events(&resolved).is_none());
    }

    #[test]
    fn large_event_timeline_mapping_is_bounded_and_fast() {
        let events = (0..10_000).map(|index| serde_json::json!({"event":"validation_recorded","data":{"action_id":index.to_string(),"status":"passed","evidence":"ok"}})).collect::<Vec<_>>();
        let started = std::time::Instant::now();
        assert_eq!(cards_from_events(&events).len(), 10_000);
        assert!(started.elapsed() < std::time::Duration::from_millis(250));
    }
}
