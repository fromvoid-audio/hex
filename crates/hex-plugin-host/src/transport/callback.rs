//! Real-time callback-side staging, concealment, and bounded reply consumption.
use super::*;

impl Callback {
    /// Whether the last accepted physical callback completed its processing
    /// quantum and no speculative input is currently staged on this endpoint.
    pub fn at_committed_quantum_boundary(&self) -> bool {
        self.staged.is_none()
            && self.committed_quantum == Some(CommittedQuantumState::Closed)
            && !self.revoked.is_revoked()
    }

    /// Committed request prefix for an exclusively owned offline endpoint.
    /// No observation or borrow of the device callback is permitted.
    pub fn dispatched_blocks_on_control(&self) -> u64 {
        self.committed_position / u64::from(self.quantum)
    }
    /// Permanently refuse this exchange at its actual host validation boundary.
    /// Rejection neither replays work nor destroys either endpoint.
    pub fn reject(&mut self, fault: Fault) {
        let _ = self.fail::<()>(fault);
    }
    /// Constructor pairing only; never native/root publication authority.
    pub fn matches_control(&self, control: &Control) -> bool {
        Arc::ptr_eq(&self.revoked, &control.revoked)
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    /// Diagnostic observation of actual committed transport progress. Staging
    /// alone does not advance it; this grants no worker/readiness authority.
    #[cfg(any(test, feature = "test-support", feature = "allocation-probe"))]
    pub fn committed_position_for_test(&self) -> u64 {
        self.committed_position
    }
    /// Snapshot the original producer's pending requests. This is diagnostic
    /// progress only, not processing completion or native/publication custody.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_requests_for_test(&self) -> usize {
        QUEUE_CAPACITY - self.requests.slots()
    }
    pub fn quantum_frames(&self) -> usize {
        usize::from(self.quantum)
    }
    pub fn revoke(&mut self) {
        self.revoked.revoke();
        self.staged = None;
        self.accepted_manual = None;
        self.accepted_editor = None;
    }
    pub fn latency_frames(&self) -> u32 {
        u32::from(self.quantum) * 2
    }
    /// Whether the visited physical callback has complete staged coverage.
    /// This is not final host acceptance and grants no dispatch authority.
    pub fn has_complete_staged_callback(&self) -> bool {
        self.staged
            .as_ref()
            .is_some_and(|stage| stage.staged_frames == stage.expected_frames)
    }
    /// End at a proven transport hold, such as an audible Pause or a stream
    /// waiting for pages. A contiguous stage ending exactly there may omit the
    /// unvisited suffix. A fully staged monitoring path keeps its extent.
    /// Final host acceptance is still required.
    pub fn end_staging_at_rendered_prefix(&mut self, frame: usize) {
        if let Some(stage) = self.staged.as_mut()
            && frame != 0
            && stage.staged_frames == frame
            && frame < stage.expected_frames
        {
            stage.expected_frames = frame;
        }
    }
    /// Current epoch revocation, including a worker/control request that the
    /// callback has not yet visited. This read grants no future liveness.
    pub fn is_revoked(&self) -> bool {
        self.revoked.is_revoked()
    }
    pub fn fault(&self) -> Option<Fault> {
        Revocation::decode_fault(self.revoked.callback_fault.load(Relaxed))
    }
    pub(super) fn fail<T>(&mut self, fault: Fault) -> Result<T, Fault> {
        self.revoked.record_callback_fault(fault);
        self.staged = None;
        self.accepted_manual = None;
        self.accepted_editor = None;
        Err(fault)
    }
    /// Declare one actual physical callback, even when the graph renders it in
    /// several ordered segments. The complete extent remains at most Q; graph
    /// chunking never grants a second dispatch or same-callback worker wait.
    pub fn begin_physical_callback(&mut self, frames: usize) -> Result<(), Fault> {
        if self.revoked.is_revoked() {
            return self.fail(Fault::Revoked);
        }
        if self.staged.is_some() || frames == 0 || frames > usize::from(self.quantum) {
            return self.fail(Fault::Protocol);
        }
        self.accepted_manual = None;
        self.accepted_editor = None;
        self.staged = Some(Stage {
            has_request: false,
            manual: None,
            editor: None,
            concealed: None,
            expected_frames: frames,
            staged_frames: 0,
            input_events: 0,
            legacy_input_events: false,
            output_events: 0,
            input_parameters: 0,
            output_parameters: 0,
        });
        Ok(())
    }
    /// Stage one whole physical callback. Output events use offsets within it.
    /// The first 2Q frames are explicit pipeline warmup silence, not an
    /// audible-start guarantee. Segment callers declare the full extent first.
    pub fn stage(
        &mut self,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
    ) -> Result<Events, Fault> {
        output.fill([0.0; 2]);
        self.begin_physical_callback(input.len())?;
        self.stage_segment(0, input, events, output)
    }
    /// Append the next contiguous graph segment. Input and output event offsets
    /// are relative to this segment. Replies may be consumed, but no request is
    /// published before the single final host-format acceptance in `finish`.
    /// Any gap, overlap, incomplete callback or refusal revokes the whole epoch;
    /// already consumed replies/foreign history are never rolled back/replayed.
    pub fn stage_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
    ) -> Result<Events, Fault> {
        let reply = self.stage_segment_with_parameters(frame_offset, input, events, &[], output)?;
        if !reply.parameters.as_slice().is_empty() {
            // Legacy event-only consumers cannot silently discard processor
            // feedback. Parameter-capable consumers use the complete reply.
            output.fill([0.0; 2]);
            return self.fail(Fault::InvalidParameters);
        }
        Ok(reply.events)
    }
    /// Stage input and collect processor feedback using the same physical
    /// callback/quantum acceptance as PCM and MIDI. At most 64 input and output
    /// points may cross one callback and at most 64 remain in either quantum.
    /// Malformed/overflowing input revokes before any staged request is sent.
    pub fn stage_segment_with_parameters(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
    ) -> Result<SegmentReply, Fault> {
        self.stage_segment_with_timing(
            frame_offset,
            input,
            events,
            parameters,
            output,
            SegmentTiming {
                playing: None,
                at: |_| None,
            },
        )
    }

    /// A partial quantum retains the exact first origin and any opted-in
    /// transport changes across physical callbacks. Copy a new origin only
    /// when the segment changes the most recent valid playing state.
    pub fn stage_segment_with_timing(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
    ) -> Result<SegmentReply, Fault> {
        self.stage_effect_segment(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            EffectSegmentOrigins::default(),
        )
    }

    /// Accumulate compensated origins through graph and physical callback
    /// partitions. Render the authored points once for the actual complete
    /// processing quantum. Ordinary parameter input has a separate path.
    pub fn stage_segment_with_authored_parameters(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        authored: AuthoredSegment<'_>,
    ) -> Result<SegmentReply, Fault> {
        self.stage_effect_segment(
            frame_offset,
            input,
            events,
            &[],
            output,
            timing,
            EffectSegmentOrigins {
                authored: Some(authored),
                manual: None,
            },
        )
    }

    /// Stage the actual effect's compensated clock alongside the normal
    /// parameter and PCM input. The copied marker is never sent to the SDK.
    #[allow(clippy::too_many_arguments)] // Borrowed PCM, events and exact clock views.
    pub fn stage_effect_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
    ) -> Result<SegmentReply, Fault> {
        let mut reply = SegmentReply {
            events: Events::default(),
            parameters: Parameters::default(),
            concealed_output: None,
        };
        reply.concealed_output = self.stage_effect_segment_with_reply(
            frame_offset,
            input,
            None,
            events,
            parameters,
            output,
            timing,
            origins,
            Some(&mut reply),
        )?;
        Ok(reply)
    }

    /// Stage native PCM for a consumer that observes reply metadata on control.
    /// Validate the same output bounds without copying unused events and points.
    #[allow(clippy::too_many_arguments)] // Same borrowed windows as ordinary effect staging.
    pub fn stage_effect_pcm_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        self.stage_effect_pcm_segment_with_sidechain(
            frame_offset,
            input,
            None,
            events,
            parameters,
            output,
            timing,
            origins,
        )
    }

    /// Stage native PCM alongside an optional aligned stereo sidechain key
    /// segment (for VST3 `kAux` sidechain inputs).
    #[allow(clippy::too_many_arguments)] // Same borrowed windows as ordinary effect staging.
    pub fn stage_effect_pcm_segment_with_sidechain(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        sidechain: Option<&[Frame]>,
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        self.stage_effect_segment_with_reply(
            frame_offset,
            input,
            sidechain,
            events,
            parameters,
            output,
            timing,
            origins,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Borrowed windows share one staging transaction.
    pub(super) fn stage_effect_segment_with_reply(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        sidechain: Option<&[Frame]>,
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
        reply: Option<&mut SegmentReply>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        output.fill([0.0; 2]);
        let result = self.stage_segment_inner(
            frame_offset,
            input,
            sidechain,
            events,
            parameters,
            output,
            timing,
            origins,
            reply,
        );
        if let Err(fault) = result {
            output.fill([0.0; 2]);
            return self.fail(fault);
        }
        result
    }

    #[allow(clippy::too_many_arguments)] // Same borrowed windows, with a single staging transaction.
    pub(super) fn stage_segment_inner(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        sidechain: Option<&[Frame]>,
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        mut timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
        mut reply: Option<&mut SegmentReply>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        let stage = self.staged.as_mut().ok_or(Fault::Protocol)?;
        let end = frame_offset
            .checked_add(input.len())
            .ok_or(Fault::Protocol)?;
        if frame_offset != stage.staged_frames
            || input.is_empty()
            || input.len() != output.len()
            || sidechain.is_some_and(|key| key.len() != input.len())
            || end > stage.expected_frames
        {
            return Err(Fault::Protocol);
        }
        if !input.iter().flatten().all(|x| x.is_finite())
            || sidechain.is_some_and(|key| !key.iter().flatten().all(|x| x.is_finite()))
        {
            return Err(Fault::InvalidSamples);
        }
        let input_event_count = stage
            .input_events
            .checked_add(events.len())
            .filter(|count| *count <= MAX_IDENTIFIED_INPUT_NOTES)
            .ok_or(Fault::InvalidEvents)?;
        let legacy_input_events =
            stage.legacy_input_events || events.iter().any(|event| event.note_id.is_none());
        if (input_event_count > MAX_EVENTS && legacy_input_events)
            || !events.iter().all(|e| e.valid(input.len()))
            || events.windows(2).any(|w| w[0].frame > w[1].frame)
        {
            return Err(Fault::InvalidEvents);
        }
        if parameters.len() > MAX_PARAMETER_POINTS - stage.input_parameters
            || !parameters
                .iter()
                .all(|point| parameters::valid(*point, input.len()))
            || parameters
                .windows(2)
                .any(|pair| pair[0].frame > pair[1].frame)
        {
            return Err(Fault::InvalidParameters);
        }
        let q = u64::from(self.quantum);
        let quantum_frames = usize::from(self.quantum);
        let initial_offset = (self.state.position % q) as usize;
        let EffectSegmentOrigins { authored, manual } = origins;
        if authored.is_some() && !parameters.is_empty() {
            return Err(Fault::InvalidParameters);
        }
        let program = authored
            .as_ref()
            .map(|segment| segment.program.quantum_program_key());
        let clock_epoch = authored
            .as_ref()
            .and_then(|segment| segment.program.authored_clock_epoch());
        if initial_offset != 0
            && self.authored_origins.program.is_some()
            && self.authored_origins.program != program
        {
            let retained = self
                .authored_origins
                .program
                .zip(authored.as_ref())
                .is_some_and(|(key, segment)| segment.program.contains_quantum_program(key));
            if !retained {
                return Err(Fault::InvalidParameters);
            }
        }
        let mut concealed_output = (initial_offset != 0 && self.state.output_gap)
            .then_some((0, input.len().min(quantum_frames - initial_offset)));
        let mut output_events = 0;
        let mut output_parameters = 0;
        let mut event_index = 0;
        let mut parameter_index = 0;
        let mut input_parameters = parameters.len();
        for (index, (source, destination)) in input.iter().zip(output).enumerate() {
            let position = self.state.position;
            let offset = (position % q) as usize;
            let sequence = position / q;
            let origin = authored
                .as_ref()
                .and_then(|segment| (segment.origin)(index));
            if offset == 0 {
                self.state.input = Block::empty(self.binding, self.quantum);
                self.state.input.sequence = sequence;
                self.state.input.first_frame = position;
                self.state.input_sidechain = sidechain.map(|_| [[0.0; 2]; MAX_FRAMES]);
                self.state.input_manual_origin = manual.and_then(|origin| origin(index));
                let editor_scope = self.revoked.editor_scope.load(Acquire);
                self.state.input_editor_scope = if editor_scope & 1 != 0 {
                    NonZeroU64::new(editor_scope >> 1)
                } else {
                    None
                };
                self.state.parameter_capacity = MAX_PARAMETER_POINTS;
                self.authored_origins.open(program, clock_epoch);
                self.state.input.timing = (timing.at)(index);
                if self.state.input.timing.is_some_and(|origin| {
                    !origin.is_valid()
                        || timing
                            .playing
                            .is_some_and(|playing| origin.playing != playing)
                }) {
                    return Err(Fault::Protocol);
                }
                if sequence >= self.first_reply_sequence {
                    stage_reply(
                        &mut self.replies,
                        &self.revoked,
                        self.binding,
                        self.quantum,
                        self.conceal_output_gap,
                        &mut self.state,
                        sequence - 2,
                    )?;
                    if self.state.output_gap {
                        stage.concealed = Some(DeadlineMiss {
                            reply_sequence: sequence - 2,
                            worker_phase: self.revoked.worker_progress(sequence - 2).0,
                        });
                        let start = concealed_output.map_or(index, |span| span.0);
                        concealed_output = Some((start, input.len().min(index + quantum_frames)));
                    }
                    if let Some(manual) = self.state.output.manual {
                        // Extent is at most one quantum, so only one reply
                        // start can fall in the complete physical callback.
                        stage.manual = Some(AcceptedManualParameter {
                            binding: self.binding,
                            sequence: self.state.output.block.sequence,
                            first_frame: self.state.output.block.first_frame,
                            callback_offset: (frame_offset + index) as u16,
                            input: manual.input,
                            origin: manual.origin,
                        });
                    }
                    if let Some(editor) = self.state.output.editor {
                        stage.editor = Some(AcceptedEditorQuantum {
                            binding: self.binding,
                            sequence: self.state.output.block.sequence,
                            first_frame: self.state.output.block.first_frame,
                            callback_offset: (frame_offset + index) as u16,
                            scope: editor.scope,
                            performs: editor.facts.performs,
                            restorations: editor.facts.restorations,
                            lost_origin: editor.facts.lost_origin,
                            origin: editor.origin,
                        });
                    }
                }
            } else if origin.is_some()
                && (self
                    .authored_origins
                    .switch_clock(offset, program, clock_epoch)?
                    || self.authored_origins.frames[offset - 1].is_none())
            {
                // A new clock region or resumed history gets its own SDK call.
                // Its first parameter point must not interpolate backward into
                // the preceding region.
                let change = (timing.at)(index)
                    .filter(|origin| origin.playing)
                    .ok_or(Fault::Protocol)?;
                self.state.input.add_timing_change(offset as u16, change)?;
            } else if index == 0
                && let Some(playing) = timing.playing
                && let Some((prior_frame, prior)) = self.state.input.latest_timing_entry()
            {
                if prior.playing != playing {
                    let change = (timing.at)(index)
                        .filter(|origin| origin.playing == playing)
                        .ok_or(Fault::Protocol)?;
                    self.state.input.add_timing_change(offset as u16, change)?;
                } else if playing && let Some(change) = (timing.at)(index) {
                    if !change.playing {
                        return Err(Fault::Protocol);
                    }
                    let expected = prior
                        .arrangement_frame
                        .checked_add(i64::from(offset as u16 - prior_frame));
                    if expected != Some(change.arrangement_frame)
                        || prior.tempo_micro_bpm != change.tempo_micro_bpm
                    {
                        self.state.input.add_timing_change(offset as u16, change)?;
                    }
                }
            }
            self.state.input.pcm[offset] = *source;
            if let Some(key) = sidechain {
                let buf = self
                    .state
                    .input_sidechain
                    .get_or_insert([[0.0; 2]; MAX_FRAMES]);
                buf[offset] = key[index];
            }
            while let Some(event) = events
                .get(event_index)
                .filter(|e| usize::from(e.frame) == index)
            {
                let mut event = *event;
                event.frame = offset as u16;
                self.state.input.events.push(event)?;
                event_index += 1;
            }
            while let Some(point) = parameters
                .get(parameter_index)
                .filter(|point| usize::from(point.frame) == index)
            {
                self.state.input.parameters.push(ParameterPoint {
                    frame: offset as u16,
                    ..*point
                })?;
                parameter_index += 1;
            }
            if sequence >= 2 {
                *destination = self.state.output.block.pcm[offset];
                let out_events = self.state.output.block.events.as_slice();
                while let Some(event) = out_events
                    .get(self.state.output_event_cursor)
                    .filter(|e| usize::from(e.frame) == offset)
                {
                    self.state.output_event_cursor += 1;
                    output_events += 1;
                    if let Some(reply) = reply.as_deref_mut() {
                        let mut event = *event;
                        event.frame = index as u16;
                        reply.events.push(event)?;
                    }
                }
                let out_parameters = self.state.output.block.parameters.as_slice();
                while let Some(point) = out_parameters
                    .get(self.state.output_parameter_cursor)
                    .filter(|point| usize::from(point.frame) == offset)
                {
                    self.state.output_parameter_cursor += 1;
                    output_parameters += 1;
                    if let Some(reply) = reply.as_deref_mut() {
                        reply.parameters.push(ParameterPoint {
                            frame: index as u16,
                            ..*point
                        })?;
                    }
                }
            }
            if authored.is_some() {
                self.authored_origins.frames[offset] = origin;
            }
            self.state.position = position.checked_add(1).ok_or(Fault::Exhausted)?;
            if offset + 1 == quantum_frames {
                // Absence is also frozen at the opening frame. A programme
                // first supplied mid-quantum starts at the next boundary;
                // it cannot replace this quantum's ordinary parameter input.
                if let Some(authored) = &authored
                    && self.authored_origins.program.is_some()
                {
                    let points = self.authored_origins.render(
                        authored,
                        usize::from(self.quantum),
                        &self.state.input.timing_changes,
                    )?;
                    input_parameters += points.as_slice().len();
                    if input_parameters > MAX_PARAMETER_POINTS - stage.input_parameters {
                        return Err(Fault::InvalidParameters);
                    }
                    self.state.input.parameters = points;
                }
                self.staged_request = InputRequest {
                    block: self.state.input,
                    sidechain: self.state.input_sidechain.take(),
                    parameter_capacity: self.state.parameter_capacity,
                    authored_program: self.authored_origins.program,
                    manual_origin: self.state.input_manual_origin,
                    editor_scope: self.state.input_editor_scope,
                };
                stage.has_request = true;
            }
        }
        if output_events > MAX_EVENTS - stage.output_events {
            return Err(Fault::InvalidEvents);
        }
        if output_parameters > MAX_PARAMETER_POINTS - stage.output_parameters {
            return Err(Fault::InvalidParameters);
        }
        stage.input_events = input_event_count;
        stage.legacy_input_events = legacy_input_events;
        stage.output_events += output_events;
        stage.input_parameters += input_parameters;
        stage.output_parameters += output_parameters;
        stage.staged_frames = end;
        Ok(concealed_output)
    }
    /// Call only after final host-format validation. Refusal terminates this
    /// epoch; consumed replies and foreign DSP are never rolled back/replayed.
    pub fn finish(&mut self, host_accepted: bool) -> Result<(), Fault> {
        if self.committed_quantum.is_none() {
            // Mark the first transition before any state can be accepted.
            self.revoked
                .quantum_serial
                .store(1, std::sync::atomic::Ordering::SeqCst);
        }
        if !host_accepted {
            return self.fail(Fault::Protocol);
        }
        if self.revoked.is_revoked() {
            return self.fail(Fault::Revoked);
        }
        let Some(stage) = self.staged.take() else {
            return self.fail(Fault::Protocol);
        };
        if stage.staged_frames != stage.expected_frames {
            return self.fail(Fault::Protocol);
        }
        let remaining = MAX_PARAMETER_POINTS - stage.input_parameters;
        if stage.has_request {
            self.staged_request.parameter_capacity =
                self.staged_request.parameter_capacity.min(remaining);
            if self.requests.push(self.staged_request).is_err() {
                return self.fail(Fault::Saturated);
            }
            self.revoked.wake_worker();
        }
        if !self.state.position.is_multiple_of(u64::from(self.quantum)) {
            self.state.parameter_capacity = self.state.parameter_capacity.min(remaining);
        } else {
            self.state.parameter_capacity = MAX_PARAMETER_POINTS;
        }
        self.committed_position = self.state.position;
        self.accepted_manual = stage.manual;
        self.accepted_editor = stage.editor;
        let quantum = if self.state.position.is_multiple_of(u64::from(self.quantum)) {
            CommittedQuantumState::Closed
        } else if let Some(key) = self.authored_origins.program {
            CommittedQuantumState::OpenProgram(key)
        } else {
            CommittedQuantumState::OpenUnprogrammed
        };
        // Identical observations carry no additional receipt or authority.
        if self.committed_quantum != Some(quantum) {
            self.revoked.publish_quantum(quantum);
            self.committed_quantum = Some(quantum);
        }
        if let Some(miss) = stage.concealed {
            self.revoked.publish_concealment(miss);
        }
        Ok(())
    }

    /// Consume the exact accepted point once, before the next physical
    /// callback. An unaccepted or revoked endpoint never exposes a marker.
    pub fn take_accepted_manual_parameter(&mut self) -> Option<AcceptedManualParameter> {
        self.accepted_manual
            .take()
            .filter(|_| !self.revoked.is_revoked())
    }

    /// Consume once before the next physical callback. Rejection, revocation
    /// and ending/replacing the control scope never return an old marker.
    pub fn take_accepted_editor_quantum(&mut self) -> Option<AcceptedEditorQuantum> {
        self.accepted_editor.take().filter(|marker| {
            self.revoked.editor_scope.load(Acquire) == ((marker.scope.get() << 1) | 1)
                && !self.revoked.is_revoked()
        })
    }
}

pub(super) fn stage_reply(
    replies: &mut Consumer<OutputReply>,
    revoked: &Revocation,
    binding: Binding,
    quantum: u16,
    conceal_output_gap: bool,
    state: &mut State,
    expected: u64,
) -> Result<(), Fault> {
    let validate = |reply: &OutputReply, sequence| {
        if conceal_output_gap && !reply.block.events.as_slice().is_empty() {
            return Err(Fault::InvalidEvents);
        }
        reply.valid(binding, quantum, sequence)
    };
    if state.next_output_reply > expected {
        return Err(Fault::Protocol);
    }
    // At most six stale replies and the current reply can be consumed in one
    // callback. Validate every sequence before discarding its PCM and facts.
    for _ in 0..=MAX_CONCEALED_OUTPUT_GAPS {
        match replies.pop() {
            Ok(reply) => {
                validate(&reply, state.next_output_reply)?;
                let sequence = state.next_output_reply;
                state.next_output_reply = sequence.checked_add(1).ok_or(Fault::Exhausted)?;
                if sequence == expected {
                    state.output = reply;
                    state.output_event_cursor = 0;
                    state.output_parameter_cursor = 0;
                    state.output_gap = false;
                    state.consecutive_output_gaps = 0;
                    return Ok(());
                }
            }
            Err(_)
                if conceal_output_gap
                    && state.consecutive_output_gaps < MAX_CONCEALED_OUTPUT_GAPS =>
            {
                state.output = OutputReply {
                    block: Block::empty(binding, quantum),
                    manual: None,
                    editor: None,
                };
                state.output_event_cursor = 0;
                state.output_parameter_cursor = 0;
                state.output_gap = true;
                state.consecutive_output_gaps += 1;
                return Ok(());
            }
            Err(_) => {
                revoked.record_deadline_miss(expected);
                return Err(Fault::MissingOutput);
            }
        }
    }
    Err(Fault::Protocol)
}
