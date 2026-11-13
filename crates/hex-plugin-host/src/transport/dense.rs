//! Optional copied parameter storage for a reserved endpoint. Ordinary PCM
//! requests and replies carry no sample array for this lane.
use super::*;

pub struct DenseParameterRequest {
    sequence: u64,
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
    sender: Producer<DenseParameterRequest>,
    active: Option<DenseParameterRequest>,
    pending: Option<DenseParameterRequest>,
}

impl DenseStaging {
    fn new(sender: Producer<DenseParameterRequest>) -> Self {
        Self {
            sender,
            active: None,
            pending: None,
        }
    }

    // Keep the sidecar copy and its stack storage outside the ordinary
    // callback's commit path.
    fn publish(&mut self, request: &InputRequest) -> Result<(), Fault> {
        let Some(dense) = self.pending.take() else {
            return Ok(());
        };
        if dense.sequence != request.block.sequence
            || request
                .block
                .parameters
                .as_slice()
                .iter()
                .any(|point| point.id == dense.id)
        {
            return Err(Fault::InvalidParameters);
        }
        // The worker acquires the sparse commit record before reading the
        // sidecar. A refused commit leaves no DSP-visible orphan.
        self.sender.push(dense).map_err(|_| Fault::Saturated)?;
        Ok(())
    }

    fn stage(
        &mut self,
        position: u64,
        quantum: u16,
        frames: usize,
        parameters: &[ParameterPoint],
        lane: Option<DenseParameterLane<'_>>,
    ) -> Result<(), Fault> {
        if frames > usize::from(quantum)
            || lane.is_some_and(|lane| {
                !lane.valid(frames) || parameters.iter().any(|point| point.id == lane.id)
            })
        {
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

/// A copied effect endpoint with an independent dense staging lane. Using
/// a separate endpoint keeps ordinary effect and instrument callbacks on the
/// original staging and commit paths.
pub struct DenseEffectCallback {
    callback: Callback,
    staging: Box<DenseStaging>,
}

/// Reserve the additional eight-slot ring on control before publication.
pub fn dense_effect_channel_from_completed_prefix(
    binding: Binding,
    quantum: u16,
    completed_blocks: u64,
) -> Result<(Control, DenseEffectCallback, Worker), Fault> {
    let (control, callback, mut worker) =
        native_pcm_channel_from_completed_prefix(binding, quantum, completed_blocks)?;
    let (sender, receive) = RingBuffer::new(QUEUE_CAPACITY);
    worker.dense_requests = Some(receive);
    Ok((
        control,
        DenseEffectCallback {
            callback,
            staging: Box::new(DenseStaging::new(sender)),
        },
        worker,
    ))
}

impl DenseEffectCallback {
    /// Compare the original constructor pair without exposing an ordinary
    /// endpoint view that could undercount the reserved dense storage.
    pub fn matches_control(&self, control: &Control) -> bool {
        self.callback.matches_control(control)
    }

    pub fn constructor_storage_on_control(&self, control: &Control) -> Option<ConstructorStorage> {
        let ring = self.staging.sender.buffer();
        let ring_bytes = ring
            .capacity()
            .checked_mul(std::mem::size_of::<DenseParameterRequest>())?
            .checked_add(std::mem::size_of_val(ring))?;
        self.callback
            .constructor_storage_on_control(control)?
            .with_dense(ring_bytes)
    }

    pub fn begin_physical_callback(&mut self, frames: usize) -> Result<(), Fault> {
        self.callback.begin_physical_callback(frames)
    }

    /// The lane is either absent or present for every segment of a quantum.
    /// Its raw parameter ID must stay the same until that quantum closes.
    #[allow(clippy::too_many_arguments)] // Same borrowed windows as ordinary effect staging.
    pub fn stage_effect_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
        lane: Option<DenseParameterLane<'_>>,
    ) -> Result<SegmentReply, Fault> {
        let Some(stage) = &self.callback.staged else {
            output.fill([0.0; 2]);
            return self.callback.fail(Fault::Protocol);
        };
        if let Err(fault) = self.staging.stage(
            stage.state.position,
            self.callback.quantum,
            input.len(),
            parameters,
            lane,
        ) {
            output.fill([0.0; 2]);
            return self.callback.fail(fault);
        }
        self.callback.stage_effect_segment(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            origins,
        )
    }

    pub fn finish(&mut self, host_accepted: bool) -> Result<(), Fault> {
        if host_accepted
            && !self.callback.is_revoked()
            && let Some(stage) = &self.callback.staged
            && stage.staged_frames == stage.expected_frames
            && let Some(request) = &stage.request
            && let Err(fault) = self.staging.publish(request)
        {
            return self.callback.fail(fault);
        }
        self.callback.finish(host_accepted)
    }

    pub fn fault(&self) -> Option<Fault> {
        self.callback.fault()
    }

    pub fn reject(&mut self, fault: Fault) {
        self.callback.reject(fault);
    }

    pub fn has_complete_staged_callback(&self) -> bool {
        self.callback.has_complete_staged_callback()
    }

    pub fn end_staging_at_pause(&mut self, frame: usize) {
        self.callback.end_staging_at_pause(frame);
    }

    pub fn take_accepted_editor_quantum(&mut self) -> Option<AcceptedEditorQuantum> {
        self.callback.take_accepted_editor_quantum()
    }

    pub fn take_accepted_manual_parameter(&mut self) -> Option<AcceptedManualParameter> {
        self.callback.take_accepted_manual_parameter()
    }

    pub fn dispatched_blocks_on_control(&self) -> u64 {
        self.callback.dispatched_blocks_on_control()
    }

    #[cfg(any(test, feature = "test-support", feature = "allocation-probe"))]
    pub fn committed_position_for_test(&self) -> u64 {
        self.callback.committed_position_for_test()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_requests_for_test(&self) -> usize {
        self.callback.pending_requests_for_test()
    }

    pub fn is_revoked(&self) -> bool {
        self.callback.is_revoked()
    }

    /// Reserve an observed generation only on its actual paired dense endpoint.
    pub fn reserve_observed_parameter_input_on_callback(
        &self,
        source: &super::ParameterInputSource,
    ) -> Option<super::ParameterInputFence<'_>> {
        self.callback
            .reserve_observed_parameter_input_on_callback(source)
    }

    /// Reserve the exact input generation on this paired dense endpoint.
    pub fn reserve_parameter_input_on_callback(
        &self,
        ordinal: std::num::NonZeroU64,
    ) -> Option<super::ParameterInputFence<'_>> {
        self.callback.reserve_parameter_input_on_callback(ordinal)
    }

    #[cfg(test)]
    pub(super) fn replace_dense_ring_for_test(&mut self, worker: &mut Worker, capacity: usize) {
        let (sender, receiver) = RingBuffer::new(capacity);
        self.staging.sender = sender;
        worker.dense_requests = Some(receiver);
    }
}

// Acquire the ordinary request first. A sidecar for a future quantum can
// already be present, but cannot be borrowed by this earlier request.
pub(super) fn receive(
    requests: &mut Consumer<DenseParameterRequest>,
    block: &Block,
) -> Result<Option<DenseParameterRequest>, Fault> {
    if requests
        .peek()
        .is_ok_and(|lane| lane.sequence <= block.sequence)
    {
        let dense = requests.pop().map_err(|_| Fault::Protocol)?;
        if dense.sequence != block.sequence || !dense.lane().valid(usize::from(block.frames)) {
            return Err(Fault::InvalidParameters);
        }
        Ok(Some(dense))
    } else {
        Ok(None)
    }
}
