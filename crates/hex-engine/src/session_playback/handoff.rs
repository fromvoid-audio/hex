use super::*;
use crate::MidiTiming;

impl SessionPlayback {
    pub(super) fn inherit_state(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.revision.arrangement().sample_rate()
            != previous.revision.arrangement().sample_rate()
        {
            return Err(SessionPlaybackError::RevisionSampleRateMismatch);
        }
        for (new_index, new_route) in self.routes.iter().enumerate() {
            if let Some(old_index) = previous
                .routes
                .iter()
                .position(|old_route| old_route.id == new_route.id)
            {
                let candidate = self
                    .device_chains
                    .get(new_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let old = previous
                    .device_chains
                    .get(old_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                if !candidate.can_inherit_from(old) {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
            }
        }

        let old_base = previous.revision.session().timing();
        let new_base = self.revision.session().timing();
        let same_base = old_base.same_map(new_base);
        let same_periodic = previous.periodic_tempo == self.periodic_tempo;
        let old_clock = previous.timing_view();
        let old_beat = old_clock
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let new_base_view = base_timing_for(self.revision.session(), self.periodic_tempo);
        let new_beat = new_base_view
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let timing_changed = !same_base || !same_periodic;
        let candidate_anchor = if timing_changed {
            Some(SessionClockAnchor {
                cut_frame: previous.performance_frame,
                old_beat,
                new_beat,
            })
        } else {
            previous.anchor
        };
        let candidate_floor = if timing_changed {
            Some(old_beat)
        } else {
            previous.historical_floor
        };
        let candidate_timing = candidate_anchor.map_or_else(
            || SessionTimingView::from_base(new_base_view),
            |anchor| {
                SessionTimingView::Anchored(AnchoredSessionTempoView::new(new_base_view, anchor))
            },
        );

        self.route_scratch.copy_from_slice(&self.routes);
        let mut report = SessionHandoffReport {
            cancelled_pending: [None; MAX_TRACKS],
            cancelled_pending_len: 0,
        };
        let mut source_changed = timing_changed;
        self.release_scratch.clear();
        let mut next_order = previous.release_order;
        let mut track_map = [usize::MAX; MAX_TRACKS];
        for (old_index, old_route) in previous.routes.iter().enumerate() {
            let Some(new_index) = self
                .routes
                .iter()
                .position(|route| route.id == old_route.id)
            else {
                if matches!(
                    old_route.owner,
                    RouteOwner::Session { .. } | RouteOwner::SessionAudio { .. }
                ) {
                    source_changed = true;
                } else if matches!(old_route.owner, RouteOwner::Arrangement) {
                    let old_track = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let mut failure = None;
                    old_track
                        .for_each_active_midi_span(previous.arrangement_frame, |span| {
                            let voice = MidiReleaseVoice::from_finite_span(
                                span.start,
                                span.gate_end,
                                span.end,
                                span.voice,
                                previous.arrangement_frame,
                                previous.performance_frame,
                            );
                            match voice {
                                Ok(Some(voice)) => {
                                    if let Err(error) = append_release(
                                        &mut self.release_scratch,
                                        self.release_capacity,
                                        &mut next_order,
                                        ReleaseEntry {
                                            track_index: usize::MAX,
                                            track_id: old_route.id,
                                            order: 0,
                                            mix: old_track.mix(),
                                            tail_group: None,
                                            continuation: None,
                                            voice,
                                        },
                                    ) {
                                        failure = Some(error);
                                    }
                                }
                                Ok(None) => {}
                                Err(_) => failure = Some(SessionPlaybackError::ArithmeticOverflow),
                            }
                        })
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(error) = failure {
                        return Err(error);
                    }
                }
                if old_route.pending.is_some()
                    && report.cancelled_pending_len < report.cancelled_pending.len()
                {
                    report.cancelled_pending[report.cancelled_pending_len] = Some(old_route.id);
                    report.cancelled_pending_len += 1;
                }
                continue;
            };
            let Some(route) = self.route_scratch.get_mut(new_index) else {
                return Err(SessionPlaybackError::UnknownTrack);
            };
            let Some(mapped_index) = track_map.get_mut(old_index) else {
                return Err(SessionPlaybackError::RevisionTrackMismatch);
            };
            *mapped_index = new_index;
            route.owner = match old_route.owner {
                RouteOwner::Arrangement => RouteOwner::Arrangement,
                RouteOwner::Silence => RouteOwner::Silence,
                RouteOwner::Session { clip, launch } => {
                    if let Some(next) = self.revision.session().midi_clip(clip) {
                        let old = previous
                            .revision
                            .session()
                            .midi_clip(clip)
                            .ok_or(SessionPlaybackError::UnknownClip)?;
                        if next.track != old_route.id {
                            source_changed = true;
                            RouteOwner::Silence
                        } else {
                            if !next.source_matches(old) {
                                source_changed = true;
                            }
                            RouteOwner::Session { clip, launch }
                        }
                    } else {
                        source_changed = true;
                        RouteOwner::Silence
                    }
                }
                RouteOwner::SessionAudio { clip, launch_frame } => {
                    if let Some(next) = self.revision.session().audio_clip(clip) {
                        let old = previous
                            .revision
                            .session()
                            .audio_clip(clip)
                            .ok_or(SessionPlaybackError::UnknownClip)?;
                        if next.track != old_route.id {
                            source_changed = true;
                            RouteOwner::Silence
                        } else {
                            if !next.source_matches(old) {
                                source_changed = true;
                            }
                            RouteOwner::SessionAudio { clip, launch_frame }
                        }
                    } else {
                        source_changed = true;
                        RouteOwner::Silence
                    }
                }
            };
            route.pending = old_route.pending;
            if let Some(mut pending) = route.pending {
                let valid = match pending.target {
                    RouteTarget::Arrangement | RouteTarget::Silence => true,
                    RouteTarget::Session(clip) => {
                        self.revision
                            .session()
                            .midi_clip(clip)
                            .is_some_and(|prepared| prepared.track == old_route.id)
                            || self
                                .revision
                                .session()
                                .audio_clip(clip)
                                .is_some_and(|prepared| prepared.track == old_route.id)
                    }
                };
                if !valid {
                    route.pending = None;
                    if report.cancelled_pending_len < report.cancelled_pending.len() {
                        report.cancelled_pending[report.cancelled_pending_len] = Some(old_route.id);
                        report.cancelled_pending_len += 1;
                    }
                } else {
                    pending.frame = if old_route
                        .pending
                        .is_some_and(|old| old.frame <= previous.performance_frame)
                    {
                        previous.performance_frame
                    } else {
                        candidate_timing
                            .beat_to_frame(pending.beat)
                            .map_err(|_| SessionPlaybackError::TempoConversion)?
                            .max(previous.performance_frame)
                    };
                    route.pending = Some(pending);
                }
            }
        }

        self.suppression_scratch.clear();
        for entry in &previous.releases {
            if entry.voice.end_frame() <= previous.performance_frame {
                continue;
            }
            if self.release_scratch.len() >= self.release_capacity {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let remapped_gate = if source_changed {
                remap_release_gate(previous, self, entry, candidate_timing)?
            } else {
                None
            };
            let voice = if source_changed && entry.continuation.is_some() {
                entry
                    .voice
                    .reconcile_at(previous.performance_frame, remapped_gate)
                    .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
            } else {
                Some(entry.voice)
            };
            let Some(voice) = voice else {
                continue;
            };
            let mut copied = *entry;
            copied.voice = voice;
            let old_index = previous
                .routes
                .get(entry.track_index)
                .filter(|route| route.id == entry.track_id)
                .map(|_| entry.track_index);
            let new_index = old_index
                .and_then(|index| track_map.get(index).copied())
                .filter(|index| *index != usize::MAX);
            if let Some(index) = new_index {
                copied.track_index = index;
            } else {
                copied.track_index = usize::MAX;
            }
            self.release_scratch.push(copied);
            if source_changed && let Some(continuation) = copied.continuation {
                if self.suppression_scratch.len() >= self.release_capacity {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
                self.suppression_scratch.push(MidiOccurrenceSuppression {
                    track: entry.track_id,
                    clip: continuation.key.clip,
                    note: continuation.key.note,
                    iteration: continuation.key.iteration,
                    until_frame: voice.end_frame(),
                });
            }
        }

        if source_changed {
            for (old_index, old_route) in previous.routes.iter().enumerate() {
                let RouteOwner::Session { clip, launch } = old_route.owner else {
                    continue;
                };
                let old_clip = previous
                    .revision
                    .session()
                    .midi_clip(clip)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                let end = previous
                    .performance_frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let old_template = old_clip.instrument_template();
                if let Some(floor) = previous.historical_floor {
                    old_template
                        .query_after_onset(
                            launch,
                            &old_clock,
                            previous.performance_frame,
                            end,
                            &mut self.spans,
                            floor,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                } else {
                    old_template
                        .query(
                            launch,
                            &old_clock,
                            previous.performance_frame,
                            end,
                            &mut self.spans,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                }
                for span in self.spans.spans().copied() {
                    let key = ContinuationKey {
                        clip: span.clip,
                        note: span.note,
                        iteration: span.iteration,
                    };
                    if is_suppressed(previous, old_route.id, key) {
                        continue;
                    }
                    let candidate_clip = self.revision.session().midi_clip(clip);
                    let candidate_occurrence = candidate_clip
                        .filter(|candidate| candidate.track == old_route.id)
                        .and_then(|candidate| {
                            candidate.occurrence(span.note, span.iteration, launch)
                        });
                    let matches = candidate_occurrence.is_some_and(|occurrence| {
                        occurrence.onset == span.onset_beat
                            && occurrence.gate_end == span.gate_end_beat
                            && occurrence.pitch == span.pitch
                            && occurrence.channel == span.channel
                            && occurrence.velocity == span.velocity
                    });
                    let mapped_gate = if matches && span.gate_end_frame > previous.performance_frame
                    {
                        Some(
                            candidate_timing
                                .beat_to_frame(span.gate_end_beat)
                                .map_err(|_| SessionPlaybackError::TempoConversion)?,
                        )
                    } else {
                        None
                    };
                    let Some(voice) = span
                        .reconcile_at(previous.performance_frame, mapped_gate)
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
                    else {
                        continue;
                    };
                    if self.release_scratch.len() >= self.release_capacity {
                        return Err(SessionPlaybackError::ReleaseBankFull);
                    }
                    let new_index = track_map
                        .get(old_index)
                        .copied()
                        .filter(|index| *index != usize::MAX);
                    let track_index = new_index.unwrap_or(usize::MAX);
                    let mix = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?
                        .mix();
                    let continuation = matches.then_some(SessionContinuation {
                        key,
                        onset_beat: span.onset_beat,
                        gate_end_beat: span.gate_end_beat,
                        pitch: span.pitch,
                        channel: span.channel,
                        velocity: span.velocity,
                    });
                    append_release(
                        &mut self.release_scratch,
                        self.release_capacity,
                        &mut next_order,
                        ReleaseEntry {
                            track_index,
                            track_id: old_route.id,
                            order: 0,
                            mix,
                            tail_group: None,
                            continuation,
                            voice,
                        },
                    )?;
                    if continuation.is_some() {
                        if self.suppression_scratch.len() >= self.release_capacity {
                            return Err(SessionPlaybackError::ReleaseBankFull);
                        }
                        self.suppression_scratch.push(MidiOccurrenceSuppression {
                            track: old_route.id,
                            clip: key.clip,
                            note: key.note,
                            iteration: key.iteration,
                            until_frame: voice.end_frame(),
                        });
                    }
                }
            }
        } else {
            self.suppression_scratch.extend(
                previous
                    .suppressions
                    .iter()
                    .copied()
                    .filter(|entry| entry.until_frame > previous.performance_frame),
            );
        }
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));
        self.suppression_scratch
            .sort_unstable_by_key(|entry| (entry.track, entry.clip, entry.note, entry.iteration));

        // Only tail groups still referenced by live copied releases cross a
        // publication boundary. Completed groups stay on `previous` so their
        // processor allocations are reclaimed when that graph retires on the
        // control side. This mask also makes completed slot numbers reusable in
        // the candidate without touching the old graph during preflight.
        let mut active_tail_groups = [false; SESSION_TAIL_GROUP_CAPACITY];
        for release in &self.release_scratch {
            let Some(group) = release.tail_group else {
                continue;
            };
            let group_index = usize::from(group);
            let Some(active) = active_tail_groups.get_mut(group_index) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            if previous
                .tail_groups
                .get(group_index)
                .is_none_or(Option::is_none)
            {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            *active = true;
        }
        if active_tail_groups
            .iter()
            .enumerate()
            .any(|(slot, active)| *active && self.tail_groups.get(slot).is_none_or(Option::is_some))
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }

        let mut old_track_tail_slots = [None; MAX_TRACKS];
        let mut tail_processor_count = 0_usize;
        let mut tail_runtime_bytes = 0_usize;
        for (slot, active) in active_tail_groups.iter().copied().enumerate() {
            if !active {
                continue;
            }
            let chain = previous
                .tail_groups
                .get(slot)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            tail_processor_count = tail_processor_count
                .checked_add(chain.stateful_count())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_runtime_bytes = tail_runtime_bytes
                .checked_add(chain.declared_runtime_bytes())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        }
        for release in &mut self.release_scratch {
            if release.tail_group.is_some() {
                continue;
            }
            let Some(old_index) = previous
                .routes
                .iter()
                .position(|route| route.id == release.track_id)
            else {
                continue;
            };
            let Some(old_chain) = previous.device_chains.get(old_index) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            if !old_chain.has_stateful_processors() {
                continue;
            }
            let already_assigned = old_track_tail_slots
                .get(old_index)
                .copied()
                .flatten()
                .is_some();
            let Some(slot) = old_track_tail_slots
                .get(old_index)
                .copied()
                .flatten()
                .or_else(|| {
                    (0..self.tail_groups.len()).find(|slot| {
                        self.tail_groups.get(*slot).is_some_and(Option::is_none)
                            && !active_tail_groups.get(*slot).copied().unwrap_or(true)
                            && !old_track_tail_slots.contains(&Some(*slot))
                    })
                })
            else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            };
            if let Some(old_slot) = old_track_tail_slots.get_mut(old_index) {
                *old_slot = Some(slot);
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            if !already_assigned {
                tail_processor_count = tail_processor_count
                    .checked_add(old_chain.stateful_count())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(old_chain.declared_runtime_bytes())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                    || tail_runtime_bytes
                        > hex_lattice::MAX_STATEFUL_PROCESSORS
                            * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
                {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
            }
            release.tail_group = u16::try_from(slot).ok();
            if release.tail_group.is_none() {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            release.track_index = usize::MAX;
        }
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));

        let inherited_floor = if source_changed {
            Some(old_beat)
        } else {
            candidate_floor
        };
        let visible_state_changed = self.route_scratch != previous.routes
            || previous.performance_frame != self.performance_frame
            || timing_changed;
        for (old_index, old_route) in previous.routes.iter().enumerate() {
            if let Some(new_index) = self
                .routes
                .iter()
                .position(|new_route| new_route.id == old_route.id)
            {
                if let (Some(candidate), Some(old)) = (
                    self.device_chains.get_mut(new_index),
                    previous.device_chains.get(old_index),
                ) {
                    candidate.inherit_from(old);
                }
            }
        }
        for slot in 0..previous.tail_groups.len() {
            if active_tail_groups.get(slot).copied().unwrap_or(false)
                && previous.tail_groups[slot].is_some()
                && let (Some(candidate), Some(previous_group)) = (
                    self.tail_groups.get_mut(slot),
                    previous.tail_groups.get_mut(slot),
                )
            {
                std::mem::swap(candidate, previous_group);
                if let (Some(candidate_mix), Some(previous_mix)) = (
                    self.tail_mixes.get_mut(slot),
                    previous.tail_mixes.get(slot).copied(),
                ) {
                    *candidate_mix = previous_mix;
                }
            }
        }
        for (old_index, slot) in old_track_tail_slots.iter().copied().enumerate() {
            if let Some(slot) = slot
                && let (Some(candidate), Some(old_chain)) = (
                    self.tail_groups.get_mut(slot),
                    previous.device_chains.get_mut(old_index),
                )
                && candidate.is_none()
            {
                *candidate = Some(std::mem::replace(
                    old_chain,
                    hex_lattice::NativeRuntimeChain::empty(),
                ));
                if let (Some(tail_mix), Some(old_track)) = (
                    self.tail_mixes.get_mut(slot),
                    previous.revision.arrangement().tracks().get(old_index),
                ) {
                    *tail_mix = old_track.mix();
                }
            }
        }
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        self.release_order = next_order;
        self.release_order_scratch = next_order;
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.performance_frame = previous.performance_frame;
        self.arrangement_frame = previous.arrangement_frame;
        self.playing = previous.playing;
        self.anchor = candidate_anchor;
        self.historical_floor = inherited_floor;
        self.route_state_generation = if visible_state_changed {
            previous.route_state_generation.wrapping_add(1)
        } else {
            previous.route_state_generation
        };
        Ok(report)
    }
}

fn is_suppressed(playback: &SessionPlayback, track: TrackId, key: ContinuationKey) -> bool {
    playback
        .suppressions
        .binary_search_by(|entry| {
            (entry.track, entry.clip, entry.note, entry.iteration).cmp(&(
                track,
                key.clip,
                key.note,
                key.iteration,
            ))
        })
        .ok()
        .and_then(|index| playback.suppressions.get(index))
        .is_some_and(|entry| entry.until_frame > playback.performance_frame)
}

fn remap_release_gate(
    previous: &SessionPlayback,
    candidate: &SessionPlayback,
    entry: &ReleaseEntry,
    timing: impl MidiTiming,
) -> Result<Option<u64>, SessionPlaybackError> {
    let Some(continuation) = entry.continuation else {
        return Ok(None);
    };
    if entry.voice.gate_end_frame() <= previous.performance_frame {
        return Ok(None);
    }
    let key = continuation.key;
    let Some((_, RouteOwner::Session { clip, launch })) = previous
        .routes
        .iter()
        .enumerate()
        .find(|(_, route)| route.id == entry.track_id)
        .map(|(index, route)| (index, route.owner))
    else {
        return Ok(None);
    };
    if clip != key.clip {
        return Ok(None);
    }
    let Some(next) = candidate.revision.session().midi_clip(clip) else {
        return Ok(None);
    };
    if next.track != entry.track_id {
        return Ok(None);
    }
    let Some(occurrence) = next.occurrence(key.note, key.iteration, launch) else {
        return Ok(None);
    };
    let matches = occurrence.onset == continuation.onset_beat
        && occurrence.gate_end == continuation.gate_end_beat
        && occurrence.pitch == continuation.pitch
        && occurrence.channel == continuation.channel
        && occurrence.velocity == continuation.velocity;
    if !matches {
        return Ok(None);
    }
    timing
        .beat_to_frame(occurrence.gate_end)
        .map(Some)
        .map_err(|_| SessionPlaybackError::TempoConversion)
}
