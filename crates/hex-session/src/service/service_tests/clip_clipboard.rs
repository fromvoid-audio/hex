use super::*;

#[test]
fn selected_clip_duplicate_uses_cached_audio_and_one_history_step_and_refuses_stale_targets() {
    let directory = TestDir::new();
    let source = directory.0.join("duplicate-source.wav");
    fs::write(&source, wav_mono(&[16_384; 128])).expect("authored PCM");
    let service = Service::new(48_000, 32, 8_000_000).expect("service");
    next_event(&service);
    service.submit(Request::Import(source.clone())).unwrap();
    let imported = next_event(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let audio = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "midi".into(),
        }))
        .unwrap();
    let track = next_event(&service).state.project.tracks()[1].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(1).unwrap(),
        }))
        .unwrap();
    let before = next_event(&service);
    let midi = before.state.project.track(track).unwrap().midi_clips()[0].id();
    let clips = vec![audio, midi];
    fs::remove_file(&source).unwrap();
    // Source custody is already canonical: duplication must not reopen media.
    service
        .submit(Request::DuplicateClips {
            project_generation: 0,
            revision: before.state.revision,
            clips: clips.clone(),
        })
        .unwrap();
    let duplicated = next_event(&service);
    assert!(duplicated.request_complete);
    assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
    assert!(matches!(duplicated.created, Some(Created::Clips { .. })));
    assert_eq!(duplicated.state.revision, before.state.revision + 1);
    assert_eq!(duplicated.state.project.tracks()[0].clips().len(), 2);
    assert_eq!(
        duplicated
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        2
    );
    let mut playback = duplicated.playback.expect("prepared copied playback");
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut pcm = [[0.0; 2]; 128];
    playback.render(&mut pcm).unwrap();
    assert!(pcm.iter().flatten().any(|sample| sample.abs() > 0.1));
    playback.apply(hex_engine::PlaybackCommand::Seek(24_000));
    let mut copied_pcm = [[0.0; 2]; 128];
    playback.render(&mut copied_pcm).unwrap();
    assert_eq!(
        copied_pcm, pcm,
        "the copied clip retains exact original PCM"
    );
    for (generation, revision, selected) in [
        (1, duplicated.state.revision, clips.clone()),
        (0, before.state.revision, clips.clone()),
        (0, duplicated.state.revision, Vec::new()),
        (0, duplicated.state.revision, vec![audio, audio]),
    ] {
        service
            .submit(Request::DuplicateClips {
                project_generation: generation,
                revision,
                clips: selected,
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.request_complete);
        assert!(refused.error.is_some());
        assert!(refused.created.is_none());
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.revision, duplicated.state.revision);
        assert_eq!(refused.state.project, duplicated.state.project);
        assert_eq!(refused.state.can_undo, duplicated.state.can_undo);
        assert_eq!(refused.state.can_redo, duplicated.state.can_redo);
    }
    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    let restored = undone.state.project.snapshot();
    let mut original = before.state.project.snapshot();
    original.next_id = restored.next_id;
    assert_eq!(restored, original);
    service.submit(Request::Redo).unwrap();
    assert_eq!(next_event(&service).state.project, duplicated.state.project);
    // Restore the authored external file before testing reopen; missing-media
    // recovery is separate from the cached-source duplication guarantee.
    fs::write(&source, wav_mono(&[16_384; 128])).unwrap();
    let path = directory.0.join("duplicated.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    service.submit(Request::Open(path)).unwrap();
    let reopened = next_event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert!(reopened.project_replaced);
    assert_eq!(reopened.state.project, duplicated.state.project);
}

#[test]
fn project_sample_request_reuses_resident_identity_and_refuses_stale_source() {
    let directory = TestDir::new();
    let wav = directory.0.join("project-sample.wav");
    fs::write(&wav, wav_mono(&[16_384; 128])).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next_event(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let initial = next_event(&service);
    assert!(initial.error.is_none(), "{:?}", initial.error);
    let track = initial.state.project.tracks()[0].id();
    let asset = initial.state.project.assets().next().unwrap().id();
    fs::remove_file(wav).unwrap();
    let request = |generation, revision| Request::InsertProjectSample {
        project_generation: generation,
        revision,
        asset,
        destination: crate::import_batch::AudioBatchDestination::Track { track, start: 256 },
        new_track_gain: None,
        warp: hex_project::AudioImportWarp::Off,
        audition_timing: None,
    };
    service.submit(request(0, initial.state.revision)).unwrap();
    let placed = next_event(&service);
    assert!(placed.error.is_none(), "{:?}", placed.error);
    assert!(
        matches!(placed.created, Some(Created::Imported { asset: actual, .. }) if actual == asset)
    );
    assert_eq!(placed.state.project.assets().count(), 1);
    assert_eq!(placed.state.project.track(track).unwrap().clips().len(), 2);
    for (generation, revision) in [(1, placed.state.revision), (0, initial.state.revision)] {
        service.submit(request(generation, revision)).unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, placed.state.revision);
        assert_eq!(refused.state.project, placed.state.project);
    }
    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    assert_eq!(undone.state.project.track(track).unwrap().clips().len(), 1);
    service.submit(Request::Redo).unwrap();
    let redone = next_event(&service);
    assert_eq!(redone.state.project, placed.state.project);
    let insert = |revision, destination| Request::InsertProjectSample {
        project_generation: 0,
        revision,
        asset,
        destination,
        new_track_gain: Some(0.5),
        warp: hex_project::AudioImportWarp::Off,
        audition_timing: None,
    };
    service
        .submit(insert(
            redone.state.revision,
            AudioBatchDestination::NewTracks {
                before_track: None,
                start: 128,
            },
        ))
        .unwrap();
    let new_track = next_event(&service);
    assert!(new_track.error.is_none(), "{:?}", new_track.error);
    assert_eq!(new_track.state.project.assets().count(), 1);
    assert_eq!(new_track.state.project.tracks().len(), 2);
    assert_eq!(new_track.state.project.tracks()[1].gain(), 0.5);
    assert_eq!(
        new_track.state.project.tracks()[1].clips()[0].asset(),
        asset
    );
    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    assert_eq!(undone.state.project.tracks().len(), 1);
    service
        .submit(insert(
            undone.state.revision,
            AudioBatchDestination::Console(crate::import_batch::ConsoleDestination::NewTrack {
                before_track: None,
                scene: hex_project::SessionSceneTarget::Insert { before: None },
            }),
        ))
        .unwrap();
    let console = next_event(&service);
    assert!(console.error.is_none(), "{:?}", console.error);
    assert_eq!(console.state.project.assets().count(), 1);
    let created_track = console.state.project.tracks()[1].id();
    assert_eq!(console.state.project.tracks()[1].gain(), 0.5);
    let scene = console.state.project.scenes()[0].id();
    assert!(
        matches!(console.state.project.session_cell(created_track, scene),
        Some(hex_project::SessionCellContent::Audio(audio)) if audio.asset() == asset)
    );
    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    assert_eq!(undone.state.project.tracks().len(), 1);
}

#[test]
fn grouping_linked_audio_clips_keeps_the_outside_alias_and_round_trips() {
    let directory = TestDir::new();
    let wav = directory.0.join("linked-audio.wav");
    fs::write(&wav, wav_mono(&[16_384; 128])).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next_event(&service);
    let submit = |request| {
        service.submit(request).unwrap();
        let event = next_event(&service);
        assert!(event.request_complete);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let imported = submit(Request::Import(wav));
    let first = imported.state.project.tracks()[0].clips()[0].id();
    let duplicate = |clip, revision| {
        let event = submit(Request::DuplicateAliasClips {
            project_generation: 0,
            revision,
            clips: vec![clip],
        });
        let Some(Created::Clips { first, last }) = event.created else {
            panic!("linked duplicate");
        };
        assert_eq!(first, last);
        (first, event)
    };
    let (second, duplicated) = duplicate(first, imported.state.revision);
    let (surviving, before) = duplicate(second, duplicated.state.revision);
    let before_project = &before.state.project;
    let source = before_project.clip_alias(first).unwrap();
    assert_eq!(before_project.clip_alias(second), Some(source));
    assert_eq!(before_project.clip_alias(surviving), Some(source));

    let grouped = submit(Request::GroupClips {
        project_generation: 0,
        revision: before.state.revision,
        clips: vec![first, second],
    });
    let Some(Created::Clip(group)) = grouped.created else {
        panic!("grouped audio clip");
    };
    assert_eq!(grouped.state.revision, before.state.revision + 1);
    let project = &grouped.state.project;
    assert!(project.clip(first).is_none());
    assert!(project.clip(second).is_none());
    assert_eq!(project.clip_alias(group), None);
    assert_eq!(project.clip_alias(surviving), Some(source));
    assert_eq!(project.clip(surviving), before_project.clip(surviving));
    let clip = project.clip(group).unwrap();
    assert_eq!((clip.start(), clip.duration()), (0, 256));
    let events = clip.editable_audio_events().unwrap();
    assert_eq!(events.sources.len(), 2);
    assert_eq!(events.events.len(), 2);
    for (event, (id, start)) in events.events.iter().zip([(first, 0), (second, 128)]) {
        assert_eq!((event.start, event.end), (start, start + 128));
        assert_eq!(
            events
                .sources
                .iter()
                .find(|root| root.id == event.source)
                .map(|root| &root.clip),
            before_project.clip(id)
        );
    }

    let undone = submit(Request::Undo).state.project.snapshot();
    let mut original = before_project.snapshot();
    original.next_id = undone.next_id;
    assert_eq!(undone, original);
    assert_eq!(submit(Request::Redo).state.project, *project);
    let path = directory.0.join("grouped.hex");
    submit(Request::SaveNew(path.clone()));
    assert_eq!(submit(Request::Open(path)).state.project, *project);
}

#[test]
fn grouping_requests_refuse_stale_generation_or_revision_without_history_changes() {
    for time_range in [false, true] {
        let service = Service::new(48_000, 32, 8_000_000).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "midi".into(),
            }))
            .unwrap();
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            }))
            .unwrap();
        next_event(&service);
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::from_quarters(2).unwrap(),
                length: hex_project::BeatTime::ONE,
            }))
            .unwrap();
        let before = next_event(&service);
        let clips = before.state.project.tracks()[0]
            .midi_clips()
            .iter()
            .map(|clip| clip.id())
            .collect::<Vec<_>>();
        let request = |project_generation, revision| {
            if time_range {
                Request::GroupTimeRange {
                    project_generation,
                    revision,
                    range: hex_project::TimeRange::new(vec![track], 12_000, 60_000).unwrap(),
                    create_if_empty: false,
                }
            } else {
                Request::GroupClips {
                    project_generation,
                    revision,
                    clips: clips.clone(),
                }
            }
        };
        service.submit(request(0, before.state.revision)).unwrap();
        let grouped = next_event(&service);
        assert!(grouped.request_complete);
        assert!(grouped.error.is_none(), "{:?}", grouped.error);
        assert!(matches!(grouped.created, Some(Created::Clip(_))));
        assert_eq!(grouped.state.revision, before.state.revision + 1);

        for (project_generation, revision) in
            [(1, grouped.state.revision), (0, before.state.revision)]
        {
            service
                .submit(request(project_generation, revision))
                .unwrap();
            let refused = next_event(&service);
            assert!(refused.request_complete);
            assert!(refused.error.is_some());
            assert!(refused.created.is_none());
            assert!(refused.playback.is_none());
            assert_eq!(refused.state.revision, grouped.state.revision);
            assert_eq!(refused.state.project, grouped.state.project);
            assert_eq!(refused.state.can_undo, grouped.state.can_undo);
            assert_eq!(refused.state.can_redo, grouped.state.can_redo);
        }
    }
}

#[test]
fn selected_clip_duplicate_early_native_refusal_is_a_terminal_without_legacy_flag() {
    // Only the output boundary is substituted. The worker's real early
    // native-source refusal precedes the ordinary Edit match/legacy flag.
    struct HeldOutput {
        armed: Arc<AtomicBool>,
        context: Option<hex_engine::AudioSourceContext>,
    }
    impl WorkerAudioSink for HeldOutput {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Err("fixture offline output".into())
        }
        fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
            self.context = Some(source.context());
        }
        fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
            NativeSamplerReservationHold {
                current_context: self
                    .armed
                    .load(Ordering::Acquire)
                    .then_some(self.context)
                    .flatten(),
                ..Default::default()
            }
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
    }
    let armed = Arc::new(AtomicBool::new(false));
    let service = Service::with_audio_sink(
        16,
        0,
        None,
        Box::new(HeldOutput {
            armed: Arc::clone(&armed),
            context: None,
        }),
    )
    .unwrap();
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "midi".into(),
        }))
        .unwrap();
    let track = next_event(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(1).unwrap(),
        }))
        .unwrap();
    let before = next_event(&service);
    let clip = before.state.project.tracks()[0].midi_clips()[0].id();
    armed.store(true, Ordering::Release);
    service
        .submit(Request::DuplicateClips {
            project_generation: 0,
            revision: before.state.revision,
            clips: vec![clip],
        })
        .unwrap();
    let refused = next_event(&service);
    assert!(refused.request_complete);
    assert!(!refused.duplicate_complete);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("fresh output"))
    );
    assert!(refused.created.is_none());
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.project, before.state.project);
    assert_eq!(refused.state.can_undo, before.state.can_undo);
    assert_eq!(refused.state.can_redo, before.state.can_redo);
    armed.store(false, Ordering::Release);
    service
        .submit(Request::DuplicateClips {
            project_generation: 0,
            revision: before.state.revision,
            clips: vec![clip],
        })
        .unwrap();
    let retried = next_event(&service);
    assert!(retried.request_complete);
    assert!(retried.error.is_none(), "{:?}", retried.error);
    assert_eq!(retried.state.project.tracks()[0].midi_clips().len(), 2);
}
