//! Conversation state, message types, and event polling.

use crate::timeline::{cards_from_events, pending_action_from_events, TimelineCard};
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
    pub mode: ConversationMode,
    pub scroll: usize,
    pub evidence: Vec<String>,
    pub phase: String,
    pub timeline: Vec<TimelineCard>,
    pub selected_card: Option<usize>,
    pub expanded_card: Option<usize>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            streaming_message: None,
            pending_action: None,
            mode: ConversationMode::Build,
            scroll: 0,
            evidence: Vec::new(),
            phase: "ready".into(),
            timeline: Vec::new(),
            selected_card: None,
            expanded_card: None,
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
    }

    pub fn start_streaming(&mut self, model: Option<String>) {
        self.streaming_message = Some(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: "assistant".into(),
            content: String::new(),
            timestamp: Utc::now(),
            model,
        });
    }

    pub fn append_streaming(&mut self, delta: &str) {
        if let Some(msg) = &mut self.streaming_message {
            msg.content.push_str(delta);
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
        }
    }

    pub fn cancel_streaming(&mut self) {
        self.streaming_message = None;
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
        if let Ok(response) = reqwest::Client::new()
            .get(url)
            .bearer_auth(token)
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(messages) = response.json::<Vec<Message>>().await {
                    self.messages = messages;
                }
            }
        }
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
                self.phase = events
                    .iter()
                    .rev()
                    .find_map(|event| event["event"].as_str())
                    .map(runtime_phase)
                    .unwrap_or("ready")
                    .to_owned();
                self.pending_action = pending_action_from_events(&events);
                self.evidence = events
                    .iter()
                    .filter_map(|event| match event["event"].as_str() {
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
                    })
                    .rev()
                    .take(3)
                    .collect();
                self.timeline = cards_from_events(&events);
                self.selected_card = self
                    .selected_card
                    .map(|index| index.min(self.timeline.len().saturating_sub(1)));
            }
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
