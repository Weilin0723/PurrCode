//! Provider-stream lifecycle tracking that is independent of any UI or daemon transport.

use crate::ModelEvent;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

/// The user-relevant lifecycle of one provider stream.
///
/// `Queued` begins at request creation. Context preparation, request dispatch, and finalization are
/// explicit increments owned by the runtime. Receiving headers, bytes, or a parsed provider event
/// advances an early stream to `WaitingForFirstToken`. Only non-whitespace assistant content
/// advances it to `Receiving`; a tool call uses `ParsingToolCall`. The last three variants are
/// terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Returns whether no later increment may change this stream.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::Completed)
    }
}

/// One ordered input to [`StreamTracker`].
///
/// Network adapters should emit `Connected` after response headers and `BytesReceived` before
/// parsing those bytes. [`ModelEvent`] values can be converted without changing the existing
/// provider trait or event enum.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum StreamIncrement {
    Queued,
    PreparingContext,
    SendingRequest,
    Connected,
    BytesReceived {
        byte_count: usize,
    },
    ResponseStarted {
        response_id: String,
    },
    ContentDelta {
        delta: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Finalizing,
    Finished,
    Interrupted {
        reason: String,
    },
    Error {
        message: String,
    },
}

impl From<ModelEvent> for StreamIncrement {
    fn from(event: ModelEvent) -> Self {
        match event {
            ModelEvent::ResponseStarted { response_id } => Self::ResponseStarted { response_id },
            ModelEvent::TextDelta(delta) => Self::ContentDelta { delta },
            ModelEvent::ToolCall {
                call_id,
                name,
                arguments,
            } => Self::ToolCall {
                call_id,
                name,
                arguments,
            },
            ModelEvent::Usage {
                input_tokens,
                output_tokens,
            } => Self::Usage {
                input_tokens,
                output_tokens,
            },
            ModelEvent::Finished => Self::Finished,
        }
    }
}

/// Monotonic timings relative to request start.
///
/// `first_semantic_event_ms` includes either a non-whitespace content delta or a tool call so a
/// tool-only response has a meaningful responsiveness metric. `first_semantic_delta_ms` is
/// content-only. `completion_ms` is set only for a successful `Finished`; interrupted and failed
/// streams set `terminal_ms` without claiming completion.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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

/// The accepted increment together with its resulting lifecycle snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StreamUpdate {
    pub sequence: u64,
    pub previous_phase: StreamPhase,
    pub phase: StreamPhase,
    pub elapsed_ms: u64,
    pub timing: StreamTiming,
    pub increment: StreamIncrement,
}

/// A deterministic state machine for one provider request.
///
/// Callers supply monotonic timestamps. The tracker never reads the clock itself, which makes
/// phase and latency behavior testable without sleeps and lets the daemon own the clock boundary.
#[derive(Clone, Debug)]
pub struct StreamTracker {
    request_started_at: Instant,
    last_observed_at: Instant,
    connected_at: Option<Instant>,
    first_byte_at: Option<Instant>,
    first_semantic_event_at: Option<Instant>,
    first_semantic_delta_at: Option<Instant>,
    last_delta_at: Option<Instant>,
    last_semantic_event_at: Option<Instant>,
    completion_at: Option<Instant>,
    terminal_at: Option<Instant>,
    sequence: u64,
    phase: StreamPhase,
}

impl StreamTracker {
    /// Starts a stream in `Queued` at the supplied request timestamp.
    pub fn new(request_started_at: Instant) -> Self {
        Self {
            request_started_at,
            last_observed_at: request_started_at,
            connected_at: None,
            first_byte_at: None,
            first_semantic_event_at: None,
            first_semantic_delta_at: None,
            last_delta_at: None,
            last_semantic_event_at: None,
            completion_at: None,
            terminal_at: None,
            sequence: 0,
            phase: StreamPhase::Queued,
        }
    }

    pub fn phase(&self) -> StreamPhase {
        self.phase
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn timing(&self) -> StreamTiming {
        StreamTiming {
            connected_ms: self.offset_ms(self.connected_at),
            first_byte_ms: self.offset_ms(self.first_byte_at),
            first_semantic_event_ms: self.offset_ms(self.first_semantic_event_at),
            first_semantic_delta_ms: self.offset_ms(self.first_semantic_delta_at),
            last_delta_ms: self.offset_ms(self.last_delta_at),
            last_semantic_event_ms: self.offset_ms(self.last_semantic_event_at),
            completion_ms: self.offset_ms(self.completion_at),
            terminal_ms: self.offset_ms(self.terminal_at),
        }
    }

    /// Records one ordered stream increment.
    ///
    /// Parsed provider events imply that at least one byte and the connection have already been
    /// observed. Explicit `Connected` and `BytesReceived` increments retain the more precise
    /// network timestamps when an adapter can provide them.
    pub fn observe(
        &mut self,
        observed_at: Instant,
        increment: StreamIncrement,
    ) -> Result<StreamUpdate, StreamStateError> {
        if observed_at < self.request_started_at {
            return Err(StreamStateError::BeforeRequestStart);
        }
        if observed_at < self.last_observed_at {
            return Err(StreamStateError::NonMonotonicTimestamp);
        }
        if self.phase.is_terminal() {
            return Err(StreamStateError::AlreadyTerminal(self.phase));
        }
        if matches!(&increment, StreamIncrement::BytesReceived { byte_count: 0 }) {
            return Err(StreamStateError::EmptyByteObservation);
        }
        if self.phase == StreamPhase::Finalizing {
            let target = match &increment {
                StreamIncrement::ContentDelta { .. } => Some(StreamPhase::Receiving),
                StreamIncrement::ToolCall { .. } => Some(StreamPhase::ParsingToolCall),
                _ => None,
            };
            if let Some(to) = target {
                return Err(StreamStateError::InvalidTransition {
                    from: self.phase,
                    to,
                });
            }
        }
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(StreamStateError::SequenceOverflow)?;

        let previous_phase = self.phase;
        match &increment {
            StreamIncrement::Queued => {
                self.transition_explicit(StreamPhase::Queued)?;
            }
            StreamIncrement::PreparingContext => {
                self.transition_explicit(StreamPhase::PreparingContext)?;
            }
            StreamIncrement::SendingRequest => {
                self.transition_explicit(StreamPhase::SendingRequest)?;
            }
            StreamIncrement::Connected => {
                self.mark_connected(observed_at);
            }
            StreamIncrement::BytesReceived { .. } => {
                self.mark_transport_progress(observed_at);
            }
            StreamIncrement::ResponseStarted { .. } | StreamIncrement::Usage { .. } => {
                self.mark_parsed_event(observed_at);
            }
            StreamIncrement::ContentDelta { delta } => {
                self.mark_parsed_event(observed_at);
                if !delta.is_empty() {
                    self.last_delta_at = Some(observed_at);
                }
                if !delta.trim().is_empty() {
                    self.first_semantic_event_at.get_or_insert(observed_at);
                    self.first_semantic_delta_at.get_or_insert(observed_at);
                    self.last_semantic_event_at = Some(observed_at);
                    self.transition_semantic(StreamPhase::Receiving)?;
                }
            }
            StreamIncrement::ToolCall { .. } => {
                self.mark_parsed_event(observed_at);
                self.first_semantic_event_at.get_or_insert(observed_at);
                self.last_semantic_event_at = Some(observed_at);
                self.transition_semantic(StreamPhase::ParsingToolCall)?;
            }
            StreamIncrement::Finalizing => {
                self.transition_explicit(StreamPhase::Finalizing)?;
            }
            StreamIncrement::Finished => {
                self.mark_parsed_event(observed_at);
                self.completion_at = Some(observed_at);
                self.terminal_at = Some(observed_at);
                self.phase = StreamPhase::Completed;
            }
            StreamIncrement::Interrupted { .. } => {
                self.terminal_at = Some(observed_at);
                self.phase = StreamPhase::Cancelled;
            }
            StreamIncrement::Error { .. } => {
                self.terminal_at = Some(observed_at);
                self.phase = StreamPhase::Failed;
            }
        }

        self.last_observed_at = observed_at;
        self.sequence = next_sequence;
        Ok(StreamUpdate {
            sequence: self.sequence,
            previous_phase,
            phase: self.phase,
            elapsed_ms: milliseconds(
                observed_at
                    .checked_duration_since(self.request_started_at)
                    .expect("timestamp order was checked"),
            ),
            timing: self.timing(),
            increment,
        })
    }

    /// Records an existing provider event without modifying [`ModelEvent`] or [`crate::ModelProvider`].
    pub fn observe_model_event(
        &mut self,
        observed_at: Instant,
        event: ModelEvent,
    ) -> Result<StreamUpdate, StreamStateError> {
        self.observe(observed_at, event.into())
    }

    /// Time since connection (or request start before connection) or the latest semantic event.
    pub fn semantic_idle_for(&self, observed_at: Instant) -> Result<Duration, StreamStateError> {
        if observed_at < self.request_started_at {
            return Err(StreamStateError::BeforeRequestStart);
        }
        if observed_at < self.last_observed_at {
            return Err(StreamStateError::NonMonotonicTimestamp);
        }
        let anchor = self
            .last_semantic_event_at
            .or(self.connected_at)
            .unwrap_or(self.request_started_at);
        Ok(observed_at
            .checked_duration_since(anchor)
            .expect("semantic activity cannot follow the observation"))
    }

    /// Returns true only for a non-terminal stream whose semantic silence reached the threshold.
    pub fn is_stalled(
        &self,
        observed_at: Instant,
        threshold: Duration,
    ) -> Result<bool, StreamStateError> {
        if self.phase.is_terminal() || self.connected_at.is_none() {
            return Ok(false);
        }
        Ok(self.semantic_idle_for(observed_at)? >= threshold)
    }

    fn mark_connected(&mut self, observed_at: Instant) {
        self.connected_at.get_or_insert(observed_at);
        if matches!(
            self.phase,
            StreamPhase::Queued
                | StreamPhase::PreparingContext
                | StreamPhase::SendingRequest
                | StreamPhase::WaitingForFirstToken
        ) {
            self.phase = StreamPhase::WaitingForFirstToken;
        }
    }

    fn mark_transport_progress(&mut self, observed_at: Instant) {
        self.mark_connected(observed_at);
        self.first_byte_at.get_or_insert(observed_at);
    }

    fn mark_parsed_event(&mut self, observed_at: Instant) {
        self.mark_transport_progress(observed_at);
    }

    fn transition_explicit(&mut self, target: StreamPhase) -> Result<(), StreamStateError> {
        let allowed = match target {
            StreamPhase::Queued => self.phase == StreamPhase::Queued,
            StreamPhase::PreparingContext => matches!(
                self.phase,
                StreamPhase::Queued | StreamPhase::PreparingContext
            ),
            StreamPhase::SendingRequest => matches!(
                self.phase,
                StreamPhase::Queued | StreamPhase::PreparingContext | StreamPhase::SendingRequest
            ),
            StreamPhase::Finalizing => !self.phase.is_terminal(),
            _ => false,
        };
        if !allowed {
            return Err(StreamStateError::InvalidTransition {
                from: self.phase,
                to: target,
            });
        }
        self.phase = target;
        Ok(())
    }

    fn transition_semantic(&mut self, target: StreamPhase) -> Result<(), StreamStateError> {
        if self.phase == StreamPhase::Finalizing {
            return Err(StreamStateError::InvalidTransition {
                from: self.phase,
                to: target,
            });
        }
        self.phase = target;
        Ok(())
    }

    fn offset_ms(&self, timestamp: Option<Instant>) -> Option<u64> {
        timestamp.map(|timestamp| {
            milliseconds(
                timestamp
                    .checked_duration_since(self.request_started_at)
                    .expect("tracker timestamps cannot precede request start"),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StreamStateError {
    #[error("stream observation precedes request start")]
    BeforeRequestStart,
    #[error("stream observations must use nondecreasing timestamps")]
    NonMonotonicTimestamp,
    #[error("stream is already terminal in phase {0:?}")]
    AlreadyTerminal(StreamPhase),
    #[error("invalid stream phase transition from {from:?} to {to:?}")]
    InvalidTransition { from: StreamPhase, to: StreamPhase },
    #[error("a first-byte observation must contain at least one byte")]
    EmptyByteObservation,
    #[error("stream update sequence exhausted")]
    SequenceOverflow,
}

fn milliseconds(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, milliseconds: u64) -> Instant {
        start + Duration::from_millis(milliseconds)
    }

    #[test]
    fn content_stream_records_distinct_transport_and_semantic_timings() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);

        let connected = tracker
            .observe(at(start, 10), StreamIncrement::Connected)
            .unwrap();
        assert_eq!(connected.previous_phase, StreamPhase::Queued);
        assert_eq!(connected.phase, StreamPhase::WaitingForFirstToken);

        tracker
            .observe(
                at(start, 18),
                StreamIncrement::BytesReceived { byte_count: 7 },
            )
            .unwrap();
        let whitespace = tracker
            .observe(
                at(start, 25),
                StreamIncrement::ContentDelta {
                    delta: " \n".into(),
                },
            )
            .unwrap();
        assert_eq!(whitespace.phase, StreamPhase::WaitingForFirstToken);
        assert_eq!(whitespace.timing.first_semantic_delta_ms, None);

        let semantic = tracker
            .observe_model_event(at(start, 40), ModelEvent::TextDelta("hello".into()))
            .unwrap();
        assert_eq!(semantic.phase, StreamPhase::Receiving);
        assert_eq!(
            semantic.timing,
            StreamTiming {
                connected_ms: Some(10),
                first_byte_ms: Some(18),
                first_semantic_event_ms: Some(40),
                first_semantic_delta_ms: Some(40),
                last_delta_ms: Some(40),
                last_semantic_event_ms: Some(40),
                completion_ms: None,
                terminal_ms: None,
            }
        );

        let completed = tracker
            .observe_model_event(at(start, 75), ModelEvent::Finished)
            .unwrap();
        assert_eq!(completed.phase, StreamPhase::Completed);
        assert_eq!(completed.timing.completion_ms, Some(75));
        assert_eq!(completed.timing.terminal_ms, Some(75));
        assert_eq!(completed.sequence, 5);
    }

    #[test]
    fn parsed_event_supplies_safe_fallback_transport_timings() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        let update = tracker
            .observe_model_event(
                at(start, 12),
                ModelEvent::ResponseStarted {
                    response_id: "response-1".into(),
                },
            )
            .unwrap();

        assert_eq!(update.phase, StreamPhase::WaitingForFirstToken);
        assert_eq!(update.timing.connected_ms, Some(12));
        assert_eq!(update.timing.first_byte_ms, Some(12));
        assert_eq!(update.timing.first_semantic_event_ms, None);
    }

    #[test]
    fn tool_only_stream_has_semantic_latency_without_claiming_content() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        let update = tracker
            .observe_model_event(
                at(start, 30),
                ModelEvent::ToolCall {
                    call_id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"README.md\"}".into(),
                },
            )
            .unwrap();

        assert_eq!(update.phase, StreamPhase::ParsingToolCall);
        assert_eq!(update.timing.first_semantic_event_ms, Some(30));
        assert_eq!(update.timing.first_semantic_delta_ms, None);
        assert_eq!(update.timing.last_semantic_event_ms, Some(30));
    }

    #[test]
    fn interruption_and_error_are_terminal_without_claiming_completion() {
        let start = Instant::now();
        for (increment, expected_phase) in [
            (
                StreamIncrement::Interrupted {
                    reason: "cancelled by user".into(),
                },
                StreamPhase::Cancelled,
            ),
            (
                StreamIncrement::Error {
                    message: "provider disconnected".into(),
                },
                StreamPhase::Failed,
            ),
        ] {
            let mut tracker = StreamTracker::new(start);
            let update = tracker.observe(at(start, 9), increment).unwrap();
            assert_eq!(update.phase, expected_phase);
            assert_eq!(update.timing.completion_ms, None);
            assert_eq!(update.timing.terminal_ms, Some(9));
            assert_eq!(
                tracker.observe(at(start, 10), StreamIncrement::Finished),
                Err(StreamStateError::AlreadyTerminal(expected_phase))
            );
        }
    }

    #[test]
    fn semantic_stall_requires_connection_then_uses_latest_semantic_event() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        assert!(
            !tracker
                .is_stalled(at(start, 60_000), Duration::from_secs(30))
                .unwrap()
        );
        tracker
            .observe(at(start, 2), StreamIncrement::Connected)
            .unwrap();

        assert!(
            !tracker
                .is_stalled(at(start, 30_001), Duration::from_secs(30))
                .unwrap()
        );
        assert!(
            tracker
                .is_stalled(at(start, 30_002), Duration::from_secs(30))
                .unwrap()
        );

        tracker
            .observe_model_event(at(start, 31_000), ModelEvent::TextDelta("ready".into()))
            .unwrap();
        assert!(
            !tracker
                .is_stalled(at(start, 60_999), Duration::from_secs(30))
                .unwrap()
        );
        assert!(
            tracker
                .is_stalled(at(start, 61_000), Duration::from_secs(30))
                .unwrap()
        );

        tracker
            .observe_model_event(at(start, 61_001), ModelEvent::Finished)
            .unwrap();
        assert!(
            !tracker
                .is_stalled(at(start, 120_000), Duration::from_secs(30))
                .unwrap()
        );
    }

    #[test]
    fn invalid_timing_and_empty_byte_observations_do_not_mutate_state() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);

        assert_eq!(
            tracker.observe(start - Duration::from_millis(1), StreamIncrement::Connected),
            Err(StreamStateError::BeforeRequestStart)
        );
        assert_eq!(
            tracker.observe(
                at(start, 1),
                StreamIncrement::BytesReceived { byte_count: 0 }
            ),
            Err(StreamStateError::EmptyByteObservation)
        );
        tracker
            .observe(at(start, 5), StreamIncrement::Connected)
            .unwrap();
        assert_eq!(
            tracker.observe(at(start, 4), StreamIncrement::Finished),
            Err(StreamStateError::NonMonotonicTimestamp)
        );
        assert_eq!(tracker.sequence(), 1);
        assert_eq!(tracker.phase(), StreamPhase::WaitingForFirstToken);
    }

    #[test]
    fn sequence_overflow_is_explicit_and_does_not_mutate_state() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        tracker.sequence = u64::MAX;

        assert_eq!(
            tracker.observe(at(start, 1), StreamIncrement::Connected),
            Err(StreamStateError::SequenceOverflow)
        );
        assert_eq!(tracker.phase(), StreamPhase::Queued);
        assert_eq!(tracker.timing(), StreamTiming::default());
    }

    #[test]
    fn increments_and_updates_have_stable_serializable_shapes() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        let update = tracker
            .observe_model_event(at(start, 4), ModelEvent::TextDelta("hi".into()))
            .unwrap();
        let value = serde_json::to_value(&update).unwrap();

        assert_eq!(value["phase"], "receiving");
        assert_eq!(value["increment"]["type"], "content_delta");
        assert_eq!(value["increment"]["data"]["delta"], "hi");
        assert_eq!(
            serde_json::from_value::<StreamUpdate>(value).unwrap(),
            update
        );
    }

    #[test]
    fn explicit_and_observed_increments_cover_the_complete_prd_lifecycle() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        let cases = [
            (1, StreamIncrement::Queued, StreamPhase::Queued),
            (
                2,
                StreamIncrement::PreparingContext,
                StreamPhase::PreparingContext,
            ),
            (
                3,
                StreamIncrement::SendingRequest,
                StreamPhase::SendingRequest,
            ),
            (
                4,
                StreamIncrement::Connected,
                StreamPhase::WaitingForFirstToken,
            ),
            (
                5,
                StreamIncrement::ContentDelta {
                    delta: "answer".into(),
                },
                StreamPhase::Receiving,
            ),
            (
                6,
                StreamIncrement::ToolCall {
                    call_id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
                StreamPhase::ParsingToolCall,
            ),
            (
                7,
                StreamIncrement::ContentDelta {
                    delta: "continued".into(),
                },
                StreamPhase::Receiving,
            ),
            (8, StreamIncrement::Finalizing, StreamPhase::Finalizing),
            (9, StreamIncrement::Finished, StreamPhase::Completed),
        ];

        for (milliseconds, increment, expected) in cases {
            let update = tracker.observe(at(start, milliseconds), increment).unwrap();
            assert_eq!(update.phase, expected);
        }
        assert_eq!(tracker.sequence(), 9);
        assert_eq!(tracker.timing().completion_ms, Some(9));
    }

    #[test]
    fn rejected_semantic_regression_from_finalizing_does_not_mutate_state() {
        let start = Instant::now();
        let mut tracker = StreamTracker::new(start);
        tracker
            .observe(at(start, 1), StreamIncrement::Finalizing)
            .unwrap();
        let timing = tracker.timing();

        assert_eq!(
            tracker.observe(
                at(start, 2),
                StreamIncrement::ContentDelta {
                    delta: "late".into(),
                }
            ),
            Err(StreamStateError::InvalidTransition {
                from: StreamPhase::Finalizing,
                to: StreamPhase::Receiving,
            })
        );
        assert_eq!(tracker.phase(), StreamPhase::Finalizing);
        assert_eq!(tracker.sequence(), 1);
        assert_eq!(tracker.timing(), timing);
    }
}
