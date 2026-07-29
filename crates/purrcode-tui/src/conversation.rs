//! Conversation state, message types, and event polling.

use crate::timeline::{
    cards_from_events, collapsed_card_from_event, pending_action_from_events, TimelineCard,
};
use chrono::{DateTime, Utc};
use purrcode_runtime_core::ConversationMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub model: Option<String>,
}

pub struct Conversation {
    pub messages: Vec<Message>,
    pub streaming_message: Option<Message>,
    pub pending_action: Option<Value>,
    pending_action_candidate: Option<(String, Value)>,
    pub mode: ConversationMode,
    pub scroll: usize,
    pub auto_follow: bool,
    pub new_output: bool,
    pub evidence: Vec<String>,
    pub phase: String,
    pub timeline: Vec<TimelineCard>,
    pub selected_card: Option<usize>,
    pub expanded_card: Option<usize>,
    last_durable_sequence: u64,
}

impl Conversation {
    pub fn last_durable_sequence(&self) -> u64 {
        self.last_durable_sequence
    }

    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            streaming_message: None,
            pending_action: None,
            pending_action_candidate: None,
            mode: ConversationMode::Build,
            scroll: 0,
            auto_follow: true,
            new_output: false,
            evidence: Vec::new(),
            phase: "ready".into(),
            timeline: Vec::new(),
            selected_card: None,
            expanded_card: None,
            last_durable_sequence: 0,
        }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: "user".into(),
            content: content.to_string(),
            timestamp: Utc::now(),
            model: None,
        });
        self.note_new_output();
    }

    pub fn start_streaming(&mut self, model: Option<String>) {
        self.finalize_streaming();
        self.streaming_message = Some(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: "assistant".into(),
            content: String::new(),
            timestamp: Utc::now(),
            model,
        });
        self.note_new_output();
    }

    pub fn append_streaming(&mut self, delta: &str) {
        if let Some(msg) = &mut self.streaming_message {
            msg.content.push_str(delta);
            self.note_new_output();
        }
    }

    pub fn replace_streaming(&mut self, content: &str) {
        if let Some(message) = &mut self.streaming_message {
            message.content.clear();
            message.content.push_str(content);
            self.note_new_output();
        }
    }

    pub fn finalize_streaming(&mut self) {
        if let Some(msg) = self.streaming_message.take() {
            if !msg.content.is_empty()
                && !self
                    .messages
                    .iter()
                    .any(|existing| existing.role == msg.role && existing.content == msg.content)
            {
                self.messages.push(msg);
            }
            self.note_new_output();
        }
    }

    /// Preserve partial assistant output when a live request is cancelled or interrupted.
    pub fn cancel_streaming(&mut self) {
        self.finalize_streaming();
    }

    pub fn apply_durable_audit(&mut self, sequence: u64, event: Value) {
        if sequence <= self.last_durable_sequence {
            return;
        }
        self.last_durable_sequence = sequence;
        let name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.phase = runtime_phase(name).to_owned();

        if name == "action_proposed" {
            if let (Some(action_id), Some(action)) = (
                event.pointer("/data/action_id").and_then(Value::as_str),
                event.pointer("/data/action"),
            ) {
                self.pending_action_candidate = Some((action_id.to_owned(), action.clone()));
            }
        } else if name == "judgment_recorded" {
            let action_id = event.pointer("/data/action_id").and_then(Value::as_str);
            let decision = event
                .pointer("/data/decision/decision")
                .and_then(Value::as_str)
                .unwrap_or("");
            if self
                .pending_action_candidate
                .as_ref()
                .is_some_and(|(id, _)| Some(id.as_str()) == action_id)
            {
                if decision == "require_approval" {
                    self.pending_action = self
                        .pending_action_candidate
                        .as_ref()
                        .map(|(_, action)| action.clone());
                } else if matches!(decision, "deny" | "modify_action" | "replan") {
                    self.pending_action_candidate = None;
                    self.pending_action = None;
                }
            }
        } else if matches!(
            name,
            "approval_rejected"
                | "authorization_persisted"
                | "execution_started"
                | "session_completed"
                | "session_failed"
                | "session_cancelled"
        ) {
            self.pending_action = None;
            self.pending_action_candidate = None;
        }

        if let Some(evidence) = collapsed_evidence(&event) {
            self.evidence.insert(0, evidence);
            self.evidence.truncate(3);
        }

        let is_streamed_assistant_snapshot = name == "conversation_message_added"
            && event.pointer("/data/message/role").and_then(Value::as_str) == Some("assistant")
            && self.streaming_message.is_some();
        if is_streamed_assistant_snapshot {
            if let Some(content) = event
                .pointer("/data/message/content")
                .and_then(Value::as_str)
                .filter(|content| content.contains("\n\nProposed plan:\n"))
            {
                self.replace_streaming(content);
            }
        }
        if !is_streamed_assistant_snapshot {
            let card = collapsed_card_from_event(&event);
            let expand_plan = card.kind == crate::timeline::CardKind::Plan;
            self.timeline.push(card);
            if expand_plan {
                let index = self.timeline.len() - 1;
                self.selected_card = Some(index);
                self.expanded_card = Some(index);
            }
        }
        self.note_new_output();
    }

    pub fn user_scroll_up(&mut self, amount: usize) {
        if self.auto_follow {
            self.scroll = self.display_item_count().saturating_sub(amount.max(1) + 1);
        } else {
            self.scroll = self.scroll.saturating_sub(amount);
        }
        self.auto_follow = false;
    }

    pub fn user_scroll_down(&mut self, amount: usize) {
        let latest = self.display_item_count().saturating_sub(1);
        self.scroll = self.scroll.saturating_add(amount).min(latest);
        if self.scroll >= latest {
            self.resume_auto_follow();
        }
    }

    pub fn resume_auto_follow(&mut self) {
        self.auto_follow = true;
        self.new_output = false;
    }

    pub fn display_item_count(&self) -> usize {
        let base = if self.timeline.is_empty() {
            self.messages.len()
        } else {
            self.timeline.len()
        };
        base + usize::from(self.streaming_message.is_some())
    }

    fn note_new_output(&mut self) {
        if self.auto_follow {
            // Keep enough recent cards visible to preserve context while ensuring the
            // final response and terminal event cannot land below the viewport.
            const RECENT_CARD_WINDOW: usize = 8;
            self.scroll = self.display_item_count().saturating_sub(RECENT_CARD_WINDOW);
            self.new_output = false;
        } else {
            self.new_output = true;
        }
    }

    pub async fn refresh_events(
        &mut self,
        daemon_url: &str,
        token: &str,
        session_id: Option<String>,
    ) {
        let Some(session_id) = session_id else {
            return;
        };
        let url = format!(
            "{}/v1/sessions/{session_id}/messages",
            daemon_url.trim_end_matches('/')
        );
        let messages = if let Ok(response) = reqwest::Client::new()
            .get(url)
            .bearer_auth(token)
            .send()
            .await
        {
            if response.status().is_success() {
                response.json::<Vec<Message>>().await.ok()
            } else {
                None
            }
        } else {
            None
        };
        let events_url = format!(
            "{}/v1/sessions/{session_id}/events",
            daemon_url.trim_end_matches('/')
        );
        if let Ok(response) = reqwest::Client::new()
            .get(events_url)
            .bearer_auth(token)
            .send()
            .await
        {
            if let Ok(events) = response.json::<Vec<Value>>().await {
                self.reconcile(messages, events);
            }
        }
    }

    pub fn reconcile(&mut self, messages: Option<Vec<Message>>, events: Vec<Value>) {
        if let Some(messages) = messages {
            self.messages = messages;
        }
        self.phase = events
            .iter()
            .rev()
            .find_map(|event| event["event"].as_str())
            .map(runtime_phase)
            .unwrap_or("ready")
            .to_owned();
        self.pending_action = pending_action_from_events(&events);
        self.pending_action_candidate = None;
        self.evidence = events
            .iter()
            .filter_map(collapsed_evidence)
            .rev()
            .take(3)
            .collect();
        self.timeline = cards_from_events(&events);
        self.last_durable_sequence = events.len() as u64;
        if self.expanded_card.is_none() {
            if let Some(index) = self
                .timeline
                .iter()
                .rposition(|card| card.kind == crate::timeline::CardKind::Plan)
            {
                self.selected_card = Some(index);
                self.expanded_card = Some(index);
            }
        }
        self.selected_card = self
            .selected_card
            .map(|index| index.min(self.timeline.len().saturating_sub(1)));
        if self.auto_follow {
            self.new_output = false;
        }
    }

    pub fn select_card(&mut self, delta: isize) {
        if self.timeline.is_empty() {
            return;
        }
        let current = self.selected_card.unwrap_or(self.timeline.len() - 1) as isize;
        self.selected_card =
            Some((current + delta).clamp(0, self.timeline.len() as isize - 1) as usize);
    }

    pub fn toggle_selected_card(&mut self) {
        let Some(selected) = self.selected_card else {
            return;
        };
        self.expanded_card = (self.expanded_card != Some(selected)).then_some(selected);
    }

    pub fn current_objective(&self) -> String {
        self.messages
            .first()
            .map(|message| message.content.clone())
            .unwrap_or_default()
    }

    pub fn latest_user_message(&self) -> String {
        self.messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .unwrap_or_default()
    }
}

fn collapsed_evidence(event: &Value) -> Option<String> {
    match event.get("event").and_then(Value::as_str) {
        Some("judgment_recorded") => Some(format!(
            "Judgment: {}",
            event.pointer("/data/decision").unwrap_or(&Value::Null)
        )),
        Some("validation_recorded") => Some(format!(
            "Validation: {} — {}",
            event
                .pointer("/data/status")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            event
                .pointer("/data/evidence")
                .and_then(Value::as_str)
                .unwrap_or("")
        )),
        _ => None,
    }
}

fn runtime_phase(event: &str) -> &'static str {
    match event {
        "provider_request_started" => "thinking",
        "context_retrieved" => "retrieving",
        "action_proposed" => "proposing",
        "approval_requested" | "outcome_review_required" => "awaiting approval",
        "execution_started" => "executing",
        "validation_started" | "validation_recorded" => "validating",
        "session_completed" => "completed",
        "session_failed" => "failed",
        "session_cancelled" => "cancelled",
        "recovery_required" => "recovery required",
        _ => "active",
    }
}

#[cfg(test)]
mod tests {
    use super::Conversation;
    use serde_json::json;

    #[test]
    fn scroll_up_pauses_follow_and_new_output_sets_indicator() {
        let mut conversation = Conversation::new();
        for index in 0..4 {
            conversation.add_user_message(&format!("message {index}"));
        }
        conversation.user_scroll_up(2);
        assert!(!conversation.auto_follow);
        assert!(!conversation.new_output);
        conversation.start_streaming(Some("model".into()));
        conversation.append_streaming("new");
        assert!(conversation.new_output);
        conversation.user_scroll_down(usize::MAX);
        assert!(conversation.auto_follow);
        assert!(!conversation.new_output);
    }

    #[test]
    fn cancellation_keeps_partial_assistant_output() {
        let mut conversation = Conversation::new();
        conversation.start_streaming(Some("model".into()));
        conversation.append_streaming("partial answer");
        conversation.cancel_streaming();
        assert!(conversation.streaming_message.is_none());
        assert_eq!(
            conversation
                .messages
                .last()
                .map(|message| message.content.as_str()),
            Some("partial answer")
        );
    }

    #[test]
    fn durable_audit_is_collapsed_and_not_used_as_stream_text() {
        let mut conversation = Conversation::new();
        conversation.start_streaming(None);
        conversation.append_streaming("live");
        conversation.apply_durable_audit(
            1,
            json!({
                "event": "conversation_message_added",
                "data": {
                    "message": {
                        "role": "assistant",
                        "content": "durable snapshot must not replace live text"
                    }
                }
            }),
        );
        assert_eq!(
            conversation
                .streaming_message
                .as_ref()
                .map(|message| message.content.as_str()),
            Some("live")
        );
        assert!(conversation.timeline.is_empty());

        conversation.apply_durable_audit(
            2,
            json!({
                "event": "future_runtime_event",
                "data": {"secret": "must not render"}
            }),
        );
        let card = conversation.timeline.last().unwrap();
        assert_eq!(card.title, "Runtime audit");
        assert!(!card.summary.contains("must not render"));
    }

    #[test]
    fn durable_advice_completion_replaces_temporary_rationale_with_the_plan() {
        let mut conversation = Conversation::new();
        conversation.start_streaming(None);
        conversation.append_streaming("I have enough information");
        conversation.apply_durable_audit(
            1,
            json!({
                "event": "conversation_message_added",
                "data": {"message": {
                    "role": "assistant",
                    "content": "Repository review complete.\n\nProposed plan:\n1. Split the daemon router\n2. Add regression tests"
                }}
            }),
        );
        let content = &conversation.streaming_message.as_ref().unwrap().content;
        assert!(content.contains("1. Split the daemon router"));
        assert!(!content.contains("enough information"));
    }

    #[test]
    fn auto_follow_keeps_the_latest_terminal_cards_in_view() {
        let mut conversation = Conversation::new();
        for sequence in 1..=12 {
            conversation.apply_durable_audit(
                sequence,
                json!({
                    "event": "context_indexed",
                    "data": {"files": sequence, "symbols": 0, "sensitive_files": 0}
                }),
            );
        }
        assert!(conversation.auto_follow);
        assert_eq!(conversation.scroll, 4);
        assert!(!conversation.new_output);
    }
}
