//! One bounded controller lease, bound to an immutable project/instrument identity.
use hex_project::{DeviceId, TrackId};
use std::sync::atomic::{
    AtomicBool, AtomicU64,
    Ordering::{Acquire, Release},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub pressure: u8,
    pub bend: i16,
    pub wheel: u8,
}
impl Snapshot {
    pub fn new(pressure: u8, bend: i16, wheel: u8) -> Option<Self> {
        (pressure <= 127 && (-8192..=8191).contains(&bend) && wheel <= 127).then_some(Self {
            pressure,
            bend,
            wheel,
        })
    }
    pub(crate) fn events(self) -> [hex_lattice::NativeInstrumentEvent; 3] {
        use hex_lattice::{NativeInstrumentEvent::Expression, NativeInstrumentExpression::*};
        [
            ChannelPressure(self.pressure),
            PitchBend(self.bend),
            ModWheel(self.wheel),
        ]
        .map(|expression| Expression {
            frame_offset: 0,
            expression,
        })
    }
}
/// The MIDI callback is the sole snapshot writer. Intermediate messages coalesce.
/// Cancellation is terminal and overrides even a late callback publication.
#[derive(Debug)]
pub struct LiveExpressionInput {
    pub project_generation: u64,
    pub track: TrackId,
    pub instrument: DeviceId,
    cancelled: AtomicBool,
    packed: AtomicU64,
}
impl LiveExpressionInput {
    pub fn new(project_generation: u64, track: TrackId, instrument: DeviceId) -> Self {
        Self {
            project_generation,
            track,
            instrument,
            cancelled: AtomicBool::new(false),
            packed: AtomicU64::new(0),
        }
    }
    pub fn publish(&self, value: Snapshot) {
        if Snapshot::new(value.pressure, value.bend, value.wheel).is_some() {
            let word = u64::from(value.pressure)
                | (u64::from(value.bend as u16) << 8)
                | (u64::from(value.wheel) << 24)
                | (1u64 << 32);
            self.packed.store(word, Release);
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Acquire)
    }
    /// A new or cancelled input has no source-global controller publication.
    /// Keep this flag in the same atomic word as the value so an untouched
    /// input cannot replace a shared source's last controller state with zero.
    pub(crate) fn published_snapshot(&self) -> Option<Snapshot> {
        let word = self.packed.load(Acquire);
        if word & (1u64 << 32) == 0 || self.is_cancelled() {
            return None;
        }
        Some(Snapshot {
            pressure: word as u8,
            bend: (word >> 8) as u16 as i16,
            wheel: (word >> 24) as u8,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        let word = self.packed.load(Acquire);
        if self.is_cancelled() {
            Snapshot::default()
        } else {
            Snapshot {
                pressure: word as u8,
                bend: (word >> 8) as u16 as i16,
                wheel: (word >> 24) as u8,
            }
        }
    }
}
