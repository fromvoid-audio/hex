//! Source-bound play-start metadata. The native streaming driver keeps
//! construction, page custody and callback application; Session owns intent.
use super::*;

/// Temporary project metadata derived by executing one validated play-start
/// edit against the active authoring state. The underlying source catalog is sealed.
pub struct VerifiedArrangementPlayStartSource {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    project: Project,
    sequence: u64,
}

impl VerifiedArrangementPlayStartSource {
    fn capture(
        canonical: &Arc<CanonicalAudioSourceSnapshot>,
        project: &Project,
        target: &ArrangementPlayStartAudition,
        sequence: u64,
    ) -> Result<Self, SessionError> {
        let mut editor = hex_project::Editor::new(project.clone(), 1)?;
        editor.execute(target.edit.command())?;
        if matches!(
            target.edit,
            PlayStartEdit::Audio { .. }
                | PlayStartEdit::AudioSourcePhase { .. }
                | PlayStartEdit::AudioTrimStart { .. }
        ) {
            let clip = editor
                .project()
                .clip(target.edit.clip())
                .ok_or(SessionError::Invalid(
                    "play start audition clip unavailable",
                ))?;
            // The verified candidate constructor materializes transformed or
            // placed audio PCM only from resident bindings. Never decode or
            // substitute a verified file behind the active catalog.
            if clip.audio_edit().is_some()
                && clip.referenced_assets().any(|asset| {
                    !matches!(
                        canonical.sources().get(&asset),
                        Some(crate::AudioSourceBinding::Resident(_))
                    )
                })
            {
                return Err(SessionError::Invalid(
                    "play start audition source is unavailable for streaming",
                ));
            }
        }
        Ok(Self {
            canonical: Arc::clone(canonical),
            project: editor.project().clone(),
            sequence,
        })
    }

    pub fn canonical(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.canonical
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Reserve the overlay against the canonical catalog. The native
    /// constructor still checks the exact raw and derived source identities.
    pub fn reservation_quote(
        &self,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        self.canonical
            .candidate_reservation_quote_for_effects(&self.project, effects)
    }

    pub fn prepare_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        if let Some(effects) = effects {
            effects.prepare(
                destination,
                &self.project,
                catalog,
                self.canonical.context(),
                None,
            )
        } else {
            destination.prepare_on_control(&self.project, catalog, self.canonical.context())
        }
    }
}

/// Exact worker-owned intent. Release keeps the last accepted alignment until
/// the canonical source changes or the ordinary edit receipt retires the gesture.
pub enum VerifiedArrangementPlayStartUpdate {
    Preview(Arc<VerifiedArrangementPlayStartSource>),
    Cancel(Arc<VerifiedArrangementPlayStartSource>),
}

#[derive(Default)]
pub(in crate::service) struct VerifiedPlayStartLane {
    attempted: Option<ArrangementPlayStartAudition>,
    active: Option<(ClipId, Arc<VerifiedArrangementPlayStartSource>)>,
    next_attempt: Option<Instant>,
    sequence: u64,
}

impl VerifiedPlayStartLane {
    pub(in crate::service) fn drive(
        &mut self,
        mailbox: &Mailbox,
        state: &ViewState,
        project: &Project,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        available: bool,
    ) -> bool {
        let intent = mailbox.lock().intent.clone();
        if self.active.is_none() && !matches!(intent, Intent::Held(Target::PlayStart(_))) {
            self.attempted = None;
            return false;
        }
        let canonical = state.canonical_audio_source.as_ref().filter(|source| {
            available
                && source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && context.editor_revision == state.revision
                && state.audio_authoring_revision(context) == Some(state.revision)
                && state.project.as_ref() == project
        });
        if self.active.as_ref().is_some_and(|(clip, preview)| {
            canonical.is_none_or(|source| !Arc::ptr_eq(source, preview.canonical()))
                || !matches!(
                    &intent,
                    Intent::Held(Target::PlayStart(target))
                        | Intent::Released(Target::PlayStart(target))
                        if target.context == context && target.edit.clip() == *clip
                )
        }) && let Some((_, preview)) = self.active.take()
        {
            sink.verified_arrangement_play_start_update(
                VerifiedArrangementPlayStartUpdate::Cancel(preview),
            );
            self.attempted = None;
        }
        let (Intent::Held(Target::PlayStart(target)), Some(canonical)) = (intent, canonical) else {
            self.attempted = None;
            return false;
        };
        if target.context != context || self.attempted.as_ref() == Some(&target) {
            return false;
        }
        let started = Instant::now();
        if self.next_attempt.is_some_and(|next| next > started) {
            return true;
        }
        let Some(cancel) = mailbox.claim(Target::PlayStart(target)) else {
            return true;
        };
        let Some(sequence) = self.sequence.checked_add(1) else {
            return false;
        };
        self.sequence = sequence;
        self.attempted = Some(target);
        let Ok(preview) =
            VerifiedArrangementPlayStartSource::capture(canonical, project, &target, sequence)
        else {
            self.next_attempt = Some(started + RETRY_INTERVAL);
            return false;
        };
        let preview = Arc::new(preview);
        let cancelled = cancel.load(Ordering::Acquire);
        let accepted = !cancelled
            && sink.verified_arrangement_play_start_update(
                VerifiedArrangementPlayStartUpdate::Preview(Arc::clone(&preview)),
            );
        self.next_attempt = Some(started + attempt_interval(Kind::PlayStart, !accepted, cancelled));
        if !accepted {
            self.attempted = None;
            return true;
        }
        self.active = Some((target.edit.clip(), preview));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::verified_source_tests::{Files, next, reservation, run};
    use hex_project::BeatTime;
    use std::sync::atomic::AtomicBool;

    struct RecordingSink {
        updates: Vec<VerifiedArrangementPlayStartUpdate>,
        accept: bool,
    }

    impl RecordingSink {
        fn new(accept: bool) -> Self {
            Self {
                updates: Vec::new(),
                accept,
            }
        }
    }

    impl WorkerAudioSink for RecordingSink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn verified_arrangement_play_start_update(
            &mut self,
            update: VerifiedArrangementPlayStartUpdate,
        ) -> bool {
            let accept =
                self.accept || matches!(update, VerifiedArrangementPlayStartUpdate::Cancel(_));
            self.updates.push(update);
            accept
        }
    }

    #[test]
    fn verified_play_start_allows_plain_streamed_slips_and_refuses_streamed_transforms() {
        let files = Files::new();
        let wav = files.wav("play-start.wav", 48_000, 1_024, 8192);
        let service = Service::new(48_000, 16, 4096).unwrap();
        next(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(reservation(2)),
            },
        );
        let imported = run(&service, Request::Import(wav.clone()));
        let imported_source = imported.state.canonical_audio_source.as_ref().unwrap();
        let imported_project = imported.state.project.as_ref();
        let clip_id = imported_project.tracks()[0].clips()[0].id();
        let imported_clip = imported_project.clip(clip_id).unwrap();

        // Plain streamed TrimClip play-start shift keeps the sealed VerifiedFile binding.
        let trim = ArrangementPlayStartAudition {
            context: imported_source.context(),
            edit: PlayStartEdit::AudioTrimStart {
                clip: imported_clip.id(),
                start: imported_clip.start(),
                duration: imported_clip.duration(),
                source_offset: 256,
                source_length: 768,
            },
        };
        let preview = VerifiedArrangementPlayStartSource::capture(
            imported_source,
            imported_project,
            &trim,
            1,
        )
        .expect("plain streamed trim start must succeed");
        assert!(Arc::ptr_eq(preview.canonical(), imported_source));
        assert_eq!(
            preview
                .project
                .clip(imported_clip.id())
                .unwrap()
                .source_offset(),
            256
        );

        // Creating a placed audio_edit on a VerifiedFile clip cannot decode behind the catalog.
        let placed = ArrangementPlayStartAudition {
            context: imported_source.context(),
            edit: PlayStartEdit::Audio {
                clip: imported_clip.id(),
                position: 256,
            },
        };
        assert!(matches!(
            VerifiedArrangementPlayStartSource::capture(
                imported_source,
                imported_project,
                &placed,
                2
            ),
            Err(SessionError::Invalid(
                "play start audition source is unavailable for streaming"
            ))
        ));

        // Plain streamed SetClipSourcePhase on a looping clip also keeps VerifiedFile binding.
        let looped = run(
            &service,
            Request::Edit(Command::SetClipLooping {
                clip: clip_id,
                looping: true,
            }),
        );
        let source = looped.state.canonical_audio_source.as_ref().unwrap();
        let project = looped.state.project.as_ref();
        let clip = project.clip(clip_id).unwrap();
        let phase = ArrangementPlayStartAudition {
            context: source.context(),
            edit: PlayStartEdit::AudioSourcePhase {
                clip: clip.id(),
                phase: 384,
            },
        };
        let preview = VerifiedArrangementPlayStartSource::capture(source, project, &phase, 3)
            .expect("plain streamed source phase must succeed");
        assert!(Arc::ptr_eq(preview.canonical(), source));
        assert_eq!(preview.project.clip(clip.id()).unwrap().source_phase(), 384);

        // A Resident-bound audio clip (e.g., recorded/resident asset) allows placed PlayStartEdit::Audio.
        let resident_service = Service::new(48_000, 16, 4096).unwrap();
        next(&resident_service);
        let resident_imported = run(&resident_service, Request::Import(wav));
        let resident_snapshot = resident_imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap();
        let resident_project = resident_imported.state.project.as_ref();
        let resident_clip = &resident_project.tracks()[0].clips()[0];
        let resident_placed = ArrangementPlayStartAudition {
            context: resident_snapshot.context(),
            edit: PlayStartEdit::Audio {
                clip: resident_clip.id(),
                position: 256,
            },
        };
        let resident_preview = VerifiedArrangementPlayStartSource::capture(
            resident_snapshot,
            resident_project,
            &resident_placed,
            4,
        )
        .expect("resident-bound placed audio play-start must succeed");
        assert!(
            resident_preview
                .project
                .clip(resident_clip.id())
                .unwrap()
                .audio_edit()
                .is_some()
        );

        // MIDI play-start edits in VerifiedStreaming succeed without audio-edit rejections.
        let midi_track = run(
            &service,
            Request::Edit(Command::AddMidiTrack {
                name: "midi".into(),
            }),
        );
        let Created::Track(midi_track_id) = midi_track.created.unwrap() else {
            panic!("midi track")
        };
        let beat = |q| BeatTime::from_quarters(q).unwrap();
        let placed_midi = run(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track: midi_track_id,
                start: BeatTime::ZERO,
                length: beat(4),
            }),
        );
        let Created::Clip(midi_clip_id) = placed_midi.created.unwrap() else {
            panic!("midi clip")
        };
        let midi_state = placed_midi.state;
        let midi_source = midi_state.canonical_audio_source.as_ref().unwrap();
        let midi_edit = PlayStartEdit::from_midi_command(&Command::SetMidiClipSource {
            clip: midi_clip_id,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: true,
            length: beat(4),
        })
        .unwrap();
        let midi_target = ArrangementPlayStartAudition {
            context: midi_source.context(),
            edit: midi_edit,
        };
        let midi_preview = VerifiedArrangementPlayStartSource::capture(
            midi_source,
            midi_state.project.as_ref(),
            &midi_target,
            5,
        )
        .expect("midi play start capture must succeed in VerifiedStreaming");
        assert_eq!(
            midi_preview
                .project
                .midi_clip(midi_clip_id)
                .unwrap()
                .source_start(),
            beat(1)
        );
    }

    #[test]
    fn verified_play_start_lane_drives_coalesced_preview_release_cancel_and_refusal() {
        let files = Files::new();
        let wav = files.wav("lane.wav", 48_000, 1_024, 8192);
        let service = Service::new(48_000, 16, 4096).unwrap();
        next(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(reservation(2)),
            },
        );
        let imported = run(&service, Request::Import(wav));
        let state = imported.state;
        let clip_id = state.project.tracks()[0].clips()[0].id();
        let clip = state.project.clip(clip_id).unwrap();
        let source = Arc::clone(state.canonical_audio_source.as_ref().unwrap());
        let context = source.context();
        let project = Arc::clone(&state.project);
        let make_target = |offset| ArrangementPlayStartAudition {
            context,
            edit: PlayStartEdit::AudioTrimStart {
                clip: clip.id(),
                start: clip.start(),
                duration: clip.duration(),
                source_offset: offset,
                source_length: 1_024 - offset,
            },
        };

        let mailbox = Mailbox::default();
        let mut lane = VerifiedPlayStartLane::default();
        let mut sink = RecordingSink::new(true);

        // Idle -> no updates.
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert!(sink.updates.is_empty());

        // Held -> sends Preview(seq 1) and paces next attempt by MOVE_PREVIEW_INTERVAL.
        mailbox.set(Target::PlayStart(make_target(128)));
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 1);
        let first_seq = match &sink.updates[0] {
            VerifiedArrangementPlayStartUpdate::Preview(p) => {
                assert_eq!(p.sequence(), 1);
                p.sequence()
            }
            VerifiedArrangementPlayStartUpdate::Cancel(_) => panic!("expected Preview"),
        };

        // Identical target -> no duplicate Preview.
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 1);

        // Newer Held target within 16ms cadence -> returns true (wait) without sending yet.
        mailbox.set(Target::PlayStart(make_target(256)));
        mailbox.set(Target::PlayStart(make_target(384)));
        assert!(lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 1);

        // Once 16ms cadence elapses -> coalesced latest value sends Preview(seq 2).
        lane.next_attempt = Some(Instant::now() - Duration::from_millis(1));
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 2);
        let second_preview = match &sink.updates[1] {
            VerifiedArrangementPlayStartUpdate::Preview(p) => {
                assert!(p.sequence() > first_seq);
                assert_eq!(p.project.clip(clip_id).unwrap().source_offset(), 384);
                Arc::clone(p)
            }
            VerifiedArrangementPlayStartUpdate::Cancel(_) => panic!("expected Preview"),
        };

        // Intent::Released retains active preview (no Cancel).
        mailbox.release(Kind::PlayStart);
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 2);

        // Intent::Idle cancels active preview.
        mailbox.clear(Some(Kind::PlayStart), false);
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 3);
        assert!(matches!(
            &sink.updates[2],
            VerifiedArrangementPlayStartUpdate::Cancel(p) if Arc::ptr_eq(p, &second_preview)
        ));

        // Canonical snapshot replacement (available = false) cancels active preview.
        lane.next_attempt = None;
        mailbox.set(Target::PlayStart(make_target(192)));
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, true));
        assert_eq!(sink.updates.len(), 4);
        let third_preview = match &sink.updates[3] {
            VerifiedArrangementPlayStartUpdate::Preview(p) => Arc::clone(p),
            VerifiedArrangementPlayStartUpdate::Cancel(_) => panic!("expected Preview"),
        };
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut sink, false));
        assert_eq!(sink.updates.len(), 5);
        assert!(matches!(
            &sink.updates[4],
            VerifiedArrangementPlayStartUpdate::Cancel(p) if Arc::ptr_eq(p, &third_preview)
        ));

        // Sink refusal resets attempted and asks worker to retry after RETRY_INTERVAL.
        mailbox.clear(Some(Kind::PlayStart), true);
        lane.next_attempt = None;
        let mut refusing = RecordingSink::new(false);
        mailbox.set(Target::PlayStart(make_target(512)));
        assert!(lane.drive(&mailbox, &state, &project, context, &mut refusing, true));
        assert!(lane.attempted.is_none());
        assert_eq!(refusing.updates.len(), 1);
        refusing.accept = true;
        lane.next_attempt = Some(Instant::now() - Duration::from_millis(1));
        assert!(!lane.drive(&mailbox, &state, &project, context, &mut refusing, true));
        assert_eq!(refusing.updates.len(), 2);
        assert!(matches!(
            &refusing.updates[1],
            VerifiedArrangementPlayStartUpdate::Preview(p)
                if p.project.clip(clip_id).unwrap().source_offset() == 512
        ));
    }
}
