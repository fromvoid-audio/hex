use super::*;
use std::sync::atomic::Ordering;

#[test]
fn mixed_reverse_prepare_refusal_preserves_project_history_and_accepted_pcm() {
    use hex_project::{BeatTime, Tempo};
    let (mut session, audio, mut previous) = fixture();
    let mut run = |command| {
        let (created, prepared) = session
            .command_prepared_revision(command, Some(&previous))
            .unwrap();
        previous = prepared;
        created
    };
    run(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(60_000_000).unwrap(),
    });
    run(Command::SetTempoPoint {
        beat: BeatTime::ONE,
        tempo: Tempo::new(240_000_000).unwrap(),
    });
    let Created::Track(track) = run(Command::AddMidiTrack {
        name: "midi".into(),
    }) else {
        panic!("track")
    };
    run(Command::InsertInstrument {
        track,
        state: hex_project::default_instrument_state().unwrap(),
    });
    let Created::Clip(midi) = run(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(2).unwrap(),
    }) else {
        panic!("clip")
    };
    run(Command::AddMidiNote {
        clip: midi,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 16_000).unwrap(),
        pitch: 60,
        channel: 2,
        velocity: 99,
    });
    let before = session.project().snapshot();
    let resident = session.resident_frames;
    let result = session
        .command_prepared_revision(Command::ReverseClips(vec![audio, midi]), Some(&previous));
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("quantizes to zero frames")
    );
    assert_eq!(session.project().snapshot(), before);
    assert_eq!(session.resident_frames, resident);
    let undone = session
        .undo_prepared_revision(Some(&previous))
        .unwrap()
        .unwrap();
    assert!(
        session
            .project()
            .midi_clip(midi)
            .unwrap()
            .notes()
            .is_empty()
    );
    let redone = session
        .redo_prepared_revision(Some(&undone))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot(), before);
    let render = |revision| {
        let mut playback =
            hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut frames = [[0.0; 2]; 257];
        playback.render(&mut frames).unwrap();
        frames
    };
    let accepted = render(previous);
    assert!(accepted.iter().flatten().any(|s| *s != 0.0));
    assert_eq!(render(redone), accepted);
}

#[test]
fn bounded_audio_placement_refusal_preserves_session_project_history_and_audio() {
    let (mut session, clip, previous) = fixture();
    let before_repeat = session.project().snapshot();
    let (_, previous) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 12_000,
                },
            },
            Some(&previous),
        )
        .unwrap();
    let before = session.project().snapshot();
    let history = (session.can_undo(), session.can_redo());
    let resident_frames = session.resident_frames;
    let result = session.command_prepared_revision(
        Command::ResizeAudioClipView {
            clip,
            operation: hex_project::AudioViewResizeOperation::OuterEnd {
                project_frame: hex_project::MAX_AUDIO_DOMAIN_FRAMES + 1,
            },
        },
        Some(&previous),
    );
    assert!(result.is_err());
    assert_eq!(session.project().snapshot(), before);
    assert_eq!((session.can_undo(), session.can_redo()), history);
    assert_eq!(session.resident_frames, resident_frames);
    let undone = session
        .undo_prepared_revision(Some(&previous))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot(), before_repeat);
    let redone = session
        .redo_prepared_revision(Some(&undone))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot(), before);
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(redone).unwrap();
    playback.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 257];
    playback.render(&mut output).unwrap();
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn unresolved_device_asset_resources_refuse_actual_preparation_without_committing_history() {
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    for bypassed in [false, true] {
        let (mut session, _, previous) = fixture();
        let (Created::Track(track), previous) = session
            .command_prepared_revision(
                Command::InsertTrackWithDevice {
                    name: "unused instrument".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
                },
                Some(&previous),
            )
            .unwrap()
        else {
            panic!("instrument track")
        };
        let device = session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let (_, previous) = session
            .command_prepared_revision(
                Command::SetDeviceBypass { device, bypassed },
                Some(&previous),
            )
            .unwrap();
        let asset = session.project().assets().next().unwrap().id();
        let before = session.project().snapshot();
        let history = (session.can_undo(), session.can_redo());
        let resident_frames = session.resident_frames;
        let state = session
            .project()
            .device(device)
            .unwrap()
            .state()
            .clone()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap();
        let result = session.command_prepared_revision(
            Command::ReplaceDeviceState { device, state },
            Some(&previous),
        );
        assert!(
            result.is_err(),
            "unused/bypassed slots still require admitted resources"
        );
        assert_eq!(session.project().snapshot(), before);
        assert_eq!((session.can_undo(), session.can_redo()), history);
        assert_eq!(session.resident_frames, resident_frames);
    }
}

fn fixture() -> (
    Session,
    hex_project::ClipId,
    Arc<hex_engine::PreparedRevision>,
) {
    fixture_at_rate(16_000)
}
fn fixture_at_rate(
    rate: u32,
) -> (
    Session,
    hex_project::ClipId,
    Arc<hex_engine::PreparedRevision>,
) {
    let length = rate / 2;
    let mut session = Session::new(rate, 32, u64::from(length) + 192).unwrap();
    let audio = Audio::new(
        rate,
        (0..length)
            .map(|i| {
                let x = (i as f32 * 0.173).sin() * 0.4;
                [x, -x]
            })
            .collect(),
    )
    .unwrap();
    let waveform = waveform::summarize(&audio, 1_024).unwrap();
    let prepared = PreparedImport {
        path: "root.wav".into(),
        source_sample_rate: rate,
        source_frames: u64::from(length),
        conversion_version: hex_project::IDENTITY_CONVERSION,
        source_audio: audio.clone(),
        audio,
        waveform,
        file_identity: None,
    };
    let (Created::Imported { clip, .. }, revision) =
        session.import_prepared_revision(prepared, None).unwrap()
    else {
        panic!("import")
    };
    (session, clip, revision)
}

#[test]
fn extending_first_audio_repeat_boundary_preserves_silence_and_history() {
    use hex_project::AudioViewResizeOperation;

    let (mut session, clip, original) = fixture();
    session.max_resident_frames = 100_000;
    let (_, repeated) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 22_000,
                },
            },
            Some(&original),
        )
        .unwrap();
    let before = session.project().snapshot();
    let original_clip = session.project().clip(clip).unwrap().clone();
    let (_, extended) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::CycleEnd {
                    project_frame: 10_000,
                },
            },
            Some(&repeated),
        )
        .expect("the first boundary can extend beyond the original source");
    let saved = session.project().snapshot();
    let edited_clip = session.project().clip(clip).unwrap();
    assert_eq!(edited_clip.start(), original_clip.start());
    assert_eq!(edited_clip.duration(), original_clip.duration());
    assert_eq!(edited_clip.asset(), original_clip.asset());

    let render = |revision: &Arc<hex_engine::PreparedRevision>, block: usize| {
        let mut renderer = hex_engine::Renderer::new(revision.arrangement());
        renderer.play();
        let mut output = vec![[0.0; 2]; 22_000];
        for chunk in output.chunks_mut(block) {
            renderer.render(chunk).unwrap();
        }
        output
    };
    let source = render(&repeated, 257);
    let actual = render(&extended, 257);
    assert!(source[..8_000].iter().any(|frame| *frame != [0.0; 2]));
    for (index, frame) in actual.iter().enumerate() {
        let position = index % 10_000;
        let expected = if position < 8_000 {
            source[position]
        } else {
            [0.0; 2]
        };
        assert_eq!(*frame, expected, "frame {index}");
    }
    assert_eq!(render(&extended, 1), actual);
    let bytes = hex_storage::encode(session.project()).unwrap();
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(reopened.snapshot(), saved);
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &reopened,
            &session.media_bank().resident,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    assert_eq!(render(&prepared, 257), actual);
    for command in [
        Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::CycleEnd {
                project_frame: 22_000,
            },
        },
        Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 1_000,
        },
        Command::TrimClip {
            clip,
            start: 0,
            duration: 8_000,
            source_offset: 0,
            source_length: 8_000,
        },
        Command::ReverseClips(vec![clip]),
    ] {
        assert!(
            session
                .command_prepared_revision(command, Some(&extended))
                .is_err()
        );
        assert_eq!(session.project().snapshot(), saved);
    }
    let undone = session
        .undo_prepared_revision(Some(&extended))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot().tracks, before.tracks);
    let redone = session
        .redo_prepared_revision(Some(&undone))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot().tracks, saved.tracks);
    assert_eq!(render(&redone, 257), actual);
    let (Created::Clip(duplicate), duplicated) = session
        .command_prepared_revision(Command::DuplicateClip(clip), Some(&redone))
        .unwrap()
    else {
        panic!("duplicate")
    };
    let before_group = session.project().snapshot();
    assert!(
        session
            .command_prepared_revision(
                Command::GroupClips(vec![clip, duplicate]),
                Some(&duplicated)
            )
            .is_err()
    );
    assert_eq!(session.project().snapshot(), before_group);
}

#[test]
fn silent_audio_repeat_can_be_shortened_and_outer_extent_extended() {
    use hex_project::AudioViewResizeOperation;
    let (mut session, clip, original) = fixture();
    session.max_resident_frames = 150_000;
    let mut revision = original;
    for operation in [
        AudioViewResizeOperation::RepeatOuterEnd {
            project_frame: 22_000,
        },
        AudioViewResizeOperation::CycleEnd {
            project_frame: 10_000,
        },
        AudioViewResizeOperation::RepeatOuterEnd {
            project_frame: 24_000,
        },
    ] {
        revision = session
            .command_prepared_revision(
                Command::ResizeAudioClipView { clip, operation },
                Some(&revision),
            )
            .unwrap()
            .1;
    }
    let clip_before = session.project().clip(clip).unwrap();
    assert_eq!(clip_before.duration(), 24_000);
    assert_eq!(
        clip_before.audio_edit().unwrap().repeat_period,
        Some(10_000)
    );
    let (_, shortened) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::CycleEnd {
                    project_frame: 6_000,
                },
            },
            Some(&revision),
        )
        .unwrap();
    let clip_after = session.project().clip(clip).unwrap();
    assert_eq!(clip_after.duration(), 24_000);
    assert_eq!(clip_after.audio_edit().unwrap().repeat_period, None);
    assert_eq!(clip_after.audio_edit().unwrap().view_end, 6_000);
    session
        .undo_prepared_revision(Some(&shortened))
        .unwrap()
        .unwrap();
    assert_eq!(
        session
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .repeat_period,
        Some(10_000)
    );
}

#[test]
fn silent_audio_repeat_renders_at_48000_hz() {
    use hex_project::AudioViewResizeOperation;
    let (mut session, clip, original) = fixture_at_rate(48_000);
    session.max_resident_frames = 200_000;
    let (_, repeated) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 53_000,
                },
            },
            Some(&original),
        )
        .unwrap();
    let (_, extended) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::CycleEnd {
                    project_frame: 26_000,
                },
            },
            Some(&repeated),
        )
        .unwrap();
    let render = |revision: &Arc<hex_engine::PreparedRevision>| {
        let mut renderer = hex_engine::Renderer::new(revision.arrangement());
        renderer.play();
        let mut frames = vec![[0.0; 2]; 53_000];
        for chunk in frames.chunks_mut(257) {
            renderer.render(chunk).unwrap();
        }
        frames
    };
    let source = render(&repeated);
    let actual = render(&extended);
    for (index, frame) in actual.iter().enumerate() {
        let position = index % 26_000;
        assert_eq!(
            *frame,
            if position < 24_000 {
                source[position]
            } else {
                [0.0; 2]
            },
            "frame {index}"
        );
    }
}

// Each rate/mode owns independent Sessions for all three tempos. Separate
// cases let libtest balance the work without reducing the history/PCM matrix.
mod subframe_tempo_history {
    use super::*;
    use hex_project::{BeatTime, Tempo, WarpMarker, WarpMode, default_audio_warp};

    const TONES: WarpMode = WarpMode::Tones { grain_ms: 32 };
    const BEATS: WarpMode = WarpMode::Beats {
        preserve: hex_project::BeatsPreserve::Transients,
        tail: hex_project::BeatsTail::Forward,
        decay: 100,
    };

    fn check(rate: u32, mode: WarpMode) {
        for bpm in [30_000_000, 240_000_000, 480_000_000] {
            let (mut session, clip, previous) = fixture_at_rate(rate);
            let length = u64::from(rate / 2);
            let mut settings = default_audio_warp(
                session.project().clip(clip).unwrap(),
                session.project().tempo_map(),
                mode,
            )
            .unwrap();
            let tick = BeatTime::ONE.raw() / length;
            settings.markers.insert(
                1,
                WarpMarker {
                    source_frame: 1,
                    beat: BeatTime::from_raw(tick),
                },
            );
            settings.markers.insert(
                2,
                WarpMarker {
                    source_frame: length - 1,
                    beat: BeatTime::from_raw(BeatTime::ONE.raw() - tick),
                },
            );
            let (_, previous) = session
                .command_prepared_revision(
                    Command::SetAudioClipWarp {
                        clip,
                        settings: Some(settings),
                    },
                    Some(&previous),
                )
                .unwrap();
            let before = session.project().clone();
            let (_, changed) = session
                .command_prepared_revision(
                    Command::SetProjectTempo {
                        tempo: Tempo::new(bpm).unwrap(),
                    },
                    Some(&previous),
                )
                .unwrap_or_else(|error| {
                    panic!("supported global bpm {bpm}, {mode:?} at {rate} must prepare: {error}")
                });
            let after = session.project().clone();
            let output = after.tempo_map().beat_to_frame(BeatTime::ONE).unwrap();
            assert_eq!(after.clip(clip).unwrap().duration(), output);
            assert_eq!(
                after.clip(clip).unwrap().audio_edit().unwrap().domain,
                before.clip(clip).unwrap().audio_edit().unwrap().domain
            );
            let render = |revision: &hex_engine::PreparedRevision| {
                let mut renderer = hex_engine::Renderer::new(revision.arrangement());
                renderer.play();
                let mut out = vec![[0.0; 2]; output as usize];
                for chunk in out.chunks_mut(63) {
                    renderer.render(chunk).unwrap();
                }
                assert!(out.iter().flatten().all(|sample| sample.is_finite()));
                assert!(out.iter().any(|frame| frame[0].abs() > 0.01));
                out
            };
            let pcm = render(&changed);
            assert_eq!(
                render(&session.compile_prepared_revision(None).unwrap()),
                pcm
            );
            let undone = session
                .undo_prepared_revision(Some(&changed))
                .unwrap()
                .unwrap();
            assert_eq!(session.project(), &before);
            let redone = session
                .redo_prepared_revision(Some(&undone))
                .unwrap()
                .unwrap();
            assert_eq!(session.project(), &after);
            assert_eq!(render(&redone), pcm);
        }
    }

    #[test]
    fn repitch_16000_hz() {
        check(16_000, WarpMode::RePitch);
    }

    #[test]
    fn tones_16000_hz() {
        check(16_000, TONES);
    }

    #[test]
    fn beats_16000_hz() {
        check(16_000, BEATS);
    }

    #[test]
    fn repitch_44100_hz() {
        check(44_100, WarpMode::RePitch);
    }

    #[test]
    fn tones_44100_hz() {
        check(44_100, TONES);
    }

    #[test]
    fn beats_44100_hz() {
        check(44_100, BEATS);
    }

    #[test]
    fn repitch_48000_hz() {
        check(48_000, WarpMode::RePitch);
    }

    #[test]
    fn tones_48000_hz() {
        check(48_000, TONES);
    }

    #[test]
    fn beats_48000_hz() {
        check(48_000, BEATS);
    }
}
#[test]
fn global_bpm_after_warped_section_reorder_prepares_and_roundtrips_complete_history() {
    use hex_project::{BeatTime, ElementColor, Tempo, WarpMode, default_audio_warp};
    for rate in [44_100, 48_000] {
        for mode in [
            WarpMode::RePitch,
            WarpMode::Beats {
                preserve: hex_project::BeatsPreserve::Transients,
                tail: hex_project::BeatsTail::Forward,
                decay: 100,
            },
        ] {
            let (mut session, clip, mut previous) = fixture_at_rate(rate);
            let settings = default_audio_warp(
                session.project().clip(clip).unwrap(),
                session.project().tempo_map(),
                mode,
            )
            .unwrap();
            (_, previous) = session
                .command_prepared_revision(
                    Command::SetAudioClipWarp {
                        clip,
                        settings: Some(settings),
                    },
                    Some(&previous),
                )
                .unwrap();
            (_, previous) = session
                .command_prepared_revision(
                    Command::SetProjectTempo {
                        tempo: Tempo::new(143_000_000).unwrap(),
                    },
                    Some(&previous),
                )
                .unwrap();
            let mut sections = Vec::new();
            for (start, end) in [(1, 2), (3, 4)] {
                let (Created::ArrangementSection(id), next) = session
                    .command_prepared_revision(
                        Command::AddArrangementSection {
                            name: "section".into(),
                            color: ElementColor::Rose,
                            start: BeatTime::from_raw(BeatTime::ONE.raw() * start / 4),
                            end: BeatTime::from_raw(BeatTime::ONE.raw() * end / 4),
                        },
                        Some(&previous),
                    )
                    .unwrap()
                else {
                    panic!("section")
                };
                sections.push(id);
                previous = next;
            }
            (_, previous) = session
                .command_prepared_revision(
                    Command::ReorderSectionBefore {
                        section: sections[0],
                        before: sections[1],
                    },
                    Some(&previous),
                )
                .unwrap();
            let before = session.project().clone();
            assert!(
                before.tracks()[0].clips().iter().all(|c| c
                    .audio_edit()
                    .unwrap()
                    .processing_projection
                    .is_some())
            );
            let (_, changed) = session
                .command_prepared_revision(
                    Command::SetProjectTempo {
                        tempo: Tempo::new(180_000_000).unwrap(),
                    },
                    Some(&previous),
                )
                .unwrap();
            let after = session.project().clone();
            for old in before.tracks()[0].clips() {
                let new = after.clip(old.id()).unwrap();
                let edit = new.audio_edit().unwrap();
                assert!(Arc::ptr_eq(&edit.domain, &old.audio_edit().unwrap().domain));
                let start = edit.live_musical_start().unwrap();
                let end = BeatTime::from_raw(start.raw() + edit.view_end - edit.view_start);
                assert_eq!(new.start(), after.tempo_map().beat_to_frame(start).unwrap());
                assert_eq!(
                    new.start() + new.duration(),
                    after.tempo_map().beat_to_frame(end).unwrap()
                );
            }
            assert_eq!(
                hex_storage::decode(&hex_storage::encode(&after).unwrap()).unwrap(),
                after
            );
            let frames = after.tempo_map().beat_to_frame(BeatTime::ONE).unwrap() as usize;
            let render = |revision: &hex_engine::PreparedRevision, block: usize| {
                let mut renderer = hex_engine::Renderer::new(revision.arrangement());
                renderer.play();
                let mut out = vec![[0.0; 2]; frames];
                for chunk in out.chunks_mut(block) {
                    renderer.render(chunk).unwrap();
                }
                assert!(out.iter().flatten().all(|s| s.is_finite()));
                assert!(out.iter().any(|f| f[0].abs() > 0.01));
                out
            };
            let pcm = render(&changed, 63);
            assert_eq!(
                render(&session.compile_prepared_revision(None).unwrap(), 257),
                pcm
            );
            let undone = session
                .undo_prepared_revision(Some(&changed))
                .unwrap()
                .unwrap();
            assert_eq!(session.project(), &before);
            let redone = session
                .redo_prepared_revision(Some(&undone))
                .unwrap()
                .unwrap();
            assert_eq!(session.project(), &after);
            assert_eq!(render(&redone, 127), pcm);
        }
    }
}

#[test]
fn accepted_waveforms_share_cropped_pcm_keep_gain_outside_and_failed_edit_keeps_history_and_peaks()
{
    let (mut session, clip, previous) = fixture();
    let (_, pitched) = session
        .command_prepared_revision(
            Command::SetAudioClipPitch { clip, cents: 137 },
            Some(&previous),
        )
        .unwrap();
    let root = Arc::clone(&session.audio_clip_waveforms.borrow()[&clip].waveform);
    let (Created::Clip(tail), split) = session
        .command_prepared_revision(Command::SplitClip { clip, frame: 3_137 }, Some(&pitched))
        .unwrap()
    else {
        panic!("split")
    };
    {
        let views = session.audio_clip_waveforms.borrow();
        assert!(Arc::ptr_eq(&root, &views[&tail].waveform));
        assert_eq!(views[&tail].offset, 3_137);
        assert_eq!(views[&tail].length, 4_863);
    }
    session
        .command_prepared_revision(
            Command::SetAudioClipGain {
                clip: tail,
                millidb: -6_000,
            },
            Some(&split),
        )
        .unwrap();
    assert!(Arc::ptr_eq(
        &root,
        &session.audio_clip_waveforms.borrow()[&tail].waveform
    ));
    let before = session.project().clone();
    let can_undo = session.can_undo();
    let can_redo = session.can_redo();
    session.shutdown.store(true, Ordering::Release);
    assert!(
        session
            .command_prepared_revision(
                Command::SetAudioClipPitch {
                    clip: tail,
                    cents: 211
                },
                Some(&split)
            )
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert_eq!(
        (session.can_undo(), session.can_redo()),
        (can_undo, can_redo)
    );
    assert!(Arc::ptr_eq(
        &root,
        &session.audio_clip_waveforms.borrow()[&tail].waveform
    ));
    let mut renderer = hex_engine::Renderer::new(split.arrangement());
    renderer.play();
    let mut actual = [[0.0; 2]; 31];
    renderer.render(&mut actual).unwrap();
    assert!(actual.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn long_dense_clip_edge_stretch_commits_and_renders_late_attacks_through_history() {
    const RATE: u32 = 44_100;
    const SOURCE_FRAMES: u64 = RATE as u64 * 27;
    const STRETCHED_END: u64 = 1_209_600;
    const ATTACK: [f32; 2] = [0.9, -0.45];
    let mut source = vec![[0.0; 2]; SOURCE_FRAMES as usize];
    for frame in source.iter_mut().skip(1_000).step_by(1_000) {
        *frame = ATTACK;
    }
    let audio = Audio::new(RATE, source).unwrap();
    let waveform = waveform::summarize(&audio, 1_024).unwrap();
    let mut session = Session::new(RATE, 32, SOURCE_FRAMES + 192).unwrap();
    let (Created::Imported { clip, .. }, previous) = session
        .import_prepared_revision(
            PreparedImport {
                path: "long-dense.wav".into(),
                source_sample_rate: RATE,
                source_frames: SOURCE_FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
                source_audio: audio.clone(),
                audio,
                waveform,
                file_identity: None,
            },
            None,
        )
        .unwrap()
    else {
        panic!("import")
    };
    let before = session.project().snapshot();
    let (_, stretched) = session
        .command_prepared_revision(
            Command::StretchAudioClipEdge {
                clip,
                edge: hex_project::TrimEdge::Right,
                frame: STRETCHED_END,
            },
            Some(&previous),
        )
        .unwrap();
    let after = session.project().snapshot();
    assert_eq!(
        session.project().clip(clip).unwrap().duration(),
        STRETCHED_END
    );
    assert_eq!(
        session.audio_clip_waveforms.borrow()[&clip].length,
        STRETCHED_END
    );

    let render_late_attack = |revision: &hex_engine::PreparedRevision| {
        let expected = 1_150_000 * STRETCHED_END / SOURCE_FRAMES;
        let mut renderer = hex_engine::Renderer::new(revision.arrangement());
        renderer.seek(expected - 64);
        renderer.play();
        let mut pcm = [[0.0; 2]; 128];
        renderer.render(&mut pcm).unwrap();
        assert!(
            pcm[60..68].contains(&ATTACK),
            "the late attack must survive the stretch"
        );
        pcm
    };
    let pcm = render_late_attack(&stretched);
    let undone = session
        .undo_prepared_revision(Some(&stretched))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot(), before);
    let redone = session
        .redo_prepared_revision(Some(&undone))
        .unwrap()
        .unwrap();
    assert_eq!(session.project().snapshot(), after);
    assert_eq!(render_late_attack(&redone), pcm);
}

#[test]
fn excessive_stretch_is_refused_before_project_or_undo_commit() {
    let (mut session, clip, previous) = fixture();
    let before = session.project().snapshot();
    let history = (session.can_undo(), session.can_redo());
    let result = session.command_prepared_revision(
        Command::StretchAudioClipEdge {
            clip,
            edge: hex_project::TrimEdge::Right,
            frame: 80_000,
        },
        Some(&previous),
    );
    assert!(
        result.is_err(),
        "tenfold output exceeds the existing warp ratio limit"
    );
    assert_eq!(session.project().snapshot(), before);
    assert_eq!((session.can_undo(), session.can_redo()), history);
}

#[test]
fn grouped_repeat_transaction_prepares_complete_multi_source_waveform_root() {
    let (mut session, first, previous) = fixture();
    let track = session.project().tracks()[0].id();
    let audio = Audio::new(16_000, vec![[0.25, -0.5]; 128]).unwrap();
    let imported = PreparedImport {
        path: "second.wav".into(),
        source_sample_rate: 16_000,
        source_frames: 128,
        conversion_version: hex_project::IDENTITY_CONVERSION,
        source_audio: audio.clone(),
        waveform: waveform::summarize(&audio, 128).unwrap(),
        audio,
        file_identity: None,
    };
    let (Created::Imported { clip: second, .. }, previous) = session
        .import_prepared_revision(imported, Some(&previous))
        .unwrap()
    else {
        panic!("second import")
    };
    let (_, previous) = session
        .command_prepared_revision(
            Command::MoveClip {
                clip: second,
                track,
                start: 8_064,
            },
            Some(&previous),
        )
        .unwrap();
    let (Created::Clip(group), previous) = session
        .command_prepared_revision(Command::GroupClips(vec![first, second]), Some(&previous))
        .unwrap()
    else {
        panic!("group")
    };
    let (_, repeated) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip: group,
                operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 10_000,
                },
            },
            Some(&previous),
        )
        .unwrap();
    let (_, cropped) = session
        .command_prepared_revision(
            Command::ResizeAudioClipView {
                clip: group,
                operation: hex_project::AudioViewResizeOperation::CycleEnd {
                    project_frame: 4_000,
                },
            },
            Some(&repeated),
        )
        .unwrap();
    let views = session.audio_clip_waveforms.borrow();
    let root = views[&group].preview_root.as_ref().unwrap();
    assert_eq!(root.waveform.source_frames(), 8_192);
    assert_eq!(
        root.waveform
            .peaks_for_frames(8_000, 8_064)
            .unwrap()
            .map(|peak| (peak.min, peak.max)),
        [(0.0, 0.0); 2]
    );
    assert_eq!(
        root.waveform
            .peaks_for_frames(8_064, 8_192)
            .unwrap()
            .map(|peak| (peak.min, peak.max)),
        [(0.25, 0.25), (-0.5, -0.5)]
    );
    assert_eq!(views[&group].length, 10_000);
    // The looped clip still exposes outer-clip peaks, built from the shared
    // root rather than a materialized repeat, and distinct from its root preview.
    assert_eq!(views[&group].offset, 0);
    assert_eq!(views[&group].waveform.source_frames(), 10_000);
    assert!(!Arc::ptr_eq(&views[&group].waveform, &root.waveform));
    let root_waveform = Arc::clone(&root.waveform);
    drop(views);
    let mut renderer = hex_engine::Renderer::new(cropped.arrangement());
    renderer.play();
    let mut output = [[0.0; 2]; 257];
    renderer.render(&mut output).unwrap();
    assert!(output.iter().any(|frame| *frame != [0.0; 2]));

    let (_, muted) = session
        .command_prepared_revision(
            Command::EditClips {
                clips: vec![group],
                edit: hex_project::ClipEdit::Active { active: false },
            },
            Some(&cropped),
        )
        .unwrap();
    assert!(Arc::ptr_eq(
        &session.audio_clip_waveforms.borrow()[&group]
            .preview_root
            .as_ref()
            .unwrap()
            .waveform,
        &root_waveform,
    ));
    let mut renderer = hex_engine::Renderer::new(muted.arrangement());
    renderer.play();
    let mut samples = [[1.0; 2]; 257];
    renderer.render(&mut samples).unwrap();
    assert_eq!(samples, [[0.0; 2]; 257]);

    let restored = session
        .undo_prepared_revision(Some(&muted))
        .unwrap()
        .unwrap();
    let mut renderer = hex_engine::Renderer::new(restored.arrangement());
    renderer.play();
    renderer.render(&mut samples).unwrap();
    assert_eq!(samples, output);
}

#[test]
fn directed_clip_previews_share_the_total_waveform_leaf_budget() {
    let (session, first, _) = fixture();
    let source = session.project().clip(first).unwrap();
    let track = session.project().tracks()[0].id();
    let asset = source.asset();
    let mut editor = Editor::new(session.project().clone(), 1).unwrap();
    let mut ids = vec![first];
    // Every clip has distinct processed PCM and a separate full-root preview.
    // 129 pairs exceed the cap if only output summaries are counted.
    for index in 1..129 {
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: index * 8_000,
                duration: 8_000,
                source_offset: 0,
                source_length: 8_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        ids.push(id);
    }
    for (index, id) in ids.iter().enumerate() {
        editor
            .execute(Command::SetAudioClipGain {
                clip: *id,
                millidb: -(index as i32),
            })
            .unwrap();
        editor
            .execute(Command::SetClipFades {
                clip: *id,
                fade_in: 2 + index as u64,
                fade_out: 0,
            })
            .unwrap();
    }
    editor.execute(Command::ReverseClips(ids)).unwrap();
    let retained = std::cell::RefCell::new(BTreeMap::new());
    prepare_with_waveforms(
        editor.project(),
        &session.media_bank(),
        session.sine_table(),
        None,
        &session.audio_transforms,
        &retained,
    )
    .unwrap();
    let views = retained.borrow();
    assert_eq!(views.len(), 129);
    let total: usize = views
        .values()
        .map(|view| {
            view.waveform.bins().len() + view.preview_root.as_ref().unwrap().waveform.bins().len()
        })
        .sum();
    assert!(
        total <= 262_144,
        "directed summaries exceeded leaf budget: {total}"
    );
}

#[test]
fn repeated_cycle_pyramids_share_the_waveform_cache_budget() {
    let (session, first, _) = fixture();
    let source = session.project().clip(first).unwrap();
    let track = session.project().tracks()[0].id();
    let asset = source.asset();
    let mut editor = Editor::new(session.project().clone(), 1).unwrap();
    let mut ids = vec![first];
    for index in 1..129 {
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: index * 16_000,
                duration: 8_000,
                source_offset: 0,
                source_length: 8_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        ids.push(id);
    }
    for id in ids {
        let end = editor.project().clip(id).unwrap().start() + 16_000;
        editor
            .execute(Command::ResizeAudioClipView {
                clip: id,
                operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: end,
                },
            })
            .unwrap();
    }
    let retained = std::cell::RefCell::new(BTreeMap::new());
    prepare_with_waveforms(
        editor.project(),
        &session.media_bank(),
        session.sine_table(),
        None,
        &session.audio_transforms,
        &retained,
    )
    .unwrap();
    let views = retained.borrow();
    assert_eq!(views.len(), 129);
    let bins: usize = views
        .values()
        .map(|view| {
            assert!(view.waveform.is_repeated());
            view.waveform.cached_leaf_count()
                + view
                    .preview_root
                    .as_ref()
                    .unwrap()
                    .waveform
                    .cached_leaf_count()
        })
        .sum();
    assert!(
        bins <= 262_144,
        "repeated waveform pyramids exceeded their shared budget: {bins}"
    );
}

#[test]
fn time_range_group_prepares_identical_audio_and_roundtrips_complete_state() {
    for (start, end) in [(500, 4_500), (1_000, 11_000), (500, 11_000)] {
        let (mut session, first, previous) = fixture();
        let (_, previous) = session
            .command_prepared_revision(
                Command::MoveClip {
                    track: session.project().tracks()[0].id(),
                    clip: first,
                    start: 1_000,
                },
                Some(&previous),
            )
            .unwrap();
        let (_, previous) = session
            .command_prepared_revision(
                Command::SplitClip {
                    clip: first,
                    frame: 5_000,
                },
                Some(&previous),
            )
            .unwrap();
        let before = session.project().clone();
        let track = before.tracks()[0].id();
        let render = |revision: &hex_engine::PreparedRevision, block| {
            let mut renderer = hex_engine::Renderer::new(revision.arrangement());
            renderer.play();
            let mut out = vec![[0.0; 2]; 12_000];
            for chunk in out.chunks_mut(block) {
                renderer.render(chunk).unwrap();
            }
            out
        };
        let expected = render(&previous, 63);
        let (Created::Clip(group), changed) = session
            .command_prepared_revision(
                Command::GroupTimeRange(
                    hex_project::TimeRange::new(vec![track], start, end).unwrap(),
                ),
                Some(&previous),
            )
            .unwrap()
        else {
            panic!("group")
        };
        assert_eq!(
            (
                session.project().clip(group).unwrap().start(),
                session.project().clip(group).unwrap().duration()
            ),
            (start, end - start)
        );
        assert_eq!(render(&changed, 257), expected);
        let after = session.project().clone();
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&after).unwrap()).unwrap(),
            after
        );
        let undone = session
            .undo_prepared_revision(Some(&changed))
            .unwrap()
            .unwrap();
        let restored = session.project().snapshot();
        let mut expected_before = before.snapshot();
        expected_before.next_id = restored.next_id;
        assert_eq!(restored, expected_before);
        assert_eq!(render(&undone, 63), expected);
        let redone = session
            .redo_prepared_revision(Some(&undone))
            .unwrap()
            .unwrap();
        assert_eq!(session.project(), &after);
        assert_eq!(render(&redone, 127), expected);
    }
}

#[test]
fn clip_loop_cycle_range_repeats_retained_pcm_without_changing_placement_or_history_custody() {
    use hex_project::{AudioViewMode, AudioViewResizeOperation};
    fn render(revision: &hex_engine::PreparedRevision) -> Vec<hex_engine::Frame> {
        let mut r = hex_engine::Renderer::new(revision.arrangement());
        r.play();
        let mut frames = vec![[0.0; 2]; 8000];
        for block in frames.chunks_mut(113) {
            r.render(block).unwrap();
        }
        frames
    }
    for (musical, pitched) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut session, clip, mut previous) = fixture();
        if musical {
            previous = session
                .command_prepared_revision(
                    Command::SetAudioClipMusical {
                        clip,
                        enabled: true,
                    },
                    Some(&previous),
                )
                .unwrap()
                .1;
        }
        if pitched {
            session.max_resident_frames = 100_000;
            previous = session
                .command_prepared_revision(
                    Command::SetAudioClipPitch { clip, cents: 700 },
                    Some(&previous),
                )
                .unwrap()
                .1;
        }
        let original = session.project().snapshot();
        let original_clip = session.project().clip(clip).unwrap().clone();
        let root = original_clip.audio_event_root();
        let expected = render(&previous);
        let first = root.domain.clock.length() / 4;
        let last = first * 3;
        let (_, bounced) = session
            .command_prepared_revision(
                Command::ResizeAudioClipView {
                    clip,
                    operation: AudioViewResizeOperation::CycleRange {
                        start: first,
                        end: last,
                        looping: true,
                    },
                },
                Some(&previous),
            )
            .unwrap();
        let c = session.project().clip(clip).unwrap();
        let edited = c.audio_event_root();
        assert_eq!(
            (c.start(), c.duration()),
            (original_clip.start(), original_clip.duration())
        );
        assert_eq!(edited.domain, root.domain);
        assert_eq!((edited.view_start, edited.view_end), (first, last));
        assert_eq!(edited.placement.unwrap().mode, AudioViewMode::Repeat);
        let actual = render(&bounced);
        for (i, frame) in actual.iter().enumerate() {
            assert_eq!(*frame, expected[2000 + i % 4000], "{musical} frame {i}");
        }
        let saved = session.project().snapshot();
        let bytes = hex_storage::encode(session.project()).unwrap();
        assert_eq!(hex_storage::decode(&bytes).unwrap().snapshot(), saved);
        let resident = session.resident_frames;
        for (start, end) in [(first, first), (0, u64::MAX)] {
            assert!(
                session
                    .command_prepared_revision(
                        Command::ResizeAudioClipView {
                            clip,
                            operation: AudioViewResizeOperation::CycleRange {
                                start,
                                end,
                                looping: true
                            },
                        },
                        Some(&bounced)
                    )
                    .is_err()
            );
            assert_eq!(session.project().snapshot(), saved);
            assert_eq!(session.resident_frames, resident);
        }
        let undo = session
            .undo_prepared_revision(Some(&bounced))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().snapshot().tracks, original.tracks);
        assert_eq!(render(&undo), expected);
        let redo = session
            .redo_prepared_revision(Some(&undo))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().snapshot().tracks, saved.tracks);
        assert_eq!(render(&redo), actual);
    }
}

#[test]
fn accepted_sparse_root_peaks_keep_exact_samples_and_silence_at_distant_edges() {
    use hex_project::AudioEventEdit;
    let (mut session, clip, mut previous) = fixture();
    for operation in [
        AudioEventEdit::Split { at: 1_000 },
        AudioEventEdit::Place {
            event: 1,
            start: 4_000_000,
        },
    ] {
        previous = session
            .command_prepared_revision(
                Command::EditAudioEvents { clip, operation },
                Some(&previous),
            )
            .unwrap()
            .1;
    }
    let prepared = &previous.arrangement().audio_clip_views()[0];
    assert!(prepared.sparse.is_some());
    let views = session.audio_clip_waveforms.borrow();
    let waveform = &views[&clip].waveform;
    assert_eq!(waveform.source_frames(), 4_001_000);
    assert_eq!(
        waveform.has_identical_channels(),
        prepared
            .audio
            .frames()
            .iter()
            .all(|frame| frame[0].to_bits() == frame[1].to_bits())
    );
    let exact = |a, b, budget| {
        waveform
            .bounded_exact_peaks_for_frames(a, b, budget)
            .unwrap()
    };
    let zero = [waveform::Peak { min: 0.0, max: 0.0 }; 2];
    assert_eq!(exact(20_000, 3_999_997, 0), zero);
    let sample = prepared.audio.frames()[7_000];
    assert_eq!(
        exact(4_000_000, 4_000_001, 1),
        sample.map(|s| waveform::Peak { min: s, max: s })
    );
    assert_eq!(
        exact(3_999_997, 4_000_001, 1),
        sample.map(|s| waveform::Peak {
            min: s.min(0.0),
            max: s.max(0.0)
        })
    );
    let mut visited = Vec::new();
    let over = waveform
        .bounded_overrange(3_999_997..4_000_001, 4, |frame| {
            visited.push(frame);
            Some((1.0, 100.0))
        })
        .unwrap();
    assert_eq!(visited, vec![3_999_997, 3_999_998, 3_999_999, 4_000_000]);
    for (channel, value) in sample.iter().copied().enumerate() {
        assert_eq!(over.positive[channel], value * 100.0 > 1.0);
        assert_eq!(over.negative[channel], value * 100.0 < -1.0);
    }
    let (offset, loud) = prepared.audio.frames()[7_000..]
        .iter()
        .enumerate()
        .find(|(_, frame)| frame[0].abs() > 0.02 && frame[1].abs() > 0.02)
        .expect("occupied sparse event has audible samples");
    let logical = 4_000_000 + offset;
    let loud_result = waveform
        .bounded_overrange(logical..logical + 1, 1, |_| Some((1.0, 100.0)))
        .unwrap();
    for (channel, value) in loud.iter().copied().enumerate() {
        assert_eq!(loud_result.positive[channel], value * 100.0 > 1.0);
        assert_eq!(loud_result.negative[channel], value * 100.0 < -1.0);
    }
    assert!(loud_result.positive.into_iter().any(|present| present));
    assert!(loud_result.negative.into_iter().any(|present| present));
    assert!(
        waveform
            .bounded_overrange(3_999_997..4_000_001, 3, |_| Some((1.0, 100.0)))
            .is_none()
    );
    assert!(
        waveform
            .bounded_overrange(3_999_997..4_000_001, 4, |frame| (frame != 3_999_998)
                .then_some((1.0, 1.0)))
            .is_none()
    );
}

#[test]
fn long_frame_events_stay_outside_the_gate_until_trimmed_and_reopen_at_original_pcm() {
    use hex_project::{AudioEventEdit, TrimEdge};
    const DISTANCE: u64 = hex_project::MAX_AUDIO_DOMAIN_FRAMES + 10_000;
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        let (mut session, clip, mut revision) = fixture();
        let track = session.project().tracks()[0].id();
        let placement = DISTANCE + 100;
        revision = session
            .command_prepared_revision(
                Command::MoveClip {
                    clip,
                    track,
                    start: placement,
                },
                Some(&revision),
            )
            .unwrap()
            .1;
        revision = session
            .command_prepared_revision(
                Command::EditAudioEvents {
                    clip,
                    operation: AudioEventEdit::Split { at: 1_000 },
                },
                Some(&revision),
            )
            .unwrap()
            .1;
        let before = session.project().clone();
        let signed = match edge {
            TrimEdge::Left => -(DISTANCE as i64),
            TrimEdge::Right => DISTANCE as i64,
        };
        revision = session
            .command_prepared_revision(
                Command::EditAudioEvents {
                    clip,
                    operation: AudioEventEdit::Place {
                        event: 1,
                        start: signed,
                    },
                },
                Some(&revision),
            )
            .unwrap()
            .1;
        let moved = session.project().clone();
        let c = moved.clip(clip).unwrap();
        assert_eq!((c.start(), c.duration()), (placement, 8_000));
        let view = &revision.arrangement().audio_clip_views()[0];
        assert!(view.sparse.is_some());
        assert_eq!(view.audio.pcm_capacity_frames(), 8_000);
        let destination = placement.checked_add_signed(signed).unwrap();
        let read = |revision: &hex_engine::PreparedRevision, position| {
            let mut renderer = hex_engine::Renderer::new(revision.arrangement());
            renderer.seek(position);
            renderer.play();
            let mut output = [[0.0; 2]; 137];
            for block in output.chunks_mut(31) {
                renderer.render(block).unwrap();
            }
            output
        };
        assert_eq!(read(&revision, destination), [[0.0; 2]; 137]);

        revision = session
            .undo_prepared_revision(Some(&revision))
            .unwrap()
            .unwrap();
        assert_eq!(session.project(), &before);
        revision = session
            .redo_prepared_revision(Some(&revision))
            .unwrap()
            .unwrap();
        assert_eq!(session.project(), &moved);
        let trim_frame = if edge == TrimEdge::Left {
            destination
        } else {
            destination + 1_000
        };
        revision = session
            .command_prepared_revision(
                Command::TrimClipEdge {
                    clip,
                    edge,
                    frame: trim_frame,
                },
                Some(&revision),
            )
            .unwrap()
            .1;
        let media = session.media_bank();
        let asset = c.asset();
        let original = &media.resident[&asset];
        assert_eq!(read(&revision, destination), original.frames()[..137]);
        let saved = hex_storage::encode(session.project()).unwrap();
        assert_eq!(
            hex_storage::schema_version_for(session.project()),
            hex_storage::EXTENDED_FRAME_EVENTS_SCHEMA_VERSION
        );
        let opened = hex_storage::decode(&saved).unwrap();
        assert_eq!(&opened, session.project());
        let reopened = hex_engine::PreparedRevision::prepare(
            &opened,
            &media.resident,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap();
        assert_eq!(read(&reopened, destination), original.frames()[..137]);

        // Deactivating all events must not turn a long silent root into dense PCM.
        let (_, silent) = session
            .command_prepared_revision(
                Command::EditAudioEvents {
                    clip,
                    operation: AudioEventEdit::SetActive {
                        events: vec![1, 2],
                        active: false,
                    },
                },
                Some(&revision),
            )
            .unwrap();
        assert_eq!(
            silent.arrangement().audio_clip_views()[0]
                .audio
                .frames()
                .len(),
            1
        );
        assert_eq!(read(&silent, destination), [[0.0; 2]; 137]);
    }
}
