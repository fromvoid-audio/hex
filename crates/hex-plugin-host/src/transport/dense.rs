//! Optional copied parameter storage for a reserved endpoint. Ordinary PCM
//! requests and replies carry no sample array for this lane.
use super::*;

pub struct DenseParameterRequest {
    pub(super) sequence: u64,
    id: u32,
    frames: u16,
    normalized: [f64; MAX_FRAMES],
}
impl DenseParameterRequest {
    pub fn lane(&self) -> DenseParameterLane<'_> {
        DenseParameterLane {
            id: self.id,
            normalized: &self.normalized[..usize::from(self.frames)],
        }
    }
}

pub(super) struct DenseStaging {
    pub(super) sender: Producer<DenseParameterRequest>,
    active: Option<DenseParameterRequest>,
    pub(super) pending: Option<DenseParameterRequest>,
}

impl DenseStaging {
    pub(super) fn new(sender: Producer<DenseParameterRequest>) -> Self {
        Self {
            sender,
            active: None,
            pending: None,
        }
    }

    pub(super) fn stage(
        &mut self,
        position: u64,
        quantum: u16,
        frames: usize,
        lane: Option<DenseParameterLane<'_>>,
    ) -> Result<(), Fault> {
        if lane.is_some_and(|lane| !lane.valid(frames)) {
            return Err(Fault::InvalidParameters);
        }
        let quantum_frames = usize::from(quantum);
        let mut consumed = 0;
        // A physical callback is at most one quantum long. It can complete
        // one quantum and open at most one additional partial prefix.
        while consumed < frames {
            let start = position
                .checked_add(consumed as u64)
                .ok_or(Fault::Exhausted)?;
            let offset = (start % u64::from(quantum)) as usize;
            let count = (frames - consumed).min(quantum_frames - offset);
            if offset == 0 {
                self.active = lane.map(|lane| DenseParameterRequest {
                    sequence: start / u64::from(quantum),
                    id: lane.id,
                    frames: quantum,
                    normalized: [0.0; MAX_FRAMES],
                });
            }
            match (&mut self.active, lane) {
                (Some(active), Some(lane)) if active.id == lane.id => {
                    active.normalized[offset..offset + count]
                        .copy_from_slice(&lane.normalized[consumed..consumed + count]);
                }
                (None, None) => {}
                _ => return Err(Fault::InvalidParameters),
            }
            if offset + count == quantum_frames {
                self.pending = self.active.take();
            }
            consumed += count;
        }
        Ok(())
    }
}
