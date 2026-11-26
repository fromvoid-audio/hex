//! # Purpose
//! Own optional control-side MIDI controller input and bounded scalar delivery.
//!
//! # Dependencies
//! Uses the standard library and the configured platform MIDI dependency on supported targets.
//!
//! # Does not own
//! Does not schedule musical notes, mutate project state or perform audio callback work. Hardware
//! note hosting is outside this input-capture boundary. Physical held snapshots
//! support control-side step entry without scheduling or monitoring sound.

mod aggregator;
mod framing;
mod held_notes;
mod mpe;
mod native;
mod note_events;
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
pub use timebase::{MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent};
pub use worker::{Command, Event, Handle, Port, Sink, Worker};

#[cfg(test)]
pub(crate) use framing::{CALLBACK_BYTES, CALLBACK_PACKETS};
