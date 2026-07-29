//! Bounded daemon SSE streaming for live assistant responses.
//!
//! High-frequency content deltas are deliberately separate from durable audit events. The latter
//! are collapsed into timeline cards by the conversation layer and are never interpreted as
//! assistant text.

use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const STREAM_CHANNEL_CAPACITY: usize = 64;
pub const MAX_CONTENT_DELTA_BYTES: usize = 256 * 1024;
pub const MAX_VISIBLE_CONTENT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PENDING_DELTA_BYTES: usize = 256 * 1024;
pub const MAX_SSE_FRAME_BYTES: usize = MAX_VISIBLE_CONTENT_BYTES + 16 * 1024;
pub const DELTA_BATCH_MIN: Duration = Duration::from_millis(16);
pub const DELTA_BATCH_TARGET: Duration = Duration::from_millis(24);
pub const DELTA_BATCH_MAX: Duration = Duration::from_millis(33);
pub const FIRST_TOKEN_STALL: Duration = Duration::from_secs(30);
const MAX_EVENTS_PER_TICK: usize = STREAM_CHANNEL_CAPACITY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamPhase {
    Queued,
    PreparingContext,
    SendingRequest,
    WaitingForFirstToken,
    Receiving,
    ParsingToolCall,
    Finalizing,
    Cancelled,
    Failed,
    Completed,
}

impl StreamPhase {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "preparing_context" => Some(Self::PreparingContext),
            "sending_request" => Some(Self::SendingRequest),
            "waiting_for_first_token" => Some(Self::WaitingForFirstToken),
            "receiving" => Some(Self::Receiving),
            "parsing_tool_call" => Some(Self::ParsingToolCall),
            "finalizing" => Some(Self::Finalizing),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::PreparingContext => "Preparing context",
            Self::SendingRequest => "Sending request",
            Self::WaitingForFirstToken => "Waiting for first token",
            Self::Receiving => "Receiving",
            Self::ParsingToolCall => "Parsing tool call",
            Self::Finalizing => "Finalizing",
            Self::Cancelled => "Cancelled",
            Self::Failed => "Failed",
            Self::Completed => "Completed",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StreamTiming {
    pub connected_ms: Option<u64>,
    pub first_byte_ms: Option<u64>,
    pub first_semantic_event_ms: Option<u64>,
    pub first_semantic_delta_ms: Option<u64>,
    pub last_delta_ms: Option<u64>,
    pub last_semantic_event_ms: Option<u64>,
    pub completion_ms: Option<u64>,
    pub terminal_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhaseUpdate {
    pub phase: StreamPhase,
    pub previous_phase: Option<StreamPhase>,
    pub sequence: Option<u64>,
    pub role: Option<String>,
    pub attempt: Option<u8>,
    pub request_index: Option<u64>,
    pub model: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub timing: StreamTiming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifiedStreamEnd {
    Completed,
    Failed,
    Cancelled,
    AwaitingApproval,
    AwaitingReview,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamEvent {
    Phase(PhaseUpdate),
    ContentDelta {
        delta: String,
        snapshot: bool,
        role: Option<String>,
        attempt: Option<u8>,
        request_index: Option<u64>,
    },
    DurableAudit {
        sequence: u64,
        event: Value,
    },
    Diagnostic(String),
    TransportError(String),
    TransportClosed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamOutput {
    PhaseChanged(PhaseUpdate),
    AttemptRestarted {
        role: Option<String>,
        attempt: Option<u8>,
    },
    Content {
        text: String,
        replace: bool,
        role: Option<String>,
        attempt: Option<u8>,
        request_index: Option<u64>,
    },
    DurableAudit {
        sequence: u64,
        event: Value,
    },
    Diagnostic(String),
    TransportError(String),
    VerifiedEnd(VerifiedStreamEnd),
}

#[derive(Debug)]
pub struct StreamController {
    pub active: bool,
    pub receiver: Option<mpsc::Receiver<StreamEvent>>,
    phase: StreamPhase,
    timing: StreamTiming,
    role: Option<String>,
    attempt: Option<u8>,
    model: Option<String>,
    batcher: DeltaBatcher,
    connected_at: Option<Instant>,
    first_semantic_delta_at: Option<Instant>,
    verified_end: Option<VerifiedStreamEnd>,
    request_index: Option<u64>,
    visible_content_bytes: usize,
}

impl StreamController {
    pub fn new() -> Self {
        Self {
            active: false,
            receiver: None,
            phase: StreamPhase::Queued,
            timing: StreamTiming::default(),
            role: None,
            attempt: None,
            model: None,
            batcher: DeltaBatcher::new(),
            connected_at: None,
            first_semantic_delta_at: None,
            verified_end: None,
            request_index: None,
            visible_content_bytes: 0,
        }
    }

    /// Starts observation immediately in the user-visible context preparation phase.
    pub fn start(&mut self, model: Option<String>, now: Instant) -> mpsc::Sender<StreamEvent> {
        let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        self.active = true;
        self.receiver = Some(receiver);
        self.phase = StreamPhase::PreparingContext;
        self.timing = StreamTiming::default();
        self.role = None;
        self.attempt = None;
        self.model = model;
        self.batcher = DeltaBatcher::new();
        self.connected_at = None;
        self.first_semantic_delta_at = None;
        self.verified_end = None;
        self.request_index = None;
        self.visible_content_bytes = 0;
        let _ = now;
        sender
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.receiver = None;
        self.batcher.clear();
    }

    pub fn finish_cancelled(&mut self) -> Option<String> {
        self.active = false;
        self.receiver = None;
        self.phase = StreamPhase::Cancelled;
        self.verified_end = Some(VerifiedStreamEnd::Cancelled);
        self.batcher.take().map(|batch| batch.text)
    }

    #[cfg(test)]
    pub fn phase(&self) -> StreamPhase {
        self.phase
    }

    #[cfg(test)]
    pub fn verified_end(&self) -> Option<VerifiedStreamEnd> {
        self.verified_end
    }

    pub fn actionable_stall(&self, now: Instant) -> Option<&'static str> {
        let connected_at = self.connected_at?;
        if self.active
            && self.phase == StreamPhase::WaitingForFirstToken
            && self.first_semantic_delta_at.is_none()
            && now.saturating_duration_since(connected_at) >= FIRST_TOKEN_STALL
        {
            Some(
                "Provider connected but produced no semantic output for 30s. Ctrl+C cancels and preserves partial output; check the selected model or provider.",
            )
        } else {
            None
        }
    }

    /// Drains at most one bounded channel's capacity and emits a display batch every 16–33ms.
    pub fn drain(&mut self, now: Instant) -> Vec<StreamOutput> {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(receiver) = &mut self.receiver {
            for _ in 0..MAX_EVENTS_PER_TICK {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        let mut output = Vec::new();
        for event in events {
            self.consume(event, now, &mut output);
        }
        if let Some(batch) = self.batcher.take_if_due(now) {
            output.push(batch.into_output(false));
        }
        if disconnected {
            self.receiver = None;
            if self.active && self.verified_end.is_none() {
                output.push(StreamOutput::TransportError(
                    "live stream closed before a verified daemon end state".into(),
                ));
            }
        }
        output
    }

    fn consume(&mut self, event: StreamEvent, now: Instant, output: &mut Vec<StreamOutput>) {
        match event {
            StreamEvent::Phase(update) => {
                let repair_attempt_started = update.attempt.is_some()
                    && self.attempt.is_some()
                    && update.attempt != self.attempt
                    && update.request_index.or(self.request_index) == self.request_index;
                if update.request_index != self.request_index {
                    if let Some(batch) = self.batcher.take() {
                        output.push(batch.into_output(false));
                    }
                    self.visible_content_bytes = 0;
                }
                if repair_attempt_started {
                    if let Some(batch) = self.batcher.take() {
                        output.push(batch.into_output(false));
                    }
                    self.batcher.clear();
                    self.visible_content_bytes = 0;
                    output.push(StreamOutput::AttemptRestarted {
                        role: update.role.clone(),
                        attempt: update.attempt,
                    });
                }
                self.apply_phase(&update, now);
                if matches!(
                    update.phase,
                    StreamPhase::Completed | StreamPhase::Cancelled | StreamPhase::Failed
                ) {
                    if let Some(batch) = self.batcher.take() {
                        output.push(batch.into_output(false));
                    }
                }
                output.push(StreamOutput::PhaseChanged(update));
            }
            StreamEvent::ContentDelta {
                delta,
                snapshot,
                role,
                attempt,
                request_index,
            } => {
                let request_index = request_index.or(self.request_index);
                let role = role.or_else(|| self.role.clone());
                let attempt = attempt.or(self.attempt);
                if request_index != self.request_index {
                    if let Some(batch) = self.batcher.take() {
                        output.push(batch.into_output(false));
                    }
                    self.request_index = request_index;
                    self.visible_content_bytes = 0;
                }
                if snapshot {
                    if self
                        .batcher
                        .context_matches(request_index, role.as_deref(), attempt)
                    {
                        self.batcher.clear();
                    } else if let Some(batch) = self.batcher.take() {
                        output.push(batch.into_output(false));
                    }
                    if delta.len() > MAX_VISIBLE_CONTENT_BYTES {
                        output.push(StreamOutput::TransportError(format!(
                            "content snapshot exceeded the {MAX_VISIBLE_CONTENT_BYTES}-byte limit"
                        )));
                        return;
                    }
                    self.visible_content_bytes = delta.len();
                    if !delta.trim().is_empty() {
                        self.first_semantic_delta_at.get_or_insert(now);
                    }
                    output.push(StreamOutput::Content {
                        text: delta,
                        replace: true,
                        role,
                        attempt,
                        request_index,
                    });
                    return;
                }
                let Some(total) = self.visible_content_bytes.checked_add(delta.len()) else {
                    output.push(StreamOutput::TransportError(
                        "visible content size overflowed".into(),
                    ));
                    return;
                };
                if total > MAX_VISIBLE_CONTENT_BYTES {
                    output.push(StreamOutput::TransportError(format!(
                        "visible content exceeded the {MAX_VISIBLE_CONTENT_BYTES}-byte limit"
                    )));
                    return;
                }
                self.visible_content_bytes = total;
                match self.batcher.push(&delta, now, request_index, role, attempt) {
                    Ok(Some(full_batch)) => {
                        output.push(full_batch.into_output(false));
                    }
                    Ok(None) => {}
                    Err(error) => output.push(StreamOutput::TransportError(error)),
                }
                if !delta.trim().is_empty() {
                    self.first_semantic_delta_at.get_or_insert(now);
                }
            }
            StreamEvent::DurableAudit { sequence, event } => {
                let verified_end = verified_end_from_audit(&event);
                output.push(StreamOutput::DurableAudit { sequence, event });
                if let Some(end) = verified_end {
                    if let Some(batch) = self.batcher.take() {
                        output.push(batch.into_output(false));
                    }
                    self.verified_end = Some(end);
                    self.active = false;
                    self.receiver = None;
                    self.phase = match end {
                        VerifiedStreamEnd::Completed => StreamPhase::Completed,
                        VerifiedStreamEnd::Failed => StreamPhase::Failed,
                        VerifiedStreamEnd::Cancelled => StreamPhase::Cancelled,
                        VerifiedStreamEnd::AwaitingApproval | VerifiedStreamEnd::AwaitingReview => {
                            StreamPhase::Finalizing
                        }
                    };
                    output.push(StreamOutput::VerifiedEnd(end));
                }
            }
            StreamEvent::Diagnostic(message) => output.push(StreamOutput::Diagnostic(message)),
            StreamEvent::TransportError(message) => {
                if let Some(batch) = self.batcher.take() {
                    output.push(batch.into_output(false));
                }
                output.push(StreamOutput::TransportError(message));
            }
            StreamEvent::TransportClosed => {
                self.receiver = None;
                if self.active && self.verified_end.is_none() {
                    output.push(StreamOutput::TransportError(
                        "live stream closed before a verified daemon end state".into(),
                    ));
                }
            }
        }
    }

    fn apply_phase(&mut self, update: &PhaseUpdate, now: Instant) {
        // PreparingContext is intentionally shown immediately at start. A late Queued observation
        // must not regress the visible state.
        if !(self.phase == StreamPhase::PreparingContext && update.phase == StreamPhase::Queued) {
            self.phase = update.phase;
        }
        self.timing = update.timing.clone();
        if let Some(role) = &update.role {
            self.role = Some(role.clone());
        }
        if let Some(attempt) = update.attempt {
            self.attempt = Some(attempt);
        }
        if let Some(request_index) = update.request_index {
            self.request_index = Some(request_index);
        }
        if let Some(model) = &update.model {
            self.model = Some(model.clone());
        }

        if let Some(connected_ms) = update.timing.connected_ms {
            self.connected_at.get_or_insert_with(|| {
                update
                    .elapsed_ms
                    .and_then(|elapsed| {
                        now.checked_sub(Duration::from_millis(elapsed.saturating_sub(connected_ms)))
                    })
                    .unwrap_or(now)
            });
        }
        if update.timing.first_semantic_delta_ms.is_some() {
            self.first_semantic_delta_at.get_or_insert(now);
        }
    }
}

#[derive(Debug)]
struct DeltaBatcher {
    pending: String,
    first_pending_at: Option<Instant>,
    role: Option<String>,
    attempt: Option<u8>,
    request_index: Option<u64>,
}

#[derive(Debug)]
struct BatchedContent {
    text: String,
    role: Option<String>,
    attempt: Option<u8>,
    request_index: Option<u64>,
}

impl BatchedContent {
    fn into_output(self, replace: bool) -> StreamOutput {
        StreamOutput::Content {
            text: self.text,
            replace,
            role: self.role,
            attempt: self.attempt,
            request_index: self.request_index,
        }
    }
}

impl DeltaBatcher {
    fn new() -> Self {
        Self {
            pending: String::new(),
            first_pending_at: None,
            role: None,
            attempt: None,
            request_index: None,
        }
    }

    fn push(
        &mut self,
        delta: &str,
        now: Instant,
        request_index: Option<u64>,
        role: Option<String>,
        attempt: Option<u8>,
    ) -> Result<Option<BatchedContent>, String> {
        if delta.len() > MAX_CONTENT_DELTA_BYTES {
            return Err(format!(
                "content delta exceeded the {MAX_CONTENT_DELTA_BYTES}-byte limit"
            ));
        }
        let context_changed = !self.context_matches(request_index, role.as_deref(), attempt)
            && !self.pending.is_empty();
        let Some(total) = self.pending.len().checked_add(delta.len()) else {
            return Err("content delta buffer size overflowed".into());
        };
        let full = if context_changed || total > MAX_PENDING_DELTA_BYTES {
            self.take()
        } else {
            None
        };
        if delta.len() > MAX_PENDING_DELTA_BYTES {
            return Err(format!(
                "content delta exceeded the {MAX_PENDING_DELTA_BYTES}-byte batch limit"
            ));
        }
        if self.pending.is_empty() {
            self.first_pending_at = Some(now);
            self.role = role;
            self.attempt = attempt;
            self.request_index = request_index;
        }
        self.pending.push_str(delta);
        Ok(full)
    }

    fn take_if_due(&mut self, now: Instant) -> Option<BatchedContent> {
        let elapsed = now.saturating_duration_since(self.first_pending_at?);
        if elapsed >= DELTA_BATCH_TARGET || elapsed >= DELTA_BATCH_MAX {
            debug_assert!(DELTA_BATCH_TARGET >= DELTA_BATCH_MIN);
            debug_assert!(DELTA_BATCH_TARGET <= DELTA_BATCH_MAX);
            self.take()
        } else {
            None
        }
    }

    fn take(&mut self) -> Option<BatchedContent> {
        self.first_pending_at = None;
        if self.pending.is_empty() {
            return None;
        }
        Some(BatchedContent {
            text: std::mem::take(&mut self.pending),
            role: self.role.take(),
            attempt: self.attempt.take(),
            request_index: self.request_index.take(),
        })
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.first_pending_at = None;
        self.role = None;
        self.attempt = None;
        self.request_index = None;
    }

    fn context_matches(
        &self,
        request_index: Option<u64>,
        role: Option<&str>,
        attempt: Option<u8>,
    ) -> bool {
        self.request_index == request_index
            && self.role.as_deref() == role
            && self.attempt == attempt
    }
}

fn verified_end_from_audit(event: &Value) -> Option<VerifiedStreamEnd> {
    if event.get("event").and_then(Value::as_str) == Some("judgment_recorded")
        && event
            .pointer("/data/decision/decision")
            .and_then(Value::as_str)
            == Some("require_approval")
    {
        return Some(VerifiedStreamEnd::AwaitingApproval);
    }
    match event.get("event").and_then(Value::as_str) {
        Some("session_completed") => Some(VerifiedStreamEnd::Completed),
        Some("session_failed") => Some(VerifiedStreamEnd::Failed),
        Some("session_cancelled") => Some(VerifiedStreamEnd::Cancelled),
        Some("session_paused") => Some(VerifiedStreamEnd::AwaitingReview),
        Some("approval_requested") => Some(VerifiedStreamEnd::AwaitingApproval),
        Some("outcome_review_required") => Some(VerifiedStreamEnd::AwaitingReview),
        _ => None,
    }
}

/// Incremental, bounded decoder for the daemon's three-kind SSE contract.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, String> {
        let Some(total) = self.buffer.len().checked_add(chunk.len()) else {
            return Err("SSE buffer size overflowed".into());
        };
        if total > MAX_SSE_FRAME_BYTES {
            self.buffer.clear();
            return Err(format!(
                "SSE frame exceeded the {MAX_SSE_FRAME_BYTES}-byte limit"
            ));
        }
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((end, delimiter_len)) = next_sse_block(&self.buffer) {
            let block = self.buffer[..end].to_vec();
            self.buffer.drain(..end + delimiter_len);
            if block.is_empty() {
                continue;
            }
            if let Some(event) = decode_sse_block(&block)? {
                events.push(event);
            }
        }
        Ok(events)
    }
}

fn next_sse_block(buffer: &[u8]) -> Option<(usize, usize)> {
    if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
        return Some((index, 4));
    }
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
}

fn decode_sse_block(block: &[u8]) -> Result<Option<StreamEvent>, String> {
    let block = std::str::from_utf8(block).map_err(|_| "SSE frame was not valid UTF-8")?;
    let mut data = String::new();
    let mut event_name = None;
    let mut event_id = None;
    for raw_line in block.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event_name = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("id:") {
            event_id = value.trim().parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_str(&data).map_err(|_| "SSE data was not valid bounded JSON")?;
    decode_wire_value(value, event_name.as_deref(), event_id).map(Some)
}

fn decode_wire_value(
    value: Value,
    event_name: Option<&str>,
    event_id: Option<u64>,
) -> Result<StreamEvent, String> {
    let kind = value
        .get("kind")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .or(event_name);
    let payload = value
        .get("data")
        .filter(|data| data.is_object())
        .unwrap_or(&value);
    match kind {
        Some("phase") => decode_phase(payload).map(StreamEvent::Phase),
        Some("content_delta") | Some("assistant_delta") => {
            let delta = payload
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| "content_delta omitted its delta string".to_owned())?;
            let snapshot = payload
                .get("snapshot")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let limit = if snapshot {
                MAX_VISIBLE_CONTENT_BYTES
            } else {
                MAX_CONTENT_DELTA_BYTES
            };
            if delta.len() > limit {
                return Err(format!("content delta exceeded the {limit}-byte limit"));
            }
            validate_display_text(delta)?;
            Ok(StreamEvent::ContentDelta {
                delta: delta.to_owned(),
                snapshot,
                role: bounded_string(payload.get("role"), 128),
                attempt: payload
                    .get("attempt")
                    .and_then(Value::as_u64)
                    .and_then(|attempt| u8::try_from(attempt).ok()),
                request_index: payload.get("request_index").and_then(Value::as_u64),
            })
        }
        Some("durable_audit") => {
            let event = payload
                .get("event")
                .filter(|event| event.is_object())
                .cloned()
                .ok_or_else(|| "durable_audit omitted its event object".to_owned())?;
            let sequence = payload
                .get("sequence")
                .and_then(Value::as_u64)
                .or(event_id)
                .ok_or_else(|| "durable_audit omitted its sequence".to_owned())?;
            Ok(StreamEvent::DurableAudit { sequence, event })
        }
        // Compatibility with the pre-v0.5 daemon. It is still audit data, never assistant text.
        None if value.get("event").and_then(Value::as_str).is_some() => {
            let sequence =
                event_id.ok_or_else(|| "legacy durable audit omitted its SSE id".to_owned())?;
            Ok(StreamEvent::DurableAudit {
                sequence,
                event: value,
            })
        }
        Some(unknown) => Ok(StreamEvent::Diagnostic(format!(
            "Unsupported live event `{}` was collapsed.",
            safe_kind(unknown)
        ))),
        None => Ok(StreamEvent::Diagnostic(
            "An untyped live event was collapsed.".into(),
        )),
    }
}

fn decode_phase(payload: &Value) -> Result<PhaseUpdate, String> {
    let phase_name = payload
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| "phase event omitted its phase".to_owned())?;
    let phase = StreamPhase::parse(phase_name).ok_or_else(|| {
        format!(
            "phase event used unsupported phase `{}`",
            safe_kind(phase_name)
        )
    })?;
    let timing = payload.get("timing").unwrap_or(&Value::Null);
    Ok(PhaseUpdate {
        phase,
        previous_phase: payload
            .get("previous_phase")
            .and_then(Value::as_str)
            .and_then(StreamPhase::parse),
        sequence: payload.get("sequence").and_then(Value::as_u64),
        role: bounded_string(payload.get("role"), 128),
        attempt: payload
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|attempt| u8::try_from(attempt).ok()),
        request_index: payload.get("request_index").and_then(Value::as_u64),
        model: bounded_string(payload.get("model"), 256),
        elapsed_ms: payload.get("elapsed_ms").and_then(Value::as_u64),
        timing: StreamTiming {
            connected_ms: optional_u64(timing, "connected_ms"),
            first_byte_ms: optional_u64(timing, "first_byte_ms"),
            first_semantic_event_ms: optional_u64(timing, "first_semantic_event_ms"),
            first_semantic_delta_ms: optional_u64(timing, "first_semantic_delta_ms"),
            last_delta_ms: optional_u64(timing, "last_delta_ms"),
            last_semantic_event_ms: optional_u64(timing, "last_semantic_event_ms"),
            completion_ms: optional_u64(timing, "completion_ms"),
            terminal_ms: optional_u64(timing, "terminal_ms"),
        },
    })
}

fn optional_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn bounded_string(value: Option<&Value>, maximum: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| value.len() <= maximum)
        .map(str::to_owned)
}

fn validate_display_text(value: &str) -> Result<(), String> {
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err("content delta contained an unsupported terminal control character".into());
    }
    Ok(())
}

fn safe_kind(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(48)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn phase(phase: StreamPhase, connected_ms: Option<u64>) -> PhaseUpdate {
        PhaseUpdate {
            phase,
            previous_phase: None,
            sequence: Some(1),
            role: Some("coding_worker".into()),
            attempt: Some(1),
            request_index: Some(1),
            model: Some("ollama/coder".into()),
            elapsed_ms: Some(10),
            timing: StreamTiming {
                connected_ms,
                ..StreamTiming::default()
            },
        }
    }

    fn delta(value: &str) -> StreamEvent {
        StreamEvent::ContentDelta {
            delta: value.into(),
            snapshot: false,
            role: Some("coding_worker".into()),
            attempt: Some(1),
            request_index: Some(1),
        }
    }

    #[test]
    fn starts_preparing_context_then_accepts_waiting_phase() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(Some("ollama/coder".into()), now);
        assert_eq!(controller.phase(), StreamPhase::PreparingContext);
        sender
            .try_send(StreamEvent::Phase(phase(
                StreamPhase::WaitingForFirstToken,
                Some(8),
            )))
            .unwrap();
        let output = controller.drain(now + Duration::from_millis(10));
        assert!(matches!(
            output.as_slice(),
            [StreamOutput::PhaseChanged(PhaseUpdate {
                phase: StreamPhase::WaitingForFirstToken,
                ..
            })]
        ));
        assert_eq!(controller.phase(), StreamPhase::WaitingForFirstToken);
    }

    #[test]
    fn batches_deltas_inside_the_frame_window_and_preserves_order() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(None, now);
        sender.try_send(delta("pur")).unwrap();
        sender.try_send(delta("r")).unwrap();
        assert!(controller
            .drain(now + DELTA_BATCH_MIN)
            .iter()
            .all(|event| !matches!(event, StreamOutput::Content { .. })));
        assert_eq!(
            controller.drain(now + DELTA_BATCH_MIN + DELTA_BATCH_TARGET),
            vec![StreamOutput::Content {
                text: "purr".into(),
                replace: false,
                role: Some("coding_worker".into()),
                attempt: Some(1),
                request_index: Some(1),
            }]
        );
    }

    #[test]
    fn stall_requires_a_connection_and_no_semantic_delta() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(None, now);
        assert!(controller
            .actionable_stall(now + FIRST_TOKEN_STALL)
            .is_none());
        sender
            .try_send(StreamEvent::Phase(phase(
                StreamPhase::WaitingForFirstToken,
                Some(10),
            )))
            .unwrap();
        controller.drain(now + Duration::from_millis(10));
        assert!(controller
            .actionable_stall(now + FIRST_TOKEN_STALL + Duration::from_millis(10))
            .is_some());
        sender.try_send(delta("semantic")).unwrap();
        controller.drain(now + FIRST_TOKEN_STALL + Duration::from_millis(11));
        assert!(controller
            .actionable_stall(now + FIRST_TOKEN_STALL * 2)
            .is_none());
    }

    #[test]
    fn cancellation_flushes_and_preserves_partial_batch() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(None, now);
        sender.try_send(delta("partial")).unwrap();
        controller.drain(now);
        assert_eq!(controller.finish_cancelled(), Some("partial".into()));
        assert!(!controller.active);
        assert_eq!(
            controller.verified_end(),
            Some(VerifiedStreamEnd::Cancelled)
        );
    }

    #[test]
    fn raw_durable_audit_is_never_decoded_as_content() {
        let event = json!({
            "event": "action_output_recorded",
            "data": {"stdout": "must remain collapsed"}
        });
        let wire = format!(
            "id: 7\nevent: durable_audit\ndata: {}\n\n",
            json!({"kind": "durable_audit", "sequence": 7, "event": event})
        );
        let decoded = SseDecoder::default().push(wire.as_bytes()).unwrap();
        assert_eq!(
            decoded,
            vec![StreamEvent::DurableAudit { sequence: 7, event }]
        );
        assert!(!decoded
            .iter()
            .any(|event| matches!(event, StreamEvent::ContentDelta { .. })));
    }

    #[test]
    fn unknown_kind_is_a_safe_collapsed_diagnostic() {
        let wire = "data: {\"kind\":\"future_secret_event\",\"raw\":\"do not echo\"}\n\n";
        let decoded = SseDecoder::default().push(wire.as_bytes()).unwrap();
        assert_eq!(
            decoded,
            vec![StreamEvent::Diagnostic(
                "Unsupported live event `future_secret_event` was collapsed.".into()
            )]
        );
        assert!(!format!("{decoded:?}").contains("do not echo"));
    }

    #[test]
    fn decoder_handles_split_utf8_and_rejects_oversized_delta() {
        let wire = "data: {\"kind\":\"content_delta\",\"delta\":\"猫\"}\n\n".as_bytes();
        let split = wire.iter().position(|byte| *byte >= 0x80).unwrap() + 1;
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(&wire[..split]).unwrap().is_empty());
        assert_eq!(
            decoder.push(&wire[split..]).unwrap(),
            vec![StreamEvent::ContentDelta {
                delta: "猫".into(),
                snapshot: false,
                role: None,
                attempt: None,
                request_index: None,
            }]
        );

        let oversized = "x".repeat(MAX_CONTENT_DELTA_BYTES + 1);
        let value = json!({"kind": "content_delta", "delta": oversized});
        let error = decode_wire_value(value, None, None).unwrap_err();
        assert!(error.contains("exceeded"));
    }

    #[test]
    fn snapshot_replaces_pending_same_request_and_is_deduplicatable() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(None, now);
        sender.try_send(delta("par")).unwrap();
        controller.drain(now);
        sender
            .try_send(StreamEvent::ContentDelta {
                delta: "partial".into(),
                snapshot: true,
                role: Some("coding_worker".into()),
                attempt: Some(1),
                request_index: Some(1),
            })
            .unwrap();
        assert_eq!(
            controller.drain(now + Duration::from_millis(1)),
            vec![StreamOutput::Content {
                text: "partial".into(),
                replace: true,
                role: Some("coding_worker".into()),
                attempt: Some(1),
                request_index: Some(1),
            }]
        );
    }

    #[test]
    fn repair_attempt_preserves_rejected_output_before_starting_a_new_message() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(Some("provider/model".into()), now);
        sender
            .try_send(StreamEvent::Phase(phase(StreamPhase::Receiving, Some(1))))
            .unwrap();
        sender.try_send(delta("rejected output")).unwrap();
        let _ = controller.drain(now + DELTA_BATCH_MAX);

        let mut repair = phase(StreamPhase::Queued, Some(1));
        repair.attempt = Some(2);
        sender.try_send(StreamEvent::Phase(repair)).unwrap();
        let output = controller.drain(now + DELTA_BATCH_MAX + Duration::from_millis(1));
        assert!(output.iter().any(|event| matches!(
            event,
            StreamOutput::AttemptRestarted {
                attempt: Some(2),
                ..
            }
        )));
        assert!(!output.iter().any(|event| matches!(
            event,
            StreamOutput::Content {
                replace: true,
                text,
                ..
            } if text.is_empty()
        )));
    }

    #[test]
    fn terminal_control_characters_are_rejected() {
        let value = json!({"kind": "content_delta", "delta": "safe\u{1b}[2J"});
        assert!(decode_wire_value(value, None, None)
            .unwrap_err()
            .contains("control character"));
    }

    #[test]
    fn only_durable_terminal_audit_finishes_the_operation() {
        let now = Instant::now();
        let mut controller = StreamController::new();
        let sender = controller.start(None, now);
        sender
            .try_send(StreamEvent::Phase(phase(StreamPhase::Completed, Some(1))))
            .unwrap();
        controller.drain(now);
        assert!(
            controller.active,
            "provider completion is not session completion"
        );

        sender
            .try_send(StreamEvent::DurableAudit {
                sequence: 2,
                event: json!({"event": "session_completed", "data": {}}),
            })
            .unwrap();
        let output = controller.drain(now);
        assert!(output.contains(&StreamOutput::VerifiedEnd(VerifiedStreamEnd::Completed)));
        assert!(!controller.active);
    }
}
