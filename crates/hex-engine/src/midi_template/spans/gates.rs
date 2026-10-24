//! Resource-free compiled MIDI gate/index custody and the shared bounded span reporter.
//! Capturing never retains note banks, analytic contexts, native configurations or factories.
use super::*;

#[derive(Debug)]
pub(crate) struct PreparedMidiGateTemplate {
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    sample_rate: u32,
    admitted_max_micro_bpm: u64,
    first_pass: u64,
    intro_iteration: u64,
    repeat_iteration: u64,
    period: Option<u64>,
    release_raw: u64,
    intro: Vec<VoiceSpan>,
    repeats: Vec<VoiceSpan>,
    intro_nodes: Vec<IntervalNode>,
    repeat_nodes: Vec<IntervalNode>,
    intro_root: Option<usize>,
    repeat_root: Option<usize>,
    release_frames: u64,
}
pub(super) struct SpanQueryView<'a> {
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    sample_rate: u32,
    admitted_max_micro_bpm: u64,
    first_pass: u64,
    intro_iteration: u64,
    repeat_iteration: u64,
    period: Option<u64>,
    release_raw: u64,
    intro: &'a [VoiceSpan],
    repeats: &'a [VoiceSpan],
    intro_nodes: &'a [IntervalNode],
    repeat_nodes: &'a [IntervalNode],
    intro_root: Option<usize>,
    repeat_root: Option<usize>,
    release_frames: u64,
    bank: Option<&'a crate::midi_note_bank::PreparedMidiNoteBank>,
    event_source: bool,
}
impl PreparedMidiInstrumentTemplate {
    pub(crate) fn gate_capture_quote(&self) -> Option<usize> {
        std::mem::size_of::<PreparedMidiGateTemplate>()
            .checked_add(
                self.intro
                    .len()
                    .checked_mul(std::mem::size_of::<VoiceSpan>())?,
            )?
            .checked_add(
                self.repeats
                    .len()
                    .checked_mul(std::mem::size_of::<VoiceSpan>())?,
            )?
            .checked_add(
                self.intro_nodes
                    .len()
                    .checked_mul(std::mem::size_of::<IntervalNode>())?,
            )?
            .checked_add(
                self.repeat_nodes
                    .len()
                    .checked_mul(std::mem::size_of::<IntervalNode>())?,
            )
    }
    pub(crate) fn capture_gate_template(
        &self,
    ) -> Result<PreparedMidiGateTemplate, MidiTemplateError> {
        Ok(PreparedMidiGateTemplate {
            track: self.track,
            clip: self.clip,
            sample_rate: self.sample_rate,
            admitted_max_micro_bpm: self.admitted_max_micro_bpm,
            first_pass: self.first_pass,
            intro_iteration: self.intro_iteration,
            repeat_iteration: self.repeat_iteration,
            period: self.period,
            release_raw: self.release_raw,
            intro_root: self.intro_root,
            repeat_root: self.repeat_root,
            release_frames: self.release_frames,
            intro: copy(&self.intro)?,
            repeats: copy(&self.repeats)?,
            intro_nodes: copy(&self.intro_nodes)?,
            repeat_nodes: copy(&self.repeat_nodes)?,
        })
    }
    pub(super) fn query_view(&self) -> SpanQueryView<'_> {
        SpanQueryView {
            track: self.track,
            clip: self.clip,
            sample_rate: self.sample_rate,
            admitted_max_micro_bpm: self.admitted_max_micro_bpm,
            first_pass: self.first_pass,
            intro_iteration: self.intro_iteration,
            repeat_iteration: self.repeat_iteration,
            period: self.period,
            release_raw: self.release_raw,
            intro_root: self.intro_root,
            repeat_root: self.repeat_root,
            release_frames: self.release_frames,
            intro: &self.intro,
            repeats: &self.repeats,
            intro_nodes: &self.intro_nodes,
            repeat_nodes: &self.repeat_nodes,
            bank: Some(&self.bank),
            event_source: self.event_source,
        }
    }
}
impl PreparedMidiGateTemplate {
    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        let bytes = std::mem::size_of::<Self>()
            .checked_add(
                self.intro
                    .capacity()
                    .checked_mul(std::mem::size_of::<VoiceSpan>())?,
            )?
            .checked_add(
                self.repeats
                    .capacity()
                    .checked_mul(std::mem::size_of::<VoiceSpan>())?,
            )?
            .checked_add(
                self.intro_nodes
                    .capacity()
                    .checked_mul(std::mem::size_of::<IntervalNode>())?,
            )?
            .checked_add(
                self.repeat_nodes
                    .capacity()
                    .checked_mul(std::mem::size_of::<IntervalNode>())?,
            )?;
        Some(bytes)
    }
    pub(crate) fn query_with_origin<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        let view = SpanQueryView {
            track: self.track,
            clip: self.clip,
            sample_rate: self.sample_rate,
            admitted_max_micro_bpm: self.admitted_max_micro_bpm,
            first_pass: self.first_pass,
            intro_iteration: self.intro_iteration,
            repeat_iteration: self.repeat_iteration,
            period: self.period,
            release_raw: self.release_raw,
            intro_root: self.intro_root,
            repeat_root: self.repeat_root,
            release_frames: self.release_frames,
            intro: &self.intro,
            repeats: &self.repeats,
            intro_nodes: &self.intro_nodes,
            repeat_nodes: &self.repeat_nodes,
            bank: None,
            event_source: true,
        };
        buffer.len = 0;
        let result = view.query_inner(origin, timing, start_frame, end_frame, buffer, None);
        if result.is_err() {
            buffer.len = 0;
        }
        result
    }
}
fn copy<T: Copy>(source: &[T]) -> Result<Vec<T>, MidiTemplateError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| MidiTemplateError::Allocation)?;
    result.extend_from_slice(source);
    Ok(result)
}
impl SpanQueryView<'_> {
    pub(super) fn query_inner<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiSpanBuffer,
        onset_floor_exclusive: Option<BeatTime>,
    ) -> Result<(), MidiTemplateError> {
        if start_frame > end_frame {
            return Err(MidiTemplateError::InvalidFrameRange);
        }
        if end_frame - start_frame > MIDI_TEMPLATE_QUERY_FRAMES {
            return Err(MidiTemplateError::QueryTooWide);
        }
        if timing.sample_rate() != self.sample_rate {
            return Err(MidiTemplateError::TempoRateMismatch);
        }
        if timing.max_micro_bpm() > self.admitted_max_micro_bpm {
            return Err(MidiTemplateError::TempoMapTooFast);
        }
        if start_frame == end_frame {
            return Ok(());
        }
        let lower = timing
            .beat_lower_bound_for_frame(start_frame)
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        let upper = timing
            .beat_lower_bound_for_frame(end_frame)
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        if lower == TempoMap::BEAT_RANGE_SENTINEL {
            return Err(MidiTemplateError::BeatDomainExhausted);
        }
        let Some((relative_start, relative_end)) = origin.query_bounds(lower, upper)? else {
            return Ok(());
        };
        let low = relative_start;
        let high = relative_end;
        let mut context = QueryContext {
            origin,
            timing,
            start_frame,
            end_frame,
            buffer,
            visits: 0,
            onset_floor_exclusive: onset_floor_exclusive.map(BeatTime::raw),
        };
        self.append_intervals(
            self.intro,
            self.intro_nodes,
            self.intro_root,
            0,
            low,
            high,
            self.intro_iteration,
            &mut context,
        )?;
        if let Some(period) = self.period {
            self.append_periods(low, high, period, &mut context)?;
        }
        context.buffer.slots[..context.buffer.len].sort_unstable_by_key(|span| {
            span.map_or((u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX), |span| {
                (
                    span.start_frame,
                    span.track.value(),
                    span.clip.value(),
                    span.note.value(),
                    span.iteration,
                )
            })
        });
        Ok(())
    }

    fn append_periods<T: MidiTiming + ?Sized>(
        &self,
        low: u128,
        high: u128,
        period: u64,
        context: &mut QueryContext<'_, T>,
    ) -> Result<(), MidiTemplateError> {
        if self.repeats.is_empty() || high <= u128::from(self.first_pass) {
            return Ok(());
        }
        let period = u128::from(period);
        let first_pass = u128::from(self.first_pass);
        let earliest_base = low.saturating_sub(period + u128::from(self.release_raw));
        let first_iteration = if earliest_base <= first_pass {
            0
        } else {
            (earliest_base - first_pass) / period
        };
        let last_iteration = (high - 1 - first_pass) / period;
        if first_iteration > last_iteration {
            return Ok(());
        }
        let count = last_iteration - first_iteration + 1;
        if count > MIDI_SPAN_MAX_PERIOD_WINDOWS {
            return Err(MidiTemplateError::SpanPeriodWorkBoundExceeded);
        }
        let mut iteration = first_iteration;
        while iteration <= last_iteration {
            let base = first_pass
                .checked_add(
                    iteration
                        .checked_mul(period)
                        .ok_or(MidiTemplateError::BeatDomainExhausted)?,
                )
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            let repeat_number = u64::try_from(iteration)
                .map_err(|_| MidiTemplateError::BeatDomainExhausted)?
                .checked_add(self.repeat_iteration)
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            self.append_intervals(
                self.repeats,
                self.repeat_nodes,
                self.repeat_root,
                base,
                low,
                high,
                repeat_number,
                context,
            )?;
            iteration = iteration
                .checked_add(1)
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn append_intervals<T: MidiTiming + ?Sized>(
        &self,
        spans: &[VoiceSpan],
        nodes: &[IntervalNode],
        root: Option<usize>,
        base: u128,
        low: u128,
        high: u128,
        iteration: u64,
        context: &mut QueryContext<'_, T>,
    ) -> Result<(), MidiTemplateError> {
        let Some(root) = root else {
            return Ok(());
        };
        let mut stack = [0_usize; 64];
        let mut stack_len = 1;
        stack[0] = root;
        while stack_len > 0 {
            stack_len -= 1;
            let node_index = stack[stack_len];
            context.visits += 1;
            if context.visits > MIDI_SPAN_MAX_QUERY_VISITS {
                return Err(MidiTemplateError::SpanVisitBoundExceeded);
            }
            let node = nodes[node_index];
            let span = spans[node.span];
            let start = base
                .checked_add(u128::from(span.start))
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            let end = base
                .checked_add(span.raw_end)
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            if start < high && end > low {
                self.push_span(span, base, iteration, context)?;
            }
            // The stored min-start/max-end aggregates prune the interval
            // reporter on both sides of the raw query range. The admission
            // proof bounds raw candidates before exact frame filtering.
            let local_low = low.saturating_sub(base);
            let local_high = high.saturating_sub(base);
            let may_intersect = |child: usize| {
                u128::from(nodes[child].min_start) < local_high && nodes[child].max_end > local_low
            };
            if let Some(left) = node.left.filter(|left| may_intersect(*left)) {
                if stack_len >= stack.len() {
                    return Err(MidiTemplateError::SpanVisitBoundExceeded);
                }
                stack[stack_len] = left;
                stack_len += 1;
            }
            if let Some(right) = node.right.filter(|right| may_intersect(*right)) {
                if stack_len >= stack.len() {
                    return Err(MidiTemplateError::SpanVisitBoundExceeded);
                }
                stack[stack_len] = right;
                stack_len += 1;
            }
        }
        Ok(())
    }

    fn push_span<T: MidiTiming + ?Sized>(
        &self,
        span: VoiceSpan,
        base: u128,
        iteration: u64,
        context: &mut QueryContext<'_, T>,
    ) -> Result<(), MidiTemplateError> {
        let start_relative = base
            .checked_add(u128::from(span.start))
            .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        // Reject pre-cue onsets before projection or tempo conversion. Equality
        // is included, matching the existing global handoff-floor semantics.
        let Some(start_raw) = context.origin.project(start_relative)? else {
            return Ok(());
        };
        if context
            .onset_floor_exclusive
            .is_some_and(|floor| start_raw < floor)
        {
            return Ok(());
        }
        let gate_relative = base
            .checked_add(u128::from(span.gate_end))
            .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        let Some(gate_raw) = context.origin.project(gate_relative)? else {
            return Err(MidiTemplateError::InvalidSource(
                "midi gate precedes its origin",
            ));
        };
        let onset = context
            .timing
            .beat_to_frame(BeatTime::from_raw(start_raw))
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        let gate_end = context
            .timing
            .beat_to_frame(BeatTime::from_raw(gate_raw))
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        let voice = if self.event_source {
            None
        } else {
            Some(
                self.bank
                    .and_then(|bank| bank.sampler(span.sampler_index))
                    .ok_or(MidiTemplateError::InvalidSource(
                        "midi note sampler index is invalid",
                    ))?,
            )
        };
        let end = gate_end
            .checked_add(self.release_frames)
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        if onset >= context.end_frame || end <= context.start_frame {
            return Ok(());
        }
        if context.buffer.len >= MIDI_SPAN_BUFFER_CAPACITY {
            return Err(MidiTemplateError::TooManySpans);
        }
        context.buffer.slots[context.buffer.len] = Some(MidiRenderSpan {
            start_frame: onset,
            gate_end_frame: gate_end,
            end_frame: end,
            onset_beat: BeatTime::from_raw(start_raw),
            gate_end_beat: BeatTime::from_raw(gate_raw),
            pitch: span.pitch,
            channel: span.channel,
            velocity: span.velocity,
            track: self.track,
            clip: self.clip,
            note: span.note,
            iteration,
            voice,
        });
        context.buffer.len += 1;
        Ok(())
    }
}
