//! Streaming observer and rationale extraction for live clients.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

use purrcode_provider_gateway::{StreamPhase, StreamTiming, MAX_PROVIDER_STREAM_FRAME_BYTES};

/// At the provider frame limit, the largest permitted queue retains at most 16 MiB of deltas.
pub const MAX_STREAM_OBSERVER_CAPACITY: usize = 64;

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
/// Sending awaits available capacity, so a slow live client applies backpressure instead of
/// creating an unbounded queue. Dropping the receiver disables observation without changing the
/// authoritative agent result.
#[derive(Clone, Debug)]
pub struct AgentStreamObserver {
    pub(crate) sender: mpsc::Sender<AgentStreamEvent>,
}

pub type AgentStreamReceiver = mpsc::Receiver<AgentStreamEvent>;

pub fn bounded_agent_stream_channel(
    capacity: usize,
) -> Result<(AgentStreamObserver, AgentStreamReceiver), AgentStreamObserverError> {
    if capacity == 0 || capacity > MAX_STREAM_OBSERVER_CAPACITY {
        return Err(AgentStreamObserverError::InvalidCapacity {
            requested: capacity,
            maximum: MAX_STREAM_OBSERVER_CAPACITY,
        });
    }
    let (sender, receiver) = mpsc::channel(capacity);
    Ok((AgentStreamObserver { sender }, receiver))
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
}
