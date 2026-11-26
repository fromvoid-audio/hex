//! # Purpose
//! Own optional control-side MIDI controller input, bounded scalar delivery,
//! and bounded selected hardware MIDI output handoff with deterministic note cleanup.
//!
//! # Dependencies
//! Uses the standard library and the configured platform MIDI dependency on supported targets.
//!
//! # Does not own
//! Does not schedule musical notes, mutate project state, own external-instrument UI,
//! or perform audio callback DSP. Physical held snapshots support control-side step
//! entry without scheduling or monitoring sound; hardware output calibration and
//! cross-platform drivers remain outside this macOS-first boundary.

mod aggregator;
mod framing;
mod held_notes;
mod mpe;
mod native;
mod note_events;
mod output;
mod output_native;
mod timebase;
mod worker;

pub use aggregator::{MAX_AGGREGATED_PORTS, MultiPortAggregator};
pub use framing::{
    ChannelFilter, Controllers, Decoder, deliver_packets, deliver_packets_filtered,
    deliver_packets_with_mpe, deliver_packets_with_notes, deliver_packets_with_notes_filtered,
    deliver_timed_packets_with_notes, deliver_timed_packets_with_notes_filtered, parse,
    parse_filtered,
};
pub use held_notes::{
    HELD_NOTE_COUNT, HeldNote, HeldNoteFault, HeldNoteInput, HeldNotes, HeldNotesRead,
};
pub use mpe::{
    DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES, DEFAULT_MPE_PITCH_BEND_SEMITONES,
    MAX_MPE_MASTER_PITCH_BEND_SEMITONES, MAX_MPE_PITCH_BEND_CENTS, MAX_MPE_PITCH_BEND_SEMITONES,
    MpeActiveNote, MpeConfig, MpeDecoder, MpeExpressionEvent, MpeZone,
};
pub use note_events::{ControlChange, NoteEvent};
pub use output::{
    EncodedMidiMessage, MAX_OUTPUT_PORT_NAME_BYTES, MAX_OUTPUT_PORTS, OUTPUT_MAX_EVENTS_PER_DRAIN,
    OUTPUT_NORMAL_CAPACITY, OUTPUT_PRIORITY_CAPACITY, OutputBackend, OutputCommand,
    OutputConnectionState, OutputEnqueueResult, OutputError, OutputEvent, OutputHandle,
    OutputMessage, OutputPort, OutputProducer, OutputSendError, OutputWorker,
    TimestampedOutputMessage,
};
pub use timebase::{MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent};
pub use worker::{Command, Event, Handle, Port, Sink, Worker};

#[cfg(test)]
pub(crate) use framing::{CALLBACK_BYTES, CALLBACK_PACKETS};
