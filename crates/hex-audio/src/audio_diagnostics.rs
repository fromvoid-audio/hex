//! # Purpose
//! Own opt-in, bounded, resource-free audio trace records. The callback is the
//! sole producer; the existing control/client endpoint is the sole consumer.
//!
//! # Dependencies
//! Uses existing engine value types, rtrb and standard atomics. Construction,
//! draining, formatting and destruction happen on control, never on audio.
//!
//! # Does not own
//! Does not establish readiness, admission, command completion or asset custody.
//! Overflow drops diagnostic records only; musical queues and fault latches keep
//! their existing contracts. Consumers must bound each drain and report losses.

use hex_engine::{AudioSourceContext, PlaybackAction, PlaybackHandoffError};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU64, Ordering};

pub const TRACE_CAPACITY: usize = 256;
pub const MAX_DRAIN_PER_POLL: usize = 64;

/// Categories are independent of application log severity. Zero is default off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioDiagnosticsFlags(u32);
impl AudioDiagnosticsFlags {
    pub const OFF: Self = Self(0);
    pub const PUBLICATION: Self = Self(1);
    pub const TRANSPORT: Self = Self(2);
    pub const READINESS: Self = Self(4);
    pub const TIMING: Self = Self(8);
    pub const ALL: Self = Self(15);

    pub(crate) const fn bits(self) -> u32 {
        self.0
    }
    pub(crate) const fn from_bits(bits: u32) -> Self {
        Self(bits & Self::ALL.0)
    }
    pub const fn contains(self, category: Self) -> bool {
        self.0 & category.0 != 0
    }
    pub const fn enabled(self) -> bool {
        self.0 != 0
    }
}
impl std::str::FromStr for AudioDiagnosticsFlags {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() > 128 {
            return Err("audio diagnostics category list is too long");
        }
        if value == "off" {
            return Ok(Self::OFF);
        }
        if value == "all" {
            return Ok(Self::ALL);
        }
        let mut flags = Self::OFF;
        for category in value.split(',') {
            flags.0 |= match category {
                "publication" => Self::PUBLICATION.0,
                "transport" => Self::TRANSPORT.0,
                "readiness" => Self::READINESS.0,
                "timing" => Self::TIMING.0,
                _ => {
                    return Err(
                        "unknown audio diagnostics category; use publication,transport,readiness,timing,all or off",
                    );
                }
            };
        }
        Ok(flags)
    }
}

/// Copy-only payloads retain exact static/typed causes, never graphs or strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDiagnosticKind {
    PublicationAttempt,
    PublicationWaiting,
    PublicationPrepared,
    PublicationApplied,
    PublicationRejected {
        reason: hex_engine::PlaybackHandoffRejection,
    },
    StreamPreflightFailed {
        detail: &'static str,
        handoff: Option<PlaybackHandoffError>,
    },
    CommandDequeued {
        ticket: u64,
        action: PlaybackAction,
        discarded_by_stop: bool,
    },
    PriorityStop {
        through_ticket: u64,
    },
    CallbackTiming {
        elapsed_ns: u64,
        deadline_ns: u64,
    },
}
impl AudioDiagnosticKind {
    pub const fn category(self) -> AudioDiagnosticsFlags {
        match self {
            Self::StreamPreflightFailed { .. } | Self::PublicationWaiting => {
                AudioDiagnosticsFlags::READINESS
            }
            Self::CommandDequeued { .. } | Self::PriorityStop { .. } => {
                AudioDiagnosticsFlags::TRANSPORT
            }
            Self::CallbackTiming { .. } => AudioDiagnosticsFlags::TIMING,
            _ => AudioDiagnosticsFlags::PUBLICATION,
        }
    }
}

/// A stage observation, not a coherent authoritative snapshot of the whole engine.
/// Revision/context at the stage is exact; scalar frame/queue observations are
/// best-effort telemetry. Control log time is receipt time, not callback time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioDiagnosticEvent {
    pub sequence: u64,
    pub output: u64,
    pub revision: u64,
    pub context: Option<AudioSourceContext>,
    pub applied_revision: u64,
    pub consumed_command: u64,
    pub performance_frame: u64,
    pub arrangement_frame: u64,
    pub rate: u32,
    pub frames: usize,
    /// Readable command slots at this stage.
    pub commands_pending: usize,
    /// Free slots in the control-side retirement return queue.
    pub retirement_slots: usize,
    pub kind: AudioDiagnosticKind,
}

pub(crate) struct TraceWriter {
    queue: Producer<AudioDiagnosticEvent>,
    sequence: u64,
}
pub(crate) struct TraceReader {
    queue: Consumer<AudioDiagnosticEvent>,
}
pub(crate) fn channel() -> (TraceWriter, TraceReader) {
    let (writer, reader) = RingBuffer::new(TRACE_CAPACITY);
    (
        TraceWriter {
            queue: writer,
            sequence: 0,
        },
        TraceReader { queue: reader },
    )
}
impl TraceWriter {
    /// Caller checks category before constructing the snapshot. A failed push
    /// drops only Copy data, and the sequence gap/loss counter stay observable.
    pub(crate) fn push(&mut self, mut event: AudioDiagnosticEvent, dropped: &AtomicU64) {
        self.sequence = self.sequence.wrapping_add(1);
        event.sequence = self.sequence;
        if self.queue.push(event).is_err() {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        self.queue
            .buffer()
            .capacity()
            .checked_mul(std::mem::size_of::<AudioDiagnosticEvent>())?
            .checked_add(std::mem::size_of_val(self.queue.buffer()))
    }
}
impl TraceReader {
    pub(crate) fn pop(&mut self) -> Option<AudioDiagnosticEvent> {
        self.queue.pop().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn categories_are_explicit_and_default_off() {
        assert!(!AudioDiagnosticsFlags::default().enabled());
        let flags: AudioDiagnosticsFlags = "publication,readiness".parse().unwrap();
        assert!(flags.contains(AudioDiagnosticsFlags::PUBLICATION));
        assert!(flags.contains(AudioDiagnosticsFlags::READINESS));
        assert!(!flags.contains(AudioDiagnosticsFlags::TIMING));
        for bad in ["", "all,off", "unknown", "publication,"] {
            assert!(bad.parse::<AudioDiagnosticsFlags>().is_err());
        }
        assert_eq!("off".parse(), Ok(AudioDiagnosticsFlags::OFF));
    }
    #[test]
    fn overflow_preserves_records_exact_causes_and_visible_sequence_gaps() {
        let (mut writer, mut reader) = channel();
        let dropped = AtomicU64::new(0);
        let cause = PlaybackHandoffError::RuntimeFenceChanged;
        let event = AudioDiagnosticEvent {
            sequence: 0,
            output: 7,
            revision: 315,
            context: None,
            applied_revision: 314,
            consumed_command: 20,
            performance_frame: 128,
            arrangement_frame: 256,
            rate: 44_100,
            frames: 128,
            commands_pending: 3,
            retirement_slots: 4,
            kind: AudioDiagnosticKind::StreamPreflightFailed {
                detail: "stream handoff demand refused",
                handoff: Some(cause),
            },
        };
        for _ in 0..TRACE_CAPACITY + 2 {
            writer.push(event, &dropped);
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        for sequence in 1..=TRACE_CAPACITY as u64 {
            assert_eq!(
                reader.pop(),
                Some(AudioDiagnosticEvent { sequence, ..event })
            );
        }
        assert!(reader.pop().is_none());
        writer.push(event, &dropped);
        assert_eq!(reader.pop().unwrap().sequence, TRACE_CAPACITY as u64 + 3);
    }
}
