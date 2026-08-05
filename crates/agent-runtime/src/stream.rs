//! Streaming observer and rationale extraction for live clients.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use thiserror::Error;
use tokio::sync::mpsc;

use purrcode_provider_gateway::{MAX_PROVIDER_STREAM_FRAME_BYTES, StreamPhase, StreamTiming};

/// At the provider frame limit, the largest permitted queue retains at most 16 MiB of deltas.
pub const MAX_STREAM_OBSERVER_CAPACITY: usize = 64;

/// Terminal lifecycle updates use a separate bounded lane. A single agent
/// operation can make at most two structured calls per autonomous iteration
/// (the initial response and one repair), and the runtime caps iterations at
/// 32. Keeping a larger fixed terminal lane makes that upper bound explicit
/// without allowing an unbounded observer backlog.
const MAX_TERMINAL_OBSERVER_CAPACITY: usize = 128;

pub(crate) const MAX_STREAMED_RATIONALE_BYTES: usize = MAX_PROVIDER_STREAM_FRAME_BYTES;
const MAX_STREAM_JSON_KEY_CHARS: usize = 256;
const MAX_STREAM_JSON_NESTING: usize = 128;

/// Ephemeral provider observations for live clients.
///
/// These events are intentionally not [`purrcode_runtime_core::SessionEvent`] values. Durable request/audit
/// events remain authoritative in [`purrcode_ninelives::SessionStore`], while this channel carries
/// high-frequency UI observations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    Phase {
        role: String,
        attempt: u8,
        sequence: u64,
        previous_phase: StreamPhase,
        phase: StreamPhase,
        timing: StreamTiming,
    },
    ContentDelta {
        role: String,
        attempt: u8,
        delta: String,
    },
}

/// Cloneable sending side of a bounded observer channel.
///
/// Normal observations are best-effort and may be dropped when the receiver
/// is slow. Terminal lifecycle updates use a separate bounded lane so a full
/// normal queue can never erase `Completed`, `Failed`, or `Cancelled`.
#[derive(Clone, Debug)]
pub struct AgentStreamObserver {
    normal_sender: mpsc::Sender<AgentStreamEvent>,
    terminal_sender: mpsc::Sender<AgentStreamEvent>,
    normal_slots: Arc<NormalSlotCounter>,
}

/// Receiving side of a bounded observer channel.
///
/// The receiver keeps the normal queue's slot counter in sync, allowing a
/// producer to keep accepting normal updates after a client drains a full
/// queue. Terminal updates are checked after already-queued normal updates so
/// content remains ordered ahead of the lifecycle event that closes a turn.
#[derive(Debug)]
pub struct AgentStreamReceiver {
    normal_receiver: mpsc::Receiver<AgentStreamEvent>,
    terminal_receiver: mpsc::Receiver<AgentStreamEvent>,
    normal_slots: Arc<NormalSlotCounter>,
    normal_closed: bool,
    terminal_closed: bool,
}

#[derive(Debug)]
struct NormalSlotCounter {
    capacity: usize,
    in_flight: std::sync::atomic::AtomicUsize,
}

impl NormalSlotCounter {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            in_flight: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn reserve(&self) -> bool {
        self.in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.capacity).then_some(count + 1)
            })
            .is_ok()
    }

    fn release(&self) {
        let previous = self.in_flight.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "normal observer slot released without a send");
    }
}

impl AgentStreamObserver {
    /// Best-effort send that never blocks the agent loop.
    ///
    /// A slow or aborted live client drops observations rather than stalling
    /// the loop with the lease held; the durable session events stay
    /// authoritative either way.
    pub(crate) fn try_send(&self, event: AgentStreamEvent) -> bool {
        // Normal updates are best effort; terminal updates have a dedicated
        // bounded lane below.
        let terminal = matches!(
            &event,
            AgentStreamEvent::Phase { phase, .. } if phase.is_terminal()
        );
        if terminal {
            // The terminal lane is deliberately larger than the maximum
            // number of provider attempts in one operation. Sending remains
            // non-blocking; unlike normal updates, terminal updates never
            // compete with a full content queue.
            return self.terminal_sender.try_send(event).is_ok();
        }
        if !self.normal_slots.reserve() {
            return false;
        }
        match self.normal_sender.try_send(event) {
            Ok(()) => true,
            Err(_) => {
                self.normal_slots.release();
                false
            }
        }
    }
}

impl AgentStreamReceiver {
    fn normal_received(&self) {
        self.normal_slots.release();
    }

    /// Receive the next observation, prioritising already queued normal
    /// content before a terminal update. This preserves the rationale/content
    /// ordering while still making terminal delivery independent of normal
    /// queue pressure.
    pub async fn recv(&mut self) -> Option<AgentStreamEvent> {
        loop {
            if let Ok(event) = self.normal_receiver.try_recv() {
                self.normal_received();
                return Some(event);
            }
            if let Ok(event) = self.terminal_receiver.try_recv() {
                return Some(event);
            }

            if self.normal_closed && self.terminal_closed {
                return None;
            }
            tokio::select! {
                biased;
                event = self.normal_receiver.recv(), if !self.normal_closed => {
                    match event {
                        Some(event) => {
                            self.normal_received();
                            return Some(event);
                        }
                        None => self.normal_closed = true,
                    }
                }
                event = self.terminal_receiver.recv(), if !self.terminal_closed => {
                    match event {
                        Some(event) => return Some(event),
                        None => self.terminal_closed = true,
                    }
                }
            }
        }
    }

    /// Non-blocking counterpart used by tests and callers draining a known
    /// batch. It reports `Disconnected` only after both bounded lanes close.
    pub fn try_recv(&mut self) -> Result<AgentStreamEvent, mpsc::error::TryRecvError> {
        match self.normal_receiver.try_recv() {
            Ok(event) => {
                self.normal_received();
                Ok(event)
            }
            Err(mpsc::error::TryRecvError::Empty) => match self.terminal_receiver.try_recv() {
                Ok(event) => Ok(event),
                Err(mpsc::error::TryRecvError::Empty) => Err(mpsc::error::TryRecvError::Empty),
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.terminal_closed = true;
                    if self.normal_closed {
                        Err(mpsc::error::TryRecvError::Disconnected)
                    } else {
                        Err(mpsc::error::TryRecvError::Empty)
                    }
                }
            },
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.normal_closed = true;
                match self.terminal_receiver.try_recv() {
                    Ok(event) => Ok(event),
                    Err(mpsc::error::TryRecvError::Empty) => Err(mpsc::error::TryRecvError::Empty),
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        self.terminal_closed = true;
                        Err(mpsc::error::TryRecvError::Disconnected)
                    }
                }
            }
        }
    }
}

pub fn bounded_agent_stream_channel(
    capacity: usize,
) -> Result<(AgentStreamObserver, AgentStreamReceiver), AgentStreamObserverError> {
    if capacity == 0 || capacity > MAX_STREAM_OBSERVER_CAPACITY {
        return Err(AgentStreamObserverError::InvalidCapacity {
            requested: capacity,
            maximum: MAX_STREAM_OBSERVER_CAPACITY,
        });
    }
    let (normal_sender, normal_receiver) = mpsc::channel(capacity);
    let (terminal_sender, terminal_receiver) = mpsc::channel(MAX_TERMINAL_OBSERVER_CAPACITY);
    let normal_slots = Arc::new(NormalSlotCounter::new(capacity));
    let receiver = AgentStreamReceiver {
        normal_receiver,
        terminal_receiver,
        normal_slots: Arc::clone(&normal_slots),
        normal_closed: false,
        terminal_closed: false,
    };
    Ok((
        AgentStreamObserver {
            normal_sender,
            terminal_sender,
            normal_slots,
        },
        receiver,
    ))
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentStreamObserverError {
    #[error("observer capacity {requested} must be between 1 and {maximum}")]
    InvalidCapacity { requested: usize, maximum: usize },
}

pub(crate) fn is_unsafe_terminal_control(character: char) -> bool {
    matches!(
        character,
        '\u{0000}'..='\u{0008}'
            | '\u{000b}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
    )
}

#[derive(Debug)]
pub(crate) struct RationaleStreamExtractor {
    pub(crate) state: TopLevelJsonState,
    pub(crate) active_string: Option<ActiveJsonString>,
    pub(crate) rationale_seen: bool,
    pub(crate) rationale_finished: bool,
    pub(crate) decoded_rationale: String,
    pub(crate) emitted_bytes: usize,
}

impl Default for RationaleStreamExtractor {
    fn default() -> Self {
        Self {
            state: TopLevelJsonState::BeforeObject,
            active_string: None,
            rationale_seen: false,
            rationale_finished: false,
            decoded_rationale: String::new(),
            emitted_bytes: 0,
        }
    }
}

impl RationaleStreamExtractor {
    /// Extracts only the decoded top-level `rationale` JSON string.
    ///
    /// Any malformed, excessively nested, or oversized input disables extraction. The structured
    /// response still goes through the authoritative bounded serde parser; this preview parser can
    /// never expose raw JSON or another field.
    pub(crate) fn push(&mut self, input: &str) -> Option<String> {
        if matches!(self.state, TopLevelJsonState::Disabled) {
            return None;
        }
        let mut visible = String::new();
        for character in input.chars() {
            if let Some(mut active) = self.active_string.take() {
                match active.decoder.push(character) {
                    Ok(JsonStringProgress::Decoded(decoded)) => {
                        match &mut active.role {
                            JsonStringRole::Key { value, overflow } => {
                                if !*overflow {
                                    if value.chars().count() == MAX_STREAM_JSON_KEY_CHARS {
                                        *overflow = true;
                                        value.clear();
                                    } else {
                                        value.push(decoded);
                                    }
                                }
                            }
                            JsonStringRole::Rationale => {
                                if is_unsafe_terminal_control(decoded) {
                                    self.disable();
                                    break;
                                }
                                let Some(total) =
                                    self.emitted_bytes.checked_add(decoded.len_utf8())
                                else {
                                    self.disable();
                                    break;
                                };
                                if total > MAX_STREAMED_RATIONALE_BYTES {
                                    self.disable();
                                    break;
                                }
                                self.emitted_bytes = total;
                                self.decoded_rationale.push(decoded);
                                visible.push(decoded);
                            }
                            JsonStringRole::SkippedValue | JsonStringRole::Nested => {}
                        }
                        self.active_string = Some(active);
                    }
                    Ok(JsonStringProgress::Continue) => {
                        self.active_string = Some(active);
                    }
                    Ok(JsonStringProgress::Finished) => self.finish_string(active.role),
                    Err(()) => {
                        self.disable();
                        break;
                    }
                }
                continue;
            }
            self.consume_syntax(character);
            if matches!(self.state, TopLevelJsonState::Disabled) {
                break;
            }
        }
        (!visible.is_empty()).then_some(visible)
    }

    pub(crate) fn matches_final(&self, rationale: &str) -> bool {
        !matches!(self.state, TopLevelJsonState::Disabled)
            && self.rationale_seen
            && self.rationale_finished
            && self.decoded_rationale == rationale
            && !rationale.chars().any(is_unsafe_terminal_control)
    }

    fn consume_syntax(&mut self, character: char) {
        let state = std::mem::replace(&mut self.state, TopLevelJsonState::Disabled);
        self.state = match state {
            TopLevelJsonState::BeforeObject if character.is_whitespace() => {
                TopLevelJsonState::BeforeObject
            }
            TopLevelJsonState::BeforeObject if character == '{' => TopLevelJsonState::KeyOrEnd,
            TopLevelJsonState::KeyOrEnd if character.is_whitespace() => TopLevelJsonState::KeyOrEnd,
            TopLevelJsonState::KeyOrEnd if character == '}' => TopLevelJsonState::Complete,
            TopLevelJsonState::KeyOrEnd if character == '"' => {
                self.active_string = Some(ActiveJsonString::new(JsonStringRole::Key {
                    value: String::new(),
                    overflow: false,
                }));
                TopLevelJsonState::KeyOrEnd
            }
            TopLevelJsonState::Colon { target } if character.is_whitespace() => {
                TopLevelJsonState::Colon { target }
            }
            TopLevelJsonState::Colon { target } if character == ':' => {
                TopLevelJsonState::Value { target }
            }
            TopLevelJsonState::Value { target } if character.is_whitespace() => {
                TopLevelJsonState::Value { target }
            }
            TopLevelJsonState::Value { target } if character == '"' => {
                let role = if target && !self.rationale_seen {
                    self.rationale_seen = true;
                    JsonStringRole::Rationale
                } else {
                    JsonStringRole::SkippedValue
                };
                self.active_string = Some(ActiveJsonString::new(role));
                TopLevelJsonState::AfterValue
            }
            TopLevelJsonState::Value { .. } if character == '{' || character == '[' => {
                TopLevelJsonState::Nested {
                    closing: vec![if character == '{' { '}' } else { ']' }],
                }
            }
            TopLevelJsonState::Value { .. } if character != ',' && character != '}' => {
                TopLevelJsonState::Primitive
            }
            TopLevelJsonState::Nested { closing } if character == '"' => {
                self.active_string = Some(ActiveJsonString::new(JsonStringRole::Nested));
                TopLevelJsonState::Nested { closing }
            }
            TopLevelJsonState::Nested { mut closing } if character == '{' || character == '[' => {
                if closing.len() == MAX_STREAM_JSON_NESTING {
                    TopLevelJsonState::Disabled
                } else {
                    closing.push(if character == '{' { '}' } else { ']' });
                    TopLevelJsonState::Nested { closing }
                }
            }
            TopLevelJsonState::Nested { mut closing }
                if closing.last().copied() == Some(character) =>
            {
                closing.pop();
                if closing.is_empty() {
                    TopLevelJsonState::AfterValue
                } else {
                    TopLevelJsonState::Nested { closing }
                }
            }
            TopLevelJsonState::Nested { closing } => TopLevelJsonState::Nested { closing },
            TopLevelJsonState::Primitive if character == ',' => TopLevelJsonState::KeyOrEnd,
            TopLevelJsonState::Primitive if character == '}' => TopLevelJsonState::Complete,
            TopLevelJsonState::Primitive => TopLevelJsonState::Primitive,
            TopLevelJsonState::AfterValue if character.is_whitespace() => {
                TopLevelJsonState::AfterValue
            }
            TopLevelJsonState::AfterValue if character == ',' => TopLevelJsonState::KeyOrEnd,
            TopLevelJsonState::AfterValue if character == '}' => TopLevelJsonState::Complete,
            TopLevelJsonState::Complete if character.is_whitespace() => TopLevelJsonState::Complete,
            _ => TopLevelJsonState::Disabled,
        };
    }

    fn finish_string(&mut self, role: JsonStringRole) {
        self.state = match role {
            JsonStringRole::Key { value, overflow } => TopLevelJsonState::Colon {
                target: !overflow && value == "rationale",
            },
            JsonStringRole::Rationale => {
                self.rationale_finished = true;
                TopLevelJsonState::AfterValue
            }
            JsonStringRole::SkippedValue => TopLevelJsonState::AfterValue,
            JsonStringRole::Nested => {
                match std::mem::replace(&mut self.state, TopLevelJsonState::Disabled) {
                    TopLevelJsonState::Nested { closing } => TopLevelJsonState::Nested { closing },
                    _ => TopLevelJsonState::Disabled,
                }
            }
        };
    }

    fn disable(&mut self) {
        self.state = TopLevelJsonState::Disabled;
        self.active_string = None;
    }
}

#[derive(Debug)]
pub(crate) enum TopLevelJsonState {
    BeforeObject,
    KeyOrEnd,
    Colon { target: bool },
    Value { target: bool },
    Nested { closing: Vec<char> },
    Primitive,
    AfterValue,
    Complete,
    Disabled,
}

#[derive(Debug)]
pub(crate) struct ActiveJsonString {
    role: JsonStringRole,
    decoder: JsonStringDecoder,
}

impl ActiveJsonString {
    fn new(role: JsonStringRole) -> Self {
        Self {
            role,
            decoder: JsonStringDecoder::default(),
        }
    }
}

#[derive(Debug)]
enum JsonStringRole {
    Key { value: String, overflow: bool },
    Rationale,
    SkippedValue,
    Nested,
}

#[derive(Debug, Default)]
struct JsonStringDecoder {
    escape: JsonEscapeState,
}

impl JsonStringDecoder {
    fn push(&mut self, character: char) -> Result<JsonStringProgress, ()> {
        let state = std::mem::take(&mut self.escape);
        match state {
            JsonEscapeState::None => match character {
                '"' => Ok(JsonStringProgress::Finished),
                '\\' => {
                    self.escape = JsonEscapeState::AfterSlash;
                    Ok(JsonStringProgress::Continue)
                }
                character if character <= '\u{001f}' => Err(()),
                character => Ok(JsonStringProgress::Decoded(character)),
            },
            JsonEscapeState::AfterSlash => {
                let decoded = match character {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{0008}',
                    'f' => '\u{000c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        self.escape = JsonEscapeState::Unicode {
                            value: 0,
                            digits: 0,
                        };
                        return Ok(JsonStringProgress::Continue);
                    }
                    _ => return Err(()),
                };
                Ok(JsonStringProgress::Decoded(decoded))
            }
            JsonEscapeState::Unicode { value, digits } => {
                let digit = character.to_digit(16).ok_or(())? as u16;
                let value = value
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(digit))
                    .ok_or(())?;
                let digits = digits + 1;
                if digits < 4 {
                    self.escape = JsonEscapeState::Unicode { value, digits };
                    return Ok(JsonStringProgress::Continue);
                }
                if (0xd800..=0xdbff).contains(&value) {
                    self.escape = JsonEscapeState::LowSurrogateSlash { high: value };
                    Ok(JsonStringProgress::Continue)
                } else if (0xdc00..=0xdfff).contains(&value) {
                    Err(())
                } else {
                    char::from_u32(u32::from(value))
                        .map(JsonStringProgress::Decoded)
                        .ok_or(())
                }
            }
            JsonEscapeState::LowSurrogateSlash { high } if character == '\\' => {
                self.escape = JsonEscapeState::LowSurrogateU { high };
                Ok(JsonStringProgress::Continue)
            }
            JsonEscapeState::LowSurrogateU { high } if character == 'u' => {
                self.escape = JsonEscapeState::LowSurrogate {
                    high,
                    value: 0,
                    digits: 0,
                };
                Ok(JsonStringProgress::Continue)
            }
            JsonEscapeState::LowSurrogate {
                high,
                value,
                digits,
            } => {
                let digit = character.to_digit(16).ok_or(())? as u16;
                let value = value
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(digit))
                    .ok_or(())?;
                let digits = digits + 1;
                if digits < 4 {
                    self.escape = JsonEscapeState::LowSurrogate {
                        high,
                        value,
                        digits,
                    };
                    return Ok(JsonStringProgress::Continue);
                }
                if !(0xdc00..=0xdfff).contains(&value) {
                    return Err(());
                }
                let scalar =
                    0x1_0000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(value) - 0xdc00);
                char::from_u32(scalar)
                    .map(JsonStringProgress::Decoded)
                    .ok_or(())
            }
            JsonEscapeState::LowSurrogateSlash { .. } | JsonEscapeState::LowSurrogateU { .. } => {
                Err(())
            }
        }
    }
}

#[derive(Debug, Default)]
enum JsonEscapeState {
    #[default]
    None,
    AfterSlash,
    Unicode {
        value: u16,
        digits: u8,
    },
    LowSurrogateSlash {
        high: u16,
    },
    LowSurrogateU {
        high: u16,
    },
    LowSurrogate {
        high: u16,
        value: u16,
        digits: u8,
    },
}

#[derive(Debug)]
enum JsonStringProgress {
    Decoded(char),
    Continue,
    Finished,
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn rationale_stream_extracts_only_target_string_across_frames_and_escapes() {
        let mut extractor = RationaleStreamExtractor::default();
        let frames = [
            "{\"action\":{\"content\":\"must-not-leak\"},\"complete\":false,\"rati",
            "onale\":\"Line 1\\nquote: \\\"ok\\\" emoji: \\uD83D",
            "\\uDE3A and 汉字\",\"plan\":[\"also-not-visible\"]}",
        ];
        let visible = frames
            .into_iter()
            .filter_map(|frame| extractor.push(frame))
            .collect::<String>();
        assert_eq!(visible, "Line 1\nquote: \"ok\" emoji: 😺 and 汉字");
        assert!(!visible.contains("must-not-leak"));
        assert!(!visible.contains("also-not-visible"));
        assert!(!visible.contains("\"rationale\""));
        assert!(!visible.contains('{'));
    }

    #[test]
    fn rationale_stream_disables_before_exceeding_its_byte_bound() {
        let mut extractor = RationaleStreamExtractor {
            emitted_bytes: MAX_STREAMED_RATIONALE_BYTES - 1,
            ..RationaleStreamExtractor::default()
        };
        assert!(extractor.push("{\"rationale\":\"é\"}").is_none());
        assert!(matches!(extractor.state, TopLevelJsonState::Disabled));
    }

    #[test]
    fn observer_channel_rejects_unbounded_or_zero_capacity() {
        assert!(bounded_agent_stream_channel(1).is_ok());
        assert_eq!(
            bounded_agent_stream_channel(0).unwrap_err(),
            AgentStreamObserverError::InvalidCapacity {
                requested: 0,
                maximum: MAX_STREAM_OBSERVER_CAPACITY,
            }
        );
        assert_eq!(
            bounded_agent_stream_channel(MAX_STREAM_OBSERVER_CAPACITY + 1).unwrap_err(),
            AgentStreamObserverError::InvalidCapacity {
                requested: MAX_STREAM_OBSERVER_CAPACITY + 1,
                maximum: MAX_STREAM_OBSERVER_CAPACITY,
            }
        );
    }

    #[test]
    fn try_send_drops_when_the_queue_is_full_without_blocking() {
        // A live client that stops draining must not stall the agent loop.
        // Once the bounded queue fills, further observations are dropped.
        let (observer, mut receiver) = bounded_agent_stream_channel(2).unwrap();
        let event = AgentStreamEvent::Phase {
            role: "coder".into(),
            attempt: 0,
            sequence: 1,
            previous_phase: StreamPhase::SendingRequest,
            phase: StreamPhase::WaitingForFirstToken,
            timing: StreamTiming::default(),
        };
        assert!(observer.try_send(event.clone()));
        assert!(observer.try_send(event.clone()));
        // Queue is now full; this must not panic and must not block.
        assert!(!observer.try_send(event.clone()));
        assert_eq!(receiver.try_recv().unwrap(), event);
        assert_eq!(receiver.try_recv().unwrap(), event);
        assert!(receiver.try_recv().is_err());
    }
}
