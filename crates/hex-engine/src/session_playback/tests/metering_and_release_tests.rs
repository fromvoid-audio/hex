use super::*;

#[test]
fn instrument_metering_preserves_pcm_and_host_trim_keeps_live_note_identity() {
    for input in [true, false] {
        let project = project(0.758_499_15, 0, 8 << 32);
        let table = Arc::new(hex_devices::SineTable::new());
        let prepared = revision(&project, Arc::clone(&table));
        let device = project.tracks()[0].instrument().unwrap().id();
        let meter = Arc::clone(prepared.arrangement().device_meter(device.value()).unwrap());
        let mut ordinary = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        let track = project.tracks()[0].id();
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
        let mut metered = SessionPlayback::new(prepared).unwrap();
        metered.set_device_metering(true);
        ordinary.apply(SessionPlaybackCommand::Play).unwrap();
        metered.apply(SessionPlaybackCommand::Play).unwrap();
        let mut a = [[0.0; 2]; 127];
        let mut b = a;
        let mut output_peak = 0.0_f32;
        for _ in 0..5 {
            ordinary.render(&mut a).unwrap();
            metered.render(&mut b).unwrap();
            assert_eq!(a, b, "telemetry must preserve per-voice summation order");
            output_peak = b
                .iter()
                .flatten()
                .fold(output_peak, |peak, sample| peak.max(sample.abs()));
        }
        let track_meter = observation.track_output_meter(track).unwrap();
        assert_eq!(track_meter.take_track_output(), output_peak);
        assert_eq!(track_meter.take_track_output(), 0.0);
        let peaks = meter.take_peaks();
        assert_eq!(peaks.input, 0.0);
        assert!(peaks.output.iter().any(|peak| *peak > 0.0));
        let mut snapshot = project.snapshot();
        let instrument = snapshot.tracks[0].instrument.as_mut().unwrap();
        instrument.state = if input {
            instrument.state.clone().with_input_gain_db(-12.0).unwrap()
        } else {
            instrument.state.clone().with_output_gain_db(-12.0).unwrap()
        };
        let changed = Project::from_legacy_snapshot(snapshot).unwrap();
        let prepared = Arc::new(
            PreparedRevision::prepare(
                &changed,
                &BTreeMap::new(),
                table,
                Some(ordinary.revision.session()),
            )
            .unwrap(),
        );
        let mut trimmed = SessionPlayback::new(prepared).unwrap();
        trimmed.try_inherit_state_from(&mut ordinary).unwrap();
        assert!(
            trimmed.releases.is_empty(),
            "host trim must not create a duplicate voice release"
        );
        ordinary.render(&mut a).unwrap();
        trimmed.render(&mut b).unwrap();
        let gain = if input {
            1.0
        } else {
            10.0_f32.powf(-12.0 / 20.0)
        };
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            assert!((a * gain - b).abs() < 1e-7);
        }
    }
}

#[test]
fn meter_capture_combines_each_owner_before_advancing_and_discards_refused_blocks() {
    use super::capture::{CaptureSink, OutputCaptureId};
    use super::meter_capture::{Cell, MeterCapture};
    use crate::signal_graph::Owner;

    let mut editor = Editor::new(project(1.0, 0, 8 << 32), 8).unwrap();
    let first = editor.project().tracks()[0].id();
    let Created::Track(second) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("fixture track");
    };
    let prepared = revision(editor.project(), Arc::new(hex_devices::SineTable::new()));
    let observation = crate::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
    let mut cells = vec![
        Cell::new(OutputCaptureId::Track(first)),
        Cell::new(OutputCaptureId::Track(second)),
    ];
    super::meter_capture::sort(&mut cells);
    let mut capture = MeterCapture::new(&prepared, &mut cells, None);
    // Owners can be interleaved; two contributions to the first owner at the
    // same offset cancel before the peak is measured.
    capture
        .add_prepared_frame(
            Owner::Track(0),
            OutputCaptureId::Track(first),
            0,
            [0.0, 0.5],
        )
        .unwrap();
    // A stale slot hint must still select the canonical owner.
    capture
        .add_prepared_frame(
            Owner::Track(0),
            OutputCaptureId::Track(second),
            1,
            [f32::NAN, -0.9],
        )
        .unwrap();
    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.0, -0.25]])
        .unwrap();
    capture
        .add(OutputCaptureId::Track(first), 1, &[[0.1, 0.0]])
        .unwrap();
    capture.commit(2);
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output_stereo(),
        [0.1, 0.25]
    );
    assert_eq!(
        observation
            .track_output_meter(second)
            .unwrap()
            .take_track_output_stereo(),
        [0.0, 0.9]
    );

    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.3, 0.0]])
        .unwrap();
    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.3, 0.0], [0.7, 0.0]])
        .unwrap();
    capture.commit(2);
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output_stereo(),
        [0.7, 0.0]
    );

    capture
        .add(OutputCaptureId::Track(first), 5, &[[0.8, 0.0]])
        .unwrap();
    // Out-of-order telemetry is discarded without interrupting audio.
    capture
        .add(OutputCaptureId::Track(first), 4, &[[0.1, 0.0]])
        .unwrap();
    capture.commit(2);
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output_stereo(),
        [0.0; 2]
    );
    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.7, 0.0]])
        .unwrap();
    capture.invalidate();
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output_stereo(),
        [0.0; 2]
    );
}

#[test]
fn track_metering_preserves_multiple_track_audio() {
    let original = project(0.758_499_15, 0, 8 << 32);
    let mut snapshot = original.snapshot();
    let clips = match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips) => clips,
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        _ => panic!("fixture must contain MIDI"),
    };
    {
        let mut note = clips[0].notes[0].clone();
        note.id = 24;
        note.pitch = 67;
        clips[0].notes.push(note);
    }
    let mut second = snapshot.tracks[0].clone();
    second.id = 20;
    second.gain = 0.431;
    second.instrument.as_mut().unwrap().id = 21;
    let clips = match &mut second.content {
        TrackContentSnapshot::Instrument(clips) => clips,
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        _ => panic!("fixture must contain MIDI"),
    };
    {
        clips[0].id = 22;
        clips[0].notes[0].id = 23;
        clips[0].notes[1].id = 25;
    }
    snapshot.next_id = 26;
    snapshot.tracks.push(second);
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
    let prepared = revision(&project, Arc::new(hex_devices::SineTable::new()));
    let observation = crate::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
    let mut ordinary = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut metered = SessionPlayback::new(prepared).unwrap();
    metered.set_device_metering(true);
    ordinary.apply(SessionPlaybackCommand::Play).unwrap();
    metered.apply(SessionPlaybackCommand::Play).unwrap();
    let mut a = [[0.0; 2]; 127];
    let mut b = a;
    for _ in 0..5 {
        ordinary.render(&mut a).unwrap();
        metered.render(&mut b).unwrap();
        // The pre-existing instrument-meter path groups per-track sums, so
        // metered and unmetered renderers can differ by f32 rounding.
        for (actual, reference) in b.iter().flatten().zip(a.iter().flatten()) {
            assert!((actual - reference).abs() < 1e-7);
        }
    }
    for track in project.tracks() {
        assert!(
            observation
                .track_output_meter(track.id())
                .unwrap()
                .take_track_output()
                > 0.0
        );
    }
}

#[test]
fn closed_candidate_main_checks_prepared_and_actual_runtime_before_any_groups() {
    use hex_lattice_contract::NativeDeviceMetadata;
    fn runtime(with_main: bool) -> SessionPlayback {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        if with_main {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
        }
        let revision = PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap();
        SessionPlayback::new(Arc::new(revision)).unwrap()
    }
    let mut empty = runtime(false);
    let mut main = runtime(true);
    let mut locations = std::collections::BTreeSet::new();
    empty
        .visit_candidate_groups(&mut |kind, index, _| {
            assert!(locations.insert((kind, index)), "duplicate owning location");
            Ok(())
        })
        .unwrap();
    assert_eq!(
        locations.len() + 5,
        candidate_group_reservation(0, 0).unwrap()
    );
    // Both prepared and runtime Main are real populated chains, not placeholders.
    assert!(!main.revision.arrangement().main_device_chain.is_empty());
    assert!(main.main_chain.storage_view().entries().next().is_some());
    // Fresh matched Main storage is now supported; owning handoff remains
    // separately refused until its origin transfer is represented.
    main.visit_candidate_groups(&mut |_, _, _| Ok(())).unwrap();
    let refuses_without_emitting = |runtime: &SessionPlayback| {
        let mut emitted = 0;
        assert_eq!(
            runtime.visit_candidate_groups(&mut |_, _, _| {
                emitted += 1;
                Ok(())
            }),
            Err(crate::AudioStorageError::UnsupportedGraph)
        );
        assert_eq!(emitted, 0);
    };
    std::mem::swap(&mut empty.main_chain, &mut main.main_chain);
    refuses_without_emitting(&empty); // runtime-only population
    refuses_without_emitting(&main); // prepared-only population
}

#[test]
fn analytic_clip_volume_survives_track_stop_and_removed_track_handoffs() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    for session in [false, true] {
        for removed in [false, true] {
            let run = |volume: bool| {
                let mut editor = Editor::new(project(0.5, 0, 8 << 32), 8).unwrap();
                let track = editor.project().tracks()[0].id();
                let clip = if session {
                    revision(editor.project(), Arc::new(hex_devices::SineTable::new()))
                        .session()
                        .midi_clips()[0]
                        .id
                } else {
                    editor.project().track(track).unwrap().midi_clips()[0].id()
                };
                if volume {
                    let curve = MidiClipVolume::new(
                        vec![MidiClipVolumePoint::linear(
                            BeatTime::ZERO,
                            MidiClipGain::new(0.25).unwrap(),
                        )],
                        editor.project().midi_source(clip).unwrap().source_length(),
                    )
                    .unwrap();
                    editor
                        .execute(Command::ReplaceMidiClipVolume {
                            clip,
                            volume: curve,
                        })
                        .unwrap();
                }
                let table = Arc::new(hex_devices::SineTable::new());
                let mut playback =
                    SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
                if session {
                    playback
                        .apply(SessionPlaybackCommand::LaunchClip {
                            clip,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                playback.apply(SessionPlaybackCommand::Play).unwrap();
                playback.render(&mut [[0.0; 2]; 64]).unwrap();
                if removed {
                    editor.execute(Command::RemoveTrack(track)).unwrap();
                    let mut next = SessionPlayback::new(revision(editor.project(), table)).unwrap();
                    next.try_inherit_state_from(&mut playback).unwrap();
                    playback = next;
                } else {
                    playback
                        .apply(SessionPlaybackCommand::StopTrack {
                            track,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                let mut tail = [[0.0; 2]; 64];
                playback.render(&mut tail).unwrap();
                tail
            };
            let baseline = run(false);
            let actual = run(true);
            assert!(baseline.iter().any(|v| v[0].abs() > 1e-5));
            for (a, b) in actual.iter().zip(baseline) {
                for c in 0..2 {
                    assert!(
                        (a[c] - b[c] * 0.25).abs() < 1e-7,
                        "session={session} removed={removed}: {a:?} vs {b:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn session_audio_release_handoff_never_renders_a_replaced_source() {
    let project = audio_project(0, 32, 4);
    let original = crate::Audio::new(48_000, vec![[0.5; 2]; 32]).unwrap();
    let replacement = crate::Audio::new(48_000, vec![[-0.5; 2]; 32]).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let prepare = |project: &Project, audio: &crate::Audio| {
        Arc::new(
            PreparedRevision::prepare(
                project,
                &audio_media(project, audio, audio),
                Arc::clone(&table),
                None,
            )
            .unwrap(),
        )
    };
    let original_revision = prepare(&project, &original);
    let track = project.tracks()[0].id();
    let clip = original_revision.session().audio_clips()[0].id;
    let changed_region = audio_project(1, 32, 4);
    let changed_asset = audio_project(0, 32, 5);
    let mut snapshot = project.snapshot();
    snapshot.session.cells.clear();
    let removed = Project::from_snapshot(snapshot).unwrap();
    for (candidate, expected_release) in [
        (prepare(&project, &original), true),
        (prepare(&project, &replacement), false),
        (prepare(&changed_region, &original), false),
        (prepare(&changed_asset, &original), false),
        (prepare(&removed, &original), false),
    ] {
        let mut old = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
        old.apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 8]).unwrap();
        old.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
        let mut first = [[0.0; 2]; 1];
        old.render(&mut first).unwrap();
        assert!(first[0][0] > 0.0);
        let mut next = SessionPlayback::new(candidate).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut after = [[0.0; 2]; 1];
        next.render(&mut after).unwrap();
        if expected_release {
            assert!(after[0][0] > 0.0, "unchanged source retains its release");
        } else {
            assert_eq!(
                after,
                [[0.0; 2]],
                "a replacement must not sound as the old release"
            );
        }
    }
}

#[test]
fn streamed_session_release_handoff_demands_only_the_unchanged_source() {
    struct Pages {
        frames: [[f32; 2]; 32],
    }
    impl MediaPages for Pages {
        fn page(
            &mut self,
            _key: StreamSourceKey,
            page_index: u64,
        ) -> Option<BorrowedMediaPage<'_>> {
            (page_index == 0).then_some(BorrowedMediaPage {
                page_index,
                frames: &self.frames,
            })
        }
        fn request_page(&mut self, _key: StreamSourceKey, _page_index: u64) {}
    }
    let project = audio_project(0, 32, 4);
    let prepare = |generation| {
        let sources = project
            .assets()
            .map(|asset| {
                let metadata = StreamedSourceMetadata::new(
                    StreamSourceKey::new(generation, asset.id()),
                    48_000,
                    48_000,
                    32,
                    32,
                    hex_project::IDENTITY_CONVERSION,
                    [0x42; 32],
                )
                .unwrap();
                (
                    asset.id(),
                    PreparedAudioSource::Streamed(Arc::new(metadata)),
                )
            })
            .collect();
        Arc::new(
            PreparedRevision::prepare_with_prepared_audio_sources(
                &project,
                &sources,
                generation,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    };
    let revision = prepare(1);
    let track = project.tracks()[0].id();
    let clip = revision.session().audio_clips()[0].id;
    for (generation, keeps_release) in [(1, true), (2, false)] {
        let mut pages = Pages {
            frames: [[0.5; 2]; 32],
        };
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        old.apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render_with_pages(&mut [[0.0; 2]; 8], &mut pages)
            .unwrap();
        old.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
        old.render_with_pages(&mut [[0.0; 2]; 1], &mut pages)
            .unwrap();
        let mut next = SessionPlayback::new(prepare(generation)).unwrap();
        let mut extents = Vec::new();
        next.preview_stream_handoff(&mut old, 1, None, &mut |extent| {
            extents.push(extent);
            Ok(())
        })
        .unwrap();
        assert_eq!(!extents.is_empty(), keeps_release);
        next.try_inherit_state_from(&mut old).unwrap();
        let mut after = [[0.0; 2]; 1];
        next.render_with_pages(&mut after, &mut pages).unwrap();
        assert_eq!(after[0][0] > 0.0, keeps_release);
    }
}

#[test]
fn console_audio_loop_intro_repeats_across_partitions_handoff_and_stop() {
    let source: Vec<Frame> = (0..32)
        .map(|i| [i as f32 / 64.0, -(i as f32) / 64.0])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    for (first, last) in [(7, 13), (7, 8)] {
        let mut snapshot = audio_project(3, 29, 4).snapshot();
        let SessionCellContentSnapshot::Audio(clip) = &mut snapshot.session.cells[0].content else {
            panic!("audio")
        };
        clip.loop_range = Some(hex_project::SessionAudioLoop::new(first, last).unwrap());
        let project = Project::from_snapshot(snapshot).unwrap();
        let media = audio_media(&project, &audio, &audio);
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let clip = revision.session().audio_clips()[0].id;
        let expected: Vec<_> = (0..97)
            .map(|i| {
                source[if i < last - 3 {
                    3 + i
                } else {
                    first + (i - (last - 3)) % (last - first)
                } as usize]
            })
            .collect();
        for block in [1, 7, 32, 97] {
            let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = vec![[0.0; 2]; 97];
            for chunk in actual.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            assert_eq!(actual, expected, "cycle {first}..{last}, block {block}");
            let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            candidate.try_inherit_state_from(&mut playback).unwrap();
            let mut next = [[0.0; 2]; 3];
            candidate.render(&mut next).unwrap();
            for (j, frame) in next.iter().enumerate() {
                assert_eq!(
                    *frame,
                    source[(first + (97 + j as u64 - (last - 3)) % (last - first)) as usize]
                );
            }
            candidate.apply(SessionPlaybackCommand::Stop).unwrap();
            let mut stopped = [[1.0; 2]; 3];
            candidate.render(&mut stopped).unwrap();
            assert_eq!(stopped, [[0.0; 2]; 3]);
        }
    }
}

#[test]
fn mixed_streamed_resident_and_midi_compensation_preserves_page_readiness_and_storage() {
    let track_latency = mixed_streamed_resident_and_midi_compensation_case(None);
    let silent_return = mixed_streamed_resident_and_midi_compensation_case(Some(0.0));
    let sounding_return = mixed_streamed_resident_and_midi_compensation_case(Some(1.0));
    assert_eq!(sounding_return.1, silent_return.1);
    assert!(
        sounding_return.1 > track_latency.1,
        "the return must add latency"
    );
    assert_ne!(
        sounding_return.0, silent_return.0,
        "the return send must change rendered PCM"
    );
}
