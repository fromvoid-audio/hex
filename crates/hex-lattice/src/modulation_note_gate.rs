//! One control-preallocated note gate shared by the sources in a bank.
//!
//! An occurrence starts at its Note On and ends at its Note Off. Unknown
//! releases can follow an insertion during playback, so they leave the gate
//! alone. The ordinary host admits at most 64 simultaneous occurrences.
use crate::NativeInstrumentEvent;
use hex_lattice_contract::DeviceContractError as Error;

const MAX_HELD: usize = 64;

#[derive(Clone, Copy)]
struct HeldNotes {
    ids: [u64; MAX_HELD],
    len: usize,
}

impl HeldNotes {
    const EMPTY: Self = Self {
        ids: [0; MAX_HELD],
        len: 0,
    };

    fn on(&mut self, id: u64) -> Result<bool, Error> {
        if id == 0 || self.ids[..self.len].contains(&id) {
            return Err(Error::InvalidId);
        }
        let opened = self.len == 0;
        let slot = self
            .ids
            .get_mut(self.len)
            .ok_or(Error::RuntimeBudgetExceeded)?;
        *slot = id;
        self.len += 1;
        Ok(opened)
    }

    fn off(&mut self, id: u64) -> Result<(), Error> {
        if id == 0 {
            return Err(Error::InvalidId);
        }
        if let Some(index) = self.ids[..self.len].iter().position(|held| *held == id) {
            self.len -= 1;
            self.ids[index] = self.ids[self.len];
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct GateFrame {
    pub(crate) held: bool,
    pub(crate) onset: bool,
    pub(crate) opened: bool,
}

pub(crate) struct NoteGate {
    committed: HeldNotes,
    scratch: HeldNotes,
    frames: Vec<GateFrame>,
}

impl NoteGate {
    pub(crate) fn quoted_heap_bytes(frames: usize) -> Option<usize> {
        frames
            .checked_mul(std::mem::size_of::<GateFrame>())?
            .checked_add(std::mem::size_of::<Self>())
    }

    pub(crate) fn new(frames: usize) -> Result<Box<Self>, Error> {
        let mut cache = Vec::new();
        cache
            .try_reserve_exact(frames)
            .map_err(|_| Error::RuntimeAllocation)?;
        cache.resize(frames, GateFrame::default());
        Ok(Box::new(Self {
            committed: HeldNotes::EMPTY,
            scratch: HeldNotes::EMPTY,
            frames: cache,
        }))
    }

    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        Self::quoted_heap_bytes(self.frames.capacity())
    }

    pub(crate) fn begin(&mut self) {
        self.scratch = self.committed;
    }

    pub(crate) fn commit(&mut self) {
        self.committed = self.scratch;
    }

    pub(crate) fn reset(&mut self) {
        self.committed = HeldNotes::EMPTY;
        self.scratch = HeldNotes::EMPTY;
    }

    pub(crate) fn inherit(&mut self, previous: &Self) {
        self.committed = previous.committed;
        self.scratch = previous.committed;
    }

    /// The bank has already checked offset order and range. Refusal leaves
    /// the previously accepted held state available for a retry.
    pub(crate) fn render(
        &mut self,
        count: usize,
        events: &[NativeInstrumentEvent],
    ) -> Result<&[GateFrame], Error> {
        let frames = self
            .frames
            .get_mut(..count)
            .ok_or(Error::RuntimeBudgetExceeded)?;
        let mut held = self.scratch;
        let mut events = events.iter().copied().peekable();
        for (offset, frame) in frames.iter_mut().enumerate() {
            let mut gate = GateFrame::default();
            while let Some(event) = events.next_if(|event| event.frame_offset() == offset) {
                match event {
                    NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        gate.opened |= held.on(occurrence_id)?;
                        gate.onset = true;
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        held.off(occurrence_id)?
                    }
                    _ => {}
                }
            }
            gate.held = held.len != 0;
            *frame = gate;
        }
        self.scratch = held;
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(frame_offset: usize, occurrence_id: u64) -> NativeInstrumentEvent {
        NativeInstrumentEvent::NoteOn {
            frame_offset,
            occurrence_id,
            pitch: 60,
            velocity: 100,
        }
    }
    fn off(frame_offset: usize, occurrence_id: u64) -> NativeInstrumentEvent {
        NativeInstrumentEvent::NoteOff {
            frame_offset,
            occurrence_id,
        }
    }

    #[test]
    fn overlapping_occurrences_and_same_sample_replacement_preserve_gate_order() {
        let mut gate = NoteGate::new(4).unwrap();
        gate.begin();
        let events = [
            on(0, 1),
            on(1, 2),
            off(2, 1),
            off(2, 3),
            off(3, 2),
            on(3, 4),
        ];
        let block = gate.render(4, &events).unwrap();
        let held = block.iter().map(|frame| frame.held).collect::<Vec<_>>();
        let opened = block.iter().map(|frame| frame.opened).collect::<Vec<_>>();
        assert_eq!(held, [true; 4]);
        assert_eq!(opened, [true, false, false, true]);
        gate.commit();

        let mut next = NoteGate::new(4).unwrap();
        next.inherit(&gate);
        next.begin();
        assert!(next.render(1, &[on(0, 4)]).is_err());
        assert_eq!(
            next.render(1, &[off(0, 4)])
                .unwrap()
                .first()
                .map(|frame| frame.held),
            Some(false),
        );
        next.begin();
        assert!(next.render(1, &[]).unwrap()[0].held);
        next.reset();
        assert!(!next.render(1, &[]).unwrap()[0].held);
    }

    #[test]
    fn a_refused_batch_keeps_the_last_accepted_gate_state() {
        let mut gate = NoteGate::new(2).unwrap();
        gate.begin();
        assert!(gate.render(1, &[on(0, 1)]).unwrap()[0].held);
        let dense: [NativeInstrumentEvent; MAX_HELD] = std::array::from_fn(|i| on(0, i as u64 + 2));
        assert!(matches!(
            gate.render(1, &dense),
            Err(Error::RuntimeBudgetExceeded)
        ));
        assert!(gate.render(1, &[off(0, 0)]).is_err());
        assert!(!gate.render(1, &[off(0, 1)]).unwrap()[0].held);
    }
}
