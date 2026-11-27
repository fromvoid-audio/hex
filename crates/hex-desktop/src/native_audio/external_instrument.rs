//! Desktop adapter binding `hex_midi::OutputProducer` to
//! `hex_audio::external_instrument::ExternalMidiSender` without duplicating
//! hardware MIDI endpoint workers (#3474).

use hex_audio::external_instrument::{
    ExternalMidiEnqueueStatus, ExternalMidiEvent, ExternalMidiSender,
};
use hex_midi::{OutputEnqueueResult, OutputMessage, OutputProducer, TimestampedOutputMessage};

/// Zero-allocation wrapper adapting the existing `hex_midi::OutputProducer`
/// to `hex_audio`'s `ExternalMidiSender` callback contract.
pub(crate) struct MidiOutputProducerBridge<'a>(pub(crate) &'a OutputProducer);

impl ExternalMidiSender for MidiOutputProducerBridge<'_> {
    fn active_activation(&self) -> u64 {
        self.0.active_activation()
    }

    fn try_send_event(
        &self,
        activation: u64,
        timestamp_ns: u64,
        channel: u8,
        event: ExternalMidiEvent,
    ) -> ExternalMidiEnqueueStatus {
        let message = match event {
            ExternalMidiEvent::NoteOn {
                pitch, velocity, ..
            } => OutputMessage::NoteOn {
                channel,
                pitch,
                velocity,
            },
            ExternalMidiEvent::NoteOff {
                pitch, velocity, ..
            } => OutputMessage::NoteOff {
                channel,
                pitch,
                velocity,
            },
            ExternalMidiEvent::ControlChange { cc, value, .. } => {
                OutputMessage::ControlChange { channel, cc, value }
            }
            ExternalMidiEvent::PitchBend { value, .. } => {
                OutputMessage::PitchBend { channel, value }
            }
            ExternalMidiEvent::ChannelPressure { value, .. } => {
                OutputMessage::ChannelPressure { channel, value }
            }
            ExternalMidiEvent::PolyPressure { pitch, value, .. } => OutputMessage::PolyPressure {
                channel,
                pitch,
                value,
            },
        };
        map_enqueue_result(self.0.try_send(
            activation,
            TimestampedOutputMessage {
                timestamp_ns,
                message,
            },
        ))
    }

    fn request_stop(&self, activation: u64) -> ExternalMidiEnqueueStatus {
        map_enqueue_result(self.0.request_stop(activation))
    }
}

fn map_enqueue_result(result: OutputEnqueueResult) -> ExternalMidiEnqueueStatus {
    match result {
        OutputEnqueueResult::Queued | OutputEnqueueResult::QueuedPriority => {
            ExternalMidiEnqueueStatus::Queued
        }
        OutputEnqueueResult::Overflowed | OutputEnqueueResult::PriorityOverflowCleanupLatched => {
            ExternalMidiEnqueueStatus::Overflowed
        }
        OutputEnqueueResult::StaleActivation => ExternalMidiEnqueueStatus::StaleActivation,
        OutputEnqueueResult::Disconnected => ExternalMidiEnqueueStatus::Disconnected,
        OutputEnqueueResult::InvalidMessage(_) => ExternalMidiEnqueueStatus::InvalidMessage,
    }
}
