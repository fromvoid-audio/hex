use crate::{
    Invalid,
    live_input::{PreparedTypingArm, TypingApplication, TypingPerformance},
};
impl crate::SessionPlayback {
    pub(super) fn refresh_typing_capacity(&mut self) -> Result<(), super::SessionPlaybackError> {
        let Some(input) = &self.typing else {
            return Ok(());
        };
        let owner = input.owner();
        let pending = self
            .routes
            .iter()
            .find(|route| route.id == input.arm().target().track)
            .is_some_and(|route| route.pending.is_some());
        let mut authored = input
            .authored_budget_at(self.performance_frame, pending)
            .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
        for (slot, tail) in self.tail_typing.iter().enumerate() {
            if let Some(tail) = tail.as_ref().filter(|tail| tail.owner() == owner) {
                authored = authored
                    .checked_add(tail.speculative_active_voices())
                    .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
                if let Some(runtime) = &self.tail_mutable_instruments[slot] {
                    authored = authored
                        .checked_add(
                            runtime
                                .runtime
                                .proofs()
                                .filter(|proof| proof.quiet_deadline_frame > self.performance_frame)
                                .count(),
                        )
                        .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
                }
            }
        }
        self.typing
            .as_mut()
            .ok_or(super::SessionPlaybackError::UnknownTrack)?
            .refresh_live_limit(authored)
            .map_err(|error| super::SessionPlaybackError::MutableInstrumentDiagnostic(error.0))
    }
    pub(super) fn remember_typing_route_authored(
        &mut self,
        index: usize,
        frame: u64,
        speculative: bool,
    ) -> Result<(), super::SessionPlaybackError> {
        if self
            .typing
            .as_ref()
            .is_none_or(|input| self.routes[index].id != input.arm().target().track)
        {
            return Ok(());
        }
        let mut count = 0usize;
        let mut until = 0;
        if let Some(runtime) = &self.mutable_instruments[index] {
            let mut record = |deadline| -> Result<(), super::SessionPlaybackError> {
                if deadline > frame {
                    count = count
                        .checked_add(1)
                        .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
                    until = until.max(deadline);
                }
                Ok(())
            };
            if speculative {
                for proof in runtime.speculative_proofs() {
                    record(proof.quiet_deadline_frame)?;
                }
            } else {
                for proof in runtime.proofs() {
                    record(proof.quiet_deadline_frame)?;
                }
            }
        } else {
            for release in self.release_scratch.iter().filter(|release| {
                release.track_id == self.routes[index].id && release.tail_group.is_none()
            }) {
                if release.voice.end_frame() > frame {
                    count = count
                        .checked_add(1)
                        .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
                    until = until.max(release.voice.end_frame());
                }
            }
        }
        if let Some(input) = &mut self.typing {
            input.reserve_inherited_authored(count, until);
        }
        Ok(())
    }
    pub(super) fn typing_route_reservation(&self, index: usize) -> Option<usize> {
        let input = self.typing.as_ref().filter(|input| {
            self.routes
                .get(index)
                .is_some_and(|route| route.id == input.arm().target().track)
        })?;
        if input.arm().is_mutable()
            && self
                .mutable_instruments
                .get(index)?
                .as_ref()?
                .proofs()
                .any(|proof| proof.quiet_deadline_frame > self.performance_frame)
        {
            // Preserve the existing authored mutable route contract. Live
            // ownership alone must not impose this unrelated refusal.
            return None;
        }
        let (candidate, events) = input.authored_reservation();
        // Reserve both source peaks even for a future quantized boundary:
        // authored notes may start between this command and its application.
        let old_source = match self.routes[index].owner {
            super::RouteOwner::Arrangement => {
                candidate.checked_add(input.inherited_authored(self.performance_frame).0)?
            }
            super::RouteOwner::Session { clip, .. } => self
                .revision
                .session()
                .midi_clip(clip)?
                .instrument_template()
                .max_active_voices(),
            super::RouteOwner::Silence | super::RouteOwner::SessionAudio { .. } => 0,
        };
        let retained = if let Some(old) = self.mutable_instruments.get(index)?.as_ref() {
            old_source.max(
                old.proofs()
                    .filter(|proof| proof.quiet_deadline_frame > self.performance_frame)
                    .count(),
            )
        } else {
            old_source.checked_add(
                self.releases
                    .iter()
                    .filter(|release| {
                        release.track_id == input.arm().target().track
                            && release.voice.end_frame() > self.performance_frame
                    })
                    .count(),
            )?
        };
        let total = candidate.checked_add(retained)?;
        (input.can_reserve_authored(total)
            && (!input.arm().is_mutable() || events.checked_add(retained)? <= 256 - 64 - 32))
            .then_some(total)
    }
    pub(super) fn typing_route_is_author_free(&self, index: usize) -> bool {
        match self.routes[index].owner {
            super::RouteOwner::Arrangement => self.revision.arrangement().tracks()[index]
                .instrument
                .as_ref()
                .is_none_or(|instrument| instrument.spans.is_empty()),
            super::RouteOwner::Session { clip, .. } => self
                .revision
                .session()
                .midi_clip(clip)
                .is_some_and(|clip| clip.instrument_template().max_active_voices() == 0),
            super::RouteOwner::Silence | super::RouteOwner::SessionAudio { .. } => true,
        }
    }
    pub(super) fn preflight_typing_handoff(
        &mut self,
        previous: &Self,
    ) -> Result<Option<Option<hex_lattice::AnalyticContinuation>>, super::SessionPlaybackError>
    {
        let Some(source) = &previous.typing else {
            return Ok(None);
        };
        let Some(target) = &mut self.typing else {
            return Ok(None);
        };
        if source.owner() != target.owner() || source.epoch() != target.epoch() {
            return Ok(None);
        }
        let track = source.arm().target().track;
        let topology_matches = target.arm().topology_matches(source.arm())
            && (!source.arm().is_mutable()
                || previous
                    .revision
                    .mutable_track_retarget_topology_matches(&self.revision, track));
        if !topology_matches {
            return Ok(None);
        }
        if source.arm().is_mutable() {
            let old_index = previous
                .routes
                .iter()
                .position(|route| route.id == track)
                .ok_or(super::SessionPlaybackError::UnknownTrack)?;
            let old = previous.mutable_instruments[old_index]
                .as_ref()
                .ok_or(super::SessionPlaybackError::MutableInstrumentActive)?;
            old.preflight_live_publication_ids().map_err(|error| {
                super::SessionPlaybackError::MutableInstrumentDiagnostic(error.0)
            })?;
            let unchanged = match previous.routes[old_index].owner {
                super::RouteOwner::Arrangement => previous
                    .revision
                    .mutable_arrangement_track_matches(&self.revision, track),
                super::RouteOwner::Session { clip, .. } => previous
                    .revision
                    .session()
                    .midi_clip(clip)
                    .zip(self.revision.session().midi_clip(clip))
                    .is_some_and(|(old, new)| old.source_content_matches(new)),
                super::RouteOwner::Silence | super::RouteOwner::SessionAudio { .. } => true,
            } && previous
                .revision
                .session()
                .timing()
                .same_map(self.revision.session().timing())
                && previous.periodic_tempo == self.periodic_tempo;
            if !unchanged {
                // Conservative finite proof: every surviving old physical
                // occurrence can overlap the candidate's admitted peak. Their
                // original gates remain untouched. No callback allocation and
                // no invented open-gate deadline is needed for this bound.
                let inherited = old
                    .proofs()
                    .filter(|proof| proof.quiet_deadline_frame > previous.performance_frame)
                    .count();
                let (candidate, events) = target.authored_reservation();
                let combined = candidate
                    .checked_add(inherited)
                    .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
                if events
                    .checked_add(inherited)
                    .is_none_or(|events| events > 256 - 64 - 32)
                {
                    return Err(super::SessionPlaybackError::MutableInstrumentActive);
                }
                target.reserve_authored_voices(combined).map_err(|error| {
                    super::SessionPlaybackError::MutableInstrumentDiagnostic(error.0)
                })?;
                let until = old
                    .proofs()
                    .filter(|proof| proof.quiet_deadline_frame > previous.performance_frame)
                    .map(|proof| proof.quiet_deadline_frame)
                    .max()
                    .unwrap_or(0);
                target.reserve_inherited_authored(inherited, until);
            } else {
                let (inherited, until) = source.inherited_authored(previous.performance_frame);
                let (candidate, _) = target.authored_reservation();
                target
                    .reserve_authored_voices(
                        candidate
                            .checked_add(inherited)
                            .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?,
                    )
                    .map_err(|error| {
                        super::SessionPlaybackError::MutableInstrumentDiagnostic(error.0)
                    })?;
                target.reserve_inherited_authored(inherited, until);
            }
        } else {
            let old_index = previous
                .routes
                .iter()
                .position(|route| route.id == track)
                .ok_or(super::SessionPlaybackError::UnknownTrack)?;
            if matches!(
                previous.routes[old_index].owner,
                super::RouteOwner::Arrangement
            ) {
                let old_track = &previous.revision.arrangement().tracks()[old_index];
                let unchanged = self
                    .revision
                    .arrangement()
                    .tracks()
                    .iter()
                    .find(|row| row.canonical_track_id() == Some(track))
                    .is_some_and(|new| old_track.typing_arrangement_sources_match(new))
                    && previous
                        .revision
                        .session()
                        .timing()
                        .same_map(self.revision.session().timing())
                    && previous.periodic_tempo == self.periodic_tempo
                    && previous.revision.arrangement().loop_region()
                        == self.revision.arrangement().loop_region();
                let inherited = old_track.instrument.as_ref().map_or(0, |instrument| {
                    instrument
                        .spans
                        .iter()
                        .filter(|span| {
                            span.start < previous.arrangement_frame
                                && span.end > previous.arrangement_frame
                        })
                        .count()
                });
                let retained = previous
                    .releases
                    .iter()
                    .filter(|release| {
                        release.track_id == track
                            && release.voice.end_frame() > previous.performance_frame
                    })
                    .count();
                let (candidate, _) = target.authored_reservation();
                let combined = candidate
                    .checked_add(if unchanged {
                        source.inherited_authored(previous.performance_frame).0
                    } else {
                        inherited
                            .checked_add(retained)
                            .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?
                    })
                    .ok_or(super::SessionPlaybackError::ArithmeticOverflow)?;
                if !target.can_reserve_authored(combined) {
                    return Err(super::SessionPlaybackError::MutableInstrumentActive);
                }
                target.reserve_authored_voices(combined).map_err(|error| {
                    super::SessionPlaybackError::MutableInstrumentDiagnostic(error.0)
                })?;
            }
        }
        target
            .preflight_inherit(source, previous.performance_frame)
            .map(Some)
            .map_err(|error| super::SessionPlaybackError::MutableInstrumentDiagnostic(error.0))
    }
    /// Control-side allocation and exact-source admission. Installing the
    /// returned owner is separate so callback publication need not allocate.
    pub fn prepare_typing_input(
        &self,
        arm: PreparedTypingArm,
        epoch: u64,
    ) -> Result<Box<TypingPerformance>, Invalid> {
        self.prepare_typing_input_with_bridge(arm, epoch, 0)
    }
    pub fn prepare_typing_input_with_bridge(
        &self,
        arm: PreparedTypingArm,
        epoch: u64,
        bridge_bytes: usize,
    ) -> Result<Box<TypingPerformance>, Invalid> {
        if self.linear_arrangement_export
            || arm.sample_rate() != self.revision.arrangement().sample_rate()
            || !self
                .revision
                .arrangement()
                .tracks()
                .iter()
                .any(|track| arm.matches_track(track))
        {
            return Err(Invalid("typing arm does not match the playback revision"));
        }
        let mut input = TypingPerformance::new(arm, epoch)?;
        input.charge_bridge(bridge_bytes);
        let (voices, events) = self.typing_authored_reservation(input.arm())?;
        input.reserve_authored_voices(voices)?;
        input.set_authored_reservation(voices, events);
        if input.arm().is_mutable() && events > 256 - 64 - 32 {
            return Err(Invalid(
                "authored events leave no guaranteed typing release capacity",
            ));
        }
        let contexts = self
            .revision
            .arrangement()
            .shared_contexts()
            .payload_bytes()
            .checked_add(input.arm().shared_context_bytes())
            .ok_or(Invalid("typing context accounting overflow"))?;
        if contexts > crate::shared_analytic::MAX_SHARED_CONTEXT_BYTES_PER_REVISION {
            return Err(Invalid("typing complete shared context budget exceeded"));
        }
        let current = self
            .accounted_runtime_storage_bytes()
            .ok_or(Invalid("typing runtime accounting overflow"))?;
        let previous = self
            .typing
            .as_ref()
            .map_or(Some(0), |typing| typing.declared_storage_bytes())
            .ok_or(Invalid("typing retained accounting overflow"))?;
        let combined = current
            .checked_sub(previous)
            .and_then(|bytes| bytes.checked_add(input.declared_storage_bytes()?))
            .ok_or(Invalid("typing complete runtime accounting overflow"))?;
        if combined > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(Invalid(
                "typing complete runtime exceeds its storage budget",
            ));
        }
        input.admit_source(&self.revision);
        Ok(Box::new(input))
    }
    /// Move only; callers retain a refused/retired owner for control-side drop.
    pub fn install_typing_input(
        &mut self,
        mut input: Box<TypingPerformance>,
    ) -> Result<Option<Box<TypingPerformance>>, Box<TypingPerformance>> {
        if !input.source_matches(&self.revision)
            || self.linear_arrangement_export
            || input.arm().sample_rate() != self.revision.arrangement().sample_rate()
            || !self
                .revision
                .arrangement()
                .tracks()
                .iter()
                .any(|track| input.arm().matches_track(track))
            || self
                .typing
                .as_ref()
                .is_some_and(|old| old.active_voices() != 0)
        {
            return Err(input);
        }
        input.cut(self.typing_cut_through);
        Ok(self.typing.replace(input))
    }
    pub fn queue_typing_input(
        &mut self,
        ticket: u64,
        action: crate::typing_protocol::Action,
    ) -> Result<(), crate::typing_protocol::Refusal> {
        self.typing
            .as_mut()
            .ok_or(crate::typing_protocol::Refusal::NotAdmitted)?
            .queue(ticket, action, self.playing)
    }
    pub fn cut_typing_input(&mut self, through_epoch: u64) {
        self.typing_cut_through = self.typing_cut_through.max(through_epoch);
        if let Some(typing) = &mut self.typing {
            typing.cut(through_epoch);
        }
    }
    pub fn typing_cut_through_epoch(&self) -> u64 {
        self.typing_cut_through.max(
            self.typing
                .as_ref()
                .map_or(0, |typing| typing.cut_through_epoch()),
        )
    }
    pub fn take_typing_application(&mut self) -> Option<TypingApplication> {
        self.typing.as_mut()?.take_application()
    }
    pub fn typing_admission(&self) -> Option<crate::live_input::TypingAdmission> {
        let input = self.typing.as_ref()?;
        Some(crate::live_input::TypingAdmission {
            target: input.arm().target(),
            owner: input.owner(),
            epoch: input.epoch(),
            armed: input.is_armed(),
            allowed_keys: input.arm().allowed_keys(),
            cut_through_epoch: self.typing_cut_through_epoch(),
        })
    }

    fn typing_authored_reservation(
        &self,
        arm: &PreparedTypingArm,
    ) -> Result<(usize, usize), Invalid> {
        let track = self
            .revision
            .arrangement()
            .tracks()
            .iter()
            .find(|track| arm.matches_track(track))
            .ok_or(Invalid("typing track identity changed"))?;
        let mut edges = Vec::new();
        let mut gate_edges = Vec::new();
        if let Some(instrument) = &track.instrument {
            edges
                .try_reserve_exact(
                    instrument
                        .spans
                        .len()
                        .checked_mul(2)
                        .ok_or(Invalid("typing span count overflow"))?,
                )
                .map_err(|_| Invalid("typing density allocation failed"))?;
            gate_edges
                .try_reserve_exact(edges.capacity())
                .map_err(|_| Invalid("typing edge allocation failed"))?;
            for span in &instrument.spans {
                edges.push((span.start, true));
                edges.push((span.end, false));
                gate_edges.push(span.start);
                gate_edges.push(span.gate_end);
            }
        }
        edges.sort_unstable();
        let mut count: usize = 0;
        let mut voices = 0;
        for (_, on) in edges {
            if on {
                count += 1;
                voices = voices.max(count);
            } else {
                count = count
                    .checked_sub(1)
                    .ok_or(Invalid("typing authored span order is invalid"))?;
            }
        }
        gate_edges.sort_unstable();
        let mut events = 0;
        let mut first = 0;
        for last in 0..gate_edges.len() {
            while gate_edges[last] - gate_edges[first] >= 256 {
                first += 1;
            }
            events = events.max(last + 1 - first);
        }
        if let Some(region) = self
            .revision
            .arrangement()
            .loop_region()
            .filter(|region| region.enabled())
        {
            let (loop_voices, loop_events) =
                super::transport_loop_admission::measure_track(track, region, arm.release_frames())
                    .map_err(|_| Invalid("typing transport loop has no bounded capacity proof"))?;
            voices = voices.max(loop_voices);
            events = events.max(loop_events);
        }
        for clip in self
            .revision
            .session()
            .midi_clips()
            .iter()
            .filter(|clip| clip.track == arm.target().track)
        {
            voices = voices.max(clip.instrument_template().max_active_voices());
            events = events.max(clip.source_template().max_query_edges());
        }
        // Existing captured releases belong to this same stable instrument.
        let retained = self
            .releases
            .iter()
            .filter(|release| {
                self.routes
                    .get(release.track_index)
                    .is_some_and(|route| route.id == arm.target().track)
            })
            .count();
        voices = voices
            .checked_add(retained)
            .ok_or(Invalid("typing retained voice count overflow"))?;
        for (slot, typing) in self.tail_typing.iter().enumerate() {
            let Some(typing) = typing.as_ref().filter(|typing| {
                typing.arm().target().track == arm.target().track
                    && typing.arm().target().device == arm.target().device
            }) else {
                continue;
            };
            let retained = self.tail_mutable_instruments[slot]
                .as_ref()
                .filter(|tail| !tail.runtime.committed_is_quiescent())
                .map_or(typing.active_voices(), |tail| {
                    tail.runtime.max_active_occurrences()
                });
            voices = voices
                .checked_add(retained)
                .ok_or(Invalid("typing retained native capacity overflow"))?;
        }
        Ok((voices, events))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typing_protocol::{Action, Token};
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn publication_id_exhaustion_refuses_before_any_original_runtime_or_gate_moves() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: "ids".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Instrument,
            DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(82),
            ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let target = crate::live_input::TypingTarget {
            project_generation: 0,
            project_revision: 1,
            track,
            device,
        };
        let revision = Arc::new(
            crate::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let prepare = || {
            let mut playback = crate::SessionPlayback::new(Arc::clone(&revision)).unwrap();
            let input = playback
                .prepare_typing_input(
                    PreparedTypingArm::prepare(editor.project(), target).unwrap(),
                    1,
                )
                .unwrap();
            assert!(playback.install_typing_input(input).is_ok());
            playback
        };
        let mut source = prepare();
        let mut oracle = prepare();
        let on = Action::On {
            owner: target.owner().unwrap(),
            token: Token {
                epoch: 1,
                serial: 1,
            },
            key: 9,
            pitch: 69,
            velocity: 100,
        };
        for runtime in [&mut source, &mut oracle] {
            runtime.apply(crate::SessionPlaybackCommand::Play).unwrap();
            runtime.queue_typing_input(1, on).unwrap();
            runtime.render(&mut [[0.0; 2]; 9]).unwrap();
            runtime.take_typing_application().unwrap();
        }
        let counter = u64::MAX - 200;
        source.mutable_instruments[0]
            .as_mut()
            .unwrap()
            .set_next_occurrence_id_for_test(counter);
        let mut candidate = prepare();
        assert!(candidate.try_inherit_state_from(&mut source).is_err());
        assert_eq!(
            source.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            counter
        );
        assert_eq!(source.performance_frame(), 9);
        assert!(source.typing_admission().unwrap().armed);
        let off = Action::Off {
            owner: target.owner().unwrap(),
            token: on.token(),
        };
        for runtime in [&mut source, &mut oracle] {
            runtime.queue_typing_input(2, off).unwrap();
        }
        let mut actual = [[0.0; 2]; 128];
        let mut expected = actual;
        source.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert!(source.take_typing_application().unwrap().result.is_ok());
    }

    #[cfg(feature = "instrument-test-support")]
    #[test]
    fn tracked_owner_groups_typing_copy_and_original_box_transfer_remain_independent() {
        use crate::storage_report::{ClaimReservation, incomplete_owned_pair_for_test};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "groups".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let target = crate::live_input::TypingTarget {
            project_generation: 0,
            project_revision: 1,
            track,
            device,
        };
        let make = |epoch| {
            let root = crate::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap()
            .into_owned_shared();
            let (mut owner, anchors) = incomplete_owned_pair_for_test(root).ok().unwrap();
            owner.with_owner_groups_runtime_for_test(|runtime| {
                let input = runtime
                    .prepare_typing_input(
                        PreparedTypingArm::prepare(editor.project(), target).unwrap(),
                        epoch,
                    )
                    .unwrap();
                assert!(runtime.install_typing_input(input).is_ok());
                runtime.prepare_retiring_contexts().unwrap();
            });
            (owner, anchors)
        };
        let reservation = ClaimReservation {
            items: 32_768,
            anchors: 16,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        };
        let (mut a, a_anchors) = make(1);
        let original_typing = a.with_owner_groups_runtime_for_test(|runtime| {
            runtime.apply(crate::SessionPlaybackCommand::Play).unwrap();
            runtime
                .queue_typing_input(
                    1,
                    Action::On {
                        owner: target.owner().unwrap(),
                        token: Token {
                            epoch: 1,
                            serial: 1,
                        },
                        key: 9,
                        pitch: 69,
                        velocity: 100,
                    },
                )
                .unwrap();
            runtime.render(&mut [[0.0; 2]; 1]).unwrap();
            runtime.take_typing_application().unwrap();
            runtime.typing.as_ref().unwrap().as_ref() as *const TypingPerformance as usize
        });
        let (a, mut a_archive, _) = a
            .try_acquire_owner_groups_on_control(&a_anchors, reservation)
            .unwrap();
        let (b, b_anchors) = make(2);
        let (mut b, mut b_archive, _) = b
            .try_acquire_owner_groups_on_control(&b_anchors, reservation)
            .unwrap();
        let mut previous = Some(a);
        let mut retired = None;
        let mut manifest = None;
        b.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut manifest)
            .unwrap();
        assert!(
            manifest
                .as_ref()
                .unwrap()
                .transferred_handle_count_from(&a_archive)
                >= 2
        );
        let own_b_typing = b.with_owner_groups_runtime_for_test(|runtime| {
            assert!(
                runtime
                    .tail_typing
                    .iter()
                    .flatten()
                    .any(
                        |typing| typing.as_ref() as *const TypingPerformance as usize
                            == original_typing
                    )
            );
            runtime.typing.as_ref().unwrap().as_ref() as *const TypingPerformance as usize
        });
        let disposed_a = retired
            .take()
            .unwrap()
            .into_control_teardown()
            .dispose_on_control()
            .unwrap();
        a_archive.record_disposal_on_control(&disposed_a).unwrap();
        let (c, c_anchors) = make(2);
        let (mut c, _, _) = c
            .try_acquire_owner_groups_on_control(&c_anchors, reservation)
            .unwrap();
        let mut previous = Some(b);
        let mut next_manifest = None;
        c.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut next_manifest)
            .unwrap();
        c.with_owner_groups_runtime_for_test(|runtime| {
            assert!(
                runtime
                    .tail_typing
                    .iter()
                    .flatten()
                    .any(
                        |typing| typing.as_ref() as *const TypingPerformance as usize
                            == original_typing
                    )
            );
            assert_ne!(
                runtime.typing.as_ref().unwrap().as_ref() as *const TypingPerformance as usize,
                own_b_typing,
                "compatible history copy retains two independent prepared Boxes"
            );
        });
        assert_eq!(
            next_manifest
                .as_ref()
                .unwrap()
                .transferred_handle_count_from(&b_archive),
            0
        );
        let disposed_b = retired
            .take()
            .unwrap()
            .into_control_teardown()
            .dispose_on_control()
            .unwrap();
        b_archive.record_disposal_on_control(&disposed_b).unwrap();
        assert_eq!(b_archive.remaining_handle_count(), 0);
        assert!(a_archive.remaining_handle_count() >= 2);
        c.apply(crate::playback::PlaybackCommand::Stop);
        c.render(&mut [[0.0; 2]; 1]).unwrap();
        assert!(
            a_archive.remaining_handle_count() >= 2,
            "Stop/silence is not owner disposal"
        );
        let disposed_c = c.into_control_teardown().dispose_on_control().unwrap();
        a_archive.record_disposal_on_control(&disposed_c).unwrap();
        assert_eq!(a_archive.remaining_handle_count(), 0);
        assert!(
            a_archive.report().incomplete_source,
            "nested arm/Weak ownership still Unknown"
        );
    }
    #[test]
    fn rearming_uses_the_original_allocator_floor_while_old_release_pool_is_retained() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "retained ids".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Instrument,
            DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(82),
            ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let target = crate::live_input::TypingTarget {
            project_generation: 0,
            project_revision: 1,
            track,
            device,
        };
        let revision = Arc::new(
            crate::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let prepare = |epoch| {
            let mut playback = crate::SessionPlayback::new(Arc::clone(&revision)).unwrap();
            let input = playback
                .prepare_typing_input(
                    PreparedTypingArm::prepare(editor.project(), target).unwrap(),
                    epoch,
                )
                .unwrap();
            assert!(playback.install_typing_input(input).is_ok());
            playback
        };
        let mut old = prepare(1);
        old.apply(crate::SessionPlaybackCommand::Play).unwrap();
        let on = |epoch| Action::On {
            owner: target.owner().unwrap(),
            token: Token { epoch, serial: 1 },
            key: 9,
            pitch: 69,
            velocity: 100,
        };
        old.queue_typing_input(1, on(1)).unwrap();
        old.render(&mut [[0.0; 2]; 128]).unwrap();
        old.take_typing_application().unwrap();
        assert_eq!(
            old.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            1
        );
        old.cut_typing_input(1);
        old.render(&mut [[0.0; 2]; 1]).unwrap();
        let mut next = prepare(2);
        next.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            1
        );
        assert!(
            next.tail_mutable_instruments
                .iter()
                .flatten()
                .any(|tail| tail.runtime.next_occurrence_id() == 1)
        );
        next.queue_typing_input(2, on(2)).unwrap();
        next.render(&mut [[0.0; 2]; 128]).unwrap();
        assert!(next.take_typing_application().unwrap().result.is_ok());
        assert_eq!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            2,
            "new physical occurrence must not alias retained original id 1"
        );
    }
}
